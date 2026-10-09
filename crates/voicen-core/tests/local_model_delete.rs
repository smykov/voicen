//! T-019 (spec 002 US4, FR-021–FR-023; FR-28 deletion part): the "model-delete
//! tests" of the Acceptance. `voicen_core::local_models::service::LocalModels::delete`
//! is the one delete path (decision #57, option A of docs/tasks/T-019.md):
//! refuse if in use, release, remove, clear the transient state, reset the selection.
//!
//! Pinned API (docs/tasks/T-019.md `## Tests`), all in
//! `voicen_core::local_models::service`:
//! - `LocalModels::delete(&self, id: &str, residency: &dyn ModelRelease,
//!   settings: &SettingsService) -> Result<DeleteOutcome, ReasonView>`;
//! - `pub struct DeleteOutcome { pub engine_reset: bool, pub reset_failed: bool }`
//!   (`Debug + PartialEq`), wire `{ "engineReset", "resetFailed" }` (OQ-26 (a));
//! - `pub enum DeleteError { ModelInUse, NotDownloaded, DeleteFailed }` with
//!   `From<&DeleteError> for ReasonView`: codes `model_in_use`, `not_downloaded`,
//!   `delete_failed` (contracts/ipc.md:39), message keys `delete.model_in_use`,
//!   `delete.not_downloaded`, `delete.failed`, each a declared message id;
//! - `pub trait ModelRelease { fn release_for_delete(&self, id: ModelId) ->
//!   Result<ReleaseGuard, InUse>; }` (the guard may carry the `&self` lifetime),
//!   `pub struct InUse;`, `ReleaseGuard::new(held)` taking any `Send + 'static`
//!   value that is dropped with the guard, and `pub struct NoResidency;` (the
//!   production value until T-017: never in use).
//!
//! The settings side goes through the real `SettingsService` over core's
//! `FakeSettingsFile` / `FakeCredentialStore`, with the coordinator's own store as
//! its `DownloadedModels` (P-010). `delete_failed` is proven by the unit test
//! `local_models::service::delete_tests` (an injected removal error; the gate runs
//! as uid 0, so no permission trick makes `remove_file` fail) and by the Windows CI
//! lock test in `src-tauri/tests/local_models.rs`. Fake data only: 127.0.0.1.

mod common;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use common::timing::{deadline, left};
use serde_json::{json, Value};
use voicen_core::autostart::FakeAutostart;
use voicen_core::clock::FakeClock;
use voicen_core::hotkey_registrar::FakeHotkeyRegistrar;
use voicen_core::i18n::MESSAGE_IDS;
use voicen_core::local_models::catalog::{CatalogEntry, ModelId};
use voicen_core::local_models::download::{DiskSpace, DownloadFailure};
use voicen_core::local_models::service::{
    DeleteError, DeleteOutcome, InUse, LocalModelEvent, LocalModels, ModelRelease, NoResidency,
    ReasonView, ReleaseGuard,
};
use voicen_core::local_models::store::LocalModelState;
use voicen_core::models::DownloadedModels;
use voicen_core::secrets::{CredentialError, CredentialOp, FakeCredentialStore, KeySlot};
use voicen_core::settings::file::{FakeSettingsFile, FileCall};
use voicen_core::settings::service::{SettingsDeps, SettingsService};
use voicen_core::settings::{defaults, EngineKind, LoadOutcome, Settings};
use voicen_core::test_support::local_models::{
    dir_entries, five_entries, model_bytes, FakeDisk, Serve, Server, NEEDED,
};
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;

const OS: Option<&str> = Some("en-US");
/// Long enough for any local end event; a hang fails the test instead of the run.
const END_WAIT: Duration = Duration::from_secs(10);

fn timeouts() -> Timeouts {
    Timeouts {
        connect: Duration::from_secs(2),
        download_no_data: Duration::from_secs(5),
        ..Timeouts::default()
    }
}

// ---- harness ------------------------------------------------------------------

