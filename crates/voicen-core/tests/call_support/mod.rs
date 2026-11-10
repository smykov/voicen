//! Shared fakes of the T-126 call session tests (`tests/call_session.rs`, and the
//! independence test in `tests/dictation_session.rs`): a scripted `TrackSource`, a
//! scripted `CallRoots`, the three session ports, and a rig that wires them to a real
//! `CallStore` over temp dirs, a real `Log` and a fake clock.
//!
//! The API these tests pin (the developer builds it; the docs land in
//! docs/decisions/call-session.md and contracts/core-call.md "Built (T-126)"):
//!
//! ```text
//! voicen_core::call::session::{CallSession, CallDeps, CallIndicator, CallShell,
//!                              CallSettingsPort, Toggled}
//! CallSession::new(CallDeps) -> CallSession                  // off; Send + Sync
//! CallSession::toggle_record(&self, on: bool, source: CallSource) -> Toggled
//! CallSession::toggle_subtitles(&self, on: bool, source: CallSource) -> Toggled   // FR-019; 1a's UI never calls it
//! CallSession::is_subtitles(&self) -> bool
//! CallSession::stop(&self, reason: StopReason) -> bool       // true when a call was recording
//! CallSession::is_recording(&self) -> bool
//! CallSession::try_is_recording(&self) -> Option<bool>       // None while the session lock is held
//! CallSession::tick(&self)                                   // reminder expiry (mono clock)
//! CallSession::next_deadline(&self) -> Option<Instant>       // reminder end; None when nothing is due
//! CallSession::pause_writer(&self) -> WriterPause            // test-fakes: holds the writer thread
//!                                                            // off the channels until dropped; returns once
//!                                                            // the writer is parked, so blocks fed after it stay queued
//! Toggled::{Started, AlreadyOn, Refused { part: RefusedPart, reason: RefusedReason },
//!           Stopped, AlreadyOff}                             // Debug, PartialEq
//! toggle_subtitles uses the same Toggled values and the one session lock as toggle_record. On
//! turns the track captures on (system, mic; a refusal is Refused { part: System | Mic, .. } and
//! leaves subtitles off), and NEVER reaches CallStore, the root probe, the calls tab or the
//! recording indicator; off closes the captures unless record is on. Record on while subtitles
//! is on reuses the open captures (no second open); record off while subtitles is on finishes
//! the call folder and keeps the captures feeding the consumer. The captures close when both
//! toggles are off. active = record || subtitles.
//! trait SubtitleConsumer: Send + Sync   // fed by the writer thread, only while subtitles is on
//!     { audio(&self, track: Track, samples: &[f32]) }   // 16 kHz mono blocks in capture order
//! CallDeps gains `subtitles: Arc<dyn SubtitleConsumer>`.
//!
//! T-149 (device changes and silent-side warnings) pins, in addition:
//!
//! ```text
//! voicen_core::call::{DeviceEvent, DeviceEvents, DeviceEventsFactory, CommunicationsCheck}
//!                                                   // also at voicen_core::call::capture::*
//! enum DeviceEvent { DefaultOutputChanged, DefaultInputChanged, DeviceRemoved(DeviceId) }
//!                                                   // Debug, Clone, PartialEq, Eq
//! trait DeviceEvents: Send { fn next(&self) -> Option<DeviceEvent>; }   // non-blocking pull
//! trait DeviceEventsFactory: Send + Sync { fn open(&self) -> Box<dyn DeviceEvents>; }
//!     // called ON the call's device thread, once per capture lifetime (so the shell registers
//!     // its IMMNotificationClient there); the box is dropped on that thread when the captures
//!     // close (unregister). The thread calls `next` in a loop, sleeping a short IDLE between
//!     // rounds, so `next` is called at least once per round whatever the queue holds.
//! trait CommunicationsCheck: Send + Sync { fn differs(&self) -> Result<bool, i32>; }
//!     // Err carries an HRESULT; core maps E_NOTFOUND (0x80070490) and every other Err to "no
//!     // difference"; called once per record start, before call_started, never on a subtitles
//!     // start
//! CallDeps gains `devices: Arc<dyn DeviceEventsFactory>` and `comms: Arc<dyn CommunicationsCheck>`.
//! CallSession::dismiss_warning(&self, id: u32) -> bool   // removes the warning with that id from
//!                                                        // the call's list and republishes the
//!                                                        // indicator state under the lock; false
//!                                                        // for an unknown id or when no call records
//! ```
//!
//! Behaviour the T-149 tests pin (docs/decisions/call-session.md D1-D6 lands with the code):
//! the device thread makes the first reopen attempt at once when a trigger comes (a
//! `TrackEvent::Lost` of a track, or a `DeviceEvent`), measures 2 s on the session's monotonic
//! clock (`CallDeps::mono`), declares the track silent at the first round with mono >= trigger +
//! 2 s and then makes one attempt every 2 s from the trigger (at most one attempt per 2 s mark,
//! none in between). A trigger that comes while an open is running is absorbed.
//! toggle_record(false, Tray) stops with StopReason::User, (false, Hotkey) with Hotkey
//!
//! trait CallIndicator: Send + Sync   // called INSIDE the session lock, never blocks
//!     { set_state(&self, &IndicatorState); set_tick(&self, &IndicatorTick); set_call_recording(&self, bool) }
//! trait CallShell: Send + Sync       // called AFTER the lock is released
//!     { notice(&self, CallNotice); open_calls_tab(&self) }
//! trait CallSettingsPort: Send + Sync
//!     { snapshot(&self) -> Arc<Settings>;
//!       remember_warned_host(&self, &ProviderHost) -> Result<(), WarnedHostFailed> }
//! struct CallDeps { source: Arc<dyn TrackSource + Send + Sync>, store: Arc<CallStore>,
//!     settings: Arc<dyn CallSettingsPort>, indicator: Arc<dyn CallIndicator>,
//!     shell: Arc<dyn CallShell>, log: Arc<Log>, clock: Arc<dyn Clock>,
//!     offset: Arc<dyn LocalOffset>, mono: Arc<dyn MonotonicClock>, app_version: String,
//!     open_track: Arc<dyn Fn(&Path) -> io::Result<Box<dyn SyncWrite + Send>> + Send + Sync>,   // T-144: boxed
//!     disk: Arc<dyn DiskSpace>,   // T-144: polled every 10 s while recording; < 1 GB warns once, < 200 MB stops
//!     // T-144: tick() also polls the disk and stops on storage error / low disk / 4 h; next_deadline() is
//!     // Some(<= now + 10 s) while a call records, None otherwise   // the CallStore::create_with opener
//!     channel_capacity: usize }                                             // events per track channel
//! voicen_core::settings::service::WarnedHostFailed        // unit struct, like ForgetFailed (Debug, Clone, Copy, PartialEq, Eq)
//! SettingsService::remember_warned_host(&self, &ProviderHost) -> Result<(), WarnedHostFailed>
//! ```
#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use voicen_core::call::codec::SyncWrite;
use voicen_core::call::indicator::{IndicatorState, IndicatorTick};
use voicen_core::call::session::{
    CallDeps, CallIndicator, CallSession, CallSettingsPort, CallShell, SubtitleConsumer,
};
use voicen_core::call::{
    CallNotice, CallStore, CaptureStamp, CommunicationsCheck, DeviceEvent, DeviceEvents,
    DeviceEventsFactory, EndpointId, LossCause, OpenError, Track, TrackDevice, TrackSink,
    TrackSource,
};
use voicen_core::clock::{FakeClock, MonotonicClock};
use voicen_core::diag::{Log, LogConfig};
use voicen_core::local_models::download::DiskSpace;
use voicen_core::platform::CaptureHandle;
use voicen_core::recording::CaptureError;
use voicen_core::settings::roots::{CallRoots, UnwritableReason};
use voicen_core::settings::service::WarnedHostFailed;
use voicen_core::settings::url::ProviderHost;
use voicen_core::settings::{defaults, Settings};
use voicen_core::test_support::TempDir;

