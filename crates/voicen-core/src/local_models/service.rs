//! The local-models coordinator (T-044, decision #57 option A; spec 002
//! contracts/ipc.md "local_models_list", "local_model_download",
//! "local_model_cancel_download", events `local-model://progress|state`).
//!
//! Invariant (T-044 analysis): one [`LocalModels`] per process, made only by
//! [`LocalModels::open`], which runs `ModelStore::cleanup_at_start` before it
//! returns. Its one `Arc<ModelStore>` ([`LocalModels::store`]) is the settings
//! validation's `DownloadedModels` and the store behind every local-model command.
//! Only `Downloader::start` begins a download. Every [`LocalModelEvent`] is passed to
//! the caller's `emit` only from the download thread, after the in-memory state
//! is updated and with no lock held, so a [`LocalModels::list`] after an event
//! agrees with it. Every refusal leaves as a [`ReasonView`] with a contracts/ipc.md
//! code: `download_busy`, `already_downloaded`, `not_enough_disk_space{needed}`,
//! `not_in_catalog`, `download_cannot_start`; and for a delete `model_in_use`,
//! `not_downloaded`, `delete_failed`.
//!
//! Delete (T-019, option A): [`LocalModels::delete`] is the one delete path, in
//! one order: refuse unless downloaded, release through the [`ModelRelease`] port
//! (refuse if in use), remove the final file, clear the transient state, reset the
//! selection through `SettingsService::forget_model`, drop the release guard. It
//! emits no event: the settings window re-lists after the command settles.
//!
//! The shell (`src-tauri/src/local_models.rs`) is an adapter: it parses nothing,
//! delegates the commands here and emits each event to the settings window.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::ser::{SerializeMap, SerializeStruct};
use serde::Serialize;

use super::catalog::{CatalogEntry, ModelId};
use super::download::{DiskSpace, DownloadError, DownloadEvent, DownloadFailure, Downloader};
use super::store::{LocalModelState, ModelStore};
use crate::i18n::{self, MessageId};
use crate::settings::service::SettingsService;
use crate::timeouts::Timeouts;

/// `local-model://progress`, payload `{ id, received, total }`.
pub const PROGRESS_EVENT: &str = "local-model://progress";
/// `local-model://state`, payload `{ id, state: ModelState }`.
pub const STATE_EVENT: &str = "local-model://state";

/// One entry of `local_models_list` (contracts/ipc.md `LocalModelView`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalModelView {
    pub id: ModelId,
    /// `local_model.name.<id>`.
    pub name_key: MessageId,
    pub size_bytes: u64,
    pub recommended: bool,
    pub state: LocalModelState,
    /// Always `false` until T-017's residency exists.
    pub loaded: bool,
}

/// contracts/ipc.md `FailureReason { code, messageKey, params? }`: the reason of a
/// `failed` state and the rejection of a refused `local_model_download`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasonView {
    pub code: &'static str,
    pub message_key: MessageId,
    pub params: Vec<(&'static str, String)>,
}

/// A local-model event for the settings window; [`name`](Self::name) is the event
/// name, the serialized value its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalModelEvent {
    Progress {
        id: ModelId,
        received: u64,
        total: u64,
    },
    State {
        id: ModelId,
        state: LocalModelState,
    },
}

impl LocalModelEvent {
    /// [`PROGRESS_EVENT`] or [`STATE_EVENT`].
    pub fn name(&self) -> &'static str {
        match self {
            LocalModelEvent::Progress { .. } => PROGRESS_EVENT,
            LocalModelEvent::State { .. } => STATE_EVENT,
        }
    }
}

/// `local_model.name.<id>`: the declared message id of a model's display name.
fn name_key(id: ModelId) -> MessageId {
    match id {
        ModelId::Tiny => i18n::LOCAL_MODEL_NAME_TINY,
        ModelId::Base => i18n::LOCAL_MODEL_NAME_BASE,
        ModelId::Small => i18n::LOCAL_MODEL_NAME_SMALL,
        ModelId::MediumQ5_0 => i18n::LOCAL_MODEL_NAME_MEDIUM_Q5_0,
        ModelId::LargeV3TurboQ5_0 => i18n::LOCAL_MODEL_NAME_LARGE_V3_TURBO_Q5_0,
    }
}

