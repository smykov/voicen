//! T-024 step 2 (spec 006 T026, FR-011, FR-014): the installed app records its own
//! crashes. Each scenario runs in a child process: this test exe re-run with
//! `crash_child --exact --ignored` and two environment variables (the mode and the
//! child's own logs dir), because a panic in `extern "C"`, an access violation or a
//! kill ends the process. The parent asserts on the files the child left.
//! Windows CI only (decision #5).
//!
//! - No crash probe binary: `scripts/ci/shell-targets.txt` is unchanged (analysis
//!   decision (v)).
//! - Every child runs with a budget and is killed and waited for at the budget
//!   (F-006). Null stdin, piped output read on threads, CREATE_NO_WINDOW. One child
//!   at a time ([`serial`]).
//! - No OS state is shared between tests (F-012): each test has its own `TempDir`,
//!   and each child its own logs dir under it. The real-runtime child uses its own
//!   app identifier, so WebView2's profile is not the release app's.
//! - The planted panic payload is a fake value. No key is used.
//!
//! The shell API these tests pin (the developer builds it, analysis "seam"):
//!
//! ```text
//! voicen_lib::diag::begin_session(logs_dir: PathBuf, log: &Arc<Log>) -> voicen_lib::diag::SessionEnd
//!     // RtlGetVersion -> Session::begin -> install_panic_hook -> exception filter -> retain_crash_files
//! voicen_lib::diag::SessionEnd           // managed by the app; on_run_event's RunEvent::Exit arm
//!                                        // ends the session once (Session::end_clean)
//! voicen_lib::diag::os_version() -> OsVersion   // RtlGetVersion
//! ```
#![cfg(windows)]

use std::ffi::OsString;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use tauri::menu::{MenuEvent, MenuId};
use tauri::test::{mock_context, noop_assets};
use tauri::{AppHandle, Context, Manager, RunEvent, Wry};
use voicen_core::autostart::{Autostart, FakeAutostart};
use voicen_core::diag::{Log, OsVersion};
use voicen_core::local_models::catalog::MODELS;
use voicen_core::local_models::service::LocalModels;
use voicen_core::local_models::store::ModelStore;
use voicen_core::secrets::{CredentialStore, FakeCredentialStore};
use voicen_core::settings::LoadOutcome;
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_core::tray::TrayAction;
use voicen_lib::settings_ipc::load_settings;
use voicen_lib::tray;
use windows::core::w;
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Diagnostics::Debug::{
    SetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX,
};
use windows::Win32::System::Registry::{
    RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::CREATE_NO_WINDOW;
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToFileTime};

/// The child's mode; unset in a normal run, so `crash_child` does nothing there.
const MODE_VAR: &str = "VOICEN_CRASH_CHILD";
/// The child's logs dir.
const DIR_VAR: &str = "VOICEN_CRASH_DIR";

/// A fake secret the panic carries (`diag_crash.rs`'s canary): it must reach no file.
const PAYLOAD: &str = "secret words 123 sk-test-PLANTED";

/// A child that ends itself (panic, fault, a plain return) must be gone by then.
const CHILD_BUDGET: Duration = Duration::from_secs(60);
/// The real-runtime child: a cold WebView2 start on the runner (F-005).
const APP_BUDGET: Duration = Duration::from_secs(150);
/// How long the parent waits for the idle child's marker before it gives up.
const MARKER_BUDGET: Duration = Duration::from_secs(30);
/// How long the idle child stays up when nobody kills it.
const IDLE_FOR: Duration = Duration::from_secs(120);
/// How long the real-runtime child's driver waits for the loop to end after Exit.
const EXIT_FALLBACK: Duration = Duration::from_secs(30);
/// The fallback code: the loop was still running after the tray Exit item.
const FALLBACK_CODE: i32 = 70;

const MARKER: &str = "session.marker";
const LOG: &str = "voicen.log";
const ACCESS_VIOLATION: u32 = 0xC000_0005;
const IDENTIFIER: &str = "dev.voicen.test.crash";

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- the child side -------------------------------------------------------------------------

