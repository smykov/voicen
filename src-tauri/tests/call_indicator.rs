//! T-130 (spec 007 T032/T033, ADR-007 #235): the call indicator window on tauri's real Wry
//! runtime (WebView2), driven from a libtest thread. Windows CI only (decision #5). The spike
//! rows S1-S6 and S8 of the task's Investigation are tests here, for variant N (ADR-007
//! primary: `WS_EX_NOACTIVATE`, the click does not take the focus). If S3/S4/S5 cannot pass under
//! N, the developer switches to variant F (R12 as written: focusable, the shell returns the
//! foreground) and these tests are changed with the evidence recorded in the task (the owner
//! decides when F fails as well).
//!
//! The harness is `tests/overlay.rs`'s (T-052, T-057), repeated here because each test file is
//! its own crate: the loop runs with `Builder::any_thread` and `App::run_return`; a driver thread
//! starts on `Ready`, acts and reports; the test asserts after `run_return` returned; a watchdog
//! ends the exe with code 3 if a loop never returns; the tests are serialised. Every fact is a
//! raw Win32 value (F-003). A build or show is decided through the indicator thread's receipt
//! (`call_indicator::settled`, a hang bound only, T-173), the facts read after it.
//!
//! Assumed shell API (`src-tauri/src/call_indicator.rs`, `voicen_lib::call_indicator`; the
//! developer implements exactly this, see the task's `## Tests`):
//! - `LABEL = "call-indicator"`, `STATE_EVENT = "indicator://state"`, `TICK_EVENT =
//!   "indicator://tick"`;
//! - `IndicatorPart` (`Clone`, implements core's `CallIndicator`) and `part(&AppHandle) ->
//!   Option<IndicatorPart>`;
//! - `Settled { seq, window, failed }` and `settled(&AppHandle, hang) -> Option<Settled>`, the
//!   indicator thread's receipt with the overlay's meaning;
//! - `trait IndicatorControl { fn stop(&self); fn dismiss_warning(&self, id: u32); }` and
//!   `set_control(&AppHandle, Arc<dyn IndicatorControl>)` (the slot T-128 fills);
//! - `moved(&AppHandle, x: i32, y: i32)`, the function the command `call_indicator_moved` calls;
//! - the commands `call_indicator_stop`, `call_indicator_dismiss { id }`, `call_indicator_moved
//!   { x, y }`, `call_indicator_state`.
//!
//! The app identifier is a test one; each test has its own `TempDir`; no keys.
#![cfg(windows)]

mod helper_windows;
mod timing_support;
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

use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{get_ipc_response, mock_builder, mock_context, MockRuntime};
use tauri::utils::assets::{AssetKey, AssetsIter, CspHash};
use tauri::webview::InvokeRequest;
use tauri::{App, AppHandle, Assets, Context, Listener, Manager, RunEvent, Url, Wry};
use timing_support::{at_least, between, eventually, holds_for, measure, now};
use voicen_core::call::indicator::{IndicatorState, IndicatorTick};
use voicen_core::call::session::CallIndicator;
use voicen_core::diag::Log;
use voicen_core::i18n::UiLanguage;
use voicen_core::local_models::catalog::MODELS;
use voicen_core::local_models::service::LocalModels;
use voicen_core::local_models::store::ModelStore;
use voicen_core::secrets::{CredentialStore, FakeCredentialStore};
use voicen_core::settings::file::SETTINGS_FILE;
use voicen_core::settings::service::SettingsService;
use voicen_core::settings::{IndicatorPosition, LoadOutcome};
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_lib::call_indicator::{self, IndicatorControl, Settled};
use voicen_lib::settings_ipc::load_settings;
use win32_support::{class_of, foreground, send, Shape, TestWindow};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{GetDC, GetPixel, ReleaseDC, CLR_INVALID};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, MOUSE_EVENT_FLAGS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetCursorPos, GetSystemMetrics, GetWindow, GetWindowLongPtrW,
    GetWindowRect, GetWindowThreadProcessId, IsWindow, IsWindowVisible, WindowFromPoint, GA_ROOT,
    GWL_EXSTYLE, GWL_STYLE, GW_OWNER, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, WS_CAPTION, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};

/// The indicator window's label (spec 007 contracts/ipc.md).
const LABEL: &str = "call-indicator";

/// A test identifier: WebView2's profile goes under it.
const IDENTIFIER: &str = "dev.voicen.test.callindicator";

/// How long the driver waits for its report after the loop returned (Windows-sized, F-005).
const BUDGET: Duration = Duration::from_secs(20);

/// A hang bound, not a budget (T-173): how long the indicator's receipt may take; above a cold
/// WebView2 build, below `WATCHDOG`.
const INDICATOR_HANG: Duration = Duration::from_secs(120);

/// How long a state is watched after it showed, so a late activation, a late second window or a
/// late style rewrite is seen (a bounded negative).
const SETTLE: Duration = Duration::from_secs(2);

/// The exe ends with code 3 if one test's loop has not returned by then.
const WATCHDOG: Duration = Duration::from_secs(240);

/// The page's three zones (`IndicatorPage`): the left half is the "button" (turns
/// `CLICKED_RGB` on mousedown), the right half is the drag region. Far from every system colour.
const IDLE_RGB: (u8, u8, u8) = (32, 160, 64);
const CLICKED_RGB: (u8, u8, u8) = (32, 64, 160);
const DRAG_RGB: (u8, u8, u8) = (160, 32, 64);

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
                "call indicator watchdog: `{test}`: the event loop did not return within \
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

fn test_log(dir: &Path) -> Arc<Log> {
    voicen_lib::diag::start(dir.join("logs"), Box::new(|_| {}))
}

/// The lines the app logged under `dir` so far (none when nothing was written).
fn logged(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("logs").join("voicen.log"))
        .map(|t| t.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// An asset provider whose every `.html` key is the indicator test page: a left "button" zone
/// that turns [`CLICKED_RGB`] on mousedown and a right zone that is a tauri drag region. The
/// real page (T-131) needs the built UI, which the shell tests do not have.
struct IndicatorPage;

impl IndicatorPage {
    fn html() -> String {
        let (ir, ig, ib) = IDLE_RGB;
        let (cr, cg, cb) = CLICKED_RGB;
        let (dr, dg, db) = DRAG_RGB;
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"></head>\
             <body style=\"margin:0;width:100vw;height:100vh;display:flex;overflow:hidden\">\
             <div id=\"button\" style=\"width:50vw;height:100vh;background:rgb({ir},{ig},{ib})\">\
             </div>\
             <div id=\"drag\" data-tauri-drag-region \
              style=\"width:50vw;height:100vh;background:rgb({dr},{dg},{db})\"></div>\
             <script>document.getElementById('button').addEventListener('mousedown',\
              function(){{this.style.background='rgb({cr},{cg},{cb})'}});</script>\
             </body></html>"
        )
    }
}

