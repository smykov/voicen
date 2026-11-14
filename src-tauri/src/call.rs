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
//! - [`ShellCallIndicator`] forwards `set_call_recording` to the tray part; `set_state` /
//!   `set_tick` are no-ops until T-130's indicator window (#277: a call shows only the
//!   tray icon until then). [`ShellCallShell`] maps each `CallNotice` through core's
//!   `CallNotice::message` to the shared notifier and opens Settings on its Calls tab.
//! - [`open_calls`] is the tray "Calls" route; the Calls window is T-142's, so it only
//!   counts (`calls_opened`).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::thread;

use tauri::{AppHandle, Manager, Runtime};
use voicen_core::call::events::refused_notice;
use voicen_core::call::indicator::{IndicatorState, IndicatorTick};
use voicen_core::call::session::{
    CallDeps, CallIndicator, CallSession, CallShell, SubtitleConsumer, Toggled, TrackFile,
    TrackOpener,
};
use voicen_core::call::{CallNotice, CallSource, CallStore, SwitchRefusal, Track, TrackSource};
use voicen_core::clock::{MonotonicClock, SystemClock};
use voicen_core::diag::{Log, LogEvent};
use voicen_core::local_models::download::DiskSpace;
use voicen_core::platform::Notifier;
use voicen_core::settings::gate::{CallSwitch, LaunchArgs, SettingsTab};
use voicen_core::settings::roots::FsCallRoots;
use voicen_core::settings::service::SettingsService;

use crate::settings_window::{self, OpenTarget};
use crate::tray::{self, TrayPart};
use crate::win::audio_notify::{ComCheck, NotifyFactory};

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
}

/// One shell start or stop of a call, posted with [`request`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallRequest {
    /// From the tray: a toggle (stops a recording call); from the switch: a start.
    Record(CallSource),
    /// Stops the recording call (`call_stopped reason=user|hotkey`).
    Stop(CallSource),
    /// An unknown `--call` value: one `call_switch_refused reason=unknown_value`.
    RefuseSwitch,
}

