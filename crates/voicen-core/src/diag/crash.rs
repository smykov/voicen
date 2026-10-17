//! Crash files (T-100; spec 006 FR-011, FR-014..FR-016, research R5, data-model
//! "CrashFile"): `logs/crash-YYYYMMDD-HHMMSS-<pid>.txt`, plain `key: value` lines in
//! data-model order, rendered by [`render_crash_file`] from closed typed fields only.
//!
//! Invariants (T-100 analysis (3), (4)):
//! - every byte of a crash file comes from diag: integers, diag literals,
//!   [`BuildInfo`] (kept to its allowed characters), [`OsVersion`] (three `u32`),
//!   local times written by diag's timestamp format, and three bounded texts built
//!   only through validating constructors ([`ThreadName`], [`CodeLocation`],
//!   [`Frame`]: no control character, so no line break; a module reduced to its base
//!   name, so no directory and no user name). The panic payload is never read.
//! - the panic hook ([`install_panic_hook`]) never panics, takes no lock and never
//!   calls `Log::write` (a hook that logs can deadlock on the log's mutex held by the
//!   panicking thread); one installed hook writes at most one file (the first panic
//!   wins); it does not chain the previous hook (std's default hook prints the
//!   payload to stderr).
//! - a file is written as `crash-*.tmp` and hard-linked to its name, so a process
//!   killed mid-write never leaves a partial `crash-*.txt` (`Session::begin` deletes
//!   stale tmp files), and an existing crash file is never replaced: a taken name
//!   steps to the next second (same pid, true `time:` line), see [`write_crash_file`].
//!   The shell's native-fault filter has its own writer that does not allocate,
//!   [`NativeFaultWriter`](super::NativeFaultWriter) (T-024), with the same
//!   rendering and the same names.
//! - [`retain_crash_files`] removes only regular `crash-*.txt` files: older than
//!   [`CRASH_MAX_AGE_DAYS`] by modification time, then all but the
//!   [`CRASH_MAX_FILES`] newest. The log's own retention never touches them.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::fmt::{self, Display, Write as _};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::format::{commit_text, version_text, write_timestamp, LocalTime};
use crate::build_info::BuildInfo;
use crate::clock::{Clock, LocalOffset};

/// Crash files older than this many days are deleted at start.
pub const CRASH_MAX_AGE_DAYS: u32 = 30;
/// At most this many crash files are kept (the newest).
pub const CRASH_MAX_FILES: usize = 20;

/// The longest bounded text written (thread name, location file, module name), in
/// bytes; longer text is cut at a character boundary.
pub(crate) const MAX_TEXT_BYTES: usize = 255;
/// The most frames the panic hook captures.
const MAX_FRAMES: usize = 64;

const CRASH_PREFIX: &str = "crash-";
const CRASH_SUFFIX: &str = ".txt";
pub(crate) const TMP_SUFFIX: &str = ".tmp";

/// The Windows version `major.minor.build` (shell: `RtlGetVersion`, T-024).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OsVersion {
    pub major: u32,
    pub minor: u32,
    pub build: u32,
}

impl Display for OsVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.build)
    }
}

/// `kind:` of a crash file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashKind {
    Panic,
    NativeFault,
    AbnormalEnd,
}

impl CrashKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CrashKind::Panic => "panic",
            CrashKind::NativeFault => "native_fault",
            CrashKind::AbnormalEnd => "abnormal_end",
        }
    }
}

/// An instant and the offset east of UTC its local text is written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalStamp {
    pub at: SystemTime,
    pub seconds_east: i32,
}

impl LocalStamp {
    /// `at` with the offset `offset` gives for it.
    pub fn of(at: SystemTime, offset: &dyn LocalOffset) -> LocalStamp {
        LocalStamp {
            at,
            seconds_east: offset.seconds_east(at),
        }
    }

    pub(crate) fn local(self) -> LocalTime {
        LocalTime::of(self.at, self.seconds_east)
    }