impl Assets<Wry> for IndicatorPage {
    fn get(&self, key: &AssetKey) -> Option<Cow<'_, [u8]>> {
        key.as_ref()
            .ends_with(".html")
            .then(|| Cow::Owned(IndicatorPage::html().into_bytes()))
    }

    fn iter(&self) -> Box<AssetsIter<'_>> {
        Box::new(std::iter::empty())
    }

    fn csp_hashes(&self, _html_path: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        Box::new(std::iter::empty())
    }
}

/// The app as `run()` builds it (`build_app`), on the real Wry runtime, on a loaded
/// settings.json (so `Ready` opens no settings window), whose pages are [`IndicatorPage`].
/// `before_build` may edit the settings service first (a saved position).
fn wry_app(dir: &Path, before_build: impl FnOnce(&SettingsService)) -> (App<Wry>, LoadOutcome) {
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
    before_build(&service);
    let mut context: Context<Wry> = mock_context(IndicatorPage);
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

fn panic_text(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "a panic without a message".to_string())
}

/// Runs the loop with `on_run_event` (as `run()` does) and starts `driver` on its own thread
/// once `Ready` was handled. The driver's panic is caught, the loop is ended, and the test
/// re-raises it. Returns `run_return`'s code and the driver's report.
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

fn end_loop(handle: &AppHandle<Wry>, code: i32) {
    let inner = handle.clone();
    let _ = handle.run_on_main_thread(move || inner.exit(code));
}

// ---- driving the indicator the way T-128 will ------------------------------------------

fn recording_state() -> IndicatorState {
    IndicatorState::recording(UiLanguage::En, true, Vec::new())
}

fn publish(handle: &AppHandle<Wry>, state: &IndicatorState) {
    call_indicator::part(handle)
        .expect("assemble wired the call indicator (call_indicator::part)")
        .set_state(state);
}

/// What the driver saw of one published state: the indicator thread's receipt and, read once
/// after it, whether the label's window is visible (T-173).
#[derive(Debug, Default, Clone, Copy)]
struct Shown {
    receipt: Option<Settled>,
    visible: bool,
}

impl Shown {
    fn ok(&self) -> bool {
        self.receipt.is_some_and(|r| !r.failed && r.window) && self.visible
    }
}

fn shown(handle: &AppHandle<Wry>) -> Shown {
    let receipt = call_indicator::settled(handle, INDICATOR_HANG);
    Shown {
        receipt,
        visible: receipt.is_some() && indicator_hwnd(handle).is_some_and(is_visible),
    }
}

fn to_hwnd(raw: isize) -> HWND {
    HWND(raw as *mut std::ffi::c_void)
}

fn indicator_hwnd(handle: &AppHandle<Wry>) -> Option<isize> {
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
    unsafe { GetWindowLongPtrW(to_hwnd(raw), GWL_EXSTYLE) as u32 }
}

fn style(raw: isize) -> u32 {
    // SAFETY: GetWindowLongPtrW reads the window's style; no message is sent.
    unsafe { GetWindowLongPtrW(to_hwnd(raw), GWL_STYLE) as u32 }
}

/// The raw facts of the indicator window at one moment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Facts {
    hwnd: isize,
    visible: bool,
    noactivate: bool,
    topmost: bool,
    toolwindow: bool,
    appwindow: bool,
    /// Click-through (`WS_EX_TRANSPARENT` with `WS_EX_LAYERED`): the indicator takes clicks.
    click_through: bool,
    /// A caption (title bar): the indicator is undecorated.
    caption: bool,
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
        toolwindow: ex & WS_EX_TOOLWINDOW.0 != 0,
        appwindow: ex & WS_EX_APPWINDOW.0 != 0,
        click_through: ex & WS_EX_TRANSPARENT.0 != 0 && ex & WS_EX_LAYERED.0 != 0,
        caption: style(raw) & WS_CAPTION.0 != 0,
        // SAFETY: both calls only read the window's relations; no message is sent.
        owned: unsafe { GetWindow(hwnd, GW_OWNER) }.is_ok_and(|o| !o.is_invalid()),
        top_level: unsafe { GetAncestor(hwnd, GA_ROOT) } == hwnd,
    }
}

/// What a shown indicator lacks of its shape (S2, variant N): non-activating, topmost, a tool
/// window without APPWINDOW (no taskbar entry, out of Alt+Tab), undecorated, takes clicks,
/// unowned, top-level.
fn shape_errors(f: &Facts) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if !f.visible {
        missing.push("not visible");
    }
    if !f.noactivate {
        missing.push("no WS_EX_NOACTIVATE (a click would activate it)");
    }
    if !f.topmost {
        missing.push("no WS_EX_TOPMOST");
    }
    if !f.toolwindow {
        missing.push("no WS_EX_TOOLWINDOW (listed in Alt+Tab)");
    }
    if f.appwindow {
        missing.push("WS_EX_APPWINDOW set (taskbar entry)");
    }
    if f.click_through {
        missing.push("click-through (WS_EX_TRANSPARENT | WS_EX_LAYERED): it must take clicks");
    }
    if f.caption {
        missing.push("WS_CAPTION (decorated)");
    }
    if f.owned {
        missing.push("owned (the smoke counts unowned windows only)");
    }
    if !f.top_level {
        missing.push("not top-level");
    }
    missing
}

fn window_rect(raw: isize) -> Option<RECT> {
    let mut rect = RECT::default();
    // SAFETY: `rect` is a writable local; a stale handle gives an error.
    unsafe { GetWindowRect(to_hwnd(raw), &mut rect) }
        .ok()
        .map(|()| rect)
}

/// The virtual screen (the bounding box of all monitors): `(left, top, right, bottom)`.
fn virtual_screen() -> (i32, i32, i32, i32) {
    // SAFETY: plain metric queries.
    let (x, y, w, h) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    (x, y, x + w, y + h)
}

/// A point a quarter of the way across the window's width (`quarter` 1: the "button" zone) or
/// three quarters (`quarter` 3: the drag region), at mid height.
fn zone_point(rect: &RECT, quarter: i32) -> POINT {
    POINT {
        x: rect.left + (rect.right - rect.left) * quarter / 4,
        y: rect.top + (rect.bottom - rect.top) / 2,
    }
}

