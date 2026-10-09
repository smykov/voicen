//! The dictation session (T-051, decision #64): turns hotkey events into a
//! delivered dictation over the platform ports, so every decision is proven on the
//! Linux gate before the Windows adapters (T-006) exist.
//!
//! Invariant (T-051 analysis): a hotkey press reaches the microphone, the
//! pipeline, the clipboard and Ctrl+V only through the one [`DictationSession`].
//! Capture opens only from idle, after `settings::gate::dictation_gate(snapshot)`
//! passed and `RecordingController::press` returned `Start`; one FIFO worker is the
//! only caller of `Pipeline::run_job` and `RecordingController::job_finished`, and
//! `run_job` never runs under the controller lock; every indicator change reaches
//! the [`Indicator`](crate::platform::Indicator) port from inside the session lock,
//! once per change; `retry_available` is read from the pending slot on the worker.
//!
//! T-009: every end of a recording (hold release, toggle press, the 10-minute
//! tick, Esc) is decided by `RecordingController`; every end that keeps the audio
//! goes through the one stop path (`stop_recording`), and the Esc claim
//! ([`CancelKey`](crate::platform::CancelKey)) follows `live_id().is_some()`.
//!
//! T-012: the device a press opens is decided here, from the adapter's
//! `AudioSource::devices()` taken at that press and the snapshot's saved
//! microphone (`microphone::choose`); `notice.mic_fallback` is raised only for a
//! press whose capture opened (`microphone::MicrophoneState`). A capture that ends
//! on its own (`FrameSink::device_lost`) is handed by its sink, without the
//! session lock, to the loss thread, which ends that recording, by its id, through
//! the same stop path (end `DeviceLost`).
//!
//! Threads: the callers' input threads (hotkey, tray), one worker (owns the
//! [`Pipeline`]; no other thread can reach `run_job` or `Pipeline::pending`), one
//! timer (message expiry and the recording's maximum length) and one loss thread
//! (device losses reported by the captures' sinks). The why of each rule:
//! `docs/decisions/dictation-session.md`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::audio::{mix_to_mono, AudioBuffer};
use crate::events::{DeviceKind, DictationEvent, PipelineObserver, WarningCode};
use crate::i18n::{NOTICE_HOTKEY_UNAVAILABLE, NOTICE_MIC_FALLBACK};
use crate::microphone::{choose, Choice, MicrophoneState};
use crate::pipeline::{EngineFactory, Pipeline, PipelineDeps, PressContext};
use crate::platform::{
    AudioSource, CancelKey, CaptureHandle, DeviceId, FrameSink, Indicator, InputDevice, Paster,
    ShellRequests,
};
use crate::recording::{
    CaptureError, FinishedRecording, MicCause, OverlayState, PressOutcome, RecordingController,
    RecordingEnd, RecordingId, Release, StopTicket, TrayState,
};
use crate::settings::gate::{blocked_actions, dictation_gate, SettingsTab, ShellAction};
use crate::settings::service::SettingsService;
use crate::settings::FieldId;

/// Everything the session talks to. The session builds its one `Pipeline` from
/// `pipeline` (so the observer and the paster it uses are the pipeline's own).
pub struct SessionDeps {
    pub pipeline: PipelineDeps,
    /// Replaces `engine::engine_for` (T-017's `BuiltinLocal` wrapper; test fakes).
    pub engine_factory: Option<Box<EngineFactory>>,
    pub audio: Arc<dyn AudioSource>,
    pub indicator: Arc<dyn Indicator>,
    pub requests: Arc<dyn ShellRequests>,
    /// Settings are read with `snapshot()` at each press (P-013; no
    /// `SettingsSource`, decision #47).
    pub settings: Arc<SettingsService>,
    /// The Esc claim (T-009): claimed while a recording is on.
    pub cancel_key: Arc<dyn CancelKey>,
}

/// The one dictation session. Inputs take `&self` and the instant the shell
/// stamped for its event. Dropping it lets the job in flight finish, drops the
/// queued recordings and joins its threads.
pub struct DictationSession {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    timer: Option<JoinHandle<()>>,
    losses: Option<JoinHandle<()>>,
}

/// What the loss thread receives.
enum Loss {
    /// The capture of `id` ended on its own at `at` (once per capture).
    Lost { id: RecordingId, at: Instant },
    /// The session is dropped: the thread ends even though adapters may still
    /// hold sinks (and so senders).
    Shutdown,
}

