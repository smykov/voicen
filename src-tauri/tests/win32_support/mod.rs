//! Shared helpers of T-006's Windows CI tests (`hotkey.rs`, `clipboard.rs`, `paste.rs`,
//! `dictation_e2e.rs`, `delivery.rs`): test windows on their own thread, injected keys,
//! clipboard read-back, the loud runner preconditions, a fixed-text test engine and the
//! app rig over `build_app` + `start_dictation`. T-057's `overlay.rs` adds the focus-loss
//! record of a test window ([`TestWindow::focus_losses`]).
//!
//! Runner capabilities: every helper that needs one asserts it through [`precondition`],
//! which names the capability and `docs/decisions/windows-ci-runner.md` (all `ok` in runs A
//! and B, T-059), or, for `foreground_again` (the probe re-measures it in every job), through
//! [`precondition_rechecked`]. Timing (F-005, T-006 Refresh 2): `SendInput` and `WM_HOTKEY` took ~1 s
//! in run B, so every observation waits at least [`WAIT`] (3 s), and holds are timed from
//! the observed press, never from the `SendInput` call.
//!
//! Fake data only: `sk-test-…` keys, texts with an obvious canary.
#![allow(dead_code)]

use std::ffi::c_void;
use std::fmt::Display;
use std::mem::{size_of, size_of_val};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::{App, Manager};
use voicen_core::audio::AudioBuffer;
use voicen_core::autostart::{Autostart, FakeAutostart};
use voicen_core::diag::Log;
use voicen_core::dictation::DictationSession;
use voicen_core::engine::{Engine, TranscribeRequest};
use voicen_core::failure::FailureReason;
use voicen_core::local_models::catalog::MODELS;
use voicen_core::local_models::service::LocalModels;
use voicen_core::local_models::store::ModelStore;
use voicen_core::pipeline::EngineFactory;
use voicen_core::recording::OverlayState;
use voicen_core::secrets::{CredentialStore, FakeCredentialStore, KeyEdits, KeySlot};
use voicen_core::settings::hotkey::parse_hotkey;
use voicen_core::settings::service::{SaveOutcome, SaveRequest, SettingsService};
use voicen_core::settings::{EngineKind, Settings};
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_core::win32_data::hotkey_codes;
use voicen_lib::settings_ipc::{load_settings, load_settings_with};
use voicen_lib::win::hotkey::HotkeyRegistrarHandle;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, SendInput, UnregisterHotKey, HOT_KEY_MODIFIERS, INPUT,
    INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, MOD_ALT, MOD_CONTROL,
    MOD_NOREPEAT, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, CreateWindowExW, DestroyWindow, DispatchMessageW, GetClassNameW,
    GetForegroundWindow, GetWindowTextW, PeekMessageW, SetForegroundWindow, SetWindowLongPtrW,
    TranslateMessage, CW_USEDEFAULT, GWLP_WNDPROC, HWND_MESSAGE, MSG, PM_REMOVE, SC_KEYMENU,
    WA_INACTIVE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_ACTIVATE, WM_CHAR, WM_ENTERMENULOOP,
    WM_INITMENU, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_NCACTIVATE, WM_PASTE, WM_SYSCHAR,
    WM_SYSCOMMAND, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDPROC, WS_CAPTION, WS_OVERLAPPEDWINDOW, WS_POPUP,
    WS_VISIBLE,
};

/// The shortest wait for anything the runner does (run B: ~1 s for `SendInput` and
/// `WM_HOTKEY`; F-005).
pub const WAIT: Duration = Duration::from_secs(3);

/// The wait for a whole dictation (engine, delivery, log line).
pub const BUDGET: Duration = Duration::from_secs(15);

/// How long a hold lasts, counted from the observed press (well above the 0.3 s
/// minimum hold, so a slow release poll can never turn it into `too_short`).
pub const HOLD: Duration = Duration::from_millis(1500);

/// A bounded negative watches this long for a change that must not come.
pub const SETTLE: Duration = Duration::from_secs(1);

/// A held fake gives up after this, so a failing test cannot hang the exe.
pub const HOLD_LIMIT: Duration = Duration::from_secs(30);

pub const VK_SHIFT: u16 = 0x10;
pub const VK_CONTROL: u16 = 0x11;
pub const VK_MENU: u16 = 0x12;
pub const VK_SPACE: u16 = 0x20;
/// T-009: the cancel key (FR-22).
pub const VK_ESCAPE: u16 = 0x1B;
pub const VK_V: u16 = 0x56;
pub const VK_LWIN: u16 = 0x5B;
pub const VK_RWIN: u16 = 0x5C;
/// The unassigned key T-006 R-2 uses to keep an Alt release from opening a menu; the
/// tests send it only in their own cleanup, never as part of a hold.
pub const VK_MASK: u16 = 0xE8;

/// The default hotkey (`Settings::hotkey` default) as its keys, in press order.
pub const HOTKEY_KEYS: [u16; 3] = [VK_CONTROL, VK_MENU, VK_SPACE];

/// The id the tests register their own Ctrl+Alt+Space with (thread-associated).
pub const TEST_HOTKEY_ID: i32 = 0x0A06;

/// A fake transcription key (never a real one).
pub const FAKE_KEY: &str = "sk-test-T006-FAKE-KEY";