fn hit_root(p: POINT) -> isize {
    // SAFETY: any point; the result is a handle or null.
    let hit = unsafe { WindowFromPoint(p) };
    // SAFETY: GetAncestor only reads the window tree.
    unsafe { GetAncestor(hit, GA_ROOT) }.0 as isize
}

fn screen_pixel(p: POINT) -> Option<(u8, u8, u8)> {
    // SAFETY: the screen DC, released below on this thread.
    let dc = unsafe { GetDC(None) };
    // SAFETY: a DC from GetDC; any point (outside gives CLR_INVALID).
    let colour = unsafe { GetPixel(dc, p.x, p.y) };
    // SAFETY: the DC GetDC returned on this thread.
    unsafe { ReleaseDC(None, dc) };
    (colour.0 != CLR_INVALID).then(|| {
        let [r, g, b, _] = colour.0.to_le_bytes();
        (r, g, b)
    })
}

fn near(a: (u8, u8, u8), b: (u8, u8, u8)) -> bool {
    a.0.abs_diff(b.0) <= 6 && a.1.abs_diff(b.1) <= 6 && a.2.abs_diff(b.2) <= 6
}

/// Every top-level window of this process, visible or not, with its class (S6 census).
fn census() -> Vec<(isize, String)> {
    unsafe extern "system" fn collect(hwnd: HWND, found: LPARAM) -> BOOL {
        let mut pid = 0u32;
        // SAFETY: `pid` is a writable local; the call only reads the window's owner.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == std::process::id() {
            // SAFETY: `found` is the `Vec` that `census` passes, alive and not otherwise
            // touched for the whole `EnumWindows` call.
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
        .map(|raw| (raw, class_of(to_hwnd(raw))))
        .collect()
}

/// The sample count and every window other than the target seen in front (with its class).
type Sampled = (u64, Vec<(isize, String)>);

/// Samples `GetForegroundWindow` every 5 ms on its own thread and keeps every sample that was
/// not `target` (with its class), until stopped.
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
                let front = foreground();
                samples += 1;
                if front.0 as isize != target && !away.iter().any(|(h, _)| *h == front.0 as isize) {
                    away.push((front.0 as isize, class_of(front)));
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

    fn stop(mut self) -> Sampled {
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

// ---- injected mouse ---------------------------------------------------------------------

fn mouse_input(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Moves the cursor to `p` (SendInput, absolute over the virtual desktop) and waits until it is
/// there (within 2 px: the normalised coordinates round).
#[track_caller]
fn move_cursor(p: POINT) {
    let (left, top, right, bottom) = virtual_screen();
    let norm =
        |v: i32, lo: i32, hi: i32| ((v - lo) as i64 * 65535 / ((hi - lo - 1).max(1) as i64)) as i32;
    let sent = send(&[mouse_input(
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        norm(p.x, left, right),
        norm(p.y, top, bottom),
    )]);
    assert_eq!(sent, 1, "premise: SendInput inserted the mouse move");
    let there = eventually(|| {
        let mut at = POINT::default();
        // SAFETY: `at` is a writable local.
        unsafe { GetCursorPos(&mut at) }.is_ok()
            && (at.x - p.x).abs() <= 2
            && (at.y - p.y).abs() <= 2
    });
    assert!(
        there,
        "premise: the cursor did not reach ({}, {})",
        p.x, p.y
    );
}

/// The left button held down by the test; released on drop so a failed test never leaves it.
struct LeftButton(bool);

impl LeftButton {
    fn press() -> LeftButton {
        send(&[mouse_input(MOUSEEVENTF_LEFTDOWN, 0, 0)]);
        LeftButton(true)
    }

    fn release(&mut self) {
        if self.0 {
            send(&[mouse_input(MOUSEEVENTF_LEFTUP, 0, 0)]);
            self.0 = false;
        }
    }
}

impl Drop for LeftButton {
    fn drop(&mut self) {
        self.release();
    }
}

// ---- S1: no window before the first recording state -------------------------------------

#[derive(Debug, Default)]
struct StartReport {
    label_at_ready: bool,
    census_at_ready: Vec<(isize, String)>,
    idle: Option<Settled>,
    label_after_idle: bool,
    census_after_idle: Vec<(isize, String)>,
    recording: Shown,
    log: Vec<String>,
}

#[test]
fn no_indicator_window_exists_at_app_start_or_before_the_first_recording_state() {
    // Spike S1 / Acceptance 1 ("created hidden at the first Record, never at app start"):
    // after `build_app` and `Ready` there is no window labelled `call-indicator`; an idle state
    // (recording=false) builds none either (the process' top-level windows are unchanged);
    // the first recording=true state builds it. Bite: a window built in `start`/`wire`, a
    // build on any state, an idle state that builds.
    let _serial = serial();
    let _watchdog =
        watchdog("no_indicator_window_exists_at_app_start_or_before_the_first_recording_state");
    let dir = TempDir::new();
    let (app, outcome) = wry_app(dir.path(), |_| {});
    let logs_of = dir.path().to_path_buf();

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let mut report = StartReport {
            label_at_ready: handle.get_webview_window(LABEL).is_some(),
            census_at_ready: census(),
            ..StartReport::default()
        };
        publish(&handle, &IndicatorState::idle());
        report.idle = call_indicator::settled(&handle, INDICATOR_HANG);
        report.label_after_idle = handle.get_webview_window(LABEL).is_some();
        report.census_after_idle = census();
        publish(&handle, &recording_state());
        report.recording = shown(&handle);
        report.log = logged(&logs_of);
        report
    });

    assert!(
        !report.label_at_ready,
        "a `{LABEL}` window exists at app start: {report:#?}"
    );
    assert!(
        report.idle.is_some_and(|r| !r.failed && !r.window),
        "an idle state must be settled without a window: {report:#?}"
    );
    assert!(
        !report.label_after_idle,
        "an idle state built the window: {report:#?}"
    );
    assert_eq!(
        report.census_after_idle, report.census_at_ready,
        "the process' top-level windows changed before any recording state"
    );
    assert!(
        report.recording.ok(),
        "the first recording state showed no visible `{LABEL}` window: {report:#?}"
    );
    assert_eq!(code, 0, "run_return code after end_loop(0)");
}

// ---- T033: destroyed at stop, rebuilt at the next start ---------------------------------

#[derive(Debug, Default)]
struct CycleReport {
    first: Shown,
    first_hwnd: isize,
    gone_at_stop: bool,
    label_freed: bool,
    second: Shown,
    second_hwnd: isize,
    stayed_one: bool,
    new_warnings: Vec<String>,
    log: Vec<String>,
}

#[test]
fn the_indicator_is_destroyed_when_the_call_stops_and_rebuilt_at_the_next_start() {
    // Acceptance 5 / NFR-03 (webviews are created on demand and destroyed when closed): the
    // window exists exactly while the newest state has recording=true. Stop (an idle state)
    // destroys it (handle gone, label freed); the next recording state builds a new one;
    // exactly one indicator window afterwards; no warning logged. Bite: hide instead of
    // destroy, a window kept for the whole run, a rebuild before `Destroyed` (label exists).
    let _serial = serial();
    let _watchdog =
        watchdog("the_indicator_is_destroyed_when_the_call_stops_and_rebuilt_at_the_next_start");
    let dir = TempDir::new();
    let (app, outcome) = wry_app(dir.path(), |_| {});
    let logs_of = dir.path().to_path_buf();

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let mut report = CycleReport::default();
        publish(&handle, &recording_state());
        report.first = shown(&handle);
        report.first_hwnd = indicator_hwnd(&handle).unwrap_or(0);
        if !report.first.ok() {
            report.log = logged(&logs_of);
            return report;
        }
        let warnings_before = logged(&logs_of)
            .iter()
            .filter(|l| l.contains("warning"))
            .count();
        publish(&handle, &IndicatorState::idle());
        report.gone_at_stop = eventually(|| !is_window(report.first_hwnd));
        report.label_freed = eventually(|| handle.get_webview_window(LABEL).is_none());
        // The next call, at once after the label was freed.
        publish(&handle, &recording_state());
        report.second = shown(&handle);
        report.second_hwnd = indicator_hwnd(&handle).unwrap_or(0);
        let one = |handle: &AppHandle<Wry>| {
            let mine = indicator_hwnd(handle).unwrap_or(0);
            census()
                .iter()
                .filter(|(raw, _)| *raw == mine && is_visible(*raw))
                .count()
                == 1
        };
        report.stayed_one = report.second.ok() && holds_for(SETTLE, || one(&handle));
        let lines = logged(&logs_of);
        report.new_warnings = lines
            .iter()
            .filter(|l| l.contains("warning"))
            .skip(warnings_before)
            .cloned()
            .collect();
        report.log = lines;
        report
    });

    assert!(
        report.first.ok(),
        "the first call showed no indicator: {report:#?}"
    );
    assert!(
        report.gone_at_stop && report.label_freed,
        "stop did not destroy the indicator (NFR-03): {report:#?}"
    );
    assert!(
        report.second.ok() && report.stayed_one,
        "the next call's indicator: {report:#?}"
    );
    assert_ne!(
        report.first_hwnd, report.second_hwnd,
        "the window was kept, not rebuilt"
    );
    assert!(
        report.new_warnings.is_empty(),
        "the app logged warnings across stop and restart: {:#?}",
        report.new_warnings
    );
    assert_eq!(code, 0, "run_return code after end_loop(0)");
}

