//! The overlay window (T-057; spec 001 FR-008, contracts/ipc.md "overlay"; why:
//! `docs/decisions/overlay.md`).
//!
//! - The dictation session publishes every overlay change once, under its lock,
//!   through `Indicator::set_overlay`. [`OverlayPart::set_overlay`] (the overlay half
//!   of `dictation::ShellIndicator`) only records the newest `(seq, state,
//!   received-at)` in the app's overlay mailbox and wakes the overlay thread. It never
//!   blocks on another thread, never touches a window or the settings and never calls
//!   the session (invariant 1).
//! - Every window operation (build, emit, destroy) runs on the one "overlay" thread,
//!   started by `assemble`'s wiring ([`start`]), which holds no lock while doing it.
//!   The thread follows core's `OverlayLifecycle`: it builds a window only when none
//!   exists and none is being destroyed, and learns "destroyed" from the window's own
//!   `Destroyed` event, which tauri delivers after it freed the label (invariant 2).
//!   Each window is tagged with the reducer's build generation and its `Destroyed`
//!   posts it; one wake hands all `Destroyed` and the newest state to `on_wake`
//!   together, so a stale `Destroyed` is ignored and the order they arrived in does
//!   not matter. tauri frees the label (and runs the app's run callback) before the
//!   window's listeners run, so each wake first asks tauri through the shared label
//!   probe (`overlay_class::probe_label`, T-224): a held, `Live` window whose label
//!   tauri no longer knows counts as destroyed in this wake, and a shown state taken
//!   in that gap builds a new window instead of emitting to the gone one; the late real
//!   `Destroyed` is then stale. The thread drops its window handle exactly when
//!   `on_wake` reports `window_gone` (the current window's `Destroyed`); it never
//!   re-derives that rule (T-172).
//! - The window is built hidden with `focused(false)` and `focusable(false)`
//!   (`WS_EX_NOACTIVATE`, `WS_EX_TOPMOST`). [`show_click_through`], on the overlay
//!   thread, then clears the overlay class's `WS_CAPTION`, writes the click-through
//!   and tool-window bits (`WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_TOOLWINDOW`,
//!   `WS_EX_APPWINDOW` cleared) on the hidden window and shows it with one
//!   `ShowWindow(SW_SHOWNOACTIVATE)`, all through the shared overlay-class helper
//!   (`overlay_class::show_no_activate_with`, T-247); it is the only place that shows
//!   the overlay (T-057, T-067), reached only through [`reveal_with`] with the
//!   window's real handle reader. A handle that cannot be read leaves the window
//!   hidden, and a failed restyle of a live window still shows it without activation;
//!   each failure writes one `overlay_failed` line (T-172; `reveal_with` and
//!   [`show_click_through_with`] are test seams with one production path each).
//!   Every reducer answer runs through the shared `overlay_class::execute`.
//!   Nothing calls `show`, `set_focus`, `set_ignore_cursor_events`
//!   or any other tao setter on it afterwards: tao rewrites `GWL_EXSTYLE` from its
//!   own flags (dropping the raw bits) and re-shows a window with `SW_SHOW` on every
//!   flag change. `Hidden` destroys it (invariant 3; NFR-03).
//! - `overlay_ready` and every emit carry `overlay_payload` of the mailbox's newest
//!   state, in the snapshot's `ui_language`, read on the overlay thread or in the
//!   command, never under the session lock (invariant 4).
//! - A failed build, emit or destroy, or a failed start of the thread, writes one
//!   `warning kind=overlay_failed` line (the OS code only, no payload or error text).
//! - After each wake's window call returned, the thread stores its receipt ([`Settled`]:
//!   the seq it settled on, whether it holds a window, whether an answer for that seq
//!   failed) unless the wake left the phase `Destroying`; [`settled`] waits on it (T-173,
//!   a test seam: no command).

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tauri::{
    AppHandle, Emitter, Manager, Runtime, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent,
};
use voicen_core::diag::{Log, LogEvent, WarningKind};
use voicen_core::overlay::{
    overlay_payload, OverlayLifecycle, OverlayPayload, WindowAction, WindowPhase,
};
use voicen_core::recording::OverlayState;
use voicen_core::settings::service::SettingsService;

