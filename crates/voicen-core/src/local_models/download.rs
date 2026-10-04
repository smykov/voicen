//! The one model downloader (spec 002 contracts/core-traits.md "Downloader",
//! research R-2, R-9; T-016 design points 1-6; decision #49).
//!
//! Synchronous (decisions #22, #42): [`Downloader::start`] runs a blocking reqwest
//! GET on its own std thread and reports [`DownloadEvent`]s through a callback.
//! The no-data timeout is reqwest's per-read timeout
//! (`ClientBuilder::timeout(Timeouts::download_no_data)` + `connect_timeout`), never
//! a total request timeout: a slow but steady download longer than
//! `download_no_data` must succeed. Bytes go to `<file>.part` and into a streamed
//! SHA-256; only a byte count equal to the catalog size and the pinned digest lead
//! to the rename to the final name. Every other end removes `<file>.part` before
//! the end event. At most one download runs at a time.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use sha2::{Digest, Sha256};

use super::catalog::{CatalogEntry, ModelId};
use super::store::ModelStore;
use crate::engine::openai::{body_error, host_port, send_error};
use crate::failure::TransportError;
use crate::i18n::{self, MessageId};
use crate::timeouts::Timeouts;

/// Free space of the volume holding a directory (R-9). Windows impl in the shell
/// (`GetDiskFreeSpaceExW`); tests use a fake.
pub trait DiskSpace: Send + Sync {
    fn available_bytes(&self, dir: &Path) -> io::Result<u64>;
}

/// Why a download failed (wire codes of contracts/ipc.md). Built from an HTTP
/// status, transport facts and the URL's `host[:port]` only: never carries the URL,
/// its query, a `reqwest::Error` or OS error text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadFailure {
    /// The body ended early, the connection dropped, or no data arrived for
    /// `Timeouts::download_no_data`.
    DownloadInterrupted,
    /// All bytes arrived but the SHA-256 is not the pinned one, or the body is
    /// longer than the catalog size (not the pinned file).
    ChecksumMismatch,
    /// Free space is below catalog size + 1 % (`needed` in bytes).
    NotEnoughDiskSpace { needed: u64 },
    /// Connect refused / timed out / TLS failed / DNS failed, or the HTTP client
    /// could not be built; `host[:port]` of the URL.
    SourceUnreachable { host: String },
    /// The `.part` file could not be created, written or renamed.
    DiskError,
    /// A non-2xx status.
    HttpStatus { code: u16 },
}

