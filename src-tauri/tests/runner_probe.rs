//! T-059: runner capability probe. It prints one line per fact about what a cargo test exe can
//! do on the Windows CI runner: the session and desktop it runs in, whether its own window
//! becomes the foreground window, whether keys injected with `SendInput` reach that window, fire
//! a `RegisterHotKey` hotkey and show in `GetAsyncKeyState`, and whether it can own the
//! clipboard. Windows CI only (decision #5), and only through the windows job's last step, which
//! runs it with `--ignored --no-capture --test-threads=1` and checks the end line against the
//! fact lines. The rule, the line grammar and the observed runs: `docs/decisions/windows-ci-runner.md`.
//!
//! A missing capability is a result line, never a failure. The test fails only on its own
//! defect: a probe thread that panicked or ended before sending its facts, or a fact sent twice.
//! It prints raw values and leaves the interpretation to the doc (F-003), and every wait is a
//! poll against the constants below (F-005). Only `std` and `windows`: no tauri app and no
//! `voicen_lib`, so nothing of the app's runtime touches the facts. The clipboard text is fake
//! and the clipboard is emptied at the end.
#![cfg(windows)]

use std::ffi::c_void;
use std::io::Write;
use std::mem::{size_of, size_of_val};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::core::{w, Error, PCWSTR};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSGetActiveConsoleSessionId};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, GetProcessWindowStation, GetThreadDesktop, GetUserObjectInformationW,
    OpenInputDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS, UOI_FLAGS, UOI_NAME,
    USEROBJECTFLAGS,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetFocus, RegisterHotKey, SendInput, UnregisterHotKey, INPUT, INPUT_0,
    INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, MOD_ALT, MOD_CONTROL,
    MOD_NOREPEAT, VIRTUAL_KEY, VK_A, VK_CONTROL, VK_MENU, VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, FindWindowW, GetClassNameW,
    GetForegroundWindow, GetSystemMetrics, GetWindowTextLengthW, PeekMessageW, SetForegroundWindow,
    SystemParametersInfoW, TranslateMessage, CW_USEDEFAULT, MSG, PM_REMOVE, SM_REMOTESESSION,
    SPI_GETFOREGROUNDLOCKTIMEOUT, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_EX_STYLE, WM_CHAR,
    WM_HOTKEY, WM_KEYDOWN, WM_KEYFIRST, WM_KEYLAST, WSF_VISIBLE, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

/// Budget of one observation (F-005: sized for Windows, not for the Linux gate).
const WAIT: Duration = Duration::from_secs(3);

/// Poll interval of every wait.
const POLL: Duration = Duration::from_millis(10);

/// `OpenClipboard` attempts, and the pause after a refused one (another process may hold the
/// clipboard for a moment).
const CLIPBOARD_TRIES: u32 = 10;
const CLIPBOARD_PAUSE: Duration = Duration::from_millis(50);

/// The whole probe: a fact still missing then prints `timeout`, and a hung thread is abandoned.
const WATCHDOG: Duration = Duration::from_secs(60);

/// At most this many messages per pump, so even a queue that keeps refilling cannot hold a wait.
const PUMP_MAX: u32 = 10_000;

/// Facts of the session thread, in the order it sends them.
const SESSION_FACTS: &[&str] = &[
    "image",
    "session",
    "station",
    "desktop",
    "foreground_lock",
    "taskbar",
];

/// Facts of the window thread, in the order it sends them.
const WINDOW_FACTS: &[&str] = &[
    "foreground",
    "sendinput",
    "clipboard",
    "clipboard_null_owner",
    "hotkey",
    "async_keys",
];

/// The probe's hotkey id (an application may use 0x0000..=0xBFFF).
const HOTKEY_ID: i32 = 0x0759;

/// An unassigned virtual key, pressed after the hotkey's key-downs and before Alt's key-up, so
/// the Alt release activates no menu in whichever window has the focus (T-006 R-2).
const VK_MASK: VIRTUAL_KEY = VIRTUAL_KEY(0xE8);

/// The hotkey's keys, in press order, with the names the `async_keys` line uses.
const HELD: [(VIRTUAL_KEY, &str); 3] =
    [(VK_CONTROL, "ctrl"), (VK_MENU, "alt"), (VK_SPACE, "space")];

const INPUT_SIZE: i32 = size_of::<INPUT>() as i32;