use crate::diag::io_os_code;
use crate::overlay_class::{self, overlay_failed, OverlayClassThread};

/// The label of the one overlay window; `capabilities/overlay.json` is granted to it.
pub const LABEL: &str = "overlay";

/// The event that carries each new state to the overlay page (contracts/ipc.md).
pub const STATE_EVENT: &str = "overlay://state";

/// The page's app URL (`src/routes/overlay`).
const URL: &str = "overlay";

/// The window title (not shown: no decorations, no taskbar button).
const TITLE: &str = "Voicen";

/// The pill's size and its gap to the bottom of the work area, in logical pixels
/// (OQ-10 proposal: bottom-centre, 48 px above the bottom, ~320x56).
const WIDTH: f64 = 320.0;
const HEIGHT: f64 = 56.0;
const BOTTOM_GAP: f64 = 48.0;

/// One published state, numbered in publishing order.
#[derive(Debug, Clone)]
struct Entry {
    seq: u64,
    state: OverlayState,
    /// When `set_overlay` recorded it (a Recording's elapsed time counts from here).
    received: Instant,
}

/// What the overlay thread has not handled yet.
#[derive(Debug, Default)]
struct Mailbox {
    /// The newest published state; `None` before the first.
    newest: Option<Entry>,
    /// The seq of the last published state (0: none yet).
    last_seq: u64,
    /// `Destroyed` events of overlay windows not yet handed to the reducer, each the
    /// generation its window was built with (review 1 finding 3).
    destroyed: Vec<u64>,
    /// Something changed since the thread last looked.
    woken: bool,
    /// The overlay thread's receipt (T-173): what it settled on after its last handled
    /// wake that did not leave the phase `Destroying` (seq 0: nothing handled yet).
    settled: Settled,
}

/// The overlay mailbox of one app: a mutex held only to copy or store an entry,
/// never across a window operation, the condvar that wakes the thread and the one
/// that wakes [`settled`] callers.
#[derive(Default)]
struct Shared {
    mailbox: Mutex<Mailbox>,
    wake: Condvar,
    receipt: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Mailbox> {
        self.mailbox.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records `state` as the newest, numbered one past the last, and wakes the
    /// thread. Returns at once.
    fn publish(&self, state: &OverlayState) {
        {
            let mut mailbox = self.lock();
            mailbox.last_seq = mailbox.last_seq.wrapping_add(1);
            mailbox.newest = Some(Entry {
                seq: mailbox.last_seq,
                state: state.clone(),
                received: Instant::now(),
            });
            mailbox.woken = true;
        }
        self.wake.notify_one();
    }

    /// Records one `Destroyed` of the overlay window built with `generation` and
    /// wakes the thread.
    fn destroyed(&self, generation: u64) {
        {
            let mut mailbox = self.lock();
            mailbox.destroyed.push(generation);
            mailbox.woken = true;
        }
        self.wake.notify_one();
    }

    /// Waits until woken; returns the generations of the `Destroyed` events since
    /// the last call and a copy of the newest state.
    fn wait(&self) -> (Vec<u64>, Option<Entry>) {
        let mut mailbox = self.lock();
        while !mailbox.woken {
            mailbox = self
                .wake
                .wait(mailbox)
                .unwrap_or_else(PoisonError::into_inner);
        }
        mailbox.woken = false;
        let destroyed = std::mem::take(&mut mailbox.destroyed);
        (destroyed, mailbox.newest.clone())
    }

    /// A copy of the newest state.
    fn newest(&self) -> Option<Entry> {
        self.lock().newest.clone()
    }

    /// Stores the overlay thread's receipt and wakes every [`settled`] caller. Called by
    /// the overlay thread only, after a window call returned (never across one).
    fn settle(&self, settled: Settled) {
        self.lock().settled = settled;
        self.receipt.notify_all();
    }

    /// Waits until the receipt covers the newest seq published when called, or `hang`
    /// passed (`None`). Never wakes the overlay thread and never touches a window.
    fn wait_settled(&self, hang: Duration) -> Option<Settled> {
        let mailbox = self.lock();
        let target = mailbox.last_seq;
        let (mailbox, _) = self
            .receipt
            .wait_timeout_while(mailbox, hang, |m| m.settled.seq < target)
            .unwrap_or_else(PoisonError::into_inner);
        (mailbox.settled.seq >= target).then_some(mailbox.settled)
    }
}

/// The managed overlay mailbox of an app (managed by [`start`]; read by
/// [`overlay_ready`] and [`part`]).
pub struct Overlay(Arc<Shared>);

/// The overlay half of the core `Indicator` port (`dictation::ShellIndicator`
/// forwards `set_overlay` here).
#[derive(Clone)]
pub struct OverlayPart {
    shared: Arc<Shared>,
}

impl OverlayPart {
    /// Records `state` as the newest overlay state (numbered one past the last) and
    /// wakes the overlay thread. Returns at once: it never waits for the overlay
    /// thread or the main thread, never touches a window or the settings and never
    /// calls into the session (dictation-session.md: called under the session lock).
    pub fn set_overlay(&self, state: &OverlayState) {
        self.shared.publish(state);
    }
}

/// The overlay part of `app`; `None` when `assemble` did not wire the app.
pub fn part<R: Runtime>(app: &AppHandle<R>) -> Option<OverlayPart> {
    app.try_state::<Overlay>().map(|overlay| OverlayPart {
        shared: Arc::clone(&overlay.0),
    })
}

/// What the overlay thread settled on (T-173), stored by the thread after the window
/// call of a wake returned, only when that wake did not leave the phase `Destroying`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Settled {
    /// The newest seq the thread had been handed when it settled (0: none yet).
    pub seq: u64,
    /// The thread holds an overlay window it built (a window labelled `overlay` that
    /// the thread did not build is not counted).
    pub window: bool,
    /// A reducer answer for this seq failed: a build, emit or destroy that returned an
    /// error or panicked (each writes one `overlay_failed` line). A failed first emit
    /// or show inside a build that returned the window writes its line but does not
    /// set this.
    pub failed: bool,
}

