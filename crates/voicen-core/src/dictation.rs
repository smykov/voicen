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
//! Threads: the callers' input threads (hotkey, tray), one worker (owns the
//! [`Pipeline`]; no other thread can reach `run_job` or `Pipeline::pending`) and one
//! message timer. The why of each rule: `docs/decisions/dictation-session.md`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::io;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::audio::{mix_to_mono, AudioBuffer};
use crate::events::{DeviceKind, DictationEvent, PipelineObserver};
use crate::pipeline::{EngineFactory, Pipeline, PipelineDeps, PressContext};
use crate::platform::{AudioSource, CaptureHandle, FrameSink, Indicator, Paster, ShellRequests};
use crate::recording::{
    CaptureError, FinishedRecording, MicCause, OverlayState, Press, RecordingController,
    RecordingId, Release, TrayState,
};
use crate::settings::gate::{blocked_actions, dictation_gate, SettingsTab, ShellAction};
use crate::settings::service::SettingsService;

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
}

/// The one dictation session. Inputs take `&self` and the instant the shell
/// stamped for its event. Dropping it lets the job in flight finish, drops the
/// queued recordings and joins its threads.
pub struct DictationSession {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    timer: Option<JoinHandle<()>>,
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
    /// Serialises the release path (release → stop → finish), so recordings
    /// reach the queue in recording order even if releases come from several
    /// threads. Taken before `state`, never while holding it.
    releasing: Mutex<()>,
    audio: Arc<dyn AudioSource>,
    indicator: Arc<dyn Indicator>,
    requests: Arc<dyn ShellRequests>,
    settings: Arc<SettingsService>,
    /// The pipeline's own instances (cloned from `PipelineDeps` before the build).
    observer: Arc<dyn PipelineObserver>,
    paster: Arc<dyn Paster>,
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
    /// A stop is between its `release` and its `finish`: the controller shows
    /// neither the recording nor its job yet, so nothing is published until the
    /// `finish` (at most one: the release path is serialised). Cleared by the
    /// `finish`, or by [`StopInFlight`] if the release path panics before it.
    stopping: bool,
    shutdown: bool,
}

