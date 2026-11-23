//! The call wiring (T-128; spec 007 FR-001, FR-002, FR-003, FR-017, FR-018, FR-043;
//! decisions #197, #223, #277, #278): [`start_calls`] is the one function `run()` and the
//! tests use to build the only `CallSession` of an app. It is a sibling of
//! `dictation::start_dictation`, outside `build_app`: an app `build_app` built manages no
//! call part, and every entry below is then a no-op.
//!
//! - [`start_calls`] builds the one `Arc<CallStore>` (its default root is
//!   [`CallPorts::calls_root`]) and the one `Arc<CallSession>` over the managed
//!   `SettingsService` and `Log`, manages both (later call tasks reuse them), and starts
//!   two threads: the `call-requests` worker and the `call-tick` thread.
//! - Every shell start or stop (tray "Record" / "Stop", `--call record` on a first or a
//!   second launch) is one [`CallRequest`] posted with [`request`]; the worker alone calls
//!   `CallSession::toggle_record`, so the main thread and the single-instance callback
//!   never take the session lock, write a log line or open a capture. A Record request
//!   from the tray toggles (its intent is `!is_recording()`, read on the worker, never
//!   muda's mark, #223); from the switch it only starts. A refused start offers
//!   `refused_notice(part, reason)` to the shared notifier (core logs the lines).
//! - The `call-tick` thread waits, holding no session lock, until
//!   `CallSession::next_deadline` (measured on [`CallPorts::mono`]) or until the worker
//!   wakes it after a request, then calls `CallSession::tick` and reads the deadline
//!   again (#278). While no call records it waits for a request only.
//! - [`ShellCallIndicator`] forwards `set_call_recording` to the tray part and
//!   `set_state` / `set_tick` to the indicator part (`call_indicator::IndicatorPart`,
//!   T-130: store and notify only; the `call-indicator` thread does every window call).
//!   [`start_calls`] starts that part (`call_indicator::start`) before it builds the
//!   session. The indicator page's stop and dismiss are [`CallRequest::IndicatorStop`]
//!   and [`CallRequest::Dismiss`] on the worker (`CallSession::stop(StopReason::User)`,
//!   `CallSession::dismiss_warning`). [`ShellCallShell`] maps each `CallNotice` through core's
//!   `CallNotice::message` to the shared notifier and opens Settings on its Calls tab.
//! - The Calls window (T-142; contracts/ipc.md "Windows", FR-033, FR-034, FR-036): at
//!   most one window labelled [`LABEL`] at `/calls`, created only on the call part's own
//!   opener thread ("calls-window", the `settings_window` pattern, #223). [`open_calls`]
//!   (the tray "Calls") counts the request (`calls_opened`) and posts it, never waiting. An open with a [`CallRef`]
//!   reaches the page as exactly one `calls://open {root, id}`: on a window whose page has
//!   not yet listed (a window just built, whose listeners may not exist), the target is
//!   held and emitted by the window's first `calls_list`, which the page sends only after
//!   it registered its listeners; on a window that listed, it is emitted at once.
//! - The five commands `calls_list`, `calls_get`, `calls_copy_markdown`, `calls_export`
//!   and `calls_open_folder` read the managed `Arc<CallStore>` and `Arc<SettingsService>`
//!   with `try_state` (an app without a call part answers an empty list / `notFound`) and
//!   resolve `{root, id}` only through core's guard (`CallStore` methods, P-009). Copy
//!   writes `CallStore::export_bytes(Md)` through the one [`Clipboard`] port, the
//!   dictation's (P-011, R18); export takes its target only from the [`SaveDialog`] port
//!   (a cancel writes nothing) and writes only through `export_to`, the guard, the modal
//!   dialog and the write on `spawn_blocking`, never on a runtime worker; open folder hands the
//!   guarded folder (a damaged call's too) to the [`FolderOpener`] port.
//! - [`ShellCallShell::changed`] emits `calls://changed {root, id}` to the Calls window only
//!   (`emit_to`), after the session lock, once per start and once per stop.
//! - The pass worker (T-139): [`start_calls`] also builds and manages the one
//!   `Arc<PassQueue>` over the same store, its one `ApiPassRunner` reading the managed
//!   `SettingsService`'s snapshot per pass, [`CallPorts::credentials`] (the service's own
//!   key store) and [`CallPorts::detector`] (the pass half of `vad::load_shared`; `None`:
//!   core's energy fallback). Every folder a stop leaves ready-to-transcribe reaches it
//!   through `CallShell::call_ready` ([`ShellCallShell`] enqueues it); the pass's notices
//!   go to the shared notifier as the session's do ([`ShellPassEvents`]).
//! - [`open_transcript`] (T-139) emits `calls://open` at once to an open Calls window. The
//!   "Transcript ready" toast's Open button does not use it: it calls [`open_calls`] with
//!   the notice's root and id (T-213; decisions #312, #313), which builds or focuses the
//!   window and selects the call. Every `calls://open`, held or immediate, goes out through
//!   the one private `emit_open` (decisions #298).
//! - The app's end, sleep and the next start (T-145; spec 007 FR-007, FR-008, FR-017):
//!   the worker's first job is the one start-up `CallStore::recover` over the managed store,
//!   every `Recovery.queue` folder enqueued oldest first. The tray's Exit posts
//!   `CallRequest::Exit` ([`request_exit`]): with no call recording it ends the loop; with
//!   one, the [`ExitConfirm`] port asks on its own question thread (at most one), and a Yes
//!   posts `ExitConfirmed` (gate exiting, `stop(Exit)`, then the loop ends). A suspend posts
//!   `Sleep` (gate held, `stop(Sleep)`), a resume `Resumed` (held items delivered in stop
//!   order; any other request releases them too). `on_run_event`'s `RunEvent::Exit` arm
//!   calls [`finalize_on_exit`] first (gate exiting, `stop(SystemEnd)`). Every session
//!   notice and stop hand-off goes through the one `DeliveryGate` of [`ShellCallShell`].

