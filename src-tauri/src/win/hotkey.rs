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
//!
//! T-055: the thread is the only code that registers or releases the dictation
//! hotkey. Besides the startup registration it serves the two-step registration of
//! a settings save through [`HotkeyRegistrarHandle`] (a `HotkeyRegistrar`, spec 004
//! R-3): `Prepare` registers the new combination under the spare id next to the
//! active one (`RegisterHotKey` must run on the thread that owns the window), `Commit`
//! releases the active id, makes the spare one active and reports
//! `hotkey_registration(true)`, `Abort` releases the spare id. A hold in progress
//! keeps polling the keys it was pressed with. Requests travel over a channel; a
//! posted `WM_REGISTRAR` wakes the message loop. The thread never takes the settings
//! save lock (it only reads `snapshot()` through the session), so a save waiting on
//! a request cannot deadlock with it.

use std::io;
use std::mem::size_of;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use voicen_core::diag::{Log, LogEvent, WarningKind};
use voicen_core::dictation::DictationSession;
use voicen_core::hotkey_registrar::{HotkeyRegistrar, Prepared, Unavailable};
use voicen_core::settings::hotkey::{parse_hotkey, Hotkey};
use voicen_core::settings::Mode;
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
    SetTimer, TranslateMessage, MSG, WINDOW_EX_STYLE, WM_APP, WM_HOTKEY, WM_QUIT, WM_TIMER,
    WS_OVERLAPPED,
};

use super::os_code;

/// The id of the startup registration (per window). A save's registration takes
/// the other of [`HOTKEY_ID`] and [`SPARE_ID`], so the old and the new combination
/// can be registered at once until the commit (R-3).
const HOTKEY_ID: i32 = 1;
/// The second registration id (see [`HOTKEY_ID`]).
const SPARE_ID: i32 = 2;
/// The thread message that wakes the loop for the queued registrar requests.
const WM_REGISTRAR: u32 = WM_APP + 1;
/// How long a registrar call waits for the hotkey thread. The thread may be inside
/// `hotkey_pressed` (the device open, up to the session's 3 s open budget); a
/// `prepare` not answered in time is `Unavailable`, and the thread then releases
/// what it registered for it.
const REQUEST_BUDGET: Duration = Duration::from_secs(10);
/// The id of the release-poll timer.
const POLL_TIMER_ID: usize = 1;
/// The release poll period (R-1: 10 ms; the system rounds it to its timer tick).
const POLL_MS: u32 = 10;

/// The running hotkey thread. Dropping it ends the thread (quit message, join) and
/// unregisters the hotkey, so the combination is free again once `drop` returns.
pub struct HotkeyThread {
    thread_id: u32,
    requests: mpsc::Sender<Request>,
    thread: Option<JoinHandle<()>>,
}

/// One registrar request into the hotkey thread (T-055).
enum Request {
    /// Register `codes` under the spare id; the reply is the token of the prepared
    /// registration, or `None` when `RegisterHotKey` refused it. A reply nobody
    /// receives any more releases the registration again.
    Prepare {
        codes: HotkeyCodes,
        reply: mpsc::Sender<Option<u64>>,
    },
    /// Make the prepared registration `token` the active one.
    Commit { token: u64, done: mpsc::Sender<()> },
    /// Release the prepared registration `token`.
    Abort { token: u64, done: mpsc::Sender<()> },
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
        let (requests, queue) = mpsc::channel::<Request>();
        let thread_log = Arc::clone(&log);
        let spawned = thread::Builder::new()
            .name("hotkey".into())
            .spawn(move || run(&session, codes, &thread_log, &ready, &queue));
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
                requests,
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

/// The shell's [`HotkeyRegistrar`] (T-055): one instance, created before the
/// settings service (`settings_ipc::load_settings_with`) and given to
/// `start_dictation` (`DictationPorts::hotkeys`), which attaches the started hotkey
/// thread. Fails closed: until a thread is attached, and once it has ended, every
/// `prepare` is `Unavailable` (a save of a new hotkey is refused, never saved
/// unregistered) and `commit` / `abort` do nothing.
pub struct HotkeyRegistrarHandle {
    link: Mutex<Option<Link>>,
}

/// Where the requests of an attached thread go.
#[derive(Clone)]
struct Link {
    thread_id: u32,
    requests: mpsc::Sender<Request>,
}

impl HotkeyRegistrarHandle {
    /// A handle with no thread attached.
    pub fn new() -> Arc<HotkeyRegistrarHandle> {
        Arc::new(HotkeyRegistrarHandle {
            link: Mutex::new(None),
        })
    }