/// The resolved value of `local_model_delete` (contracts/ipc.md, OQ-26 (a)), wire
/// `{ engineReset, resetFailed }`. `engine_reset`: the engine was `builtin_local` on
/// the deleted model and is now `none`, persisted and published. `reset_failed`: the
/// file is gone but the selection naming it could not be written; the settings in
/// force are unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteOutcome {
    pub engine_reset: bool,
    pub reset_failed: bool,
}

/// A refused `local_model_delete`; nothing changed on disk or in the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteError {
    /// The residency holds the model for a transcription (wire `model_in_use`).
    ModelInUse,
    /// No final file with the catalog size, a running download, or an id outside
    /// the catalog (wire `not_downloaded`).
    NotDownloaded,
    /// The removal failed with an error other than `NotFound`, e.g. a file held
    /// open (wire `delete_failed`); the model stays downloaded.
    DeleteFailed,
}

/// The residency answered that a transcription holds the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InUse;

/// Held by [`LocalModels::delete`] from the release until after the removal and
/// the selection reset; while it lives the residency must not load the model again.
/// Dropping it ends that (the held value is dropped with it).
pub struct ReleaseGuard {
    _held: Box<dyn Send>,
}

impl ReleaseGuard {
    /// A guard that drops `held` when it is dropped.
    pub fn new(held: impl Send + 'static) -> ReleaseGuard {
        ReleaseGuard {
            _held: Box::new(held),
        }
    }
}

/// The port from the delete to whatever keeps a model loaded (T-017's residency).
pub trait ModelRelease {
    /// Unloads `id` if it is loaded and blocks its loading until the guard drops;
    /// `InUse` when a transcription holds it (nothing is unloaded then).
    fn release_for_delete(&self, id: ModelId) -> Result<ReleaseGuard, InUse>;
}

/// The production [`ModelRelease`] until T-017: nothing is ever loaded or in use.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoResidency;

impl ModelRelease for NoResidency {
    fn release_for_delete(&self, _id: ModelId) -> Result<ReleaseGuard, InUse> {
        Ok(ReleaseGuard::new(()))
    }
}

/// The in-memory `Downloading` / `Failed` states by model; a model without an entry
/// shows its disk state.
type Transient = Arc<Mutex<HashMap<ModelId, LocalModelState>>>;

fn lock(
    transient: &Mutex<HashMap<ModelId, LocalModelState>>,
) -> MutexGuard<'_, HashMap<ModelId, LocalModelState>> {
    transient.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The coordinator: the store, the one downloader and the in-memory
/// `Downloading` / `Failed` states (lost on restart, spec FR-008).
pub struct LocalModels {
    store: Arc<ModelStore>,
    downloader: Downloader,
    transient: Transient,
    /// The removal of a final file: `std::fs::remove_file`, set in `open`; a unit
    /// test replaces it to reach `delete_failed` (the gate runs as uid 0).
    remove: fn(&Path) -> io::Result<()>,
}

impl LocalModels {
    /// The only constructor: a `ModelStore` over `models_dir` and `catalog`
    /// (`MODELS` in production), its `cleanup_at_start` run before this returns,
    /// and the downloader over `disk` and `timeouts`. The cleanup result is returned
    /// beside the coordinator, which is usable either way.
    pub fn open(
        models_dir: PathBuf,
        disk: Arc<dyn DiskSpace>,
        timeouts: Timeouts,
        catalog: &'static [CatalogEntry],
    ) -> (LocalModels, io::Result<()>) {
        let store = Arc::new(ModelStore::new(models_dir, catalog));
        let cleanup = store.cleanup_at_start();
        let downloader = Downloader::new(Arc::clone(&store), disk, timeouts);
        let models = LocalModels {
            store,
            downloader,
            transient: Arc::new(Mutex::new(HashMap::new())),
            remove: |path| std::fs::remove_file(path),
        };
        (models, cleanup)
    }

    /// The one store (settings validation, every command, later T-017 / T-019).
    pub fn store(&self) -> Arc<ModelStore> {
        Arc::clone(&self.store)
    }

