//! T-016: the model downloader against a raw-TCP mock server (spec 002 US1,
//! contracts/core-traits.md "Downloader", T-016 analysis approach (1)-(6),
//! decision #49).
//!
//! Every test injects a catalog entry for the fake ~64 KiB model (tests/common) and
//! millisecond timeouts. The downloader runs on its own std thread; the tests call
//! it from plain `#[test]` threads (no tokio runtime: the blocking reqwest client
//! panics inside one, T-040). The event callback snapshots the models dir at each
//! event, so "`.part` removed before the end event" is checked at the moment the
//! event is emitted, not afterwards.

mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{
    catalog, dir_entries, entry, model_bytes, refused_port, url_for, FakeDisk, Serve, Server,
    NEEDED, QUERY_SECRET, SIZE, URL_PATH_MARK,
};
use voicen_core::local_models::catalog::{CatalogEntry, ModelId};
use voicen_core::local_models::download::{
    DiskSpace, DownloadError, DownloadEvent, DownloadFailure, Downloader,
};
use voicen_core::local_models::store::ModelStore;
use voicen_core::models::DownloadedModels;
use voicen_core::secrets::{KeyEdits, KeyPresence};
use voicen_core::settings::validate::{validate, KeyEditsWithPresence};
use voicen_core::settings::{defaults, EngineKind, ErrorCode, FieldId};
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;

const FILE: &str = "ggml-base.bin";
const PART: &str = "ggml-base.bin.part";
/// Long enough for any local end event; a hang fails the test instead of the run.
const END_WAIT: Duration = Duration::from_secs(10);

// ---- harness ------------------------------------------------------------------

/// One event as seen by the callback at the moment it was emitted.
#[derive(Debug, Clone)]
struct Seen {
    event: DownloadEvent,
    /// Names in the models dir at that moment.
    dir: Vec<String>,
    at: Instant,
}

impl Seen {
    fn is_end(&self) -> bool {
        !matches!(self.event, DownloadEvent::Progress { .. })
    }
}

struct Events {
    rx: Receiver<Seen>,
    seen: Vec<Seen>,
}

impl Events {
    /// Blocks until an end event (`Finished` / `Failed` / `Cancelled`); returns it.
    fn wait_end(&mut self) -> Seen {
        let deadline = Instant::now() + END_WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(s) => {
                    self.seen.push(s.clone());
                    if s.is_end() {
                        return s;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    panic!("no end event within {END_WAIT:?}; seen: {:?}", self.seen)
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!(
                        "event callback dropped without an end event; seen: {:?}",
                        self.seen
                    )
                }
            }
        }
    }

    /// Blocks until the first `Progress` (the transfer has started).
    fn wait_progress(&mut self) -> Seen {
        match self.rx.recv_timeout(END_WAIT) {
            Ok(s) => {
                self.seen.push(s.clone());
                assert!(!s.is_end(), "ended before any progress: {:?}", self.seen);
                s
            }
            Err(e) => panic!(
                "no progress within {END_WAIT:?} ({e:?}); seen: {:?}",
                self.seen
            ),
        }
    }

    /// Collects whatever arrives within `d` (to prove nothing more comes).
    fn drain_for(&mut self, d: Duration) -> Vec<Seen> {
        let deadline = Instant::now() + d;
        let mut more = Vec::new();
        while let Ok(s) = self
            .rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            self.seen.push(s.clone());
            more.push(s);
        }
        more
    }

    fn progress(&self) -> Vec<(u64, u64)> {
        self.seen
            .iter()
            .filter_map(|s| match s.event {
                DownloadEvent::Progress {
                    received, total, ..
                } => Some((received, total)),
                _ => None,
            })
            .collect()
    }
}

