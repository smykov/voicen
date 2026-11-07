//! The call session (spec 007 data-model "CallSession", contracts/core-call.md; T-126).
//!
//! The session holds the two toggles of FR-001: `record` and `subtitles`; call mode is
//! active while at least one is on. [`CallSession::toggle_record`] is the one way the tray,
//! the hotkey and the switch start or stop a call, and [`CallSession::toggle_subtitles`]
//! the one way to turn subtitles on or off (1a's UI never calls it, FR-019); both run under
//! the one session lock.
//!
//! The two track captures are open exactly while the session is active. Whichever toggle
//! comes first opens them (the system track, the default output's loopback, then the mic);
//! the other toggle reuses them, and they close when both toggles are off. Only a record
//! start creates a call folder through [`CallStore`]: subtitles alone never reach the store,
//! the root probe, the Calls tab or the recording indicator, so nothing reaches the disk
//! while record is off (FR-019).
//!
//! One writer thread per capture lifetime drains both track channels. While record is on
//! it places each track through a [`Timeline`] into the Ogg Opus encoders, publishes
//! indicator ticks and keeps channel drops in the log and in `meta.json`; while subtitles
//! is on it hands each block, downmixed to mono at 16 kHz, to the [`SubtitleConsumer`]. A toggle that changes what a running writer does is a
//! synchronous command: every block queued before the toggle returns is handled under the
//! old state, every later one under the new state.
//!
//! Lock rules: the indicator port is called only with the session lock held; the shell
//! port only after it is released. The call session has no Esc input and shares no state
//! with the dictation session.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::audio::{mix_to_mono, SAMPLE_RATE};
use crate::call::capture::{
    CaptureStamp, ChannelSink, Report, Step, Timeline, TrackDevice, TrackEvent, TrackReceiver,
    TrackSink, TrackSource,
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
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};

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

/// Where live subtitle audio goes (FR-019; the subtitle engine is 1b). Called on the
/// writer thread, without the session lock, only while subtitles is on: 16 kHz mono
/// samples of one track, in capture order. It must not block for long, or the track
/// channels fill and drop.
pub trait SubtitleConsumer: Send + Sync {
    fn audio(&self, track: Track, samples: &[f32]);
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
    /// Fed while subtitles is on.
    pub subtitles: Arc<dyn SubtitleConsumer>,
}

/// What a `toggle_record` or `toggle_subtitles` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggled {
    Started,
    /// The toggle was already on: nothing changed.
    AlreadyOn,
    /// The start was refused at the first failing part (system, mic, root).
    Refused {
        part: RefusedPart,
        reason: RefusedReason,
    },
    Stopped,
    /// The toggle was already off: nothing changed.
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

/// The session state. `captures` is `Some` exactly while `call.is_some() || subtitles`
/// (the data-model's `active`); `call` is `Some` exactly while record is on.
#[derive(Default)]
struct Inner {
    captures: Option<Captures>,
    call: Option<Call>,
    subtitles: bool,
}

/// The two open captures and the writer thread draining them.
struct Captures {
    mic: Box<dyn CaptureHandle>,
    system: Box<dyn CaptureHandle>,
    mic_id: EndpointId,
    system_id: EndpointId,
    /// The warnings of the opens (`MicChanged`), shown by every call on these captures.
    warnings: Vec<CallWarning>,
    writer: JoinHandle<WriterEnd>,
    control: Arc<Control>,
    commands: Sender<Envelope>,
}

/// The recording call (record on).
struct Call {
    lang: UiLanguage,
    warnings: Vec<IndicatorWarning>,
    /// The end of the reminder; `None` once hidden or when the reminder is off.
    reminder_until: Option<Instant>,
    dir: PathBuf,
    /// The meta written at create: the fallback when the writer thread is gone.
    meta: CallMeta,
}

/// The flags the session sets for the writer thread.
#[derive(Default)]
struct Control {
    /// The captures are closing: drain what is queued, then end.
    stop: AtomicBool,
    /// The session holds its lock while it waits for the writer: skip the tick.
    hold: AtomicBool,
}

/// The writer thread is gone (it panicked); what it held is lost.
struct WriterGone;

