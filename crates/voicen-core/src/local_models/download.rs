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

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use super::catalog::ModelId;
use super::store::ModelStore;
use crate::i18n::MessageId;
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
    /// All bytes arrived but the SHA-256 is not the pinned one.
    ChecksumMismatch,
    /// Free space is below catalog size + 1 % (`needed` in bytes).
    NotEnoughDiskSpace { needed: u64 },
    /// Connect refused / timed out / TLS failed; `host[:port]` of the URL.
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
        // Skeleton (T-016 red tests): not implemented yet.
        todo!("T-016: DownloadFailure::code")
    }

    /// The catalog message (en/ru).
    pub fn message_id(&self) -> MessageId {
        // Skeleton (T-016 red tests): not implemented yet.
        todo!("T-016: DownloadFailure::message_id")
    }

    /// Placeholder values for [`message_id`](Self::message_id): `needed`, `host`,
    /// `code` for the variants that carry them, none otherwise.
    pub fn message_params(&self) -> Vec<(&'static str, String)> {
        // Skeleton (T-016 red tests): not implemented yet.
        todo!("T-016: DownloadFailure::message_params")
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
}

/// Decides when a `Progress` event is due, from an injected monotonic `now`
/// (design point 5): at most 4/s, at least 1/s while data arrives, the first chunk
/// reported at once.
#[derive(Debug, Default)]
pub struct ProgressThrottle {
    // Skeleton (T-016 red tests): the field is read by the implementation.
    #[allow(dead_code)]
    last: Option<Instant>,
}

impl ProgressThrottle {
    pub fn new() -> ProgressThrottle {
        ProgressThrottle { last: None }
    }

    /// Called once per received chunk at `now`; true when a `Progress` event is due.
    pub fn should_emit(&mut self, now: Instant) -> bool {
        // Skeleton (T-016 red tests): not implemented yet.
        let _ = now;
        todo!("T-016: ProgressThrottle::should_emit")
    }
}

/// The one downloader. `Send + Sync`; the shell shares one instance.
pub struct Downloader {
    // Skeleton (T-016 red tests): the fields are read by the implementation.
    #[allow(dead_code)]
    store: Arc<ModelStore>,
    #[allow(dead_code)]
    disk: Arc<dyn DiskSpace>,
    #[allow(dead_code)]
    timeouts: Timeouts,
}

impl Downloader {
    /// `timeouts` is [`Timeouts::default()`] in production (connect,
    /// download_no_data); tests pass milliseconds.
    pub fn new(store: Arc<ModelStore>, disk: Arc<dyn DiskSpace>, timeouts: Timeouts) -> Downloader {
        Downloader {
            store,
            disk,
            timeouts,
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
        // Skeleton (T-016 red tests): not implemented yet.
        let _ = (id, events);
        todo!("T-016: Downloader::start")
    }

    /// Cancels the running download of `id`; true if one was running. Takes effect
    /// when the current read returns (at most `download_no_data` during a stall).
    pub fn cancel(&self, id: ModelId) -> bool {
        // Skeleton (T-016 red tests): not implemented yet.
        let _ = id;
        todo!("T-016: Downloader::cancel")
    }
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
