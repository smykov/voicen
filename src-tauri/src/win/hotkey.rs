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
//! the OS code; `hotkey_request_timeout`, with none, when a save's commit or abort
//! is not answered in time).
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
//!
//! T-009 (FR-22, OQ-21 (1)): the thread also serves the session's Esc claim through
//! [`CancelKeyHandle`] (the `CancelKey` port), over the same request channel, so a
//! claim or release runs on the thread that owns the window and in queue order with
//! `WM_HOTKEY`, whichever thread ended the recording. A claim registers Esc bare and
//! under the active hotkey's modifiers (the hotkey's keys are held during a hold) and
//! reports each claim's outcome with `DictationSession::cancel_key_result(bare,
//! modified, at)`; core judges a refusal by the live recording's mode (T-212: hold
//! needs the modified claim, toggle the bare one), the thread reads no mode; a release
//! unregisters both. `WM_HOTKEY` on an Esc id calls `esc_pressed`. The thread does
//! not decide when a recording is on: the session asks (it follows
//! `RecordingController::live_id`). Premise (T-174): a recording starts only in
//! `hotkey_pressed`, called only on this thread, so while the thread serves a claim
//! the live recording the session sees is the claim's own (see
//! `Registrations::cancel_key`). T-129 re-checked it for the call hotkey: a call
//! press only runs the call port's press callback, never a session input.
//!
//! T-010 (FR-26, spec 001 US4): the window has a class of our own whose procedure
//! only re-posts the power and session events to this thread's queue (`WM_LIFECYCLE`):
//! `PBT_APMSUSPEND`, `PBT_APMRESUMEAUTOMATIC` (`WM_POWERBROADCAST`) and
//! `WTS_SESSION_UNLOCK` (`WM_WTSSESSION_CHANGE`, after
//! `WTSRegisterSessionNotification`). The message loop acts on them: after a resume
//! or an unlock it re-makes the active registration from its own codes and reports
//! the result with `DictationSession::hotkey_registration` (the only retry besides a
//! save); at a suspend it stops the release poll and calls
//! `DictationSession::suspended`, so the recording that is on ends `Suspended`.
//!
//! T-129 (spec 007 FR-004, FR-040): the thread owns a second registration slot, the
//! call hotkey's, with its own two ids ([`CALL_ID`], [`CALL_SPARE_ID`]). It is
//! registered by [`HotkeyThread::register_call`] (answered synchronously, with a
//! [`CallPort`]), by a save's `prepare_call` / commit / abort (tokens are unique
//! across both slots, so commit and abort find their slot by token), re-made by the
//! same reassert and released by the same `release_all`. A `WM_HOTKEY` on its active
//! id sends the menu mask and runs the port's press callback only: no session input,
//! no release poll, no Esc. No call-slot outcome reaches the dictation session: a
//! failure (logged `call_hotkey_register_failed`) sets the port's
//! [`CallHotkeyMark`] and offers `notice.call_hotkey_conflict` to the port's notifier
//! once per transition to failing; a success clears the mark.

use std::io;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use voicen_core::diag::{Log, LogEvent, WarningKind};
use voicen_core::dictation::DictationSession;
use voicen_core::hotkey_registrar::{HotkeyRegistrar, Prepared, Unavailable};
use voicen_core::i18n::NOTICE_CALL_HOTKEY_CONFLICT;
use voicen_core::platform::{CancelKey, Notifier};
use voicen_core::settings::hotkey::{parse_hotkey, Hotkey};
use voicen_core::settings::{FieldId, Mode};
use voicen_core::win32_data::{hotkey_codes, menu_mask_vk, released, HotkeyCodes};
use windows::core::w;
use windows::Win32::Foundation::{ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, SendInput, UnregisterHotKey, HOT_KEY_MODIFIERS, INPUT,
    INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, MOD_NOREPEAT,
    VIRTUAL_KEY, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer,
    PostMessageW, PostThreadMessageW, RegisterClassExW, SetTimer, TranslateMessage, MSG,
    PBT_APMRESUMEAUTOMATIC, PBT_APMSUSPEND, WINDOW_EX_STYLE, WM_APP, WM_HOTKEY, WM_POWERBROADCAST,
    WM_QUIT, WM_TIMER, WM_WTSSESSION_CHANGE, WNDCLASSEXW, WS_OVERLAPPED, WTS_SESSION_UNLOCK,
};

