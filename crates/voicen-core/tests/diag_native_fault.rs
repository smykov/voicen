//! T-024 step 1 (spec 006 T026, FR-011, FR-014; T-060 T-100 r2#3): the native-fault
//! writer, the second crash-file writer of diag. The shell's unhandled-exception
//! filter runs it on a faulting thread, where the heap may be locked or corrupt, so
//! everything it needs is prepared at install time and its record path must not
//! allocate.
//!
//! Its own test binary: a counting `#[global_allocator]` ([`Counting`]) counts the
//! allocations of the current thread while armed ([`allocations_in`]). The panic
//! hook (the reference writer, `write_crash_file`) is process-global, so every test
//! takes [`serial`] first.
//!
//! The API these tests pin (the developer builds it; `contracts/core-diag.md` "Crash
//! files" names it as the second writer):
//!
//! ```text
//! voicen_core::diag::{NativeFaultSetup, NativeFaultWriter, FaultFiles}
//! pub struct NativeFaultSetup {             // pub fields; what the filter needs, fixed at install
//!     build: BuildInfo, os: OsVersion,
//!     session_started: SystemTime,          // Session::started()
//!     pid: u32,
//!     seconds_east: i32,                    // the offset of every time of the file, fixed at install
//! }
//! pub trait FaultFiles {                    // the shell: CreateFileW CREATE_NEW + WriteFile,
//!                                           // CreateHardLinkW, DeleteFileW; names are bare file
//!                                           // names in the logs dir
//!     fn create_new(&mut self, tmp: &str, bytes: &[u8]) -> io::Result<()>;   // AlreadyExists: not ours
//!     fn hard_link(&mut self, tmp: &str, name: &str) -> io::Result<()>;      // AlreadyExists: taken
//!     fn remove(&mut self, tmp: &str);
//! }
//! pub struct NativeFaultWriter;             // Send + Sync (the shell keeps it in a static)
//! NativeFaultWriter::new(NativeFaultSetup) -> NativeFaultWriter      // may allocate
//! NativeFaultWriter::record(&self, now: SystemTime, exception_code: u32, fault_address: usize,
//!     module: Option<(&[u16], usize)>,      // (module path as GetModuleFileNameW gives it, offset)
//!     files: &mut dyn FaultFiles) -> Option<CrashFileName>          // never allocates
//! ```
//!
//! Its guarantees, each pinned below:
//! - the bytes it writes equal `render_crash_file` of the same `CrashFields`
//!   (`kind: native_fault`, `time` = `now` and `session_started` both at the fixed
//!   offset, `fault_address` = `Frame::new(<lossy UTF-8 of the module path>, offset)`
//!   or, unresolved, `Frame::new("", fault_address)`, as the panic hook writes an
//!   unresolved frame);
//! - `record` makes zero allocations;
//! - it writes `<first name's stem>-native_fault.tmp` (created new), hard-links it to
//!   the name of `now`, or while that name is taken to the next second's, at most 60
//!   names, the pid kept: the same names `write_crash_file` picks; then removes the
//!   tmp. A taken name is never replaced;
//! - one writer records at most once (first wins); a failure ends the record with no
//!   file left.

mod diag_support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use diag_support::{at, crash_file_name, Offset, NOON_UTC};
use voicen_core::build_info::BuildInfo;
use voicen_core::clock::{Clock, FakeClock};
use voicen_core::diag::{
    install_panic_hook, render_crash_file, CrashContext, CrashDetail, CrashFields, FaultFiles,
    Frame, FrameSource, LocalStamp, ModuleResolver, NativeFaultSetup, NativeFaultWriter, OsVersion,
};
use voicen_core::test_support::TempDir;

// ---- the counting allocator -----------------------------------------------------------------

/// `System`, counting the allocations (alloc, alloc_zeroed, realloc) of a thread
/// while that thread is armed. Thread-local, so parallel test threads do not count.
struct Counting;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
}

fn note_allocation() {
    let _ = ARMED.try_with(|armed| {
        if armed.get() {
            let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        }
    });
}