/// A recording on its way to the worker.
type Job = FinishedRecording<PressContext>;

/// The text of a `CaptureError::Other` the session raises itself: the adapter
/// delivered frames that cannot become audio (an adapter bug, never OS text).
const UNUSABLE_FRAMES: &str = "captured frames could not be converted";
/// `CaptureError::Other` for a stop with no capture to stop (not expected).
const NO_CAPTURE: &str = "no capture for the stopped recording";

/// What the session threads share.
struct Shared {
    state: Mutex<State>,
    /// Wakes the timer after every change of the state.
    changed: Condvar,
    /// Serialises the stop path (decision → capture stop → finish) of a release,
    /// a toggle press and the max-length tick, so recordings reach the queue in
    /// recording order whichever thread ends them. Taken before `state`, never
    /// while holding it.
    releasing: Mutex<()>,
    audio: Arc<dyn AudioSource>,
    indicator: Arc<dyn Indicator>,
    cancel_key: Arc<dyn CancelKey>,
    requests: Arc<dyn ShellRequests>,
    settings: Arc<SettingsService>,
    /// The pipeline's own instances (cloned from `PipelineDeps` before the build).
    observer: Arc<dyn PipelineObserver>,
    paster: Arc<dyn Paster>,
    /// To the loss thread; every capture sink holds a clone.
    losses: mpsc::Sender<Loss>,
}

/// Everything behind the session lock.
struct State {
    ctrl: RecordingController<PressContext>,
    /// The open capture of the live recording.
    capture: Option<LiveCapture>,
    /// The FIFO to the worker; `None` once shutdown began.
    queue: Option<mpsc::Sender<Job>>,
    /// The pending slot as the worker last read it after a job.
    retry_available: bool,
    /// What the indicator port was last given.
    published_tray: (TrayState, bool),
    published_overlay: OverlayState,
    /// What the cancel-key port was last given (`false` before the first call).
    published_claim: bool,
    /// A stop is between its `release` and its `finish`: the controller shows
    /// neither the recording nor its job yet, so nothing is published until the
    /// `finish` (at most one: the release path is serialised). Cleared by the
    /// `finish`, or by [`StopInFlight`] if the release path panics before it.
    stopping: bool,
    shutdown: bool,
    /// Which fallback device the last opened capture used (the notice's memory).
    microphone: MicrophoneState,
}

/// The live recording's open device and where its frames go.
struct LiveCapture {
    id: RecordingId,
    pressed_at: Instant,
    /// Whether the device opened is the selected one or the fallback.
    kind: DeviceKind,
    handle: Box<dyn CaptureHandle>,
    sink: Arc<CaptureSink>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name(name.to_string())
        .spawn(f)
        .map_err(|e| io::Error::from(e.kind()))
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    fn emit(&self, e: DictationEvent) {
        self.observer.event(&e);
    }

    /// Hands every indicator change since the last call to the port, then wakes
    /// the timer. Called with the lock held, right after the controller call that
    /// made the change; an unchanged value is not sent again. While a stop is in
    /// flight it sends no indicator change: the stop's `finish` publishes what
    /// changed meanwhile, so the overlay goes Recording → Processing, never
    /// through Hidden. The Esc claim is synced first and is not held back by a
    /// stop: it is released when the recording ends, not when its audio is queued.
    fn publish(&self, st: &mut State) {
        self.changed.notify_all();
        let claim = st.ctrl.live_id().is_some();
        if claim != st.published_claim {
            self.cancel_key.set(claim);
            st.published_claim = claim;
        }
        if st.stopping {
            return;
        }
        let indicator = st.ctrl.indicator();
        let tray = (indicator.tray, st.retry_available);
        if tray != st.published_tray {
            self.indicator.set_tray(tray.0, tray.1);
            st.published_tray = tray;
        }
        if indicator.overlay != st.published_overlay {
            self.indicator.set_overlay(&indicator.overlay);
            st.published_overlay = indicator.overlay.clone();
        }
    }
}

