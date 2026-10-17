//! T-100 (spec 006 T024, FR-011, FR-014..FR-016; FR-20, NFR-04, NFR-07): crash
//! files in core: `render_crash_file`, `retain_crash_files` and the panic hook.
//!
//! Its own test binary: the panic hook is process-global. Every test takes
//! [`serial`] first, and every test that installs the hook puts the previous hook
//! back before it asserts anything ([`with_hook`]), so a failing assertion is
//! reported by the default hook, not written as a crash file.
//!
//! A panic inside the hook cannot be tested in-process: std aborts it (T-100
//! Investigation probe, exit 134). So "the hook never panics" is tested as: an
//! unwritable logs path writes nothing and the hook returns (the thread unwinds and
//! `join` gives `Err`); a frame source that reports more frames than the buffer
//! holds does not panic. A hook that panics aborts this whole binary.
//!
//! The API these tests pin (the developer builds it; contracts/core-diag.md "Crash
//! files" is updated with it):
//!
//! ```text
//! voicen_core::diag::{CrashContext, CrashDetail, CrashFields, CrashKind, CodeLocation, Frame,
//!     FrameSource, LocalStamp, ModuleResolver, OsVersion, ThreadName, install_panic_hook,
//!     render_crash_file, retain_crash_files, CRASH_MAX_AGE_DAYS, CRASH_MAX_FILES}
//! pub trait FrameSource: Send + Sync { fn capture(&self, out: &mut [usize]) -> usize; }  // ips, count
//! pub trait ModuleResolver: Send + Sync { fn resolve(&self, ip: usize) -> Option<(String, usize)>; }
//! pub struct CrashContext { logs_dir: PathBuf, build: BuildInfo, os: OsVersion,
//!     session_started: SystemTime, pid: u32, clock: Arc<dyn Clock>, offset: Arc<dyn LocalOffset>,
//!     frames: Arc<dyn FrameSource>, modules: Arc<dyn ModuleResolver> }            // pub fields
//! pub struct LocalStamp { at: SystemTime, seconds_east: i32 }                       // pub fields
//! pub struct CrashFields { time: LocalStamp, build: BuildInfo, os: OsVersion,
//!     session_started: Option<LocalStamp>, detail: CrashDetail }                   // pub fields
//! pub enum CrashDetail { Panic { thread: ThreadName, location: Option<CodeLocation>, backtrace: Vec<Frame> },
//!     NativeFault { exception_code: u32, fault_address: Option<Frame> },
//!     AbnormalEnd { previous_session_pid: Option<u32> } }   // CrashDetail::kind() -> CrashKind
//! ThreadName::new(Option<&str>)  CodeLocation::new(&str, u32, u32)  Frame::new(&str, usize)
//! ```
//!
//! Portable fixtures (these tests also run on the Windows CI job): the panic
//! location is compared with `file!()` (backslashes on Windows), paths are built
//! with `Path::join`. Module-path strings below are adapter output, not paths of
//! this host: core must reduce both separators on every OS.

mod diag_support;

use std::panic::{AssertUnwindSafe, Location};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use diag_support::{
    all_lines, at, closed, contains, crash_file_name, files_under, lines_of, open_log, parse_ts,
    utf16le, Line, Offset, NOON_UTC,
};
use voicen_core::build_info::BuildInfo;
use voicen_core::clock::{Clock, FakeClock};
use voicen_core::diag::{
    install_panic_hook, render_crash_file, retain_crash_files, CodeLocation, CrashContext,
    CrashDetail, CrashFields, CrashKind, Frame, FrameSource, LocalStamp, LogConfig, ModuleResolver,
    OsVersion, PreviousSession, Session, SessionStart, ThreadName, CRASH_MAX_AGE_DAYS,
    CRASH_MAX_FILES,
};
use voicen_core::test_support::TempDir;

const DAY: u64 = 86_400;
const EAST: i32 = 7_200;
const PID: u32 = 4_242;
/// The planted panic payload (spec 006 T024): never in any crash file.
const PAYLOAD: &str = "secret words 123 sk-test-PLANTED";

// ---- serialisation and the hook ---------------------------------------------------------

static HOOK: Mutex<()> = Mutex::new(());

/// Every test of this binary runs under this lock (the hook is process-global).
fn serial() -> MutexGuard<'static, ()> {
    HOOK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Installs the crash hook over `ctx`, runs `body`, and puts the previous hook back
/// (also when `body` panics), before the caller asserts.
fn with_hook<R>(ctx: CrashContext, body: impl FnOnce() -> R) -> R {
    let previous = std::panic::take_hook();
    install_panic_hook(ctx);
    let out = std::panic::catch_unwind(AssertUnwindSafe(body));
    let _ours = std::panic::take_hook();
    std::panic::set_hook(previous);
    match out {
        Ok(r) => r,
        Err(p) => std::panic::resume_unwind(p),
    }
}

/// Where [`boom`] panicked: (file, line, column).
type Seen = Mutex<Option<(String, u32, u32)>>;

fn seen() -> Arc<Seen> {
    Arc::new(Mutex::new(None))
}

