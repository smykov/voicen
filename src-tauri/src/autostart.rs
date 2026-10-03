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

use voicen_core::autostart::{Autostart, AutostartError};

/// The release value name under the Run key.
pub const RUN_VALUE_NAME: &str = "Voicen";

/// The argument the Run value passes; read back by [`launched_by_autostart`].
pub const AUTOSTART_ARG: &str = "--autostart";

/// The Run key, under `HKEY_CURRENT_USER`.
pub const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// `true` when the process was started by the Run value (`--autostart` among the
/// arguments). T-004's startup executor passes it to `startup_action`.
pub fn launched_by_autostart<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    // RED STUB (T-014 test-writer): the developer replaces this.
    let _ = args;
    false
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

impl Autostart for WinAutostart {
    fn is_enabled(&self) -> Result<bool, AutostartError> {
        // RED STUB (T-014 test-writer): the developer replaces this.
        let _ = &self.value_name;
        Ok(false)
    }

    fn set(&self, enabled: bool) -> Result<(), AutostartError> {
        // RED STUB (T-014 test-writer): the developer replaces this.
        let _ = (enabled, &self.value_name);
        Ok(())
    }
}
