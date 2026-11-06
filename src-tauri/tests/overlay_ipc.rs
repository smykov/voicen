//! T-057: the overlay page's IPC over tauri's mock runtime and the release context
//! (tauri.conf.json and the capabilities as tauri-build resolved them): the
//! capability of label `overlay` (analysis red-test table row 5; decision #45 least
//! privilege) and the `overlay_ready` command (invariant 4; spec 001
//! contracts/ipc.md: the page listens to `overlay://state`, then invokes
//! `overlay_ready` and keeps the payload with the highest `seq`). Windows CI only
//! (decision #5).
//!
//! The mock runtime has no run loop, so a destroyed window is never dropped from the
//! manager (tests/settings_window.rs module docs): these tests never publish Hidden.
//! The window-lifecycle facts are `tests/overlay.rs`'s, on the real Wry runtime.
//!
//! Each test gets its own `TempDir` as the data dir; no keys are used.
#![cfg(windows)]

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{get_ipc_response, mock_builder, MockRuntime};
use tauri::webview::InvokeRequest;
use tauri::{
    App, Context, Listener, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use voicen_core::autostart::FakeAutostart;
use voicen_core::diag::Log;
use voicen_core::local_models::catalog::MODELS;
use voicen_core::local_models::service::LocalModels;
use voicen_core::local_models::store::ModelStore;
use voicen_core::platform::Indicator;
use voicen_core::recording::OverlayState;
use voicen_core::secrets::FakeCredentialStore;
use voicen_core::settings::service::SettingsService;
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_lib::dictation::ShellIndicator;
use voicen_lib::overlay;
use voicen_lib::settings_ipc::load_settings;

/// The overlay window's label (contracts/ipc.md).
const LABEL: &str = "overlay";

/// The overlay's state event (contracts/ipc.md).
const STATE_EVENT: &str = "overlay://state";

/// A hang bound, not a budget (T-173): how long the overlay's receipt may take. It only
/// ends a thread that never answers; it stays below the test runner's own limits.
const HANG: Duration = Duration::from_secs(120);

static SERIAL: Mutex<()> = Mutex::new(());

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

/// The rejection is tauri's ACL refusal (debug text "... not allowed ...").
fn is_acl_refusal(rejection: &Value) -> bool {
    rejection.to_string().contains("not allowed")
}

/// A window labelled `overlay` at the overlay page, built by the test (not by the
/// overlay thread: nothing is published), so only the capability decides.
fn overlay_window(app: &App<MockRuntime>) -> WebviewWindow<MockRuntime> {
    WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("overlay".into()))
        .build()
        .expect("mock webview builds")
}

#[test]
fn overlay_window_may_listen_and_unlisten_its_state_event() {
    // Row 5 / decision #45: capabilities/overlay.json grants label `overlay`
    // core:event:allow-listen and core:event:allow-unlisten (the page's
    // `listen("overlay://state")` and its cleanup), through tauri's real ACL. Red
    // today: no capability names the label. Bite: the capability scoped to another
    // label, or a permission missing.
    let _serial = serial();
    let dir = TempDir::new();
    let app = release_app(&load(dir.path()));
    let window = overlay_window(&app);

    let event_id = invoke(
        &window,
        "plugin:event|listen",
        json!({ "event": STATE_EVENT, "target": { "kind": "Any" }, "handler": 7 }),
    )
    .unwrap_or_else(|e| panic!("listen to {STATE_EVENT} refused for `{LABEL}`: {e}"));
    invoke(
        &window,
        "plugin:event|unlisten",
        json!({ "event": STATE_EVENT, "eventId": event_id }),
    )
    .unwrap_or_else(|e| panic!("unlisten refused for `{LABEL}`: {e}"));
}