impl DownloadFailure {
    /// Wire code (contracts/ipc.md): `download_interrupted`, `checksum_mismatch`,
    /// `not_enough_disk_space`, `source_unreachable`, `disk_error`, `http_status`.
    pub fn code(&self) -> &'static str {
        match self {
            DownloadFailure::DownloadInterrupted => "download_interrupted",
            DownloadFailure::ChecksumMismatch => "checksum_mismatch",
            DownloadFailure::NotEnoughDiskSpace { .. } => "not_enough_disk_space",
            DownloadFailure::SourceUnreachable { .. } => "source_unreachable",
            DownloadFailure::DiskError => "disk_error",
            DownloadFailure::HttpStatus { .. } => "http_status",
        }
    }

    /// The catalog message (en/ru).
    pub fn message_id(&self) -> MessageId {
        match self {
            DownloadFailure::DownloadInterrupted => i18n::DOWNLOAD_INTERRUPTED,
            DownloadFailure::ChecksumMismatch => i18n::DOWNLOAD_CHECKSUM_MISMATCH,
            DownloadFailure::NotEnoughDiskSpace { .. } => i18n::DOWNLOAD_NOT_ENOUGH_DISK_SPACE,
            DownloadFailure::SourceUnreachable { .. } => i18n::DOWNLOAD_SOURCE_UNREACHABLE,
            DownloadFailure::DiskError => i18n::DOWNLOAD_DISK_ERROR,
            DownloadFailure::HttpStatus { .. } => i18n::DOWNLOAD_HTTP_STATUS,
        }
    }

    /// Placeholder values for [`message_id`](Self::message_id): `needed`, `host`,
    /// `code` for the variants that carry them, none otherwise.
    pub fn message_params(&self) -> Vec<(&'static str, String)> {
        match self {
            DownloadFailure::NotEnoughDiskSpace { needed } => vec![("needed", needed.to_string())],
            DownloadFailure::SourceUnreachable { host } => vec![("host", host.clone())],
            DownloadFailure::HttpStatus { code } => vec![("code", code.to_string())],
            DownloadFailure::DownloadInterrupted
            | DownloadFailure::ChecksumMismatch
            | DownloadFailure::DiskError => Vec::new(),
        }
    }

    /// The one mapping from transport facts (`engine::openai`'s reqwest adapter)
    /// to a download reason. Order: status; DNS, connect (refused, connect
    /// timeout, TLS), OS network/host unreachable or client setup ->
    /// `SourceUnreachable`; everything else (no response headers within
    /// `download_no_data`, a reset, an early close, a body stall) ->
    /// `DownloadInterrupted`.
    fn from_transport(err: &TransportError, host: &str) -> DownloadFailure {
        match *err {
            TransportError::Status(code) => DownloadFailure::HttpStatus { code },
            TransportError::Send {
                dns, connect, io, ..
            } if dns
                || connect
                || matches!(
                    io,
                    Some(io::ErrorKind::NetworkUnreachable | io::ErrorKind::HostUnreachable)
                ) =>
            {
                DownloadFailure::SourceUnreachable {
                    host: host.to_string(),
                }
            }
            TransportError::Setup => DownloadFailure::SourceUnreachable {
                host: host.to_string(),
            },
            TransportError::Send { .. }
            | TransportError::BodyRead { .. }
            | TransportError::BadBody
            | TransportError::UnusableKey => DownloadFailure::DownloadInterrupted,
        }
    }
}

/// What a running download reports. Exactly one of `Finished`, `Failed`,
/// `Cancelled` ends it, and only after `<file>.part` is gone (renamed or removed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadEvent {
    /// At least 1/s and at most 4/s while data arrives.
    Progress {
        id: ModelId,
        received: u64,
        total: u64,
    },
    /// The final file is in place with the catalog size and SHA-256.
    Finished { id: ModelId },
    /// Retry is offered (`start` again).
    Failed {
        id: ModelId,
        reason: DownloadFailure,
    },
    /// No retry reason.
    Cancelled { id: ModelId },
}

/// Why [`Downloader::start`] refused before any request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    /// A download is already running (any model).
    Busy,
    /// The model is already downloaded (`ModelStore` says so).
    AlreadyDownloaded,
    /// The disk probe reported less than catalog size + 1 %.
    NotEnoughDiskSpace { needed: u64 },
    /// The model is not in the store's catalog (never with the production
    /// catalog, which lists every [`ModelId`]).
    NotInCatalog,
    /// The download thread could not be started (the OS refused a new thread).
    CannotStart,
}

/// The shortest interval between two `Progress` events (at most 4/s).
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// Decides when a `Progress` event is due, from an injected monotonic `now`
/// (design point 5): at most 4/s, at least 1/s while data arrives, the first chunk
/// reported at once.
///
/// Rule: a chunk emits when no event was emitted yet or the last one is at least
/// 250 ms old. So five events span at least 1 s, and a chunk never sees the last
/// event 250 ms or more in the past without emitting.
#[derive(Debug, Default)]
pub struct ProgressThrottle {
    last: Option<Instant>,
}

impl ProgressThrottle {
    pub fn new() -> ProgressThrottle {
        ProgressThrottle { last: None }
    }

    /// Called once per received chunk at `now`; true when a `Progress` event is due.
    pub fn should_emit(&mut self, now: Instant) -> bool {
        let due = self
            .last
            .is_none_or(|last| now.saturating_duration_since(last) >= PROGRESS_INTERVAL);
        if due {
            self.last = Some(now);
        }
        due
    }
}