use crate::common::timing;
use crate::diag_support::{self, Line};

pub const MIC_ID: &str = "{0.0.1.00000000}.{11111111-1111-1111-1111-111111111111}";
pub const SYSTEM_ID: &str = "{0.0.0.00000000}.{22222222-2222-2222-2222-222222222222}";
pub const OS: Option<&str> = Some("en-US");
/// A generous bound that only ends a hung wait; the waits poll state, not time.
pub const BUDGET: Duration = Duration::from_secs(20);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The shared order log: the source writes `open system` / `open mic`, the roots
/// write `probe`.
pub type Order = Arc<Mutex<Vec<String>>>;

// ---- monotonic clock -----------------------------------------------------------------

pub struct FakeMono {
    now: Mutex<Instant>,
}

impl FakeMono {
    pub fn new() -> Arc<FakeMono> {
        Arc::new(FakeMono {
            now: Mutex::new(timing::now()),
        })
    }

    pub fn advance(&self, d: Duration) {
        *lock(&self.now) += d;
    }

    pub fn read(&self) -> Instant {
        *lock(&self.now)
    }
}

impl MonotonicClock for FakeMono {
    fn now(&self) -> Instant {
        *lock(&self.now)
    }
}

// ---- the track source ----------------------------------------------------------------

/// Open handles per track: (now, most at once).
type Peaks = Arc<Mutex<HashMap<Track, (usize, usize)>>>;

/// The in-order trace of `open <track>` / `stop <track>` calls.
type Trace = Arc<Mutex<Vec<String>>>;

