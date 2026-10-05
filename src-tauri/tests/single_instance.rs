//! T-052 invariant 1 and the second-instance callback (spec 001 FR-002, spec 004
//! FR-019), over the shell's one assembly body (`voicen_lib::assemble`) through
//! Tauri's mock runtime. Windows CI only (decision #5).
//!
//! - No process touches the log, the data dir, the Run value or a window before
//!   tauri's `build()` has run the plugins' setup: that is where the single-instance
//!   plugin decides, and a second instance never returns from it (plugin 2.5.2
//!   windows.rs:55-105). `assemble` builds first, then calls `parts()`.
//! - The real plugin is never registered here: its second-instance path calls
//!   `process::exit(0)` inside `build()`, which cargo would count as a pass (T-052
//!   analysis, "new hazard"). A probe plugin stands in: its setup runs where the
//!   real plugin decides (tauri 2.12.1 app.rs:2607, inside `Builder::build`) and
//!   records what is on disk at that moment. The real plugin is proven by the
//!   install smoke only (second launch: exit 0, no `started pid=<second>` line, a
//!   planted `.part` kept).
//! - `parts` here is the test's copy of `run()`'s startup side effects over a
//!   `TempDir` (`diag::start`, `LocalModels::open` with its `.part` cleanup,
//!   `load_settings`), in `run()`'s order; the runner's real `%LOCALAPPDATA%\Voicen`
//!   is never written. No keys are used.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::Value;
use tauri::plugin::TauriPlugin;
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::{App, Listener, Manager, Url};
use voicen_core::autostart::{Autostart, FakeAutostart};
use voicen_core::diag::Log;
use voicen_core::local_models::catalog::MODELS;
use voicen_core::local_models::service::LocalModels;
use voicen_core::local_models::store::ModelStore;
use voicen_core::secrets::{CredentialStore, FakeCredentialStore};
use voicen_core::settings::file::SETTINGS_FILE;
use voicen_core::settings::service::SettingsService;
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_lib::autostart::AUTOSTART_ARG;
use voicen_lib::settings_ipc::load_settings;
use voicen_lib::settings_window::{Receipt, LABEL};
use voicen_lib::Parts;

/// How long a test waits for the opener thread (Windows-sized, F-005).
const BUDGET: Duration = Duration::from_secs(10);

/// A stale download left by the primary's running download (the smoke's name).
const PART: &str = "ggml-base.bin.part";

/// The second process's argv[0] and cwd as the plugin passes them (fake paths).
const EXE: &str = r"C:\Users\fake-user\AppData\Local\Voicen\voicen.exe";
const CWD: &str = r"C:\Users\fake-user";

fn no_keys() -> Arc<dyn CredentialStore> {
    Arc::new(FakeCredentialStore::new())
}

fn no_autostart() -> Arc<dyn Autostart> {
    Arc::new(FakeAutostart::new())
}

fn logs_dir(dir: &Path) -> PathBuf {
    dir.join("logs")
}

fn models_dir(dir: &Path) -> PathBuf {
    dir.join("models")
}

/// Plants `models/ggml-base.bin.part` (1 KiB) and returns its path.
fn plant_part(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(models_dir(dir)).expect("create models dir");
    let part = models_dir(dir).join(PART);
    std::fs::write(&part, [0u8; 1024]).expect("plant .part");
    part
}

/// The startup side effects of `run()`, in its order, over `dir`: the one log
/// (Started first), the one `LocalModels` (its `open` deletes every `.part`), the
/// settings load (a first run writes `settings.json`; the Run value reconcile goes
/// to the fake). `called` is set when it runs.
fn startup_parts(dir: &Path, called: &Arc<AtomicBool>) -> impl FnOnce() -> Parts {
    let dir = dir.to_path_buf();
    let called = Arc::clone(called);
    move || {
        called.store(true, Ordering::SeqCst);
        let log = voicen_lib::diag::start(logs_dir(&dir), Box::new(|_| {}));
        let (local_models, cleanup) = LocalModels::open(
            models_dir(&dir),
            FakeDisk::with_available(u64::MAX),
            Timeouts::default(),
            MODELS,
        );
        cleanup.expect("the .part cleanup");
        let local_models = Arc::new(local_models);
        let (service, _outcome) = load_settings(
            dir.clone(),
            no_keys(),
            no_autostart(),
            local_models.store(),
            None,
            &log,
        );
        Parts::new(service, local_models, log)
    }
}

/// What the probe plugin's setup found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    logs_dir_exists: bool,
    settings_exists: bool,
    part_exists: bool,
}