    /// Every catalog model in catalog order: the in-memory `Downloading` / `Failed`
    /// state where one exists, otherwise the disk state. A `Failed` never hides a
    /// model the disk says is downloaded (the store settings validation reads): the
    /// view is `Downloaded` and the stale `Failed` is dropped.
    pub fn list(&self) -> Vec<LocalModelView> {
        let transient = lock(&self.transient).clone();
        let mut stale = Vec::new();
        let views = self
            .store
            .catalog()
            .iter()
            .zip(self.store.states())
            .map(|(entry, (id, disk_state))| {
                let state = match transient.get(&id) {
                    Some(failed @ LocalModelState::Failed { .. })
                        if disk_state == LocalModelState::Downloaded =>
                    {
                        stale.push((id, failed.clone()));
                        disk_state
                    }
                    Some(state) => state.clone(),
                    None => disk_state,
                };
                LocalModelView {
                    id,
                    name_key: name_key(id),
                    size_bytes: entry.size_bytes,
                    recommended: entry.recommended,
                    state,
                    loaded: false,
                }
            })
            .collect();
        if !stale.is_empty() {
            let mut current = lock(&self.transient);
            for (id, failed) in stale {
                // Only the entry read above: a download started meanwhile keeps its
                // `Downloading`.
                if current.get(&id) == Some(&failed) {
                    current.remove(&id);
                }
            }
        }
        views
    }

    /// `local_model_download { id }`. A refusal changes no state and never calls
    /// `emit`; otherwise `emit` is called only from the download thread.
    pub fn download(
        &self,
        id: &str,
        emit: impl Fn(LocalModelEvent) + Send + 'static,
    ) -> Result<(), ReasonView> {
        let Some(id) = ModelId::parse(id) else {
            return Err(ReasonView::from(&DownloadError::NotInCatalog));
        };
        // Held across `start`: the download thread's first callback waits for the
        // `Downloading` set here (core never calls the callback inside `start`).
        let mut transient = lock(&self.transient);
        let total = self.store.entry(id).map_or(0, |entry| entry.size_bytes);
        let previous = transient.insert(id, LocalModelState::Downloading { received: 0, total });
        let shared = Arc::clone(&self.transient);
        let callback = move |event: DownloadEvent| {
            let event = record(&shared, event);
            // No lock is held here: `emit` may call `list` (or a command) itself.
            emit(event);
        };
        match self.downloader.start(id, callback) {
            Ok(()) => Ok(()),
            Err(error) => {
                // A refusal keeps the earlier state (a `Failed` keeps its reason),
                // except `AlreadyDownloaded`: the disk has the model, so an earlier
                // `Failed` is stale and is dropped.
                match previous {
                    Some(state) if error != DownloadError::AlreadyDownloaded => {
                        transient.insert(id, state)
                    }
                    _ => transient.remove(&id),
                };
                Err(ReasonView::from(&error))
            }
        }
    }

    /// `local_model_cancel_download { id }`: true if a download of `id` was
    /// running; an unknown id is `false`.
    pub fn cancel(&self, id: &str) -> bool {
        ModelId::parse(id).is_some_and(|id| self.downloader.cancel(id))
    }

    /// `local_model_delete { id }`, the one delete path, in this order:
    ///
    /// 1. `not_downloaded` for an id outside the catalog, a running download or no
    ///    final file with the catalog size (the residency is not asked);
    /// 2. `residency.release_for_delete(id)`: `InUse` → `model_in_use`;
    /// 3. the removal of the final file (`NotFound` counts as removed); any other
    ///    error → `delete_failed`, the model stays downloaded and the settings are
    ///    untouched;
    /// 4. the transient state of `id` (a stale `Failed`) is dropped;
    /// 5. `settings.forget_model(id)`: a write failure does not undo the removal and
    ///    is reported as `reset_failed`;
    /// 6. the release guard is dropped.
    ///
    /// The transient map stays locked throughout, so the delete is serialized with
    /// `download` and `list`; `forget_model` takes only the settings `save_lock`,
    /// and nothing under that lock reads the transient map. No event is emitted.
    pub fn delete(
        &self,
        id: &str,
        residency: &dyn ModelRelease,
        settings: &SettingsService,
    ) -> Result<DeleteOutcome, ReasonView> {
        let refuse = |error: DeleteError| Err(ReasonView::from(&error));
        let Some(id) = ModelId::parse(id) else {
            return refuse(DeleteError::NotDownloaded);
        };
        let mut transient = lock(&self.transient);
        if matches!(
            transient.get(&id),
            Some(LocalModelState::Downloading { .. })
        ) {
            return refuse(DeleteError::NotDownloaded);
        }
        let Some(path) = self.store.path_if_downloaded(id) else {
            return refuse(DeleteError::NotDownloaded);
        };
        let Ok(guard) = residency.release_for_delete(id) else {
            return refuse(DeleteError::ModelInUse);
        };
        match (self.remove)(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return refuse(DeleteError::DeleteFailed),
        }
        transient.remove(&id);
        let outcome = match settings.forget_model(id.as_str()) {
            Ok(forgotten) => DeleteOutcome {
                engine_reset: forgotten.engine_reset,
                reset_failed: false,
            },
            Err(_) => DeleteOutcome {
                engine_reset: false,
                reset_failed: true,
            },
        };
        drop(guard);
        Ok(outcome)
    }
}

