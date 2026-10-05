//! T-052 invariant 4 and the real-runtime half of invariant 3, on tauri's real Wry
//! runtime (WebView2) driven from a libtest thread. Windows CI only (decision #5).
//!
//! - The loop runs with `Builder::any_thread` and `App::run_return`, never `run`
//!   (`run` ends in `process::exit`, tao 0.37.1 event_loop.rs:221-227, which would
//!   end this exe mid-run). The single-instance plugin is never registered
//!   (`build_app` does not).
//! - Each test builds its own app (`build_app`, so the tray and the opener are the
//!   ones `run()` gets) and calls `voicen_lib::on_run_event` from the loop callback,
//!   as `run()` does. A driver thread, started on `Ready`, acts on the app and
//!   reports; the test asserts after `run_return` returned.
//! - The loop is only ever ended from inside a main-thread task (`end_loop`):
//!   `AppHandle::exit` on a loop that has already ended falls back to
//!   `process::exit` (tauri 2.12.1 app.rs:581-587), which cargo would count as a
//!   pass. A posted task on an ended loop simply never runs.
//! - A watchdog ends the exe non-zero, with the test's name, if a loop never
//!   returns. The tests are serialised (one tray, one WebView2 profile at a time).
//! - The app identifier is a test one, so WebView2's profile is not the release
//!   app's. Each test has its own `TempDir`; no keys are used.
#![cfg(windows)]

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use tauri::menu::{MenuEvent, MenuId};
use tauri::test::{mock_context, noop_assets};
use tauri::{App, AppHandle, Context, Manager, RunEvent, Wry};
use voicen_core::autostart::{Autostart, FakeAutostart};
use voicen_core::diag::Log;
use voicen_core::local_models::catalog::MODELS;
use voicen_core::local_models::service::LocalModels;
use voicen_core::local_models::store::ModelStore;
use voicen_core::recording::TrayState;
use voicen_core::secrets::{CredentialStore, FakeCredentialStore};
use voicen_core::settings::LoadOutcome;
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_core::tray::TrayAction;
use voicen_lib::settings_ipc::load_settings;
use voicen_lib::settings_window::LABEL;
use voicen_lib::tray::{self, TRAY_ID};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindow,
};

/// A test identifier: WebView2's profile goes under it, not under the release one.
const IDENTIFIER: &str = "dev.voicen.test.lifecycle";

/// How long the driver waits for a window, a removal or an apply (Windows-sized:
/// a cold WebView2 creation on the runner, F-005).
const BUDGET: Duration = Duration::from_secs(20);

/// How long a main-thread round trip may take on a running loop.
const ROUND_TRIP: Duration = Duration::from_secs(5);

/// How long a blocked main-thread task waits for its release.
const HOLD: Duration = Duration::from_secs(10);

/// The exe ends with code 3 if one test's loop has not returned by then.
const WATCHDOG: Duration = Duration::from_secs(180);

/// The code of the fallback exit in `the_tray_exit_item_ends_the_loop_with_code_0`.
const FALLBACK_CODE: i32 = 70;

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Ends the exe with code 3 unless dropped within `WATCHDOG` (dropping also covers
/// a failed assertion).
struct Watchdog(Option<mpsc::Sender<()>>);

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.0.take();
    }
}

fn watchdog(test: &'static str) -> Watchdog {
    let (tx, rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        if let Err(RecvTimeoutError::Timeout) = rx.recv_timeout(WATCHDOG) {
            eprintln!(
                "lifecycle watchdog: `{test}`: the event loop did not return within \
                 {WATCHDOG:?}; ending the test exe"
            );
            std::process::exit(3);
        }
    });
    Watchdog(Some(tx))
}

fn no_keys() -> Arc<dyn CredentialStore> {
    Arc::new(FakeCredentialStore::new())
}

fn no_autostart() -> Arc<dyn Autostart> {
    Arc::new(FakeAutostart::new())
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

/// A degraded log under this test exe (a file), so it never writes anything.
fn discard_log() -> Arc<Log> {
    let exe = std::env::current_exe().expect("current_exe");
    voicen_lib::diag::start(exe.join("logs"), Box::new(|_| {}))
}

/// Which start the app makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    /// No settings.json: `Ready` opens the settings window.
    FirstRun,
    /// A settings.json from an earlier start: no window.
    Loaded,
}