#[derive(Clone, Copy)]
enum Status {
    /// The capability was observed; the line carries the measured ms.
    Ok,
    /// The API reported failure.
    Denied,
    /// The API reported success, but the effect was not observed within `WAIT`.
    Lost,
    /// An object was not found.
    Absent,
    /// A precondition of the probe itself is missing (`reason=`).
    Skipped,
    /// No answer before the watchdog.
    Timeout,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Denied => "denied",
            Status::Lost => "lost",
            Status::Absent => "absent",
            Status::Skipped => "skipped",
            Status::Timeout => "timeout",
        }
    }
}

/// One result, printed by the main thread as exactly one line.
struct Fact {
    name: &'static str,
    status: Status,
    detail: Vec<(&'static str, String)>,
}

impl Fact {
    fn new(name: &'static str, status: Status) -> Self {
        Fact {
            name,
            status,
            detail: Vec::new(),
        }
    }

    fn with(mut self, key: &'static str, value: impl ToString) -> Self {
        self.detail.push((key, value.to_string()));
        self
    }

    /// `runner-probe: <fact>=<status>` or `runner-probe: <fact>=<status>(k=v,...)`; every
    /// character of a key or value outside `A-Za-z0-9_.:-` becomes `_`, so a value can never
    /// break the line grammar the CI step checks.
    fn line(&self) -> String {
        let mut line = format!("runner-probe: {}={}", self.name, self.status.as_str());
        if !self.detail.is_empty() {
            let pairs: Vec<String> = self
                .detail
                .iter()
                .map(|(key, value)| format!("{}={}", clean(key), clean(value)))
                .collect();
            line.push('(');
            line.push_str(&pairs.join(","));
            line.push(')');
        }
        line
    }
}

fn clean(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// An OS code as `0xHHHHHHHH` (`windows::core::Error::code()`).
fn code(err: &Error) -> String {
    format!("0x{:08X}", err.code().0 as u32)
}

/// The calling thread's last error, read right after the call that failed.
fn last_err() -> String {
    code(&Error::from_thread())
}

fn ms(since: Instant) -> u128 {
    since.elapsed().as_millis()
}

fn bit(value: bool) -> &'static str {
    if value {
        "1"
    } else {
        "0"
    }
}

/// What the main thread printed, and the probe's own defects.
#[derive(Default)]
struct Report {
    printed: Vec<&'static str>,
    defects: Vec<String>,
}

impl Report {
    /// One line to stdout under one lock, flushed at once, so the lines printed before a hang
    /// stay in the log. libtest captures what a passing test prints unless it runs with
    /// `--no-capture`, as the probe step does; the step's guard fails on missing lines.
    fn out(&self, line: &str) {
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{line}");
        let _ = stdout.flush();
    }

    fn print(&mut self, fact: &Fact) {
        self.out(&fact.line());
        self.printed.push(fact.name);
    }

    fn is_printed(&self, name: &str) -> bool {
        self.printed.iter().any(|&printed| printed == name)
    }

