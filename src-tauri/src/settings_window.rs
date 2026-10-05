//! The settings window (T-037, invariant S1; contracts/ipc.md "Window").
//!
//! [`open`] is the only constructor of the window labelled [`LABEL`]: there is at
//! most one, and a call while it exists creates nothing, brings it forward and
//! emits `settings://focus` to it. [`on_ready`] is the startup executor: it carries
//! out `startup_action(outcome, launched_by_autostart)` and decides nothing itself.
//! `run()` calls it on `RunEvent::Ready`; the tests call it directly.
//!
//! T-052 invariant 2: at runtime `open` runs only on the one opener thread
//! ("settings-window"). Every caller (the startup executor, the tray "Settings"
//! item, the second-instance callback, T-006's session requests) posts with
//! [`request`] and never waits; tauri's label check is not atomic with the window
//! creation, so two concurrent `open`s could otherwise build two windows. Only
//! tests wait, on the [`Receipt`].
//!
//! The URL is `settings?tab=<token>[&field=<FieldId>]`. Both tokens come from closed
//! sets of `[a-z_.]` words (`SettingsTab::as_str`, `FieldId::as_str`), so they are
//! put into the URL unencoded. `tauri.conf.json` declares no window, and the
//! capability in `capabilities/default.json` is granted to [`LABEL`] only.

use std::sync::mpsc;
use std::time::Duration;

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

/// Where a [`request`] points the window (contracts/ipc.md "Window").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenTarget {
    /// [`open`] on `tab` and `field`: an open window is brought forward and gets
    /// `settings://focus {tab, field?}` (the startup executor; T-006's
    /// `ShellRequests::open_settings`).
    Tab(SettingsTab, Option<FieldId>),
    /// The open window to the front on the tab it shows (unminimized, shown,
    /// focused; no `settings://focus`), or [`open`] on Engine when none is open: the
    /// tray "Settings" item and a second launch (OQ-11 Q2 default).
    Front,
}

/// One posted [`request`]. Runtime callers drop it; tests wait on it.
pub struct Receipt {
    done: mpsc::Receiver<tauri::Result<()>>,
}

impl Receipt {
    /// Waits at most `timeout` for the opener thread to run the request: `Some(Ok)`
    /// once the window is open or fronted, `Some(Err)` with the open's error (also
    /// written to the log as one `warning kind=settings_window_failed` line), `None`
    /// if it has not run by then.
    pub fn wait_timeout(self, timeout: Duration) -> Option<tauri::Result<()>> {
        // Skeleton (T-052 red tests): not implemented yet.
        let _ = (self.done, timeout);
        todo!("T-052: Receipt::wait_timeout")
    }
}

/// Posts `target` to the one opener thread and returns at once. The opener runs the
/// requests one at a time, in the order they were posted (FIFO), so no two runs of
/// [`open`] overlap; a failed one writes `warning kind=settings_window_failed`
/// (`os_code=` for an I/O error) and the next request runs as usual. Never waits for
/// the opener or the main thread, so it may be called from any thread, the main
/// thread and window procedures included.
pub fn request<R: Runtime>(app: &AppHandle<R>, target: OpenTarget) -> Receipt {
    // Skeleton (T-052 red tests): not implemented yet.
    let _ = (app, target);
    todo!("T-052: settings_window::request")
}

/// The startup executor: carries out `startup_action(outcome, launched_by_autostart)`,
/// that is a [`request`] for [`OpenTarget::Tab`] on the decided tab with no field
/// (its receipt returned), or nothing for `TrayOnly` (`None`: nothing is posted).
pub fn on_ready<R: Runtime>(
    app: &AppHandle<R>,
    outcome: &LoadOutcome,
    launched_by_autostart: bool,
) -> Option<Receipt> {
    // Skeleton (T-052 red tests): not implemented yet.
    let _ = (
        app,
        outcome,
        launched_by_autostart,
        startup_action,
        StartupAction::TrayOnly,
    );
    todo!("T-052: on_ready through the opener")
}
