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
//!   event, tagged with the reducer's generation, or earlier from the shared label probe
//!   (`overlay_class::probe_label`, T-224): a `Live` window whose label tauri already
//!   freed counts as destroyed in that wake.
//! - The window is built hidden, undecorated, topmost, without a taskbar button and
//!   non-activating (`focusable(false)`: `WS_EX_NOACTIVATE`, variant N of the spike). The
//!   thread waits until its page finished loading (at most `PAGE_LOAD_WAIT`). On
//!   Windows it then clears `WS_CAPTION` (tao keeps it on undecorated windows), writes the
//!   tool-window bits (`WS_EX_TOOLWINDOW` set, `WS_EX_APPWINDOW` cleared), moves the hidden
//!   window to its placement with a raw `SetWindowPos(SWP_NOACTIVATE)` and shows it once
//!   with `ShowWindow(SW_SHOWNOACTIVATE)` ([`show_indicator`], through the shared
//!   `overlay_class::show_no_activate`, T-247; every reducer answer runs through the shared
//!   `overlay_class::execute`). It is not click-through
//!   (Stop, Dismiss and the drag region take clicks). No tao setter touches it afterwards
//!   (overlay.md §3). At the stop the window is destroyed and its webview closed (the
//!   label is freed at once).
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

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tauri::webview::PageLoadEvent;
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
use crate::overlay_class::{self, overlay_failed, OverlayClassThread};

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

/// How long a freshly built, hidden window waits for its page to finish loading before it
/// is shown anyway (a page that never reports its load still gets its window).
const PAGE_LOAD_WAIT: Duration = Duration::from_secs(2);

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
    /// The generation of the newest window whose page finished loading.
    loaded: Option<u64>,
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

    /// Records that the page of the window built with `generation` finished loading and
    /// wakes the thread if it waits for it ([`Shared::wait_loaded`]); not a wake of the
    /// main loop (`woken` stays as it is).
    fn mark_loaded(&self, generation: u64) {
        self.lock().loaded = Some(generation);
        self.wake.notify_one();
    }

    /// Waits until the page of the window built with `generation` finished loading, or
    /// `hang` passed; returns whether it loaded. Other posts meanwhile stay in the mailbox
    /// for the next wake.
    fn wait_loaded(&self, generation: u64, hang: Duration) -> bool {
        let mailbox = self.lock();
        let (mailbox, _) = self
            .wake
            .wait_timeout_while(mailbox, hang, |m| m.loaded != Some(generation))
            .unwrap_or_else(PoisonError::into_inner);
        mailbox.loaded == Some(generation)
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
        let mut wake = thread.shared.wait(live, thread.last_tick);
        // The label probe (T-224): a window tauri already freed counts as destroyed in
        // this wake, before its own Destroyed listener posted.
        overlay_class::probe_label(&mut thread, &mut wake.destroyed);
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

impl<R: Runtime> OverlayClassThread<R> for IndicatorThread<R> {
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
        self.app.emit_to(LABEL, STATE_EVENT, self.state())
    }

    /// After the window's destroy, its webview is closed too: that frees the label in
    /// tauri's manager at once (`Webview::close`), on a runtime whose destroy posts no
    /// `Destroyed` (the mock) as on Wry, where the queued close finds the window gone and
    /// the window's own `Destroyed` follows. A failed destroy closes nothing, so the label
    /// stays until a real `Destroyed`.
    fn destroy(window: &WebviewWindow<R>) -> tauri::Result<()> {
        window
            .destroy()
            .and_then(|()| AsRef::<tauri::Webview<R>>::as_ref(window).close())
    }

    /// The thread's placement belongs to the window being destroyed.
    fn on_destroy_start(&mut self) {
        self.placed = None;
    }

    fn post_destroyed(&self, generation: u64) {
        self.shared.post(|m| m.destroyed.push(generation));
    }
}

impl<R: Runtime> IndicatorThread<R> {
    fn warn(&self, os_code: Option<i32>) {
        overlay_failed(&self.log, os_code);
    }

    /// Runs one reducer answer through the overlay-class executor
    /// ([`overlay_class::execute`]) and returns whether it failed.
    fn execute(&mut self, action: WindowAction) -> bool {
        overlay_class::execute(self, action)
    }