/// Each binary serialises its tests: the clipboard, the foreground window, injected
/// keys and the global hotkey are per session.
static SERIAL: Mutex<()> = Mutex::new(());

pub fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A runner capability this test relies on (`docs/decisions/windows-ci-runner.md`):
/// panics with a message naming it, so a runner change never reads as a code defect
/// and the test never passes vacuously.
#[track_caller]
pub fn precondition(capability: &str, holds: bool, detail: impl Display) {
    assert!(
        holds,
        "runner precondition `{capability}` does not hold (docs/decisions/windows-ci-runner.md \
         records it ok in runs A and B; a change of the runner image moves this test's \
         Acceptance line to the owner): {detail}"
    );
}

/// A runner fact the probe re-measures in every Windows CI job, used as a precondition
/// from its first recorded run (the orchestrator's exception for `foreground_again`,
/// docs/decisions/windows-ci-runner.md): the message points at the same job's probe line
/// instead of runs A and B.
#[track_caller]
pub fn precondition_rechecked(fact: &str, holds: bool, detail: impl Display) {
    assert!(
        holds,
        "runner precondition `{fact}` does not hold (docs/decisions/windows-ci-runner.md; \
         re-measured by the probe step of this same job, line `runner-probe: {fact}=…`: \
         not `ok` there means the runner changed and this test's Acceptance line moves to \
         the owner; `ok` there means this test's path differs from the probe's): {detail}"
    );
}

