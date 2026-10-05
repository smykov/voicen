//! T-037: the settings window lifecycle (invariant S1; contracts/ipc.md "Window",
//! `settings://focus`) and the startup executor, over the shell's one construction
//! path (`load_settings`) and app wiring (`build_app`) through Tauri's mock runtime.
//! Windows CI only (decision #5).
//!
//! The tests never call `.run()`: the mock runtime would loop until the last window
//! closes, forever with `TrayOnly` (T-037 Investigation). They call
//! `settings_window::on_ready` directly, as `run()` does on `RunEvent::Ready`; only the
//! install smoke proves that `run()` calls it. Close-then-reopen is not testable in
//! the mock (the manager drops a window only from the run loop) and is not written.
//!
//! The URL is asserted by path and query pairs, never by origin: the origin is
//! `http://tauri.localhost` with `mock_context` and the devUrl with
//! `generate_context!` in a debug build.
//!
//! Each test gets its own `TempDir` as the data dir; the CI runner's real
//! `%LOCALAPPDATA%\Voicen` is never written. No keys are used.
//!
//! T-052 (invariant 2): at runtime `open` runs only on the one opener thread;
//! `on_ready` and every other caller post with `settings_window::request` and get a
//! `Receipt`, which only tests wait on (a Windows-sized budget, F-005). tauri's own
//! emit already serializes Rust listeners (a second concurrent emit is queued and
//! delivered by the first emitter's thread, tauri 2.12.1 event/listener.rs:208-233),
//! so a focus count cannot tell an opener from callers running `open` themselves;
//! these tests decide on which thread `open` ran (a probe plugin's `on_window_ready`
//! runs inline on the building thread in the mock) and on whether `request` returns
//! while its open is held.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Barrier, Condvar, Mutex, PoisonError};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::plugin::TauriPlugin;
use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::webview::InvokeRequest;
use tauri::{
    App, Builder, Context, Listener, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use voicen_core::autostart::{Autostart, FakeAutostart};
use voicen_core::diag::Log;
use voicen_core::local_models::catalog::MODELS;
use voicen_core::local_models::service::LocalModels;
use voicen_core::local_models::store::ModelStore;
use voicen_core::secrets::{CredentialStore, FakeCredentialStore};
use voicen_core::settings::file::SETTINGS_FILE;
use voicen_core::settings::gate::SettingsTab;
use voicen_core::settings::service::SettingsService;
use voicen_core::settings::{FieldId, LoadOutcome};
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_lib::settings_ipc::load_settings;
use voicen_lib::settings_window::{self, OpenTarget, Receipt, LABEL};

const FOCUS: &str = "settings://focus";

/// How long a test waits for the opener thread (Windows-sized, F-005).
const BUDGET: Duration = Duration::from_secs(10);

/// A held probe gives up after this, so a failing test cannot hang the exe.
const HOLD_LIMIT: Duration = Duration::from_secs(30);

/// `request` must return well within this while its open is held.
const RETURNS_AT_ONCE: Duration = Duration::from_secs(2);

/// Waits for the opener to run `receipt` and returns the open's result.
#[track_caller]
fn ran(receipt: Receipt) -> tauri::Result<()> {
    receipt
        .wait_timeout(BUDGET)
        .unwrap_or_else(|| panic!("the opener did not run the request within {BUDGET:?}"))
}

fn no_keys() -> Arc<dyn CredentialStore> {
    Arc::new(FakeCredentialStore::new())
}

fn no_autostart() -> Arc<dyn Autostart> {
    Arc::new(FakeAutostart::new())
}

/// T-044: the store `load_settings` takes. No model is downloaded in these tests,
/// and `ModelStore::new` creates nothing, so the data dir keeps only its own files.
fn idle_store(data_dir: &Path) -> Arc<ModelStore> {
    Arc::new(ModelStore::new(data_dir.join("models"), MODELS))
}

/// T-044: the one `LocalModels` that `build_app` manages; these tests never
/// download. Its models dir lies under a throwaway dir that is removed at once, so
/// it is never created.
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

/// T-008: the log handed to `load_settings` / `build_app` by tests that do not read
/// it. Its folder lies under this test exe (a file), so it can never be created: the
/// log stays degraded and writes nothing, neither in the data dir nor in the
/// runner's `%LOCALAPPDATA%`.
fn discard_log() -> Arc<Log> {
    let exe = std::env::current_exe().expect("current_exe");
    voicen_lib::diag::start(exe.join("logs"), Box::new(|_| {}))
}

fn load(dir: &Path) -> (Arc<SettingsService>, LoadOutcome) {
    load_settings(
        dir.to_path_buf(),
        no_keys(),
        no_autostart(),
        idle_store(dir),
        None,
        &discard_log(),
    )
}

/// The app as `run()` builds it (`build_app`), on the mock runtime with `context`.
fn app_with(service: &Arc<SettingsService>, context: Context<MockRuntime>) -> App<MockRuntime> {
    voicen_lib::build_app(
        mock_builder(),
        context,
        service.clone(),
        idle_models(),
        discard_log(),
    )
    .expect("mock app builds")
}

fn mock_app(service: &Arc<SettingsService>) -> App<MockRuntime> {
    app_with(service, mock_context(noop_assets()))
}

/// A fresh data dir and the app over it; the outcome is FirstRun.
fn fresh_app() -> (TempDir, App<MockRuntime>) {
    let dir = TempDir::new();
    let (service, outcome) = load(dir.path());
    assert!(
        matches!(outcome, LoadOutcome::FirstRun(_)),
        "fresh dir: {outcome:?}"
    );
    let app = mock_app(&service);
    (dir, app)
}

/// The labels of every webview window the app has, sorted.
fn labels(app: &App<MockRuntime>) -> Vec<String> {
    let set: BTreeSet<String> = app.webview_windows().into_keys().collect();
    set.into_iter().collect()
}

#[track_caller]
fn settings_window(app: &App<MockRuntime>) -> WebviewWindow<MockRuntime> {
    app.get_webview_window(LABEL)
        .unwrap_or_else(|| panic!("no `{LABEL}` window; windows: {:?}", labels(app)))
}

fn url_of(window: &WebviewWindow<MockRuntime>) -> Url {
    window.url().expect("window url")
}

fn query(url: &Url) -> Vec<(String, String)> {
    url.query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Exactly one window, labelled `settings`, at path `/settings` with exactly the
/// query `expected`.
#[track_caller]
fn assert_one_settings_window(app: &App<MockRuntime>, expected: &[(&str, &str)]) {
    assert_eq!(labels(app), vec![LABEL.to_string()], "windows");
    let url = url_of(&settings_window(app));
    assert_eq!(url.path(), "/settings", "url {url}");
    assert_eq!(query(&url), pairs(expected), "url {url}");
}

/// Every `settings://focus` payload, in order, as JSON. Rust `listen_any` handlers
/// run inside the emit call, so no waiting is needed.
fn focus_events(app: &App<MockRuntime>) -> Arc<Mutex<Vec<Value>>> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    app.listen_any(FOCUS, move |event| {
        let payload = serde_json::from_str(event.payload()).expect("focus payload is JSON");
        sink.lock().expect("lock").push(payload);
    });
    events
}

fn taken(events: &Arc<Mutex<Vec<Value>>>) -> Vec<Value> {
    events.lock().expect("lock").clone()
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

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("list data dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// ---- startup executor --------------------------------------------------------

#[test]
fn first_run_opens_one_settings_window_at_tab_engine() {
    // Acceptance 1: OpenSettings(Engine) from a FirstRun outcome creates exactly one
    // `settings` window at `settings?tab=engine`. Bite: on_ready not opening (today:
    // the function is a no-op and the only window comes from tauri.conf.json), a
    // window under another label, another tab, a `field=` on a startup open, or two
    // windows.
    let dir = TempDir::new();
    let (service, outcome) = load(dir.path());
    assert!(matches!(outcome, LoadOutcome::FirstRun(_)), "{outcome:?}");
    let app = mock_app(&service);
    assert!(
        labels(&app).is_empty(),
        "a window before on_ready: {:?}",
        labels(&app)
    );

    // T-052: on_ready posts to the opener and returns the request's receipt.
    let receipt = settings_window::on_ready(app.handle(), &outcome, false)
        .expect("FirstRun posts an open request");
    ran(receipt).expect("open");

    assert_one_settings_window(&app, &[("tab", "engine")]);
}

#[test]
fn first_run_launched_by_autostart_still_opens_the_window() {
    // Spec US5-3: the autostart flag does not change the startup action. Bite:
    // on_ready deciding on launched_by_autostart itself instead of startup_action.
    let dir = TempDir::new();
    let (service, outcome) = load(dir.path());
    let app = mock_app(&service);

    let receipt = settings_window::on_ready(app.handle(), &outcome, true)
        .expect("FirstRun posts an open request");
    ran(receipt).expect("open");

    assert_one_settings_window(&app, &[("tab", "engine")]);
}

#[test]
fn reset_opens_one_settings_window_at_tab_engine() {
    // FR-21 / spec T025: a Reset (unreadable settings.json moved aside) opens the
    // window like FirstRun. Bite: on_ready matching only FirstRun.
    let dir = TempDir::new();
    std::fs::write(dir.path().join(SETTINGS_FILE), b"{ not json").expect("corrupt file");
    let (service, outcome) = load(dir.path());
    assert!(matches!(outcome, LoadOutcome::Reset { .. }), "{outcome:?}");
    let app = mock_app(&service);

    let receipt = settings_window::on_ready(app.handle(), &outcome, false)
        .expect("Reset posts an open request");
    ran(receipt).expect("open");

    assert_one_settings_window(&app, &[("tab", "engine")]);
}

#[test]
fn loaded_opens_no_window() {
    // Acceptance 1: TrayOnly (Loaded) creates none, launched by autostart or not.
    // Paired with the FirstRun case: this one catches an executor that always
    // opens. Bite: on_ready opening without consulting startup_action.
    let dir = TempDir::new();
    let (first, outcome) = load(dir.path());
    assert!(matches!(outcome, LoadOutcome::FirstRun(_)), "{outcome:?}");
    drop(first);
    let (service, outcome) = load(dir.path());
    assert!(matches!(outcome, LoadOutcome::Loaded(_)), "{outcome:?}");

    for autostart in [false, true] {
        let app = mock_app(&service);
        // T-052: TrayOnly posts nothing, so there is no receipt to wait on.
        assert!(
            settings_window::on_ready(app.handle(), &outcome, autostart).is_none(),
            "autostart {autostart}: TrayOnly posted an open request"
        );
        assert!(
            labels(&app).is_empty(),
            "autostart {autostart}: windows {:?}",
            labels(&app)
        );
    }
}

#[test]
fn unavailable_opens_the_window_and_no_save_succeeds() {
    // Acceptance 4 (failure branch): a directory at settings.json makes the read
    // fail -> Unavailable -> the window opens at tab=engine; from that window
    // settings_get reports `unavailable: true` and settings_save is Refused with
    // form_error settings_unavailable; settings.json is still the directory. Bite:
    // on_ready skipping Unavailable, or the window wired to another service.
    let dir = TempDir::new();
    std::fs::create_dir(dir.path().join(SETTINGS_FILE)).expect("dir at settings.json");
    let (service, outcome) = load(dir.path());
    assert!(
        matches!(outcome, LoadOutcome::Unavailable(_)),
        "{outcome:?}"
    );
    let app = mock_app(&service);

    let receipt = settings_window::on_ready(app.handle(), &outcome, false)
        .expect("Unavailable posts an open request");
    ran(receipt).expect("open");

    assert_one_settings_window(&app, &[("tab", "engine")]);
    let window = settings_window(&app);

    let view = invoke(&window, "settings_get", json!({}))
        .unwrap_or_else(|e| panic!("settings_get rejected: {e}"));
    assert_eq!(view["unavailable"], json!(true), "{view}");

    let request = json!({ "request": {
        "settings": view["settings"].clone(),
        "keys": {
            "transcription_api": "Untouched",
            "local_server": "Untouched",
            "post_processing": "Untouched",
        },
    } });
    let outcome = invoke(&window, "settings_save", request)
        .unwrap_or_else(|e| panic!("settings_save rejected: {e}"));
    assert!(
        outcome.get("Saved").is_none(),
        "a save succeeded: {outcome}"
    );
    assert_eq!(
        outcome["Refused"]["form_error"]["kind"],
        json!("settings_unavailable"),
        "{outcome}"
    );

    assert!(
        dir.path().join(SETTINGS_FILE).is_dir(),
        "settings.json is no longer the directory"
    );
    assert_eq!(entries(dir.path()), vec![SETTINGS_FILE.to_string()]);
}

// ---- open(): single instance, URL, focus event --------------------------------

#[test]
fn second_open_keeps_one_window_and_emits_one_focus_event() {
    // Acceptance 1: a second `open` leaves one window and emits `settings://focus`
    // (spec FR-002, contracts/ipc.md). Bite: a second window built (or the build
    // error swallowed), the existing window navigated to a new URL, no event, the
    // event on the first open too, two events for one open, a payload other than
    // `{tab, field?}` with the contract's tokens, or `field: null` instead of an
    // omitted field.
    let (_dir, app) = fresh_app();
    let events = focus_events(&app);

    settings_window::open(app.handle(), SettingsTab::Engine, None).expect("first open");
    assert_one_settings_window(&app, &[("tab", "engine")]);
    let first_url = url_of(&settings_window(&app));
    assert_eq!(
        taken(&events),
        Vec::<Value>::new(),
        "focus on the first open"
    );

    settings_window::open(
        app.handle(),
        SettingsTab::Recording,
        Some(FieldId::RecordingHotkey),
    )
    .expect("second open");
    assert_eq!(
        labels(&app),
        vec![LABEL.to_string()],
        "windows after the second open"
    );
    assert_eq!(url_of(&settings_window(&app)), first_url, "url changed");
    assert_eq!(
        taken(&events),
        vec![json!({ "tab": "recording", "field": "recording.hotkey" })]
    );

    settings_window::open(app.handle(), SettingsTab::General, None).expect("third open");
    assert_eq!(
        labels(&app),
        vec![LABEL.to_string()],
        "windows after the third open"
    );
    assert_eq!(url_of(&settings_window(&app)), first_url, "url changed");
    assert_eq!(
        taken(&events),
        vec![
            json!({ "tab": "recording", "field": "recording.hotkey" }),
            json!({ "tab": "general" }),
        ]
    );
}

#[test]
fn open_with_a_field_puts_tab_and_field_in_the_url() {
    // contracts/ipc.md: URL `settings?tab=<tab>[&field=<FieldId>]`. Bite: the field
    // dropped from the URL, a token other than FieldId::as_str, or another order.
    let (_dir, app) = fresh_app();

    settings_window::open(
        app.handle(),
        SettingsTab::Recording,
        Some(FieldId::RecordingHotkey),
    )
    .expect("open");

    assert_one_settings_window(&app, &[("tab", "recording"), ("field", "recording.hotkey")]);
}

#[test]
fn open_puts_each_tab_token_in_the_url() {
    // The URL's tab is SettingsTab::as_str for every tab (contracts/ipc.md tokens).
    // Bite: a shell-side spelling of a tab (`PostProcessing`, `post-processing`), or
    // one tab hard-coded.
    for (tab, token) in [
        (SettingsTab::Engine, "engine"),
        (SettingsTab::Recording, "recording"),
        (SettingsTab::Output, "output"),
        (SettingsTab::PostProcessing, "post_processing"),
        (SettingsTab::History, "history"),
        (SettingsTab::General, "general"),
    ] {
        let (_dir, app) = fresh_app();
        settings_window::open(app.handle(), tab, None).expect("open");
        assert_one_settings_window(&app, &[("tab", token)]);
    }
}

// ---- request(): the one opener thread (T-052 invariant 2) ---------------------

/// A one-shot hold: the first `enter` blocks until `release` (or `HOLD_LIMIT`, so a
/// failing test cannot hang); later entries pass. Counts the entries.
#[derive(Default)]
struct Hold {
    state: Mutex<HoldState>,
    changed: Condvar,
}

#[derive(Default)]
struct HoldState {
    entries: usize,
    released: bool,
}

impl Hold {
    fn enter(&self) {
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        st.entries += 1;
        self.changed.notify_all();
        if st.entries == 1 {
            let _held = self
                .changed
                .wait_timeout_while(st, HOLD_LIMIT, |s| !s.released)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn entries(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entries
    }

    /// Waits until at least `n` entries happened; false after `BUDGET`.
    fn wait_entries(&self, n: usize) -> bool {
        let st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let (st, _) = self
            .changed
            .wait_timeout_while(st, BUDGET, |s| s.entries < n)
            .unwrap_or_else(PoisonError::into_inner);
        st.entries >= n
    }

    fn release(&self) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .released = true;
        self.changed.notify_all();
    }
}

/// Releases the hold when the test ends, also by a failed assertion.
struct ReleaseOnDrop(Arc<Hold>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// `(thread, label)` of every window the app created, in order.
type Creations = Arc<Mutex<Vec<(ThreadId, String)>>>;

/// A plugin whose `on_window_ready` records the thread and label of each window
/// creation and then enters `hold` (if any). tauri runs the hook right after the
/// window is inserted into the manager, through `run_on_main_thread`, which the
/// mock runs inline on the building thread while the loop is not running
/// (manager/window.rs:116-131, mock_runtime.rs:84-96).
fn creation_probe(creations: &Creations, hold: Option<Arc<Hold>>) -> TauriPlugin<MockRuntime> {
    let creations = Arc::clone(creations);
    tauri::plugin::Builder::new("t052-window-probe")
        .on_window_ready(move |window| {
            creations
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((thread::current().id(), window.label().to_string()));
            if let Some(hold) = &hold {
                hold.enter();
            }
        })
        .build()
}

/// The app as `run()` builds it, over `builder`, logging to `logs`.
fn app_on(
    builder: Builder<MockRuntime>,
    service: &Arc<SettingsService>,
    logs: &Path,
) -> App<MockRuntime> {
    voicen_lib::build_app(
        builder,
        mock_context(noop_assets()),
        service.clone(),
        idle_models(),
        voicen_lib::diag::start(logs.to_path_buf(), Box::new(|_| {})),
    )
    .expect("mock app builds")
}

/// `(thread, payload)` of every `settings://focus`, in order.
fn focus_threads(app: &App<MockRuntime>) -> Arc<Mutex<Vec<(ThreadId, Value)>>> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    app.listen_any(FOCUS, move |event| {
        let payload = serde_json::from_str(event.payload()).expect("focus payload is JSON");
        sink.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((thread::current().id(), payload));
    });
    events
}

/// The lines of `logs/voicen.log` that name `settings_window_failed`.
fn failed_open_lines(logs: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(logs.join("voicen.log"))
        .unwrap_or_else(|e| panic!("read {}: {e}", logs.join("voicen.log").display()));
    text.lines()
        .filter(|l| l.contains("settings_window_failed"))
        .map(str::to_string)
        .collect()
}

#[test]
fn open_runs_on_one_thread_that_is_no_callers_thread() {
    // T-052 invariant 2: every request is run by the one opener thread, so no two
    // runs of `open` overlap (the label check and the insert bracket the window
    // creation, manager/window.rs:70-72 and :116-117: two concurrent opens can both
    // pass the check). One request from the test thread creates the window; three
    // threads behind a barrier then post one each. The creation and the three focus
    // emits all ran on one thread, which is none of the callers'. Bite: `request`
    // calling `open` on the caller's thread (the creation on the test thread, the
    // focus emits on the posters), or a thread per request (several threads).
    let dir = TempDir::new();
    let (service, _) = load(dir.path());
    let creations: Creations = Arc::default();
    let app = app_on(
        mock_builder().plugin(creation_probe(&creations, None)),
        &service,
        &dir.path().join("logs"),
    );
    let focus = focus_threads(&app);
    let test_thread = thread::current().id();

    ran(settings_window::request(
        app.handle(),
        OpenTarget::Tab(SettingsTab::Engine, None),
    ))
    .expect("first open");

    let barrier = Arc::new(Barrier::new(3));
    let posters: Vec<_> = [
        SettingsTab::Recording,
        SettingsTab::Output,
        SettingsTab::General,
    ]
    .into_iter()
    .map(|tab| {
        let handle = app.handle().clone();
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            barrier.wait();
            let receipt = settings_window::request(&handle, OpenTarget::Tab(tab, None));
            (thread::current().id(), receipt)
        })
    })
    .collect();
    let mut poster_threads = Vec::new();
    for poster in posters {
        let (id, receipt) = poster.join().expect("a poster thread panicked");
        poster_threads.push(id);
        ran(receipt).expect("a later request");
    }

    assert_one_settings_window(&app, &[("tab", "engine")]);
    let creations = creations
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let focus = focus.lock().unwrap_or_else(PoisonError::into_inner).clone();
    assert_eq!(
        creations
            .iter()
            .map(|(_, l)| l.as_str())
            .collect::<Vec<_>>(),
        vec![LABEL],
        "window creations"
    );
    assert_eq!(focus.len(), 3, "focus events: {focus:?}");
    let threads: BTreeSet<String> = creations
        .iter()
        .map(|(t, _)| format!("{t:?}"))
        .chain(focus.iter().map(|(t, _)| format!("{t:?}")))
        .collect();
    assert_eq!(
        threads.len(),
        1,
        "open ran on more than one thread: {threads:?}"
    );
    let opener = creations[0].0;
    assert_ne!(opener, test_thread, "open ran on the calling (test) thread");
    assert!(
        !poster_threads.contains(&opener),
        "open ran on a posting thread"
    );
}

#[test]
fn request_returns_while_its_open_is_held() {
    // T-052 invariant 2: callers post and never wait. The opener is held inside the
    // window creation (the probe's hook); the caller's `request` has already
    // returned its receipt, and a second `request` from another thread returns at
    // once while the first is still held. After the release both run, in order:
    // one window, one focus event for the second. Bite: `request` running `open`
    // itself (its caller blocked inside the creation, no receipt), or waiting for
    // the opener to finish the previous request.
    let dir = TempDir::new();
    let (service, _) = load(dir.path());
    let creations: Creations = Arc::default();
    let hold = Arc::new(Hold::default());
    let _release = ReleaseOnDrop(Arc::clone(&hold));
    let app = app_on(
        mock_builder().plugin(creation_probe(&creations, Some(Arc::clone(&hold)))),
        &service,
        &dir.path().join("logs"),
    );
    let focus = focus_events(&app);

    let (tx, rx) = mpsc::channel();
    let handle = app.handle().clone();
    thread::spawn(move || {
        let receipt = settings_window::request(&handle, OpenTarget::Tab(SettingsTab::Engine, None));
        let _ = tx.send(receipt);
    });
    assert!(
        hold.wait_entries(1),
        "the window creation did not start within {BUDGET:?}"
    );
    let first = rx
        .recv_timeout(RETURNS_AT_ONCE)
        .expect("request did not return while its own open was held: it ran open itself");

    let started = Instant::now();
    let second =
        settings_window::request(app.handle(), OpenTarget::Tab(SettingsTab::General, None));
    let took = started.elapsed();
    assert!(
        took < RETURNS_AT_ONCE,
        "request waited {took:?} for the busy opener"
    );
    assert_eq!(
        hold.entries(),
        1,
        "a second creation started during the hold"
    );

    hold.release();
    ran(first).expect("the held open");
    ran(second).expect("the open posted during the hold");
    assert_one_settings_window(&app, &[("tab", "engine")]);
    assert_eq!(taken(&focus), vec![json!({ "tab": "general" })]);
}

#[test]
fn eight_concurrent_requests_leave_one_window_and_no_failure() {
    // T-052 Acceptance (failure branch): concurrent requests from non-main threads
    // leave exactly one settings window: one creation, a focus event for each other
    // request, every receipt Ok (no "already exists"), and no
    // `settings_window_failed` line in the log. Bite: requests racing on the build
    // path (two creations, a WebviewLabelAlreadyExists error, a failed-open line).
    let dir = TempDir::new();
    let (service, _) = load(dir.path());
    let logs = dir.path().join("logs");
    let creations: Creations = Arc::default();
    let app = app_on(
        mock_builder().plugin(creation_probe(&creations, None)),
        &service,
        &logs,
    );
    let focus = focus_events(&app);

    let barrier = Arc::new(Barrier::new(8));
    let posters: Vec<_> = (0..8)
        .map(|_| {
            let handle = app.handle().clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                settings_window::request(&handle, OpenTarget::Tab(SettingsTab::General, None))
            })
        })
        .collect();
    let receipts: Vec<Receipt> = posters
        .into_iter()
        .map(|p| p.join().expect("a poster thread panicked"))
        .collect();
    for (i, receipt) in receipts.into_iter().enumerate() {
        if let Err(err) = ran(receipt) {
            panic!("request {i} failed: {err}");
        }
    }

    assert_one_settings_window(&app, &[("tab", "general")]);
    assert_eq!(
        creations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len(),
        1,
        "window creations"
    );
    assert_eq!(taken(&focus), vec![json!({ "tab": "general" }); 7]);
    assert_eq!(failed_open_lines(&logs), Vec::<String>::new());
}