// SAFETY: every call is forwarded unchanged to `System`; the counter only reads and
// writes two const-initialised thread-locals without destructors (no allocation).
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        // SAFETY: the caller's contract is passed on.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        // SAFETY: the caller's contract is passed on.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_allocation();
        // SAFETY: the caller's contract is passed on.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract is passed on.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// `body`'s result and the allocations this thread made while it ran.
fn allocations_in<R>(body: impl FnOnce() -> R) -> (R, usize) {
    COUNT.with(|c| c.set(0));
    ARMED.with(|a| a.set(true));
    let out = body();
    ARMED.with(|a| a.set(false));
    (out, COUNT.with(Cell::get))
}

// ---- serialisation --------------------------------------------------------------------------

static SERIAL: Mutex<()> = Mutex::new(());

/// Every test of this binary runs under this lock: the reference writer is the
/// process-global panic hook.
fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- fixtures -------------------------------------------------------------------------------

const PID: u32 = 4_242;
const EAST: i32 = 7_200;
const ACCESS_VIOLATION: u32 = 0xC000_0005;
const MAX_NAME_TRIES: usize = 60;

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

fn setup(build: BuildInfo, session_started: SystemTime, seconds_east: i32) -> NativeFaultSetup {
    NativeFaultSetup {
        build,
        os: os(),
        session_started,
        pid: PID,
        seconds_east,
    }
}

fn writer() -> NativeFaultWriter {
    NativeFaultWriter::new(setup(build(), at(NOON_UTC - 3_600, 0), EAST))
}

/// The reference text: `render_crash_file` of the fields the writer was given.
fn expected(
    build: BuildInfo,
    session_started: SystemTime,
    seconds_east: i32,
    now: SystemTime,
    code: u32,
    fault_address: usize,
    module: Option<(&[u16], usize)>,
) -> String {
    let frame = match module {
        Some((path, offset)) => Frame::new(&String::from_utf16_lossy(path), offset),
        None => Frame::new("", fault_address),
    };
    render_crash_file(&CrashFields {
        time: LocalStamp {
            at: now,
            seconds_east,
        },
        build,
        os: os(),
        session_started: Some(LocalStamp {
            at: session_started,
            seconds_east,
        }),
        detail: CrashDetail::NativeFault {
            exception_code: code,
            fault_address: Some(frame),
        },
    })
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// One call the writer made on its [`FaultFiles`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Create { tmp: String, bytes: Vec<u8> },
    Link { tmp: String, name: String },
    Remove { tmp: String },
}

/// Records every call; answers `create_new` with `create` and each `hard_link` with
/// the next of `links` (`Ok` once they run out).
struct Recording {
    calls: Vec<Call>,
    create: Option<ErrorKind>,
    links: Vec<Option<ErrorKind>>,
}

impl Recording {
    fn ok() -> Recording {
        Recording {
            calls: Vec::new(),
            create: None,
            links: Vec::new(),
        }
    }

    fn bytes(&self) -> Option<&[u8]> {
        self.calls.iter().find_map(|c| match c {
            Call::Create { bytes, .. } => Some(bytes.as_slice()),
            _ => None,
        })
    }

