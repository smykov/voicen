//! The shell half of the local models (T-044, decision #57 option A; specs/002
//! contracts/ipc.md): the commands `local_models_list`, `local_model_download { id }`
//! and `local_model_cancel_download { id }` over core's
//! `voicen_core::local_models::service::LocalModels`, the emit of its events to the
//! settings window, and [`WinDiskSpace`].
//!
//! The commands only delegate: the id string goes to the coordinator as given (it
//! parses it, an unknown id is `not_in_catalog` / `false`), and no reqwest call runs
//! or is dropped here. `LocalModels::download` does a file metadata read, the disk
//! probe and a thread spawn on the command thread; the transfer runs on core's
//! download thread.
//!
//! Events: every `local-model://progress|state` is emitted from the download thread
//! through [`emit_to_settings`], with `emit_to(EventTarget::webview_window(LABEL))`, so
//! App-target and other-label Rust listeners never see them. A JS `listen()` with the
//! default (Any) target in any webview would; only the `settings` window may call
//! `plugin:event|listen` (decision #45's capability). `settings://changed` uses
//! `app.emit` on purpose (every window follows the settings).
//!
//! Nothing is printed or logged here (logging waits for T-008); an emit error is
//! ignored, it never changes a download's outcome.

#[cfg(windows)]
use std::io;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::path::Path;
use std::sync::Arc;

use tauri::{AppHandle, Emitter, EventTarget, Runtime, State};
#[cfg(windows)]
use voicen_core::local_models::download::DiskSpace;
use voicen_core::local_models::service::{
    LocalModelEvent, LocalModelView, LocalModels, ReasonView,
};
#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

use crate::settings_window;

/// `local_models_list` → every catalog model in catalog order with its state. Runs
/// off the main thread: it reads the models dir.
#[tauri::command(async)]
pub fn local_models_list(models: State<'_, Arc<LocalModels>>) -> Vec<LocalModelView> {
    models.list()
}

/// `local_model_download { id }` → `void`, or a `FailureReason` (`download_busy`,
/// `already_downloaded`, `not_enough_disk_space{needed}`, `not_in_catalog`,
/// `download_cannot_start`). Progress and the end come as events. Runs off the main
/// thread: the start reads the disk and probes the free space.
#[tauri::command(async)]
pub fn local_model_download<R: Runtime>(
    app: AppHandle<R>,
    models: State<'_, Arc<LocalModels>>,
    id: String,
) -> Result<(), ReasonView> {
    models.download(&id, emit_to_settings(app))
}

/// `local_model_cancel_download { id }` → true if a download of `id` was running.
#[tauri::command]
pub fn local_model_cancel_download(models: State<'_, Arc<LocalModels>>, id: String) -> bool {
    models.cancel(&id)
}

/// The emit adapter: the core callback, called on the download thread with no lock
/// held, emits each event to the settings window only (see the module doc).
fn emit_to_settings<R: Runtime>(app: AppHandle<R>) -> impl Fn(LocalModelEvent) + Send + 'static {
    move |event: LocalModelEvent| {
        let _ = app.emit_to(
            EventTarget::webview_window(settings_window::LABEL),
            event.name(),
            &event,
        );
    }
}

/// Free space of the volume holding a directory (core's `DiskSpace`, R-9):
/// `GetDiskFreeSpaceExW` on the nearest existing ancestor of the directory, since
/// the models dir does not exist before the first download. Creates nothing; a path
/// with no existing ancestor is `NotFound`.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, Default)]
pub struct WinDiskSpace;

#[cfg(windows)]
impl DiskSpace for WinDiskSpace {
    fn available_bytes(&self, dir: &Path) -> io::Result<u64> {
        let existing = dir
            .ancestors()
            .find(|p| p.is_dir())
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        let mut wide: Vec<u16> = existing.as_os_str().encode_wide().collect();
        wide.push(0);
        let mut available: u64 = 0;
        // SAFETY: `wide` is NUL-terminated and outlives the call; `available` is a
        // valid u64 the call writes; the other two outputs are not requested.
        unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&raw mut available), None, None) }
            .map_err(io::Error::from)?;
        Ok(available)
    }
}
