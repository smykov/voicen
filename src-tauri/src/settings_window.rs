//! The settings window (T-037, invariant S1; contracts/ipc.md "Window").
//!
//! [`open`] is the only constructor of the window labelled [`LABEL`]: there is at
//! most one, and a call while it exists creates nothing, brings it forward and
//! emits `settings://focus` to it. [`on_ready`] is the startup executor: it carries
//! out `startup_action(outcome, launched_by_autostart)` and decides nothing itself.
//! `run()` calls it on `RunEvent::Ready`; the tests call it directly.
//!
//! The URL is `settings?tab=<token>[&field=<FieldId>]`. Both tokens come from closed
//! sets of `[a-z_.]` words (`SettingsTab::as_str`, `FieldId::as_str`), so they are
//! put into the URL unencoded. `tauri.conf.json` declares no window, and the
//! capability in `capabilities/default.json` is granted to [`LABEL`] only.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};
use voicen_core::settings::gate::{startup_action, SettingsTab, StartupAction};
use voicen_core::settings::{FieldId, LoadOutcome};

/// The label of the one settings window; the capability is granted to it only.
pub const LABEL: &str = "settings";

/// The event emitted to the open window by a later [`open`] (contracts/ipc.md).
pub const FOCUS_EVENT: &str = "settings://focus";

/// The window title and its initial size (the values of the former config window).
const TITLE: &str = "Voicen";
const WIDTH: f64 = 800.0;
const HEIGHT: f64 = 600.0;

/// The `settings://focus` payload `{tab, field?}`: `field` is omitted when `None`.
#[derive(Debug, Clone, Serialize)]
struct FocusRequest {
    tab: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<FieldId>,
}

/// The window's app URL: `settings?tab=<token>[&field=<FieldId>]`.
fn url(tab: SettingsTab, field: Option<FieldId>) -> String {
    match field {
        Some(field) => format!("settings?tab={}&field={}", tab.as_str(), field.as_str()),
        None => format!("settings?tab={}", tab.as_str()),
    }
}

/// Opens the settings window on `tab` (and `field`). If it is already open, it is
/// unminimized, shown and focused, and `settings://focus {tab, field?}` is emitted
/// to it; its URL is left as it is. The first error is returned.
pub fn open<R: Runtime>(
    app: &AppHandle<R>,
    tab: SettingsTab,
    field: Option<FieldId>,
) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(LABEL) {
        window.unminimize()?;
        window.show()?;
        window.set_focus()?;
        return app.emit_to(
            LABEL,
            FOCUS_EVENT,
            FocusRequest {
                tab: tab.as_str(),
                field,
            },
        );
    }
    WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(url(tab, field).into()))
        .title(TITLE)
        .inner_size(WIDTH, HEIGHT)
        .build()
        .map(|_| ())
}

/// The startup executor: carries out `startup_action(outcome, launched_by_autostart)`,
/// that is [`open`] on the decided tab with no field, or nothing for `TrayOnly`.
pub fn on_ready<R: Runtime>(
    app: &AppHandle<R>,
    outcome: &LoadOutcome,
    launched_by_autostart: bool,
) -> tauri::Result<()> {
    match startup_action(outcome, launched_by_autostart) {
        StartupAction::OpenSettings(tab) => open(app, tab, None),
        StartupAction::TrayOnly => Ok(()),
    }
}