    fn linked(&self) -> Vec<String> {
        self.calls
            .iter()
            .filter_map(|c| match c {
                Call::Link { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect()
    }
}

impl FaultFiles for Recording {
    fn create_new(&mut self, tmp: &str, bytes: &[u8]) -> io::Result<()> {
        self.calls.push(Call::Create {
            tmp: tmp.to_string(),
            bytes: bytes.to_vec(),
        });
        match self.create {
            Some(kind) => Err(io::Error::from(kind)),
            None => Ok(()),
        }
    }

    fn hard_link(&mut self, tmp: &str, name: &str) -> io::Result<()> {
        self.calls.push(Call::Link {
            tmp: tmp.to_string(),
            name: name.to_string(),
        });
        let next = if self.links.is_empty() {
            None
        } else {
            self.links.remove(0)
        };
        match next {
            Some(kind) => Err(io::Error::from(kind)),
            None => Ok(()),
        }
    }

    fn remove(&mut self, tmp: &str) {
        self.calls.push(Call::Remove {
            tmp: tmp.to_string(),
        });
    }
}

/// A [`FaultFiles`] that never allocates: fixed buffers for the bytes and the
/// names, so the zero-allocation count measures the writer only.
struct Fixed {
    bytes: [u8; 8192],
    len: usize,
    creates: usize,
    links: usize,
    removes: usize,
    name: [u8; 64],
    name_len: usize,
    /// How many `hard_link` calls answer `AlreadyExists` first.
    taken: usize,
}

impl Fixed {
    fn new(taken: usize) -> Fixed {
        Fixed {
            bytes: [0; 8192],
            len: 0,
            creates: 0,
            links: 0,
            removes: 0,
            name: [0; 64],
            name_len: 0,
            taken,
        }
    }
}

impl FaultFiles for Fixed {
    fn create_new(&mut self, _tmp: &str, bytes: &[u8]) -> io::Result<()> {
        self.creates += 1;
        let n = bytes.len().min(self.bytes.len());
        if let (Some(dst), Some(src)) = (self.bytes.get_mut(..n), bytes.get(..n)) {
            dst.copy_from_slice(src);
        }
        self.len = n;
        Ok(())
    }

    fn hard_link(&mut self, _tmp: &str, name: &str) -> io::Result<()> {
        self.links += 1;
        if self.links <= self.taken {
            return Err(io::Error::from(ErrorKind::AlreadyExists));
        }
        let n = name.len().min(self.name.len());
        if let (Some(dst), Some(src)) = (self.name.get_mut(..n), name.as_bytes().get(..n)) {
            dst.copy_from_slice(src);
        }
        self.name_len = n;
        Ok(())
    }

    fn remove(&mut self, _tmp: &str) {
        self.removes += 1;
    }
}

/// The real file system under `dir`, as the shell's Win32 calls behave: create new,
/// a hard link that fails on an existing name, remove.
struct Disk(PathBuf);

impl FaultFiles for Disk {
    fn create_new(&mut self, tmp: &str, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write as _;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.0.join(tmp))?
            .write_all(bytes)
    }

    fn hard_link(&mut self, tmp: &str, name: &str) -> io::Result<()> {
        fs::hard_link(self.0.join(tmp), self.0.join(name))
    }

    fn remove(&mut self, tmp: &str) {
        let _ = fs::remove_file(self.0.join(tmp));
    }
}

/// Every file name in `dir`, sorted.
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn logs_dir(tmp: &TempDir, name: &str) -> PathBuf {
    let dir = tmp.path().join(name);
    fs::create_dir(&dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
    dir
}

// ---- the reference writer: write_crash_file through the panic hook -------------------------

struct NoFrames;

impl FrameSource for NoFrames {
    fn capture(&self, _out: &mut [usize]) -> usize {
        0
    }
}

struct NoModules;

impl ModuleResolver for NoModules {
    fn resolve(&self, _ip: usize) -> Option<(String, usize)> {
        None
    }
}

/// Runs `write_crash_file` once (a fresh panic hook over `dir`, the clock at `now`,
/// [`PID`], offset `seconds_east`, one panicking thread) and returns the name it
/// added, if any. The previous hook is back before this returns.
fn reference_write(dir: &Path, now: SystemTime, seconds_east: i32) -> Option<String> {
    let before = names_in(dir);
    let clock: Arc<dyn Clock> = Arc::new(FakeClock::at(now));
    let previous = std::panic::take_hook();
    install_panic_hook(CrashContext {
        logs_dir: dir.to_path_buf(),
        build: build(),
        os: os(),
        session_started: at(NOON_UTC - 3_600, 0),
        pid: PID,
        clock,
        offset: Arc::new(Offset(seconds_east)),
        frames: Arc::new(NoFrames),
        modules: Arc::new(NoModules),
    });
    let joined = std::thread::spawn(|| panic!("reference write")).join();
    let _ours = std::panic::take_hook();
    std::panic::set_hook(previous);
    assert!(joined.is_err(), "premise: the reference thread panicked");
    let mut added: Vec<String> = names_in(dir)
        .into_iter()
        .filter(|n| !before.contains(n))
        .collect();
    assert!(added.len() <= 1, "premise: one reference file: {added:?}");
    added.pop()
}

// ---- a small deterministic generator for the property tests -------------------------------

/// xorshift64*: the same sequence on every run and every OS.
struct Gen(u64);

impl Gen {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn pick<T: Copy>(&mut self, from: &[T]) -> T {
        let i = usize::try_from(self.below(from.len() as u64)).unwrap_or(0);
        from.get(i).copied().unwrap_or_else(|| unreachable!())
    }
}

/// UTF-16 units a module path from `GetModuleFileNameW` can hold, hostile ones
/// included: both separators, ASCII, Cyrillic and CJK letters, astral characters
/// (surrogate pairs), lone surrogates, control characters, Unicode white space
/// (`str::trim` removes it), and a dot.
fn path_units(g: &mut Gen) -> Vec<u16> {
    const PIECES: &[&str] = &[
        "\\",
        "/",
        "C:",
        "Users",
        "Example User",
        "AppData",
        "Local",
        "Voicen",
        "voicen.exe",
        "ntdll.dll",
        "пользователь",
        "модуль.dll",
        "用户",
        "模块.dll",
        "\u{1F600}",
        "\u{10FFFF}",
        " ",
        "\u{3000}",
        "\u{a0}",
        "\u{2009}",
        "\t",
        "\u{7f}",
        "\u{9f}",
        "\u{1b}",
        ".",
        "a",
        "Z",
        "0123456789",
    ];
    let mut out = Vec::new();
    let pieces = g.below(24);
    for _ in 0..pieces {
        match g.below(10) {
            // A lone high or low surrogate (not valid UTF-16 text).
            0 => out.push(if g.below(2) == 0 { 0xD83D } else { 0xDE00 }),
            // A long run: more than 255 bytes in one base name, so it is cut.
            1 => {
                let unit = g.pick(&[u16::from(b'x'), 0x0436, 0x4E2D]);
                let n = 60 + g.below(120);
                out.extend(std::iter::repeat_n(unit, n as usize));
            }
            _ => out.extend(g.pick(PIECES).encode_utf16()),
        }
    }
    out
}

// ---- tests ----------------------------------------------------------------------------------

#[test]
fn the_counter_sees_an_allocation() {
    // Guard of the zero-allocation tests: the counting allocator does count this
    // thread's allocations while armed, so a zero below is a real zero.
    let _serial = serial();
    let (v, n) = allocations_in(|| std::hint::black_box(Vec::<u8>::with_capacity(64)));
    drop(v);
    assert!(
        n >= 1,
        "the counting allocator saw {n} allocations of a Vec"
    );
    let (s, n) = allocations_in(|| std::hint::black_box(format!("{}", 12_345)));
    drop(s);
    assert!(
        n >= 1,
        "the counting allocator saw {n} allocations of a format!"
    );
}

#[test]
fn the_writer_is_send_and_sync_for_a_static() {
    // The shell's filter reaches it through a static (no state on the faulting
    // thread's stack, which may be nearly exhausted).
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    let _serial = serial();
    assert_send_sync::<NativeFaultWriter>();
}

#[test]
fn a_resolved_fault_writes_render_crash_file_bytes() {
    let _serial = serial();
    let module = wide("C:\\Users\\Example User\\AppData\\Local\\Voicen\\voicen.exe");
    let now = at(NOON_UTC, 123);
    let mut files = Recording::ok();
    let name = writer().record(
        now,
        ACCESS_VIOLATION,
        0x7ff6_1234_5678,
        Some((&module, 0x1a2b)),
        &mut files,
    );
    let want = "kind: native_fault\n\
                time: 2026-10-04T14:00:00.123+02:00\n\
                version: 0.9.1\n\
                commit: abc1234\n\
                os: 10.0.22631\n\
                session_started: 2026-10-04T13:00:00.000+02:00\n\
                exception_code: 0xC0000005\n\
                fault_address: voicen.exe+0x1a2b\n";
    assert_eq!(
        want,
        expected(
            build(),
            at(NOON_UTC - 3_600, 0),
            EAST,
            now,
            ACCESS_VIOLATION,
            0,
            Some((&module, 0x1a2b))
        ),
        "premise: the reference rendering"
    );
    assert_eq!(
        files.bytes().map(String::from_utf8_lossy).as_deref(),
        Some(want),
        "the bytes written: {:?}",
        files.calls
    );
    assert_eq!(
        name.map(|n| n.to_string()).as_deref(),
        Some(format!("crash-20261004-140000-{PID}.txt").as_str()),
        "the name of `now` at the fixed offset, this pid"
    );
}

#[test]
fn an_unresolved_fault_is_written_as_unknown_with_its_absolute_address() {
    // As the panic hook writes a frame outside every module.
    let _serial = serial();
    let mut files = Recording::ok();
    let name = writer().record(at(NOON_UTC, 0), ACCESS_VIOLATION, 0xbad0, None, &mut files);
    assert!(name.is_some(), "recorded: {:?}", files.calls);
    let text = files
        .bytes()
        .map(String::from_utf8_lossy)
        .unwrap_or_default();
    assert!(
        text.ends_with("exception_code: 0xC0000005\nfault_address: unknown+0xbad0\n"),
        "{text:?}"
    );
    assert_eq!(
        text,
        expected(
            build(),
            at(NOON_UTC - 3_600, 0),
            EAST,
            at(NOON_UTC, 0),
            ACCESS_VIOLATION,
            0xbad0,
            None
        )
    );
}

#[test]
fn the_bytes_equal_render_crash_file_for_any_input() {
    // The invariant (analysis (3)): for any module path the OS can hand over (any
    // UTF-16, hostile units included), any code, address, offset, time, offset east
    // and build, the writer's bytes are render_crash_file's for the same fields, so
    // the filter's text passes through diag's validating constructors as the
    // panic hook's does. A fresh writer per case (one record each).
    let _serial = serial();
    let builds = [
        build(),
        BuildInfo {
            version: "1.2.3-dev+build.7",
            commit: "unknown",
        },
        BuildInfo {
            version: "0.9.1\nkind: panic",
            commit: "abc 1234\r",
        },
    ];
    let mut g = Gen(0x9E37_79B9_7F4A_7C15);
    for case in 0..3_000 {
        let build = builds
            .get(usize::try_from(g.below(builds.len() as u64)).unwrap_or(0))
            .cloned()
            .unwrap_or_else(build);
        let seconds_east = i32::try_from(g.below(1_681)).unwrap_or(0) * 60 - 50_400;
        let started_secs = NOON_UTC - g.below(400 * 86_400);
        let session_started = UNIX_EPOCH
            + Duration::from_secs(started_secs)
            + Duration::from_nanos(g.below(1_000_000_000));
        let now = session_started
            + Duration::from_secs(g.below(90 * 86_400))
            + Duration::from_nanos(g.below(1_000_000_000));
        let code = u32::try_from(g.next() >> 32).unwrap_or(0);
        let fault_address = usize::try_from(g.next() >> 16).unwrap_or(0);
        let path = path_units(&mut g);
        let offset = usize::try_from(g.next() >> 40).unwrap_or(0);
        let module = (g.below(5) != 0).then_some((path.as_slice(), offset));

        let w = NativeFaultWriter::new(setup(build.clone(), session_started, seconds_east));
        let mut files = Recording::ok();
        let name = w.record(now, code, fault_address, module, &mut files);
        let want = expected(
            build,
            session_started,
            seconds_east,
            now,
            code,
            fault_address,
            module,
        );
        assert!(
            name.is_some(),
            "case {case}: not recorded: {:?}",
            files.calls
        );
        assert_eq!(
            files.bytes(),
            Some(want.as_bytes()),
            "case {case}: path units {path:x?}, code {code:#x}, address {fault_address:#x}, \
             offset {offset:#x}, east {seconds_east}"
        );
    }
}

#[test]
fn record_makes_zero_allocations() {
    // Spec T026: no allocation in the filter (a fault with the heap lock held would
    // hang, a corrupt heap would re-fault). Measured on a resolved fault with a long
    // non-ASCII path and a lone surrogate (the lossy decoding), with three taken
    // names (the stepping), and on an unresolved one.
    let _serial = serial();
    let mut path = wide("C:\\Users\\Пользователь Example\\AppData\\Local\\Voicen\\");
    path.push(0xD83D);
    path.extend(std::iter::repeat_n(0x0436_u16, 200));
    path.extend(wide("\u{1F600}.exe"));
    let resolved = writer();
    let mut files = Fixed::new(3);
    let (name, n) = allocations_in(|| {
        resolved.record(
            at(NOON_UTC, 999),
            ACCESS_VIOLATION,
            0x7ff6_0000_1000,
            Some((&path, 0xdead_beef)),
            &mut files,
        )
    });
    assert!(name.is_some(), "premise: recorded");
    assert_eq!(
        (files.creates, files.links, files.removes),
        (1, 4, 1),
        "premise: one tmp, three taken names then the fourth, the tmp removed"
    );
    assert_eq!(
        files.bytes.get(..files.len),
        Some(
            expected(
                build(),
                at(NOON_UTC - 3_600, 0),
                EAST,
                at(NOON_UTC, 999),
                ACCESS_VIOLATION,
                0,
                Some((&path, 0xdead_beef))
            )
            .as_bytes()
        ),
        "premise: the bytes are the reference text"
    );
    assert_eq!(
        n, 0,
        "record allocated {n} times (resolved fault, 3 taken names)"
    );

    let unresolved = writer();
    let mut files = Fixed::new(0);
    let (name, n) =
        allocations_in(|| unresolved.record(at(NOON_UTC, 0), 0xC000_00FD, 0x10, None, &mut files));
    assert!(name.is_some(), "premise: recorded");
    assert_eq!(n, 0, "record allocated {n} times (unresolved fault)");
}

#[test]
fn a_second_record_of_one_writer_writes_nothing() {
    // First wins (the filter can be entered by two faulting threads; the writer's
    // buffers are shared): the second record touches no file.
    let _serial = serial();
    let w = writer();
    let mut first = Recording::ok();
    assert!(w
        .record(at(NOON_UTC, 0), ACCESS_VIOLATION, 0x10, None, &mut first)
        .is_some());
    let mut second = Recording::ok();
    let again = w.record(at(NOON_UTC, 500), ACCESS_VIOLATION, 0x20, None, &mut second);
    assert_eq!(again, None, "a second record of the same writer");
    assert!(
        second.calls.is_empty(),
        "the second record made calls: {:?}",
        second.calls
    );
}

#[test]
fn the_tmp_is_the_first_names_stem_and_is_removed_after_the_link() {
    // `<stem>-native_fault.tmp`, as write_crash_file names its tmp: a crash-*.tmp,
    // which Session::begin deletes when a kill cut the write short.
    let _serial = serial();
    let mut files = Recording::ok();
    let name = writer().record(at(NOON_UTC, 0), ACCESS_VIOLATION, 0x10, None, &mut files);
    let stem = format!("crash-20261004-140000-{PID}");
    let tmp = format!("{stem}-native_fault.tmp");
    let bytes = files.bytes().map(<[u8]>::to_vec).unwrap_or_default();
    assert_eq!(
        files.calls,
        vec![
            Call::Create {
                tmp: tmp.clone(),
                bytes
            },
            Call::Link {
                tmp: tmp.clone(),
                name: format!("{stem}.txt")
            },
            Call::Remove { tmp },
        ]
    );
    assert_eq!(name.map(|n| n.to_string()), Some(format!("{stem}.txt")));
}

#[test]
fn the_name_sequence_equals_write_crash_files() {
    // Same instant, offset and pid: the k-th native record into a dir holding the
    // reference writer's first k files gets the reference writer's k-th name. The
    // instant is local 23:59:58.999 on 31 Dec at -03:30, so the stepping crosses
    // a second, a minute, an hour, a day and a year.
    let _serial = serial();
    let east = -(3 * 3_600 + 30 * 60);
    // 2027-01-01T03:29:58.999Z = 2026-12-31T23:59:58.999-03:30.
    let now = UNIX_EPOCH + Duration::from_millis(1_798_774_198_999);
    let tmp = TempDir::new();
    let reference = logs_dir(&tmp, "reference");
    let mut names = Vec::new();
    for _ in 0..4 {
        names.push(
            reference_write(&reference, now, east).unwrap_or_else(|| {
                panic!("premise: the reference writer wrote a file ({names:?})")
            }),
        );
    }
    assert_eq!(
        names.first().map(String::as_str),
        Some(format!("crash-20261231-235958-{PID}.txt").as_str()),
        "premise: the reference's first name"
    );
    let native = logs_dir(&tmp, "native");
    for (k, want) in names.iter().enumerate() {
        let w = NativeFaultWriter::new(setup(build(), at(NOON_UTC - 3_600, 0), east));
        let got = w.record(now, ACCESS_VIOLATION, 0x10, None, &mut Disk(native.clone()));
        assert_eq!(
            got.map(|n| n.to_string()).as_ref(),
            Some(want),
            "record {k}: the reference writer's names are {names:?}"
        );
    }
    assert_eq!(names_in(&native), {
        let mut sorted = names.clone();
        sorted.sort();
        sorted
    });
    assert!(names.iter().all(|n| crash_file_name(n)), "{names:?}");
}

#[test]
fn a_planted_taken_name_is_skipped_and_keeps_its_bytes() {
    // The T-100 rule: an existing crash file is never replaced. Two planted files
    // under the first two names keep their bytes; the record takes the third name;
    // no tmp is left.
    let _serial = serial();
    let tmp = TempDir::new();
    let dir = logs_dir(&tmp, "logs");
    let first = format!("crash-20261004-140000-{PID}.txt");
    let second = format!("crash-20261004-140001-{PID}.txt");
    fs::write(dir.join(&first), b"planted one\n").unwrap_or_else(|e| panic!("plant: {e}"));
    fs::write(dir.join(&second), b"planted two\n").unwrap_or_else(|e| panic!("plant: {e}"));
    let got = writer().record(
        at(NOON_UTC, 400),
        ACCESS_VIOLATION,
        0x10,
        None,
        &mut Disk(dir.clone()),
    );
    let third = format!("crash-20261004-140002-{PID}.txt");
    assert_eq!(got.map(|n| n.to_string()), Some(third.clone()));
    assert_eq!(
        fs::read(dir.join(&first)).ok().as_deref(),
        Some(&b"planted one\n"[..])
    );
    assert_eq!(
        fs::read(dir.join(&second)).ok().as_deref(),
        Some(&b"planted two\n"[..])
    );
    let text = fs::read_to_string(dir.join(&third)).unwrap_or_default();
    assert!(
        text.starts_with("kind: native_fault\ntime: 2026-10-04T14:00:00.400+02:00\n"),
        "the time line is the true time, not the stepped name's: {text:?}"
    );
    assert_eq!(names_in(&dir), vec![first, second, third], "no tmp left");
}

#[test]
fn sixty_taken_names_write_nothing_and_leave_no_tmp() {
    // At most 60 names, as write_crash_file: with all of them taken (by the
    // reference writer itself) both give up, and the native record leaves nothing.
    let _serial = serial();
    let tmp = TempDir::new();
    let dir = logs_dir(&tmp, "logs");
    let now = at(NOON_UTC, 0);
    for k in 0..MAX_NAME_TRIES {
        assert!(
            reference_write(&dir, now, EAST).is_some(),
            "premise: reference write {k} found a free name"
        );
    }
    assert_eq!(
        reference_write(&dir, now, EAST),
        None,
        "premise: the reference writer gives up after {MAX_NAME_TRIES} names"
    );
    let before: Vec<(String, Vec<u8>)> = names_in(&dir)
        .into_iter()
        .map(|n| {
            let bytes = fs::read(dir.join(&n)).unwrap_or_default();
            (n, bytes)
        })
        .collect();
    assert_eq!(before.len(), MAX_NAME_TRIES);
    let mut files = Recording::ok();
    files.links = vec![Some(ErrorKind::AlreadyExists); MAX_NAME_TRIES + 5];
    let got = writer().record(now, ACCESS_VIOLATION, 0x10, None, &mut files);
    assert_eq!(got, None);
    let linked = files.linked();
    let expected_names: Vec<String> = before.iter().map(|(n, _)| n.clone()).collect();
    let mut sorted = linked.clone();
    sorted.sort();
    assert_eq!(
        sorted, expected_names,
        "the native writer tried exactly the reference writer's {MAX_NAME_TRIES} names"
    );
    assert!(
        matches!(files.calls.last(), Some(Call::Remove { .. })),
        "the tmp is removed last: {:?}",
        files.calls
    );
    let got = writer().record(now, ACCESS_VIOLATION, 0x10, None, &mut Disk(dir.clone()));
    assert_eq!(got, None, "on disk too");
    let after: Vec<(String, Vec<u8>)> = names_in(&dir)
        .into_iter()
        .map(|n| {
            let bytes = fs::read(dir.join(&n)).unwrap_or_default();
            (n, bytes)
        })
        .collect();
    assert_eq!(after, before, "no file added, replaced or left behind");
}

#[test]
fn a_tmp_that_exists_is_left_to_its_writer() {
    // create_new answers AlreadyExists: another writer's tmp. No link, no remove.
    let _serial = serial();
    let mut files = Recording::ok();
    files.create = Some(ErrorKind::AlreadyExists);
    let got = writer().record(at(NOON_UTC, 0), ACCESS_VIOLATION, 0x10, None, &mut files);
    assert_eq!(got, None);
    assert_eq!(files.calls.len(), 1, "only the create: {:?}", files.calls);
}

#[test]
fn a_failed_tmp_write_is_removed_and_nothing_is_linked() {
    // create_new fails otherwise (disk full mid-write): the partial tmp goes, no name.
    let _serial = serial();
    let mut files = Recording::ok();
    files.create = Some(ErrorKind::PermissionDenied);
    let got = writer().record(at(NOON_UTC, 0), ACCESS_VIOLATION, 0x10, None, &mut files);
    assert_eq!(got, None);
    assert!(files.linked().is_empty(), "{:?}", files.calls);
    assert!(
        matches!(files.calls.last(), Some(Call::Remove { tmp }) if tmp.ends_with("-native_fault.tmp")),
        "the tmp is removed: {:?}",
        files.calls
    );
}

#[test]
fn a_link_error_other_than_taken_ends_the_stepping() {
    // A link that fails for another reason (folder gone, access denied) is not
    // retried under later names: one attempt, the tmp removed, no name.
    let _serial = serial();
    let mut files = Recording::ok();
    files.links = vec![
        Some(ErrorKind::AlreadyExists),
        Some(ErrorKind::PermissionDenied),
    ];
    let got = writer().record(at(NOON_UTC, 0), ACCESS_VIOLATION, 0x10, None, &mut files);
    assert_eq!(got, None);
    assert_eq!(
        files.linked(),
        vec![
            format!("crash-20261004-140000-{PID}.txt"),
            format!("crash-20261004-140001-{PID}.txt"),
        ],
        "{:?}",
        files.calls
    );
    assert!(
        matches!(files.calls.last(), Some(Call::Remove { .. })),
        "{:?}",
        files.calls
    );
}
