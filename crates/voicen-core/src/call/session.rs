//! The call session (spec 007 data-model "CallSession", contracts/core-call.md; T-126).
//!
//! [`CallSession::toggle_record`] is the one way the tray, the hotkey and the switch start
//! or stop a call, and it runs under the one session lock. A start opens the system track
//! (the default output's loopback), then the mic, then creates the call folder through
//! [`CallStore`]; any failure stops what was opened and logs exactly one `call_refused`.
//! While a call records, one writer thread drains both track channels through a
//! [`Timeline`] each into the two Ogg Opus encoders, publishes indicator ticks and keeps
//! channel drops in the log and in `meta.json`.
//!
//! Lock rules: the indicator port is called only with the session lock held; the shell
//! port only after it is released. The call session has no Esc input and shares no state
//! with the dictation session.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::call::capture::{
    ChannelSink, Report, Step, Timeline, TrackDevice, TrackEvent, TrackReceiver, TrackSink,
    TrackSource,
};
use crate::call::codec::{OpusOgg, TrackEncoder};
use crate::call::events::{
    CallNotice, CallSource, CallState, CallWarning, EndpointId, RefusedPart, RefusedReason,
    StopReason, StorageKind, Track,
};
use crate::call::indicator::{IndicatorState, IndicatorTick, IndicatorWarning};
use crate::call::storage::{
    started_at, storage_kind, write_meta, ActiveCallFiles, CallMeta, CallStore, CreateError,
    NewCall, TrackDrop,
};
use crate::call::OpenError;
use crate::clock::{Clock, LocalOffset, MonotonicClock};
use crate::diag::format::LocalTime;
use crate::diag::{Log, LogEvent};
use crate::i18n::UiLanguage;
use crate::platform::{CaptureHandle, DeviceId};
use crate::settings::roots::UnwritableReason;
use crate::settings::service::{SettingsService, WarnedHostFailed};
use crate::settings::url::{check_base_url, provider_host, ProviderHost};
use crate::settings::Settings;

/// How long the reminder and the headphones hint show after a start (FR-030).
const REMINDER: Duration = Duration::from_secs(10);
/// The shortest time between two indicator ticks (at most 10 per second).
const TICK_EVERY: Duration = Duration::from_millis(100);
/// How long the writer sleeps when both channels are empty.
const IDLE: Duration = Duration::from_millis(2);
/// How long the writer waits before it asks for the session lock again for a tick.
const LOCK_RETRY: Duration = Duration::from_millis(1);
/// 16 kHz samples per millisecond of a track.
const SAMPLES_PER_MS: u64 = 16;

/// The call indicator window and the tray's call flag. Called INSIDE the session lock;
/// an implementation must never block or call back into the session.
pub trait CallIndicator: Send + Sync {
    fn set_state(&self, state: &IndicatorState);
    fn set_tick(&self, tick: &IndicatorTick);
    fn set_call_recording(&self, recording: bool);
}

/// The shell requests of a call. Called AFTER the session lock is released.
pub trait CallShell: Send + Sync {
    fn notice(&self, notice: CallNotice);
    /// Opens the Settings window on its Calls tab (every `part=root` refusal, #243).
    fn open_calls_tab(&self);
}

/// The settings the session reads, and the one narrow writer it uses (FR-031).
pub trait CallSettingsPort: Send + Sync {
    fn snapshot(&self) -> Arc<Settings>;
    fn remember_warned_host(&self, host: &ProviderHost) -> Result<(), WarnedHostFailed>;
}

impl CallSettingsPort for SettingsService {
    fn snapshot(&self) -> Arc<Settings> {
        SettingsService::snapshot(self)
    }

    fn remember_warned_host(&self, host: &ProviderHost) -> Result<(), WarnedHostFailed> {
        SettingsService::remember_warned_host(self, host)
    }
}

/// Opens a track file inside the call folder ([`CallStore::create_with`]).
pub type TrackOpener = Arc<dyn Fn(&Path) -> io::Result<File> + Send + Sync>;