/// The child entry. In a normal run (`MODE_VAR` unset, also under `--ignored`) it
/// returns at once.
#[test]
#[ignore = "child entry of the crash tests: run by them, with VOICEN_CRASH_CHILD set"]
fn crash_child() {
    let (Some(mode), Some(dir)) = (std::env::var_os(MODE_VAR), std::env::var_os(DIR_VAR)) else {
        return;
    };
    let logs = PathBuf::from(dir);
    match mode.to_str() {
        Some("panic") => child_panic(logs),
        Some("fault") => child_fault(logs),
        Some("idle") => child_idle(logs),
        Some("next") => child_next(logs),
        Some("exit") => child_exit(logs),
        Some("start_only") => child_start_only(logs),
        other => panic!("unknown child mode {other:?}"),
    }
}

/// As `release_parts` does: the log, then the session (hook and filter included).
fn begin(logs: &Path) -> (Arc<Log>, voicen_lib::diag::SessionEnd) {
    let log = voicen_lib::diag::start(logs.to_path_buf(), Box::new(|_| {}));
    let end = voicen_lib::diag::begin_session(logs.to_path_buf(), &log);
    (log, end)
}

/// No WER dialog box for the child's own fault or abort (the runner has no one to
/// close it).
fn no_error_boxes() {
    // SAFETY: SetErrorMode only sets this process's error mode.
    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
    }
}

extern "C" fn panics_across_c() {
    // A panic cannot unwind out of an `extern "C"` fn: std runs the hook, then
    // aborts (the release build's panic=abort path ends the same way).
    let payload = String::from(PAYLOAD);
    panic!("{}", std::hint::black_box(payload));
}

fn child_panic(logs: PathBuf) {
    no_error_boxes();
    let (_log, _end) = begin(&logs);
    panics_across_c();
    unreachable!("the process aborted");
}