use std::io;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::thread;

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, Manager, Runtime, Webview, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use voicen_core::call::events::refused_notice;
use voicen_core::call::final_pass::{ApiPassRunner, PassEvents, PassQueue};
use voicen_core::call::indicator::{IndicatorState, IndicatorTick};
use voicen_core::call::session::{
    CallDeps, CallIndicator, CallSession, CallSettingsPort, CallShell, SubtitleConsumer, Toggled,
    TrackFile, TrackOpener,
};
use voicen_core::call::storage::{
    export_to, list_wire, wire_root, CallDetail, CallError, CallListWire, Format,
};
use voicen_core::call::{
    CallId, CallNotice, CallSource, CallStore, StopReason, SwitchRefusal, Track, TrackSource,
};
use voicen_core::clock::{MonotonicClock, SystemClock};
use voicen_core::diag::{Log, LogEvent, WarningKind};
use voicen_core::i18n::{self, EXIT_RECORDING_QUESTION};
use voicen_core::local_models::download::DiskSpace;
use voicen_core::platform::{Clipboard, Notifier};
use voicen_core::secrets as key_store;
use voicen_core::settings::gate::{CallSwitch, LaunchArgs, SettingsTab};
use voicen_core::settings::roots::FsCallRoots;
use voicen_core::settings::service::SettingsService;
use voicen_core::settings::Settings;
use voicen_core::vad::SpeechSpans;

use crate::call_indicator::{self, IndicatorPart};
use crate::logs_folder::FolderOpener;
use crate::settings_window::{self, OpenTarget};
use crate::tray::{self, TrayPart};
use crate::win::audio_notify::{ComCheck, NotifyFactory};

/// The label of the one Calls window; `capabilities/calls.json` is granted to it only.
pub const LABEL: &str = "calls";

/// Emitted to the Calls window when a call changed state (contracts/ipc.md "Events").
pub const CHANGED_EVENT: &str = "calls://changed";

/// The window title and its initial size.
const TITLE: &str = "Voicen";
const WIDTH: f64 = 900.0;
const HEIGHT: f64 = 640.0;

/// Queued events per track channel: about 5 s of 10 ms capture blocks.
const CHANNEL_CAPACITY: usize = 512;

/// What the tests replace; `run()` passes the Windows adapters.
pub struct CallPorts {
    /// The two track captures: `win::track_capture::CpalTrackSource` in `run()`.
    pub source: Arc<dyn TrackSource + Send + Sync>,
    /// The free space of the call folder (FR-017).
    pub disk: Arc<dyn DiskSpace>,
    /// The session's monotonic clock, also the one the `call-tick` thread waits on.
    pub mono: Arc<dyn MonotonicClock>,
    /// Opens a track file inside the call folder ([`file_opener`] in `run()`).
    pub open_track: TrackOpener,
    /// The one notifier of the process (the dictation session's too).
    pub notifier: Arc<dyn Notifier>,
    /// The root in force while `calls.storage_root` is unset (`<data_dir>/calls` in `run()`).
    pub calls_root: PathBuf,
    /// The one clipboard writer of the process, the dictation's (`WinClipboard` in `run()`):
    /// `calls_copy_markdown` writes only through it (P-011, R18).
    pub clipboard: Arc<dyn Clipboard>,
    /// Shows a call folder (`win::shell_open::ShellOpener` in `run()`).
    pub folder: Arc<dyn FolderOpener>,
    /// The native save dialog of `calls_export` (`win::save_dialog::WinSaveDialog` in `run()`).
    pub save: Arc<dyn SaveDialog>,
    /// The key store the pass reads the `TranscriptionApi` key from: the `SettingsService`'s
    /// own instance in `run()` (`SharedPorts.credentials`).
    pub credentials: Arc<dyn key_store::CredentialStore>,
    /// The pass's primary detector: the pass half of `vad::load_shared` in `run()`; `None`
    /// makes core's energy detector decide (`vad_fallback cause=unavailable` per track).
    pub detector: Option<Arc<dyn SpeechSpans>>,
    /// The tray Exit question while a call records (T-145; `win::exit_dialog::WinExitConfirm`
    /// in `run()`).
    pub exit_confirm: Arc<dyn ExitConfirm>,
}

/// The Yes / No question of the tray's Exit while a call records (T-145, FR-008; the
/// [`SaveDialog`] pattern). Called on the call part's own short-lived question thread,
/// never on the main thread or the call worker; it may block until the user answers.
pub trait ExitConfirm: Send + Sync {
    /// Shows `text` (`exit.recording.question` in the UI language) and answers `true` for
    /// Yes; No, a closed box or a box that could not be shown is `false`.
    fn ask(&self, text: &str) -> bool;
}

/// The `{root, id}` of a call on the wire (`calls://open`, `calls://changed`): `root` is an
/// opaque root text `calls_list` returned, `id` the call's folder name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallRef {
    pub root: String,
    pub id: String,
}

