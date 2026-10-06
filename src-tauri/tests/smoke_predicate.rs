//! T-057: the install smoke's "shown window" predicate
//! (`scripts/ci/visible-windows.ps1`, `Get-ShownWindows`), run through `pwsh` on
//! windows this test exe creates. Windows CI only (decision #5); analysis red-test
//! table row 7 and Acceptance 2 ("a tool window shown at start makes the install
//! smoke fail").
//!
//! The smoke's loaded-launch step fails when `Get-ShownWindows` returns any window
//! (and the first-launch step wants exactly the settings window), so "the smoke
//! fails on a tool window shown at start" is exactly "`Get-ShownWindows` returns a
//! visible, unowned, top-level tool window". F-003: the predicate decides on raw
//! window facts (visible, owner, top-level, the class of tao's event-target window);
//! no "it is our overlay" exception by class or title. The windows here are EDIT
//! windows with an obviously fake title, so an exception keyed to the overlay's
//! class or title cannot make the red test pass.
//!
//! Precondition: `pwsh` (PowerShell 7) on PATH, as on the windows-latest runner the
//! smoke itself runs on.
//!
//! T-057 VERIFY_FAIL 3 (run 37419266451): the framework helper windows. tao 0.37.1
//! (`event_loop.rs` `create_event_target_window`) and tauri-plugin-single-instance
//! 2.5.2 (`platform_impl/windows.rs` `create_event_target_window`, class `{id}-sic`,
//! title `{id}-siw`) each create one top-level, unowned window with
//! `WS_EX_NOACTIVATE|WS_EX_TRANSPARENT|WS_EX_LAYERED|WS_EX_TOOLWINDOW`, `WS_OVERLAPPED`,
//! 0,0,0,0, and then set `GWL_STYLE` to `WS_VISIBLE|WS_POPUP`. The tests below register
//! those classes in this exe and create the windows with the same arguments (no tauri
//! process, no plugin: its second-instance path calls `process::exit(0)`, T-052). The
//! analysis invariant: a window is excluded only when its class is on the script's
//! pinned helper list AND it has all four helper ex-style bits; never by style alone
//! (a click-through overlay has LAYERED|TRANSPARENT), never by title. The set of helper
//! windows a real voicen.exe has stays proven only by the install smoke.
#![cfg(windows)]

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    OpenProcess, WaitForSingleObject, CREATE_NO_WINDOW, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetWindowLongPtrW,
    IsWindowVisible, PeekMessageW, RegisterClassExW, SetLayeredWindowAttributes, SetWindowLongPtrW,
    TranslateMessage, GWL_EXSTYLE, GWL_STYLE, LWA_ALPHA, MSG, PM_REMOVE, WINDOW_EX_STYLE,
    WINDOW_STYLE, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_OVERLAPPED, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE,
};

/// How long pwsh may take (its `Add-Type` compiles C# on every start; F-005).
const PWSH_BUDGET: Duration = Duration::from_secs(120);

/// The window class of one test window.
#[derive(Clone)]
enum Class {
    /// The system EDIT class, created with no instance handle (as before T-057 VF3).
    Edit,
    /// A class this exe registers (once per name, `DefWindowProcW`, this module's
    /// instance handle, as the plugin and tao do) and creates the window with that
    /// instance handle.
    Own(String),
}

/// One window to create: class, title (`None`: a null title, as tao's), extended
/// style, style, rect (x, y, width, height), whether the first window of the set owns
/// it, the `GWL_STYLE` set after creation (`SetWindowLongPtrW`, as the plugin and tao
/// do) and the alpha given by `SetLayeredWindowAttributes(LWA_ALPHA)` after creation.
#[derive(Clone)]
struct Spec {
    class: Class,
    title: Option<String>,
    ex: WINDOW_EX_STYLE,
    style: WINDOW_STYLE,
    rect: (i32, i32, i32, i32),
    owned: bool,
    restyle: Option<WINDOW_STYLE>,
    alpha: Option<u8>,
}

/// The fake title of the probe windows that model no framework window.
const PROBE_TITLE: &str = "t057 smoke predicate probe (fake)";

impl Spec {
    /// An EDIT window at 10,10 200x60 with the fake title (the shape of every window
    /// before T-057 VERIFY_FAIL 3).
    fn edit(ex: WINDOW_EX_STYLE, style: WINDOW_STYLE) -> Spec {
        Spec {
            class: Class::Edit,
            title: Some(PROBE_TITLE.to_owned()),
            ex,
            style,
            rect: (10, 10, 200, 60),
            owned: false,
            restyle: None,
            alpha: None,
        }
    }

    /// Owned by the first window of the set.
    fn owned(self) -> Spec {
        Spec {
            owned: true,
            ..self
        }
    }

    /// A framework helper window exactly as tao 0.37.1 and tauri-plugin-single-instance
    /// 2.5.2 create theirs: `ex` (the helper bits for the real shape), `WS_OVERLAPPED`,
    /// 0,0,0,0, no parent, no menu, this module's instance, then
    /// `SetWindowLongPtrW(GWL_STYLE, WS_VISIBLE|WS_POPUP)`.
    fn helper_shaped(class: &str, title: Option<String>, ex: WINDOW_EX_STYLE) -> Spec {
        Spec {
            class: Class::Own(class.to_owned()),
            title,
            ex,
            style: WS_OVERLAPPED,
            rect: (0, 0, 0, 0),
            owned: false,
            restyle: Some(WS_VISIBLE | WS_POPUP),
            alpha: None,
        }
    }
}

