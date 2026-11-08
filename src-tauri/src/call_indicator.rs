//! The call indicator window (T-130; spec 007 FR-005, FR-006, contracts/ipc.md "call-indicator",
//! research R12; ADR-007 #235, variant N; why: `docs/decisions/overlay.md`, whose rules it
//! follows).
//!
//! - The call session publishes every indicator change under its lock, through core's
//!   `CallIndicator` port. [`IndicatorPart`] only records the newest state (numbered) or the
//!   newest tick in the indicator mailbox and wakes the `call-indicator` thread. It never
//!   blocks on another thread, never touches a window or the settings and never calls the
//!   session.
//! - Every window operation (build, show, position, emit, destroy) runs on the one
//!   `call-indicator` thread, started by `assemble`'s wiring ([`start`]), which holds no lock
//!   while doing it. The thread follows core's `OverlayLifecycle` with the shown/hidden input
//!   (`on_wake_visibility`, hidden = `recording == false`): the window exists exactly while the
//!   newest state records, is destroyed at stop (NFR-03) and rebuilt at the next start, and
//!   `Destroyed` is learned from the window's own event, tagged with its build generation.
//! - The window is built hidden with `focused(false)`, `focusable(false)` (`WS_EX_NOACTIVATE`),
//!   `always_on_top`, `decorations(false)`, `resizable(false)` and `shadow(false)`.
//!   [`show_without_activation`], on the indicator thread, then clears `WS_CAPTION` and
//!   `WS_THICKFRAME` (tao keeps the caption bit on undecorated windows), sets
//!   `WS_EX_TOOLWINDOW` (and `WS_EX_NOACTIVATE`), clears `WS_EX_APPWINDOW`, moves it to its clamped position with
//!   `SetWindowPos(SWP_FRAMECHANGED | SWP_NOACTIVATE)` and shows it with one
//!   `ShowWindow(SW_SHOWNOACTIVATE)`: the only place that shows it. No tao setter touches it
//!   afterwards (tao would rewrite the style from its own flags and re-show it). It takes
//!   clicks: no click-through bits.
//! - The position is `calls.indicator_position` (physical pixels) clamped to the present
//!   monitor work areas (`clamp_to_monitors`) at each build; with none saved, the top-right of
//!   the primary work area. [`moved`] clamps a reported position and saves it through
//!   `SettingsService::remember_indicator_position`; a failed write is logged and the window
//!   stays where it is.
//! - `indicator://state` is emitted at each build and each newer recording state;
//!   `indicator://tick` carries the newest tick only, at most every 100 ms (older ones are
//!   dropped, never queued). `call_indicator_state` and every emit carry the mailbox's newest
//!   state.
//! - `call_indicator_stop` / `call_indicator_dismiss` go to the [`IndicatorControl`] slot
//!   ([`set_control`], filled by the call wiring); an empty slot answers `null` and does
//!   nothing.
//! - A failed build, emit, show or destroy, a failed thread start, or a failed position write
//!   writes one `warning kind=overlay_failed` line (the OS code only, no payload or text).
//! - After each wake's window call returned, the thread stores its receipt ([`Settled`], the
//!   overlay's meaning) unless the wake left the phase `Destroying`; [`settled`] waits on it (a
//!   test seam: no command).

use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tauri::{
    AppHandle, Emitter, Manager, Runtime, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent,
};
use voicen_core::call::indicator::{clamp_to_monitors, IndicatorState, IndicatorTick, MonitorRect};
use voicen_core::call::session::CallIndicator;
use voicen_core::diag::{Log, LogEvent, WarningKind};
use voicen_core::overlay::{OverlayLifecycle, WindowAction, WindowPhase};
use voicen_core::settings::service::SettingsService;
use voicen_core::settings::IndicatorPosition;

use crate::diag::io_os_code;

/// The label of the one indicator window; `capabilities/call-indicator.json` is granted to it.
pub const LABEL: &str = "call-indicator";