/// The native save dialog of `calls_export` (research R18: in Rust, no dialog plugin).
/// Called off the main thread, after the guard; it may block until the user answers.
pub trait SaveDialog: Send + Sync {
    /// Asks where to save a `format` export, offering `suggested_name`: `Ok(Some(target))`
    /// for a picked full path, `Ok(None)` when the user cancelled, `Err` when the dialog
    /// could not be shown.
    fn pick(&self, suggested_name: &str, format: Format) -> io::Result<Option<PathBuf>>;
}

/// Emitted to the Calls window to select a call (contracts/ipc.md "Events"; T-139, T-142):
/// only through `emit_open`.
pub const CALLS_OPEN: &str = "calls://open";

/// One shell start or stop of a call, posted with [`request`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallRequest {
    /// From the tray: a toggle (stops a recording call); from the switch: a start.
    Record(CallSource),
    /// Stops the recording call (`call_stopped reason=user|hotkey`).
    Stop(CallSource),
    /// An unknown `--call` value: one `call_switch_refused reason=unknown_value`.
    RefuseSwitch,
    /// The tray's Exit (T-145): no call recording ends the loop; a recording one asks
    /// [`ExitConfirm`] on the question thread (at most one question at a time).
    Exit,
    /// The Yes of the exit question: the shell stops delivering, the call (if one still
    /// records) stops with `exit`, then the loop ends.
    ExitConfirmed,
    /// A suspend (`PBT_APMSUSPEND`): the shell holds its notices and hand-offs, then the
    /// recording call stops with `sleep`.
    Sleep,
    /// A resume (`PBT_APMRESUMEAUTOMATIC`): what the shell held is delivered in stop order.
    Resumed,
    /// The indicator's Stop (`call_indicator_stop`, T-130): `call_stopped reason=user`.
    IndicatorStop,
    /// The indicator's Dismiss of the warning with this id (`call_indicator_dismiss`).
    Dismiss(u32),
}

/// The call part of an app: the worker's queue, the delivery gate, the "Calls" route's
/// counter, the Calls window's state and the ports of its commands.
struct CallPart {
    requests: Sender<CallRequest>,
    gate: Arc<DeliveryGate>,
    opened: AtomicU32,
    window: Mutex<WindowState>,
    clipboard: Arc<dyn Clipboard>,
    folder: Arc<dyn FolderOpener>,
    save: Arc<dyn SaveDialog>,
}

/// What the Calls window's page is ready for, under one mutex shared by the opener thread
/// and `calls_list`.
#[derive(Default)]
struct WindowState {
    /// The page of the current window sent `calls_list`, so its listeners exist.
    listening: bool,
    /// The target of an open that came before the page listened; emitted by its first
    /// `calls_list`. A later open replaces it.
    pending: Option<CallRef>,
}

/// The `call-tick` thread's wake-up: set by the worker after each request, cleared by
/// the thread before it ticks; `closed` once the worker ended.
#[derive(Default)]
struct Wake {
    state: Mutex<WakeState>,
    changed: Condvar,
}

#[derive(Default)]
struct WakeState {
    woken: bool,
    closed: bool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Wake {
    fn wake(&self) {
        lock(&self.state).woken = true;
        self.changed.notify_all();
    }

    fn close(&self) {
        lock(&self.state).closed = true;
        self.changed.notify_all();
    }
}

/// Opens a plain file: the track opener of `run()`.
pub fn file_opener() -> TrackOpener {
    Arc::new(|path: &Path| {
        let file: TrackFile = Box::new(std::fs::File::create(path)?);
        Ok(file)
    })
}

/// Builds the one call session of `app` over `ports`, manages it as `Arc<CallSession>`,
/// its store as `Arc<CallStore>` and the one pass queue over that store as
/// `Arc<PassQueue>` (T-139), and starts the `call-requests` worker and the `call-tick`
/// thread; before the session it starts the indicator part (`call_indicator::start`:
/// its mailbox and the `call-indicator` thread, no window). Needs the managed `Arc<SettingsService>` and `Arc<Log>`
/// (`assemble`'s wiring) and, for the tray flag, the tray (`run()` calls this after
/// `start_dictation`, so the tray exists). Starts no call. `Err` when a managed part is
/// missing, the app already has a call part (then nothing is started) or a thread could
/// not be started. Every `Err` after the managed log is found has written one
/// `warning kind=call_start_failed [os_code=<n>]` line (T-220; the kind and the OS code
/// only); without a managed log nothing can be written.
pub fn start_calls<R: Runtime>(app: &AppHandle<R>, ports: CallPorts) -> io::Result<()> {
    let log = app
        .try_state::<Arc<Log>>()
        .map(|log| Arc::clone(&log))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the app manages no log"))?;
    let failed = |err: io::Error| {
        log.write(LogEvent::Warning {
            kind: WarningKind::CallStartFailed,
            os_code: err.raw_os_error(),
        });
        err
    };
    let service = app
        .try_state::<Arc<SettingsService>>()
        .map(|service| Arc::clone(&service))
        .ok_or_else(|| {
            failed(io::Error::new(
                io::ErrorKind::NotFound,
                "the app manages no settings service",
            ))
        })?;
    if app.try_state::<CallPart>().is_some()
        || app.try_state::<Arc<CallSession>>().is_some()
        || app.try_state::<Arc<PassQueue>>().is_some()
    {
        return Err(failed(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "the app already has a call part",
        )));
    }
    let CallPorts {
        source,
        disk,
        mono,
        open_track,
        notifier,
        calls_root,
        clipboard,
        folder,
        save,
        credentials,
        detector,
        exit_confirm,
    } = ports;
    let store = Arc::new(CallStore::new(
        Arc::new(FsCallRoots::new(calls_root)),
        Arc::clone(&mono),
    ));
    let snapshot = Arc::clone(&service);
    let runner = ApiPassRunner::new(
        Arc::new(move || snapshot.snapshot()),
        credentials,
        detector,
        Arc::new(ShellPassEvents::new(
            Arc::clone(&log),
            Arc::clone(&notifier),
        )),
        Arc::clone(&mono),
    );
    let queue = Arc::new(PassQueue::new(Arc::clone(&store), Arc::new(runner)));
    let gate = Arc::new(DeliveryGate::new(Arc::clone(&notifier), Arc::clone(&queue)));
    call_indicator::start(app, Arc::clone(&service), Arc::clone(&log));
    let session = Arc::new(CallSession::new(CallDeps {
        source,
        store: Arc::clone(&store),
        settings: Arc::clone(&service) as Arc<dyn CallSettingsPort>,
        indicator: Arc::new(ShellCallIndicator::new(app)),
        shell: Arc::new(ShellCallShell::new(app, Arc::clone(&gate))),
        log: Arc::clone(&log),
        clock: Arc::new(SystemClock),
        offset: Arc::new(crate::diag::OsLocalOffset),
        mono: Arc::clone(&mono),
        app_version: voicen_core::build_info().version.to_string(),
        open_track,
        channel_capacity: CHANNEL_CAPACITY,
        subtitles: Arc::new(NoSubtitles),
        devices: Arc::new(NotifyFactory),
        comms: Arc::new(ComCheck),
        disk,
    }));
    let wake = Arc::new(Wake::default());
    let (requests, received) = mpsc::channel();
    spawn_tick(Arc::downgrade(&session), Arc::clone(&wake), mono).map_err(failed)?;
    spawn_worker(
        Worker {
            app: app.clone(),
            session: Arc::clone(&session),
            store: Arc::clone(&store),
            service,
            queue: Arc::clone(&queue),
            gate: Arc::clone(&gate),
            log: Arc::clone(&log),
            notifier,
            exit_confirm,
            asking: Arc::new(AtomicBool::new(false)),
        },
        received,
        Arc::clone(&wake),
    )
    .inspect_err(|_| wake.close())
    .map_err(failed)?;
    let opener = spawn_opener::<R>().map_err(failed)?;
    app.manage(store);
    app.manage(queue);
    app.manage(session);
    app.manage(opener);
    app.manage(CallPart {
        requests,
        gate,
        opened: AtomicU32::new(0),
        window: Mutex::new(WindowState::default()),
        clipboard,
        folder,
        save,
    });
    Ok(())
}