/// The callback for `start`, and the receiving side.
fn events(models_dir: &Path) -> (impl Fn(DownloadEvent) + Send + 'static, Events) {
    let (tx, rx) = channel();
    let dir = models_dir.to_path_buf();
    let callback = move |event: DownloadEvent| {
        let _ = tx.send(Seen {
            event,
            dir: dir_entries(&dir),
            at: Instant::now(),
        });
    };
    (
        callback,
        Events {
            rx,
            seen: Vec::new(),
        },
    )
}

struct Fixture {
    _tmp: TempDir,
    models: PathBuf,
    store: Arc<ModelStore>,
    disk: Arc<FakeDisk>,
    dl: Downloader,
}

impl Fixture {
    fn final_path(&self) -> PathBuf {
        self.models.join(FILE)
    }

    fn start(&self, id: ModelId) -> Result<Events, DownloadError> {
        let (cb, ev) = events(&self.models);
        self.dl.start(id, cb).map(|()| ev)
    }
}

fn timeouts(no_data: Duration) -> Timeouts {
    Timeouts {
        connect: Duration::from_secs(2),
        download_no_data: no_data,
        ..Timeouts::default()
    }
}

/// A models dir (created) with `entries`, a disk with plenty of room.
fn fixture_with(entries: Vec<CatalogEntry>, disk: Arc<FakeDisk>, no_data: Duration) -> Fixture {
    let tmp = TempDir::new();
    let models = tmp.path().join("models");
    std::fs::create_dir(&models).expect("create models dir");
    let store = Arc::new(ModelStore::new(models.clone(), catalog(entries)));
    let probe: Arc<dyn DiskSpace> = disk.clone();
    let dl = Downloader::new(Arc::clone(&store), probe, timeouts(no_data));
    Fixture {
        _tmp: tmp,
        models,
        store,
        disk,
        dl,
    }
}

/// `base` served by `server`.
fn fixture(server: &Server, no_data: Duration) -> Fixture {
    fixture_with(
        vec![entry(ModelId::Base, FILE, &server.url(FILE))],
        FakeDisk::with_available(10 * NEEDED),
        no_data,
    )
}

/// The final file holds exactly the fake model, and nothing else is in the dir.
fn assert_verified_file_only(f: &Fixture, label: &str) {
    let got = std::fs::read(f.final_path())
        .unwrap_or_else(|e| panic!("{label}: final file missing: {e}"));
    assert!(
        got == model_bytes(),
        "{label}: final file bytes differ ({} bytes)",
        got.len()
    );
    assert_eq!(
        dir_entries(&f.models),
        vec![FILE.to_string()],
        "{label}: models dir"
    );
}

/// Settings validation accepts `builtin_local` with `base` through `store`.
fn validation_accepts_base(store: &dyn DownloadedModels) -> bool {
    let mut s = defaults(Some("en-US"));
    s.engine = EngineKind::BuiltinLocal;
    s.builtin_local.model_id = Some("base".to_string());
    let edits = KeyEdits::default();
    let keys = KeyEditsWithPresence {
        edits: &edits,
        presence: KeyPresence::default(),
    };
    !validate(&s, &keys, store).iter().any(|e| {
        e.field == FieldId::EngineBuiltinLocalModelId && e.code == ErrorCode::ModelNotDownloaded
    })
}

/// No event, reason or Debug text carries the URL's query or path (T-040: reqwest
/// errors contain the URL).
fn assert_no_url_in(seen: &[Seen], label: &str) {
    for s in seen {
        let text = format!("{:?}", s.event);
        assert!(
            !text.contains(QUERY_SECRET) && !text.contains(URL_PATH_MARK) && !text.contains("http"),
            "{label}: event carries the URL: {text}"
        );
    }
}