/// The four extended-style bits both framework helper windows are created with.
const HELPER_EX: WINDOW_EX_STYLE = WINDOW_EX_STYLE(
    WS_EX_NOACTIVATE.0 | WS_EX_TRANSPARENT.0 | WS_EX_LAYERED.0 | WS_EX_TOOLWINDOW.0,
);

/// tao 0.37.1's event-target window class (`event_loop.rs` `create_event_target_window`).
const TAO_EVENT_TARGET: &str = "Tao Thread Event Target";

/// The window class tauri-runtime-wry 2.12.1 gives every product window (`lib.rs`
/// `window_classname("Tauri Window")`), the overlay included.
const TAURI_WINDOW: &str = "Tauri Window";

/// The app identifier, read from `src-tauri/tauri.conf.json` (not hard-coded, so a
/// drift between the config and the script's pinned helper class fails a test).
fn app_identifier() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("premise: {} is readable: {e}", path.display()));
    let config: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("premise: {} is JSON: {e}", path.display()));
    config["identifier"]
        .as_str()
        .unwrap_or_else(|| panic!("premise: {} has a string `identifier`", path.display()))
        .to_owned()
}

/// tauri-plugin-single-instance 2.5.2's helper class and title for this app
/// (`platform_impl/windows.rs` `init`: `{id}-sic`, `{id}-siw`; no `semver` feature).
fn single_instance_class_and_title() -> (String, String) {
    let id = app_identifier();
    (format!("{id}-sic"), format!("{id}-siw"))
}

/// A NUL-terminated UTF-16 copy of `s`.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The window procedure of every class this exe registers.
unsafe extern "system" fn probe_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: forwards the arguments the system passed in.
    unsafe { DefWindowProcW(h, msg, wp, lp) }
}

/// Class names already registered by this exe (tests run in parallel threads and may
/// share a class).
static REGISTERED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Registers `class` once in this process with `instance`.
fn register_class(class: &str, instance: HINSTANCE) -> Result<(), String> {
    let mut done = REGISTERED.lock().unwrap_or_else(|e| e.into_inner());
    if done.iter().any(|c| c == class) {
        return Ok(());
    }
    let name = wide(class);
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(probe_proc),
        hInstance: instance,
        lpszClassName: PCWSTR(name.as_ptr()),
        ..Default::default()
    };
    // SAFETY: `wc` is fully initialised; `name` outlives the call (the system copies it).
    if unsafe { RegisterClassExW(&wc) } == 0 {
        return Err(format!(
            "RegisterClassExW({class}): {}",
            windows::core::Error::from_thread()
        ));
    }
    done.push(class.to_owned());
    Ok(())
}

fn hwnd(h: isize) -> HWND {
    HWND(h as *mut core::ffi::c_void)
}

/// `IsWindowVisible` of a window of this process.
fn is_visible(h: isize) -> bool {
    // SAFETY: a query on a window handle of this process.
    unsafe { IsWindowVisible(hwnd(h)) }.as_bool()
}

/// The extended style of a window of this process (`GWL_EXSTYLE`).
fn ex_style_of(h: isize) -> u32 {
    // SAFETY: a query on a window handle of this process.
    unsafe { GetWindowLongPtrW(hwnd(h), GWL_EXSTYLE) as u32 }
}

/// Creates the window of `spec` on this thread (owner `owner`).
fn create(spec: &Spec, owner: Option<HWND>) -> Result<HWND, String> {
    let title = spec.title.as_deref().map(wide);
    let title_ptr = title
        .as_ref()
        .map_or(PCWSTR::null(), |t| PCWSTR(t.as_ptr()));
    let (x, y, cx, cy) = spec.rect;
    let created = match &spec.class {
        // SAFETY: the system EDIT class, a title that outlives the call, an owner (if
        // any) of this thread.
        Class::Edit => unsafe {
            CreateWindowExW(
                spec.ex,
                w!("EDIT"),
                title_ptr,
                spec.style,
                x,
                y,
                cx,
                cy,
                owner,
                None,
                None,
                None,
            )
        },
        Class::Own(class) => {
            // SAFETY: the module handle of this exe (null name).
            let instance: HINSTANCE = unsafe { GetModuleHandleW(PCWSTR::null()) }
                .map_err(|e| format!("GetModuleHandleW: {e}"))?
                .into();
            register_class(class, instance)?;
            let name = wide(class);
            // SAFETY: a class registered above with this instance; name and title
            // outlive the call; an owner (if any) of this thread.
            unsafe {
                CreateWindowExW(
                    spec.ex,
                    PCWSTR(name.as_ptr()),
                    title_ptr,
                    spec.style,
                    x,
                    y,
                    cx,
                    cy,
                    owner,
                    None,
                    Some(instance),
                    None,
                )
            }
        }
    };
    let h = created.map_err(|err| format!("CreateWindowExW: {err}"))?;
    if let Some(style) = spec.restyle {
        // SAFETY: a window this thread just created.
        unsafe { SetWindowLongPtrW(h, GWL_STYLE, style.0 as isize) };
    }
    if let Some(alpha) = spec.alpha {
        // SAFETY: a layered window this thread just created.
        if let Err(err) = unsafe { SetLayeredWindowAttributes(h, COLORREF(0), alpha, LWA_ALPHA) } {
            // SAFETY: the window created above.
            let _ = unsafe { DestroyWindow(h) };
            return Err(format!("SetLayeredWindowAttributes: {err}"));
        }
    }
    Ok(h)
}