/// The event that carries each new state to the indicator page (contracts/ipc.md).
pub const STATE_EVENT: &str = "indicator://state";

/// The event that carries the live levels (contracts/ipc.md), at most every [`TICK_EVERY`].
pub const TICK_EVENT: &str = "indicator://tick";

/// The page's app URL (`src/routes/call-indicator`).
const URL: &str = "call-indicator";

/// The window title (not shown: no decorations, no taskbar button).
const TITLE: &str = "Voicen";

/// The window's size in logical pixels and its default gap to the work area's top-right
/// corner in physical pixels (no saved position yet).
const WIDTH: f64 = 320.0;
const HEIGHT: f64 = 72.0;
const MARGIN: i32 = 24;

/// The shortest time between two `indicator://tick` emits (at most 10 per second).
const TICK_EVERY: Duration = Duration::from_millis(100);

/// The calls the indicator page's buttons reach (the call session; filled by the call
/// wiring through [`set_control`]). Called from a command, never under the session lock.
pub trait IndicatorControl: Send + Sync {
    /// The page's Stop (`call_indicator_stop`): stop the recording call.
    fn stop(&self);
    /// The page's Dismiss of warning `id` (`call_indicator_dismiss`).
    fn dismiss_warning(&self, id: u32);
}

/// One published state, numbered in publishing order.
#[derive(Debug, Clone)]
struct Entry {
    seq: u64,
    state: IndicatorState,
}

/// What the indicator thread has not handled yet.
#[derive(Debug, Default)]
struct Mailbox {
    /// The newest published state; `None` before the first.
    newest: Option<Entry>,
    /// The seq of the last published state (0: none yet).
    last_seq: u64,
    /// `Destroyed` events not yet handed to the reducer, each its window's generation.
    destroyed: Vec<u64>,
    /// A state or a `Destroyed` came since the thread last looked.
    woken: bool,
    /// The newest tick not emitted yet (older ones are replaced, never queued).
    tick: Option<IndicatorTick>,
    /// The thread's receipt (the overlay's meaning).
    settled: Settled,
}

/// What one wake hands the thread.
struct Wake {
    /// A state or a `Destroyed` came: run the reducer.
    woken: bool,
    destroyed: Vec<u64>,
    newest: Option<Entry>,
    /// A tick that is due now.
    tick: Option<IndicatorTick>,
}