// ---- S2: the shape ----------------------------------------------------------------------

#[derive(Debug, Default)]
struct ShapeReport {
    shown: Shown,
    facts: Facts,
    stable: bool,
    is_helper: bool,
    /// S8: logged, not asserted.
    build_to_shown: timing_support::Took,
}

#[test]
fn the_indicator_is_topmost_noactivate_toolwindow_undecorated_and_unowned() {
    // Spike S2 / Acceptance 1 ("undecorated, topmost, no taskbar entry"; click-taking per
    // #235): raw GWL_EXSTYLE / GWL_STYLE after the receipt: visible, WS_EX_TOPMOST,
    // WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, no WS_EX_APPWINDOW, no WS_CAPTION, not
    // click-through, unowned, top-level; the same facts SETTLE later (no late style rewrite or
    // hide); the install smoke's helper predicate does not exclude it (it is a product
    // window). S8: the build-to-shown time is logged. Bite: a missing `focusable(false)`, a
    // tao setter after the raw show (re-diffs and rewrites the flags), a window left
    // decorated, WS_EX_TRANSPARENT copied from the overlay.
    let _serial = serial();
    let _watchdog =
        watchdog("the_indicator_is_topmost_noactivate_toolwindow_undecorated_and_unowned");
    let dir = TempDir::new();
    let (app, outcome) = wry_app(dir.path(), |_| {});

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let mut report = ShapeReport::default();
        let (shown_now, took) = measure(|| {
            publish(&handle, &recording_state());
            shown(&handle)
        });
        report.shown = shown_now;
        report.build_to_shown = took;
        eprintln!("call indicator: build to shown (S8, informational): {took:?}");
        if let Some(raw) = indicator_hwnd(&handle) {
            report.facts = facts(raw);
            let first = report.facts;
            report.stable = holds_for(SETTLE, || facts(raw) == first);
            report.is_helper = helper_windows::is_helper(&class_of(to_hwnd(raw)), ex_style(raw));
        }
        report
    });

    assert!(
        report.shown.ok(),
        "no visible `{LABEL}` window after the receipt: {report:#?}"
    );
    assert_eq!(
        shape_errors(&report.facts),
        Vec::<&str>::new(),
        "the shown indicator lacks its shape: {report:#?}"
    );
    assert!(
        report.stable,
        "within {SETTLE:?} after it showed the indicator changed: {report:#?}"
    );
    assert!(
        !report.is_helper,
        "the indicator matches the smoke's helper-window predicate (scripts/ci/helper-windows.txt \
         must not list a product window): {report:#?}"
    );
    assert_eq!(code, 0, "run_return code after end_loop(0)");
}

// ---- S3: showing keeps the foreground ---------------------------------------------------

#[derive(Debug, Default)]
struct ForegroundReport {
    target: isize,
    shown: Shown,
    samples: u64,
    away: Vec<(isize, String)>,
    focus_losses: Vec<(u32, usize)>,
}

