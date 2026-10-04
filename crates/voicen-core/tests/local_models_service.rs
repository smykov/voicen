//! T-044 (decision #57 option A): the local-models coordinator
//! `voicen_core::local_models::service::LocalModels` that the shell's commands and
//! `local-model://` events go through (specs/002 contracts/ipc.md).
//!
//! What is proven here, on the Linux gate: `open` runs `cleanup_at_start` before it
//! returns; `list` merges the disk state with the in-memory `Downloading` / `Failed`
//! state; every event reaches `emit` from the download thread only, after the state
//! is updated and with no lock held (the callback calls `list` itself, so a held
//! lock hangs it and the test fails at `END_WAIT`); a refusal changes no state and
//! emits nothing; every `DownloadError` / `DownloadFailure` maps to its contract
//! code and message id; the wire JSON is pinned by `e2e/fixtures/local-models-wire.json`
//! (the Playwright mock's data for T-045, like `settings-wire.json`).
//!
//! The assets (raw-TCP model server, fake model, `FakeDisk`, catalog helpers) come
//! from `voicen_core::test_support::local_models`, the copy the shell tests use too.
//! No refused-port or stall case: the downloader's own tests cover those (F-004,
//! F-005). Fake data only: 127.0.0.1, `huggingface.co` as a host string.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use voicen_core::i18n::{self, MessageId, MESSAGE_IDS};
use voicen_core::local_models::catalog::{CatalogEntry, ModelId, MODELS};
use voicen_core::local_models::download::{DiskSpace, DownloadError, DownloadFailure};
use voicen_core::local_models::service::{
    LocalModelEvent, LocalModelView, LocalModels, ReasonView, PROGRESS_EVENT, STATE_EVENT,
};
use voicen_core::local_models::store::{LocalModelState, ModelStore};
use voicen_core::models::DownloadedModels;
use voicen_core::test_support::local_models::{
    catalog, dir_entries, entry, file_name, five_entries, model_bytes, FakeDisk, Serve, Server,
    NEEDED, SIZE,
};
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;

/// Long enough for any local end event; a hang (a lock held across `emit`) fails
/// the test instead of the run.
const END_WAIT: Duration = Duration::from_secs(10);
/// How long "nothing more happens" is watched.
const QUIET: Duration = Duration::from_millis(300);
/// `Downloader::start`'s worker thread name (download.rs).
const DOWNLOAD_THREAD: &str = "voicen-model-download";

fn timeouts() -> Timeouts {
    Timeouts {
        connect: Duration::from_secs(2),
        download_no_data: Duration::from_secs(5),
        ..Timeouts::default()
    }
}

/// Slow enough to look at and cancel mid-body: 1 KiB every 50 ms (~3.2 s).
fn trickle() -> Serve {
    Serve::Trickle {
        chunk: 1024,
        every: Duration::from_millis(50),
    }
}

// ---- harness ------------------------------------------------------------------

struct World {
    _tmp: TempDir,
    /// The models dir (`<tmp>/models`; created only by `prepare` or a download).
    dir: PathBuf,
    server: Server,
    catalog: &'static [CatalogEntry],
    models: Arc<LocalModels>,
}

/// The five models served by a server with `plan`, plenty of disk, nothing on disk.
fn world(plan: Vec<Serve>) -> World {
    world_on(
        plan,
        FakeDisk::with_available(10 * NEEDED),
        five_entries,
        |_| {},
    )
}

/// `prepare` runs on the models dir path before `open`; `open`'s cleanup must succeed.
fn world_on(
    plan: Vec<Serve>,
    disk: Arc<FakeDisk>,
    catalog_for: impl FnOnce(&Server) -> &'static [CatalogEntry],
    prepare: impl FnOnce(&Path),
) -> World {
    let tmp = TempDir::new();
    let dir = tmp.path().join("models");
    prepare(&dir);
    let server = Server::start(plan);
    let catalog = catalog_for(&server);
    let probe: Arc<dyn DiskSpace> = disk;
    let (models, cleanup) = LocalModels::open(dir.clone(), probe, timeouts(), catalog);
    if let Err(e) = cleanup {
        panic!("cleanup_at_start failed at open: {e}");
    }
    World {
        _tmp: tmp,
        dir,
        server,
        catalog,
        models: Arc::new(models),
    }
}

/// Creates the models dir and writes `name` into it.
fn put(dir: &Path, name: &str, bytes: &[u8]) {
    std::fs::create_dir_all(dir).expect("create models dir");
    std::fs::write(dir.join(name), bytes).unwrap_or_else(|e| panic!("write {name}: {e}"));
}

fn final_name(id: ModelId) -> String {
    format!("ggml-{}.bin", id.as_str())
}