    /// The mailbox's newest state; idle in the settings language before the first.
    fn state(&self) -> IndicatorState {
        self.shared
            .newest()
            .unwrap_or_else(|| IndicatorState::idle(self.service.snapshot().ui_language))
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
    /// `Moved` listeners (both tagged with this build's generation), waits for its page to
    /// finish loading (at most [`PAGE_LOAD_WAIT`], so it never shows a blank page), places
    /// and shows it without activation ([`IndicatorThread::reveal`]) and emits the current
    /// state (a page not listening yet catches up through `call_indicator_state`).
    fn build_window(&mut self) -> tauri::Result<WebviewWindow<R>> {
        let generation = self.lifecycle.generation();
        let loads = Arc::clone(&self.shared);
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
            .on_page_load(move |_window, payload| {
                if payload.event() == PageLoadEvent::Finished {
                    loads.mark_loaded(generation);
                }
            })
            .build()?;
        let shared = Arc::clone(&self.shared);
        window.on_window_event(move |event| match event {
            WindowEvent::Destroyed => shared.post(|m| m.destroyed.push(generation)),
            WindowEvent::Moved(position) => {
                let position = *position;
                shared.post(|m| m.moved = Some((generation, position, Instant::now())));
            }
            _ => {}
        });
        // A page that does not load in time is shown anyway (no failure: the window works
        // once it loads).
        let _loaded = self.shared.wait_loaded(generation, PAGE_LOAD_WAIT);
        self.placed = self.reveal(&window);
        if let Err(err) = self.app.emit_to(LABEL, STATE_EVENT, self.state()) {
            self.warn(io_os_code(&err));
        }
        Ok(window)
    }

    /// Places and shows the freshly built, hidden `window` through [`show_indicator`],
    /// the one place that shows the indicator, with the overlay-class
    /// [`overlay_class::reveal_with`] and the real handle reader; returns where it was
    /// placed. A handle that cannot be read leaves the window hidden and writes one
    /// `overlay_failed` line.
    #[cfg(windows)]
    fn reveal(&self, window: &WebviewWindow<R>) -> Option<IndicatorPosition> {
        let stored = self.service.snapshot().calls.indicator_position;
        overlay_class::reveal_with(
            window,
            WebviewWindow::hwnd,
            |hwnd| show_indicator(hwnd, stored, &self.log),
            &self.log,
        )
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

/// The indicator's styles: a tool window (`WS_EX_TOOLWINDOW` set, `WS_EX_APPWINDOW`
/// cleared; the creation's `WS_EX_NOACTIVATE | WS_EX_TOPMOST` stay; no click-through
/// bits), and the class's `WS_CAPTION` cleared.
#[cfg(windows)]
const TOOL_WINDOW: overlay_class::StyleSpec = overlay_class::StyleSpec::new(
    windows::Win32::UI::WindowsAndMessaging::WS_EX_TOOLWINDOW,
    windows::Win32::UI::WindowsAndMessaging::WS_EX_APPWINDOW,
);

/// Writes the tool-window bits on the hidden indicator window `hwnd`, moves it to its
/// placement and shows it without activating it (T-130), through the overlay-class
/// [`overlay_class::show_no_activate`] (T-247). Runs on the `call-indicator` thread and
/// makes no tao call:
///
/// 1. clears `WS_CAPTION` in `GWL_STYLE` (tao sets it even without decorations), reads
///    `GWL_EXSTYLE`, ORs in `WS_EX_TOOLWINDOW` and clears `WS_EX_APPWINDOW` (the
///    creation's `WS_EX_NOACTIVATE | WS_EX_TOPMOST` stay; no click-through bits);
/// 2. writes it with `SetWindowLongPtrW` and applies it with `SetWindowPos(SWP_FRAMECHANGED
///    | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER)`;
/// 3. only when 1-2 succeeded: reads the window's size and the monitor work areas, places
///    it with core's `place_indicator(stored, ..)` and moves it there with
///    `SetWindowPos(SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER)`;
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
    overlay_class::show_no_activate(hwnd, &TOOL_WINDOW, |hwnd| place(hwnd, stored), log)
}

/// Step 3 of [`show_indicator`]; `Err` carries the code of the first failure.
#[cfg(windows)]
fn place(
    hwnd: windows::Win32::Foundation::HWND,
    stored: Option<IndicatorPosition>,
) -> Result<IndicatorPosition, Option<i32>> {
    use voicen_core::call::indicator::place_indicator;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    };

    let rect = win32::window_rect(hwnd).ok_or_else(overlay_class::last_error)?;
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
    .map_err(|err| overlay_class::win32_code(&err))?;
    Ok(at)
}

/// Raw Win32 reads of the indicator's placement (no tao call, no main-thread round trip of
/// tauri's).
#[cfg(windows)]
mod win32 {
    use voicen_core::call::indicator::WorkArea;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, MONITORINFOF_PRIMARY};

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
