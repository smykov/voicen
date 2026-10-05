//! The settings window (T-037, invariant S1; contracts/ipc.md "Window").
//!
//! [`open`] is the only constructor of the window labelled [`LABEL`]: there is at
//! most one, and a call while it exists creates nothing, brings it forward and
//! emits `settings://focus` to it. [`on_ready`] is the startup executor: it carries
//! out `startup_action(outcome, launched_by_autostart)` and decides nothing itself.
//! `run()`'s loop callback (`on_run_event`) calls it on `RunEvent::Ready`; the tests
//! call it directly.
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

use std::panic::AssertUnwindSafe;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use voicen_core::diag::{Log, LogEvent, WarningKind};
use voicen_core::settings::gate::{startup_action, SettingsTab, StartupAction};
use voicen_core::settings::{FieldId, LoadOutcome};

use crate::diag::io_os_code;

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
        bring_forward(&window)?;
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

/// Unminimizes, shows and focuses the open window (the first error is returned).
fn bring_forward<R: Runtime>(window: &WebviewWindow<R>) -> tauri::Result<()> {
    window.unminimize()?;
    window.show()?;
    window.set_focus()
}

/// [`OpenTarget::Front`]: the open window forward on the tab it shows (no
/// `settings://focus`), or [`open`] on Engine when none is open. Runs on the opener
/// thread only, so the window cannot appear between the check and `open`.
fn front<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    match app.get_webview_window(LABEL) {
        Some(window) => bring_forward(&window),
        None => open(app, SettingsTab::Engine, None),
    }
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
    /// if it has not run by then. A request that can never run (no opener thread:
    /// the app was not wired by `assemble`, or the thread could not be started,
    /// which was logged once as `settings_opener_failed`) or whose open panicked
    /// (logged as `settings_window_failed` without an OS code) is
    /// `Some(Err(FailedToReceiveMessage))`.
    pub fn wait_timeout(self, timeout: Duration) -> Option<tauri::Result<()>> {
        match self.done.recv_timeout(timeout) {
            Ok(result) => Some(result),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Err(tauri::Error::FailedToReceiveMessage)),
        }
    }
}

/// One request on the opener's queue. The handle travels with the request, so the
/// opener thread holds no app handle of its own between requests.
struct Job<R: Runtime> {
    app: AppHandle<R>,
    target: OpenTarget,
    done: mpsc::Sender<tauri::Result<()>>,
}

/// The managed queue of the one opener thread of an app.
struct Opener<R: Runtime> {
    jobs: mpsc::Sender<Job<R>>,
}

/// Starts the one opener thread ("settings-window") of `app` and manages its queue.
/// Called once, by `assemble`'s wiring, after tauri's `build()`. The thread runs the
/// requests one at a time, in posting order, and writes a failed open to `log`; it
/// ends when the app's queue is dropped. A panic inside one request (an unwinding
/// build, as in the tests) is caught there: it writes `settings_window_failed`
/// without an OS code, that request's receipt reports that it did not complete
/// (`Some(Err(FailedToReceiveMessage))`) and the next request runs as usual. The
/// release profile has `panic = "abort"`, so there a panic ends the process and can
/// never leave a dead opener behind. A failed spawn writes one
/// `settings_opener_failed` warning (the OS code only); every later request then
/// reports that it cannot run.
pub(crate) fn start_opener<R: Runtime>(app: &AppHandle<R>, log: Arc<Log>) {
    let (jobs, queue) = mpsc::channel::<Job<R>>();
    let thread_log = Arc::clone(&log);
    let spawned = std::thread::Builder::new()
        .name("settings-window".into())
        .spawn(move || {
            for job in queue {
                let Job { app, target, done } = job;
                let run = std::panic::catch_unwind(AssertUnwindSafe(|| match target {
                    OpenTarget::Tab(tab, field) => open(&app, tab, field),
                    OpenTarget::Front => front(&app),
                }));
                // The kind and the OS code only, no error text (#45).
                match run {
                    Ok(result) => {
                        if let Err(err) = &result {
                            thread_log.write(LogEvent::Warning {
                                kind: WarningKind::SettingsWindowFailed,
                                os_code: io_os_code(err),
                            });
                        }
                        let _ = done.send(result);
                    }
                    // `done` is dropped unanswered: the receipt reports the request
                    // as not completed.
                    Err(_panic) => thread_log.write(LogEvent::Warning {
                        kind: WarningKind::SettingsWindowFailed,
                        os_code: None,
                    }),
                }
            }
        });
    match spawned {
        Ok(_) => {
            app.manage(Opener { jobs });
        }
        Err(err) => log.write(LogEvent::Warning {
            kind: WarningKind::SettingsOpenerFailed,
            os_code: err.raw_os_error(),
        }),
    }
}

/// Posts `target` to the one opener thread and returns at once. The opener runs the
/// requests one at a time, in the order they were posted (FIFO), so no two runs of
/// [`open`] overlap; a failed one writes `warning kind=settings_window_failed`
/// (`os_code=` for an I/O error) and the next request runs as usual, also after a
/// panicking one where panics unwind (see `start_opener`). Never waits for
/// the opener or the main thread, so it may be called from any thread, the main
/// thread and window procedures included.
pub fn request<R: Runtime>(app: &AppHandle<R>, target: OpenTarget) -> Receipt {
    let (done, receipt) = mpsc::channel();
    if let Some(opener) = app.try_state::<Opener<R>>() {
        // A refused send drops the job and its sender: the receipt then reports that
        // the request cannot run.
        let _ = opener.jobs.send(Job {
            app: app.clone(),
            target,
            done,
        });
    }
    Receipt { done: receipt }
}

/// The startup executor: carries out `startup_action(outcome, launched_by_autostart)`,
/// that is a [`request`] for [`OpenTarget::Tab`] on the decided tab with no field
/// (its receipt returned), or nothing for `TrayOnly` (`None`: nothing is posted).
pub fn on_ready<R: Runtime>(
    app: &AppHandle<R>,
    outcome: &LoadOutcome,
    launched_by_autostart: bool,
) -> Option<Receipt> {
    match startup_action(outcome, launched_by_autostart) {
        StartupAction::OpenSettings(tab) => Some(request(app, OpenTarget::Tab(tab, None))),
        StartupAction::TrayOnly => None,
    }
}