use super::os_code;

/// The id of the startup registration (per window). A save's registration takes
/// the other of [`HOTKEY_ID`] and [`SPARE_ID`], so the old and the new combination
/// can be registered at once until the commit (R-3).
const HOTKEY_ID: i32 = 1;
/// The second registration id (see [`HOTKEY_ID`]).
const SPARE_ID: i32 = 2;
/// The id of the bare Esc claim (T-009); never [`HOTKEY_ID`] or [`SPARE_ID`].
const ESC_ID: i32 = 3;
/// The id of the Esc claim under the hotkey's modifiers (T-009, OQ-21 (1)).
const ESC_MOD_ID: i32 = 4;
/// The id of the call hotkey's startup registration (T-129); a save's registration
/// takes the other of [`CALL_ID`] and [`CALL_SPARE_ID`]. Distinct from 1-4.
const CALL_ID: i32 = 5;
/// The call slot's second registration id (see [`CALL_ID`]).
const CALL_SPARE_ID: i32 = 6;
/// The thread message that wakes the loop for the queued registrar requests.
const WM_REGISTRAR: u32 = WM_APP + 1;
/// The window message the hotkey window's procedure posts to itself for a power or
/// session event (T-010); `wParam` is one of the `LIFECYCLE_*` kinds.
const WM_LIFECYCLE: u32 = WM_APP + 2;
/// `WM_LIFECYCLE` kind: the machine is going to sleep (`PBT_APMSUSPEND`).
const LIFECYCLE_SUSPEND: usize = 1;
/// `WM_LIFECYCLE` kind: the machine woke up (`PBT_APMRESUMEAUTOMATIC`).
const LIFECYCLE_RESUME: usize = 2;
/// `WM_LIFECYCLE` kind: this session was unlocked (`WTS_SESSION_UNLOCK`).
const LIFECYCLE_UNLOCK: usize = 3;
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
/// unregisters both hotkeys (the dictation and the call one, T-129), so the
/// combinations are free again once `drop` returns.
pub struct HotkeyThread {
    thread_id: u32,
    requests: mpsc::Sender<Request>,
    thread: Option<JoinHandle<()>>,
    /// The log the thread writes to; an attached handle writes its own lines
    /// (a request timeout, T-174) to the same log.
    log: Arc<Log>,
}

/// Which registration slot a request is for (T-129).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotKind {
    /// The dictation hotkey ([`HOTKEY_ID`] / [`SPARE_ID`]).
    Dictation,
    /// The call hotkey ([`CALL_ID`] / [`CALL_SPARE_ID`]).
    Call,
}

/// One registrar request into the hotkey thread (T-055).
enum Request {
    /// Register `codes` under the spare id of `slot`; the reply is the token of the
    /// prepared registration, or `None` when `RegisterHotKey` refused it. A reply
    /// nobody receives any more releases the registration again.
    Prepare {
        slot: SlotKind,
        codes: HotkeyCodes,
        reply: mpsc::Sender<Option<u64>>,
    },
    /// Make the prepared registration `token` (of either slot) the active one.
    Commit { token: u64, done: mpsc::Sender<()> },
    /// Release the prepared registration `token` (of either slot).
    Abort { token: u64, done: mpsc::Sender<()> },
    /// The session's Esc claim (T-009): register (`true`) or release (`false`) Esc.
    CancelKey { claimed: bool },
    /// The call slot's startup registration (T-129): `codes` (`None`: the text did
    /// not parse) under [`CALL_ID`], reported to `port`; `done` answers once it ran.
    StartCall {
        codes: Option<HotkeyCodes>,
        port: CallPort,
        done: mpsc::Sender<()>,
    },
}

/// What the call slot reports to (T-129); moved into the hotkey thread by
/// [`HotkeyThread::register_call`].
pub struct CallPort {
    /// Runs on the hotkey thread for each press of the call hotkey; must not block
    /// (the shell posts `CallRequest::Record(CallSource::Hotkey)`).
    pub on_press: Box<dyn Fn() + Send>,
    /// Offered `notice.call_hotkey_conflict` once per transition to failing.
    pub notifier: Arc<dyn Notifier>,
    /// Set while the call slot is failing, cleared by a success.
    pub mark: Arc<CallHotkeyMark>,
}