fn part_files(names: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|n| n.ends_with(".part"))
        .cloned()
        .collect()
}

fn state_of(models: &LocalModels, id: ModelId) -> LocalModelState {
    models
        .list()
        .into_iter()
        .find(|v| v.id == id)
        .unwrap_or_else(|| panic!("{id:?} not listed"))
        .state
}

fn states(models: &LocalModels) -> Vec<(ModelId, LocalModelState)> {
    models.list().into_iter().map(|v| (v.id, v.state)).collect()
}

fn all_not_downloaded() -> Vec<(ModelId, LocalModelState)> {
    ModelId::ALL
        .into_iter()
        .map(|id| (id, LocalModelState::NotDownloaded))
        .collect()
}

fn event_id(event: &LocalModelEvent) -> ModelId {
    match event {
        LocalModelEvent::Progress { id, .. } | LocalModelEvent::State { id, .. } => *id,
    }
}

/// The state `list` must show for the event's model once the event is emitted.
fn state_implied_by(event: &LocalModelEvent) -> LocalModelState {
    match event {
        LocalModelEvent::Progress {
            received, total, ..
        } => LocalModelState::Downloading {
            received: *received,
            total: *total,
        },
        LocalModelEvent::State { state, .. } => state.clone(),
    }
}

/// One event as the `emit` callback saw it, at the moment it was called.
#[derive(Debug, Clone)]
struct Seen {
    event: LocalModelEvent,
    /// The calling thread's name.
    thread: Option<String>,
    /// `list()`'s state of the event's model, read inside the callback.
    listed: Option<LocalModelState>,
    /// Names in the models dir.
    dir: Vec<String>,
}

struct Events {
    rx: Receiver<Seen>,
    seen: Vec<Seen>,
}

impl Events {
    /// Blocks until a `State` event; returns it.
    fn wait_state(&mut self) -> Seen {
        let deadline = Instant::now() + END_WAIT;
        loop {
            match self
                .rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(s) => {
                    self.seen.push(s.clone());
                    if matches!(s.event, LocalModelEvent::State { .. }) {
                        return s;
                    }
                }
                Err(RecvTimeoutError::Timeout) => panic!(
                    "no state event within {END_WAIT:?} (a lock held across emit hangs \
                     the callback's list()); seen: {:?}",
                    self.seen
                ),
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("emit dropped without a state event; seen: {:?}", self.seen)
                }
            }
        }
    }

    /// Blocks until the first `Progress` (the transfer has started).
    fn wait_progress(&mut self) -> Seen {
        match self.rx.recv_timeout(END_WAIT) {
            Ok(s) => {
                self.seen.push(s.clone());
                assert!(
                    matches!(s.event, LocalModelEvent::Progress { .. }),
                    "first event is not progress: {:?}",
                    self.seen
                );
                s
            }
            Err(e) => panic!("no progress within {END_WAIT:?} ({e:?})"),
        }
    }

    /// Whatever arrives within `d`.
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
}