/// The running download: which model, and its cancel flag.
#[derive(Debug)]
struct Active {
    id: ModelId,
    cancel: Arc<AtomicBool>,
}

type Slot = Arc<Mutex<Option<Active>>>;

fn lock(slot: &Mutex<Option<Active>>) -> MutexGuard<'_, Option<Active>> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The one downloader. `Send + Sync`; the shell shares one instance.
pub struct Downloader {
    store: Arc<ModelStore>,
    disk: Arc<dyn DiskSpace>,
    timeouts: Timeouts,
    /// The single active slot (R-2): `Some` from `start` until the worker has
    /// removed or renamed the `.part`, cleared before the end event.
    active: Slot,
}

impl Downloader {
    /// `timeouts` is [`Timeouts::default()`] in production (connect,
    /// download_no_data); tests pass milliseconds.
    pub fn new(store: Arc<ModelStore>, disk: Arc<dyn DiskSpace>, timeouts: Timeouts) -> Downloader {
        Downloader {
            store,
            disk,
            timeouts,
            active: Arc::new(Mutex::new(None)),
        }
    }

    /// Starts downloading `id` on a new std thread. Refuses with `Busy` while
    /// another download runs, `AlreadyDownloaded`, or `NotEnoughDiskSpace` (a probe
    /// error lets the download proceed, R-9); a refusal sends no request and emits
    /// no event. Retry = `start` again.
    pub fn start(
        &self,
        id: ModelId,
        events: impl Fn(DownloadEvent) + Send + 'static,
    ) -> Result<(), DownloadError> {
        // The slot stays locked through the checks, so two starts cannot both pass.
        let mut slot = lock(&self.active);
        if slot.is_some() {
            return Err(DownloadError::Busy);
        }
        let entry = self.store.entry(id).ok_or(DownloadError::NotInCatalog)?;
        if self.store.path_if_downloaded(id).is_some() {
            return Err(DownloadError::AlreadyDownloaded);
        }
        let needed = needed_bytes(entry.size_bytes);
        if let Ok(available) = self.disk.available_bytes(self.store.dir()) {
            if available < needed {
                return Err(DownloadError::NotEnoughDiskSpace { needed });
            }
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *slot = Some(Active {
            id,
            cancel: Arc::clone(&cancel),
        });
        // Unlocked before the spawn: if the spawn fails, the dropped closure drops
        // the guard, which takes the lock to clear the slot.
        drop(slot);
        let guard = SlotGuard {
            slot: Arc::clone(&self.active),
            cancel: Arc::clone(&cancel),
        };
        let job = Job {
            entry,
            store: Arc::clone(&self.store),
            timeouts: self.timeouts,
            cancel,
        };
        let spawned = std::thread::Builder::new()
            .name("voicen-model-download".to_string())
            .spawn(move || job.run(guard, events));
        match spawned {
            Ok(_) => Ok(()),
            Err(_) => Err(DownloadError::CannotStart),
        }
    }

    /// Cancels the running download of `id`; true if one was running. Takes effect
    /// when the current read returns (at most `download_no_data` during a stall).
    pub fn cancel(&self, id: ModelId) -> bool {
        match &*lock(&self.active) {
            Some(active) if active.id == id => {
                active.cancel.store(true, Ordering::SeqCst);
                true
            }
            _ => false,
        }
    }
}

/// Catalog size + 1 % (R-9), rounded up.
fn needed_bytes(size: u64) -> u64 {
    size.saturating_add(size.div_ceil(100))
}

/// Clears the active slot (if it still holds this download) on `release` or when
/// dropped, so a panicking worker never leaves the downloader busy for good.
struct SlotGuard {
    slot: Slot,
    cancel: Arc<AtomicBool>,
}

impl SlotGuard {
    fn release(&self) {
        let mut slot = lock(&self.slot);
        if slot
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(&active.cancel, &self.cancel))
        {
            *slot = None;
        }
    }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        self.release();
    }
}