/// Updates the in-memory state for `event` (under the lock, released on return)
/// and returns the event for the settings window.
fn record(
    transient: &Mutex<HashMap<ModelId, LocalModelState>>,
    event: DownloadEvent,
) -> LocalModelEvent {
    let mut states = lock(transient);
    match event {
        DownloadEvent::Progress {
            id,
            received,
            total,
        } => {
            states.insert(id, LocalModelState::Downloading { received, total });
            LocalModelEvent::Progress {
                id,
                received,
                total,
            }
        }
        DownloadEvent::Finished { id } => {
            states.remove(&id);
            LocalModelEvent::State {
                id,
                state: LocalModelState::Downloaded,
            }
        }
        DownloadEvent::Failed { id, reason } => {
            let state = LocalModelState::Failed { reason };
            states.insert(id, state.clone());
            LocalModelEvent::State { id, state }
        }
        DownloadEvent::Cancelled { id } => {
            states.remove(&id);
            LocalModelEvent::State {
                id,
                state: LocalModelState::NotDownloaded,
            }
        }
    }
}

/// The one refusal mapping (contracts/ipc.md command errors). `NotEnoughDiskSpace`
/// reuses the failure's code, message and `needed` param.
impl From<&DownloadError> for ReasonView {
    fn from(error: &DownloadError) -> ReasonView {
        let (code, message_key) = match error {
            DownloadError::NotEnoughDiskSpace { needed } => {
                return ReasonView::from(&DownloadFailure::NotEnoughDiskSpace { needed: *needed })
            }
            DownloadError::Busy => ("download_busy", i18n::DOWNLOAD_BUSY),
            DownloadError::AlreadyDownloaded => {
                ("already_downloaded", i18n::DOWNLOAD_ALREADY_DOWNLOADED)
            }
            DownloadError::NotInCatalog => ("not_in_catalog", i18n::DOWNLOAD_NOT_IN_CATALOG),
            DownloadError::CannotStart => ("download_cannot_start", i18n::DOWNLOAD_CANNOT_START),
        };
        ReasonView {
            code,
            message_key,
            params: Vec::new(),
        }
    }
}

/// The refusals of `local_model_delete` (contracts/ipc.md); none shares a code
/// with a download reason.
impl From<&DeleteError> for ReasonView {
    fn from(error: &DeleteError) -> ReasonView {
        let (code, message_key) = match error {
            DeleteError::ModelInUse => ("model_in_use", i18n::DELETE_MODEL_IN_USE),
            DeleteError::NotDownloaded => ("not_downloaded", i18n::DELETE_NOT_DOWNLOADED),
            DeleteError::DeleteFailed => ("delete_failed", i18n::DELETE_FAILED),
        };
        ReasonView {
            code,
            message_key,
            params: Vec::new(),
        }
    }
}

/// `DownloadFailure`'s own `code` / `message_id` / `message_params` (one mapping).
impl From<&DownloadFailure> for ReasonView {
    fn from(failure: &DownloadFailure) -> ReasonView {
        ReasonView {
            code: failure.code(),
            message_key: failure.message_id(),
            params: failure.message_params(),
        }
    }
}

/// The stable string (`ModelId::as_str`).
impl Serialize for ModelId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// contracts/ipc.md `ModelState`: `{ kind, ... }`.
impl Serialize for LocalModelState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            LocalModelState::NotDownloaded => {
                let mut s = serializer.serialize_struct("ModelState", 1)?;
                s.serialize_field("kind", "not_downloaded")?;
                s.end()
            }
            LocalModelState::Downloading { received, total } => {
                let mut s = serializer.serialize_struct("ModelState", 3)?;
                s.serialize_field("kind", "downloading")?;
                s.serialize_field("received", received)?;
                s.serialize_field("total", total)?;
                s.end()
            }
            LocalModelState::Downloaded => {
                let mut s = serializer.serialize_struct("ModelState", 1)?;
                s.serialize_field("kind", "downloaded")?;
                s.end()
            }
            LocalModelState::Failed { reason } => {
                let mut s = serializer.serialize_struct("ModelState", 2)?;
                s.serialize_field("kind", "failed")?;
                s.serialize_field("reason", &ReasonView::from(reason))?;
                s.end()
            }
        }
    }
}

