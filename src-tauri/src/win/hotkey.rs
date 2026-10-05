//! The hotkey thread (T-006 design 1; research R-1, R-2): one std thread with a
//! hidden top-level window registers the settings' hotkey with `RegisterHotKey`
//! (codes from `voicen_core::win32_data::hotkey_codes`, `MOD_NOREPEAT` always) and
//! reports the result with `DictationSession::hotkey_registration`. On `WM_HOTKEY`
//! it sends the menu-mask key (`win32_data::menu_mask_vk`) down and up, then calls
//! `hotkey_pressed` with the instant stamped on arrival; a 10 ms poll of
//! `GetAsyncKeyState` over the poll groups (`win32_data::released`) stamps the
//! release and calls `hotkey_released`. Nothing else: no repeat count, no hold
//! timing, no mode. Failures are typed warning lines (`hotkey_thread_failed`,
//! also when the release poll's `SetTimer` fails, and `hotkey_register_failed`, with
//! the OS code).
//!
//! Lock contract: the thread holds no lock of its own across a session call. While
//! `hotkey_pressed` opens the device the poll cannot run; a release meanwhile is
//! stamped at the first poll after it.

use std::io;
use std::mem::size_of;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Instant;

use voicen_core::diag::{Log, LogEvent, WarningKind};
use voicen_core::dictation::DictationSession;
use voicen_core::settings::hotkey::parse_hotkey;
use voicen_core::win32_data::{hotkey_codes, menu_mask_vk, released, HotkeyCodes};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, SendInput, UnregisterHotKey, HOT_KEY_MODIFIERS, INPUT,
    INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer, PostThreadMessageW,
    SetTimer, TranslateMessage, MSG, WINDOW_EX_STYLE, WM_HOTKEY, WM_QUIT, WM_TIMER, WS_OVERLAPPED,
};

use super::os_code;

/// The id of the one registration (per window).
const HOTKEY_ID: i32 = 1;
/// The id of the release-poll timer.
const POLL_TIMER_ID: usize = 1;
/// The release poll period (R-1: 10 ms; the system rounds it to its timer tick).
const POLL_MS: u32 = 10;