/// Ends a stop's hold on publication (`State::stopping`) if the release path
/// unwinds between the controller's release and the `finish` (a panicking
/// `CaptureHandle::stop`, observer or conversion): on drop, while armed, it
/// clears `stopping` and publishes what the controller shows. The `finish`
/// disarms it in the same critical section. Declare it before any session-lock
/// guard of the same scope, so that guard is released before this one locks.
struct StopInFlight<'a> {
    shared: &'a Shared,
    armed: bool,
}

impl Drop for StopInFlight<'_> {
    fn drop(&mut self) {
        if self.armed {
            let mut st = self.shared.lock();
            st.stopping = false;
            self.shared.publish(&mut st);
        }
    }
}

impl DictationSession {
    /// Builds the pipeline and starts the worker and timer threads. A thread that
    /// cannot be spawned is an `io::Error` (its kind only).
    pub fn start(deps: SessionDeps) -> io::Result<DictationSession> {
        let SessionDeps {
            pipeline,
            engine_factory,
            audio,
            indicator,
            requests,
            settings,
            cancel_key,
        } = deps;
        let observer = Arc::clone(&pipeline.observer);
        let paster = Arc::clone(&pipeline.paster);
        let pipeline = match engine_factory {
            Some(factory) => Pipeline::new(pipeline).with_engine_factory(factory),
            None => Pipeline::new(pipeline),
        };
        let ctrl = RecordingController::new();
        let initial = ctrl.indicator().clone();
        let (queue, jobs) = mpsc::channel::<Job>();
        let (losses, lost) = mpsc::channel::<Loss>();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                ctrl,
                capture: None,
                queue: Some(queue),
                retry_available: false,
                // Nothing is published at start: the first call is the first
                // change from the controller's initial state.
                published_tray: (initial.tray, false),
                published_overlay: initial.overlay,
                published_claim: false,
                stopping: false,
                shutdown: false,
                microphone: MicrophoneState::new(),
            }),
            changed: Condvar::new(),
            releasing: Mutex::new(()),
            audio,
            indicator,
            cancel_key,
            requests,
            settings,
            observer,
            paster,
            losses,
        });

        let for_worker = Arc::clone(&shared);
        let worker = spawn("dictation-worker", move || {
            run_worker(&for_worker, &pipeline, &jobs)
        })?;
        // From here on, an early return drops `session`, which joins the worker.
        let mut session = DictationSession {
            shared,
            worker: Some(worker),
            timer: None,
            losses: None,
        };
        let for_timer = Arc::clone(&session.shared);
        session.timer = Some(spawn("dictation-timer", move || run_timer(&for_timer))?);
        let for_losses = Arc::clone(&session.shared);
        session.losses = Some(spawn("dictation-losses", move || {
            run_losses(&for_losses, &lost)
        })?);
        Ok(session)
    }

    /// The hotkey went down at `at`.
    ///
    /// While a recording is on, the controller decides by the mode of the press
    /// that started it: a hold recording takes it as auto-repeat (nothing runs), a
    /// toggle recording stops through the stop path; neither runs the gate nor
    /// takes a start window. From idle, the
    /// settings snapshot is taken and gated; a blocked press shows the notice and
    /// asks the shell to open settings, in `blocked_actions` order, and emits
    /// exactly one `DictationEvent::PressBlocked` (one per press, not per action;
    /// it takes no `RecordingId`, and its release does nothing). Otherwise the
    /// start window is taken, the controller starts the recording and the capture
    /// opens on the device `microphone::choose` picks from the adapter's list
    /// (no device, or a list that cannot be read, fails the press like a capture
    /// that cannot open, without a `start` call); a fallback that opened raises
    /// `notice.mic_fallback` when it is due. The indicator is published only once
    /// the capture result is known, so a capture that fails never shows a
    /// recording state.
    pub fn hotkey_pressed(&self, at: Instant) {
        let shared = &*self.shared;
        if shared.lock().ctrl.live_id().is_some() {
            stop_recording(shared, at, |ctrl| match ctrl.press_while_live(at) {
                Some(PressOutcome::Stop(ticket)) => Some(Ended::Stop(ticket)),
                Some(PressOutcome::Start(_) | PressOutcome::Ignored) | None => None,
            });
            return;
        }
        let mut open_tabs: Vec<SettingsTab> = Vec::new();
        let mut emitted: Option<DictationEvent> = None;
        {
            let mut st = shared.lock();
            // Ended on another thread since the check: a press from idle is a new
            // recording, but only the hotkey thread presses, so this one is
            // dropped rather than racing it.
            if st.ctrl.live_id().is_some() {
                return;
            }
            let settings = shared.settings.snapshot();
            match dictation_gate(&settings) {
                Err(blocked) => {
                    emitted = Some(DictationEvent::PressBlocked { reason: blocked });
                    for action in blocked_actions(blocked) {
                        match action {
                            ShellAction::Notify(id) => {
                                st.ctrl.notice(id, at);
                                shared.publish(&mut st);
                            }
                            ShellAction::OpenSettings(tab) => open_tabs.push(tab),
                        }
                    }
                }
                Ok(()) => {
                    let mode = settings.mode;
                    let selected = settings.microphone.as_ref().map(|m| m.id.clone());
                    let ctx = PressContext {
                        start_window: shared.paster.capture_start_window(),
                        settings,
                    };
                    if let PressOutcome::Start(id) = st.ctrl.press_with_mode(at, mode, ctx) {
                        let sink = Arc::new(CaptureSink::new(id, shared.losses.clone()));
                        let opened = shared
                            .audio
                            .devices()
                            .and_then(|list| pick(selected.as_deref(), &list))
                            .and_then(|(device, kind, name)| {
                                let handle = shared.audio.start(&device, sink.clone())?;
                                Ok((device, kind, name, handle))
                            });
                        match opened {
                            Ok((device, kind, name, handle)) => {
                                st.capture = Some(LiveCapture {
                                    id,
                                    pressed_at: at,
                                    kind,
                                    handle,
                                    sink,
                                });
                                // After `press_with_mode`, which drops any message.
                                if st.microphone.opened(&device, kind) {
                                    st.ctrl.notice_with(
                                        NOTICE_MIC_FALLBACK,
                                        vec![("device", name)],
                                        at,
                                    );
                                }
                            }
                            Err(err) => {
                                let cause = MicCause::of(&err);
                                if st.ctrl.capture_failed(id, err, at).is_some() {
                                    emitted = Some(DictationEvent::CaptureFailed {
                                        recording: id,
                                        cause,
                                    });
                                }
                            }
                        }
                        shared.publish(&mut st);
                    }
                }
            }
        }
        if let Some(e) = emitted {
            shared.emit(e);
        }
        for tab in open_tabs {
            shared.requests.open_settings(tab, None);
        }
    }

    /// The hotkey went up at `at`.
    ///
    /// The controller decides the release (a toggle recording ignores it); a stop
    /// goes through the stop path, which closes the device before this returns
    /// (NFR-02). A discard is published at once.
    pub fn hotkey_released(&self, at: Instant) {
        stop_recording(&self.shared, at, |ctrl| match ctrl.release(at) {
            Release::Ignored => None,
            Release::Stop(ticket) => Some(Ended::Stop(ticket)),
            Release::Discarded { id, held } => Some(Ended::Dropped {
                id,
                held,
                end: RecordingEnd::TooShort,
            }),
        });
    }

    /// The timer's input at `at` (`run_timer` calls it at the controller's next
    /// deadline): expires the message and, at the recording's maximum length,
    /// stops it through the stop path (end `MaxLength`, `notice.max_length`).
    pub fn tick(&self, at: Instant) {
        tick(&self.shared, at);
    }

    /// Esc went down at `at` while claimed (FR-22). The recording that is on ends
    /// with nothing sent: the device is closed before this returns (NFR-02), the
    /// indicator goes off and `RecordingEnded{Cancelled}` is emitted. With no
    /// recording on (idle, or a stop in flight) nothing happens. It does not wait
    /// for a stop in flight: a cancel queues nothing, so it needs no queue order.
    pub fn esc_pressed(&self, at: Instant) {
        let shared = &*self.shared;
        let (id, capture) = {
            let mut st = shared.lock();
            let Some(id) = st.ctrl.cancel(at) else {
                return;
            };
            shared.publish(&mut st);
            (id, st.capture.take().filter(|c| c.id == id))
        };
        let held = capture.as_ref().map_or(Duration::ZERO, |c| {
            at.saturating_duration_since(c.pressed_at)
        });
        // The audio is dropped unread.
        let _ = close_capture(shared, id, capture);
        shared.emit(DictationEvent::RecordingEnded {
            recording: id,
            duration_ms: millis(held),
            end: RecordingEnd::Cancelled,
        });
    }

    /// The shell's answer at `at` to the last Esc claim. A refusal while a
    /// recording is on emits `Warning{EscUnavailable}`; the recording goes on (Esc
    /// just cannot cancel it). A granted claim, or a refusal that arrives after
    /// the recording ended, emits nothing.
    pub fn cancel_key_result(&self, claimed: bool, at: Instant) {
        let _ = at;
        if claimed {
            return;
        }
        let live = self.shared.lock().ctrl.live_id().is_some();
        if live {
            self.shared.emit(DictationEvent::Warning {
                code: WarningCode::EscUnavailable,
            });
        }
    }

    /// The tray menu was opened at `at` (clears tray `Error`).
    pub fn tray_menu_opened(&self, at: Instant) {
        let mut st = self.shared.lock();
        st.ctrl.tray_menu_opened(at);
        self.shared.publish(&mut st);
    }

    /// `false` while the last reported registration failed, so no hotkey works
    /// (T-055 Q1: the startup executor then posts no tab of its own); `true` before
    /// any report and after a successful one.
    pub fn hotkey_registered(&self) -> bool {
        !self.shared.lock().ctrl.hotkey_error()
    }

    /// The hotkey registration result at `at` (tray `HotkeyError` until a
    /// successful registration).
    ///
    /// Every failed registration that leaves no working hotkey comes here (T-055:
    /// the startup one; T-010's resume re-register): after the tray goes
    /// `HotkeyError`, the overlay shows `notice.hotkey_unavailable` for 3 s from `at`
    /// and, outside the session lock, the shell is asked to open settings on
    /// Recording with the hotkey field focused. A success only clears the error.
    pub fn hotkey_registration(&self, registered: bool, at: Instant) {
        {
            let mut st = self.shared.lock();
            st.ctrl.hotkey_registration(registered, at);
            self.shared.publish(&mut st);
            if !registered {
                st.ctrl.notice(NOTICE_HOTKEY_UNAVAILABLE, at);
                self.shared.publish(&mut st);
            }
        }
        if !registered {
            self.shared
                .requests
                .open_settings(SettingsTab::Recording, Some(FieldId::RecordingHotkey));
        }
    }
}

