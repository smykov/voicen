//! T-008: the shell start of the local log (FR-20, FR-18; spec 006 US2) over the
//! shell's one construction path and app wiring, through Tauri's mock runtime.
//! Windows CI only (decision #5).
//!
//! - `diag::start` opens the one log and writes `Started` as the first line.
//! - `load_settings` (the path `run()` and the tests share, J4) then writes the
//!   settings load line and the autostart reconcile line (spec 004 R-11), so the log
//!   of a start reads: Started, load, reconcile.
//! - `build_app` manages the log; `settings_save` writes one save line per save,
//!   field ids and codes only (decision #30: no base URL or query).
//! - An unwritable logs dir stops nothing: `start` returns, one `on_unwritable`
//!   call, the settings load and save work.
//!
//! That `run()` passes `paths::log_dir()` (= `%LOCALAPPDATA%\Voicen\logs`) is the
//! resolver test `settings_ipc::data_dir_is_localappdata_voicen_and_logs_live_under_it`
//! (read only) plus the install smoke, which reads the start line from that
//! folder. Each test here uses its own `TempDir`; the runner's real
//! `%LOCALAPPDATA%\Voicen` is never written. Keys and hosts are fake.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::webview::InvokeRequest;
use tauri::{App, WebviewWindow, WebviewWindowBuilder};
use voicen_core::autostart::{Autostart, FakeAutostart};
use voicen_core::diag::{Log, LogsUnwritableReason, OnUnwritable};
use voicen_core::local_models::catalog::MODELS;
use voicen_core::local_models::service::LocalModels;
use voicen_core::local_models::store::ModelStore;
use voicen_core::secrets::{CredentialStore, FakeCredentialStore};
use voicen_core::settings::file::SETTINGS_FILE;
use voicen_core::settings::service::SettingsService;
use voicen_core::settings::{defaults, EngineKind, LoadOutcome, Settings};
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_lib::settings_ipc::load_settings;

/// Obviously fake; must never reach the log.
const KEY: &str = "sk-test-SHELL-LOG-0000";
const QUERY_SECRET: &str = "SECRETQ";

/// What `on_unwritable` received.
#[derive(Default)]
struct Calls(Mutex<Vec<LogsUnwritableReason>>);

