//! T-057: the overlay window on tauri's real Wry runtime (WebView2), driven from a
//! libtest thread. Windows CI only (decision #5). Acceptance 1 and 2 (shell half);
//! analysis red-test table rows 2, 3 and 4.
//!
//! The harness is `tests/lifecycle.rs`'s (T-052), repeated here because each test
//! file is its own crate:
//! - the loop runs with `Builder::any_thread` and `App::run_return`, never `run`
//!   (`run` ends in `process::exit`); the single-instance plugin is never registered;
//! - each test builds its own app with `build_app` (so the overlay thread and the
//!   tray are the ones `run()` gets) and calls `voicen_lib::on_run_event` from the
//!   loop callback; a driver thread, started on `Ready`, acts and reports; the test
//!   asserts after `run_return` returned;
//! - the loop is only ended from inside a main-thread task (`end_loop`); a driver
//!   that panics (a loud runner precondition included) still ends the loop, and the
//!   test re-raises its message;
//! - a watchdog ends the exe non-zero, with the test's name, if a loop never
//!   returns; the tests are serialised (one WebView2 profile, one foreground).
//!
//! The overlay is reached only the way `run()` reaches it: through
//! `dictation::ShellIndicator::new(app)`, the session's `Indicator`. Every fact is a
//! raw Win32 value (F-003): `IsWindowVisible`, `GWL_EXSTYLE`, `GetForegroundWindow`,
//! the target window's own messages. Every wait polls against a Windows-sized budget
//! (F-005; runner run B showed ~1 s scheduling gaps).
//!
//! Runner capabilities (`docs/decisions/windows-ci-runner.md`): the target window is
//! brought to the front through `win32_support::bring_to_front` (fact
//! `foreground_again`, re-measured by the probe step of the same job; `sendinput`
//! ok in runs A and B), which asserts it loudly, so "the target stays in front"
//! never passes vacuously.
//!
//! The app identifier is a test one; each test has its own `TempDir`; no keys.
#![cfg(windows)]

mod win32_support;

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tauri::test::{mock_context, noop_assets};
use tauri::{App, AppHandle, Context, Manager, RunEvent, Wry};
use voicen_core::diag::Log;
use voicen_core::i18n;
use voicen_core::local_models::catalog::MODELS;
use voicen_core::local_models::service::LocalModels;
use voicen_core::local_models::store::ModelStore;
use voicen_core::platform::Indicator;
use voicen_core::recording::OverlayState;
use voicen_core::secrets::{CredentialStore, FakeCredentialStore};
use voicen_core::settings::LoadOutcome;
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_lib::dictation::ShellIndicator;
use voicen_lib::settings_ipc::load_settings;
use win32_support::{class_of, foreground, Shape, TestWindow};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowLongPtrW, GetWindowThreadProcessId, IsWindow, IsWindowVisible,
    GWL_EXSTYLE, GW_OWNER, WS_EX_NOACTIVATE, WS_EX_TOPMOST,
};

/// The overlay window's label (spec 001 contracts/ipc.md: the page at `/overlay`
/// in the window labelled `overlay`).
const LABEL: &str = "overlay";

/// tao's per-thread event-target window: top-level, unowned and WS_VISIBLE in every
/// tauri process (scripts/ci/visible-windows.ps1 header), so never "an overlay".
const TAO_EVENT_TARGET: &str = "Tao Thread Event Target";

/// A test identifier: WebView2's profile goes under it, not under the release one.
const IDENTIFIER: &str = "dev.voicen.test.overlay";

/// How long the driver waits for the overlay to appear or go (Windows-sized: a cold
/// WebView2 creation on the runner, F-005).
const BUDGET: Duration = Duration::from_secs(20);

/// How long a state is watched after it showed, so a late activation, a late
/// second window or a late `SW_SHOW` is seen (a bounded negative).
const SETTLE: Duration = Duration::from_secs(2);

/// How long a blocked main-thread task waits for its release.
const HOLD: Duration = Duration::from_secs(10);

