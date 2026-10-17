//! The native-fault writer (T-024; spec 006 T026, FR-011, FR-014; T-060 T-100
//! r2#3): diag's second crash-file writer, beside `write_crash_file`. The shell's
//! unhandled-exception filter runs it on a faulting thread, where the heap lock may
//! be held or the heap corrupt, so [`NativeFaultWriter::new`] prepares everything at
//! install time (the rendered `version` .. `session_started` lines, one buffer of
//! fixed capacity for the whole text and two for the names, the offset of every
//! time) and [`NativeFaultWriter::record`] never allocates.
//!
//! Invariants (T-024 analysis (3), (4)):
//! - the bytes are `render_crash_file` of the same fields: `kind: native_fault`,
//!   `time` = `now` and `session_started` both at the offset fixed at install, and
//!   `fault_address` as [`Frame::new`](super::Frame::new) writes it (the module path
//!   decoded as `String::from_utf16_lossy`, reduced to its base name, no control
//!   character, at most 255 bytes, trimmed; `unknown+0x<absolute address>` when the
//!   module is unresolved). The middle lines are taken from `render_crash_file`
//!   itself at install, so the two writers share one rendering;
//! - the file is written as `<first name's stem>-native_fault.tmp` (created new) and
//!   hard-linked to the name of `now` or, while that name is taken, of each next
//!   second: the names `write_crash_file` picks, at most 60, the pid kept. A taken
//!   name is never replaced; the tmp is removed after the links;
//! - one writer records at most once (first wins); it takes no lock, never panics,
//!   never writes to the log.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::cell::UnsafeCell;
use std::fmt::{self, Write as _};
use std::io::{self, ErrorKind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use super::crash::{
    render_crash_file, CrashDetail, CrashFields, CrashFileName, CrashKind, LocalStamp, OsVersion,
    MAX_NAME_TRIES, MAX_TEXT_BYTES, TMP_SUFFIX,
};
use super::format::write_timestamp;
use crate::build_info::BuildInfo;

/// What the filter's file holds besides the fault itself, fixed at install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeFaultSetup {
    pub build: BuildInfo,
    pub os: OsVersion,
    /// `Session::started` of this process's session (links the file to the marker).
    pub session_started: SystemTime,
    pub pid: u32,
    /// The offset east of UTC of every time of the file (the shell: the local
    /// offset at `session_started`, so the `session_started` text equals the
    /// marker's). Fixed at install: the filter calls no time-zone function.
    pub seconds_east: i32,
}

/// The file operations of one record, in the logs dir; `tmp` and `name` are bare
/// file names. The shell implements them with `CreateFileW(CREATE_NEW)` +
/// `WriteFile`, `CreateHardLinkW` and `DeleteFileW`; none may allocate there.
pub trait FaultFiles {
    /// Creates `tmp` new and writes `bytes` to it. `AlreadyExists`: the tmp belongs
    /// to another writer (it is left alone).
    fn create_new(&mut self, tmp: &str, bytes: &[u8]) -> io::Result<()>;
    /// Hard-links `tmp` to `name`. `AlreadyExists`: the name is taken.
    fn hard_link(&mut self, tmp: &str, name: &str) -> io::Result<()>;
    /// Removes `tmp`; errors are ignored.
    fn remove(&mut self, tmp: &str);
}

/// The fixed tail of a file: `time`'s text (29 bytes) and the line around it, the
/// `exception_code` line, the `fault_address` line with a module of at most
/// [`MAX_TEXT_BYTES`] and a 64-bit offset; rounded up.
const TAIL_CAPACITY: usize = 512;
/// The longest name or tmp name (`crash-YYYYMMDD-HHMMSS-<u32>-native_fault.tmp` is
/// 49 bytes).
const NAME_CAPACITY: usize = 64;

const KIND_AND_TIME: &str = "kind: native_fault\ntime: ";

/// The buffers of the one record; reached only by the thread that won `taken`.
struct Scratch {
    text: Box<[u8]>,
    tmp: [u8; NAME_CAPACITY],
    name: [u8; NAME_CAPACITY],
}

/// Writes one `native_fault` crash file without allocating. Built at install with
/// [`new`](Self::new) (which allocates); the shell keeps it in a static.
pub struct NativeFaultWriter {
    pid: u32,
    seconds_east: i32,
    /// `version:` .. `session_started:` lines, as `render_crash_file` writes them.
    middle: String,
    taken: AtomicBool,
    scratch: UnsafeCell<Scratch>,
}

// SAFETY: `scratch` is the only state not `Sync`. It is touched only inside
// `record`, after that call has swapped `taken` from false to true; `taken` never
// goes back to false, so exactly one thread, once, ever has access to it.
unsafe impl Sync for NativeFaultWriter {}

/// A `fmt::Write` over a fixed slice: an overflow is an error, never a growth.
struct Cursor<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl fmt::Write for Cursor<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(fmt::Error)?;
        let dst = self.buf.get_mut(self.len..end).ok_or(fmt::Error)?;
        dst.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

impl Cursor<'_> {
    fn text(&self) -> Option<&str> {
        std::str::from_utf8(self.buf.get(..self.len)?).ok()
    }
}

