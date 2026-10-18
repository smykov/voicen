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
use voicen_core::platform::Indicator;
use voicen_core::recording::{OverlayState, TrayState};
use voicen_core::secrets::{CredentialStore, FakeCredentialStore};
use voicen_core::settings::LoadOutcome;
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_core::tray::TrayAction;
use voicen_lib::dictation::ShellIndicator;
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

/// How long the driver waits for a step that is not a window build: a window's
/// removal (`gone`), the loop ending (`ended`), a tray removal, an apply, the
/// driver's report after the loop returned. It is not sized for, and must not be
/// raised for, a cold WebView2 build (T-097, os-answer-deadline-race: a build is
/// decided through the opener's FIFO in `ready_opened`). One build wait still uses
/// it as an unaudited ceiling: the overlay poll in
/// `destroying_the_overlay_as_the_last_window_keeps_the_app_while_the_tray_exists`
/// (the overlay has no receipt; T-097 follow-up (1)).
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

/// The app's log under the test's own dir (`<dir>/logs/voicen.log`), so a failure
/// message can show what the app logged (the opener writes a failed open as
/// `warning kind=settings_window_failed`; T-006 verify 1).
fn test_log(dir: &Path) -> Arc<Log> {
    voicen_lib::diag::start(dir.join("logs"), Box::new(|_| {}))
}

/// The lines the app logged under `dir` so far (none when nothing was written).
fn logged(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("logs").join("voicen.log"))
        .map(|t| t.lines().map(str::to_string).collect())
        .unwrap_or_default()
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
    let log = test_log(dir);
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

/// A hang bound, not a budget: how long `ready_opened`'s probe may wait for the
/// opener to reach it. It only ends a build that never finishes (with the
/// watchdog above it); it must exceed `OPENER_HELD` plus a cold WebView2 build
/// (up to 37.7 s seen on the runner) and stay below `WATCHDOG` (T-097).
const OPENER_HANG: Duration = Duration::from_secs(120);

/// What the driver saw of `Ready`'s first-run open: two facts, so a red verdict can
/// say which of two different failures happened.
#[derive(Debug, Default, Clone, Copy)]
struct ReadyOpen {
    /// The opener answered `ready_opened`'s probe within `OPENER_HANG`, so `Ready`'s
    /// open (if `Ready` asked for one) has run, Ok or Err. False: a build still
    /// running past the hang bound (hung).
    answered: bool,
    /// The settings window existed when the probe answered (read only then).
    window: bool,
    /// How long the probe waited for the opener (the queue ahead of it plus the
    /// window build; printed, not judged).
    waited: Duration,
}

impl ReadyOpen {
    /// "The window opened on Ready": the probe answered and the window is there.
    fn opened(&self) -> bool {
        self.answered && self.window
    }
}

/// Whether `Ready`'s first-run open has left the settings window open, as the driver
/// sees it. The one place the two first-run tests and the T-097 reproduction decide
/// "the window opened on Ready".
///
/// No wall-clock ceiling on the window build (T-097, os-answer-deadline-race): the
/// driver posts a probe through `request_with` after `Ready` was handled; the opener
/// runs its jobs one at a time in posting order and a window open returns only once
/// the main thread has built (and tauri attached) the window, so the probe's receipt
/// comes back only after `Ready`'s open has run, Ok or Err. The window is read then.
/// A probe still unanswered after `OPENER_HANG` (a hung build) reads as
/// `answered: false`.
fn ready_opened(handle: &AppHandle<Wry>) -> ReadyOpen {
    let started = Instant::now();
    let probe = voicen_lib::settings_window::request_with(handle, Box::new(|_| Ok(())));
    let answered = probe.wait_timeout(OPENER_HANG).is_some();
    let waited = started.elapsed();
    let window = answered && handle.get_webview_window(LABEL).is_some();
    eprintln!("lifecycle: Ready's first-run open probe answered={answered} after {waited:?}");
    ReadyOpen {
        answered,
        window,
        waited,
    }
}

/// The verdict "Ready opened the first-run settings window", with the app's log read
/// when the driver decided: a failed build leaves a `settings_window_failed` line
/// (T-006 verify 1: slow or failed WebView2 window build).
fn assert_opened_on_ready(open: ReadyOpen, lines: &[String]) {
    let failed = lines.iter().any(|l| l.contains("settings_window_failed"));
    let opener = if failed {
        "the opener logged a failed open (the window build failed)"
    } else {
        "the opener logged no failed open"
    };
    assert!(
        open.answered,
        "Ready's first-run open did not finish within the hang bound {OPENER_HANG:?} \
         (the window build hung); {opener}; {open:?}; the app's log then: {lines:#?}"
    );
    assert!(
        open.window,
        "Ready's first-run open ran (the opener answered after {:?}) but left no settings \
         window; {}; the app's log then: {lines:#?}",
        open.waited,
        if failed {
            opener
        } else {
            "the opener logged no failed open (the open was never asked for, or there is \
             no opener)"
        }
    );
}

