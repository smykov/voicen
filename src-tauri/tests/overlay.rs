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
//!
//! T-067 (decisions #86; `docs/decisions/overlay.md` §3): the overlay is
//! click-through (`WS_EX_TRANSPARENT | WS_EX_LAYERED`), out of Alt+Tab
//! (`WS_EX_TOOLWINDOW`, no `WS_EX_APPWINDOW`, unowned top-level) and shown without
//! activation. `Facts` carries these bits, so every overlay test checks the shape of
//! every window it sees ([`shape_errors`]). The two T-067 tests that read the screen
//! serve a test page (`DrawnPage`: an opaque body of [`PAGE_RGB`]) instead of the
//! empty `noop_assets`, so "drawn" and "the click passes through painted content"
//! are facts about pixels the WebView really painted.
#![cfg(windows)]

mod helper_windows;
mod win32_support;

use std::any::Any;
use std::borrow::Cow;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tauri::test::{mock_context, noop_assets};
use tauri::utils::assets::{AssetKey, AssetsIter, CspHash};
use tauri::{App, AppHandle, Assets, Context, Manager, RunEvent, Wry};
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
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetDC, GetPixel, GetSysColor, ReleaseDC, CLR_INVALID, COLOR_WINDOW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetWindow, GetWindowLongPtrW, GetWindowRect,
    GetWindowThreadProcessId, IsWindow, IsWindowVisible, SetWindowPos, WindowFromPoint, GA_ROOT,
    GWL_EXSTYLE, GW_OWNER, SWP_NOACTIVATE, SWP_NOZORDER, WS_EX_APPWINDOW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};

/// The overlay window's label (spec 001 contracts/ipc.md: the page at `/overlay`
/// in the window labelled `overlay`).
const LABEL: &str = "overlay";

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
    wry_app_with(dir, mock_context(noop_assets()))
}

/// The colour of the test page's body (`DrawnPage`): far from every system window
/// colour, so a pixel of it on screen is the overlay's paint and nothing else.
const PAGE_RGB: (u8, u8, u8) = (32, 160, 64);

/// T-067: an asset provider whose every `.html` key is one page with an opaque
/// [`PAGE_RGB`] body (tauri's resolver tries `/overlay`, then `/overlay.html`), so the
/// overlay window paints known pixels. The real pill page needs the built UI, which
/// the shell tests do not have.
struct DrawnPage;

impl DrawnPage {
    fn html() -> String {
        let (r, g, b) = PAGE_RGB;
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"></head>\
             <body style=\"margin:0;width:100vw;height:100vh;background:rgb({r},{g},{b})\">\
             </body></html>"
        )
    }
}

impl Assets<Wry> for DrawnPage {
    fn get(&self, key: &AssetKey) -> Option<Cow<'_, [u8]>> {
        key.as_ref()
            .ends_with(".html")
            .then(|| Cow::Owned(DrawnPage::html().into_bytes()))
    }

    fn iter(&self) -> Box<AssetsIter<'_>> {
        Box::new(std::iter::empty())
    }

    fn csp_hashes(&self, _html_path: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        Box::new(std::iter::empty())
    }
}

/// [`wry_app`] whose overlay page is [`DrawnPage`].
fn wry_app_drawn(dir: &Path) -> (App<Wry>, LoadOutcome) {
    wry_app_with(dir, mock_context(DrawnPage))
}

fn wry_app_with(dir: &Path, mut context: Context<Wry>) -> (App<Wry>, LoadOutcome) {
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
    /// T-067: click-through needs both `WS_EX_TRANSPARENT` and `WS_EX_LAYERED`.
    transparent: bool,
    layered: bool,
    /// T-067: out of Alt+Tab: `WS_EX_TOOLWINDOW` set and `WS_EX_APPWINDOW` clear (an
    /// APPWINDOW tool window is listed again), unowned, top-level.
    toolwindow: bool,
    appwindow: bool,
    owned: bool,
    top_level: bool,
}

