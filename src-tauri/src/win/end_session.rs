//! The process's end on `WM_ENDSESSION(TRUE)` (T-145 I1; spec 007 FR-007, research R14;
//! why: `docs/decisions/windows-shell.md` "The call part at exit").
//!
//! tao 0.37.1 handles `WM_ENDSESSION(TRUE)` in its thread event target's window procedure
//! with `loop_destroyed()` (which emits `LoopDestroyed`, tauri's `RunEvent::Exit`) and then
//! `std::process::exit(0)`. When the message reaches that procedure inside a nested message
//! pump of the loop's own event handler (wry's WebView2 creation waits with
//! `webview2_com::wait_with_pump`, e.g. while the call indicator window is built), the
//! handler is taken: `LoopDestroyed` panics inside tao's `catch_unwind` (with
//! `panic = "abort"`: the process aborts), so `RunEvent::Exit` never runs and the process
//! ends with the call still `recording`.
//!
//! [`install`] puts a `WH_CALLWNDPROC` hook on the loop's thread (called by the system
//! before any window procedure of that thread gets a sent message, nested pump or not).
//! At the first `WM_ENDSESSION(TRUE)` to any window of the thread it runs the installed
//! end of the process once, on its own `end-session` thread, and waits for it; only then
//! does the message go on to tao. The work runs off the loop's thread because that thread
//! may be inside tauri's runtime with its window table borrowed. The end of the process is
//! idempotent, so a `RunEvent::Exit` after it (the plain case) does nothing again.
//! [`uninstall`] removes the hook (the `RunEvent::Exit` arm). A hook that cannot be set
//! leaves `RunEvent::Exit` as the only path.

use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::thread;

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx, CWPSTRUCT, HC_ACTION, HHOOK,
    WH_CALLWNDPROC, WM_ENDSESSION,
};

/// What runs at the session end: the whole end of the process, safe to run twice.
pub type EndOfProcess = Arc<dyn Fn() + Send + Sync>;

/// The hook of this thread and the end it has not run yet.
struct Installed {
    hook: HHOOK,
    end: Option<EndOfProcess>,
}

impl Drop for Installed {
    fn drop(&mut self) {
        // SAFETY: `hook` is the handle `SetWindowsHookExW` returned for this entry; it is
        // removed once, here.
        let _ = unsafe { UnhookWindowsHookEx(self.hook) };
    }
}

thread_local! {
    static INSTALLED: RefCell<Option<Installed>> = const { RefCell::new(None) };
}

/// Hooks this thread (the event loop's) so the first `WM_ENDSESSION(TRUE)` runs `end`
/// before any window procedure sees it. Replaces an earlier install on this thread.
pub fn install(end: EndOfProcess) {
    // SAFETY: a thread hook of this thread on a procedure of this module (`hmod` None).
    let hook = unsafe {
        SetWindowsHookExW(
            WH_CALLWNDPROC,
            Some(call_wnd_proc),
            None,
            GetCurrentThreadId(),
        )
    };
    let Ok(hook) = hook else {
        return;
    };
    let installed = Installed {
        hook,
        end: Some(end),
    };
    let _ = INSTALLED.try_with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut() {
            *slot = Some(installed);
        }
    });
}

/// Removes this thread's hook, if any; the end it held is dropped unrun.
pub fn uninstall() {
    let _ = INSTALLED.try_with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut() {
            slot.take();
        }
    });
}

/// The end this thread's hook still holds, taken so it runs at most once.
fn take_end() -> Option<EndOfProcess> {
    INSTALLED
        .try_with(|slot| {
            slot.try_borrow_mut()
                .ok()
                .and_then(|mut slot| slot.as_mut().and_then(|i| i.end.take()))
        })
        .ok()
        .flatten()
}

/// Runs `end` on the `end-session` thread and waits for it; inline only when that thread
/// cannot start.
fn run_end(end: EndOfProcess) {
    let worker = Arc::clone(&end);
    match thread::Builder::new()
        .name("end-session".into())
        .spawn(move || worker())
    {
        Ok(handle) => {
            let _ = handle.join();
        }
        Err(_) => {
            let _ = catch_unwind(AssertUnwindSafe(|| end()));
        }
    }
}

unsafe extern "system" fn call_wnd_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && lparam.0 != 0 {
        // SAFETY: for HC_ACTION, `lparam` points to the system's CWPSTRUCT of the message,
        // valid for this call.
        let sent = unsafe { &*(lparam.0 as *const CWPSTRUCT) };
        if sent.message == WM_ENDSESSION && sent.wParam.0 != 0 {
            let _ = catch_unwind(|| {
                if let Some(end) = take_end() {
                    run_end(end);
                }
            });
        }
    }
    // SAFETY: passes the same arguments on to the next hook (the handle is ignored).
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}