/// Windows of `specs`, created and pumped on their own thread until dropped.
struct Windows {
    hwnds: Vec<isize>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Windows {
    #[track_caller]
    fn open(specs: Vec<Spec>) -> Windows {
        let (tx, rx) = mpsc::channel::<Result<Vec<isize>, String>>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let mut made: Vec<HWND> = Vec::new();
            for spec in &specs {
                let owner = if spec.owned {
                    made.first().copied()
                } else {
                    None
                };
                match create(spec, owner) {
                    Ok(h) => made.push(h),
                    Err(err) => {
                        for h in made.iter().rev() {
                            // SAFETY: windows this thread created.
                            let _ = unsafe { DestroyWindow(*h) };
                        }
                        let _ = tx.send(Err(err));
                        return;
                    }
                }
            }
            let _ = tx.send(Ok(made.iter().map(|h| h.0 as isize).collect()));
            while !stop_thread.load(Ordering::SeqCst) {
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
            for h in made.into_iter().rev() {
                // SAFETY: windows this thread created; owned ones first.
                let _ = unsafe { DestroyWindow(h) };
            }
        });
        let hwnds = match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(hwnds)) => hwnds,
            Ok(Err(err)) => panic!("premise: test windows not created: {err}"),
            Err(err) => panic!("premise: the window thread did not answer: {err}"),
        };
        Windows {
            hwnds,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Windows {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("scripts")
        .join("ci")
        .join("visible-windows.ps1")
}

/// What a pwsh run that ended within its budget gives back.
#[derive(Debug)]
struct PwshOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
    elapsed: Duration,
}

/// A pwsh run that did not end within its budget (T-057 VERIFY_FAIL 2, invariant 3):
/// the helper killed the child and waited for it before returning this.
#[derive(Debug)]
struct PwshStall {
    budget: Duration,
    /// How long the call waited for its turn (`PWSH_TURN`, invariant 2) before the
    /// child was spawned; not part of `elapsed`.
    waited: Duration,
    /// From the spawn (after the turn was taken) to the child killed and waited for.
    elapsed: Duration,
    /// The name of the last stage marker the child wrote to stderr (the first token of
    /// the last line that starts with `stage:`, e.g. `stage:up`); `None` when it wrote
    /// none.
    stage: Option<String>,
    /// Everything read from the child's stdout and stderr until it was killed.
    stdout: String,
    stderr: String,
}

/// Why `run_pwsh` gives no output.
#[derive(Debug)]
enum PwshError {
    /// `pwsh` could not be started (not on PATH).
    Spawn(String),
    /// It did not end within its budget (invariant 4: a pwsh stall, never a verdict).
    Stalled(PwshStall),
}

impl std::fmt::Display for PwshError {
    /// A stall prints "pwsh stalled", the elapsed time, the budget, the last stage and
    /// the captured stdout and stderr (invariant 3).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PwshError::Spawn(err) => write!(f, "pwsh could not be started: {err}"),
            PwshError::Stalled(stall) => write!(
                f,
                "pwsh stalled: killed after {:?} (budget {:?}); last stage: {}; \
                 stdout so far: {:?}; stderr so far: {:?}",
                stall.elapsed,
                stall.budget,
                stall.stage.as_deref().unwrap_or("none"),
                stall.stdout,
                stall.stderr
            ),
        }
    }
}

/// One pwsh child at a time in this test binary (invariant 2). Held for the whole run,
/// from spawn to the last byte read. A panic while it is held (none is expected: the
/// helper returns errors) must not fail every later run, so a poisoned lock is taken
/// over.
static PWSH_TURN: Mutex<()> = Mutex::new(());

/// How often the helper polls the child for its exit.
const PWSH_POLL: Duration = Duration::from_millis(50);

/// How long the helper waits for its reader threads once the child is gone. Bounded so
/// that a grandchild holding the pipes cannot hold the helper; what was read so far is
/// returned either way.
const READER_GRACE: Duration = Duration::from_secs(5);

/// A reader thread and the buffer it appends to.
type Reader = (Arc<Mutex<Vec<u8>>>, JoinHandle<()>);

/// Reads `source` to its end on a thread, appending to the returned buffer as bytes
/// arrive (so a caller that stops waiting still has everything read so far).
fn spawn_reader<R: Read + Send + 'static>(mut source: R) -> Reader {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&buffer);
    let handle = thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        loop {
            match source.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => sink
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend_from_slice(&chunk[..n]),
            }
        }
    });
    (buffer, handle)
}

/// Waits up to `READER_GRACE` for both readers, then returns what each has read.
fn drain(readers: [Reader; 2]) -> (String, String) {
    let deadline = Instant::now() + READER_GRACE;
    while readers.iter().any(|(_, h)| !h.is_finished()) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let [out, err] = readers.map(|(buffer, handle)| {
        if handle.is_finished() {
            let _ = handle.join();
        }
        let bytes = buffer.lock().unwrap_or_else(|e| e.into_inner());
        String::from_utf8_lossy(&bytes).into_owned()
    });
    (out, err)
}

/// The name of the last stage marker on `stderr`: the first token of the last line that
/// starts with `stage:`.
fn last_stage(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .rev()
        .map(str::trim_start)
        .find(|l| l.starts_with("stage:"))
        .and_then(|l| l.split_whitespace().next())
        .map(str::to_owned)
}