#[derive(Debug, Default)]
struct CloseReport {
    opened: ReadyOpen,
    destroyed: bool,
    gone: bool,
    alive_at_once: bool,
    alive_later: bool,
    /// The app's log when `opened` failed, read before the loop is ended (ending it
    /// fails a build still waiting on it, which would log a failure of its own).
    log_at_verdict: Vec<String>,
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
    let logs_of = dir.path().to_path_buf();

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let mut report = CloseReport {
            opened: ready_opened(&handle),
            ..CloseReport::default()
        };
        if !report.opened.opened() {
            report.log_at_verdict = logged(&logs_of);
        }
        if report.opened.opened() {
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
    assert_opened_on_ready(report.opened, &report.log_at_verdict);
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

/// How long the T-097 reproduction holds the opener before `Ready`'s open may run:
/// longer than `BUDGET`, as the cold first WebView2 build was on the red runs
/// (21.6 s, 35.3 s, 37.7 s).
const OPENER_HELD: Duration = Duration::from_secs(25);

#[derive(Debug, Default)]
struct HeldOpenerReport {
    /// The held job had not finished when the driver started (premise).
    held_at_ready: bool,
    opened: ReadyOpen,
    /// The window exists when the driver looks after `ready_opened` (a raw fact, not
    /// the helper's own verdict).
    window_after: bool,
    /// The app's log when `opened` failed, read before the loop is ended.
    log_at_verdict: Vec<String>,
}

#[test]
fn ready_opens_the_first_run_window_when_its_open_queues_behind_a_slow_build() {
    // T-097 (rca, os-answer-deadline-race; F-005, F-013): the deterministic red
    // reproduction of the flake of `destroying_the_last_window_keeps_the_app_running`.
    // A cold first WebView2 build can outlast any wall-clock budget (21.6-37.7 s on
    // the red runs against `BUDGET`'s 20 s); here the opener is held by a
    // `request_with` job for `OPENER_HELD` (> `BUDGET`), posted before `run_return`,
    // so `Ready`'s first-run open queues behind it exactly as it waits behind a slow
    // build. The product is correct (Ready asks at once, the opener then opens the
    // window), so "the window opened on Ready" must hold, decided through the same
    // helper as the two first-run tests (`ready_opened`). Red while that helper is a
    // wall-clock ceiling (`poll(BUDGET, ..)`); green once it waits for Ready's open
    // through the opener's FIFO order (a probe posted after Ready), with only a hang
    // bound above `OPENER_HELD` plus a build. Bite: a probe that does not go through
    // the opener's queue (it returns before Ready's open ran), or any ceiling below
    // `OPENER_HELD`.
    let _serial = serial();
    let _watchdog =
        watchdog("ready_opens_the_first_run_window_when_its_open_queues_behind_a_slow_build");
    let dir = TempDir::new();
    let (app, outcome) = wry_app(dir.path(), Start::FirstRun);
    let logs_of = dir.path().to_path_buf();

    let (held_tx, held_rx) = mpsc::channel::<()>();
    let _held = voicen_lib::settings_window::request_with(
        app.handle(),
        Box::new(move |_| {
            thread::sleep(OPENER_HELD);
            let _ = held_tx.send(());
            Ok(())
        }),
    );

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let held_at_ready = matches!(held_rx.try_recv(), Err(mpsc::TryRecvError::Empty));
        let mut report = HeldOpenerReport {
            held_at_ready,
            opened: ready_opened(&handle),
            ..HeldOpenerReport::default()
        };
        report.window_after = handle.get_webview_window(LABEL).is_some();
        if !report.opened.opened() {
            report.log_at_verdict = logged(&logs_of);
        }
        end_loop(&handle, 0);
        report
    });

    let report = report.expect("the driver sent no report");
    assert!(
        report.held_at_ready,
        "premise: the opener was still held by the planted job when Ready's driver \
         started: {report:?}"
    );
    assert_opened_on_ready(report.opened, &report.log_at_verdict);
    assert!(
        report.window_after,
        "`ready_opened` reported an open window that the app does not have: {report:?}"
    );
    assert_eq!(code, 0, "run_return code after end_loop(0)");
}

/// The overlay window's label (spec 001 contracts/ipc.md; T-057).
const OVERLAY: &str = "overlay";

#[derive(Debug, Default)]
struct OverlayCloseReport {
    tray: bool,
    shown: bool,
    gone: bool,
    alive_at_once: bool,
    alive_later: bool,
    log: Vec<String>,
}

