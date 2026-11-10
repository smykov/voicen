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
//! Every stop other than a user, hotkey or exit stop starts in [`CallSession::tick`]
//! (T-144): a failed track write (the writer drops both encoders at the first failure and
//! writes nothing more of the call, `meta.json` included) stops with `storage_error` and
//! leaves `meta.json` at `recording`; 4 h of monotonic time stop with `max_length`; the free
//! space of the call folder, polled every 10 s, warns once below 1 GB and stops with
//! `low_disk` below 200 MB. [`CallSession::next_deadline`] is never more than 10 s ahead
//! while a call records, so the shell's timer drives these.
//!
//! Lock rules: the indicator port is called only with the session lock held; the shell
//! port only after it is released. The call session has no Esc input and shares no state
//! with the dictation session.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::audio::{mix_to_mono, AudioBuffer, SAMPLE_RATE};
use crate::call::capture::{
    CaptureStamp, ChannelSink, CommunicationsCheck, DeviceEvent, DeviceEvents, DeviceEventsFactory,
    LossCause, Report, Step, Timeline, TrackDevice, TrackEvent, TrackReceiver, TrackSink,
    TrackSource,
};
use crate::call::codec::{OpusOgg, SyncWrite, TrackEncoder};
use crate::call::events::{
    CallNotice, CallSource, CallState, CallWarning, DeviceCause, EndpointId, RefusedPart,
    RefusedReason, SilentRule, StopReason, StorageKind, Track,
};
use crate::call::indicator::{IndicatorState, IndicatorTick, WarningList};
use crate::call::storage::{
    started_at, storage_kind, write_meta, ActiveCallFiles, CallMeta, CallStore, CreateError,
    DeviceChange, NewCall, TrackDrop,
};
use crate::call::OpenError;
use crate::clock::{Clock, LocalOffset, MonotonicClock};
use crate::diag::format::LocalTime;
use crate::diag::{Log, LogEvent};
use crate::i18n::UiLanguage;
use crate::local_models::download::DiskSpace;
use crate::platform::{CaptureHandle, DeviceId};
use crate::settings::roots::UnwritableReason;
use crate::settings::service::{SettingsService, WarnedHostFailed};
use crate::settings::url::{check_base_url, provider_host, ProviderHost};
use crate::settings::Settings;
use crate::vad::EnergyDetector;
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
/// FR-012: a lost track is reopened within this, or declared silent and retried this often.
const REOPEN_WITHIN: Duration = Duration::from_secs(2);
/// A live track that delivered nothing for this long is advanced with the other track (D3).
const STALL: Duration = Duration::from_millis(500);
/// A reopened stream that sends nothing for this long has its gap measured where it stands.
const SETTLE: Duration = Duration::from_secs(2);
/// FR-013: one speech verdict per second of placed audio.
const SECOND_SAMPLES: usize = 16_000;
/// FR-013: more than this many silent seconds in a row while the other track spoke.
const CANT_HEAR_AFTER_S: u64 = 60;
/// FR-017 (research R19): the call folder's free space is polled this often while recording.
const DISK_POLL_EVERY: Duration = Duration::from_secs(10);
/// FR-017: below this much free space the call shows one `low_disk` warning.
const LOW_DISK_WARN_BYTES: u64 = 1_000_000_000;
/// FR-017: below this much free space the call stops with saving.
const LOW_DISK_STOP_BYTES: u64 = 200_000_000;
/// FR-018: a call stops with `max_length` after this much time on the monotonic clock.
const MAX_LENGTH: Duration = Duration::from_secs(4 * 3_600);

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

/// An open track file: what the opener hands to the encoder.
pub type TrackFile = Box<dyn SyncWrite + Send>;

/// Opens a track file inside the call folder ([`CallStore::create_with`]); a test hands
/// in a fault-injecting writer (T-144).
pub type TrackOpener = Arc<dyn Fn(&Path) -> io::Result<TrackFile> + Send + Sync>;

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
    /// The device-change stream of each capture lifetime, opened on its device thread.
    pub devices: Arc<dyn DeviceEventsFactory>,
    /// FR-010: asked once per record start, before `call_started`.
    pub comms: Arc<dyn CommunicationsCheck>,
    /// FR-017: the free space of the call folder, polled by [`CallSession::tick`] every
    /// 10 s while a call records.
    pub disk: Arc<dyn DiskSpace>,
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

/// The two open captures: the device thread that owns their handles, and the writer
/// thread draining them.
struct Captures {
    /// The endpoints of the first opens (the call folder's `meta.json` names them).
    mic_id: EndpointId,
    system_id: EndpointId,
    /// The warnings of the opens (`MicChanged`), shown by every call on these captures.
    warnings: Vec<CallWarning>,
    device: JoinHandle<()>,
    writer: JoinHandle<WriterEnd>,
    control: Arc<Control>,
    commands: Sender<Envelope>,
}

/// The recording call (record on).
struct Call {
    lang: UiLanguage,
    /// The call's one warning list (D4): raised by the session and the writer, dismissed
    /// by [`CallSession::dismiss_warning`], always under the session lock.
    warnings: WarningList,
    /// The end of the reminder; `None` once hidden or when the reminder is off.
    reminder_until: Option<Instant>,
    dir: PathBuf,
    /// The meta written at create: the fallback when the writer thread is gone.
    meta: CallMeta,
    /// The start on the monotonic clock (the 4 h limit counts from here).
    mono_start: Instant,
    /// When `tick` polls the free space next; the first poll is due at the start.
    next_poll: Instant,
    /// The `low_disk` warning was raised in this call (at most once per call, so a
    /// dismissed one does not come back).
    low_disk_warned: bool,
}

impl Call {
    /// What the indicator shows for this call now.
    fn state(&self) -> IndicatorState {
        IndicatorState::recording(
            self.lang,
            self.reminder_until.is_some(),
            self.warnings.items(self.lang),
        )
    }
}

/// The flags the session sets for the writer and device threads.
#[derive(Default)]
struct Control {
    /// The captures are closing: drain what is queued, then end.
    stop: AtomicBool,
    /// The session holds its lock while it waits for the writer: skip the tick.
    hold: AtomicBool,
    /// The captures are closing: the device thread stops both handles and ends.
    devices: AtomicBool,
    /// A track write of the attached call failed: the writer dropped both encoders and
    /// writes nothing more of the call; the next `tick` stops it (`storage_error`).
    failed: AtomicBool,
}

/// The writer thread is gone (it panicked); what it held is lost.
struct WriterGone;

/// A refusal to report after the lock.
struct Refusal {
    part: RefusedPart,
    reason: RefusedReason,
}