impl Calls {
    fn reasons(&self) -> Vec<LogsUnwritableReason> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

fn recorder() -> (Arc<Calls>, OnUnwritable) {
    let calls = Arc::new(Calls::default());
    let sink = Arc::clone(&calls);
    let cb: OnUnwritable = Box::new(move |reason| {
        sink.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(reason);
    });
    (calls, cb)
}

fn no_keys() -> Arc<dyn CredentialStore> {
    Arc::new(FakeCredentialStore::new())
}

/// T-044: the store `load_settings` takes; nothing is downloaded here.
fn idle_store(data_dir: &Path) -> Arc<ModelStore> {
    Arc::new(ModelStore::new(data_dir.join("models"), MODELS))
}

/// T-044: the one `LocalModels` that `build_app` manages; its models dir lies under
/// a throwaway dir that is removed at once, so it is never created.
fn idle_models() -> Arc<LocalModels> {
    let (models, cleanup) = LocalModels::open(
        TempDir::new().path().join("models"),
        FakeDisk::with_available(u64::MAX),
        Timeouts::default(),
        MODELS,
    );
    cleanup.expect("cleanup over a missing models dir");
    Arc::new(models)
}

/// The lines of `logs/voicen.log`.
fn log_lines(logs: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(logs.join("voicen.log"))
        .unwrap_or_else(|e| panic!("read {}: {e}", logs.join("voicen.log").display()));
    text.lines().map(str::to_string).collect()
}

/// The message of a line: what follows `<timestamp> <LEVEL> `.
fn message(line: &str) -> String {
    line.splitn(3, ' ')
        .nth(2)
        .unwrap_or_else(|| panic!("not `<ts> <LEVEL> <message>`: {line:?}"))
        .to_string()
}

struct Harness {
    _app: App<MockRuntime>,
    webview: WebviewWindow<MockRuntime>,
}

/// The app as `run()` builds it (`build_app`), with `log` managed.
fn harness(service: &Arc<SettingsService>, log: Arc<Log>) -> Harness {
    let app = voicen_lib::build_app(
        mock_builder(),
        mock_context(noop_assets()),
        service.clone(),
        idle_models(),
        log,
    )
    .expect("mock app builds");
    let webview = WebviewWindowBuilder::new(&app, "settings", Default::default())
        .build()
        .expect("mock webview builds");
    Harness { _app: app, webview }
}

impl Harness {
    /// `settings_save` like the UI: `settings`, the API key slot as given, the
    /// other slots `Untouched`.
    #[track_caller]
    fn save(&self, settings: &Settings, api_key: Value) -> Value {
        let url = if cfg!(windows) {
            "http://tauri.localhost"
        } else {
            "tauri://localhost"
        };
        let args = json!({ "request": {
            "settings": settings,
            "keys": {
                "transcription_api": api_key,
                "local_server": "Untouched",
                "post_processing": "Untouched",
            },
        } });
        get_ipc_response(
            &self.webview,
            InvokeRequest {
                cmd: "settings_save".into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: url.parse().expect("url"),
                body: InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        )
        .map(|body| body.deserialize::<Value>().expect("response is JSON"))
        .unwrap_or_else(|e| panic!("settings_save rejected: {e:?}"))
    }
}

fn edited(history_size: u32) -> Settings {
    let mut settings = defaults(None);
    settings.history.size = history_size;
    settings
}

#[test]
fn start_writes_started_first_then_the_load_and_reconcile_lines() {
    // FR-18 / spec 006 FR-003 (the first line of a session is the start line) and
    // R-11 (the load outcome and the reconcile action, which load_settings
    // discarded before T-008). A first run with a leftover Run value: FirstRun,
    // then `removed`. Bite: Started not written by start, written after the
    // settings lines, the load or reconcile line missing, the reconcile action
    // dropped again, a stderr line instead of a log line.
    let dir = TempDir::new();
    let logs = dir.path().join("logs");
    let (calls, cb) = recorder();
    let log = voicen_lib::diag::start(logs.clone(), cb);
    let leftover: Arc<dyn Autostart> = Arc::new(FakeAutostart::enabled());
    let (_service, outcome) = load_settings(
        dir.path().to_path_buf(),
        no_keys(),
        leftover,
        idle_store(dir.path()),
        None,
        &log,
    );
    assert_eq!(outcome, LoadOutcome::FirstRun(defaults(None)));

    let messages: Vec<String> = log_lines(&logs).iter().map(|l| message(l)).collect();
    let info = voicen_core::build_info();
    assert_eq!(
        messages,
        vec![
            format!("voicen {info} started pid={}", std::process::id()),
            "settings load outcome=first_run".to_string(),
            "autostart reconcile action=removed".to_string(),
        ]
    );
    assert_eq!(calls.reasons(), vec![]);
}

#[test]
fn an_unwritable_logs_dir_does_not_stop_the_start() {
    // Acceptance failure branch: the logs folder cannot be written (here a file at
    // its path; the ACL-denied folder is T-054's) -> the app keeps working and the
    // log reports it once. Bite: start panicking or never returning, a callback per
    // write, the settings load or save failing because of the log, the file at the
    // logs path overwritten.
    let dir = TempDir::new();
    let logs = dir.path().join("logs");
    std::fs::write(&logs, b"not a directory\n").expect("plant a file at the logs path");
    let (calls, cb) = recorder();
    let log = voicen_lib::diag::start(logs.clone(), cb);
    assert_eq!(calls.reasons(), vec![LogsUnwritableReason::NotADirectory]);

    let (service, outcome) = load_settings(
        dir.path().to_path_buf(),
        no_keys(),
        Arc::new(FakeAutostart::new()),
        idle_store(dir.path()),
        None,
        &log,
    );
    assert_eq!(outcome, LoadOutcome::FirstRun(defaults(None)));
    assert!(dir.path().join(SETTINGS_FILE).is_file(), "settings.json written");
    let h = harness(&service, Arc::clone(&log));
    let saved = h.save(&edited(42), json!("Untouched"));
    assert_eq!(
        saved["Saved"]["view"]["settings"]["history"]["size"],
        json!(42),
        "{saved}"
    );

    assert_eq!(
        calls.reasons(),
        vec![LogsUnwritableReason::NotADirectory],
        "still one callback"
    );
    assert_eq!(
        std::fs::read(&logs).expect("still a file"),
        b"not a directory\n"
    );
}

#[test]
fn settings_save_writes_one_line_per_save_without_values() {
    // T-008 seam: settings_save writes the save line (spec 004 T035 / R-11):
    // outcome, warnings and errors as field ids and codes; decision #30: the base
    // URL's query string (and the URL, model and key) never reach the log. Bite: no
    // save line, the SaveOutcome or the request formatted into the log, a line only
    // for Saved.
    let dir = TempDir::new();
    let logs = dir.path().join("logs");
    let (calls, cb) = recorder();
    let log = voicen_lib::diag::start(logs.clone(), cb);
    let (service, _) = load_settings(
        dir.path().to_path_buf(),
        no_keys(),
        Arc::new(FakeAutostart::new()),
        idle_store(dir.path()),
        None,
        &log,
    );
    let h = harness(&service, Arc::clone(&log));

    let mut api = defaults(None);
    api.engine = EngineKind::Api;
    api.api.base_url = format!("http://192.0.2.10:8000/v1?api-version={QUERY_SECRET}");
    api.api.model = "MODEL-MARKER".to_string();
    let saved = h.save(&api, json!({ "Replace": KEY }));
    assert!(saved["Saved"].is_object(), "expected Saved: {saved}");
    let refused = h.save(&edited(0), json!("Untouched"));
    assert!(refused["Refused"].is_object(), "expected Refused: {refused}");

    let lines = log_lines(&logs);
    let saves: Vec<String> = lines
        .iter()
        .map(|l| message(l))
        .filter(|m| m.starts_with("settings save "))
        .collect();
    assert_eq!(
        saves,
        vec![
            "settings save outcome=ok warnings=engine.api.base_url:endpoint.insecure".to_string(),
            "settings save outcome=refused errors=history.size:history.size_range".to_string(),
        ],
        "{lines:#?}"
    );
    let text = lines.join("\n");
    for value in [KEY, QUERY_SECRET, "192.0.2.10", "MODEL-MARKER"] {
        assert!(!text.contains(value), "{value} in the log:\n{text}");
    }
    assert_eq!(calls.reasons(), vec![]);
}
