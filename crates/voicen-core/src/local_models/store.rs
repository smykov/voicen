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
/// the coordinator ([`LocalModels`](super::service::LocalModels)) and are lost on
/// restart (spec FR-008).
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
    /// directory is not an error; every `.part` is tried and the first error is
    /// returned afterwards.
    pub fn cleanup_at_start(&self) -> io::Result<()> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        // Every `.part` is tried; the first error is returned after the loop.
        let mut first_error = None;
        for entry in entries {
            let removed = entry.and_then(|entry| {
                let path = entry.path();
                let is_part = path.extension().is_some_and(|ext| ext == PART_EXTENSION);
                if is_part && entry.file_type()?.is_file() {
                    match std::fs::remove_file(&path) {
                        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                        _ => {}
                    }
                }
                Ok(())
            });
            if let Err(e) = removed {
                first_error.get_or_insert(e);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Every catalog model with its disk-derived state, in catalog order.
    pub fn states(&self) -> Vec<(ModelId, LocalModelState)> {
        self.catalog
            .iter()
            .map(|entry| {
                let state = if self.is_on_disk(entry) {
                    LocalModelState::Downloaded
                } else {
                    LocalModelState::NotDownloaded
                };
                (entry.id, state)
            })
            .collect()
    }

    /// The final file of `id` when it exists with the catalog size.
    pub fn path_if_downloaded(&self, id: ModelId) -> Option<PathBuf> {
        self.entry(id)
            .filter(|entry| self.is_on_disk(entry))
            .map(|entry| self.final_path(entry))
    }

    /// This store's catalog entry of `id`.
    pub(crate) fn entry(&self, id: ModelId) -> Option<&'static CatalogEntry> {
        self.catalog.iter().find(|entry| entry.id == id)
    }

    /// `<dir>/<file_name>`: the only name a verified model has.
    pub(crate) fn final_path(&self, entry: &CatalogEntry) -> PathBuf {
        self.dir.join(entry.file_name)
    }

    /// `<dir>/<file_name>.part`: where a running download writes.
    pub(crate) fn part_path(&self, entry: &CatalogEntry) -> PathBuf {
        self.dir
            .join(format!("{}.{PART_EXTENSION}", entry.file_name))
    }

    /// The one "downloaded" rule: the final file is a regular file with the
    /// catalog size. Read from the disk at every call (no cache); the hash is
    /// the downloader's gate, not re-checked here.
    fn is_on_disk(&self, entry: &CatalogEntry) -> bool {
        std::fs::metadata(self.final_path(entry))
            .is_ok_and(|meta| meta.is_file() && meta.len() == entry.size_bytes)
    }
}

/// The extension of an unfinished download (`<file_name>.part`).
const PART_EXTENSION: &str = "part";

impl DownloadedModels for ModelStore {
    /// `id` is the stable string (`builtin_local.model_id`); an unknown id, or a
    /// model not in this store's catalog, is not downloaded.
    fn is_downloaded(&self, id: &str) -> bool {
        ModelId::parse(id).is_some_and(|id| self.path_if_downloaded(id).is_some())
    }

    /// The stable strings of the downloaded models, in catalog order.
    fn list(&self) -> Vec<String> {
        self.catalog
            .iter()
            .filter(|entry| self.is_on_disk(entry))
            .map(|entry| entry.id.as_str().to_string())
            .collect()
    }
}