impl NativeFaultWriter {
    /// Prepares the writer: renders the fixed lines once (through
    /// `render_crash_file`) and sizes the buffers. May allocate.
    pub fn new(setup: NativeFaultSetup) -> NativeFaultWriter {
        let NativeFaultSetup {
            build,
            os,
            session_started,
            pid,
            seconds_east,
        } = setup;
        let stamp = LocalStamp {
            at: session_started,
            seconds_east,
        };
        let template = render_crash_file(&CrashFields {
            time: stamp,
            build,
            os,
            session_started: Some(stamp),
            detail: CrashDetail::NativeFault {
                exception_code: 0,
                fault_address: None,
            },
        });
        // The lines after `kind` and `time`, up to the `exception_code` line.
        let middle: String = template
            .split_inclusive('\n')
            .skip(2)
            .take_while(|line| !line.starts_with("exception_code: "))
            .collect();
        let capacity = KIND_AND_TIME.len() + middle.len() + TAIL_CAPACITY;
        NativeFaultWriter {
            pid,
            seconds_east,
            middle,
            taken: AtomicBool::new(false),
            scratch: UnsafeCell::new(Scratch {
                text: vec![0; capacity].into_boxed_slice(),
                tmp: [0; NAME_CAPACITY],
                name: [0; NAME_CAPACITY],
            }),
        }
    }

    /// Writes the file of a fault `exception_code` at `fault_address` (in
    /// `module`: its path as `GetModuleFileNameW` gives it and the offset in it;
    /// `None` when outside every module) at `now`, and returns its name. Never
    /// allocates. `None` with no file left when it cannot write, or when this
    /// writer already recorded (first wins; a later call touches no file). On a
    /// tmp that already exists nothing else is done; on any other create error the
    /// tmp is removed; a taken name steps to the next second (at most 60 names),
    /// any other link error stops; the tmp is removed after linking.
    pub fn record(
        &self,
        now: SystemTime,
        exception_code: u32,
        fault_address: usize,
        module: Option<(&[u16], usize)>,
        files: &mut dyn FaultFiles,
    ) -> Option<CrashFileName> {
        if self.taken.swap(true, Ordering::SeqCst) {
            return None;
        }
        // SAFETY: the swap above returned false, so this is the one call that
        // reaches `scratch` (see `unsafe impl Sync`); the reference ends with it.
        let scratch = unsafe { &mut *self.scratch.get() };
        let Scratch { text, tmp, name } = scratch;

        let stamp = LocalStamp {
            at: now,
            seconds_east: self.seconds_east,
        };
        let first = CrashFileName::new(stamp, self.pid);
        let mut tmp = Cursor {
            buf: tmp.as_mut_slice(),
            len: 0,
        };
        first.write_stem(&mut tmp).ok()?;
        write!(tmp, "-{}{TMP_SUFFIX}", CrashKind::NativeFault.as_str()).ok()?;
        let tmp = tmp.text()?;

        let mut text = Cursor { buf: text, len: 0 };
        self.render(&mut text, stamp, exception_code, fault_address, module)
            .ok()?;
        let bytes = text.buf.get(..text.len)?;

        if let Err(e) = files.create_new(tmp, bytes) {
            // A tmp that already existed belongs to another writer: leave it.
            if e.kind() != ErrorKind::AlreadyExists {
                files.remove(tmp);
            }
            return None;
        }
        let mut claimed = None;
        for step in 0..MAX_NAME_TRIES {
            let Some(at) = now.checked_add(Duration::from_secs(step)) else {
                break;
            };
            let candidate = CrashFileName::new(
                LocalStamp {
                    at,
                    seconds_east: self.seconds_east,
                },
                self.pid,
            );
            let mut cursor = Cursor {
                buf: name.as_mut_slice(),
                len: 0,
            };
            if write!(cursor, "{candidate}").is_err() {
                break;
            }
            let Some(text) = cursor.text() else {
                break;
            };
            match files.hard_link(tmp, text) {
                Ok(()) => {
                    claimed = Some(candidate);
                    break;
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                Err(_) => break,
            }
        }
        files.remove(tmp);
        claimed
    }

    /// The whole text, as `render_crash_file` writes it for these fields.
    fn render(
        &self,
        out: &mut Cursor<'_>,
        time: LocalStamp,
        exception_code: u32,
        fault_address: usize,
        module: Option<(&[u16], usize)>,
    ) -> fmt::Result {
        out.write_str(KIND_AND_TIME)?;
        write_timestamp(out, &time.local());
        out.write_str("\n")?;
        out.write_str(&self.middle)?;
        write!(
            out,
            "exception_code: 0x{exception_code:08X}\nfault_address: "
        )?;
        let offset = match module {
            Some((path, offset)) => {
                write_module(out, path)?;
                offset
            }
            None => {
                out.write_str("unknown")?;
                fault_address
            }
        };
        writeln!(out, "+0x{offset:x}")
    }
}

/// `Frame::new`'s module text of `path` (UTF-16, as the OS gives it), written in
/// place: the units after the last `/` or `\` decoded lossily, control characters
/// left out, cut before [`MAX_TEXT_BYTES`] would be passed, trimmed; `unknown`
/// when nothing is left. A separator is one unit and never part of a surrogate
/// pair, so splitting the units equals splitting the decoded text.
fn write_module(out: &mut Cursor<'_>, path: &[u16]) -> fmt::Result {
    let base = path
        .iter()
        .rposition(|&u| u == u16::from(b'/') || u == u16::from(b'\\'))
        .and_then(|i| path.get(i + 1..))
        .unwrap_or(path);
    let start = out.len;
    let mut utf8 = [0_u8; 4];
    for c in char::decode_utf16(base.iter().copied())
        .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
        .filter(|c| !c.is_control())
    {
        if out.len - start + c.len_utf8() > MAX_TEXT_BYTES {
            break;
        }
        out.write_str(c.encode_utf8(&mut utf8))?;
    }
    let written = out.buf.get(start..out.len).ok_or(fmt::Error)?;
    let written = std::str::from_utf8(written).map_err(|_| fmt::Error)?;
    let lead = written.len() - written.trim_start().len();
    let kept = written.trim().len();
    out.buf
        .copy_within(start + lead..start + lead + kept, start);
    out.len = start + kept;
    if kept == 0 {
        out.write_str("unknown")?;
    }
    Ok(())
}
