//! Settings over IPC (T-030; contracts/ipc.md): the one construction path of the
//! settings service, the commands `settings_get` / `settings_save` /
//! `settings_speech_languages` / `settings_test_connection` (T-046) /
//! `settings_list_microphones` (T-012), and the `settings://changed` bridge.
//!
//! - J1: a key moves only UI -> `settings_save` -> `SettingsService` ->
//!   `CredentialStore`; no response or event carries one (`SettingsView` has presence
//!   only, `SaveRequest`'s deserialize errors are fixed texts).
//! - J3: `settings://changed` is emitted only by the subscribe bridge, so every
//!   `Saved`, from any caller, gives exactly one event and a `Refused` none.
//! - J4: the release app and the tests build the service through
//!   [`load_settings_with`] ([`load_settings`] is it with a registrar no hotkey
//!   thread is attached to) and differ only in the injected credential store,
//!   autostart entry, hotkey registrar and data dir; it reconciles the autostart
//!   entry right after the load (T-014).
//!
//! The only output here goes to the one log as typed lines (T-008, spec 004 R-11):
//! the load outcome and the reconcile action from [`load_settings`], one save line
//! per [`settings_save`] (outcome, field ids and codes), one test line per
//! [`settings_test_connection`] (result kind, latency on `ok`; no host; a
//! `test_connection_failed` warning when its task cannot finish), and a
//! `change_bridge_failed`
//! warning with the OS code when [`spawn_change_bridge`] cannot start its thread. No
//! key, transcript, settings value or base URL can reach a line: the events have no
//! field for one (docs/decisions/settings.md, docs/decisions/diagnostics-log.md).

use std::path::PathBuf;
use std::sync::{Arc, Weak};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime, State};
use voicen_core::autostart::Autostart;
use voicen_core::clock::SystemClock;
use voicen_core::connection_test::{ConnectionTestRequest, ConnectionTestResult};
use voicen_core::diag::{Log, LogEvent, WarningKind};
use voicen_core::hotkey_registrar::HotkeyRegistrar;
use voicen_core::local_models::store::ModelStore;
use voicen_core::platform::AudioSource;
use voicen_core::secrets::CredentialStore;
use voicen_core::settings::file::FsSettingsFile;
use voicen_core::settings::service::{
    SaveOutcome, SaveRequest, SettingsDeps, SettingsService, SettingsView,
};
use voicen_core::settings::{LoadOutcome, WHISPER_ISO_639_1};

use crate::win::hotkey::HotkeyRegistrarHandle;

/// The event every window listens to; payload `SettingsView` (contracts/ipc.md).
pub const SETTINGS_CHANGED: &str = "settings://changed";

/// [`load_settings_with`] over a `HotkeyRegistrarHandle` that no hotkey thread is
/// ever attached to: every save of a changed hotkey is refused with
/// `hotkey.unavailable` (fail closed). For the tests that start no dictation;
/// `run()` passes its one handle through `load_settings_with`.
pub fn load_settings(
    data_dir: PathBuf,
    credentials: Arc<dyn CredentialStore>,
    autostart: Arc<dyn Autostart>,
    local_models: Arc<ModelStore>,
    os_language: Option<&str>,
    log: &Log,
) -> (Arc<SettingsService>, LoadOutcome) {
    load_settings_with(
        data_dir,
        credentials,
        autostart,
        local_models,
        HotkeyRegistrarHandle::new(),
        os_language,
        log,
    )
}

