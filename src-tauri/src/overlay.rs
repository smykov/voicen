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
//!   not matter.
//! - The window is built visible with `focused(false)` and `focusable(false)`, so
//!   tao's creation shows it with `SW_SHOWNOACTIVATE` after all its styles
//!   (`WS_EX_NOACTIVATE`, `WS_EX_TOPMOST`) are set. Nothing calls `show`, `set_focus`
//!   or any other tao setter on it afterwards: tao re-shows a visible window with
//!   `SW_SHOW` on every flag change. `Hidden` destroys it (invariant 3; NFR-03).
//! - `overlay_ready` and every emit carry `overlay_payload` of the mailbox's newest
//!   state, in the snapshot's `ui_language`, read on the overlay thread or in the
//!   command, never under the session lock (invariant 4).
//! - A failed build, emit or destroy, or a failed start of the thread, writes one
//!   `warning kind=overlay_failed` line (the OS code only, no payload or error text).

use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

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
}

/// The overlay mailbox of one app: a mutex held only to copy or store an entry,
/// never across a window operation, and the condvar that wakes the thread.
#[derive(Default)]
struct Shared {
    mailbox: Mutex<Mailbox>,
    wake: Condvar,
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
    loop {
        let (destroyed, newest) = thread.shared.wait();
        // The current window's Destroyed (requested, or the window went by itself,
        // e.g. Alt+F4 or exit): its handle is stale. A Destroyed of an older
        // generation is not this window's; the reducer ignores it too.
        if thread.lifecycle.phase() != WindowPhase::Absent
            && destroyed.contains(&thread.lifecycle.generation())
        {
            thread.window = None;
        }
        let action = thread
            .lifecycle
            .on_wake(&destroyed, newest.as_ref().map(|e| (e.seq, &e.state)));
        // The reducer keeps only the newest seq it was given; `current` is that
        // entry, so a `Build(seq)` (now or after a later Destroyed) renders it.
        if let Some(entry) = newest {
            if thread.current.as_ref().is_none_or(|c| entry.seq > c.seq) {
                thread.current = Some(entry);
            }
        }
        thread.execute(action);
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

impl<R: Runtime> OverlayThread<R> {
    fn warn(&self, os_code: Option<i32>) {
        // The kind and the OS code only: no payload, no error text (#45, FR-20).
        self.log.write(LogEvent::Warning {
            kind: WarningKind::OverlayFailed,
            os_code,
        });
    }

    /// Runs one reducer answer. A panic inside a window call (where panics unwind:
    /// tests, dev) is caught and written like a failure; release aborts.
    fn execute(&mut self, action: WindowAction) {
        match action {
            WindowAction::Nothing => {}
            WindowAction::Build(_) => {
                let built = std::panic::catch_unwind(AssertUnwindSafe(|| self.build()));
                match built {
                    Ok(Ok(window)) => self.window = Some(window),
                    Ok(Err(err)) => {
                        self.warn(io_os_code(&err));
                        self.lifecycle.on_build_failed();
                    }
                    Err(_panic) => {
                        self.warn(None);
                        self.lifecycle.on_build_failed();
                    }
                }
            }
            WindowAction::Emit(_) => {
                let emitted = std::panic::catch_unwind(AssertUnwindSafe(|| self.emit()));
                match emitted {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => self.warn(io_os_code(&err)),
                    Err(_panic) => self.warn(None),
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
                    // A window that is already gone may post no Destroyed: hand the
                    // reducer one for this window's generation, so the next shown
                    // state can build again (a late real one for the same generation
                    // is ignored once the reducer moved on). One that still exists
                    // stays until its own Destroyed (the warning is the trace).
                    if self.app.get_webview_window(LABEL).is_none() {
                        self.shared.destroyed(self.lifecycle.generation());
                    } else {
                        self.window = window;
                    }
                }
            }
        }
    }

    /// The payload of the state the reducer last saw.
    fn payload(&self) -> OverlayPayload {
        payload(self.current.as_ref(), &self.service)
    }

    /// Builds the window visible and non-activating, registers its `Destroyed`
    /// listener (which posts the reducer's generation for this build) and emits the
    /// current state to it (a page that is not listening yet catches up through
    /// `overlay_ready`).
    fn build(&self) -> tauri::Result<WebviewWindow<R>> {
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
            .visible(true);
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
        // A failed first emit is written like any emit failure; the page still
        // catches up through `overlay_ready`, so the window stays.
        if let Err(err) = self.app.emit_to(LABEL, STATE_EVENT, self.payload()) {
            self.warn(io_os_code(&err));
        }
        Ok(window)
    }

    fn emit(&self) -> tauri::Result<()> {
        self.app.emit_to(LABEL, STATE_EVENT, self.payload())
    }
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