/// The call part of an app: the worker's queue, the tick thread's wake-up and the
/// "Calls" route's counter.
struct CallPart {
    requests: Sender<CallRequest>,
    opened: AtomicU32,
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

/// Builds the one call session of `app` over `ports`, manages it as `Arc<CallSession>`
/// and its store as `Arc<CallStore>`, and starts the `call-requests` worker and the
/// `call-tick` thread. Needs the managed `Arc<SettingsService>` and `Arc<Log>`
/// (`assemble`'s wiring) and, for the tray flag, the tray (`run()` calls this after
/// `start_dictation`, so the tray exists). Starts no call. `Err` when a managed part is
/// missing, the app already has a call part (then nothing is started) or a thread could
/// not be started.
pub fn start_calls<R: Runtime>(app: &AppHandle<R>, ports: CallPorts) -> io::Result<()> {
    let log = app
        .try_state::<Arc<Log>>()
        .map(|log| Arc::clone(&log))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the app manages no log"))?;
    let service = app
        .try_state::<Arc<SettingsService>>()
        .map(|service| Arc::clone(&service))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "the app manages no settings service",
            )
        })?;
    if app.try_state::<CallPart>().is_some() || app.try_state::<Arc<CallSession>>().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "the app already has a call part",
        ));
    }
    let CallPorts {
        source,
        disk,
        mono,
        open_track,
        notifier,
        calls_root,
    } = ports;
    let store = Arc::new(CallStore::new(
        Arc::new(FsCallRoots::new(calls_root)),
        Arc::clone(&mono),
    ));
    let session = Arc::new(CallSession::new(CallDeps {
        source,
        store: Arc::clone(&store),
        settings: service,
        indicator: Arc::new(ShellCallIndicator::new(app)),
        shell: Arc::new(ShellCallShell::new(app, Arc::clone(&notifier))),
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
    let (requests, queue) = mpsc::channel();
    spawn_tick(Arc::downgrade(&session), Arc::clone(&wake), mono)?;
    spawn_worker(
        Arc::clone(&session),
        queue,
        Arc::clone(&wake),
        log,
        notifier,
    )
    .inspect_err(|_| wake.close())?;
    app.manage(store);
    app.manage(session);
    app.manage(CallPart {
        requests,
        opened: AtomicU32::new(0),
    });
    Ok(())
}

/// The `call-requests` worker: the only caller of `CallSession::toggle_record` in the
/// shell. After each request it wakes the `call-tick` thread (a start makes a deadline).
/// Ends when the queue's sender is gone, and then closes the tick thread.
fn spawn_worker(
    session: Arc<CallSession>,
    queue: Receiver<CallRequest>,
    wake: Arc<Wake>,
    log: Arc<Log>,
    notifier: Arc<dyn Notifier>,
) -> io::Result<()> {
    thread::Builder::new()
        .name("call-requests".into())
        .spawn(move || {
            for request in queue {
                serve(&session, request, &log, notifier.as_ref());
                wake.wake();
            }
            wake.close();
        })
        .map(|_| ())
}

/// One request on the worker. A refusal offers its notice here, after the session lock
/// (the session's own lines name the part and the reason).
fn serve(session: &CallSession, request: CallRequest, log: &Log, notifier: &dyn Notifier) {
    let toggled = match request {
        CallRequest::Record(CallSource::Switch) => session.toggle_record(true, CallSource::Switch),
        CallRequest::Record(source) => session.toggle_record(!session.is_recording(), source),
        CallRequest::Stop(source) => session.toggle_record(false, source),
        CallRequest::RefuseSwitch => {
            log.write(LogEvent::CallSwitchRefused {
                reason: SwitchRefusal::UnknownValue,
            });
            return;
        }
    };
    if let Toggled::Refused { part, reason } = toggled {
        notifier.notify(refused_notice(part, reason), &[], None);
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

/// The tray "Calls" route. The Calls window is T-142's: until then it only counts the
/// opens of an app with a call part (nothing without one).
pub fn open_calls<R: Runtime>(app: &AppHandle<R>) {
    if let Some(part) = app.try_state::<CallPart>() {
        part.opened.fetch_add(1, Ordering::SeqCst);
    }
}

/// How many times [`open_calls`] ran on `app`'s call part (0 without one); a test-read
/// accessor like `tray::applied`.
pub fn calls_opened<R: Runtime>(app: &AppHandle<R>) -> u32 {
    app.try_state::<CallPart>()
        .map_or(0, |part| part.opened.load(Ordering::SeqCst))
}

/// `run()`'s `CallIndicator` (called inside the call session lock): the call flag goes
/// to the tray part (store and post only); `set_state` and `set_tick` do nothing until
/// T-130's indicator window forwards them (#277).
pub struct ShellCallIndicator<R: Runtime> {
    tray: Option<TrayPart<R>>,
}

impl<R: Runtime> ShellCallIndicator<R> {
    /// The indicator of `app`: its tray part (`tray::part`), if the tray was built.
    pub fn new(app: &AppHandle<R>) -> ShellCallIndicator<R> {
        ShellCallIndicator {
            tray: tray::part(app),
        }
    }
}

impl<R: Runtime> CallIndicator for ShellCallIndicator<R> {
    /// No indicator window until T-130 (#277).
    fn set_state(&self, _state: &IndicatorState) {}

    /// No indicator window until T-130 (#277).
    fn set_tick(&self, _tick: &IndicatorTick) {}

    fn set_call_recording(&self, recording: bool) {
        if let Some(tray) = &self.tray {
            tray.set_call_recording(recording);
        }
    }
}

/// `run()`'s `CallShell` (called after the call session lock): a notice goes to the
/// shared notifier with core's `CallNotice::message` id and `retry: None`; the Calls tab
/// request posts `settings_window::request(Tab(Calls, None))` and drops the receipt.
pub struct ShellCallShell<R: Runtime> {
    app: AppHandle<R>,
    notifier: Arc<dyn Notifier>,
}

impl<R: Runtime> ShellCallShell<R> {
    pub fn new(app: &AppHandle<R>, notifier: Arc<dyn Notifier>) -> ShellCallShell<R> {
        ShellCallShell {
            app: app.clone(),
            notifier,
        }
    }
}

impl<R: Runtime> CallShell for ShellCallShell<R> {
    fn notice(&self, notice: CallNotice) {
        let (id, params) = notice.message();
        self.notifier.notify(id, &params, None);
    }

    fn open_calls_tab(&self) {
        let _ = settings_window::request(&self.app, OpenTarget::Tab(SettingsTab::Calls, None));
    }
}

/// 1a has no subtitle engine (FR-019: nothing turns subtitles on); the audio is dropped.
struct NoSubtitles;

impl SubtitleConsumer for NoSubtitles {
    fn audio(&self, _track: Track, _samples: &[f32]) {}
}
