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
//!   and differ only in the injected credential store, autostart entry and data
//!   dir; it reconciles the autostart entry right after the load (T-014).
//!
//! The only output here goes to the one log as typed lines (T-008, spec 004 R-11):
//! the load outcome and the reconcile action from [`load_settings`], one save line
//! per [`settings_save`] (outcome, field ids and codes), and a `change_bridge_failed`
//! warning with the OS code when [`spawn_change_bridge`] cannot start its thread. No
//! key, transcript, settings value or base URL can reach a line: the events have no
//! field for one (docs/decisions/settings.md, docs/decisions/diagnostics-log.md).

use std::path::PathBuf;
use std::sync::{Arc, Weak};

use tauri::{AppHandle, Emitter, Runtime, State};
use voicen_core::autostart::Autostart;
use voicen_core::clock::SystemClock;
use voicen_core::diag::{Log, LogEvent, WarningKind};
use voicen_core::hotkey_registrar::{HotkeyRegistrar, Prepared, Unavailable};
use voicen_core::local_models::store::ModelStore;
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
/// over `data_dir`, the given credential store, autostart entry and models store,
/// the interim fail-closed `HotkeyRegistrar`, `SystemClock`; then
/// `load_or_init`, then `reconcile_autostart` (T-014, R-5).
///
/// `local_models` is the store of the one `LocalModels` (`LocalModels::store`,
/// T-044): it becomes `SettingsDeps.local_models`.
///
/// `log` is the one log (`diag::start`, T-008): after the load it gets the
/// `settings load outcome=..` line, after the reconcile the `autostart reconcile
/// action=..` line (spec 004 R-11), so a start writes Started, then these two. No
/// value of the outcome (settings, backup file name) reaches the log.
pub fn load_settings(
    data_dir: PathBuf,
    credentials: Arc<dyn CredentialStore>,
    autostart: Arc<dyn Autostart>,
    local_models: Arc<ModelStore>,
    os_language: Option<&str>,
    log: &Log,
) -> (Arc<SettingsService>, LoadOutcome) {
    let deps = SettingsDeps {
        file: Arc::new(FsSettingsFile::new(data_dir)),
        credentials,
        autostart,
        hotkeys: Arc::new(InterimHotkeyRegistrar),
        local_models,
        clock: Arc::new(SystemClock),
    };
    let (service, outcome) = SettingsService::load_or_init(deps, os_language);
    log.write(LogEvent::settings_load(&outcome));
    // R-5: the Run value follows the saved setting (no call while Unavailable). The
    // action goes to the start log (R-11); nothing else depends on it.
    log.write(LogEvent::AutostartReconcile(service.reconcile_autostart()));
    (Arc::new(service), outcome)
}

/// Subscribes to the service and emits `settings://changed` with `service.view()`
/// for every received snapshot (J3). Started by `build_app` right after `build()`.
///
/// The subscription is taken before this returns, so no `Saved` after the call is
/// missed. The thread holds the service only weakly, but the `AppHandle` it owns
/// keeps the managed service alive, so in practice the thread lives as long as the
/// app (the process). An emit error is ignored: it never turns a `Saved` into a
/// failure. A failed thread spawn writes one `change_bridge_failed` warning with the
/// OS code to `log` (never the error text).
pub fn spawn_change_bridge<R: Runtime>(
    app: AppHandle<R>,
    service: Arc<SettingsService>,
    log: &Log,
) {
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
        log.write(LogEvent::Warning {
            kind: WarningKind::ChangeBridgeFailed,
            os_code: err.raw_os_error(),
        });
    }
}

/// `settings_get` → `SettingsView` (keys as presence only).
#[tauri::command]
pub fn settings_get(service: State<'_, Arc<SettingsService>>) -> SettingsView {
    service.view()
}

/// `settings_save { request }` → `SaveOutcome`. Runs off the main thread: a save
/// is up to nine credential calls and a file write. Every outcome writes one
/// `settings save` line to the managed log (T-008; field ids and codes only).
#[tauri::command(async)]
pub fn settings_save(
    service: State<'_, Arc<SettingsService>>,
    log: State<'_, Arc<Log>>,
    request: SaveRequest,
) -> SaveOutcome {
    let outcome = service.save(request);
    log.write(LogEvent::settings_save(&outcome));
    outcome
}

/// `settings_speech_languages` → core's `WHISPER_ISO_639_1`, in core order (#30).
#[tauri::command]
pub fn settings_speech_languages() -> Vec<&'static str> {
    WHISPER_ISO_639_1.to_vec()
}

/// Interim until T-055 (the real registrar): fails closed. `SettingsService` does
/// not call it before T-010 adds the hotkey save step.
struct InterimHotkeyRegistrar;

impl HotkeyRegistrar for InterimHotkeyRegistrar {
    fn prepare(&self, _hotkey: Hotkey, _mode: Mode) -> Result<Prepared, Unavailable> {
        Err(Unavailable)
    }

    fn commit(&self, _prepared: Prepared) {}

    fn abort(&self, _prepared: Prepared) {}
}