/// contracts/ipc.md `LocalModelView` (`nameKey`, `sizeBytes`).
impl Serialize for LocalModelView {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("LocalModelView", 6)?;
        s.serialize_field("id", &self.id)?;
        s.serialize_field("nameKey", &self.name_key)?;
        s.serialize_field("sizeBytes", &self.size_bytes)?;
        s.serialize_field("recommended", &self.recommended)?;
        s.serialize_field("state", &self.state)?;
        s.serialize_field("loaded", &self.loaded)?;
        s.end()
    }
}

/// `params` as a JSON object of strings, in the order given.
struct Params<'a>(&'a [(&'static str, String)]);

impl Serialize for Params<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, value) in self.0 {
            map.serialize_entry(name, value)?;
        }
        map.end()
    }
}

/// contracts/ipc.md `FailureReason`: `params` omitted when empty.
impl Serialize for ReasonView {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("FailureReason", 3)?;
        s.serialize_field("code", self.code)?;
        s.serialize_field("messageKey", &self.message_key)?;
        if self.params.is_empty() {
            s.skip_field("params")?;
        } else {
            s.serialize_field("params", &Params(&self.params))?;
        }
        s.end()
    }
}

/// The event payload only (the name is [`LocalModelEvent::name`]).
impl Serialize for LocalModelEvent {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            LocalModelEvent::Progress {
                id,
                received,
                total,
            } => {
                let mut s = serializer.serialize_struct("LocalModelProgress", 3)?;
                s.serialize_field("id", id)?;
                s.serialize_field("received", received)?;
                s.serialize_field("total", total)?;
                s.end()
            }
            LocalModelEvent::State { id, state } => {
                let mut s = serializer.serialize_struct("LocalModelStateEvent", 2)?;
                s.serialize_field("id", id)?;
                s.serialize_field("state", state)?;
                s.end()
            }
        }
    }
}

/// T-019: the `delete_failed` branch, which the Linux gate cannot reach through the
/// file system (the container runs as uid 0, so no permission trick makes
/// `remove_file` fail on a regular file). The removal is the private
/// `LocalModels::remove: fn(&Path) -> io::Result<()>`, which `open` sets to
/// `std::fs::remove_file`; these tests replace it. The real lock is the Windows CI
/// test in `src-tauri/tests/local_models.rs`.
#[cfg(test)]
mod delete_tests {
    use super::*;
    use crate::autostart::FakeAutostart;
    use crate::clock::FakeClock;
    use crate::hotkey_registrar::FakeHotkeyRegistrar;
    use crate::models::DownloadedModels;
    use crate::secrets::FakeCredentialStore;
    use crate::settings::file::{FakeSettingsFile, FileCall};
    use crate::settings::service::{SettingsDeps, SettingsService};
    use crate::settings::{defaults, EngineKind, Settings};
    use crate::test_support::local_models::{catalog, entry, file_name, model_bytes, FakeDisk};
    use crate::test_support::TempDir;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::TryRecvError;
    use std::time::{Duration, UNIX_EPOCH};

    const OS: Option<&str> = Some("en-US");

    struct Rig {
        _tmp: TempDir,
        file_path: PathBuf,
        models: LocalModels,
        file: Arc<FakeSettingsFile>,
        service: SettingsService,
        stored: Settings,
        changes: std::sync::mpsc::Receiver<Arc<Settings>>,
    }

