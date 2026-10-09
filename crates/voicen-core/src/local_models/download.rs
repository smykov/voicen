//! The one model downloader (spec 002 contracts/core-traits.md "Downloader",
//! research R-2, R-9; T-016 design points 1-6; decision #49).
//!
//! Synchronous (decisions #22, #42): [`Downloader::start`] runs a blocking reqwest
//! GET on its own std thread and reports [`DownloadEvent`]s through a callback.
//! The client comes from `engine::http::client_with_read_timeout` (T-079: the
//! shared resolver, so a host-name lookup with no answer ends the download at the
//! connect deadline as `SourceUnreachable` and is never awaited). The no-data
//! timeout is reqwest's per-read timeout
//! (`ClientBuilder::timeout(Timeouts::download_no_data)` + `connect_timeout`), never
//! a total request timeout: a slow but steady download longer than
//! `download_no_data` must succeed. Bytes go to `<file>.part` and into a streamed
//! SHA-256; only a byte count equal to the catalog size and the pinned digest lead
//! to the rename to the final name. Every other end tries to remove
//! `<file>.part` before the end event (best effort, see `remove_part`). At most
//! one download runs at a time.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::catalog::{CatalogEntry, ModelId};
use super::store::ModelStore;
use crate::engine::http;
use crate::engine::openai::{body_error, host_port};
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
/// `Cancelled` ends it, and only after `<file>.part` was renamed or its removal
/// was attempted (a failed removal is ignored, see `remove_part`).
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
    /// renamed the `.part` or attempted its removal, cleared before the end event.
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
    /// another download runs, `NotInCatalog`, `AlreadyDownloaded`,
    /// `NotEnoughDiskSpace` (a probe error lets the download proceed, R-9), or
    /// `CannotStart` when the thread cannot be spawned; a refusal sends no
    /// request, emits no event and leaves no active slot. Retry = `start` again.
    ///
    /// `events` is called only on the download thread, never on the caller's thread
    /// and never before `start` returns on it: `LocalModels::download` holds its
    /// state lock across this call, and the callback takes that lock, so a call here
    /// would deadlock it.
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
    /// Runs the transfer, settles the `.part` (renamed, or removal attempted),
    /// clears the active slot, then emits exactly one end event.
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
        // The slot is free before the end event, so a new `start` of the same model
        // can begin in the short gap before this event is handled; the coordinator
        // then shows this download's end state over the new one until the new
        // download's first progress event, or its own end event if that comes first;
        // if the new download had already ended, until the next start (accepted,
        // docs/decisions/model-download.md).
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

        // Per-read timeout (also bounds the wait for the headers), never a total one;
        // the client and its resolver come from `engine::http` (T-079).
        let client =
            http::client_with_read_timeout(self.timeouts.connect, self.timeouts.download_no_data)
                .map_err(|e| transport(&e))?;
        let sent = client.send(client.get(url));
        if self.cancelled() {
            return Err(End::Cancelled);
        }
        let mut response = sent.map_err(|e| transport(&e))?;
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

/// Removes an unfinished download, best effort. A missing file is fine; any
/// other error is ignored and the end event is still emitted: the leftover
/// `.part` is never read as a model (the store looks only at the final name),
/// the next `start` truncates it and `ModelStore::cleanup_at_start` removes it at
/// the next app start. Reporting `DiskError` instead would turn a user's cancel
/// into a retry reason. Open item: log the failed removal once T-008 provides
/// logging.
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
    use crate::i18n::{text, UiLanguage, MESSAGE_IDS};
    use std::collections::BTreeSet;
    use std::time::Duration;

    /// One value of every variant. `exhaustive` below stops compiling when a
    /// variant is added, so this list cannot silently fall behind.
    fn every_failure() -> Vec<DownloadFailure> {
        vec![
            DownloadFailure::DownloadInterrupted,
            DownloadFailure::ChecksumMismatch,
            DownloadFailure::NotEnoughDiskSpace { needed: 66_256 },
            DownloadFailure::SourceUnreachable {
                host: "models.example.com:8443".to_string(),
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

    /// (message id, en text, ru text) per variant, with the params of
    /// `every_failure` filled in. The texts are T-016's `download.*` catalog
    /// entries. `{needed}` is rendered as given (a byte count here): the UI formats
    /// it with `formatSize` before rendering (T-045, decision #58), so the texts
    /// carry no unit of their own.
    fn expected(f: &DownloadFailure) -> (&'static str, &'static str, &'static str) {
        match f {
            DownloadFailure::DownloadInterrupted => (
                "download.interrupted",
                "The download was interrupted. Check the connection and try again.",
                "Загрузка прервалась. Проверьте подключение и попробуйте снова.",
            ),
            DownloadFailure::ChecksumMismatch => (
                "download.checksum_mismatch",
                "The downloaded file is damaged (checksum mismatch). Try again.",
                "Скачанный файл повреждён (контрольная сумма не совпадает). Попробуйте снова.",
            ),
            DownloadFailure::NotEnoughDiskSpace { .. } => (
                "download.not_enough_disk_space",
                "Not enough free disk space: 66256 needed.",
                "Недостаточно места на диске: нужно 66256.",
            ),
            DownloadFailure::SourceUnreachable { .. } => (
                "download.source_unreachable",
                "Cannot reach models.example.com:8443",
                "Не удаётся подключиться к models.example.com:8443",
            ),
            DownloadFailure::DiskError => (
                "download.disk_error",
                "The model file could not be written to the disk.",
                "Не удалось записать файл модели на диск.",
            ),
            DownloadFailure::HttpStatus { .. } => (
                "download.http_status",
                "The download failed (HTTP 404).",
                "Не удалось скачать модель (HTTP 404).",
            ),
        }
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
    fn message_ids_per_download_failure() {
        // The failure.rs `message_ids_per_reason` pattern (docs/decisions/i18n.md:
        // completeness and parity stay in i18n::tests). Each variant has its own
        // `download.*` id declared with messages! (so `message_ids_exist_in_both_catalogs`
        // covers it), and that id renders the expected en and ru text with the
        // variant's params. Bite: a shared or wrong id, an id not in MESSAGE_IDS, a
        // missing catalog entry (text() falls back to the id), `needed`/`host`/`code`
        // not passed or under another name (the placeholder stays unfilled).
        let mut wrong = Vec::new();
        let mut ids = BTreeSet::new();
        for f in every_failure() {
            let (id, en, ru) = expected(&f);
            let got_id = serde_json::to_value(f.message_id()).unwrap_or_default();
            if got_id != serde_json::Value::String(id.to_string()) {
                wrong.push(format!("{f:?}: message id {got_id}, expected {id:?}"));
            }
            ids.insert(got_id.to_string());
            if !MESSAGE_IDS.contains(&f.message_id()) {
                wrong.push(format!("{f:?}: message id not declared with messages!"));
            }
            let params = f.message_params();
            let args: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
            for (lang, want) in [(UiLanguage::En, en), (UiLanguage::Ru, ru)] {
                let got = text(lang, f.message_id(), &args);
                if got != want {
                    wrong.push(format!("{f:?} {lang:?}: {got:?}, expected {want:?}"));
                }
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
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
