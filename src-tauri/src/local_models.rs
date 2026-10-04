//! The shell half of the local models (T-044, decision #57 option A; specs/002
//! contracts/ipc.md): the commands `local_models_list`, `local_model_download { id }`
//! and `local_model_cancel_download { id }` over core's
//! `voicen_core::local_models::service::LocalModels`, the emit of its events to the
//! settings window, and [`WinDiskSpace`].
//!
//! Skeleton (T-044 red tests): only the disk probe's declaration exists yet; the
//! commands, their registration in `commands()` and the emit adapter are the
//! implementation's (tests: `src-tauri/tests/local_models.rs`).

#[cfg(windows)]
use std::io;
#[cfg(windows)]
use std::path::Path;

#[cfg(windows)]
use voicen_core::local_models::download::DiskSpace;

/// Free space of the volume holding a directory (core's `DiskSpace`, R-9):
/// `GetDiskFreeSpaceExW` on the nearest existing ancestor of the directory, since
/// the models dir does not exist before the first download.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, Default)]
pub struct WinDiskSpace;

#[cfg(windows)]
impl DiskSpace for WinDiskSpace {
    fn available_bytes(&self, dir: &Path) -> io::Result<u64> {
        // Skeleton (T-044 red tests): not implemented yet.
        let _ = dir;
        todo!("T-044: WinDiskSpace::available_bytes")
    }
}