/// What the `call-requests` worker serves with.
struct Worker<R: Runtime> {
    app: AppHandle<R>,
    session: Arc<CallSession>,
    store: Arc<CallStore>,
    service: Arc<SettingsService>,
    queue: Arc<PassQueue>,
    gate: Arc<DeliveryGate>,
    log: Arc<Log>,
    notifier: Arc<dyn Notifier>,
    exit_confirm: Arc<dyn ExitConfirm>,
    /// Set while the exit question is shown (D2: at most one at a time).
    asking: Arc<AtomicBool>,
}

/// The `call-requests` worker: the only caller of `CallSession::toggle_record` in the
/// shell. Its first job, before any request, is the one start-up recovery (T-145, I5):
/// `CallStore::recover` over the managed store and the current settings, then every
/// `Recovery.queue` folder to the pass queue, oldest first; `recovered` folders are only
/// listed. After each request it wakes the `call-tick` thread (a start makes a deadline).
/// Ends when the queue's sender is gone, and then closes the tick thread.
fn spawn_worker<R: Runtime>(
    worker: Worker<R>,
    queue: Receiver<CallRequest>,
    wake: Arc<Wake>,
) -> io::Result<()> {
    thread::Builder::new()
        .name("call-requests".into())
        .spawn(move || {
            worker.recover();
            for request in queue {
                worker.serve(request);
                wake.wake();
            }
            wake.close();
        })
        .map(|_| ())
}

impl<R: Runtime> Worker<R> {
    /// The start-up recovery (I5): the only `CallStore::recover` and the only start-up
    /// enqueue of the shell.
    fn recover(&self) {
        let settings = self.service.snapshot();
        let recovery = self.store.recover(&settings.calls, &self.log);
        for entry in &recovery.queue {
            self.queue.enqueue(&entry.root.join(entry.id.as_str()));
        }
    }

    /// One request on the worker. A refusal offers its notice here, after the session lock
    /// (the session's own lines name the part and the reason). Any request but a suspend
    /// first releases what the shell holds since a suspend (D1: a lost resume does not
    /// lose the notice).
    fn serve(&self, request: CallRequest) {
        let toggled = match request {
            CallRequest::Sleep => {
                self.gate.hold();
                self.session.stop(StopReason::Sleep);
                return;
            }
            CallRequest::Resumed => {
                self.gate.release();
                return;
            }
            CallRequest::Exit => {
                if self.session.is_recording() {
                    self.gate.release();
                    self.ask_exit();
                } else {
                    self.gate.exiting();
                    end_loop(&self.app);
                }
                return;
            }
            CallRequest::ExitConfirmed => {
                // I2: from the confirmed exit on nothing is offered or enqueued; the next
                // start queues the call (D3). A call already stopped just exits (D2).
                self.gate.exiting();
                self.session.stop(StopReason::Exit);
                end_loop(&self.app);
                return;
            }
            CallRequest::Record(CallSource::Switch) => {
                self.gate.release();
                self.session.toggle_record(true, CallSource::Switch)
            }
            CallRequest::Record(source) => {
                self.gate.release();
                self.session
                    .toggle_record(!self.session.is_recording(), source)
            }
            CallRequest::Stop(source) => {
                self.gate.release();
                self.session.toggle_record(false, source)
            }
            CallRequest::IndicatorStop => {
                self.gate.release();
                self.session.stop(StopReason::User);
                return;
            }
            CallRequest::Dismiss(id) => {
                self.gate.release();
                self.session.dismiss_warning(id);
                return;
            }
            CallRequest::RefuseSwitch => {
                self.gate.release();
                self.log.write(LogEvent::CallSwitchRefused {
                    reason: SwitchRefusal::UnknownValue,
                });
                return;
            }
        };
        if let Toggled::Refused { part, reason } = toggled {
            self.notifier
                .notify(refused_notice(part, reason), &[], None);
        }
    }