/// One open capture: its handle, the sink its callback feeds and the endpoint it reports.
struct OpenTrack {
    handle: Box<dyn CaptureHandle>,
    sink: Arc<OpenSink>,
    endpoint: EndpointId,
}

/// The tracks a start opened, before a writer drains them.
struct Opened {
    mic: OpenTrack,
    system: OpenTrack,
    /// The saved mic the mic track opened on; `None` when it is on the default input.
    mic_selected: Option<DeviceId>,
    warnings: Vec<CallWarning>,
    /// The channel both tracks feed (through their [`OpenSink`]s).
    channel: Arc<dyn TrackSink>,
    mic_rx: TrackReceiver,
    system_rx: TrackReceiver,
    /// The source's clock just before the opens: where the timelines start.
    start: CaptureStamp,
}

impl Opened {
    fn stop(self) {
        stop_handle(self.mic.handle);
        stop_handle(self.system.handle);
    }
}

/// The sink of one open capture, in front of the shared [`ChannelSink`]. A stream that is
/// retired (its device changed, or it died) is cut off here, so nothing it still delivers
/// reaches the channel after the track's `Lost`. The device thread reads `lost` to learn
/// that the stream died (D1).
struct OpenSink {
    channel: Arc<dyn TrackSink>,
    live: AtomicBool,
    lost: AtomicBool,
}

impl OpenSink {
    fn new(channel: Arc<dyn TrackSink>) -> Arc<OpenSink> {
        Arc::new(OpenSink {
            channel,
            live: AtomicBool::new(true),
            lost: AtomicBool::new(false),
        })
    }

    /// Cuts the stream off; `true` when it was still live (no `Lost` reached the channel).
    fn retire(&self) -> bool {
        self.live.swap(false, Ordering::SeqCst)
    }
}

impl TrackSink for OpenSink {
    fn block(&self, track: Track, interleaved: &[f32], rate: u32, channels: u16, at: CaptureStamp) {
        if self.live.load(Ordering::SeqCst) {
            self.channel.block(track, interleaved, rate, channels, at);
        }
    }

