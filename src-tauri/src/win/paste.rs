//! Paste into the start window (T-006 design 4; research R-12): `WinPaster`.
//! `capture_start_window` (under the session lock) reads the foreground window's
//! root owner, its process id and its integrity level against ours
//! (`win32_data::target_elevated`), and never sends a message to that window;
//! `wait_modifiers_released` polls `win32_data::modifier_wait_keys` for at most the
//! given time; `is_in_front` compares the foreground root owner and its process id;
//! `send_ctrl_v` sends Ctrl+V in one `SendInput` batch. No mutex is held across
//! its methods.

use std::ffi::c_void;
use std::mem::size_of;
use std::thread;
use std::time::{Duration, Instant};

use voicen_core::platform::{PasteError, Paster, StartWindow, WindowRef};
use voicen_core::win32_data::{modifier_wait_keys, target_elevated, IntegrityLevel};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetForegroundWindow, GetWindowThreadProcessId, GA_ROOTOWNER,
};

/// `VK_CONTROL` and `V` (the paste chord; not a hotkey or modifier-wait fact).
const VK_CONTROL: u16 = 0x11;
const VK_V: u16 = 0x56;

/// The modifier poll period (data-model "DeliveryDecision").
const POLL: Duration = Duration::from_millis(10);

/// Ctrl+V into the start window. Its own integrity level is read once.
pub struct WinPaster {
    own_level: Option<IntegrityLevel>,
}

impl WinPaster {
    pub fn new() -> WinPaster {
        // SAFETY: the pseudo handle of this process (never closed).
        let own = unsafe { GetCurrentProcess() };
        WinPaster {
            own_level: process_level(own),
        }
    }
}

impl Default for WinPaster {
    fn default() -> WinPaster {
        WinPaster::new()
    }
}

/// A handle closed on drop.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: a handle this module opened and still owns, closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// The mandatory integrity level of the process behind `process` (a handle with
/// query access); `None` when any step fails (then elevated counts as unknown).
fn process_level(process: HANDLE) -> Option<IntegrityLevel> {
    let mut token = HANDLE::default();
    // SAFETY: `token` is writable; `process` has query access.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
    let token = Owned(token);
    let mut len = 0u32;
    // SAFETY: a size query with no buffer; it fails with the needed length.
    let _ = unsafe { GetTokenInformation(token.0, TokenIntegrityLevel, None, 0, &mut len) };
    if (len as usize) < size_of::<TOKEN_MANDATORY_LABEL>() {
        return None;
    }
    // u64 units: aligned for TOKEN_MANDATORY_LABEL's pointer.
    let mut buf = vec![0u64; (len as usize).div_ceil(size_of::<u64>())];
    // SAFETY: `buf` holds at least `len` writable bytes.
    unsafe {
        GetTokenInformation(
            token.0,
            TokenIntegrityLevel,
            Some(buf.as_mut_ptr().cast::<c_void>()),
            len,
            &mut len,
        )
    }
    .ok()?;
    // SAFETY: the call above filled `buf` with a TOKEN_MANDATORY_LABEL whose SID
    // points into `buf`, which outlives these reads.
    unsafe {
        let label = &*buf.as_ptr().cast::<TOKEN_MANDATORY_LABEL>();
        let sid = label.Label.Sid;
        if sid.is_invalid() {
            return None;
        }
        let count = GetSidSubAuthorityCount(sid);
        if count.is_null() || *count == 0 {
            return None;
        }
        let rid = GetSidSubAuthority(sid, u32::from(*count - 1));
        if rid.is_null() {
            return None;
        }
        Some(IntegrityLevel(*rid))
    }
}

/// The level of process `pid`; `None` when it cannot be read.
fn level_of(pid: u32) -> Option<IntegrityLevel> {
    // SAFETY: query-only access to a process id; the handle is closed by `Owned`.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let process = Owned(process);
    process_level(process.0)
}

/// The foreground window's root owner and its process id; `None` when nothing is
/// in front or the window is gone. Sends no message to the window.
fn front_root() -> Option<(HWND, u32)> {
    // SAFETY: no arguments.
    let front = unsafe { GetForegroundWindow() };
    if front.is_invalid() {
        return None;
    }
    // SAFETY: any window handle; a stale one gives null.
    let root = unsafe { GetAncestor(front, GA_ROOTOWNER) };
    let root = if root.is_invalid() { front } else { root };
    let mut pid = 0u32;
    // SAFETY: `pid` is writable; a stale handle gives thread id 0.
    let thread = unsafe { GetWindowThreadProcessId(root, Some(&mut pid)) };
    (thread != 0 && pid != 0).then_some((root, pid))
}

fn window_ref(hwnd: HWND) -> WindowRef {
    WindowRef(hwnd.0 as usize as u64)
}

fn key(vk: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn any_modifier_down() -> bool {
    modifier_wait_keys().iter().any(|&vk| {
        // SAFETY: a plain virtual-key code.
        unsafe { GetAsyncKeyState(i32::from(vk)) < 0 }
    })
}

impl Paster for WinPaster {
    fn capture_start_window(&self) -> Option<StartWindow> {
        let (root, pid) = front_root()?;
        Some(StartWindow {
            handle: window_ref(root),
            process_id: pid,
            elevated: target_elevated(self.own_level, level_of(pid)),
        })
    }

    fn wait_modifiers_released(&self, max_wait: Duration) -> bool {
        let deadline = Instant::now() + max_wait;
        loop {
            if !any_modifier_down() {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            thread::sleep(POLL.min(deadline - now));
        }
    }

    fn is_in_front(&self, w: &StartWindow) -> bool {
        front_root().is_some_and(|(root, pid)| window_ref(root) == w.handle && pid == w.process_id)
    }

    fn send_ctrl_v(&self) -> Result<(), PasteError> {
        let inputs = [
            key(VK_CONTROL, false),
            key(VK_V, false),
            key(VK_V, true),
            key(VK_CONTROL, true),
        ];
        // SAFETY: a valid INPUT slice and the size of one element.
        let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
        if sent as usize == inputs.len() {
            Ok(())
        } else {
            Err(PasteError)
        }
    }
}
