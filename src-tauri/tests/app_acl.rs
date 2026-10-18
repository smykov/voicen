//! T-049: tauri's ACL over the app's own commands (`#[tauri::command]`, registered
//! in `lib.rs::commands`), through the mock runtime and the release context
//! (`generate_context!(test = true)`: tauri.conf.json, the capabilities and the ACL
//! manifests as tauri-build resolved them into OUT_DIR; tauri-codegen 2.7.1
//! context.rs:377-410). Decision #141: `settings` alone may call the ten settings-page
//! commands, `overlay` alone may call `overlay_ready`, and a command without a
//! permission is refused for every label (tauri default-deny once the app ACL
//! manifest exists, webview/mod.rs:2080). Windows CI only (decision #5).
//!
//! Every invoke comes from the window's own URL, as its page's would, so the origin
//! is Local and only the label decides. `get_ipc_response` runs `Webview::on_message`,
//! the same check a real webview's invoke goes through (tauri test/mod.rs:302).
//!
//! No call here has a side effect: a refused call never reaches the handler, and
//! every command that takes an argument gets `{}`, so even an allowed call stops at
//! argument decoding (`invalid args ...`). Each test gets its own `TempDir` as the
//! data dir; no keys are used.
#![cfg(windows)]

use std::path::Path;
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
use voicen_core::settings::gate::SettingsTab;
use voicen_core::settings::service::SettingsService;
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_lib::settings_ipc::load_settings;
use voicen_lib::settings_window;

/// The window labels (contracts/ipc.md), spelled out here, not taken from the code.
const SETTINGS: &str = "settings";
const OVERLAY: &str = "overlay";
/// A label no Voicen window has.
const OTHER: &str = "other";

/// How a cell's own-window call must end when the ACL lets it through.
#[derive(Clone, Copy, Debug)]
enum Owned {
    /// Zero-argument, harmless: resolves.
    Resolves,
    /// Takes an argument and gets `{}`: tauri rejects at argument decoding, so the
    /// handler body never runs.
    InvalidArgs,
    /// Reaches the handler, which may answer with a command error (device
    /// enumeration on the CI runner, docs/decisions/windows-ci-runner.md).
    Reaches,
}

/// Every registered app command with the one label that may call it (decision #141),
/// the args every call here sends, and how the owner's call ends. Literal on purpose:
/// a command added to `lib.rs::commands` must be added here by hand.
const COMMANDS: [(&str, &str, Owned); 11] = [
    ("get_build_info", SETTINGS, Owned::Resolves),
    ("settings_get", SETTINGS, Owned::Resolves),
    ("settings_save", SETTINGS, Owned::InvalidArgs),
    ("settings_test_connection", SETTINGS, Owned::InvalidArgs),
    ("settings_speech_languages", SETTINGS, Owned::Resolves),
    ("settings_list_microphones", SETTINGS, Owned::Reaches),
    ("local_models_list", SETTINGS, Owned::Resolves),
    ("local_model_download", SETTINGS, Owned::InvalidArgs),
    ("local_model_cancel_download", SETTINGS, Owned::InvalidArgs),
    ("local_model_delete", SETTINGS, Owned::InvalidArgs),
    ("overlay_ready", OVERLAY, Owned::Resolves),
];

const LABELS: [&str; 3] = [SETTINGS, OVERLAY, OTHER];

/// What the test-only probe returns when it runs.
const PROBE_RAN: &str = "probe ran";

static SERIAL: Mutex<()> = Mutex::new(());

/// One app at a time: each builds the overlay and settings-opener threads.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

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
/// context, so tauri's real ACL decides.
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