/// The overlay receipt (T-173; a test seam like `settings_window::request_with`, no
/// command): waits, without polling, until the overlay thread has settled on a seq at
/// least the newest one published when this is called (handled it and is not
/// `Destroying`), and returns what it settled on. `None` when `hang` passed first (the
/// thread is still in a window call, hung), or at once when `assemble` did not wire
/// the app. Never wakes the overlay thread and never holds the mailbox across a
/// window call; `set_overlay` never waits on it.
pub fn settled<R: Runtime>(app: &AppHandle<R>, hang: Duration) -> Option<Settled> {
    let shared = Arc::clone(&app.try_state::<Overlay>()?.0);
    shared.wait_settled(hang)
}

/// The payload of `entry` in the current settings' UI language (`Hidden` with seq 0
/// before anything was published).
fn payload(entry: Option<&Entry>, service: &SettingsService) -> OverlayPayload {
    let lang = service.snapshot().ui_language;
    match entry {
        Some(entry) => overlay_payload(&entry.state, lang, entry.seq, entry.received.elapsed()),
        None => overlay_payload(&OverlayState::Hidden, lang, 0, Default::default()),
    }
}

/// The overlay page's catch-up (contracts/ipc.md `overlay_ready`): the payload of the
/// mailbox's newest state, the one the last `overlay://state` emit carried or a newer
/// one. Reads the mailbox only; no window call.
#[tauri::command]
pub fn overlay_ready(
    overlay: State<'_, Overlay>,
    service: State<'_, Arc<SettingsService>>,
) -> OverlayPayload {
    payload(overlay.0.newest().as_ref(), &service)
}

/// Starts the one overlay thread ("overlay") of `app` and manages its mailbox. Called
/// once, by `assemble`'s wiring, after tauri's `build()`. A failed spawn writes one
/// `overlay_failed` warning (the OS code only); the mailbox stays managed, so
/// `set_overlay` and `overlay_ready` still answer, but no window is ever shown in
/// this run.
pub(crate) fn start<R: Runtime>(app: &AppHandle<R>, service: Arc<SettingsService>, log: Arc<Log>) {
    let shared = Arc::new(Shared::default());
    app.manage(Overlay(Arc::clone(&shared)));
    let thread_app = app.clone();
    let thread_log = Arc::clone(&log);
    let spawned = std::thread::Builder::new()
        .name("overlay".into())
        .spawn(move || run(thread_app, shared, service, thread_log));
    if let Err(err) = spawned {
        log.write(LogEvent::Warning {
            kind: WarningKind::OverlayFailed,
            os_code: err.raw_os_error(),
        });
    }
}