    /// Prints a fact a probe thread sent, once; a second copy or a fact of another thread is a
    /// probe defect and is not printed.
    fn accept(&mut self, expected: &[&'static str], fact: Fact) {
        if !expected.contains(&fact.name) || self.is_printed(fact.name) {
            self.defect(format!(
                "fact {} sent twice or by the wrong thread",
                fact.name
            ));
        } else {
            self.print(&fact);
        }
    }

    fn defect(&mut self, what: String) {
        eprintln!("runner probe defect: {what}");
        self.defects.push(what);
    }
}

#[test]
#[ignore = "runner probe: run only by the windows job's probe step (--ignored --no-capture)"]
fn runner_probe() {
    let start = Instant::now();
    let deadline = start + WATCHDOG;
    let mut report = Report::default();
    // With one test thread libtest may have printed "test runner_probe ... " without a newline:
    // end that line, so every probe line starts a line, where the step's guard reads it.
    report.out("");
    let session = phase(
        &mut report,
        "probe-session",
        SESSION_FACTS,
        deadline,
        session_facts,
    );
    let window = phase(
        &mut report,
        "probe-window",
        WINDOW_FACTS,
        deadline,
        window_facts,
    );
    for &name in SESSION_FACTS.iter().chain(WINDOW_FACTS) {
        if !report.is_printed(name) {
            report.print(&Fact::new(name, Status::Timeout).with("ms", ms(start)));
        }
    }
    let end = format!("runner-probe: end(facts={})", report.printed.len());
    report.out(&end);
    for (name, handle) in [("probe-session", session), ("probe-window", window)] {
        if let Some(handle) = handle {
            settle(&mut report, name, handle);
        }
    }
    assert!(
        report.defects.is_empty(),
        "runner probe defect: {}",
        report.defects.join("; ")
    );
}

/// Runs `body` on its own thread and prints its facts as they arrive, until all of them are
/// in, the thread ends, or the watchdog's `deadline` passes. The main thread makes no Win32
/// call. Nothing starts after the deadline: its facts then print `timeout`.
fn phase(
    report: &mut Report,
    thread_name: &'static str,
    facts: &'static [&'static str],
    deadline: Instant,
    body: fn(&Sender<Fact>),
) -> Option<JoinHandle<()>> {
    if Instant::now() >= deadline {
        return None;
    }
    let (tx, rx) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name(thread_name.to_owned())
        .spawn(move || body(&tx));
    let handle = match spawned {
        Ok(handle) => handle,
        Err(err) => {
            report.defect(format!("{thread_name} not started: {err}"));
            for &name in facts {
                report.print(&Fact::new(name, Status::Skipped).with("reason", "no_thread"));
            }
            return None;
        }
    };
    while !facts.iter().all(|name| report.is_printed(name)) {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(fact) => report.accept(facts, fact),
            Err(RecvTimeoutError::Disconnected) => {
                for &name in facts {
                    if !report.is_printed(name) {
                        report.print(
                            &Fact::new(name, Status::Skipped).with("reason", "probe_thread_ended"),
                        );
                    }
                }
                report.defect(format!("{thread_name} ended before sending all its facts"));
                break;
            }
            Err(RecvTimeoutError::Timeout) => break,
        }
    }
    Some(handle)
}

/// Joins a probe thread once it has finished, giving it up to `WAIT` to end (it may still be
/// cleaning up or unwinding after its last fact). It never blocks on a hung thread: one still
/// running then is abandoned (its missing facts printed `timeout`), and the process ends when
/// libtest returns.
fn settle(report: &mut Report, name: &str, handle: JoinHandle<()>) {
    let until = Instant::now() + WAIT;
    while !handle.is_finished() && Instant::now() < until {
        thread::sleep(POLL);
    }
    if !handle.is_finished() {
        eprintln!("runner probe: {name} still running after the watchdog, abandoned");
        return;
    }
    if handle.join().is_err() {
        report.defect(format!("{name} panicked"));
    }
}

fn session_facts(tx: &Sender<Fact>) {
    let send = |fact: Fact| {
        let _ = tx.send(fact);
    };
    send(image());
    send(session());
    send(station());
    send(desktop());
    send(foreground_lock());
    send(taskbar());
}

/// The runner image, from the system variables the image build sets.
fn image() -> Fact {
    let os = std::env::var("ImageOS").ok();
    let version = std::env::var("ImageVersion").ok();
    let status = if os.is_some() && version.is_some() {
        Status::Ok
    } else {
        Status::Absent
    };
    Fact::new("image", status)
        .with("os", os.as_deref().unwrap_or("none"))
        .with("version", version.as_deref().unwrap_or("none"))
}

fn session() -> Fact {
    let mut id = 0u32;
    // SAFETY: `id` is a valid u32 the call writes.
    let own = unsafe { ProcessIdToSessionId(std::process::id(), &mut id) };
    // SAFETY: no arguments.
    let console = unsafe { WTSGetActiveConsoleSessionId() };
    // SAFETY: a plain index.
    let remote = unsafe { GetSystemMetrics(SM_REMOTESESSION) };
    let fact = match own {
        Ok(()) => Fact::new("session", Status::Ok).with("id", id),
        Err(err) => Fact::new("session", Status::Denied).with("err", code(&err)),
    };
    fact.with("console", console).with("remote", remote)
}

/// `UOI_NAME` of a window station or desktop.
fn object_name(handle: HANDLE) -> windows::core::Result<String> {
    let mut buf = [0u16; 256];
    let mut needed = 0u32;
    // SAFETY: `buf` is writable for the byte length passed, and `needed` is a valid u32.
    unsafe {
        GetUserObjectInformationW(
            handle,
            UOI_NAME,
            Some(buf.as_mut_ptr().cast::<c_void>()),
            size_of_val(&buf) as u32,
            Some(&mut needed as *mut u32),
        )
    }?;
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Ok(String::from_utf16_lossy(&buf[..len]))
}

