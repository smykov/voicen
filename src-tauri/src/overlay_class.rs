//! The overlay class (T-247; why: `docs/decisions/overlay.md` invariant 3 and "Call
//! indicator"): what every overlay-class window (today the dictation overlay and the call
//! indicator) shares, in one place.
//!
//! - [`show_no_activate`] restyles the hidden window by its [`StyleSpec`] (`WS_CAPTION`
//!   cleared in `GWL_STYLE` for every window, the window's own `WS_EX` bits set and
//!   cleared), runs its optional pre-show placement only after a successful restyle and
//!   shows it with exactly one `ShowWindow(SW_SHOWNOACTIVATE)` when the handle is a
//!   window, whatever failed before. It is the only place that shows a window of this
//!   class; [`show_no_activate_with`] is its test seam (one production path).
//! - [`reveal_with`] reads the freshly built window's handle and hands it to the window's
//!   show; a handle that cannot be read leaves the window hidden.
//! - [`execute`] runs every reducer answer of either window: `catch_unwind` per action,
//!   one `overlay_failed` line per failure and the Destroy-failure rule; the per-window
//!   differences are the hooks of [`OverlayClassThread`].
//! - [`probe_label`] decides each wake on tauri's liveness (T-224): a held, `Live`
//!   window whose label tauri no longer knows counts as destroyed in that wake, before
//!   its own `Destroyed` listener posted (the same label test as the Destroy-failure
//!   rule).
//!
//! Every failure writes one `warning kind=overlay_failed` line: the kind and the OS code
//! only, no payload, handle or error text (#45, FR-20).

use std::panic::AssertUnwindSafe;

use tauri::{AppHandle, Manager, Runtime, WebviewWindow};
use voicen_core::diag::{Log, LogEvent, WarningKind};
use voicen_core::overlay::{OverlayLifecycle, WindowAction, WindowPhase};

use crate::diag::io_os_code;

/// Writes one `warning kind=overlay_failed` line: the kind and the OS code only, no
/// payload, handle or error text (#45, FR-20).
pub(crate) fn overlay_failed(log: &Log, os_code: Option<i32>) {
    log.write(LogEvent::Warning {
        kind: WarningKind::OverlayFailed,
        os_code,
    });
}

/// One overlay-class window thread, as [`execute`] sees it: its window slot, its reducer
/// and the hooks where the windows differ.
pub(crate) trait OverlayClassThread<R: Runtime> {
    /// The window's label (the Destroy-failure rule asks tauri whether it still exists).
    const LABEL: &'static str;

    fn app(&self) -> &AppHandle<R>;

    fn log(&self) -> &Log;

    fn lifecycle(&mut self) -> &mut OverlayLifecycle;

    /// The thread's live window, if it holds one.
    fn window(&mut self) -> &mut Option<WebviewWindow<R>>;

    /// Builds, reveals and first-emits the window (a failed first emit or show writes its
    /// own line and still returns the window).
    fn build(&mut self) -> tauri::Result<WebviewWindow<R>>;

    /// Emits the current state to the window.
    fn emit(&self) -> tauri::Result<()>;

    /// Destroys `window` (and whatever the window frees with it).
    fn destroy(window: &WebviewWindow<R>) -> tauri::Result<()>;

    /// Runs when a Destroy starts, after the window was taken out of its slot.
    fn on_destroy_start(&mut self) {}

    /// Hands the reducer a `Destroyed` for the window built with `generation`.
    fn post_destroyed(&self, generation: u64);
}

/// The label probe (T-224): decides a wake on the window's liveness as tauri shows it.
/// When `thread` holds a window, the phase is `Live` and tauri no longer knows
/// [`OverlayClassThread::LABEL`], the window went from outside and tauri already freed
/// its label, but its own `Destroyed` listener has not posted yet (tauri frees the label
/// and runs the app's run callback before the window's listeners, in one main-thread
/// dispatch). The current generation is then added to this wake's `destroyed`, so the
/// newest state taken in the same wake is decided together with that `Destroyed` (a
/// newer shown state builds instead of emitting to a gone window). The late real
/// `Destroyed` is then stale by generation, or ignored with no window. The label test is
/// the Destroy-failure rule's in [`execute`]; the HWND is not asked (it dies before the
/// label is freed, and a build then would find the label still taken). Called by each
/// thread right after its mailbox snapshot, before the reducer.
pub(crate) fn probe_label<R: Runtime, T: OverlayClassThread<R>>(
    thread: &mut T,
    destroyed: &mut Vec<u64>,
) {
    let live = thread.window().is_some() && thread.lifecycle().phase() == WindowPhase::Live;
    if live && thread.app().get_webview_window(T::LABEL).is_none() {
        let generation = thread.lifecycle().generation();
        if !destroyed.contains(&generation) {
            destroyed.push(generation);
        }
    }
}

