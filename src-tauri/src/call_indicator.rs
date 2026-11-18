//! The call indicator window (T-130; spec 007 FR-005, FR-006, contracts/ipc.md
//! "call-indicator"; research R12, ADR-007 #235; why: `docs/decisions/overlay.md`
//! "Call indicator").
//!
//! - The call session publishes every indicator change under its lock through
//!   `CallIndicator::set_state` / `set_tick` (`call::ShellCallIndicator`).
//!   [`IndicatorPart::set_state`] and [`IndicatorPart::set_tick`] only store the newest
//!   value in the app's indicator mailbox and wake the `call-indicator` thread: they
//!   never block on another thread, never touch a window or the settings and never call
//!   the session.
//! - Every window operation (build, raw show, emit, destroy) runs on the one
//!   `call-indicator` thread ([`start`], called by `call::start_calls` before the
//!   session exists), which never takes the session lock. It follows core's
//!   `OverlayLifecycle` (the dictation overlay's reducer, its shown/hidden input:
//!   `visible` = the newest `IndicatorState.recording`): the window is built at the first
//!   recording state, never at app start, destroyed when recording ends (NFR-03), and
//!   built again at the next call; "destroyed" comes from the window's own `Destroyed`
//!   event, tagged with the reducer's generation.
//! - The window is built hidden, undecorated, topmost, without a taskbar button and
//!   non-activating (`focusable(false)`: `WS_EX_NOACTIVATE`, variant N of the spike). On
//!   Windows the thread then writes the tool-window bits (`WS_EX_TOOLWINDOW` set,
//!   `WS_EX_APPWINDOW` cleared), moves the hidden window to its placement with a raw
//!   `SetWindowPos(SWP_NOACTIVATE)` and shows it once with `ShowWindow(SW_SHOWNOACTIVATE)`
//!   ([`show_indicator`]). It is not click-through (Stop, Dismiss and the drag region take
//!   clicks). No tao setter touches it afterwards (overlay.md §3).
//! - Placement: core's `call::indicator::place_indicator` over the Win32 monitor work
//!   areas (physical pixels, the primary first) and the stored `calls.indicator_position`.
//!   The window's own `Moved` events are debounced to the drag's end; the window's
//!   top-left, clamped by `place_indicator`, is then stored with
//!   `SettingsService::remember_indicator_position` (the page sends no position).
//! - Ticks are coalesced to the newest and emitted at most every [`TICK_INTERVAL`]
//!   (10 Hz). `indicator://state` and `call_indicator_state` carry the mailbox's newest
//!   state.
//! - The commands: `call_indicator_state` reads the mailbox; `call_indicator_stop` and
//!   `call_indicator_dismiss` post `CallRequest::IndicatorStop` / `CallRequest::Dismiss`
//!   to the `call-requests` worker and return at once.
//! - A failed build, show, emit, destroy or position write, or a failed start of the
//!   thread, writes one `warning kind=overlay_failed` line (the OS code only, no state,
//!   transcript or error text): the indicator is the second overlay-class window.
//! - After each wake's window call returned, the thread stores its receipt ([`Settled`])
//!   unless the wake left the phase `Destroying`; [`settled`] waits on it (a test seam).

use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, Runtime, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};
use voicen_core::call::indicator::{IndicatorState, IndicatorTick};
use voicen_core::diag::{Log, LogEvent, WarningKind};
use voicen_core::i18n::UiLanguage;
use voicen_core::overlay::{OverlayLifecycle, WindowAction, WindowPhase};
use voicen_core::settings::service::SettingsService;
use voicen_core::settings::IndicatorPosition;

use crate::call::{self, CallRequest};
use crate::diag::io_os_code;

/// The label of the one indicator window; `capabilities/call-indicator.json` is granted
/// to it only.
pub const LABEL: &str = "call-indicator";

/// The event that carries each new state to the indicator page (contracts/ipc.md).
pub const STATE_EVENT: &str = "indicator://state";

/// The event that carries the live levels to the indicator page (contracts/ipc.md).
pub const TICK_EVENT: &str = "indicator://tick";

/// The shortest gap between two `indicator://tick` emits: at most 10 per second.
pub const TICK_INTERVAL: Duration = Duration::from_millis(100);