/// `UOI_FLAGS` of a window station: its `dwFlags`.
fn object_flags(handle: HANDLE) -> windows::core::Result<u32> {
    let mut flags = USEROBJECTFLAGS::default();
    // SAFETY: `flags` is a writable USEROBJECTFLAGS of the size passed.
    unsafe {
        GetUserObjectInformationW(
            handle,
            UOI_FLAGS,
            Some((&mut flags as *mut USEROBJECTFLAGS).cast::<c_void>()),
            size_of::<USEROBJECTFLAGS>() as u32,
            None,
        )
    }?;
    Ok(flags.dwFlags)
}

fn station() -> Fact {
    // SAFETY: no arguments. The handle is the process's own and is never closed.
    let station = match unsafe { GetProcessWindowStation() } {
        Ok(station) => HANDLE(station.0),
        Err(err) => {
            return Fact::new("station", Status::Denied)
                .with("step", "get")
                .with("err", code(&err))
        }
    };
    match (object_name(station), object_flags(station)) {
        (Ok(name), Ok(flags)) => Fact::new("station", Status::Ok)
            .with("name", name)
            .with("visible", bit((flags & WSF_VISIBLE as u32) != 0)),
        (Err(err), _) => Fact::new("station", Status::Denied)
            .with("step", "name")
            .with("err", code(&err)),
        (Ok(name), Err(err)) => Fact::new("station", Status::Denied)
            .with("name", name)
            .with("step", "flags")
            .with("err", code(&err)),
    }
}

fn desktop() -> Fact {
    // SAFETY: no arguments.
    let thread_id = unsafe { GetCurrentThreadId() };
    // SAFETY: the calling thread's id. The returned handle needs no CloseDesktop.
    let own = unsafe { GetThreadDesktop(thread_id) }.and_then(|desk| object_name(HANDLE(desk.0)));
    // SAFETY: plain flags; the handle is closed right after its name is read.
    let input = unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) }
        .and_then(|desk| {
            let name = object_name(HANDLE(desk.0));
            // SAFETY: the handle OpenInputDesktop returned, closed once.
            let _ = unsafe { CloseDesktop(desk) };
            name
        });
    let status = if own.is_ok() && input.is_ok() {
        Status::Ok
    } else {
        Status::Denied
    };
    let fact = Fact::new("desktop", status);
    let fact = match own {
        Ok(name) => fact.with("thread", name),
        Err(err) => fact.with("thread_err", code(&err)),
    };
    match input {
        Ok(name) => fact.with("input", name),
        Err(err) => fact.with("input_err", code(&err)),
    }
}

fn foreground_lock() -> Fact {
    let mut timeout = 0u32;
    // SAFETY: SPI_GETFOREGROUNDLOCKTIMEOUT writes one u32 to the pointer passed.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETFOREGROUNDLOCKTIMEOUT,
            0,
            Some((&mut timeout as *mut u32).cast::<c_void>()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    match read {
        Ok(()) => Fact::new("foreground_lock", Status::Ok).with("ms", timeout),
        Err(err) => Fact::new("foreground_lock", Status::Denied).with("err", code(&err)),
    }
}

/// Whether the shell's taskbar (the notification area's host) exists in this session.
fn taskbar() -> Fact {
    // SAFETY: a static class name and no title.
    match unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) } {
        Ok(_) => Fact::new("taskbar", Status::Ok),
        Err(err) if err.code().0 != 0 => {
            Fact::new("taskbar", Status::Absent).with("err", code(&err))
        }
        Err(_) => Fact::new("taskbar", Status::Absent),
    }
}

/// The probe's window: destroyed on drop, on the thread that created it.
struct Window(HWND);

impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: the window this thread created; destroyed once.
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

/// Empties the clipboard on drop, whatever the clipboard probes left on it.
struct ClipboardCleanup(HWND);

impl Drop for ClipboardCleanup {
    fn drop(&mut self) {
        if open_clipboard(Some(self.0)).is_ok() {
            let _open = Opened;
            // SAFETY: the clipboard is open (closed by `_open`).
            let _ = unsafe { EmptyClipboard() };
        }
    }
}

/// An open clipboard: closed on drop, so every path closes it.
struct Opened;

impl Drop for Opened {
    fn drop(&mut self) {
        // SAFETY: created only right after a successful OpenClipboard on this thread.
        let _ = unsafe { CloseClipboard() };
    }
}