#[test]
fn requests_run_in_the_order_they_were_posted() {
    // T-052 design 2: the opener is a FIFO. Four requests posted back to back from
    // one thread, only the last one waited on: by then the earlier ones have run, in
    // posting order (the first created the window, the next two emitted their focus
    // events in order), and `Front` emitted nothing and left the URL as it was
    // (OQ-11 Q2 default). Bite: requests run out of order (a thread per request),
    // the last receipt resolving before the earlier requests ran, or `Front`
    // switching the tab.
    let dir = TempDir::new();
    let (service, _) = load(dir.path());
    let app = app_on(mock_builder(), &service, &dir.path().join("logs"));
    let focus = focus_events(&app);

    let first = settings_window::request(app.handle(), OpenTarget::Tab(SettingsTab::Engine, None));
    let second = settings_window::request(
        app.handle(),
        OpenTarget::Tab(SettingsTab::Recording, Some(FieldId::RecordingHotkey)),
    );
    let third = settings_window::request(app.handle(), OpenTarget::Tab(SettingsTab::General, None));
    let last = settings_window::request(app.handle(), OpenTarget::Front);
    ran(last).expect("Front");

    assert_eq!(
        taken(&focus),
        vec![
            json!({ "tab": "recording", "field": "recording.hotkey" }),
            json!({ "tab": "general" }),
        ]
    );
    assert_one_settings_window(&app, &[("tab", "engine")]);
    for (name, receipt) in [("first", first), ("second", second), ("third", third)] {
        match receipt.wait_timeout(Duration::ZERO) {
            Some(result) => result.unwrap_or_else(|e| panic!("{name}: {e}")),
            None => panic!("{name} had not run when the later request's receipt resolved"),
        }
    }
}

