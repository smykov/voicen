//! T-130: the call indicator window (`call-indicator`) on tauri's real Wry runtime
//! (WebView2) and, for the commands, over the shell's call wiring (`call::start_calls` on the
//! mock runtime). Spec 007 FR-005, FR-006; research R12 / spike T032; ADR-007 (#235);
//! docs/tasks/T-130.md rows S1-S8. Windows CI only (decision #5): none of this runs on the
//! Linux dev host, so the red and green evidence is the `ci.yml` run on `wip/T-130`. On Linux the
//! gate only compile-checks it (`make check-shell-windows`).
//!
//! The harness is `tests/overlay.rs`'s (T-057; each test file is its own crate): serial tests,
//! `Builder::any_thread` + `App::run_return`, a driver thread started on `Ready`, a watchdog that
//! ends the exe with the test's name, a `TestWindow` target in front, a `ForegroundSampler`,
//! SendInput. Every fact is a raw Win32 value (F-003). Clock readings decide no upper bound
//! (T-201): waits are `timing_support::eventually` and the shell's own receipt.
//!
//! Signatures this file assumes (the developer's target; `voicen_lib::call_indicator` does not
//! exist yet, so the file fails to compile until it does: that unresolved import is the red):
//!
//! ```text
//! voicen_lib::call_indicator::
//!   LABEL: &str = "call-indicator"
//!   Settled { seq: u64, window: bool, failed: bool }   // the overlay's `Settled` shape
//!   start<R: Runtime>(app: &AppHandle<R>, service: Arc<SettingsService>, log: Arc<Log>)
//!       // creates and manages the indicator mailbox, spawns the one `call-indicator` thread;
//!       // `call::start_calls` calls it before `CallSession::new` (a test of the bare Wry app
//!       // calls it itself); no window exists after it (S1)
//!   settled<R: Runtime>(app: &AppHandle<R>, hang: Duration) -> Option<Settled>
//!       // the overlay's `settled` receipt: waits, without polling, until the thread settled on
//!       // a seq at least the newest published when called (never `Destroying`); `None` at `hang`
//!   commands call_indicator_state / call_indicator_stop / call_indicator_dismiss(warning_id: u32)
//!       // page args `{}` / `{}` / `{warningId}`; stop and dismiss return null at once
//! voicen_lib::call::ShellCallIndicator::new(&AppHandle<R>)
//!   // `set_state` / `set_tick` forward to `call_indicator::part(app)`; set_call_recording: tray
//! voicen_lib::call::CallRequest::{IndicatorStop, Dismiss(u32)}   // the worker's two new requests
//! Events: `indicator://state` (IndicatorState camelCase), `indicator://tick`, to the label only.
//! ```
//!
//! The Wry apps use the RELEASE context (`generate_context!(test = true)`: real capabilities, so
//! `core:window:allow-start-dragging` is the real grant) with `set_assets` replaced by a test
//! page (`Page`: a body that turns colour on mousedown; with or without a drag region) and the
//! CSP off (an inline script). The commands test needs no ACL (the ACL is `app_acl.rs`).
//!
//! Spike variant: `VARIANT_N` is ADR-007's primary (WS_EX_NOACTIVATE, the target keeps the
//! foreground through click and drag). If S3-S5 cannot pass under N, the developer falls back to
//! variant F (focusable; the shell restores the foreign foreground window) and flips the const
//! with the CI evidence recorded in the task; S4/S5/S7 then hold the same end state ("the target
//! is in front again") and drop only the "never lost it" half. If neither passes, S7 stays red
//! and the evidence plus the native-window question go to the owner (#235).
#![cfg(windows)]

mod call_shell_support;
mod timing_support;
mod win32_support;

use std::borrow::Cow;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{get_ipc_response, MockRuntime};
use tauri::utils::assets::{AssetKey, AssetsIter, CspHash};
use tauri::webview::InvokeRequest;
use tauri::{App, AppHandle, Assets, Context, Listener, Manager, RunEvent, Wry};
use timing_support::{eventually, holds_for, measure, now};
use voicen_core::call::indicator::{IndicatorState, IndicatorTick};
use voicen_core::call::session::CallIndicator;
use voicen_core::call::CallSource;
use voicen_core::diag::Log;
use voicen_core::i18n::UiLanguage;
use voicen_core::local_models::catalog::MODELS;
use voicen_core::local_models::service::LocalModels;
use voicen_core::local_models::store::ModelStore;
use voicen_core::secrets::{CredentialStore, FakeCredentialStore};
use voicen_core::settings::service::SettingsService;
use voicen_core::settings::{IndicatorPosition, LoadOutcome};
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_lib::call::{self, CallRequest, ShellCallIndicator};
use voicen_lib::call_indicator::{self, Settled, LABEL};
use voicen_lib::settings_ipc::load_settings;
use win32_support::{class_of, foreground, precondition, send, Shape, TestWindow};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetDC, GetMonitorInfoW, GetPixel, MonitorFromPoint, ReleaseDC, CLR_INVALID, MONITORINFO,
    MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEINPUT, MOUSE_EVENT_FLAGS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetWindow, GetWindowLongPtrW, GetWindowRect,
    GetWindowThreadProcessId, IsWindow, IsWindowVisible, SetCursorPos, SetWindowPos,
    WindowFromPoint, GA_ROOT, GWL_EXSTYLE, GWL_STYLE, GW_OWNER, SWP_NOACTIVATE, SWP_NOSIZE,
    SWP_NOZORDER, WS_CAPTION, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};

/// ADR-007's primary variant: the window is non-activating (`WS_EX_NOACTIVATE`).
const VARIANT_N: bool = true;

/// A test identifier: WebView2's profile goes under it, not under the release one.
const IDENTIFIER: &str = "dev.voicen.test.callindicator";

/// A hang bound for the shell's receipt, not a budget (T-173): above a cold WebView2 build.
const RECEIPT_HANG: Duration = Duration::from_secs(120);

/// How long a state is watched after it showed (a bounded negative): a late activation, a late
/// second window, a late `SW_SHOW`.
const SETTLE: Duration = Duration::from_secs(2);

/// How long a blocked main-thread task waits for its release.
const HOLD: Duration = Duration::from_secs(10);

/// The driver's report after the loop returned.
const REPORT_WAIT: Duration = Duration::from_secs(20);

/// The exe ends with code 3 if one test's loop has not returned by then.
const WATCHDOG: Duration = Duration::from_secs(240);

/// The page's body colour, and the colour a mousedown turns it to: far from every system
/// colour, so a pixel of either on screen is the indicator's paint.
const PAGE_RGB: (u8, u8, u8) = (32, 160, 64);
const DOWN_RGB: (u8, u8, u8) = (200, 40, 40);

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
                "call_indicator watchdog: `{test}`: the event loop did not return within \
                 {WATCHDOG:?}; ending the test exe"
            );
            std::process::exit(3);
        }
    });
    Watchdog(Some(tx))
}