/// The exe ends with code 3 if one test's loop has not returned by then.
const WATCHDOG: Duration = Duration::from_secs(240);

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Ends the exe with code 3 unless dropped within `WATCHDOG`.
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
                "overlay watchdog: `{test}`: the event loop did not return within \
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

/// The app's log under the test's own dir, so a failure can show what it logged.
fn test_log(dir: &Path) -> Arc<Log> {
    voicen_lib::diag::start(dir.join("logs"), Box::new(|_| {}))
}

/// The lines the app logged under `dir` so far (none when nothing was written).
fn logged(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("logs").join("voicen.log"))
        .map(|t| t.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// The app as `run()` builds it (`build_app`), on the real Wry runtime, on a loaded
/// settings.json (so `Ready` opens no settings window: the overlay is the only
/// window the test makes the app show).
fn wry_app(dir: &Path) -> (App<Wry>, LoadOutcome) {
    let log = test_log(dir);
    let load = || {
        load_settings(
            dir.to_path_buf(),
            no_keys(),
            Arc::new(voicen_core::autostart::FakeAutostart::new()),
            Arc::new(ModelStore::new(dir.join("models"), MODELS)),
            None,
            &log,
        )
    };
    let (_, first) = load();
    assert!(
        matches!(first, LoadOutcome::FirstRun(_)),
        "premise: {first:?}"
    );
    let (service, outcome) = load();
    assert!(
        matches!(outcome, LoadOutcome::Loaded(_)),
        "premise: {outcome:?}"
    );
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

/// The text of a caught panic.
fn panic_text(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "a panic without a message".to_string())
}

/// Runs the loop with `on_run_event` (as `run()` does) and starts `driver` on its own
/// thread once `Ready` was handled. The driver's panic (a failed precondition
/// included) is caught, the loop is ended, and the test re-raises it. Returns
/// `run_return`'s code and the driver's report.
#[track_caller]
fn run_with_driver<T: Send + 'static>(
    app: App<Wry>,
    outcome: LoadOutcome,
    driver: impl FnOnce(AppHandle<Wry>) -> T + Send + 'static,
) -> (i32, T) {
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
                    let result = panic::catch_unwind(AssertUnwindSafe(|| driver(handle.clone())));
                    let result = result.map_err(|p| panic_text(p.as_ref()));
                    end_loop(&handle, 0);
                    let _ = tx.send(result);
                });
            }
        }
    });
    match rx.recv_timeout(BUDGET) {
        Ok(Ok(report)) => (code, report),
        Ok(Err(message)) => panic!("{message}"),
        Err(err) => panic!("the driver sent no report ({err}); run_return code {code}"),
    }
}