/// A retry right after the end event succeeds and leaves only the verified file.
fn assert_retry_succeeds(f: &Fixture, label: &str) {
    let mut ev = f
        .start(ModelId::Base)
        .unwrap_or_else(|e| panic!("{label}: retry refused right after the end event: {e:?}"));
    let end = ev.wait_end();
    assert_eq!(
        end.event,
        DownloadEvent::Finished { id: ModelId::Base },
        "{label}: retry did not finish; seen {:?}",
        ev.seen
    );
    assert_verified_file_only(f, &format!("{label} (retry)"));
    assert!(f.store.is_downloaded("base"), "{label}: store after retry");
}

/// The failure branch (Acceptance line 2): `Failed{reason}` with `reason` as
/// expected, `.part` gone and no final file at the moment of the event, exactly
/// one end event, no URL in it, then a retry succeeds.
fn fails_then_retry_succeeds(
    label: &str,
    f: &Fixture,
    expect: impl Fn(&DownloadFailure) -> bool,
) -> Duration {
    let started = Instant::now();
    let mut ev = f
        .start(ModelId::Base)
        .unwrap_or_else(|e| panic!("{label}: start refused: {e:?}"));
    let end = ev.wait_end();
    let took = end.at - started;
    match &end.event {
        DownloadEvent::Failed { id, reason } => {
            assert_eq!(*id, ModelId::Base, "{label}: id");
            assert!(expect(reason), "{label}: unexpected reason {reason:?}");
        }
        other => panic!(
            "{label}: expected Failed, got {other:?}; seen {:?}",
            ev.seen
        ),
    }
    assert!(
        end.dir.is_empty(),
        "{label}: models dir at the Failed event (no .part, no final file): {:?}",
        end.dir
    );
    let more = ev.drain_for(Duration::from_millis(200));
    assert!(more.is_empty(), "{label}: events after the end: {more:?}");
    for s in &ev.seen {
        if let DownloadEvent::Progress {
            total, received, ..
        } = s.event
        {
            assert_eq!(total, SIZE, "{label}: progress total");
            assert!(received <= SIZE, "{label}: progress received {received}");
        }
    }
    assert_no_url_in(&ev.seen, label);
    assert!(
        dir_entries(&f.models).is_empty(),
        "{label}: models dir after"
    );
    assert!(
        !f.store.is_downloaded("base"),
        "{label}: store says downloaded"
    );
    assert_retry_succeeds(f, label);
    took
}

// ---- (1) happy path ---------------------------------------------------------

#[test]
fn happy_path_reports_progress_then_finished_and_the_verified_file_is_downloaded() {
    // Acceptance line 1. Bite: no Progress before Finished, a wrong total, received
    // going backwards or past the size, Finished before the rename (final file not
    // there yet, or `.part` still there), a second end event, the disk probe not
    // asked about the models dir, the store or validation not seeing the file.
    let server = Server::start(vec![]);
    let f = fixture(&server, Duration::from_secs(2));
    let mut ev = f.start(ModelId::Base).expect("start");
    let end = ev.wait_end();

    assert_eq!(end.event, DownloadEvent::Finished { id: ModelId::Base });
    assert_eq!(
        end.dir,
        vec![FILE.to_string()],
        "at Finished the final file is in place and no .part remains"
    );
    let progress = ev.progress();
    assert!(!progress.is_empty(), "no Progress before Finished");
    assert!(
        progress.iter().all(|(_, t)| *t == SIZE),
        "totals {progress:?}"
    );
    assert!(
        progress.windows(2).all(|w| w[0].0 <= w[1].0) && progress.iter().all(|(r, _)| *r <= SIZE),
        "received not monotonic within the size: {progress:?}"
    );
    assert!(
        ev.seen.iter().take_while(|s| !s.is_end()).count() == ev.seen.len() - 1,
        "Finished is not the last event: {:?}",
        ev.seen
    );
    let more = ev.drain_for(Duration::from_millis(200));
    assert!(more.is_empty(), "events after Finished: {more:?}");

    assert_verified_file_only(&f, "happy");
    assert_eq!(server.accepts(), 1, "requests");
    assert_eq!(f.disk.asked(), vec![f.models.clone()], "disk probe dir");
    assert!(f.store.is_downloaded("base"));
    assert_eq!(
        f.store.path_if_downloaded(ModelId::Base),
        Some(f.final_path())
    );
    assert!(
        validation_accepts_base(f.store.as_ref()),
        "settings validation refuses builtin_local with the downloaded model"
    );
}