struct Handle {
    track: Track,
    live: Arc<AtomicUsize>,
    stops: Arc<AtomicUsize>,
    peaks: Peaks,
    trace: Trace,
}

impl CaptureHandle for Handle {
    fn stop(self: Box<Self>) -> Result<(), CaptureError> {
        lock(&self.trace).push(format!("stop {}", self.track.as_str()));
        if let Some(count) = lock(&self.peaks).get_mut(&self.track) {
            count.0 = count.0.saturating_sub(1);
        }
        self.live.fetch_sub(1, Ordering::SeqCst);
        self.stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// The id the `k`-th successful open (k >= 2) of `track` reports: a reopen lands on
/// another endpoint.
pub fn reopened_id(track: Track, k: usize) -> String {
    let t = match track {
        Track::Mic => 1,
        Track::System => 0,
    };
    format!("{{0.0.{t}.00000000}}.{{{k:08x}-3333-3333-3333-333333333333}}")
}

/// HRESULT `E_NOTFOUND` (0x80070490) as the `i32` an `Err` carries.
pub const E_NOTFOUND: i32 = -2_147_023_728;

#[derive(Default)]
struct SourceState {
    /// The open results still to be served, per track; an empty queue opens fine.
    /// Each entry fails exactly one open (F-024).
    script: HashMap<Track, VecDeque<OpenError>>,
    opens: Vec<(Track, TrackDevice)>,
    sinks: HashMap<Track, Arc<dyn TrackSink>>,
    stamp_ms: u64,
    /// Opens that fail every time until `heal` (a guarantee about repeated failures: the
    /// 2 s retries of T-149; every other fake fails exactly one call, F-024).
    broken: HashMap<Track, OpenError>,
    /// Successful opens so far, per track.
    ok_opens: HashMap<Track, usize>,
}

pub struct FakeSource {
    order: Order,
    state: Mutex<SourceState>,
    live: Arc<AtomicUsize>,
    stops: Arc<AtomicUsize>,
    peaks: Peaks,
    trace: Trace,
    /// While `true`, every `open` blocks before it records anything (a slow device).
    hold: Arc<(Mutex<bool>, Condvar)>,
    blocked: AtomicUsize,
}

impl FakeSource {
    pub fn new(order: Order) -> Arc<FakeSource> {
        Arc::new(FakeSource {
            order,
            state: Mutex::new(SourceState::default()),
            live: Arc::new(AtomicUsize::new(0)),
            stops: Arc::new(AtomicUsize::new(0)),
            peaks: Arc::new(Mutex::new(HashMap::new())),
            trace: Arc::new(Mutex::new(Vec::new())),
            hold: Arc::new((Mutex::new(false), Condvar::new())),
            blocked: AtomicUsize::new(0),
        })
    }

    /// Every open of `track` fails with `error` until `heal(track)`.
    pub fn fail_all(&self, track: Track, error: OpenError) {
        lock(&self.state).broken.insert(track, error);
    }

    /// The opens of `track` work again.
    pub fn heal(&self, track: Track) {
        lock(&self.state).broken.remove(&track);
    }

    /// The track's stream dies at `at_ms` (the audio callback reports the loss).
    pub fn lose(&self, track: Track, cause: LossCause, at_ms: u64) {
        let sink = lock(&self.state)
            .sinks
            .get(&track)
            .cloned()
            .expect("the track is open");
        sink.lost(track, cause, CaptureStamp::from_ms(at_ms));
    }

    /// From now on every `open` blocks until `release_opens`.
    pub fn hold_opens(&self) {
        *lock(&self.hold.0) = true;
    }

    pub fn release_opens(&self) {
        *lock(&self.hold.0) = false;
        self.hold.1.notify_all();
    }

    /// Opens blocked right now by `hold_opens`.
    pub fn blocked_opens(&self) -> usize {
        self.blocked.load(Ordering::SeqCst)
    }

    /// `open <track>` / `stop <track>`, in the order they happened.
    pub fn trace(&self) -> Vec<String> {
        lock(&self.trace).clone()
    }

    /// The most handles of `track` that were open at one moment.
    pub fn peak_handles(&self, track: Track) -> usize {
        lock(&self.peaks).get(&track).map_or(0, |c| c.1)
    }

    /// Opens of `track` that succeeded so far.
    pub fn ok_opens(&self, track: Track) -> usize {
        lock(&self.state).ok_opens.get(&track).copied().unwrap_or(0)
    }

    /// Opens of `track` attempted so far (failed ones included).
    pub fn attempts(&self, track: Track) -> usize {
        lock(&self.state)
            .opens
            .iter()
            .filter(|(t, _)| *t == track)
            .count()
    }

    /// The next open of `track` fails with `error`; the one after it opens.
    pub fn fail_next(&self, track: Track, error: OpenError) {
        lock(&self.state)
            .script
            .entry(track)
            .or_default()
            .push_back(error);
    }

    /// Every open so far, in order.
    pub fn opens(&self) -> Vec<(Track, TrackDevice)> {
        lock(&self.state).opens.clone()
    }

    /// Captures opened and not yet stopped.
    pub fn open_handles(&self) -> usize {
        self.live.load(Ordering::SeqCst)
    }

    /// `CaptureHandle::stop` calls so far.
    pub fn stops(&self) -> usize {
        self.stops.load(Ordering::SeqCst)
    }

    pub fn set_stamp_ms(&self, ms: u64) {
        lock(&self.state).stamp_ms = ms;
    }

    /// Delivers 16 kHz mono blocks of 20 ms of constant `amp` to the track's sink,
    /// stamped from `from_ms` up to (not including) `to_ms`.
    pub fn feed(&self, track: Track, from_ms: u64, to_ms: u64, amp: f32) {
        let sink = lock(&self.state)
            .sinks
            .get(&track)
            .cloned()
            .expect("the track is open");
        let block = vec![amp; 320];
        let mut at = from_ms;
        while at < to_ms {
            sink.block(track, &block, 16_000, 1, CaptureStamp::from_ms(at));
            at += 20;
        }
    }

    /// Like `feed`, but a 300 Hz sine of peak `amp` (RMS `amp / sqrt 2`), continuous across
    /// calls: a track that goes through Opus keeps it (Voip mode removes the DC of a
    /// constant block, so a constant is stored as silence).
    pub fn feed_tone(&self, track: Track, from_ms: u64, to_ms: u64, amp: f32) {
        let sink = lock(&self.state)
            .sinks
            .get(&track)
            .cloned()
            .expect("the track is open");
        let mut at = from_ms;
        while at < to_ms {
            let first = at * 16;
            let block: Vec<f32> = (0..320u64)
                .map(|i| {
                    let t = (first + i) as f32 / 16_000.0;
                    amp * (2.0 * std::f32::consts::PI * 300.0 * t).sin()
                })
                .collect();
            sink.block(track, &block, 16_000, 1, CaptureStamp::from_ms(at));
            at += 20;
        }
    }

    /// `feed` on both tracks.
    pub fn feed_both(&self, from_ms: u64, to_ms: u64, mic_amp: f32, system_amp: f32) {
        self.feed(Track::System, from_ms, to_ms, system_amp);
        self.feed(Track::Mic, from_ms, to_ms, mic_amp);
    }
}

impl TrackSource for FakeSource {
    fn open(
        &self,
        track: Track,
        device: TrackDevice,
        sink: Arc<dyn TrackSink>,
    ) -> Result<(Box<dyn CaptureHandle>, EndpointId), OpenError> {
        {
            let mut held = lock(&self.hold.0);
            if *held {
                self.blocked.fetch_add(1, Ordering::SeqCst);
                while *held {
                    held = self
                        .hold
                        .1
                        .wait(held)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                self.blocked.fetch_sub(1, Ordering::SeqCst);
            }
        }
        lock(&self.order).push(format!("open {}", track.as_str()));
        let mut state = lock(&self.state);
        state.opens.push((track, device));
        if let Some(error) = state.script.get_mut(&track).and_then(VecDeque::pop_front) {
            return Err(error);
        }
        if let Some(error) = state.broken.get(&track) {
            return Err(*error);
        }
        state.sinks.insert(track, sink);
        let nth = {
            let n = state.ok_opens.entry(track).or_default();
            *n += 1;
            *n
        };
        self.live.fetch_add(1, Ordering::SeqCst);
        lock(&self.trace).push(format!("open {}", track.as_str()));
        {
            let mut peaks = lock(&self.peaks);
            let count = peaks.entry(track).or_default();
            count.0 += 1;
            count.1 = count.1.max(count.0);
        }
        let id = match (nth, track) {
            (1, Track::Mic) => MIC_ID.to_string(),
            (1, Track::System) => SYSTEM_ID.to_string(),
            (k, t) => reopened_id(t, k),
        };
        Ok((
            Box::new(Handle {
                track,
                live: self.live.clone(),
                stops: self.stops.clone(),
                peaks: self.peaks.clone(),
                trace: self.trace.clone(),
            }),
            EndpointId::new(&id),
        ))
    }

    fn now(&self) -> CaptureStamp {
        CaptureStamp::from_ms(lock(&self.state).stamp_ms)
    }
}

// ---- device events and the communications check (T-149) -----------------------------------

/// The factory the call's device thread opens its event stream from, and the stream's queue.
pub struct FakeDevices {
    queue: Arc<Mutex<VecDeque<DeviceEvent>>>,
    polls: Arc<AtomicUsize>,
    opened: AtomicUsize,
    alive: Arc<AtomicUsize>,
}

struct FakeEvents {
    queue: Arc<Mutex<VecDeque<DeviceEvent>>>,
    polls: Arc<AtomicUsize>,
    alive: Arc<AtomicUsize>,
}

impl DeviceEvents for FakeEvents {
    fn next(&self) -> Option<DeviceEvent> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        lock(&self.queue).pop_front()
    }
}

impl Drop for FakeEvents {
    fn drop(&mut self) {
        self.alive.fetch_sub(1, Ordering::SeqCst);
    }
}

impl FakeDevices {
    pub fn new() -> Arc<FakeDevices> {
        Arc::new(FakeDevices {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            polls: Arc::new(AtomicUsize::new(0)),
            opened: AtomicUsize::new(0),
            alive: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// The shell reports `event`.
    pub fn push(&self, event: DeviceEvent) {
        lock(&self.queue).push_back(event);
    }

    /// Events pushed and not yet taken by the device thread.
    pub fn pending(&self) -> usize {
        lock(&self.queue).len()
    }

    /// `DeviceEvents::next` calls so far.
    pub fn polls(&self) -> usize {
        self.polls.load(Ordering::SeqCst)
    }

    /// Streams the factory handed out.
    pub fn opened(&self) -> usize {
        self.opened.load(Ordering::SeqCst)
    }

    /// Streams handed out and not yet dropped.
    pub fn alive(&self) -> usize {
        self.alive.load(Ordering::SeqCst)
    }
}

impl DeviceEventsFactory for FakeDevices {
    fn open(&self) -> Box<dyn DeviceEvents> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        self.alive.fetch_add(1, Ordering::SeqCst);
        Box::new(FakeEvents {
            queue: self.queue.clone(),
            polls: self.polls.clone(),
            alive: self.alive.clone(),
        })
    }
}

/// The communications-device check: answers what the test set, counts its calls.
pub struct FakeComms {
    answer: Mutex<Result<bool, i32>>,
    calls: AtomicUsize,
}

impl FakeComms {
    pub fn new() -> Arc<FakeComms> {
        Arc::new(FakeComms {
            answer: Mutex::new(Ok(false)),
            calls: AtomicUsize::new(0),
        })
    }

    pub fn set(&self, answer: Result<bool, i32>) {
        *lock(&self.answer) = answer;
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl CommunicationsCheck for FakeComms {
    fn differs(&self) -> Result<bool, i32> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *lock(&self.answer)
    }
}

// ---- the roots ------------------------------------------------------------------------

/// A `CallRoots` whose probe refuses exactly once when told to (F-024) and records
/// `probe` into the shared order.
pub struct ScriptedRoots {
    default: PathBuf,
    order: Order,
    refuse_next: Mutex<Option<UnwritableReason>>,
    probes: AtomicUsize,
}

impl ScriptedRoots {
    pub fn new(default: PathBuf, order: Order) -> Arc<ScriptedRoots> {
        Arc::new(ScriptedRoots {
            default,
            order,
            refuse_next: Mutex::new(None),
            probes: AtomicUsize::new(0),
        })
    }

    pub fn refuse_next(&self, reason: UnwritableReason) {
        *lock(&self.refuse_next) = Some(reason);
    }

    pub fn probes(&self) -> usize {
        self.probes.load(Ordering::SeqCst)
    }
}

impl CallRoots for ScriptedRoots {
    fn default_root(&self) -> PathBuf {
        self.default.clone()
    }

    fn probe(&self, _root: &Path) -> Result<(), UnwritableReason> {
        lock(&self.order).push("probe".to_string());
        self.probes.fetch_add(1, Ordering::SeqCst);
        match lock(&self.refuse_next).take() {
            Some(reason) => Err(reason),
            None => Ok(()),
        }
    }
}

// ---- the ports ------------------------------------------------------------------------

/// One call the indicator port received.
#[derive(Debug, Clone, PartialEq)]
pub enum IndicatorCall {
    State(IndicatorState),
    Tick(IndicatorTick),
    Recording(bool),
}

/// The indicator port. Every call also records whether the session lock was held
/// while it ran (`try_is_recording` answered `None`).
pub struct FakeIndicator {
    session: Mutex<Option<Arc<CallSession>>>,
    calls: Mutex<Vec<(IndicatorCall, bool)>>,
}

impl FakeIndicator {
    pub fn new() -> Arc<FakeIndicator> {
        Arc::new(FakeIndicator {
            session: Mutex::new(None),
            calls: Mutex::new(Vec::new()),
        })
    }

    pub fn bind(&self, session: Arc<CallSession>) {
        *lock(&self.session) = Some(session);
    }

    fn record(&self, call: IndicatorCall) {
        let session = lock(&self.session).clone();
        let held = session.is_some_and(|s| s.try_is_recording().is_none());
        lock(&self.calls).push((call, held));
    }

    pub fn calls(&self) -> Vec<IndicatorCall> {
        lock(&self.calls).iter().map(|(c, _)| c.clone()).collect()
    }

    /// `true` for each call that ran with the session lock held.
    pub fn held_flags(&self) -> Vec<bool> {
        lock(&self.calls).iter().map(|(_, h)| *h).collect()
    }

    pub fn states(&self) -> Vec<IndicatorState> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                IndicatorCall::State(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    pub fn ticks(&self) -> Vec<IndicatorTick> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                IndicatorCall::Tick(t) => Some(t),
                _ => None,
            })
            .collect()
    }

    pub fn recording_flags(&self) -> Vec<bool> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                IndicatorCall::Recording(r) => Some(r),
                _ => None,
            })
            .collect()
    }
}