/// Dispatches every message queued for this thread (the mock app's main thread is the
/// test thread: tray renders and window builds may wait for it).
pub fn pump() {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid, writable MSG; only this thread's own queue is read.
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Polls `cond` every 10 ms (pumping this thread) until it holds or `within` passed.
pub fn eventually(within: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + within;
    loop {
        pump();
        if cond() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Pumps for `SETTLE`; false as soon as `cond` fails (a bounded negative).
pub fn holds_for(within: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        pump();
        if !cond() {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
    pump();
    cond()
}

// ---- keys ---------------------------------------------------------------------------

pub fn key(vk: u16, up: bool) -> INPUT {
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

/// `SendInput` of `inputs`; the inserted count.
pub fn send(inputs: &[INPUT]) -> u32 {
    // SAFETY: a valid INPUT slice and the size of one element.
    unsafe { SendInput(inputs, size_of::<INPUT>() as i32) }
}

/// `GetAsyncKeyState(vk)` reads the key down.
pub fn is_down(vk: u16) -> bool {
    // SAFETY: a plain virtual-key code.
    let state = unsafe { GetAsyncKeyState(i32::from(vk)) };
    state < 0
}

/// Keys the test pressed: released on drop (mask key first, so an Alt or Win release
/// opens no menu), so no path leaves an injected key down.
pub struct Keys {
    down: Vec<u16>,
}

impl Keys {
    /// Presses `vks` in order with one `SendInput` and waits until `GetAsyncKeyState`
    /// reads every one down (capabilities `sendinput` and `async_keys`).
    #[track_caller]
    pub fn press(vks: &[u16]) -> Keys {
        let inputs: Vec<INPUT> = vks.iter().map(|&vk| key(vk, false)).collect();
        let keys = Keys { down: vks.to_vec() };
        let inserted = send(&inputs);
        precondition(
            "sendinput",
            inserted as usize == inputs.len(),
            format!(
                "SendInput inserted {inserted} of {} key-downs",
                inputs.len()
            ),
        );
        let all_down = eventually(WAIT, || vks.iter().all(|&vk| is_down(vk)));
        precondition(
            "async_keys",
            all_down,
            format!("GetAsyncKeyState did not read {vks:x?} down within {WAIT:?}"),
        );
        keys
    }

    /// Releases `vks` (and forgets them), one `SendInput`; waits until each reads up.
    #[track_caller]
    pub fn release(&mut self, vks: &[u16]) {
        let inputs: Vec<INPUT> = vks.iter().map(|&vk| key(vk, true)).collect();
        let inserted = send(&inputs);
        self.down.retain(|vk| !vks.contains(vk));
        precondition(
            "sendinput",
            inserted as usize == inputs.len(),
            format!("SendInput inserted {inserted} of {} key-ups", inputs.len()),
        );
        let all_up = eventually(WAIT, || vks.iter().all(|&vk| !is_down(vk)));
        precondition(
            "async_keys",
            all_up,
            format!("GetAsyncKeyState did not read {vks:x?} up within {WAIT:?}"),
        );
    }

    /// Releases everything still down.
    pub fn release_all(&mut self) {
        let down = std::mem::take(&mut self.down);
        if !down.is_empty() {
            let up: Vec<u16> = down.iter().rev().copied().collect();
            self.down = down;
            self.release(&up);
        }
    }
}

impl Drop for Keys {
    fn drop(&mut self) {
        if self.down.is_empty() {
            return;
        }
        let mut inputs = vec![key(VK_MASK, false), key(VK_MASK, true)];
        inputs.extend(self.down.iter().rev().map(|&vk| key(vk, true)));
        send(&inputs);
    }
}

/// Registers a combination for this thread (no window): it is then taken for everyone
/// else. Unregistered on drop (on the thread that took it).
pub struct TakenHotkey {
    id: i32,
}

/// The ids of [`TakenHotkey::take_combo`] (after [`TEST_HOTKEY_ID`], which `take` uses).
static NEXT_TAKEN_ID: AtomicI32 = AtomicI32::new(TEST_HOTKEY_ID + 1);

impl TakenHotkey {
    /// Takes Ctrl+Alt+Space (the default hotkey).
    pub fn take() -> windows::core::Result<TakenHotkey> {
        // SAFETY: a thread-associated registration (no window), undone on drop.
        unsafe {
            RegisterHotKey(
                None,
                TEST_HOTKEY_ID,
                MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                u32::from(VK_SPACE),
            )
        }?;
        Ok(TakenHotkey { id: TEST_HOTKEY_ID })
    }

    /// T-055: takes any combination in canonical text (`"Ctrl+Alt+F9"`), with the codes
    /// the hotkey thread would register (`win32_data::hotkey_codes`), under an id of its
    /// own, so it can be held next to [`take`](Self::take).
    pub fn take_combo(text: &str) -> windows::core::Result<TakenHotkey> {
        let hotkey = parse_hotkey(text).expect("a canonical test hotkey");
        let codes = hotkey_codes(&hotkey);
        let id = NEXT_TAKEN_ID.fetch_add(1, Ordering::SeqCst);
        // SAFETY: a thread-associated registration (no window), undone on drop.
        unsafe {
            RegisterHotKey(
                None,
                id,
                HOT_KEY_MODIFIERS(codes.modifiers),
                u32::from(codes.vk),
            )
        }?;
        Ok(TakenHotkey { id })
    }
}

impl TakenHotkey {
    /// T-009: takes Esc under `modifiers` (`MOD_*` bits; 0 = bare Esc) for this thread,
    /// under an id of its own: the claim the hotkey thread makes while recording.
    pub fn take_esc(modifiers: u32) -> windows::core::Result<TakenHotkey> {
        let id = NEXT_TAKEN_ID.fetch_add(1, Ordering::SeqCst);
        // SAFETY: a thread-associated registration (no window), undone on drop.
        unsafe {
            RegisterHotKey(
                None,
                id,
                HOT_KEY_MODIFIERS(modifiers) | MOD_NOREPEAT,
                u32::from(VK_ESCAPE),
            )
        }?;
        Ok(TakenHotkey { id })
    }
}

/// T-009: Esc under `modifiers` is free right now (this thread can take it, then frees
/// it again); `false` while the hotkey thread holds the claim.
pub fn esc_is_free(modifiers: u32) -> bool {
    TakenHotkey::take_esc(modifiers).is_ok()
}

impl Drop for TakenHotkey {
    fn drop(&mut self) {
        // SAFETY: the id this thread registered.
        let _ = unsafe { UnregisterHotKey(None, self.id) };
    }
}

/// T-055: `text` is free right now (this thread can take it, then frees it again).
pub fn combo_is_free(text: &str) -> bool {
    TakenHotkey::take_combo(text).is_ok()
}

/// Asserts that nobody holds Ctrl+Alt+Space (capability `hotkey`: registration): this
/// thread can register it, then frees it again.
#[track_caller]
pub fn assert_hotkey_free() {
    let taken = TakenHotkey::take();
    precondition(
        "hotkey (registration)",
        taken.is_ok(),
        format!(
            "Ctrl+Alt+Space is already registered by someone ({:?})",
            taken.as_ref().err()
        ),
    );
}

// ---- test windows -------------------------------------------------------------------

/// Messages the recorder keeps: keys, characters, the menu path and pastes.
const RECORDED: [u32; 10] = [
    WM_KEYDOWN,
    WM_KEYUP,
    WM_SYSKEYDOWN,
    WM_SYSKEYUP,
    WM_CHAR,
    WM_SYSCHAR,
    WM_SYSCOMMAND,
    WM_ENTERMENULOOP,
    WM_INITMENU,
    WM_PASTE,
];

/// `(hwnd, message, wParam)` of every recorded message of every test window.
static RECORD: Mutex<Vec<(isize, u32, usize)>> = Mutex::new(Vec::new());

/// T-057: `(hwnd, message, wParam)` of every focus or activation loss of every test
/// window (`WM_KILLFOCUS`; `WM_ACTIVATE` with `WA_INACTIVE`; `WM_NCACTIVATE` drawn
/// inactive). Kept apart from [`RECORD`], which `bring_to_front` clears and the key
/// tests read whole.
static FOCUS_LOST: Mutex<Vec<(isize, u32, usize)>> = Mutex::new(Vec::new());

/// True for a message that tells a window it lost the keyboard focus or activation.
fn is_focus_loss(msg: u32, wparam: usize) -> bool {
    match msg {
        WM_KILLFOCUS => true,
        WM_ACTIVATE => (wparam & 0xFFFF) as u32 == WA_INACTIVE,
        WM_NCACTIVATE => wparam == 0,
        _ => false,
    }
}

/// The EDIT class's own window procedure (the same for every EDIT window).
static ORIGINAL: AtomicIsize = AtomicIsize::new(0);

/// The test windows' procedure: records, swallows `SC_KEYMENU` (recorded; entering the
/// modal menu loop would stall the window thread), forwards everything else to EDIT.
unsafe extern "system" fn recorder(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if RECORDED.contains(&msg) {
        lock(&RECORD).push((hwnd.0 as isize, msg, wparam.0));
    }
    if is_focus_loss(msg, wparam.0) {
        lock(&FOCUS_LOST).push((hwnd.0 as isize, msg, wparam.0));
    }
    if msg == WM_SYSCOMMAND && (wparam.0 & 0xFFF0) == SC_KEYMENU as usize {
        return LRESULT(0);
    }
    // SAFETY: ORIGINAL holds the EDIT procedure read by SetWindowLongPtrW before any
    // window was subclassed (a non-null function pointer of the WNDPROC type).
    unsafe {
        let original: WNDPROC =
            std::mem::transmute::<isize, WNDPROC>(ORIGINAL.load(Ordering::SeqCst));
        CallWindowProcW(original, hwnd, msg, wparam, lparam)
    }
}

fn to_hwnd(raw: isize) -> HWND {
    HWND(raw as *mut c_void)
}

/// The value of a window handle as core's `WindowRef` carries it.
pub fn window_ref(hwnd: HWND) -> u64 {
    hwnd.0 as usize as u64
}

/// The shape of a [`TestWindow`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// One visible top-level EDIT window.
    TopLevel,
    /// A top-level EDIT window and a visible popup EDIT window it owns.
    WithOwnedPopup,
}

/// Visible EDIT test windows on their own thread, which pumps (and dispatches) its
/// messages until the value is dropped; then the windows are destroyed on that thread.
pub struct TestWindow {
    hwnds: Vec<isize>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl TestWindow {
    #[track_caller]
    pub fn open(shape: Shape) -> TestWindow {
        let (tx, rx) = mpsc::channel::<Result<Vec<isize>, String>>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("t006-test-window".into())
            .spawn(move || window_thread(shape, &tx, &stop_thread))
            .expect("spawn the test window thread");
        let hwnds = match rx.recv_timeout(WAIT) {
            Ok(Ok(hwnds)) => hwnds,
            Ok(Err(err)) => panic!("premise: test window not created: {err}"),
            Err(err) => panic!("premise: the test window thread did not answer: {err}"),
        };
        TestWindow {
            hwnds,
            stop,
            thread: Some(thread),
        }
    }

    /// The top-level window.
    pub fn hwnd(&self) -> HWND {
        to_hwnd(self.hwnds[0])
    }

    /// The owned popup (`Shape::WithOwnedPopup`).
    pub fn popup(&self) -> HWND {
        to_hwnd(*self.hwnds.get(1).expect("a window with an owned popup"))
    }

    /// The window text (the EDIT's content), read through `WM_GETTEXT`.
    pub fn text(&self) -> String {
        text_of(self.hwnd())
    }

    /// The recorded messages of the top-level window, in order.
    pub fn messages(&self) -> Vec<(u32, usize)> {
        let me = self.hwnds[0];
        lock(&RECORD)
            .iter()
            .filter(|(h, _, _)| *h == me)
            .map(|&(_, m, w)| (m, w))
            .collect()
    }

    /// T-057: every focus or activation loss of the top-level window so far, in order
    /// (`(message, wParam)`; see [`FOCUS_LOST`]). A test takes the length after
    /// [`TestWindow::front`] and reads only what came later.
    pub fn focus_losses(&self) -> Vec<(u32, usize)> {
        let me = self.hwnds[0];
        lock(&FOCUS_LOST)
            .iter()
            .filter(|(h, _, _)| *h == me)
            .map(|&(_, m, w)| (m, w))
            .collect()
    }

    /// Brings the top-level window to the front ([`bring_to_front`], fact
    /// `foreground_again`).
    #[track_caller]
    pub fn front(&self) {
        bring_to_front(self.hwnd());
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The text of `hwnd` (another thread's window: a sent `WM_GETTEXT`).
pub fn text_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 1024];
    // SAFETY: `buf` is writable; the window's thread pumps, so the sent message returns.
    let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..usize::try_from(len).unwrap_or(0).min(buf.len())])
}

/// Brings `hwnd` to the front through the injected-Alt path and waits until it is the
/// foreground window (runner fact `foreground_again`).
///
/// A process may set the foreground only while it has foreground rights. It has them for
/// its first window (the probe's `foreground` fact, `created_fg=1`), but a test exe loses
/// them once an earlier test's window is gone and another process's window (the runner's
/// Windows Terminal) is back in front; `foreground_lock` is infinite on the runner, so they
/// never come back by time (T-006 verify 1: paste.rs's later tests, `set=0`). An Alt key
/// press re-enables `SetForegroundWindow` (LockSetForegroundWindow remarks). So: Alt down
/// (read down), `SetForegroundWindow`, wait for the window in front, then the mask key 0xE8
/// and Alt up (so the release opens no menu, T-006 R-2). These keys are test setup: once
/// Alt's key-up has reached a test window, the record of every test window is cleared, so
/// `hotkey.rs` never mistakes this path's Alt or mask key for the hotkey thread's.
#[track_caller]
pub fn bring_to_front(hwnd: HWND) {
    let seen_before = lock(&RECORD).len();
    let mut alt = Keys::press(&[VK_MENU]);
    // SAFETY: a live window of this process.
    let set = unsafe { SetForegroundWindow(hwnd) }.as_bool();
    let took = eventually(WAIT, || foreground() == hwnd);
    let in_front = class_of(foreground());
    let masked = send(&[key(VK_MASK, false), key(VK_MASK, true)]);
    alt.release(&[VK_MENU]);
    precondition(
        "sendinput",
        masked == 2,
        format!("SendInput inserted {masked} of 2 mask-key events"),
    );
    precondition_rechecked(
        "foreground_again",
        took,
        format!(
            "with an injected Alt held (foreground rights, LockSetForegroundWindow remarks), \
             SetForegroundWindow returned {set} but the test window was not in front within \
             {WAIT:?} (in front: {in_front})"
        ),
    );
    let alt_up = |m: &(isize, u32, usize)| {
        (m.1 == WM_KEYUP || m.1 == WM_SYSKEYUP) && m.2 == usize::from(VK_MENU)
    };
    let reached = eventually(WAIT, || {
        lock(&RECORD)
            .get(seen_before..)
            .is_some_and(|new| new.iter().any(alt_up))
    });
    assert!(
        reached,
        "premise: the Alt key-up of the foreground path did not reach a test window within \
         {WAIT:?} although one is in front, so its keys cannot be told from a test's"
    );
    lock(&RECORD).clear();
}

pub fn foreground() -> HWND {
    // SAFETY: no arguments.
    unsafe { GetForegroundWindow() }
}

pub fn class_of(hwnd: HWND) -> String {
    if hwnd.is_invalid() {
        return "none".to_owned();
    }
    let mut buf = [0u16; 256];
    // SAFETY: `buf` is writable; GetClassNameW sends no message.
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..usize::try_from(len).unwrap_or(0).min(buf.len())])
}

/// An EDIT window with empty text (an EDIT's window name is its initial text).
fn create_edit(
    style: windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE,
    owner: Option<HWND>,
    title: PCWSTR,
) -> windows::core::Result<HWND> {
    // SAFETY: the system EDIT class, static strings, an optional owner of this thread.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            title,
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            480,
            160,
            owner,
            None,
            None,
            None,
        )
    }
}