/// The indicator mailbox of one app: a mutex held only to copy or store, never across a
/// window operation; the condvar that wakes the thread, the one that wakes [`settled`]
/// callers; and the control slot.
#[derive(Default)]
struct Shared {
    mailbox: Mutex<Mailbox>,
    wake: Condvar,
    receipt: Condvar,
    control: Mutex<Option<Arc<dyn IndicatorControl>>>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Mailbox> {
        self.mailbox.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records `state` as the newest, numbered one past the last, and wakes the thread.
    fn publish(&self, state: &IndicatorState) {
        {
            let mut mailbox = self.lock();
            mailbox.last_seq = mailbox.last_seq.wrapping_add(1);
            mailbox.newest = Some(Entry {
                seq: mailbox.last_seq,
                state: state.clone(),
            });
            mailbox.woken = true;
        }
        self.wake.notify_one();
    }

    /// Records `tick` as the newest (replacing one not emitted yet) and wakes the thread.
    fn tick(&self, tick: &IndicatorTick) {
        self.lock().tick = Some(tick.clone());
        self.wake.notify_one();
    }

    /// Records one `Destroyed` of the window built with `generation` and wakes the thread.
    fn destroyed(&self, generation: u64) {
        {
            let mut mailbox = self.lock();
            mailbox.destroyed.push(generation);
            mailbox.woken = true;
        }
        self.wake.notify_one();
    }

    /// Waits until a state or a `Destroyed` came, or a pending tick is due (`last_tick` +
    /// [`TICK_EVERY`], at once before the first emit).
    fn wait(&self, last_tick: Option<Instant>) -> Wake {
        let mut mailbox = self.lock();
        loop {
            let tick_at = mailbox
                .tick
                .as_ref()
                .map(|_| last_tick.map(|t| t + TICK_EVERY));
            let now = Instant::now();
            let tick_due =
                matches!(tick_at, Some(None)) || matches!(tick_at, Some(Some(at)) if at <= now);
            if mailbox.woken || tick_due {
                let woken = std::mem::take(&mut mailbox.woken);
                return Wake {
                    woken,
                    destroyed: std::mem::take(&mut mailbox.destroyed),
                    newest: if woken { mailbox.newest.clone() } else { None },
                    tick: if tick_due { mailbox.tick.take() } else { None },
                };
            }
            mailbox = match tick_at {
                Some(Some(at)) => {
                    self.wake
                        .wait_timeout(mailbox, at.saturating_duration_since(now))
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
                _ => self
                    .wake
                    .wait(mailbox)
                    .unwrap_or_else(PoisonError::into_inner),
            };
        }
    }

    /// A copy of the newest state.
    fn newest(&self) -> Option<Entry> {
        self.lock().newest.clone()
    }

    /// Stores the thread's receipt and wakes every [`settled`] caller.
    fn settle(&self, settled: Settled) {
        self.lock().settled = settled;
        self.receipt.notify_all();
    }

    /// Waits until the receipt covers the newest seq published when called, or `hang` passed.
    fn wait_settled(&self, hang: Duration) -> Option<Settled> {
        let mailbox = self.lock();
        let target = mailbox.last_seq;
        let (mailbox, _) = self
            .receipt
            .wait_timeout_while(mailbox, hang, |m| m.settled.seq < target)
            .unwrap_or_else(PoisonError::into_inner);
        (mailbox.settled.seq >= target).then_some(mailbox.settled)
    }

    /// The control in the slot, cloned out of the lock.
    fn control(&self) -> Option<Arc<dyn IndicatorControl>> {
        self.control
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The managed indicator mailbox of an app (managed by [`start`]).
pub struct Indicator(Arc<Shared>);

/// The shell side of core's `CallIndicator` port: the call wiring forwards the session's
/// calls here. Every method returns at once (called under the session lock).
#[derive(Clone)]
pub struct IndicatorPart {
    shared: Arc<Shared>,
}

impl CallIndicator for IndicatorPart {
    /// Records `state` as the newest and wakes the indicator thread.
    fn set_state(&self, state: &IndicatorState) {
        self.shared.publish(state);
    }

    /// Records `tick` as the newest tick and wakes the indicator thread.
    fn set_tick(&self, tick: &IndicatorTick) {
        self.shared.tick(tick);
    }

    /// The tray's call flag is not the window's: nothing here (the call wiring forwards it
    /// to the tray).
    fn set_call_recording(&self, _recording: bool) {}
}

/// The indicator part of `app`; `None` when `assemble` did not wire the app.
pub fn part<R: Runtime>(app: &AppHandle<R>) -> Option<IndicatorPart> {
    app.try_state::<Indicator>().map(|indicator| IndicatorPart {
        shared: Arc::clone(&indicator.0),
    })
}

/// Fills the control slot the stop and dismiss commands call (replacing any earlier one).
/// Nothing when `assemble` did not wire the app.
pub fn set_control<R: Runtime>(app: &AppHandle<R>, control: Arc<dyn IndicatorControl>) {
    if let Some(indicator) = app.try_state::<Indicator>() {
        *indicator
            .0
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(control);
    }
}

/// What the indicator thread settled on (the overlay's [`crate::overlay::Settled`] meaning).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Settled {
    /// The newest seq the thread had been handed when it settled (0: none yet).
    pub seq: u64,
    /// The thread holds an indicator window it built.
    pub window: bool,
    /// A reducer answer for this seq failed (a build, emit or destroy that returned an
    /// error or panicked).
    pub failed: bool,
}

/// The indicator receipt (a test seam, no command): waits, without polling, until the
/// thread has settled on a seq at least the newest one published when called, and returns
/// it. `None` when `hang` passed first, or at once when `assemble` did not wire the app.
pub fn settled<R: Runtime>(app: &AppHandle<R>, hang: Duration) -> Option<Settled> {
    let shared = Arc::clone(&app.try_state::<Indicator>()?.0);
    shared.wait_settled(hang)
}

/// The function behind `call_indicator_moved`: the reported top-left `(x, y)` (physical
/// pixels) clamped to the present monitors for the window's size, then saved through
/// `remember_indicator_position`. A failed write writes one `overlay_failed` line; the
/// window is not moved.
pub fn moved<R: Runtime>(app: &AppHandle<R>, x: i32, y: i32) {
    let Some(service) = app.try_state::<Arc<SettingsService>>() else {
        return;
    };
    let wanted = IndicatorPosition { x, y };
    let position = clamp_to_monitors(wanted, window_size_at(wanted), &monitors());
    if service.remember_indicator_position(position).is_err() {
        if let Some(log) = app.try_state::<Arc<Log>>() {
            log.write(LogEvent::Warning {
                kind: WarningKind::OverlayFailed,
                os_code: None,
            });
        }
    }
}

/// `call_indicator_stop` (contracts/ipc.md): the control's `stop`; `null`.
#[tauri::command(async)]
pub fn call_indicator_stop(indicator: State<'_, Indicator>) {
    if let Some(control) = indicator.0.control() {
        control.stop();
    }
}

/// `call_indicator_dismiss { id }` (contracts/ipc.md): the control's `dismiss_warning(id)`;
/// `null`.
#[tauri::command(async)]
pub fn call_indicator_dismiss(indicator: State<'_, Indicator>, id: u32) {
    if let Some(control) = indicator.0.control() {
        control.dismiss_warning(id);
    }
}

/// `call_indicator_moved { x, y }` (contracts/ipc.md): [`moved`]; `null`.
#[tauri::command(async)]
pub fn call_indicator_moved<R: Runtime>(app: AppHandle<R>, x: i32, y: i32) {
    moved(&app, x, y);
}

/// `call_indicator_state` (contracts/ipc.md): the mailbox's newest state (idle before any).
#[tauri::command]
pub fn call_indicator_state(indicator: State<'_, Indicator>) -> IndicatorState {
    indicator
        .0
        .newest()
        .map_or_else(IndicatorState::idle, |entry| entry.state)
}

/// Starts the one `call-indicator` thread of `app` and manages its mailbox. Called once, by
/// `assemble`'s wiring. No window is built here. A failed spawn writes one `overlay_failed`
/// warning; the mailbox stays managed, so the part and the commands still answer.
pub(crate) fn start<R: Runtime>(app: &AppHandle<R>, service: Arc<SettingsService>, log: Arc<Log>) {
    let shared = Arc::new(Shared::default());
    app.manage(Indicator(Arc::clone(&shared)));
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

/// The indicator thread: waits for a change or a due tick, hands every `Destroyed` and the
/// newest state to the reducer in one wake, runs the one answer, then emits a due tick to a
/// live window. Never ends (the process ends it).
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
        current: None,
        window: None,
    };
    let mut handled = Settled::default();
    let mut last_tick: Option<Instant> = None;
    loop {
        let wake = thread.shared.wait(last_tick);
        if wake.woken {
            if thread.lifecycle.phase() != WindowPhase::Absent
                && wake.destroyed.contains(&thread.lifecycle.generation())
            {
                thread.window = None;
            }
            let action = thread.lifecycle.on_wake_visibility(
                &wake.destroyed,
                wake.newest.as_ref().map(|e| (e.seq, !e.state.recording)),
            );
            if let Some(entry) = wake.newest {
                if thread.current.as_ref().is_none_or(|c| entry.seq > c.seq) {
                    thread.current = Some(entry);
                }
            }
            let failed = thread.execute(action);
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
        if let Some(tick) = wake.tick {
            // A tick with no live window (before the build, after a stop) is dropped.
            if thread.window.is_some() && thread.lifecycle.phase() == WindowPhase::Live {
                if let Err(err) = thread.app.emit_to(LABEL, TICK_EVENT, tick) {
                    thread.warn(io_os_code(&err));
                }
                last_tick = Some(Instant::now());
            }
        }
    }
}

/// The indicator thread's own state: the reducer, the entry it last saw and the live window.
struct IndicatorThread<R: Runtime> {
    app: AppHandle<R>,
    shared: Arc<Shared>,
    service: Arc<SettingsService>,
    log: Arc<Log>,
    lifecycle: OverlayLifecycle,
    current: Option<Entry>,
    window: Option<WebviewWindow<R>>,
}

impl<R: Runtime> IndicatorThread<R> {
    fn warn(&self, os_code: Option<i32>) {
        // The kind and the OS code only: no payload, no error text (#45, FR-20).
        self.log.write(LogEvent::Warning {
            kind: WarningKind::OverlayFailed,
            os_code,
        });
    }

    /// Runs one reducer answer and returns whether it failed (each failure writes one
    /// line). A panic inside a window call (where panics unwind) is caught likewise.
    fn execute(&mut self, action: WindowAction) -> bool {
        match action {
            WindowAction::Nothing => false,
            WindowAction::Build(_) => {
                match std::panic::catch_unwind(AssertUnwindSafe(|| self.build())) {
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
                match std::panic::catch_unwind(AssertUnwindSafe(|| self.emit())) {
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
                    // As the overlay (overlay.md §3, finding 3): a window already gone may
                    // post no Destroyed, so one is posted for its generation.
                    if self.app.get_webview_window(LABEL).is_none() {
                        self.shared.destroyed(self.lifecycle.generation());
                    } else {
                        self.window = window;
                    }
                }
                failed.is_some()
            }
        }
    }

    /// The state the reducer last saw (idle before any).
    fn state(&self) -> IndicatorState {
        self.current
            .as_ref()
            .map_or_else(IndicatorState::idle, |entry| entry.state.clone())
    }

    /// Builds the window hidden and non-activating, registers its `Destroyed` listener (it
    /// posts this build's generation), places and shows it without activation
    /// ([`show_without_activation`]) and emits the current state to it (a page not
    /// listening yet catches up through `call_indicator_state`).
    fn build(&self) -> tauri::Result<WebviewWindow<R>> {
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
        window.on_window_event(move |event| {
            if let WindowEvent::Destroyed = event {
                shared.destroyed(generation);
            }
        });
        self.reveal(&window);
        if let Err(err) = self.emit() {
            self.warn(io_os_code(&err));
        }
        Ok(window)
    }

    fn emit(&self) -> tauri::Result<()> {
        self.app.emit_to(LABEL, STATE_EVENT, self.state())
    }

    /// Places the freshly built, hidden `window` at the saved position (or the default),
    /// clamped to the monitors for its real size, and shows it through
    /// [`show_without_activation`]. A handle that cannot be read leaves it hidden and writes
    /// one line.
    #[cfg(windows)]
    fn reveal(&self, window: &WebviewWindow<R>) {
        match window.hwnd() {
            Ok(hwnd) => {
                let size = outer_size(hwnd)
                    .unwrap_or_else(|| window_size_at(IndicatorPosition { x: 0, y: 0 }));
                let saved = self.service.snapshot().calls.indicator_position;
                let position = saved
                    .or_else(|| default_position(size))
                    .map(|p| clamp_to_monitors(p, size, &monitors()));
                show_without_activation(hwnd, position, &self.log);
            }
            Err(err) => self.warn(io_os_code(&err)),
        }
    }

    /// The shell runs only on Windows; elsewhere the window stays hidden.
    #[cfg(not(windows))]
    fn reveal(&self, _window: &WebviewWindow<R>) {
        let _ = &self.service;
    }
}

/// Makes the hidden indicator window `hwnd` a non-activating tool window, moves it to
/// `position` (physical pixels; `None`: where it is), then shows it without activating it
/// (ADR-007 variant N). Runs on the indicator thread and makes no tao call:
///
/// 1. reads `GWL_STYLE` and clears `WS_CAPTION | WS_THICKFRAME` (tao keeps `WS_CAPTION` on
///    an undecorated top-level window and only hides the frame in `WM_NCCALCSIZE`), then
///    reads `GWL_EXSTYLE`, ORs in `WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE` and clears
///    `WS_EX_APPWINDOW` (the creation's `WS_EX_TOPMOST` stays; no click-through bits);
/// 2. writes each with `SetWindowLongPtrW`, the last error cleared before and read after;
/// 3. applies it with `SetWindowPos(SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOSIZE |
///    SWP_NOZORDER)` at `position` (`SWP_NOMOVE` without one);
/// 4. shows it with `ShowWindow(SW_SHOWNOACTIVATE)`.
///
/// Returns `true` when every step succeeded. On the first failure it writes exactly one
/// `warning kind=overlay_failed os_code=N`, still shows the window with `SW_SHOWNOACTIVATE`
/// when `hwnd` is a window (never an activating show) and returns `false`.
#[cfg(windows)]
pub fn show_without_activation(
    hwnd: windows::Win32::Foundation::HWND,
    position: Option<IndicatorPosition>,
    log: &Log,
) -> bool {
    use windows::Win32::Foundation::{GetLastError, SetLastError, WIN32_ERROR};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, IsWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE,
        GWL_STYLE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
        SW_SHOWNOACTIVATE, WINDOW_LONG_PTR_INDEX, WS_CAPTION, WS_EX_APPWINDOW, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_THICKFRAME,
    };

    fn last_error() -> Option<i32> {
        // SAFETY: reads the calling thread's last-error value.
        let WIN32_ERROR(code) = unsafe { GetLastError() };
        i32::try_from(code).ok().filter(|code| *code != 0)
    }

    fn win32_code(err: &windows::core::Error) -> Option<i32> {
        let hresult = err.code().0 as u32;
        ((hresult >> 16) & 0x1FFF == 7)
            .then_some((hresult & 0xFFFF) as i32)
            .filter(|code| *code != 0)
    }

    /// Reads the `index` style long of `hwnd`, ORs in `add`, clears `clear` and writes it
    /// back; the last error is cleared before each call and read after a 0 return.
    fn rewrite(
        hwnd: windows::Win32::Foundation::HWND,
        index: WINDOW_LONG_PTR_INDEX,
        add: isize,
        clear: isize,
    ) -> Result<(), Option<i32>> {
        // SAFETY: clears the calling thread's last error.
        unsafe { SetLastError(WIN32_ERROR(0)) };
        // SAFETY: any handle; a stale one returns 0 with the last error set.
        let style = unsafe { GetWindowLongPtrW(hwnd, index) };
        if style == 0 {
            if let Some(code) = last_error() {
                return Err(Some(code));
            }
        }
        let wanted = (style | add) & !clear;
        // SAFETY: clears the calling thread's last error.
        unsafe { SetLastError(WIN32_ERROR(0)) };
        // SAFETY: any handle; a failure returns 0 with the last error set.
        let previous = unsafe { SetWindowLongPtrW(hwnd, index, wanted) };
        if previous == 0 {
            if let Some(code) = last_error() {
                return Err(Some(code));
            }
        }
        Ok(())
    }

    fn restyle(
        hwnd: windows::Win32::Foundation::HWND,
        position: Option<IndicatorPosition>,
    ) -> Result<(), Option<i32>> {
        // tao keeps `WS_CAPTION` on an undecorated top-level window (it only hides the frame
        // in WM_NCCALCSIZE), so the frame bits are cleared here, raw.
        rewrite(
            hwnd,
            GWL_STYLE,
            0,
            (WS_CAPTION.0 | WS_THICKFRAME.0) as isize,
        )?;
        rewrite(
            hwnd,
            GWL_EXSTYLE,
            (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) as isize,
            WS_EX_APPWINDOW.0 as isize,
        )?;
        let mut flags = SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER;
        let (x, y) = match position {
            Some(p) => (p.x, p.y),
            None => {
                flags |= SWP_NOMOVE;
                (0, 0)
            }
        };
        // SAFETY: any handle; no size, z-order change or activation.
        unsafe { SetWindowPos(hwnd, None, x, y, 0, 0, flags) }.map_err(|err| win32_code(&err))
    }

    let styled = restyle(hwnd, position);

    // SAFETY: any handle; `IsWindow` only answers whether it names a window.
    if unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        // SAFETY: `hwnd` names a window; the return is the previous visibility, not an
        // error. `SW_SHOWNOACTIVATE` never activates it.
        let _was_visible = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    }

    match styled {
        Ok(()) => true,
        Err(os_code) => {
            log.write(LogEvent::Warning {
                kind: WarningKind::OverlayFailed,
                os_code,
            });
            false
        }
    }
}

/// The window's outer size in physical pixels (raw `GetWindowRect`); `None` when unreadable.
#[cfg(windows)]
fn outer_size(hwnd: windows::Win32::Foundation::HWND) -> Option<(i32, i32)> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
    let mut rect = RECT::default();
    // SAFETY: `rect` is writable; a stale handle gives an error.
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    Some((rect.right - rect.left, rect.bottom - rect.top))
}

/// The window's physical size if it were on the monitor at `at`: [`WIDTH`] x [`HEIGHT`] at
/// that monitor's DPI (96 when unreadable). Raw Win32 reads only.
#[cfg(windows)]
fn window_size_at(at: IndicatorPosition) -> (i32, i32) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    // SAFETY: any point; the flag makes the result a valid monitor.
    let monitor = unsafe { MonitorFromPoint(POINT { x: at.x, y: at.y }, MONITOR_DEFAULTTONEAREST) };
    let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
    // SAFETY: both outputs are writable locals.
    let dpi = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    let scale = if dpi.is_ok() && dpi_x > 0 {
        f64::from(dpi_x) / 96.0
    } else {
        1.0
    };
    (
        (WIDTH * scale).round() as i32,
        (HEIGHT * scale).round() as i32,
    )
}