impl CallIndicator for FakeIndicator {
    fn set_state(&self, state: &IndicatorState) {
        self.record(IndicatorCall::State(state.clone()));
    }

    fn set_tick(&self, tick: &IndicatorTick) {
        self.record(IndicatorCall::Tick(tick.clone()));
    }

    fn set_call_recording(&self, recording: bool) {
        self.record(IndicatorCall::Recording(recording));
    }
}

/// One request the shell port received.
#[derive(Debug, Clone, PartialEq)]
pub enum ShellCall {
    Notice(CallNotice),
    OpenCallsTab,
}

pub struct FakeShell {
    session: Mutex<Option<Arc<CallSession>>>,
    calls: Mutex<Vec<(ShellCall, bool)>>,
}

impl FakeShell {
    pub fn new() -> Arc<FakeShell> {
        Arc::new(FakeShell {
            session: Mutex::new(None),
            calls: Mutex::new(Vec::new()),
        })
    }

    pub fn bind(&self, session: Arc<CallSession>) {
        *lock(&self.session) = Some(session);
    }

    fn record(&self, call: ShellCall) {
        let session = lock(&self.session).clone();
        let held = session.is_some_and(|s| s.try_is_recording().is_none());
        lock(&self.calls).push((call, held));
    }

    pub fn calls(&self) -> Vec<ShellCall> {
        lock(&self.calls).iter().map(|(c, _)| c.clone()).collect()
    }