fn facts(raw: isize) -> Facts {
    let ex = ex_style(raw);
    let hwnd = to_hwnd(raw);
    Facts {
        hwnd: raw,
        visible: is_visible(raw),
        noactivate: ex & WS_EX_NOACTIVATE.0 != 0,
        topmost: ex & WS_EX_TOPMOST.0 != 0,
        transparent: ex & WS_EX_TRANSPARENT.0 != 0,
        layered: ex & WS_EX_LAYERED.0 != 0,
        toolwindow: ex & WS_EX_TOOLWINDOW.0 != 0,
        appwindow: ex & WS_EX_APPWINDOW.0 != 0,
        // SAFETY: both calls only read the window's relations; no message is sent.
        owned: unsafe { GetWindow(hwnd, GW_OWNER) }.is_ok_and(|o| !o.is_invalid()),
        top_level: unsafe { GetAncestor(hwnd, GA_ROOT) } == hwnd,
    }
}

/// What a shown overlay window lacks of its required shape (T-057 invariant 3 and
/// T-067's invariant): non-activating, topmost, click-through, a tool window without
/// APPWINDOW, unowned and top-level. Empty when the shape is whole.
fn shape_errors(f: &Facts) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if !f.noactivate {
        missing.push("no WS_EX_NOACTIVATE");
    }
    if !f.topmost {
        missing.push("no WS_EX_TOPMOST");
    }
    if !f.transparent {
        missing.push("no WS_EX_TRANSPARENT (not click-through)");
    }
    if !f.layered {
        missing.push("no WS_EX_LAYERED (not click-through)");
    }
    if !f.toolwindow {
        missing.push("no WS_EX_TOOLWINDOW (listed in Alt+Tab)");
    }
    if f.appwindow {
        missing.push("WS_EX_APPWINDOW set (listed in Alt+Tab and the taskbar)");
    }
    if f.owned {
        missing.push("owned (the install smoke and these tests count unowned windows only)");
    }
    if !f.top_level {
        missing.push("not top-level");
    }
    missing
}

/// The screen rectangle of `raw` (physical pixels: tao makes the process
/// per-monitor DPI aware).
fn window_rect(raw: isize) -> Option<RECT> {
    let mut rect = RECT::default();
    // SAFETY: `rect` is a writable local; a stale handle gives an error.
    unsafe { GetWindowRect(to_hwnd(raw), &mut rect) }
        .ok()
        .map(|()| rect)
}

fn centre(rect: &RECT) -> POINT {
    POINT {
        x: rect.left / 2 + rect.right / 2,
        y: rect.top / 2 + rect.bottom / 2,
    }
}

fn contains(rect: &RECT, p: POINT) -> bool {
    rect.left <= p.x && p.x < rect.right && rect.top <= p.y && p.y < rect.bottom
}

/// The top-level window a click at `p` would reach (`WindowFromPoint`, then its
/// `GA_ROOT`: the WebView2 child windows count as the overlay), with its class.
fn hit_root(p: POINT) -> (isize, String) {
    // SAFETY: any point; the result is a handle or null.
    let hit = unsafe { WindowFromPoint(p) };
    // SAFETY: GetAncestor only reads the window tree.
    let root = unsafe { GetAncestor(hit, GA_ROOT) };
    (root.0 as isize, class_of(root))
}

/// Moves `target` (without activating it or changing its z-order) so it covers
/// `over` with a margin: whatever is not the overlay at the overlay's area is then
/// the target.
fn cover(target: isize, over: &RECT) -> bool {
    const MARGIN: i32 = 40;
    // SAFETY: a live window of this process whose thread pumps; the flags keep the
    // activation and the z-order.
    unsafe {
        SetWindowPos(
            to_hwnd(target),
            None,
            over.left - MARGIN,
            over.top - MARGIN,
            over.right - over.left + 2 * MARGIN,
            over.bottom - over.top + 2 * MARGIN,
            SWP_NOACTIVATE | SWP_NOZORDER,
        )
    }
    .is_ok()
}