/// How the controller ended a recording, for [`stop_recording`].
enum Ended {
    /// The audio goes on to `finish` and a job.
    Stop(StopTicket<PressContext>),
    /// Nothing is sent (a too-short hold).
    Dropped {
        id: RecordingId,
        held: Duration,
        end: RecordingEnd,
    },
}

/// The device a press opens, with its kind and display name: the
/// `microphone::choose` decision over `devices`; no device is `NoDevice`.
fn pick(
    selected: Option<&str>,
    devices: &[InputDevice],
) -> Result<(DeviceId, DeviceKind, String), CaptureError> {
    match choose(selected, devices) {
        Choice::Use { id, kind } => {
            let name = devices
                .iter()
                .find(|d| d.id == id)
                .map(|d| d.name.clone())
                .unwrap_or_default();
            Ok((id, kind, name))
        }
        Choice::NoDevice => Err(CaptureError::NoDevice),
    }
}

/// The timer's input: expires the message, and stops a recording at its maximum
/// length through the stop path.
fn tick(shared: &Shared, at: Instant) {
    stop_recording(shared, at, |ctrl| ctrl.tick(at).map(Ended::Stop));
}

/// Closes the capture of recording `id` (outside the session lock: once `stop`
/// returns the sink gets no more frames) and emits `RecordingStarted` if a frame
/// came. Returns what was recorded.
fn close_capture(
    shared: &Shared,
    id: RecordingId,
    capture: Option<LiveCapture>,
) -> Result<Recorded, CaptureError> {
    let Some(c) = capture else {
        return Err(CaptureError::Other(NO_CAPTURE.to_string()));
    };
    let stopped = c.handle.stop();
    let recorded = c.sink.take();
    if let Some(first) = recorded.first_at {
        shared.emit(DictationEvent::RecordingStarted {
            recording: id,
            hotkey_to_first_frame_ms: millis(first.saturating_duration_since(c.pressed_at)),
            device: c.kind,
        });
    }
    stopped.map(|()| recorded)
}