    /// Shows the exit question on its own short-lived thread (D2: neither the main thread
    /// nor this worker waits for it); a Yes posts `ExitConfirmed`. While one is shown, a
    /// further Exit asks nothing. A thread that cannot start asks nothing (the call goes on).
    fn ask_exit(&self) {
        if self.asking.swap(true, Ordering::SeqCst) {
            return;
        }
        let text = i18n::text(
            self.service.snapshot().ui_language,
            EXIT_RECORDING_QUESTION,
            &[],
        );
        let confirm = Arc::clone(&self.exit_confirm);
        let asking = Arc::clone(&self.asking);
        let app = self.app.clone();
        let spawned = thread::Builder::new()
            .name("call-exit-question".into())
            .spawn(move || {
                let yes = confirm.ask(&text);
                asking.store(false, Ordering::SeqCst);
                if yes {
                    request(&app, CallRequest::ExitConfirmed);
                }
            });
        if spawned.is_err() {
            self.asking.store(false, Ordering::SeqCst);
        }
    }
}

/// Ends the loop of `app` with code 0 from a main-thread task (the tray Exit's own end);
/// a loop that already ended runs no task, so nothing happens.
fn end_loop<R: Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || handle.exit(0));
}

/// The tray's Exit (T-145): posts `CallRequest::Exit` to the call worker and answers
/// `true`; `false` when the app has no call part or its worker is gone (the caller then
/// ends the loop itself).
pub fn request_exit<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<CallPart>()
        .is_some_and(|part| part.requests.send(CallRequest::Exit).is_ok())
}

/// The call part's end of the process (T-145, I1, I2; `on_run_event`'s `RunEvent::Exit`
/// arm, before the session end): the delivery gate turns to exiting (no notice, no pass
/// enqueue from here on; the next start queues the call), then a recording call stops with
/// `system_end` (EOS, sync, meta `ready-to-transcribe`). Runs on the main thread and takes
/// the session lock there: nothing inside the lock waits on the main thread. A no-op
/// without a call part, and a stop that finds no recording call does nothing.
pub fn finalize_on_exit<R: Runtime>(app: &AppHandle<R>) {
    let Some(part) = app.try_state::<CallPart>() else {
        return;
    };
    part.gate.exiting();
    if let Some(session) = app.try_state::<Arc<CallSession>>() {
        session.stop(StopReason::SystemEnd);
    }
}

/// The `call-tick` thread (#278): ticks the session no later than its `next_deadline`
/// (plus scheduling) and waits without a session lock in between; without a deadline it
/// waits for the worker's wake-up. Holds the session weakly: it ends once the session is
/// gone or the worker closed it.
fn spawn_tick(
    session: Weak<CallSession>,
    wake: Arc<Wake>,
    mono: Arc<dyn MonotonicClock>,
) -> io::Result<()> {
    thread::Builder::new()
        .name("call-tick".into())
        .spawn(move || loop {
            let Some(deadline) = session.upgrade().map(|s| s.next_deadline()) else {
                return;
            };
            {
                let mut state = lock(&wake.state);
                loop {
                    if state.closed {
                        return;
                    }
                    if state.woken {
                        break;
                    }
                    match deadline {
                        None => {
                            state = wake
                                .changed
                                .wait(state)
                                .unwrap_or_else(PoisonError::into_inner);
                        }
                        Some(at) => {
                            let now = mono.now();
                            if now >= at {
                                break;
                            }
                            state = wake
                                .changed
                                .wait_timeout(state, at - now)
                                .unwrap_or_else(PoisonError::into_inner)
                                .0;
                        }
                    }
                }
                state.woken = false;
            }
            match session.upgrade() {
                Some(session) => session.tick(),
                None => return,
            }
        })
        .map(|_| ())
}

/// Posts `request` to the call worker of `app` and returns at once; does nothing when
/// the app has no call part (`start_calls` was not called) or the worker is gone.
pub fn request<R: Runtime>(app: &AppHandle<R>, request: CallRequest) {
    if let Some(part) = app.try_state::<CallPart>() {
        let _ = part.requests.send(request);
    }
}

/// The first launch's `--call` switch (`run()` calls it after `start_calls`, so the tray
/// exists), and the second launch's (`tray::on_second_instance`): `record` posts
/// `Record(Switch)`, an unknown or missing value `RefuseSwitch`, no switch nothing.
pub fn apply_launch<R: Runtime>(app: &AppHandle<R>, args: &LaunchArgs) {
    match args.call {
        Some(CallSwitch::Record) => request(app, CallRequest::Record(CallSource::Switch)),
        Some(CallSwitch::Unknown) => request(app, CallRequest::RefuseSwitch),
        None => {}
    }
}