#[test]
fn showing_the_indicator_keeps_the_foreground_window() {
    // Spike S3 / Acceptance 2 (first half): with a target window in front (asserted loudly by
    // `front`), the recording state shows the indicator and the target stays the foreground
    // window at every 5 ms sample through the build, the show and SETTLE after it, and gets no
    // WM_KILLFOCUS / WM_ACTIVATE / inactive WM_NCACTIVATE. Bite: `show()`/`set_focus()` or a
    // tao setter (SW_SHOW activates), a window built without `focusable(false)`, a build with
    // `focused(true)`, a raw show with SW_SHOW instead of SW_SHOWNOACTIVATE.
    let _serial = serial();
    let _watchdog = watchdog("showing_the_indicator_keeps_the_foreground_window");
    let dir = TempDir::new();
    let (app, outcome) = wry_app(dir.path(), |_| {});

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let target = TestWindow::open(Shape::TopLevel);
        target.front();
        let target_raw = target.hwnd().0 as isize;
        let losses_before = target.focus_losses().len();
        let sampler = ForegroundSampler::start(target_raw);
        publish(&handle, &recording_state());
        let shown_now = shown(&handle);
        let _ = holds_for(SETTLE, || foreground().0 as isize == target_raw);
        let (samples, away) = sampler.stop();
        ForegroundReport {
            target: target_raw,
            shown: shown_now,
            samples,
            away,
            focus_losses: target
                .focus_losses()
                .get(losses_before..)
                .map(<[_]>::to_vec)
                .unwrap_or_default(),
        }
    });

    assert!(
        report.shown.ok(),
        "no visible `{LABEL}` window after the receipt: {report:#?}"
    );
    assert!(
        report.samples > 0,
        "premise: the foreground sampler ran: {report:#?}"
    );
    assert!(
        report.away.is_empty(),
        "the target lost the foreground while the indicator showed; in front instead: {:?} \
         (target {:#x})",
        report.away,
        report.target
    );
    assert!(
        report.focus_losses.is_empty(),
        "the target got focus/activation-loss messages while the indicator showed \
         ((message, wParam): 0x8 WM_KILLFOCUS, 0x6 WM_ACTIVATE, 0x86 WM_NCACTIVATE): {:?}",
        report.focus_losses
    );
    assert_eq!(code, 0, "run_return code after end_loop(0)");
}

// ---- S4: a click reaches the page and keeps the foreground ------------------------------

#[derive(Debug, Default)]
struct ClickReport {
    target: isize,
    shown: Shown,
    rect: Option<(i32, i32, i32, i32)>,
    hit: isize,
    indicator: isize,
    idle_pixel_before: bool,
    clicked_pixel_after: bool,
    pixel_after: Option<(u8, u8, u8)>,
    samples: u64,
    away: Vec<(isize, String)>,
    focus_losses: Vec<(u32, usize)>,
}

#[test]
fn a_click_on_the_indicator_reaches_the_page_and_the_target_keeps_the_foreground() {
    // Spike S4 / Acceptance 2 (second half, variant N): a SendInput click on the page's
    // "button" zone reaches the page (its pixel turns CLICKED_RGB: the indicator takes
    // clicks) while the target stays the foreground window at every 5 ms sample and for
    // SETTLE after, with no focus-loss message. (Under variant F the target would be back in
    // front within a budget instead; that variant has its own test only if N fails.) Bite: a
    // click-through window (pixel never turns), an activating window (target loses the
    // foreground), WebView2 taking the focus (WM_SETFOCUS -> SetFocus child, wry
    // webview2/mod.rs).
    let _serial = serial();
    let _watchdog =
        watchdog("a_click_on_the_indicator_reaches_the_page_and_the_target_keeps_the_foreground");
    let dir = TempDir::new();
    let (app, outcome) = wry_app(dir.path(), |_| {});

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let target = TestWindow::open(Shape::TopLevel);
        target.front();
        let target_raw = target.hwnd().0 as isize;
        let mut report = ClickReport {
            target: target_raw,
            ..ClickReport::default()
        };
        publish(&handle, &recording_state());
        report.shown = shown(&handle);
        let Some(raw) = indicator_hwnd(&handle).filter(|_| report.shown.ok()) else {
            return report;
        };
        let Some(rect) = window_rect(raw) else {
            return report;
        };
        report.rect = Some((rect.left, rect.top, rect.right, rect.bottom));
        let button = zone_point(&rect, 1);
        report.indicator = raw;
        report.hit = hit_root(button);
        report.idle_pixel_before =
            eventually(|| screen_pixel(button).is_some_and(|p| near(p, IDLE_RGB)));
        let losses_before = target.focus_losses().len();
        let sampler = ForegroundSampler::start(target_raw);
        move_cursor(button);
        let mut left = LeftButton::press();
        left.release();
        report.clicked_pixel_after =
            eventually(|| screen_pixel(button).is_some_and(|p| near(p, CLICKED_RGB)));
        report.pixel_after = screen_pixel(button);
        let _ = holds_for(SETTLE, || foreground().0 as isize == target_raw);
        let (samples, away) = sampler.stop();
        report.samples = samples;
        report.away = away;
        report.focus_losses = target
            .focus_losses()
            .get(losses_before..)
            .map(<[_]>::to_vec)
            .unwrap_or_default();
        report
    });

    assert!(
        report.shown.ok(),
        "no visible `{LABEL}` window after the receipt: {report:#?}"
    );
    assert_eq!(
        report.hit, report.indicator,
        "premise: the click point is not over the indicator (another window is on top): {report:#?}"
    );
    assert!(
        report.idle_pixel_before,
        "premise: the page never painted its idle colour: {report:#?}"
    );
    assert!(
        report.clicked_pixel_after,
        "the click did not reach the page (pixel stayed {:?}, want {CLICKED_RGB:?}): the \
         indicator is click-through or never received the mouse: {report:#?}",
        report.pixel_after
    );
    assert!(
        report.samples > 0,
        "premise: the foreground sampler ran: {report:#?}"
    );
    assert!(
        report.away.is_empty(),
        "the click took the foreground from the target; in front instead: {:?} (target {:#x}). \
         ADR-007 variant N failed: record the evidence and try variant F",
        report.away,
        report.target
    );
    assert!(
        report.focus_losses.is_empty(),
        "the target got focus/activation-loss messages from the click: {:?}",
        report.focus_losses
    );
    assert_eq!(code, 0, "run_return code after end_loop(0)");
}

// ---- S5: dragging -----------------------------------------------------------------------

#[derive(Debug, Default)]
struct DragReport {
    target: isize,
    shown: Shown,
    before: Option<(i32, i32, i32, i32)>,
    after: Option<(i32, i32, i32, i32)>,
    moved: bool,
    samples: u64,
    away: Vec<(isize, String)>,
}