/// Runs `pwsh -NoProfile -NonInteractive -Command <command>`, the only external process
/// a shell test starts. Invariant (T-057 VERIFY_FAIL 2 analysis): null stdin and
/// CREATE_NO_WINDOW; one run at a time in this binary (a file-local static Mutex held
/// for the whole run); stdout and stderr read on threads; when `budget` runs out the
/// child is killed and waited for and `Err(PwshError::Stalled)` carries the elapsed
/// time, the last stage marker and the output read so far.
fn run_pwsh(command: &str, budget: Duration) -> Result<PwshOutput, PwshError> {
    let called = Instant::now();
    let _turn = PWSH_TURN.lock().unwrap_or_else(|e| e.into_inner());
    let waited = called.elapsed();
    let started = Instant::now();
    let mut child = Command::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-Command", command])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()
        .map_err(|err| PwshError::Spawn(err.to_string()))?;
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(PwshError::Spawn(
            "pwsh started without piped stdout and stderr".into(),
        ));
    };
    let readers = [spawn_reader(stdout), spawn_reader(stderr)];
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < budget => thread::sleep(PWSH_POLL),
            // Out of budget, or the child can no longer be polled: either way it is
            // killed and waited for, never left running.
            Ok(None) | Err(_) => break None,
        }
    };
    match status {
        Some(status) => {
            let (stdout, stderr) = drain(readers);
            Ok(PwshOutput {
                status,
                stdout,
                stderr,
                elapsed: started.elapsed(),
            })
        }
        None => {
            let _ = child.kill();
            let _ = child.wait();
            let elapsed = started.elapsed();
            let (stdout, stderr) = drain(readers);
            Err(PwshError::Stalled(PwshStall {
                budget,
                waited,
                elapsed,
                stage: last_stage(&stderr),
                stdout,
                stderr,
            }))
        }
    }
}

/// The `-Command` string the predicate tests run: dot-source the smoke's script, call
/// `Get-ShownWindows <pid>` once, print each handle and then `end` on stdout. Invariant
/// 3 adds stage markers on stderr, each `<name> <[Environment]::TickCount64>`:
/// `stage:up` before the dot-source, `stage:loaded` after it (Add-Type done),
/// `stage:enumerated` after `Get-ShownWindows`.
fn predicate_command(pid: u32) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'; \
         [Console]::Error.WriteLine('stage:up ' + [Environment]::TickCount64); \
         . '{}'; \
         [Console]::Error.WriteLine('stage:loaded ' + [Environment]::TickCount64); \
         $s = Get-ShownWindows {}; \
         [Console]::Error.WriteLine('stage:enumerated ' + [Environment]::TickCount64); \
         foreach ($w in $s) {{ [Console]::Out.WriteLine($w.Handle.ToInt64()) }}; \
         [Console]::Out.WriteLine('end')",
        script().display(),
        pid
    )
}

/// The handles `Get-ShownWindows <this pid>` returns, as the smoke's steps call it
/// (dot-sourced script, one call).
#[track_caller]
fn shown_by_smoke() -> Vec<isize> {
    let path = script();
    assert!(path.is_file(), "premise: {} exists", path.display());
    let output = match run_pwsh(&predicate_command(std::process::id()), PWSH_BUDGET) {
        Ok(output) => output,
        Err(PwshError::Spawn(err)) => panic!(
            "precondition `pwsh` does not hold: PowerShell 7 is not on PATH ({err}); the \
             windows-latest runner ships it and the install smoke runs under it"
        ),
        Err(err) => panic!("{err}"),
    };
    let PwshOutput {
        status,
        stdout,
        stderr,
        elapsed,
    } = output;
    assert!(
        status.success() && stdout.lines().last() == Some("end"),
        "premise: Get-ShownWindows ran ({status:?} after {elapsed:?}); stdout: {stdout}; \
         stderr: {stderr}"
    );
    stdout
        .lines()
        .filter(|l| *l != "end")
        .map(|l| {
            l.trim()
                .parse::<i64>()
                .unwrap_or_else(|e| panic!("not a handle `{l}`: {e}")) as isize
        })
        .collect()
}

/// `run_pwsh`, which must return; a premise failure otherwise.
#[track_caller]
fn ran(command: &str, budget: Duration) -> PwshOutput {
    match run_pwsh(command, budget) {
        Ok(output) => output,
        Err(PwshError::Spawn(err)) => panic!("precondition `pwsh` on PATH does not hold: {err}"),
        Err(PwshError::Stalled(stall)) => panic!("premise: pwsh ended in time: {stall:?}"),
    }
}

/// The `name tick` lines on `stderr` whose name starts with `stage:`, in order.
fn stage_lines(stderr: &str) -> Vec<(String, i64)> {
    stderr
        .lines()
        .filter(|l| l.trim_start().starts_with("stage:"))
        .map(|l| {
            let mut parts = l.split_whitespace();
            let name = parts.next().unwrap_or_default().to_owned();
            let tick = parts
                .next()
                .and_then(|t| t.parse::<i64>().ok())
                .unwrap_or_else(|| panic!("stage line `{l}` carries no TickCount64"));
            (name, tick)
        })
        .collect()
}

/// The value of the `key value` line on `text`.
#[track_caller]
fn field<'a>(text: &'a str, key: &str) -> &'a str {
    text.lines()
        .find_map(|l| l.trim().strip_prefix(key).map(str::trim))
        .unwrap_or_else(|| panic!("premise: no `{key}` line in: {text}"))
}

/// Whether process `pid` is still running (it may still exist as an exited object).
fn process_running(pid: u32) -> bool {
    // SAFETY: a query-only handle to a process id, closed below.
    match unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            false,
            pid,
        )
    } {
        Err(_) => false,
        Ok(handle) => {
            // SAFETY: the handle just opened; a zero timeout only polls.
            let state = unsafe { WaitForSingleObject(handle, 0) };
            // SAFETY: closes the handle opened above, once.
            let _ = unsafe { CloseHandle(handle) };
            state != WAIT_OBJECT_0
        }
    }
}

