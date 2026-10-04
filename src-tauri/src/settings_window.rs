//! The settings window (T-037, invariant S1; contracts/ipc.md "Window").
//!
//! RED STUB: only the signatures the tests in `src-tauri/tests/settings_window.rs`
//! call. `open` and `on_ready` do nothing yet; the developer implements them.

use tauri::{AppHandle, Runtime};
use voicen_core::settings::gate::SettingsTab;
use voicen_core::settings::{FieldId, LoadOutcome};

/// The label of the one settings window; the capability is granted to it only.
pub const LABEL: &str = "settings";

/// Opens the settings window on `tab` (and `field`), or focuses the open one and
/// emits `settings://focus` to it. RED STUB: does nothing.
pub fn open<R: Runtime>(
    _app: &AppHandle<R>,
    _tab: SettingsTab,
    _field: Option<FieldId>,
) -> tauri::Result<()> {
    Ok(())
}

/// The startup executor: carries out `startup_action(outcome, launched_by_autostart)`.
/// RED STUB: does nothing.
pub fn on_ready<R: Runtime>(
    _app: &AppHandle<R>,
    _outcome: &LoadOutcome,
    _launched_by_autostart: bool,
) -> tauri::Result<()> {
    Ok(())
}
