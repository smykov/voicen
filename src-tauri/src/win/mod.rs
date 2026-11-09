//! The Windows dictation adapters (T-006 invariants 1-4): they carry out and stamp,
//! and decide nothing. Each implements one core port (`voicen_core::platform`) or,
//! for the hotkey, calls the session's inputs; the Win32 numbers come from
//! `voicen_core::win32_data`. Windows CI proves them (`src-tauri/tests/{hotkey,
//! capture,clipboard,paste}.rs`). `shell_open` is the tray's Explorer opener, the
//! shell's `logs_folder::FolderOpener` port (T-071, `src-tauri/tests/tray.rs`). `crash` holds
//! the crash records' frame source, module resolver, exception filter and OS
//! version (T-024, `src-tauri/tests/crash.rs`). `track_capture` is the call mode's
//! `TrackSource` (mic and system loopback, QPC capture stamps; T-127,
//! `src-tauri/tests/call_capture.rs`).

pub mod capture;
pub mod clipboard;
pub mod crash;
pub mod hotkey;
pub mod paste;
pub mod purge;
pub mod shell_open;
pub mod track_capture;

use voicen_core::secrets::CredentialError;
use windows::Win32::Foundation::WIN32_ERROR;

/// The Win32 code of a failed call (the HRESULT when it is not a Win32 one): the
/// only part of an OS error a log line carries (#45; never its text).
pub(crate) fn os_code(err: &windows::core::Error) -> i32 {
    WIN32_ERROR::from_error(err)
        .map(|code| code.0 as i32)
        .unwrap_or(err.code().0)
}

/// `s` as a NUL-terminated UTF-16 string (a `PCWSTR` argument). The one copy for the
/// Credential Manager callers, `credentials` and `win::purge` (T-167).
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// A key-port error carrying the Win32 `code`.
pub(crate) fn credential_error(code: WIN32_ERROR) -> CredentialError {
    CredentialError {
        os_code: code.0 as i32,
    }
}

/// Overwrites `bytes` with zeros in a way the compiler keeps (key material).
pub(crate) fn wipe(bytes: &mut [u8]) {
    for byte in bytes.iter_mut() {
        // SAFETY: `byte` is a valid, aligned, exclusive reference.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
}