    /// The timestamp text (`YYYY-MM-DDTHH:MM:SS.mmm+HH:MM`), as the log writes it.
    pub(crate) fn text(self) -> String {
        let mut out = String::with_capacity(29);
        write_timestamp(&mut out, &self.local());
        out
    }
}

/// A crash file's name, `crash-YYYYMMDD-HHMMSS-<pid>.txt` (local time), held as
/// its integer parts so it can ride on a [`LogEvent`](super::LogEvent). `Display`
/// is the file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrashFileName {
    year: u64,
    month: u64,
    day: u64,
    hour: u64,
    minute: u64,
    second: u64,
    pid: u32,
}

impl CrashFileName {
    /// The name of a crash file written at `stamp` by process `pid`.
    pub(crate) fn new(stamp: LocalStamp, pid: u32) -> CrashFileName {
        let t = stamp.local();
        CrashFileName {
            year: t.year,
            month: t.month,
            day: t.day,
            hour: t.hour,
            minute: t.minute,
            second: t.second,
            pid,
        }
    }

    /// The name back from a file name diag wrote; `None` for any other text (it
    /// must render back byte for byte).
    pub(crate) fn parse(name: &str) -> Option<CrashFileName> {
        let body = name
            .strip_prefix(CRASH_PREFIX)?
            .strip_suffix(CRASH_SUFFIX)?;
        let mut parts = body.split('-');
        let (date, time, pid) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() || date.len() != 8 || time.len() != 6 {
            return None;
        }
        let digits = |s: &str, from: usize, n: usize| -> Option<u64> {
            let part = s.get(from..from + n)?;
            part.bytes()
                .all(|b| b.is_ascii_digit())
                .then(|| part.parse().ok())
                .flatten()
        };
        if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let parsed = CrashFileName {
            year: digits(date, 0, 4)?,
            month: digits(date, 4, 2)?,
            day: digits(date, 6, 2)?,
            hour: digits(time, 0, 2)?,
            minute: digits(time, 2, 2)?,
            second: digits(time, 4, 2)?,
            pid: pid.parse().ok()?,
        };
        (parsed.to_string() == name).then_some(parsed)
    }

    /// Test constructor (feature `test-fakes`): the same acceptance as the
    /// parser diag uses for its own names, `None` for any other text.
    #[cfg(any(test, feature = "test-fakes"))]
    pub fn fake(name: &str) -> Option<CrashFileName> {
        CrashFileName::parse(name)
    }

    fn stem(&self) -> String {
        let mut out = String::with_capacity(40);
        let _ = self.write_stem(&mut out);
        out
    }

    /// Writes `crash-YYYYMMDD-HHMMSS-<pid>` to `out` without allocating (the
    /// native-fault writer renders its names into fixed buffers).
    pub(crate) fn write_stem(&self, out: &mut dyn fmt::Write) -> fmt::Result {
        write!(
            out,
            "{CRASH_PREFIX}{:04}{:02}{:02}-{:02}{:02}{:02}-{}",
            self.year, self.month, self.day, self.hour, self.minute, self.second, self.pid
        )
    }
}

impl Display for CrashFileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write_stem(f)?;
        f.write_str(CRASH_SUFFIX)
    }
}

/// `text` without control characters, trimmed, cut to [`MAX_TEXT_BYTES`] at a
/// character boundary.
fn bounded(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_TEXT_BYTES));
    for c in text.chars().filter(|c| !c.is_control()) {
        if out.len() + c.len_utf8() > MAX_TEXT_BYTES {
            break;
        }
        out.push(c);
    }
    out.trim().to_string()
}

/// A thread name as written: bounded, no control character; `unnamed` when absent
/// or empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadName(String);

impl ThreadName {
    pub fn new(name: Option<&str>) -> ThreadName {
        let kept = name.map(bounded).unwrap_or_default();
        ThreadName(if kept.is_empty() {
            "unnamed".to_string()
        } else {
            kept
        })
    }
}