// ---- config and capability (tauri's own parse and ACL) --------------------------

/// The release context (tauri.conf.json and the capabilities as tauri-build resolved
/// them into OUT_DIR), for the mock runtime.
fn release_context() -> Context<MockRuntime> {
    tauri::generate_context!(test = true)
}

#[test]
fn config_declares_no_window() {
    // Acceptance 2: no window is created from tauri.conf.json (tauri builds config
    // windows in setup() on every Ready, whatever the outcome). Decided on tauri's
    // own parse of the config, not a text scan (F-003). Bite: a window left in
    // `app.windows` (today: one, unlabelled = `main`).
    let context = release_context();
    let windows = &context.config().app.windows;
    assert!(
        windows.is_empty(),
        "tauri.conf.json declares windows: {:?}",
        windows.iter().map(|w| w.label.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn settings_window_may_listen_unlisten_and_destroy_itself() {
    // Acceptance 2, decision #45: the window `open` made may listen to (and stop
    // listening to) `settings://changed` and close itself (`onCloseRequested` ->
    // destroy). Through tauri's real ACL path. Bite: the capability scoped to
    // another label (today: `main`), or a permission missing (destroy is in neither
    // core:window:default nor core:event:default).
    let dir = TempDir::new();
    let (service, _) = load(dir.path());
    let app = app_with(&service, release_context());
    settings_window::open(app.handle(), SettingsTab::Engine, None).expect("open");
    let window = settings_window(&app);

    let event_id = invoke(
        &window,
        "plugin:event|listen",
        json!({ "event": "settings://changed", "target": { "kind": "Any" }, "handler": 7 }),
    )
    .unwrap_or_else(|e| panic!("listen refused for `{LABEL}`: {e}"));
    invoke(
        &window,
        "plugin:event|listen",
        json!({ "event": "tauri://close-requested", "target": { "kind": "Any" }, "handler": 8 }),
    )
    .unwrap_or_else(|e| panic!("listen to close-requested refused for `{LABEL}`: {e}"));
    invoke(
        &window,
        "plugin:event|unlisten",
        json!({ "event": "settings://changed", "eventId": event_id }),
    )
    .unwrap_or_else(|e| panic!("unlisten refused for `{LABEL}`: {e}"));
    invoke(&window, "plugin:window|destroy", json!({ "label": LABEL }))
        .unwrap_or_else(|e| panic!("destroy refused for `{LABEL}`: {e}"));
}

#[test]
fn another_window_label_gets_no_permission() {
    // S1 / decision #45: the permissions are granted to the `settings` label and
    // none to any other. A window with another label at the same URL is refused.
    // Bite: a wildcard or extra label in capabilities/default.json windows.
    let dir = TempDir::new();
    let (service, _) = load(dir.path());
    let app = app_with(&service, release_context());
    let other = WebviewWindowBuilder::new(&app, "other", WebviewUrl::App("settings".into()))
        .build()
        .expect("mock webview builds");

    for (cmd, args) in [
        (
            "plugin:event|listen",
            json!({ "event": "settings://changed", "target": { "kind": "Any" }, "handler": 7 }),
        ),
        ("plugin:window|destroy", json!({ "label": "other" })),
    ] {
        match invoke(&other, cmd, args) {
            Err(rejection) => assert!(
                is_acl_refusal(&rejection),
                "{cmd} rejected, but not by the ACL: {rejection}"
            ),
            Ok(value) => panic!("{cmd} allowed for label `other`: {value}"),
        }
    }
}

#[test]
fn settings_window_gets_nothing_beyond_the_least_privilege_set() {
    // Decision #45: least privilege, exactly core:event:allow-listen,
    // core:event:allow-unlisten and core:window:allow-destroy, not core:default.
    // Bite: `core:default` (or core:window:default / core:app:default) kept in the
    // capability.
    let dir = TempDir::new();
    let (service, _) = load(dir.path());
    let app = app_with(&service, release_context());
    settings_window::open(app.handle(), SettingsTab::Engine, None).expect("open");
    let window = settings_window(&app);

    for (cmd, args) in [
        ("plugin:window|title", json!({ "label": LABEL })),
        ("plugin:app|version", json!({})),
        (
            "plugin:event|emit",
            json!({ "event": "settings://changed", "payload": null }),
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