/// Ends the loop with `code` from inside a main-thread task; on a loop that has
/// already ended the task is never run.
fn end_loop(handle: &AppHandle<Wry>, code: i32) {
    let inner = handle.clone();
    let _ = handle.run_on_main_thread(move || inner.exit(code));
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

/// True when `cond` holds at every 10 ms sample for `within` (a bounded negative).
fn holds_for(within: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if !cond() {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
    cond()
}

fn to_hwnd(raw: isize) -> HWND {
    HWND(raw as *mut std::ffi::c_void)
}

/// The raw handle of the window labelled `overlay`, if tauri has one.
fn overlay_hwnd(handle: &AppHandle<Wry>) -> Option<isize> {
    handle
        .get_webview_window(LABEL)
        .and_then(|w| w.hwnd().ok())
        .map(|h| h.0 as isize)
}

fn is_window(raw: isize) -> bool {
    // SAFETY: IsWindow only checks the handle.
    unsafe { IsWindow(Some(to_hwnd(raw))) }.as_bool()
}

fn is_visible(raw: isize) -> bool {
    // SAFETY: IsWindowVisible only reads the window's style bit.
    unsafe { IsWindowVisible(to_hwnd(raw)) }.as_bool()
}

fn ex_style(raw: isize) -> u32 {
    // SAFETY: GetWindowLongPtrW reads the window's extended style; no message is sent.
    let value = unsafe { GetWindowLongPtrW(to_hwnd(raw), GWL_EXSTYLE) };
    value as u32
}

/// The raw facts of the overlay window at one moment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Facts {
    hwnd: isize,
    visible: bool,
    noactivate: bool,
    topmost: bool,
}

fn facts(raw: isize) -> Facts {
    let ex = ex_style(raw);
    Facts {
        hwnd: raw,
        visible: is_visible(raw),
        noactivate: ex & WS_EX_NOACTIVATE.0 != 0,
        topmost: ex & WS_EX_TOPMOST.0 != 0,
    }
}

/// Every visible, unowned, top-level window of this process but tao's event-target
/// window and `except` (the install smoke's rule after T-057, tool windows counted).
fn shown_windows(except: &[isize]) -> Vec<(isize, String)> {
    unsafe extern "system" fn collect(hwnd: HWND, found: LPARAM) -> BOOL {
        let mut pid = 0u32;
        // SAFETY: `pid` is a writable local; the call only reads the window's owner.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        // SAFETY: both calls only read the window's state; no message is sent.
        let shown = pid == std::process::id()
            && unsafe { IsWindowVisible(hwnd) }.as_bool()
            && unsafe { GetWindow(hwnd, GW_OWNER) }.is_err();
        if shown {
            // SAFETY: `found` is the `Vec` that `shown_windows` passes, alive and not
            // otherwise touched for the whole `EnumWindows` call.
            unsafe { (*(found.0 as *mut Vec<isize>)).push(hwnd.0 as isize) };
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
    found
        .into_iter()
        .filter(|raw| !except.contains(raw))
        .map(|raw| (raw, class_of(to_hwnd(raw))))
        .filter(|(_, class)| class != TAO_EVENT_TARGET)
        .collect()
}

/// The sample count and every window other than the target seen in front (with
/// its class).
type Sampled = (u64, Vec<(isize, String)>);

/// Samples `GetForegroundWindow` every 5 ms on its own thread and keeps every sample
/// that was not `target` (with its class), until stopped.
struct ForegroundSampler {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Sampled>>,
}

impl ForegroundSampler {
    fn start(target: isize) -> ForegroundSampler {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let mut samples = 0u64;
            let mut away: Vec<(isize, String)> = Vec::new();
            while !stop_thread.load(Ordering::SeqCst) {
                let now = foreground();
                samples += 1;
                if now.0 as isize != target && !away.iter().any(|(h, _)| *h == now.0 as isize) {
                    away.push((now.0 as isize, class_of(now)));
                }
                thread::sleep(Duration::from_millis(5));
            }
            (samples, away)
        });
        ForegroundSampler {
            stop,
            thread: Some(thread),
        }
    }

    /// The number of samples and every window other than the target seen in front.
    fn stop(mut self) -> (u64, Vec<(isize, String)>) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread
            .take()
            .map(|t| t.join().unwrap_or_default())
            .unwrap_or_default()
    }
}

impl Drop for ForegroundSampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn message() -> OverlayState {
    OverlayState::Message {
        id: i18n::NOTICE_NO_SPEECH,
        params: Vec::new(),
        until: Instant::now() + Duration::from_secs(3),
    }
}

/// Warning lines the app logged (any kind): an overlay build or emit failure is a
/// typed warning (analysis design), never silent.
fn warnings(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .filter(|l| l.contains("warning"))
        .cloned()
        .collect()
}

#[derive(Debug, Default)]
struct StateStep {
    state: &'static str,
    shown_within_budget: bool,
    facts: Facts,
    /// Every sample of the overlay's facts during `SETTLE` after it showed matched
    /// the first (still visible, same window, styles kept).
    stable: bool,
}

#[derive(Debug, Default)]
struct ShowReport {
    target: isize,
    steps: Vec<StateStep>,
    foreground_samples: u64,
    /// Windows other than the target that were in front while the overlay showed.
    foreground_away: Vec<(isize, String)>,
    /// The target's focus or activation losses while the overlay showed.
    focus_losses: Vec<(u32, usize)>,
    hidden_gone: bool,
    hidden_label_freed: bool,
    log: Vec<String>,
}