#[test]
fn destroying_the_overlay_as_the_last_window_keeps_the_app_while_the_tray_exists() {
    // T-057 analysis fact 5 and red-test table row 6: on a loaded start the overlay
    // is the app's only window, so Hidden destroying it raises
    // ExitRequested { code: None } (runtime-wry lib.rs:4256-4270). With the tray the
    // app keeps running: a main-thread round trip still answers, at once and a second
    // later. Red until the overlay exists (no window is built). Bite: the overlay
    // window destroyed without the guard of `on_run_event` applying (a second exit
    // rule), or the overlay never built.
    let _serial = serial();
    let _watchdog =
        watchdog("destroying_the_overlay_as_the_last_window_keeps_the_app_while_the_tray_exists");
    let dir = TempDir::new();
    let (app, outcome) = wry_app(dir.path(), Start::Loaded);
    let logs_of = dir.path().to_path_buf();

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let indicator = ShellIndicator::new(&handle);
        let mut report = OverlayCloseReport {
            tray: handle.tray_by_id(TRAY_ID).is_some(),
            ..OverlayCloseReport::default()
        };
        indicator.set_overlay(&OverlayState::Recording);
        report.shown = poll(BUDGET, || handle.get_webview_window(OVERLAY).is_some());
        if report.shown {
            indicator.set_overlay(&OverlayState::Hidden);
            report.gone = poll(BUDGET, || handle.get_webview_window(OVERLAY).is_none());
            report.alive_at_once = main_round_trip(&handle);
            thread::sleep(Duration::from_secs(1));
            report.alive_later = main_round_trip(&handle);
        } else {
            report.log = logged(&logs_of);
        }
        end_loop(&handle, 0);
        report
    });

    let report = report.expect("the driver sent no report");
    assert!(report.tray, "premise: the tray exists: {report:?}");
    assert!(
        report.shown,
        "Recording built no `{OVERLAY}` window within {BUDGET:?}: {report:#?}"
    );
    assert!(
        report.gone,
        "premise: Hidden destroyed the overlay: {report:?}"
    );
    assert!(
        report.alive_at_once && report.alive_later,
        "the app ended when the overlay, its last window, was destroyed while the tray \
         exists (ExitRequested {{ code: None }} not prevented): {report:?}"
    );
    assert_eq!(code, 0, "run_return code after end_loop(0)");
}

#[derive(Debug, Default)]
struct NoTrayReport {
    opened: ReadyOpen,
    tray_removed: bool,
    destroyed: bool,
    /// A main-thread round trip failed within `BUDGET` after the destroy: the loop
    /// ended by itself.
    ended: bool,
}

#[test]
fn without_the_tray_destroying_the_last_window_ends_the_app() {
    // T-052 validation F3, T-057 Q1 (default: keep the guard as it is): the branch
    // "no tray -> do not prevent exit" of `on_run_event`. Without a tray there is no
    // other way to end the process, so destroying the last window ends the loop by
    // itself (ExitRequested { code: None } passes): `run_return` returns before the
    // fallback ends it with code 70. Characterization (green before T-057; it must
    // stay green): the window is the first-run settings window, which exists today.
    // Bite: `prevent_exit` applied whatever the tray (the loop runs on until the
    // fallback), e.g. an "it was the overlay" exception that also catches this path.
    let _serial = serial();
    let _watchdog = watchdog("without_the_tray_destroying_the_last_window_ends_the_app");
    let dir = TempDir::new();
    let (app, outcome) = wry_app(dir.path(), Start::FirstRun);

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let mut report = NoTrayReport {
            opened: ready_opened(&handle),
            ..NoTrayReport::default()
        };
        // The fallback ends a loop that is still running; on an ended loop its task
        // never runs.
        let fallback = handle.clone();
        thread::spawn(move || {
            thread::sleep(BUDGET * 2);
            end_loop(&fallback, FALLBACK_CODE);
        });
        if !report.opened.opened() {
            return report;
        }
        // Removed (and the removed icon dropped) on the main thread, where tauri's
        // tray calls run.
        let (removed_tx, removed_rx) = mpsc::channel();
        let inner = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            let removed = inner.remove_tray_by_id(TRAY_ID);
            let _ = removed_tx.send(removed.is_some());
        });
        report.tray_removed = removed_rx.recv_timeout(BUDGET).unwrap_or(false)
            && handle.tray_by_id(TRAY_ID).is_none();
        if !report.tray_removed {
            return report;
        }
        report.destroyed = handle
            .get_webview_window(LABEL)
            .is_some_and(|w| w.destroy().is_ok());
        report.ended = poll(BUDGET, || !main_round_trip(&handle));
        report
    });

    let report = report.expect("the driver sent no report");
    assert!(
        report.opened.opened() && report.tray_removed && report.destroyed,
        "premise: the settings window opened, the tray was removed and the window \
         destroyed: {report:?}"
    );
    assert!(
        report.ended,
        "without a tray the loop still answered {BUDGET:?} after its last window was \
         destroyed (ExitRequested {{ code: None }} prevented): {report:?}"
    );
    assert_ne!(
        code, FALLBACK_CODE,
        "run_return code {FALLBACK_CODE}: the loop ran on until the fallback ended it"
    );
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
