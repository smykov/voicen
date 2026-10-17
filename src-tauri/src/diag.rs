//! The shell side of the local log (T-008; FR-20, FR-18): opens the one
//! `voicen_core::diag::Log` of the process over the logs dir and writes `Started`
//! as its first line. Replaces the skeleton's `log_start`. The resolver of the
//! logs dir stays `paths::log_dir()` (P-010); `run()` passes it, tests pass a temp
//! dir. The local UTC offset comes from the Windows time-zone rules; the wall time
//! from core's `SystemClock`.
//!
//! Every diagnostic of the shell is a typed `LogEvent` written to this log; there
//! is no `eprintln!`, no `log` / `tracing` logger and no other file under `logs/`
//! (docs/decisions/diagnostics-log.md).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use voicen_core::clock::{Clock, LocalOffset, SystemClock};
use voicen_core::diag::{
    install_panic_hook, retain_crash_files, CrashContext, Log, LogConfig, LogEvent,
    NativeFaultSetup, OnUnwritable, OsVersion, Session, SessionStart,
};

/// Opens the one log over `logs_dir` (`SystemClock`, the local UTC offset,
/// `LogConfig::default()`) and writes `LogEvent::Started { build_info(),
/// std::process::id() }` as its first line. Never fails and never panics: an
/// unwritable folder gives a degraded log and one `on_unwritable` call (T-054
/// turns it into the one-time notice).
pub fn start(logs_dir: PathBuf, on_unwritable: OnUnwritable) -> Arc<Log> {
    let log = Arc::new(Log::open(
        logs_dir,
        Arc::new(SystemClock),
        Arc::new(OsLocalOffset),
        LogConfig::default(),
        on_unwritable,
    ));
    log.write(LogEvent::Started {
        build: voicen_core::build_info(),
        pid: std::process::id(),
    });
    log
}

/// This process's session (T-024): ended cleanly at most once, by
/// [`SessionEnd::end`] from `on_run_event`'s `RunEvent::Exit` arm (tray Exit, and
/// `WM_ENDSESSION(TRUE)` through tao). Dropping it ends nothing: only `end` deletes
/// the marker, so every other way out of the process leaves it for the next start.
pub struct SessionEnd(Mutex<Option<(Session, Arc<Log>)>>);

impl SessionEnd {
    /// Deletes the marker and writes one `exited` line (`Session::end_clean`) the
    /// first time; later calls do nothing.
    pub fn end(&self) {
        let taken = self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some((session, log)) = taken {
            session.end_clean(&log);
        }
    }
}

/// Begins this process's session over `logs_dir` and arms the crash records
/// (T-024 invariant 1), in this order: the OS version (`RtlGetVersion`),
/// `Session::begin` (the previous session's verdict, its `previous session` line,
/// stale `crash-*.tmp` deleted, the new marker), the panic hook (frames from
/// `RtlCaptureStackBackTrace`, modules from `GetModuleHandleExW`), the
/// unhandled-exception filter (a `native_fault` file, offset fixed now), then
/// `retain_crash_files`. Call once, in the primary instance only, right after
/// [`start`] (`release_parts`; the crash tests' children). [`start`] stays
/// hook-free. Never fails or panics.
pub fn begin_session(logs_dir: PathBuf, log: &Arc<Log>) -> SessionEnd {
    let os = os_version();
    let build = voicen_core::build_info();
    let pid = std::process::id();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let offset: Arc<dyn LocalOffset> = Arc::new(OsLocalOffset);
    let (session, _previous) = Session::begin(
        SessionStart {
            logs_dir: &logs_dir,
            build: build.clone(),
            os,
            pid,
            clock: clock.as_ref(),
            offset: offset.as_ref(),
        },
        log,
    );
    let started = session.started();
    install_panic_hook(CrashContext {
        logs_dir: logs_dir.clone(),
        build: build.clone(),
        os,
        session_started: started,
        pid,
        clock: Arc::clone(&clock),
        offset: Arc::clone(&offset),
        frames: crash_ports::frames(),
        modules: crash_ports::modules(),
    });
    crash_ports::install_fault_filter(
        &logs_dir,
        NativeFaultSetup {
            build,
            os,
            session_started: started,
            pid,
            // The marker's offset, so the file's `session_started` equals its `started`.
            seconds_east: offset.seconds_east(started),
        },
    );
    retain_crash_files(&logs_dir, clock.now());
    SessionEnd(Mutex::new(Some((session, Arc::clone(log)))))
}

/// The Windows version (`RtlGetVersion`, which the compatibility layer does not
/// shim); `0.0.0` when it cannot be read.
pub fn os_version() -> OsVersion {
    crash_ports::os_version()
}