    /// `true` for each request that ran with the session lock held.
    pub fn held_flags(&self) -> Vec<bool> {
        lock(&self.calls).iter().map(|(_, h)| *h).collect()
    }

    pub fn tab_opens(&self) -> usize {
        self.calls()
            .iter()
            .filter(|c| **c == ShellCall::OpenCallsTab)
            .count()
    }
}

impl CallShell for FakeShell {
    fn notice(&self, notice: CallNotice) {
        self.record(ShellCall::Notice(notice));
    }

    fn open_calls_tab(&self) {
        self.record(ShellCall::OpenCallsTab);
    }
}

/// The settings port: a snapshot the test edits, and `remember_warned_host` that
/// records the host text and updates `calls.warned_hosts` like the real writer.
pub struct FakePort {
    settings: Mutex<Arc<Settings>>,
    remembered: Mutex<Vec<String>>,
    fail_next: Mutex<bool>,
}

impl FakePort {
    pub fn new(settings: Settings) -> Arc<FakePort> {
        Arc::new(FakePort {
            settings: Mutex::new(Arc::new(settings)),
            remembered: Mutex::new(Vec::new()),
            fail_next: Mutex::new(false),
        })
    }

    pub fn edit(&self, f: impl FnOnce(&mut Settings)) {
        let mut guard = lock(&self.settings);
        let mut next = (**guard).clone();
        f(&mut next);
        *guard = Arc::new(next);
    }