/// Opens the Calls window (the tray "Calls": `target` `None`), and with a `target` selects
/// that call through one `calls://open`. Counts the request, posts it to the opener thread
/// and returns at once; does nothing on an app without a call part.
pub fn open_calls<R: Runtime>(app: &AppHandle<R>, target: Option<CallRef>) {
    let Some(part) = app.try_state::<CallPart>() else {
        return;
    };
    part.opened.fetch_add(1, Ordering::SeqCst);
    if let Some(opener) = app.try_state::<CallsOpener<R>>() {
        // A refused send (the opener thread is gone) drops the request.
        let _ = opener.jobs.send(OpenJob {
            app: app.clone(),
            target,
        });
    }
}

/// One posted [`open_calls`]. The handle travels with the request, so the opener thread
/// holds no app handle of its own between requests.
struct OpenJob<R: Runtime> {
    app: AppHandle<R>,
    target: Option<CallRef>,
}

/// The managed queue of the Calls window's one opener thread.
struct CallsOpener<R: Runtime> {
    jobs: Sender<OpenJob<R>>,
}

/// Starts the "calls-window" opener thread: it runs the posted opens one at a time, in
/// posting order, so the label check and the window build never race. Ends when the
/// app's queue is dropped. A panic inside one open (an unwinding build, as in the tests)
/// is caught and the next open runs; the release profile aborts on panic.
fn spawn_opener<R: Runtime>() -> io::Result<CallsOpener<R>> {
    let (jobs, queue) = mpsc::channel::<OpenJob<R>>();
    thread::Builder::new()
        .name("calls-window".into())
        .spawn(move || {
            for OpenJob { app, target } in queue {
                let _ = std::panic::catch_unwind(AssertUnwindSafe(|| open_window(&app, target)));
            }
        })
        .map(|_| CallsOpener { jobs })
}