/// A plugin whose setup records what exists under `dir` when tauri runs it, then
/// returns `Err` if `fail` (a plugin that does not let the process start).
fn probe(dir: &Path, seen: &Arc<Mutex<Option<Seen>>>, fail: bool) -> TauriPlugin<MockRuntime> {
    let dir = dir.to_path_buf();
    let seen = Arc::clone(seen);
    tauri::plugin::Builder::new("t052-instance-probe")
        .setup(move |_app, _api| {
            *seen.lock().unwrap_or_else(PoisonError::into_inner) = Some(Seen {
                logs_dir_exists: logs_dir(&dir).exists(),
                settings_exists: dir.join(SETTINGS_FILE).exists(),
                part_exists: models_dir(&dir).join(PART).exists(),
            });
            if fail {
                Err("the probe refuses the start (test)".into())
            } else {
                Ok(())
            }
        })
        .build()
}

fn seen(cell: &Arc<Mutex<Option<Seen>>>) -> Option<Seen> {
    cell.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// The lines of `logs/voicen.log` with a `started pid=` (the Started line).
fn started_lines(dir: &Path) -> Vec<String> {
    let path = logs_dir(dir).join("voicen.log");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    text.lines()
        .filter(|l| l.contains(" started pid="))
        .map(str::to_string)
        .collect()
}

// ---- invariant 1: the startup side effects run only after the plugins' setup ----

#[test]
fn startup_side_effects_run_only_after_the_plugins_setup() {
    // T-052 red-test table row 3 (T-008 review r1 #3, widened): when the plugins'
    // setup runs (where the single-instance plugin decides), the log folder does
    // not exist, no settings.json was written and the primary's `.part` is still
    // there; once `assemble` returns, the side effects have run: one Started line,
    // the `.part` deleted, settings.json written. Bite: `parts()` called before
    // `build()` (today's `run()` order: log, cleanup, settings, then build), or
    // never called.
    let dir = TempDir::new();
    let part = plant_part(dir.path());
    let seen_at_setup = Arc::new(Mutex::new(None));
    let called = Arc::new(AtomicBool::new(false));

    let app = voicen_lib::assemble(
        mock_builder().plugin(probe(dir.path(), &seen_at_setup, false)),
        mock_context(noop_assets()),
        startup_parts(dir.path(), &called),
    )
    .expect("the app assembles");

    assert_eq!(
        seen(&seen_at_setup),
        Some(Seen {
            logs_dir_exists: false,
            settings_exists: false,
            part_exists: true,
        }),
        "what the plugins' setup found on disk"
    );
    assert!(called.load(Ordering::SeqCst), "parts() was never called");
    assert_eq!(
        started_lines(dir.path()).len(),
        1,
        "Started lines: {:?}",
        started_lines(dir.path())
    );
    assert!(!part.exists(), "the .part cleanup did not run");
    assert!(
        dir.path().join(SETTINGS_FILE).is_file(),
        "the first-run settings.json was not written"
    );
    assert!(
        app.try_state::<Arc<SettingsService>>().is_some(),
        "the service of parts() is not managed"
    );
    assert!(
        app.try_state::<Arc<Log>>().is_some(),
        "the log of parts() is not managed"
    );
}

#[test]
fn a_failing_plugin_setup_leaves_no_side_effect() {
    // T-052 red-test table row 4: a plugin setup that does not let the process
    // start (as the single-instance plugin never returns in a second instance)
    // makes `assemble` fail with no side effect: parts() not called, no log folder,
    // the `.part` kept, no settings.json. Bite: side effects before `build()`, or
    // `parts()` called whatever `build()` returned.
    let dir = TempDir::new();
    let part = plant_part(dir.path());
    let seen_at_setup = Arc::new(Mutex::new(None));
    let called = Arc::new(AtomicBool::new(false));

    let result = voicen_lib::assemble(
        mock_builder().plugin(probe(dir.path(), &seen_at_setup, true)),
        mock_context(noop_assets()),
        startup_parts(dir.path(), &called),
    );

    assert!(
        result.is_err(),
        "assemble succeeded past a failing plugin setup"
    );
    assert!(
        seen(&seen_at_setup).is_some(),
        "premise: the probe's setup ran"
    );
    assert!(
        !called.load(Ordering::SeqCst),
        "parts() ran after build() failed"
    );
    assert!(!logs_dir(dir.path()).exists(), "the log folder was created");
    assert!(part.exists(), "the primary's .part was deleted");
    assert!(
        !dir.path().join(SETTINGS_FILE).exists(),
        "settings.json was written"
    );
}

// ---- the second-instance callback ---------------------------------------------

fn idle_store(data_dir: &Path) -> Arc<ModelStore> {
    Arc::new(ModelStore::new(data_dir.join("models"), MODELS))
}

/// The app as `run()` builds it (`build_app`, no single-instance plugin) over a
/// fresh data dir; nothing is open.
fn fresh_app(dir: &Path) -> App<MockRuntime> {
    let log = voicen_lib::diag::start(logs_dir(dir), Box::new(|_| {}));
    let (service, _) = load_settings(
        dir.to_path_buf(),
        no_keys(),
        no_autostart(),
        idle_store(dir),
        None,
        &log,
    );
    let (models, cleanup) = LocalModels::open(
        models_dir(dir),
        FakeDisk::with_available(u64::MAX),
        Timeouts::default(),
        MODELS,
    );
    cleanup.expect("cleanup");
    voicen_lib::build_app(
        mock_builder(),
        mock_context(noop_assets()),
        service,
        Arc::new(models),
        log,
    )
    .expect("mock app builds")
}

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

#[track_caller]
fn ran(receipt: Receipt) -> tauri::Result<()> {
    receipt
        .wait_timeout(BUDGET)
        .unwrap_or_else(|| panic!("the opener did not run the request within {BUDGET:?}"))
}

fn labels(app: &App<MockRuntime>) -> Vec<String> {
    let mut labels: Vec<String> = app.webview_windows().into_keys().collect();
    labels.sort();
    labels
}

fn settings_url(app: &App<MockRuntime>) -> Url {
    app.get_webview_window(LABEL)
        .unwrap_or_else(|| panic!("no `{LABEL}` window; windows: {:?}", labels(app)))
        .url()
        .expect("window url")
}

fn query(url: &Url) -> Vec<(String, String)> {
    url.query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

#[test]
fn a_second_launch_opens_one_settings_window_at_tab_engine() {
    // spec 001 FR-002 (T-052 red-test table row 5): with no window open, a second
    // launch (in the primary: the plugin's callback) opens the settings window on
    // Engine, through the opener. Bite: the callback ignored, or opening on another
    // tab or under another label.
    let dir = TempDir::new();
    let app = fresh_app(dir.path());
    assert!(labels(&app).is_empty(), "premise: no window");

    let receipt = voicen_lib::tray::on_second_instance(app.handle(), argv(&[EXE]), CWD.into())
        .expect("a plain second launch posts an open request");
    ran(receipt).expect("open");

    assert_eq!(labels(&app), vec![LABEL.to_string()], "windows");
    let url = settings_url(&app);
    assert_eq!(url.path(), "/settings", "url {url}");
    assert_eq!(
        query(&url),
        vec![("tab".to_string(), "engine".to_string())],
        "url {url}"
    );
}

#[test]
fn a_second_launch_while_open_fronts_the_one_window_on_its_tab() {
    // spec 001 FR-002 with OQ-11 Q2 default: a second launch while the window is
    // open brings that window forward on the tab it shows: still one window, its
    // URL unchanged, and no `settings://focus` (no tab switch). Bite: a second
    // window, the window navigated, or a focus event switching to Engine.
    let dir = TempDir::new();
    let app = fresh_app(dir.path());
    ran(
        voicen_lib::tray::on_second_instance(app.handle(), argv(&[EXE]), CWD.into())
            .expect("first second launch posts"),
    )
    .expect("open");
    let first_url = settings_url(&app);
    let focus = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = Arc::clone(&focus);
    app.listen_any(voicen_lib::settings_window::FOCUS_EVENT, move |event| {
        sink.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(serde_json::from_str(event.payload()).expect("focus payload is JSON"));
    });

    let receipt = voicen_lib::tray::on_second_instance(app.handle(), argv(&[EXE]), CWD.into())
        .expect("a plain second launch posts a request");
    ran(receipt).expect("front");

    assert_eq!(labels(&app), vec![LABEL.to_string()], "windows");
    assert_eq!(settings_url(&app), first_url, "url changed");
    assert_eq!(
        focus.lock().unwrap_or_else(PoisonError::into_inner).clone(),
        Vec::<Value>::new(),
        "a second launch switched the tab"
    );
}

#[test]
fn a_second_launch_by_autostart_opens_no_window() {
    // spec 004 FR-019 (T-052 stated default): a logon start that reaches the running
    // instance opens nothing and posts nothing. Bite: the callback opening a window
    // whatever argv says, or reading the primary's own args instead of the second
    // process's argv.
    let dir = TempDir::new();
    let app = fresh_app(dir.path());

    let posted =
        voicen_lib::tray::on_second_instance(app.handle(), argv(&[EXE, AUTOSTART_ARG]), CWD.into());

    assert!(
        posted.is_none(),
        "an --autostart second launch posted a request"
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(labels(&app).is_empty(), "windows: {:?}", labels(&app));
}
