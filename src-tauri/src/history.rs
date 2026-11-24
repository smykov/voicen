//! The transcript history's shell part (T-022; spec 005 contracts/ipc.md "Shell hooks";
//! FR-16, NFR-06). The list itself, its file and every rule live in core
//! (`voicen_core::history`); this module only wires them to the app.
//!
//! - [`start_history`] is the one function `run()` uses to build the app's only
//!   `HistoryRecorder`: `History::open` over `FileHistoryStore` in the directory it is
//!   given (`run()` passes `paths::data_dir()`, the single resolver), with the history
//!   settings of the managed `SettingsService`'s snapshot, read after the follower's own
//!   `subscribe()` so no saved change is missed. It starts the settings follower thread
//!   ("history-settings", core's `follow_settings`) and the History window's opener
//!   thread ("history-window"), then manages the part. A load error of the file and
//!   every later disk error are one `history_failed` line (the op and a closed kind,
//!   never text); a start that fails after the managed `Log` is found is one
//!   `warning kind=history_start_failed [os_code=<n>]` line (no path, no error text).
//!   Sibling of `call::start_calls`, outside `build_app`: an app `build_app` built has no
//!   history part, so nothing is recorded there and the commands answer an empty list.
//! - The pipeline's `TranscriptSink` is the caller's required choice
//!   (`dictation::DictationPorts::history`): `HistorySink::Managed` takes the managed
//!   recorder ([`managed_sink`]; no part is a refused dictation start, never a silent
//!   keep-nothing sink), `HistorySink::Discard` the explicit keep-nothing sink
//!   ([`discard_sink`]). `run()` starts the history before the dictation and passes
//!   `Managed` when the start succeeded, `Discard` after its logged failure.
//! - Each change of the view is emitted as `history://changed` to the window labelled
//!   [`LABEL`] only (`emit_to`), in camelCase ([`HistoryWire`]).
//! - The History window: at most one window labelled [`LABEL`] at `/history`, built only
//!   on the opener thread ([`open_history`], the tray "History", posts and returns at
//!   once; the `call::open_calls` pattern). A second open brings it forward; closing it
//!   destroys it.
//! - The commands `history_get`, `history_copy` and `history_clear` (granted to
//!   [`LABEL`] only, `capabilities/history.json`): memory reads, a copy through the
//!   process's one clipboard writer (`Clipboard::set_text_excluded_from_history`, the
//!   FR-10 exclusion formats), a clear. Their errors carry a code only, never text.

use std::io;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use voicen_core::clock::SystemClock;
use voicen_core::diag::{Log, LogEvent, WarningKind};
use voicen_core::history::{
    follow_settings, size_of, EngineLabel, FileHistoryStore, History, HistoryNotice,
    HistoryRecorder, HistoryView,
};
use voicen_core::pipeline::TranscriptSink;
use voicen_core::platform::Clipboard;
use voicen_core::settings::service::SettingsService;

/// The label of the one History window; `capabilities/history.json` is granted to it only.
pub const LABEL: &str = "history";

/// Emitted to the History window after every change of the view (contracts/ipc.md "Event").
pub const CHANGED_EVENT: &str = "history://changed";

/// The window title and its initial size.
const TITLE: &str = "Voicen";
const WIDTH: f64 = 640.0;
const HEIGHT: f64 = 600.0;

/// The app's recorder type.
type Recorder = HistoryRecorder<FileHistoryStore>;

/// The history part of an app: the one recorder and the clipboard writer of its copy.
struct HistoryPart {
    recorder: Arc<Recorder>,
    clipboard: Arc<dyn Clipboard>,
}

/// One entry on the wire (contracts/ipc.md "Wire types", camelCase).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntryWire {
    pub id: u64,
    pub text: String,
    pub ready_at_ms: i64,
    pub engine: EngineLabel,
}

/// `HistoryView` on the wire (contracts/ipc.md "Wire types", camelCase).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryWire {
    pub enabled: bool,
    pub entries: Vec<HistoryEntryWire>,
    pub notice: Option<HistoryNotice>,
}

impl From<HistoryView> for HistoryWire {
    fn from(view: HistoryView) -> HistoryWire {
        HistoryWire {
            enabled: view.enabled,
            entries: view
                .entries
                .into_iter()
                .map(|entry| HistoryEntryWire {
                    id: entry.id,
                    text: entry.text,
                    ready_at_ms: entry.ready_at_ms,
                    engine: entry.engine,
                })
                .collect(),
            notice: view.notice,
        }
    }
}

