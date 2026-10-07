//! The tray's "Open logs folder" (T-071; spec 006 FR-017; FR-20).
//!
//! The one path that opens the logs folder: `tray::on_menu_event` (`OpenLogs`) ->
//! [`request`]. `request` never does the work on the calling (main) thread: it
//! hands it to a short-lived named thread, because the Explorer launch can block
//! on shell extensions. That thread creates the installed dir when it is missing
//! (`create_dir_all`), hands it to the injected [`FolderOpener`] port only when it
//! is a directory (verb "open" on a file would execute it), and ends every failure
//! (thread spawn, create, not a directory, open) in exactly one
//! `warning kind=logs_folder_failed [os_code=N]` line: the kind and the OS code
//! only, never the path or the OS text (#45). The app keeps running.
//!
//! `run()` installs `paths::log_dir()` with the Win32 `win::shell_open::ShellOpener`;
//! tests install a fake opener over a temp dir. Without [`install`] a click does
//! nothing. When the logs dir itself is unwritable the log is degraded and drops
//! the warning about it (docs/decisions/diagnostics-log.md).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::{AppHandle, Manager, Runtime};
use voicen_core::diag::{Log, LogEvent, WarningKind};

/// The port that shows a folder to the user (the Explorer on Windows). Called off
/// the main thread, with a path that was a directory just before the call; it may
/// block until the launch is done. It never creates the folder.
pub trait FolderOpener: Send + Sync {
    /// Opens `dir`; `Err` with the OS code when it could not.
    fn open(&self, dir: &Path) -> io::Result<()>;
}

/// The managed logs folder of an app: the dir to open, the opener and the log
/// for the failure line (absent when the app manages no log).
struct LogsFolder {
    dir: PathBuf,
    opener: Arc<dyn FolderOpener>,
    log: Option<Arc<Log>>,
}

/// Manages the logs folder `dir` with `opener` on `app`, and the app's managed
/// `Arc<Log>` (if any) for the failure line. A second call on the same app
/// changes nothing (tauri keeps the first managed value).
pub fn install<R: Runtime>(app: &AppHandle<R>, dir: PathBuf, opener: Arc<dyn FolderOpener>) {
    let log = app.try_state::<Arc<Log>>().map(|log| Arc::clone(&log));
    app.manage(LogsFolder { dir, opener, log });
}

/// Opens the installed logs folder on a new thread named `logs-folder` and
/// returns at once; does nothing when nothing is installed. A failed spawn writes
/// the one `logs_folder_failed` line here.
pub fn request<R: Runtime>(app: &AppHandle<R>) {
    let Some(folder) = app.try_state::<LogsFolder>() else {
        return;
    };
    let dir = folder.dir.clone();
    let opener = Arc::clone(&folder.opener);
    let log = folder.log.clone();
    let spawn_log = log.clone();
    let spawned = std::thread::Builder::new()
        .name("logs-folder".into())
        .spawn(move || {
            if let Err(err) = open(&dir, opener.as_ref()) {
                warn(log.as_deref(), &err);
            }
        });
    if let Err(err) = spawned {
        warn(spawn_log.as_deref(), &err);
    }
}

/// Creates `dir` when missing, checks it is a directory and opens it.
fn open(dir: &Path, opener: &dyn FolderOpener) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    if !dir.is_dir() {
        return Err(io::Error::other("the logs path is not a directory"));
    }
    opener.open(dir)
}

/// The one failure line: the kind and the OS code, nothing else.
fn warn(log: Option<&Log>, err: &io::Error) {
    if let Some(log) = log {
        log.write(LogEvent::Warning {
            kind: WarningKind::LogsFolderFailed,
            os_code: err.raw_os_error(),
        });
    }
}