/// Budget of the stall tests: small, so a stall is seen quickly.
const STALL_BUDGET: Duration = Duration::from_secs(15);
/// How much later than its budget a stalled run may return (kill, wait, readers).
const STALL_SLACK: Duration = Duration::from_secs(10);

/// A command that reports its pid, writes `markers` to stderr (each with its tick) and
/// a line to stdout, then sleeps for ten minutes.
fn stalling_command(markers: &[&str]) -> String {
    let mut command = String::from(
        "[Console]::Error.WriteLine('pid ' + $PID); \
         [Console]::Out.WriteLine('t057-out before the stall'); ",
    );
    for marker in markers {
        command.push_str(&format!(
            "[Console]::Error.WriteLine('{marker} ' + [Environment]::TickCount64); "
        ));
    }
    command.push_str(
        "[Console]::Error.WriteLine('t057-err before the stall'); Start-Sleep -Seconds 600",
    );
    command
}

/// Runs `stalling_command(markers)` with `STALL_BUDGET` and checks the stall exit of
/// invariant 3: an `Err` within budget + slack of its turn carrying the stdout and
/// stderr read so far, a Display that names the stall, its stage and both outputs, and
/// a child that is no longer running. Returns the stall.
///
/// The helper's own time is measured from outside minus the time it waited for its
/// turn (invariant 2: the stall tests run in parallel threads of one binary, so one of
/// them waits a whole stall of the other for the lock). `waited` is reported by the
/// helper and pinned from both sides: `outside - waited <= budget + slack` (no
/// blocking on readers or the child past the budget) and `waited + elapsed <= outside`
/// (an honest wait; an inflated one would hide that blocking).
#[track_caller]
fn stall_of(markers: &[&str]) -> PwshStall {
    let started = Instant::now();
    let result = run_pwsh(&stalling_command(markers), STALL_BUDGET);
    let outside = started.elapsed();
    let err = match result {
        Ok(output) => panic!("a ten-minute sleep ended within {STALL_BUDGET:?}: {output:?}"),
        Err(err) => err,
    };
    let shown = format!("{err}");
    let stall = match err {
        PwshError::Stalled(stall) => stall,
        PwshError::Spawn(err) => panic!("precondition `pwsh` on PATH does not hold: {err}"),
    };
    assert!(
        stall.waited + stall.elapsed <= outside,
        "the stall reports a wait of {:?} and {:?} after it, more than the {outside:?} the \
         call took",
        stall.waited,
        stall.elapsed
    );
    let held = outside.saturating_sub(stall.waited);
    assert!(
        held <= STALL_BUDGET + STALL_SLACK,
        "a stalled pwsh held the helper for {held:?} after its turn (budget \
         {STALL_BUDGET:?}, waited {:?} for the turn)",
        stall.waited
    );
    assert!(
        stall.elapsed >= STALL_BUDGET && stall.elapsed <= STALL_BUDGET + STALL_SLACK,
        "the stall reports {:?}, not the time it waited (budget {STALL_BUDGET:?})",
        stall.elapsed
    );
    assert_eq!(
        stall.budget, STALL_BUDGET,
        "the stall reports another budget"
    );
    assert!(
        stall.stderr.contains("t057-err before the stall"),
        "the stall lost the child's stderr: {stall:?}"
    );
    assert!(
        stall.stdout.contains("t057-out before the stall"),
        "the stall lost the child's stdout: {stall:?}"
    );
    for needle in [
        "stalled",
        "t057-err before the stall",
        "t057-out before the stall",
    ]
    .into_iter()
    .chain(markers.last().copied())
    {
        assert!(
            shown.contains(needle),
            "the stall's message does not carry `{needle}`: {shown}"
        );
    }
    let pid: u32 = field(&stall.stderr, "pid ")
        .parse()
        .unwrap_or_else(|e| panic!("premise: the child wrote its pid: {e}; {stall:?}"));
    assert!(
        !process_running(pid),
        "the stalled pwsh (pid {pid}) still runs after the helper returned: it was not \
         killed and waited for"
    );
    stall
}

fn plain() -> Spec {
    Spec::edit(WINDOW_EX_STYLE(0), WS_OVERLAPPEDWINDOW | WS_VISIBLE)
}

#[test]
fn a_visible_unowned_tool_window_counts_as_shown() {
    // Acceptance 2, F-003, analysis invariant 5: tool windows are counted. A visible,
    // unowned, top-level WS_EX_TOOLWINDOW popup (and one styled like the overlay:
    // tool window, no-activate, topmost) is a shown window, so an overlay (or any tool
    // window) visible at start fails the loaded-launch step. Red today:
    // visible-windows.ps1 drops every WS_EX_TOOLWINDOW window. Bite: the tool-window
    // clause kept, or replaced by an exception keyed to a class or title.
    let windows = Windows::open(vec![
        plain(),
        Spec::edit(WS_EX_TOOLWINDOW, WS_POPUP | WS_VISIBLE),
        Spec::edit(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
            WS_POPUP | WS_VISIBLE,
        ),
    ]);
    let shown = shown_by_smoke();
    assert!(
        shown.contains(&windows.hwnds[0]),
        "premise: the plain visible window is shown: {shown:?} of {:?}",
        windows.hwnds
    );
    for (i, what) in [(1, "tool window"), (2, "overlay-styled tool window")] {
        assert!(
            shown.contains(&windows.hwnds[i]),
            "the smoke predicate does not count a visible, unowned, top-level {what} \
             ({:#x}); shown: {shown:?}",
            windows.hwnds[i]
        );
    }
}