#[test]
fn overlay_window_gets_nothing_beyond_listen_and_unlisten() {
    // Decision #45, row 5: the page cannot destroy (or otherwise drive) its own
    // window: the overlay thread is the window's only owner (analysis hypothesis
    // 2(a)). Guard (green before T-057: no capability at all; must stay green after).
    // Bite: core:window:allow-destroy, core:default (or core:window:default /
    // core:app:default) in capabilities/overlay.json, or the overlay label added to
    // default.json.
    let _serial = serial();
    let dir = TempDir::new();
    let app = release_app(&load(dir.path()));
    let window = overlay_window(&app);

    for (cmd, args) in [
        ("plugin:window|destroy", json!({ "label": LABEL })),
        ("plugin:window|close", json!({ "label": LABEL })),
        ("plugin:window|show", json!({ "label": LABEL })),
        ("plugin:window|set_focus", json!({ "label": LABEL })),
        ("plugin:window|title", json!({ "label": LABEL })),
        ("plugin:app|version", json!({})),
        (
            "plugin:event|emit",
            json!({ "event": STATE_EVENT, "payload": null }),
        ),
    ] {
        match invoke(&window, cmd, args) {
            Err(rejection) => assert!(
                is_acl_refusal(&rejection),
                "{cmd} rejected, but not by the ACL: {rejection}"
            ),
            Ok(value) => panic!("{cmd} allowed for `{LABEL}` beyond decision #45: {value}"),
        }
    }
}

/// Every `overlay://state` payload emitted so far, in order (a Rust `listen_any`
/// handler sees an event emitted to any target).
fn emitted(app: &App<MockRuntime>) -> Arc<Mutex<Vec<Value>>> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    app.listen_any(STATE_EVENT, move |event| {
        let payload: Value =
            serde_json::from_str(event.payload()).expect("overlay://state payload is JSON");
        sink.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(payload);
    });
    events
}

fn last(events: &Arc<Mutex<Vec<Value>>>) -> Option<Value> {
    events
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .last()
        .cloned()
}

fn kind(payload: &Value) -> Option<&str> {
    payload.pointer("/state/kind").and_then(Value::as_str)
}

#[test]
fn overlay_ready_returns_the_newest_published_state_as_the_last_emit_carries_it() {
    // Invariant 4 (contracts/ipc.md `overlay_ready`): a page that was not listening
    // yet catches up through `overlay_ready`, which returns `overlay_payload` of the
    // overlay mailbox's newest state: the same seq and state as the last
    // `overlay://state` emit, in the settings' ui_language, with a newer seq after a
    // newer state. Red today: no overlay window is built, no emit, no command. Bite:
    // `overlay_ready` reading a copy other than the emits' mailbox (stale state or
    // seq), a constant seq, a hard-coded language, or no emit after the build.
    let _serial = serial();
    let dir = TempDir::new();
    let service = load(dir.path());
    let app = release_app(&service);
    let events = emitted(&app);
    let lang = serde_json::to_value(service.snapshot().ui_language).expect("lang is JSON");
    let indicator = ShellIndicator::new(app.handle());

    indicator.set_overlay(&OverlayState::Recording);
    // T-173: decided through the overlay thread's receipt, never a wall-clock ceiling;
    // the window and the emit are read once, after it.
    let receipt = overlay::settled(app.handle(), HANG);
    assert!(
        receipt.is_some_and(|r| !r.failed),
        "Recording: the overlay receipt (`overlay::settled`) did not report a handled, \
         unfailed state within the hang bound {HANG:?}: {receipt:?} (emitted: {:?})",
        last(&events)
    );
    assert!(
        app.get_webview_window(LABEL).is_some()
            && last(&events).as_ref().and_then(kind) == Some("recording"),
        "the receipt settled on Recording but the overlay has no `{LABEL}` window with an \
         {STATE_EVENT} emit ({receipt:?}; emitted: {:?})",
        last(&events)
    );
    let window = app.get_webview_window(LABEL).expect("the overlay window");
    let first = invoke(&window, "overlay_ready", json!({}))
        .unwrap_or_else(|e| panic!("overlay_ready rejected for `{LABEL}`: {e}"));
    let first_emit = last(&events).expect("an emit");
    assert_eq!(kind(&first), Some("recording"), "overlay_ready: {first}");
    assert!(
        first.pointer("/state/elapsedMs").is_some_and(Value::is_u64),
        "Recording carries elapsedMs: {first}"
    );
    assert_eq!(
        first.get("lang"),
        Some(&lang),
        "overlay_ready lang: {first}"
    );
    assert_eq!(
        first.get("seq"),
        first_emit.get("seq"),
        "overlay_ready seq differs from the last emit: {first} vs {first_emit}"
    );
    let first_seq = first
        .get("seq")
        .and_then(Value::as_u64)
        .expect("seq is a u64");

    indicator.set_overlay(&OverlayState::Processing);
    let receipt = overlay::settled(app.handle(), HANG);
    assert!(
        receipt.is_some_and(|r| !r.failed),
        "Processing: the overlay receipt (`overlay::settled`) did not report a handled, \
         unfailed state within the hang bound {HANG:?}: {receipt:?} (last: {:?})",
        last(&events)
    );
    assert_eq!(
        last(&events).as_ref().and_then(kind),
        Some("processing"),
        "the receipt settled on Processing but it was not emitted to the live overlay \
         ({receipt:?})"
    );
    let second = invoke(&window, "overlay_ready", json!({}))
        .unwrap_or_else(|e| panic!("overlay_ready rejected for `{LABEL}`: {e}"));
    let second_emit = last(&events).expect("an emit");
    assert_eq!(
        second, second_emit,
        "overlay_ready differs from the last {STATE_EVENT} emit"
    );
    assert_eq!(
        second,
        json!({ "seq": second.get("seq").cloned().unwrap_or(Value::Null), "lang": lang,
                "state": { "kind": "processing" } }),
        "overlay_ready after Processing"
    );
    let second_seq = second
        .get("seq")
        .and_then(Value::as_u64)
        .expect("seq is a u64");
    assert!(
        second_seq > first_seq,
        "the newer state has no newer seq: {first_seq} then {second_seq}"
    );
}

