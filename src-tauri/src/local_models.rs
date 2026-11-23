//! The shell half of the local models (T-044, decision #57 option A; specs/002
//! contracts/ipc.md): the commands `local_models_list`, `local_model_download { id }`,
//! `local_model_cancel_download { id }` and `local_model_delete { id }` (T-019) over
//! core's
//! `voicen_core::local_models::service::LocalModels`, the emit of its events to the
//! settings window, and [`WinDiskSpace`].
//!
//! The commands only delegate: the id string goes to the coordinator as given (it
//! parses it, an unknown id is `not_in_catalog` / `false`), and no reqwest call runs
//! or is dropped here. `LocalModels::download` does a file metadata read, the disk
//! probe and a thread spawn on the command thread; the transfer runs on core's
//! download thread. `local_model_delete` is `LocalModels::delete_resident` with the
//! managed `SettingsService`: the release goes through the process's one
//! `ModelResidency` (T-017, [`builtin_deps`]), which `run()` attached to the coordinator.
//! It emits nothing itself: a selection reset reaches the window as `settings://changed`
//! through the change bridge.
//!
//! [`builtin_deps`] is the one constructor of the process's `BuiltinDeps` (T-017):
//! `run()` calls it once and hands the result to the dictation session, the
//! coordinator and the idle thread ([`start_model_idle`]).
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

use std::io;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::path::Path;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Emitter, EventTarget, Runtime, State};
use voicen_core::clock::{MonotonicClock, SystemMonotonic};
use voicen_core::engine::builtin::BuiltinDeps;
use voicen_core::engine::speech_model::SpeechModelLoader;
#[cfg(windows)]
use voicen_core::local_models::download::DiskSpace;
use voicen_core::local_models::residency::ModelResidency;
use voicen_core::local_models::service::{
    DeleteOutcome, LocalModelEvent, LocalModelView, LocalModels, ReasonView,
};
use voicen_core::local_models::store::ModelStore;
use voicen_core::settings::service::SettingsService;
use voicen_core::timeouts::MODEL_IDLE_UNLOAD;
use voicen_whisper::WhisperLoader;
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

/// `local_model_delete { id }` → `{ engineReset, resetFailed }`, or a
/// `FailureReason` (`model_in_use`, `not_downloaded`, `delete_failed`). Runs off the
/// main thread: it removes a file and may write `settings.json`.
#[tauri::command(async)]
pub fn local_model_delete(
    models: State<'_, Arc<LocalModels>>,
    settings: State<'_, Arc<SettingsService>>,
    id: String,
) -> Result<DeleteOutcome, ReasonView> {
    models.delete_resident(&id, &settings)
}

/// The process's one built-in engine deps (T-017): one `ModelResidency` over
/// [`WhisperLoader`] and the system monotonic clock, idle `MODEL_IDLE_UNLOAD`, and
/// `store` (the `LocalModels` store). Loads nothing: a model is loaded only by a
/// BuiltinLocal recording's prewarm or job.
pub fn builtin_deps(store: Arc<ModelStore>) -> BuiltinDeps {
    BuiltinDeps {
        residency: Arc::new(ModelResidency::new(
            Arc::new(WhisperLoader::new()) as Arc<dyn SpeechModelLoader>,
            Arc::new(SystemMonotonic) as Arc<dyn MonotonicClock>,
            MODEL_IDLE_UNLOAD,
        )),
        store,
    }
}

/// How often the idle thread asks the residency to unload (spec 002 T027's periodic
/// fallback; the unload point itself is the residency's `MODEL_IDLE_UNLOAD` deadline).
pub const MODEL_IDLE_TICK: Duration = Duration::from_secs(5);

/// Starts the `voicen-model-idle` thread over `residency`: `tick` every
/// [`MODEL_IDLE_TICK`], and every settings snapshot `settings` publishes (each save,
/// each written `forget_model`) goes to `on_settings` at once (spec 002 FR-012). It
/// never loads. The thread ends when the service is gone (its channel closes); a
/// spawn error is returned and nothing runs (the model then stays until exit).
pub fn start_model_idle(
    residency: Arc<ModelResidency>,
    settings: &SettingsService,
) -> io::Result<()> {
    let changes = settings.subscribe();
    thread::Builder::new()
        .name("voicen-model-idle".to_string())
        .spawn(move || loop {
            match changes.recv_timeout(MODEL_IDLE_TICK) {
                Ok(snapshot) => residency.on_settings(&snapshot),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            residency.tick();
        })
        .map(|_| ())
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