// ---- (2) failure branches ---------------------------------------------------

#[test]
fn cut_off_body_fails_interrupted_then_retry_succeeds() {
    // Content-Length = size, half the bytes, FIN. Bite: the short file renamed, the
    // `.part` left behind, another reason, no retry possible (busy flag not cleared
    // before the end event).
    let server = Server::start(vec![Serve::CutOff]);
    let f = fixture(&server, Duration::from_secs(2));
    fails_then_retry_succeeds("cut off", &f, |r| {
        *r == DownloadFailure::DownloadInterrupted
    });
}

#[test]
fn dropped_chunked_body_fails_interrupted_then_retry_succeeds() {
    // A chunked body closed without its last chunk (connection drop mid-body).
    let server = Server::start(vec![Serve::DropChunked]);
    let f = fixture(&server, Duration::from_secs(2));
    fails_then_retry_succeeds("drop mid-body", &f, |r| {
        *r == DownloadFailure::DownloadInterrupted
    });
}

#[test]
fn one_altered_byte_fails_checksum_mismatch_then_retry_succeeds() {
    // All SIZE bytes arrive, one flipped. Bite: no hash check, a hash over another
    // range, a comparison against anything but the entry's pinned value.
    let server = Server::start(vec![Serve::Altered]);
    let f = fixture(&server, Duration::from_secs(2));
    fails_then_retry_succeeds("altered byte", &f, |r| {
        *r == DownloadFailure::ChecksumMismatch
    });
}

#[test]
fn short_complete_body_fails_then_retry_succeeds() {
    // A clean end 100 bytes short of the catalog size (a consistent Content-Length).
    // Bite: trusting the server's length instead of the catalog size.
    let server = Server::start(vec![Serve::Short]);
    let f = fixture(&server, Duration::from_secs(2));
    fails_then_retry_succeeds("short body", &f, |r| {
        matches!(
            r,
            DownloadFailure::DownloadInterrupted | DownloadFailure::ChecksumMismatch
        )
    });
}

#[test]
fn overlong_body_fails_then_retry_succeeds() {
    // The model plus one byte. Bite: stopping at the catalog size and accepting a
    // body that is not the pinned file, or hashing only the first SIZE bytes.
    let server = Server::start(vec![Serve::Long]);
    let f = fixture(&server, Duration::from_secs(2));
    fails_then_retry_succeeds("overlong body", &f, |r| {
        matches!(
            r,
            DownloadFailure::DownloadInterrupted | DownloadFailure::ChecksumMismatch
        )
    });
}

#[test]
fn http_404_fails_http_status_then_retry_succeeds() {
    // Bite: the error body written as the model, a status not reported, a 4xx
    // mapped to another reason.
    let server = Server::start(vec![Serve::Status(404)]);
    let f = fixture(&server, Duration::from_secs(2));
    fails_then_retry_succeeds("HTTP 404", &f, |r| {
        *r == DownloadFailure::HttpStatus { code: 404 }
    });
}

#[test]
fn http_500_fails_http_status_then_retry_succeeds() {
    let server = Server::start(vec![Serve::Status(500)]);
    let f = fixture(&server, Duration::from_secs(2));
    fails_then_retry_succeeds("HTTP 500", &f, |r| {
        *r == DownloadFailure::HttpStatus { code: 500 }
    });
}

