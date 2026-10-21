//! The Win32 adapters of the crash records (T-024; spec 006 T026, FR-011, FR-014).
//! They hand only integers and raw OS text to core's validating constructors and
//! decide nothing:
//! - [`WinFrames`]: core's `FrameSource`, `RtlCaptureStackBackTrace`;
//! - [`WinModules`]: core's `ModuleResolver`, `GetModuleHandleExW(FROM_ADDRESS |
//!   UNCHANGED_REFCOUNT)` + `GetModuleFileNameW` (core reduces the path to its base
//!   name);
//! - [`install_fault_filter`]: the unhandled-exception filter
//!   (`SetUnhandledExceptionFilter`), which writes one `native_fault` file through
//!   core's `NativeFaultWriter` over [`WinFaultFiles`] (`CreateFileW(CREATE_NEW)` +
//!   `WriteFile`, `CreateHardLinkW`, `DeleteFileW`) and returns
//!   `EXCEPTION_CONTINUE_SEARCH`, so WER and the exit code stay Windows';
//! - [`os_version`]: `RtlGetVersion` (not shimmed by the compatibility layer, unlike
//!   `GetVersionExW`).
//!
//! The filter runs on the faulting thread, where the heap may be locked or corrupt:
//! everything it touches is prepared at install and kept in a static (not on that
//! thread's stack, which may be nearly exhausted). It does not allocate, takes no
//! Voicen lock, never panics and never writes to the log. The Win32 calls it makes
//! may take OS loader locks (accepted residual, docs/decisions/diagnostics-log.md).
//! Proven on Windows CI by `src-tauri/tests/crash.rs`.

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::io::{self, ErrorKind};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::SystemTime;

use voicen_core::diag::{
    FaultFiles, FrameSource, ModuleResolver, NativeFaultSetup, NativeFaultWriter, OsVersion,
};
use windows::core::PCWSTR;
use windows::Wdk::System::SystemServices::RtlGetVersion;
use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HMODULE, WIN32_ERROR};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, CreateHardLinkW, DeleteFileW, WriteFile, CREATE_NEW, FILE_ATTRIBUTE_NORMAL,
    FILE_SHARE_NONE,
};
use windows::Win32::System::Diagnostics::Debug::{
    RtlCaptureStackBackTrace, SetUnhandledExceptionFilter, EXCEPTION_CONTINUE_SEARCH,
    EXCEPTION_POINTERS,
};
use windows::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
};
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;

/// The most frames one capture asks `RtlCaptureStackBackTrace` for.
const MAX_FRAMES: usize = 64;
/// The longest module path read (UTF-16 units); a longer one is cut by the OS, and
/// only its base name is written anyway.
const MODULE_PATH_UNITS: usize = 1024;
/// Room for a bare crash file or tmp name after the logs dir (core's names are at
/// most 49 ASCII bytes) and the terminating NUL.
const NAME_UNITS: usize = 64;

/// The Windows version from `RtlGetVersion`; [`OsVersion::UNKNOWN`] (`0.0.0`) when
/// the call fails.
pub fn os_version() -> OsVersion {
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is a valid OSVERSIONINFOW with its size set, as the call requires.
    let status = unsafe { RtlGetVersion(&mut info) };
    if status.is_ok() {
        OsVersion {
            major: info.dwMajorVersion,
            minor: info.dwMinorVersion,
            build: info.dwBuildNumber,
        }
    } else {
        OsVersion::UNKNOWN
    }
}

/// The return addresses of the current stack (`RtlCaptureStackBackTrace`).
pub struct WinFrames;

impl FrameSource for WinFrames {
    fn capture(&self, out: &mut [usize]) -> usize {
        let mut raw = [std::ptr::null_mut::<c_void>(); MAX_FRAMES];
        let n = out.len().min(MAX_FRAMES);
        let Some(raw) = raw.get_mut(..n) else {
            return 0;
        };
        // SAFETY: `raw` is a valid buffer of `n` (at most 64) pointers; no hash is asked.
        let got = usize::from(unsafe { RtlCaptureStackBackTrace(0, raw, None) }).min(n);
        for (o, r) in out.iter_mut().zip(raw.iter()).take(got) {
            *o = *r as usize;
        }
        got
    }
}

/// The module of an address (`GetModuleHandleExW` + `GetModuleFileNameW`).
pub struct WinModules;

