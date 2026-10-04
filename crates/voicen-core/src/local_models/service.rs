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
//! `not_in_catalog`, `download_cannot_start`.
//!
//! The shell (`src-tauri/src/local_models.rs`) is an adapter: it parses nothing,
//! delegates the commands here and emits each event to the settings window.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::ser::{SerializeMap, SerializeStruct};
use serde::Serialize;

use super::catalog::{CatalogEntry, ModelId};
use super::download::{DiskSpace, DownloadError, DownloadEvent, DownloadFailure, Downloader};
use super::store::{LocalModelState, ModelStore};
use crate::i18n::{self, MessageId};
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
        };
        (models, cleanup)
    }

    /// The one store (settings validation, every command, later T-017 / T-019).
    pub fn store(&self) -> Arc<ModelStore> {
        Arc::clone(&self.store)
    }

    /// Every catalog model in catalog order: the in-memory `Downloading` / `Failed`
    /// state where one exists, otherwise the disk state.
    pub fn list(&self) -> Vec<LocalModelView> {
        let transient = lock(&self.transient).clone();
        self.store
            .catalog()
            .iter()
            .zip(self.store.states())
            .map(|(entry, (id, disk_state))| LocalModelView {
                id,
                name_key: name_key(id),
                size_bytes: entry.size_bytes,
                recommended: entry.recommended,
                state: transient.get(&id).cloned().unwrap_or(disk_state),
                loaded: false,
            })
            .collect()
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
                match previous {
                    Some(state) => transient.insert(id, state),
                    None => transient.remove(&id),
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
