//! The download harness shared by `tests/local_download.rs` and
//! `tests/local_download_refused.rs` (T-016): the event recorder, the fixture
//! (models dir, store, fake disk, downloader) and the end-state assertions.
//!
//! The event callback snapshots the models dir at each event, so "`.part` removed
//! before the end event" is checked at the moment the event is emitted.
//!
//! T-165 (docs/decisions/core-tests.md, in-time rule; guard
//! `scripts/ci/core-test-deadlines.sh`): the only place a download test gets its
//! deadlines and its timed server plans. [`fixture`] and [`fixture_with`] take no
//! `Duration`: they run under `os_answer::in_time()`. The other limits and plans are
//! named constructors whose name states their kind (stall, trickle and cancel legs:
//! residuals at `timing::M`; the held-lookup leg: a connect timeout leg), pinned by
//! `common/deadlines_tests.rs`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::os_answer::{in_time, OS_ANSWER_BUDGET};
use super::timing::M;
use super::{
    catalog, dir_entries, entry, model_bytes, FakeDisk, Serve, Server, NEEDED, QUERY_SECRET, SIZE,
    URL_PATH_MARK,
};
use voicen_core::local_models::catalog::{CatalogEntry, ModelId};
use voicen_core::local_models::download::{DiskSpace, DownloadError, DownloadEvent, Downloader};
use voicen_core::local_models::store::ModelStore;
use voicen_core::models::DownloadedModels;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;

pub const FILE: &str = "ggml-base.bin";
pub const PART: &str = "ggml-base.bin.part";
/// Long enough for any local end event, the ~16.5 s trickle of
/// `local_download::trickle_*` by at least `timing::M`; a hang fails the test instead
/// of the run. Below reqwest's 30 s default, so a client without a read timeout shows
/// as "no end event" (T-111).
pub const END_WAIT: Duration = Duration::from_secs(25);

// ---- named deadlines and timed plans (T-165) ------------------------------------

/// How long a stalling or silent server holds the connection: past [`END_WAIT`], so a
/// client that waits for the hold (no read timeout, reqwest's 30 s default, a cancel
/// that waits for the read) ends after `END_WAIT` and the test fails on "no end
/// event", not on a wall-clock ceiling (T-080 I2).
pub const HOLD: Duration = Duration::from_secs(30);

/// The chunk and the gap of [`trickle_plan`]: 66 chunks, about 16.5 s, at least 3 x
/// [`trickle_no_data`].
pub const TRICKLE_CHUNK: usize = 1_000;
pub const TRICKLE_EVERY: Duration = Duration::from_millis(250);

/// The stall legs' no-data limit. Bite-bound (T-111 residual at `M`, named in
/// docs/decisions/core-tests.md): the stall is the subject and the end comes after
/// it, and the retry shares the `Downloader` and this limit.
pub fn stall_no_data() -> Duration {
    M
}

/// The trickle legs' no-data limit, `M` above the gap of [`trickle_plan`] (T-111
/// residual).
pub fn trickle_no_data() -> Duration {
    TRICKLE_EVERY + M
}

/// The no-data limit of the cancel-during-stall leg, which the cancel must beat; a
/// host stall past it is a missed premise (`with_premise`), never a red.
pub fn cancel_no_data() -> Duration {
    M
}

/// The deadlines of the stall legs: [`stall_no_data`], the rest in time.
pub fn stall_timeouts() -> Timeouts {
    Timeouts {
        download_no_data: stall_no_data(),
        ..in_time()
    }
}

/// The deadlines of the trickle legs: [`trickle_no_data`], the rest in time.
pub fn trickle_timeouts() -> Timeouts {
    Timeouts {
        download_no_data: trickle_no_data(),
        ..in_time()
    }
}

/// The deadlines of the cancel-during-stall leg: [`cancel_no_data`], the rest in time.
pub fn cancel_timeouts() -> Timeouts {
    Timeouts {
        download_no_data: cancel_no_data(),
        ..in_time()
    }
}

/// The held-lookup timeout leg (T-079): connect 300 ms (the limit the download must
/// end by while the lookup is held), `download_no_data` the budget.
pub fn held_lookup_timeouts() -> Timeouts {
    Timeouts {
        connect: Duration::from_millis(300),
        download_no_data: OS_ANSWER_BUDGET,
        ..in_time()
    }
}

