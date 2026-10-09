//! The download harness shared by `tests/local_download.rs` and
//! `tests/local_download_refused.rs` (T-016): the event recorder, the fixture
//! (models dir, store, fake disk, downloader) and the end-state assertions.
//!
//! The event callback snapshots the models dir at each event, so "`.part` removed
//! before the end event" is checked at the moment the event is emitted.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{
    catalog, dir_entries, entry, model_bytes, FakeDisk, Server, NEEDED, QUERY_SECRET, URL_PATH_MARK,
};
use voicen_core::local_models::catalog::{CatalogEntry, ModelId};
use voicen_core::local_models::download::{DiskSpace, DownloadError, DownloadEvent, Downloader};
use voicen_core::local_models::store::ModelStore;
use voicen_core::models::DownloadedModels;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;

pub const FILE: &str = "ggml-base.bin";
pub const PART: &str = "ggml-base.bin.part";
/// Long enough for any local end event; a hang fails the test instead of the run.
pub const END_WAIT: Duration = Duration::from_secs(10);

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

pub fn timeouts(no_data: Duration) -> Timeouts {
    Timeouts {
        connect: Duration::from_secs(2),
        download_no_data: no_data,
        ..Timeouts::default()
    }
}

/// A models dir (created) with `entries`, a disk with plenty of room.
pub fn fixture_with(entries: Vec<CatalogEntry>, disk: Arc<FakeDisk>, no_data: Duration) -> Fixture {
    fixture_with_timeouts(entries, disk, timeouts(no_data))
}

/// [`fixture_with`] under the given deadlines. A refused-port test passes the
/// deadlines of its `super::os_answer::OsAnswer` case: the 2 s connect and no-data
/// of [`timeouts`] are below a refused connect's ~2.17 s on windows-latest (T-048).
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

/// `base` served by `server`.
pub fn fixture(server: &Server, no_data: Duration) -> Fixture {
    fixture_with(
        vec![entry(ModelId::Base, FILE, &server.url(FILE))],
        FakeDisk::with_available(10 * NEEDED),
        no_data,
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
