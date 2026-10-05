//! The clipboard (T-006 design 3; research R-11): `WinClipboard` writes the text as
//! `CF_UNICODETEXT` together with the registered formats
//! `ExcludeClipboardContentFromMonitorProcessing` (present),
//! `CanIncludeInClipboardHistory` (DWORD 0) and `CanUploadToCloudClipboard`
//! (DWORD 0), so Windows keeps it out of the clipboard history and the cloud
//! clipboard (FR-022). `OpenClipboard` is retried briefly; any failure is
//! `ClipboardError` and the clipboard is closed on every path.

use std::mem::size_of_val;
use std::thread;
use std::time::Duration;

use voicen_core::platform::{Clipboard, ClipboardError};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;

/// `OpenClipboard` attempts and the pause after a refused one (R-11: up to
/// 10 x 20 ms), so a write never waits for a holder longer than about 200 ms.
const OPEN_TRIES: u32 = 10;
const OPEN_PAUSE: Duration = Duration::from_millis(20);

/// The registered formats of R-11, each written as one DWORD 0.
const EXCLUSION_FORMATS: [&str; 3] = [
    "ExcludeClipboardContentFromMonitorProcessing",
    "CanIncludeInClipboardHistory",
    "CanUploadToCloudClipboard",
];

/// The system clipboard. Holds no state across calls.
pub struct WinClipboard {
    _private: (),
}

impl WinClipboard {
    pub fn new() -> WinClipboard {
        WinClipboard { _private: () }
    }
}

impl Default for WinClipboard {
    fn default() -> WinClipboard {
        WinClipboard::new()
    }
}

/// The open clipboard: closed on drop, on every path.
struct Opened;

impl Drop for Opened {
    fn drop(&mut self) {
        // SAFETY: created only right after a successful OpenClipboard on this thread.
        let _ = unsafe { CloseClipboard() };
    }
}

fn open() -> Option<Opened> {
    for attempt in 0..OPEN_TRIES {
        // SAFETY: no owner window (T-059 probe fact `clipboard_null_owner`).
        if unsafe { OpenClipboard(None) }.is_ok() {
            return Some(Opened);
        }
        if attempt + 1 < OPEN_TRIES {
            thread::sleep(OPEN_PAUSE);
        }
    }
    None
}

/// Copies `data` into a fresh movable block and hands it to the clipboard as
/// `format` (the clipboard must be open and emptied by this thread). The block
/// belongs to the clipboard on success and is freed here on failure.
fn put<T: Copy>(format: u32, data: &[T]) -> Result<(), ClipboardError> {
    let bytes = size_of_val(data);
    // SAFETY: a fresh block of `bytes` bytes.
    let mem: HGLOBAL = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) }.map_err(|_| ClipboardError)?;
    // SAFETY: the block allocated above.
    let ptr = unsafe { GlobalLock(mem) }.cast::<T>();
    if ptr.is_null() {
        free(mem);
        return Err(ClipboardError);
    }
    // SAFETY: the locked block holds `bytes` bytes = `data.len()` values of `T`.
    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len()) };
    // The last unlock reports Err with code 0 (T-059 probe): not a failure.
    // SAFETY: the lock taken above.
    let _ = unsafe { GlobalUnlock(mem) };
    // SAFETY: the clipboard is open and owned by this thread's EmptyClipboard.
    match unsafe { SetClipboardData(format, Some(HANDLE(mem.0))) } {
        Ok(_) => Ok(()),
        Err(_) => {
            free(mem);
            Err(ClipboardError)
        }
    }
}

fn free(mem: HGLOBAL) {
    // SAFETY: a block this thread allocated and still owns, freed once.
    let _ = unsafe { GlobalFree(Some(mem)) };
}

fn format_id(name: &str) -> Result<u32, ClipboardError> {
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: a NUL-terminated UTF-16 string that outlives the call.
    match unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) } {
        0 => Err(ClipboardError),
        id => Ok(id),
    }
}

impl Clipboard for WinClipboard {
    fn set_text_excluded_from_history(&self, text: &str) -> Result<(), ClipboardError> {
        let mut formats = [0u32; EXCLUSION_FORMATS.len()];
        for (slot, name) in formats.iter_mut().zip(EXCLUSION_FORMATS) {
            *slot = format_id(name)?;
        }
        let with_nul: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let _open = open().ok_or(ClipboardError)?;
        // SAFETY: the clipboard is open on this thread (closed by `_open`).
        unsafe { EmptyClipboard() }.map_err(|_| ClipboardError)?;
        // The exclusion formats first, so no monitor ever sees the text without them.
        for format in formats {
            put(format, &[0u32])?;
        }
        put(u32::from(CF_UNICODETEXT.0), &with_nul)
    }
}
