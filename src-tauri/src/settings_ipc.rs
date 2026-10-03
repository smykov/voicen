//! Settings over IPC (T-030; contracts/ipc.md): the one construction path of the
//! settings service, the commands `settings_get` / `settings_save` /
//! `settings_speech_languages`, and the `settings://changed` bridge.
//!
//! - J1: a key moves only UI -> `settings_save` -> `SettingsService` ->
//!   `CredentialStore`; no response or event carries one (`SettingsView` has presence
//!   only, `SaveRequest`'s deserialize errors are fixed texts).
//! - J3: `settings://changed` is emitted only by the subscribe bridge, so every
//!   `Saved`, from any caller, gives exactly one event and a `Refused` none.
//! - J4: the release app and the tests build the service through [`load_settings`]
//!   and differ only in the injected credential store and data dir.
//!
//! The only output here is one `eprintln!` in [`spawn_change_bridge`] when the OS
//! refuses to start its thread: a fixed text plus the `std::io::Error`, to stderr
//! (which goes nowhere in a windowed release build until T-008's log exists). No
//! key, transcript, settings value or base URL is printed or logged from this module
//! (docs/decisions/settings.md).

use std::path::PathBuf;
use std::sync::{Arc, Weak};

use tauri::{AppHandle, Emitter, Runtime, State};
use voicen_core::clock::SystemClock;
use voicen_core::hotkey_registrar::{HotkeyRegistrar, Prepared, Unavailable};
use voicen_core::models::DownloadedModels;
use voicen_core::secrets::CredentialStore;
use voicen_core::settings::file::FsSettingsFile;
use voicen_core::settings::hotkey::Hotkey;
use voicen_core::settings::service::{
    SaveOutcome, SaveRequest, SettingsDeps, SettingsService, SettingsView,
};
use voicen_core::settings::{LoadOutcome, Mode, WHISPER_ISO_639_1};

/// The event every window listens to; payload `SettingsView` (contracts/ipc.md).
pub const SETTINGS_CHANGED: &str = "settings://changed";

/// Builds the service the release app and the tests share (J4): `FsSettingsFile`
/// over `data_dir`, the given credential store, the interim `NoDownloadedModels`
/// and fail-closed `HotkeyRegistrar`, `SystemClock`; then `load_or_init`.
pub fn load_settings(
    data_dir: PathBuf,
    credentials: Arc<dyn CredentialStore>,
    os_language: Option<&str>,
) -> (Arc<SettingsService>, LoadOutcome) {
    let deps = SettingsDeps {
        file: Arc::new(FsSettingsFile::new(data_dir)),
        credentials,
        hotkeys: Arc::new(InterimHotkeyRegistrar),
        local_models: Arc::new(NoDownloadedModels),
        clock: Arc::new(SystemClock),
    };
    let (service, outcome) = SettingsService::load_or_init(deps, os_language);
    (Arc::new(service), outcome)
}

/// Subscribes to the service and emits `settings://changed` with `service.view()`
/// for every received snapshot (J3). Started by `build_app` right after `build()`.
///
/// The subscription is taken before this returns, so no `Saved` after the call is
/// missed. The thread holds the service only weakly, but the `AppHandle` it owns
/// keeps the managed service alive, so in practice the thread lives as long as the
/// app (the process). An emit error is ignored: it never turns a `Saved` into a
/// failure. A failed thread spawn prints one line to stderr (see the module doc).
pub fn spawn_change_bridge<R: Runtime>(app: AppHandle<R>, service: Arc<SettingsService>) {
    let changes = service.subscribe();
    let service: Weak<SettingsService> = Arc::downgrade(&service);
    let spawned = std::thread::Builder::new()
        .name("settings-changed".into())
        .spawn(move || {
            for _snapshot in changes {
                let Some(service) = service.upgrade() else {
                    break;
                };
                let _ = app.emit(SETTINGS_CHANGED, service.view());
            }
        });
    if let Err(err) = spawned {
        eprintln!("cannot start the settings change bridge: {err}");
    }
}

/// `settings_get` → `SettingsView` (keys as presence only).
#[tauri::command]
pub fn settings_get(service: State<'_, Arc<SettingsService>>) -> SettingsView {
    service.view()
}

/// `settings_save { request }` → `SaveOutcome`. Runs off the main thread: a save
/// is up to nine credential calls and a file write.
#[tauri::command(async)]
pub fn settings_save(
    service: State<'_, Arc<SettingsService>>,
    request: SaveRequest,
) -> SaveOutcome {
    service.save(request)
}

/// `settings_speech_languages` → core's `WHISPER_ISO_639_1`, in core order (#30).
#[tauri::command]
pub fn settings_speech_languages() -> Vec<&'static str> {
    WHISPER_ISO_639_1.to_vec()
}

/// Interim until T-016 (002's `ModelStore`): no built-in model is downloaded, which
/// is the true state while models cannot be downloaded (`builtin_local` →
/// `model.not_downloaded`).
struct NoDownloadedModels;

impl DownloadedModels for NoDownloadedModels {
    fn is_downloaded(&self, _id: &str) -> bool {
        false
    }

    fn list(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Interim until T-006 (the real registrar): fails closed. `SettingsService` does
/// not call it before T-010 adds the hotkey save step.
struct InterimHotkeyRegistrar;

impl HotkeyRegistrar for InterimHotkeyRegistrar {
    fn prepare(&self, _hotkey: Hotkey, _mode: Mode) -> Result<Prepared, Unavailable> {
        Err(Unavailable)
    }

    fn commit(&self, _prepared: Prepared) {}

    fn abort(&self, _prepared: Prepared) {}
}