/// The coordinator over a temp models dir with the five fake models, and the
/// settings service over an in-memory file, both as `run()` wires them.
struct World {
    _tmp: TempDir,
    dir: PathBuf,
    _server: Server,
    models: Arc<LocalModels>,
    file: Arc<FakeSettingsFile>,
    creds: Arc<FakeCredentialStore>,
    service: SettingsService,
    /// Subscribed right after the load: every publish of the delete.
    changes: Receiver<Arc<Settings>>,
    /// The settings file bytes right after the load.
    bytes_before: Option<Vec<u8>>,
}

/// `on_disk` are written under their final names before `open`; `stored` is the
/// settings file (`None` = a read error, i.e. `Unavailable`).
fn world(plan: Vec<Serve>, on_disk: &[ModelId], stored: Option<&Settings>) -> World {
    world_with_creds(plan, on_disk, stored, FakeCredentialStore::new())
}

fn world_with_creds(
    plan: Vec<Serve>,
    on_disk: &[ModelId],
    stored: Option<&Settings>,
    creds: FakeCredentialStore,
) -> World {
    let tmp = TempDir::new();
    let dir = tmp.path().join("models");
    for id in on_disk {
        put(&dir, &final_name(*id), &model_bytes());
    }
    let server = Server::start(plan);
    let catalog: &'static [CatalogEntry] = five_entries(&server);
    let probe: Arc<dyn DiskSpace> = FakeDisk::with_available(10 * NEEDED);
    let (models, cleanup) = LocalModels::open(dir.clone(), probe, timeouts(), catalog);
    if let Err(e) = cleanup {
        panic!("cleanup_at_start failed at open: {e}");
    }
    let models = Arc::new(models);
    let file = Arc::new(match stored {
        Some(settings) => {
            FakeSettingsFile::with_bytes(&serde_json::to_vec(settings).expect("serialize"))
        }
        None => {
            let file = FakeSettingsFile::new();
            file.fail_read(io::ErrorKind::PermissionDenied);
            file
        }
    });
    let creds = Arc::new(creds);
    let deps = SettingsDeps {
        file: file.clone(),
        credentials: creds.clone(),
        autostart: Arc::new(FakeAutostart::new()),
        hotkeys: Arc::new(FakeHotkeyRegistrar::new()),
        local_models: models.store(),
        clock: Arc::new(FakeClock::at(
            UNIX_EPOCH + Duration::from_secs(1_709_251_199),
        )),
    };
    let (service, outcome) = SettingsService::load_or_init(deps, OS);
    match (stored, &outcome) {
        (Some(_), LoadOutcome::Loaded(_)) | (None, LoadOutcome::Unavailable(_)) => {}
        _ => panic!("precondition: unexpected load outcome {outcome:?}"),
    }
    let changes = service.subscribe();
    let bytes_before = file.bytes();
    World {
        _tmp: tmp,
        dir,
        _server: server,
        models,
        file,
        creds,
        service,
        changes,
        bytes_before,
    }
}

fn put(dir: &Path, name: &str, bytes: &[u8]) {
    std::fs::create_dir_all(dir).expect("create models dir");
    std::fs::write(dir.join(name), bytes).unwrap_or_else(|e| panic!("write {name}: {e}"));
}

fn final_name(id: ModelId) -> String {
    format!("ggml-{}.bin", id.as_str())
}

/// `defaults` with `engine` and `builtin_local.model_id`.
fn settings(engine: EngineKind, model_id: Option<&str>) -> Settings {
    let mut s = defaults(OS);
    s.engine = engine;
    s.builtin_local.model_id = model_id.map(str::to_string);
    s
}

impl World {
    fn path(&self, id: ModelId) -> PathBuf {
        self.dir.join(final_name(id))
    }

    fn state_of(&self, id: ModelId) -> LocalModelState {
        self.models
            .list()
            .into_iter()
            .find(|v| v.id == id)
            .unwrap_or_else(|| panic!("{id:?} not listed"))
            .state
    }

    /// The settings as persisted now (parsed from the fake file's bytes).
    fn persisted(&self) -> Settings {
        let bytes = self.file.bytes().expect("a settings file");
        serde_json::from_slice(&bytes).expect("persisted settings parse")
    }