/// What the session is built from.
pub struct CallDeps {
    pub source: Arc<dyn TrackSource + Send + Sync>,
    pub store: Arc<CallStore>,
    pub settings: Arc<dyn CallSettingsPort>,
    pub indicator: Arc<dyn CallIndicator>,
    pub shell: Arc<dyn CallShell>,
    pub log: Arc<Log>,
    pub clock: Arc<dyn Clock>,
    pub offset: Arc<dyn LocalOffset>,
    pub mono: Arc<dyn MonotonicClock>,
    pub app_version: String,
    /// Opens a track file inside the call folder ([`CallStore::create_with`]).
    pub open_track: TrackOpener,
    /// Queued events per track channel.
    pub channel_capacity: usize,
}

/// What a `toggle_record` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggled {
    Started,
    /// Record was already on: nothing changed.
    AlreadyOn,
    /// The start was refused at the first failing part (system, mic, root).
    Refused {
        part: RefusedPart,
        reason: RefusedReason,
    },
    Stopped,
    /// Record was already off: nothing changed.
    AlreadyOff,
}

/// The call session. Off at construction; `Send + Sync`.
pub struct CallSession {
    deps: CallDeps,
    shared: Arc<Shared>,
}

/// What the session and its writer thread share.
struct Shared {
    /// The one session lock.
    inner: Mutex<Inner>,
    indicator: Arc<dyn CallIndicator>,
    pause: Pause,
}

#[derive(Default)]
struct Inner {
    active: Option<Active>,
}

/// The recording call.
struct Active {
    mic: Box<dyn CaptureHandle>,
    system: Box<dyn CaptureHandle>,
    writer: JoinHandle<WriterEnd>,
    stop: Arc<AtomicBool>,
    lang: UiLanguage,
    warnings: Vec<IndicatorWarning>,
    /// The end of the reminder; `None` once hidden or when the reminder is off.
    reminder_until: Option<Instant>,
    dir: PathBuf,
    /// The meta written at create: the fallback when the writer thread is gone.
    meta: CallMeta,
}

/// A refusal to report after the lock.
struct Refusal {
    part: RefusedPart,
    reason: RefusedReason,
}