fn load(dir: &Path, log: &Log) -> LoadOutcome {
    let store = Arc::new(ModelStore::new(dir.join("models"), MODELS));
    load_settings(
        dir.to_path_buf(),
        no_keys(),
        no_autostart(),
        store,
        None,
        log,
    )
    .1
}

/// The app as `run()` builds it (`build_app`), on the real Wry runtime, allowed to
/// run on this (libtest) thread, with its load outcome.
fn wry_app(dir: &Path, start: Start) -> (App<Wry>, LoadOutcome) {
    let log = discard_log();
    if start == Start::Loaded {
        let first = load(dir, &log);
        assert!(
            matches!(first, LoadOutcome::FirstRun(_)),
            "premise: {first:?}"
        );
    }
    let store = Arc::new(ModelStore::new(dir.join("models"), MODELS));
    let (service, outcome) = load_settings(
        dir.to_path_buf(),
        no_keys(),
        no_autostart(),
        store,
        None,
        &log,
    );
    match start {
        Start::FirstRun => assert!(matches!(outcome, LoadOutcome::FirstRun(_)), "{outcome:?}"),
        Start::Loaded => assert!(matches!(outcome, LoadOutcome::Loaded(_)), "{outcome:?}"),
    }
    let mut context: Context<Wry> = mock_context(noop_assets());
    context.config_mut().identifier = IDENTIFIER.to_string();
    let app = voicen_lib::build_app(
        tauri::Builder::default().any_thread(),
        context,
        service,
        idle_models(),
        log,
    )
    .expect("the Wry app builds");
    (app, outcome)
}

/// Runs the loop with `on_run_event` (as `run()` does) and starts `driver` on its
/// own thread once `Ready` was handled. Returns `run_return`'s code and the
/// driver's report, if it sent one within `BUDGET` after the loop returned.
fn run_with_driver<T: Send + 'static>(
    app: App<Wry>,
    outcome: LoadOutcome,
    driver: impl FnOnce(AppHandle<Wry>) -> T + Send + 'static,
) -> (i32, Option<T>) {
    let (tx, rx) = mpsc::channel();
    let mut driver = Some(driver);
    let code = app.run_return(move |handle, event| {
        let ready = matches!(event, RunEvent::Ready);
        voicen_lib::on_run_event(handle, event, &outcome, false);
        if ready {
            if let Some(driver) = driver.take() {
                let handle = handle.clone();
                let tx = tx.clone();
                thread::spawn(move || {
                    let _ = tx.send(driver(handle));
                });
            }
        }
    });
    (code, rx.recv_timeout(BUDGET).ok())
}

/// Ends the loop with `code` from inside a main-thread task; on a loop that has
/// already ended the task is never run (see the module docs).
fn end_loop(handle: &AppHandle<Wry>, code: i32) {
    let inner = handle.clone();
    let _ = handle.run_on_main_thread(move || inner.exit(code));
}

/// True when a task posted to the main thread ran within `ROUND_TRIP`: the loop
/// is still running.
fn main_round_trip(handle: &AppHandle<Wry>) -> bool {
    let (tx, rx) = mpsc::channel();
    if handle
        .run_on_main_thread(move || {
            let _ = tx.send(());
        })
        .is_err()
    {
        return false;
    }
    rx.recv_timeout(ROUND_TRIP).is_ok()
}