    /// Every snapshot published since the load.
    fn published(&self) -> Vec<Settings> {
        let mut got = Vec::new();
        loop {
            match self.changes.try_recv() {
                Ok(s) => got.push((*s).clone()),
                Err(TryRecvError::Empty) => return got,
                Err(TryRecvError::Disconnected) => panic!("subscriber dropped"),
            }
        }
    }

    /// Nothing about the settings changed: the file was read once at the load and
    /// never written, its bytes are the same, nothing was published, the snapshot
    /// is `expected`, and no credential call was made.
    #[track_caller]
    fn assert_settings_untouched(&self, expected: &Settings) {
        assert_eq!(
            self.file.calls(),
            vec![FileCall::Read],
            "settings file calls"
        );
        assert_eq!(self.file.bytes(), self.bytes_before, "settings file bytes");
        assert_eq!(
            self.published(),
            Vec::<Settings>::new(),
            "published snapshots"
        );
        assert_eq!(&*self.service.snapshot(), expected, "snapshot in force");
        assert_eq!(self.creds.calls(), vec![], "credential calls");
    }
}

// ---- fake residency -----------------------------------------------------------------

/// What the fake residency saw, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    /// `release_for_delete(id)`; whether the final file of `id` existed then.
    Released { id: ModelId, file_present: bool },
    /// The guard was dropped; whether the final file existed then.
    GuardDropped { file_present: bool },
}

/// A `ModelRelease` that either holds the model (`InUse`) or releases it and hands
/// out a guard whose drop it records.
struct FakeResidency {
    dir: PathBuf,
    in_use: bool,
    steps: Arc<Mutex<Vec<Step>>>,
}