impl CallSession {
    pub fn new(deps: CallDeps) -> CallSession {
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner::default()),
            indicator: deps.indicator.clone(),
            pause: Pause::default(),
        });
        CallSession { deps, shared }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.shared
            .inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Turns record on or off. `on` while recording, and off while off, change nothing.
    /// Off from the tray or the switch stops with [`StopReason::User`], from the hotkey
    /// with [`StopReason::Hotkey`].
    pub fn toggle_record(&self, on: bool, source: CallSource) -> Toggled {
        if on {
            self.start(source)
        } else {
            let reason = match source {
                CallSource::Hotkey => StopReason::Hotkey,
                CallSource::Tray | CallSource::Switch => StopReason::User,
            };
            if self.stop(reason) {
                Toggled::Stopped
            } else {
                Toggled::AlreadyOff
            }
        }
    }

    /// Stops the recording call for `reason`; `true` when a call was recording.
    pub fn stop(&self, reason: StopReason) -> bool {
        let stopped = {
            let mut inner = self.lock();
            match inner.active.take() {
                Some(active) => {
                    self.finish(active, reason);
                    true
                }
                None => false,
            }
        };
        if stopped {
            self.deps.shell.notice(CallNotice::CallSavedTranscribing);
        }
        stopped
    }

    pub fn is_recording(&self) -> bool {
        self.lock().active.is_some()
    }

    /// `None` while the session lock is held (by any thread), else whether a call records.
    pub fn try_is_recording(&self) -> Option<bool> {
        match self.shared.inner.try_lock() {
            Ok(inner) => Some(inner.active.is_some()),
            Err(TryLockError::Poisoned(p)) => Some(p.into_inner().active.is_some()),
            Err(TryLockError::WouldBlock) => None,
        }
    }

    /// Applies the reminder's end on the monotonic clock: once it is due, one state hides
    /// the reminder and the hint.
    pub fn tick(&self) {
        let mut inner = self.lock();
        let Some(active) = inner.active.as_mut() else {
            return;
        };
        let Some(until) = active.reminder_until else {
            return;
        };
        if self.deps.mono.now() < until {
            return;
        }
        active.reminder_until = None;
        let state = IndicatorState::recording(active.lang, false, active.warnings.clone());
        self.deps.indicator.set_state(&state);
    }

    /// When [`CallSession::tick`] has something to do: the reminder's end, or `None`.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.lock().active.as_ref().and_then(|a| a.reminder_until)
    }

    /// Holds the writer thread off the channels until the guard is dropped. Returns once
    /// the writer is parked, so blocks fed while the guard lives stay queued (or drop).
    /// Without a recording call it returns at once.
    #[cfg(any(test, feature = "test-fakes"))]
    pub fn pause_writer(&self) -> WriterPause {
        let recording = self.is_recording();
        let pause = &self.shared.pause;
        let mut state = pause.lock();
        state.requested = true;
        while recording && !state.parked {
            state = pause
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        drop(state);
        WriterPause {
            shared: self.shared.clone(),
        }
    }

    // ---- start ----------------------------------------------------------------------

    fn start(&self, source: CallSource) -> Toggled {
        let (toggled, ask_tab) = {
            let mut inner = self.lock();
            if inner.active.is_some() {
                return Toggled::AlreadyOn;
            }
            self.deps.log.write(LogEvent::CallRequested { source });
            match self.open_call() {
                Ok(active) => {
                    inner.active = Some(active);
                    (Toggled::Started, false)
                }
                Err(refusal) => {
                    self.deps.log.write(LogEvent::CallRefused {
                        part: refusal.part,
                        reason: refusal.reason,
                    });
                    (
                        Toggled::Refused {
                            part: refusal.part,
                            reason: refusal.reason,
                        },
                        refusal.part == RefusedPart::Root,
                    )
                }
            }
        };
        if ask_tab {
            self.deps.shell.open_calls_tab();
        }
        toggled
    }

    /// The start under the lock: system, mic, root, then the writer and the indicator.
    fn open_call(&self) -> Result<Active, Refusal> {
        let settings = self.deps.settings.snapshot();
        let (sink, mic_rx, system_rx) = ChannelSink::new(self.deps.channel_capacity);
        let sink: Arc<dyn TrackSink> = Arc::new(sink);
        let call_start = self.deps.source.now();

        let (system, system_id) = self
            .deps
            .source
            .open(
                Track::System,
                TrackDevice::DefaultOutputLoopback,
                sink.clone(),
            )
            .map_err(|e| Refusal {
                part: RefusedPart::System,
                reason: e.into(),
            })?;
        let mut warnings = Vec::new();
        let (mic, mic_id) = match self.open_mic(&settings, sink, &mut warnings) {
            Ok(opened) => opened,
            Err(e) => {
                stop_handle(system);
                return Err(Refusal {
                    part: RefusedPart::Mic,
                    reason: e.into(),
                });
            }
        };

        let started = self.deps.clock.now();
        let new = NewCall {
            started,
            seconds_east: self.deps.offset.seconds_east(started),
            mic: mic_id.clone(),
            system: system_id.clone(),
            lang: settings.ui_language,
            app_version: self.deps.app_version.clone(),
        };
        let open_track = self.deps.open_track.clone();
        let files = match self
            .deps
            .store
            .create_with(&settings.calls, &new, |path| open_track(path))
        {
            Ok(files) => files,
            Err(e) => {
                stop_handle(mic);
                stop_handle(system);
                let reason = match e {
                    CreateError::Root(reason) => root_reason(reason),
                    CreateError::Storage(kind) => {
                        self.deps
                            .log
                            .write(LogEvent::CallStorageFailed { kind, os: None });
                        RefusedReason::Other
                    }
                };
                return Err(Refusal {
                    part: RefusedPart::Root,
                    reason,
                });
            }
        };

        if let Some(host) = self.new_provider_host(&settings) {
            // The notice shows even when the host could not be remembered: it then shows
            // again at the next call (FR-031).
            let _ = self.deps.settings.remember_warned_host(&host);
            warnings.push(CallWarning::ProviderNotice { host });
        }
        let lang = settings.ui_language;
        let warnings: Vec<IndicatorWarning> = warnings
            .iter()
            .zip(1u32..)
            .map(|(w, id)| IndicatorWarning::of(id, w, lang))
            .collect();

        let dir = files.dir().to_path_buf();
        let meta = files.meta().clone();
        let stop = Arc::new(AtomicBool::new(false));
        let mono_start = self.deps.mono.now();
        let writer = match self.spawn_writer(
            files,
            (mic_rx, system_rx),
            Timeline::new(Track::Mic, call_start),
            Timeline::new(Track::System, call_start),
            stop.clone(),
            mono_start,
        ) {
            Ok(writer) => writer,
            Err(_) => {
                // No thread to write the call: the start is refused like any other
                // create failure after the probe, and the folder made for it is removed.
                stop_handle(mic);
                stop_handle(system);
                let _ = fs::remove_dir_all(&dir);
                return Err(Refusal {
                    part: RefusedPart::Root,
                    reason: RefusedReason::Other,
                });
            }
        };

        let reminder_until = settings.calls.reminder.then(|| mono_start + REMINDER);
        let state = IndicatorState::recording(lang, reminder_until.is_some(), warnings.clone());
        self.deps.indicator.set_state(&state);
        self.deps.indicator.set_call_recording(true);
        self.deps.log.write(LogEvent::CallStarted {
            mic: mic_id,
            system: system_id,
            comms_differs: false,
        });
        Ok(Active {
            mic,
            system,
            writer,
            stop,
            lang,
            warnings,
            reminder_until,
            dir,
            meta,
        })
    }

    /// The mic: the saved one by id, else the default input. A saved mic that is gone
    /// (`NoDevice`) is retried once on the default input with [`CallWarning::MicChanged`].
    fn open_mic(
        &self,
        settings: &Settings,
        sink: Arc<dyn TrackSink>,
        warnings: &mut Vec<CallWarning>,
    ) -> Result<(Box<dyn CaptureHandle>, EndpointId), OpenError> {
        let source = &self.deps.source;
        match &settings.microphone {
            None => source.open(Track::Mic, TrackDevice::DefaultInput, sink),
            Some(mic) => {
                let selected = TrackDevice::Selected(DeviceId(mic.id.clone()));
                match source.open(Track::Mic, selected, sink.clone()) {
                    Err(OpenError::NoDevice) => {
                        let opened = source.open(Track::Mic, TrackDevice::DefaultInput, sink)?;
                        warnings.push(CallWarning::MicChanged);
                        Ok(opened)
                    }
                    other => other,
                }
            }
        }
    }

    /// The API host to announce: remote, and not in `calls.warned_hosts` (FR-031).
    fn new_provider_host(&self, settings: &Settings) -> Option<ProviderHost> {
        let url = check_base_url(&settings.api.base_url).ok()?;
        let host = provider_host(&url)?;
        let known = settings
            .calls
            .warned_hosts
            .iter()
            .any(|h| h == host.as_str());
        (!known).then_some(host)
    }

    fn spawn_writer(
        &self,
        files: ActiveCallFiles<File>,
        (mic_rx, system_rx): (TrackReceiver, TrackReceiver),
        mic_timeline: Timeline,
        system_timeline: Timeline,
        stop: Arc<AtomicBool>,
        mono_start: Instant,
    ) -> io::Result<JoinHandle<WriterEnd>> {
        let dir = files.dir().to_path_buf();
        let meta = files.meta().clone();
        let ActiveCallFiles { mic, system, .. } = files;
        let lane = |rx: TrackReceiver, timeline: Timeline, enc: OpusOgg<File>| Lane {
            rx,
            timeline,
            enc: Some(enc),
            level: 0.0,
        };
        let mic_lane = lane(mic_rx, mic_timeline, mic);
        let system_lane = lane(system_rx, system_timeline, system);
        let writer = Writer {
            shared: self.shared.clone(),
            log: self.deps.log.clone(),
            mono: self.deps.mono.clone(),
            stop,
            mic: mic_lane,
            system: system_lane,
            meta,
            dir,
            mono_start,
            last_tick: None,
        };
        thread::Builder::new()
            .name("voicen-call-writer".to_string())
            .spawn(move || writer.run())
    }

    // ---- stop -----------------------------------------------------------------------

    /// The stop under the lock: captures off, the writer drained and joined, both files
    /// finished, `meta.json` ready-to-transcribe, `call_stopped`, the indicator off.
    fn finish(&self, active: Active, reason: StopReason) {
        let Active {
            mic,
            system,
            writer,
            stop,
            dir,
            meta,
            ..
        } = active;
        stop_handle(mic);
        stop_handle(system);
        stop.store(true, Ordering::SeqCst);
        let end = writer.join().ok();
        let (mut meta, mic_written, system_written) = match end {
            Some(end) => {
                for lane in [end.mic, end.system] {
                    if let Some(enc) = lane.enc {
                        if let Err(e) = enc.finish() {
                            self.deps.log.write(LogEvent::CallStorageFailed {
                                kind: storage_kind(e.op),
                                os: None,
                            });
                        }
                    }
                }
                (end.meta, end.mic_written, end.system_written)
            }
            None => (meta, 0, 0),
        };
        let now = self.deps.clock.now();
        let duration_ms = mic_written.min(system_written) / SAMPLES_PER_MS;
        meta.state = CallState::ReadyToTranscribe;
        meta.ended_at = Some(started_at(&LocalTime::of(
            now,
            self.deps.offset.seconds_east(now),
        )));
        meta.duration_ms = Some(duration_ms);
        meta.stop_reason = Some(reason);
        if let Err(e) = write_meta(&dir, &meta) {
            self.deps.log.write(LogEvent::CallStorageFailed {
                kind: StorageKind::Meta,
                os: e.raw_os_error(),
            });
        }
        let bytes = |track: Track| {
            meta.tracks
                .iter()
                .find(|t| t.track == track)
                .and_then(|t| fs::metadata(dir.join(&t.file)).ok())
                .map_or(0, |m| m.len())
        };
        self.deps.log.write(LogEvent::CallStopped {
            reason,
            duration_ms,
            mic_bytes: bytes(Track::Mic),
            system_bytes: bytes(Track::System),
        });
        self.deps.indicator.set_state(&IndicatorState::idle());
        self.deps.indicator.set_call_recording(false);
    }
}

