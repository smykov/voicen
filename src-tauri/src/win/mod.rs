//! The Windows dictation adapters (T-006 invariants 1-4): they carry out and stamp,
//! and decide nothing. Each implements one core port (`voicen_core::platform`) or,
//! for the hotkey, calls the session's inputs; the Win32 numbers come from
//! `voicen_core::win32_data`. Windows CI proves them (`src-tauri/tests/{hotkey,
//! capture,clipboard,paste}.rs`). `shell_open` is the tray's Explorer opener, the
//! shell's `logs_folder::FolderOpener` port (T-071, `src-tauri/tests/tray.rs`).

pub mod capture;
pub mod clipboard;
pub mod hotkey;
pub mod paste;
pub mod purge;
pub mod shell_open;

use windows::Win32::Foundation::WIN32_ERROR;

/// The Win32 code of a failed call (the HRESULT when it is not a Win32 one): the
/// only part of an OS error a log line carries (#45; never its text).
fn os_code(err: &windows::core::Error) -> i32 {
    WIN32_ERROR::from_error(err)
        .map(|code| code.0 as i32)
        .unwrap_or(err.code().0)
}