/// A panic location `file:line:column`; the file bounded, no control character
/// (std's `Location`: a source path compiled into the binary, FR-014).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeLocation {
    file: String,
    line: u32,
    column: u32,
}

impl CodeLocation {
    pub fn new(file: &str, line: u32, column: u32) -> CodeLocation {
        let file = bounded(file);
        CodeLocation {
            file: if file.is_empty() {
                "unknown".to_string()
            } else {
                file
            },
            line,
            column,
        }
    }
}

impl Display for CodeLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.column)
    }
}

/// A frame `<module>+0x<offset>`: the module reduced to its base name (the text
/// after the last `/` or `\`, so a full path, which names the user, is never
/// written), bounded, no control character; `unknown` when nothing is left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    module: String,
    offset: usize,
}

impl Frame {
    pub fn new(module: &str, offset: usize) -> Frame {
        let base = module.rsplit(['/', '\\']).next().unwrap_or(module);
        let base = bounded(base);
        Frame {
            module: if base.is_empty() {
                "unknown".to_string()
            } else {
                base
            },
            offset,
        }
    }
}

impl Display for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}+0x{:x}", self.module, self.offset)
    }
}

/// The kind-specific fields of a crash file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrashDetail {
    Panic {
        thread: ThreadName,
        location: Option<CodeLocation>,
        backtrace: Vec<Frame>,
    },
    NativeFault {
        exception_code: u32,
        fault_address: Option<Frame>,
    },
    AbnormalEnd {
        previous_session_pid: Option<u32>,
    },
}

impl CrashDetail {
    pub fn kind(&self) -> CrashKind {
        match self {
            CrashDetail::Panic { .. } => CrashKind::Panic,
            CrashDetail::NativeFault { .. } => CrashKind::NativeFault,
            CrashDetail::AbnormalEnd { .. } => CrashKind::AbnormalEnd,
        }
    }
}

/// Everything a crash file holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashFields {
    pub time: LocalStamp,
    pub build: BuildInfo,
    pub os: OsVersion,
    /// The `started` of the session the file belongs to (links it to the marker).
    pub session_started: Option<LocalStamp>,
    pub detail: CrashDetail,
}

/// The crash file text: `key: value` lines in data-model order, the panic
/// backtrace as a bare `backtrace:` line followed by `  <frame>` lines; an absent
/// optional is left out. Pure.
pub fn render_crash_file(fields: &CrashFields) -> String {
    let mut out = String::with_capacity(512);
    let mut line = |key: &str, value: &dyn Display| {
        let _ = writeln!(out, "{key}: {value}");
    };
    line("kind", &fields.detail.kind().as_str());
    line("time", &fields.time.text());
    line("version", &version_text(fields.build.version));
    line("commit", &commit_text(fields.build.commit));
    line("os", &fields.os);
    if let Some(started) = fields.session_started {
        line("session_started", &started.text());
    }
    match &fields.detail {
        CrashDetail::Panic {
            thread,
            location,
            backtrace,
        } => {
            line("thread", &thread.0);
            if let Some(location) = location {
                line("location", location);
            }
            out.push_str("backtrace:\n");
            for frame in backtrace {
                let _ = writeln!(out, "  {frame}");
            }
        }
        CrashDetail::NativeFault {
            exception_code,
            fault_address,
        } => {
            line("exception_code", &format_args!("0x{exception_code:08X}"));
            if let Some(frame) = fault_address {
                line("fault_address", frame);
            }
        }
        CrashDetail::AbnormalEnd {
            previous_session_pid,
        } => {
            if let Some(pid) = previous_session_pid {
                line("previous_session_pid", pid);
            }
        }
    }
    out
}

/// How many names [`write_crash_file`] tries: the name of `fields.time`, then each
/// following second (same pid).
pub(crate) const MAX_NAME_TRIES: u64 = 60;