#[test]
fn dragging_the_drag_region_moves_the_window_and_the_target_keeps_the_foreground() {
    // Spike S5 / Acceptance 6 (dragging works; the owner's real-desktop check stays, T-159):
    // a SendInput press on the `data-tauri-drag-region` zone, a move of 80 px and a release
    // move the window's rectangle by about that much (tauri's drag.js ->
    // plugin:window|start_dragging -> tao drag_window), and the target keeps the foreground at
    // every sample. NOTE for the developer: this harness uses `mock_context`, whose ACL may
    // refuse `core:window:allow-start-dragging`; if the page's drag cannot reach tao here, the
    // test cannot carry the capability and S5 becomes the owner's check only (record it in the
    // task, T-130 Investigation S5). The capability itself is pinned by `app_acl.rs`. Bite: a
    // page without the drag region, a missing `allow-start-dragging` grant, a window that
    // activates when dragged (foreground lost).
    let _serial = serial();
    let _watchdog =
        watchdog("dragging_the_drag_region_moves_the_window_and_the_target_keeps_the_foreground");
    let dir = TempDir::new();
    let (app, outcome) = wry_app(dir.path(), |_| {});

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let target = TestWindow::open(Shape::TopLevel);
        target.front();
        let target_raw = target.hwnd().0 as isize;
        let mut report = DragReport {
            target: target_raw,
            ..DragReport::default()
        };
        publish(&handle, &recording_state());
        report.shown = shown(&handle);
        let Some(raw) = indicator_hwnd(&handle).filter(|_| report.shown.ok()) else {
            return report;
        };
        let Some(rect) = window_rect(raw) else {
            return report;
        };
        report.before = Some((rect.left, rect.top, rect.right, rect.bottom));
        let grip = zone_point(&rect, 3);
        let _ = eventually(|| screen_pixel(grip).is_some_and(|p| near(p, DRAG_RGB)));
        let (vl, _, vr, _) = virtual_screen();
        let dx = if rect.left - 80 >= vl { -80 } else { 80 };
        let dx = if rect.right + dx > vr { -80 } else { dx };
        let sampler = ForegroundSampler::start(target_raw);
        move_cursor(grip);
        let mut left = LeftButton::press();
        for step in 1..=8 {
            move_cursor(POINT {
                x: grip.x + dx * step / 8,
                y: grip.y,
            });
            thread::sleep(Duration::from_millis(20));
        }
        left.release();
        report.moved = eventually(|| {
            window_rect(raw).is_some_and(|now| (now.left - rect.left - dx).abs() <= 4)
        });
        report.after = window_rect(raw).map(|r| (r.left, r.top, r.right, r.bottom));
        let (samples, away) = sampler.stop();
        report.samples = samples;
        report.away = away;
        report
    });

    assert!(
        report.shown.ok(),
        "no visible `{LABEL}` window after the receipt: {report:#?}"
    );
    assert!(
        report.moved,
        "dragging the drag region did not move the window by the dragged distance: {report:#?}"
    );
    assert!(
        report.samples > 0,
        "premise: the foreground sampler ran: {report:#?}"
    );
    assert!(
        report.away.is_empty(),
        "the drag took the foreground from the target; in front instead: {:?} (target {:#x})",
        report.away,
        report.target
    );
    assert_eq!(code, 0, "run_return code after end_loop(0)");
}

// ---- S6: the census ---------------------------------------------------------------------

#[derive(Debug, Default)]
struct CensusReport {
    before: Vec<(isize, String)>,
    shown: Shown,
    indicator: isize,
    after: Vec<(isize, String)>,
    after_settled: Vec<(isize, String)>,
    unlisted_visible: Vec<(isize, String)>,
}

#[test]
fn the_indicator_adds_exactly_one_top_level_window_to_the_process() {
    // Spike S6 / Acceptance 3 ("a second webview adds no unlisted helper window"): the
    // top-level windows of the process (visible or not, with class) before the first
    // recording state and after the show (and SETTLE later) differ by exactly the indicator's
    // HWND. A new class here is a helper window: the evidence goes into the task and, if it
    // is listed in scripts/ci/helper-windows.txt, the task becomes checker: true. Bite: a
    // second window built with the indicator (an event-target or a hidden pre-built page), a
    // window that survives a rebuild.
    let _serial = serial();
    let _watchdog = watchdog("the_indicator_adds_exactly_one_top_level_window_to_the_process");
    let dir = TempDir::new();
    let (app, outcome) = wry_app(dir.path(), |_| {});

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let mut report = CensusReport {
            before: census(),
            ..CensusReport::default()
        };
        publish(&handle, &recording_state());
        report.shown = shown(&handle);
        report.indicator = indicator_hwnd(&handle).unwrap_or(0);
        report.after = census();
        let _ = holds_for(SETTLE, || census().len() == report.after.len());
        report.after_settled = census();
        report.unlisted_visible = report
            .after_settled
            .iter()
            .filter(|(raw, class)| {
                is_visible(*raw) && !helper_windows::is_helper(class, ex_style(*raw))
            })
            .filter(|(raw, _)| !report.before.iter().any(|(b, _)| b == raw))
            .cloned()
            .collect();
        report
    });

    assert!(
        report.shown.ok(),
        "no visible `{LABEL}` window after the receipt: {report:#?}"
    );
    let added = |later: &[(isize, String)]| -> Vec<(isize, String)> {
        later
            .iter()
            .filter(|(raw, _)| !report.before.iter().any(|(b, _)| b == raw))
            .cloned()
            .collect()
    };
    let removed: Vec<_> = report
        .before
        .iter()
        .filter(|(raw, _)| !report.after_settled.iter().any(|(a, _)| a == raw))
        .collect();
    assert_eq!(
        added(&report.after)
            .iter()
            .map(|(raw, _)| *raw)
            .collect::<Vec<_>>(),
        vec![report.indicator],
        "the show added windows other than the indicator: {report:#?}"
    );
    assert_eq!(
        added(&report.after_settled)
            .iter()
            .map(|(raw, _)| *raw)
            .collect::<Vec<_>>(),
        vec![report.indicator],
        "a window appeared within {SETTLE:?} after the show: {report:#?}"
    );
    assert!(removed.is_empty(), "the show removed windows: {removed:?}");
    assert_eq!(
        report.unlisted_visible.iter().map(|(raw, _)| *raw).collect::<Vec<_>>(),
        vec![report.indicator],
        "the visible windows the install smoke would count are not exactly the indicator: {report:#?}"
    );
    assert_eq!(code, 0, "run_return code after end_loop(0)");
}

// ---- T033: the position is saved and clamped --------------------------------------------

#[derive(Debug, Default)]
struct PositionReport {
    visible_saved: Option<IndicatorPosition>,
    far_saved: Option<IndicatorPosition>,
    file_has_position: bool,
    rebuilt: Shown,
    rebuilt_rect: Option<(i32, i32, i32, i32)>,
    far_build: Shown,
    far_rect: Option<(i32, i32, i32, i32)>,
    log: Vec<String>,
}