/// Runs one reducer answer of `thread` and returns whether it failed (each failure writes
/// one `overlay_failed` line). A panic inside a window call (where panics unwind: tests,
/// dev) is caught and written like a failure; release aborts.
///
/// The Destroy-failure rule: a failed destroy writes its line; when tauri no longer knows
/// the label, the window is already gone and may post no `Destroyed`, so the thread hands
/// the reducer one for its generation (a late real one for the same generation is ignored
/// once the reducer moved on); a window that still exists goes back into its slot and
/// stays until its own `Destroyed` (the warning is the trace).
pub(crate) fn execute<R: Runtime, T: OverlayClassThread<R>>(
    thread: &mut T,
    action: WindowAction,
) -> bool {
    match action {
        WindowAction::Nothing => false,
        WindowAction::Build(_) => {
            let built = std::panic::catch_unwind(AssertUnwindSafe(|| thread.build()));
            match built {
                Ok(Ok(window)) => {
                    *thread.window() = Some(window);
                    false
                }
                Ok(Err(err)) => {
                    overlay_failed(thread.log(), io_os_code(&err));
                    thread.lifecycle().on_build_failed();
                    true
                }
                Err(_panic) => {
                    overlay_failed(thread.log(), None);
                    thread.lifecycle().on_build_failed();
                    true
                }
            }
        }
        WindowAction::Emit(_) => {
            let emitted = std::panic::catch_unwind(AssertUnwindSafe(|| thread.emit()));
            match emitted {
                Ok(Ok(())) => false,
                Ok(Err(err)) => {
                    overlay_failed(thread.log(), io_os_code(&err));
                    true
                }
                Err(_panic) => {
                    overlay_failed(thread.log(), None);
                    true
                }
            }
        }
        WindowAction::Destroy => {
            let window = thread.window().take();
            thread.on_destroy_start();
            let destroyed = std::panic::catch_unwind(AssertUnwindSafe(|| match &window {
                Some(window) => T::destroy(window),
                None => Ok(()),
            }));
            let failed = match destroyed {
                Ok(Ok(())) => None,
                Ok(Err(err)) => Some(io_os_code(&err)),
                Err(_panic) => Some(None),
            };
            if let Some(os_code) = failed {
                overlay_failed(thread.log(), os_code);
                if thread.app().get_webview_window(T::LABEL).is_none() {
                    let generation = thread.lifecycle().generation();
                    thread.post_destroyed(generation);
                } else {
                    *thread.window() = window;
                }
            }
            failed.is_some()
        }
    }
}

/// The styles one overlay-class window is shown with: the `GWL_STYLE` bits cleared for
/// every window of the class ([`StyleSpec::STYLE_CLEAR`]) and the window's own
/// `GWL_EXSTYLE` bits to set and to clear.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StyleSpec {
    ex_set: windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE,
    ex_clear: windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE,
}

#[cfg(windows)]
impl StyleSpec {
    /// Cleared in `GWL_STYLE` for every window of the class: tao writes `WS_CAPTION` on
    /// every top-level window, `decorations(false)` included (tao 0.37.1 window_state.rs
    /// `to_window_styles`; the frame is removed only through its `WM_NCCALCSIZE`), so the
    /// window is undecorated in its style too. A drag's `WM_NCLBUTTONDOWN(HTCAPTION)` move
    /// loop needs no caption bit; `WS_SYSMENU` is inert without a caption and stays.
    pub const STYLE_CLEAR: windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE =
        windows::Win32::UI::WindowsAndMessaging::WS_CAPTION;

    /// The window's own extended bits: `ex_set` ORed in, then `ex_clear` cleared (the
    /// creation's other bits stay).
    pub const fn new(
        ex_set: windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE,
        ex_clear: windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE,
    ) -> Self {
        Self { ex_set, ex_clear }
    }
}

/// The calling thread's last error as a log code; `None` when it is 0.
#[cfg(windows)]
pub(crate) fn last_error() -> Option<i32> {
    use windows::Win32::Foundation::{GetLastError, WIN32_ERROR};
    // SAFETY: reads the calling thread's last-error value.
    let WIN32_ERROR(code) = unsafe { GetLastError() };
    i32::try_from(code).ok().filter(|code| *code != 0)
}

/// The Win32 code inside a `windows` error (an `HRESULT_FROM_WIN32`); `None` for any other
/// `HRESULT`.
#[cfg(windows)]
pub(crate) fn win32_code(err: &windows::core::Error) -> Option<i32> {
    let hresult = err.code().0 as u32;
    ((hresult >> 16) & 0x1FFF == 7)
        .then_some((hresult & 0xFFFF) as i32)
        .filter(|code| *code != 0)
}

/// Shows the freshly built, hidden `window` through `show` with the handle `hwnd` reads
/// from it (T-172; a test seam like `settings_window::request_with`: production reaches it
/// only through each window's reveal, with `WebviewWindow::hwnd`). A handle that cannot be
/// read leaves the window hidden (no show of any kind), writes exactly one
/// `overlay_failed` line and returns `None`; otherwise it returns what `show` returns (a
/// failed restyle or placement writes its own one line there).
#[cfg(windows)]
pub fn reveal_with<R: Runtime, T>(
    window: &WebviewWindow<R>,
    hwnd: impl FnOnce(&WebviewWindow<R>) -> tauri::Result<windows::Win32::Foundation::HWND>,
    show: impl FnOnce(windows::Win32::Foundation::HWND) -> Option<T>,
    log: &Log,
) -> Option<T> {
    match hwnd(window) {
        Ok(hwnd) => show(hwnd),
        Err(err) => {
            overlay_failed(log, io_os_code(&err));
            None
        }
    }
}