/// Writes `fields` to a new `logs_dir/crash-*.txt` and returns the name written;
/// `None` (and no file left) when it cannot. An existing file is never replaced:
/// the text goes to `<stem>-<kind>.tmp` (created new, so a concurrent writer's tmp
/// is not overwritten either), which is then hard-linked to the name of
/// `fields.time` or, while that name is taken, of the next second (at most
/// [`MAX_NAME_TRIES`] names, the pid kept, the `time:` line unchanged). A hard link
/// fails when its path exists (`link` / `CreateHardLinkW`), so the claim of a name
/// is atomic between writers and the file appears complete. Never panics, takes no
/// lock, never creates the folder.
pub(crate) fn write_crash_file(
    logs_dir: &Path,
    pid: u32,
    fields: &CrashFields,
) -> Option<CrashFileName> {
    let first = CrashFileName::new(fields.time, pid);
    let tmp = logs_dir.join(format!(
        "{}-{}{TMP_SUFFIX}",
        first.stem(),
        fields.detail.kind().as_str()
    ));
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut file| {
            use std::io::Write as _;
            file.write_all(render_crash_file(fields).as_bytes())
        });
    if let Err(e) = written {
        // A tmp that already existed belongs to another writer: leave it.
        if e.kind() != ErrorKind::AlreadyExists {
            let _ = fs::remove_file(&tmp);
        }
        return None;
    }
    let mut claimed = None;
    for step in 0..MAX_NAME_TRIES {
        let Some(at) = fields.time.at.checked_add(Duration::from_secs(step)) else {
            break;
        };
        let name = CrashFileName::new(
            LocalStamp {
                at,
                seconds_east: fields.time.seconds_east,
            },
            pid,
        );
        match fs::hard_link(&tmp, logs_dir.join(name.to_string())) {
            Ok(()) => {
                claimed = Some(name);
                break;
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(_) => break,
        }
    }
    let _ = fs::remove_file(&tmp);
    claimed
}

/// Deletes stale `crash-*.tmp` files of `logs_dir` (a write cut short by a kill).
pub(crate) fn remove_stale_tmp(logs_dir: &Path) {
    let Ok(entries) = fs::read_dir(logs_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let is_tmp = entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with(CRASH_PREFIX) && n.ends_with(TMP_SUFFIX));
        if is_tmp && entry.metadata().is_ok_and(|m| m.is_file()) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// The names and modification times of the regular `crash-*.txt` files of `dir`.
pub(crate) fn crash_files(dir: &Path) -> Vec<(PathBuf, String, Option<SystemTime>)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if !(name.starts_with(CRASH_PREFIX) && name.ends_with(CRASH_SUFFIX)) {
                return None;
            }
            // Not followed: a link or a directory is not a crash file.
            let meta = entry.metadata().ok().filter(fs::Metadata::is_file)?;
            Some((entry.path(), name, meta.modified().ok()))
        })
        .collect()
}

/// Deletes the regular `crash-*.txt` files of `logs_dir` older than
/// [`CRASH_MAX_AGE_DAYS`] (by modification time; a file from the future has age
/// zero), then the oldest of the rest while more than [`CRASH_MAX_FILES`] remain.
/// Nothing else is touched; a missing folder does nothing; errors are ignored.
pub fn retain_crash_files(logs_dir: &Path, now: SystemTime) {
    let max_age = Duration::from_secs(u64::from(CRASH_MAX_AGE_DAYS) * 86_400);
    let mut files = crash_files(logs_dir);
    files.retain(|(path, _, modified)| {
        let old = modified
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age > max_age);
        !(old && fs::remove_file(path).is_ok())
    });
    // Newest first; an unreadable time counts as now (kept longest).
    files.sort_by(|a, b| (b.2.unwrap_or(now), &b.1).cmp(&(a.2.unwrap_or(now), &a.1)));
    for (path, _, _) in files.iter().skip(CRASH_MAX_FILES) {
        let _ = fs::remove_file(path);
    }
}