#[test]
fn stall_past_no_data_fails_interrupted_within_the_timeout_then_retry_succeeds() {
    // 4 KiB, then 3 s of silence; no_data = 300 ms. Bite: no read timeout (reqwest's
    // 30 s default, or none: the test sees the end only after the 3 s hold), the
    // timeout taken from another field, another reason.
    let server = Server::start(vec![Serve::Stall {
        after: 4096,
        hold: Duration::from_secs(3),
    }]);
    let f = fixture(&server, Duration::from_millis(300));
    let took =
        fails_then_retry_succeeds("stall", &f, |r| *r == DownloadFailure::DownloadInterrupted);
    assert!(
        took < Duration::from_millis(2_000),
        "the stall ended after {took:?}, not after ~download_no_data"
    );
}

#[test]
fn no_response_headers_within_no_data_fails_then_retry_succeeds() {
    // The server accepts and never answers for 3 s; no_data = 300 ms also bounds the
    // wait for the headers (ClientBuilder::timeout). Bite: a client without a
    // timeout on the response wait.
    let server = Server::start(vec![Serve::SilentHeaders {
        hold: Duration::from_secs(3),
    }]);
    let f = fixture(&server, Duration::from_millis(300));
    let took = fails_then_retry_succeeds("silent headers", &f, |r| {
        matches!(
            r,
            DownloadFailure::DownloadInterrupted | DownloadFailure::SourceUnreachable { .. }
        )
    });
    assert!(
        took < Duration::from_millis(2_000),
        "no answer ended after {took:?}, not after ~download_no_data"
    );
}

#[test]
fn refused_port_fails_source_unreachable_with_host_port_then_retry_succeeds() {
    // Nothing listens; then a server comes up on the same port. Bite: another
    // reason, the host without the port, the whole URL (path, query) in the reason.
    let port = refused_port();
    let f = fixture_with(
        vec![entry(ModelId::Base, FILE, &url_for(port, FILE))],
        FakeDisk::with_available(10 * NEEDED),
        Duration::from_secs(2),
    );
    let mut ev = f.start(ModelId::Base).expect("start");
    let end = ev.wait_end();
    assert_eq!(
        end.event,
        DownloadEvent::Failed {
            id: ModelId::Base,
            reason: DownloadFailure::SourceUnreachable {
                host: format!("127.0.0.1:{port}")
            }
        }
    );
    assert!(end.dir.is_empty(), "models dir at Failed: {:?}", end.dir);
    assert_no_url_in(&ev.seen, "refused");

    let _server = Server::start_on(port, vec![]);
    assert_retry_succeeds(&f, "refused");
}

#[test]
fn part_file_that_cannot_be_created_fails_disk_error_then_retry_succeeds() {
    // The models "dir" is a regular file, so `<file>.part` cannot be created. Bite:
    // a panic or a hang on the write error, another reason, a final file anyway.
    let server = Server::start(vec![]);
    let f = fixture(&server, Duration::from_secs(2));
    std::fs::remove_dir(&f.models).expect("remove models dir");
    std::fs::write(&f.models, b"not a dir").expect("file in place of the dir");

    let mut ev = f.start(ModelId::Base).expect("start");
    let end = ev.wait_end();
    assert_eq!(
        end.event,
        DownloadEvent::Failed {
            id: ModelId::Base,
            reason: DownloadFailure::DiskError
        }
    );
    assert_eq!(
        std::fs::read(&f.models).expect("the blocking file stays"),
        b"not a dir"
    );

    std::fs::remove_file(&f.models).expect("remove the blocking file");
    std::fs::create_dir(&f.models).expect("create models dir");
    assert_retry_succeeds(&f, "disk error");
}

// ---- (3) trickle vs stall -----------------------------------------------------