/// The running hotkey thread. Dropping it ends the thread (quit message, join) and
/// unregisters the hotkey, so the combination is free again once `drop` returns.
pub struct HotkeyThread {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl HotkeyThread {
    /// Starts the hotkey thread for `hotkey` (canonical text, `Settings::hotkey`)
    /// over `session`. A hotkey that does not parse or that `RegisterHotKey` refuses
    /// is reported to the session as not registered (and logged); the thread still
    /// runs. `Err` only when the thread could not be started (its hidden window
    /// included; logged as `hotkey_thread_failed`, and the session is told the
    /// hotkey is not registered).
    pub fn start(
        session: Arc<DictationSession>,
        hotkey: &str,
        log: Arc<Log>,
    ) -> io::Result<HotkeyThread> {
        let codes = parse_hotkey(hotkey).ok().map(|h| hotkey_codes(&h));
        let (ready, started) = mpsc::channel::<Option<u32>>();
        let thread_log = Arc::clone(&log);
        let spawned = thread::Builder::new()
            .name("hotkey".into())
            .spawn(move || run(&session, codes, &thread_log, &ready));
        let thread = match spawned {
            Ok(thread) => thread,
            Err(err) => {
                log.write(LogEvent::Warning {
                    kind: WarningKind::HotkeyThreadFailed,
                    os_code: err.raw_os_error(),
                });
                return Err(err);
            }
        };
        match started.recv() {
            Ok(Some(thread_id)) => Ok(HotkeyThread {
                thread_id,
                thread: Some(thread),
            }),
            // The window failed (logged by the thread, which has ended) or the
            // thread ended without an answer.
            _ => {
                let _ = thread.join();
                Err(io::Error::other("the hotkey thread could not start"))
            }
        }
    }
}

impl Drop for HotkeyThread {
    fn drop(&mut self) {
        // SAFETY: a plain post to the hotkey thread's queue (created with its window).
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// `GetAsyncKeyState(vk)` reads the key down.
fn is_down(vk: u16) -> bool {
    // SAFETY: a plain virtual-key code.
    unsafe { GetAsyncKeyState(i32::from(vk)) < 0 }
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

/// The menu-mask key down and up (R-2), so the user's later Alt release opens no
/// menu in the focused window. A refused send changes nothing else.
fn send_menu_mask() {
    let mask = menu_mask_vk();
    let inputs = [key(mask, false), key(mask, true)];
    // SAFETY: a valid INPUT slice and the size of one element.
    let _ = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
}

/// The hidden top-level window that receives `WM_HOTKEY` (the system STATIC class:
/// no class of our own, never shown).
fn create_window() -> windows::core::Result<HWND> {
    // SAFETY: a system class and static strings; no parent, menu or instance.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("Voicen hotkey"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
        )
    }
}

/// Registers `codes` for `hwnd`; `false` (logged) when the hotkey did not parse or
/// was refused.
fn register(hwnd: HWND, codes: Option<&HotkeyCodes>, log: &Log) -> bool {
    let Some(codes) = codes else {
        log.write(LogEvent::Warning {
            kind: WarningKind::HotkeyRegisterFailed,
            os_code: None,
        });
        return false;
    };
    // SAFETY: a window of this thread; the codes come from win32_data.
    let result = unsafe {
        RegisterHotKey(
            Some(hwnd),
            HOTKEY_ID,
            HOT_KEY_MODIFIERS(codes.modifiers),
            u32::from(codes.vk),
        )
    };
    match result {
        Ok(()) => true,
        Err(err) => {
            log.write(LogEvent::Warning {
                kind: WarningKind::HotkeyRegisterFailed,
                os_code: Some(os_code(&err)),
            });
            false
        }
    }
}

/// The thread body: window, registration, message loop; on quit it unregisters
/// and destroys the window on this thread (the thread that registered).
fn run(
    session: &DictationSession,
    codes: Option<HotkeyCodes>,
    log: &Log,
    ready: &mpsc::Sender<Option<u32>>,
) {
    let hwnd = match create_window() {
        Ok(hwnd) => hwnd,
        Err(err) => {
            log.write(LogEvent::Warning {
                kind: WarningKind::HotkeyThreadFailed,
                os_code: Some(os_code(&err)),
            });
            session.hotkey_registration(false, Instant::now());
            let _ = ready.send(None);
            return;
        }
    };
    let registered = register(hwnd, codes.as_ref(), log);
    // SAFETY: no arguments.
    let thread_id = unsafe { GetCurrentThreadId() };
    let _ = ready.send(Some(thread_id));
    session.hotkey_registration(registered, Instant::now());

    let mut polling = false;
    let mut msg = MSG::default();
    loop {
        // SAFETY: `msg` is valid and writable; this thread's own queue.
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        // 0: WM_QUIT; -1: an error (no valid window filter is used, so not expected).
        if got.0 == 0 || got.0 == -1 {
            break;
        }
        match msg.message {
            WM_HOTKEY if msg.wParam.0 == HOTKEY_ID as usize => {
                let at = Instant::now();
                send_menu_mask();
                session.hotkey_pressed(at);
                if !polling {
                    // SAFETY: a window of this thread; no timer procedure (WM_TIMER).
                    polling = unsafe { SetTimer(Some(hwnd), POLL_TIMER_ID, POLL_MS, None) } != 0;
                    if !polling {
                        // No poll: the hold ends only at the next press (which retries
                        // the timer), so the open microphone is at least on record.
                        log.write(LogEvent::Warning {
                            kind: WarningKind::HotkeyThreadFailed,
                            os_code: Some(os_code(&windows::core::Error::from_thread())),
                        });
                    }
                }
            }
            WM_TIMER if msg.wParam.0 == POLL_TIMER_ID => {
                let up = codes
                    .as_ref()
                    .is_some_and(|codes| released(&codes.poll, is_down));
                if polling && up {
                    let at = Instant::now();
                    // SAFETY: the timer this thread set on its window.
                    let _ = unsafe { KillTimer(Some(hwnd), POLL_TIMER_ID) };
                    polling = false;
                    session.hotkey_released(at);
                }
            }
            _ => {
                // SAFETY: a message this thread just retrieved.
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }
    // SAFETY: the registration, timer and window of this thread, undone once.
    unsafe {
        if polling {
            let _ = KillTimer(Some(hwnd), POLL_TIMER_ID);
        }
        if registered {
            let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID);
        }
        let _ = DestroyWindow(hwnd);
    }
}