#[test]
fn recording_processing_and_message_show_the_overlay_while_the_target_keeps_the_foreground() {
    // Acceptance 1 (FR-04, NFR-03, spec 001 FR-008), invariants 2 and 3: with a target
    // window in front (asserted loudly by `front`), Recording, Processing and Message
    // published through the session's `Indicator` show one overlay window: visible,
    // WS_EX_NOACTIVATE and WS_EX_TOPMOST, the same window for all three (later states
    // are emits). The target stays the foreground window at every 5 ms sample and gets
    // no WM_KILLFOCUS / WA_INACTIVE / inactive WM_NCACTIVATE. Hidden destroys the
    // window: the handle is gone and tauri freed the label. Bite: `set_overlay` left a
    // no-op (no window), a window built without `focusable(false)` (no NOACTIVATE) or
    // with `focused(true)`, a `show()`/`set_focus()`/tao setter on the visible window
    // (SW_SHOW activates), a rebuild per state, or Hidden only hiding the window.
    let _serial = serial();
    let _watchdog = watchdog(
        "recording_processing_and_message_show_the_overlay_while_the_target_keeps_the_foreground",
    );
    let dir = TempDir::new();
    let (app, outcome) = wry_app(dir.path());
    let logs_of = dir.path().to_path_buf();

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let target = TestWindow::open(Shape::TopLevel);
        target.front();
        let target_raw = target.hwnd().0 as isize;
        let losses_before = target.focus_losses().len();
        let sampler = ForegroundSampler::start(target_raw);
        let indicator = ShellIndicator::new(&handle);
        let mut report = ShowReport {
            target: target_raw,
            ..ShowReport::default()
        };
        for (name, state) in [
            ("Recording", OverlayState::Recording),
            ("Processing", OverlayState::Processing),
            ("Message", message()),
        ] {
            indicator.set_overlay(&state);
            let mut step = StateStep {
                state: name,
                ..StateStep::default()
            };
            step.shown_within_budget = poll(BUDGET, || {
                overlay_hwnd(&handle).is_some_and(|raw| facts(raw).visible)
            });
            if let Some(raw) = overlay_hwnd(&handle) {
                step.facts = facts(raw);
                let first = step.facts;
                step.stable = holds_for(SETTLE, || facts(raw) == first);
            }
            let stop = !step.shown_within_budget;
            report.steps.push(step);
            if stop {
                break;
            }
        }
        let (samples, away) = sampler.stop();
        report.foreground_samples = samples;
        report.foreground_away = away;
        report.focus_losses = target
            .focus_losses()
            .get(losses_before..)
            .map(<[_]>::to_vec)
            .unwrap_or_default();
        let last = report.steps.last().map(|s| s.facts.hwnd).unwrap_or(0);
        indicator.set_overlay(&OverlayState::Hidden);
        report.hidden_label_freed = poll(BUDGET, || handle.get_webview_window(LABEL).is_none());
        report.hidden_gone = last != 0 && poll(BUDGET, || !is_window(last));
        report.log = logged(&logs_of);
        report
    });

    let steps = &report.steps;
    for step in steps {
        assert!(
            step.shown_within_budget,
            "{}: no visible `{LABEL}` window within {BUDGET:?} after set_overlay; the app's \
             log: {:#?}; report: {report:#?}",
            step.state, report.log
        );
        assert!(
            step.facts.noactivate,
            "{}: the overlay has no WS_EX_NOACTIVATE (GWL_EXSTYLE): {step:?}",
            step.state
        );
        assert!(
            step.facts.topmost,
            "{}: the overlay has no WS_EX_TOPMOST (GWL_EXSTYLE): {step:?}",
            step.state
        );
        assert!(
            step.stable,
            "{}: within {SETTLE:?} after it showed the overlay changed (hidden, rebuilt or \
             its styles rewritten): {step:?}",
            step.state
        );
    }
    assert_eq!(steps.len(), 3, "states shown: {report:#?}");
    let first = steps[0].facts.hwnd;
    assert!(
        steps.iter().all(|s| s.facts.hwnd == first),
        "the overlay window was rebuilt between states (a later state must be an emit to \
         the live window): {steps:#?}"
    );
    assert!(
        report.foreground_samples > 0,
        "premise: the foreground sampler ran: {report:#?}"
    );
    assert!(
        report.foreground_away.is_empty(),
        "the target lost the foreground while the overlay showed; in front instead: {:?} \
         (target {:#x})",
        report.foreground_away,
        report.target
    );
    assert!(
        report.focus_losses.is_empty(),
        "the target got focus/activation-loss messages while the overlay showed \
         ((message, wParam): 0x8 WM_KILLFOCUS, 0x6 WM_ACTIVATE, 0x86 WM_NCACTIVATE): {:?}",
        report.focus_losses
    );
    assert!(
        report.hidden_gone && report.hidden_label_freed,
        "Hidden did not destroy the overlay within {BUDGET:?} (NFR-03: no window while \
         Hidden): {report:#?}"
    );
    assert_eq!(code, 0, "run_return code after end_loop(0)");
}