/// The error of `history_copy`: a code only (contracts/ipc.md "Commands").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HistoryCopyError {
    pub code: &'static str,
}

impl HistoryCopyError {
    fn not_found() -> HistoryCopyError {
        HistoryCopyError { code: "not_found" }
    }

    fn clipboard_unavailable() -> HistoryCopyError {
        HistoryCopyError {
            code: "clipboard_unavailable",
        }
    }
}

/// Builds the one history of `app` over `dir` (see the module docs), starts its two
/// threads and manages it. Needs the managed `Arc<SettingsService>` and `Arc<Log>`
/// (`assemble`'s wiring). `Err` when a managed part is missing, the app already has a
/// history part, or a thread could not be started; then nothing is managed and the app
/// records nothing. Every `Err` after the managed `Log` is found writes one
/// `warning kind=history_start_failed` line, with the OS code when the error has one.
pub fn start_history<R: Runtime>(
    app: &AppHandle<R>,
    dir: PathBuf,
    clipboard: Arc<dyn Clipboard>,
) -> io::Result<()> {
    let log = app
        .try_state::<Arc<Log>>()
        .map(|log| Arc::clone(&log))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the app manages no log"))?;
    start_logged(app, dir, clipboard, &log).inspect_err(|err| {
        // The kind and the OS code only: no path, no error text (P-009).
        log.write(LogEvent::Warning {
            kind: WarningKind::HistoryStartFailed,
            os_code: err.raw_os_error(),
        });
    })
}

/// [`start_history`] after the `Log` is found; its every `Err` is logged by the caller.
fn start_logged<R: Runtime>(
    app: &AppHandle<R>,
    dir: PathBuf,
    clipboard: Arc<dyn Clipboard>,
    log: &Arc<Log>,
) -> io::Result<()> {
    let service = app
        .try_state::<Arc<SettingsService>>()
        .map(|service| Arc::clone(&service))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "the app manages no settings service",
            )
        })?;
    if app.try_state::<HistoryPart>().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "the app already has a history part",
        ));
    }
    // The follower's own subscription first, then the snapshot: a save between the two
    // reaches the follower, so none is missed (the tray language follower's order).
    let changes = service.subscribe();
    let settings = service.snapshot();
    let (history, warning) = History::open(
        FileHistoryStore::new(dir),
        settings.history.enabled,
        size_of(settings.history.size),
    );
    if let Some(error) = warning {
        log.write(LogEvent::history_failed(&error));
    }
    let emit_app = app.clone();
    let error_log = Arc::clone(log);
    let recorder = Arc::new(
        HistoryRecorder::new(
            history,
            Arc::new(SystemClock),
            Box::new(move |view| {
                // Nothing hears it while no History window exists.
                let _ = emit_app.emit_to(LABEL, CHANGED_EVENT, HistoryWire::from(view));
            }),
        )
        .with_errors(Box::new(move |error| {
            error_log.write(LogEvent::history_failed(error));
        })),
    );
    let follower = Arc::clone(&recorder);
    thread::Builder::new()
        .name("history-settings".into())
        .spawn(move || follow_settings(follower, changes))
        .map(|_| ())?;
    let opener = spawn_opener::<R>()?;
    app.manage(opener);
    app.manage(HistoryPart {
        recorder,
        clipboard,
    });
    Ok(())
}

/// A sink that keeps nothing: the explicit `HistorySink::Discard` choice.
struct NoHistory;

impl TranscriptSink for NoHistory {
    fn delivered(&self, _text: &str, _engine: EngineLabel) {}
}

/// The managed recorder of `app` as a `TranscriptSink`, or `None` when [`start_history`]
/// did not run or failed (`HistorySink::Managed` then refuses the dictation start).
pub(crate) fn managed_sink<R: Runtime>(app: &AppHandle<R>) -> Option<Arc<dyn TranscriptSink>> {
    app.try_state::<HistoryPart>()
        .map(|part| Arc::clone(&part.recorder) as Arc<dyn TranscriptSink>)
}

/// The keep-nothing sink of `HistorySink::Discard`.
pub(crate) fn discard_sink() -> Arc<dyn TranscriptSink> {
    Arc::new(NoHistory)
}