#[test]
fn hidden_and_owned_windows_are_not_shown_and_a_plain_one_is() {
    // Characterization (green before T-057; must stay green): the predicate's other
    // raw facts. A plain visible top-level window is shown; a hidden one and a
    // visible window owned by another are not (an owned popup is part of its owner).
    // Bite (for the T-057 edit of the script): dropping more than the tool-window
    // clause (the visible or the owner test).
    let windows = Windows::open(vec![
        plain(),
        Spec::edit(WINDOW_EX_STYLE(0), WS_OVERLAPPEDWINDOW),
        Spec::edit(WINDOW_EX_STYLE(0), WS_POPUP | WS_VISIBLE).owned(),
    ]);
    let shown = shown_by_smoke();
    assert!(
        shown.contains(&windows.hwnds[0]),
        "a plain visible top-level window is not shown: {shown:?} of {:?}",
        windows.hwnds
    );
    assert!(
        !shown.contains(&windows.hwnds[1]),
        "a hidden window is shown: {shown:?}"
    );
    assert!(
        !shown.contains(&windows.hwnds[2]),
        "an owned window is shown: {shown:?}"
    );
}

// --- T-057 VERIFY_FAIL 3 (run 37419266451): framework helper windows ---
//
// Each test also opens a plain visible EDIT window and requires it shown: so a
// predicate that returns nothing (wrong pid, broken script) cannot pass a "not shown"
// assertion. Each window that must be counted or not counted is first checked
// visible (`IsWindowVisible`) as a loud premise, and the helper-shaped ones also for
// the four helper bits in `GWL_EXSTYLE`.

/// Opens `plain()` followed by `specs`, checks the premises (every window of `specs`
/// is visible; the plain one is shown) and returns the windows (index 0 the plain one,
/// `i + 1` for `specs[i]`) and the handles the smoke predicate returns.
#[track_caller]
fn shown_with_premises(specs: Vec<Spec>) -> (Windows, Vec<isize>) {
    let mut all = vec![plain()];
    all.extend(specs);
    let windows = Windows::open(all);
    for (i, h) in windows.hwnds.iter().enumerate().skip(1) {
        assert!(
            is_visible(*h),
            "premise: test window {i} ({h:#x}, ex-style {:#x}) is visible (IsWindowVisible)",
            ex_style_of(*h)
        );
    }
    let shown = shown_by_smoke();
    assert!(
        shown.contains(&windows.hwnds[0]),
        "premise: the plain visible window is shown: {shown:?} of {:?}",
        windows.hwnds
    );
    (windows, shown)
}

/// The premise that window `h` carries all four helper bits after creation.
#[track_caller]
fn assert_has_helper_bits(h: isize, what: &str) {
    let ex = ex_style_of(h);
    assert_eq!(
        ex & HELPER_EX.0,
        HELPER_EX.0,
        "premise: the {what} ({h:#x}) kept the four helper ex-style bits: {ex:#x}"
    );
}

#[test]
fn the_single_instance_helper_window_is_not_shown() {
    // VERIFY_FAIL 3, the step-12 failure of run 37419266451 ("class
    // 'dev.voicen.app-sic', title 'dev.voicen.app-siw'" counted next to the settings
    // window). The window is created with tauri-plugin-single-instance 2.5.2's exact
    // arguments (class `{identifier}-sic` from tauri.conf.json, title `{identifier}-siw`,
    // NOACTIVATE|TRANSPARENT|LAYERED|TOOLWINDOW, WS_OVERLAPPED, 0,0,0,0, then GWL_STYLE
    // = WS_VISIBLE|WS_POPUP). Red today: visible-windows.ps1 excludes only tao's class.
    // Bites: the plugin's class missing from the helper list, or a list entry that
    // drifts from tauri.conf.json's identifier (e.g. a `semver` suffix or a renamed
    // identifier), or the helper-bit check comparing against another mask.
    let (class, title) = single_instance_class_and_title();
    let (windows, shown) = shown_with_premises(vec![Spec::helper_shaped(
        &class,
        Some(title.clone()),
        HELPER_EX,
    )]);
    let helper = windows.hwnds[1];
    assert_has_helper_bits(helper, "single-instance helper window");
    assert!(
        !shown.contains(&helper),
        "the smoke predicate counts tauri-plugin-single-instance's helper window (class \
         '{class}', title '{title}', {helper:#x}) as shown: {shown:?}"
    );
}

#[test]
fn taos_event_target_window_is_not_shown() {
    // Guard (green today; must stay green): tao 0.37.1's event-target window, created
    // as tao creates it (class 'Tao Thread Event Target', null title, the four helper
    // bits, WS_OVERLAPPED, 0,0,0,0, then GWL_STYLE = WS_VISIBLE|WS_POPUP), is not a
    // shown window: it exists, visible, in every tauri process, so counting it fails
    // every launch. Bites: tao's entry dropped from the helper list when the plugin's is
    // added; a helper-bit mask that tao's window does not match.
    let (windows, shown) =
        shown_with_premises(vec![Spec::helper_shaped(TAO_EVENT_TARGET, None, HELPER_EX)]);
    let helper = windows.hwnds[1];
    assert_has_helper_bits(helper, "tao event-target window");
    assert!(
        !shown.contains(&helper),
        "the smoke predicate counts tao's event-target window ({helper:#x}) as shown: \
         {shown:?}"
    );
}