    /// `small` on disk (never served: the URL is a host string only), and the
    /// settings selecting it as the built-in engine.
    fn rig() -> Rig {
        let tmp = TempDir::new();
        let dir = tmp.path().join("models");
        let name = file_name(ModelId::Small);
        std::fs::create_dir_all(&dir).expect("models dir");
        let file_path = dir.join(name);
        std::fs::write(&file_path, model_bytes()).expect("model file");
        let cat = catalog(vec![entry(
            ModelId::Small,
            name,
            "https://huggingface.co/fake/ggml-small.bin",
        )]);
        let disk: Arc<dyn DiskSpace> = FakeDisk::with_available(u64::MAX);
        let (models, cleanup) = LocalModels::open(dir, disk, Timeouts::default(), cat);
        cleanup.expect("cleanup");
        let mut stored = defaults(OS);
        stored.engine = EngineKind::BuiltinLocal;
        stored.builtin_local.model_id = Some("small".into());
        let file = Arc::new(FakeSettingsFile::with_bytes(
            &serde_json::to_vec(&stored).expect("serialize"),
        ));
        let deps = SettingsDeps {
            file: file.clone(),
            credentials: Arc::new(FakeCredentialStore::new()),
            autostart: Arc::new(FakeAutostart::new()),
            hotkeys: Arc::new(FakeHotkeyRegistrar::new()),
            local_models: models.store(),
            clock: Arc::new(FakeClock::at(
                UNIX_EPOCH + Duration::from_secs(1_709_251_199),
            )),
        };
        let (service, _) = SettingsService::load_or_init(deps, OS);
        let changes = service.subscribe();
        Rig {
            _tmp: tmp,
            file_path,
            models,
            file,
            service,
            stored,
            changes,
        }
    }

    /// Releases every model and counts the guards dropped.
    #[derive(Default)]
    struct CountingRelease {
        released: AtomicUsize,
        dropped: Arc<AtomicUsize>,
    }

    struct Counted(Arc<AtomicUsize>);

    impl Drop for Counted {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl ModelRelease for CountingRelease {
        fn release_for_delete(&self, _id: ModelId) -> Result<ReleaseGuard, InUse> {
            self.released.fetch_add(1, Ordering::SeqCst);
            Ok(ReleaseGuard::new(Counted(Arc::clone(&self.dropped))))
        }
    }

    fn sharing_violation(_: &Path) -> io::Result<()> {
        // Stands in for Windows' ERROR_SHARING_VIOLATION: any error but NotFound is
        // a failed removal.
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "injected: file locked",
        ))
    }

    fn already_gone(_: &Path) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::NotFound))
    }

    #[test]
    fn a_removal_error_is_delete_failed_and_the_model_stays_downloaded_and_selected() {
        // FR-023 / spec US4: a removal I/O error → `delete_failed`; the model stays
        // downloaded (listed, and in the store the settings validation reads) and the
        // settings are untouched. The guard is still dropped (the model reloads on
        // the next use). Bite: the error swallowed (Ok), the settings reset before the
        // removal, the transient state set to Failed, the guard leaked on the error
        // path, another code.
        let mut r = rig();
        r.models.remove = sharing_violation;
        let residency = CountingRelease::default();

        let got = r.models.delete("small", &residency, &r.service);

        assert_eq!(got, Err(ReasonView::from(&DeleteError::DeleteFailed)));
        assert_eq!(got.map_err(|e| e.code), Err("delete_failed"));
        assert!(r.file_path.exists());
        assert!(r.models.store().is_downloaded("small"));
        let listed: Vec<_> = r.models.list().into_iter().map(|v| v.state).collect();
        assert_eq!(listed, vec![LocalModelState::Downloaded]);
        assert_eq!(r.file.calls(), vec![FileCall::Read], "settings file calls");
        assert_eq!(&*r.service.snapshot(), &r.stored);
        assert_eq!(r.changes.try_recv().err(), Some(TryRecvError::Empty));
        assert_eq!(residency.released.load(Ordering::SeqCst), 1);
        assert_eq!(residency.dropped.load(Ordering::SeqCst), 1, "guard dropped");
    }

    #[test]
    fn a_removal_that_finds_the_file_already_gone_counts_as_removed() {
        // Analysis step 4: `NotFound` from the removal counts as removed (a second
        // instance or the user removed it meanwhile); the selection is reset. Bite:
        // NotFound mapped to delete_failed, the reset skipped.
        let mut r = rig();
        r.models.remove = already_gone;

        let got = r.models.delete("small", &NoResidency, &r.service);

        assert_eq!(
            got,
            Ok(DeleteOutcome {
                engine_reset: true,
                reset_failed: false,
            })
        );
        assert_eq!(r.service.snapshot().engine, EngineKind::None);
        assert_eq!(r.service.snapshot().builtin_local.model_id, None);
    }

    #[test]
    fn open_removes_through_std_fs_remove_file() {
        // The production removal is the real one. Bite: `open` leaving a no-op or a
        // test double in `remove`.
        let r = rig();
        assert!((r.models.remove)(&r.file_path).is_ok());
        assert!(!r.file_path.exists());
    }
}