/// A refusal to report after the lock.
struct Refusal {
    part: RefusedPart,
    reason: RefusedReason,
}

/// The tracks a start opened, before a writer drains them.
struct Opened {
    mic: Box<dyn CaptureHandle>,
    system: Box<dyn CaptureHandle>,
    mic_id: EndpointId,
    system_id: EndpointId,
    warnings: Vec<CallWarning>,
    mic_rx: TrackReceiver,
    system_rx: TrackReceiver,
    /// The source's clock just before the opens: where the timelines start.
    start: CaptureStamp,
}

impl Opened {
    fn stop(self) {
        stop_handle(self.mic);
        stop_handle(self.system);
    }
}

impl Captures {
    /// Runs `command` on the writer and waits for its answer (under the session lock).
    fn command(&self, command: Command) -> Result<Option<Box<Recording>>, WriterGone> {
        self.control.hold.store(true, Ordering::SeqCst);
        let (done, answer) = mpsc::channel();
        let result = match self.commands.send(Envelope { command, done }) {
            Ok(()) => answer.recv().map_err(|_| WriterGone),
            Err(_) => Err(WriterGone),
        };
        self.control.hold.store(false, Ordering::SeqCst);
        result
    }

    /// Closes the captures: both stopped, the writer drained and joined. Returns the
    /// recording the writer still held.
    fn close(self) -> Result<Option<Box<Recording>>, WriterGone> {
        stop_handle(self.mic);
        stop_handle(self.system);
        self.control.stop.store(true, Ordering::SeqCst);
        self.writer
            .join()
            .map(|end| end.rec)
            .map_err(|_| WriterGone)
    }
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

    /// Turns subtitles on or off (FR-019; 1a's UI never calls it). On opens the two
    /// captures unless record already has them (a refusal is `Refused { part: System |
    /// Mic, .. }` and leaves subtitles off) and feeds the [`SubtitleConsumer`]; it never
    /// reaches the call store, the root probe, the Calls tab, the indicator or the log.
    /// Off stops the feed and closes the captures unless record is on.
    pub fn toggle_subtitles(&self, on: bool, _source: CallSource) -> Toggled {
        let mut inner = self.lock();
        if on {
            if inner.subtitles {
                return Toggled::AlreadyOn;
            }
            let other = Toggled::Refused {
                part: RefusedPart::System,
                reason: RefusedReason::Other,
            };
            match inner.captures.as_ref() {
                Some(captures) => {
                    if captures.command(Command::Subtitles(true)).is_err() {
                        return other;
                    }
                }
                None => {
                    let settings = self.deps.settings.snapshot();
                    let opened = match self.open_tracks(&settings) {
                        Ok(opened) => opened,
                        Err(r) => {
                            return Toggled::Refused {
                                part: r.part,
                                reason: r.reason,
                            }
                        }
                    };
                    match self.spawn(opened, None, true) {
                        Some(captures) => inner.captures = Some(captures),
                        None => return other,
                    }
                }
            }
            inner.subtitles = true;
            Toggled::Started
        } else {
            if !inner.subtitles {
                return Toggled::AlreadyOff;
            }
            inner.subtitles = false;
            if inner.call.is_some() {
                if let Some(captures) = inner.captures.as_ref() {
                    // A writer that is gone feeds nobody either.
                    let _ = captures.command(Command::Subtitles(false));
                }
            } else if let Some(captures) = inner.captures.take() {
                // No recording rides on these captures: nothing is lost with the writer.
                let _ = captures.close();
            }
            Toggled::Stopped
        }
    }