/// Key-ups (and the mask key) for keys a batch pressed: sent on drop unless `send` already got
/// all of them inserted, so no path leaves an injected key down.
struct Release {
    inputs: Vec<INPUT>,
    armed: bool,
}

impl Release {
    fn arm(inputs: Vec<INPUT>) -> Self {
        Release {
            inputs,
            armed: true,
        }
    }

    /// Sends the batch now; returns the inserted count.
    fn send(&mut self) -> u32 {
        // SAFETY: a valid INPUT slice and the size of one element.
        let inserted = unsafe { SendInput(&self.inputs, INPUT_SIZE) };
        if inserted as usize == self.inputs.len() {
            self.armed = false;
        }
        inserted
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for Release {
    fn drop(&mut self) {
        if self.armed {
            self.send();
        }
    }
}

/// The probe's hotkey registration: unregistered on drop.
struct Hotkey(HWND);

impl Drop for Hotkey {
    fn drop(&mut self) {
        // SAFETY: the id this thread registered for its own window.
        let _ = unsafe { UnregisterHotKey(Some(self.0), HOTKEY_ID) };
    }
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
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

fn is_down(vk: VIRTUAL_KEY) -> bool {
    // SAFETY: a plain virtual-key code.
    let state = unsafe { GetAsyncKeyState(i32::from(vk.0)) };
    state < 0
}

/// Takes the messages queued for this thread (`PeekMessageW`, never a blocking `GetMessageW`)
/// and returns them. Each is translated (which makes `WM_CHAR`) and dispatched, except the
/// keyboard messages when `dispatch_keys` is false: in the hotkey probe, Alt or Alt+Space
/// dispatched to the window could open its menu, a modal loop inside `DispatchMessageW`.
fn pump(dispatch_keys: bool) -> Vec<MSG> {
    let mut taken = Vec::new();
    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid, writable MSG; only this thread's own queue is read.
    while taken.len() < PUMP_MAX as usize
        && unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool()
    {
        let keyboard = (WM_KEYFIRST..=WM_KEYLAST).contains(&msg.message);
        if dispatch_keys || !keyboard {
            // SAFETY: a message this thread just took from its own queue.
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        taken.push(msg);
    }
    taken
}

/// Pumps this thread's messages and asks `done` (with the messages just taken) every `POLL`,
/// until it says yes or `WAIT` has passed since `since`. Returns the ms from `since` on yes.
fn poll(dispatch_keys: bool, since: Instant, mut done: impl FnMut(&[MSG]) -> bool) -> Option<u128> {
    loop {
        let taken = pump(dispatch_keys);
        if done(&taken) {
            return Some(ms(since));
        }
        if since.elapsed() >= WAIT {
            return None;
        }
        thread::sleep(POLL);
    }
}

/// The class name of a window, or `none` for no window.
fn class_of(hwnd: HWND) -> String {
    if hwnd.is_invalid() {
        return "none".to_owned();
    }
    let mut buf = [0u16; 256];
    // SAFETY: `buf` is writable; GetClassNameW sends no message.
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    if len <= 0 {
        return "unknown".to_owned();
    }
    String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())])
}

fn window_facts(tx: &Sender<Fact>) {
    let send = |fact: Fact| {
        let _ = tx.send(fact);
    };
    // Read before the window exists: a window created visible may take the foreground at once.
    // SAFETY: no arguments.
    let before = class_of(unsafe { GetForegroundWindow() });
    // SAFETY: the system EDIT class (no RegisterClassW, so no Win32_Graphics_Gdi), static
    // strings, no parent, menu or instance. `Window` destroys it on this thread.
    let created = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            w!("voicen runner probe"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            480,
            160,
            None,
            None,
            None,
            None,
        )
    };
    let window = match created {
        Ok(hwnd) => Window(hwnd),
        Err(err) => {
            for &name in WINDOW_FACTS {
                send(
                    Fact::new(name, Status::Skipped)
                        .with("reason", "no_window")
                        .with("err", code(&err)),
                );
            }
            return;
        }
    };
    let hwnd = window.0;
    // Dropped before `window`: the clipboard is emptied while its owner window still exists.
    let _clipboard = ClipboardCleanup(hwnd);
    send(foreground(hwnd, &before));
    send(sendinput(hwnd));
    send(clipboard("clipboard", Some(hwnd)));
    send(clipboard("clipboard_null_owner", None));
    let (hotkey, async_keys) = hotkey_and_async_keys(hwnd);
    send(hotkey);
    send(async_keys);
}