/// The one stop path (T-009 invariant) of a release, a toggle press, the
/// max-length tick and a device loss (T-012). `decide` runs the controller call under the session lock,
/// after `releasing` is taken (queue order = recording order); it may also just
/// expire a message (`None`), which is published.
///
/// The device is stopped outside the lock (closed once this returns, NFR-02). A
/// discard is published at once. For a stop, the frames become audio outside the
/// lock and `finish` queues the job under it; no indicator change is published
/// between the decision and the `finish` (`State::stopping`), so the overlay goes
/// Recording → Processing with no Hidden in between. If this path panics in
/// between, the panic reaches the caller and publication resumes at once
/// (`StopInFlight`).
fn stop_recording(
    shared: &Shared,
    at: Instant,
    decide: impl FnOnce(&mut RecordingController<PressContext>) -> Option<Ended>,
) {
    let _order = lock(&shared.releasing);
    // Before the lock guards below, so a panic releases them first.
    let mut stop_in_flight = StopInFlight {
        shared,
        armed: false,
    };
    let (ended, id, capture) = {
        let mut st = shared.lock();
        // Shutdown dropped the capture: nothing is ended or reported any more.
        if st.shutdown {
            return;
        }
        let Some(ended) = decide(&mut st.ctrl) else {
            // A message may have expired; `publish` also wakes the timer.
            shared.publish(&mut st);
            return;
        };
        let id = match &ended {
            Ended::Stop(ticket) => ticket.id(),
            Ended::Dropped { id, .. } => *id,
        };
        // A stop publishes its indicator at its `finish`; `publish` still syncs
        // the Esc claim and wakes the timer, whose deadline may have moved.
        st.stopping = matches!(ended, Ended::Stop(_));
        stop_in_flight.armed = st.stopping;
        shared.publish(&mut st);
        let capture = st.capture.take().filter(|c| c.id == id);
        (ended, id, capture)
    };

    let audio = close_capture(shared, id, capture);
    let (held, end) = match &ended {
        Ended::Stop(ticket) => (ticket.held(), ticket.end()),
        Ended::Dropped { held, end, .. } => (*held, *end),
    };
    shared.emit(DictationEvent::RecordingEnded {
        recording: id,
        duration_ms: millis(held),
        end,
    });

    let Ended::Stop(ticket) = ended else {
        return;
    };
    // On the caller's thread (T-051 analysis Q6).
    let audio = audio.and_then(Recorded::into_audio);
    let cause = audio.as_ref().err().map(MicCause::of);
    {
        let mut st = shared.lock();
        if let Ok(job) = st.ctrl.finish(ticket, audio, at) {
            // Under the lock, so the queue order is the finish order.
            if let Some(queue) = &st.queue {
                let _ = queue.send(job);
            }
        }
        st.stopping = false;
        stop_in_flight.armed = false;
        shared.publish(&mut st);
    }
    if let Some(cause) = cause {
        shared.emit(DictationEvent::CaptureFailed {
            recording: id,
            cause,
        });
    }
}