/// The screen pixel at `p` as `(r, g, b)`; `None` when the screen cannot be read
/// (`CLR_INVALID`).
fn screen_pixel(p: POINT) -> Option<(u8, u8, u8)> {
    // SAFETY: the screen DC, released below on this thread.
    let dc = unsafe { GetDC(None) };
    // SAFETY: a DC from GetDC; any point (outside gives CLR_INVALID).
    let colour = unsafe { GetPixel(dc, p.x, p.y) };
    // SAFETY: the DC GetDC returned on this thread.
    unsafe { ReleaseDC(None, dc) };
    (colour.0 != CLR_INVALID).then(|| rgb(colour.0))
}

/// A COLORREF (`0x00BBGGRR`) as `(r, g, b)`.
fn rgb(colorref: u32) -> (u8, u8, u8) {
    let [r, g, b, _] = colorref.to_le_bytes();
    (r, g, b)
}

/// True when every channel of `a` is within `tolerance` of `b`'s.
fn near(a: (u8, u8, u8), b: (u8, u8, u8), tolerance: u8) -> bool {
    a.0.abs_diff(b.0) <= tolerance
        && a.1.abs_diff(b.1) <= tolerance
        && a.2.abs_diff(b.2) <= tolerance
}

/// The colour an EDIT window paints its empty client area with (the target).
fn window_colour() -> (u8, u8, u8) {
    // SAFETY: a valid system colour index.
    rgb(unsafe { GetSysColor(COLOR_WINDOW) })
}