    /// Sends the later registrar calls to `thread` (`start_dictation`).
    pub fn attach(&self, thread: &HotkeyThread) {
        *self.link.lock().unwrap_or_else(PoisonError::into_inner) = Some(Link {
            thread_id: thread.thread_id,
            requests: thread.requests.clone(),
        });
    }

    /// Queues `request` and wakes the thread; `false` when no thread takes it.
    fn post(&self, request: Request) -> bool {
        let link = self
            .link
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(link) = link else {
            return false;
        };
        if link.requests.send(request).is_err() {
            return false;
        }
        // SAFETY: a plain post to the hotkey thread's queue (created with its window).
        unsafe { PostThreadMessageW(link.thread_id, WM_REGISTRAR, WPARAM(0), LPARAM(0)) }.is_ok()
    }

    /// Posts a commit or an abort and waits (bounded) until the thread ran it.
    fn finish(&self, request: impl FnOnce(mpsc::Sender<()>) -> Request) {
        let (done, ran) = mpsc::channel();
        if self.post(request(done)) {
            let _ = ran.recv_timeout(REQUEST_BUDGET);
        }
    }
}

impl HotkeyRegistrar for HotkeyRegistrarHandle {
    fn prepare(&self, hotkey: Hotkey, mode: Mode) -> Result<Prepared, Unavailable> {
        let (reply, answer) = mpsc::channel();
        let request = Request::Prepare {
            codes: hotkey_codes(&hotkey),
            reply,
        };
        if !self.post(request) {
            return Err(Unavailable);
        }
        match answer.recv_timeout(REQUEST_BUDGET) {
            Ok(Some(token)) => Ok(Prepared {
                hotkey,
                mode,
                token,
            }),
            _ => Err(Unavailable),
        }
    }

    fn commit(&self, prepared: Prepared) {
        self.finish(|done| Request::Commit {
            token: prepared.token,
            done,
        });
    }