/// The overlay thread: waits for a change, hands every `Destroyed` (with its
/// window's generation) and the newest state to the reducer in one wake and runs
/// the one answer. Never ends (the process ends it).
fn run<R: Runtime>(
    app: AppHandle<R>,
    shared: Arc<Shared>,
    service: Arc<SettingsService>,
    log: Arc<Log>,
) {
    let mut thread = OverlayThread {
        app,
        shared,
        service,
        log,
        lifecycle: OverlayLifecycle::new(),
        current: None,
        window: None,
    };
    // The receipt's failure flag of the seq last handled: a failed answer for a seq
    // stays reported through later wakes of the same seq.
    let mut handled = Settled::default();
    loop {
        let (mut destroyed, newest) = thread.shared.wait();
        // The label probe (T-224): a window tauri already freed counts as destroyed in
        // this wake, before its own Destroyed listener posted.
        overlay_class::probe_label(&mut thread, &mut destroyed);
        let wake = thread
            .lifecycle
            .on_wake(&destroyed, newest.as_ref().map(|e| (e.seq, &e.state)));
        // The current window's Destroyed (requested, or the window went by itself,
        // e.g. Alt+F4 or exit): its handle is stale. Only the reducer decides which
        // Destroyed is the current window's (T-172); a stale one leaves the handle.
        if wake.window_gone {
            thread.window = None;
        }
        // The reducer keeps only the newest seq it was given; `current` is that
        // entry, so a `Build(seq)` (now or after a later Destroyed) renders it.
        if let Some(entry) = newest {
            if thread.current.as_ref().is_none_or(|c| entry.seq > c.seq) {
                thread.current = Some(entry);
            }
        }
        let failed = thread.execute(wake.action);
        // The receipt (T-173), after the window call returned and outside it: a wake
        // that leaves the phase `Destroying` has not settled (the rebuild for a newer
        // shown state is still to come on the window's Destroyed).
        let seq = thread.current.as_ref().map_or(0, |c| c.seq);
        if seq != handled.seq {
            handled = Settled {
                seq,
                ..Settled::default()
            };
        }
        handled.failed |= failed;
        handled.window = thread.window.is_some();
        if thread.lifecycle.phase() != WindowPhase::Destroying {
            thread.shared.settle(handled);
        }
    }
}

/// The overlay thread's own state: the reducer, the entry it last saw and the live
/// window.
struct OverlayThread<R: Runtime> {
    app: AppHandle<R>,
    shared: Arc<Shared>,
    service: Arc<SettingsService>,
    log: Arc<Log>,
    lifecycle: OverlayLifecycle,
    current: Option<Entry>,
    window: Option<WebviewWindow<R>>,
}

impl<R: Runtime> OverlayClassThread<R> for OverlayThread<R> {
    const LABEL: &'static str = LABEL;

    fn app(&self) -> &AppHandle<R> {
        &self.app
    }

    fn log(&self) -> &Log {
        &self.log
    }

    fn lifecycle(&mut self) -> &mut OverlayLifecycle {
        &mut self.lifecycle
    }

    fn window(&mut self) -> &mut Option<WebviewWindow<R>> {
        &mut self.window
    }

    fn build(&mut self) -> tauri::Result<WebviewWindow<R>> {
        self.build_window()
    }

    fn emit(&self) -> tauri::Result<()> {
        self.app.emit_to(LABEL, STATE_EVENT, self.payload())
    }

    fn destroy(window: &WebviewWindow<R>) -> tauri::Result<()> {
        window.destroy()
    }

    fn post_destroyed(&self, generation: u64) {
        self.shared.destroyed(generation);
    }
}

impl<R: Runtime> OverlayThread<R> {
    fn warn(&self, os_code: Option<i32>) {
        overlay_failed(&self.log, os_code);
    }

    /// Runs one reducer answer through the overlay-class executor
    /// ([`overlay_class::execute`]) and returns whether it failed.
    fn execute(&mut self, action: WindowAction) -> bool {
        overlay_class::execute(self, action)
    }