/// The local time `seconds` after now, as `crash-YYYYMMDD-HHMMSS-<pid>.txt`.
fn local_name_after(now: SYSTEMTIME, seconds: u64, pid: u32) -> String {
    let mut ft = FILETIME::default();
    // SAFETY: both pointers are to locals of the right type.
    unsafe { SystemTimeToFileTime(&now, &mut ft) }.expect("SystemTimeToFileTime");
    let ticks =
        (u64::from(ft.dwHighDateTime) << 32 | u64::from(ft.dwLowDateTime)) + seconds * 10_000_000;
    let later = FILETIME {
        dwLowDateTime: (ticks & 0xFFFF_FFFF) as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut st = SYSTEMTIME::default();
    // SAFETY: both pointers are to locals of the right type.
    unsafe { FileTimeToSystemTime(&later, &mut st) }.expect("FileTimeToSystemTime");
    format!(
        "crash-{:04}{:02}{:02}-{:02}{:02}{:02}-{pid}.txt",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
    )
}

/// The planted files of the fault child: the names of the current local second and
/// the next two, this pid (the names its filter tries first).
fn planted_names(logs: &Path) -> Vec<PathBuf> {
    // SAFETY: GetLocalTime has no preconditions.
    let now = unsafe { GetLocalTime() };
    let pid = std::process::id();
    (0..3)
        .map(|s| logs.join(local_name_after(now, s, pid)))
        .collect()
}

fn planted_bytes(i: usize) -> Vec<u8> {
    format!("planted {i}: not the filter's\n").into_bytes()
}

fn child_fault(logs: PathBuf) {
    no_error_boxes();
    let (_log, _end) = begin(&logs);
    for (i, path) in planted_names(&logs).iter().enumerate() {
        std::fs::write(path, planted_bytes(i)).expect("plant a taken name");
    }
    // An access violation in this exe's own code: a read of address 0x10.
    let address = std::hint::black_box(0x10_usize) as *const u8;
    // SAFETY: none; the read faults on purpose, and the process ends in it.
    let _ = unsafe { std::ptr::read_volatile(address) };
    unreachable!("the read of 0x10 did not fault");
}

fn child_idle(logs: PathBuf) {
    let (_log, _end) = begin(&logs);
    // Up until the parent kills it; bounded in case it never does.
    thread::sleep(IDLE_FOR);
}

fn child_next(logs: PathBuf) {
    let (_log, _end) = begin(&logs);
}

/// Characterization (T-024 analysis seam: `diag::start` stays hook-free): a panic
/// after `start` alone writes no crash file and no marker.
fn child_start_only(logs: PathBuf) {
    let _log = voicen_lib::diag::start(logs.clone(), Box::new(|_| {}));
    let caught = std::panic::catch_unwind(|| panic!("{PAYLOAD}"));
    assert!(caught.is_err());
}

fn no_keys() -> Arc<dyn CredentialStore> {
    Arc::new(FakeCredentialStore::new())
}

fn no_autostart() -> Arc<dyn Autostart> {
    Arc::new(FakeAutostart::new())
}

fn idle_models(data: &Path) -> Arc<LocalModels> {
    let (models, cleanup) = LocalModels::open(
        data.join("models"),
        FakeDisk::with_available(u64::MAX),
        Timeouts::default(),
        MODELS,
    );
    cleanup.expect("cleanup over a missing models dir");
    Arc::new(models)
}

/// The real-runtime child: `build_app` on Wry with the session managed, the loop
/// run as `run()` runs it (`on_run_event`), ended by the tray's Exit item
/// (`app.exit(0)`, then `RunEvent::Exit`). Prints `marker_at_ready=<bool>`.
fn child_exit(logs: PathBuf) {
    let data = logs
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| logs.clone());
    let (log, end) = begin(&logs);
    let load = |log: &Log| {
        load_settings(
            data.clone(),
            no_keys(),
            no_autostart(),
            Arc::new(ModelStore::new(data.join("models"), MODELS)),
            None,
            log,
        )
    };
    // A settings.json first, so Ready opens no window (a loaded start).
    let _ = load(&log);
    let (service, outcome) = load(&log);
    assert!(
        matches!(outcome, LoadOutcome::Loaded(_)),
        "premise: {outcome:?}"
    );
    let mut context: Context<Wry> = mock_context(noop_assets());
    context.config_mut().identifier = IDENTIFIER.to_string();
    let app = voicen_lib::build_app(
        tauri::Builder::default().any_thread(),
        context,
        service,
        idle_models(&data),
        log,
    )
    .expect("the Wry app builds");
    app.manage(end);
    let marker = logs.join(MARKER);
    let (tx, rx) = mpsc::channel::<bool>();
    let mut tx = Some(tx);
    let code = app.run_return(move |handle, event| {
        let ready = matches!(event, RunEvent::Ready);
        voicen_lib::on_run_event(handle, event, &outcome, false);
        if ready {
            if let Some(tx) = tx.take() {
                let handle = handle.clone();
                let marker = marker.clone();
                thread::spawn(move || drive_exit(handle, &marker, tx));
            }
        }
    });
    let marker_at_ready = rx.recv_timeout(Duration::from_secs(5)).unwrap_or(false);
    println!("marker_at_ready={marker_at_ready}");
    println!("run_return={code}");
    assert_eq!(code, 0, "run_return after the tray Exit item");
}

fn drive_exit(handle: AppHandle<Wry>, marker: &Path, tx: mpsc::Sender<bool>) {
    let _ = tx.send(marker.exists());
    let inner = handle.clone();
    let _ = handle.run_on_main_thread(move || {
        tray::on_menu_event(
            &inner,
            MenuEvent {
                id: MenuId::new(TrayAction::Exit.as_str()),
            },
        );
    });
    thread::sleep(EXIT_FALLBACK);
    let inner = handle.clone();
    let _ = handle.run_on_main_thread(move || inner.exit(FALLBACK_CODE));
}

// ---- the parent side ------------------------------------------------------------------------