/// How long the window must stay still after a `Moved` before its position is stored
/// (the end of a drag).
const MOVE_SETTLE: Duration = Duration::from_millis(300);

/// The page's app URL (`src/routes/call-indicator`).
const URL: &str = "call-indicator";

/// The window title (not shown: no decorations, no taskbar button).
const TITLE: &str = "Voicen";

/// The window's size in logical pixels.
const WIDTH: f64 = 360.0;
const HEIGHT: f64 = 220.0;

/// What the `call-indicator` thread has not handled yet.
#[derive(Debug, Default)]
struct Mailbox {
    /// The newest published state with its seq; `None` before the first.
    newest: Option<(u64, IndicatorState)>,
    /// The seq of the last published state (0: none yet).
    last_seq: u64,
    /// The newest tick not yet emitted.
    tick: Option<IndicatorTick>,
    /// `Destroyed` events not yet handed to the reducer, each its window's generation.
    destroyed: Vec<u64>,
    /// The last `Moved` of a window not yet handled: its generation, the position and
    /// when it came.
    moved: Option<(u64, PhysicalPosition<i32>, Instant)>,
    /// Something changed since the thread last looked.
    woken: bool,
    /// The thread's receipt: what it settled on after its last handled wake that did not
    /// leave the phase `Destroying` (seq 0: nothing handled yet).
    settled: Settled,
}

/// The indicator mailbox of one app: a mutex held only to copy or store a value, never
/// across a window operation, the condvar that wakes the thread and the one that wakes
/// [`settled`] callers.
#[derive(Default)]
struct Shared {
    mailbox: Mutex<Mailbox>,
    wake: Condvar,
    receipt: Condvar,
}