#[cfg(not(windows))]
fn window_size_at(_at: IndicatorPosition) -> (i32, i32) {
    (WIDTH as i32, HEIGHT as i32)
}

/// The work areas of every present monitor (physical pixels; raw `EnumDisplayMonitors`).
#[cfg(windows)]
fn monitors() -> Vec<MonitorRect> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };

    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _dc: HDC,
        _clip: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        // SAFETY: `data` is the `&mut Vec<MonitorRect>` passed below, alive for the call.
        let list = unsafe { &mut *(data.0 as *mut Vec<MonitorRect>) };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: `info` is writable and its `cbSize` is set.
        if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            let work = info.rcWork;
            list.push(MonitorRect {
                x: work.left,
                y: work.top,
                width: work.right - work.left,
                height: work.bottom - work.top,
            });
        }
        BOOL(1)
    }

    let mut list: Vec<MonitorRect> = Vec::new();
    // SAFETY: the callback only writes into `list`, which outlives the call.
    let _all = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(&mut list as *mut Vec<MonitorRect> as isize),
        )
    };
    list
}

#[cfg(not(windows))]
fn monitors() -> Vec<MonitorRect> {
    Vec::new()
}

/// With no saved position: the top-right of the primary work area, [`MARGIN`] inside it.
#[cfg(windows)]
fn default_position(size: (i32, i32)) -> Option<IndicatorPosition> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
    };
    // SAFETY: any point; the flag makes the result a valid monitor.
    let monitor = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is writable and its `cbSize` is set.
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return None;
    }
    let work = info.rcWork;
    Some(IndicatorPosition {
        x: work.right - size.0 - MARGIN,
        y: work.top + MARGIN,
    })
}