impl FakeResidency {
    fn new(dir: &Path, in_use: bool) -> FakeResidency {
        FakeResidency {
            dir: dir.to_path_buf(),
            in_use,
            steps: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn steps(&self) -> Vec<Step> {
        self.steps.lock().expect("steps").clone()
    }
}

struct DropProbe {
    path: PathBuf,
    steps: Arc<Mutex<Vec<Step>>>,
}

impl Drop for DropProbe {
    fn drop(&mut self) {
        let file_present = self.path.exists();
        self.steps
            .lock()
            .expect("steps")
            .push(Step::GuardDropped { file_present });
    }
}

impl ModelRelease for FakeResidency {
    fn release_for_delete(&self, id: ModelId) -> Result<ReleaseGuard, InUse> {
        let path = self.dir.join(final_name(id));
        self.steps.lock().expect("steps").push(Step::Released {
            id,
            file_present: path.exists(),
        });
        if self.in_use {
            return Err(InUse);
        }
        Ok(ReleaseGuard::new(DropProbe {
            path,
            steps: Arc::clone(&self.steps),
        }))
    }
}

fn ok(engine_reset: bool, reset_failed: bool) -> Result<DeleteOutcome, ReasonView> {
    Ok(DeleteOutcome {
        engine_reset,
        reset_failed,
    })
}

fn wire<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("serializes")
}

// ---- happy path ---------------------------------------------------------------------

#[test]
fn deleting_an_unselected_downloaded_model_removes_its_file_and_lists_it_not_downloaded() {
    // Acceptance 1 ("removes the file and frees the space"). Through `NoResidency`,
    // the production value until T-017. The saved selection names another model, so
    // the settings are not touched at all. Bite: no remove_file (still downloaded),
    // the wrong file removed (`base` gone), a write of unchanged settings, a publish,
    // engineReset hard-coded true, NoResidency answering in use.
    let stored = settings(EngineKind::BuiltinLocal, Some("base"));
    let w = world(vec![], &[ModelId::Base, ModelId::Small], Some(&stored));

    let got = w.models.delete("small", &NoResidency, &w.service);

    assert_eq!(got, ok(false, false));
    assert!(
        !w.path(ModelId::Small).exists(),
        "the file of small is still there"
    );
    assert_eq!(
        dir_entries(&w.dir),
        vec![final_name(ModelId::Base)],
        "only small's file is removed"
    );
    assert_eq!(w.state_of(ModelId::Small), LocalModelState::NotDownloaded);
    assert_eq!(w.state_of(ModelId::Base), LocalModelState::Downloaded);
    assert!(!w.models.store().is_downloaded("small"));
    assert!(w.models.store().is_downloaded("base"));
    w.assert_settings_untouched(&stored);
}

// ---- failure branch: the selected model ----------------------------------------------

#[test]
fn deleting_the_selected_model_sets_engine_none_and_clears_the_model_persisted_and_published() {
    // Acceptance 2, first half (spec US4 scenario 2): engine builtin_local with
    // `small`, delete `small` → engine none, model_id null, in the bytes written and
    // in the snapshot published (the shell's change bridge emits settings://changed
    // from it), engineReset true. Every other field is kept (a narrow reset, not a
    // save of a default). Bite: no reset, the reset not persisted (snapshot only),
    // persisted but not published, the engine kept, model_id kept, engineReset
    // false, other fields reset to defaults.
    let mut stored = settings(EngineKind::BuiltinLocal, Some("small"));
    stored.hotkey = "Ctrl+Shift+F9".into();
    stored.history.size = 7;
    stored.api.model = "whisper-test".into();
    let w = world(vec![], &[ModelId::Small], Some(&stored));

    let got = w.models.delete("small", &NoResidency, &w.service);

    assert_eq!(got, ok(true, false));
    assert!(!w.path(ModelId::Small).exists());
    assert_eq!(w.state_of(ModelId::Small), LocalModelState::NotDownloaded);
    let mut expected = stored.clone();
    expected.engine = EngineKind::None;
    expected.builtin_local.model_id = None;
    assert_eq!(w.persisted(), expected, "persisted settings");
    assert_eq!(
        wire(&w.persisted())["builtin_local"]["model_id"],
        Value::Null,
        "model_id on disk"
    );
    assert_eq!(&*w.service.snapshot(), &expected, "snapshot in force");
    assert_eq!(w.published(), vec![expected], "one publish of the reset");
    assert_eq!(
        w.file.calls(),
        vec![FileCall::Read, FileCall::WriteAtomic],
        "one write"
    );
}

#[test]
fn the_selection_reset_neither_reads_keys_nor_revalidates_the_stored_settings() {
    // Rejected option C of the analysis: a reset through `save` refuses when a
    // credential read fails and when a hand-edited field is invalid. Here the
    // credential store fails every read and the stored history size is out of range;
    // the reset must still land. Bite: forget_model implemented via save (Refused →
    // nothing written), or reading the key slots.
    let mut stored = settings(EngineKind::BuiltinLocal, Some("small"));
    stored.history.size = 0;
    let creds = FakeCredentialStore::new();
    for slot in KeySlot::all() {
        creds.fail(CredentialOp::Read, slot, CredentialError { os_code: 1312 });
    }
    let w = world_with_creds(vec![], &[ModelId::Small], Some(&stored), creds);

    let got = w.models.delete("small", &NoResidency, &w.service);

    assert_eq!(got, ok(true, false));
    let mut expected = stored.clone();
    expected.engine = EngineKind::None;
    expected.builtin_local.model_id = None;
    assert_eq!(w.persisted(), expected);
    assert_eq!(&*w.service.snapshot(), &expected);
    assert_eq!(w.creds.calls(), vec![], "credential calls");
}

#[test]
fn deleting_the_saved_builtin_model_while_another_engine_is_active_clears_only_the_model() {
    // OQ-25 proposal (a): engine api with builtin_local.model_id = small → model_id
    // cleared, engine stays api, engineReset false; persisted and published. Bite:
    // the engine set to none whenever model_id matches, model_id left naming a
    // missing model, engineReset true.
    let stored = settings(EngineKind::Api, Some("small"));
    let w = world(vec![], &[ModelId::Small], Some(&stored));

    let got = w.models.delete("small", &NoResidency, &w.service);

    assert_eq!(got, ok(false, false));
    assert!(!w.path(ModelId::Small).exists());
    let mut expected = stored.clone();
    expected.builtin_local.model_id = None;
    assert_eq!(w.persisted(), expected, "persisted settings");
    assert_eq!(&*w.service.snapshot(), &expected);
    assert_eq!(w.published(), vec![expected]);
}

// ---- failure branch: the loaded model ------------------------------------------------

#[test]
fn the_loaded_model_is_released_before_its_file_is_removed_and_the_guard_drops_after() {
    // Acceptance 2, second half ("deleting the loaded model unloads it first"): the
    // residency is asked while the file is still on disk, its guard (which blocks a
    // reload of `small` until the delete is over) is dropped after the removal and
    // before delete returns. Bite: removal before the release (file_present false at
    // Released), the residency never asked, the guard dropped before the removal
    // (file_present true at GuardDropped), the guard leaked (no GuardDropped), the
    // release asked for another id.
    let stored = settings(EngineKind::BuiltinLocal, Some("small"));
    let w = world(vec![], &[ModelId::Small], Some(&stored));
    let residency = FakeResidency::new(&w.dir, false);

    let got = w.models.delete("small", &residency, &w.service);

    assert_eq!(got, ok(true, false));
    assert_eq!(
        residency.steps(),
        vec![
            Step::Released {
                id: ModelId::Small,
                file_present: true,
            },
            Step::GuardDropped {
                file_present: false,
            },
        ]
    );
    assert!(!w.path(ModelId::Small).exists());
}

#[test]
fn a_model_in_use_is_refused_with_model_in_use_and_nothing_changes() {
    // Spec US4 scenario 3: a transcription holds the model → `model_in_use`; the
    // file, the list and the settings are unchanged. Bite: the file removed anyway,
    // the settings reset anyway, another code, Ok.
    let stored = settings(EngineKind::BuiltinLocal, Some("small"));
    let w = world(vec![], &[ModelId::Small], Some(&stored));
    let residency = FakeResidency::new(&w.dir, true);

    let got = w.models.delete("small", &residency, &w.service);

    assert_eq!(got, Err(ReasonView::from(&DeleteError::ModelInUse)));
    assert_eq!(got.as_ref().map_err(|r| r.code), Err("model_in_use"));
    assert_eq!(
        residency.steps(),
        vec![Step::Released {
            id: ModelId::Small,
            file_present: true,
        }]
    );
    assert!(w.path(ModelId::Small).exists(), "the file was removed");
    assert_eq!(w.state_of(ModelId::Small), LocalModelState::Downloaded);
    assert!(w.models.store().is_downloaded("small"));
    w.assert_settings_untouched(&stored);
}

// ---- not_downloaded -------------------------------------------------------------------

#[test]
fn a_model_not_on_disk_is_not_downloaded_and_the_residency_is_not_asked() {
    // contracts/ipc.md `not_downloaded`. Also a file under the final name with the
    // wrong size: the store says not downloaded, and it is never deleted (store.rs).
    // Bite: Ok for a missing file (NotFound taken as removed before the check), the
    // wrong-size file removed, the residency asked (a model unloaded for nothing),
    // the settings reset for a model that was never there.
    let stored = settings(EngineKind::BuiltinLocal, Some("small"));
    let w = world(vec![], &[], Some(&stored));
    put(&w.dir, &final_name(ModelId::Base), &[7u8; 100]);
    let residency = FakeResidency::new(&w.dir, false);

    let missing = w.models.delete("small", &residency, &w.service);
    let wrong_size = w.models.delete("base", &residency, &w.service);

    let not_downloaded = Err(ReasonView::from(&DeleteError::NotDownloaded));
    assert_eq!(missing, not_downloaded);
    assert_eq!(wrong_size, not_downloaded);
    assert_eq!(missing.map_err(|r| r.code), Err("not_downloaded"));
    assert_eq!(residency.steps(), vec![], "the residency was asked");
    assert_eq!(
        std::fs::read(w.path(ModelId::Base)).expect("wrong-size file kept"),
        vec![7u8; 100]
    );
    w.assert_settings_untouched(&stored);
}

#[test]
fn an_unknown_id_is_not_downloaded() {
    // contracts/ipc.md lists no `not_in_catalog` for delete: an id that is not a
    // ModelId string is `not_downloaded`. Bite: not_in_catalog, a panic, Ok.
    let stored = settings(EngineKind::BuiltinLocal, Some("small"));
    let w = world(vec![], &[ModelId::Small], Some(&stored));
    let residency = FakeResidency::new(&w.dir, false);

    for id in ["medium", "", "SMALL", "../ggml-small.bin"] {
        let got = w.models.delete(id, &residency, &w.service);
        assert_eq!(
            got,
            Err(ReasonView::from(&DeleteError::NotDownloaded)),
            "{id:?}"
        );
    }
    assert_eq!(residency.steps(), vec![]);
    assert!(w.path(ModelId::Small).exists());
    w.assert_settings_untouched(&stored);
}

#[test]
fn a_model_being_downloaded_is_not_downloaded_and_its_download_goes_on() {
    // A `Downloading` model has no final file yet: `not_downloaded`, and the
    // running download is neither cancelled nor its `.part` removed. Bite: a delete
    // that cancels the download or removes the `.part` (the end is not Downloaded),
    // Ok, the residency asked.
    let stored = settings(EngineKind::None, None);
    let w = world(
        vec![Serve::Trickle {
            chunk: 1024,
            every: Duration::from_millis(50),
        }],
        &[],
        Some(&stored),
    );
    let (tx, rx) = channel();
    w.models
        .download("base", move |event: LocalModelEvent| {
            let _ = tx.send(event);
        })
        .unwrap_or_else(|e| panic!("download refused: {e:?}"));
    match rx.recv_timeout(END_WAIT) {
        Ok(LocalModelEvent::Progress { .. }) => {}
        other => panic!("precondition: no progress first: {other:?}"),
    }
    let residency = FakeResidency::new(&w.dir, false);

    let got = w.models.delete("base", &residency, &w.service);

    assert_eq!(got, Err(ReasonView::from(&DeleteError::NotDownloaded)));
    assert_eq!(residency.steps(), vec![]);
    assert!(
        matches!(
            w.state_of(ModelId::Base),
            LocalModelState::Downloading { .. }
        ) || w.state_of(ModelId::Base) == LocalModelState::Downloaded,
        "the delete changed the running download: {:?}",
        w.state_of(ModelId::Base)
    );
    let end = deadline(END_WAIT);
    let last = loop {
        match rx.recv_timeout(left(end)) {
            Ok(LocalModelEvent::State { state, .. }) => break state,
            Ok(LocalModelEvent::Progress { .. }) => continue,
            Err(e) => panic!("no end event: {e:?}"),
        }
    };
    assert_eq!(last, LocalModelState::Downloaded, "the download's end");
    assert!(w.models.store().is_downloaded("base"));
    w.assert_settings_untouched(&stored);
}

// ---- transient state --------------------------------------------------------------------

#[test]
fn a_failed_hidden_by_a_disk_download_does_not_come_back_after_the_delete() {
    // model-download.md "Exception": a transient Failed is hidden while the disk has
    // the model. The delete clears it: afterwards the model lists not_downloaded,
    // not failed. No list() runs between the copy and the delete (list() itself
    // drops the stale Failed, which would make this toothless). Bite: delete without
    // the transient removal.
    let stored = settings(EngineKind::None, None);
    let w = world(vec![Serve::Altered], &[], Some(&stored));
    let (tx, rx) = channel();
    w.models
        .download("base", move |event: LocalModelEvent| {
            let _ = tx.send(event);
        })
        .unwrap_or_else(|e| panic!("download refused: {e:?}"));
    let end = deadline(END_WAIT);
    let state = loop {
        match rx.recv_timeout(left(end)) {
            Ok(LocalModelEvent::State { state, .. }) => break state,
            Ok(LocalModelEvent::Progress { .. }) => continue,
            Err(e) => panic!("no end event: {e:?}"),
        }
    };
    assert_eq!(
        state,
        LocalModelState::Failed {
            reason: DownloadFailure::ChecksumMismatch,
        },
        "precondition: the download failed"
    );
    put(&w.dir, &final_name(ModelId::Base), &model_bytes());

    let got = w.models.delete("base", &NoResidency, &w.service);

    assert_eq!(got, ok(false, false));
    assert_eq!(w.state_of(ModelId::Base), LocalModelState::NotDownloaded);
}

// ---- settings side failures ----------------------------------------------------------------

#[test]
fn a_settings_write_failure_after_the_removal_is_success_with_reset_failed() {
    // OQ-26 proposal (a): the file is already gone, so the delete reports success
    // with resetFailed true; the snapshot stays (engine builtin_local on a missing
    // model, FR-07's "no local model" applies at the next hotkey), nothing is
    // published, engineReset is false (the engine did not become none). Bite:
    // delete_failed reported, resetFailed false, the snapshot swapped or published
    // although the write failed, the removal undone or skipped.
    let stored = settings(EngineKind::BuiltinLocal, Some("small"));
    let w = world(vec![], &[ModelId::Small], Some(&stored));
    w.file.fail_write(io::ErrorKind::PermissionDenied);

    let got = w.models.delete("small", &NoResidency, &w.service);

    assert_eq!(got, ok(false, true));
    assert!(!w.path(ModelId::Small).exists(), "the removal was undone");
    assert_eq!(w.state_of(ModelId::Small), LocalModelState::NotDownloaded);
    assert_eq!(&*w.service.snapshot(), &stored, "snapshot in force");
    assert_eq!(w.published(), Vec::<Settings>::new(), "published");
    assert_eq!(w.file.bytes(), w.bytes_before, "settings bytes");
    assert_eq!(
        w.file.calls(),
        vec![FileCall::Read, FileCall::WriteAtomic],
        "the reset was attempted once"
    );
}

#[test]
fn unavailable_settings_are_not_touched_and_the_delete_still_succeeds() {
    // Decision #19: while Unavailable no call reaches the file or the credential
    // store. The delete itself does not depend on the settings. Bite: a refusal
    // because the settings are unavailable, a write of the in-memory defaults, a
    // credential read, resetFailed reported for a reset that was not needed.
    let w = world(vec![], &[ModelId::Small], None);

    let got = w.models.delete("small", &NoResidency, &w.service);

    assert_eq!(got, ok(false, false));
    assert!(!w.path(ModelId::Small).exists());
    assert_eq!(w.file.calls(), vec![FileCall::Read], "settings file calls");
    assert_eq!(w.creds.calls(), vec![], "credential calls");
    assert_eq!(w.published(), Vec::<Settings>::new());
}

// ---- wire ------------------------------------------------------------------------------------

#[test]
fn delete_outcome_and_refusals_have_the_contract_wire_form() {
    // contracts/ipc.md:39 plus OQ-26 (a): `{ engineReset, resetFailed }`; the three
    // refusals are FailureReasons with their contract codes and a declared message
    // id each (the UI shows it through `asFailureReason`). Bite: snake_case field
    // names, a missing resetFailed, a code spelled another way, a message key the UI
    // cannot render, two codes sharing a message.
    assert_eq!(
        wire(&DeleteOutcome {
            engine_reset: true,
            reset_failed: false,
        }),
        json!({ "engineReset": true, "resetFailed": false })
    );
    assert_eq!(
        wire(&DeleteOutcome {
            engine_reset: false,
            reset_failed: true,
        }),
        json!({ "engineReset": false, "resetFailed": true })
    );
    let expected = [
        (
            DeleteError::ModelInUse,
            "model_in_use",
            "delete.model_in_use",
        ),
        (
            DeleteError::NotDownloaded,
            "not_downloaded",
            "delete.not_downloaded",
        ),
        (DeleteError::DeleteFailed, "delete_failed", "delete.failed"),
    ];
    for (error, code, key) in expected {
        let reason = ReasonView::from(&error);
        assert_eq!(
            wire(&reason),
            json!({ "code": code, "messageKey": key }),
            "{error:?}"
        );
        assert!(
            MESSAGE_IDS.contains(&reason.message_key),
            "{key} is not a declared message id"
        );
    }
}