fn position_in_file(dir: &Path) -> Option<Value> {
    let bytes = std::fs::read(dir.join(SETTINGS_FILE)).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value.pointer("/calls/indicator_position").cloned()
}

fn inside_virtual_screen(rect: (i32, i32, i32, i32)) -> bool {
    let (l, t, r, b) = virtual_screen();
    rect.0 >= l && rect.1 >= t && rect.2 <= r && rect.3 <= b
}

#[test]
fn position_saved_and_clamped() {
    // Acceptance 6 (T033: "the position is saved in calls.indicator_position and clamped to a
    // visible monitor"): `call_indicator::moved` (the function behind `call_indicator_moved`)
    // with a visible point saves exactly that point, in the service and in settings.json; with
    // a point far outside every monitor it saves a point inside the virtual screen, not the
    // requested one; the next build puts the window at the saved point (physical pixels); a
    // saved far-away position (written by core's writer before the first build, as an unplugged
    // monitor would leave it) is clamped at the build: the shown window lies inside the virtual
    // screen. Bite: `moved` writing the raw request, `moved` that does not persist (snapshot
    // only), a build that ignores the saved position or trusts it unclamped, a clamp against
    // the primary monitor only (the virtual-screen bound is looser, so the unit tests of
    // `clamp_to_monitors` carry the exact rule).
    let _serial = serial();
    let _watchdog = watchdog("position_saved_and_clamped");
    let dir = TempDir::new();
    let (app, outcome) = wry_app(dir.path(), |_| {});
    let logs_of = dir.path().to_path_buf();
    let data = dir.path().to_path_buf();
    let wanted = IndicatorPosition { x: 50, y: 60 };
    const FAR: i32 = 30_000;

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let service = handle.state::<Arc<SettingsService>>().inner().clone();
        let saved = |service: &SettingsService| service.snapshot().calls.indicator_position;
        let mut report = PositionReport::default();

        call_indicator::moved(&handle, wanted.x, wanted.y);
        report.visible_saved = eventually(|| saved(&service) == Some(wanted)).then_some(wanted);
        report.file_has_position =
            eventually(|| position_in_file(&data) == Some(json!({ "x": wanted.x, "y": wanted.y })));

        // The next build lands on the saved point.
        publish(&handle, &recording_state());
        report.rebuilt = shown(&handle);
        report.rebuilt_rect = indicator_hwnd(&handle)
            .and_then(window_rect)
            .map(|r| (r.left, r.top, r.right, r.bottom));
        publish(&handle, &IndicatorState::idle());
        let _ = eventually(|| handle.get_webview_window(LABEL).is_none());

        // A point far outside every monitor is clamped before it is saved.
        call_indicator::moved(&handle, -FAR, -FAR);
        let _ = eventually(|| saved(&service) != Some(wanted));
        report.far_saved = saved(&service);

        // A far-away position already stored (a removed monitor) is clamped at the build.
        let _ = service.remember_indicator_position(IndicatorPosition { x: FAR, y: FAR });
        publish(&handle, &recording_state());
        report.far_build = shown(&handle);
        report.far_rect = indicator_hwnd(&handle)
            .and_then(window_rect)
            .map(|r| (r.left, r.top, r.right, r.bottom));
        report.log = logged(&logs_of);
        report
    });

    assert_eq!(
        report.visible_saved,
        Some(wanted),
        "a visible point was not saved as it is: {report:#?}"
    );
    assert!(
        report.file_has_position,
        "settings.json lacks calls.indicator_position: {report:#?}"
    );
    assert!(
        report.rebuilt.ok(),
        "no indicator at the next start: {report:#?}"
    );
    assert_eq!(
        report.rebuilt_rect.map(|r| (r.0, r.1)),
        Some((wanted.x, wanted.y)),
        "the rebuilt window is not at the saved position: {report:#?}"
    );
    let far = report
        .far_saved
        .expect("the far position was not saved at all");
    assert_ne!(
        far,
        IndicatorPosition { x: -FAR, y: -FAR },
        "an off-screen point was saved unclamped"
    );
    let (vl, vt, vr, vb) = virtual_screen();
    assert!(
        far.x >= vl && far.y >= vt && far.x < vr && far.y < vb,
        "the clamped position {far:?} is outside the virtual screen ({vl}, {vt}, {vr}, {vb})"
    );
    assert!(
        report.far_build.ok(),
        "no indicator for the far-away saved position: {report:#?}"
    );
    assert!(
        report.far_rect.is_some_and(inside_virtual_screen),
        "the window built for a saved far-away position lies outside the virtual screen \
         {:?}: {report:#?}",
        virtual_screen()
    );
    assert_eq!(code, 0, "run_return code after end_loop(0)");
}

// ---- commands and events (mock runtime, the release context: the real ACL) ---------------

fn release_context() -> Context<MockRuntime> {
    tauri::generate_context!(test = true)
}

fn discard_log() -> Arc<Log> {
    let exe = std::env::current_exe().expect("current_exe");
    voicen_lib::diag::start(exe.join("logs"), Box::new(|_| {}))
}

fn mock_app(dir: &Path) -> (App<MockRuntime>, Arc<SettingsService>) {
    let service = load_settings(
        dir.to_path_buf(),
        no_keys(),
        Arc::new(voicen_core::autostart::FakeAutostart::new()),
        Arc::new(ModelStore::new(dir.join("models"), MODELS)),
        None,
        &discard_log(),
    )
    .0;
    let app = voicen_lib::build_app(
        mock_builder(),
        release_context(),
        Arc::clone(&service),
        idle_models(),
        discard_log(),
    )
    .expect("mock app builds");
    (app, service)
}

fn invoke(
    window: &tauri::WebviewWindow<MockRuntime>,
    cmd: &str,
    args: Value,
) -> Result<Value, Value> {
    let url: Url = window.url().expect("window url");
    get_ipc_response(
        window,
        InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url,
            body: InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    )
    .map(|body| body.deserialize::<Value>().expect("response is JSON"))
}

/// A control that records what the commands asked.
#[derive(Default)]
struct RecordingControl {
    calls: Mutex<Vec<String>>,
}

impl RecordingControl {
    fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl IndicatorControl for RecordingControl {
    fn stop(&self) {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push("stop".to_string());
    }

    fn dismiss_warning(&self, id: u32) {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("dismiss {id}"));
    }
}