/// For a helper class: a plain visible 200x60 popup of that class, and the exact
/// helper shape with each one of the four helper bits left out. Each must be counted
/// (invariant (b): all four bits, not some, not the class alone).
#[track_caller]
fn assert_helper_class_without_the_helper_bits_counts(class: &str, title: Option<String>) {
    let mut specs = vec![Spec {
        class: Class::Own(class.to_owned()),
        title: title.clone(),
        ex: WINDOW_EX_STYLE(0),
        style: WS_POPUP | WS_VISIBLE,
        rect: (10, 10, 200, 60),
        owned: false,
        restyle: None,
        alpha: None,
    }];
    let dropped = [
        WS_EX_NOACTIVATE,
        WS_EX_TRANSPARENT,
        WS_EX_LAYERED,
        WS_EX_TOOLWINDOW,
    ];
    for bit in dropped {
        specs.push(Spec::helper_shaped(
            class,
            title.clone(),
            WINDOW_EX_STYLE(HELPER_EX.0 & !bit.0),
        ));
    }
    let (windows, shown) = shown_with_premises(specs);
    let plain_popup = windows.hwnds[1];
    assert!(
        shown.contains(&plain_popup),
        "the smoke predicate hides a plain visible popup of class '{class}' \
         ({plain_popup:#x}, ex-style {:#x}): it excludes by class alone; shown: {shown:?}",
        ex_style_of(plain_popup)
    );
    for (i, bit) in dropped.iter().enumerate() {
        let h = windows.hwnds[i + 2];
        let ex = ex_style_of(h);
        assert_eq!(
            ex & bit.0,
            0,
            "premise: window {h:#x} of class '{class}' lacks ex bit {:#x}: {ex:#x}",
            bit.0
        );
        assert!(
            shown.contains(&h),
            "the smoke predicate hides a helper-shaped window of class '{class}' without \
             ex bit {:#x} ({h:#x}, ex-style {ex:#x}): it does not require all four helper \
             bits; shown: {shown:?}",
            bit.0
        );
    }
}

#[test]
fn a_single_instance_class_window_without_the_helper_bits_counts() {
    // Guard (green today; must stay green after the fix): invariant (b). A window of
    // the plugin's class that is not of the plugin's non-rendering shape (a plain
    // visible popup, or the helper shape missing any one of the four bits) is counted.
    // Bites: a class-only exclusion (option D of the analysis); a check of some helper
    // bits only (e.g. LAYERED|TRANSPARENT).
    let (class, title) = single_instance_class_and_title();
    assert_helper_class_without_the_helper_bits_counts(&class, Some(title));
}

#[test]
fn a_tao_class_window_without_the_helper_bits_counts() {
    // Invariant (b) for tao's list entry: "a listed class that changes shape counts
    // again". Red today: visible-windows.ps1 excludes class 'Tao Thread Event Target'
    // whatever its style. Bites: the helper-bit check applied to the plugin's entry but
    // not to tao's; a check of some helper bits only.
    assert_helper_class_without_the_helper_bits_counts(TAO_EVENT_TARGET, None);
}

#[test]
fn a_click_through_overlay_window_counts() {
    // Guard (green today; must stay green): an overlay made click-through the way tao
    // 0.37.1 does it (`set_ignore_cursor_events(true)` adds exactly
    // WS_EX_TRANSPARENT|WS_EX_LAYERED, window_state.rs IGNORE_CURSOR_EVENT), on top of
    // the overlay's NOACTIVATE|TOPMOST|TOOLWINDOW, class 'Tauri Window' (every product
    // window's), visible popup 320x56. It carries all four helper bits, so only the
    // class keeps it counted (F-003; OQ-10 click-through follow-up). One with
    // SetLayeredWindowAttributes(alpha 255) and one without (tao never calls it).
    // Bites: a style-only exclusion (LAYERED|TRANSPARENT, the four helper bits, or
    // "layered without layered attributes", option B); the WS_EX_TOOLWINDOW exclusion
    // restored (option C).
    let overlay = |alpha: Option<u8>| Spec {
        class: Class::Own(TAURI_WINDOW.to_owned()),
        title: Some(PROBE_TITLE.to_owned()),
        ex: WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        style: WS_POPUP | WS_VISIBLE,
        rect: (40, 40, 320, 56),
        owned: false,
        restyle: None,
        alpha,
    };
    let (windows, shown) = shown_with_premises(vec![overlay(Some(255)), overlay(None)]);
    for (i, what) in [(1, "with alpha 255"), (2, "without layered attributes")] {
        let h = windows.hwnds[i];
        assert_has_helper_bits(h, "click-through overlay window");
        assert!(
            shown.contains(&h),
            "the smoke predicate hides a visible click-through overlay window {what} \
             (class '{TAURI_WINDOW}', {h:#x}, ex-style {:#x}): it excludes by style; \
             shown: {shown:?}",
            ex_style_of(h)
        );
    }
}

// --- T-057 VERIFY_FAIL 2 (run 37365491860): the pwsh child is bounded and observable ---
//
// Red today: `run_pwsh` is a skeleton with today's behaviour (inherited stdin, no
// creation flags, no lock, a panic on timeout that drops the output and leaks the
// child). Each test names the line whose removal turns it red again.

#[test]
fn a_stalled_pwsh_is_killed_and_reported_with_its_stage() {
    // Invariant 3 and 4. Red today: the helper panics "pwsh did not finish within 15s"
    // with no output, and the child keeps sleeping (holding this exe's pipes, as in run
    // 37365491860). Bites: no kill (child still running; or the helper blocks on the
    // reader threads past budget + slack), kill without reading what was written (no
    // stderr / stdout), a panic instead of an Err, a stage left out of the message.
    let stall = stall_of(&["stage:up"]);
    assert_eq!(
        stall.stage.as_deref(),
        Some("stage:up"),
        "the stall does not name the stage the child reached: {stall:?}"
    );
}