/// How a download ended (before the end event is emitted).
enum End {
    Finished,
    Failed(DownloadFailure),
    Cancelled,
}

/// One download, run on its own thread.
struct Job {
    entry: &'static CatalogEntry,
    store: Arc<ModelStore>,
    timeouts: Timeouts,
    cancel: Arc<AtomicBool>,
}

/// Read buffer of the body loop.
const CHUNK: usize = 64 * 1024;

impl Job {
    /// Runs the transfer, settles the `.part` (renamed or removed), clears the
    /// active slot, then emits exactly one end event.
    fn run(self, guard: SlotGuard, events: impl Fn(DownloadEvent)) {
        let id = self.entry.id;
        let part = self.store.part_path(self.entry);
        // `transfer` returns only after its file handle is closed (Windows cannot
        // remove or rename an open file).
        let end = match self.transfer(&part, &events) {
            Ok(()) => match std::fs::rename(&part, self.store.final_path(self.entry)) {
                Ok(()) => End::Finished,
                Err(_) => End::Failed(DownloadFailure::DiskError),
            },
            Err(end) => end,
        };
        if !matches!(end, End::Finished) {
            remove_part(&part);
        }
        guard.release();
        events(match end {
            End::Finished => DownloadEvent::Finished { id },
            End::Failed(reason) => DownloadEvent::Failed { id, reason },
            End::Cancelled => DownloadEvent::Cancelled { id },
        });
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// A failure, unless the user cancelled meanwhile (a cancel during a stall
    /// surfaces as the read timeout; it is still a cancel).
    fn failed(&self, reason: DownloadFailure) -> End {
        if self.cancelled() {
            End::Cancelled
        } else {
            End::Failed(reason)
        }
    }

    /// GET the catalog URL into `part`, hashing as it goes. `Ok` only when the
    /// byte count equals the catalog size and the digest equals the pinned value,
    /// with `part` written, synced and closed.
    fn transfer(&self, part: &Path, events: &impl Fn(DownloadEvent)) -> Result<(), End> {
        let entry = self.entry;
        let url = url::Url::parse(entry.url).map_err(|_| {
            End::Failed(DownloadFailure::SourceUnreachable {
                host: String::new(),
            })
        })?;
        let host = host_port(&url);
        let transport = |e: &TransportError| self.failed(DownloadFailure::from_transport(e, &host));

        // Per-read timeout (also bounds the wait for the headers), never a total one.
        let client = Client::builder()
            .connect_timeout(self.timeouts.connect)
            .timeout(self.timeouts.download_no_data)
            .build()
            .map_err(|_| transport(&TransportError::Setup))?;
        let sent = client.get(url).send();
        if self.cancelled() {
            return Err(End::Cancelled);
        }
        let mut response = sent.map_err(|e| transport(&send_error(&e)))?;
        let status = response.status();
        if !status.is_success() {
            // The error body is never read or written (P-009).
            return Err(transport(&TransportError::Status(status.as_u16())));
        }

        let disk_error = |_: io::Error| self.failed(DownloadFailure::DiskError);
        std::fs::create_dir_all(self.store.dir()).map_err(disk_error)?;
        let mut file = File::create(part).map_err(disk_error)?;
        let mut hasher = Sha256::new();
        let mut throttle = ProgressThrottle::new();
        let mut received: u64 = 0;
        let mut buf = vec![0u8; CHUNK];
        loop {
            if self.cancelled() {
                return Err(End::Cancelled);
            }
            let n = match response.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(transport(&body_error(&e))),
            };
            let chunk = &buf[..n];
            received = received.saturating_add(chunk.len() as u64);
            if received > entry.size_bytes {
                // Longer than the pinned file: it cannot be the pinned file.
                return Err(self.failed(DownloadFailure::ChecksumMismatch));
            }
            file.write_all(chunk).map_err(disk_error)?;
            hasher.update(chunk);
            if throttle.should_emit(Instant::now()) {
                events(DownloadEvent::Progress {
                    id: entry.id,
                    received,
                    total: entry.size_bytes,
                });
            }
        }
        if self.cancelled() {
            return Err(End::Cancelled);
        }
        if received < entry.size_bytes {
            return Err(self.failed(DownloadFailure::DownloadInterrupted));
        }
        file.sync_all().map_err(disk_error)?;
        drop(file);
        if !hex_eq(&hasher.finalize(), entry.sha256) {
            return Err(self.failed(DownloadFailure::ChecksumMismatch));
        }
        Ok(())
    }
}