    /// The next `remember_warned_host` fails; the one after it succeeds.
    pub fn fail_next_remember(&self) {
        *lock(&self.fail_next) = true;
    }

    /// Hosts passed to `remember_warned_host`, in order.
    pub fn remembered(&self) -> Vec<String> {
        lock(&self.remembered).clone()
    }
}

impl CallSettingsPort for FakePort {
    fn snapshot(&self) -> Arc<Settings> {
        lock(&self.settings).clone()
    }

    fn remember_warned_host(&self, host: &ProviderHost) -> Result<(), WarnedHostFailed> {
        if std::mem::take(&mut *lock(&self.fail_next)) {
            return Err(WarnedHostFailed);
        }
        lock(&self.remembered).push(host.as_str().to_string());
        self.edit(|s| {
            if !s.calls.warned_hosts.iter().any(|h| h == host.as_str()) {
                s.calls.warned_hosts.push(host.as_str().to_string());
            }
        });
        Ok(())
    }
}

// ---- the subtitle consumer --------------------------------------------------------------

/// A subtitle consumer that records what the session fed it (FR-019).
pub struct FakeSubtitles {
    samples: Mutex<HashMap<Track, usize>>,
    calls: AtomicUsize,
}

impl FakeSubtitles {
    pub fn new() -> Arc<FakeSubtitles> {
        Arc::new(FakeSubtitles {
            samples: Mutex::new(HashMap::new()),
            calls: AtomicUsize::new(0),
        })
    }

