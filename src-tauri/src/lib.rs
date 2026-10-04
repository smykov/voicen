use std::sync::Arc;

use tauri::{App, Builder, Context, RunEvent, Runtime};
use voicen_core::autostart::Autostart;
use voicen_core::diag::{Log, LogEvent, WarningKind};
use voicen_core::local_models::catalog::MODELS;
use voicen_core::local_models::download::DiskSpace;
use voicen_core::local_models::service::LocalModels;
use voicen_core::secrets::CredentialStore;
use voicen_core::settings::service::SettingsService;
use voicen_core::timeouts::Timeouts;
use voicen_core::BuildInfo;

#[cfg(windows)]
pub mod autostart;
#[cfg(windows)]
pub mod credentials;
pub mod diag;
pub mod local_models;
pub mod locale;
pub mod paths;
pub mod settings_ipc;
pub mod settings_window;

#[tauri::command]
fn get_build_info() -> BuildInfo {
    voicen_core::build_info()
}

/// The command registration; reached only through [`build_app`] (T-030 J4).
fn commands<R: Runtime>(builder: Builder<R>) -> Builder<R> {
    builder.invoke_handler(tauri::generate_handler![
        get_build_info,
        settings_ipc::settings_get,
        settings_ipc::settings_save,
        settings_ipc::settings_speech_languages,
        local_models::local_models_list,
        local_models::local_model_download,
        local_models::local_model_cancel_download
    ])
}

/// The one app wiring, shared by `run()` and the tests (T-030 J4): registers
/// `commands`, manages `service`, `local_models` and `log`, builds the app with
/// `context`, and starts the `settings://changed` bridge on the built app's handle.
/// tauri 2.12.1 runs `.setup()` only from `run` / `run_iteration`, never from
/// `build()`, so the bridge starts here, after `build()`; `run()` then only calls
/// `.run(…)` on the result.
///
/// `local_models` is the one coordinator behind the local-model commands (T-044);
/// its store is the one `run()` passed to `settings_ipc::load_settings`.
///
/// `log` is the one log (`diag::start`, T-008): managed for `settings_save` (the
/// save line) and handed to the bridge (its spawn failure is a typed warning).
pub fn build_app<R: Runtime>(
    builder: Builder<R>,
    context: Context<R>,
    service: Arc<SettingsService>,
    local_models: Arc<LocalModels>,
    log: Arc<Log>,
) -> tauri::Result<App<R>> {
    let app = commands(builder)
        .manage(service.clone())
        .manage(local_models)
        .manage(log.clone())
        .build(context)?;
    settings_ipc::spawn_change_bridge(app.handle().clone(), service, &log);
    Ok(app)
}

/// The release key store: Credential Manager, the only one (NFR-04; no fallback).
#[cfg(windows)]
fn release_credentials() -> Arc<dyn CredentialStore> {
    Arc::new(credentials::WinCredentialStore::new())
}

#[cfg(not(windows))]
fn release_credentials() -> Arc<dyn CredentialStore> {
    compile_error!(
        "the Voicen app runs on Windows only: its key store is Credential Manager \
         (NFR-04, decisions #5); there is no other key storage"
    )
}

/// The release logon start entry: the HKCU Run value `Voicen` (T-014).
#[cfg(windows)]
fn release_autostart() -> Arc<dyn Autostart> {
    Arc::new(autostart::WinAutostart::new())
}

#[cfg(not(windows))]
fn release_autostart() -> Arc<dyn Autostart> {
    compile_error!("the Voicen app runs on Windows only: autostart is the HKCU Run value")
}

/// The release free-space probe of the models dir: `GetDiskFreeSpaceExW` (T-044).
#[cfg(windows)]
fn release_disk_space() -> Arc<dyn DiskSpace> {
    Arc::new(local_models::WinDiskSpace)
}

#[cfg(not(windows))]
fn release_disk_space() -> Arc<dyn DiskSpace> {
    compile_error!("the Voicen app runs on Windows only: the disk probe is GetDiskFreeSpaceExW")
}

/// `true` when the Run value started this process (`--autostart`, T-014).
#[cfg(windows)]
fn release_launched_by_autostart() -> bool {
    autostart::launched_by_autostart(std::env::args_os())
}

#[cfg(not(windows))]
fn release_launched_by_autostart() -> bool {
    compile_error!(
        "the Voicen app runs on Windows only: the autostart flag comes from the HKCU \
         Run value"
    )
}

/// The OS code of a tauri error that is an I/O error; `None` otherwise (the text
/// of a tauri error is never logged).
fn io_os_code(err: &tauri::Error) -> Option<i32> {
    match err {
        tauri::Error::Io(io) => io.raw_os_error(),
        _ => None,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // The one log (T-008), Started first; every later diagnostic is a typed line on
    // it. The unwritable callback is T-054's one-time notice; until then the log
    // only degrades.
    let log = diag::start(paths::log_dir(), Box::new(|_| {}));
    // The one LocalModels (T-044): its `.part` cleanup runs inside `open`, before
    // any list or settings validation reads the models dir.
    let (local_models, cleanup) = LocalModels::open(
        paths::models_dir(),
        release_disk_space(),
        Timeouts::default(),
        MODELS,
    );
    if let Err(err) = cleanup {
        // The kind and the OS code only, no path or OS text (#45). The app runs on;
        // the leftover is never read as a model.
        log.write(LogEvent::Warning {
            kind: WarningKind::ModelsCleanupFailed,
            os_code: err.raw_os_error(),
        });
    }
    let local_models = Arc::new(local_models);
    let os_language = locale::os_language();
    let (service, load_outcome) = settings_ipc::load_settings(
        paths::data_dir(),
        release_credentials(),
        release_autostart(),
        local_models.store(),
        os_language.as_deref(),
        &log,
    );
    let launched_by_autostart = release_launched_by_autostart();
    let window_log = Arc::clone(&log);
    build_app(
        tauri::Builder::default(),
        tauri::generate_context!(),
        service,
        local_models,
        Arc::clone(&log),
    )
    .expect("error while building tauri application")
    .run(move |app, event| {
        // The one startup window decision (S1): Ready fires once.
        if let RunEvent::Ready = event {
            if let Err(err) = settings_window::on_ready(app, &load_outcome, launched_by_autostart) {
                // The kind and, for an I/O error, the OS code only (decision #45);
                // the user-facing path (the one-time notice, the tray "Open logs
                // folder") is T-054's (decision #64).
                window_log.write(LogEvent::Warning {
                    kind: WarningKind::SettingsWindowFailed,
                    os_code: io_os_code(&err),
                });
            }
        }
    });
}