fn foreground(hwnd: HWND, before: &str) -> Fact {
    // SAFETY: no arguments.
    let at_create = unsafe { GetForegroundWindow() } == hwnd;
    let since = Instant::now();
    // SAFETY: the probe's own live window.
    let set = unsafe { SetForegroundWindow(hwnd) }.as_bool();
    // SAFETY: no arguments.
    let took = poll(true, since, |_| unsafe { GetForegroundWindow() } == hwnd);
    // SAFETY: no arguments; the focus window of this thread's queue, if any.
    let focus = unsafe { GetFocus() };
    let focus = if focus == hwnd {
        "self".to_owned()
    } else {
        class_of(focus)
    };
    let status = match (took, set) {
        (Some(_), _) => Status::Ok,
        (None, true) => Status::Lost,
        (None, false) => Status::Denied,
    };
    let mut fact = Fact::new("foreground", status);
    if let Some(took) = took {
        fact = fact.with("ms", took);
    }
    fact = fact
        .with("before", before)
        .with("created_fg", bit(at_create))
        .with("set", bit(set))
        .with("focus", focus);
    if took.is_none() {
        // SAFETY: no arguments.
        fact = fact.with("now", class_of(unsafe { GetForegroundWindow() }));
    }
    fact
}

fn text_len(hwnd: HWND) -> i32 {
    // SAFETY: the probe's own window, on its own thread (a direct WM_GETTEXTLENGTH call).
    unsafe { GetWindowTextLengthW(hwnd) }
}

fn sendinput(hwnd: HWND) -> Fact {
    // SAFETY: no arguments.
    let foreground = unsafe { GetForegroundWindow() } == hwnd;
    let len_before = text_len(hwnd);
    let mut release = Release::arm(vec![key(VK_A, true)]);
    let since = Instant::now();
    // SAFETY: a valid INPUT slice and the size of one element.
    let inserted = unsafe { SendInput(&[key(VK_A, false), key(VK_A, true)], INPUT_SIZE) };
    let send_err = (inserted == 0).then(last_err);
    if inserted == 2 {
        release.disarm();
    }
    if let Some(err) = send_err {
        return Fact::new("sendinput", Status::Denied)
            .with("inserted", 0)
            .with("err", err)
            .with("fg", bit(foreground));
    }
    let mut keydown = None;
    let mut char_code = None;
    poll(true, since, |taken| {
        for msg in taken.iter().filter(|msg| msg.hwnd == hwnd) {
            if msg.message == WM_KEYDOWN && msg.wParam.0 == usize::from(VK_A.0) {
                keydown.get_or_insert(ms(since));
            }
            if msg.message == WM_CHAR {
                char_code.get_or_insert(msg.wParam.0);
            }
        }
        keydown.is_some() && char_code.is_some()
    });
    let fact = match keydown {
        Some(took) => Fact::new("sendinput", Status::Ok).with("ms", took),
        None => Fact::new("sendinput", Status::Lost),
    };
    fact.with("inserted", inserted)
        .with("keydown", bit(keydown.is_some()))
        .with(
            "char",
            char_code.map_or("none".to_owned(), |c| format!("0x{c:X}")),
        )
        .with("text_len_before", len_before)
        .with("text_len", text_len(hwnd))
        .with("fg", bit(foreground))
}

/// Puts `text` (without its NUL) on the open clipboard as CF_UNICODETEXT.
fn put_text(text: &[u16]) -> Result<(), (&'static str, Error)> {
    let with_nul: Vec<u16> = text.iter().copied().chain(Some(0)).collect();
    // SAFETY: a plain allocation; freed below unless the clipboard took it.
    let mem = unsafe { GlobalAlloc(GMEM_MOVEABLE, size_of_val(with_nul.as_slice())) }
        .map_err(|e| ("alloc", e))?;
    // SAFETY: the live block just allocated.
    let ptr = unsafe { GlobalLock(mem) }.cast::<u16>();
    if ptr.is_null() {
        let err = Error::from_thread();
        free(mem);
        return Err(("lock", err));
    }
    // SAFETY: the locked block holds `with_nul.len()` u16s, and the regions do not overlap.
    unsafe { std::ptr::copy_nonoverlapping(with_nul.as_ptr(), ptr, with_nul.len()) };
    // SAFETY: the block locked above. The last unlock reports FALSE with code 0 (windows-result
    // 0.4.1 turns it into an Err with code 0): that is success.
    if let Err(err) = unsafe { GlobalUnlock(mem) } {
        if err.code().0 != 0 {
            free(mem);
            return Err(("unlock", err));
        }
    }
    // SAFETY: the clipboard is open. On success the system owns `mem`, so it is never freed here.
    match unsafe { SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(mem.0))) } {
        Ok(_) => Ok(()),
        Err(err) => {
            free(mem);
            Err(("set", err))
        }
    }
}