/// One open, on the opener thread only. An open window is brought forward; a missing one
/// is built at `/calls` with a fresh state (its page has not listed yet). The target is
/// emitted at once when the page listens, else held for its first `calls_list`. A failed
/// build leaves no window; the next open tries again.
fn open_window<R: Runtime>(app: &AppHandle<R>, target: Option<CallRef>) {
    let Some(part) = app.try_state::<CallPart>() else {
        return;
    };
    match app.get_webview_window(LABEL) {
        Some(window) => {
            let _ = bring_forward(&window);
            let Some(target) = target else {
                return;
            };
            let emit_now = {
                let mut state = lock(&part.window);
                if state.listening {
                    Some(target)
                } else {
                    state.pending = Some(target);
                    None
                }
            };
            if let Some(target) = emit_now {
                emit_open(app, target);
            }
        }
        None => {
            *lock(&part.window) = WindowState {
                listening: false,
                pending: target,
            };
            let _ = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("calls".into()))
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

/// How many times [`open_calls`] ran on `app`'s call part (0 without one); a test-read
/// accessor like `tray::applied`.
pub fn calls_opened<R: Runtime>(app: &AppHandle<R>) -> u32 {
    app.try_state::<CallPart>()
        .map_or(0, |part| part.opened.load(Ordering::SeqCst))
}

/// Asks the Calls window to show the call `id` under `root` (T-139, FR-030): emits one
/// `calls://open` with `{root: wire_root(root), id}` at once, through `emit_open`.
/// `root` is the one core's `TranscriptReady` notice carries, never the current settings'.
/// It neither opens the window nor waits for its page (that is [`open_calls`] with a
/// target).
pub fn open_transcript<R: Runtime>(app: &AppHandle<R>, root: &Path, id: &CallId) {
    emit_open(
        app,
        CallRef {
            root: wire_root(root),
            id: id.to_string(),
        },
    );
}

/// The one emit site of `calls://open` (decisions #298): [`open_transcript`], an open on a
/// listening Calls window and the first `calls_list` of a fresh one all deliver here, to
/// the Calls window only.
fn emit_open<R: Runtime>(app: &AppHandle<R>, target: CallRef) {
    let _ = app.emit_to(LABEL, CALLS_OPEN, target);
}

/// `run()`'s `CallIndicator` (called inside the call session lock): the call flag goes
/// to the tray part, the state and the tick to the indicator part (T-130); each only
/// stores and notifies.
pub struct ShellCallIndicator<R: Runtime> {
    tray: Option<TrayPart<R>>,
    indicator: Option<Arc<IndicatorPart>>,
}

impl<R: Runtime> ShellCallIndicator<R> {
    /// The indicator of `app`: its tray part (`tray::part`), if the tray was built, and
    /// its indicator part (`call_indicator::part`), if `call_indicator::start` ran.
    pub fn new(app: &AppHandle<R>) -> ShellCallIndicator<R> {
        ShellCallIndicator {
            tray: tray::part(app),
            indicator: call_indicator::part(app),
        }
    }
}

impl<R: Runtime> CallIndicator for ShellCallIndicator<R> {
    /// Stores `state` in the indicator mailbox and wakes its thread; never blocks.
    fn set_state(&self, state: &IndicatorState) {
        if let Some(indicator) = &self.indicator {
            indicator.set_state(state);
        }
    }

    /// Stores `tick` in the indicator mailbox and wakes its thread; never blocks.
    fn set_tick(&self, tick: &IndicatorTick) {
        if let Some(indicator) = &self.indicator {
            indicator.set_tick(tick);
        }
    }

    fn set_call_recording(&self, recording: bool) {
        if let Some(tray) = &self.tray {
            tray.set_call_recording(recording);
        }
    }
}

/// A call notice to the shared notifier: core's `CallNotice::message` id and its button,
/// `CallNotice::action` (Open for "Transcript ready" only, T-213).
fn offer(notifier: &dyn Notifier, notice: &CallNotice) {
    let (id, params) = notice.message();
    notifier.notify(id, &params, notice.action());
}

/// What the delivery gate holds since a suspend, in stop order.
enum Held {
    Notice(CallNotice),
    Ready(PathBuf),
}

/// The delivery state of the call shell (T-145, I2, I4).
enum Delivery {
    /// Every notice is offered and every ready folder enqueued at once.
    Normal,
    /// Since a suspend: kept in order until a resume or the next call request.
    Held(Vec<Held>),
    /// Since the exit began: dropped (the next start queues the call through recover).
    Exiting,
}

/// The one place the call session's notices and stop hand-offs are delivered (T-145 seam
/// (c)): a notice to the shared notifier ([`offer`]), a ready-to-transcribe folder to the
/// one pass queue (T-139), each through the current [`Delivery`]. Delivery runs under the
/// gate's own lock (never the session lock), so a release keeps stop order. Exiting is
/// final.
struct DeliveryGate {
    state: Mutex<Delivery>,
    notifier: Arc<dyn Notifier>,
    queue: Arc<PassQueue>,
}

impl DeliveryGate {
    fn new(notifier: Arc<dyn Notifier>, queue: Arc<PassQueue>) -> DeliveryGate {
        DeliveryGate {
            state: Mutex::new(Delivery::Normal),
            notifier,
            queue,
        }
    }

    fn deliver(&self, item: Held) {
        match item {
            Held::Notice(notice) => offer(self.notifier.as_ref(), &notice),
            Held::Ready(dir) => self.queue.enqueue(&dir),
        }
    }

    fn pass(&self, item: Held) {
        let mut state = lock(&self.state);
        match &mut *state {
            Delivery::Normal => self.deliver(item),
            Delivery::Held(items) => items.push(item),
            Delivery::Exiting => {}
        }
    }

    /// A suspend: hold from now on (an exiting gate stays exiting).
    fn hold(&self) {
        let mut state = lock(&self.state);
        if matches!(*state, Delivery::Normal) {
            *state = Delivery::Held(Vec::new());
        }
    }

    /// A resume or the next request: what was held is delivered in order, then delivery
    /// is normal again (an exiting gate stays exiting).
    fn release(&self) {
        let mut state = lock(&self.state);
        if let Delivery::Held(items) = &mut *state {
            let items = std::mem::take(items);
            *state = Delivery::Normal;
            for item in items {
                self.deliver(item);
            }
        }
    }

    /// The exit began: what was held and everything later is dropped.
    fn exiting(&self) {
        *lock(&self.state) = Delivery::Exiting;
    }
}

/// `run()`'s `CallShell` (called after the call session lock): a notice and a folder a
/// stop left ready-to-transcribe go through the one `DeliveryGate` (offered and
/// enqueued at once, held since a suspend, dropped since the exit began); the Calls tab
/// request posts `settings_window::request(Tab(Calls, None))` and drops the receipt.
pub struct ShellCallShell<R: Runtime> {
    app: AppHandle<R>,
    gate: Arc<DeliveryGate>,
}

impl<R: Runtime> ShellCallShell<R> {
    fn new(app: &AppHandle<R>, gate: Arc<DeliveryGate>) -> ShellCallShell<R> {
        ShellCallShell {
            app: app.clone(),
            gate,
        }
    }
}

impl<R: Runtime> CallShell for ShellCallShell<R> {
    fn notice(&self, notice: CallNotice) {
        self.gate.pass(Held::Notice(notice));
    }

    fn open_calls_tab(&self) {
        let _ = settings_window::request(&self.app, OpenTarget::Tab(SettingsTab::Calls, None));
    }

    /// One `calls://changed {root, id}` to the Calls window only; nothing hears it while no
    /// Calls window exists.
    fn changed(&self, root: &Path, id: &CallId) {
        let _ = self.app.emit_to(
            LABEL,
            CHANGED_EVENT,
            CallRef {
                root: wire_root(root),
                id: id.as_str().to_string(),
            },
        );
    }

    fn call_ready(&self, dir: &Path) {
        self.gate.pass(Held::Ready(dir.to_path_buf()));
    }
}

// ---- the Calls window's commands ----------------------------------------------------------

/// The `format` of `calls_export` on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Md,
    Json,
}

impl From<ExportFormat> for Format {
    fn from(format: ExportFormat) -> Format {
        match format {
            ExportFormat::Md => Format::Md,
            ExportFormat::Json => Format::Json,
        }
    }
}

/// The answer of `calls_export`: `saved: false` when the dialog was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ExportOutcome {
    pub saved: bool,
}

/// The managed store and the current settings; `None` on an app without a call part.
fn store_and_settings<R: Runtime>(app: &AppHandle<R>) -> Option<(Arc<CallStore>, Arc<Settings>)> {
    let store = app.try_state::<Arc<CallStore>>()?;
    let service = app.try_state::<Arc<SettingsService>>()?;
    Some((Arc::clone(&store), service.snapshot()))
}

/// What a call command works with: the store, the current settings and the call part's
/// ports.
struct Served<'a> {
    store: Arc<CallStore>,
    settings: Arc<Settings>,
    part: tauri::State<'a, CallPart>,
}

/// The store, the settings and the call part; `notFound` on an app without a call part.
fn call_part<R: Runtime>(app: &AppHandle<R>) -> Result<Served<'_>, CallError> {
    let (store, settings) = store_and_settings(app).ok_or_else(CallError::not_found)?;
    let part = app
        .try_state::<CallPart>()
        .ok_or_else(CallError::not_found)?;
    Ok(Served {
        store,
        settings,
        part,
    })
}