/// Removes an unfinished download. A missing file is fine; any other error is
/// ignored (nothing else can be done; `cleanup_at_start` retries at next start).
fn remove_part(part: &Path) {
    let _ = std::fs::remove_file(part);
}

/// `digest` as lowercase hex equals `pinned` (64 lowercase hex digits).
fn hex_eq(digest: &[u8], pinned: &str) -> bool {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let pinned = pinned.as_bytes();
    pinned.len() == digest.len() * 2
        && digest.iter().zip(pinned.chunks(2)).all(|(byte, pair)| {
            pair[0] == HEX[usize::from(byte >> 4)] && pair[1] == HEX[usize::from(byte & 0x0f)]
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::time::Duration;

    const EN_JSON: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../i18n/en.json"));
    const RU_JSON: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../i18n/ru.json"));

    /// One value of every variant. `exhaustive` below stops compiling when a
    /// variant is added, so this list cannot silently fall behind.
    fn every_failure() -> Vec<DownloadFailure> {
        vec![
            DownloadFailure::DownloadInterrupted,
            DownloadFailure::ChecksumMismatch,
            DownloadFailure::NotEnoughDiskSpace { needed: 66_256 },
            DownloadFailure::SourceUnreachable {
                host: "127.0.0.1:9".to_string(),
            },
            DownloadFailure::DiskError,
            DownloadFailure::HttpStatus { code: 404 },
        ]
    }

    #[allow(dead_code)]
    fn exhaustive(f: &DownloadFailure) {
        match f {
            DownloadFailure::DownloadInterrupted
            | DownloadFailure::ChecksumMismatch
            | DownloadFailure::NotEnoughDiskSpace { .. }
            | DownloadFailure::SourceUnreachable { .. }
            | DownloadFailure::DiskError
            | DownloadFailure::HttpStatus { .. } => {}
        }
    }

    fn placeholders(text: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut rest = text;
        while let Some(open) = rest.find('{') {
            let after = &rest[open + 1..];
            match after.find('}') {
                Some(close) => {
                    let name = &after[..close];
                    if !name.is_empty()
                        && name
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                    {
                        out.insert(name.to_string());
                    }
                    rest = &after[close + 1..];
                }
                None => break,
            }
        }
        out
    }

    #[test]
    fn download_failure_wire_codes_match_the_ipc_contract() {
        // contracts/ipc.md FailureReason codes. Bite: any code renamed, two swapped,
        // a Debug-derived or CamelCase code.
        let expected = [
            "download_interrupted",
            "checksum_mismatch",
            "not_enough_disk_space",
            "source_unreachable",
            "disk_error",
            "http_status",
        ];
        let got: Vec<&str> = every_failure().iter().map(DownloadFailure::code).collect();
        assert_eq!(got, expected);
    }

    #[test]
    fn every_download_failure_has_en_and_ru_text_with_its_placeholders() {
        // T-016 Acceptance (en/ru message ids). Bite: a variant without its own id, an
        // id missing or empty in i18n/en.json or i18n/ru.json (the UI would show the
        // raw id), two variants sharing one id, a param the text does not use or a
        // placeholder no param fills (`needed`, `host`, `code`).
        let en: BTreeMap<String, String> = serde_json::from_str(EN_JSON).expect("en.json");
        let ru: BTreeMap<String, String> = serde_json::from_str(RU_JSON).expect("ru.json");
        let mut problems = Vec::new();
        let mut ids = BTreeSet::new();
        for f in every_failure() {
            let id = match serde_json::to_value(f.message_id()) {
                Ok(serde_json::Value::String(id)) => id,
                other => panic!("{f:?}: message id did not serialize to a string: {other:?}"),
            };
            ids.insert(id.clone());
            let params: BTreeSet<String> = f
                .message_params()
                .into_iter()
                .map(|(name, _)| name.to_string())
                .collect();
            for (lang, map) in [("en", &en), ("ru", &ru)] {
                match map.get(&id).filter(|t| !t.trim().is_empty()) {
                    None => problems.push(format!("{} -> {id}: no {lang} text", f.code())),
                    Some(t) if placeholders(t) != params => problems.push(format!(
                        "{} -> {id} ({lang}): placeholders {:?} != params {params:?}",
                        f.code(),
                        placeholders(t)
                    )),
                    Some(_) => {}
                }
            }
        }
        assert!(problems.is_empty(), "{problems:#?}");
        assert_eq!(
            ids.len(),
            every_failure().len(),
            "one message id per variant: {ids:?}"
        );
    }

    // ---- progress throttle (design point 5) ----------------------------------

    /// Feeds chunk times (ms after `t0`) to a fresh throttle; returns the emit times.
    fn emits(chunks_ms: &[u64]) -> Vec<u64> {
        let t0 = Instant::now();
        let mut throttle = ProgressThrottle::new();
        chunks_ms
            .iter()
            .copied()
            .filter(|ms| throttle.should_emit(t0 + Duration::from_millis(*ms)))
            .collect()
    }

    /// The contract: ≤ 4 events in any 1 s window, and while data arrives no chunk
    /// sees the last event 1 s or more in the past.
    fn assert_rate(label: &str, chunks_ms: &[u64]) {
        let e = emits(chunks_ms);
        assert_eq!(
            e.first().copied(),
            chunks_ms.first().copied(),
            "{label}: the first chunk is not reported at once: {e:?}"
        );
        for w in e.windows(5) {
            assert!(
                w[4] - w[0] >= 1_000,
                "{label}: 5 events within 1 s: {w:?} (all: {e:?})"
            );
        }
        let mut last = None;
        let mut ei = e.iter().peekable();
        for &c in chunks_ms {
            while ei.peek().is_some_and(|&&x| x <= c) {
                last = ei.next().copied();
            }
            let last = last.unwrap_or_else(|| panic!("{label}: chunk at {c} ms before any event"));
            assert!(
                c - last < 1_000,
                "{label}: chunk at {c} ms, last event at {last} ms (≥ 1 s): {e:?}"
            );
        }
    }

    #[test]
    fn progress_throttle_steady_stream_is_between_1_and_4_per_second() {
        // Bite: an event per chunk (100/s), a throttle that never emits again after
        // the first, a 1 s+ interval, a first chunk not reported.
        let chunks: Vec<u64> = (0..=3_000).step_by(10).collect();
        assert_rate("every 10 ms for 3 s", &chunks);
        let n = emits(&chunks).len();
        assert!((4..=13).contains(&n), "3 s at 1..4/s gave {n} events");
    }

    #[test]
    fn progress_throttle_irregular_chunks_keep_the_rate() {
        // Bursts, near-boundary gaps (249/250/251 ms, 999 ms) and a pause over 1 s.
        // Bite: a throttle counting chunks instead of time, a window reset that lets a
        // burst through, an off-by-one at the 250 ms or 1 s boundary that breaks a rule.
        let gaps: [u64; 16] = [
            1, 7, 300, 2, 2, 249, 251, 999, 1, 1_500, 3, 3, 250, 250, 250, 10,
        ];
        let mut t = 0;
        let mut chunks = vec![0];
        for _ in 0..4 {
            for g in gaps {
                t += g;
                chunks.push(t);
            }
        }
        assert_rate("irregular", &chunks);
    }
}