/// The Win32 crash adapters (`win::crash`).
#[cfg(windows)]
mod crash_ports {
    use std::path::Path;
    use std::sync::Arc;

    use voicen_core::diag::{FrameSource, ModuleResolver, NativeFaultSetup, OsVersion};

    use crate::win::crash;

    pub(super) fn frames() -> Arc<dyn FrameSource> {
        Arc::new(crash::WinFrames)
    }

    pub(super) fn modules() -> Arc<dyn ModuleResolver> {
        Arc::new(crash::WinModules)
    }

    pub(super) fn install_fault_filter(logs_dir: &Path, setup: NativeFaultSetup) {
        crash::install_fault_filter(logs_dir, setup);
    }

    pub(super) fn os_version() -> OsVersion {
        crash::os_version()
    }
}

/// Not Windows (the shell runs on Windows only, decision #5): no frames, no
/// modules, no filter, version `0.0.0`.
#[cfg(not(windows))]
mod crash_ports {
    use std::path::Path;
    use std::sync::Arc;

    use voicen_core::diag::{FrameSource, ModuleResolver, NativeFaultSetup, OsVersion};

    struct Nothing;

    impl FrameSource for Nothing {
        fn capture(&self, _out: &mut [usize]) -> usize {
            0
        }
    }

    impl ModuleResolver for Nothing {
        fn resolve(&self, _ip: usize) -> Option<(String, usize)> {
            None
        }
    }

    pub(super) fn frames() -> Arc<dyn FrameSource> {
        Arc::new(Nothing)
    }

    pub(super) fn modules() -> Arc<dyn ModuleResolver> {
        Arc::new(Nothing)
    }

    pub(super) fn install_fault_filter(_logs_dir: &Path, _setup: NativeFaultSetup) {}

    pub(super) fn os_version() -> OsVersion {
        OsVersion {
            major: 0,
            minor: 0,
            build: 0,
        }
    }
}

/// The OS code of a tauri error that is an I/O error; `None` otherwise. A warning
/// line carries this code and the kind only: the text of a tauri error is never
/// logged (#45).
pub fn io_os_code(err: &tauri::Error) -> Option<i32> {
    match err {
        tauri::Error::Io(io) => io.raw_os_error(),
        _ => None,
    }
}

/// The offset of the active Windows time zone at an instant, daylight saving
/// included; UTC (0) when the instant cannot be converted.
struct OsLocalOffset;

impl LocalOffset for OsLocalOffset {
    fn seconds_east(&self, t: SystemTime) -> i32 {
        seconds_east_at(t).unwrap_or(0)
    }
}

/// Local time minus UTC at `t`, in seconds: `t` (whole seconds) as a UTC
/// `SYSTEMTIME`, converted by `SystemTimeToTzSpecificLocalTime` with the active
/// time zone (no explicit zone), both back to `FILETIME` and subtracted.
#[cfg(windows)]
fn seconds_east_at(t: SystemTime) -> Option<i32> {
    use std::time::UNIX_EPOCH;

    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{
        FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime,
    };

    /// 100 ns ticks from 1601-01-01 (the `FILETIME` epoch) to 1970-01-01.
    const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;
    const TICKS_PER_SECOND: i64 = 10_000_000;

    fn ticks(ft: &FILETIME) -> Option<i64> {
        i64::try_from((u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)).ok()
    }

    let secs = i64::try_from(t.duration_since(UNIX_EPOCH).ok()?.as_secs()).ok()?;
    let utc_ticks = secs
        .checked_mul(TICKS_PER_SECOND)?
        .checked_add(UNIX_EPOCH_TICKS)?;
    let split = u64::try_from(utc_ticks).ok()?;
    let utc_file = FILETIME {
        dwLowDateTime: (split & 0xFFFF_FFFF) as u32,
        dwHighDateTime: (split >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    // SAFETY: both pointers are valid locals for the duration of the call.
    unsafe { FileTimeToSystemTime(&utc_file, &mut utc) }.ok()?;
    let mut local = SYSTEMTIME::default();
    // SAFETY: `None` selects the active time zone; both pointers are valid locals.
    unsafe { SystemTimeToTzSpecificLocalTime(None, &utc, &mut local) }.ok()?;
    let mut local_file = FILETIME::default();
    // SAFETY: both pointers are valid locals for the duration of the call.
    unsafe { SystemTimeToFileTime(&local, &mut local_file) }.ok()?;
    let diff = ticks(&local_file)?.checked_sub(utc_ticks)?;
    i32::try_from(diff / TICKS_PER_SECOND).ok()
}

/// Not Windows: UTC (the shell runs on Windows only, decision #5).
#[cfg(not(windows))]
fn seconds_east_at(_t: SystemTime) -> Option<i32> {
    Some(0)
}