/// What a child left: its exit code (`None` when killed at the budget), pid and
/// output.
#[derive(Debug)]
#[allow(
    dead_code,
    reason = "stderr is shown through Debug in the failure messages"
)]
struct Ran {
    code: Option<i32>,
    pid: u32,
    timed_out: bool,
    stdout: String,
    stderr: String,
}

fn spawn_reader(mut from: impl Read + Send + 'static) -> JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = from.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

fn spawn_child(mode: &str, logs: &Path) -> (Child, [JoinHandle<String>; 2]) {
    let exe = std::env::current_exe().expect("this test exe's path");
    let mut child = Command::new(exe)
        .args([
            "crash_child",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(MODE_VAR, mode)
        .env(DIR_VAR, OsString::from(logs.as_os_str()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()
        .expect("spawn the crash child");
    let (Some(out), Some(err)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("the child started without piped output");
    };
    (child, [spawn_reader(out), spawn_reader(err)])
}

/// Waits for `child` up to `budget`; kills and waits for it at the budget.
fn finish(mut child: Child, readers: [JoinHandle<String>; 2], budget: Duration) -> Ran {
    let pid = child.id();
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < budget => thread::sleep(Duration::from_millis(20)),
            Ok(None) | Err(_) => break None,
        }
    };
    let timed_out = status.is_none();
    if timed_out {
        let _ = child.kill();
        let _ = child.wait();
    }
    let [out, err] = readers;
    Ran {
        code: status.and_then(|s| s.code()),
        pid,
        timed_out,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    }
}

fn run_child(mode: &str, logs: &Path, budget: Duration) -> Ran {
    let (child, readers) = spawn_child(mode, logs);
    let ran = finish(child, readers, budget);
    assert!(
        !ran.timed_out,
        "the `{mode}` child ran past {budget:?}: {ran:#?}"
    );
    ran
}

fn logs_dir(tmp: &TempDir) -> PathBuf {
    tmp.path().join("logs")
}

/// The `crash-*.txt` names in `logs`, sorted.
fn crash_files(logs: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(logs)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| n.starts_with("crash-") && n.ends_with(".txt"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn all_files(logs: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(logs)
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default()
}

fn log_lines(logs: &Path) -> Vec<String> {
    std::fs::read_to_string(logs.join(LOG))
        .map(|t| t.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// The log lines whose message (after `<ts> <LEVEL> `) starts with `prefix`.
fn messages<'a>(lines: &'a [String], prefix: &str) -> Vec<&'a str> {
    lines
        .iter()
        .filter_map(|l| l.splitn(3, ' ').nth(2))
        .filter(|m| m.starts_with(prefix))
        .collect()
}

fn read_text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|l| l.strip_prefix(&format!("{key}: ")))
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn exe_base_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .expect("this test exe's file name")
}

fn new_names(before: &[String], after: &[String]) -> Vec<String> {
    after
        .iter()
        .filter(|n| !before.contains(n))
        .cloned()
        .collect()
}

/// Runs an `idle` child over `logs`, waits for its marker naming its pid, kills it
/// (TerminateProcess) and returns that pid: the marker is left for the next begin.
fn killed_idle_session(logs: &Path) -> u32 {
    let (mut child, readers) = spawn_child("idle", logs);
    let pid = child.id();
    let deadline = Instant::now() + MARKER_BUDGET;
    let marked = loop {
        let text = std::fs::read_to_string(logs.join(MARKER)).unwrap_or_default();
        if text.lines().any(|l| l == format!("pid={pid}")) {
            break true;
        }
        if Instant::now() >= deadline || matches!(child.try_wait(), Ok(Some(_))) {
            break false;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let _ = child.kill();
    let ran = finish(child, readers, CHILD_BUDGET);
    assert!(
        marked,
        "the idle child wrote no marker naming pid {pid} within {MARKER_BUDGET:?}: {ran:#?}"
    );
    pid
}

#[test]
fn a_panic_in_extern_c_leaves_one_panic_file_without_the_payload() {
    // Acceptance (a): the panic hook runs before std aborts; its first-wins guard
    // absorbs the second (no-unwind) panic, and the abort's fail-fast bypasses the
    // exception filter: exactly one `kind: panic` file. Then the next begin names it.
    // Bite: begin_session not installing the hook (no file), the filter also
    // writing (two files), the message written, frames not resolved to modules.
    let _serial = serial();
    let tmp = TempDir::new();
    let logs = logs_dir(&tmp);
    let ran = run_child("panic", &logs, CHILD_BUDGET);
    assert!(
        ran.code.is_some_and(|c| c != 0),
        "the panicking child ended with {:?}: {ran:#?}",
        ran.code
    );
    let files = crash_files(&logs);
    assert_eq!(files.len(), 1, "one crash file: {files:?}; {ran:#?}");
    let name = files.first().cloned().unwrap_or_default();
    let text = read_text(&logs.join(&name));
    assert_eq!(value(&text, "kind"), Some("panic"), "{text}");
    assert!(
        name.ends_with(&format!("-{}.txt", ran.pid)),
        "the file names the child's pid {}: {name}",
        ran.pid
    );
    assert!(
        value(&text, "location").is_some_and(|l| l.starts_with(&format!("{}:", file!()))),
        "the location is this file ({}): {text}",
        file!()
    );
    let exe = exe_base_name();
    assert!(
        text.lines().any(|l| l
            .strip_prefix("  ")
            .is_some_and(|f| f.starts_with(&format!("{exe}+0x")))),
        "a backtrace frame in this exe ({exe}): {text}"
    );
    for path in all_files(&logs) {
        let bytes = std::fs::read(&path).unwrap_or_default();
        assert!(
            !contains(&bytes, PAYLOAD.as_bytes()) && !contains(&bytes, &utf16le(PAYLOAD)),
            "the panic payload reached {}",
            path.display()
        );
    }
    assert!(logs.join(MARKER).exists(), "a crash keeps the marker");

    let next = run_child("next", &logs, CHILD_BUDGET);
    assert_eq!(next.code, Some(0), "{next:#?}");
    let lines = log_lines(&logs);
    let previous = messages(&lines, "previous session");
    assert_eq!(
        previous,
        vec![format!("previous session outcome=crashed crash_file={name}").as_str()],
        "{lines:#?}"
    );
    assert_eq!(crash_files(&logs), files, "the next begin adds no file");
}

#[test]
fn an_access_violation_leaves_one_native_fault_file_and_planted_files_keep_their_bytes() {
    // Acceptance (b): the filter writes the file and returns
    // EXCEPTION_CONTINUE_SEARCH, so the exit code stays Windows' (0xC0000005). The
    // child planted files under its own first three names first: the filter never
    // replaces them. Bite: no filter, a filter that handles the exception (exit code
    // changes), a writer that replaces a taken name, the OS version not from
    // RtlGetVersion, an unresolved module.
    let _serial = serial();
    let tmp = TempDir::new();
    let logs = logs_dir(&tmp);
    let ran = run_child("fault", &logs, CHILD_BUDGET);
    assert_eq!(
        ran.code,
        Some(ACCESS_VIOLATION as i32),
        "the faulting child's exit code: {ran:#?}"
    );
    let files = crash_files(&logs);
    let planted: Vec<String> = files
        .iter()
        .filter(|n| std::fs::read(logs.join(n.as_str())).is_ok_and(|b| b.starts_with(b"planted ")))
        .cloned()
        .collect();
    assert_eq!(
        planted.len(),
        3,
        "premise: three planted files: {files:?}; {ran:#?}"
    );
    for name in &planted {
        let bytes = std::fs::read(logs.join(name)).unwrap_or_default();
        let i = planted_index(&bytes);
        assert_eq!(bytes, planted_bytes(i), "planted {name} changed");
    }
    let written = new_names(&planted, &files);
    assert_eq!(
        written.len(),
        1,
        "one native_fault file: {files:?}; {ran:#?}"
    );
    let name = written.first().cloned().unwrap_or_default();
    assert!(name.ends_with(&format!("-{}.txt", ran.pid)), "{name}");
    let text = read_text(&logs.join(&name));
    assert_eq!(value(&text, "kind"), Some("native_fault"), "{text}");
    assert_eq!(value(&text, "exception_code"), Some("0xC0000005"), "{text}");
    let exe = exe_base_name();
    assert!(
        value(&text, "fault_address").is_some_and(|f| f.starts_with(&format!("{exe}+0x"))),
        "the fault is in this exe ({exe}): {text}"
    );
    let os = voicen_lib::diag::os_version();
    assert_eq!(value(&text, "os"), Some(os.to_string().as_str()), "{text}");
    assert!(logs.join(MARKER).exists(), "a fault keeps the marker");

    let next = run_child("next", &logs, CHILD_BUDGET);
    assert_eq!(next.code, Some(0), "{next:#?}");
    let lines = log_lines(&logs);
    assert_eq!(
        messages(&lines, "previous session"),
        vec![format!("previous session outcome=crashed crash_file={name}").as_str()],
        "{lines:#?}"
    );
}

fn planted_index(bytes: &[u8]) -> usize {
    (0..3)
        .find(|&i| planted_bytes(i) == bytes)
        .unwrap_or(usize::MAX)
}

#[test]
fn a_killed_session_gets_one_abnormal_end_file_on_the_next_begin() {
    // Acceptance (c): TerminateProcess leaves the marker; the next begin writes one
    // abnormal_end file naming the killed pid and logs ended_abnormally with it.
    // Bite: begin_session not writing the marker, or a Drop that deletes it.
    let _serial = serial();
    let tmp = TempDir::new();
    let logs = logs_dir(&tmp);
    let pid = killed_idle_session(&logs);
    assert!(
        crash_files(&logs).is_empty(),
        "premise: no crash file before the next begin"
    );

    let next = run_child("next", &logs, CHILD_BUDGET);
    assert_eq!(next.code, Some(0), "{next:#?}");
    let files = crash_files(&logs);
    assert_eq!(files.len(), 1, "one abnormal_end file: {files:?}");
    let name = files.first().cloned().unwrap_or_default();
    let text = read_text(&logs.join(&name));
    assert_eq!(value(&text, "kind"), Some("abnormal_end"), "{text}");
    assert_eq!(
        value(&text, "previous_session_pid"),
        Some(pid.to_string().as_str()),
        "{text}"
    );
    let lines = log_lines(&logs);
    assert_eq!(
        messages(&lines, "previous session"),
        vec![format!("previous session outcome=ended_abnormally crash_file={name}").as_str()],
        "{lines:#?}"
    );
}

const DAY: Duration = Duration::from_secs(86_400);
const HOUR: Duration = Duration::from_secs(3_600);

/// Writes a planted crash file `name` in `logs` whose modification time is `age`
/// before now. Its text matches no session (no `session_started:` line).
fn plant_crash_file(logs: &Path, name: &str, age: Duration) {
    let path = logs.join(name);
    std::fs::write(&path, format!("planted for retention: {name}\n"))
        .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    let at = SystemTime::now()
        .checked_sub(age)
        .expect("now minus the planted age");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .and_then(|f| f.set_modified(at))
        .unwrap_or_else(|e| panic!("set_modified {}: {e}", path.display()));
}

/// A planted name: a year-2000 local time and a pid no child has (`i` in the
/// seconds), so it never collides with a name diag writes now.
fn planted_crash_name(i: usize) -> String {
    format!("crash-20000101-0000{i:02}-1.txt")
}

#[test]
fn the_next_begin_retains_the_20_newest_crash_files_counting_its_own_abnormal_end() {
    // Acceptance failure branch (review 1 #1): "a crash file 20 days old survives
    // rotation; the 21st crash file removes the oldest". begin_session runs
    // retain_crash_files AFTER Session::begin (invariant 1), so begin's own
    // abnormal_end file counts toward the cap of 20. Planted after the kill (the idle
    // child's own begin must not prune them): one 31 days old, one 25 days old, one
    // 20 days old and 18 of the last 18 hours = 21 files; the next begin adds its
    // abnormal_end = 22. Right order: the 31-day file goes by age, then the 25-day one
    // by the cap: 20 left, the 20-day one and the new file among them.
    // Bite (P-017, by reading; Windows CI only):
    // - no retain_crash_files call (or over another dir): 22 files, the 31-day file
    //   still there -> `old_gone` red;
    // - the call moved above Session::begin: it sees 21 plants, drops the 31-day one
    //   (20 left, no cap), then begin adds its file -> 21 files and the 25-day one kept
    //   -> `capped_gone` / the count red.
    let _serial = serial();
    let tmp = TempDir::new();
    let logs = logs_dir(&tmp);
    let pid = killed_idle_session(&logs);
    assert!(
        crash_files(&logs).is_empty(),
        "premise: the kill left no crash file before the plants"
    );
    let old = planted_crash_name(0);
    let capped = planted_crash_name(1);
    let kept = planted_crash_name(2);
    plant_crash_file(&logs, &old, DAY * 31);
    plant_crash_file(&logs, &capped, DAY * 25);
    plant_crash_file(&logs, &kept, DAY * 20);
    let recent: Vec<String> = (3..21).map(planted_crash_name).collect();
    for (hours, name) in (1_u32..).zip(&recent) {
        plant_crash_file(&logs, name, HOUR * hours);
    }
    let planted = crash_files(&logs);
    assert_eq!(planted.len(), 21, "premise: 21 planted files: {planted:?}");

    let next = run_child("next", &logs, CHILD_BUDGET);
    assert_eq!(next.code, Some(0), "{next:#?}");
    let files = crash_files(&logs);
    let old_gone = !files.contains(&old);
    assert!(
        old_gone,
        "the 31-day file survived the next begin: {files:?}"
    );
    let capped_gone = !files.contains(&capped);
    assert!(
        capped_gone,
        "the 25-day file (the oldest of 21 within 30 days, begin's file included) was kept: {files:?}"
    );
    assert!(
        files.contains(&kept),
        "the 20-day file was removed: {files:?}"
    );
    assert_eq!(files.len(), 20, "exactly 20 crash files remain: {files:?}");
    for name in &recent {
        assert!(
            files.contains(name),
            "the recent {name} was removed: {files:?}"
        );
    }
    let written = new_names(&planted, &files);
    assert_eq!(
        written.len(),
        1,
        "the next begin's own file is among the 20: {files:?}"
    );
    let name = written.first().cloned().unwrap_or_default();
    let text = read_text(&logs.join(&name));
    assert_eq!(value(&text, "kind"), Some("abnormal_end"), "{text}");
    assert_eq!(
        value(&text, "previous_session_pid"),
        Some(pid.to_string().as_str()),
        "{text}"
    );
}

#[test]
fn a_begin_deletes_a_crash_file_past_30_days_and_keeps_a_29_day_one() {
    // The age rule alone (below the cap, so the cap cannot remove anything): with no
    // previous marker the next begin writes no file of its own, and of two planted
    // files only the one past CRASH_MAX_AGE_DAYS goes.
    // Bite (by reading): no retain_crash_files call, or over another dir -> the
    // 31-day file kept; a `now` that is not the clock's (e.g. the epoch, so nothing is
    // old) -> the 31-day file kept (the 20-file test above cannot tell, since its cap
    // drops the oldest anyway).
    let _serial = serial();
    let tmp = TempDir::new();
    let logs = logs_dir(&tmp);
    std::fs::create_dir_all(&logs).expect("create the logs dir");
    let old = planted_crash_name(0);
    let young = planted_crash_name(1);
    plant_crash_file(&logs, &old, DAY * 31);
    plant_crash_file(&logs, &young, DAY * 29);

    let next = run_child("next", &logs, CHILD_BUDGET);
    assert_eq!(next.code, Some(0), "{next:#?}");
    assert_eq!(
        crash_files(&logs),
        vec![young],
        "only the 29-day file is left"
    );
}

#[test]
fn the_tray_exit_item_ends_the_session_once() {
    // Acceptance (d): app.exit(0) -> RunEvent::Exit -> on_run_event ends the
    // managed session: the marker (present at Ready) is gone, one `exited` line, no
    // crash file; the next begin finds a clean previous session (no line, no file).
    // Bite: no RunEvent::Exit arm, end_clean called twice (a second `exited`), the
    // session ended at Ready or on ExitRequested.
    let _serial = serial();
    let tmp = TempDir::new();
    let logs = logs_dir(&tmp);
    let ran = run_child("exit", &logs, APP_BUDGET);
    assert_eq!(ran.code, Some(0), "the app child: {ran:#?}");
    assert!(
        ran.stdout.contains("marker_at_ready=true"),
        "premise: the marker existed while the app ran: {ran:#?}"
    );
    assert!(
        !logs.join(MARKER).exists(),
        "the clean exit deleted the marker"
    );
    let lines = log_lines(&logs);
    assert_eq!(
        messages(&lines, "exited").len(),
        1,
        "one `exited` line: {lines:#?}"
    );
    assert!(crash_files(&logs).is_empty(), "{:?}", crash_files(&logs));

    let next = run_child("next", &logs, CHILD_BUDGET);
    assert_eq!(next.code, Some(0), "{next:#?}");
    let lines = log_lines(&logs);
    assert!(
        messages(&lines, "previous session").is_empty(),
        "a clean exit is no previous-session outcome: {lines:#?}"
    );
    assert!(crash_files(&logs).is_empty(), "{:?}", crash_files(&logs));
}

/// A DWORD or a string value under `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion`.
fn current_version_dword(name: windows::core::PCWSTR) -> Option<u32> {
    let mut data = 0_u32;
    let mut size = 4_u32;
    // SAFETY: data and size point to locals; size is data's size in bytes.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
            name,
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut data).cast()),
            Some(&mut size),
        )
    };
    status.is_ok().then_some(data)
}

fn current_build_number() -> Option<u32> {
    let mut buf = [0_u16; 64];
    let mut size = u32::try_from(std::mem::size_of_val(&buf)).unwrap_or(0);
    // SAFETY: buf and size point to locals; size is buf's size in bytes.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
            w!("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status.is_err() {
        return None;
    }
    let len = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(buf.get(..len)?)
        .trim()
        .parse()
        .ok()
}

#[test]
fn the_os_version_is_rtl_get_version_s() {
    // Acceptance (e): RtlGetVersion is not shimmed by the compatibility layer (a test
    // exe has no manifest, so GetVersionExW would say 6.2): major >= 10, a build, and
    // the same numbers as the registry's CurrentVersion values.
    let _serial = serial();
    let os: OsVersion = voicen_lib::diag::os_version();
    assert!(os.major >= 10, "{os:?}");
    assert!(os.build > 0, "{os:?}");
    let registry = (
        current_version_dword(w!("CurrentMajorVersionNumber")),
        current_version_dword(w!("CurrentMinorVersionNumber")),
        current_build_number(),
    );
    assert_eq!(
        (Some(os.major), Some(os.minor), Some(os.build)),
        registry,
        "RtlGetVersion vs the registry"
    );
}

#[test]
fn characterization_diag_start_alone_installs_no_hook() {
    // Characterization (behaviour the task keeps): `diag::start` stays hook-free, so
    // shell tests that panic on purpose write no crash file and no marker.
    let _serial = serial();
    let tmp = TempDir::new();
    let logs = logs_dir(&tmp);
    let ran = run_child("start_only", &logs, CHILD_BUDGET);
    assert_eq!(ran.code, Some(0), "{ran:#?}");
    assert!(crash_files(&logs).is_empty(), "{:?}", crash_files(&logs));
    assert!(!logs.join(MARKER).exists(), "diag::start wrote a marker");
}