/// 4 KiB, then [`HOLD`] of silence.
pub fn stall_plan() -> Serve {
    Serve::Stall {
        after: 4096,
        hold: HOLD,
    }
}

/// Accepts and sends nothing, not even headers, for [`HOLD`].
pub fn silent_headers_plan() -> Serve {
    Serve::SilentHeaders { hold: HOLD }
}

/// [`TRICKLE_CHUNK`] every [`TRICKLE_EVERY`] (about 16.5 s), for the trickle legs.
pub fn trickle_plan() -> Serve {
    Serve::Trickle {
        chunk: TRICKLE_CHUNK,
        every: TRICKLE_EVERY,
    }
}

/// Two chunks 1 s apart: a host stall inside the transfer that a trickle or stall
/// limit must bridge with `M` of slack (T-111 residual).
pub fn one_second_gap_trickle() -> Serve {
    Serve::Trickle {
        chunk: 40_000,
        every: Duration::from_secs(1),
    }
}

/// Two chunks 2.5 s apart, past the old 2 s in-time limit (T-111): an in-time
/// download must survive it.
pub fn in_time_gap_trickle() -> Serve {
    Serve::Trickle {
        chunk: 40_000,
        every: Duration::from_millis(2_500),
    }
}

/// A transfer that outlasts any test thread: chunks spread over at least the budget,
/// under the in-time limit. For the tests that act on a running download and whose
/// subject is not its end (T-111: the race with the end of the transfer is removed).
pub fn long_trickle() -> Serve {
    const CHUNK: usize = 1_000;
    Serve::Trickle {
        chunk: CHUNK,
        every: OS_ANSWER_BUDGET / (SIZE.div_ceil(CHUNK as u64) as u32),
    }
}

/// A trickle that lasts [`END_WAIT`] plus 3 s: a cancel must end it inside `END_WAIT`.
pub fn trickle_past_end_wait() -> Serve {
    let chunks = SIZE.div_ceil(1_000) as u32;
    Serve::Trickle {
        chunk: 1_000,
        every: (END_WAIT + Duration::from_secs(3)) / chunks,
    }
}

/// 1 KiB every second (about 64 s, past the budget): slow enough to look at, cancel
/// or delete mid-body, so no test thread races the end of the transfer (T-111).
pub fn slow_trickle() -> Serve {
    Serve::Trickle {
        chunk: 1024,
        every: Duration::from_secs(1),
    }
}

/// How long a trickle plan takes to send the fake model; `None` for any other plan.
pub fn trickle_lasts(plan: &Serve) -> Option<Duration> {
    match plan {
        Serve::Trickle { chunk, every } => Some(*every * (SIZE.div_ceil(*chunk as u64) as u32)),
        _ => None,
    }
}

// ---- harness ------------------------------------------------------------------

/// One event as seen by the callback at the moment it was emitted.
#[derive(Debug, Clone)]
pub struct Seen {
    pub event: DownloadEvent,
    /// Names in the models dir at that moment.
    pub dir: Vec<String>,
    pub at: Instant,
}

impl Seen {
    pub fn is_end(&self) -> bool {
        !matches!(self.event, DownloadEvent::Progress { .. })
    }
}

pub struct Events {
    rx: Receiver<Seen>,
    pub seen: Vec<Seen>,
}