#[test]
fn trickle_longer_than_no_data_succeeds_and_stall_fails() {
    // T-016 Investigation "Recommended" first test. A chunk every 60 ms for ~1 s with
    // no_data = 150 ms must finish: the no-data timeout is per read, not a total.
    // The same no_data fails a stall. Bite: RequestBuilder::timeout(no_data) (a
    // total timeout ends the trickle at ~150 ms), no timeout at all (the stall
    // half hangs for the 3 s hold), progress only at the end, a temp name other than
    // `<file>.part` (cleanup_at_start would miss it).
    let no_data = Duration::from_millis(150);
    let server = Server::start(vec![Serve::Trickle {
        chunk: 4_000,
        every: Duration::from_millis(60),
    }]);
    let f = fixture(&server, no_data);
    let started = Instant::now();
    let mut ev = f.start(ModelId::Base).expect("start");
    let first = ev.wait_progress();
    assert!(
        first.dir.contains(&PART.to_string()),
        "while downloading, the bytes go to {PART}: dir {:?}",
        first.dir
    );
    let end = ev.wait_end();
    let took = end.at - started;
    assert_eq!(
        end.event,
        DownloadEvent::Finished { id: ModelId::Base },
        "trickle with no_data {no_data:?}; seen {:?}",
        ev.seen
    );
    assert!(
        took >= Duration::from_millis(800),
        "the trickle took {took:?}; it must outlast no_data many times over"
    );
    let n = ev.progress().len();
    assert!(
        n >= 2,
        "only {n} Progress events over {took:?} (≥ 1/s while data arrives)"
    );
    let max = 4 * (took.as_millis() as usize).div_ceil(1_000) + 1;
    assert!(n <= max, "{n} Progress events over {took:?} (≤ 4/s)");
    assert_verified_file_only(&f, "trickle");

    let stall = Server::start(vec![Serve::Stall {
        after: 4096,
        hold: Duration::from_secs(3),
    }]);
    let g = fixture(&stall, no_data);
    let started = Instant::now();
    let mut ev = g.start(ModelId::Base).expect("start stall");
    let end = ev.wait_end();
    assert_eq!(
        end.event,
        DownloadEvent::Failed {
            id: ModelId::Base,
            reason: DownloadFailure::DownloadInterrupted
        }
    );
    assert!(
        end.at - started < Duration::from_millis(2_000),
        "stall with no_data {no_data:?} ended after {:?}",
        end.at - started
    );
    assert!(end.dir.is_empty(), "dir at Failed: {:?}", end.dir);
}

// ---- (4) cancel ---------------------------------------------------------------

/// Cancel after the first Progress; returns the end event and the time from cancel.
fn cancel_after_first_progress(f: &Fixture, label: &str) -> (Seen, Duration, Events) {
    let mut ev = f.start(ModelId::Base).expect("start");
    let first = ev.wait_progress();
    assert!(
        first.dir.contains(&PART.to_string()),
        "{label}: no .part while downloading"
    );
    let at = Instant::now();
    assert!(
        f.dl.cancel(ModelId::Base),
        "{label}: cancel of the running download"
    );
    let end = ev.wait_end();
    let took = end.at - at;
    (end, took, ev)
}

#[test]
fn cancel_during_stall_ends_cancelled_only_after_the_part_is_gone() {
    // Design point 4: the flag takes effect when the current read returns
    // (≤ no_data = 800 ms here); the end is Cancelled, not Failed, and comes after
    // the `.part` is removed. Bite: Cancelled emitted before the remove, the read
    // timeout reported as Failed{DownloadInterrupted}, the `.part` left, cancel
    // waiting for the 5 s hold.
    let server = Server::start(vec![Serve::Stall {
        after: 4096,
        hold: Duration::from_secs(5),
    }]);
    let f = fixture(&server, Duration::from_millis(800));
    let (end, took, mut ev) = cancel_after_first_progress(&f, "stall");
    assert_eq!(end.event, DownloadEvent::Cancelled { id: ModelId::Base });
    assert!(end.dir.is_empty(), "dir at Cancelled: {:?}", end.dir);
    assert!(took < Duration::from_millis(2_500), "cancel took {took:?}");
    let more = ev.drain_for(Duration::from_millis(300));
    assert!(more.is_empty(), "events after Cancelled: {more:?}");
    assert!(
        !ev.seen
            .iter()
            .any(|s| matches!(s.event, DownloadEvent::Failed { .. })),
        "a retry reason after cancel: {:?}",
        ev.seen
    );
    assert!(!f.dl.cancel(ModelId::Base), "cancel when nothing runs");
    assert!(!f.store.is_downloaded("base"));
    assert_retry_succeeds(&f, "after cancel");
}