/// Every payload of `event` emitted so far, with the instant it reached the listener (a Rust
/// `listen_any` handler runs on the emitting thread).
fn collect_events(app: &App<MockRuntime>, event: &str) -> Arc<Mutex<Vec<(Instant, Value)>>> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    app.listen_any(event, move |e| {
        let payload: Value = serde_json::from_str(e.payload()).expect("event payload is JSON");
        sink.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((now(), payload));
    });
    events
}

fn mock_part(app: &App<MockRuntime>) -> call_indicator::IndicatorPart {
    call_indicator::part(app.handle()).expect("assemble wired the call indicator")
}

fn mock_show(app: &App<MockRuntime>, state: &IndicatorState) -> tauri::WebviewWindow<MockRuntime> {
    mock_part(app).set_state(state);
    let receipt = call_indicator::settled(app.handle(), INDICATOR_HANG);
    assert!(
        receipt.is_some_and(|r| !r.failed && r.window),
        "the indicator receipt did not report a handled, unfailed state holding the built \
         window within {INDICATOR_HANG:?}: {receipt:?}"
    );
    app.get_webview_window(LABEL)
        .unwrap_or_else(|| panic!("no `{LABEL}` window after the receipt"))
}

#[test]
fn commands_reach_the_control_and_tick_is_at_most_10_hz() {
    // Acceptance 5 (commands and events of contracts/ipc.md, through the real ACL of the
    // release context, from the indicator thread's own window):
    // - call_indicator_stop reaches the control's `stop` once; call_indicator_dismiss { id }
    //   reaches `dismiss_warning(id)` with that id; call_indicator_moved saves the position;
    // - call_indicator_state returns the mailbox's newest state, the same as the last
    //   `indicator://state` emit, and a newer state after a newer publish;
    // - `indicator://tick`: 200 ticks published 5 ms apart reach the listener at least 90 ms
    //   apart (at most 10 Hz, 10% for delivery jitter), coalesced to far fewer emits, and the
    //   newest tick is the last one emitted. Bite: a tick emitted per `set_tick`, a stop that
    //   is never forwarded, an id dropped, `state` reading a copy other than the emitted one, a
    //   tick coalesced to the oldest.
    let _serial = serial();
    let dir = TempDir::new();
    let (app, service) = mock_app(dir.path());
    let states = collect_events(&app, call_indicator::STATE_EVENT);
    let ticks = collect_events(&app, call_indicator::TICK_EVENT);
    let control = Arc::new(RecordingControl::default());
    call_indicator::set_control(app.handle(), control.clone());

    let window = mock_show(&app, &recording_state());
    let first_state = serde_json::to_value(recording_state()).expect("state is JSON");
    let emitted = |events: &Arc<Mutex<Vec<(Instant, Value)>>>| {
        events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .last()
            .map(|(_, v)| v.clone())
    };
    assert_eq!(
        emitted(&states),
        Some(first_state.clone()),
        "indicator://state after the build"
    );
    assert_eq!(
        invoke(&window, "call_indicator_state", json!({})),
        Ok(first_state.clone()),
        "call_indicator_state is the newest published state"
    );

    assert_eq!(
        invoke(&window, "call_indicator_stop", json!({})),
        Ok(Value::Null)
    );
    assert_eq!(control.calls(), vec!["stop".to_string()]);
    assert_eq!(
        invoke(&window, "call_indicator_dismiss", json!({ "id": 7 })),
        Ok(Value::Null)
    );
    assert_eq!(
        control.calls(),
        vec!["stop".to_string(), "dismiss 7".to_string()]
    );

    assert_eq!(
        invoke(&window, "call_indicator_moved", json!({ "x": 50, "y": 60 })),
        Ok(Value::Null)
    );
    assert!(
        eventually(|| service.snapshot().calls.indicator_position
            == Some(IndicatorPosition { x: 50, y: 60 })),
        "call_indicator_moved did not save the position"
    );

    let hidden_reminder = IndicatorState::recording(UiLanguage::En, false, Vec::new());
    mock_part(&app).set_state(&hidden_reminder);
    let newer = serde_json::to_value(&hidden_reminder).expect("state is JSON");
    assert!(
        eventually(|| emitted(&states) == Some(newer.clone())),
        "the newer state was not emitted: {:?}",
        emitted(&states)
    );
    assert_eq!(
        invoke(&window, "call_indicator_state", json!({})),
        Ok(newer)
    );

    let part = mock_part(&app);
    for n in 0..200u64 {
        part.set_tick(&IndicatorTick {
            elapsed_ms: n,
            mic_level: 0.1,
            system_level: 0.2,
            mic_silent: false,
            system_silent: false,
        });
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        eventually(|| emitted(&ticks).is_some_and(|t| t["elapsedMs"] == json!(199))),
        "the newest tick (elapsedMs 199) was never emitted: {:?}",
        emitted(&ticks)
    );
    let seen = ticks.lock().unwrap_or_else(PoisonError::into_inner).clone();
    assert!(
        seen.len() >= 2,
        "ticks must flow while the call records: {} emits",
        seen.len()
    );
    assert!(
        seen.len() < 100,
        "ticks are not coalesced: {} emits for 200 ticks",
        seen.len()
    );
    for pair in seen.windows(2) {
        if let [(a, _), (b, _)] = pair {
            at_least(between(*a, *b), Duration::from_millis(90));
        }
    }
    let elapsed: Vec<u64> = seen
        .iter()
        .filter_map(|(_, v)| v["elapsedMs"].as_u64())
        .collect();
    assert!(
        elapsed.windows(2).all(|w| w[0] < w[1]),
        "tick payloads are not strictly newer one after the other: {elapsed:?}"
    );
}

#[test]
fn commands_without_a_control_answer_null_and_do_nothing() {
    // Risk "IndicatorControl stays unset until T-128 wires the session": with no control the
    // stop and dismiss commands resolve `null`, call nothing and do not panic; the state
    // command still answers. Bite: an `unwrap` on the empty slot.
    let _serial = serial();
    let dir = TempDir::new();
    let (app, _service) = mock_app(dir.path());
    let window = mock_show(&app, &recording_state());
    assert_eq!(
        invoke(&window, "call_indicator_stop", json!({})),
        Ok(Value::Null)
    );
    assert_eq!(
        invoke(&window, "call_indicator_dismiss", json!({ "id": 1 })),
        Ok(Value::Null)
    );
    let state = invoke(&window, "call_indicator_state", json!({})).expect("state answers");
    assert_eq!(state["recording"], json!(true));
}