    /// The payload of the state the reducer last saw.
    fn payload(&self) -> OverlayPayload {
        payload(self.current.as_ref(), &self.service)
    }

    /// Builds the window hidden and non-activating, registers its `Destroyed`
    /// listener (which posts the reducer's generation for this build), shows it
    /// click-through and without activation ([`show_click_through`]) and emits the
    /// current state to it (a page that is not listening yet catches up through
    /// `overlay_ready`).
    fn build_window(&self) -> tauri::Result<WebviewWindow<R>> {
        let mut builder = WebviewWindowBuilder::new(&self.app, LABEL, WebviewUrl::App(URL.into()))
            .title(TITLE)
            .inner_size(WIDTH, HEIGHT)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .resizable(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .focused(false)
            .focusable(false)
            .visible(false);
        if let Some((x, y)) = placement() {
            builder = builder.position(x, y);
        }
        let window = builder.build()?;
        let shared = Arc::clone(&self.shared);
        let generation = self.lifecycle.generation();
        window.on_window_event(move |event| {
            if let WindowEvent::Destroyed = event {
                shared.destroyed(generation);
            }
        });
        self.reveal(&window);
        // A failed first emit is written like any emit failure; the page still
        // catches up through `overlay_ready`, so the window stays.
        if let Err(err) = self.app.emit_to(LABEL, STATE_EVENT, self.payload()) {
            self.warn(io_os_code(&err));
        }
        Ok(window)
    }

    /// Shows the freshly built, hidden `window` through [`reveal_with`] with the real
    /// handle reader, the one production path to [`show_click_through`].
    #[cfg(windows)]
    fn reveal(&self, window: &WebviewWindow<R>) {
        reveal_with(window, WebviewWindow::hwnd, &self.log);
    }

    /// The shell runs only on Windows; elsewhere the window stays hidden.
    #[cfg(not(windows))]
    fn reveal(&self, _window: &WebviewWindow<R>) {}
}

/// The overlay's styles: click-through and tool window (`WS_EX_TRANSPARENT |
/// WS_EX_LAYERED | WS_EX_TOOLWINDOW` set, `WS_EX_APPWINDOW` cleared; the creation's
/// `WS_EX_NOACTIVATE | WS_EX_TOPMOST` stay), and the class's `WS_CAPTION` cleared.
#[cfg(windows)]
const CLICK_THROUGH: overlay_class::StyleSpec = {
    use windows::Win32::UI::WindowsAndMessaging::{
        WINDOW_EX_STYLE, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
    };
    overlay_class::StyleSpec::new(
        WINDOW_EX_STYLE(WS_EX_TRANSPARENT.0 | WS_EX_LAYERED.0 | WS_EX_TOOLWINDOW.0),
        WS_EX_APPWINDOW,
    )
};

/// Shows the freshly built, hidden `window` through [`show_click_through`], the one
/// place that shows the overlay, with the handle `hwnd` reads from it: the overlay's
/// form of [`overlay_class::reveal_with`] (T-172; a test seam like
/// `settings_window::request_with`: production calls it only from
/// `OverlayThread::reveal`, with `WebviewWindow::hwnd`). A handle that cannot be read
/// leaves the window hidden (no show of any kind), writes exactly one
/// `overlay_failed` line and returns `false`; otherwise it returns what
/// `show_click_through` returns (a failed style write writes its own one line there).
#[cfg(windows)]
pub fn reveal_with<R: Runtime>(
    window: &WebviewWindow<R>,
    hwnd: impl FnOnce(&WebviewWindow<R>) -> tauri::Result<windows::Win32::Foundation::HWND>,
    log: &Log,
) -> bool {
    overlay_class::reveal_with(
        window,
        hwnd,
        |hwnd| show_click_through(hwnd, log).then_some(()),
        log,
    )
    .is_some()
}

/// Makes the hidden overlay window `hwnd` click-through, keeps it out of Alt+Tab and
/// clears its caption, then shows it without activating it (T-067, T-247;
/// `docs/decisions/overlay.md` §3): [`show_click_through_with`] with the overlay-class
/// [`overlay_class::restyle`] of [`CLICK_THROUGH`], the one production path. Runs on
/// the overlay thread and makes no tao call:
///
/// 1. clears `WS_CAPTION` in `GWL_STYLE`, reads `GWL_EXSTYLE`, ORs in
///    `WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_TOOLWINDOW` and clears
///    `WS_EX_APPWINDOW` (the creation's `WS_EX_NOACTIVATE | WS_EX_TOPMOST` stay);
/// 2. writes them with `SetWindowLongPtrW`, the last error cleared before and read
///    after (a return of 0 is otherwise ambiguous);
/// 3. applies them with `SetWindowPos(SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE |
///    SWP_NOSIZE | SWP_NOZORDER)`;
/// 4. shows it with `ShowWindow(SW_SHOWNOACTIVATE)`.
///
/// Returns `true` when every step succeeded. On the first failure it writes exactly
/// one `warning kind=overlay_failed os_code=N` (the OS code only, no handle or
/// text), still shows the window with `SW_SHOWNOACTIVATE` when `hwnd` is a window
/// (never with an activating show) and returns `false`.
#[cfg(windows)]
pub fn show_click_through(hwnd: windows::Win32::Foundation::HWND, log: &Log) -> bool {
    show_click_through_with(
        hwnd,
        |hwnd| overlay_class::restyle(hwnd, &CLICK_THROUGH),
        log,
    )
}

/// [`show_click_through`] with steps 1-3 done by `restyle` (T-172; a test seam:
/// production calls it only from `show_click_through`, with the real restyle): the
/// overlay's form of [`overlay_class::show_no_activate_with`], with no pre-show
/// placement (the overlay's position is set at build). Calls `restyle` once, then
/// step 4: shows `hwnd` with one `ShowWindow(SW_SHOWNOACTIVATE)` when it names a
/// window, whether or not the restyle succeeded (never with an activating show).
/// Returns `true` when the restyle succeeded; on its `Err` writes exactly one
/// `warning kind=overlay_failed os_code=N` and returns `false`.
#[cfg(windows)]
pub fn show_click_through_with(
    hwnd: windows::Win32::Foundation::HWND,
    restyle: impl FnOnce(windows::Win32::Foundation::HWND) -> Result<(), Option<i32>>,
    log: &Log,
) -> bool {
    overlay_class::show_no_activate_with(hwnd, restyle, |_| Ok(()), log).is_some()
}

/// The window's logical top-left: bottom-centre of the work area of the monitor
/// that holds the centre of the window in front (the primary monitor when nothing
/// is in front), `BOTTOM_GAP` above its bottom (OQ-10 proposal). Raw Win32 reads
/// only, no main-thread round trip; `None` when the monitor cannot be read (tao
/// then picks the position).
#[cfg(windows)]
fn placement() -> Option<(f64, f64)> {
    use windows::Win32::Foundation::{POINT, RECT};
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
    };
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};

    // SAFETY: no arguments.
    let front = unsafe { GetForegroundWindow() };
    let mut rect = RECT::default();
    // SAFETY: `rect` is writable; a stale handle gives an error.
    let centre = if !front.is_invalid() && unsafe { GetWindowRect(front, &mut rect) }.is_ok() {
        POINT {
            x: rect.left / 2 + rect.right / 2,
            y: rect.top / 2 + rect.bottom / 2,
        }
    } else {
        POINT { x: 0, y: 0 }
    };
    // SAFETY: any point; the flag makes the result a valid monitor.
    let monitor = unsafe { MonitorFromPoint(centre, MONITOR_DEFAULTTOPRIMARY) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is writable and its `cbSize` is set.
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return None;
    }
    let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
    // SAFETY: both outputs are writable locals.
    let dpi = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    let scale = if dpi.is_ok() && dpi_x > 0 {
        f64::from(dpi_x) / 96.0
    } else {
        1.0
    };
    let work = info.rcWork;
    let left = f64::from(work.left) / scale;
    let width = f64::from(work.right - work.left) / scale;
    let bottom = f64::from(work.bottom) / scale;
    Some((left + (width - WIDTH) / 2.0, bottom - BOTTOM_GAP - HEIGHT))
}

#[cfg(not(windows))]
fn placement() -> Option<(f64, f64)> {
    None
}
