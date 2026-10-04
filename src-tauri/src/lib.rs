use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use tauri::{App, Builder, Context, Runtime};
use voicen_core::autostart::Autostart;
use voicen_core::secrets::CredentialStore;
use voicen_core::settings::service::SettingsService;
use voicen_core::BuildInfo;

#[cfg(windows)]
pub mod autostart;
#[cfg(windows)]
pub mod credentials;
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
        settings_ipc::settings_speech_languages
    ])
}

/// The one app wiring, shared by `run()` and the tests (T-030 J4): registers
/// `commands`, manages `service`, builds the app with `context`, and starts the
/// `settings://changed` bridge on the built app's handle. tauri 2.12.1 runs
/// `.setup()` only from `run` / `run_iteration`, never from `build()`, so the bridge
/// starts here, after `build()`; `run()` then only calls `.run(…)` on the result.
pub fn build_app<R: Runtime>(
    builder: Builder<R>,
    context: Context<R>,
    service: Arc<SettingsService>,
) -> tauri::Result<App<R>> {
    let app = commands(builder).manage(service.clone()).build(context)?;
    settings_ipc::spawn_change_bridge(app.handle().clone(), service);
    Ok(app)
}

/// Writes the start line with version and commit into `dir/voicen.log` (FR-18); a
/// failure never stops the app.
pub fn log_start(dir: &Path) {
    let line = format!("voicen {} started\n", voicen_core::build_info());
    let written = std::fs::create_dir_all(dir).and_then(|_| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("voicen.log"))
            .and_then(|mut f| f.write_all(line.as_bytes()))
    });
    if let Err(err) = written {
        eprintln!("cannot write log in {}: {err}", dir.display());
    }
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    log_start(&paths::log_dir());
    let os_language = locale::os_language();
    // The load outcome drives the startup executor (open the settings window on
    // FirstRun/Reset, the reset notice): T-004 consumes it.
    let (service, _load_outcome) = settings_ipc::load_settings(
        paths::data_dir(),
        release_credentials(),
        release_autostart(),
        os_language.as_deref(),
    );
    build_app(
        tauri::Builder::default(),
        tauri::generate_context!(),
        service,
    )
    .expect("error while building tauri application")
    .run(|_, _| {});
}