/// Restyles the hidden overlay-class window `hwnd` by `spec`, runs `place` (the window's
/// pre-show placement; `|_| Ok(())` for none) and shows it without activating it:
/// [`show_no_activate_with`] with the real [`restyle`], the one production path. Runs on
/// the window's own thread and makes no tao call.
///
/// Returns `place`'s value when the restyle and the placement succeeded. On the first
/// failure it writes exactly one `warning kind=overlay_failed os_code=N` (the OS code
/// only), still shows the window with `SW_SHOWNOACTIVATE` when `hwnd` is a window (never
/// with an activating show) and returns `None`.
#[cfg(windows)]
pub fn show_no_activate<T>(
    hwnd: windows::Win32::Foundation::HWND,
    spec: &StyleSpec,
    place: impl FnOnce(windows::Win32::Foundation::HWND) -> Result<T, Option<i32>>,
    log: &Log,
) -> Option<T> {
    show_no_activate_with(hwnd, |hwnd| restyle(hwnd, spec), place, log)
}

/// [`show_no_activate`] with the restyle done by `restyle` (T-172; a test seam: production
/// reaches it only through `show_no_activate` and the overlay's `show_click_through`, each
/// with the real [`restyle`]). Calls `restyle` once, then `place` only when it succeeded,
/// then shows `hwnd` with one `ShowWindow(SW_SHOWNOACTIVATE)` when it names a window,
/// whether or not either succeeded (never with an activating show). Returns `place`'s
/// value; on the first `Err` writes exactly one `warning kind=overlay_failed os_code=N`
/// and returns `None`.
#[cfg(windows)]
pub fn show_no_activate_with<T>(
    hwnd: windows::Win32::Foundation::HWND,
    restyle: impl FnOnce(windows::Win32::Foundation::HWND) -> Result<(), Option<i32>>,
    place: impl FnOnce(windows::Win32::Foundation::HWND) -> Result<T, Option<i32>>,
    log: &Log,
) -> Option<T> {
    use windows::Win32::UI::WindowsAndMessaging::{IsWindow, ShowWindow, SW_SHOWNOACTIVATE};

    let prepared = restyle(hwnd).and_then(|()| place(hwnd));

    // SAFETY: any handle; `IsWindow` only answers whether it names a window.
    if unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        // SAFETY: `hwnd` names a window; the return is the previous visibility, not an
        // error. `SW_SHOWNOACTIVATE` never activates it.
        let _was_visible = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    }

    match prepared {
        Ok(placed) => Some(placed),
        Err(os_code) => {
            overlay_failed(log, os_code);
            None
        }
    }
}

/// The restyle of [`show_no_activate`]: clears [`StyleSpec::STYLE_CLEAR`] in `GWL_STYLE`
/// (written only when a bit was set), ORs `spec`'s set bits into `GWL_EXSTYLE` and clears
/// its clear bits, each written with `SetWindowLongPtrW` (the last error cleared before
/// and read after: a return of 0 is otherwise ambiguous), and applies them with
/// `SetWindowPos(SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE |
/// SWP_NOZORDER)`. Each step runs only when the previous one succeeded; `Err` carries the
/// code of the first failure.
#[cfg(windows)]
pub fn restyle(
    hwnd: windows::Win32::Foundation::HWND,
    spec: &StyleSpec,
) -> Result<(), Option<i32>> {
    use windows::Win32::Foundation::{SetLastError, WIN32_ERROR};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, GWL_STYLE,
        SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    };

    // SAFETY: clears the calling thread's last error.
    unsafe { SetLastError(WIN32_ERROR(0)) };
    // SAFETY: any handle; a stale one returns 0 with the last error set.
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) };
    if style == 0 {
        if let Some(code) = last_error() {
            return Err(Some(code));
        }
    }
    let wanted = style & !(StyleSpec::STYLE_CLEAR.0 as isize);
    if wanted != style {
        // SAFETY: clears the calling thread's last error.
        unsafe { SetLastError(WIN32_ERROR(0)) };
        // SAFETY: any handle; a failure returns 0 with the last error set.
        let previous = unsafe { SetWindowLongPtrW(hwnd, GWL_STYLE, wanted) };
        if previous == 0 {
            if let Some(code) = last_error() {
                return Err(Some(code));
            }
        }
    }
    // SAFETY: clears the calling thread's last error.
    unsafe { SetLastError(WIN32_ERROR(0)) };
    // SAFETY: any handle; a stale one returns 0 with the last error set.
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    if style == 0 {
        if let Some(code) = last_error() {
            return Err(Some(code));
        }
    }
    let wanted = (style | spec.ex_set.0 as isize) & !(spec.ex_clear.0 as isize);
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