#[derive(Debug, Default)]
struct RapidReport {
    first_shown: bool,
    /// Pauses between Hidden and Recording, in ms, in the order run.
    pauses_ms: Vec<u64>,
    /// The old window was gone (`IsWindow` false) before the last Recording.
    old_gone_before_last: bool,
    one_within_budget: bool,
    stayed_one: bool,
    shown: Vec<(isize, String)>,
    label_hwnd: Option<isize>,
    label_facts: Facts,
    new_warnings: Vec<String>,
    log: Vec<String>,
}

#[test]
fn hidden_then_recording_at_once_leaves_exactly_one_visible_overlay() {
    // Acceptance 2 (failure branch): a sub-0.3 s tap then a new press publishes
    // Recording -> Hidden -> Recording. `destroy()` is only posted and tauri frees the
    // label only on Destroyed (manager/window.rs:70-71), so a rebuild before it fails
    // with "label already exists". Run back to back with pauses from 0 to 400 ms, and
    // once more with Recording sent the moment the old handle is gone (before tauri
    // may have handled Destroyed). Afterwards exactly one visible overlay window
    // exists (the label's, WS_EX_NOACTIVATE), it stays one for `SETTLE`, and the app
    // logged no warning (a failed build is a typed warning). Bite: a build issued on
    // the state change instead of after Destroyed, a build error swallowed with no
    // rebuild (no window), or two windows.
    let _serial = serial();
    let _watchdog = watchdog("hidden_then_recording_at_once_leaves_exactly_one_visible_overlay");
    let dir = TempDir::new();
    let (app, outcome) = wry_app(dir.path());
    let logs_of = dir.path().to_path_buf();

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let indicator = ShellIndicator::new(&handle);
        let mut report = RapidReport::default();
        indicator.set_overlay(&OverlayState::Recording);
        report.first_shown = poll(BUDGET, || overlay_hwnd(&handle).is_some_and(is_visible));
        if !report.first_shown {
            report.log = logged(&logs_of);
            return report;
        }
        let warnings_before = warnings(&logged(&logs_of)).len();
        for pause in [0u64, 0, 1, 2, 5, 10, 20, 50, 100, 200, 400] {
            indicator.set_overlay(&OverlayState::Hidden);
            thread::sleep(Duration::from_millis(pause));
            indicator.set_overlay(&OverlayState::Recording);
            report.pauses_ms.push(pause);
        }
        // Once more, Recording the moment the old window's handle is gone.
        if poll(BUDGET, || overlay_hwnd(&handle).is_some_and(is_visible)) {
            let old = overlay_hwnd(&handle).unwrap_or(0);
            indicator.set_overlay(&OverlayState::Hidden);
            let deadline = Instant::now() + BUDGET;
            while is_window(old) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(1));
            }
            report.old_gone_before_last = !is_window(old);
            indicator.set_overlay(&OverlayState::Recording);
        }
        let one_visible = |handle: &AppHandle<Wry>| {
            let shown = shown_windows(&[]);
            shown.len() == 1 && overlay_hwnd(handle) == Some(shown[0].0)
        };
        report.one_within_budget = poll(BUDGET, || one_visible(&handle));
        report.stayed_one = report.one_within_budget && holds_for(SETTLE, || one_visible(&handle));
        report.shown = shown_windows(&[]);
        report.label_hwnd = overlay_hwnd(&handle);
        report.label_facts = report.label_hwnd.map(facts).unwrap_or_default();
        let lines = logged(&logs_of);
        report.new_warnings = warnings(&lines)
            .get(warnings_before..)
            .map(<[_]>::to_vec)
            .unwrap_or_default();
        report.log = lines;
        report
    });

    assert!(
        report.first_shown,
        "Recording showed no visible `{LABEL}` window within {BUDGET:?} (nothing to \
         rebuild); the app's \
         log: {:#?}",
        report.log
    );
    assert!(
        report.old_gone_before_last,
        "premise: Hidden did not destroy the overlay within {BUDGET:?}: {report:#?}"
    );
    assert!(
        report.one_within_budget && report.stayed_one,
        "after Hidden then Recording at once (pauses {:?} ms) the shown windows of the \
         process are {:?} (want exactly the `{LABEL}` window {:?}, for {SETTLE:?}); the \
         app's log: {:#?}",
        report.pauses_ms,
        report.shown,
        report.label_hwnd,
        report.log
    );
    assert!(
        report.label_facts.visible && report.label_facts.noactivate,
        "the rebuilt overlay is not a visible WS_EX_NOACTIVATE window: {:?}",
        report.label_facts
    );
    assert!(
        report.new_warnings.is_empty(),
        "the app logged warnings during Hidden -> Recording (a failed overlay build, e.g. \
         \"label already exists\"): {:#?}",
        report.new_warnings
    );
    assert_eq!(code, 0, "run_return code after end_loop(0)");
}