/// Every visible, unowned, top-level window of this process but `except` and the
/// framework helper windows, decided by the install smoke's own rule
/// (`helper_windows::is_helper`: a visible-helper class of
/// `scripts/ci/helper-windows.txt` AND the four helper ex-style bits, T-065); tool
/// windows count.
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
        .filter(|(raw, class)| !helper_windows::is_helper(class, ex_style(*raw)))
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
        // T-067: every shown state has the whole shape (click-through, out of
        // Alt+Tab); `stable` below then holds it for SETTLE.
        assert_eq!(
            shape_errors(&step.facts),
            Vec::<&str>::new(),
            "{}: the shown overlay lacks its shape: {step:?}",
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
    // T-067: a rebuild gets the same shape as the first build.
    assert_eq!(
        shape_errors(&report.label_facts),
        Vec::<&str>::new(),
        "the rebuilt overlay lacks its shape: {:?}",
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
struct GoneReport {
    first_shown: bool,
    /// The window destroyed from outside the overlay thread was gone and its label
    /// freed.
    outside_gone: bool,
    /// After the newer shown state: exactly the label's window, visible, for SETTLE.
    one_after_newer: bool,
    shown_after_newer: Vec<(isize, String)>,
    /// T-067: the facts of the window rebuilt after the outside destroy.
    rebuilt_facts: Facts,
    /// Hidden then destroys that window (the reducer owns it; not stuck Destroying).
    hidden_gone: bool,
    /// And Recording after it shows one again.
    one_after_hidden_and_recording: bool,
    new_warnings: Vec<String>,
    log: Vec<String>,
}

#[test]
fn an_overlay_destroyed_from_outside_is_rebuilt_by_a_newer_shown_state() {
    // T-057 review 1 findings 1 and 2 (FR-04): the overlay can go without a Hidden
    // (Alt+Tab then Alt+F4; here `destroy()` from the driver thread). Its Destroyed
    // is one the reducer did not ask for. Then a newer shown state is published at
    // once (it may reach the overlay thread in the same wake as that Destroyed):
    // exactly one visible overlay window follows and stays; Hidden then destroys it
    // and Recording shows one again; no warning is logged. Bite: the shell keeping
    // the stale handle / the reducer staying Live (the newer state an Emit to a gone
    // window: no overlay; the next Hidden a Destroy that waits forever), or the
    // newest state handed to the reducer before the Destroyed (no overlay until the
    // next state).
    let _serial = serial();
    let _watchdog = watchdog("an_overlay_destroyed_from_outside_is_rebuilt_by_a_newer_shown_state");
    let dir = TempDir::new();
    let (app, outcome) = wry_app(dir.path());
    let logs_of = dir.path().to_path_buf();

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let indicator = ShellIndicator::new(&handle);
        let mut report = GoneReport::default();
        let one_visible = |handle: &AppHandle<Wry>| {
            let shown = shown_windows(&[]);
            shown.len() == 1 && overlay_hwnd(handle) == Some(shown[0].0)
        };
        indicator.set_overlay(&OverlayState::Recording);
        report.first_shown = poll(BUDGET, || overlay_hwnd(&handle).is_some_and(is_visible));
        if !report.first_shown {
            report.log = logged(&logs_of);
            return report;
        }
        let warnings_before = warnings(&logged(&logs_of)).len();
        let old = overlay_hwnd(&handle).unwrap_or(0);
        if let Some(window) = handle.get_webview_window(LABEL) {
            let _ = window.destroy();
        }
        report.outside_gone = poll(BUDGET, || {
            !is_window(old) && handle.get_webview_window(LABEL).is_none()
        });
        // At once: the overlay thread may see the Destroyed and this state together.
        indicator.set_overlay(&OverlayState::Processing);
        report.one_after_newer =
            poll(BUDGET, || one_visible(&handle)) && holds_for(SETTLE, || one_visible(&handle));
        report.shown_after_newer = shown_windows(&[]);
        if report.one_after_newer {
            let rebuilt = overlay_hwnd(&handle).unwrap_or(0);
            report.rebuilt_facts = facts(rebuilt);
            indicator.set_overlay(&OverlayState::Hidden);
            report.hidden_gone = poll(BUDGET, || {
                !is_window(rebuilt) && handle.get_webview_window(LABEL).is_none()
            });
            indicator.set_overlay(&OverlayState::Recording);
            report.one_after_hidden_and_recording =
                poll(BUDGET, || one_visible(&handle)) && holds_for(SETTLE, || one_visible(&handle));
        }
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
        "Recording showed no visible `{LABEL}` window within {BUDGET:?}; the app's log: \
         {:#?}",
        report.log
    );
    assert!(
        report.outside_gone,
        "premise: `destroy()` from outside did not remove the overlay within {BUDGET:?}: \
         {report:#?}"
    );
    assert!(
        report.one_after_newer,
        "after the overlay was destroyed from outside, a newer Processing left the shown \
         windows {:?} (want exactly the `{LABEL}` window, for {SETTLE:?}); the app's log: \
         {:#?}",
        report.shown_after_newer, report.log
    );
    // T-067: the rebuild after an outside destroy gets the same shape.
    assert_eq!(
        shape_errors(&report.rebuilt_facts),
        Vec::<&str>::new(),
        "the overlay rebuilt after an outside destroy lacks its shape: {:?}",
        report.rebuilt_facts
    );
    assert!(
        report.hidden_gone,
        "Hidden did not destroy the rebuilt overlay within {BUDGET:?}: {report:#?}"
    );
    assert!(
        report.one_after_hidden_and_recording,
        "Recording after Hidden showed no single overlay (the lifecycle is stuck): \
         {report:#?}"
    );
    assert!(
        report.new_warnings.is_empty(),
        "the app logged warnings after the overlay was destroyed from outside: {:#?}",
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

// ---- T-067: click-through, out of Alt+Tab, shown without activation ------------------

#[derive(Debug, Default)]
struct ClickThroughReport {
    target: isize,
    shown_within_budget: bool,
    facts: Facts,
    /// Every sample of the overlay's facts during `SETTLE` matched the first.
    stable: bool,
    overlay_rect: Option<RECT>,
    centre: POINT,
    /// The target was moved under the overlay and its rectangle holds the centre.
    covered: bool,
    /// The top-level window a click at the overlay's centre reaches, right after
    /// the target covered it, with its class.
    hit: (isize, String),
    /// The click reached the target at every sample during `SETTLE`.
    hit_stable: bool,
    foreground_samples: u64,
    foreground_away: Vec<(isize, String)>,
    focus_losses: Vec<(u32, usize)>,
    hidden_gone: bool,
    log: Vec<String>,
}

#[test]
fn the_overlay_is_click_through_out_of_alt_tab_and_shown_without_activation() {
    // T-067 Acceptance 1, 2, 3 and 6 (decisions #86): with a target window in front,
    // Recording shows the overlay with WS_EX_TRANSPARENT, WS_EX_LAYERED and
    // WS_EX_TOOLWINDOW set, WS_EX_APPWINDOW clear (NOACTIVATE and TOPMOST kept),
    // unowned and top-level (the Alt+Tab check by style: the switcher's list has no
    // API); the target, moved under the overlay without activation, is what a click
    // at the overlay's centre reaches (WindowFromPoint -> GA_ROOT), at every sample
    // for SETTLE; the target stays the foreground window at every 5 ms sample and
    // gets no focus/activation-loss message; the facts hold for SETTLE (no late
    // rewrite). Bite: the T-057 build (visible through tao, no raw bits: red on every
    // new bit, APPWINDOW and the hit test); APPWINDOW left set; LAYERED without
    // TRANSPARENT (still hit); the bits written but the window shown with tauri
    // `show()` / `set_ignore_cursor_events` (tao rewrites GWL_EXSTYLE from its flags,
    // dropping TOOLWINDOW, and SW_SHOW activates); an owner window (Alt+Tab by
    // owner, but the window stops counting as unowned).
    let _serial = serial();
    let _watchdog =
        watchdog("the_overlay_is_click_through_out_of_alt_tab_and_shown_without_activation");
    let dir = TempDir::new();
    let (app, outcome) = wry_app_drawn(dir.path());
    let logs_of = dir.path().to_path_buf();

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let target = TestWindow::open(Shape::TopLevel);
        target.front();
        let target_raw = target.hwnd().0 as isize;
        let losses_before = target.focus_losses().len();
        let sampler = ForegroundSampler::start(target_raw);
        let indicator = ShellIndicator::new(&handle);
        let mut report = ClickThroughReport {
            target: target_raw,
            ..ClickThroughReport::default()
        };
        indicator.set_overlay(&OverlayState::Recording);
        report.shown_within_budget = poll(BUDGET, || {
            overlay_hwnd(&handle).is_some_and(|raw| facts(raw).visible)
        });
        if let Some(raw) = overlay_hwnd(&handle).filter(|_| report.shown_within_budget) {
            report.facts = facts(raw);
            report.overlay_rect = window_rect(raw);
            if let Some(rect) = report.overlay_rect {
                report.centre = centre(&rect);
                report.covered = cover(target_raw, &rect)
                    && window_rect(target_raw).is_some_and(|t| contains(&t, report.centre));
                report.hit = hit_root(report.centre);
                let at = report.centre;
                report.hit_stable = holds_for(SETTLE, || hit_root(at).0 == target_raw);
            }
            let first = report.facts;
            report.stable = holds_for(SETTLE, || facts(raw) == first);
        }
        let (samples, away) = sampler.stop();
        report.foreground_samples = samples;
        report.foreground_away = away;
        report.focus_losses = target
            .focus_losses()
            .get(losses_before..)
            .map(<[_]>::to_vec)
            .unwrap_or_default();
        let last = report.facts.hwnd;
        indicator.set_overlay(&OverlayState::Hidden);
        report.hidden_gone = last != 0 && poll(BUDGET, || !is_window(last));
        report.log = logged(&logs_of);
        report
    });

    assert!(
        report.shown_within_budget,
        "Recording showed no visible `{LABEL}` window within {BUDGET:?}; the app's log: \
         {:#?}",
        report.log
    );
    assert_eq!(
        shape_errors(&report.facts),
        Vec::<&str>::new(),
        "the shown overlay is not click-through / out of Alt+Tab: {:?}",
        report.facts
    );
    assert!(
        report.covered,
        "premise: the target could not be moved under the overlay (overlay {:?}, centre \
         {:?}): {report:#?}",
        report.overlay_rect, report.centre
    );
    assert_eq!(
        report.hit.0, report.target,
        "a click at the overlay's centre {:?} reaches {:?}, not the target {:#x} under it \
         (the overlay takes the click)",
        report.centre, report.hit, report.target
    );
    assert!(
        report.hit_stable,
        "within {SETTLE:?} a click at the overlay's centre stopped reaching the target: \
         {report:#?}"
    );
    assert!(
        report.stable,
        "within {SETTLE:?} after it showed the overlay changed (hidden, rebuilt or its \
         styles rewritten): {report:#?}"
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
        report.hidden_gone,
        "Hidden did not destroy the click-through overlay within {BUDGET:?}: {report:#?}"
    );
    assert_eq!(code, 0, "run_return code after end_loop(0)");
}

#[derive(Debug, Default)]
struct DrawnReport {
    shown_within_budget: bool,
    facts: Facts,
    centre: POINT,
    covered: bool,
    /// The screen could be read (`GetPixel` gave no `CLR_INVALID`).
    readable: bool,
    /// The page's colour was on screen at the overlay's centre within `BUDGET`.
    drawn: bool,
    last_pixel: Option<(u8, u8, u8)>,
    target_colour: (u8, u8, u8),
    log: Vec<String>,
}

#[test]
fn the_click_through_overlay_still_paints_its_page() {
    // T-067 guard (analysis hypothesis 5): a WS_EX_LAYERED window may not be drawn at
    // all without layered attributes. With the target window moved under the overlay,
    // the screen pixel at the overlay's centre becomes the test page's colour
    // (PAGE_RGB, opaque) within BUDGET, not the target's COLOR_WINDOW. Green with the
    // T-057 build (not layered); must stay green with the click-through bits. Bite:
    // LAYERED set and never composed (no SetLayeredWindowAttributes where the system
    // needs it): the target shows through, the pixel is COLOR_WINDOW.
    let _serial = serial();
    let _watchdog = watchdog("the_click_through_overlay_still_paints_its_page");
    let dir = TempDir::new();
    let (app, outcome) = wry_app_drawn(dir.path());
    let logs_of = dir.path().to_path_buf();

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let target = TestWindow::open(Shape::TopLevel);
        target.front();
        let target_raw = target.hwnd().0 as isize;
        let indicator = ShellIndicator::new(&handle);
        let mut report = DrawnReport {
            target_colour: window_colour(),
            ..DrawnReport::default()
        };
        indicator.set_overlay(&OverlayState::Recording);
        report.shown_within_budget = poll(BUDGET, || {
            overlay_hwnd(&handle).is_some_and(|raw| facts(raw).visible)
        });
        if let Some(raw) = overlay_hwnd(&handle).filter(|_| report.shown_within_budget) {
            report.facts = facts(raw);
            if let Some(rect) = window_rect(raw) {
                report.centre = centre(&rect);
                report.covered = cover(target_raw, &rect)
                    && window_rect(target_raw).is_some_and(|t| contains(&t, report.centre));
                let at = report.centre;
                report.readable = true;
                report.drawn = poll(BUDGET, || {
                    let pixel = screen_pixel(at);
                    report.readable &= pixel.is_some();
                    report.last_pixel = pixel;
                    pixel.is_some_and(|p| near(p, PAGE_RGB, 24))
                });
            }
        }
        let last = report.facts.hwnd;
        indicator.set_overlay(&OverlayState::Hidden);
        let _ = last != 0 && poll(BUDGET, || !is_window(last));
        report.log = logged(&logs_of);
        report
    });

    assert!(
        report.shown_within_budget,
        "Recording showed no visible `{LABEL}` window within {BUDGET:?}; the app's log: \
         {:#?}",
        report.log
    );
    assert!(
        report.covered,
        "premise: the target could not be moved under the overlay: {report:#?}"
    );
    assert!(
        report.readable,
        "premise: GetPixel on the screen DC returned CLR_INVALID (no readable interactive \
         desktop; docs/decisions/windows-ci-runner.md `session`/`desktop` facts): \
         {report:#?}"
    );
    assert!(
        report.drawn,
        "the overlay's page (rgb{PAGE_RGB:?}) never showed at its centre {:?} within \
         {BUDGET:?}; last pixel {:?} (the target's COLOR_WINDOW is rgb{:?}): a layered \
         window that is not composed. Facts: {:?}",
        report.centre, report.last_pixel, report.target_colour, report.facts
    );
    assert_eq!(code, 0, "run_return code after end_loop(0)");
}