impl ModuleResolver for WinModules {
    fn resolve(&self, ip: usize) -> Option<(String, usize)> {
        let mut path = [0_u16; MODULE_PATH_UNITS];
        let (len, offset) = module_of(ip, &mut path)?;
        Some((String::from_utf16_lossy(path.get(..len)?), offset))
    }
}

/// The module holding `ip`: its path written into `path` (its length returned) and
/// the offset of `ip` from the module's base. No allocation; `None` outside every
/// module or when the path cannot be read.
fn module_of(ip: usize, path: &mut [u16]) -> Option<(usize, usize)> {
    let mut module = HMODULE::default();
    // SAFETY: with FROM_ADDRESS the name argument is an address inside the module, not
    // read as text; UNCHANGED_REFCOUNT takes no reference, so nothing is to be freed.
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(ip as *const u16),
            &mut module,
        )
    }
    .ok()?;
    let offset = ip.checked_sub(module.0 as usize)?;
    // SAFETY: `path` is a valid, writable buffer; the call writes at most its length.
    let len = unsafe { GetModuleFileNameW(Some(module), path) } as usize;
    (len > 0).then_some((len.min(path.len()), offset))
}

/// A `windows` error as an `io::Error` of its Win32 code (no allocation:
/// `windows`' own conversion boxes non-Win32 errors).
fn io_error(err: &windows::core::Error) -> io::Error {
    WIN32_ERROR::from_error(err).map_or_else(
        || io::Error::from(ErrorKind::Other),
        |code| io::Error::from_raw_os_error(code.0 as i32),
    )
}

/// The filter's files: full NUL-terminated UTF-16 paths built in fixed buffers
/// from the logs dir (prepared at install) and core's bare names.
pub struct WinFaultFiles {
    /// The logs dir and a trailing `\`, no NUL.
    dir: Box<[u16]>,
    tmp: Box<[u16]>,
    name: Box<[u16]>,
}

impl WinFaultFiles {
    /// Prepares the buffers for `logs_dir`. Allocates (install time).
    pub fn new(logs_dir: &Path) -> WinFaultFiles {
        let mut dir: Vec<u16> = logs_dir.as_os_str().encode_wide().collect();
        if !matches!(dir.last(), Some(&u) if u == u16::from(b'\\') || u == u16::from(b'/')) {
            dir.push(u16::from(b'\\'));
        }
        let room = dir.len() + NAME_UNITS;
        WinFaultFiles {
            dir: dir.into_boxed_slice(),
            tmp: vec![0; room].into_boxed_slice(),
            name: vec![0; room].into_boxed_slice(),
        }
    }
}

/// `dir` + `file` + NUL into `buf`; `None` when it does not fit.
fn full_path(buf: &mut [u16], dir: &[u16], file: &str) -> Option<PCWSTR> {
    let mut units = dir.iter().copied().chain(file.encode_utf16()).chain([0]);
    let mut len = 0;
    for slot in buf.iter_mut() {
        match units.next() {
            Some(u) => {
                *slot = u;
                len += 1;
            }
            None => break,
        }
    }
    // Everything, NUL included, was copied.
    (units.next().is_none() && len > 0 && buf.get(len - 1) == Some(&0))
        .then_some(PCWSTR(buf.as_ptr()))
}

