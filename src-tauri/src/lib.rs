use std::sync::Arc;

use tauri::{App, AppHandle, Builder, Context, RunEvent, Runtime};
use voicen_core::autostart::Autostart;
use voicen_core::diag::{Log, LogEvent, WarningKind};
use voicen_core::local_models::catalog::MODELS;
use voicen_core::local_models::download::DiskSpace;
use voicen_core::local_models::service::LocalModels;
use voicen_core::secrets::CredentialStore;
use voicen_core::settings::service::SettingsService;
use voicen_core::settings::LoadOutcome;
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
pub mod tray;

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
    // Skeleton (T-052 red tests): becomes `assemble(builder, context, move ||
    // Parts::new(service, local_models, log))`, which also starts the settings
    // opener, the tray and its language follower; the body below is unchanged.
    let app = commands(builder)
        .manage(service.clone())
        .manage(local_models)
        .manage(log.clone())
        .build(context)?;
    settings_ipc::spawn_change_bridge(app.handle().clone(), service, &log);
    Ok(app)
}

/// What the startup side effects produce (T-052 invariant 1): the one log
/// (`diag::start`), the one `LocalModels` (its `.part` cleanup ran in `open`) and
/// the settings service (`load_settings`, which may write `settings.json` and
/// reconciles the Run value). [`assemble`] makes them only after tauri's `build()`
/// has run the plugins' setup.
pub struct Parts {
    pub service: Arc<SettingsService>,
    pub local_models: Arc<LocalModels>,
    pub log: Arc<Log>,
}

impl Parts {
    pub fn new(
        service: Arc<SettingsService>,
        local_models: Arc<LocalModels>,
        log: Arc<Log>,
    ) -> Parts {
        Parts {
            service,
            local_models,
            log,
        }
    }
}

/// The one assembly body of `run()` and the tests (T-052 invariant 1): registers
/// `commands`, builds the app with `context` (tauri runs every plugin's setup inside
/// `build()`: the single-instance plugin decides there, and a second instance never
/// returns from it), only then calls `parts()` (the startup side effects: log,
/// `.part` cleanup, settings load, Run-value reconcile), and wires the result:
/// manages `service`, `local_models` and `log`, starts the `settings://changed`
/// bridge, the settings opener thread (`settings_window::request`), the tray
/// (`tray`, built in code, never from `tauri.conf.json`) and its language follower.
/// A `build()` error is returned and `parts` is never called.
///
/// `run()` passes `Builder::default()` with the single-instance plugin registered
/// first; [`build_app`] and the tests never register that plugin (its second-instance
/// path calls `process::exit(0)` inside `build()`).
pub fn assemble<R: Runtime>(
    builder: Builder<R>,
    context: Context<R>,
    parts: impl FnOnce() -> Parts,
) -> tauri::Result<App<R>> {
    // Skeleton (T-052 red tests): not implemented yet.
    let _ = (builder, context, parts);
    todo!("T-052: assemble")
}

/// The run-loop callback of `run()` and the real-runtime tests (T-052 invariant 4):
/// - `Ready`: `settings_window::on_ready(app, outcome, launched_by_autostart)`
///   (the opener carries it out and logs a failed open);
/// - `ExitRequested { code: None }` (in tauri 2.12.1 only the last window's
///   `Destroyed`) while the tray exists: `prevent_exit`, so closing the settings
///   window keeps the app; `ExitRequested { code: Some(_) }` (`AppHandle::exit`, the
///   tray "Exit" item) passes, and without a tray nothing is prevented;
/// - everything else: nothing.
pub fn on_run_event<R: Runtime>(
    app: &AppHandle<R>,
    event: RunEvent,
    outcome: &LoadOutcome,
    launched_by_autostart: bool,
) {
    // Skeleton (T-052 red tests): not implemented yet. `io_os_code` moves with the
    // failure line to the opener thread.
    let _ = (app, event, outcome, launched_by_autostart, io_os_code);
    todo!("T-052: on_run_event")
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
    // Skeleton (T-052 red tests): `run()` becomes `assemble` with the single-instance
    // plugin registered first and the startup side effects as its `parts`, then
    // `.run(on_run_event)`.
    build_app(
        tauri::Builder::default(),
        tauri::generate_context!(),
        service,
        local_models,
        Arc::clone(&log),
    )
    .expect("error while building tauri application")
    .run(move |app, event| on_run_event(app, event, &load_outcome, launched_by_autostart));
}