    /// Samples received for `track` so far.
    pub fn samples(&self, track: Track) -> usize {
        lock(&self.samples).get(&track).copied().unwrap_or(0)
    }

    /// `audio` calls so far.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl SubtitleConsumer for FakeSubtitles {
    fn audio(&self, track: Track, samples: &[f32]) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *lock(&self.samples).entry(track).or_default() += samples.len();
    }
}

// ---- the opener -----------------------------------------------------------------------

/// A track-file opener that fails exactly one call: the `n`-th (1-based) after
/// `fail_on` was set, with `kind`; every other call is `File::create` (F-024).
pub struct FlakyOpener {
    calls: AtomicUsize,
    fail_on: Mutex<Option<(usize, io::ErrorKind)>>,
    /// The file name whose next write fails, once (T-144, F-024).
    write_fail: Arc<Mutex<Option<String>>>,
}

impl FlakyOpener {
    pub fn new() -> Arc<FlakyOpener> {
        Arc::new(FlakyOpener {
            calls: AtomicUsize::new(0),
            fail_on: Mutex::new(None),
            write_fail: Arc::new(Mutex::new(None)),
        })
    }

    /// The `n`-th next call fails.
    pub fn fail_nth(&self, n: usize, kind: io::ErrorKind) {
        self.calls.store(0, Ordering::SeqCst);
        *lock(&self.fail_on) = Some((n, kind));
    }

    /// The next write to the track file named `file_name` (e.g. `mic.opus`) fails with
    /// an I/O error, exactly once; every later write succeeds (F-024).
    pub fn fail_next_write(&self, file_name: &str) {
        *lock(&self.write_fail) = Some(file_name.to_string());
    }

    pub fn open(&self, path: &Path) -> io::Result<Box<dyn SyncWrite + Send>> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some((target, kind)) = *lock(&self.fail_on) {
            if n == target {
                return Err(io::Error::from(kind));
            }
        }
        Ok(Box::new(FlakySink {
            file: File::create(path)?,
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            armed: self.write_fail.clone(),
        }))
    }
}

/// A track file that can fail one write.
struct FlakySink {
    file: File,
    name: String,
    armed: Arc<Mutex<Option<String>>>,
}

impl io::Write for FlakySink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut armed = lock(&self.armed);
        if armed.as_deref() == Some(self.name.as_str()) {
            *armed = None;
            return Err(io::Error::other("injected write failure"));
        }
        drop(armed);
        io::Write::write(&mut self.file, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        io::Write::flush(&mut self.file)
    }
}

impl SyncWrite for FlakySink {
    fn sync_data(&mut self) -> io::Result<()> {
        File::sync_data(&self.file)
    }
}

// ---- the disk -------------------------------------------------------------------------

/// A scriptable `DiskSpace`: answers a settable number of bytes (or an error) and
/// records, for each poll, the session's mono time and the directory asked.
pub struct ScriptedDisk {
    mono: Arc<FakeMono>,
    answer: Mutex<Result<u64, io::ErrorKind>>,
    polls: Mutex<Vec<(Instant, PathBuf)>>,
}

impl ScriptedDisk {
    pub fn new(mono: Arc<FakeMono>) -> Arc<ScriptedDisk> {
        Arc::new(ScriptedDisk {
            mono,
            answer: Mutex::new(Ok(100 * GB)),
            polls: Mutex::new(Vec::new()),
        })
    }

    pub fn set_free(&self, bytes: u64) {
        *lock(&self.answer) = Ok(bytes);
    }

    pub fn set_error(&self, kind: io::ErrorKind) {
        *lock(&self.answer) = Err(kind);
    }

    /// The mono time of every poll so far.
    pub fn poll_times(&self) -> Vec<Instant> {
        lock(&self.polls).iter().map(|(t, _)| *t).collect()
    }

    pub fn polled_dirs(&self) -> Vec<PathBuf> {
        lock(&self.polls).iter().map(|(_, d)| d.clone()).collect()
    }
}

pub const GB: u64 = 1_000_000_000;
pub const MB: u64 = 1_000_000;

impl DiskSpace for ScriptedDisk {
    fn available_bytes(&self, dir: &Path) -> io::Result<u64> {
        lock(&self.polls).push((self.mono.read(), dir.to_path_buf()));
        (*lock(&self.answer)).map_err(io::Error::from)
    }
}

// ---- the rig --------------------------------------------------------------------------