/// Frees a block the clipboard did not take. GlobalFree returns NULL on success, which windows
/// 0.62 reports as an Err, so its result says nothing and is dropped.
fn free(mem: HGLOBAL) {
    // SAFETY: a block this thread allocated and still owns, freed once.
    let _ = unsafe { GlobalFree(Some(mem)) };
}

/// Reads CF_UNICODETEXT from the open clipboard, up to its NUL.
fn get_text() -> Result<Vec<u16>, (&'static str, Error)> {
    // SAFETY: the clipboard is open; the handle stays the system's (never freed or kept).
    let handle =
        unsafe { GetClipboardData(u32::from(CF_UNICODETEXT.0)) }.map_err(|e| ("get", e))?;
    let mem = HGLOBAL(handle.0);
    // SAFETY: a live clipboard block while the clipboard is open.
    let size = unsafe { GlobalSize(mem) };
    // SAFETY: as above.
    let ptr = unsafe { GlobalLock(mem) }.cast::<u16>();
    if ptr.is_null() {
        return Err(("read_lock", Error::from_thread()));
    }
    // SAFETY: the locked block holds `size` bytes; only whole u16s within it are read.
    let all = unsafe { std::slice::from_raw_parts(ptr, size / 2) };
    let text = all.iter().take_while(|&&c| c != 0).copied().collect();
    // SAFETY: the lock taken above. Its result is not needed for a read.
    let _ = unsafe { GlobalUnlock(mem) };
    Ok(text)
}

/// `OpenClipboard(owner)`, retried. Returns the attempts it took.
fn open_clipboard(owner: Option<HWND>) -> Result<u32, (u32, Error)> {
    let mut tries = 0;
    loop {
        tries += 1;
        // SAFETY: `owner` is the probe's live window or none.
        match unsafe { OpenClipboard(owner) } {
            Ok(()) => return Ok(tries),
            Err(err) if tries >= CLIPBOARD_TRIES => return Err((tries, err)),
            Err(_) => thread::sleep(CLIPBOARD_PAUSE),
        }
    }
}

/// Opens the clipboard with `owner`, empties it, puts fake text on it, closes it, then opens it
/// again and reads the text back. `owner: None` measures what T-006 design 3 left open.
fn clipboard(name: &'static str, owner: Option<HWND>) -> Fact {
    let want: Vec<u16> = format!("voicen-probe-{}", std::process::id())
        .encode_utf16()
        .collect();
    let since = Instant::now();
    let tries = match open_clipboard(owner) {
        Ok(tries) => tries,
        Err((tries, err)) => {
            return Fact::new(name, Status::Denied)
                .with("step", "open")
                .with("open_tries", tries)
                .with("err", code(&err))
        }
    };
    let put = {
        let _open = Opened;
        // SAFETY: the clipboard is open (closed by `_open`).
        unsafe { EmptyClipboard() }
            .map_err(|e| ("empty", e))
            .and_then(|()| put_text(&want))
    };
    if let Err((step, err)) = put {
        return Fact::new(name, Status::Denied)
            .with("step", step)
            .with("open_tries", tries)
            .with("err", code(&err));
    }
    let got = match open_clipboard(owner) {
        Ok(_) => {
            let _open = Opened;
            get_text()
        }
        Err((_, err)) => Err(("reopen", err)),
    };
    match got {
        Ok(got) if got == want => Fact::new(name, Status::Ok)
            .with("ms", ms(since))
            .with("open_tries", tries),
        Ok(got) => Fact::new(name, Status::Lost)
            .with("step", "compare")
            .with("read_len", got.len())
            .with("open_tries", tries),
        Err((step, err)) => Fact::new(name, Status::Lost)
            .with("step", step)
            .with("open_tries", tries)
            .with("err", code(&err)),
    }
}

