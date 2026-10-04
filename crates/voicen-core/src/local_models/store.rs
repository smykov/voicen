//! Which catalog models are on disk (spec 002 contracts/core-traits.md
//! "ModelStore", data-model "LocalModelState").
//!
//! "Downloaded" is derived from the disk only: the final file exists with the
//! catalog size. A file with the final name but another size is not downloaded and
//! is not deleted (a re-download overwrites it by rename). `ModelStore` is the one
//! [`DownloadedModels`] of the app: settings validation, the IPC list and the
//! built-in engine read the same instance (P-010, P-011).

use std::io;
use std::path::{Path, PathBuf};

use super::catalog::{CatalogEntry, ModelId};
use super::download::DownloadFailure;
use crate::models::DownloadedModels;

/// A model's state as the UI shows it. The store reports only `NotDownloaded`
/// and `Downloaded` (disk-derived); `Downloading` and `Failed` live in memory in
/// the shell and are lost on restart (spec FR-008).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalModelState {
    NotDownloaded,
    Downloading {
        received: u64,
        total: u64,
    },
    Downloaded,
    /// Shown as not downloaded, with the reason and a Retry action.
    Failed {
        reason: DownloadFailure,
    },
}

/// The models directory and the catalog it is checked against.
#[derive(Debug)]
pub struct ModelStore {
    dir: PathBuf,
    catalog: &'static [CatalogEntry],
}

impl ModelStore {
    /// `models_dir` comes from the shell's one data-dir resolver
    /// (`paths::data_dir().join("models")`); `catalog` is
    /// [`MODELS`](super::catalog::MODELS) in production.
    pub fn new(models_dir: PathBuf, catalog: &'static [CatalogEntry]) -> ModelStore {
        ModelStore {
            dir: models_dir,
            catalog,
        }
    }

    /// The models directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The catalog this store checks against.
    pub fn catalog(&self) -> &'static [CatalogEntry] {
        self.catalog
    }

    /// Deletes every `*.part` file in the models directory (a download cut by an
    /// exit or crash). Runs before the first [`states`](Self::states). A missing
    /// directory is not an error.
    pub fn cleanup_at_start(&self) -> io::Result<()> {
        // Skeleton (T-016 red tests): not implemented yet.
        todo!("T-016: ModelStore::cleanup_at_start")
    }

    /// Every catalog model with its disk-derived state, in catalog order.
    pub fn states(&self) -> Vec<(ModelId, LocalModelState)> {
        // Skeleton (T-016 red tests): not implemented yet.
        todo!("T-016: ModelStore::states")
    }

    /// The final file of `id` when it exists with the catalog size.
    pub fn path_if_downloaded(&self, id: ModelId) -> Option<PathBuf> {
        // Skeleton (T-016 red tests): not implemented yet.
        let _ = id;
        todo!("T-016: ModelStore::path_if_downloaded")
    }
}

impl DownloadedModels for ModelStore {
    /// `id` is the stable string (`builtin_local.model_id`); an unknown id, or a
    /// model not in this store's catalog, is not downloaded.
    fn is_downloaded(&self, id: &str) -> bool {
        // Skeleton (T-016 red tests): not implemented yet.
        let _ = id;
        todo!("T-016: ModelStore::is_downloaded")
    }

    /// The stable strings of the downloaded models, in catalog order.
    fn list(&self) -> Vec<String> {
        // Skeleton (T-016 red tests): not implemented yet.
        todo!("T-016: ModelStore::list")
    }
}