// ---- the app -------------------------------------------------------------------------------

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

/// The test page: an opaque [`PAGE_RGB`] body that turns [`DOWN_RGB`] on mousedown, with
/// `data-tauri-drag-region` when `drag` (tauri's own drag script then calls
/// `plugin:window|start_dragging`, which needs the real `core:window:allow-start-dragging`).
struct Page {
    drag: bool,
}

impl Page {
    fn html(&self) -> String {
        let (r, g, b) = PAGE_RGB;
        let (dr, dg, db) = DOWN_RGB;
        let drag = if self.drag {
            " data-tauri-drag-region"
        } else {
            ""
        };
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"></head>\
             <body{drag} style=\"margin:0;width:100vw;height:100vh;background:rgb({r},{g},{b})\">\
             <script>document.addEventListener('mousedown',function(){{\
             document.body.style.background='rgb({dr},{dg},{db})';}});</script>\
             </body></html>"
        )
    }
}

impl Assets<Wry> for Page {
    fn get(&self, key: &AssetKey) -> Option<Cow<'_, [u8]>> {
        key.as_ref()
            .ends_with(".html")
            .then(|| Cow::Owned(self.html().into_bytes()))
    }

    fn iter(&self) -> Box<AssetsIter<'_>> {
        Box::new(std::iter::empty())
    }

    fn csp_hashes(&self, _html_path: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        Box::new(std::iter::empty())
    }
}

/// T-267: a page whose first frame is late. The body starts white (unpainted, as WebView2's
/// surface is before its first composition) and turns [`PAGE_RGB`] `paint_after` later from a
/// `setTimeout`; `None` never turns it. It reacts to mousedown like [`Page`].
struct LatePage {
    paint_after: Option<Duration>,
}

impl LatePage {
    fn html(&self) -> String {
        let (r, g, b) = PAGE_RGB;
        let (dr, dg, db) = DOWN_RGB;
        let paint = self.paint_after.map_or_else(String::new, |after| {
            format!(
                "setTimeout(function(){{document.body.style.background='rgb({r},{g},{b})';}},{});",
                after.as_millis()
            )
        });
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"></head>\
             <body style=\"margin:0;width:100vw;height:100vh;background:rgb(255,255,255)\">\
             <script>{paint}document.addEventListener('mousedown',function(){{\
             document.body.style.background='rgb({dr},{dg},{db})';}});</script>\
             </body></html>"
        )
    }
}

impl Assets<Wry> for LatePage {
    fn get(&self, key: &AssetKey) -> Option<Cow<'_, [u8]>> {
        key.as_ref()
            .ends_with(".html")
            .then(|| Cow::Owned(self.html().into_bytes()))
    }

    fn iter(&self) -> Box<AssetsIter<'_>> {
        Box::new(std::iter::empty())
    }

    fn csp_hashes(&self, _html_path: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        Box::new(std::iter::empty())
    }
}

/// What a Wry test app hands its driver.
struct Rig {
    app: App<Wry>,
    outcome: LoadOutcome,
    service: Arc<SettingsService>,
    log: Arc<Log>,
}

/// The app as `run()` builds it (`build_app`) on the real Wry runtime, on a loaded settings.json
/// (so `Ready` opens no settings window), with the release context's capabilities and `page` as
/// the only asset.
fn wry_rig(dir: &Path, page: impl Assets<Wry> + 'static) -> Rig {
    let log = voicen_lib::diag::start(dir.join("logs"), Box::new(|_| {}));
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
    let mut context: Context<Wry> = tauri::generate_context!(test = true);
    context.config_mut().identifier = IDENTIFIER.to_string();
    context.config_mut().app.security.csp = None;
    // Plain `cargo test` has no `custom-protocol` feature, so tauri is cfg(dev) and
    // `WebviewUrl::App` resolves against `build.dev_url` (http://localhost:1420): without this the
    // indicator would load the WebView2 error page, never `page`. Bite: with the line removed s4
    // sees a white pixel and s5/s7 get no JS (no `start_dragging`).
    context.config_mut().build.dev_url = None;
    context.set_assets(Box::new(page));
    let app = voicen_lib::build_app(
        tauri::Builder::default().any_thread(),
        context,
        Arc::clone(&service),
        idle_models(),
        Arc::clone(&log),
    )
    .expect("the Wry app builds");
    Rig {
        app,
        outcome,
        service,
        log,
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "a panic without a message".to_string())
}

/// Ends the loop with `code` from inside a main-thread task.
fn end_loop(handle: &AppHandle<Wry>, code: i32) {
    let inner = handle.clone();
    let _ = handle.run_on_main_thread(move || inner.exit(code));
}

/// Runs the loop with `on_run_event` (as `run()` does), starts the indicator thread and `driver`
/// once `Ready` was handled, and returns the driver's report. The driver's panic ends the loop and
/// is re-raised here.
#[track_caller]
fn run_with_driver<T: Send + 'static>(
    rig: Rig,
    driver: impl FnOnce(AppHandle<Wry>, Arc<SettingsService>) -> T + Send + 'static,
) -> T {
    let Rig {
        app,
        outcome,
        service,
        log,
    } = rig;
    let (tx, rx) = mpsc::channel();
    let mut driver = Some(driver);
    let code = app.run_return(move |handle, event| {
        let ready = matches!(event, RunEvent::Ready);
        voicen_lib::on_run_event(handle, event, &outcome, false);
        if ready {
            // The part `start_calls` makes in the product: no window yet (S1).
            call_indicator::start(handle, Arc::clone(&service), Arc::clone(&log));
            if let Some(driver) = driver.take() {
                let handle = handle.clone();
                let service = Arc::clone(&service);
                let tx = tx.clone();
                thread::spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        driver(handle.clone(), service)
                    }));
                    let result = result.map_err(|p| panic_text(p.as_ref()));
                    end_loop(&handle, 0);
                    let _ = tx.send(result);
                });
            }
        }
    });
    match rx.recv_timeout(REPORT_WAIT) {
        Ok(Ok(report)) => report,
        Ok(Err(message)) => panic!("{message}"),
        Err(err) => panic!("the driver sent no report ({err}); run_return code {code}"),
    }
}

// ---- raw Win32 facts -----------------------------------------------------------------------

fn to_hwnd(raw: isize) -> HWND {
    HWND(raw as *mut std::ffi::c_void)
}

/// The raw handle of the window labelled `call-indicator`, if tauri has one.
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
    (unsafe { GetWindowLongPtrW(to_hwnd(raw), GWL_EXSTYLE) }) as u32
}