/// Polls `cond` every 10 ms until it holds or `budget` has passed.
fn poll(budget: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + budget;
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[derive(Debug, Default)]
struct CloseReport {
    opened: bool,
    destroyed: bool,
    gone: bool,
    alive_at_once: bool,
    alive_later: bool,
}

#[test]
fn destroying_the_last_window_keeps_the_app_running() {
    // T-052 red-test table row 12 (Acceptance 1, decision #38 Q3): on a first run
    // `Ready` opens the settings window through the opener; destroying it (the last
    // window: tauri emits ExitRequested { code: None }, runtime-wry lib.rs:4255-4270)
    // leaves the loop running while the tray exists: a main-thread round trip still
    // answers, at once and a second later. Bite: no `prevent_exit` (the #38 interim:
    // the process ends with its window), or `Ready` not opening the window.
    let _serial = serial();
    let _watchdog = watchdog("destroying_the_last_window_keeps_the_app_running");
    let dir = TempDir::new();
    let (app, outcome) = wry_app(dir.path(), Start::FirstRun);

    let (code, report) = run_with_driver(app, outcome, |handle| {
        let mut report = CloseReport {
            opened: poll(BUDGET, || handle.get_webview_window(LABEL).is_some()),
            ..CloseReport::default()
        };
        if report.opened {
            report.destroyed = handle
                .get_webview_window(LABEL)
                .is_some_and(|w| w.destroy().is_ok());
            report.gone = poll(BUDGET, || handle.get_webview_window(LABEL).is_none());
            report.alive_at_once = main_round_trip(&handle);
            thread::sleep(Duration::from_secs(1));
            report.alive_later = main_round_trip(&handle);
        }
        end_loop(&handle, 0);
        report
    });

    let report = report.expect("the driver sent no report");
    assert!(
        report.opened,
        "Ready did not open the first-run settings window within {BUDGET:?}"
    );
    assert!(
        report.destroyed && report.gone,
        "premise: the window was destroyed: {report:?}"
    );
    assert!(
        report.alive_at_once && report.alive_later,
        "the app ended when its last window was destroyed \
         (ExitRequested {{ code: None }} not prevented): {report:?}"
    );
    assert_eq!(code, 0, "run_return code after end_loop(0)");
}

#[derive(Debug, Default)]
struct SetTrayReport {
    part: bool,
    main_blocked: bool,
    set_tray_took: Duration,
    released_by_driver: bool,
    applied: bool,
}

#[test]
fn set_tray_returns_while_the_main_thread_is_blocked() {
    // T-052 red-test table row 13 (invariant 3; dictation-session.md: the session
    // calls `set_tray` under its lock, so it must not wait for another thread).
    // The main thread is held in a task until the driver releases it; the driver's
    // `set_tray(Recording)` returns before that release, and once released the main
    // thread applies it. Bite: `set_tray` calling a `TrayIcon` setter (they post to
    // the main thread and wait, menu/mod.rs:26-40), so it returns only after the
    // held task gave up.
    let _serial = serial();
    let _watchdog = watchdog("set_tray_returns_while_the_main_thread_is_blocked");
    let dir = TempDir::new();
    let (app, outcome) = wry_app(dir.path(), Start::Loaded);

    let (code, report) = run_with_driver(app, outcome, |handle| {
        let Some(part) = tray::part(&handle) else {
            end_loop(&handle, 0);
            return SetTrayReport::default();
        };
        let (blocked_tx, blocked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (seen_tx, seen_rx) = mpsc::channel();
        let _ = handle.run_on_main_thread(move || {
            let _ = blocked_tx.send(());
            let released = release_rx.recv_timeout(HOLD).is_ok();
            let _ = seen_tx.send(released);
        });
        let main_blocked = blocked_rx.recv_timeout(BUDGET).is_ok();
        let started = Instant::now();
        part.set_tray(TrayState::Recording, false);
        let set_tray_took = started.elapsed();
        let _ = release_tx.send(());
        let released_by_driver = seen_rx.recv_timeout(HOLD + BUDGET).unwrap_or(false);
        let applied = poll(BUDGET, || {
            tray::applied(&handle).map(|a| a.state) == Some(TrayState::Recording)
        });
        end_loop(&handle, 0);
        SetTrayReport {
            part: true,
            main_blocked,
            set_tray_took,
            released_by_driver,
            applied,
        }
    });

    let report = report.expect("the driver sent no report");
    assert!(report.part, "no tray part in the real runtime");
    assert!(
        report.main_blocked,
        "premise: the main thread ran the blocking task: {report:?}"
    );
    assert!(
        report.released_by_driver && report.set_tray_took < HOLD,
        "set_tray returned only after the blocked main thread gave up \
         (it waited for the main thread): {report:?}"
    );
    assert!(
        report.applied,
        "the main thread did not apply Recording once released: {report:?}"
    );
    assert_eq!(code, 0, "run_return code after end_loop(0)");
}

/// The class of tray-icon's hidden window (tray-icon 0.25.1 windows/mod.rs:100).
/// tray-icon destroys that window, and deletes the notification-area icon, when the
/// last copy of the icon is dropped (windows/mod.rs:313-325).
const TRAY_WINDOW_CLASS: &str = "tray_icon_app";

/// Every top-level window of this process with class `tray_icon_app` (hidden ones
/// included), as raw handles (a raw fact, F-003).
fn tray_windows() -> BTreeSet<isize> {
    unsafe extern "system" fn collect(hwnd: HWND, found: LPARAM) -> BOOL {
        let mut pid = 0u32;
        // SAFETY: `pid` is a writable local; the call only reads the window's owner.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == std::process::id() {
            let mut buf = [0u16; 64];
            // SAFETY: `buf` is writable; GetClassNameW sends no message.
            let len = unsafe { GetClassNameW(hwnd, &mut buf) };
            let len = usize::try_from(len).unwrap_or(0).min(buf.len());
            if String::from_utf16_lossy(&buf[..len]) == TRAY_WINDOW_CLASS {
                // SAFETY: `found` is the `Vec` that `tray_windows` passes, alive and
                // not otherwise touched for the whole `EnumWindows` call.
                unsafe { (*(found.0 as *mut Vec<isize>)).push(hwnd.0 as isize) };
            }
        }
        BOOL::from(true)
    }
    let mut found: Vec<isize> = Vec::new();
    // SAFETY: the callback writes only to `found`, and only during this call.
    unsafe {
        EnumWindows(
            Some(collect),
            LPARAM(&mut found as *mut Vec<isize> as isize),
        )
    }
    .expect("EnumWindows");
    found.into_iter().collect()
}

/// True while `raw` is a window handle.
fn is_window(raw: isize) -> bool {
    // SAFETY: IsWindow only checks the handle.
    unsafe { IsWindow(Some(HWND(raw as *mut std::ffi::c_void))) }.as_bool()
}

#[derive(Debug, Default)]
struct ExitReport {
    tray: bool,
    posted: bool,
    /// The `tray_icon_app` windows of this process that appeared with this app.
    tray_windows: Vec<isize>,
}

#[test]
fn the_tray_exit_item_ends_the_loop_with_code_0() {
    // T-052 red-test table row 14 (Acceptance 2): the tray's "Exit" item, handled
    // on the main thread like a real menu event, ends the loop: `run_return`
    // returns 0 (`AppHandle::exit(0)` is ExitRequested { code: Some(0) }, never
    // prevented). If the loop still runs `BUDGET` later, a fallback ends it with
    // code 70, which the test refuses. Once `run_return` has returned, the tray's
    // hidden window is gone: tauri's exit cleanup dropped the last copy of the
    // icon, so tray-icon deleted it from the notification area (review round 1 #1).
    // Bite: Exit not wired, a code other than 0, `prevent_exit` applied to a
    // programmatic exit (then the watchdog fires), or a `TrayIcon` copy kept past
    // the cleanup (the icon stays in the notification area after Exit).
    let _serial = serial();
    let _watchdog = watchdog("the_tray_exit_item_ends_the_loop_with_code_0");
    let dir = TempDir::new();
    let before = tray_windows();
    let (app, outcome) = wry_app(dir.path(), Start::Loaded);

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let tray = handle.tray_by_id(TRAY_ID).is_some();
        let tray_windows: Vec<isize> = tray_windows().difference(&before).copied().collect();
        let inner = handle.clone();
        let posted = handle
            .run_on_main_thread(move || {
                tray::on_menu_event(
                    &inner,
                    MenuEvent {
                        id: MenuId::new(TrayAction::Exit.as_str()),
                    },
                );
            })
            .is_ok();
        let fallback = handle.clone();
        thread::spawn(move || {
            thread::sleep(BUDGET);
            end_loop(&fallback, FALLBACK_CODE);
        });
        ExitReport {
            tray,
            posted,
            tray_windows,
        }
    });

    let report = report.expect("the driver sent no report");
    assert!(report.tray, "no tray icon `{TRAY_ID}` in the real runtime");
    assert!(
        report.posted,
        "premise: the Exit item was posted to the main thread"
    );
    assert_eq!(
        code, 0,
        "run_return code after the tray Exit item ({FALLBACK_CODE} = the loop was still \
         running {BUDGET:?} later and the fallback ended it)"
    );
    assert!(
        !report.tray_windows.is_empty(),
        "premise: no `{TRAY_WINDOW_CLASS}` window of this process appeared with the app: \
         {report:?}"
    );
    let left: Vec<isize> = report
        .tray_windows
        .iter()
        .copied()
        .filter(|&raw| is_window(raw))
        .collect();
    assert!(
        left.is_empty(),
        "`{TRAY_WINDOW_CLASS}` window(s) {left:?} still exist after the tray Exit: a copy of \
         the icon outlived tauri's exit cleanup, so the icon stays in the notification area"
    );
}
