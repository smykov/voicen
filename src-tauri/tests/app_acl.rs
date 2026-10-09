//! T-049: the app commands (`#[tauri::command]`, lib.rs `commands()`) are checked by
//! tauri's real ACL against the capabilities, per window label (invariant: every
//! command has an `allow-<command>` permission from build.rs `APP_COMMANDS`; label
//! `settings` is granted the ten settings / local-model / build-info commands,
//! label `overlay` only `overlay_ready`; any other (command, label) pair and any
//! command without a permission is refused). Windows CI only (decision #5).
//!
//! Every app here is built with the release context (`generate_context!(test =
//! true)`: tauri.conf.json and the ACL that tauri-build resolved into OUT_DIR) on
//! the mock runtime, so the real ACL decides; `mock_context` has no app ACL and
//! cannot show any of this. Invokes come from the window's own URL, like its page.
//!
//! Each test gets its own `TempDir` as the data dir; no keys are used; nothing is
//! downloaded (`local_model_download` asks for an id outside the catalog). The Linux
//! twin of these tests is `scripts/ci/app-acl.sh` (`make check-app-acl`): it keeps the
//! command, `generate_handler!`, `APP_COMMANDS` and capability `allow-*` sets equal,
//! but only these tests prove which label gets what.
#![cfg(windows)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{get_ipc_response, mock_builder, MockRuntime};
use tauri::webview::InvokeRequest;
use tauri::{App, Context, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use voicen_core::autostart::FakeAutostart;
use voicen_core::diag::Log;
use voicen_core::local_models::catalog::MODELS;
use voicen_core::local_models::service::LocalModels;
use voicen_core::local_models::store::ModelStore;
use voicen_core::secrets::FakeCredentialStore;
use voicen_core::settings::file::SETTINGS_FILE;
use voicen_core::settings::gate::SettingsTab;
use voicen_core::settings::service::SettingsService;
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_lib::settings_ipc::load_settings;
use voicen_lib::settings_window::{self, LABEL as SETTINGS};

/// The overlay window's label (spec 001 contracts/ipc.md).
const OVERLAY: &str = "overlay";

/// A label no capability names; its page is the settings page (the ACL decides by
/// label, not by URL).
const OTHER: &str = "other";

/// The ten commands the settings page invokes (T-049 Investigation, command
/// inventory), each with args that have no side effect: a read, an id outside the
/// catalog, a model that is not downloaded, or args the command cannot parse (still
/// a call that reached the command: tauri parses args after the ACL).
fn settings_commands() -> Vec<(&'static str, Value)> {
    vec![
        ("get_build_info", json!({})),
        ("settings_get", json!({})),
        ("settings_save", json!({})),
        ("settings_speech_languages", json!({})),
        ("settings_list_microphones", json!({})),
        ("settings_test_connection", json!({})),
        ("local_models_list", json!({})),
        ("local_model_download", json!({ "id": "medium" })),
        ("local_model_cancel_download", json!({ "id": "base" })),
        ("local_model_delete", json!({ "id": "base" })),
    ]
}

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The one `LocalModels`; its models dir lies under a throwaway dir that is removed
/// at once, so it is never created.
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

/// A log that cannot be written (its folder lies under this test exe, a file).
fn discard_log() -> Arc<Log> {
    let exe = std::env::current_exe().expect("current_exe");
    voicen_lib::diag::start(exe.join("logs"), Box::new(|_| {}))
}

fn load(dir: &Path) -> Arc<SettingsService> {
    load_settings(
        dir.to_path_buf(),
        Arc::new(FakeCredentialStore::new()),
        Arc::new(FakeAutostart::new()),
        Arc::new(ModelStore::new(dir.join("models"), MODELS)),
        None,
        &discard_log(),
    )
    .0
}

/// The release context, for the mock runtime.
fn release_context() -> Context<MockRuntime> {
    tauri::generate_context!(test = true)
}

/// The app as `run()` builds it (`build_app`), on the mock runtime with the release
/// context (so tauri's real ACL decides).
fn release_app(service: &Arc<SettingsService>) -> App<MockRuntime> {
    voicen_lib::build_app(
        mock_builder(),
        release_context(),
        Arc::clone(service),
        idle_models(),
        discard_log(),
    )
    .expect("mock app builds")
}

/// The settings window as the product makes it (`settings_window::open`).
fn settings_window(app: &App<MockRuntime>) -> WebviewWindow<MockRuntime> {
    settings_window::open(app.handle(), SettingsTab::Engine, None).expect("open settings");
    app.get_webview_window(SETTINGS)
        .unwrap_or_else(|| panic!("no `{SETTINGS}` window"))
}

/// A window labelled `label` at the app page `page`, built by the test.
fn window_at(app: &App<MockRuntime>, label: &str, page: &str) -> WebviewWindow<MockRuntime> {
    WebviewWindowBuilder::new(app, label, WebviewUrl::App(page.into()))
        .build()
        .unwrap_or_else(|e| panic!("mock webview `{label}` builds: {e}"))
}

fn url_of(window: &WebviewWindow<MockRuntime>) -> Url {
    window.url().expect("window url")
}

/// Invokes `cmd` from `window`, from the window's own URL, like its page does.
/// `Ok` = resolved value, `Err` = rejection (an ACL refusal is a rejection).
fn invoke(window: &WebviewWindow<MockRuntime>, cmd: &str, args: Value) -> Result<Value, Value> {
    get_ipc_response(
        window,
        InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: url_of(window),
            body: InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    )
    .map(|body| body.deserialize::<Value>().expect("response is JSON"))
}

/// The rejection is tauri's ACL refusal (debug text "<cmd> not allowed ...",
/// tauri 2.12.1 ipc/authority.rs `resolve_access_message`).
fn is_acl_refusal(rejection: &Value) -> bool {
    rejection.to_string().contains("not allowed")
}

/// The rejection is tauri's "Command <cmd> not found" (no handler registered).
fn is_not_found(rejection: &Value) -> bool {
    rejection.to_string().contains("not found")
}

/// `cmd` from `window` is refused by the ACL, before its handler.
#[track_caller]
fn assert_refused(window: &WebviewWindow<MockRuntime>, cmd: &str, args: Value) {
    let label = window.label().to_string();
    match invoke(window, cmd, args) {
        Err(rejection) => assert!(
            is_acl_refusal(&rejection),
            "{cmd} from `{label}` rejected, but not by the ACL: {rejection}"
        ),
        Ok(value) => panic!("{cmd} from `{label}` reached its handler past the ACL: {value}"),
    }
}

/// The bytes of `settings.json` in `dir`, if there is one.
fn settings_file(dir: &Path) -> Option<Vec<u8>> {
    std::fs::read(dir.join(SETTINGS_FILE)).ok()
}

#[test]
fn settings_window_reaches_every_settings_command_past_the_acl() {
    // Acceptance 1, pass branch (guard: green today, must stay green after the
    // change). From the window `settings_window::open` made, each of the ten
    // commands of the settings page reaches its handler: a value or the command's
    // own error, never an ACL refusal or "Command .. not found". Bite: a missing
    // `allow-<cmd>` grant for `settings` in capabilities/default.json (e.g.
    // `allow-get-build-info`, T-060 (iii)), a command missing from APP_COMMANDS, or
    // a command no longer registered in `commands()`.
    let _serial = serial();
    let dir = TempDir::new();
    let app = release_app(&load(dir.path()));
    let window = settings_window(&app);

    for (cmd, args) in settings_commands() {
        if let Err(rejection) = invoke(&window, cmd, args) {
            assert!(
                !is_acl_refusal(&rejection),
                "{cmd} refused by the ACL for `{SETTINGS}`: {rejection}"
            );
            assert!(
                !is_not_found(&rejection),
                "{cmd} is not registered: {rejection}"
            );
        }
    }

    // The handlers ran: their own answers, not a generic rejection.
    let view = invoke(&window, "settings_get", json!({}))
        .unwrap_or_else(|e| panic!("settings_get rejected for `{SETTINGS}`: {e}"));
    assert!(view.get("settings").is_some(), "settings_get: {view}");
    let info = invoke(&window, "get_build_info", json!({}))
        .unwrap_or_else(|e| panic!("get_build_info rejected for `{SETTINGS}`: {e}"));
    assert!(info.is_object(), "get_build_info: {info}");
    let languages = invoke(&window, "settings_speech_languages", json!({}))
        .unwrap_or_else(|e| panic!("settings_speech_languages rejected: {e}"));
    assert!(
        languages.as_array().is_some_and(|l| !l.is_empty()),
        "settings_speech_languages: {languages}"
    );
    let list = invoke(&window, "local_models_list", json!({}))
        .unwrap_or_else(|e| panic!("local_models_list rejected for `{SETTINGS}`: {e}"));
    assert_eq!(list.as_array().map(Vec::len), Some(MODELS.len()), "{list}");
    assert_eq!(
        invoke(&window, "local_model_download", json!({ "id": "medium" })),
        Err(json!({ "code": "not_in_catalog", "messageKey": "download.not_in_catalog" }))
    );
    assert_eq!(
        invoke(
            &window,
            "local_model_cancel_download",
            json!({ "id": "base" })
        ),
        Ok(json!(false))
    );
    assert_eq!(
        invoke(&window, "local_model_delete", json!({ "id": "base" })),
        Err(json!({ "code": "not_downloaded", "messageKey": "delete.not_downloaded" }))
    );
}

#[test]
fn settings_commands_are_refused_from_any_window_but_settings() {
    // Acceptance 1, refusal branch (red today: without an app ACL manifest tauri
    // checks no local-origin app command, so every one of them resolves). From a
    // window labelled `other` at the settings page and from one labelled `overlay`
    // at the overlay page, each of the ten settings commands is refused by the ACL
    // before its handler. Bite: no `AppManifest::commands` in build.rs (no
    // `__app__` manifest), the settings grants also in capabilities/overlay.json, a
    // window wildcard in default.json, or a `deny-*` (which ignores labels).
    let _serial = serial();
    let dir = TempDir::new();
    let app = release_app(&load(dir.path()));
    let other = window_at(&app, OTHER, "settings");
    let overlay = window_at(&app, OVERLAY, "overlay");

    for window in [&other, &overlay] {
        for (cmd, args) in settings_commands() {
            assert_refused(window, cmd, args);
        }
    }
}

#[test]
fn a_refused_settings_save_changes_nothing() {
    // Acceptance 1, refusal branch, the effect (red today): a well-formed
    // settings_save from `other` that flips auto_paste is refused by the ACL, and
    // neither the service's settings nor settings.json change. Bite: as above; also
    // a refusal that comes only after the save ran.
    let _serial = serial();
    let dir = TempDir::new();
    let service = load(dir.path());
    let app = release_app(&service);
    let other = window_at(&app, OTHER, "settings");
    let before = service.snapshot();
    let file_before = settings_file(dir.path());

    let mut settings = serde_json::to_value(&*before).expect("settings are JSON");
    settings["auto_paste"] = json!(!before.auto_paste);
    let request = json!({ "request": {
        "settings": settings,
        "keys": {
            "transcription_api": "Untouched",
            "local_server": "Untouched",
            "post_processing": "Untouched",
        },
    } });
    assert_refused(&other, "settings_save", request);

    assert_eq!(
        service.snapshot().auto_paste,
        before.auto_paste,
        "a refused settings_save changed the settings"
    );
    assert_eq!(
        settings_file(dir.path()),
        file_before,
        "a refused settings_save wrote {SETTINGS_FILE}"
    );
}

#[test]
fn overlay_ready_is_refused_from_any_window_but_overlay() {
    // Invariant: label `overlay` alone is granted `allow-overlay-ready` (red today:
    // the command resolves from every window). From the settings window and from
    // `other`, `overlay_ready` is refused by the ACL. Its pass from `overlay` is
    // pinned by tests/overlay_ipc.rs. Bite: `allow-overlay-ready` also in
    // capabilities/default.json, no app ACL manifest.
    let _serial = serial();
    let dir = TempDir::new();
    let app = release_app(&load(dir.path()));
    let settings = settings_window(&app);
    let other = window_at(&app, OTHER, "overlay");

    for window in [&settings, &other] {
        assert_refused(window, "overlay_ready", json!({}));
    }
}

/// Set when [`unpermitted_probe`]'s handler runs.
static PROBE_REACHED: AtomicBool = AtomicBool::new(false);

/// A new app command that no `APP_COMMANDS` entry and no capability names.
#[tauri::command]
fn unpermitted_probe() -> &'static str {
    PROBE_REACHED.store(true, Ordering::SeqCst);
    "reached"
}

#[test]
fn a_command_without_a_permission_is_refused_even_from_the_settings_window() {
    // Acceptance 2, the failure branch (red today): an app command registered in
    // `generate_handler!` but given no permission is refused by the ACL from the
    // settings window (fail closed), with the release context's ACL, and its handler
    // never runs. Bite: no `__app__` manifest (tauri then skips the check for every
    // local app command), or a capability granting every command (e.g. a permission
    // set that is not per command).
    let _serial = serial();
    PROBE_REACHED.store(false, Ordering::SeqCst);
    let app = mock_builder()
        .invoke_handler(tauri::generate_handler![unpermitted_probe])
        .build(release_context())
        .expect("mock app builds");
    let window = window_at(&app, SETTINGS, "settings");

    assert_refused(&window, "unpermitted_probe", json!({}));
    assert!(
        !PROBE_REACHED.load(Ordering::SeqCst),
        "unpermitted_probe's handler ran"
    );
}
