//! Settings over IPC (T-030; contracts/ipc.md): the one construction path of the
//! settings service, the commands `settings_get` / `settings_save` /
//! `settings_speech_languages`, and the `settings://changed` bridge.
//!
//! T-030 RED SKELETON (test-writer): signatures settled by the T-030 Investigation
//! (items 3, 4, 6); the bodies are stubs the developer replaces. Every stub returns a
//! wrong value or panics with `todo!`, never a right one.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, Runtime, State};
use voicen_core::secrets::{CredentialStore, KeyPresence};
use voicen_core::settings::service::{
    FormError, SaveOutcome, SaveRequest, SettingsService, SettingsView,
};
use voicen_core::settings::{defaults, LoadOutcome};

/// Builds the service the release app and the tests share (J4): `FsSettingsFile`
/// over `data_dir`, the given credential store, the interim `NoDownloadedModels`
/// and fail-closed `HotkeyRegistrar`, `SystemClock`; then `load_or_init`.
pub fn load_settings(
    data_dir: PathBuf,
    credentials: Arc<dyn CredentialStore>,
    os_language: Option<&str>,
) -> (Arc<SettingsService>, LoadOutcome) {
    // RED STUB
    let _ = (data_dir, credentials, os_language);
    todo!("T-030: SettingsDeps + SettingsService::load_or_init")
}

/// Subscribes to the service and emits `settings://changed` with `service.view()`
/// for every received snapshot (J3). Started in `.setup()`.
pub fn spawn_change_bridge<R: Runtime>(app: AppHandle<R>, service: Arc<SettingsService>) {
    // RED STUB: emits nothing.
    let _ = (app, service);
}

/// `settings_get` → `SettingsView` (keys as presence only).
#[tauri::command]
pub fn settings_get(service: State<'_, Arc<SettingsService>>) -> SettingsView {
    // RED STUB: not the service's view.
    let _ = service;
    SettingsView {
        settings: defaults(None),
        keys: KeyPresence::default(),
        first_run: false,
        reset_notice: false,
    }
}

/// `settings_save { request }` → `SaveOutcome`; off the main thread.
#[tauri::command(async)]
pub fn settings_save(
    service: State<'_, Arc<SettingsService>>,
    request: SaveRequest,
) -> SaveOutcome {
    // RED STUB: never calls the service.
    let _ = (service, request);
    SaveOutcome::Refused {
        errors: Vec::new(),
        form_error: Some(FormError::SettingsUnavailable),
    }
}

/// `settings_speech_languages` → core's `WHISPER_ISO_639_1`, in core order (#30).
#[tauri::command]
pub fn settings_speech_languages() -> Vec<&'static str> {
    // RED STUB
    Vec::new()
}
