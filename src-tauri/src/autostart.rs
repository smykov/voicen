//! The Windows implementation of the autostart port over the per-user Run key (T-014;
//! spec 004 FR-019, research R-5; contracts/core-traits.md#autostart). The only code
//! that writes or removes the `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
//! value; only `SettingsService` calls it (save step and `reconcile_autostart`).
//!
//! - Value name [`RUN_VALUE_NAME`] (`Voicen`), the one constant; 006's uninstaller
//!   hook deletes the same literal.
//! - `set(true)` always writes `REG_SZ` `"<current exe>" --autostart` (idempotent,
//!   fixes a stale path); `set(false)` deletes, an absent value is `Ok`;
//!   `is_enabled` = the value exists, whatever its data.
//! - Any failure: `AutostartError` with the Win32 code. The exe path (it holds the
//!   Windows user name) is never put in an error or a log.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_INVALID_DATA, ERROR_INVALID_PARAMETER, ERROR_SUCCESS, WIN32_ERROR,
};
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_ANY,
};

use voicen_core::autostart::{Autostart, AutostartError};

/// The release value name under the Run key.
pub const RUN_VALUE_NAME: &str = "Voicen";

/// The argument the Run value passes; read back by [`launched_by_autostart`].
pub const AUTOSTART_ARG: &str = "--autostart";

/// The Run key, under `HKEY_CURRENT_USER`.
pub const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// `true` when the process was started by the Run value (`--autostart` among the
/// arguments). `settings_window::on_ready` (T-037) passes it to `startup_action`.
pub fn launched_by_autostart<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter()
        .any(|arg| arg.as_ref() == OsStr::new(AUTOSTART_ARG))
}

/// Run-key autostart. Value name = [`RUN_VALUE_NAME`] for the release app.
pub struct WinAutostart {
    value_name: String,
}

impl WinAutostart {
    /// The release entry: value name exactly [`RUN_VALUE_NAME`].
    pub fn new() -> WinAutostart {
        WinAutostart::with_value_name(RUN_VALUE_NAME)
    }

    /// An entry under another value name (tests: never the user's real `Voicen`
    /// value). Same calls as the release entry.
    pub fn with_value_name(name: impl Into<String>) -> WinAutostart {
        WinAutostart {
            value_name: name.into(),
        }
    }
}

impl Default for WinAutostart {
    fn default() -> Self {
        WinAutostart::new()
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn autostart_error(code: WIN32_ERROR) -> AutostartError {
    AutostartError {
        os_code: code.0 as i32,
    }
}

/// `"<current exe>" --autostart` as NUL-terminated UTF-16 (`REG_SZ` data). The path
/// is taken as the OS gives it (no lossy conversion) and never leaves this function
/// except as the registry data.
fn run_command() -> Result<Vec<u16>, AutostartError> {
    let exe = std::env::current_exe().map_err(|err| AutostartError {
        os_code: err.raw_os_error().unwrap_or(ERROR_INVALID_DATA.0 as i32),
    })?;
    let mut command: Vec<u16> = Vec::new();
    command.extend("\"".encode_utf16());
    command.extend(exe.as_os_str().encode_wide());
    command.extend("\" ".encode_utf16());
    command.extend(AUTOSTART_ARG.encode_utf16());
    command.push(0);
    Ok(command)
}

impl WinAutostart {
    /// (Re)writes the value with the current command.
    fn write(&self) -> Result<(), AutostartError> {
        let subkey = wide(RUN_SUBKEY);
        let value = wide(&self.value_name);
        let data = run_command()?;
        // The terminating NUL is counted (REG_SZ).
        let cbdata = data
            .len()
            .checked_mul(std::mem::size_of::<u16>())
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| autostart_error(ERROR_INVALID_PARAMETER))?;
        // SAFETY: `subkey`, `value` and `data` are NUL-terminated, outlive the call,
        // and `cbdata` is the byte length of `data`.
        let written = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                PCWSTR(value.as_ptr()),
                REG_SZ.0,
                Some(data.as_ptr().cast::<core::ffi::c_void>()),
                cbdata,
            )
        };
        if written == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(autostart_error(written))
        }
    }

    /// Deletes the value; an absent value (or Run key) is `Ok`.
    fn remove(&self) -> Result<(), AutostartError> {
        let subkey = wide(RUN_SUBKEY);
        let value = wide(&self.value_name);
        // SAFETY: `subkey` and `value` are NUL-terminated and outlive the call.
        let deleted = unsafe {
            RegDeleteKeyValueW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                PCWSTR(value.as_ptr()),
            )
        };
        if deleted == ERROR_SUCCESS || deleted == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(autostart_error(deleted))
        }
    }
}

impl Autostart for WinAutostart {
    fn is_enabled(&self) -> Result<bool, AutostartError> {
        let subkey = wide(RUN_SUBKEY);
        let value = wide(&self.value_name);
        let mut size = 0u32;
        // SAFETY: `subkey` and `value` are NUL-terminated and outlive the call; no
        // data buffer is passed, only the size out-parameter, which outlives it.
        let queried = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                PCWSTR(value.as_ptr()),
                RRF_RT_ANY,
                None,
                None,
                Some(&mut size as *mut u32),
            )
        };
        if queried == ERROR_SUCCESS {
            Ok(true)
        } else if queried == ERROR_FILE_NOT_FOUND {
            Ok(false)
        } else {
            Err(autostart_error(queried))
        }
    }

    fn set(&self, enabled: bool) -> Result<(), AutostartError> {
        if enabled {
            self.write()
        } else {
            self.remove()
        }
    }
}