impl Drop for CallSession {
    /// A session dropped while recording lets its writer thread end; the files stay as
    /// written (a later start finds the call `recording` and recovers it).
    fn drop(&mut self) {
        if let Some(active) = self.lock().active.as_ref() {
            active.stop.store(true, Ordering::SeqCst);
        }
    }
}

/// The #243 mapping of a refused probe onto the `part=root` reason.
fn root_reason(reason: UnwritableReason) -> RefusedReason {
    match reason {
        UnwritableReason::Missing => RefusedReason::Missing,
        UnwritableReason::AccessDenied => RefusedReason::AccessDenied,
        UnwritableReason::NotAFolder | UnwritableReason::Relative | UnwritableReason::DiskFull => {
            RefusedReason::Unwritable
        }
        UnwritableReason::Other => RefusedReason::Other,
    }
}

/// Stops a capture. A capture that fails to stop is closed by dropping it; there is
/// nothing else to undo.
fn stop_handle(handle: Box<dyn CaptureHandle>) {
    let _ = handle.stop();
}

// ---- the writer thread ------------------------------------------------------------------

/// One track on the writer: its channel, its timeline and its encoder (`None` once a
/// write failed).
struct Lane {
    rx: TrackReceiver,
    timeline: Timeline,
    enc: Option<OpusOgg<File>>,
    /// The latest 100 ms level.
    level: f32,
}