impl Events {
    /// Blocks until an end event (`Finished` / `Failed` / `Cancelled`); returns it.
    pub fn wait_end(&mut self) -> Seen {
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
    pub fn wait_progress(&mut self) -> Seen {
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
    pub fn drain_for(&mut self, d: Duration) -> Vec<Seen> {
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

    pub fn progress(&self) -> Vec<(u64, u64)> {
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
pub fn events(models_dir: &Path) -> (impl Fn(DownloadEvent) + Send + 'static, Events) {
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

pub struct Fixture {
    _tmp: TempDir,
    pub models: PathBuf,
    pub store: Arc<ModelStore>,
    pub disk: Arc<FakeDisk>,
    pub dl: Downloader,
}

impl Fixture {
    pub fn final_path(&self) -> PathBuf {
        self.models.join(FILE)
    }

    pub fn start(&self, id: ModelId) -> Result<Events, DownloadError> {
        let (cb, ev) = events(&self.models);
        self.dl.start(id, cb).map(|()| ev)
    }
}

/// A models dir (created) with `entries`, a disk with plenty of room, under the
/// in-time deadlines (T-111). No `Duration` parameter (T-165, pinned by
/// `common/deadlines_tests.rs`).
pub fn fixture_with(entries: Vec<CatalogEntry>, disk: Arc<FakeDisk>) -> Fixture {
    fixture_with_timeouts(entries, disk, in_time())
}

/// [`fixture_with`] under the given deadlines. A refused-port test passes the
/// deadlines of its `super::os_answer::OsAnswer` case (T-048: a refused connect takes
/// ~2.17 s on windows-latest; the case's probe checks it within the budget).
pub fn fixture_with_timeouts(
    entries: Vec<CatalogEntry>,
    disk: Arc<FakeDisk>,
    timeouts: Timeouts,
) -> Fixture {
    let tmp = TempDir::new();
    let models = tmp.path().join("models");
    std::fs::create_dir(&models).expect("create models dir");
    let store = Arc::new(ModelStore::new(models.clone(), catalog(entries)));
    let probe: Arc<dyn DiskSpace> = disk.clone();
    let dl = Downloader::new(Arc::clone(&store), probe, timeouts);
    Fixture {
        _tmp: tmp,
        models,
        store,
        disk,
        dl,
    }
}

/// `base` served by `server`, under the in-time deadlines.
pub fn fixture(server: &Server) -> Fixture {
    fixture_with(
        vec![entry(ModelId::Base, FILE, &server.url(FILE))],
        FakeDisk::with_available(10 * NEEDED),
    )
}

/// `base` served by `server`, under [`trickle_timeouts`].
pub fn trickle_fixture(server: &Server) -> Fixture {
    fixture_with_timeouts(
        vec![entry(ModelId::Base, FILE, &server.url(FILE))],
        FakeDisk::with_available(10 * NEEDED),
        trickle_timeouts(),
    )
}

/// `base` served by `server`, under [`stall_timeouts`]; every other deadline
/// (connect included) longer than [`END_WAIT`]: a client that takes its read timeout
/// from another field ends after `END_WAIT`, and the test fails on "no end event"
/// (T-080 I2: the outcome, not a ceiling, catches a longer limit).
pub fn stall_fixture(server: &Server) -> Fixture {
    stall_fixture_under(server, stall_timeouts())
}

/// [`stall_fixture`] under [`trickle_timeouts`]: the stall half of the trickle leg,
/// which the same no-data limit must fail.
pub fn trickle_stall_fixture(server: &Server) -> Fixture {
    stall_fixture_under(server, trickle_timeouts())
}

/// [`stall_fixture`] under [`cancel_timeouts`].
pub fn cancel_stall_fixture(server: &Server) -> Fixture {
    stall_fixture_under(server, cancel_timeouts())
}

fn stall_fixture_under(server: &Server, t: Timeouts) -> Fixture {
    assert!(
        HOLD > END_WAIT,
        "HOLD {HOLD:?} must exceed END_WAIT {END_WAIT:?}"
    );
    for (name, v) in [
        ("connect", t.connect),
        ("api_transcription", t.api_transcription),
        ("local_server", t.local_server),
        ("post_processing", t.post_processing),
        ("builtin", t.builtin),
    ] {
        assert!(
            v > END_WAIT,
            "stall fixture: {name} {v:?} is not above END_WAIT"
        );
    }
    fixture_with_timeouts(
        vec![entry(ModelId::Base, FILE, &server.url(FILE))],
        FakeDisk::with_available(10 * NEEDED),
        t,
    )
}

/// The final file holds exactly the fake model, and nothing else is in the dir.
pub fn assert_verified_file_only(f: &Fixture, label: &str) {
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

/// No event, reason or Debug text carries the URL's query or path (T-040: reqwest
/// errors contain the URL).
pub fn assert_no_url_in(seen: &[Seen], label: &str) {
    for s in seen {
        let text = format!("{:?}", s.event);
        assert!(
            !text.contains(QUERY_SECRET) && !text.contains(URL_PATH_MARK) && !text.contains("http"),
            "{label}: event carries the URL: {text}"
        );
    }
}

/// A retry right after the end event succeeds and leaves only the verified file.
pub fn assert_retry_succeeds(f: &Fixture, label: &str) {
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