/// The live recording's open device and where its frames go.
struct LiveCapture {
    id: RecordingId,
    pressed_at: Instant,
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
    /// flight it sends nothing: the stop's `finish` publishes what changed
    /// meanwhile, so the overlay goes Recording → Processing, never through Hidden.
    fn publish(&self, st: &mut State) {
        self.changed.notify_all();
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
                stopping: false,
                shutdown: false,
            }),
            changed: Condvar::new(),
            releasing: Mutex::new(()),
            audio,
            indicator,
            requests,
            settings,
            observer,
            paster,
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
        };
        let for_timer = Arc::clone(&session.shared);
        session.timer = Some(spawn("dictation-timer", move || run_timer(&for_timer))?);
        Ok(session)
    }

    /// The hotkey went down at `at` (hold mode; toggle behaves as hold until
    /// T-009, decision #63).
    ///
    /// While a recording is on this is auto-repeat: nothing runs. From idle, the
    /// settings snapshot is taken and gated; a blocked press shows the notice and
    /// asks the shell to open settings, in `blocked_actions` order, and emits
    /// exactly one `DictationEvent::PressBlocked` (one per press, not per action;
    /// it takes no `RecordingId`, and its release does nothing). Otherwise the
    /// start window is taken, the controller starts the recording and the capture
    /// opens; the indicator is published only once the capture result is known,
    /// so a capture that fails never shows a recording state.
    pub fn hotkey_pressed(&self, at: Instant) {
        let shared = &*self.shared;
        let mut open_tabs: Vec<SettingsTab> = Vec::new();
        let mut emitted: Option<DictationEvent> = None;
        {
            let mut st = shared.lock();
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
                    let ctx = PressContext {
                        start_window: shared.paster.capture_start_window(),
                        settings,
                    };
                    if let Press::Start(id) = st.ctrl.press(at, ctx) {
                        let sink = Arc::new(CaptureSink::default());
                        match shared.audio.start(sink.clone()) {
                            Ok(handle) => {
                                st.capture = Some(LiveCapture {
                                    id,
                                    pressed_at: at,
                                    handle,
                                    sink,
                                });
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
            shared.requests.open_settings(tab);
        }
    }

    /// The hotkey went up at `at`.
    ///
    /// The controller decides the release under the lock; the device is stopped
    /// outside it (closed once this returns, NFR-02). A discard is published at
    /// once. For a stop, the frames become audio outside the lock and `finish`
    /// queues the job under it; no publication happens between the release and
    /// the `finish` (`State::stopping`), so the overlay goes Recording →
    /// Processing with no Hidden in between. If this path panics in between, the
    /// panic reaches the caller and publication resumes at once (`StopInFlight`).
    pub fn hotkey_released(&self, at: Instant) {
        let shared = &*self.shared;
        let _order = lock(&shared.releasing);
        // Before the lock guards below, so a panic releases them first.
        let mut stop_in_flight = StopInFlight {
            shared,
            armed: false,
        };
        let (release, id, capture) = {
            let mut st = shared.lock();
            let release = st.ctrl.release(at);
            let id = match &release {
                Release::Ignored => return,
                Release::Stop(ticket) => ticket.id(),
                Release::Discarded { id, .. } => *id,
            };
            // A stop publishes at its `finish`; `publish` still wakes the timer,
            // whose deadline the release may have moved.
            st.stopping = matches!(release, Release::Stop(_));
            stop_in_flight.armed = st.stopping;
            shared.publish(&mut st);
            let capture = st.capture.take().filter(|c| c.id == id);
            (release, id, capture)
        };

        // Outside the lock: once `stop` returns the sink gets no more frames.
        let audio = match capture {
            Some(c) => {
                let stopped = c.handle.stop();
                let recorded = c.sink.take();
                if let Some(first) = recorded.first_at {
                    shared.emit(DictationEvent::RecordingStarted {
                        recording: id,
                        hotkey_to_first_frame_ms: millis(
                            first.saturating_duration_since(c.pressed_at),
                        ),
                        device: DeviceKind::Selected,
                    });
                }
                stopped.map(|()| recorded)
            }
            None => Err(CaptureError::Other(NO_CAPTURE.to_string())),
        };
        if let (Some(end), Some(held)) = (release.end(), release.held()) {
            shared.emit(DictationEvent::RecordingEnded {
                recording: id,
                duration_ms: millis(held),
                end,
            });
        }

        let Release::Stop(ticket) = release else {
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

    /// The tray menu was opened at `at` (clears tray `Error`).
    pub fn tray_menu_opened(&self, at: Instant) {
        let mut st = self.shared.lock();
        st.ctrl.tray_menu_opened(at);
        self.shared.publish(&mut st);
    }

    /// The hotkey registration result at `at` (tray `HotkeyError` until a
    /// successful registration).
    pub fn hotkey_registration(&self, registered: bool, at: Instant) {
        let mut st = self.shared.lock();
        st.ctrl.hotkey_registration(registered, at);
        self.shared.publish(&mut st);
    }
}

/// Lets the job in flight finish (`run_job` cannot be interrupted), drops the
/// queued recordings and joins the worker and timer threads (analysis Q3).
impl Drop for DictationSession {
    fn drop(&mut self) {
        {
            let mut st = self.shared.lock();
            st.shutdown = true;
            st.queue = None;
            // A recording still on: close the device now.
            st.capture = None;
            self.shared.changed.notify_all();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(timer) = self.timer.take() {
            let _ = timer.join();
        }
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

/// The message timer: calls `tick` once the controller's next deadline is
/// reached, and re-reads the deadline after every change (`Shared::changed`).
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
                    st.ctrl.tick(now);
                    shared.publish(&mut st);
                    st
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
#[derive(Default)]
struct CaptureSink {
    inner: Mutex<Recorded>,
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