struct Writer {
    shared: Arc<Shared>,
    log: Arc<Log>,
    mono: Arc<dyn MonotonicClock>,
    stop: Arc<AtomicBool>,
    mic: Lane,
    system: Lane,
    meta: CallMeta,
    dir: PathBuf,
    mono_start: Instant,
    last_tick: Option<Instant>,
}

/// What the writer hands back at stop.
struct WriterEnd {
    mic: Lane,
    system: Lane,
    meta: CallMeta,
    mic_written: u64,
    system_written: u64,
}

impl Writer {
    fn run(mut self) -> WriterEnd {
        loop {
            self.shared.pause.park_if_asked();
            // Read before the drain: every event queued before the stop is drained.
            let stopping = self.stop.load(Ordering::SeqCst);
            let mut busy = false;
            for track in [Track::Mic, Track::System] {
                while let Some(event) = self.lane(track).rx.try_recv() {
                    busy = true;
                    self.handle(track, event);
                }
            }
            if !busy {
                if stopping {
                    break;
                }
                thread::sleep(IDLE);
            }
        }
        WriterEnd {
            mic_written: self.mic.timeline.written(),
            system_written: self.system.timeline.written(),
            mic: self.mic,
            system: self.system,
            meta: self.meta,
        }
    }

    fn lane(&mut self, track: Track) -> &mut Lane {
        match track {
            Track::Mic => &mut self.mic,
            Track::System => &mut self.system,
        }
    }