/// The words of a log line after its timestamp and level.
fn message_words(line: &str) -> Vec<&str> {
    line.split_whitespace().skip(2).collect()
}

#[test]
fn show_click_through_on_a_destroyed_window_returns_false_and_writes_one_overlay_failed_line() {
    // T-067 Acceptance 4 (failure branch; FR-20, #45): the style write of
    // `overlay::show_click_through` cannot be applied to a window that is gone
    // (GetWindowLongPtrW / SetWindowLongPtrW fail with ERROR_INVALID_WINDOW_HANDLE,
    // 1400). It returns false, writes exactly one line `warning kind=overlay_failed
    // os_code=1400` (no handle, no OS text) and nothing else, shows no window, and
    // the target in front keeps the foreground and its focus. Bite: true returned on
    // the failure (the thread would think the bits hold); the error swallowed (no
    // line); one line per failed call (two or three lines); the code lost
    // (`os_code` missing: the ambiguous 0 of SetWindowLongPtrW not resolved with
    // SetLastError(0)/GetLastError) or a stale one; error text in the line.
    let _serial = serial();
    let dir = TempDir::new();
    let log = test_log(dir.path());

    let dead = {
        let gone = TestWindow::open(Shape::TopLevel);
        gone.hwnd()
    };
    assert!(
        !is_window(dead.0 as isize),
        "premise: the dropped test window {dead:?} still exists"
    );
    let target = TestWindow::open(Shape::TopLevel);
    target.front();
    let target_raw = target.hwnd().0 as isize;
    let losses_before = target.focus_losses().len();
    let shown_before = shown_windows(&[]);
    let lines_before = logged(dir.path()).len();
    let sampler = ForegroundSampler::start(target_raw);

    let applied = voicen_lib::overlay::show_click_through(dead, &log);

    let foreground_kept = holds_for(SETTLE, || foreground().0 as isize == target_raw);
    let (samples, away) = sampler.stop();
    let new_lines: Vec<String> = logged(dir.path())
        .get(lines_before..)
        .map(<[_]>::to_vec)
        .unwrap_or_default();
    let focus_losses: Vec<(u32, usize)> = target
        .focus_losses()
        .get(losses_before..)
        .map(<[_]>::to_vec)
        .unwrap_or_default();
    let shown_after = shown_windows(&[]);

    assert!(
        !applied,
        "show_click_through on a destroyed window reported the bits applied"
    );
    assert_eq!(
        new_lines.len(),
        1,
        "want exactly one log line for the failed style write: {new_lines:#?}"
    );
    assert_eq!(
        message_words(&new_lines[0]),
        vec!["warning", "kind=overlay_failed", "os_code=1400"],
        "{:?}",
        new_lines[0]
    );
    assert!(samples > 0, "premise: the foreground sampler ran");
    assert!(
        foreground_kept && away.is_empty(),
        "the target lost the foreground during the failed show; in front instead: {away:?} \
         (target {target_raw:#x})"
    );
    assert!(
        focus_losses.is_empty(),
        "the target got focus/activation-loss messages during the failed show: \
         {focus_losses:?}"
    );
    assert_eq!(
        shown_after, shown_before,
        "the failed show changed the process's shown windows"
    );
}