#[test]
fn cancel_during_steady_transfer_ends_cancelled_quickly() {
    // Small chunks every 60 ms (~4 s in total): the flag is seen between reads.
    // Bite: cancel not checked in the read loop (the download finishes instead).
    let server = Server::start(vec![Serve::Trickle {
        chunk: 1_000,
        every: Duration::from_millis(60),
    }]);
    let f = fixture(&server, Duration::from_secs(2));
    let (end, took, _ev) = cancel_after_first_progress(&f, "steady");
    assert_eq!(end.event, DownloadEvent::Cancelled { id: ModelId::Base });
    assert!(end.dir.is_empty(), "dir at Cancelled: {:?}", end.dir);
    assert!(took < Duration::from_millis(1_000), "cancel took {took:?}");
}

#[test]
fn cancel_of_another_model_or_when_idle_returns_false_and_changes_nothing() {
    // Bite: cancel ignoring the id (stops the running `base` when asked for `tiny`),
    // cancel returning true with nothing running.
    let server = Server::start(vec![Serve::Stall {
        after: 4096,
        hold: Duration::from_secs(3),
    }]);
    let f = fixture(&server, Duration::from_secs(2));
    assert!(!f.dl.cancel(ModelId::Base), "idle cancel");
    let mut ev = f.start(ModelId::Base).expect("start");
    ev.wait_progress();
    assert!(
        !f.dl.cancel(ModelId::Tiny),
        "cancel of a model that is not running"
    );
    let more = ev.drain_for(Duration::from_millis(300));
    assert!(
        more.iter().all(|s| !s.is_end()),
        "cancel(tiny) ended the base download: {more:?}"
    );
    assert!(f.dl.cancel(ModelId::Base));
    assert_eq!(
        ev.wait_end().event,
        DownloadEvent::Cancelled { id: ModelId::Base }
    );
}

// ---- (5) one at a time ------------------------------------------------------