    fn handle(&mut self, track: Track, event: TrackEvent) {
        let lane = self.lane(track);
        // The track position before this step: where a drop happened.
        let at_ms = lane.timeline.written() / SAMPLES_PER_MS;
        let step = match event {
            TrackEvent::Block {
                interleaved,
                rate,
                channels,
                at,
            } => match lane.timeline.push(&interleaved, rate, channels, at) {
                Ok(step) => step,
                // A block the timeline cannot place emitted nothing; the next one starts
                // afresh at its own stamp.
                Err(_) => return,
            },
            TrackEvent::Lost { cause, at } => lane.timeline.lost(cause, at),
            TrackEvent::Dropped { blocks, ms } => lane.timeline.dropped(blocks, ms),
        };
        self.apply(track, step, at_ms);
    }

    fn apply(&mut self, track: Track, step: Step, at_ms: u64) {
        let lane = self.lane(track);
        let pushed = match lane.enc.as_mut() {
            Some(enc) => enc.push(&step.pcm),
            None => Ok(()),
        };
        if let Err(e) = pushed {
            // The track stops being written; the other one goes on.
            lane.enc = None;
            self.log.write(LogEvent::CallStorageFailed {
                kind: storage_kind(e.op),
                os: None,
            });
        }
        for report in &step.reports {
            match *report {
                Report::Dropped { blocks, ms } => self.dropped(track, blocks, ms, at_ms),
                // A gap tied to a device change is a `call_device` line (a later task);
                // a stall gap is neither logged nor stored (#244).
                Report::Gap { .. } => {}
            }
        }
        if let Some(level) = step.levels.last() {
            self.lane(track).level = *level;
            self.maybe_tick();
        }
    }

    /// One `call_track_dropped` line and one `drops[]` entry per `Report::Dropped` (#244).
    fn dropped(&mut self, track: Track, blocks: u32, ms: u32, at_ms: u64) {
        self.log.write(LogEvent::CallTrackDropped {
            track,
            blocks,
            ms,
            at_ms,
        });
        self.meta.drops.push(TrackDrop {
            at_ms,
            track,
            blocks,
            ms,
        });
        if let Err(e) = write_meta(&self.dir, &self.meta) {
            // The call goes on; the stop writes the full list again.
            self.log.write(LogEvent::CallStorageFailed {
                kind: StorageKind::Meta,
                os: e.raw_os_error(),
            });
        }
    }

    /// Publishes the levels under the session lock, at most every 100 ms of the mono
    /// clock. A stopping session holds the lock to join this thread: the tick is skipped.
    fn maybe_tick(&mut self) {
        let now = self.mono.now();
        if let Some(last) = self.last_tick {
            if now.saturating_duration_since(last) < TICK_EVERY {
                return;
            }
        }
        let tick = IndicatorTick {
            elapsed_ms: millis(now.saturating_duration_since(self.mono_start)),
            mic_level: self.mic.level,
            system_level: self.system.level,
            mic_silent: false,
            system_silent: false,
        };
        loop {
            if self.stop.load(Ordering::SeqCst) {
                return;
            }
            match self.shared.inner.try_lock() {
                Ok(_inner) => {
                    self.shared.indicator.set_tick(&tick);
                    break;
                }
                Err(TryLockError::Poisoned(p)) => {
                    let _inner = p.into_inner();
                    self.shared.indicator.set_tick(&tick);
                    break;
                }
                Err(TryLockError::WouldBlock) => thread::sleep(LOCK_RETRY),
            }
        }
        self.last_tick = Some(now);
    }
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

// ---- pausing the writer (tests) ---------------------------------------------------------

#[derive(Default)]
struct Pause {
    state: Mutex<PauseState>,
    changed: Condvar,
}

#[derive(Default)]
struct PauseState {
    requested: bool,
    parked: bool,
}

impl Pause {
    fn lock(&self) -> MutexGuard<'_, PauseState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Parks the writer while a [`WriterPause`] lives.
    fn park_if_asked(&self) {
        let mut state = self.lock();
        if !state.requested {
            return;
        }
        state.parked = true;
        self.changed.notify_all();
        while state.requested {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        state.parked = false;
    }
}

/// Holds the writer thread parked until dropped ([`CallSession::pause_writer`]).
#[cfg(any(test, feature = "test-fakes"))]
pub struct WriterPause {
    shared: Arc<Shared>,
}

#[cfg(any(test, feature = "test-fakes"))]
impl Drop for WriterPause {
    fn drop(&mut self) {
        let mut state = self.shared.pause.lock();
        state.requested = false;
        drop(state);
        self.shared.pause.changed.notify_all();
    }
}