pub struct CallRig {
    pub session: Arc<CallSession>,
    pub source: Arc<FakeSource>,
    pub roots: Arc<ScriptedRoots>,
    pub indicator: Arc<FakeIndicator>,
    pub shell: Arc<FakeShell>,
    pub port: Arc<FakePort>,
    pub opener: Arc<FlakyOpener>,
    pub disk: Arc<ScriptedDisk>,
    pub subtitles: Arc<FakeSubtitles>,
    pub devices: Arc<FakeDevices>,
    pub comms: Arc<FakeComms>,
    pub mono: Arc<FakeMono>,
    pub order: Order,
    pub log: Arc<Log>,
    /// The calls root (the effective `calls.storage_root`).
    pub root: PathBuf,
    pub logs: PathBuf,
    _tmp: TempDir,
}

/// The settings every rig starts from: the English UI, the reminder on, a remote
/// API host that is not in `warned_hosts` unless a test says so.
pub fn settings(root: &Path) -> Settings {
    let mut s = defaults(OS);
    s.api.base_url = "https://api.example.com/v1".to_string();
    s.calls.storage_root = Some(root.to_path_buf());
    s.calls.reminder = true;
    s.calls.warned_hosts = Vec::new();
    s.microphone = None;
    s
}

impl CallRig {
    pub fn new() -> CallRig {
        CallRig::with(|_| {})
    }

    /// A rig whose settings are `settings(root)` edited by `edit`, with the default
    /// channel capacity (64 events per track).
    pub fn with(edit: impl FnOnce(&mut Settings)) -> CallRig {
        CallRig::with_capacity(64, edit)
    }

    pub fn with_capacity(capacity: usize, edit: impl FnOnce(&mut Settings)) -> CallRig {
        let tmp = TempDir::new();
        let root = tmp.path().join("calls");
        std::fs::create_dir_all(&root).expect("the calls root");
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).expect("the logs dir");
        let mut s = settings(&root);
        edit(&mut s);
        let order: Order = Arc::new(Mutex::new(Vec::new()));
        let source = FakeSource::new(order.clone());
        let roots = ScriptedRoots::new(tmp.path().join("default-calls"), order.clone());
        let mono = FakeMono::new();
        let store = Arc::new(CallStore::new(roots.clone(), mono.clone()));
        let indicator = FakeIndicator::new();
        let shell = FakeShell::new();
        let port = FakePort::new(s);
        let opener = FlakyOpener::new();
        let disk = ScriptedDisk::new(mono.clone());
        let subtitles = FakeSubtitles::new();
        let devices = FakeDevices::new();
        let comms = FakeComms::new();
        let clock = Arc::new(FakeClock::at(diag_support::at(diag_support::NOON_UTC, 0)));
        let offset = Arc::new(diag_support::Offset(0));
        let (log, _, _) = diag_support::open_log(
            &logs,
            diag_support::at(diag_support::NOON_UTC, 0),
            0,
            LogConfig::default(),
        );
        let open_with = opener.clone();
        let session = Arc::new(CallSession::new(CallDeps {
            source: source.clone(),
            store,
            settings: port.clone(),
            indicator: indicator.clone(),
            shell: shell.clone(),
            log: log.clone(),
            clock,
            offset,
            mono: mono.clone(),
            app_version: "0.0.0-test".to_string(),
            open_track: Arc::new(move |path| open_with.open(path)),
            channel_capacity: capacity,
            subtitles: subtitles.clone(),
            devices: devices.clone(),
            comms: comms.clone(),
            disk: disk.clone(),
        }));
        indicator.bind(session.clone());
        shell.bind(session.clone());
        CallRig {
            session,
            source,
            roots,
            indicator,
            shell,
            port,
            opener,
            disk,
            subtitles,
            devices,
            comms,
            mono,
            order,
            log,
            root,
            logs,
            _tmp: tmp,
        }
    }

    /// Every `call_*` line of the log, parsed against the closed grammar.
    pub fn call_lines(&self) -> Vec<Line> {
        diag_support::all_lines(&self.logs)
            .iter()
            .map(|raw| diag_support::closed(raw))
            .filter(|l| l.head.starts_with("call_"))
            .collect()
    }

    /// The `call_*` lines named `head`.
    pub fn lines_named(&self, head: &str) -> Vec<Line> {
        self.call_lines()
            .into_iter()
            .filter(|l| l.head == head)
            .collect()
    }

    /// The call folders under the root, sorted.
    pub fn folders(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = std::fs::read_dir(&self.root)
            .map(|rd| rd.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        out.sort();
        out
    }

    /// Everything under the root (files and folders), recursively.
    pub fn root_files(&self) -> Vec<PathBuf> {
        diag_support::files_under(&self.root)
    }

    /// The `meta.json` of the only call folder, as a JSON value.
    pub fn only_meta(&self) -> serde_json::Value {
        let folders = self.folders();
        assert_eq!(folders.len(), 1, "exactly one call folder: {folders:?}");
        let bytes = std::fs::read(folders[0].join("meta.json")).expect("meta.json");
        serde_json::from_slice(&bytes).expect("meta.json is JSON")
    }
}