fn subclass(hwnd: HWND) {
    let own: WNDPROC = Some(recorder);
    let own = own.map_or(0, |f| f as usize as isize);
    // SAFETY: a window of this thread; the recorder forwards to the procedure it replaces.
    let previous = unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, own) };
    if previous != 0 && previous != own {
        ORIGINAL.store(previous, Ordering::SeqCst);
    }
}

fn window_thread(shape: Shape, tx: &mpsc::Sender<Result<Vec<isize>, String>>, stop: &AtomicBool) {
    let top = match create_edit(WS_OVERLAPPEDWINDOW | WS_VISIBLE, None, w!("")) {
        Ok(h) => h,
        Err(err) => {
            let _ = tx.send(Err(format!("CreateWindowExW: {err}")));
            return;
        }
    };
    subclass(top);
    let mut hwnds = vec![top];
    if shape == Shape::WithOwnedPopup {
        match create_edit(WS_POPUP | WS_CAPTION | WS_VISIBLE, Some(top), w!("")) {
            Ok(h) => {
                subclass(h);
                hwnds.push(h);
            }
            Err(err) => {
                // SAFETY: the window this thread created above.
                let _ = unsafe { DestroyWindow(top) };
                let _ = tx.send(Err(format!("CreateWindowExW (owned popup): {err}")));
                return;
            }
        }
    }
    let raw: Vec<isize> = hwnds.iter().map(|h| h.0 as isize).collect();
    let _ = tx.send(Ok(raw));
    while !stop.load(Ordering::SeqCst) {
        let mut msg = MSG::default();
        // SAFETY: `msg` is valid and writable; this thread's own queue.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
    for h in hwnds.into_iter().rev() {
        // SAFETY: windows this thread created; destroyed once, owned ones first.
        let _ = unsafe { DestroyWindow(h) };
    }
}

// ---- clipboard ----------------------------------------------------------------------

/// The three exclusion formats of research R-11.
pub const EXCLUDE_FROM_MONITOR: &str = "ExcludeClipboardContentFromMonitorProcessing";
pub const CAN_INCLUDE_IN_HISTORY: &str = "CanIncludeInClipboardHistory";
pub const CAN_UPLOAD_TO_CLOUD: &str = "CanUploadToCloudClipboard";

/// An open clipboard: closed on drop.
pub struct Opened;

impl Drop for Opened {
    fn drop(&mut self) {
        // SAFETY: created only right after a successful OpenClipboard on this thread.
        let _ = unsafe { CloseClipboard() };
    }
}

/// `OpenClipboard(None)` with 20 tries 50 ms apart (capability `clipboard`).
#[track_caller]
pub fn open_clipboard() -> Opened {
    for _ in 0..20 {
        // SAFETY: no owner window.
        if unsafe { OpenClipboard(None) }.is_ok() {
            return Opened;
        }
        thread::sleep(Duration::from_millis(50));
    }
    precondition(
        "clipboard",
        false,
        "OpenClipboard(NULL) refused 20 times over 1 s",
    );
    unreachable!()
}

/// `OpenClipboard(None)` once.
pub fn try_open_clipboard() -> Option<Opened> {
    // SAFETY: no owner window.
    unsafe { OpenClipboard(None) }.is_ok().then_some(Opened)
}

fn format_id(name: &str) -> u32 {
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: a NUL-terminated UTF-16 string that outlives the call.
    unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) }
}