#[test]
fn second_start_while_one_runs_is_busy_and_sends_no_request() {
    // At most one download (invariant). Bite: a second thread started, Busy only for
    // the same id, a request sent before the refusal.
    let first = Server::start(vec![Serve::Stall {
        after: 4096,
        hold: Duration::from_secs(3),
    }]);
    let other = Server::start(vec![]);
    let f = fixture_with(
        vec![
            entry(ModelId::Tiny, "ggml-tiny.bin", &other.url("ggml-tiny.bin")),
            entry(ModelId::Base, FILE, &first.url(FILE)),
        ],
        FakeDisk::with_available(10 * NEEDED),
        Duration::from_secs(2),
    );
    let mut ev = f.start(ModelId::Base).expect("start");
    ev.wait_progress();
    assert_eq!(
        f.start(ModelId::Base).err(),
        Some(DownloadError::Busy),
        "same id"
    );
    assert_eq!(
        f.start(ModelId::Tiny).err(),
        Some(DownloadError::Busy),
        "other id"
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(first.accepts(), 1, "requests to the running model's server");
    assert_eq!(other.accepts(), 0, "requests for the refused model");
    assert!(f.dl.cancel(ModelId::Base));
    assert_eq!(
        ev.wait_end().event,
        DownloadEvent::Cancelled { id: ModelId::Base }
    );
}

// ---- (6) disk space ---------------------------------------------------------

#[test]
fn disk_below_size_plus_1_percent_refuses_with_needed_and_sends_no_request() {
    // R-9: needed = size + 1 % = 66 256 bytes; available one byte less. Bite: no
    // check, the check without the margin, the check after the request, `needed`
    // not the required total, an event or a `.part` on refusal.
    let server = Server::start(vec![]);
    let f = fixture_with(
        vec![entry(ModelId::Base, FILE, &server.url(FILE))],
        FakeDisk::with_available(NEEDED - 1),
        Duration::from_secs(2),
    );
    let (cb, mut ev) = events(&f.models);
    assert_eq!(
        f.dl.start(ModelId::Base, cb).err(),
        Some(DownloadError::NotEnoughDiskSpace { needed: NEEDED })
    );
    let more = ev.drain_for(Duration::from_millis(300));
    assert!(more.is_empty(), "events after a refused start: {more:?}");
    assert_eq!(server.accepts(), 0, "a request was sent");
    assert!(
        dir_entries(&f.models).is_empty(),
        "models dir {:?}",
        dir_entries(&f.models)
    );
    assert_eq!(f.disk.asked(), vec![f.models.clone()], "disk probe dir");
    // The refusal leaves the downloader idle.
    assert!(!f.dl.cancel(ModelId::Base));
}

#[test]
fn disk_with_exactly_size_plus_1_percent_downloads() {
    // Boundary of R-9. Bite: `>` instead of `>=`, a margin above 1 %.
    let server = Server::start(vec![]);
    let f = fixture_with(
        vec![entry(ModelId::Base, FILE, &server.url(FILE))],
        FakeDisk::with_available(NEEDED),
        Duration::from_secs(2),
    );
    let mut ev = f
        .start(ModelId::Base)
        .expect("start with exactly the needed space");
    assert_eq!(
        ev.wait_end().event,
        DownloadEvent::Finished { id: ModelId::Base }
    );
    assert_verified_file_only(&f, "exact space");
}

#[test]
fn disk_probe_error_lets_the_download_proceed() {
    // R-9: a probe error is not a refusal. Bite: mapping the probe error to
    // NotEnoughDiskSpace or DiskError.
    let server = Server::start(vec![]);
    let f = fixture_with(
        vec![entry(ModelId::Base, FILE, &server.url(FILE))],
        FakeDisk::failing(),
        Duration::from_secs(2),
    );
    let mut ev = f.start(ModelId::Base).expect("start despite a probe error");
    assert_eq!(
        ev.wait_end().event,
        DownloadEvent::Finished { id: ModelId::Base }
    );
    assert_verified_file_only(&f, "probe error");
}

// ---- already downloaded / wrong-size file -----------------------------------

#[test]
fn already_downloaded_model_is_refused_without_a_request() {
    // contracts/core-traits.md DownloadError::AlreadyDownloaded. Bite: downloading
    // again over a good file.
    let server = Server::start(vec![]);
    let f = fixture(&server, Duration::from_secs(2));
    std::fs::write(f.final_path(), model_bytes()).expect("place the model");
    assert_eq!(
        f.start(ModelId::Base).err(),
        Some(DownloadError::AlreadyDownloaded)
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(server.accepts(), 0, "a request was sent");
}

#[test]
fn wrong_size_final_file_is_downloaded_again_and_replaced() {
    // data-model: a final-name file of the wrong size is not downloaded; a
    // re-download overwrites it via rename. Bite: AlreadyDownloaded for any file
    // with the final name, a rename that fails because the target exists.
    let server = Server::start(vec![]);
    let f = fixture(&server, Duration::from_secs(2));
    std::fs::write(f.final_path(), b"stale partial model").expect("place a wrong file");
    let mut ev = f
        .start(ModelId::Base)
        .expect("start over a wrong-size file");
    assert_eq!(
        ev.wait_end().event,
        DownloadEvent::Finished { id: ModelId::Base }
    );
    assert_verified_file_only(&f, "replaced");
}