/// The call-hotkey mark of the Calls tab (T-129, US9 #3): set while the call slot's
/// registration is failing (startup, reassert), cleared once it is registered.
/// Shell state only; no wire carries it yet.
#[derive(Debug, Default)]
pub struct CallHotkeyMark {
    failing: AtomicBool,
}

impl CallHotkeyMark {
    /// `Some(FieldId::CallHotkey)` while the call slot is failing, else `None`.
    pub fn field(&self) -> Option<FieldId> {
        self.failing
            .load(Ordering::SeqCst)
            .then_some(FieldId::CallHotkey)
    }

    fn set(&self, failing: bool) {
        self.failing.store(failing, Ordering::SeqCst);
    }
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
                log,
            }),
            // The window failed (logged by the thread, which has ended) or the
            // thread ended without an answer.
            _ => {
                let _ = thread.join();
                Err(io::Error::other("the hotkey thread could not start"))
            }
        }
    }

    /// Registers the call hotkey `hotkey` (canonical text, `CallSettings::hotkey`) in
    /// the call slot and hands the thread `port` (T-129). Returns once the thread
    /// ran it (up to `REQUEST_BUDGET`), so the result (a failure's mark and notice
    /// included) is in place before the caller goes on. A hotkey that does not parse
    /// or that `RegisterHotKey` refuses is a call-slot failure (logged
    /// `call_hotkey_register_failed`); the dictation slot is never touched. Before
    /// this call a save's `prepare_call` is refused.
    pub fn register_call(&self, hotkey: &str, port: CallPort) {
        let codes = parse_hotkey(hotkey).ok().map(|h| hotkey_codes(&h));
        let (done, ran) = mpsc::channel();
        let request = Request::StartCall { codes, port, done };
        if Link::to(self).post(request) {
            let _ = ran.recv_timeout(REQUEST_BUDGET);
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

/// Where the requests of an attached thread go, and the thread's log (for the
/// lines the handle writes itself: a request timeout, T-174).
#[derive(Clone)]
struct Link {
    thread_id: u32,
    requests: mpsc::Sender<Request>,
    log: Arc<Log>,
}

impl Link {
    /// The link to `thread`.
    fn to(thread: &HotkeyThread) -> Link {
        Link {
            thread_id: thread.thread_id,
            requests: thread.requests.clone(),
            log: Arc::clone(&thread.log),
        }
    }

    /// Queues `request` for the thread and wakes it (never waits); `false` when no
    /// thread takes it.
    fn post(&self, request: Request) -> bool {
        if self.requests.send(request).is_err() {
            return false;
        }
        // SAFETY: a plain post to the hotkey thread's queue (created with its window).
        unsafe { PostThreadMessageW(self.thread_id, WM_REGISTRAR, WPARAM(0), LPARAM(0)) }.is_ok()
    }
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
        *self.link.lock().unwrap_or_else(PoisonError::into_inner) = Some(Link::to(thread));
    }

    /// Queues `request` and wakes the thread; `false` when no thread takes it.
    fn post(&self, request: Request) -> bool {
        post(&self.link, request)
    }

    /// [`HotkeyRegistrar::commit`] waiting up to `budget` instead of `REQUEST_BUDGET`
    /// (the seam the shell tests reach, T-174; `commit` calls it with
    /// `REQUEST_BUDGET`). A timeout writes one `Warning{HotkeyRequestTimeout}` line;
    /// an ended thread or an unattached handle writes nothing (see `finish`).
    #[doc(hidden)]
    pub fn commit_within(&self, budget: Duration, prepared: Prepared) {
        self.finish(budget, |done| Request::Commit {
            token: prepared.token,
            done,
        });
    }

    /// [`HotkeyRegistrar::abort`] waiting up to `budget` instead of `REQUEST_BUDGET`
    /// (the seam the shell tests reach, T-174; `abort` calls it with
    /// `REQUEST_BUDGET`). A timeout writes one `Warning{HotkeyRequestTimeout}` line;
    /// an ended thread or an unattached handle writes nothing (see `finish`).
    #[doc(hidden)]
    pub fn abort_within(&self, budget: Duration, prepared: Prepared) {
        self.finish(budget, |done| Request::Abort {
            token: prepared.token,
            done,
        });
    }

    /// Posts a commit or an abort and waits up to `budget` until the thread ran it.
    /// Not answered within `budget` (the thread is busy, e.g. inside
    /// `hotkey_pressed`): one `Warning{HotkeyRequestTimeout}` line with no OS code
    /// and no key, to the attached thread's log (T-174); the request stays queued
    /// and runs late, so until then the registered hotkey may differ from the
    /// saved one. A thread that ended before answering writes nothing here (its
    /// end released every registration). Unattached: nothing is posted or written.
    fn finish(&self, budget: Duration, request: impl FnOnce(mpsc::Sender<()>) -> Request) {
        let (done, ran) = mpsc::channel();
        if !self.post(request(done)) {
            return;
        }
        if let Err(mpsc::RecvTimeoutError::Timeout) = ran.recv_timeout(budget) {
            let log = self
                .link
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
                .map(|link| Arc::clone(&link.log));
            if let Some(log) = log {
                log.write(LogEvent::Warning {
                    kind: WarningKind::HotkeyRequestTimeout,
                    os_code: None,
                });
            }
        }
    }
}

/// Queues `request` for the thread attached to `link` and wakes it (never waits);
/// `false` when no thread takes it.
fn post(link: &Mutex<Option<Link>>, request: Request) -> bool {
    let link = link.lock().unwrap_or_else(PoisonError::into_inner).clone();
    link.is_some_and(|link| link.post(request))
}

/// The shell's [`CancelKey`] (T-009, FR-22): `start_dictation` creates it, gives it
/// to the session (`SessionDeps::cancel_key`) and attaches the started hotkey thread.
/// `set` only queues a request and posts a thread message (it never waits and never
/// calls the session; the session calls it under its lock). Until a thread is
/// attached, and once it has ended, `set` does nothing: Esc is never claimed.
pub struct CancelKeyHandle {
    link: Mutex<Option<Link>>,
}

impl CancelKeyHandle {
    /// A handle with no thread attached.
    pub fn new() -> Arc<CancelKeyHandle> {
        Arc::new(CancelKeyHandle {
            link: Mutex::new(None),
        })
    }

    /// Sends the later claims to `thread` (`start_dictation`).
    pub fn attach(&self, thread: &HotkeyThread) {
        *self.link.lock().unwrap_or_else(PoisonError::into_inner) = Some(Link::to(thread));
    }
}

impl CancelKey for CancelKeyHandle {
    fn set(&self, claimed: bool) {
        let _ = post(&self.link, Request::CancelKey { claimed });
    }
}

impl HotkeyRegistrarHandle {
    /// A `Prepare` for `slot`, waiting up to `REQUEST_BUDGET` for the token.
    fn prepare_in(
        &self,
        slot: SlotKind,
        hotkey: Hotkey,
        mode: Mode,
    ) -> Result<Prepared, Unavailable> {
        let (reply, answer) = mpsc::channel();
        let request = Request::Prepare {
            slot,
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
}

impl HotkeyRegistrar for HotkeyRegistrarHandle {
    fn prepare(&self, hotkey: Hotkey, mode: Mode) -> Result<Prepared, Unavailable> {
        self.prepare_in(SlotKind::Dictation, hotkey, mode)
    }

    /// T-129: the call slot's prepare. The call hotkey has no mode; the returned
    /// `Prepared::mode` is `Toggle` and is not used by the thread (commit and abort
    /// go by token). `Unavailable` also before [`HotkeyThread::register_call`] ran.
    fn prepare_call(&self, hotkey: Hotkey) -> Result<Prepared, Unavailable> {
        self.prepare_in(SlotKind::Call, hotkey, Mode::Toggle)
    }

    fn commit(&self, prepared: Prepared) {
        self.commit_within(REQUEST_BUDGET, prepared);
    }

    fn abort(&self, prepared: Prepared) {
        self.abort_within(REQUEST_BUDGET, prepared);
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

/// The class of the hotkey window (T-010): ours, so its procedure is
/// [`hotkey_window_proc`].
const CLASS_NAME: windows::core::PCWSTR = w!("VoicenHotkey");

/// The hotkey window's procedure (T-010). It only re-posts the power and session
/// events the loop acts on to this window's queue as [`WM_LIFECYCLE`], so they are
/// handled by the loop in order with `WM_HOTKEY` and the registrar requests, never
/// inside a sent message. Everything else goes to `DefWindowProcW`.
unsafe extern "system" fn hotkey_window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let kind = match msg {
        WM_POWERBROADCAST => match u32::try_from(wparam.0) {
            Ok(PBT_APMSUSPEND) => Some(LIFECYCLE_SUSPEND),
            // PBT_APMRESUMESUSPEND (after user input) follows the automatic one and
            // is ignored, so one resume makes one retry.
            Ok(PBT_APMRESUMEAUTOMATIC) => Some(LIFECYCLE_RESUME),
            _ => None,
        },
        WM_WTSSESSION_CHANGE if wparam.0 == WTS_SESSION_UNLOCK as usize => Some(LIFECYCLE_UNLOCK),
        WM_WTSSESSION_CHANGE => None,
        // SAFETY: the arguments this procedure was called with.
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    };
    if let Some(kind) = kind {
        // SAFETY: a post to this thread's own window. A refused post (the queue at
        // its 10 000-message quota) loses the event with no line: an accepted loss,
        // docs/decisions/windows-shell.md invariant 1 (T-174); the next lifecycle
        // event retries the reassert.
        let _ = unsafe { PostMessageW(Some(hwnd), WM_LIFECYCLE, WPARAM(kind), LPARAM(0)) };
    }
    // WM_POWERBROADCAST: TRUE (processed); WM_WTSSESSION_CHANGE: the return is unused.
    LRESULT(1)
}

/// The hidden top-level window that receives `WM_HOTKEY` and the power and session
/// events (a class of our own, [`CLASS_NAME`], registered once per process; never
/// shown, so the install smoke's census does not see it).
fn create_window() -> windows::core::Result<HWND> {
    // SAFETY: no module name: the handle of this process's executable.
    let instance = unsafe { GetModuleHandleW(None) }?.into();
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(hotkey_window_proc),
        hInstance: instance,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    // SAFETY: a valid class description with a static name and a procedure of ours.
    if unsafe { RegisterClassExW(&class) } == 0 {
        let err = windows::core::Error::from_thread();
        // A later hotkey thread of the same process finds the class registered.
        if err.code() != ERROR_CLASS_ALREADY_EXISTS.to_hresult() {
            return Err(err);
        }
    }
    // SAFETY: the class registered above and static strings; no parent or menu.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            w!("Voicen hotkey"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
    }
}

/// Registers `codes` for `hwnd` under `id`; `false` (logged as `kind`, the slot's
/// failure kind) when the hotkey did not parse or was refused.
fn register(
    hwnd: HWND,
    id: i32,
    codes: Option<&HotkeyCodes>,
    kind: WarningKind,
    log: &Log,
) -> bool {
    let Some(codes) = codes else {
        log.write(LogEvent::Warning {
            kind,
            os_code: None,
        });
        return false;
    };
    match try_register(hwnd, id, codes) {
        Ok(()) => true,
        Err(err) => {
            log.write(LogEvent::Warning {
                kind,
                os_code: Some(os_code(&err)),
            });
            false
        }
    }
}

/// `RegisterHotKey` of `codes` for `hwnd` (this thread's window) under `id`.
fn try_register(hwnd: HWND, id: i32, codes: &HotkeyCodes) -> windows::core::Result<()> {
    // SAFETY: a window of this thread; the codes come from win32_data.
    unsafe {
        RegisterHotKey(
            Some(hwnd),
            id,
            HOT_KEY_MODIFIERS(codes.modifiers),
            u32::from(codes.vk),
        )
    }
}

/// Esc under `modifiers` (`MOD_*` bits, `MOD_NOREPEAT` included) as registration
/// codes (T-009); the poll groups are unused (Esc is never polled).
fn esc_codes(modifiers: u32) -> HotkeyCodes {
    HotkeyCodes {
        modifiers,
        vk: VK_ESCAPE.0,
        poll: Vec::new(),
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

/// One registration slot of the hotkey thread (T-129: the dictation hotkey's and the
/// call hotkey's): the active registration (the startup one until a commit) and at
/// most one prepared one, under the slot's own two ids.
struct Slot {
    /// The slot's startup id and its spare id; a prepare takes the one not active.
    ids: [i32; 2],
    /// The warning kind of a failed registration of this slot.
    kind: WarningKind,
    /// The id `WM_HOTKEY` is matched against.
    active_id: i32,
    /// The codes of the active registration (`None`: the startup text did not parse).
    codes: Option<HotkeyCodes>,
    /// `active_id` is registered now.
    registered: bool,
    pending: Option<Pending>,
}

impl Slot {
    /// A slot with nothing registered, `codes` for its startup id.
    fn new(ids: [i32; 2], kind: WarningKind, codes: Option<HotkeyCodes>) -> Slot {
        Slot {
            ids,
            kind,
            active_id: ids[0],
            codes,
            registered: false,
            pending: None,
        }
    }

    /// The id a prepare registers under: the one of `ids` that is not active.
    fn spare_id(&self) -> i32 {
        if self.active_id == self.ids[0] {
            self.ids[1]
        } else {
            self.ids[0]
        }
    }

    /// (Re-)makes the active registration from the active codes (startup, and T-010
    /// after a resume or an unlock): unregister if registered, then register under
    /// the same id (a failure is logged by [`register`]). The result.
    fn register_active(&mut self, hwnd: HWND, log: &Log) -> bool {
        if self.registered {
            unregister(hwnd, self.active_id);
        }
        self.registered = register(hwnd, self.active_id, self.codes.as_ref(), self.kind, log);
        self.registered
    }

    /// Registers `codes` under the spare id next to the active one and replies with
    /// `token`, or `None` when `RegisterHotKey` refused it. Saves are serialized, so
    /// a stale prepared one is never expected; it is released rather than leaked.
    fn prepare(
        &mut self,
        hwnd: HWND,
        log: &Log,
        codes: HotkeyCodes,
        token: u64,
        reply: &mpsc::Sender<Option<u64>>,
    ) {
        if let Some(stale) = self.pending.take() {
            unregister(hwnd, stale.id);
        }
        let id = self.spare_id();
        if !register(hwnd, id, Some(&codes), self.kind, log) {
            let _ = reply.send(None);
            return;
        }
        if reply.send(Some(token)).is_ok() {
            self.pending = Some(Pending { token, id, codes });
        } else {
            // The save stopped waiting (`REQUEST_BUDGET`): it was refused.
            unregister(hwnd, id);
        }
    }

    /// Makes the prepared registration `token` the active one and releases the old
    /// one; `false` when `token` is not this slot's prepared one.
    fn commit(&mut self, hwnd: HWND, token: u64) -> bool {
        let Some(pending) = self.pending.take_if(|p| p.token == token) else {
            return false;
        };
        if self.registered {
            unregister(hwnd, self.active_id);
        }
        self.active_id = pending.id;
        self.codes = Some(pending.codes);
        self.registered = true;
        true
    }

    /// Releases the prepared registration `token`, if it is this slot's.
    fn abort(&mut self, hwnd: HWND, token: u64) {
        if let Some(pending) = self.pending.take_if(|p| p.token == token) {
            unregister(hwnd, pending.id);
        }
    }

    /// Releases the active and the prepared registration of this slot.
    fn release_all(&mut self, hwnd: HWND) {
        if self.registered {
            unregister(hwnd, self.active_id);
            self.registered = false;
        }
        if let Some(pending) = self.pending.take() {
            unregister(hwnd, pending.id);
        }
    }
}

/// The registrations of the hotkey thread: the dictation slot, the call slot (T-129)
/// and the Esc claims. Only the hotkey thread holds it.
struct Registrations<'a> {
    hwnd: HWND,
    session: &'a DictationSession,
    log: &'a Log,
    /// The dictation hotkey ([`HOTKEY_ID`] / [`SPARE_ID`]); its outcomes go to the
    /// session.
    dictation: Slot,
    /// The call hotkey ([`CALL_ID`] / [`CALL_SPARE_ID`]); its outcomes go to
    /// `call_port` only, never to the session.
    call: Slot,
    /// Set by the call slot's startup request; until then a call prepare is refused.
    call_port: Option<CallPort>,
    /// The call slot's last registration failed (one notice per transition to it).
    call_failing: bool,
    /// The last token handed out, for either slot (tokens are unique across slots).
    next_token: u64,
    /// The Esc claims registered now (T-009): bare ([`ESC_ID`]) and under the
    /// hotkey's modifiers ([`ESC_MOD_ID`]).
    esc: bool,
    esc_mod: bool,
}

impl Registrations<'_> {
    fn handle(&mut self, request: Request) {
        match request {
            Request::Prepare { slot, codes, reply } => {
                if slot == SlotKind::Call && self.call_port.is_none() {
                    // No startup registration yet: nothing would serve its presses.
                    let _ = reply.send(None);
                    return;
                }
                self.next_token += 1;
                let token = self.next_token;
                let target = match slot {
                    SlotKind::Dictation => &mut self.dictation,
                    SlotKind::Call => &mut self.call,
                };
                target.prepare(self.hwnd, self.log, codes, token, &reply);
            }
            Request::Commit { token, done } => {
                if self.dictation.commit(self.hwnd, token) {
                    self.session.hotkey_registration(true, Instant::now());
                } else if self.call.commit(self.hwnd, token) {
                    self.call_result(true);
                }
                let _ = done.send(());
            }
            Request::Abort { token, done } => {
                self.dictation.abort(self.hwnd, token);
                self.call.abort(self.hwnd, token);
                let _ = done.send(());
            }
            Request::CancelKey { claimed } => self.cancel_key(claimed),
            Request::StartCall { codes, port, done } => {
                // One startup request is expected; a later one replaces the slot.
                self.call.release_all(self.hwnd);
                self.call = Slot::new(
                    [CALL_ID, CALL_SPARE_ID],
                    WarningKind::CallHotkeyRegisterFailed,
                    codes,
                );
                self.call_port = Some(port);
                self.call_failing = false;
                let registered = self.call.register_active(self.hwnd, self.log);
                self.call_result(registered);
                let _ = done.send(());
            }
        }
    }

    /// A call-slot registration result (T-129, I3): the mark follows it, and a
    /// transition to failing offers `notice.call_hotkey_conflict` once. Nothing
    /// reaches the dictation session.
    fn call_result(&mut self, registered: bool) {
        let Some(port) = &self.call_port else {
            return;
        };
        port.mark.set(!registered);
        if !registered && !self.call_failing {
            port.notifier.notify(NOTICE_CALL_HOTKEY_CONFLICT, &[], None);
        }
        self.call_failing = !registered;
    }

    /// A press of the call hotkey (T-129, I2): the port's press callback only.
    fn call_pressed(&self) {
        if let Some(port) = &self.call_port {
            (port.on_press)();
        }
    }

    /// The session's Esc claim (T-009). A claim registers Esc bare and under the
    /// active hotkey's modifiers and reports to the session each one's outcome,
    /// separately (T-212). The session decides what a refusal means by the live
    /// recording's mode (hold: only the modified claim can cancel; toggle: only
    /// the bare one); this thread reads no mode. A refusal is not logged here: the
    /// session's `Warning{EscUnavailable}` is its line, and what was registered
    /// stays. A release unregisters what is registered and reports nothing.
    ///
    /// Premise (T-009, T-174): `cancel_key_result` judges *any* live recording,
    /// and that equals "the claim's own recording" only because a recording starts only in `hotkey_pressed`, which is called only
    /// on this thread (the `WM_HOTKEY` arm of `run`); while this thread serves
    /// a claim no later recording can start. T-129 re-checked it for the call
    /// hotkey: its press only runs the call port's callback (a call recording, not
    /// a dictation one), never `hotkey_pressed`.
    fn cancel_key(&mut self, claimed: bool) {
        if !claimed {
            self.release_esc();
            return;
        }
        if !self.esc {
            self.esc = try_register(self.hwnd, ESC_ID, &esc_codes(MOD_NOREPEAT.0)).is_ok();
        }
        if !self.esc_mod {
            self.esc_mod = self.dictation.codes.as_ref().is_some_and(|codes| {
                let modded = esc_codes(codes.modifiers | MOD_NOREPEAT.0);
                try_register(self.hwnd, ESC_MOD_ID, &modded).is_ok()
            });
        }
        self.session
            .cancel_key_result(self.esc, self.esc_mod, Instant::now());
    }

    /// Unregisters the Esc claims that are registered.
    fn release_esc(&mut self) {
        if self.esc {
            unregister(self.hwnd, ESC_ID);
            self.esc = false;
        }
        if self.esc_mod {
            unregister(self.hwnd, ESC_MOD_ID);
            self.esc_mod = false;
        }
    }

    /// Re-makes the active registration of each slot from its active codes (T-010:
    /// after a resume or an unlock): the dictation slot's result goes to the
    /// session, the call slot's (once its startup request ran, T-129) to
    /// [`call_result`](Self::call_result) only. A failure is logged by [`register`].
    /// Pending prepares and the Esc claims are left alone.
    fn reassert(&mut self) {
        let registered = self.dictation.register_active(self.hwnd, self.log);
        self.session.hotkey_registration(registered, Instant::now());
        if self.call_port.is_some() {
            let registered = self.call.register_active(self.hwnd, self.log);
            self.call_result(registered);
        }
    }

    /// Releases every registration of this thread, both slots and Esc (the end of
    /// the thread).
    fn release_all(&mut self) {
        self.release_esc();
        self.dictation.release_all(self.hwnd);
        self.call.release_all(self.hwnd);
    }
}

/// The thread body: window, registration, session notifications (T-010), message
/// loop; on quit it unregisters, ends the session notifications and destroys the
/// window on this thread (the thread that registered). The
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
    let mut dictation = Slot::new(
        [HOTKEY_ID, SPARE_ID],
        WarningKind::HotkeyRegisterFailed,
        codes,
    );
    let registered = dictation.register_active(hwnd, log);
    session.hotkey_registration(registered, Instant::now());
    // T-010: unlock events reach the window only after this; a failure is logged
    // (before `start` returns) and the hotkey keeps working (resume still
    // re-registers).
    // SAFETY: this thread's own window.
    let sessions = match unsafe { WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) } {
        Ok(()) => true,
        Err(err) => {
            log.write(LogEvent::Warning {
                kind: WarningKind::HotkeyThreadFailed,
                os_code: Some(os_code(&err)),
            });
            false
        }
    };
    // SAFETY: no arguments.
    let thread_id = unsafe { GetCurrentThreadId() };
    let _ = ready.send(Some(thread_id));
    let mut regs = Registrations {
        hwnd,
        session,
        log,
        dictation,
        // Empty until the call slot's startup request (T-129).
        call: Slot::new(
            [CALL_ID, CALL_SPARE_ID],
            WarningKind::CallHotkeyRegisterFailed,
            None,
        ),
        call_port: None,
        call_failing: false,
        next_token: 0,
        esc: false,
        esc_mod: false,
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
            WM_HOTKEY if msg.wParam.0 == regs.dictation.active_id as usize => {
                let at = Instant::now();
                send_menu_mask();
                session.hotkey_pressed(at);
                if !polling {
                    hold_codes = regs.dictation.codes.clone();
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
            // T-129 (I2): the call hotkey: the menu mask and the port's press callback
            // only; no session input, no release poll, no Esc.
            WM_HOTKEY if msg.wParam.0 == regs.call.active_id as usize => {
                send_menu_mask();
                regs.call_pressed();
            }
            // T-009: a claimed Esc (bare or under the hotkey's modifiers).
            WM_HOTKEY if msg.wParam.0 == ESC_ID as usize || msg.wParam.0 == ESC_MOD_ID as usize => {
                session.esc_pressed(Instant::now());
            }
            // T-010: a power or session event re-posted by the window procedure.
            WM_LIFECYCLE if msg.wParam.0 == LIFECYCLE_SUSPEND => {
                if polling {
                    // SAFETY: the timer this thread set on its window.
                    let _ = unsafe { KillTimer(Some(hwnd), POLL_TIMER_ID) };
                    polling = false;
                }
                hold_codes = None;
                session.suspended(Instant::now());
            }
            WM_LIFECYCLE
                if msg.wParam.0 == LIFECYCLE_RESUME || msg.wParam.0 == LIFECYCLE_UNLOCK =>
            {
                regs.reassert();
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
    // SAFETY: the timer, session notification and window of this thread, undone once.
    unsafe {
        if sessions {
            let _ = WTSUnRegisterSessionNotification(hwnd);
        }
        if polling {
            let _ = KillTimer(Some(hwnd), POLL_TIMER_ID);
        }
        let _ = DestroyWindow(hwnd);
    }
}