fn free(mem: HGLOBAL) {
    // SAFETY: a block this thread allocated and still owns, freed once.
    let _ = unsafe { GlobalFree(Some(mem)) };
}

/// Empties the clipboard and puts `text` on it as `CF_UNICODETEXT` only (no exclusion
/// formats): the test's own write, independent of `WinClipboard`.
#[track_caller]
pub fn put_text(text: &str) {
    let _open = open_clipboard();
    let with_nul: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    // SAFETY: the clipboard is open (closed by `_open`); a fresh block is filled, unlocked
    // and handed to the clipboard, or freed when it is refused.
    unsafe {
        EmptyClipboard().expect("EmptyClipboard");
        let mem =
            GlobalAlloc(GMEM_MOVEABLE, size_of_val(with_nul.as_slice())).expect("GlobalAlloc");
        let ptr = GlobalLock(mem).cast::<u16>();
        assert!(!ptr.is_null(), "GlobalLock");
        std::ptr::copy_nonoverlapping(with_nul.as_ptr(), ptr, with_nul.len());
        let _ = GlobalUnlock(mem);
        if let Err(err) = SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(mem.0))) {
            free(mem);
            panic!("SetClipboardData: {err}");
        }
    }
}

/// The bytes of clipboard format `format` (the clipboard must be open).
fn format_bytes(format: u32) -> Option<Vec<u8>> {
    // SAFETY: the clipboard is open; the handle stays the system's (never freed or kept).
    let handle = unsafe { GetClipboardData(format) }.ok()?;
    let mem = HGLOBAL(handle.0);
    // SAFETY: a live clipboard block while the clipboard is open.
    let size = unsafe { GlobalSize(mem) };
    // SAFETY: as above.
    let ptr = unsafe { GlobalLock(mem) }.cast::<u8>();
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the locked block holds `size` bytes.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, size) }.to_vec();
    // SAFETY: the lock taken above.
    let _ = unsafe { GlobalUnlock(mem) };
    Some(bytes)
}