    fn lost(&self, track: Track, cause: LossCause, at: CaptureStamp) {
        if self.retire() {
            self.channel.lost(track, cause, at);
            self.lost.store(true, Ordering::SeqCst);
        }
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

    /// Closes the captures: the device thread stops both handles (after an open it is
    /// running, if any) and is joined, then the writer is drained and joined. Returns the
    /// recording the writer still held.
    fn close(self) -> Result<Option<Box<Recording>>, WriterGone> {
        // The session holds its lock until the writer is joined: no tick, no warning.
        self.control.hold.store(true, Ordering::SeqCst);
        self.control.devices.store(true, Ordering::SeqCst);
        // A device thread that panicked dropped its handles, which closes them.
        let _ = self.device.join();
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

    /// Stops the recording call for `reason`; `true` when a call was recording. A call
    /// whose track write failed stops as `storage_error` whatever `reason` asked (the
    /// log line and the notice say so, I12). Subtitles, when on, go on with the captures.
    pub fn stop(&self, reason: StopReason) -> bool {
        let stopped = {
            let mut inner = self.lock();
            self.stop_locked(&mut inner, reason)
        };
        if let Some(effective) = stopped {
            self.notice_stopped(effective);
        }
        stopped.is_some()
    }

    /// The stop under the session lock: the writer hands the call back (or the captures
    /// close), then [`CallSession::finish`]. The reason the call stopped with (see
    /// `finish`); `None` when no call was recording.
    fn stop_locked(&self, inner: &mut Inner, reason: StopReason) -> Option<StopReason> {
        let call = inner.call.take()?;
        let rec = if inner.subtitles {
            inner.captures.as_ref().and_then(|c| {
                let rec = c.command(Command::Detach).ok().flatten();
                c.control.failed.store(false, Ordering::SeqCst);
                rec
            })
        } else {
            inner.captures.take().and_then(|c| c.close().ok().flatten())
        };
        Some(self.finish(call, rec, reason))
    }

    /// The one notice of a stop, after the lock: a stop the session decided itself
    /// (`max_length`, `low_disk`, `storage_error`) says why; any other says the call is
    /// saved and goes to transcription.
    fn notice_stopped(&self, reason: StopReason) {
        let notice = match reason {
            StopReason::MaxLength | StopReason::LowDisk | StopReason::StorageError => {
                CallNotice::Stopped { reason }
            }
            StopReason::User
            | StopReason::Hotkey
            | StopReason::Exit
            | StopReason::SystemEnd
            | StopReason::Sleep => CallNotice::CallSavedTranscribing,
        };
        self.deps.shell.notice(notice);
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

    /// Every stop that is not a user, hotkey or exit stop starts here (T-144), in this
    /// order: a failed track write stops with `storage_error` (meta.json left
    /// `recording`); 4 h on the monotonic clock with `max_length`; the free-space poll of
    /// the call folder (every 10 s, the first at the start) warns once below 1 GB and stops
    /// below 200 MB with `low_disk`, after `call_storage_failed kind=low_disk`; a poll that
    /// fails is skipped until the next one, silently. Then the reminder's end: once it is
    /// due, one state hides the reminder and the hint.
    pub fn tick(&self) {
        let stopped = {
            let mut inner = self.lock();
            let reason = self.due_stop(&mut inner);
            match reason {
                Some(reason) => self.stop_locked(&mut inner, reason),
                None => {
                    self.reminder_tick(&mut inner);
                    None
                }
            }
        };
        if let Some(reason) = stopped {
            self.notice_stopped(reason);
        }
    }

    /// The stop `tick` has to make now, if any; raises the low-disk warning on the way.
    fn due_stop(&self, inner: &mut Inner) -> Option<StopReason> {
        let failed = self.write_failed(inner);
        let call = inner.call.as_mut()?;
        if failed {
            return Some(StopReason::StorageError);
        }
        let now = self.deps.mono.now();
        if now.saturating_duration_since(call.mono_start) >= MAX_LENGTH {
            return Some(StopReason::MaxLength);
        }
        if now < call.next_poll {
            return None;
        }
        call.next_poll = now + DISK_POLL_EVERY;
        // A failed poll is skipped until the next one (it logs nothing).
        let free = self.deps.disk.available_bytes(&call.dir).ok()?;
        if free < LOW_DISK_STOP_BYTES {
            self.deps.log.write(LogEvent::CallStorageFailed {
                kind: StorageKind::LowDisk,
                os: None,
            });
            return Some(StopReason::LowDisk);
        }
        if free < LOW_DISK_WARN_BYTES && !call.low_disk_warned {
            call.low_disk_warned = true;
            if call.warnings.raise(CallWarning::LowDisk).is_some() {
                self.deps.indicator.set_state(&call.state());
            }
        }
        None
    }

    /// The writer reported a failed track write of the recording call.
    fn write_failed(&self, inner: &Inner) -> bool {
        inner.call.is_some()
            && inner
                .captures
                .as_ref()
                .is_some_and(|c| c.control.failed.load(Ordering::SeqCst))
    }

    fn reminder_tick(&self, inner: &mut Inner) {
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
        self.deps.indicator.set_state(&call.state());
    }

    /// Removes the warning `id` from the recording call's list and republishes the
    /// indicator state, under the session lock (D4). `false` when no call records or no
    /// listed warning has that id. The next trigger of the same kind lists it again.
    pub fn dismiss_warning(&self, id: u32) -> bool {
        let mut inner = self.lock();
        let Some(call) = inner.call.as_mut() else {
            return false;
        };
        if !call.warnings.dismiss(id) {
            return false;
        }
        self.deps.indicator.set_state(&call.state());
        true
    }

    /// When [`CallSession::tick`] has something to do while a call records: now after a
    /// failed track write, else the earliest of the reminder's end, the next free-space
    /// poll and the 4 h limit (never more than 10 s ahead). `None` while no call records.
    pub fn next_deadline(&self) -> Option<Instant> {
        let inner = self.lock();
        let failed = self.write_failed(&inner);
        let call = inner.call.as_ref()?;
        if failed {
            return Some(self.deps.mono.now());
        }
        let limit = call.mono_start + MAX_LENGTH;
        Some(
            [call.reminder_until, Some(call.next_poll), Some(limit)]
                .into_iter()
                .flatten()
                .min()
                .unwrap_or(limit),
        )
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
                    self.deps.store.release(&dir);
                    return Err(root_other);
                }
                warnings = captures.warnings.clone();
                mic_id = captures.mic_id.clone();
                system_id = captures.system_id.clone();
            }
            None => {
                let opened = self.open_tracks(&settings)?;
                let files = match self.create_files(
                    &settings,
                    &opened.mic.endpoint,
                    &opened.system.endpoint,
                ) {
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
                mic_id = opened.mic.endpoint.clone();
                system_id = opened.system.endpoint.clone();
                let rec = recording(files, opened.start, mono_start);
                match self.spawn(opened, Some(rec), false) {
                    Some(captures) => inner.captures = Some(captures),
                    None => {
                        let _ = fs::remove_dir_all(&dir);
                        self.deps.store.release(&dir);
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
        let channel: Arc<dyn TrackSink> = Arc::new(sink);
        let start = self.deps.source.now();
        let system = open_track(
            &*self.deps.source,
            Track::System,
            TrackDevice::DefaultOutputLoopback,
            &channel,
        )
        .map_err(|e| Refusal {
            part: RefusedPart::System,
            reason: e.into(),
        })?;
        let mut warnings = Vec::new();
        let (mic, mic_selected) = match self.open_mic(settings, &channel, &mut warnings) {
            Ok(opened) => opened,
            Err(e) => {
                stop_handle(system.handle);
                return Err(Refusal {
                    part: RefusedPart::Mic,
                    reason: e.into(),
                });
            }
        };
        Ok(Opened {
            mic,
            system,
            mic_selected,
            warnings,
            channel,
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
    ) -> Result<ActiveCallFiles<TrackFile>, Refusal> {
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
        // FR-010 (D5): only a difference the OS confirms is one; every Err (E_NOTFOUND:
        // no communications device) is none, and none refuses the start.
        let comms_differs = matches!(self.deps.comms.differs(), Ok(true));
        if comms_differs {
            warnings.push(CallWarning::CommunicationsDeviceDiffers);
        }
        if let Some(host) = self.new_provider_host(settings) {
            // The notice shows even when the host could not be remembered: it then shows
            // again at the next call (FR-031).
            let _ = self.deps.settings.remember_warned_host(&host);
            warnings.push(CallWarning::ProviderNotice { host });
        }
        let mut list = WarningList::new();
        for warning in warnings {
            list.raise(warning);
        }
        let call = Call {
            lang: settings.ui_language,
            warnings: list,
            reminder_until: settings.calls.reminder.then(|| mono_start + REMINDER),
            dir,
            meta,
            mono_start,
            next_poll: mono_start,
            low_disk_warned: false,
        };
        self.deps.indicator.set_state(&call.state());
        self.deps.indicator.set_call_recording(true);
        self.deps.log.write(LogEvent::CallStarted {
            mic: mic_id,
            system: system_id,
            comms_differs,
        });
        call
    }

    /// The mic: the saved one by id, else the default input. A saved mic that is gone
    /// (`NoDevice`) is retried once on the default input with [`CallWarning::MicChanged`].
    /// Also returns the saved mic's id when the track opened on it.
    fn open_mic(
        &self,
        settings: &Settings,
        channel: &Arc<dyn TrackSink>,
        warnings: &mut Vec<CallWarning>,
    ) -> Result<(OpenTrack, Option<DeviceId>), OpenError> {
        let source = &*self.deps.source;
        let default = || open_track(source, Track::Mic, TrackDevice::DefaultInput, channel);
        match &settings.microphone {
            None => Ok((default()?, None)),
            Some(mic) => {
                let id = DeviceId(mic.id.clone());
                match open_track(
                    source,
                    Track::Mic,
                    TrackDevice::Selected(id.clone()),
                    channel,
                ) {
                    Ok(opened) => Ok((opened, Some(id))),
                    Err(OpenError::NoDevice) => {
                        let opened = default()?;
                        warnings.push(CallWarning::MicChanged);
                        Ok((opened, None))
                    }
                    Err(e) => Err(e),
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

    /// Starts the device thread (it owns both handles from here on) and the writer over
    /// the opened tracks. Returns once the device thread has opened its event stream. On
    /// failure both tracks are closed.
    fn spawn(
        &self,
        opened: Opened,
        rec: Option<Box<Recording>>,
        subtitles: bool,
    ) -> Option<Captures> {
        let Opened {
            mic,
            system,
            mic_selected,
            warnings,
            channel,
            mic_rx,
            system_rx,
            ..
        } = opened;
        let mic_id = mic.endpoint.clone();
        let system_id = system.endpoint.clone();
        let control = Arc::new(Control::default());
        let (outcomes_tx, outcomes) = mpsc::channel();
        let devices = DeviceThread {
            source: self.deps.source.clone(),
            mono: self.deps.mono.clone(),
            channel,
            control: control.clone(),
            outcomes: outcomes_tx,
            mic: DeviceLane::new(mic),
            system: DeviceLane::new(system),
            mic_selected,
        };
        let factory = self.deps.devices.clone();
        let (ready_tx, ready) = mpsc::channel();
        // A spawn that fails drops the closure, and with it both handles (closed).
        let device = thread::Builder::new()
            .name("voicen-call-devices".to_string())
            .spawn(move || devices.run(&*factory, ready_tx))
            .ok()?;
        // An Err: the thread is gone (it panicked); the writer still drains what came.
        let _ = ready.recv();
        let (commands, inbox) = mpsc::channel();
        let now = self.deps.mono.now();
        let writer = Writer {
            shared: self.shared.clone(),
            log: self.deps.log.clone(),
            mono: self.deps.mono.clone(),
            source: self.deps.source.clone(),
            consumer: self.deps.subtitles.clone(),
            control: control.clone(),
            inbox,
            outcomes,
            mic_rx,
            system_rx,
            seen: Pair {
                mic: Seen::new(now),
                system: Seen::new(now),
            },
            pending: Vec::new(),
            sub: subtitles.then(Box::default),
            rec,
        };
        match thread::Builder::new()
            .name("voicen-call-writer".to_string())
            .spawn(move || writer.run())
        {
            Ok(writer) => Some(Captures {
                mic_id,
                system_id,
                warnings,
                device,
                writer,
                control,
                commands,
            }),
            Err(_) => {
                control.devices.store(true, Ordering::SeqCst);
                let _ = device.join();
                None
            }
        }
    }

    // ---- stop -----------------------------------------------------------------------

    /// The record stop under the lock, after the writer handed the recording back: both
    /// files finished, `meta.json` ready-to-transcribe, `call_stopped`, the indicator off,
    /// the folder released from the store's active set. `rec` is `None` when the writer
    /// thread is gone: that is logged, and the meta of the create is finished with no
    /// audio. A
    /// `storage_error` stop leaves `meta.json` as it is (`recording`): the next start
    /// recovers the call from what its tracks hold (FR-016). A recording the writer hands
    /// back as failed makes any stop a `storage_error` stop (I12): that is the reason
    /// logged and returned, the one the caller notices.
    fn finish(&self, call: Call, rec: Option<Box<Recording>>, reason: StopReason) -> StopReason {
        let Call { dir, meta, .. } = call;
        let mut reason = reason;
        let (mut meta, mic_written, system_written) = match rec {
            Some(rec) => {
                let Recording {
                    mic,
                    system,
                    meta,
                    failed,
                    ..
                } = *rec;
                if failed {
                    reason = StopReason::StorageError;
                }
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
        let keep_meta = reason == StopReason::StorageError;
        let now = self.deps.clock.now();
        let duration_ms = mic_written.min(system_written) / SAMPLES_PER_MS;
        meta.state = CallState::ReadyToTranscribe;
        meta.ended_at = Some(started_at(&LocalTime::of(
            now,
            self.deps.offset.seconds_east(now),
        )));
        meta.duration_ms = Some(duration_ms);
        meta.stop_reason = Some(reason);
        if !keep_meta {
            if let Err(e) = write_meta(&dir, &meta) {
                self.deps.log.write(LogEvent::CallStorageFailed {
                    kind: StorageKind::Meta,
                    os: e.raw_os_error(),
                });
            }
        }
        self.deps.store.release(&dir);
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
        reason
    }
}

impl Drop for CallSession {
    /// A session dropped while active lets its writer thread end; the files stay as
    /// written (a later start finds the call `recording` and recovers it).
    fn drop(&mut self) {
        if let Some(captures) = self.lock().captures.as_ref() {
            captures.control.devices.store(true, Ordering::SeqCst);
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

/// Opens `track` on `device` behind a fresh [`OpenSink`] over `channel`.
fn open_track(
    source: &(dyn TrackSource + Send + Sync),
    track: Track,
    device: TrackDevice,
    channel: &Arc<dyn TrackSink>,
) -> Result<OpenTrack, OpenError> {
    let sink = OpenSink::new(channel.clone());
    let (handle, endpoint) = source.open(track, device, sink.clone())?;
    Ok(OpenTrack {
        handle,
        sink,
        endpoint,
    })
}

/// The other track of the call.
fn other(track: Track) -> Track {
    match track {
        Track::Mic => Track::System,
        Track::System => Track::Mic,
    }
}

/// One value per track.
struct Pair<T> {
    mic: T,
    system: T,
}

impl<T> Pair<T> {
    fn get(&self, track: Track) -> &T {
        match track {
            Track::Mic => &self.mic,
            Track::System => &self.system,
        }
    }

    fn get_mut(&mut self, track: Track) -> &mut T {
        match track {
            Track::Mic => &mut self.mic,
            Track::System => &mut self.system,
        }
    }
}

// ---- the device thread ------------------------------------------------------------------

/// What the device thread tells the writer (D1, D2).
enum Outcome {
    /// `track` runs again on endpoint `to`; `cause` is the trigger's, or `Reopened` after
    /// the track was declared silent.
    Reopened {
        track: Track,
        cause: DeviceCause,
        to: EndpointId,
    },
    /// `track` was not reopened within 2 s: it is silent until a retry opens it.
    Silent { track: Track, cause: DeviceCause },
}

/// A reopen that has not succeeded yet.
struct Reopen {
    /// The trigger, on the session's monotonic clock.
    since: Instant,
    cause: DeviceCause,
    /// The next 2 s mark (counted from `since`) that gets an attempt.
    next_mark: u32,
    silent: bool,
    /// The error of the latest attempt.
    last: Option<OpenError>,
}

/// One track on the device thread.
struct DeviceLane {
    /// `None` while a reopen has not succeeded.
    handle: Option<Box<dyn CaptureHandle>>,
    sink: Arc<OpenSink>,
    endpoint: EndpointId,
    reopen: Option<Reopen>,
}

impl DeviceLane {
    fn new(opened: OpenTrack) -> DeviceLane {
        DeviceLane {
            handle: Some(opened.handle),
            sink: opened.sink,
            endpoint: opened.endpoint,
            reopen: None,
        }
    }

    /// Cuts off and stops the current stream. `true` when it was still live.
    fn retire(&mut self) -> bool {
        let live = self.sink.retire();
        if let Some(handle) = self.handle.take() {
            stop_handle(handle);
        }
        live
    }
}

/// The one place that stops or opens a track handle while the captures are open (D1):
/// one state machine per track, fed by the stream's own loss ([`OpenSink`]) and by the
/// shell's [`DeviceEvents`]. It never takes the session lock.
struct DeviceThread {
    source: Arc<dyn TrackSource + Send + Sync>,
    mono: Arc<dyn MonotonicClock>,
    /// The channel both tracks feed: a retired stream's `Lost` marker goes here.
    channel: Arc<dyn TrackSink>,
    control: Arc<Control>,
    outcomes: Sender<Outcome>,
    mic: DeviceLane,
    system: DeviceLane,
    /// The saved mic while the mic track is on it (a default-input change leaves it be).
    mic_selected: Option<DeviceId>,
}

impl DeviceThread {
    fn run(mut self, factory: &dyn DeviceEventsFactory, ready: Sender<()>) {
        let events = factory.open();
        // The session waits for this before it returns from the start.
        let _ = ready.send(());
        while !self.control.devices.load(Ordering::SeqCst) {
            for track in [Track::Mic, Track::System] {
                let lane = self.lane(track);
                if lane.reopen.is_none() && lane.sink.lost.load(Ordering::SeqCst) {
                    let cause = match track {
                        Track::Mic => DeviceCause::MicLost,
                        Track::System => DeviceCause::DefaultOutputChanged,
                    };
                    self.trigger(track, cause, &*events);
                }
            }
            while let Some(event) = events.next() {
                self.event(event, &*events);
            }
            self.deadlines(&*events);
            thread::sleep(IDLE);
        }
        for track in [Track::Mic, Track::System] {
            self.lane_mut(track).retire();
        }
        // Dropped here, on this thread: the shell unregisters its client.
        drop(events);
    }

    fn lane(&self, track: Track) -> &DeviceLane {
        match track {
            Track::Mic => &self.mic,
            Track::System => &self.system,
        }
    }

    fn lane_mut(&mut self, track: Track) -> &mut DeviceLane {
        match track {
            Track::Mic => &mut self.mic,
            Track::System => &mut self.system,
        }
    }

    /// The track an event concerns, and why; `None` for a device no track is on.
    fn route(&self, event: &DeviceEvent) -> Option<(Track, DeviceCause)> {
        match event {
            DeviceEvent::DefaultOutputChanged => {
                Some((Track::System, DeviceCause::DefaultOutputChanged))
            }
            // A selected mic is explicit: only a mic on "default" follows the default.
            DeviceEvent::DefaultInputChanged => self
                .mic_selected
                .is_none()
                .then_some((Track::Mic, DeviceCause::MicDefaultChanged)),
            DeviceEvent::DeviceRemoved(id) => {
                let on = |lane: &DeviceLane| lane.endpoint.as_str() == Some(id.0.as_str());
                if self.mic_selected.as_ref() == Some(id) || on(&self.mic) {
                    Some((Track::Mic, DeviceCause::MicLost))
                } else if on(&self.system) {
                    Some((Track::System, DeviceCause::DefaultOutputChanged))
                } else {
                    None
                }
            }
        }
    }

    fn event(&mut self, event: DeviceEvent, events: &dyn DeviceEvents) {
        if let Some((track, cause)) = self.route(&event) {
            self.trigger(track, cause, events);
        }
    }

    /// A device signal for `track`: absorbed while a reopen is pending; otherwise the old
    /// stream is cut off and stopped (before any new open), its `Lost` reaches the writer
    /// (unless the stream sent its own), and the first attempt comes at once.
    fn trigger(&mut self, track: Track, cause: DeviceCause, events: &dyn DeviceEvents) {
        let since = self.mono.now();
        let at = self.source.now();
        let lane = self.lane_mut(track);
        if lane.reopen.is_some() {
            return;
        }
        let live = lane.retire();
        lane.reopen = Some(Reopen {
            since,
            cause,
            next_mark: 1,
            silent: false,
            last: None,
        });
        if live {
            self.channel.lost(track, LossCause::Other, at);
        }
        self.attempt(track, events);
    }

    /// One open of `track` on its default. On success the writer is told, and the events
    /// that came meanwhile for the same track are absorbed (the new open already follows
    /// the change they report).
    fn attempt(&mut self, track: Track, events: &dyn DeviceEvents) {
        let device = match track {
            Track::Mic => TrackDevice::DefaultInput,
            Track::System => TrackDevice::DefaultOutputLoopback,
        };
        match open_track(&*self.source, track, device, &self.channel) {
            Ok(opened) => {
                let lane = self.lane_mut(track);
                let reopen = lane.reopen.take();
                lane.handle = Some(opened.handle);
                lane.sink = opened.sink;
                lane.endpoint = opened.endpoint.clone();
                if track == Track::Mic {
                    self.mic_selected = None;
                }
                let cause = match reopen {
                    Some(r) if !r.silent => r.cause,
                    _ => DeviceCause::Reopened,
                };
                // A writer that is gone has nobody to tell.
                let _ = self.outcomes.send(Outcome::Reopened {
                    track,
                    cause,
                    to: opened.endpoint,
                });
                let mut rest = Vec::new();
                while let Some(event) = events.next() {
                    if self.route(&event).map(|(t, _)| t) != Some(track) {
                        rest.push(event);
                    }
                }
                for event in rest {
                    self.event(event, events);
                }
            }
            Err(e) => {
                if let Some(reopen) = self.lane_mut(track).reopen.as_mut() {
                    reopen.last = Some(e);
                }
            }
        }
    }

    /// The 2 s rule on the monotonic clock: silent at the first round at or after the
    /// trigger + 2 s, then one attempt per 2 s mark.
    fn deadlines(&mut self, events: &dyn DeviceEvents) {
        let now = self.mono.now();
        for track in [Track::Mic, Track::System] {
            let Some(reopen) = self.lane_mut(track).reopen.as_mut() else {
                continue;
            };
            let elapsed = now.saturating_duration_since(reopen.since);
            if elapsed < REOPEN_WITHIN {
                continue;
            }
            let declare = (!reopen.silent).then(|| {
                reopen.silent = true;
                if reopen.last == Some(OpenError::NoDevice) {
                    DeviceCause::NoDevice
                } else {
                    DeviceCause::TrackLost
                }
            });
            let mark =
                u32::try_from(elapsed.as_millis() / REOPEN_WITHIN.as_millis()).unwrap_or(u32::MAX);
            let due = mark >= reopen.next_mark;
            if due {
                reopen.next_mark = mark.saturating_add(1);
            }
            if let Some(cause) = declare {
                let _ = self.outcomes.send(Outcome::Silent { track, cause });
            }
            if due {
                self.attempt(track, events);
            }
        }
    }
}

/// The writer's side of a new call: both encoders, a timeline per track from `start`.
fn recording(
    files: ActiveCallFiles<TrackFile>,
    start: CaptureStamp,
    mono_start: Instant,
) -> Box<Recording> {
    let dir = files.dir().to_path_buf();
    let meta = files.meta().clone();
    let ActiveCallFiles { mic, system, .. } = files;
    let lane = |track: Track, enc: OpenOgg| RecLane {
        timeline: Timeline::new(track, start),
        enc: Some(enc),
        level: 0.0,
        gap: None,
        activity: Activity::new(),
    };
    Box::new(Recording {
        mic: lane(Track::Mic, mic),
        system: lane(Track::System, system),
        meta,
        dir,
        mono_start,
        last_tick: None,
        failed: false,
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
    enc: Option<OpenOgg>,
    /// The latest 100 ms level.
    level: f32,
    /// The device change whose gap is still being measured (D3).
    gap: Option<DeviceGap>,
    /// The FR-013 speech accounting.
    activity: Activity,
}

type OpenOgg = OpusOgg<TrackFile>;

/// A device change of one track while record is on: where the track went silent, where
/// its new stream's audio landed, and what the device thread reported.
struct DeviceGap {
    /// The track position (ms) right after the `Lost`.
    at_ms: u64,
    /// The track position (ms) where the new stream's first block landed.
    landed_ms: Option<u64>,
    /// The reopen: its cause, the new endpoint and when the writer heard of it.
    reopened: Option<(DeviceCause, EndpointId, Instant)>,
}

/// FR-013 on one track (D4): a speech verdict per second of placed audio.
struct Activity {
    /// The samples of the second being filled.
    second: Vec<i16>,
    /// Its index on the track's axis.
    index: u64,
    /// Some of it was placed while the track was lost.
    tainted: bool,
    /// The first second of the current stretch without speech.
    stretch: u64,
    /// The latest second with speech.
    last_speech: Option<u64>,
    /// The rule fired in this stretch (the tick's `*_silent`).
    fired: bool,
}

impl Activity {
    fn new() -> Activity {
        Activity {
            second: Vec::with_capacity(SECOND_SAMPLES),
            index: 0,
            tainted: false,
            stretch: 0,
            last_speech: None,
            fired: false,
        }
    }

    /// Takes the placed samples `pcm` and gives each completed second its verdict. `lost`:
    /// the samples were placed while the track was lost (such a second ends the stretch, so
    /// a lost track never fires).
    fn listen(&mut self, pcm: &[f32], lost: bool) {
        self.tainted |= lost;
        let mut rest = pcm;
        while !rest.is_empty() {
            let take = (SECOND_SAMPLES - self.second.len()).min(rest.len());
            let (head, tail) = rest.split_at(take);
            self.second.extend(head.iter().map(|x| to_i16(*x)));
            rest = tail;
            if self.second.len() == SECOND_SAMPLES {
                self.second_done();
                self.tainted = lost;
            }
        }
    }

    fn second_done(&mut self) {
        let k = self.index;
        self.index += 1;
        let samples = std::mem::replace(&mut self.second, Vec::with_capacity(SECOND_SAMPLES));
        if self.tainted {
            self.stretch = k + 1;
            self.fired = false;
            return;
        }
        let speech = samples.iter().any(|s| *s != 0)
            && EnergyDetector::new().detect(&AudioBuffer::from_16k_mono(samples));
        if speech {
            self.last_speech = Some(k);
            self.stretch = k + 1;
            self.fired = false;
        }
    }

    /// `true` once per stretch: more than 60 completed seconds without speech and a speech
    /// second of the other track (`other`) in the stretch. Checked after either track's
    /// step, so the order the writer drains the tracks in does not matter.
    fn cant_hear(&mut self, other: Option<u64>) -> bool {
        let silent_for = self.index.saturating_sub(self.stretch);
        let other_spoke = other.is_some_and(|s| s >= self.stretch);
        if self.fired || silent_for <= CANT_HEAR_AFTER_S || !other_spoke {
            return false;
        }
        self.fired = true;
        true
    }
}

fn to_i16(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
}

/// The recording call on the writer.
struct Recording {
    mic: RecLane,
    system: RecLane,
    meta: CallMeta,
    dir: PathBuf,
    mono_start: Instant,
    last_tick: Option<Instant>,
    /// A track write failed: both encoders are gone and no mid-call `write_meta` follows
    /// (device changes and drops stay in memory, I-d).
    failed: bool,
}

impl Recording {
    fn lane(&mut self, track: Track) -> &mut RecLane {
        match track {
            Track::Mic => &mut self.mic,
            Track::System => &mut self.system,
        }
    }

    fn lane_ref(&self, track: Track) -> &RecLane {
        match track {
            Track::Mic => &self.mic,
            Track::System => &self.system,
        }
    }
}

/// What the writer saw of one track's stream, record on or off.
struct Seen {
    /// The end stamp of its latest block.
    last_end: Option<CaptureStamp>,
    /// When its latest event was taken (monotonic clock).
    last_event: Instant,
}

impl Seen {
    fn new(now: Instant) -> Seen {
        Seen {
            last_end: None,
            last_event: now,
        }
    }
}

struct Writer {
    shared: Arc<Shared>,
    log: Arc<Log>,
    mono: Arc<dyn MonotonicClock>,
    /// Its clock places a track when neither delivers (D3).
    source: Arc<dyn TrackSource + Send + Sync>,
    consumer: Arc<dyn SubtitleConsumer>,
    control: Arc<Control>,
    inbox: Receiver<Envelope>,
    outcomes: Receiver<Outcome>,
    mic_rx: TrackReceiver,
    system_rx: TrackReceiver,
    seen: Pair<Seen>,
    /// Warnings raised on this thread, not yet in the call's list (D4).
    pending: Vec<CallWarning>,
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
            // Read before the drain: every event queued before the stop is drained. The
            // device thread is joined before the stop, so its outcomes are all sent.
            let stopping = self.control.stop.load(Ordering::SeqCst);
            while let Ok(envelope) = self.inbox.try_recv() {
                self.command(envelope);
            }
            let busy = self.round();
            self.advance();
            self.flush_warnings();
            if !busy {
                if stopping {
                    break;
                }
                thread::sleep(IDLE);
            }
        }
        self.end_recording();
        WriterEnd { rec: self.rec }
    }

    /// One pass over both channels, then the device outcomes sent before it: a track's
    /// `Lost` is queued before the device thread reports its reopen, so the writer has it
    /// when the outcome applies. `true` when an event was handled.
    fn round(&mut self) -> bool {
        let outcomes: Vec<Outcome> = self.outcomes.try_iter().collect();
        let busy = self.drain();
        for outcome in outcomes {
            self.outcome(outcome);
        }
        busy
    }

    /// Record ends: both tracks are brought to the other's latest block (D3), a measured
    /// reopen is written, and both timelines hand over what their resamplers still hold, so
    /// the files keep the audio up to the last block.
    fn end_recording(&mut self) {
        for track in [Track::Mic, Track::System] {
            if let Some(at) = self.seen.get(other(track)).last_end {
                self.advance_track(track, at);
            }
        }
        for track in [Track::Mic, Track::System] {
            let Some(rec) = self.rec.as_mut() else {
                return;
            };
            let lane = rec.lane(track);
            if let Some(gap) = lane.gap.as_mut() {
                gap.landed_ms
                    .get_or_insert(lane.timeline.written() / SAMPLES_PER_MS);
            }
            self.settle(track);
            let Some(rec) = self.rec.as_mut() else {
                return;
            };
            let lane = rec.lane(track);
            let lost = lane.timeline.is_lost();
            let at_ms = lane.timeline.written() / SAMPLES_PER_MS;
            let step = lane.timeline.flush();
            self.apply(track, step, at_ms, lost);
        }
    }

    /// Everything queued so far goes under the old state, then the command applies.
    fn command(&mut self, envelope: Envelope) {
        let outcomes: Vec<Outcome> = self.outcomes.try_iter().collect();
        while self.drain() {}
        for outcome in outcomes {
            self.outcome(outcome);
        }
        let answer = match envelope.command {
            Command::Attach(rec) => {
                self.control.failed.store(false, Ordering::SeqCst);
                self.rec = Some(rec);
                self.pending.clear();
                None
            }
            Command::Detach => {
                self.end_recording();
                self.pending.clear();
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
        let seen = self.seen.get_mut(track);
        seen.last_event = self.mono.now();
        if let TrackEvent::Block {
            interleaved,
            rate,
            channels,
            at,
        } = &event
        {
            if *rate > 0 && *channels > 0 {
                let frames = (interleaved.len() / usize::from(*channels)) as u64;
                let ns = frames.saturating_mul(1_000_000_000) / u64::from(*rate);
                seen.last_end = Some(CaptureStamp(at.0.saturating_add(ns)));
            }
        }
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
        let written = lane.timeline.written();
        let at_ms = written / SAMPLES_PER_MS;
        let was_lost = lane.timeline.is_lost();
        let Some(step) = step_of(&mut lane.timeline, &event) else {
            return;
        };
        match &event {
            // Every `Lost` is a device change (the device thread retires a stream before it
            // reopens it): its gap starts where the track now stands.
            TrackEvent::Lost { .. } => {
                let at_ms = lane.timeline.written() / SAMPLES_PER_MS;
                lane.gap.get_or_insert(DeviceGap {
                    at_ms,
                    landed_ms: None,
                    reopened: None,
                });
            }
            TrackEvent::Block { at, .. } => {
                if let Some(gap) = lane.gap.as_mut() {
                    let landed = lane.timeline.position_ms(*at).max(at_ms);
                    gap.landed_ms.get_or_insert(landed);
                }
            }
            TrackEvent::Dropped { .. } => {}
        }
        let lost = was_lost || lane.timeline.is_lost();
        self.apply(track, step, at_ms, lost);
        self.settle(track);
    }

    /// `lost`: the step was placed while the track was lost (FR-013 ignores it).
    fn apply(&mut self, track: Track, step: Step, at_ms: u64, lost: bool) {
        let Some(rec) = self.rec.as_mut() else {
            return;
        };
        let lane = rec.lane(track);
        let pushed = match lane.enc.as_mut() {
            Some(enc) => enc.push(&step.pcm),
            None => Ok(()),
        };
        if let Err(e) = pushed {
            // FR-016: the first failed write ends the call's audio on both tracks at once
            // (dropped unfinished, nothing more written); `tick` stops the call.
            rec.mic.enc = None;
            rec.system.enc = None;
            rec.failed = true;
            self.control.failed.store(true, Ordering::SeqCst);
            self.log.write(LogEvent::CallStorageFailed {
                kind: storage_kind(e.op),
                os: None,
            });
        }
        let lane = rec.lane(track);
        lane.activity.listen(&step.pcm, lost);
        for silent in [track, other(track)] {
            let speech = rec.lane_ref(other(silent)).activity.last_speech;
            if rec.lane(silent).activity.cant_hear(speech) {
                self.log.write(LogEvent::CallTrackSilent {
                    track: silent,
                    rule: SilentRule::CantHear,
                });
                self.pending.push(match silent {
                    Track::System => CallWarning::CantHearOtherParty,
                    Track::Mic => CallWarning::CantHearYou,
                });
            }
        }
        for report in &step.reports {
            match *report {
                Report::Dropped { blocks, ms } => self.dropped(track, blocks, ms, at_ms),
                // A device change is measured from the track's `Lost` and the device
                // thread's outcome (`DeviceGap`), not from this report; a gap without a
                // device change is neither logged nor stored (#244).
                Report::Gap { .. } => {}
            }
        }
        if let Some(level) = step.levels.last() {
            if let Some(rec) = self.rec.as_mut() {
                rec.lane(track).level = *level;
            }
        }
        // Placed audio ticks (at most every `TICK_EVERY`), with the latest level: a tick
        // follows the writer's round whether or not the step closed a level window.
        if !step.pcm.is_empty() {
            self.maybe_tick();
        }
    }

    /// What the device thread reported. Without a recording call nothing is logged,
    /// stored or shown (D6).
    fn outcome(&mut self, outcome: Outcome) {
        let Some(rec) = self.rec.as_mut() else {
            return;
        };
        match outcome {
            Outcome::Reopened { track, cause, to } => {
                if track == Track::Mic {
                    // The mic is on the default input now.
                    self.pending.push(CallWarning::MicChanged);
                }
                let lane = rec.lane(track);
                let now = self.mono.now();
                match lane.gap.as_mut() {
                    Some(gap) => gap.reopened = Some((cause, to, now)),
                    None => {
                        // Record started after the loss: no silence of this call.
                        let at_ms = lane.timeline.written() / SAMPLES_PER_MS;
                        self.device_change(track, cause, Some(to), at_ms, 0);
                    }
                }
                self.settle(track);
            }
            Outcome::Silent { track, cause } => {
                let lane = rec.lane(track);
                let now_ms = lane.timeline.written() / SAMPLES_PER_MS;
                let at_ms = lane.gap.as_ref().map_or(now_ms, |g| g.at_ms);
                self.log.write(LogEvent::CallTrackSilent {
                    track,
                    rule: SilentRule::Lost,
                });
                self.device_change(track, cause, None, at_ms, now_ms.saturating_sub(at_ms));
                self.pending.push(CallWarning::TrackLost { track });
            }
        }
    }

    /// A reopen whose audio has landed: one `call_device` line and one entry (D3).
    fn settle(&mut self, track: Track) {
        let Some(rec) = self.rec.as_mut() else {
            return;
        };
        let lane = rec.lane(track);
        let Some(DeviceGap {
            at_ms,
            landed_ms: Some(landed),
            reopened: Some(_),
        }) = lane.gap.as_ref()
        else {
            return;
        };
        let (at_ms, gap_ms) = (*at_ms, landed.saturating_sub(*at_ms));
        let Some(DeviceGap {
            reopened: Some((cause, to, _)),
            ..
        }) = lane.gap.take()
        else {
            return;
        };
        self.device_change(track, cause, Some(to), at_ms, gap_ms);
    }

    /// One `call_device` line and one `device_changes` entry, written at once.
    fn device_change(
        &mut self,
        track: Track,
        cause: DeviceCause,
        to: Option<EndpointId>,
        at_ms: u64,
        gap_ms: u64,
    ) {
        let Some(rec) = self.rec.as_mut() else {
            return;
        };
        self.log.write(LogEvent::CallDeviceChanged {
            track,
            cause,
            gap_ms,
            to: to.clone(),
        });
        rec.meta.device_changes.push(DeviceChange {
            at_ms,
            track,
            cause,
            to,
            gap_ms,
        });
        if rec.failed {
            // After a failed track write the call's folder is not written again (I-d).
            return;
        }
        if let Err(e) = write_meta(&rec.dir, &rec.meta) {
            // The call goes on; the stop writes the full list again.
            self.log.write(LogEvent::CallStorageFailed {
                kind: StorageKind::Meta,
                os: e.raw_os_error(),
            });
        }
    }

    /// D3: a lost track follows the other track's latest block at once, a live one that
    /// delivered nothing for [`STALL`] follows it too (200 ms behind, see
    /// [`Timeline::advance_to`]); when neither delivers, both follow the source's clock. A
    /// reopened stream that sent nothing for [`SETTLE`] has its gap measured where the
    /// track stands.
    fn advance(&mut self) {
        if self.rec.is_none() {
            return;
        }
        let now = self.mono.now();
        let stalled = |seen: &Seen| now.saturating_duration_since(seen.last_event) >= STALL;
        let both = stalled(&self.seen.mic) && stalled(&self.seen.system);
        let clock = both.then(|| self.source.now());
        for track in [Track::Mic, Track::System] {
            let lost = self
                .rec
                .as_ref()
                .is_some_and(|r| r.lane_ref(track).timeline.is_lost());
            if !lost && !stalled(self.seen.get(track)) {
                continue;
            }
            let target = self.seen.get(other(track)).last_end.max(clock);
            if let Some(at) = target {
                self.advance_track(track, at);
            }
            let Some(rec) = self.rec.as_mut() else {
                return;
            };
            let lane = rec.lane(track);
            let written_ms = lane.timeline.written() / SAMPLES_PER_MS;
            if let Some(gap) = lane.gap.as_mut() {
                let quiet = gap
                    .reopened
                    .as_ref()
                    .is_some_and(|(_, _, heard)| now.saturating_duration_since(*heard) >= SETTLE);
                if quiet {
                    gap.landed_ms.get_or_insert(written_ms);
                }
            }
            self.settle(track);
        }
    }

    fn advance_track(&mut self, track: Track, at: CaptureStamp) {
        let Some(rec) = self.rec.as_mut() else {
            return;
        };
        let lane = rec.lane(track);
        let lost = lane.timeline.is_lost();
        let at_ms = lane.timeline.written() / SAMPLES_PER_MS;
        let step = lane.timeline.advance_to(at);
        if !step.pcm.is_empty() || !step.reports.is_empty() {
            self.apply(track, step, at_ms, lost);
        }
    }

    /// Moves the warnings raised here into the call's list and republishes the state,
    /// under the session lock (D4). A session that holds the lock (or waits for this
    /// thread) keeps them pending for the next round.
    fn flush_warnings(&mut self) {
        if self.pending.is_empty()
            || self.control.stop.load(Ordering::SeqCst)
            || self.control.hold.load(Ordering::SeqCst)
        {
            return;
        }
        let mut inner = match self.shared.inner.try_lock() {
            Ok(inner) => inner,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => return,
        };
        let Some(call) = inner.call.as_mut() else {
            self.pending.clear();
            return;
        };
        let mut added = false;
        for warning in self.pending.drain(..) {
            added |= call.warnings.raise(warning).is_some();
        }
        if added {
            self.shared.indicator.set_state(&call.state());
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
        if rec.failed {
            return;
        }
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
            mic_silent: rec.mic.activity.fired,
            system_silent: rec.system.activity.fired,
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

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: usize = SECOND_SAMPLES;

    fn seconds(n: usize, amp: f32) -> Vec<f32> {
        vec![amp; n * SECOND]
    }

    #[test]
    fn cant_hear_fires_once_after_more_than_60_silent_seconds_whatever_the_track_order() {
        // The silent side placed first: its seconds are all done before the other track
        // speaks, so the rule fires on the other track's step.
        let (mut silent, mut other) = (Activity::new(), Activity::new());
        silent.listen(&seconds(61, 0.0), false);
        assert!(!silent.cant_hear(other.last_speech), "nobody spoke yet");
        other.listen(&seconds(61, 0.3), false);
        assert!(silent.cant_hear(other.last_speech));
        assert!(!silent.cant_hear(other.last_speech), "once per stretch");

        // The other side placed first; 60 seconds is not more than 60.
        let (mut silent, mut other) = (Activity::new(), Activity::new());
        other.listen(&seconds(61, 0.3), false);
        silent.listen(&seconds(60, 0.0), false);
        assert!(!silent.cant_hear(other.last_speech));
        silent.listen(&seconds(1, 0.0), false);
        assert!(silent.cant_hear(other.last_speech));
        assert!(silent.fired);

        // A speech second re-arms it and clears the flag.
        silent.listen(&seconds(1, 0.3), false);
        assert!(!silent.fired);
        assert!(
            !silent.cant_hear(other.last_speech),
            "the other spoke before"
        );
    }

    #[test]
    fn a_lost_second_ends_the_stretch() {
        let (mut silent, mut other) = (Activity::new(), Activity::new());
        other.listen(&seconds(70, 0.3), false);
        silent.listen(&seconds(35, 0.0), false);
        silent.listen(&seconds(1, 0.0), true);
        silent.listen(&seconds(34, 0.0), false);
        assert!(!silent.cant_hear(other.last_speech), "34 s since the loss");
        let mut lost = Activity::new();
        lost.listen(&seconds(70, 0.0), true);
        assert!(
            !lost.cant_hear(other.last_speech),
            "a lost track never fires"
        );
    }

    #[test]
    fn a_partial_second_gives_no_verdict() {
        let mut a = Activity::new();
        a.listen(&vec![0.3; SECOND - 1], false);
        assert_eq!((a.index, a.last_speech), (0, None));
        a.listen(&[0.3], false);
        assert_eq!((a.index, a.last_speech), (1, Some(0)));
    }
}