fn style(raw: isize) -> u32 {
    // SAFETY: GetWindowLongPtrW reads the window's style; no message is sent.
    (unsafe { GetWindowLongPtrW(to_hwnd(raw), GWL_STYLE) }) as u32
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
    /// Click-through would make Stop unreachable (T-130: the window takes clicks).
    click_through: bool,
    captioned: bool,
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
        captioned: style(raw) & WS_CAPTION.0 == WS_CAPTION.0,
        // SAFETY: both calls only read the window's relations; no message is sent.
        owned: unsafe { GetWindow(hwnd, GW_OWNER) }.is_ok_and(|o| !o.is_invalid()),
        top_level: unsafe { GetAncestor(hwnd, GA_ROOT) } == hwnd,
    }
}

/// What a shown indicator lacks of its shape (S2): topmost, a tool window without APPWINDOW (no
/// taskbar entry, out of Alt+Tab), undecorated, unowned top-level, not click-through, and (variant
/// N) non-activating.
fn shape_errors(f: &Facts) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if VARIANT_N && !f.noactivate {
        missing.push("no WS_EX_NOACTIVATE (variant N)");
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
        missing
            .push("click-through (WS_EX_TRANSPARENT | WS_EX_LAYERED): Stop could not be clicked");
    }
    if f.captioned {
        missing.push("WS_CAPTION set (decorated)");
    }
    if f.owned {
        missing.push("owned");
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

fn centre(rect: &RECT) -> POINT {
    POINT {
        x: rect.left / 2 + rect.right / 2,
        y: rect.top / 2 + rect.bottom / 2,
    }
}

/// The primary monitor's work area (physical pixels).
fn primary_work_area() -> RECT {
    // SAFETY: any point; the primary monitor is the default.
    let monitor = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..MONITORINFO::default()
    };
    // SAFETY: `info` is writable with its size set.
    precondition(
        "monitor_info",
        unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool(),
        "GetMonitorInfoW on the primary monitor failed",
    );
    info.rcWork
}

fn inside(outer: &RECT, inner: &RECT) -> bool {
    outer.left <= inner.left
        && outer.top <= inner.top
        && inner.right <= outer.right
        && inner.bottom <= outer.bottom
}

/// The top-level window a click at `p` reaches, with its class.
fn hit_root(p: POINT) -> (isize, String) {
    // SAFETY: any point; the result is a handle or null.
    let hit = unsafe { WindowFromPoint(p) };
    // SAFETY: GetAncestor only reads the window tree.
    let root = unsafe { GetAncestor(hit, GA_ROOT) };
    (root.0 as isize, class_of(root))
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
    const TOLERANCE: u8 = 24;
    a.0.abs_diff(b.0) <= TOLERANCE
        && a.1.abs_diff(b.1) <= TOLERANCE
        && a.2.abs_diff(b.2) <= TOLERANCE
}