/// The `TranscriptSink` of `app` for a caller outside the dictation wiring (the tests):
/// the managed recorder, or the keep-nothing sink when [`start_history`] did not run.
/// The dictation does not use it: its sink is `DictationPorts::history`.
pub fn transcript_sink<R: Runtime>(app: &AppHandle<R>) -> Arc<dyn TranscriptSink> {
    managed_sink(app).unwrap_or_else(discard_sink)
}

/// Opens the History window (the tray "History"): posts the request to the opener
/// thread and returns at once; does nothing on an app without a history part.
pub fn open_history<R: Runtime>(app: &AppHandle<R>) {
    if let Some(opener) = app.try_state::<HistoryOpener<R>>() {
        // A refused send (the opener thread is gone) drops the request.
        let _ = opener.jobs.send(app.clone());
    }
}

/// The managed queue of the History window's one opener thread. Each request carries
/// the handle, so the thread holds none between requests.
struct HistoryOpener<R: Runtime> {
    jobs: Sender<AppHandle<R>>,
}

/// Starts the "history-window" opener thread: it runs the posted opens one at a time,
/// in posting order, so the label check and the window build never race. Ends when the
/// app's queue is dropped. A panic inside one open (an unwinding build, as in the tests)
/// is caught and the next open runs; the release profile aborts on panic.
fn spawn_opener<R: Runtime>() -> io::Result<HistoryOpener<R>> {
    let (jobs, queue) = mpsc::channel::<AppHandle<R>>();
    thread::Builder::new()
        .name("history-window".into())
        .spawn(move || {
            for app in queue {
                let _ = std::panic::catch_unwind(AssertUnwindSafe(|| open_window(&app)));
            }
        })
        .map(|_| HistoryOpener { jobs })
}

/// One open, on the opener thread only: an open window is brought forward, a missing
/// one is built at `/history`. A failed build leaves no window; the next open tries
/// again.
fn open_window<R: Runtime>(app: &AppHandle<R>) {
    match app.get_webview_window(LABEL) {
        Some(window) => {
            let _ = bring_forward(&window);
        }
        None => {
            let _ = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("history".into()))
                .title(TITLE)
                .inner_size(WIDTH, HEIGHT)
                .build();
        }
    }
}

/// Unminimizes, shows and focuses the open window (the first error is returned).
fn bring_forward<R: Runtime>(window: &WebviewWindow<R>) -> tauri::Result<()> {
    window.unminimize()?;
    window.show()?;
    window.set_focus()
}

/// The view of an app without a history part: no entries, `enabled` as the settings
/// say (nothing is recorded there).
fn empty_view<R: Runtime>(app: &AppHandle<R>) -> HistoryWire {
    HistoryWire {
        enabled: app
            .try_state::<Arc<SettingsService>>()
            .is_some_and(|service| service.snapshot().history.enabled),
        entries: Vec::new(),
        notice: None,
    }
}

/// `history_get` → `HistoryView` (FR-005, FR-012): memory only, never the disk.
#[tauri::command(async)]
pub fn history_get<R: Runtime>(app: AppHandle<R>) -> HistoryWire {
    match app.try_state::<HistoryPart>() {
        Some(part) => HistoryWire::from(part.recorder.view()),
        None => empty_view(&app),
    }
}

/// `history_copy { id }` → `null` (FR-006): the entry's full text, written once through
/// the process's one clipboard writer. `not_found` when `id` is not listed (or the app
/// has no history part), `clipboard_unavailable` when the write failed.
#[tauri::command(async)]
pub fn history_copy<R: Runtime>(app: AppHandle<R>, id: u64) -> Result<(), HistoryCopyError> {
    let part = app
        .try_state::<HistoryPart>()
        .ok_or_else(HistoryCopyError::not_found)?;
    let text = part
        .recorder
        .text_of(id)
        .ok_or_else(HistoryCopyError::not_found)?;
    part.clipboard
        .set_text_excluded_from_history(&text)
        .map_err(|_| HistoryCopyError::clipboard_unavailable())
}

/// `history_clear` → `HistoryView` after clearing (FR-007, FR-009); `notice` may be
/// `delete_failed`. Runs off the main thread: it deletes files.
#[tauri::command(async)]
pub fn history_clear<R: Runtime>(app: AppHandle<R>) -> HistoryWire {
    match app.try_state::<HistoryPart>() {
        Some(part) => HistoryWire::from(part.recorder.clear()),
        None => empty_view(&app),
    }
}