impl FaultFiles for WinFaultFiles {
    fn create_new(&mut self, tmp: &str, bytes: &[u8]) -> io::Result<()> {
        let path = full_path(&mut self.tmp, &self.dir, tmp)
            .ok_or_else(|| io::Error::from(ErrorKind::InvalidInput))?;
        // SAFETY: `path` is NUL-terminated and lives in `self.tmp` for the call.
        let handle = unsafe {
            CreateFileW(
                path,
                GENERIC_WRITE.0,
                FILE_SHARE_NONE,
                None,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .map_err(|e| io_error(&e))?;
        let mut rest = bytes;
        let mut result = Ok(());
        while !rest.is_empty() {
            let mut written = 0_u32;
            // SAFETY: `handle` is the file just created; `rest` and `written` are valid
            // for the call; no overlapped I/O.
            if let Err(e) = unsafe { WriteFile(handle, Some(rest), Some(&mut written), None) } {
                result = Err(io_error(&e));
                break;
            }
            match rest.get(written as usize..) {
                Some(after) if written > 0 => rest = after,
                _ => {
                    result = Err(io::Error::from(ErrorKind::WriteZero));
                    break;
                }
            }
        }
        // SAFETY: `handle` is open and closed once, here.
        let _ = unsafe { CloseHandle(handle) };
        result
    }

    fn hard_link(&mut self, tmp: &str, name: &str) -> io::Result<()> {
        let (Some(existing), Some(link)) = (
            full_path(&mut self.tmp, &self.dir, tmp),
            full_path(&mut self.name, &self.dir, name),
        ) else {
            return Err(io::Error::from(ErrorKind::InvalidInput));
        };
        // SAFETY: both paths are NUL-terminated and live in `self` for the call. The
        // link fails on an existing name (ERROR_ALREADY_EXISTS): nothing is replaced.
        unsafe { CreateHardLinkW(link, existing, None) }.map_err(|e| io_error(&e))
    }

    fn remove(&mut self, tmp: &str) {
        if let Some(path) = full_path(&mut self.tmp, &self.dir, tmp) {
            // SAFETY: `path` is NUL-terminated and lives in `self.tmp` for the call.
            let _ = unsafe { DeleteFileW(path) };
        }
    }
}

/// What the filter needs, prepared at install. `buffers` is reached only by the
/// thread that swapped `entered` from false to true (it never goes back).
struct FaultState {
    writer: NativeFaultWriter,
    entered: AtomicBool,
    buffers: UnsafeCell<FaultBuffers>,
}

struct FaultBuffers {
    files: WinFaultFiles,
    module: Box<[u16]>,
}

// SAFETY: `buffers` is the only state not `Sync`; `record` touches it only after its
// swap of `entered` returned false, which happens at most once per process.
unsafe impl Sync for FaultState {}

static FAULT: OnceLock<FaultState> = OnceLock::new();

/// Installs the process's unhandled-exception filter over `logs_dir` with the fields
/// of `setup`. Once per process: a later call changes nothing (the first session's
/// filter stays). Call after `Session::begin` (the file names its `started`).
pub fn install_fault_filter(logs_dir: &Path, setup: NativeFaultSetup) {
    let state = FaultState {
        writer: NativeFaultWriter::new(setup),
        entered: AtomicBool::new(false),
        buffers: UnsafeCell::new(FaultBuffers {
            files: WinFaultFiles::new(logs_dir),
            module: vec![0; MODULE_PATH_UNITS].into_boxed_slice(),
        }),
    };
    if FAULT.set(state).is_ok() {
        // SAFETY: `on_unhandled` has the filter's signature and only reads its argument.
        // The previous filter is not chained: WER handles what this one passes on.
        let _ = unsafe { SetUnhandledExceptionFilter(Some(on_unhandled)) };
    }
}

/// The filter: one file for the first fault, then `EXCEPTION_CONTINUE_SEARCH`.
unsafe extern "system" fn on_unhandled(info: *const EXCEPTION_POINTERS) -> i32 {
    if let Some(state) = FAULT.get() {
        // SAFETY: the OS passes a valid EXCEPTION_POINTERS (or null) for the call.
        unsafe { state.record(info) };
    }
    EXCEPTION_CONTINUE_SEARCH
}

impl FaultState {
    /// # Safety
    /// `info` is null or the `EXCEPTION_POINTERS` the OS handed to the filter.
    unsafe fn record(&self, info: *const EXCEPTION_POINTERS) {
        if self.entered.swap(true, Ordering::SeqCst) {
            return;
        }
        // SAFETY: the swap returned false: this is the one access (see `unsafe impl Sync`).
        let buffers = unsafe { &mut *self.buffers.get() };
        // SAFETY: the OS's pointers are valid for the filter call; null is checked.
        let Some(record) =
            (unsafe { info.as_ref() }).and_then(|i| unsafe { i.ExceptionRecord.as_ref() })
        else {
            return;
        };
        let code = record.ExceptionCode.0 as u32;
        let address = record.ExceptionAddress as usize;
        let module = module_of(address, &mut buffers.module)
            .and_then(|(len, offset)| Some((buffers.module.get(..len)?, offset)));
        let _ = self
            .writer
            .record(SystemTime::now(), code, address, module, &mut buffers.files);
    }
}