/// Lets the job in flight finish (`run_job` cannot be interrupted), drops the
/// queued recordings and joins the worker, timer and loss threads (analysis Q3).
impl Drop for DictationSession {
    fn drop(&mut self) {
        {
            let mut st = self.shared.lock();
            st.shutdown = true;
            st.queue = None;
            // A recording still on: close the device now (its drop may report a
            // loss: the sink only sends it, and the loss thread ignores it).
            st.capture = None;
            self.shared.changed.notify_all();
        }
        let _ = self.shared.losses.send(Loss::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(timer) = self.timer.take() {
            let _ = timer.join();
        }
        if let Some(losses) = self.losses.take() {
            let _ = losses.join();
        }
    }
}

/// The loss thread: ends the recording whose capture reported a loss through the
/// one stop path, decided by the controller for that id (a stale id, or a
/// recording already ended, changes nothing). Ends at `Loss::Shutdown`, or at a
/// loss received once shutdown began.
fn run_losses(shared: &Shared, losses: &mpsc::Receiver<Loss>) {
    while let Ok(Loss::Lost { id, at }) = losses.recv() {
        if shared.lock().shutdown {
            return;
        }
        stop_recording(shared, at, |ctrl| ctrl.device_lost(id, at).map(Ended::Stop));
    }
}

/// The one worker: the only caller of `run_job` and `job_finished`, in queue
/// order, with no lock held during `run_job`. After shutdown began it takes no
/// further job; the queued ones are dropped with the receiver.
fn run_worker(shared: &Shared, pipeline: &Pipeline, jobs: &mpsc::Receiver<Job>) {
    while let Ok(job) = jobs.recv() {
        if shared.lock().shutdown {
            return;
        }
        let id = job.id();
        let report = pipeline.run_job(job);
        // The slot itself, read here: only this thread changes it.
        let retry_available = pipeline.pending().is_some();
        let mut st = shared.lock();
        st.ctrl.job_finished(id, report.end, Instant::now());
        st.retry_available = retry_available;
        shared.publish(&mut st);
    }
}

/// The timer: calls [`tick`] once the controller's next deadline (a message's
/// expiry or a recording's maximum length) is reached, and re-reads the deadline
/// after every change (`Shared::changed`). The tick runs with the session lock
/// released, because a max-length stop takes the stop path (`releasing` first).
fn run_timer(shared: &Shared) {
    let mut st = shared.lock();
    loop {
        if st.shutdown {
            return;
        }
        st = match st.ctrl.next_deadline() {
            None => shared
                .changed
                .wait(st)
                .unwrap_or_else(PoisonError::into_inner),
            Some(deadline) => {
                let now = Instant::now();
                if now >= deadline {
                    drop(st);
                    tick(shared, now);
                    shared.lock()
                } else {
                    shared
                        .changed
                        .wait_timeout(st, deadline.saturating_duration_since(now))
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
            }
        };
    }
}

/// Where one capture's frames go: mixed down to mono as they arrive (the one
/// rule, `audio::mix_to_mono`), at the first frame's format. A format change or
/// frames that cannot be mixed make the capture unusable. Its own short lock,
/// never the session's: the adapter's audio thread must not wait for the session.
/// It knows its recording, so a loss it reports can only end that one.
struct CaptureSink {
    inner: Mutex<Recorded>,
    id: RecordingId,
    /// Set by the first `device_lost`; later ones are dropped.
    lost: AtomicBool,
    losses: mpsc::Sender<Loss>,
}

#[derive(Default)]
struct Recorded {
    /// The adapter's instant of the first non-empty frame block.
    first_at: Option<Instant>,
    /// `(rate, channels)` of the first block.
    format: Option<(u32, u16)>,
    mono: Vec<f32>,
    unusable: bool,
}

impl CaptureSink {
    fn new(id: RecordingId, losses: mpsc::Sender<Loss>) -> CaptureSink {
        CaptureSink {
            inner: Mutex::new(Recorded::default()),
            id,
            lost: AtomicBool::new(false),
            losses,
        }
    }

    fn take(&self) -> Recorded {
        std::mem::take(&mut *lock(&self.inner))
    }
}

impl FrameSink for CaptureSink {
    fn frames(&self, interleaved: &[f32], rate: u32, channels: u16, at: Instant) {
        if interleaved.is_empty() {
            return;
        }
        let mut r = lock(&self.inner);
        if r.first_at.is_none() {
            r.first_at = Some(at);
        }
        if r.unusable {
            return;
        }
        let format = *r.format.get_or_insert((rate, channels));
        if format != (rate, channels) || mix_to_mono(interleaved, channels, &mut r.mono).is_err() {
            r.unusable = true;
            r.mono = Vec::new();
        }
    }

    /// Hands the loss to the loss thread (once); never blocks, never takes the
    /// session lock, so it is safe inside `start` and in a handle's drop.
    fn device_lost(&self, at: Instant) {
        if !self.lost.swap(true, Ordering::SeqCst) {
            let _ = self.losses.send(Loss::Lost { id: self.id, at });
        }
    }
}

impl Recorded {
    /// The 16 kHz mono audio of the capture. No frame at all is an empty
    /// recording (the speech gate finds no speech).
    fn into_audio(self) -> Result<AudioBuffer, CaptureError> {
        let unusable = || CaptureError::Other(UNUSABLE_FRAMES.to_string());
        if self.unusable {
            return Err(unusable());
        }
        match self.format {
            None => Ok(AudioBuffer::from_16k_mono(Vec::new())),
            Some((rate, _)) => {
                AudioBuffer::from_frames(&self.mono, rate, 1).map_err(|_| unusable())
            }
        }
    }
}