/// Registers Ctrl+Alt+Space for the window and presses it with one `SendInput`: `hotkey` waits
/// for `WM_HOTKEY`, `async_keys` reads the three keys down while they are held and up after the
/// release (mask key first, then Space, Alt, Ctrl). Keyboard messages are not dispatched here.
fn hotkey_and_async_keys(hwnd: HWND) -> (Fact, Fact) {
    // SAFETY: the probe's own live window; `Hotkey` unregisters it.
    let registered = unsafe {
        RegisterHotKey(
            Some(hwnd),
            HOTKEY_ID,
            MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
            u32::from(VK_SPACE.0),
        )
    };
    let _hotkey = registered.is_ok().then(|| Hotkey(hwnd));
    let mut release = Release::arm(vec![
        key(VK_MASK, false),
        key(VK_MASK, true),
        key(VK_SPACE, true),
        key(VK_MENU, true),
        key(VK_CONTROL, true),
    ]);
    let downs: Vec<INPUT> = HELD.iter().map(|&(vk, _)| key(vk, false)).collect();
    // GetAsyncKeyState may read 0 while another process's thread is in front: recorded, not judged.
    // SAFETY: no arguments.
    let foreground = unsafe { GetForegroundWindow() } == hwnd;
    let since = Instant::now();
    // SAFETY: a valid INPUT slice and the size of one element.
    let inserted = unsafe { SendInput(&downs, INPUT_SIZE) };
    let send_err = (inserted == 0).then(last_err);
    let mut wm_hotkey = None;
    let mut down: [Option<u128>; 3] = [None; 3];
    if inserted > 0 {
        poll(false, since, |taken| {
            if taken.iter().any(|msg| {
                msg.hwnd == hwnd && msg.message == WM_HOTKEY && msg.wParam.0 == HOTKEY_ID as usize
            }) {
                wm_hotkey.get_or_insert(ms(since));
            }
            for (seen, &(vk, _)) in down.iter_mut().zip(HELD.iter()) {
                if seen.is_none() && is_down(vk) {
                    *seen = Some(ms(since));
                }
            }
            (wm_hotkey.is_some() || registered.is_err()) && down.iter().all(Option::is_some)
        });
    }
    let released = release.send();
    let since_up = Instant::now();
    let mut up: [Option<u128>; 3] = [None; 3];
    poll(false, since_up, |_| {
        for (seen, &(vk, _)) in up.iter_mut().zip(HELD.iter()) {
            if seen.is_none() && !is_down(vk) {
                *seen = Some(ms(since_up));
            }
        }
        up.iter().all(Option::is_some)
    });

    let hotkey = match (&registered, &send_err, wm_hotkey) {
        (Err(err), _, _) => Fact::new("hotkey", Status::Denied)
            .with("step", "register")
            .with("err", code(err)),
        (Ok(()), Some(err), _) => Fact::new("hotkey", Status::Denied)
            .with("step", "sendinput")
            .with("inserted", 0)
            .with("err", err),
        (Ok(()), None, Some(took)) => Fact::new("hotkey", Status::Ok)
            .with("ms", took)
            .with("inserted", inserted),
        (Ok(()), None, None) => Fact::new("hotkey", Status::Lost)
            .with("inserted", inserted)
            .with("wm_hotkey", 0),
    }
    .with("fg", bit(foreground));

    let seen = |marks: &[Option<u128>; 3]| {
        marks
            .iter()
            .zip(HELD.iter())
            .map(|(mark, &(_, label))| format!("{label}:{}", bit(mark.is_some())))
            .collect::<Vec<_>>()
            .join(".")
    };
    let slowest = |marks: &[Option<u128>; 3]| marks.iter().flatten().max().copied().unwrap_or(0);
    let async_keys = if let Some(err) = &send_err {
        Fact::new("async_keys", Status::Denied)
            .with("inserted", 0)
            .with("err", err)
    } else if down.iter().all(Option::is_some) && up.iter().all(Option::is_some) {
        Fact::new("async_keys", Status::Ok)
            .with("ms", slowest(&down))
            .with("up_ms", slowest(&up))
    } else {
        Fact::new("async_keys", Status::Lost).with("inserted", inserted)
    };
    let async_keys = async_keys
        .with("down", seen(&down))
        .with("up", seen(&up))
        .with("released", released)
        .with("fg", bit(foreground));
    (hotkey, async_keys)
}