/// Every top-level window of this process, visible or not, with its class (S6's census).
fn all_windows() -> Vec<(isize, String)> {
    unsafe extern "system" fn collect(hwnd: HWND, found: LPARAM) -> BOOL {
        let mut pid = 0u32;
        // SAFETY: `pid` is a writable local; the call only reads the window's owner.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == std::process::id() {
            // SAFETY: `found` is the `Vec` that `all_windows` passes, alive and not otherwise
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

fn mouse(flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn move_cursor(p: POINT) {
    // SAFETY: a plain screen point.
    let moved = unsafe { SetCursorPos(p.x, p.y) }.is_ok();
    precondition(
        "sendinput",
        moved,
        format!("SetCursorPos({}, {}) failed", p.x, p.y),
    );
}

fn mouse_button(down: bool) {
    let flags = if down {
        MOUSEEVENTF_LEFTDOWN
    } else {
        MOUSEEVENTF_LEFTUP
    };
    let inserted = send(&[mouse(flags)]);
    precondition(
        "sendinput",
        inserted == 1,
        format!("SendInput inserted {inserted} of 1 mouse event"),
    );
}

/// A left click at `p`.
fn click(p: POINT) {
    move_cursor(p);
    mouse_button(true);
    mouse_button(false);
}

/// A left-button drag from `from` by `(dx, dy)` in steps.
fn drag(from: POINT, dx: i32, dy: i32) {
    move_cursor(from);
    mouse_button(true);
    for step in 1..=10 {
        move_cursor(POINT {
            x: from.x + dx * step / 10,
            y: from.y + dy * step / 10,
        });
        thread::sleep(Duration::from_millis(20));
    }
    mouse_button(false);
}

/// The sample count and every window other than the target seen in front.
type Sampled = (u64, Vec<(isize, String)>);

/// Samples `GetForegroundWindow` every 5 ms on its own thread and keeps every window other than
/// `target` it saw in front (with its class), until stopped.
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

// ---- driving the indicator through the production path -------------------------------------

fn recording_state() -> IndicatorState {
    IndicatorState::recording(UiLanguage::En, true, Vec::new())
}

fn idle_state() -> IndicatorState {
    IndicatorState::idle(UiLanguage::En)
}

fn tick(elapsed_ms: u64) -> IndicatorTick {
    IndicatorTick {
        elapsed_ms,
        mic_level: 0.25,
        system_level: 0.5,
        mic_silent: false,
        system_silent: false,
    }
}

/// What the driver saw of one published state: the thread's receipt and, read once after it, the
/// indicator window's handle and its visibility, both read inside `shown()` while the window is
/// alive (a later state may destroy it: a lazy read at assert time would see a dead handle).
#[derive(Debug, Default, Clone, Copy)]
struct Shown {
    receipt: Option<Settled>,
    hwnd: Option<isize>,
    visible: bool,
}

impl Shown {
    /// The thread settled on the state without a failed answer and the window was visible when
    /// `shown()` read it. Bite: a window never shown captures `visible = false`, so this is false.
    fn ok(&self) -> bool {
        self.receipt.is_some_and(|r| r.window && !r.failed) && self.visible
    }
}

/// The receipt of the newest published state (never a wall-clock ceiling on the build, T-173),
/// then the window read once.
fn shown(handle: &AppHandle<Wry>) -> Shown {
    let (receipt, took) = measure(|| call_indicator::settled(handle, RECEIPT_HANG));
    eprintln!("call indicator: receipt {receipt:?} after {took:?}");
    let hwnd = receipt.and_then(|_| indicator_hwnd(handle));
    Shown {
        receipt,
        hwnd,
        visible: hwnd.is_some_and(is_visible),
    }
}

/// A target window in front (asserted loudly by `front`) and a sampler on it.
struct Front {
    target: TestWindow,
    raw: isize,
    losses_before: usize,
    sampler: ForegroundSampler,
}

fn front() -> Front {
    let target = TestWindow::open(Shape::TopLevel);
    target.front();
    let raw = target.hwnd().0 as isize;
    let losses_before = target.focus_losses().len();
    let sampler = ForegroundSampler::start(raw);
    Front {
        target,
        raw,
        losses_before,
        sampler,
    }
}

/// What the foreground did while a `Front` was sampled.
#[derive(Debug, Default)]
struct ForegroundFacts {
    samples: u64,
    away: Vec<(isize, String)>,
    focus_losses: Vec<(u32, usize)>,
}

impl Front {
    fn finish(self) -> ForegroundFacts {
        let (samples, away) = self.sampler.stop();
        ForegroundFacts {
            samples,
            away,
            focus_losses: self
                .target
                .focus_losses()
                .get(self.losses_before..)
                .map(<[_]>::to_vec)
                .unwrap_or_default(),
        }
    }
}

fn show_recording(handle: &AppHandle<Wry>) -> (ShellCallIndicator<Wry>, Shown) {
    let indicator = ShellCallIndicator::new(handle);
    indicator.set_state(&recording_state());
    let shown = shown(handle);
    (indicator, shown)
}

// ==== S1: no window before the first recording state =========================================

#[test]
fn s1_no_indicator_window_after_build_app_or_before_the_first_recording_state() {
    // Acceptance 1 (created at the first Record, never at app start), S1. After `build_app`, the
    // run loop's Ready and the call part's start there is no `call-indicator` window and no new
    // top-level window of the process at all; an idle state (the call part's own first publish)
    // builds none; the first `recording=true` state does. Bite: a window built at start (eager
    // build, hidden or not), a build on any state including idle, a build that ignores
    // `recording`.
    let _serial = serial();
    let _watchdog =
        watchdog("s1_no_indicator_window_after_build_app_or_before_the_first_recording_state");
    let dir = TempDir::new();
    let rig = wry_rig(dir.path(), Page { drag: false });
    let report = run_with_driver(rig, move |handle, _service| {
        let before = all_windows();
        let none_at_start = handle.get_webview_window(LABEL).is_none();
        let indicator = ShellCallIndicator::new(&handle);
        indicator.set_state(&idle_state());
        let idle_receipt = call_indicator::settled(&handle, RECEIPT_HANG);
        let none_after_idle = handle.get_webview_window(LABEL).is_none();
        let none_held = holds_for(SETTLE, || handle.get_webview_window(LABEL).is_none());
        let census_unchanged = all_windows() == before;
        indicator.set_state(&recording_state());
        let after = shown(&handle);
        (
            none_at_start,
            idle_receipt,
            none_after_idle,
            none_held,
            census_unchanged,
            after,
        )
    });
    let (none_at_start, idle_receipt, none_after_idle, none_held, census_unchanged, after) = report;
    assert!(none_at_start, "a `{LABEL}` window exists at app start");
    assert!(
        idle_receipt.is_some_and(|r| !r.window && !r.failed),
        "the thread's receipt for an idle state: {idle_receipt:?}"
    );
    assert!(none_after_idle && none_held, "an idle state built a window");
    assert!(
        census_unchanged,
        "the process gained a top-level window before any recording state"
    );
    assert!(
        after.ok(),
        "the first recording state did not show the window: {after:?}"
    );
}

// ==== S2 and S3: shape, and the show keeps the foreground =====================================

#[test]
fn s2_s3_the_indicator_has_its_shape_and_the_show_keeps_the_foreground() {
    // Acceptance 1 + 2 (shape; shown with SW_SHOWNOACTIVATE, the foreground unchanged after the
    // show), S2 and S3. With a TestWindow in front (asserted loudly), a recording state shown
    // through `ShellCallIndicator::set_state`: the window is visible, topmost, a tool window
    // without APPWINDOW, undecorated, unowned top-level, not click-through, non-activating
    // (variant N); the facts hold for SETTLE (nothing rewrites the styles or hides it); the
    // target is in front at every 5 ms sample and got no focus loss. Bite: a window built focused
    // or without `focusable(false)`, a tao `show()`/`set_focus()`/`set_always_on_top` on the
    // shown window (SW_SHOW activates), WS_EX_APPWINDOW left set, decorations kept.
    let _serial = serial();
    let _watchdog = watchdog("s2_s3_the_indicator_has_its_shape_and_the_show_keeps_the_foreground");
    let dir = TempDir::new();
    let rig = wry_rig(dir.path(), Page { drag: false });
    let report = run_with_driver(rig, move |handle, _service| {
        let front = front();
        let (_indicator, shown_state) = show_recording(&handle);
        let first = shown_state.hwnd.map(facts);
        let stable = shown_state
            .hwnd
            .zip(first)
            .map(|(raw, first)| holds_for(SETTLE, || facts(raw) == first))
            .unwrap_or(false);
        (shown_state, first, stable, front.finish())
    });
    let (shown_state, first, stable, fg) = report;
    assert!(
        shown_state.ok(),
        "no visible `{LABEL}` window: {shown_state:?}"
    );
    let first = first.expect("a window handle");
    assert_eq!(
        shape_errors(&first),
        Vec::<&str>::new(),
        "the shown indicator lacks its shape: {first:?}"
    );
    assert!(
        stable,
        "within {SETTLE:?} the indicator's facts changed: {first:?}"
    );
    assert!(fg.samples > 0, "the sampler never ran");
    assert_eq!(
        fg.away,
        Vec::<(isize, String)>::new(),
        "the foreground left the target while the indicator showed ({} samples)",
        fg.samples
    );
    assert_eq!(
        fg.focus_losses,
        Vec::<(u32, usize)>::new(),
        "the target lost the focus or activation while the indicator showed"
    );
}

// ==== S4: a click reaches the page and the foreground survives ================================

#[test]
fn s4_a_click_reaches_the_page_and_the_target_keeps_or_regains_the_foreground() {
    // Acceptance 2 (after a stop-control click the shell returns the foreground), S4. A SendInput
    // click at the indicator's centre (the top-level window there is the indicator, asserted):
    // the page turns its pixel (the click reached WebView2), and the target is the foreground
    // window again within the hang bound and stays so for SETTLE. Under variant N it was never
    // lost (`away` empty, recorded as evidence); under F the shell restored it. Bite: a
    // click-through window (the click lands on the target, the pixel stays), a window that takes
    // the activation and never gives it back.
    s4_driver(
        "s4_a_click_reaches_the_page_and_the_target_keeps_or_regains_the_foreground",
        Page { drag: false },
    );
}

#[test]
fn s4_a_slow_first_paint_does_not_fail_the_page_premise() {
    // T-267 Acceptance 1 and 3: the page paints PAGE_RGB 1.5 s after load (the shell's receipt
    // only proves shown, not composed). The S4 driver must wait for the paint before it clicks.
    // Red on a single-sample premise: `premise: ... not painted before the click: Some((255, 255,
    // 255))`. Bite: a premise that samples once, or a fixed sleep shorter than the delay.
    s4_driver(
        "s4_a_slow_first_paint_does_not_fail_the_page_premise",
        LatePage {
            paint_after: Some(Duration::from_millis(1500)),
        },
    );
}

#[test]
#[should_panic(expected = "premise: the indicator's page is not painted before the click")]
fn s4_a_page_that_never_paints_fails_with_the_premise_not_the_watchdog() {
    // T-267 Acceptance 2: the page stays white. The S4 driver ends with the premise message after
    // the poll's HANG bound (120 s), not by the 240 s watchdog (exit code 3, which would not be
    // this panic) and not by a later message (`the click did not reach the page`). This passes on
    // a single sample too (it fails at once); it bites a poll that skips the premise, swallows
    // its failure, or runs the click and later waits (3 x 120 s) after it failed.
    s4_driver(
        "s4_a_page_that_never_paints_fails_with_the_premise_not_the_watchdog",
        LatePage { paint_after: None },
    );
}

/// The S4 driver and its verdict over `page`: one click at the indicator's centre.
#[track_caller]
fn s4_driver(name: &'static str, page: impl Assets<Wry> + 'static) {
    let _serial = serial();
    let _watchdog = watchdog(name);
    let dir = TempDir::new();
    let rig = wry_rig(dir.path(), page);
    let report = run_with_driver(rig, move |handle, _service| {
        let front = front();
        let (_indicator, shown_state) = show_recording(&handle);
        let raw = shown_state.hwnd.expect("a shown indicator window");
        let rect = window_rect(raw).expect("the indicator's rect");
        let at = centre(&rect);
        let hit = hit_root(at);
        let before = screen_pixel(at);
        click(at);
        let painted = eventually(|| screen_pixel(at).is_some_and(|p| near(p, DOWN_RGB)));
        let regained = eventually(|| foreground().0 as isize == front.raw);
        let kept = holds_for(SETTLE, || foreground().0 as isize == front.raw);
        (raw, hit, before, painted, regained, kept, front.finish())
    });
    let (raw, hit, before, painted, regained, kept, fg) = report;
    assert_eq!(
        hit.0, raw,
        "premise: a click at the centre reaches {hit:?}, not the indicator"
    );
    assert!(
        before.is_some_and(|p| near(p, PAGE_RGB)),
        "premise: the indicator's page is not painted before the click: {before:?}"
    );
    assert!(
        painted,
        "the click did not reach the page (its mousedown pixel never showed)"
    );
    assert!(
        regained && kept,
        "after the click the target is not the foreground window (regained {regained}, kept \
         {kept}); in front during the run: {:?}",
        fg.away
    );
    eprintln!(
        "S4 evidence: variant {} lost the foreground during the click: {}",
        if VARIANT_N { "N" } else { "F" },
        !fg.away.is_empty() || !fg.focus_losses.is_empty()
    );
    if VARIANT_N {
        assert_eq!(
            fg.away,
            Vec::<(isize, String)>::new(),
            "variant N: the foreground left the target"
        );
        assert_eq!(
            fg.focus_losses,
            Vec::<(u32, usize)>::new(),
            "variant N: the target lost focus"
        );
    }
}

// ==== S5: a drag moves the window =============================================================

#[test]
fn s5_a_drag_on_the_drag_region_moves_the_window_and_the_foreground_survives() {
    // Acceptance 2 / FR-006 (dragging works), S5, with the REAL capability
    // `core:window:allow-start-dragging` of capabilities/call-indicator.json (the release context
    // grants it to the `call-indicator` label only). SendInput down / move / up on a page whose
    // body is a `data-tauri-drag-region`: the window's rect moved by about the drag (at least half
    // of it), the target is the foreground again and stays so. If the harness cannot carry the
    // drag, S5 is the owner's manual check only and the developer records why under `## Tests`.
    // Bite: no start_dragging grant (the drag IPC is refused, the rect stays), a window the
    // HTCAPTION loop cannot move, a drag that steals the foreground.
    let _serial = serial();
    let _watchdog =
        watchdog("s5_a_drag_on_the_drag_region_moves_the_window_and_the_foreground_survives");
    let dir = TempDir::new();
    let rig = wry_rig(dir.path(), Page { drag: true });
    let report = run_with_driver(rig, move |handle, _service| {
        let front = front();
        let (_indicator, shown_state) = show_recording(&handle);
        let raw = shown_state.hwnd.expect("a shown indicator window");
        let before = window_rect(raw).expect("the indicator's rect");
        let work = primary_work_area();
        // Toward the work area's middle, so the move is not stopped by an edge.
        let dx = if before.left > (work.left + work.right) / 2 {
            -160
        } else {
            160
        };
        let dy = if before.top > (work.top + work.bottom) / 2 {
            -90
        } else {
            90
        };
        drag(centre(&before), dx, dy);
        let moved = eventually(|| {
            window_rect(raw).is_some_and(|r| (r.left - before.left).abs() >= dx.abs() / 2)
        });
        let after = window_rect(raw);
        let regained = eventually(|| foreground().0 as isize == front.raw);
        let kept = holds_for(SETTLE, || foreground().0 as isize == front.raw);
        (before, after, dx, dy, moved, regained, kept, front.finish())
    });
    let (before, after, dx, dy, moved, regained, kept, fg) = report;
    assert!(
        moved,
        "the drag did not move the window: before {before:?}, after {after:?}, dragged ({dx}, {dy})"
    );
    assert!(
        regained && kept,
        "after the drag the target is not the foreground window; in front: {:?}",
        fg.away
    );
    if VARIANT_N {
        assert_eq!(
            fg.away,
            Vec::<(isize, String)>::new(),
            "variant N: the foreground left the target"
        );
    }
}

// ==== S6: census ==============================================================================

#[test]
fn s6_a_second_webview_adds_exactly_the_indicator_window_to_the_census() {
    // Acceptance 3 (no unlisted helper window), S6. Every top-level window of the process
    // (visible or not, with its class) before the build and after the indicator showed: the
    // difference is exactly the indicator's HWND, so the extra webview added no top-level helper
    // window; a difference means the evidence plus a `scripts/ci/helper-windows.txt` decision go
    // under `## Tests` (and the task becomes `checker: true`). Bite: an indicator build that adds
    // a second top-level window (a helper, an owner window).
    let _serial = serial();
    let _watchdog = watchdog("s6_a_second_webview_adds_exactly_the_indicator_window_to_the_census");
    let dir = TempDir::new();
    let rig = wry_rig(dir.path(), Page { drag: false });
    let report = run_with_driver(rig, move |handle, _service| {
        let before = all_windows();
        let (_indicator, shown_state) = show_recording(&handle);
        let settled = holds_for(SETTLE, || shown_state.hwnd.is_some_and(is_window));
        (before, all_windows(), shown_state, settled)
    });
    let (before, after, shown_state, settled) = report;
    assert!(
        shown_state.ok() && settled,
        "premise: the indicator showed: {shown_state:?}"
    );
    let raw = shown_state.hwnd.expect("handle");
    let added: Vec<&(isize, String)> = after.iter().filter(|w| !before.contains(w)).collect();
    let removed: Vec<&(isize, String)> = before.iter().filter(|w| !after.contains(w)).collect();
    assert_eq!(
        added.iter().map(|(h, _)| *h).collect::<Vec<_>>(),
        vec![raw],
        "the census gained windows other than the indicator ({added:?}); removed {removed:?}"
    );
}

// ==== S7: the fallback decision ===============================================================

#[test]
fn s7_the_foreground_contract_holds_in_one_variant_or_the_owner_decides() {
    // Acceptance 4 (failure branch: Tauri cannot keep the focus -> stop and record the
    // native-window fallback for the owner), S7. The variant-agnostic end state of S3-S5 in one
    // run: show, click, drag, stop; the foreground window is the target at the end, the click
    // reached the page, the window moved. This test stays red when neither variant N nor F
    // gets there; the red run's printed evidence (`S7 evidence` lines, the sampler's `away`) is
    // what goes to the owner with the native Win32 window question (#235). It does not pass by
    // weakening S3-S5. Bite: any implementation that lets the foreground end elsewhere.
    // The drag waits out the OS double-click time after the click (same spot, same window): a
    // mousedown inside that time arrives with `detail == 2`, and tauri 2.12.1's drag.js (l. 78-104)
    // then sends `internal_toggle_maximize` (a no-op on a non-resizable window) instead of
    // `start_dragging`, so the window does not move (CI run 44446192344, `moved false`). Bite:
    // removing the wait reproduces `moved false`.
    let _serial = serial();
    let _watchdog =
        watchdog("s7_the_foreground_contract_holds_in_one_variant_or_the_owner_decides");
    let dir = TempDir::new();
    let rig = wry_rig(dir.path(), Page { drag: true });
    let report = run_with_driver(rig, move |handle, _service| {
        let front = front();
        let (indicator, shown_state) = show_recording(&handle);
        let raw = shown_state.hwnd.expect("a shown indicator window");
        let before = window_rect(raw).expect("rect");
        click(centre(&before));
        let after_click = eventually(|| foreground().0 as isize == front.raw);
        // SAFETY: a plain query of the system setting, no arguments.
        let double_click = unsafe { GetDoubleClickTime() };
        thread::sleep(Duration::from_millis(u64::from(double_click) + 200));
        let work = primary_work_area();
        // Toward the work area's middle, so the move is not stopped by an edge (as in S5).
        let dx = if before.left > (work.left + work.right) / 2 {
            -160
        } else {
            160
        };
        let dy = if before.top > (work.top + work.bottom) / 2 {
            -90
        } else {
            90
        };
        drag(centre(&before), dx, dy);
        let moved = eventually(|| {
            window_rect(raw).is_some_and(|r| (r.left - before.left).abs() >= dx.abs() / 2)
        });
        let after = window_rect(raw);
        let after_drag = eventually(|| foreground().0 as isize == front.raw);
        indicator.set_state(&idle_state());
        let gone = eventually(|| handle.get_webview_window(LABEL).is_none());
        let end = holds_for(SETTLE, || foreground().0 as isize == front.raw);
        (
            after_click,
            moved,
            after_drag,
            gone,
            end,
            front.finish(),
            before,
            after,
            dx,
            dy,
        )
    });
    let (after_click, moved, after_drag, gone, end, fg, before, after, dx, dy) = report;
    eprintln!(
        "S7 evidence: variant {}, away {:?}, focus losses {:?}, samples {}",
        if VARIANT_N { "N" } else { "F" },
        fg.away,
        fg.focus_losses,
        fg.samples
    );
    assert!(
        after_click && after_drag && end && moved && gone,
        "neither variant keeps the foreground: foreground after click {after_click}, after drag \
         {after_drag}, at the end {end}, window moved {moved}, destroyed at stop {gone}; \
         rect before {before:?}, after {after:?}, dragged ({dx}, {dy}); \
         away {:?}. Record this evidence and ask the owner about a native window (#235)",
        fg.away
    );
}

// ==== S8: build-to-shown time (logged, not asserted) ==========================================

#[test]
fn s8_the_build_to_shown_time_is_logged_and_the_window_is_shown() {
    // S8: `shown` logs the receipt time of the first state (`call indicator: receipt .. after
    // ..`), which feeds the conditional owner question (> 1 s: NFR-03 vs US1 #1). Nothing here
    // asserts a time (T-201: a `Took` has no ceiling); the test pins that the first state is shown
    // at all and that the time is read from the shell's receipt, not polled.
    let _serial = serial();
    let _watchdog = watchdog("s8_the_build_to_shown_time_is_logged_and_the_window_is_shown");
    let dir = TempDir::new();
    let rig = wry_rig(dir.path(), Page { drag: false });
    let shown_state = run_with_driver(rig, move |handle, _service| {
        let (_indicator, shown_state) = show_recording(&handle);
        shown_state
    });
    assert!(
        shown_state.ok(),
        "the first recording state is not shown: {shown_state:?}"
    );
}

// ==== lifecycle: destroy at stop, rebuild at the next start (NFR-03) ===========================

#[test]
fn the_window_is_destroyed_when_the_call_stops_and_built_again_at_the_next_start() {
    // NFR-03 over contracts/ipc.md "hidden at stop" (decision in T-130 Investigation): the first
    // recording state builds the window, an idle state destroys it (the handle is gone and tauri
    // freed the label), the next recording state builds a new one. A start right after a stop (the
    // label may still be taken) builds exactly once and ends with a window. Bite: hiding instead
    // of destroying (the handle stays), a rebuild that fails with "label already exists" (a
    // build while Destroying), no rebuild after the first call. `first.ok()` reads the visibility
    // captured by `shown()` while the window was alive, so it holds for a correct destroy; a stop
    // that leaves the window alive turns `freed && gone` red, a window never shown turns
    // `first.ok()` red (`visible` false).
    let _serial = serial();
    let _watchdog =
        watchdog("the_window_is_destroyed_when_the_call_stops_and_built_again_at_the_next_start");
    let dir = TempDir::new();
    let rig = wry_rig(dir.path(), Page { drag: false });
    let report = run_with_driver(rig, move |handle, _service| {
        let (indicator, first) = show_recording(&handle);
        let first_raw = first.hwnd;
        indicator.set_state(&idle_state());
        let freed = eventually(|| handle.get_webview_window(LABEL).is_none());
        let gone = first_raw.is_some_and(|raw| eventually(|| !is_window(raw)));
        indicator.set_state(&recording_state());
        let second = shown(&handle);
        // Stop and start without waiting in between.
        indicator.set_state(&idle_state());
        indicator.set_state(&recording_state());
        let third = shown(&handle);
        (first, freed, gone, second, third)
    });
    let (first, freed, gone, second, third) = report;
    assert!(first.ok(), "the first start did not show: {first:?}");
    assert!(
        freed && gone,
        "the stop did not destroy the window (label freed {freed}, handle gone {gone})"
    );
    assert!(
        second.ok(),
        "the next start did not build a new window: {second:?}"
    );
    assert_ne!(
        first.hwnd, second.hwnd,
        "the second call reused the first window's handle"
    );
    assert!(
        third.ok() && third.receipt.is_some_and(|r| !r.failed),
        "a start right after a stop did not end with a shown window: {third:?}"
    );
}

// ==== set_state never blocks ==================================================================

#[test]
fn set_state_and_set_tick_under_a_held_lock_return_without_waiting_for_the_main_thread() {
    // Invariant (call-session I4, the session calls the indicator under its lock): `set_state`
    // and `set_tick` only store the newest value and notify. With a lock held by the caller and
    // the main thread blocked (every window call needs it), both return, and the main thread is
    // released only afterwards; had a call waited for the window thread or the main thread, the
    // main thread's own wait would time out first. The state is then shown once the main thread
    // is free. Bite: `set_state` that builds or emits inline, waits for the receipt, or takes
    // the session lock.
    let _serial = serial();
    let _watchdog = watchdog(
        "set_state_and_set_tick_under_a_held_lock_return_without_waiting_for_the_main_thread",
    );
    let dir = TempDir::new();
    let rig = wry_rig(dir.path(), Page { drag: false });
    let report = run_with_driver(rig, move |handle, _service| {
        let indicator = ShellCallIndicator::new(&handle);
        let session_lock = Mutex::new(());
        let (release, released) = mpsc::channel::<()>();
        let (entered, entered_rx) = mpsc::channel::<()>();
        let timed_out = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&timed_out);
        handle
            .run_on_main_thread(move || {
                let _ = entered.send(());
                if released.recv_timeout(HOLD).is_err() {
                    flag.store(true, Ordering::SeqCst);
                }
            })
            .expect("a main-thread task");
        entered_rx
            .recv_timeout(HOLD)
            .expect("the main thread is held");
        {
            let _held = session_lock.lock().unwrap_or_else(PoisonError::into_inner);
            indicator.set_state(&recording_state());
            indicator.set_tick(&tick(1_000));
            indicator.set_state(&recording_state());
        }
        let _ = release.send(());
        let after = shown(&handle);
        (timed_out.load(Ordering::SeqCst), after)
    });
    let (timed_out, after) = report;
    assert!(
        !timed_out,
        "set_state / set_tick did not return until the held main thread timed out"
    );
    assert!(
        after.ok(),
        "the newest state is not shown once the main thread is free: {after:?}"
    );
}

// ==== tick rate ===============================================================================

#[test]
fn tick_is_emitted_at_most_10_hz_and_the_newest_tick_is_delivered() {
    // Acceptance 5: `indicator://tick` at most 10 per second. 300 ticks offered 2 ms apart; the
    // events the window hears are at most `10 * span + 2` (span: the instants of the first
    // offer and the last event heard, both read through `now()`; a loaded runner stretches the
    // span and so the allowance, F-031: no exact count), at least one, and the last event heard
    // after the stream ends carries the NEWEST tick (coalescing to the newest, not the oldest).
    // Bite: an emit per `set_tick` (300 events), a tick thread that never emits, one that
    // emits the first tick forever.
    let _serial = serial();
    let _watchdog = watchdog("tick_is_emitted_at_most_10_hz_and_the_newest_tick_is_delivered");
    let dir = TempDir::new();
    let rig = wry_rig(dir.path(), Page { drag: false });
    let report = run_with_driver(rig, move |handle, _service| {
        let (indicator, shown_state) = show_recording(&handle);
        assert!(
            shown_state.ok(),
            "premise: the indicator showed: {shown_state:?}"
        );
        let (tx, rx) = mpsc::channel::<(std::time::Instant, Value)>();
        handle.listen_any("indicator://tick", move |e| {
            let value: Value = serde_json::from_str(e.payload()).unwrap_or(Value::Null);
            let _ = tx.send((now(), value));
        });
        let start = now();
        for n in 1..=300u64 {
            indicator.set_tick(&tick(n * 100));
            thread::sleep(Duration::from_millis(2));
        }
        // The newest tick (elapsed 30000 ms) arrives once the stream ended.
        let mut heard: Vec<(std::time::Instant, Value)> = Vec::new();
        let delivered = eventually(|| {
            while let Ok(item) = rx.try_recv() {
                heard.push(item);
            }
            heard
                .last()
                .is_some_and(|(_, v)| v["elapsedMs"] == json!(30_000))
        });
        (start, heard, delivered)
    });
    let (start, heard, delivered) = report;
    assert!(
        delivered,
        "the newest tick (elapsedMs 30000) was never delivered; heard {}",
        heard.len()
    );
    let last = heard.last().map(|(t, _)| *t).unwrap_or(start);
    let span = last.duration_since(start).as_secs_f64();
    let allowed = 10.0 * span + 2.0;
    assert!(
        (heard.len() as f64) <= allowed,
        "{} tick events in {span:.2} s: more than 10 per second (allowed {allowed:.1})",
        heard.len()
    );
    assert!(!heard.is_empty(), "no tick event at all");
    // The payload is the wire shape (camelCase) of the tick offered.
    let (_, v) = heard.last().expect("an event");
    assert_eq!(v["micLevel"], json!(0.25), "{v}");
    assert_eq!(v["systemLevel"], json!(0.5), "{v}");
}

// ==== position: recorded from the window's own moves, clamped =================================

#[test]
fn position_saved_and_clamped() {
    // Acceptance 6: `calls.indicator_position` is saved and clamped to a visible monitor.
    // (1) A stored position far off every monitor is clamped when the window is built: its rect
    // lies inside the primary work area. (2) The window moved (raw SetWindowPos, as a drag ends)
    // to a spot inside the work area is recorded: the stored position becomes the window's own
    // top-left (settings snapshot, written by `SettingsService::remember_indicator_position`).
    // (3) Moved partly off-screen, the stored value is clamped inside the work area, never the raw
    // value. (4) The next call builds at the stored position. Bite: no clamp at build (a window
    // off-screen), no Moved capture (nothing stored), the raw off-screen value stored, a stored
    // position not used at the next build.
    let _serial = serial();
    let _watchdog = watchdog("position_saved_and_clamped");
    let dir = TempDir::new();
    let rig = wry_rig(dir.path(), Page { drag: false });
    rig.service
        .remember_indicator_position(IndicatorPosition {
            x: -30_000,
            y: -30_000,
        })
        .expect("seeding the stored position");
    let report = run_with_driver(rig, move |handle, service| {
        let work = primary_work_area();
        let (indicator, first) = show_recording(&handle);
        let raw = first.hwnd.expect("a shown indicator window");
        let built = window_rect(raw).expect("rect");
        let (w, h) = (built.right - built.left, built.bottom - built.top);
        let target = IndicatorPosition {
            x: work.left + 60,
            y: work.top + 70,
        };
        // SAFETY: a live window of this process; the flags keep the activation, size and z-order.
        unsafe {
            SetWindowPos(
                to_hwnd(raw),
                None,
                target.x,
                target.y,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER,
            )
        }
        .expect("move the indicator");
        let recorded = eventually(|| service.snapshot().calls.indicator_position == Some(target));
        let stored_after_move = service.snapshot().calls.indicator_position;
        // Partly off the right edge.
        // SAFETY: as above.
        unsafe {
            SetWindowPos(
                to_hwnd(raw),
                None,
                work.right - w / 2,
                work.top + 80,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER,
            )
        }
        .expect("move the indicator off-screen");
        let clamped = eventually(|| {
            service
                .snapshot()
                .calls
                .indicator_position
                .is_some_and(|p| {
                    p != target
                        && p.x >= work.left
                        && p.y >= work.top
                        && p.x + w <= work.right
                        && p.y + h <= work.bottom
                })
        });
        let stored_after_edge = service.snapshot().calls.indicator_position;
        // Stop, then start: the next window is built at the stored position.
        indicator.set_state(&idle_state());
        let gone = eventually(|| handle.get_webview_window(LABEL).is_none());
        indicator.set_state(&recording_state());
        let second = shown(&handle);
        let rebuilt = second.hwnd.and_then(window_rect);
        (
            work,
            built,
            recorded,
            stored_after_move,
            clamped,
            stored_after_edge,
            gone,
            rebuilt,
        )
    });
    let (work, built, recorded, stored_after_move, clamped, stored_after_edge, gone, rebuilt) =
        report;
    assert!(
        inside(&work, &built),
        "the window built for a stored off-screen position is not inside the work area: \
         {built:?} vs {work:?}"
    );
    assert!(
        recorded,
        "the window's own move was not stored in calls.indicator_position: {stored_after_move:?}"
    );
    assert!(
        clamped,
        "a move partly off-screen was not stored clamped inside the work area {work:?}: \
         {stored_after_edge:?}"
    );
    assert!(gone, "premise: the stop destroyed the window");
    let rebuilt = rebuilt.expect("the next call's window");
    let stored = stored_after_edge.expect("a stored position");
    assert_eq!(
        (rebuilt.left, rebuilt.top),
        (stored.x, stored.y),
        "the next window is not built at the stored position"
    );
}

// ==== commands over the call wiring ===========================================================

/// Invokes `cmd` from `window` (the mock runtime has no ACL here; `app_acl.rs` is the ACL's).
fn invoke(
    window: &tauri::WebviewWindow<MockRuntime>,
    cmd: &str,
    args: Value,
) -> Result<Value, Value> {
    get_ipc_response(
        window,
        InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: window.url().expect("window url"),
            body: InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    )
    .map(|body| body.deserialize::<Value>().expect("response is JSON"))
}

/// Every payload of `event` heard on the whole app.
fn any_payloads(app: &AppHandle<MockRuntime>, event: &str) -> Receiver<Value> {
    let (tx, rx) = mpsc::channel();
    app.listen_any(event, move |e| {
        let _ = tx.send(serde_json::from_str(e.payload()).expect("event payload is JSON"));
    });
    rx
}

#[test]
fn commands_post_stop_and_dismiss_to_the_worker() {
    // Acceptance 5 (the commands exist and act through the call wiring), over `start_calls` with
    // fakes. A recording call over 500 MB of free disk raises the low-disk warning at the start
    // (the first free-space poll is at the start, core session.rs). From the `call-indicator`
    // window the product built: `call_indicator_state` answers the mailbox's newest state (the
    // recording state with that warning); `call_indicator_dismiss {warningId}` returns `null` at
    // once and the next `indicator://state` lacks the id (the worker ran
    // `CallSession::dismiss_warning`); `call_indicator_stop` returns `null` at once and logs
    // exactly one `call_stopped reason=user` (the worker ran `session.stop(StopReason::User)`;
    // `call_requested` is the start's only). Bite: commands that call the session on the IPC
    // thread (the call blocks under its lock), a dismiss that never reaches the session (the id
    // stays), a stop mapped to another reason (`hotkey`), a `call_indicator_state` that reads
    // anything but the newest state, a stop that logs twice.
    use call_shell_support::{plain_opener, CallRig, WavSource};
    let _serial = serial();
    let source = WavSource::new();
    let rig = CallRig::started(
        source,
        FakeDisk::with_available(500_000_000),
        plain_opener(),
    );
    let handle = rig.handle().clone();
    call::request(&handle, CallRequest::Record(CallSource::Hotkey));
    assert!(
        call_shell_support::eventually(|| handle.get_webview_window(LABEL).is_some()),
        "no `{LABEL}` window after the call started"
    );
    let window = handle
        .get_webview_window(LABEL)
        .expect("the indicator window");
    let states = any_payloads(&handle, "indicator://state");

    let state = invoke(&window, "call_indicator_state", json!({})).expect("call_indicator_state");
    assert_eq!(state["recording"], json!(true), "{state}");
    let warnings = state["warnings"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        warnings.len(),
        1,
        "one low-disk warning in the state: {state}"
    );
    let id = warnings[0]["id"].as_u64().expect("a warning id");

    assert_eq!(
        invoke(
            &window,
            "call_indicator_dismiss",
            json!({ "warningId": id })
        ),
        Ok(Value::Null),
        "dismiss returns null at once"
    );
    let mut heard: Vec<Value> = Vec::new();
    let dismissed = call_shell_support::eventually(|| {
        while let Ok(v) = states.try_recv() {
            heard.push(v);
        }
        heard.last().is_some_and(|s| {
            s["warnings"]
                .as_array()
                .is_some_and(|w| w.iter().all(|x| x["id"].as_u64() != Some(id)))
        })
    });
    assert!(
        dismissed,
        "no indicator://state without warning {id} after the dismiss: {heard:?}"
    );
    assert!(rig.recording(), "a dismiss stopped the call");

    assert_eq!(
        invoke(&window, "call_indicator_stop", json!({})),
        Ok(Value::Null),
        "stop returns null at once"
    );
    assert!(
        call_shell_support::eventually(|| rig.lines(&["call_stopped", "reason=user"]).len() == 1),
        "no `call_stopped reason=user` after call_indicator_stop; log lines: {:?}",
        rig.lines(&["call_stopped"])
    );
    assert!(
        call_shell_support::holds_for_a_while(|| rig.lines(&["call_stopped"]).len() == 1),
        "more than one call_stopped line: {:?}",
        rig.lines(&["call_stopped"])
    );
    assert!(
        !rig.recording(),
        "the call still records after call_indicator_stop"
    );
    // The stop published the idle state: the window is destroyed (NFR-03).
    assert!(
        call_shell_support::eventually(|| handle.get_webview_window(LABEL).is_none()),
        "the indicator window outlived the call"
    );
}