/// The lines the app logged under `dir` so far (none when nothing was written).
fn logged(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("logs").join("voicen.log"))
        .map(|text| text.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

#[test]
fn an_overlay_that_cannot_be_built_is_reported_failed_by_the_receipt_and_the_log() {
    // T-173 failure branch (os-answer-deadline-race): an overlay that never builds
    // still fails the verdict, with a named cause and without a wall-clock budget. A
    // window labelled `overlay` already exists, so the overlay thread's build fails
    // (tauri: WindowLabelAlreadyExists) and writes one `overlay_failed` line. The
    // overlay's receipt (`overlay::settled`) must report that outcome as `failed` (not
    // as a hang and not as a success), and the log must hold the line. The assertion
    // messages name which of the facts is missing. Red while the receipt does not
    // exist (it answers `None`). Bite: a receipt that reports a failed build as not
    // failed (the `Ok(Err(_))` arm of `OverlayThread::execute` swallowed), one that
    // never settles after a failed build, or a build error that writes no line.
    let _serial = serial();
    let dir = TempDir::new();
    let service = load(dir.path());
    let log = voicen_lib::diag::start(dir.path().join("logs"), Box::new(|_| {}));
    let app = voicen_lib::build_app(
        mock_builder(),
        release_context(),
        Arc::clone(&service),
        idle_models(),
        log,
    )
    .expect("mock app builds");
    let _existing = overlay_window(&app);
    let indicator = ShellIndicator::new(app.handle());

    indicator.set_overlay(&OverlayState::Recording);
    let receipt = overlay::settled(app.handle(), HANG);

    let lines = logged(dir.path());
    let failed_line = lines.iter().any(|l| l.contains("overlay_failed"));
    let settled = receipt.unwrap_or_else(|| {
        panic!(
            "the overlay receipt (`overlay::settled`) did not settle within the hang bound \
         {HANG:?} although the build had to fail at once (a window labelled `{LABEL}` \
         already exists); `overlay_failed` logged: {failed_line}; the app's log: {lines:#?}"
        )
    });
    assert!(
        settled.failed,
        "the build of a second `{LABEL}` window failed but the receipt says it did not: \
         {settled:?}; `overlay_failed` logged: {failed_line}; the app's log: {lines:#?}"
    );
    assert!(
        failed_line,
        "the receipt says the build failed ({settled:?}) but the app logged no \
         `overlay_failed` line: {lines:#?}"
    );
}