#[test]
fn a_stall_names_the_last_stage_reached_not_the_first() {
    // Invariant 3 ("the last stage marker it reached"). Bites: the first `stage:` line
    // taken instead of the last; the whole line (with its tick) kept as the name.
    let stall = stall_of(&["stage:up", "stage:loaded"]);
    assert_eq!(
        stall.stage.as_deref(),
        Some("stage:loaded"),
        "the stall names another stage than the last one reached: {stall:?}"
    );
}

#[test]
fn the_predicate_run_reports_its_stages_in_order() {
    // Invariant 3 on the real predicate: so the next stall says whether pwsh was still
    // starting (stage:up), in Add-Type (no stage:loaded) or in Get-ShownWindows (no
    // stage:enumerated): H1 vs H4 of the VERIFY_FAIL 2 analysis. The predicate's own
    // stdout is unchanged (handles, then `end`). Red today: `predicate_command` writes
    // no markers. Bites: a marker dropped or written to stdout (which `shown_by_smoke`
    // parses as handles), markers without their tick.
    let path = script();
    assert!(path.is_file(), "premise: {} exists", path.display());
    let output = ran(&predicate_command(std::process::id()), PWSH_BUDGET);
    assert!(
        output.status.success() && output.stdout.lines().last() == Some("end"),
        "premise: Get-ShownWindows ran: {output:?}"
    );
    let stages = stage_lines(&output.stderr);
    let names: Vec<&str> = stages.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        ["stage:up", "stage:loaded", "stage:enumerated"],
        "the predicate's stage markers on stderr: {output:?}"
    );
    assert!(
        stages.windows(2).all(|w| w[0].1 <= w[1].1),
        "stage ticks go backwards: {stages:?}"
    );
    assert!(
        !output.stdout.contains("stage:"),
        "a stage marker went to stdout, which carries the handles: {output:?}"
    );
}

#[test]
fn concurrent_pwsh_runs_take_turns() {
    // Invariant 2 (H4 removed: two pwsh never run at once in this binary). Two threads
    // call the helper at the same moment; each child prints its start and end tick
    // ([Environment]::TickCount64 is system-wide) around a 3 s sleep. Their intervals
    // must not overlap. Red today: no lock, both children run together. Bites: no
    // Mutex; a Mutex held only around spawn or only around the wait.
    let command = "[Console]::Out.WriteLine('t0 ' + [Environment]::TickCount64); \
                   Start-Sleep -Seconds 3; \
                   [Console]::Out.WriteLine('t1 ' + [Environment]::TickCount64)";
    let barrier = Arc::new(Barrier::new(2));
    let runs: Vec<JoinHandle<(i64, i64)>> = (0..2)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                let output = ran(command, PWSH_BUDGET);
                let tick = |key: &str| -> i64 {
                    field(&output.stdout, key)
                        .parse()
                        .unwrap_or_else(|e| panic!("premise: tick `{key}`: {e}; {output:?}"))
                };
                (tick("t0 "), tick("t1 "))
            })
        })
        .collect();
    let spans: Vec<(i64, i64)> = runs
        .into_iter()
        .map(|run| run.join().expect("a pwsh run thread panicked"))
        .collect();
    let (a, b) = (spans[0], spans[1]);
    assert!(
        a.1 <= b.0 || b.1 <= a.0,
        "two pwsh children ran at the same time: {a:?} and {b:?} (ms, TickCount64)"
    );
}

#[test]
fn the_pwsh_child_gets_a_null_stdin_and_no_console_window() {
    // Invariant 1 (H2 removed, and no console of its own). The child reports its stdin
    // handle's file type (NUL is FILE_TYPE_CHAR = 2 and not a console; an inherited
    // runner pipe is FILE_TYPE_PIPE = 3, a console is char and a console) and its
    // console window (none under CREATE_NO_WINDOW). Red today for stdin on the runner
    // (inherited pipe). The console-window half is a guard: under the runner, which
    // starts the step without a console window, it may hold already. Bites:
    // Stdio::null() dropped (stdin inherited); creation_flags(CREATE_NO_WINDOW) dropped
    // where this exe has a console window.
    let command = "$q = [char]34; \
         Add-Type -Namespace T057 -Name Std -MemberDefinition ( \
           '[DllImport(' + $q + 'kernel32.dll' + $q + ')] public static extern IntPtr GetStdHandle(int n); ' + \
           '[DllImport(' + $q + 'kernel32.dll' + $q + ')] public static extern uint GetFileType(IntPtr h); ' + \
           '[DllImport(' + $q + 'kernel32.dll' + $q + ')] public static extern bool GetConsoleMode(IntPtr h, out uint m); ' + \
           '[DllImport(' + $q + 'kernel32.dll' + $q + ')] public static extern IntPtr GetConsoleWindow();'); \
         $h = [T057.Std]::GetStdHandle(-10); $m = [uint32]0; \
         [Console]::Out.WriteLine('stdin_type ' + [T057.Std]::GetFileType($h)); \
         [Console]::Out.WriteLine('stdin_console ' + [T057.Std]::GetConsoleMode($h, [ref]$m)); \
         [Console]::Out.WriteLine('console_window ' + [T057.Std]::GetConsoleWindow().ToInt64())";
    let output = ran(command, PWSH_BUDGET);
    assert!(
        output.status.success(),
        "premise: the probe ran: {output:?}"
    );
    assert_eq!(
        (
            field(&output.stdout, "stdin_type "),
            field(&output.stdout, "stdin_console ")
        ),
        ("2", "False"),
        "the pwsh child's stdin is not the NUL device (file type 2, not a console): {output:?}"
    );
    assert_eq!(
        field(&output.stdout, "console_window "),
        "0",
        "the pwsh child has a console window of its own: {output:?}"
    );
}