#[derive(Debug, Default)]
struct BlockedReport {
    main_blocked: bool,
    set_overlay_took: Duration,
    released_by_driver: bool,
    shown_after_release: bool,
}

#[test]
fn set_overlay_returns_while_the_main_thread_is_blocked() {
    // Invariant 1 (dictation-session.md: the session calls `set_overlay` under its
    // lock, so it must not wait for another thread). The main thread is held in a
    // task until the driver releases it; the driver's `set_overlay(Recording)`
    // returns before that release, and once released the overlay shows. Green with
    // the no-op `set_overlay` only on its first half; the second half is red until
    // the overlay exists. Bite: a window built (or a payload emitted through a
    // main-thread call) inside `set_overlay`: `create_window` off the main thread
    // blocks on `rx.recv()` (runtime-wry lib.rs:300-336), so it returns only after
    // the held task gave up.
    let _serial = serial();
    let _watchdog = watchdog("set_overlay_returns_while_the_main_thread_is_blocked");
    let dir = TempDir::new();
    let (app, outcome) = wry_app(dir.path());

    let (code, report) = run_with_driver(app, outcome, |handle| {
        let indicator = ShellIndicator::new(&handle);
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
        indicator.set_overlay(&OverlayState::Recording);
        let set_overlay_took = started.elapsed();
        let _ = release_tx.send(());
        let released_by_driver = seen_rx.recv_timeout(HOLD + BUDGET).unwrap_or(false);
        let shown_after_release = poll(BUDGET, || overlay_hwnd(&handle).is_some_and(is_visible));
        BlockedReport {
            main_blocked,
            set_overlay_took,
            released_by_driver,
            shown_after_release,
        }
    });

    assert!(
        report.main_blocked,
        "premise: the main thread ran the blocking task: {report:?}"
    );
    assert!(
        report.released_by_driver && report.set_overlay_took < HOLD,
        "set_overlay returned only after the blocked main thread gave up (it waited for \
         the main thread): {report:?}"
    );
    assert!(
        report.shown_after_release,
        "the overlay did not show once the main thread was released: {report:?}"
    );
    assert_eq!(code, 0, "run_return code after end_loop(0)");
}