/// Builds the service the release app and the tests share (J4): `FsSettingsFile`
/// over `data_dir`, the given credential store, autostart entry, models store and
/// hotkey registrar (T-055: the `HotkeyRegistrarHandle` that `start_dictation` later
/// attaches its hotkey thread to, the same instance as `DictationPorts::hotkeys`),
/// `SystemClock`; then `load_or_init`, then `reconcile_autostart` (T-014, R-5).
///
/// `local_models` is the store of the one `LocalModels` (`LocalModels::store`,
/// T-044): it becomes `SettingsDeps.local_models`.
///
/// `log` is the one log (`diag::start`, T-008): after the load it gets the
/// `settings load outcome=..` line, after the reconcile the `autostart reconcile
/// action=..` line (spec 004 R-11), so a start writes Started, then these two. No
/// value of the outcome (settings, backup file name) reaches the log.
pub fn load_settings_with(
    data_dir: PathBuf,
    credentials: Arc<dyn CredentialStore>,
    autostart: Arc<dyn Autostart>,
    local_models: Arc<ModelStore>,
    hotkeys: Arc<dyn HotkeyRegistrar>,
    os_language: Option<&str>,
    log: &Log,
) -> (Arc<SettingsService>, LoadOutcome) {
    let deps = SettingsDeps {
        file: Arc::new(FsSettingsFile::new(data_dir)),
        credentials,
        autostart,
        hotkeys,
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

/// `settings_test_connection { request }` → `ConnectionTestResult` (T-046,
/// FR-017). Core's `test_connection` is blocking (#42: reqwest's blocking client
/// panics on a tokio worker), so it runs on `spawn_blocking`, never on the async
/// runtime's worker or the main thread. Saves nothing; writes one `settings
/// test_connection` line to the managed log (result kind only, no host). Rejects
/// with `{ "code": "ipc.unavailable" }` only when the blocking task cannot finish
/// (contracts/ipc.md "Errors"), and then writes one `warning
/// kind=test_connection_failed` line instead; malformed args reject with core's
/// fixed text.
#[tauri::command]
pub async fn settings_test_connection(
    service: State<'_, Arc<SettingsService>>,
    log: State<'_, Arc<Log>>,
    request: ConnectionTestRequest,
) -> Result<ConnectionTestResult, IpcUnavailable> {
    let service = Arc::clone(&service);
    let log = Arc::clone(&log);
    let task_log = Arc::clone(&log);
    tauri::async_runtime::spawn_blocking(move || {
        let result = service.test_connection(request);
        task_log.write(LogEvent::settings_test_connection(&result));
        result
    })
    .await
    .map_err(|_join_error| {
        // The task panicked or was cancelled: its own line was never written, so
        // one typed warning (no cause text) is the trace (T-046 review 1 #6).
        log.write(LogEvent::Warning {
            kind: WarningKind::TestConnectionFailed,
            os_code: None,
        });
        IpcUnavailable::new()
    })
}

/// The rejection of a command that cannot run at all: `{ "code":
/// "ipc.unavailable" }` (contracts/ipc.md "Errors"); never carries the cause.
#[derive(Debug, Serialize)]
pub struct IpcUnavailable {
    code: &'static str,
}

impl IpcUnavailable {
    fn new() -> IpcUnavailable {
        IpcUnavailable {
            code: "ipc.unavailable",
        }
    }
}

/// `settings_speech_languages` → core's `WHISPER_ISO_639_1`, in core order (#30).
#[tauri::command]
pub fn settings_speech_languages() -> Vec<&'static str> {
    WHISPER_ISO_639_1.to_vec()
}

/// The microphones the settings list (T-012): the same `AudioSource` the
/// dictation session opens, managed by `wire` (one list for a press and the UI).
pub struct Microphones(pub Arc<dyn AudioSource>);

/// One entry of `settings_list_microphones` (contracts/ipc.md
/// `[{ id, name, is_default }]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MicrophoneEntry {
    /// The endpoint id (`settings::Microphone::id`).
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// `source.devices()` as entries, in the source's order. A list that cannot be
/// read rejects with [`IpcUnavailable`] (`{ "code": "ipc.unavailable" }`, no
/// device or OS text), never an empty list: "no microphones" and "the list could
/// not be read" stay apart, and the Recording tab shows `error.ipc_unavailable`.
pub fn list_microphones(source: &dyn AudioSource) -> Result<Vec<MicrophoneEntry>, IpcUnavailable> {
    source
        .devices()
        .map(|devices| {
            devices
                .into_iter()
                .map(|d| MicrophoneEntry {
                    id: d.id.0,
                    name: d.name,
                    is_default: d.is_default,
                })
                .collect()
        })
        .map_err(|_capture_error| IpcUnavailable::new())
}

/// `settings_list_microphones` → [`list_microphones`] over the managed
/// [`Microphones`]. Async: the enumeration may take up to the open budget.
#[tauri::command(async)]
pub fn settings_list_microphones(
    microphones: State<'_, Microphones>,
) -> Result<Vec<MicrophoneEntry>, IpcUnavailable> {
    list_microphones(microphones.0.as_ref())
}