/// Panics with [`PAYLOAD`] at the caller's location and records that location.
#[track_caller]
fn boom(seen: &Seen) -> ! {
    let here = Location::caller();
    *seen.lock().unwrap_or_else(PoisonError::into_inner) =
        Some((here.file().to_string(), here.line(), here.column()));
    panic!("{}", PAYLOAD);
}

// ---- fakes ----------------------------------------------------------------------------------

fn build() -> BuildInfo {
    BuildInfo {
        version: "0.9.1",
        commit: "abc1234",
    }
}

fn os() -> OsVersion {
    OsVersion {
        major: 10,
        minor: 0,
        build: 22_631,
    }
}

/// A frame source that reports `ips` (or claims `claim` frames without writing
/// more than the buffer).
struct Frames {
    ips: Vec<usize>,
    claim: Option<usize>,
}

impl FrameSource for Frames {
    fn capture(&self, out: &mut [usize]) -> usize {
        for (slot, ip) in out.iter_mut().zip(&self.ips) {
            *slot = *ip;
        }
        self.claim.unwrap_or_else(|| self.ips.len().min(out.len()))
    }
}

/// Resolves `ip` by the table of (base address, module text as the adapter
/// returns it).
struct Modules(Vec<(usize, String)>);

impl ModuleResolver for Modules {
    fn resolve(&self, ip: usize) -> Option<(String, usize)> {
        self.0
            .iter()
            .rev()
            .find(|(base, _)| ip >= *base)
            .map(|(base, name)| (name.clone(), ip - base))
    }
}

/// A clock one second later at every read: two writes that each read it get
/// different file names, so a missing first-wins guard shows as two files.
struct Stepping(AtomicU64);

impl Clock for Stepping {
    fn now(&self) -> SystemTime {
        at(self.0.fetch_add(1, Ordering::SeqCst), 0)
    }
}

const VOICEN_BASE: usize = 0x1000_0000;
const NTDLL_BASE: usize = 0x2000_0000;

fn default_modules() -> Modules {
    Modules(vec![
        (VOICEN_BASE, "voicen.exe".to_string()),
        (NTDLL_BASE, "C:\\Windows\\System32\\ntdll.dll".to_string()),
    ])
}

fn default_frames() -> Frames {
    Frames {
        ips: vec![VOICEN_BASE + 0x10, NTDLL_BASE + 0x2bc, VOICEN_BASE + 0xabc],
        claim: None,
    }
}

fn context(
    logs_dir: PathBuf,
    clock: Arc<dyn Clock>,
    frames: Frames,
    modules: Modules,
) -> CrashContext {
    CrashContext {
        logs_dir,
        build: build(),
        os: os(),
        session_started: at(NOON_UTC - 3_600, 0),
        pid: PID,
        clock,
        offset: Arc::new(Offset(EAST)),
        frames: Arc::new(frames),
        modules: Arc::new(modules),
    }
}