/// What the clipboard holds, as T-006 checks it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardState {
    /// `CF_UNICODETEXT` up to its NUL.
    pub text: Option<String>,
    /// `ExcludeClipboardContentFromMonitorProcessing` is on the clipboard.
    pub exclude_from_monitor: bool,
    /// The first DWORD of `CanIncludeInClipboardHistory`.
    pub can_include_in_history: Option<u32>,
    /// The first DWORD of `CanUploadToCloudClipboard`.
    pub can_upload_to_cloud: Option<u32>,
}

fn dword(bytes: Option<Vec<u8>>) -> Option<u32> {
    let b = bytes?;
    Some(u32::from_le_bytes([
        *b.first()?,
        *b.get(1)?,
        *b.get(2)?,
        *b.get(3)?,
    ]))
}

/// Reads the clipboard (capability `clipboard`).
#[track_caller]
pub fn read_clipboard() -> ClipboardState {
    let _open = open_clipboard();
    let text = format_bytes(u32::from(CF_UNICODETEXT.0)).map(|b| {
        let units: Vec<u16> = b
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();
        String::from_utf16_lossy(&units)
    });
    // SAFETY: the clipboard is open; a registered format id.
    let exclude_from_monitor =
        unsafe { IsClipboardFormatAvailable(format_id(EXCLUDE_FROM_MONITOR)) }.is_ok();
    ClipboardState {
        text,
        exclude_from_monitor,
        can_include_in_history: dword(format_bytes(format_id(CAN_INCLUDE_IN_HISTORY))),
        can_upload_to_cloud: dword(format_bytes(format_id(CAN_UPLOAD_TO_CLOUD))),
    }
}

/// `CF_UNICODETEXT` of the clipboard.
#[track_caller]
pub fn clipboard_text() -> Option<String> {
    read_clipboard().text
}

/// Another thread holds the clipboard open for `hold`, or until
/// [`ClipboardHolder::release`]; [`ClipboardHolder::hold`] returns once it is open.
///
/// The holder opens the clipboard with a message-only window (`HWND_MESSAGE`) of its own
/// thread, the way the probe's `clipboard` fact opens it with its window
/// (docs/decisions/windows-ci-runner.md). Not `OpenClipboard(NULL)`: on the runner a
/// NULL-owner hold did not refuse a second NULL open from another thread of the same
/// process (T-006 verify 1, CI run 37280075413, clipboard.rs:77), so it never held the
/// clipboard against `WinClipboard`, which opens with NULL. Each test still asserts that a
/// second open is refused while held before it relies on the hold.
pub struct ClipboardHolder {
    release: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

/// A message-only window of the calling thread (the system `STATIC` class: no class to
/// register), the holder's clipboard owner.
fn message_only_window() -> windows::core::Result<HWND> {
    // SAFETY: the system STATIC class, static strings, the message-only parent; the
    // holder thread destroys it on that thread.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("voicen test clipboard holder"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )
    }
}