    fn abort(&self, prepared: Prepared) {
        self.finish(|done| Request::Abort {
            token: prepared.token,
            done,
        });
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

/// Registers `codes` for `hwnd` under `id`; `false` (logged) when the hotkey did not
/// parse or was refused.
fn register(hwnd: HWND, id: i32, codes: Option<&HotkeyCodes>, log: &Log) -> bool {
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
            id,
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

/// Releases the registration `id` of `hwnd` (this thread's window).
fn unregister(hwnd: HWND, id: i32) {
    // SAFETY: a registration this thread made on its own window.
    let _ = unsafe { UnregisterHotKey(Some(hwnd), id) };
}

/// A registration prepared by a save and not yet committed or aborted.
struct Pending {
    token: u64,
    id: i32,
    codes: HotkeyCodes,
}

/// The registrations of the hotkey thread: the active one (the startup one until a
/// commit) and at most one prepared one. Only the hotkey thread holds it.
struct Registrations<'a> {
    hwnd: HWND,
    session: &'a DictationSession,
    log: &'a Log,
    /// The id `WM_HOTKEY` is matched against.
    active_id: i32,
    /// The codes of the active registration (`None`: the startup text did not parse).
    active_codes: Option<HotkeyCodes>,
    /// `active_id` is registered now.
    registered: bool,
    pending: Option<Pending>,
    next_token: u64,
}

impl Registrations<'_> {
    fn handle(&mut self, request: Request) {
        match request {
            Request::Prepare { codes, reply } => {
                // Saves are serialized, so a stale prepared one is never expected;
                // it is released rather than leaked.
                if let Some(stale) = self.pending.take() {
                    unregister(self.hwnd, stale.id);
                }
                let id = if self.active_id == HOTKEY_ID {
                    SPARE_ID
                } else {
                    HOTKEY_ID
                };
                if !register(self.hwnd, id, Some(&codes), self.log) {
                    let _ = reply.send(None);
                    return;
                }
                self.next_token += 1;
                let token = self.next_token;
                if reply.send(Some(token)).is_ok() {
                    self.pending = Some(Pending { token, id, codes });
                } else {
                    // The save stopped waiting (`REQUEST_BUDGET`): it was refused.
                    unregister(self.hwnd, id);
                }
            }
            Request::Commit { token, done } => {
                if let Some(pending) = self.pending.take_if(|p| p.token == token) {
                    if self.registered {
                        unregister(self.hwnd, self.active_id);
                    }
                    self.active_id = pending.id;
                    self.active_codes = Some(pending.codes);
                    self.registered = true;
                    self.session.hotkey_registration(true, Instant::now());
                }
                let _ = done.send(());
            }
            Request::Abort { token, done } => {
                if let Some(pending) = self.pending.take_if(|p| p.token == token) {
                    unregister(self.hwnd, pending.id);
                }
                let _ = done.send(());
            }
        }
    }

    /// Releases every registration of this thread (the end of the thread).
    fn release_all(&mut self) {
        if self.registered {
            unregister(self.hwnd, self.active_id);
            self.registered = false;
        }
        if let Some(pending) = self.pending.take() {
            unregister(self.hwnd, pending.id);
        }
    }
}

/// The thread body: window, registration, message loop; on quit it unregisters
/// and destroys the window on this thread (the thread that registered). The
/// registration result is reported to the session before `ready` is answered, so
/// `start` returns after the report (a startup failure has asked for the settings
/// window before `run()`'s `Ready`, T-055 Q1).
fn run(
    session: &DictationSession,
    codes: Option<HotkeyCodes>,
    log: &Log,
    ready: &mpsc::Sender<Option<u32>>,
    requests: &mpsc::Receiver<Request>,
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
    let registered = register(hwnd, HOTKEY_ID, codes.as_ref(), log);
    session.hotkey_registration(registered, Instant::now());
    // SAFETY: no arguments.
    let thread_id = unsafe { GetCurrentThreadId() };
    let _ = ready.send(Some(thread_id));
    let mut regs = Registrations {
        hwnd,
        session,
        log,
        active_id: HOTKEY_ID,
        active_codes: codes,
        registered,
        pending: None,
        next_token: 0,
    };

    let mut polling = false;
    // The codes of the hold being polled: those of the registration it was pressed
    // with, also when a commit swaps the active one during the hold.
    let mut hold_codes: Option<HotkeyCodes> = None;
    let mut msg = MSG::default();
    loop {
        // SAFETY: `msg` is valid and writable; this thread's own queue.
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        // 0: WM_QUIT; -1: an error (no valid window filter is used, so not expected).
        if got.0 == 0 || got.0 == -1 {
            break;
        }
        match msg.message {
            WM_REGISTRAR => {
                while let Ok(request) = requests.try_recv() {
                    regs.handle(request);
                }
            }
            WM_HOTKEY if msg.wParam.0 == regs.active_id as usize => {
                let at = Instant::now();
                send_menu_mask();
                session.hotkey_pressed(at);
                if !polling {
                    hold_codes = regs.active_codes.clone();
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
                let up = hold_codes
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
    regs.release_all();
    // SAFETY: the timer and window of this thread, undone once.
    unsafe {
        if polling {
            let _ = KillTimer(Some(hwnd), POLL_TIMER_ID);
        }
        let _ = DestroyWindow(hwnd);
    }
}