/// The window labelled `label` at its page's URL: `settings` as
/// `settings_window::open` makes it; `overlay` at the overlay page (built by the
/// test: nothing is published, so the overlay thread builds none); any other label
/// at the settings page, so only the label differs from the settings window.
fn window(app: &App<MockRuntime>, label: &str) -> WebviewWindow<MockRuntime> {
    match label {
        SETTINGS => {
            settings_window::open(app.handle(), SettingsTab::Engine, None).expect("open");
            app.get_webview_window(SETTINGS)
                .unwrap_or_else(|| panic!("no `{SETTINGS}` window"))
        }
        OVERLAY => WebviewWindowBuilder::new(app, OVERLAY, WebviewUrl::App("overlay".into()))
            .build()
            .expect("mock webview builds"),
        _ => WebviewWindowBuilder::new(app, label, WebviewUrl::App("settings".into()))
            .build()
            .expect("mock webview builds"),
    }
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

/// The rejection is tauri's ACL refusal of `cmd`: debug builds "`cmd` not allowed
/// ..." (ipc/authority.rs `resolve_access_message`), release builds "Command `cmd`
/// not allowed by ACL" (webview/mod.rs:2101-2104). Both name the command.
fn is_acl_refusal_of(rejection: &Value, cmd: &str) -> bool {
    let text = match rejection {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    text.contains("not allowed") && text.contains(cmd)
}

/// A test-only app command no permission file names.
#[tauri::command]
fn probe() -> &'static str {
    PROBE_RAN
}

#[test]
fn a_registered_command_without_a_permission_is_refused_from_the_settings_window() {
    // Acceptance 2 (failure branch), decision #141: once the app ACL manifest exists,
    // a registered app command that no permission allows is refused, even for the
    // `settings` window under the real capabilities. `build_app` replaces any
    // invoke handler set on the builder (tauri `Builder::invoke_handler`), so the
    // probe is registered on the mock builder alone and the app built with the
    // release context; the ACL is the context's, the same as `run()`'s. Red today:
    // the release ACL has no `__app-acl__` key, so the probe runs and returns
    // PROBE_RAN. Bite: no app permission defined (no src-tauri/permissions/ file,
    // or an empty AppManifest in build.rs), which turns tauri's app-command check
    // off for every command at once.
    let _serial = serial();
    let app = mock_builder()
        .invoke_handler(tauri::generate_handler![probe])
        .build(release_context())
        .expect("mock app builds");
    let settings = WebviewWindowBuilder::new(&app, SETTINGS, WebviewUrl::App("settings".into()))
        .build()
        .expect("mock webview builds");

    match invoke(&settings, "probe", json!({})) {
        Err(rejection) => assert!(
            is_acl_refusal_of(&rejection, "probe"),
            "probe rejected, but not by the ACL: {rejection}"
        ),
        Ok(value) => {
            panic!("probe (no permission) ran for `{SETTINGS}` under the release ACL: {value}")
        }
    }
}

#[test]
fn every_app_command_is_refused_from_every_label_but_its_window() {
    // Acceptance 1 (refused side), decision #141: each of the 11 app commands is
    // refused by the ACL for every label but its owner: `overlay` and `other` for the
    // ten settings commands, `settings` and `other` for overlay_ready: 11 x 2 = 22
    // cells (the analysis's "30" miscounts). All red today: no app command is
    // ACL-checked from a local origin. Bite: a wildcard or extra label in a
    // capability's `windows`, the settings permission granted in overlay.json (or the
    // overlay one in default.json), a command placed in the other window's
    // permission file.
    let _serial = serial();
    let dir = TempDir::new();
    let app = release_app(&load(dir.path()));
    let windows: Vec<(&str, WebviewWindow<MockRuntime>)> =
        LABELS.iter().map(|&l| (l, window(&app, l))).collect();

    let mut allowed = Vec::new();
    let mut cells = 0;
    for (cmd, owner, _) in COMMANDS {
        for (label, w) in &windows {
            if *label == owner {
                continue;
            }
            cells += 1;
            match invoke(w, cmd, json!({})) {
                Err(rejection) if is_acl_refusal_of(&rejection, cmd) => {}
                outcome => allowed.push(format!("{cmd} from `{label}`: {outcome:?}")),
            }
        }
    }
    assert_eq!(cells, 22, "11 commands x 3 labels minus the 11 owner cells");
    assert!(
        allowed.is_empty(),
        "app commands not refused by the ACL for a non-owner label ({} of {cells}):\n{}",
        allowed.len(),
        allowed.join("\n")
    );
}

#[test]
fn every_app_command_passes_the_acl_from_its_own_window() {
    // Acceptance 1 (allowed side), decision #141: each app command invoked from its
    // owner window is not refused by the ACL: the call reaches tauri's command
    // dispatch (zero-argument commands resolve; the ones taking an argument get `{}`
    // and fail at argument decoding, so no handler body runs). Green today (nothing
    // is checked); it keeps a missing grant from shipping, which the mocked-IPC UI
    // e2e tests cannot see. Bite: a command left out of its window's permission file,
    // the permission not named in the window's capability, or a capability scoped to
    // another label.
    let _serial = serial();
    let dir = TempDir::new();
    let app = release_app(&load(dir.path()));
    let settings = window(&app, SETTINGS);
    let overlay = window(&app, OVERLAY);

    let mut wrong = Vec::new();
    for (cmd, owner, owned) in COMMANDS {
        let w = if owner == SETTINGS {
            &settings
        } else {
            &overlay
        };
        let outcome = invoke(w, cmd, json!({}));
        let ok = match (&outcome, owned) {
            (Err(rejection), _) if is_acl_refusal_of(rejection, cmd) => false,
            (Ok(_), Owned::Resolves | Owned::Reaches) => true,
            (Err(_), Owned::Reaches) => true,
            (Err(rejection), Owned::InvalidArgs) => rejection
                .as_str()
                .is_some_and(|s| s.contains("invalid args") && s.contains(cmd)),
            _ => false,
        };
        if !ok {
            wrong.push(format!("{cmd} from `{owner}` ({owned:?}): {outcome:?}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "app commands that did not pass the ACL from their own window:\n{}",
        wrong.join("\n")
    );
}