/// Fills `out` with the return addresses of the current stack, innermost first,
/// and returns how many it wrote (shell: `RtlCaptureStackBackTrace`, T-024). The
/// count is not trusted: the hook clamps it to `out.len()`.
pub trait FrameSource: Send + Sync {
    fn capture(&self, out: &mut [usize]) -> usize;
}

/// The module holding `ip` and the offset of `ip` in it (shell:
/// `GetModuleHandleExW` + `GetModuleFileNameW`, T-024). The text may be a full
/// path; diag reduces it to its base name ([`Frame::new`]).
pub trait ModuleResolver: Send + Sync {
    fn resolve(&self, ip: usize) -> Option<(String, usize)>;
}

/// What the panic hook needs, prepared at install time. The clock and offset are
/// clones of the ports the log uses.
pub struct CrashContext {
    pub logs_dir: PathBuf,
    pub build: BuildInfo,
    pub os: OsVersion,
    /// `Session::started` of this process's session.
    pub session_started: SystemTime,
    pub pid: u32,
    pub clock: Arc<dyn Clock>,
    pub offset: Arc<dyn LocalOffset>,
    pub frames: Arc<dyn FrameSource>,
    pub modules: Arc<dyn ModuleResolver>,
}

/// The hook's first-wins state: idle, writing, done.
const IDLE: u8 = 0;
const WRITING: u8 = 1;
const DONE: u8 = 2;
/// How long a later panic waits for the first one's file (`panic = "abort"`
/// ends the process when the later hook returns): 200 × 10 ms.
const WAIT_STEPS: u32 = 200;
const WAIT_STEP: Duration = Duration::from_millis(10);

/// Replaces the process's panic hook with one that writes one `panic` crash file
/// for the first panic it sees and nothing for later ones. It never reads the
/// payload, never panics (every error is ignored), takes no lock, never writes to
/// the log and does not call the previous hook. A later panic on another thread
/// waits (sleeping, bounded) until the first file is written, so an abort after
/// it cannot cut that write short. Each call installs a fresh guard.
pub fn install_panic_hook(ctx: CrashContext) {
    let state = AtomicU8::new(IDLE);
    std::panic::set_hook(Box::new(move |info| {
        if state
            .compare_exchange(IDLE, WRITING, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            for _ in 0..WAIT_STEPS {
                if state.load(Ordering::SeqCst) == DONE {
                    break;
                }
                std::thread::sleep(WAIT_STEP);
            }
            return;
        }
        let location = info
            .location()
            .map(|l| CodeLocation::new(l.file(), l.line(), l.column()));
        write_panic(&ctx, location);
        state.store(DONE, Ordering::SeqCst);
    }));
}

fn write_panic(ctx: &CrashContext, location: Option<CodeLocation>) {
    let now = ctx.clock.now();
    let current = std::thread::current();
    let mut ips = [0usize; MAX_FRAMES];
    let n = ctx.frames.capture(&mut ips).min(MAX_FRAMES);
    let backtrace = ips
        .get(..n)
        .unwrap_or_default()
        .iter()
        .map(|&ip| match ctx.modules.resolve(ip) {
            Some((module, offset)) => Frame::new(&module, offset),
            // An address outside every module: its absolute value.
            None => Frame::new("", ip),
        })
        .collect();
    let fields = CrashFields {
        time: LocalStamp::of(now, ctx.offset.as_ref()),
        build: ctx.build.clone(),
        os: ctx.os,
        session_started: Some(LocalStamp::of(ctx.session_started, ctx.offset.as_ref())),
        detail: CrashDetail::Panic {
            thread: ThreadName::new(current.name()),
            location,
            backtrace,
        },
    };
    let _ = write_crash_file(&ctx.logs_dir, ctx.pid, &fields);
}