    /// Stops the recording call for `reason`; `true` when a call was recording. Subtitles,
    /// when on, go on with the captures.
    pub fn stop(&self, reason: StopReason) -> bool {
        let stopped = {
            let mut inner = self.lock();
            match inner.call.take() {
                Some(call) => {
                    let rec = if inner.subtitles {
                        inner
                            .captures
                            .as_ref()
                            .and_then(|c| c.command(Command::Detach).ok().flatten())
                    } else {
                        inner.captures.take().and_then(|c| c.close().ok().flatten())
                    };
                    self.finish(call, rec, reason);
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
        self.lock().call.is_some()
    }

    pub fn is_subtitles(&self) -> bool {
        self.lock().subtitles
    }

    /// `None` while the session lock is held (by any thread), else whether a call records.
    pub fn try_is_recording(&self) -> Option<bool> {
        match self.shared.inner.try_lock() {
            Ok(inner) => Some(inner.call.is_some()),
            Err(TryLockError::Poisoned(p)) => Some(p.into_inner().call.is_some()),
            Err(TryLockError::WouldBlock) => None,
        }
    }

    /// Applies the reminder's end on the monotonic clock: once it is due, one state hides
    /// the reminder and the hint.
    pub fn tick(&self) {
        let mut inner = self.lock();
        let Some(call) = inner.call.as_mut() else {
            return;
        };
        let Some(until) = call.reminder_until else {
            return;
        };
        if self.deps.mono.now() < until {
            return;
        }
        call.reminder_until = None;
        let state = IndicatorState::recording(call.lang, false, call.warnings.clone());
        self.deps.indicator.set_state(&state);
    }

    /// When [`CallSession::tick`] has something to do: the reminder's end, or `None`.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.lock().call.as_ref().and_then(|c| c.reminder_until)
    }

    /// Holds the writer thread off the channels until the guard is dropped. Returns once
    /// the writer is parked, so blocks fed while the guard lives stay queued (or drop).
    /// Without open captures it returns at once. A toggle while the guard lives waits for
    /// the writer, so drop the guard first.
    #[cfg(any(test, feature = "test-fakes"))]
    pub fn pause_writer(&self) -> WriterPause {
        let running = self.lock().captures.is_some();
        let pause = &self.shared.pause;
        let mut state = pause.lock();
        state.requested = true;
        while running && !state.parked {
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
            if inner.call.is_some() {
                return Toggled::AlreadyOn;
            }
            self.deps.log.write(LogEvent::CallRequested { source });
            match self.open_call(&mut inner) {
                Ok(call) => {
                    inner.call = Some(call);
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

    /// The record start under the lock: system, mic (unless subtitles already opened
    /// them), root, then the writer and the indicator. The only path to [`CallStore`].
    fn open_call(&self, inner: &mut Inner) -> Result<Call, Refusal> {
        let settings = self.deps.settings.snapshot();
        let root_other = Refusal {
            part: RefusedPart::Root,
            reason: RefusedReason::Other,
        };
        let (dir, meta, warnings, mic_id, system_id, mono_start);
        match inner.captures.as_ref() {
            Some(captures) => {
                // Subtitles run: record shares their captures (no second open).
                let start = self.deps.source.now();
                let files = self.create_files(&settings, &captures.mic_id, &captures.system_id)?;
                mono_start = self.deps.mono.now();
                dir = files.dir().to_path_buf();
                meta = files.meta().clone();
                if captures
                    .command(Command::Attach(recording(files, start, mono_start)))
                    .is_err()
                {
                    // No thread to write the call: refused like a create failure after
                    // the probe, and the folder made for it is removed.
                    let _ = fs::remove_dir_all(&dir);
                    return Err(root_other);
                }
                warnings = captures.warnings.clone();
                mic_id = captures.mic_id.clone();
                system_id = captures.system_id.clone();
            }
            None => {
                let opened = self.open_tracks(&settings)?;
                let files = match self.create_files(&settings, &opened.mic_id, &opened.system_id) {
                    Ok(files) => files,
                    Err(refusal) => {
                        opened.stop();
                        return Err(refusal);
                    }
                };
                mono_start = self.deps.mono.now();
                dir = files.dir().to_path_buf();
                meta = files.meta().clone();
                warnings = opened.warnings.clone();
                mic_id = opened.mic_id.clone();
                system_id = opened.system_id.clone();
                let rec = recording(files, opened.start, mono_start);
                match self.spawn(opened, Some(rec), false) {
                    Some(captures) => inner.captures = Some(captures),
                    None => {
                        let _ = fs::remove_dir_all(&dir);
                        return Err(root_other);
                    }
                }
            }
        }
        Ok(self.begin_call(
            &settings, warnings, mic_id, system_id, dir, meta, mono_start,
        ))
    }

    /// Opens the system track, then the mic; a mic failure stops the system track.
    fn open_tracks(&self, settings: &Settings) -> Result<Opened, Refusal> {
        let (sink, mic_rx, system_rx) = ChannelSink::new(self.deps.channel_capacity);
        let sink: Arc<dyn TrackSink> = Arc::new(sink);
        let start = self.deps.source.now();
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
        let (mic, mic_id) = match self.open_mic(settings, sink, &mut warnings) {
            Ok(opened) => opened,
            Err(e) => {
                stop_handle(system);
                return Err(Refusal {
                    part: RefusedPart::Mic,
                    reason: e.into(),
                });
            }
        };
        Ok(Opened {
            mic,
            system,
            mic_id,
            system_id,
            warnings,
            mic_rx,
            system_rx,
            start,
        })
    }

    /// The call folder through [`CallStore::create_with`] (the probe lives there), with
    /// the #243 mapping of its failures.
    fn create_files(
        &self,
        settings: &Settings,
        mic_id: &EndpointId,
        system_id: &EndpointId,
    ) -> Result<ActiveCallFiles<File>, Refusal> {
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
        self.deps
            .store
            .create_with(&settings.calls, &new, |path| open_track(path))
            .map_err(|e| {
                let reason = match e {
                    CreateError::Root(reason) => root_reason(reason),
                    CreateError::Storage(kind) => {
                        self.deps
                            .log
                            .write(LogEvent::CallStorageFailed { kind, os: None });
                        RefusedReason::Other
                    }
                };
                Refusal {
                    part: RefusedPart::Root,
                    reason,
                }
            })
    }

    /// The started call: the provider notice, the indicator, the tray flag, `call_started`.
    #[allow(clippy::too_many_arguments)]
    fn begin_call(
        &self,
        settings: &Settings,
        mut warnings: Vec<CallWarning>,
        mic_id: EndpointId,
        system_id: EndpointId,
        dir: PathBuf,
        meta: CallMeta,
        mono_start: Instant,
    ) -> Call {
        if let Some(host) = self.new_provider_host(settings) {
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
        let reminder_until = settings.calls.reminder.then(|| mono_start + REMINDER);
        let state = IndicatorState::recording(lang, reminder_until.is_some(), warnings.clone());
        self.deps.indicator.set_state(&state);
        self.deps.indicator.set_call_recording(true);
        self.deps.log.write(LogEvent::CallStarted {
            mic: mic_id,
            system: system_id,
            // Placeholder until the communications-device check (G35).
            comms_differs: false,
        });
        Call {
            lang,
            warnings,
            reminder_until,
            dir,
            meta,
        }
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

    /// Starts the writer over the opened tracks. On failure both tracks are stopped.
    fn spawn(
        &self,
        opened: Opened,
        rec: Option<Box<Recording>>,
        subtitles: bool,
    ) -> Option<Captures> {
        let Opened {
            mic,
            system,
            mic_id,
            system_id,
            warnings,
            mic_rx,
            system_rx,
            ..
        } = opened;
        let control = Arc::new(Control::default());
        let (commands, inbox) = mpsc::channel();
        let writer = Writer {
            shared: self.shared.clone(),
            log: self.deps.log.clone(),
            mono: self.deps.mono.clone(),
            consumer: self.deps.subtitles.clone(),
            control: control.clone(),
            inbox,
            mic_rx,
            system_rx,
            sub: subtitles.then(Box::default),
            rec,
        };
        match thread::Builder::new()
            .name("voicen-call-writer".to_string())
            .spawn(move || writer.run())
        {
            Ok(writer) => Some(Captures {
                mic,
                system,
                mic_id,
                system_id,
                warnings,
                writer,
                control,
                commands,
            }),
            Err(_) => {
                stop_handle(mic);
                stop_handle(system);
                None
            }
        }
    }

    // ---- stop -----------------------------------------------------------------------

    /// The record stop under the lock, after the writer handed the recording back: both
    /// files finished, `meta.json` ready-to-transcribe, `call_stopped`, the indicator off.
    /// `rec` is `None` when the writer thread is gone: that is logged, and the meta of the
    /// create is finished with no audio.
    fn finish(&self, call: Call, rec: Option<Box<Recording>>, reason: StopReason) {
        let Call { dir, meta, .. } = call;
        let (mut meta, mic_written, system_written) = match rec {
            Some(rec) => {
                let Recording {
                    mic, system, meta, ..
                } = *rec;
                let written = (mic.timeline.written(), system.timeline.written());
                for enc in [mic.enc, system.enc].into_iter().flatten() {
                    if let Err(e) = enc.finish() {
                        self.deps.log.write(LogEvent::CallStorageFailed {
                            kind: storage_kind(e.op),
                            os: None,
                        });
                    }
                }
                (meta, written.0, written.1)
            }
            None => {
                // The writer thread panicked: its encoders are lost unfinished.
                self.deps.log.write(LogEvent::CallStorageFailed {
                    kind: StorageKind::Write,
                    os: None,
                });
                (meta, 0, 0)
            }
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
    /// A session dropped while active lets its writer thread end; the files stay as
    /// written (a later start finds the call `recording` and recovers it).
    fn drop(&mut self) {
        if let Some(captures) = self.lock().captures.as_ref() {
            captures.control.stop.store(true, Ordering::SeqCst);
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

/// The writer's side of a new call: both encoders, a timeline per track from `start`.
fn recording(
    files: ActiveCallFiles<File>,
    start: CaptureStamp,
    mono_start: Instant,
) -> Box<Recording> {
    let dir = files.dir().to_path_buf();
    let meta = files.meta().clone();
    let ActiveCallFiles { mic, system, .. } = files;
    let lane = |track: Track, enc: OpusOgg<File>| RecLane {
        timeline: Timeline::new(track, start),
        enc: Some(enc),
        level: 0.0,
    };
    Box::new(Recording {
        mic: lane(Track::Mic, mic),
        system: lane(Track::System, system),
        meta,
        dir,
        mono_start,
        last_tick: None,
    })
}

// ---- the writer thread ------------------------------------------------------------------

/// What the session asks a running writer.
enum Command {
    /// Record on: write the following blocks into this call.
    Attach(Box<Recording>),
    /// Record off: hand the call back (the answer).
    Detach,
    /// Subtitles on (a fresh feed per track) or off.
    Subtitles(bool),
}

struct Envelope {
    command: Command,
    done: Sender<Option<Box<Recording>>>,
}

/// One track of a recording call: its timeline and its encoder (`None` once a write
/// failed).
struct RecLane {
    timeline: Timeline,
    enc: Option<OpusOgg<File>>,
    /// The latest 100 ms level.
    level: f32,
}

/// The recording call on the writer.
struct Recording {
    mic: RecLane,
    system: RecLane,
    meta: CallMeta,
    dir: PathBuf,
    mono_start: Instant,
    last_tick: Option<Instant>,
}

impl Recording {
    fn lane(&mut self, track: Track) -> &mut RecLane {
        match track {
            Track::Mic => &mut self.mic,
            Track::System => &mut self.system,
        }
    }
}

struct Writer {
    shared: Arc<Shared>,
    log: Arc<Log>,
    mono: Arc<dyn MonotonicClock>,
    consumer: Arc<dyn SubtitleConsumer>,
    control: Arc<Control>,
    inbox: Receiver<Envelope>,
    mic_rx: TrackReceiver,
    system_rx: TrackReceiver,
    /// `Some` while subtitles is on: the mic feed, then the system feed.
    sub: Option<Box<(SubtitleFeed, SubtitleFeed)>>,
    /// `Some` while record is on.
    rec: Option<Box<Recording>>,
}

/// What the writer hands back when the captures close.
struct WriterEnd {
    rec: Option<Box<Recording>>,
}

impl Writer {
    fn run(mut self) -> WriterEnd {
        loop {
            self.shared.pause.park_if_asked();
            // Read before the drain: every event queued before the stop is drained.
            let stopping = self.control.stop.load(Ordering::SeqCst);
            while let Ok(envelope) = self.inbox.try_recv() {
                self.command(envelope);
            }
            if !self.drain() {
                if stopping {
                    break;
                }
                thread::sleep(IDLE);
            }
        }
        self.end_recording();
        WriterEnd { rec: self.rec }
    }

    /// Record ends: both timelines hand over what their resamplers still hold, so the
    /// files keep the audio up to the last block.
    fn end_recording(&mut self) {
        for track in [Track::Mic, Track::System] {
            let Some(rec) = self.rec.as_mut() else {
                return;
            };
            let lane = rec.lane(track);
            let at_ms = lane.timeline.written() / SAMPLES_PER_MS;
            let step = lane.timeline.flush();
            self.apply(track, step, at_ms);
        }
    }

    /// Everything queued so far goes under the old state, then the command applies.
    fn command(&mut self, envelope: Envelope) {
        while self.drain() {}
        let answer = match envelope.command {
            Command::Attach(rec) => {
                self.rec = Some(rec);
                None
            }
            Command::Detach => {
                self.end_recording();
                self.rec.take()
            }
            Command::Subtitles(on) => {
                self.sub = on.then(Box::default);
                None
            }
        };
        // A session that stopped waiting has nothing to receive.
        let _ = envelope.done.send(answer);
    }

    /// One pass over both channels; `true` when an event was handled.
    fn drain(&mut self) -> bool {
        let mut busy = false;
        for track in [Track::Mic, Track::System] {
            loop {
                let rx = match track {
                    Track::Mic => &self.mic_rx,
                    Track::System => &self.system_rx,
                };
                let Some(event) = rx.try_recv() else {
                    break;
                };
                busy = true;
                self.handle(track, event);
            }
        }
        busy
    }

    fn handle(&mut self, track: Track, event: TrackEvent) {
        if let (
            Some(sub),
            TrackEvent::Block {
                interleaved,
                rate,
                channels,
                ..
            },
        ) = (self.sub.as_mut(), &event)
        {
            let (mic, system) = &mut **sub;
            let feed = match track {
                Track::Mic => mic,
                Track::System => system,
            };
            let samples = feed.block(interleaved, *rate, *channels);
            if !samples.is_empty() {
                self.consumer.audio(track, samples);
            }
        }
        let Some(rec) = self.rec.as_mut() else {
            return;
        };
        let lane = rec.lane(track);
        // The track position before this step: where a drop happened.
        let at_ms = lane.timeline.written() / SAMPLES_PER_MS;
        if let Some(step) = step_of(&mut lane.timeline, &event) {
            self.apply(track, step, at_ms);
        }
    }

    fn apply(&mut self, track: Track, step: Step, at_ms: u64) {
        let Some(rec) = self.rec.as_mut() else {
            return;
        };
        let lane = rec.lane(track);
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
            if let Some(rec) = self.rec.as_mut() {
                rec.lane(track).level = *level;
            }
            self.maybe_tick();
        }
    }

    /// One `call_track_dropped` line and one `drops[]` entry per `Report::Dropped` (#244).
    fn dropped(&mut self, track: Track, blocks: u32, ms: u32, at_ms: u64) {
        let Some(rec) = self.rec.as_mut() else {
            return;
        };
        self.log.write(LogEvent::CallTrackDropped {
            track,
            blocks,
            ms,
            at_ms,
        });
        rec.meta.drops.push(TrackDrop {
            at_ms,
            track,
            blocks,
            ms,
        });
        if let Err(e) = write_meta(&rec.dir, &rec.meta) {
            // The call goes on; the stop writes the full list again.
            self.log.write(LogEvent::CallStorageFailed {
                kind: StorageKind::Meta,
                os: e.raw_os_error(),
            });
        }
    }

    /// Publishes the levels under the session lock, at most every 100 ms of the mono
    /// clock. A session that holds the lock to wait for this thread (a stop joining it,
    /// a command) makes it skip the tick.
    fn maybe_tick(&mut self) {
        let Some(rec) = self.rec.as_mut() else {
            return;
        };
        let now = self.mono.now();
        if let Some(last) = rec.last_tick {
            if now.saturating_duration_since(last) < TICK_EVERY {
                return;
            }
        }
        let tick = IndicatorTick {
            elapsed_ms: millis(now.saturating_duration_since(rec.mono_start)),
            mic_level: rec.mic.level,
            system_level: rec.system.level,
            mic_silent: false,
            system_silent: false,
        };
        loop {
            if self.control.stop.load(Ordering::SeqCst) || self.control.hold.load(Ordering::SeqCst)
            {
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
        rec.last_tick = Some(now);
    }
}

/// One track's feed to the subtitle consumer: each block downmixed to mono and brought
/// to 16 kHz, in capture order, with no placement on the call's axis (a subtitle engine
/// needs speech, not alignment). A 16 kHz block passes as is; another rate goes through a
/// streaming FFT resampler (its filter delay holds back a few ms). A block that cannot be
/// converted gives nothing.
#[derive(Default)]
struct SubtitleFeed {
    mono: Vec<f32>,
    out: Vec<f32>,
    conv: Option<SubtitleConv>,
}

struct SubtitleConv {
    rate: u32,
    resampler: Fft<f32>,
    /// Mono input not yet handed to the resampler.
    fifo: Vec<f32>,
    buf: Vec<f32>,
}

impl SubtitleFeed {
    fn block(&mut self, interleaved: &[f32], rate: u32, channels: u16) -> &[f32] {
        self.out.clear();
        self.mono.clear();
        if rate == 0 || mix_to_mono(interleaved, channels, &mut self.mono).is_err() {
            return &self.out;
        }
        if rate == SAMPLE_RATE {
            std::mem::swap(&mut self.out, &mut self.mono);
            return &self.out;
        }
        if self.conv.as_ref().is_none_or(|c| c.rate != rate) {
            self.conv = SubtitleConv::new(rate);
        }
        let Some(conv) = self.conv.as_mut() else {
            return &self.out;
        };
        conv.fifo.extend_from_slice(&self.mono);
        if conv.run(&mut self.out).is_err() {
            // The next block starts a fresh resampler.
            self.conv = None;
        }
        &self.out
    }
}

impl SubtitleConv {
    fn new(rate: u32) -> Option<SubtitleConv> {
        let rate_in = usize::try_from(rate).ok()?;
        let rate_out = usize::try_from(SAMPLE_RATE).ok()?;
        let chunk = (rate_in / 100).max(1);
        let resampler = Fft::<f32>::new(rate_in, rate_out, chunk, 1, FixedSync::Input).ok()?;
        let buf = vec![0.0; resampler.output_frames_max()];
        Some(SubtitleConv {
            rate,
            resampler,
            fifo: Vec::new(),
            buf,
        })
    }

    /// Resamples every full input chunk of the queue, appending the output to `dst`.
    fn run(&mut self, dst: &mut Vec<f32>) -> Result<(), ()> {
        let mut off = 0;
        let mut result = Ok(());
        loop {
            let need = self.resampler.input_frames_next();
            let Some(input) = self.fifo.get(off..off + need) else {
                break;
            };
            let produced = (|| {
                let input = InterleavedSlice::new(input, 1, need).map_err(|_| ())?;
                let frames = self.buf.len();
                let mut output =
                    InterleavedSlice::new_mut(&mut self.buf, 1, frames).map_err(|_| ())?;
                let (_, produced) = self
                    .resampler
                    .process_into_buffer(&input, &mut output, None)
                    .map_err(|_| ())?;
                Ok::<usize, ()>(produced)
            })();
            match produced {
                Ok(n) => dst.extend_from_slice(self.buf.get(..n).unwrap_or(&[])),
                Err(()) => {
                    result = Err(());
                    break;
                }
            }
            off += need;
        }
        self.fifo.drain(..off);
        result
    }
}

/// One event through one timeline; `None` for a block the timeline cannot place (it
/// emitted nothing; the next one starts afresh at its own stamp).
fn step_of(timeline: &mut Timeline, event: &TrackEvent) -> Option<Step> {
    match event {
        TrackEvent::Block {
            interleaved,
            rate,
            channels,
            at,
        } => timeline.push(interleaved, *rate, *channels, *at).ok(),
        TrackEvent::Lost { cause, at } => Some(timeline.lost(*cause, *at)),
        TrackEvent::Dropped { blocks, ms } => Some(timeline.dropped(*blocks, *ms)),
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
