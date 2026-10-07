//! The Win32 [`FolderOpener`] (T-071, option A in docs/decisions/windows-shell.md):
//! `ShellExecuteExW` with verb "open" on the directory, `SEE_MASK_NOASYNC |
//! SEE_MASK_FLAG_NO_UI` (it returns only after the launch, and never shows an
//! error dialog), under `CoInitializeEx(COINIT_APARTMENTTHREADED)` on the calling
//! thread (balanced by `CoUninitialize`). No command line and no shell parsing.
//! It opens the folder only (nothing selected) and never creates it: a missing
//! path is `Err` with Win32 code 2 (`ERROR_FILE_NOT_FOUND`).

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::core::{w, PCWSTR};
use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, COINIT, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::UI::Shell::{
    ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use super::os_code;
use crate::logs_folder::FolderOpener;

/// Win32 `ERROR_DIRECTORY`: the path is not a directory.
const ERROR_DIRECTORY: i32 = 267;

/// The Explorer as a [`FolderOpener`].
pub struct ShellOpener;

impl FolderOpener for ShellOpener {
    fn open(&self, dir: &Path) -> io::Result<()> {
        // Verb "open" on a file would run it: only a directory (or a missing path,
        // which the shell refuses with code 2) goes to the shell.
        if dir.exists() && !dir.is_dir() {
            return Err(io::Error::from_raw_os_error(ERROR_DIRECTORY));
        }
        let file: Vec<u16> = dir
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let _com = Com::init();
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpVerb: w!("open"),
            lpFile: PCWSTR(file.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        // SAFETY: `info` is a valid SHELLEXECUTEINFOW with `cbSize` set; `lpFile`
        // points to the NUL-terminated `file`, which outlives the call; no process
        // handle is asked for, so nothing is left to close.
        unsafe { ShellExecuteExW(&mut info) }
            .map_err(|err| io::Error::from_raw_os_error(os_code(&err)))
    }
}

/// COM on the current thread for the call: `CoUninitialize` on drop only when
/// `CoInitializeEx` succeeded (`S_OK` or `S_FALSE`); a thread already in another
/// apartment keeps it and the call goes on.
struct Com {
    initialized: bool,
}

impl Com {
    fn init() -> Com {
        // SAFETY: no reserved pointer; balanced by `Drop` on this thread.
        let hr = unsafe {
            CoInitializeEx(
                None,
                COINIT(COINIT_APARTMENTTHREADED.0 | COINIT_DISABLE_OLE1DDE.0),
            )
        };
        Com {
            initialized: hr.is_ok(),
        }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: balances the successful `CoInitializeEx` of this thread.
            unsafe { CoUninitialize() };
        }
    }
}
