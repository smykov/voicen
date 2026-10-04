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

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;

use super::catalog::{CatalogEntry, ModelId};
use super::download::{DiskSpace, DownloadError, DownloadFailure};
use super::store::{LocalModelState, ModelStore};
use crate::i18n::MessageId;
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
        // Skeleton (T-044 red tests): not implemented yet.
        todo!("T-044: LocalModelEvent::name")
    }
}

/// The coordinator: the store, the one downloader and the in-memory
/// `Downloading` / `Failed` states (lost on restart, spec FR-008).
pub struct LocalModels {
    store: Arc<ModelStore>,
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
        // Skeleton (T-044 red tests): not implemented yet.
        let _ = (models_dir, disk, timeouts, catalog);
        todo!("T-044: LocalModels::open")
    }

    /// The one store (settings validation, every command, later T-017 / T-019).
    pub fn store(&self) -> Arc<ModelStore> {
        Arc::clone(&self.store)
    }

    /// Every catalog model in catalog order: the in-memory `Downloading` / `Failed`
    /// state where one exists, otherwise the disk state.
    pub fn list(&self) -> Vec<LocalModelView> {
        // Skeleton (T-044 red tests): not implemented yet.
        todo!("T-044: LocalModels::list")
    }

    /// `local_model_download { id }`. A refusal changes no state and never calls
    /// `emit`; otherwise `emit` is called only from the download thread.
    pub fn download(
        &self,
        id: &str,
        emit: impl Fn(LocalModelEvent) + Send + 'static,
    ) -> Result<(), ReasonView> {
        // Skeleton (T-044 red tests): not implemented yet.
        let _ = (id, emit);
        todo!("T-044: LocalModels::download")
    }

    /// `local_model_cancel_download { id }`: true if a download of `id` was
    /// running; an unknown id is `false`.
    pub fn cancel(&self, id: &str) -> bool {
        // Skeleton (T-044 red tests): not implemented yet.
        let _ = id;
        todo!("T-044: LocalModels::cancel")
    }
}

impl From<&DownloadError> for ReasonView {
    fn from(error: &DownloadError) -> ReasonView {
        // Skeleton (T-044 red tests): not implemented yet.
        let _ = error;
        todo!("T-044: ReasonView from DownloadError")
    }
}

impl From<&DownloadFailure> for ReasonView {
    fn from(failure: &DownloadFailure) -> ReasonView {
        // Skeleton (T-044 red tests): not implemented yet.
        let _ = failure;
        todo!("T-044: ReasonView from DownloadFailure")
    }
}

/// The stable string (`ModelId::as_str`).
impl Serialize for ModelId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Skeleton (T-044 red tests): not implemented yet.
        let _ = serializer;
        todo!("T-044: ModelId wire")
    }
}

/// contracts/ipc.md `ModelState`: `{ kind, ... }`.
impl Serialize for LocalModelState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Skeleton (T-044 red tests): not implemented yet.
        let _ = serializer;
        todo!("T-044: ModelState wire")
    }
}

/// contracts/ipc.md `LocalModelView` (`nameKey`, `sizeBytes`).
impl Serialize for LocalModelView {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Skeleton (T-044 red tests): not implemented yet.
        let _ = serializer;
        todo!("T-044: LocalModelView wire")
    }
}

/// contracts/ipc.md `FailureReason`: `params` omitted when empty.
impl Serialize for ReasonView {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Skeleton (T-044 red tests): not implemented yet.
        let _ = serializer;
        todo!("T-044: FailureReason wire")
    }
}

/// The event payload only (the name is [`LocalModelEvent::name`]).
impl Serialize for LocalModelEvent {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Skeleton (T-044 red tests): not implemented yet.
        let _ = serializer;
        todo!("T-044: LocalModelEvent payload")
    }
}