/// One wake's input to the thread.
struct Wake {
    destroyed: Vec<u64>,
    newest: Option<(u64, IndicatorState)>,
    tick: Option<IndicatorTick>,
    moved: Option<(u64, PhysicalPosition<i32>)>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Mailbox> {
        self.mailbox.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Stores into the mailbox under its lock, marks it woken and wakes the thread.
    fn post(&self, store: impl FnOnce(&mut Mailbox)) {
        {
            let mut mailbox = self.lock();
            store(&mut mailbox);
            mailbox.woken = true;
        }
        self.wake.notify_one();
    }

    fn newest(&self) -> Option<IndicatorState> {
        self.lock().newest.as_ref().map(|(_, state)| state.clone())
    }

    /// Waits until woken or until a held tick or move falls due; `live` says whether a
    /// tick may be emitted now (a live window), `last_tick` when the last one went out.
    fn wait(&self, live: bool, last_tick: Option<Instant>) -> Wake {
        let mut mailbox = self.lock();
        loop {
            let now = Instant::now();
            let tick_due = (live && mailbox.tick.is_some())
                .then(|| last_tick.map_or(now, |at| at + TICK_INTERVAL));
            let move_due = mailbox.moved.map(|(_, _, at)| at + MOVE_SETTLE);
            let due = match (tick_due, move_due) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            if mailbox.woken || due.is_some_and(|at| at <= now) {
                break;
            }
            mailbox = match due {
                None => self
                    .wake
                    .wait(mailbox)
                    .unwrap_or_else(PoisonError::into_inner),
                Some(at) => {
                    self.wake
                        .wait_timeout(mailbox, at - now)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
            };
        }
        mailbox.woken = false;
        let now = Instant::now();
        let tick_ready = live && last_tick.is_none_or(|at| at + TICK_INTERVAL <= now);
        let move_ready = mailbox
            .moved
            .is_some_and(|(_, _, at)| at + MOVE_SETTLE <= now);
        Wake {
            destroyed: std::mem::take(&mut mailbox.destroyed),
            newest: mailbox.newest.clone(),
            tick: if tick_ready {
                mailbox.tick.take()
            } else {
                None
            },
            moved: if move_ready {
                mailbox.moved.take().map(|(g, p, _)| (g, p))
            } else {
                None
            },
        }
    }

    /// Stores the thread's receipt and wakes every [`settled`] caller.
    fn settle(&self, settled: Settled) {
        self.lock().settled = settled;
        self.receipt.notify_all();
    }

    /// Waits until the receipt covers the newest seq published when called, or `hang`
    /// passed (`None`).
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

/// The indicator half of `call::ShellCallIndicator`: the app's indicator mailbox.
pub struct IndicatorPart {
    shared: Arc<Shared>,
}

impl IndicatorPart {
    /// Records `state` as the newest (numbered one past the last) and wakes the
    /// `call-indicator` thread. Returns at once: called under the call session lock.
    pub fn set_state(&self, state: &IndicatorState) {
        self.shared.post(|mailbox| {
            mailbox.last_seq = mailbox.last_seq.wrapping_add(1);
            mailbox.newest = Some((mailbox.last_seq, state.clone()));
        });
    }

    /// Records `tick` as the newest tick (an older unsent one is dropped) and wakes the
    /// thread. Returns at once: called under the call session lock.
    pub fn set_tick(&self, tick: &IndicatorTick) {
        self.shared
            .post(|mailbox| mailbox.tick = Some(tick.clone()));
    }
}

/// The indicator part of `app`; `None` when [`start`] did not run on it.
pub fn part<R: Runtime>(app: &AppHandle<R>) -> Option<Arc<IndicatorPart>> {
    app.try_state::<Arc<IndicatorPart>>()
        .map(|part| Arc::clone(&part))
}

/// What the `call-indicator` thread settled on, stored after the window call of a wake
/// returned, only when that wake did not leave the phase `Destroying`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Settled {
    /// The newest seq the thread had been handed when it settled (0: none yet).
    pub seq: u64,
    /// The thread holds an indicator window it built.
    pub window: bool,
    /// A reducer answer for this seq failed (a build, emit or destroy that returned an
    /// error or panicked; each writes one `overlay_failed` line).
    pub failed: bool,
}

/// The indicator receipt (a test seam, no command): waits, without polling, until the
/// thread has settled on a seq at least the newest one published when this is called,
/// and returns what it settled on. `None` when `hang` passed first, or at once when
/// [`start`] did not run on the app.
pub fn settled<R: Runtime>(app: &AppHandle<R>, hang: Duration) -> Option<Settled> {
    let part = part(app)?;
    part.shared.wait_settled(hang)
}

/// Creates and manages the indicator mailbox of `app` and starts the one
/// `call-indicator` thread; builds no window. Does nothing when the app already has the
/// part. A failed spawn writes one `overlay_failed` warning (the OS code only); the
/// mailbox stays managed, so `set_state` and `call_indicator_state` still answer, but no
/// window is ever shown in this run.
pub fn start<R: Runtime>(app: &AppHandle<R>, service: Arc<SettingsService>, log: Arc<Log>) {
    if part(app).is_some() {
        return;
    }
    let shared = Arc::new(Shared::default());
    app.manage(Arc::new(IndicatorPart {
        shared: Arc::clone(&shared),
    }));
    let thread_app = app.clone();
    let thread_log = Arc::clone(&log);
    let spawned = std::thread::Builder::new()
        .name("call-indicator".into())
        .spawn(move || run(thread_app, shared, service, thread_log));
    if let Err(err) = spawned {
        log.write(LogEvent::Warning {
            kind: WarningKind::OverlayFailed,
            os_code: err.raw_os_error(),
        });
    }
}

/// The `call-indicator` thread: waits for a change, hands every `Destroyed` and the
/// newest state to the shared reducer in one wake and runs its answer; then emits a due
/// tick and stores a settled move. Never ends (the process ends it).
fn run<R: Runtime>(
    app: AppHandle<R>,
    shared: Arc<Shared>,
    service: Arc<SettingsService>,
    log: Arc<Log>,
) {
    let mut thread = IndicatorThread {
        app,
        shared,
        service,
        log,
        lifecycle: OverlayLifecycle::new(),
        current_seq: 0,
        window: None,
        placed: None,
        last_tick: None,
    };
    let mut handled = Settled::default();
    loop {
        let live = thread.window.is_some() && thread.lifecycle.phase() == WindowPhase::Live;
        let wake = thread.shared.wait(live, thread.last_tick);
        if thread.lifecycle.phase() != WindowPhase::Absent
            && wake.destroyed.contains(&thread.lifecycle.generation())
        {
            thread.window = None;
        }
        let newest = wake.newest.as_ref().map(|(seq, s)| (*seq, s.recording));
        let action = thread.lifecycle.on_wake_visible(&wake.destroyed, newest);
        if let Some((seq, _)) = newest {
            thread.current_seq = thread.current_seq.max(seq);
        }
        let failed = thread.execute(action);
        if let Some(tick) = wake.tick {
            thread.emit_tick(&tick);
        }
        if let Some((generation, position)) = wake.moved {
            thread.moved(generation, position);
        }
        let seq = thread.current_seq;
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

/// The thread's own state: the reducer, the newest seq seen, the live window, where the
/// thread placed it and when the last tick went out.
struct IndicatorThread<R: Runtime> {
    app: AppHandle<R>,
    shared: Arc<Shared>,
    service: Arc<SettingsService>,
    log: Arc<Log>,
    lifecycle: OverlayLifecycle,
    current_seq: u64,
    window: Option<WebviewWindow<R>>,
    /// The position the thread moved the current window to (its own move is not a
    /// user's).
    placed: Option<IndicatorPosition>,
    last_tick: Option<Instant>,
}

impl<R: Runtime> IndicatorThread<R> {
    fn warn(&self, os_code: Option<i32>) {
        // The kind and the OS code only: no state, no error text (#45, FR-20).
        self.log.write(LogEvent::Warning {
            kind: WarningKind::OverlayFailed,
            os_code,
        });
    }

    /// Runs one reducer answer and returns whether it failed (each failure writes one
    /// `overlay_failed` line). A panic inside a window call (where panics unwind: tests,
    /// dev) is caught and written like a failure; release aborts.
    fn execute(&mut self, action: WindowAction) -> bool {
        match action {
            WindowAction::Nothing => false,
            WindowAction::Build(_) => {
                let built = std::panic::catch_unwind(AssertUnwindSafe(|| self.build()));
                match built {
                    Ok(Ok(window)) => {
                        self.window = Some(window);
                        false
                    }
                    Ok(Err(err)) => {
                        self.warn(io_os_code(&err));
                        self.lifecycle.on_build_failed();
                        true
                    }
                    Err(_panic) => {
                        self.warn(None);
                        self.lifecycle.on_build_failed();
                        true
                    }
                }
            }
            WindowAction::Emit(_) => {
                let emitted = std::panic::catch_unwind(AssertUnwindSafe(|| self.emit_state()));
                match emitted {
                    Ok(Ok(())) => false,
                    Ok(Err(err)) => {
                        self.warn(io_os_code(&err));
                        true
                    }
                    Err(_panic) => {
                        self.warn(None);
                        true
                    }
                }
            }
            WindowAction::Destroy => {
                let window = self.window.take();
                self.placed = None;
                let destroyed = std::panic::catch_unwind(AssertUnwindSafe(|| match &window {
                    Some(window) => window.destroy(),
                    None => Ok(()),
                }));
                let failed = match destroyed {
                    Ok(Ok(())) => None,
                    Ok(Err(err)) => Some(io_os_code(&err)),
                    Err(_panic) => Some(None),
                };
                if let Some(os_code) = failed {
                    self.warn(os_code);
                    // A window already gone may post no Destroyed: hand the reducer one
                    // for this generation so the next call can build again; one that
                    // still exists stays until its own Destroyed.
                    if self.app.get_webview_window(LABEL).is_none() {
                        let generation = self.lifecycle.generation();
                        self.shared.post(|m| m.destroyed.push(generation));
                    } else {
                        self.window = window;
                    }
                }
                failed.is_some()
            }
        }
    }

    /// The mailbox's newest state; idle in the settings language before the first.
    fn state(&self) -> IndicatorState {
        self.shared
            .newest()
            .unwrap_or_else(|| IndicatorState::idle(self.service.snapshot().ui_language))
    }

    fn emit_state(&self) -> tauri::Result<()> {
        self.app.emit_to(LABEL, STATE_EVENT, self.state())
    }

    /// Emits `tick` to a live window; a failure writes one line and drops it.
    fn emit_tick(&mut self, tick: &IndicatorTick) {
        if self.window.is_none() {
            return;
        }
        self.last_tick = Some(Instant::now());
        if let Err(err) = self.app.emit_to(LABEL, TICK_EVENT, tick) {
            self.warn(io_os_code(&err));
        }
    }

    /// Builds the window hidden and non-activating, registers its `Destroyed` and
    /// `Moved` listeners (both tagged with this build's generation), places and shows it
    /// without activation ([`IndicatorThread::reveal`]) and emits the current state (a
    /// page not listening yet catches up through `call_indicator_state`).
    fn build(&mut self) -> tauri::Result<WebviewWindow<R>> {
        let window = WebviewWindowBuilder::new(&self.app, LABEL, WebviewUrl::App(URL.into()))
            .title(TITLE)
            .inner_size(WIDTH, HEIGHT)
            .decorations(false)
            .shadow(false)
            .resizable(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .focused(false)
            .focusable(false)
            .visible(false)
            .build()?;
        let shared = Arc::clone(&self.shared);
        let generation = self.lifecycle.generation();
        window.on_window_event(move |event| match event {
            WindowEvent::Destroyed => shared.post(|m| m.destroyed.push(generation)),
            WindowEvent::Moved(position) => {
                let position = *position;
                shared.post(|m| m.moved = Some((generation, position, Instant::now())));
            }
            _ => {}
        });
        self.placed = self.reveal(&window);
        if let Err(err) = self.app.emit_to(LABEL, STATE_EVENT, self.state()) {
            self.warn(io_os_code(&err));
        }
        Ok(window)
    }

    /// Places and shows the freshly built, hidden `window` through [`show_indicator`],
    /// the one place that shows the indicator; returns where it was placed. A handle that
    /// cannot be read leaves the window hidden and writes one `overlay_failed` line.
    #[cfg(windows)]
    fn reveal(&self, window: &WebviewWindow<R>) -> Option<IndicatorPosition> {
        match window.hwnd() {
            Ok(hwnd) => {
                let stored = self.service.snapshot().calls.indicator_position;
                show_indicator(hwnd, stored, &self.log)
            }
            Err(err) => {
                self.warn(io_os_code(&err));
                None
            }
        }
    }

    /// The shell runs only on Windows; elsewhere the window stays hidden.
    #[cfg(not(windows))]
    fn reveal(&self, _window: &WebviewWindow<R>) -> Option<IndicatorPosition> {
        None
    }

    /// A settled `Moved` of the window built with `generation`: its top-left, clamped
    /// to a work area, is stored unless it is where the thread placed it. A write
    /// failure writes one line; the window stays where it is.
    fn moved(&mut self, generation: u64, position: PhysicalPosition<i32>) {
        if generation != self.lifecycle.generation() || self.window.is_none() {
            return;
        }
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let Some(clamped) = clamp_window(window, position) else {
            return;
        };
        if self.placed == Some(clamped) {
            return;
        }
        self.placed = Some(clamped);
        if self.service.remember_indicator_position(clamped).is_err() {
            self.warn(None);
        }
    }
}

/// The window's top-left after a move, clamped into a monitor work area (raw Win32 reads
/// of the window rect, so the size is the real one); `None` when the window cannot be
/// read.
#[cfg(windows)]
fn clamp_window<R: Runtime>(
    window: &WebviewWindow<R>,
    _position: PhysicalPosition<i32>,
) -> Option<IndicatorPosition> {
    use voicen_core::call::indicator::place_indicator;
    let hwnd = window.hwnd().ok()?;
    let rect = win32::window_rect(hwnd)?;
    let size = (rect.right - rect.left, rect.bottom - rect.top);
    let at = IndicatorPosition {
        x: rect.left,
        y: rect.top,
    };
    let areas = win32::work_areas();
    if areas.is_empty() {
        return None;
    }
    Some(place_indicator(Some(at), size, &areas))
}

/// The shell runs only on Windows; elsewhere the reported position is taken as is.
#[cfg(not(windows))]
fn clamp_window<R: Runtime>(
    _window: &WebviewWindow<R>,
    position: PhysicalPosition<i32>,
) -> Option<IndicatorPosition> {
    Some(IndicatorPosition {
        x: position.x,
        y: position.y,
    })
}

/// Writes the tool-window bits on the hidden indicator window `hwnd`, moves it to its
/// placement and shows it without activating it (T-130). Runs on the `call-indicator`
/// thread and makes no tao call:
///
/// 1. reads `GWL_EXSTYLE`, ORs in `WS_EX_TOOLWINDOW` and clears `WS_EX_APPWINDOW` (the
///    creation's `WS_EX_NOACTIVATE | WS_EX_TOPMOST` stay; no click-through bits);
/// 2. writes it with `SetWindowLongPtrW` and applies it with `SetWindowPos(SWP_FRAMECHANGED
///    | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER)`;
/// 3. reads the window's size and the monitor work areas, places it with core's
///    `place_indicator(stored, ..)` and moves it there with `SetWindowPos(SWP_NOACTIVATE |
///    SWP_NOSIZE | SWP_NOZORDER)`;
/// 4. shows it with `ShowWindow(SW_SHOWNOACTIVATE)`.
///
/// Returns the placement, or `None` when it could not be made. On the first failure it
/// writes exactly one `warning kind=overlay_failed os_code=N` (the OS code only) and still
/// shows the window with `SW_SHOWNOACTIVATE` when `hwnd` is a window.
#[cfg(windows)]
pub fn show_indicator(
    hwnd: windows::Win32::Foundation::HWND,
    stored: Option<IndicatorPosition>,
    log: &Log,
) -> Option<IndicatorPosition> {
    use voicen_core::call::indicator::place_indicator;
    use windows::Win32::UI::WindowsAndMessaging::{
        IsWindow, SetWindowPos, ShowWindow, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
        SW_SHOWNOACTIVATE,
    };

    /// Steps 1-3; `Err` carries the code of the first failure.
    fn prepare(
        hwnd: windows::Win32::Foundation::HWND,
        stored: Option<IndicatorPosition>,
    ) -> Result<IndicatorPosition, Option<i32>> {
        win32::restyle(hwnd)?;
        let rect = win32::window_rect(hwnd).ok_or_else(win32::last_error)?;
        let size = (rect.right - rect.left, rect.bottom - rect.top);
        let areas = win32::work_areas();
        if areas.is_empty() {
            return Err(None);
        }
        let at = place_indicator(stored, size, &areas);
        // SAFETY: any handle; no size, z-order change or activation.
        unsafe {
            SetWindowPos(
                hwnd,
                None,
                at.x,
                at.y,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER,
            )
        }
        .map_err(|err| win32::win32_code(&err))?;
        Ok(at)
    }

    let prepared = prepare(hwnd, stored);

    // SAFETY: any handle; `IsWindow` only answers whether it names a window.
    if unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        // SAFETY: `hwnd` names a window; the return is the previous visibility, not an
        // error. `SW_SHOWNOACTIVATE` never activates it.
        let _was_visible = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    }

    match prepared {
        Ok(at) => Some(at),
        Err(os_code) => {
            // The kind and the OS code only: no handle, no error text (#45, FR-20).
            log.write(LogEvent::Warning {
                kind: WarningKind::OverlayFailed,
                os_code,
            });
            None
        }
    }
}

/// Raw Win32 reads and writes of the indicator thread (no tao call, no main-thread
/// round trip of tauri's).
#[cfg(windows)]
mod win32 {
    use voicen_core::call::indicator::WorkArea;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{GetLastError, SetLastError, HWND, LPARAM, RECT, WIN32_ERROR};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, GetWindowRect, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE,
        MONITORINFOF_PRIMARY, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        SWP_NOZORDER, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
    };

    /// The calling thread's last error as a log code; `None` when it is 0.
    pub(super) fn last_error() -> Option<i32> {
        // SAFETY: reads the calling thread's last-error value.
        let WIN32_ERROR(code) = unsafe { GetLastError() };
        i32::try_from(code).ok().filter(|code| *code != 0)
    }

    /// The Win32 code inside a `windows` error (an `HRESULT_FROM_WIN32`); `None` for any
    /// other `HRESULT`.
    pub(super) fn win32_code(err: &windows::core::Error) -> Option<i32> {
        let hresult = err.code().0 as u32;
        ((hresult >> 16) & 0x1FFF == 7)
            .then_some((hresult & 0xFFFF) as i32)
            .filter(|code| *code != 0)
    }

    /// `WS_EX_TOOLWINDOW` set and `WS_EX_APPWINDOW` cleared, applied with a frame change
    /// that neither moves, sizes, reorders nor activates the window.
    pub(super) fn restyle(hwnd: HWND) -> Result<(), Option<i32>> {
        // SAFETY: clears the calling thread's last error.
        unsafe { SetLastError(WIN32_ERROR(0)) };
        // SAFETY: any handle; a stale one returns 0 with the last error set.
        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
        if style == 0 {
            if let Some(code) = last_error() {
                return Err(Some(code));
            }
        }
        let wanted = (style | WS_EX_TOOLWINDOW.0 as isize) & !(WS_EX_APPWINDOW.0 as isize);
        // SAFETY: clears the calling thread's last error.
        unsafe { SetLastError(WIN32_ERROR(0)) };
        // SAFETY: any handle; a failure returns 0 with the last error set.
        let previous = unsafe { SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted) };
        if previous == 0 {
            if let Some(code) = last_error() {
                return Err(Some(code));
            }
        }
        // SAFETY: any handle; no move, size, z-order change or activation.
        unsafe {
            SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
            )
        }
        .map_err(|err| win32_code(&err))
    }

    /// The window's rect in physical screen pixels; `None` for a stale handle.
    pub(super) fn window_rect(hwnd: HWND) -> Option<RECT> {
        let mut rect = RECT::default();
        // SAFETY: `rect` is a writable local; a stale handle gives an error.
        unsafe { GetWindowRect(hwnd, &mut rect) }
            .ok()
            .map(|()| rect)
    }

    /// Every monitor's work area in physical pixels, the primary first (empty when none
    /// could be read).
    pub(super) fn work_areas() -> Vec<WorkArea> {
        unsafe extern "system" fn collect(
            monitor: HMONITOR,
            _dc: HDC,
            _clip: *mut RECT,
            found: LPARAM,
        ) -> BOOL {
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            // SAFETY: `info` is writable and its `cbSize` is set.
            if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
                let work = info.rcWork;
                let area = WorkArea {
                    left: work.left,
                    top: work.top,
                    right: work.right,
                    bottom: work.bottom,
                };
                // SAFETY: `found` is the `Vec` that `work_areas` passes, alive and not
                // otherwise touched for the whole `EnumDisplayMonitors` call.
                let found = unsafe { &mut *(found.0 as *mut Vec<(bool, WorkArea)>) };
                found.push((info.dwFlags & MONITORINFOF_PRIMARY != 0, area));
            }
            BOOL::from(true)
        }
        let mut found: Vec<(bool, WorkArea)> = Vec::new();
        // SAFETY: the callback writes only to `found`, and only during this call.
        let _complete = unsafe {
            EnumDisplayMonitors(
                None,
                None,
                Some(collect),
                LPARAM(&mut found as *mut Vec<(bool, WorkArea)> as isize),
            )
        };
        // The primary first; the rest in enumeration order.
        found.sort_by_key(|(primary, _)| !*primary);
        found.into_iter().map(|(_, area)| area).collect()
    }
}