/// `calls_list` → `CallList` (FR-033): every call of the current and earlier roots, in the
/// UI language. An app without a call part lists nothing. The first list from the Calls
/// window's page marks it listening and emits the target an open held for it.
#[tauri::command(async)]
pub fn calls_list<R: Runtime>(app: AppHandle<R>, webview: Webview<R>) -> CallListWire {
    let wire = match store_and_settings(&app) {
        Some((store, settings)) => list_wire(&store.list(&settings.calls), settings.ui_language),
        None => CallListWire {
            calls: Vec::new(),
            unavailable_roots: Vec::new(),
        },
    };
    if webview.label() == LABEL {
        if let Some(part) = app.try_state::<CallPart>() {
            let pending = {
                let mut state = lock(&part.window);
                state.listening = true;
                state.pending.take()
            };
            if let Some(target) = pending {
                emit_open(&app, target);
            }
        }
    }
    wire
}

/// `calls_get {root, id}` → `CallDetail` (FR-034); `notFound`, `ioFailed`.
#[tauri::command(async)]
pub fn calls_get<R: Runtime>(
    app: AppHandle<R>,
    root: String,
    id: String,
) -> Result<CallDetail, CallError> {
    let (store, settings) = store_and_settings(&app).ok_or_else(CallError::not_found)?;
    store.get(&settings.calls, &root, &id, settings.ui_language)
}

/// `calls_copy_markdown {root, id}` → `null` (FR-036): the call's Markdown export, written
/// once through the dictation's clipboard writer (the FR-10 exclusion formats); a failed
/// write is `ioFailed{other}`.
#[tauri::command(async)]
pub fn calls_copy_markdown<R: Runtime>(
    app: AppHandle<R>,
    root: String,
    id: String,
) -> Result<(), CallError> {
    let Served {
        store,
        settings,
        part,
    } = call_part(&app)?;
    let bytes = store.export_bytes(&settings.calls, &root, &id, Format::Md)?;
    let text =
        String::from_utf8(bytes).map_err(|_| CallError::io_failed(io::ErrorKind::InvalidData))?;
    part.clipboard
        .set_text_excluded_from_history(&text)
        .map_err(|_| CallError::io_failed(io::ErrorKind::Other))
}

/// `calls_export {root, id, format}` → `{saved}` (FR-036): the guard and the bytes first,
/// then the save dialog; a cancel is `{saved: false}` and writes nothing, a picked target
/// gets the bytes through `export_to` only. The guard, the modal dialog and the write run
/// on `spawn_blocking` (the dialog blocks until the user answers), never on an async
/// runtime worker (#42); a blocking task that cannot finish is `ioFailed{other}`.
#[tauri::command]
pub async fn calls_export<R: Runtime>(
    app: AppHandle<R>,
    root: String,
    id: String,
    format: ExportFormat,
) -> Result<ExportOutcome, CallError> {
    let (store, settings, save) = {
        let Served {
            store,
            settings,
            part,
        } = call_part(&app)?;
        (store, settings, Arc::clone(&part.save))
    };
    let format = Format::from(format);
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = store.export_bytes(&settings.calls, &root, &id, format)?;
        let picked = save
            .pick(&suggested_name(&id, format), format)
            .map_err(|e| CallError::io_failed(e.kind()))?;
        match picked {
            None => Ok(ExportOutcome { saved: false }),
            Some(target) => {
                export_to(&target, &bytes)?;
                Ok(ExportOutcome { saved: true })
            }
        }
    })
    .await
    .map_err(|_join_error| CallError::io_failed(io::ErrorKind::Other))?
}

/// The file name the save dialog offers: the call id (already checked by the guard, so a
/// plain file name) with the format's extension.
fn suggested_name(id: &str, format: Format) -> String {
    let extension = match format {
        Format::Md => "md",
        Format::Json => "json",
    };
    format!("{id}.{extension}")
}

/// `calls_open_folder {root, id}` → `null` (FR-033, FR-036): the guarded call folder, also
/// of a damaged call, to the folder opener; a gone call is `notFound`, a failed open
/// `ioFailed`.
#[tauri::command(async)]
pub fn calls_open_folder<R: Runtime>(
    app: AppHandle<R>,
    root: String,
    id: String,
) -> Result<(), CallError> {
    let Served {
        store,
        settings,
        part,
    } = call_part(&app)?;
    let dir = store.folder(&settings.calls, &root, &id)?;
    part.folder
        .open(&dir)
        .map_err(|e| CallError::io_failed(e.kind()))
}

/// The pass worker's `PassEvents` (T-139): log lines to the one log, every notice to the
/// shared notifier through [`offer`], as the session's own notices.
pub struct ShellPassEvents {
    log: Arc<Log>,
    notifier: Arc<dyn Notifier>,
}

impl ShellPassEvents {
    pub fn new(log: Arc<Log>, notifier: Arc<dyn Notifier>) -> ShellPassEvents {
        ShellPassEvents { log, notifier }
    }
}

impl PassEvents for ShellPassEvents {
    fn log(&self, event: LogEvent) {
        self.log.write(event);
    }

    fn notice(&self, notice: CallNotice) {
        offer(self.notifier.as_ref(), &notice);
    }
}

/// 1a has no subtitle engine (FR-019: nothing turns subtitles on); the audio is dropped.
struct NoSubtitles;

impl SubtitleConsumer for NoSubtitles {
    fn audio(&self, _track: Track, _samples: &[f32]) {}
}