/// The `emit` callback for `download`, and the receiving side. The callback reads
/// `list()` itself (through a weak handle, so it never keeps the coordinator alive).
fn recorder(w: &World) -> (impl Fn(LocalModelEvent) + Send + 'static, Events) {
    let (tx, rx) = channel();
    let models = Arc::downgrade(&w.models);
    let dir = w.dir.clone();
    let callback = move |event: LocalModelEvent| {
        let id = event_id(&event);
        let listed = models
            .upgrade()
            .and_then(|m| m.list().into_iter().find(|v| v.id == id).map(|v| v.state));
        let _ = tx.send(Seen {
            event,
            thread: std::thread::current().name().map(str::to_string),
            listed,
            dir: dir_entries(&dir),
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

/// Starts `id`; panics on a refusal.
#[track_caller]
fn start(w: &World, id: &str) -> Events {
    let (cb, ev) = recorder(w);
    if let Err(reason) = w.models.download(id, cb) {
        panic!("download({id:?}) refused: {reason:?}");
    }
    ev
}

/// A refused start: returns the reason and checks that the callback was never
/// called (a refusal emits nothing).
#[track_caller]
fn refused(w: &World, id: &str) -> ReasonView {
    let (cb, mut ev) = recorder(w);
    let reason = match w.models.download(id, cb) {
        Ok(()) => panic!("download({id:?}) was not refused"),
        Err(reason) => reason,
    };
    let emitted = ev.drain_for(QUIET);
    assert!(
        emitted.is_empty(),
        "download({id:?}) refused with {reason:?} but emitted {emitted:?}"
    );
    reason
}

fn reason(
    code: &'static str,
    message_key: MessageId,
    params: &[(&'static str, &str)],
) -> ReasonView {
    ReasonView {
        code,
        message_key,
        params: params.iter().map(|(k, v)| (*k, v.to_string())).collect(),
    }
}

fn wire<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("serializes")
}

// ---- open ---------------------------------------------------------------------

#[test]
fn open_deletes_every_part_file_before_it_returns() {
    // Invariant: no store exists un-cleaned (open runs cleanup_at_start). A `.part`
    // of a full size is not a model either. Bite: open without cleanup_at_start
    // (both `.part` files stay), or a cleanup that also deletes a verified model.
    let w = world_on(
        vec![],
        FakeDisk::with_available(10 * NEEDED),
        five_entries,
        |dir| {
            put(dir, "ggml-base.bin.part", &[7u8; 100]);
            put(dir, "ggml-tiny.bin.part", &model_bytes());
            put(dir, "ggml-small.bin", &model_bytes());
        },
    );

    assert_eq!(dir_entries(&w.dir), vec!["ggml-small.bin".to_string()]);
    let mut expected = all_not_downloaded();
    expected[2].1 = LocalModelState::Downloaded;
    assert_eq!(states(&w.models), expected);
}

#[test]
fn open_over_a_missing_dir_creates_nothing_and_lists_not_downloaded() {
    // The models dir appears only with the first download (download.rs creates it
    // after the response). Bite: open creating the dir, or failing on NotFound.
    let w = world(vec![]);

    assert!(!w.dir.exists(), "open created {}", w.dir.display());
    assert_eq!(states(&w.models), all_not_downloaded());
    assert_eq!(w.server.accepts(), 0);
}

#[test]
fn open_reports_a_cleanup_error_and_still_lists() {
    // The shell prints one fixed line for a cleanup error and runs on. A regular
    // file at the models dir path makes read_dir fail. Bite: the error swallowed
    // (Ok), or no usable coordinator after it (panic, empty list).
    let tmp = TempDir::new();
    let dir = tmp.path().join("models");
    std::fs::write(&dir, b"not a dir").expect("file at the models dir path");
    let probe: Arc<dyn DiskSpace> = FakeDisk::with_available(10 * NEEDED);

    let (models, cleanup) = LocalModels::open(dir.clone(), probe, timeouts(), MODELS);

    assert!(cleanup.is_err(), "cleanup over a file reported Ok");
    assert_eq!(states(&models), all_not_downloaded());
    assert!(dir.is_file(), "the file at the models dir path was touched");
}

// ---- list ---------------------------------------------------------------------

#[test]
fn list_is_the_catalog_in_order_with_the_contract_fields() {
    // contracts/ipc.md LocalModelView: always five, catalog order; nameKey
    // `local_model.name.<id>`; sizeBytes from the catalog; recommended only small;
    // loaded false (T-017); state from the disk. Bite: another order, a name key
    // spelled another way, a size not from the catalog, recommended hard-coded,
    // `loaded` true, the disk state ignored.
    let w = world_on(
        vec![],
        FakeDisk::with_available(10 * NEEDED),
        five_entries,
        |dir| put(dir, &final_name(ModelId::Base), &model_bytes()),
    );

    let list = w.models.list();

    let name_keys = [
        i18n::LOCAL_MODEL_NAME_TINY,
        i18n::LOCAL_MODEL_NAME_BASE,
        i18n::LOCAL_MODEL_NAME_SMALL,
        i18n::LOCAL_MODEL_NAME_MEDIUM_Q5_0,
        i18n::LOCAL_MODEL_NAME_LARGE_V3_TURBO_Q5_0,
    ];
    let expected: Vec<LocalModelView> = ModelId::ALL
        .into_iter()
        .zip(name_keys)
        .map(|(id, name_key)| LocalModelView {
            id,
            name_key,
            size_bytes: SIZE,
            recommended: id == ModelId::Small,
            state: if id == ModelId::Base {
                LocalModelState::Downloaded
            } else {
                LocalModelState::NotDownloaded
            },
            loaded: false,
        })
        .collect();
    assert_eq!(list, expected);
    assert_eq!(
        wire(&list[1]),
        json!({
            "id": "base",
            "nameKey": "local_model.name.base",
            "sizeBytes": 65_600,
            "recommended": false,
            "state": { "kind": "downloaded" },
            "loaded": false,
        })
    );
}

#[test]
fn name_keys_are_declared_message_ids() {
    // A nameKey the UI cannot render shows the raw id. The texts are checked by
    // i18n's MESSAGE_IDS test; this links the wire keys to that list. Bite: a key
    // built with format! (not a declared MessageId), or one for another id.
    let tmp = TempDir::new();
    let probe: Arc<dyn DiskSpace> = FakeDisk::with_available(u64::MAX);
    let (models, _) = LocalModels::open(tmp.path().join("models"), probe, timeouts(), MODELS);
    let declared: BTreeSet<String> = MESSAGE_IDS
        .iter()
        .map(|id| wire(id).as_str().expect("an id string").to_string())
        .collect();

    for view in models.list() {
        let key = wire(&view.name_key);
        let key = key.as_str().expect("nameKey is a string");
        assert_eq!(
            key,
            format!("local_model.name.{}", view.id.as_str()),
            "{:?}",
            view.id
        );
        assert!(declared.contains(key), "{key} is not in MESSAGE_IDS");
    }
}

// ---- download: events, order, merged state --------------------------------------

#[test]
fn download_emits_progress_then_downloaded_from_the_download_thread_and_list_agrees() {
    // Invariant: every event comes from the download thread, after the in-memory
    // state is updated, with no lock held; the end comes after the file is in place;
    // the one store sees the model. Bite: an event emitted from download() itself
    // (caller thread), the state updated after emit (list() inside the callback
    // disagrees), a lock held across emit (the callback's list() hangs), a second
    // store (store() not seeing the model), more than one end event.
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<LocalModels>();

    let w = world(vec![Serve::Full]);
    let mut ev = start(&w, "base");
    let end = ev.wait_state();

    assert_eq!(
        end.event,
        LocalModelEvent::State {
            id: ModelId::Base,
            state: LocalModelState::Downloaded,
        }
    );
    assert_eq!(
        end.dir,
        vec![final_name(ModelId::Base)],
        "dir at the end event"
    );
    let more = ev.drain_for(QUIET);
    assert!(more.is_empty(), "events after the end: {more:?}");

    let (progress, ends): (Vec<&Seen>, Vec<&Seen>) = ev
        .seen
        .iter()
        .partition(|s| matches!(s.event, LocalModelEvent::Progress { .. }));
    assert!(!progress.is_empty(), "no progress before the end");
    assert_eq!(ends.len(), 1, "end events: {ends:?}");
    let mut last = 0;
    for s in &progress {
        let LocalModelEvent::Progress {
            id,
            received,
            total,
        } = s.event
        else {
            unreachable!()
        };
        assert_eq!((id, total), (ModelId::Base, SIZE), "{s:?}");
        assert!(last <= received && received <= SIZE, "{s:?} after {last}");
        last = received;
    }
    for s in &ev.seen {
        assert_eq!(s.thread.as_deref(), Some(DOWNLOAD_THREAD), "{s:?}");
        assert_eq!(
            s.listed,
            Some(state_implied_by(&s.event)),
            "list() inside the callback disagrees with the event: {s:?}"
        );
    }

    assert_eq!(
        state_of(&w.models, ModelId::Base),
        LocalModelState::Downloaded
    );
    assert!(Arc::ptr_eq(&w.models.store(), &w.models.store()));
    assert!(w.models.store().is_downloaded("base"));
    assert_eq!(w.models.store().list(), vec!["base".to_string()]);
}

#[test]
fn list_shows_downloading_from_start_until_cancel_then_not_downloaded() {
    // The transient Downloading is set by the time download() returns (the first
    // progress can be late), and a cancel ends in not_downloaded with no reason and
    // no `.part`. Bite: Downloading only after the first progress, Cancelled mapped
    // to Failed, the transient state left behind, cancel not reaching the
    // downloader, cancel true twice.
    let w = world(vec![trickle()]);
    let mut ev = start(&w, "base");

    let now = state_of(&w.models, ModelId::Base);
    assert!(
        matches!(now, LocalModelState::Downloading { total, .. } if total == SIZE),
        "right after download(): {now:?}"
    );
    let first = ev.wait_progress();
    assert_eq!(first.listed, Some(state_implied_by(&first.event)));

    assert!(w.models.cancel("base"), "cancel of the running download");
    let end = ev.wait_state();

    assert_eq!(
        end.event,
        LocalModelEvent::State {
            id: ModelId::Base,
            state: LocalModelState::NotDownloaded,
        }
    );
    assert_eq!(end.listed, Some(LocalModelState::NotDownloaded));
    assert_eq!(
        part_files(&end.dir),
        Vec::<String>::new(),
        "at the end event"
    );
    assert_eq!(dir_entries(&w.dir), Vec::<String>::new());
    assert_eq!(states(&w.models), all_not_downloaded());
    assert!(!w.models.cancel("base"), "a second cancel");
}

#[test]
fn failed_keeps_its_reason_until_a_retry_and_a_new_open_forgets_it() {
    // data-model: Failed is in memory only (lost on restart, FR-008) and is shown
    // with its reason until a retry. Bite: Failed read from the disk (shows
    // not_downloaded), cleared by a later list(), kept by a new open, or still shown
    // during the retry.
    let w = world(vec![Serve::Status(500), Serve::Full]);
    let mut ev = start(&w, "base");
    let end = ev.wait_state();

    let failed = LocalModelState::Failed {
        reason: DownloadFailure::HttpStatus { code: 500 },
    };
    assert_eq!(
        end.event,
        LocalModelEvent::State {
            id: ModelId::Base,
            state: failed.clone(),
        }
    );
    assert_eq!(end.listed, Some(failed.clone()));
    assert_eq!(
        part_files(&end.dir),
        Vec::<String>::new(),
        "at the end event"
    );
    let mut expected = all_not_downloaded();
    expected[1].1 = failed.clone();
    assert_eq!(states(&w.models), expected);
    std::thread::sleep(QUIET);
    assert_eq!(states(&w.models), expected, "Failed did not stay");

    let probe: Arc<dyn DiskSpace> = FakeDisk::with_available(10 * NEEDED);
    let (reopened, cleanup) = LocalModels::open(w.dir.clone(), probe, timeouts(), w.catalog);
    assert!(cleanup.is_ok(), "{cleanup:?}");
    assert_eq!(states(&reopened), all_not_downloaded(), "a new open");
    assert_eq!(states(&w.models), expected, "the first coordinator");

    let mut retry = start(&w, "base");
    let during = state_of(&w.models, ModelId::Base);
    assert!(
        matches!(during, LocalModelState::Downloading { .. })
            || during == LocalModelState::Downloaded,
        "during the retry: {during:?}"
    );
    let end = retry.wait_state();
    assert_eq!(
        end.event,
        LocalModelEvent::State {
            id: ModelId::Base,
            state: LocalModelState::Downloaded,
        }
    );
    assert_eq!(
        state_of(&w.models, ModelId::Base),
        LocalModelState::Downloaded
    );
}

#[test]
fn checksum_mismatch_lists_failed_with_its_wire_reason() {
    // The failed view on the wire carries the reason (contracts/ipc.md
    // FailureReason). Bite: the reason dropped from the view, a code or message
    // key not from the one mapping.
    let w = world(vec![Serve::Altered]);
    let mut ev = start(&w, "base");
    ev.wait_state();

    let view = w
        .models
        .list()
        .into_iter()
        .find(|v| v.id == ModelId::Base)
        .expect("base listed");
    assert_eq!(
        wire(&view)["state"],
        json!({
            "kind": "failed",
            "reason": { "code": "checksum_mismatch", "messageKey": "download.checksum_mismatch" },
        })
    );
    assert_eq!(dir_entries(&w.dir), Vec::<String>::new());
}

// ---- refusals --------------------------------------------------------------------

#[test]
fn a_refused_start_changes_no_state_and_emits_nothing() {
    // A refusal sends no request, emits nothing and leaves the list as it was: a
    // Failed keeps its reason, the running model stays Downloading. Bite: the
    // transient Downloading set before start and not undone on Err, a Failed
    // dropped by a refused retry (the list then disagrees with the last event), a
    // refusal reported through emit. This holds for busy, not_enough_disk_space,
    // not_in_catalog and download_cannot_start; already_downloaded is the one
    // exception (a_failed_never_hides_a_model_downloaded_on_disk_...).
    let w = world(vec![Serve::Status(500), trickle()]);
    let mut first = start(&w, "base");
    first.wait_state();
    let mut running = start(&w, "tiny");
    running.wait_progress();
    let before = states(&w.models);
    assert!(
        matches!(before[0].1, LocalModelState::Downloading { .. }),
        "{before:?}"
    );

    for id in ["base", "tiny", "small"] {
        let got = refused(&w, id);
        assert_eq!(
            got,
            reason("download_busy", i18n::DOWNLOAD_BUSY, &[]),
            "{id}"
        );
    }

    let after = states(&w.models);
    assert_eq!(
        after[1].1,
        LocalModelState::Failed {
            reason: DownloadFailure::HttpStatus { code: 500 },
        },
        "base after the refusals: {after:?}"
    );
    assert!(
        matches!(after[0].1, LocalModelState::Downloading { .. }),
        "tiny after the refusals: {after:?}"
    );
    assert_eq!(
        after[2].1,
        LocalModelState::NotDownloaded,
        "small: {after:?}"
    );

    assert!(w.models.cancel("tiny"));
    running.wait_state();
    assert_eq!(w.server.accepts(), 2, "a refused start sent a request");
}

#[test]
fn already_downloaded_is_refused_with_its_code() {
    // contracts/ipc.md `already_downloaded`. Bite: a re-download, another code, or
    // the downloaded state replaced by Downloading.
    let w = world_on(
        vec![],
        FakeDisk::with_available(10 * NEEDED),
        five_entries,
        |dir| put(dir, &final_name(ModelId::Base), &model_bytes()),
    );

    let got = refused(&w, "base");

    assert_eq!(
        got,
        reason("already_downloaded", i18n::DOWNLOAD_ALREADY_DOWNLOADED, &[])
    );
    assert_eq!(
        state_of(&w.models, ModelId::Base),
        LocalModelState::Downloaded
    );
    assert_eq!(w.server.accepts(), 0);
}

#[test]
fn a_failed_never_hides_a_model_downloaded_on_disk_and_a_retry_does_not_bring_it_back() {
    // Review round 1 #1. Invariant: an in-memory Failed never masks a disk
    // Downloaded, so list() agrees with store().is_downloaded (settings validation,
    // the same Arc). The one exception to "a refused start keeps the state it had"
    // is AlreadyDownloaded: the refusal itself says the disk has the model. The
    // final file appears outside a download (a manual copy, a second instance).
    // Bite: list() preferring a transient Failed over the disk Downloaded, or the
    // Err branch of download() restoring the Failed after already_downloaded.
    let w = world(vec![Serve::Altered]);
    let mut first = start(&w, "base");
    let end = first.wait_state();
    let failed = LocalModelState::Failed {
        reason: DownloadFailure::ChecksumMismatch,
    };
    assert_eq!(
        end.event,
        LocalModelEvent::State {
            id: ModelId::Base,
            state: failed,
        },
        "precondition: the download failed"
    );

    put(&w.dir, &final_name(ModelId::Base), &model_bytes());
    assert!(
        w.models.store().is_downloaded("base"),
        "precondition: the store (settings validation) sees the copied model"
    );

    let mut expected = all_not_downloaded();
    expected[1].1 = LocalModelState::Downloaded;
    assert_eq!(
        states(&w.models),
        expected,
        "list() after the file appeared disagrees with store().is_downloaded"
    );

    let got = refused(&w, "base");
    assert_eq!(
        got,
        reason("already_downloaded", i18n::DOWNLOAD_ALREADY_DOWNLOADED, &[])
    );
    assert_eq!(
        states(&w.models),
        expected,
        "list() after the already_downloaded refusal"
    );
    assert!(w.models.store().is_downloaded("base"));
    assert_eq!(w.server.accepts(), 1, "the refused retry sent a request");
}

#[test]
fn not_enough_disk_space_is_refused_with_needed_before_any_request() {
    // R-9, contracts/ipc.md `not_enough_disk_space{needed}`: size + 1 %. Bite:
    // `needed` missing or not size + 1 %, a request sent, the dir created, the
    // state left Downloading.
    let w = world_on(
        vec![],
        FakeDisk::with_available(NEEDED - 1),
        five_entries,
        |_| {},
    );

    let got = refused(&w, "base");

    assert_eq!(
        got,
        reason(
            "not_enough_disk_space",
            i18n::DOWNLOAD_NOT_ENOUGH_DISK_SPACE,
            &[("needed", "66256")],
        )
    );
    assert_eq!(w.server.accepts(), 0, "a request was sent");
    assert!(!w.dir.exists(), "the models dir was created");
    assert_eq!(states(&w.models), all_not_downloaded());
}

#[test]
fn ids_outside_the_catalog_are_not_in_catalog_and_cannot_be_cancelled() {
    // An id string ModelId::parse rejects, and a valid id missing from the store's
    // catalog (DownloadError::NotInCatalog), are both `not_in_catalog`. Cancel of
    // either is false (the contract has no error for it). Bite: a parse failure
    // mapped to another code or a panic, a path-like id reaching the disk, a
    // request sent.
    let w = world_on(
        vec![],
        FakeDisk::with_available(10 * NEEDED),
        |server| {
            let file = file_name(ModelId::Base);
            catalog(vec![entry(ModelId::Base, file, &server.url(file))])
        },
        |_| {},
    );

    for id in ["medium", "", "BASE", "ggml-base.bin", "../base", "tiny"] {
        let got = refused(&w, id);
        assert_eq!(
            got,
            reason("not_in_catalog", i18n::DOWNLOAD_NOT_IN_CATALOG, &[]),
            "{id:?}"
        );
        assert!(!w.models.cancel(id), "cancel({id:?})");
    }
    assert_eq!(w.server.accepts(), 0);
    assert!(!w.dir.exists());
    assert!(!w.models.cancel("base"), "cancel with nothing running");
}

// ---- the one mapping to the wire codes ---------------------------------------------

/// Exhaustive on purpose: a new `DownloadError` does not compile here until its
/// wire code and message are decided (contracts/ipc.md).
fn expected_refusal(e: &DownloadError) -> ReasonView {
    match e {
        DownloadError::Busy => reason("download_busy", i18n::DOWNLOAD_BUSY, &[]),
        DownloadError::AlreadyDownloaded => {
            reason("already_downloaded", i18n::DOWNLOAD_ALREADY_DOWNLOADED, &[])
        }
        DownloadError::NotEnoughDiskSpace { needed } => ReasonView {
            code: "not_enough_disk_space",
            message_key: i18n::DOWNLOAD_NOT_ENOUGH_DISK_SPACE,
            params: vec![("needed", needed.to_string())],
        },
        DownloadError::NotInCatalog => reason("not_in_catalog", i18n::DOWNLOAD_NOT_IN_CATALOG, &[]),
        DownloadError::CannotStart => {
            reason("download_cannot_start", i18n::DOWNLOAD_CANNOT_START, &[])
        }
    }
}

/// Exhaustive on purpose, like [`expected_refusal`].
fn expected_failure(f: &DownloadFailure) -> ReasonView {
    match f {
        DownloadFailure::DownloadInterrupted => {
            reason("download_interrupted", i18n::DOWNLOAD_INTERRUPTED, &[])
        }
        DownloadFailure::ChecksumMismatch => {
            reason("checksum_mismatch", i18n::DOWNLOAD_CHECKSUM_MISMATCH, &[])
        }
        DownloadFailure::NotEnoughDiskSpace { needed } => ReasonView {
            code: "not_enough_disk_space",
            message_key: i18n::DOWNLOAD_NOT_ENOUGH_DISK_SPACE,
            params: vec![("needed", needed.to_string())],
        },
        DownloadFailure::SourceUnreachable { host } => ReasonView {
            code: "source_unreachable",
            message_key: i18n::DOWNLOAD_SOURCE_UNREACHABLE,
            params: vec![("host", host.clone())],
        },
        DownloadFailure::DiskError => reason("disk_error", i18n::DOWNLOAD_DISK_ERROR, &[]),
        DownloadFailure::HttpStatus { code } => ReasonView {
            code: "http_status",
            message_key: i18n::DOWNLOAD_HTTP_STATUS,
            params: vec![("code", code.to_string())],
        },
    }
}

fn all_refusals() -> Vec<DownloadError> {
    vec![
        DownloadError::Busy,
        DownloadError::AlreadyDownloaded,
        DownloadError::NotEnoughDiskSpace { needed: NEEDED },
        DownloadError::NotInCatalog,
        DownloadError::CannotStart,
    ]
}

fn all_failures() -> Vec<DownloadFailure> {
    vec![
        DownloadFailure::DownloadInterrupted,
        DownloadFailure::ChecksumMismatch,
        DownloadFailure::NotEnoughDiskSpace { needed: NEEDED },
        DownloadFailure::SourceUnreachable {
            host: "huggingface.co".to_string(),
        },
        DownloadFailure::DiskError,
        DownloadFailure::HttpStatus { code: 503 },
    ]
}

#[test]
fn every_download_error_maps_to_its_contract_code_and_message() {
    // contracts/ipc.md command errors + T-044's two new codes: download_busy,
    // already_downloaded, not_enough_disk_space{needed}, not_in_catalog,
    // download_cannot_start; each with its declared message id. Bite: a code or
    // message swapped, `needed` dropped, CannotStart mapped to busy.
    for e in all_refusals() {
        assert_eq!(ReasonView::from(&e), expected_refusal(&e), "{e:?}");
    }
}

#[test]
fn every_download_failure_maps_to_its_contract_code_and_message() {
    // The six failure codes come from DownloadFailure's own code() / message_id() /
    // message_params() (one mapping, P-010). Bite: a second table in the
    // coordinator that drifts from it, a param dropped.
    for f in all_failures() {
        let got = ReasonView::from(&f);
        assert_eq!(got, expected_failure(&f), "{f:?}");
        assert_eq!(got.code, f.code(), "{f:?}");
        assert_eq!(got.message_key, f.message_id(), "{f:?}");
        assert_eq!(got.params, f.message_params(), "{f:?}");
    }
}

#[test]
fn reason_params_are_omitted_when_empty() {
    // contracts/ipc.md `params?`: absent, not `{}` or null, when there is none.
    // Bite: `params: {}` or `params: null` on the wire.
    assert_eq!(
        wire(&ReasonView::from(&DownloadError::Busy)),
        json!({ "code": "download_busy", "messageKey": "download.busy" })
    );
    assert_eq!(
        wire(&ReasonView::from(&DownloadFailure::HttpStatus {
            code: 404
        })),
        json!({
            "code": "http_status",
            "messageKey": "download.http_status",
            "params": { "code": "404" },
        })
    );
}

// ---- events and the e2e wire fixture -------------------------------------------------

#[test]
fn event_names_are_the_contract_names() {
    // contracts/ipc.md events. Bite: a name spelled another way, the two swapped.
    assert_eq!(PROGRESS_EVENT, "local-model://progress");
    assert_eq!(STATE_EVENT, "local-model://state");
    let progress = LocalModelEvent::Progress {
        id: ModelId::Tiny,
        received: 1,
        total: 2,
    };
    let state = LocalModelEvent::State {
        id: ModelId::Tiny,
        state: LocalModelState::Downloaded,
    };
    assert_eq!(progress.name(), PROGRESS_EVENT);
    assert_eq!(state.name(), STATE_EVENT);
}

/// The Playwright mock's data for T-045 (`e2e/support/tauriMock.ts`).
const E2E_WIRE_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../e2e/fixtures/local-models-wire.json"
));

#[test]
fn e2e_local_models_wire_fixture_matches_core() {
    // P-010: the e2e mock holds no hand copy of the catalog or the wire shapes.
    // Bite: a field renamed (`name_key` instead of `nameKey`), a state kind spelled
    // another way, a code or message key changed, the production catalog changed,
    // without regenerating e2e/fixtures/local-models-wire.json.
    let fixture: Value =
        serde_json::from_str(E2E_WIRE_FIXTURE).expect("local-models-wire.json is valid JSON");
    let tmp = TempDir::new();
    let probe: Arc<dyn DiskSpace> = FakeDisk::with_available(u64::MAX);
    let (models, _) = LocalModels::open(tmp.path().join("models"), probe, timeouts(), MODELS);

    let mut core = serde_json::Map::new();
    core.insert(
        "event_names".into(),
        json!({ "progress": PROGRESS_EVENT, "state": STATE_EVENT }),
    );
    core.insert("list_first_run".into(), wire(&models.list()));
    core.insert(
        "states".into(),
        json!({
            "not_downloaded": wire(&LocalModelState::NotDownloaded),
            "downloading": wire(&LocalModelState::Downloading { received: 32_800, total: SIZE }),
            "downloaded": wire(&LocalModelState::Downloaded),
            "failed": wire(&LocalModelState::Failed { reason: DownloadFailure::ChecksumMismatch }),
        }),
    );
    core.insert(
        "events".into(),
        json!({
            "progress": wire(&LocalModelEvent::Progress {
                id: ModelId::Base,
                received: 32_800,
                total: SIZE,
            }),
            "state": wire(&LocalModelEvent::State {
                id: ModelId::Base,
                state: LocalModelState::Downloaded,
            }),
        }),
    );
    let mut reasons = serde_json::Map::new();
    for f in all_failures() {
        reasons.insert(f.code().to_string(), wire(&ReasonView::from(&f)));
    }
    for e in all_refusals() {
        let r = ReasonView::from(&e);
        let value = wire(&r);
        if let Some(same) = reasons.get(r.code) {
            assert_eq!(&value, same, "{e:?} and the failure with code {}", r.code);
        }
        reasons.insert(r.code.to_string(), value);
    }
    core.insert("reasons".into(), Value::Object(reasons));

    for (key, value) in &core {
        assert_eq!(
            &fixture[key.as_str()],
            value,
            "e2e/fixtures/local-models-wire.json {key} differs from core; core says:\n{}",
            serde_json::to_string_pretty(value).expect("serializes")
        );
    }
    let keys: BTreeSet<&str> = fixture
        .as_object()
        .expect("fixture is an object")
        .keys()
        .map(String::as_str)
        .filter(|k| *k != "_format")
        .collect();
    assert_eq!(
        keys,
        core.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        "fixture keys"
    );
}

#[test]
fn the_store_of_the_coordinator_is_a_model_store_over_the_models_dir() {
    // The shell passes store() to load_settings (SettingsDeps.local_models): it must
    // be the store over this dir and catalog. Bite: store() over another dir or the
    // production catalog in a test, or a new store per call.
    let w = world_on(
        vec![],
        FakeDisk::with_available(10 * NEEDED),
        five_entries,
        |dir| put(dir, &final_name(ModelId::Small), &model_bytes()),
    );
    let store: Arc<ModelStore> = w.models.store();

    assert_eq!(store.dir(), w.dir.as_path());
    assert!(std::ptr::eq(store.catalog(), w.catalog));
    assert!(Arc::ptr_eq(&store, &w.models.store()));
    assert!(store.is_downloaded("small"));
}