impl ClipboardHolder {
    #[track_caller]
    pub fn hold(hold: Duration) -> ClipboardHolder {
        let (opened_tx, opened_rx) = mpsc::channel::<Result<(), String>>();
        let (release, release_rx) = mpsc::channel::<()>();
        let thread = thread::spawn(move || {
            let owner = match message_only_window() {
                Ok(owner) => owner,
                Err(err) => {
                    let _ = opened_tx.send(Err(format!(
                        "the holder's message-only window was not created: {err}"
                    )));
                    return;
                }
            };
            let mut opened = None;
            for _ in 0..20 {
                // SAFETY: the holder's own live window, on its thread.
                if unsafe { OpenClipboard(Some(owner)) }.is_ok() {
                    opened = Some(Opened);
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            let ok = opened.is_some();
            let _ = opened_tx.send(if ok {
                Ok(())
            } else {
                Err("OpenClipboard(holder window) refused 20 times over 1 s".to_owned())
            });
            if ok {
                // Pump the owner's thread while holding (the clipboard may send its
                // owner messages), until released or `hold` has passed.
                let until = Instant::now() + hold;
                loop {
                    pump();
                    let left = until.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    match release_rx.recv_timeout(left.min(Duration::from_millis(10))) {
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            }
            drop(opened);
            // SAFETY: the window this thread created; destroyed once, after the close.
            let _ = unsafe { DestroyWindow(owner) };
        });
        let opened = opened_rx
            .recv_timeout(WAIT)
            .unwrap_or_else(|err| Err(format!("the holder thread did not answer: {err}")));
        precondition(
            "clipboard",
            opened.is_ok(),
            format!(
                "the holder thread could not open the clipboard with its window: {}",
                opened.as_ref().err().map_or("", String::as_str)
            ),
        );
        ClipboardHolder {
            release,
            thread: Some(thread),
        }
    }

    pub fn release(mut self) {
        self.finish();
    }

    fn finish(&mut self) {
        let _ = self.release.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ClipboardHolder {
    fn drop(&mut self) {
        self.finish();
    }
}

// ---- engine, settings and the app rig ------------------------------------------------

/// The test engine of the Acceptance ("mock engine"): kind `api`, a fixed text, a call
/// count, and an optional gate it waits on before answering.
pub struct TextEngine {
    pub text: String,
    pub calls: AtomicUsize,
    gate: Option<Arc<Gate>>,
}

impl TextEngine {
    pub fn new(text: &str) -> Arc<TextEngine> {
        Arc::new(TextEngine {
            text: text.to_string(),
            calls: AtomicUsize::new(0),
            gate: None,
        })
    }

    pub fn gated(text: &str, gate: Arc<Gate>) -> Arc<TextEngine> {
        Arc::new(TextEngine {
            text: text.to_string(),
            calls: AtomicUsize::new(0),
            gate: Some(gate),
        })
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

/// An engine handle the factory can box (the factory builds one per job).
struct EngineRef(Arc<TextEngine>);

impl Engine for EngineRef {
    fn kind(&self) -> &'static str {
        "api"
    }

    fn transcribe(
        &self,
        _audio: &AudioBuffer,
        _req: &TranscribeRequest,
    ) -> Result<String, FailureReason> {
        self.0.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = &self.0.gate {
            gate.enter_and_wait();
        }
        Ok(self.0.text.clone())
    }
}

pub fn engine_factory(engine: &Arc<TextEngine>) -> Box<EngineFactory> {
    let engine = Arc::clone(engine);
    Box::new(move |_s: &Settings, _c: &dyn CredentialStore| {
        Ok(Box::new(EngineRef(Arc::clone(&engine))) as Box<dyn Engine>)
    })
}

/// A gate the test engine waits on: `entered` once the engine runs, `open` lets it go
/// (or `HOLD_LIMIT` passes).
#[derive(Default)]
pub struct Gate {
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}

impl Gate {
    fn enter_and_wait(&self) {
        let mut st = lock(&self.state);
        st.0 = true;
        self.changed.notify_all();
        let _ = self
            .changed
            .wait_timeout_while(st, HOLD_LIMIT, |s| !s.1)
            .unwrap_or_else(PoisonError::into_inner);
    }

    pub fn wait_entered(&self, within: Duration) -> bool {
        let st = lock(&self.state);
        let (st, _) = self
            .changed
            .wait_timeout_while(st, within, |s| !s.0)
            .unwrap_or_else(PoisonError::into_inner);
        st.0
    }

    pub fn open(&self) {
        lock(&self.state).1 = true;
        self.changed.notify_all();
    }
}

/// Opens the gate when dropped, also by a failed assertion.
pub struct OpenOnDrop(pub Arc<Gate>);

impl Drop for OpenOnDrop {
    fn drop(&mut self) {
        self.0.open();
    }
}

fn idle_models() -> Arc<LocalModels> {
    let (models, cleanup) = LocalModels::open(
        TempDir::new().path().join("models"),
        FakeDisk::with_available(u64::MAX),
        Timeouts::default(),
        MODELS,
    );
    cleanup.expect("cleanup over a missing models dir");
    Arc::new(models)
}

/// The credential store with the fake transcription key (one instance per rig: the
/// settings service's and the session's).
pub fn keyed_store() -> Arc<dyn CredentialStore> {
    Arc::new(FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, FAKE_KEY))
}

/// The settings service of `dir` over `creds`, logging to `log`, with `engine` saved
/// (`EngineKind::None` is the first-run default and is not saved).
pub fn settings(
    dir: &Path,
    creds: &Arc<dyn CredentialStore>,
    log: &Log,
    engine: EngineKind,
) -> Arc<SettingsService> {
    let autostart: Arc<dyn Autostart> = Arc::new(FakeAutostart::new());
    let (service, _) = load_settings(
        dir.to_path_buf(),
        Arc::clone(creds),
        autostart,
        Arc::new(ModelStore::new(dir.join("models"), MODELS)),
        None,
        log,
    );
    save_engine(&service, engine);
    service
}

/// T-055: [`settings`] through `load_settings_with`, over the given autostart entry and
/// the given hotkey registrar handle (the instance `start_dictation` attaches its hotkey
/// thread to, through `DictationPorts::hotkeys`).
pub fn settings_with(
    dir: &Path,
    creds: &Arc<dyn CredentialStore>,
    log: &Log,
    engine: EngineKind,
    autostart: Arc<dyn Autostart>,
    hotkeys: &Arc<HotkeyRegistrarHandle>,
) -> Arc<SettingsService> {
    let (service, _) = load_settings_with(
        dir.to_path_buf(),
        Arc::clone(creds),
        autostart,
        Arc::new(ModelStore::new(dir.join("models"), MODELS)),
        Arc::clone(hotkeys) as Arc<dyn voicen_core::hotkey_registrar::HotkeyRegistrar>,
        None,
        log,
    );
    save_engine(&service, engine);
    service
}

/// Saves `engine` (nothing for `EngineKind::None`, the first-run default).
fn save_engine(service: &SettingsService, engine: EngineKind) {
    if engine != EngineKind::None {
        let mut s = (*service.snapshot()).clone();
        s.engine = engine;
        match service.save(SaveRequest {
            settings: s,
            keys: KeyEdits::default(),
        }) {
            SaveOutcome::Saved { .. } => {}
            other => panic!("premise: saving engine {engine:?} is refused: {other:?}"),
        }
    }
}

/// The app as `run()` builds it (`build_app` on the mock runtime) over a temp data dir,
/// its log under `<dir>/logs`.
pub struct AppRig {
    pub dir: TempDir,
    pub logs: PathBuf,
    pub app: App<MockRuntime>,
    pub service: Arc<SettingsService>,
    pub creds: Arc<dyn CredentialStore>,
    /// T-055: the registrar handle the service was given; pass it to `start_dictation`
    /// as `DictationPorts::hotkeys` (the same instance, like `creds`).
    pub hotkeys: Arc<HotkeyRegistrarHandle>,
}

impl AppRig {
    pub fn new(engine: EngineKind) -> AppRig {
        AppRig::with_autostart(engine, Arc::new(FakeAutostart::new()))
    }

    /// T-055: [`new`](Self::new) over the given autostart entry (a failing one for the
    /// save's later-refusal branch).
    pub fn with_autostart(engine: EngineKind, autostart: Arc<dyn Autostart>) -> AppRig {
        let dir = TempDir::new();
        let logs = dir.path().join("logs");
        let log = voicen_lib::diag::start(logs.clone(), Box::new(|_| {}));
        let creds = keyed_store();
        let hotkeys = HotkeyRegistrarHandle::new();
        let service = settings_with(dir.path(), &creds, &log, engine, autostart, &hotkeys);
        let app = voicen_lib::build_app(
            mock_builder(),
            mock_context(noop_assets()),
            Arc::clone(&service),
            idle_models(),
            log,
        )
        .expect("mock app builds");
        AppRig {
            dir,
            logs,
            app,
            service,
            creds,
            hotkeys,
        }
    }

    /// The session `start_dictation` manages.
    #[track_caller]
    pub fn session(&self) -> Arc<DictationSession> {
        let state = self
            .app
            .try_state::<Arc<DictationSession>>()
            .expect("start_dictation manages the session as Arc<DictationSession>");
        Arc::clone(&state)
    }

    pub fn log_lines(&self) -> Vec<String> {
        log_lines(&self.logs)
    }
}

/// The lines of `<logs>/voicen.log` (none when it does not exist yet).
pub fn log_lines(logs: &Path) -> Vec<String> {
    std::fs::read_to_string(logs.join("voicen.log"))
        .map(|t| t.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// The lines whose message starts with `head` (`<timestamp> <LEVEL> <head> k=v …`).
pub fn headed(lines: &[String], head: &str) -> Vec<String> {
    lines
        .iter()
        .filter(|l| l.split_whitespace().nth(2) == Some(head))
        .cloned()
        .collect()
}

/// The `dictation …` lines of a log.
pub fn dictation_lines(lines: &[String]) -> Vec<String> {
    headed(lines, "dictation")
}

/// `line` carries the pair `key=value` (whole tokens).
pub fn has_pair(line: &str, key: &str, value: &str) -> bool {
    let want = format!("{key}={value}");
    line.split_whitespace().any(|t| t == want)
}

/// The overlays without their `until` (the message's id and params only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    Hidden,
    Recording,
    Processing,
    Message(&'static str, Vec<(&'static str, String)>),
}

pub fn shown(overlays: &[OverlayState]) -> Vec<Shown> {
    overlays
        .iter()
        .map(|o| match o {
            OverlayState::Hidden => Shown::Hidden,
            OverlayState::Recording => Shown::Recording,
            OverlayState::Processing => Shown::Processing,
            OverlayState::Message { id, params, .. } => {
                Shown::Message(message_id(*id), params.clone())
            }
        })
        .collect()
}

/// The catalog id of a `MessageId` (its serialized form, contracts/ipc.md).
pub fn message_id(id: voicen_core::i18n::MessageId) -> &'static str {
    let json = serde_json::to_string(&id).expect("a MessageId serializes");
    let id: String = serde_json::from_str(&json).expect("a JSON string");
    Box::leak(id.into_boxed_str())
}