// ---- the indicator page's commands --------------------------------------------------------

/// `call_indicator_state` → `IndicatorState` (contracts/ipc.md, the page's load): the
/// mailbox's newest state, the one the last `indicator://state` carried or a newer one;
/// idle in the settings language before the first (or on an app without the part).
/// Reads the mailbox only; no window call, no session call.
#[tauri::command]
pub fn call_indicator_state<R: Runtime>(app: AppHandle<R>) -> IndicatorState {
    part(&app)
        .and_then(|part| part.shared.newest())
        .unwrap_or_else(|| {
            let lang = app
                .try_state::<Arc<SettingsService>>()
                .map_or(UiLanguage::En, |service| service.snapshot().ui_language);
            IndicatorState::idle(lang)
        })
}

/// `call_indicator_stop` → `null` (FR-005): posts `CallRequest::IndicatorStop` to the
/// `call-requests` worker (`session.stop(StopReason::User)`) and returns at once.
#[tauri::command]
pub fn call_indicator_stop<R: Runtime>(app: AppHandle<R>) {
    call::request(&app, CallRequest::IndicatorStop);
}

/// `call_indicator_dismiss {warningId}` → `null` (FR-006): posts
/// `CallRequest::Dismiss(warning_id)` to the worker (`session.dismiss_warning`) and
/// returns at once.
#[tauri::command]
pub fn call_indicator_dismiss<R: Runtime>(app: AppHandle<R>, warning_id: u32) {
    call::request(&app, CallRequest::Dismiss(warning_id));
}