fn logs_dir(tmp: &TempDir) -> PathBuf {
    let dir = tmp.path().join("logs");
    std::fs::create_dir(&dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
    dir
}

/// `crash-*.txt` names in `dir`, sorted.
fn crash_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
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

/// Every file under `dir` holds the payload 0 times (UTF-8 and UTF-16LE), nor
/// any of its words.
#[track_caller]
fn assert_no_payload(dir: &Path) {
    for path in files_under(dir) {
        let bytes = std::fs::read(&path).unwrap_or_default();
        for needle in [PAYLOAD, "sk-test-PLANTED", "secret words"] {
            assert!(
                !contains(&bytes, needle.as_bytes()) && !contains(&bytes, &utf16le(needle)),
                "{needle:?} found in {}",
                path.display()
            );
        }
    }
}

/// Keys of a crash file in data-model order, and the line forms: `key: value`, the
/// bare `backtrace:` line, and `  <frame>` lines after it.
const KEY_ORDER: &[&str] = &[
    "kind",
    "time",
    "version",
    "commit",
    "os",
    "session_started",
    "thread",
    "location",
    "exception_code",
    "fault_address",
    "backtrace",
    "previous_session_pid",
];

/// A frame as written: `<module>+0x<lowercase hex>`, the module a bare file name
/// (no directory separator of either OS, no control character, at most 255 bytes).
fn frame_ok(s: &str) -> bool {
    let Some((module, hex)) = s.rsplit_once("+0x") else {
        return false;
    };
    !module.is_empty()
        && module.len() <= 255
        && !module.contains(['/', '\\'])
        && !module.chars().any(char::is_control)
        && !hex.is_empty()
        && hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Checks the file's form and returns its keys in order; panics naming the line.
#[track_caller]
fn keys_of(text: &str) -> Vec<String> {
    assert!(
        text.ends_with('\n'),
        "the last line is terminated: {text:?}"
    );
    assert!(!text.contains('\r'), "no CR: {text:?}");
    let mut keys: Vec<String> = Vec::new();
    let mut in_backtrace = false;
    for line in text.split_terminator('\n') {
        if let Some(frame) = line.strip_prefix("  ") {
            assert!(
                in_backtrace,
                "a frame line outside backtrace: {line:?} in {text:?}"
            );
            assert!(frame_ok(frame), "frame {frame:?} in {text:?}");
            continue;
        }
        in_backtrace = false;
        if line == "backtrace:" {
            keys.push("backtrace".to_string());
            in_backtrace = true;
            continue;
        }
        let (key, value) = line
            .split_once(": ")
            .unwrap_or_else(|| panic!("line {line:?} is not `key: value` in {text:?}"));
        assert!(!value.is_empty(), "empty value of {key} in {text:?}");
        keys.push(key.to_string());
    }
    let mut order = KEY_ORDER.iter();
    for key in &keys {
        assert!(
            order.any(|k| k == key),
            "key {key:?} is not allowlisted, repeated or out of data-model order: {keys:?}"
        );
    }
    keys
}

// ---- render_crash_file ------------------------------------------------------------------

fn stamp(secs: u64, millis: u64) -> LocalStamp {
    LocalStamp {
        at: at(secs, millis),
        seconds_east: EAST,
    }
}

fn fields(detail: CrashDetail) -> CrashFields {
    CrashFields {
        time: stamp(NOON_UTC, 123),
        build: build(),
        os: os(),
        session_started: Some(stamp(NOON_UTC - 3_600, 0)),
        detail,
    }
}

const HEAD: &str = "time: 2026-10-04T14:00:00.123+02:00\nversion: 0.9.1\ncommit: abc1234\n\
                    os: 10.0.22631\nsession_started: 2026-10-04T13:00:00.000+02:00\n";

#[test]
fn render_panic_writes_the_allowlisted_keys_in_order() {
    let _serial = serial();
    let detail = CrashDetail::Panic {
        thread: ThreadName::new(Some("main")),
        location: Some(CodeLocation::new("src\\main.rs", 10, 5)),
        backtrace: vec![
            Frame::new("voicen.exe", 0x1a2b),
            Frame::new("ntdll.dll", 0x10),
        ],
    };
    assert_eq!(detail.kind(), CrashKind::Panic);
    let text = render_crash_file(&fields(detail));
    assert_eq!(
        text,
        format!(
            "kind: panic\n{HEAD}thread: main\nlocation: src\\main.rs:10:5\nbacktrace:\n  \
             voicen.exe+0x1a2b\n  ntdll.dll+0x10\n"
        )
    );
    keys_of(&text);
}

#[test]
fn render_native_fault_writes_the_allowlisted_keys_in_order() {
    let _serial = serial();
    let detail = CrashDetail::NativeFault {
        exception_code: 0xC000_0005,
        fault_address: Some(Frame::new("voicen.exe", 0x1a2b)),
    };
    assert_eq!(detail.kind(), CrashKind::NativeFault);
    let text = render_crash_file(&fields(detail));
    assert_eq!(
        text,
        format!(
            "kind: native_fault\n{HEAD}exception_code: 0xC0000005\nfault_address: voicen.exe+0x1a2b\n"
        )
    );
    keys_of(&text);
}

#[test]
fn render_abnormal_end_writes_the_allowlisted_keys_in_order() {
    let _serial = serial();
    let detail = CrashDetail::AbnormalEnd {
        previous_session_pid: Some(4_100),
    };
    assert_eq!(detail.kind(), CrashKind::AbnormalEnd);
    let text = render_crash_file(&fields(detail));
    assert_eq!(
        text,
        format!("kind: abnormal_end\n{HEAD}previous_session_pid: 4100\n")
    );
    keys_of(&text);
}

#[test]
fn render_an_unnamed_thread_and_a_missing_session_link() {
    let _serial = serial();
    let mut f = fields(CrashDetail::Panic {
        thread: ThreadName::new(None),
        location: None,
        backtrace: Vec::new(),
    });
    f.session_started = None;
    let text = render_crash_file(&f);
    let keys = keys_of(&text);
    assert!(text.contains("\nthread: unnamed\n"), "{text:?}");
    assert!(
        !keys
            .iter()
            .any(|k| k == "session_started" || k == "location"),
        "{keys:?}"
    );
}

#[test]
fn adapter_text_cannot_add_a_line_or_a_directory() {
    // Invariant (3): thread name, location file and module text are validated by
    // core: no line break, no second `kind:`, a module reduced to its base name
    // (a full path names the user), bounded.
    let _serial = serial();
    let long = "a".repeat(1_000) + ".dll";
    let detail = CrashDetail::Panic {
        thread: ThreadName::new(Some("worker\nkind: native_fault\r")),
        location: Some(CodeLocation::new("src\\lib.rs\nkind: abnormal_end", 7, 9)),
        backtrace: vec![
            Frame::new("C:\\Users\\alice\\AppData\\Local\\Voicen\\voicen.exe", 0x10),
            Frame::new("/home/alice/.cargo/lib/libstd.so", 0x20),
            Frame::new("C:\\Users\\alice\\evil\nkind: x.dll", 0x30),
            Frame::new(&long, 0x40),
        ],
    };
    let text = render_crash_file(&fields(detail));
    let keys = keys_of(&text);
    assert_eq!(keys.iter().filter(|k| *k == "kind").count(), 1, "{text:?}");
    assert!(
        !text.contains("alice"),
        "a user name from a module path: {text:?}"
    );
    let frames: Vec<&str> = text.lines().filter_map(|l| l.strip_prefix("  ")).collect();
    assert_eq!(frames.len(), 4, "{text:?}");
    assert_eq!(frames.first().copied(), Some("voicen.exe+0x10"));
    assert_eq!(frames.get(1).copied(), Some("libstd.so+0x20"));
}

// ---- the panic hook ---------------------------------------------------------------------

#[test]
fn a_panicking_thread_writes_one_crash_file_without_the_payload() {
    let _serial = serial();
    let tmp = TempDir::new();
    let dir = logs_dir(&tmp);
    let clock: Arc<dyn Clock> = Arc::new(FakeClock::at(at(NOON_UTC, 0)));
    let ctx = context(dir.clone(), clock, default_frames(), default_modules());
    let seen = seen();
    let joined = with_hook(ctx, || {
        let seen = Arc::clone(&seen);
        std::thread::Builder::new()
            .name("crash-test-worker".to_string())
            .spawn(move || boom(&seen))
            .map(|h| h.join().is_err())
    });
    assert_eq!(
        joined.ok(),
        Some(true),
        "the thread unwound after the hook returned"
    );
    let (file, line, column) = seen
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .unwrap_or_else(|| panic!("the thread did not reach the panic"));
    assert_eq!(file, file!(), "the panic location is this test file");
    let name = format!("crash-20261004-140000-{PID}.txt");
    assert_eq!(
        crash_files(&dir),
        vec![name.clone()],
        "one crash file, named by local time and pid"
    );
    let expected = [
        "kind: panic".to_string(),
        "time: 2026-10-04T14:00:00.000+02:00".to_string(),
        "version: 0.9.1".to_string(),
        "commit: abc1234".to_string(),
        "os: 10.0.22631".to_string(),
        "session_started: 2026-10-04T13:00:00.000+02:00".to_string(),
        "thread: crash-test-worker".to_string(),
        format!("location: {file}:{line}:{column}"),
        "backtrace:".to_string(),
        "  voicen.exe+0x10".to_string(),
        "  ntdll.dll+0x2bc".to_string(),
        "  voicen.exe+0xabc".to_string(),
    ];
    assert_eq!(lines_of(&dir.join(&name)), expected);
    assert_no_payload(tmp.path());
    let leftovers: Vec<PathBuf> = files_under(tmp.path())
        .into_iter()
        .filter(|p| p.file_name().is_some_and(|n| n != name.as_str()))
        .collect();
    assert!(
        leftovers.is_empty(),
        "nothing but the crash file: {leftovers:?}"
    );
}

#[test]
fn the_hook_returns_and_writes_nothing_when_the_logs_path_is_a_file() {
    // The container runs as uid 0, and mode bits mean nothing on Windows: a regular
    // file at the logs path is what makes it unwritable (tests/diag_log.rs).
    let _serial = serial();
    let tmp = TempDir::new();
    let dir = tmp.path().join("logs");
    std::fs::write(&dir, b"not a folder\n").unwrap_or_else(|e| panic!("plant: {e}"));
    let clock: Arc<dyn Clock> = Arc::new(FakeClock::at(at(NOON_UTC, 0)));
    let ctx = context(dir.clone(), clock, default_frames(), default_modules());
    let seen = seen();
    let joined = with_hook(ctx, || {
        let seen = Arc::clone(&seen);
        std::thread::spawn(move || boom(&seen)).join().is_err()
    });
    assert!(joined, "the hook returned and the thread unwound normally");
    assert_eq!(
        files_under(tmp.path()),
        vec![dir.clone()],
        "no file written anywhere"
    );
    assert_eq!(
        std::fs::read(&dir).ok(),
        Some(b"not a folder\n".to_vec()),
        "the file is untouched"
    );
}

#[test]
fn one_hook_writes_at_most_one_file_first_panic_wins() {
    let _serial = serial();
    let tmp = TempDir::new();
    let dir = logs_dir(&tmp);
    let clock: Arc<dyn Clock> = Arc::new(Stepping(AtomicU64::new(NOON_UTC)));
    let ctx = context(dir.clone(), clock, default_frames(), default_modules());
    let seen = seen();
    with_hook(ctx, || {
        for name in ["first", "second"] {
            let seen = Arc::clone(&seen);
            let joined = std::thread::Builder::new()
                .name(name.to_string())
                .spawn(move || boom(&seen))
                .map(|h| h.join().is_err());
            assert_eq!(joined.ok(), Some(true));
        }
    });
    let files = crash_files(&dir);
    assert_eq!(files.len(), 1, "one file per installed hook: {files:?}");
    let lines = files
        .first()
        .map(|n| lines_of(&dir.join(n)))
        .unwrap_or_default();
    assert!(
        lines.contains(&"thread: first".to_string()),
        "the first panic wins: {lines:?}"
    );
    assert_no_payload(tmp.path());
}

#[test]
fn two_threads_panicking_at_once_write_one_file() {
    let _serial = serial();
    let tmp = TempDir::new();
    let dir = logs_dir(&tmp);
    let clock: Arc<dyn Clock> = Arc::new(Stepping(AtomicU64::new(NOON_UTC)));
    let ctx = context(dir.clone(), clock, default_frames(), default_modules());
    let seen = seen();
    let barrier = Arc::new(Barrier::new(2));
    let joined = with_hook(ctx, || {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let (seen, barrier) = (Arc::clone(&seen), Arc::clone(&barrier));
                std::thread::spawn(move || {
                    barrier.wait();
                    boom(&seen)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().is_err())
            .collect::<Vec<bool>>()
    });
    assert_eq!(joined, vec![true, true], "both threads unwound");
    let files = crash_files(&dir);
    assert_eq!(files.len(), 1, "one file for two panics: {files:?}");
    let lines = files
        .first()
        .map(|n| lines_of(&dir.join(n)))
        .unwrap_or_default();
    assert!(lines.contains(&"thread: unnamed".to_string()), "{lines:?}");
    assert_no_payload(tmp.path());
}

#[test]
fn a_reinstalled_hook_starts_fresh() {
    // The first-wins guard belongs to each installed hook, not to the process.
    let _serial = serial();
    let tmp = TempDir::new();
    let first = logs_dir(&tmp);
    let second = tmp.path().join("logs2");
    std::fs::create_dir(&second).unwrap_or_else(|e| panic!("create: {e}"));
    for dir in [&first, &second] {
        let clock: Arc<dyn Clock> = Arc::new(FakeClock::at(at(NOON_UTC, 0)));
        let ctx = context(dir.clone(), clock, default_frames(), default_modules());
        let seen = seen();
        let joined = with_hook(ctx, || {
            let seen = Arc::clone(&seen);
            std::thread::spawn(move || boom(&seen)).join().is_err()
        });
        assert!(joined);
    }
    assert_eq!(
        crash_files(&first).len(),
        1,
        "the first hook wrote its file"
    );
    assert_eq!(
        crash_files(&second).len(),
        1,
        "the second hook wrote its own file"
    );
}

#[test]
fn the_hook_reduces_resolved_module_paths_and_hostile_thread_names() {
    let _serial = serial();
    let tmp = TempDir::new();
    let dir = logs_dir(&tmp);
    let clock: Arc<dyn Clock> = Arc::new(FakeClock::at(at(NOON_UTC, 0)));
    let modules = Modules(vec![
        (
            VOICEN_BASE,
            "C:\\Users\\alice\\AppData\\Local\\Voicen\\voicen.exe".to_string(),
        ),
        (NTDLL_BASE, "/home/alice/.cargo/lib/libstd.so".to_string()),
        (0x3000_0000, "a".repeat(1_000) + ".dll"),
    ]);
    let frames = Frames {
        ips: vec![VOICEN_BASE + 0x10, NTDLL_BASE + 0x20, 0x3000_0040],
        claim: None,
    };
    let ctx = context(dir.clone(), clock, frames, modules);
    let seen = seen();
    let joined = with_hook(ctx, || {
        let seen = Arc::clone(&seen);
        std::thread::Builder::new()
            .name("worker\nkind: native_fault\r".to_string())
            .spawn(move || boom(&seen))
            .map(|h| h.join().is_err())
    });
    assert_eq!(joined.ok(), Some(true));
    let files = crash_files(&dir);
    assert_eq!(files.len(), 1, "{files:?}");
    let path = dir.join(files.first().map(String::as_str).unwrap_or_default());
    let text = String::from_utf8(std::fs::read(&path).unwrap_or_default())
        .unwrap_or_else(|e| panic!("crash file is not UTF-8: {e}"));
    let keys = keys_of(&text);
    assert_eq!(keys.iter().filter(|k| *k == "kind").count(), 1, "{text:?}");
    assert!(
        !text.contains("alice"),
        "a user name from a module path: {text:?}"
    );
    let frames: Vec<&str> = text.lines().filter_map(|l| l.strip_prefix("  ")).collect();
    assert_eq!(frames.len(), 3, "{text:?}");
    assert_eq!(frames.first().copied(), Some("voicen.exe+0x10"));
    assert_eq!(frames.get(1).copied(), Some("libstd.so+0x20"));
    assert_no_payload(tmp.path());
}

#[test]
fn a_frame_source_claiming_more_frames_than_the_buffer_does_not_panic() {
    // A panic in the hook would abort this binary: the count is not trusted.
    let _serial = serial();
    let tmp = TempDir::new();
    let dir = logs_dir(&tmp);
    let clock: Arc<dyn Clock> = Arc::new(FakeClock::at(at(NOON_UTC, 0)));
    let frames = Frames {
        ips: vec![VOICEN_BASE + 0x10],
        claim: Some(usize::MAX),
    };
    let ctx = context(dir.clone(), clock, frames, default_modules());
    let seen = seen();
    let joined = with_hook(ctx, || {
        let seen = Arc::clone(&seen);
        std::thread::spawn(move || boom(&seen)).join().is_err()
    });
    assert!(joined);
    let files = crash_files(&dir);
    assert_eq!(files.len(), 1, "{files:?}");
    let text = files
        .first()
        .and_then(|n| std::fs::read_to_string(dir.join(n)).ok())
        .unwrap_or_default();
    keys_of(&text);
}

#[test]
fn an_unresolved_frame_is_written_as_unknown_with_its_absolute_address() {
    // Review 1 #4 (characterization, contract "Crash files"): an address outside
    // every module (`resolve` -> None) is written `unknown+0x<absolute address>`.
    let _serial = serial();
    let tmp = TempDir::new();
    let dir = logs_dir(&tmp);
    let clock: Arc<dyn Clock> = Arc::new(FakeClock::at(at(NOON_UTC, 0)));
    let frames = Frames {
        // 0xbad0 lies below every module base of `default_modules`.
        ips: vec![VOICEN_BASE + 0x10, 0xbad0, NTDLL_BASE + 0x2bc],
        claim: None,
    };
    let ctx = context(dir.clone(), clock, frames, default_modules());
    let seen = seen();
    let joined = with_hook(ctx, || {
        let seen = Arc::clone(&seen);
        std::thread::spawn(move || boom(&seen)).join().is_err()
    });
    assert!(joined);
    let files = crash_files(&dir);
    assert_eq!(files.len(), 1, "{files:?}");
    let text = files
        .first()
        .and_then(|n| std::fs::read_to_string(dir.join(n)).ok())
        .unwrap_or_default();
    keys_of(&text);
    let frames: Vec<&str> = text.lines().filter_map(|l| l.strip_prefix("  ")).collect();
    assert_eq!(
        frames,
        vec!["voicen.exe+0x10", "unknown+0xbad0", "ntdll.dll+0x2bc"],
        "{text:?}"
    );
}

// ---- name collisions: a crash file is never replaced (review 1 #1) ----------------------

/// The previous session (as in diag_session.rs): pid 4100, started
/// 2026-10-03T10:00:00Z, written at +02:00, its marker last modified then.
const OLD_PID: u32 = 4_100;
const OLD_SECS: u64 = NOON_UTC - 26 * 3_600;
const OLD_STARTED: &str = "2026-10-03T12:00:00.000+02:00";
/// Now (NOON_UTC) as local text at +02:00.
const NOW_LOCAL: &str = "2026-10-04T14:00:00.000+02:00";
/// The name `begin` and the hook both pick at NOON_UTC for [`PID`] when it is free.
const SAME_SECOND: &str = "crash-20261004-140000-4242.txt";

fn plant_old_marker(dir: &Path) {
    let text = format!("pid={OLD_PID}\nstarted={OLD_STARTED}\nversion=0.9.0\ncommit=fedcba9\n");
    plant(
        &dir.join("session.marker"),
        text.as_bytes(),
        at(OLD_SECS, 0),
    );
}

/// `Session::begin` as this process ([`PID`]) at NOON_UTC.
fn begin_now(dir: &Path, log: &voicen_core::diag::Log) -> (Session, PreviousSession) {
    let clock = FakeClock::at(at(NOON_UTC, 0));
    Session::begin(
        SessionStart {
            logs_dir: dir,
            build: build(),
            os: os(),
            pid: PID,
            clock: &clock,
            offset: &Offset(EAST),
        },
        log,
    )
}

/// The one `previous session` line of the log in `dir`.
#[track_caller]
fn previous_line(dir: &Path) -> Line {
    let lines: Vec<Line> = all_lines(dir)
        .iter()
        .map(|l| closed(l))
        .filter(|l| l.head == "previous session")
        .collect();
    assert_eq!(lines.len(), 1, "one previous-session line: {lines:?}");
    lines.into_iter().next().unwrap_or_else(|| unreachable!())
}

/// A second writer's name: the crash-file grammar (`CrashFileName::parse` accepts
/// it) and this process's pid.
#[track_caller]
fn assert_second_name(name: &str, first: &str) {
    assert_ne!(name, first, "the second file has its own name");
    assert!(
        crash_file_name(name),
        "the second name follows crash-YYYYMMDD-HHMMSS-<pid>.txt: {name}"
    );
    assert!(
        name.ends_with(&format!("-{PID}.txt")),
        "the second name keeps this process's pid: {name}"
    );
}

#[test]
fn a_panic_in_the_second_of_begin_keeps_the_abnormal_end_file() {
    // Review 1 #1: begin writes the previous session's abnormal_end file under this
    // pid and second; a panic in the same second must not replace it.
    let _serial = serial();
    let tmp = TempDir::new();
    let dir = logs_dir(&tmp);
    plant_old_marker(&dir);
    let (log, _clock, _seen) = open_log(&dir, at(NOON_UTC, 0), EAST, LogConfig::default());
    let (session, verdict) = begin_now(&dir, &log);
    let PreviousSession::EndedAbnormally {
        crash_file: Some(abnormal),
    } = verdict
    else {
        panic!("expected EndedAbnormally with a file, got {verdict:?}");
    };
    let abnormal = abnormal.to_string();
    assert_eq!(abnormal, SAME_SECOND, "no collision yet: the plain name");
    let abnormal_bytes = std::fs::read(dir.join(&abnormal)).unwrap_or_default();

    let clock: Arc<dyn Clock> = Arc::new(FakeClock::at(at(NOON_UTC, 0)));
    let mut ctx = context(dir.clone(), clock, default_frames(), default_modules());
    ctx.session_started = session.started();
    let seen = seen();
    let joined = with_hook(ctx, || {
        let seen = Arc::clone(&seen);
        std::thread::spawn(move || boom(&seen)).join().is_err()
    });
    assert!(joined);

    let files = crash_files(&dir);
    assert_eq!(files.len(), 2, "both records survive: {files:?}");
    assert_eq!(
        std::fs::read(dir.join(&abnormal)).ok(),
        Some(abnormal_bytes),
        "the abnormal_end file is left as begin wrote it"
    );
    assert_eq!(
        lines_of(&dir.join(&abnormal)).first().map(String::as_str),
        Some("kind: abnormal_end")
    );
    let panic_name = files
        .iter()
        .find(|n| **n != abnormal)
        .cloned()
        .unwrap_or_default();
    assert_second_name(&panic_name, &abnormal);
    let text = std::fs::read_to_string(dir.join(&panic_name)).unwrap_or_default();
    keys_of(&text);
    let lines = lines_of(&dir.join(&panic_name));
    assert_eq!(lines.first().map(String::as_str), Some("kind: panic"));
    assert!(
        lines.contains(&format!("session_started: {NOW_LOCAL}")),
        "the panic file belongs to this session: {lines:?}"
    );
    let line = previous_line(&dir);
    assert_eq!(line.get("outcome"), Some("ended_abnormally"));
    assert_eq!(
        line.get("crash_file"),
        Some(abnormal.as_str()),
        "the verdict line names the abnormal_end file"
    );
    assert!(
        files_under(&dir)
            .iter()
            .all(|p| p.extension().is_none_or(|e| e != "tmp")),
        "no temp file left"
    );
    assert_no_payload(tmp.path());
}

#[test]
fn begin_in_the_second_of_a_panic_keeps_the_panic_file() {
    // Review 1 #1, the other order: the hook wrote first, begin's abnormal_end file
    // gets its own name and the verdict and its line name that one.
    let _serial = serial();
    let tmp = TempDir::new();
    let dir = logs_dir(&tmp);
    let clock: Arc<dyn Clock> = Arc::new(FakeClock::at(at(NOON_UTC, 0)));
    let ctx = context(dir.clone(), clock, default_frames(), default_modules());
    let seen = seen();
    let joined = with_hook(ctx, || {
        let seen = Arc::clone(&seen);
        std::thread::spawn(move || boom(&seen)).join().is_err()
    });
    assert!(joined);
    assert_eq!(crash_files(&dir), vec![SAME_SECOND.to_string()]);
    let panic_bytes = std::fs::read(dir.join(SAME_SECOND)).unwrap_or_default();

    plant_old_marker(&dir);
    let (log, _clock, _seen) = open_log(&dir, at(NOON_UTC, 0), EAST, LogConfig::default());
    let (_session, verdict) = begin_now(&dir, &log);
    let PreviousSession::EndedAbnormally {
        crash_file: Some(abnormal),
    } = verdict
    else {
        panic!("expected EndedAbnormally with a file, got {verdict:?}");
    };
    let abnormal = abnormal.to_string();
    assert_second_name(&abnormal, SAME_SECOND);
    let files = crash_files(&dir);
    assert_eq!(files.len(), 2, "both records survive: {files:?}");
    assert!(
        files.contains(&abnormal),
        "the verdict names a file that exists: {files:?}"
    );
    assert_eq!(
        std::fs::read(dir.join(SAME_SECOND)).ok(),
        Some(panic_bytes),
        "the panic file is left as the hook wrote it"
    );
    let lines = lines_of(&dir.join(&abnormal));
    assert_eq!(
        lines.first().map(String::as_str),
        Some("kind: abnormal_end")
    );
    assert!(
        lines.contains(&format!("session_started: {OLD_STARTED}")),
        "{lines:?}"
    );
    assert!(
        lines.contains(&format!("previous_session_pid: {OLD_PID}")),
        "{lines:?}"
    );
    let line = previous_line(&dir);
    assert_eq!(line.get("outcome"), Some("ended_abnormally"));
    assert_eq!(line.get("crash_file"), Some(abnormal.as_str()));
    assert_no_payload(tmp.path());
}

// ---- retain_crash_files -----------------------------------------------------------------

/// A crash file named and last modified `age` before `now`; returns its name.
fn plant_aged(dir: &Path, now: SystemTime, age: Duration, pid: u32) -> String {
    let t = now - age;
    let name = format!("crash-{}-{pid}.txt", voicen_core::clock::utc_compact(t));
    plant(&dir.join(&name), b"kind: abnormal_end\n", t);
    name
}

fn plant(path: &Path, bytes: &[u8], t: SystemTime) {
    std::fs::write(path, bytes).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(t))
        .unwrap_or_else(|e| panic!("set_modified {}: {e}", path.display()));
}

#[test]
fn crash_retention_limits_are_30_days_and_20_files() {
    let _serial = serial();
    assert_eq!(CRASH_MAX_AGE_DAYS, 30);
    assert_eq!(CRASH_MAX_FILES, 20);
}

#[test]
fn retention_deletes_files_older_than_30_days_then_keeps_the_20_newest() {
    let _serial = serial();
    let tmp = TempDir::new();
    let dir = logs_dir(&tmp);
    let now = at(NOON_UTC, 0);
    // 25 crash files: 3 older than 30 days, 22 within (1..=20 days, 28 and 29 days).
    let mut keep = Vec::new();
    let mut drop_ = Vec::new();
    let mut pid = 100;
    for days in 1..=20 {
        pid += 1;
        keep.push(plant_aged(&dir, now, Duration::from_secs(days * DAY), pid));
    }
    for days in [28, 29, 31, 45, 400] {
        pid += 1;
        drop_.push(plant_aged(&dir, now, Duration::from_secs(days * DAY), pid));
    }
    assert_eq!(crash_files(&dir).len(), 25);
    // Not crash files: never touched, however old.
    let old = now - Duration::from_secs(400 * DAY);
    let foreign = [
        "voicen.log",
        "voicen-20250901-000000.log",
        "session.marker",
        "installer-ended",
        "crash-20250901-000000-1.tmp",
        "notcrash-20250901-000000-1.txt",
        "crash-notes.md",
    ];
    for name in foreign {
        plant(&dir.join(name), b"foreign\n", old);
    }
    retain_crash_files(&dir, now);
    keep.sort();
    assert_eq!(
        crash_files(&dir),
        keep,
        "files older than 30 days, then all but the 20 newest, go"
    );
    let twenty_days = keep.first().cloned().unwrap_or_default();
    assert!(
        dir.join(&twenty_days).exists(),
        "a 20-day-old crash file survives: {twenty_days}"
    );
    for name in &drop_ {
        assert!(!dir.join(name).exists(), "{name} should be removed");
    }
    for name in foreign {
        assert_eq!(
            std::fs::read(dir.join(name)).ok(),
            Some(b"foreign\n".to_vec()),
            "{name} is not a crash file and is untouched"
        );
    }
}

#[test]
fn retention_deletes_files_older_than_30_days_under_the_count_cap() {
    // Fewer than 20 files: only the age rule can remove one (bites a count-only
    // retention, which the 25-file test above does not tell apart).
    let _serial = serial();
    let tmp = TempDir::new();
    let dir = logs_dir(&tmp);
    let now = at(NOON_UTC, 0);
    let mut keep: Vec<String> = [1, 20, 29]
        .into_iter()
        .map(|d| plant_aged(&dir, now, Duration::from_secs(d * DAY), 300 + d as u32))
        .collect();
    for d in [31, 400] {
        plant_aged(&dir, now, Duration::from_secs(d * DAY), 300 + d as u32);
    }
    retain_crash_files(&dir, now);
    keep.sort();
    assert_eq!(
        crash_files(&dir),
        keep,
        "files older than 30 days go, younger ones stay"
    );
}

#[test]
fn the_21st_crash_file_removes_the_oldest() {
    let _serial = serial();
    let tmp = TempDir::new();
    let dir = logs_dir(&tmp);
    let now = at(NOON_UTC, 0);
    let names: Vec<String> = (1..=21u32)
        .map(|h| {
            plant_aged(
                &dir,
                now,
                Duration::from_secs(u64::from(h) * 3_600),
                200 + h,
            )
        })
        .collect();
    retain_crash_files(&dir, now);
    let oldest = names.last().cloned().unwrap_or_default();
    let mut expected: Vec<String> = names.iter().filter(|n| **n != oldest).cloned().collect();
    expected.sort();
    assert_eq!(
        crash_files(&dir),
        expected,
        "only the oldest ({oldest}) goes"
    );
}

#[test]
fn retention_of_a_missing_folder_does_nothing() {
    let _serial = serial();
    let tmp = TempDir::new();
    let dir = tmp.path().join("absent");
    retain_crash_files(&dir, at(NOON_UTC, 0));
    assert!(!dir.exists(), "retention creates nothing");
    assert!(files_under(tmp.path()).is_empty());
}

#[test]
fn rendered_times_follow_the_timestamp_grammar() {
    // The `time` and `session_started` values are diag's own timestamp text.
    let _serial = serial();
    let text = render_crash_file(&fields(CrashDetail::AbnormalEnd {
        previous_session_pid: None,
    }));
    for key in ["time", "session_started"] {
        let value = text
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{key}: ")).map(str::to_string))
            .unwrap_or_else(|| panic!("no {key} in {text:?}"));
        parse_ts(&value).unwrap_or_else(|e| panic!("{key}: {e}"));
    }
    assert!(!text.contains("previous_session_pid"), "{text:?}");
}
