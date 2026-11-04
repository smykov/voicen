//! Call-track capture ports and the per-track timeline (spec 007 R2, R3, R21; T-124).
//!
//! The audio callback hands blocks to a [`TrackSink`] (in the app: [`ChannelSink`], a bounded
//! queue per track that never blocks). The consumer side places every block on its track's
//! [`Timeline`]: a 16 kHz mono axis whose position is a function of the capture stamps, so
//! both tracks share one clock (`(stamp - call_start) x 16 kHz`).
//!
//! - Small offsets (device clock drift) are trimmed by a slow PI loop on the resampler's
//!   ratio, clamped to +/-500 ppm.
//! - An offset above 40 ms re-anchors at once: ahead -> zeros, behind -> the overlapping head
//!   of the block is dropped. A run of 200 ms or more inserted or dropped is reported.
//! - A channel drop is filled with exact silence; a lost track keeps advancing with zeros.
//!
//! The timeline never logs and never touches storage: it returns [`Step`]s with the samples,
//! the [`Report`]s and the levels, and the caller decides what to write or log.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::Arc;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Adjustable, Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

use super::events::{EndpointId, RefusedReason, Track};
use crate::audio::{mix_to_mono, AudioError};
use crate::platform::{CaptureHandle, DeviceId};

/// A capture instant: nanoseconds on the source's clock (QPC on Windows).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CaptureStamp(pub u64);

impl CaptureStamp {
    /// The stamp `ms` milliseconds after the clock's zero.
    pub fn from_ms(ms: u64) -> CaptureStamp {
        CaptureStamp(ms * 1_000_000)
    }

    /// The stamp in nanoseconds.
    pub fn ns(self) -> u64 {
        self.0
    }
}

/// Which device a call track opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackDevice {
    Selected(DeviceId),
    DefaultInput,
    DefaultOutputLoopback,
}

/// Why a track could not be opened; maps one-to-one onto [`RefusedReason`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenError {
    NoDevice,
    AccessDenied,
    Busy,
    Other,
}

impl From<OpenError> for RefusedReason {
    fn from(e: OpenError) -> RefusedReason {
        match e {
            OpenError::NoDevice => RefusedReason::NoDevice,
            OpenError::AccessDenied => RefusedReason::AccessDenied,
            OpenError::Busy => RefusedReason::Busy,
            OpenError::Other => RefusedReason::Other,
        }
    }
}

/// Why a running track was lost (R4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossCause {
    Invalidated,
    NotAvailable,
    Other,
}

impl LossCause {
    fn code(self) -> u8 {
        match self {
            LossCause::Invalidated => 1,
            LossCause::NotAvailable => 2,
            LossCause::Other => 3,
        }
    }

    fn from_code(code: u8) -> Option<LossCause> {
        match code {
            1 => Some(LossCause::Invalidated),
            2 => Some(LossCause::NotAvailable),
            3 => Some(LossCause::Other),
            _ => None,
        }
    }
}

/// The audio callback's port: hands blocks and losses over, never blocks.
pub trait TrackSink: Send + Sync {
    fn block(&self, track: Track, interleaved: &[f32], rate: u32, channels: u16, at: CaptureStamp);
    fn lost(&self, track: Track, cause: LossCause, at: CaptureStamp);
}

/// A source of call-track audio: the shell's device adapter, or a WAV file in tests.
pub trait TrackSource {
    fn open(
        &self,
        track: Track,
        device: TrackDevice,
        sink: Arc<dyn TrackSink>,
    ) -> Result<(Box<dyn CaptureHandle>, EndpointId), OpenError>;
    fn now(&self) -> CaptureStamp;
}

/// What the consumer side takes out of a track's channel.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackEvent {
    Block {
        interleaved: Vec<f32>,
        rate: u32,
        channels: u16,
        at: CaptureStamp,
    },
    Lost {
        cause: LossCause,
        at: CaptureStamp,
    },
    /// Blocks the full channel refused, and the time they spanned.
    Dropped {
        blocks: u32,
        ms: u32,
    },
}

/// The drop counter packs the block count (high bits) and the dropped time in microseconds
/// (low bits) into one atomic, so the consumer never reads one without the other.
const DROP_US_BITS: u32 = 44;
const DROP_US_MASK: u64 = (1 << DROP_US_BITS) - 1;

/// What one track's producer and consumer share besides the queue.
#[derive(Default)]
struct LaneShared {
    /// Dropped blocks and time not yet delivered (`blocks << 44 | us`).
    dropped: AtomicU64,
    /// A loss that did not fit into the full queue: [`LossCause::code`], 0 = none.
    lost_cause: AtomicU8,
    lost_at: AtomicU64,
}

impl LaneShared {
    fn take_dropped(&self) -> Option<TrackEvent> {
        let v = self.dropped.swap(0, Ordering::AcqRel);
        (v != 0).then(|| unpack_dropped(v))
    }
}

fn unpack_dropped(v: u64) -> TrackEvent {
    let blocks = u32::try_from(v >> DROP_US_BITS).unwrap_or(u32::MAX);
    let ms = u32::try_from(((v & DROP_US_MASK) + 500) / 1_000).unwrap_or(u32::MAX);
    TrackEvent::Dropped { blocks, ms }
}

/// The producer end of one track.
struct Lane {
    tx: SyncSender<TrackEvent>,
    shared: Arc<LaneShared>,
}

impl Lane {
    /// Deliver a pending drop before anything queued after it. `false` when the queue is
    /// still full (the drop stays pending, so what follows must be dropped too).
    fn flush_dropped(&self) -> bool {
        let Some(event) = self.shared.take_dropped() else {
            return true;
        };
        match self.tx.try_send(event) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => true,
            Err(TrySendError::Full(TrackEvent::Dropped { blocks, ms })) => {
                let v = (u64::from(blocks) << DROP_US_BITS) | (u64::from(ms) * 1_000);
                self.shared.dropped.fetch_add(v, Ordering::AcqRel);
                false
            }
            Err(TrySendError::Full(_)) => false,
        }
    }

    fn count_drop(&self, frames: u64, rate: u32) {
        let us = if rate == 0 {
            0
        } else {
            (frames * 1_000_000 + u64::from(rate) / 2) / u64::from(rate)
        };
        let v = (1u64 << DROP_US_BITS) | (us & DROP_US_MASK);
        self.shared.dropped.fetch_add(v, Ordering::AcqRel);
    }
}

/// The bounded callback channel (one queue and one drop counter per track).
///
/// `block` and `lost` never wait: a block that does not fit is counted (blocks and time) and
/// later delivered as one [`TrackEvent::Dropped`]; a loss that does not fit is kept in a flag
/// and delivered when the queue drains, so a loss is never dropped.
pub struct ChannelSink {
    mic: Lane,
    system: Lane,
}

/// The consumer end of one track's channel.
pub struct TrackReceiver {
    rx: Receiver<TrackEvent>,
    shared: Arc<LaneShared>,
}

impl ChannelSink {
    /// A sink with `capacity` queued events per track (at least 1), and the mic and system
    /// receivers.
    pub fn new(capacity: usize) -> (ChannelSink, TrackReceiver, TrackReceiver) {
        let (mic, mic_rx) = Self::lane(capacity);
        let (system, system_rx) = Self::lane(capacity);
        (ChannelSink { mic, system }, mic_rx, system_rx)
    }

    fn lane(capacity: usize) -> (Lane, TrackReceiver) {
        let (tx, rx) = mpsc::sync_channel(capacity.max(1));
        let shared = Arc::new(LaneShared::default());
        (
            Lane {
                tx,
                shared: shared.clone(),
            },
            TrackReceiver { rx, shared },
        )
    }

    fn lane_of(&self, track: Track) -> &Lane {
        match track {
            Track::Mic => &self.mic,
            Track::System => &self.system,
        }
    }
}

impl TrackSink for ChannelSink {
    fn block(&self, track: Track, interleaved: &[f32], rate: u32, channels: u16, at: CaptureStamp) {
        let lane = self.lane_of(track);
        let frames = match usize::from(channels) {
            0 => 0,
            c => (interleaved.len() / c) as u64,
        };
        if !lane.flush_dropped() {
            lane.count_drop(frames, rate);
            return;
        }
        let event = TrackEvent::Block {
            interleaved: interleaved.to_vec(),
            rate,
            channels,
            at,
        };
        if let Err(TrySendError::Full(_)) = lane.tx.try_send(event) {
            lane.count_drop(frames, rate);
        }
    }

    fn lost(&self, track: Track, cause: LossCause, at: CaptureStamp) {
        let lane = self.lane_of(track);
        // The drop happened before the loss: deliver it first when there is room.
        let room = lane.flush_dropped();
        let sent = room
            && match lane.tx.try_send(TrackEvent::Lost { cause, at }) {
                Ok(()) | Err(TrySendError::Disconnected(_)) => true,
                Err(TrySendError::Full(_)) => false,
            };
        if !sent {
            lane.shared.lost_at.store(at.0, Ordering::Release);
            lane.shared
                .lost_cause
                .store(cause.code(), Ordering::Release);
        }
    }
}

impl TrackReceiver {
    /// The next event, or `None` when nothing is pending. A pending drop comes after the
    /// blocks queued before it; a loss kept in the flag comes after the drop.
    pub fn try_recv(&self) -> Option<TrackEvent> {
        if let Ok(event) = self.rx.try_recv() {
            return Some(event);
        }
        if let Some(event) = self.shared.take_dropped() {
            return Some(event);
        }
        let cause = LossCause::from_code(self.shared.lost_cause.swap(0, Ordering::AcqRel))?;
        let at = CaptureStamp(self.shared.lost_at.load(Ordering::Acquire));
        Some(TrackEvent::Lost { cause, at })
    }
}

/// Something the timeline reports instead of logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Report {
    /// A span of 200 ms or more inserted (as zeros) or dropped (as overlap).
    Gap { at_ms: u64, gap_ms: u64 },
    /// A full channel dropped `blocks` blocks spanning `ms`.
    Dropped { blocks: u32, ms: u32 },
}

/// What one timeline call produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Step {
    /// 16 kHz mono samples, in order, to hand to the encoder.
    pub pcm: Vec<f32>,
    pub reports: Vec<Report>,
    /// One linear RMS (0..1) per completed 1600-sample window of `pcm`.
    pub levels: Vec<f32>,
}

/// The output rate of every track.
const OUT_RATE: f64 = 16_000.0;
/// Output samples per nanosecond of stamp time.
const PER_NS: f64 = OUT_RATE / 1e9;
/// Offsets above this re-anchor at once (40 ms).
const STEP_SAMPLES: f64 = 40.0 * 16.0;
/// Runs inserted or dropped from this length on are reported (200 ms).
const GAP_SAMPLES: f64 = 200.0 * 16.0;
/// A live track is filled by `advance_to` only this far behind the stamp (200 ms).
const HOLD_OFF_SAMPLES: f64 = 200.0 * 16.0;
/// The slow loop's trim limit (500 ppm).
const MAX_ADJ: f64 = 500e-6;
/// The resampler's ratio range around the nominal one (above `1 + MAX_ADJ`).
const MAX_RELATIVE: f64 = 1.001;
/// PI gains on the offset in seconds: natural frequency 0.1 rad/s, damping 0.7.
const KP: f64 = 0.14;
const KI: f64 = 0.01;
/// The loop's time step is capped so a long pause does not kick the integrator.
const MAX_DT_S: f64 = 1.0;
/// One level per 100 ms of output.
const LEVEL_WINDOW: usize = 1_600;
/// The sinc filter length (input frames).
const SINC_LEN: usize = 64;

/// One input rate's streaming resampler to 16 kHz, with its input queue.
struct Converter {
    rate: u32,
    resampler: Async<f32>,
    /// Input frames per `process` call (10 ms).
    chunk: usize,
    /// Mono input not yet handed to the resampler.
    fifo: Vec<f32>,
    /// Output position (on the track's axis) of `fifo[0]`.
    proc_pos: f64,
    /// Leading output samples to discard (the start lands before the filter delay).
    skip: usize,
    /// Nominal output samples per input frame (`16000 / rate`).
    per_frame: f64,
    /// The relative ratio in force (`1 + ratio_adj`).
    rel: f64,
    out: Vec<f32>,
}

impl Converter {
    fn new(rate: u32, rel: f64) -> Result<Converter, AudioError> {
        let per_frame = OUT_RATE / f64::from(rate);
        let chunk = usize::try_from(rate / 100).unwrap_or(1).max(1);
        let params = SincInterpolationParameters::new(SINC_LEN, WindowFunction::BlackmanHarris2)
            .interpolation(SincInterpolationType::Linear);
        let mut resampler = Async::<f32>::new_sinc(
            per_frame,
            MAX_RELATIVE,
            &params,
            chunk,
            1,
            FixedAsync::Input,
        )
        .map_err(|_| AudioError::Resample)?;
        resampler
            .set_resample_ratio_relative(rel, false)
            .map_err(|_| AudioError::Resample)?;
        let out = vec![0.0; resampler.output_frames_max()];
        Ok(Converter {
            rate,
            resampler,
            chunk,
            fifo: Vec::new(),
            proc_pos: 0.0,
            skip: 0,
            per_frame,
            rel,
            out,
        })
    }

    /// The resampler's delay in output samples.
    fn delay(&self) -> f64 {
        self.resampler.output_delay() as f64
    }

    /// Output samples per input frame at the current trim.
    fn out_per_in(&self) -> f64 {
        self.per_frame * self.rel
    }

    /// Output position of the next input frame appended.
    fn in_pos(&self) -> f64 {
        self.proc_pos + self.fifo.len() as f64 * self.out_per_in()
    }

    fn set_rel(&mut self, rel: f64) -> Result<(), AudioError> {
        self.resampler
            .set_resample_ratio_relative(rel, false)
            .map_err(|_| AudioError::Resample)?;
        self.rel = rel;
        Ok(())
    }

    /// Resample every full chunk of the queue, appending the output to `dst`.
    fn run(&mut self, dst: &mut Vec<f32>) -> Result<(), AudioError> {
        let mut off = 0;
        let mut result = Ok(());
        while self.fifo.len() - off >= self.chunk {
            let input = &self.fifo[off..off + self.chunk];
            let produced = match process(&mut self.resampler, input, &mut self.out) {
                Ok(n) => n,
                Err(e) => {
                    result = Err(e);
                    break;
                }
            };
            self.proc_pos += self.chunk as f64 * self.out_per_in();
            let skipped = self.skip.min(produced);
            self.skip -= skipped;
            dst.extend_from_slice(&self.out[skipped..produced]);
            off += self.chunk;
        }
        self.fifo.drain(..off);
        result
    }
}

/// One `process_into_buffer` call on mono slices; the number of output frames.
fn process(
    resampler: &mut Async<f32>,
    input: &[f32],
    out: &mut [f32],
) -> Result<usize, AudioError> {
    let input = InterleavedSlice::new(input, 1, input.len()).map_err(|_| AudioError::Resample)?;
    let frames = out.len();
    let mut output = InterleavedSlice::new_mut(out, 1, frames).map_err(|_| AudioError::Resample)?;
    let (_, produced) = resampler
        .process_into_buffer(&input, &mut output, None)
        .map_err(|_| AudioError::Resample)?;
    Ok(produced)
}

/// A contiguous span inserted or dropped, on the output axis.
#[derive(Debug, Clone, Copy, Default)]
struct Run {
    start: f64,
    len: f64,
}

impl Run {
    fn add(&mut self, start: f64, len: f64) {
        if self.len <= 0.0 {
            self.start = start;
            self.len = 0.0;
        }
        self.len += len;
    }

    /// The report for a run of 200 ms or more; the run ends either way.
    fn close(&mut self) -> Option<Report> {
        let run = std::mem::take(self);
        (run.len + 0.5 >= GAP_SAMPLES).then(|| Report::Gap {
            at_ms: (run.start.max(0.0) / 16.0).round() as u64,
            gap_ms: (run.len / 16.0).round() as u64,
        })
    }
}

/// One track's 16 kHz axis, placed from capture stamps.
///
/// Invariant: after any call, `written()` is within the resampler's delay (a few ms) of
/// `(S - call_start) x 16 kHz` for the latest stamp `S` handled, unless the slow loop is
/// still absorbing an offset below 40 ms. Every sample comes from a block or is a zero the
/// timeline inserted.
pub struct Timeline {
    track: Track,
    call_start: CaptureStamp,
    written: u64,
    lost: bool,
    conv: Option<Converter>,
    adj: f64,
    integ: f64,
    loop_at: Option<CaptureStamp>,
    reanchors: u32,
    fill: Run,
    drop: Run,
    level_sum: f64,
    level_len: usize,
    mono: Vec<f32>,
    scratch: Vec<f32>,
}

impl Timeline {
    pub fn new(track: Track, call_start: CaptureStamp) -> Timeline {
        Timeline {
            track,
            call_start,
            written: 0,
            lost: false,
            conv: None,
            adj: 0.0,
            integ: 0.0,
            loop_at: None,
            reanchors: 0,
            fill: Run::default(),
            drop: Run::default(),
            level_sum: 0.0,
            level_len: 0,
            mono: Vec::new(),
            scratch: Vec::new(),
        }
    }

    /// The track this timeline places.
    pub fn track(&self) -> Track {
        self.track
    }

    /// Place a block whose first frame was captured at `at`.
    ///
    /// A block after a loss revives the track. On `Err` nothing was emitted.
    pub fn push(
        &mut self,
        interleaved: &[f32],
        rate: u32,
        channels: u16,
        at: CaptureStamp,
    ) -> Result<Step, AudioError> {
        let mut mono = std::mem::take(&mut self.mono);
        mono.clear();
        let result = match mix_to_mono(interleaved, channels, &mut mono) {
            Err(e) => Err(e),
            Ok(()) if rate == 0 => Err(AudioError::ZeroRate),
            Ok(()) => {
                let saved = (self.written, self.level_sum, self.level_len);
                let placed = self.place(&mono, rate, at);
                if placed.is_err() {
                    // A resampler failure: forget the converter and what this call emitted;
                    // the next block starts afresh at its own stamp.
                    self.conv = None;
                    (self.written, self.level_sum, self.level_len) = saved;
                }
                placed
            }
        };
        self.mono = mono;
        result
    }

    fn place(&mut self, mono: &[f32], rate: u32, at: CaptureStamp) -> Result<Step, AudioError> {
        let mut step = Step::default();
        let expected = self.pos(at);
        let fresh = match &self.conv {
            Some(c) if c.rate == rate => None,
            _ => Some(Converter::new(rate, 1.0 + self.adj)?),
        };
        let mut head = 0usize;
        if let Some(mut conv) = fresh {
            // A new start (first block, after a loss, or a new rate): flush the old
            // converter, then put the block's first frame exactly at its stamp position.
            self.retire(&mut step);
            if self.lost {
                self.lost = false;
                self.fill = Run::default();
                self.drop = Run::default();
            }
            let floor = self.written as f64;
            let target = expected.max(floor);
            let per = conv.out_per_in();
            head = frames_for(target - expected, per);
            if target > expected.max(0.0) {
                self.drop.add(target, target - expected.max(0.0));
            }
            let lead = target - conv.delay() - floor;
            if lead >= 0.5 {
                let zeros = lead.round() as u64;
                self.fill.add(floor, zeros as f64);
                self.emit_zeros(&mut step, zeros);
            } else if lead < 0.0 {
                conv.skip = (-lead).round() as usize;
            }
            conv.proc_pos = target;
            self.conv = Some(conv);
            self.loop_at = Some(at);
        } else if let Some(conv) = self.conv.as_mut() {
            let in_pos = conv.in_pos();
            let error = expected - in_pos;
            let per = conv.out_per_in();
            if error > STEP_SAMPLES {
                let zeros = frames_for(error, per);
                conv.fifo.resize(conv.fifo.len() + zeros, 0.0);
                self.fill.add(in_pos, zeros as f64 * per);
                self.reanchor(at)?;
            } else if error < -STEP_SAMPLES {
                head = frames_for(-error, per).min(mono.len());
                self.drop.add(in_pos, head as f64 * per);
                self.reanchor(at)?;
            } else {
                self.steer(error, at)?;
            }
        }
        let kept = mono.get(head..).unwrap_or(&[]);
        if !kept.is_empty() {
            step.reports.extend(self.fill.close());
            step.reports.extend(self.drop.close());
        }
        if let Some(conv) = self.conv.as_mut() {
            conv.fifo.extend_from_slice(kept);
        }
        self.drain(&mut step)?;
        Ok(step)
    }

    /// Zero-fill exactly `ms` for `blocks` blocks the channel dropped.
    ///
    /// The zeros go out at once; audio still inside the resampler follows them, and the next
    /// block's expected position moves by the same span, so placement stays exact.
    pub fn dropped(&mut self, blocks: u32, ms: u32) -> Step {
        let mut step = Step::default();
        let zeros = u64::from(ms) * 16;
        self.emit_zeros(&mut step, zeros);
        if let Some(conv) = self.conv.as_mut() {
            conv.proc_pos += zeros as f64;
        }
        step.reports.push(Report::Dropped { blocks, ms });
        step
    }

    /// Mark the track lost at `at`: what is still inside the resampler is flushed, then zeros
    /// up to `at`. Later [`advance_to`](Timeline::advance_to) calls fill zeros without a
    /// hold-off.
    pub fn lost(&mut self, _cause: LossCause, at: CaptureStamp) -> Step {
        let mut step = Step::default();
        self.retire(&mut step);
        self.lost = true;
        self.fill = Run::default();
        self.drop = Run::default();
        self.adj = 0.0;
        self.integ = 0.0;
        let target = self.pos(at);
        self.fill_direct(&mut step, target);
        step
    }

    /// The other track's stamp, or the session tick: advance this track to `at`.
    ///
    /// A lost track is filled with zeros up to `at`; a live one only up to 200 ms before it,
    /// so a block that arrives late is never clipped.
    pub fn advance_to(&mut self, at: CaptureStamp) -> Step {
        let mut step = Step::default();
        let expected = self.pos(at);
        if self.lost {
            self.fill_direct(&mut step, expected);
            return step;
        }
        let target = expected - HOLD_OFF_SAMPLES;
        match self.conv.as_mut() {
            None => {
                let floor = self.written as f64;
                if target - floor >= 0.5 {
                    self.fill.add(floor, (target - floor).round());
                    self.fill_direct(&mut step, target);
                }
            }
            Some(conv) => {
                let in_pos = conv.in_pos();
                let per = conv.out_per_in();
                let zeros = frames_for(target - in_pos, per);
                if zeros > 0 {
                    conv.fifo.resize(conv.fifo.len() + zeros, 0.0);
                    self.fill.add(in_pos, zeros as f64 * per);
                    if self.drain(&mut step).is_err() {
                        self.conv = None;
                        self.fill_direct(&mut step, target);
                    }
                }
            }
        }
        step
    }

    /// 16 kHz samples emitted so far.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// The slow loop's relative ratio trim, in ppm (positive = output stretched).
    pub fn ratio_adj_ppm(&self) -> f64 {
        self.adj * 1e6
    }

    /// How many times the offset exceeded 40 ms and the timeline re-anchored at once.
    pub fn reanchors(&self) -> u32 {
        self.reanchors
    }

    pub fn is_lost(&self) -> bool {
        self.lost
    }

    /// The output position of `at` (negative before the call start).
    fn pos(&self, at: CaptureStamp) -> f64 {
        (at.0 as f64 - self.call_start.0 as f64) * PER_NS
    }

    /// A step re-anchor: count it and restart the slow loop from zero.
    fn reanchor(&mut self, at: CaptureStamp) -> Result<(), AudioError> {
        self.reanchors = self.reanchors.saturating_add(1);
        self.integ = 0.0;
        self.adj = 0.0;
        self.loop_at = Some(at);
        match self.conv.as_mut() {
            Some(conv) => conv.set_rel(1.0),
            None => Ok(()),
        }
    }

    /// One PI update on an offset below the step threshold (`error` in output samples,
    /// positive = the block belongs later than the output has reached).
    fn steer(&mut self, error: f64, at: CaptureStamp) -> Result<(), AudioError> {
        let dt = self
            .loop_at
            .map_or(0.0, |prev| at.0.saturating_sub(prev.0) as f64 / 1e9)
            .min(MAX_DT_S);
        self.loop_at = Some(at);
        let e = error / OUT_RATE;
        let proportional = KP * e;
        let candidate = self.integ + KI * e * dt;
        let unclamped = proportional + candidate;
        // Anti-windup: the integrator does not grow while the output is saturated in the
        // direction the offset pushes.
        if unclamped.abs() <= MAX_ADJ || unclamped.signum() != e.signum() {
            self.integ = candidate.clamp(-MAX_ADJ, MAX_ADJ);
        }
        self.adj = (proportional + self.integ).clamp(-MAX_ADJ, MAX_ADJ);
        match self.conv.as_mut() {
            Some(conv) => conv.set_rel(1.0 + self.adj),
            None => Ok(()),
        }
    }

    /// Resample what is queued and emit it.
    fn drain(&mut self, step: &mut Step) -> Result<(), AudioError> {
        let Some(conv) = self.conv.as_mut() else {
            return Ok(());
        };
        let mut out = std::mem::take(&mut self.scratch);
        out.clear();
        let result = conv.run(&mut out);
        self.emit(step, &out);
        self.scratch = out;
        result
    }

    /// Flush the converter exactly up to its input position and drop it.
    fn retire(&mut self, step: &mut Step) {
        let Some(mut conv) = self.conv.take() else {
            return;
        };
        let end = conv.in_pos().round().max(0.0);
        let need = (end - self.written as f64).max(0.0) as usize;
        let mut out = std::mem::take(&mut self.scratch);
        out.clear();
        // The tail needs at most the filter delay plus one chunk; bounded in any case.
        for _ in 0..64 {
            if out.len() >= need {
                break;
            }
            let pad = conv.chunk - conv.fifo.len() % conv.chunk;
            conv.fifo.resize(conv.fifo.len() + pad, 0.0);
            if conv.run(&mut out).is_err() {
                break;
            }
        }
        out.truncate(need);
        self.emit(step, &out);
        self.scratch = out;
        self.fill_direct(step, end);
    }

    /// Emit zeros up to `target` (output position), if it is ahead.
    fn fill_direct(&mut self, step: &mut Step, target: f64) {
        let floor = self.written as f64;
        if target - floor >= 0.5 {
            self.emit_zeros(step, (target - floor).round() as u64);
        }
    }

    fn emit(&mut self, step: &mut Step, samples: &[f32]) {
        for &x in samples {
            let x = f64::from(x);
            self.level_sum += x * x;
            self.level_len += 1;
            if self.level_len == LEVEL_WINDOW {
                self.push_level(step);
            }
        }
        step.pcm.extend_from_slice(samples);
        self.written += samples.len() as u64;
    }

    fn emit_zeros(&mut self, step: &mut Step, count: u64) {
        let Ok(count) = usize::try_from(count) else {
            return;
        };
        let mut left = count;
        while left > 0 {
            let take = (LEVEL_WINDOW - self.level_len).min(left);
            self.level_len += take;
            left -= take;
            if self.level_len == LEVEL_WINDOW {
                self.push_level(step);
            }
        }
        step.pcm.resize(step.pcm.len() + count, 0.0);
        self.written += count as u64;
    }

    fn push_level(&mut self, step: &mut Step) {
        let rms = (self.level_sum / LEVEL_WINDOW as f64).sqrt().min(1.0);
        step.levels.push(rms as f32);
        self.level_sum = 0.0;
        self.level_len = 0;
    }
}

/// Input frames that cover `span` output samples at `per` samples per frame (0 if none).
fn frames_for(span: f64, per: f64) -> usize {
    if span <= 0.0 || per <= 0.0 {
        return 0;
    }
    (span / per).round() as usize
}

#[cfg(test)]
mod tests {
    //! T-124 red tests. No wall clock and no thread: the timeline is a pure function of the
    //! stamps the tests hand it (core-tests.md), so every verdict is deterministic.

    use std::sync::{Arc, Mutex, PoisonError};

    use super::super::wav_source::WavTrackSource;
    use super::*;

    fn ms(n: u64) -> CaptureStamp {
        CaptureStamp::from_ms(n)
    }

    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Everything a timeline emitted, in order.
    #[derive(Default)]
    struct Out {
        pcm: Vec<f32>,
        reports: Vec<Report>,
        levels: Vec<f32>,
    }

    impl Out {
        fn add(&mut self, s: Step) {
            self.pcm.extend(s.pcm);
            self.reports.extend(s.reports);
            self.levels.extend(s.levels);
        }

        fn gaps(&self) -> Vec<(u64, u64)> {
            self.reports
                .iter()
                .filter_map(|r| match r {
                    Report::Gap { at_ms, gap_ms } => Some((*at_ms, *gap_ms)),
                    Report::Dropped { .. } => None,
                })
                .collect()
        }
    }

    fn n(count_ms: u64, per_ms: u64) -> usize {
        usize::try_from(count_ms * per_ms).unwrap()
    }

    /// Push 16 kHz mono blocks of `v`, `block_ms` long, from `from` up to (not past) `to`.
    fn feed(tl: &mut Timeline, out: &mut Out, from: u64, to: u64, block_ms: u64, v: f32) {
        let data = vec![v; n(block_ms, 16)];
        let mut t = from;
        while t < to {
            out.add(tl.push(&data, 16_000, 1, ms(t)).unwrap());
            t += block_ms;
        }
    }

    fn push_mono(tl: &mut Timeline, out: &mut Out, at: u64, data: &[f32]) {
        out.add(tl.push(data, 16_000, 1, ms(at)).unwrap());
    }

    fn mic() -> Timeline {
        Timeline::new(Track::Mic, ms(0))
    }

    fn near(a: u64, b: u64, tol: u64) -> bool {
        a.abs_diff(b) <= tol
    }

    // ---- placement (AC-1) ---------------------------------------------------------------

    /// A block of 100 ms silence then a 0.5 plateau, `frames` long at `rate`.
    fn step_block(rate: usize, channels: usize, left_right: [f32; 2]) -> Vec<f32> {
        let frames = rate; // 1 s
        let lead = rate / 10; // 100 ms
        let mut v = Vec::with_capacity(frames * channels);
        for f in 0..frames {
            for c in 0..channels {
                v.push(if f < lead { 0.0 } else { left_right[c.min(1)] });
            }
        }
        v
    }

    fn onset(pcm: &[f32]) -> usize {
        pcm.iter()
            .position(|x| *x > 0.25)
            .expect("no onset in the output")
    }

    #[test]
    fn a_48k_stereo_block_lands_at_the_stamp_position_downmixed_and_resampled() {
        // call_start 1 s; block stamped 500 ms later; its step is 100 ms into the block.
        let mut tl = Timeline::new(Track::System, ms(1_000));
        let mut out = Out::default();
        out.add(
            tl.push(&step_block(48_000, 2, [1.0, 0.0]), 48_000, 2, ms(1_500))
                .unwrap(),
        );
        // A follower block so the resampler's tail is out.
        out.add(
            tl.push(&vec![0.5; 2 * 4_800], 48_000, 2, ms(2_500))
                .unwrap(),
        );
        let at = onset(&out.pcm);
        assert!(near(at as u64, 8_000 + 1_600, 80), "onset at {at}");
        // Everything before the block is exact zeros (the file starts at the call start).
        assert!(out.pcm[..9_400].iter().all(|x| x.abs() < 1e-6));
        // Downmix is the channel average: (1.0 + 0.0) / 2.
        assert!(out.pcm[at + 800..at + 6_000]
            .iter()
            .all(|x| (x - 0.5).abs() < 0.02));
    }

    #[test]
    fn a_16k_mono_block_lands_at_the_stamp_position() {
        let mut tl = Timeline::new(Track::Mic, ms(1_000));
        let mut out = Out::default();
        out.add(
            tl.push(&step_block(16_000, 1, [0.5, 0.5]), 16_000, 1, ms(1_500))
                .unwrap(),
        );
        out.add(tl.push(&vec![0.5; 1_600], 16_000, 1, ms(2_500)).unwrap());
        let at = onset(&out.pcm);
        assert!(near(at as u64, 8_000 + 1_600, 80), "onset at {at}");
        assert!(out.pcm[..9_400].iter().all(|x| x.abs() < 1e-6));
        assert!(out.pcm[at + 800..at + 6_000]
            .iter()
            .all(|x| (x - 0.5).abs() < 0.02));
    }

    #[test]
    fn a_ragged_block_is_an_error_not_a_panic() {
        let mut tl = mic();
        assert_eq!(
            tl.push(&[0.0; 5], 48_000, 2, ms(0)),
            Err(AudioError::RaggedFrames)
        );
        assert_eq!(
            tl.push(&[0.0; 4], 48_000, 0, ms(0)),
            Err(AudioError::ZeroChannels)
        );
        assert_eq!(tl.push(&[0.0; 4], 0, 1, ms(0)), Err(AudioError::ZeroRate));
    }

    // ---- drift (AC-2) -------------------------------------------------------------------

    /// A sink that places everything straight on a timeline and keeps what the checks need.
    struct Direct {
        timeline: Mutex<Timeline>,
        last: Mutex<CaptureStamp>,
        max_abs_ppm: Mutex<f64>,
        gaps: Mutex<Vec<(u64, u64)>>,
    }

    impl Direct {
        fn new(track: Track, call_start: CaptureStamp) -> Arc<Direct> {
            Arc::new(Direct {
                timeline: Mutex::new(Timeline::new(track, call_start)),
                last: Mutex::new(call_start),
                max_abs_ppm: Mutex::new(0.0),
                gaps: Mutex::new(Vec::new()),
            })
        }
    }

    impl TrackSink for Direct {
        fn block(&self, _: Track, interleaved: &[f32], rate: u32, channels: u16, at: CaptureStamp) {
            let mut tl = lock(&self.timeline);
            let step = tl.push(interleaved, rate, channels, at).unwrap();
            for r in &step.reports {
                if let Report::Gap { at_ms, gap_ms } = r {
                    lock(&self.gaps).push((*at_ms, *gap_ms));
                }
            }
            let mut max = lock(&self.max_abs_ppm);
            *max = max.max(tl.ratio_adj_ppm().abs());
            *lock(&self.last) = at;
        }
        fn lost(&self, _: Track, _: LossCause, _: CaptureStamp) {}
    }

    /// A 1 s silent 16-bit mono 16 kHz WAV; the source pushes silence past its end.
    fn silent_wav() -> Vec<u8> {
        let data = vec![0u8; 32_000];
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + 32_000u32).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&16_000u32.to_le_bytes());
        v.extend_from_slice(&32_000u32.to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&32_000u32.to_le_bytes());
        v.extend_from_slice(&data);
        v
    }

    /// Run a track fed by a WAV source with drift `d` for `secs` of QPC time.
    fn run_drifting(track: Track, d: f64, secs: u64) -> Arc<Direct> {
        let sink = Direct::new(track, ms(1_000));
        let src = WavTrackSource::from_bytes(&silent_wav())
            .unwrap()
            .with_block_ms(100)
            .with_drift(d)
            .with_start(ms(1_000));
        let _open = src
            .open(track, TrackDevice::DefaultInput, sink.clone())
            .unwrap();
        src.pump(ms(1_000 + secs * 1_000));
        sink
    }

    fn end_error(sink: &Direct) -> u64 {
        let last = lock(&sink.last).ns() - 1_000_000_000;
        let expected_end = last * 16 / 1_000_000 + 1_600; // last stamp + one 100 ms block
        lock(&sink.timeline).written().abs_diff(expected_end)
    }

    #[test]
    fn plus_and_minus_50_ppm_sources_stay_within_20_ms_at_minute_60() {
        let fast = run_drifting(Track::Mic, 50e-6, 3_600);
        let slow = run_drifting(Track::System, -50e-6, 3_600);
        // Both tracks are compared at ONE stamp: each track's placement error is measured
        // against the position its own last stamp implies (the two sources' last stamps
        // differ by up to a block, so their raw `written()` values must not be compared),
        // and the difference of the signed errors is the cross-track offset at one stamp.
        let signed = |s: &Direct| -> i64 {
            let last = lock(&s.last).ns() - 1_000_000_000;
            let expected_end = (last * 16 / 1_000_000 + 1_600) as i64;
            lock(&s.timeline).written() as i64 - expected_end
        };
        let (ef, es) = (signed(&fast), signed(&slow));
        assert!(
            (ef - es).abs() <= 320,
            "tracks differ by {} samples at one stamp (errors {ef} / {es})",
            (ef - es).abs()
        );
        assert!(
            end_error(&fast) <= 320,
            "fast track off by {}",
            end_error(&fast)
        );
        assert!(
            end_error(&slow) <= 320,
            "slow track off by {}",
            end_error(&slow)
        );
        // The slow loop did the work: no step re-anchor, and the trim has the sign that
        // cancels the source's drift (a fast device delivers too many samples per second).
        assert_eq!(lock(&fast.timeline).reanchors(), 0);
        assert_eq!(lock(&slow.timeline).reanchors(), 0);
        let (pf, ps) = (
            lock(&fast.timeline).ratio_adj_ppm(),
            lock(&slow.timeline).ratio_adj_ppm(),
        );
        assert!((pf + 50.0).abs() < 20.0, "fast ratio_adj {pf} ppm");
        assert!((ps - 50.0).abs() < 20.0, "slow ratio_adj {ps} ppm");
    }

    #[test]
    fn a_1000_ppm_feed_keeps_ratio_adj_clamped_at_500_ppm_and_still_reanchors() {
        for d in [1000e-6, -1000e-6, 5000e-6] {
            let sink = run_drifting(Track::Mic, d, 600);
            let max = *lock(&sink.max_abs_ppm);
            assert!(max <= 500.0 + 1e-6, "d={d}: ratio_adj reached {max} ppm");
            assert!(max >= 250.0, "d={d}: the loop never pushed ({max} ppm)");
            assert!(lock(&sink.timeline).reanchors() > 0, "d={d}: no re-anchor");
            // Re-anchoring keeps the position inside the step threshold (plus a block).
            assert!(
                end_error(&sink) <= 40 * 16 + 1_600 / 2,
                "d={d}: off by {}",
                end_error(&sink)
            );
        }
    }

    // ---- re-anchor and gaps (AC-3) -------------------------------------------------------

    fn ahead_by(gap_ms: u64) -> (Timeline, Out) {
        let mut tl = mic();
        let mut out = Out::default();
        feed(&mut tl, &mut out, 0, 100, 10, 0.5);
        feed(&mut tl, &mut out, 100 + gap_ms, 300 + gap_ms, 10, 0.5);
        (tl, out)
    }

    #[test]
    fn a_block_41_ms_ahead_is_preceded_by_exactly_the_missing_zeros() {
        let (tl, out) = ahead_by(41);
        // Zeros fill the 41 ms hole (1600..2256); the margins leave room for filter ringing.
        assert!(out.pcm[1_760..2_096].iter().all(|x| x.abs() < 1e-4));
        assert!(out.pcm[200..1_440].iter().all(|x| *x > 0.4));
        assert!(out.pcm[2_416..4_400].iter().all(|x| *x > 0.4));
        assert!(
            near(tl.written(), 341 * 16, 320),
            "written {}",
            tl.written()
        );
        assert_eq!(tl.reanchors(), 1);
    }

    #[test]
    fn a_block_39_ms_ahead_is_left_to_the_slow_loop() {
        let (tl, out) = ahead_by(39);
        assert!(
            out.pcm[200..4_400].iter().all(|x| *x > 0.4),
            "zeros were inserted"
        );
        assert!(
            near(tl.written(), 300 * 16, 320),
            "written {}",
            tl.written()
        );
        assert_eq!(tl.reanchors(), 0);
    }

    /// 100 ms of 0.5, then a 100 ms block `back` ms before the written end: its head (0.9)
    /// overlaps what is written, its rest is 0.2; then 0.2 follows contiguously.
    fn behind_by(back: u64) -> (Timeline, Out) {
        let mut tl = mic();
        let mut out = Out::default();
        feed(&mut tl, &mut out, 0, 100, 10, 0.5);
        let head = n(back, 16);
        let mut block = vec![0.9; head];
        block.extend(vec![0.2; 1_600 - head]);
        push_mono(&mut tl, &mut out, 100 - back, &block);
        feed(&mut tl, &mut out, 200 - back, 300, 10, 0.2);
        (tl, out)
    }

    #[test]
    fn a_block_41_ms_behind_has_its_overlap_dropped() {
        let (tl, out) = behind_by(41);
        assert!(
            out.pcm[1_760..].iter().all(|x| *x < 0.6),
            "the overlapping head was kept"
        );
        assert!(
            near(tl.written(), 300 * 16, 320),
            "written {}",
            tl.written()
        );
        assert_eq!(tl.reanchors(), 1);
    }

    #[test]
    fn a_block_39_ms_behind_keeps_all_its_samples() {
        let (tl, out) = behind_by(39);
        assert!(
            out.pcm[1_600..].iter().any(|x| *x > 0.8),
            "the head was dropped"
        );
        assert!(
            near(tl.written(), 339 * 16, 320),
            "written {}",
            tl.written()
        );
        assert_eq!(tl.reanchors(), 0);
    }

    #[test]
    fn a_gap_of_200_ms_is_reported_and_one_of_199_ms_is_not() {
        let (_, out) = ahead_by(200);
        let gaps = out.gaps();
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        assert!(
            near(gaps[0].0, 100, 10) && near(gaps[0].1, 200, 8),
            "{gaps:?}"
        );

        let (_, out) = ahead_by(199);
        assert!(out.gaps().is_empty(), "{:?}", out.gaps());
        assert!(
            out.pcm[1_760..4_700].iter().any(|x| x.abs() < 1e-4),
            "199 ms is still filled"
        );
    }

    #[test]
    fn an_overlap_of_250_ms_dropped_is_reported_as_a_gap_and_199_ms_is_not() {
        for (back, reported) in [(250u64, true), (199, false)] {
            let mut tl = mic();
            let mut out = Out::default();
            feed(&mut tl, &mut out, 0, 1_000, 100, 0.5);
            push_mono(&mut tl, &mut out, 1_000 - back, &vec![0.5; n(500, 16)]);
            let gaps = out.gaps();
            if reported {
                assert_eq!(gaps.len(), 1, "back={back}: {gaps:?}");
                assert!(near(gaps[0].1, back, 10), "{gaps:?}");
            } else {
                assert!(gaps.is_empty(), "back={back}: {gaps:?}");
            }
        }
    }

    // ---- lost and silent tracks (AC-4) ---------------------------------------------------

    #[test]
    fn a_lost_track_keeps_advancing_with_zeros_from_the_other_tracks_stamps() {
        let mut tl = mic();
        let mut out = Out::default();
        feed(&mut tl, &mut out, 0, 100, 10, 0.5);
        out.add(tl.lost(LossCause::Invalidated, ms(100)));
        assert!(tl.is_lost());
        let kept = out.pcm.len();
        out.add(tl.advance_to(ms(1_000)));
        assert!(
            near(tl.written(), 1_000 * 16, 160),
            "written {}",
            tl.written()
        );
        out.add(tl.advance_to(ms(2_000)));
        assert!(
            near(tl.written(), 2_000 * 16, 160),
            "written {}",
            tl.written()
        );
        // Whatever of the real audio was still inside the resampler is out within 10 ms of the
        // loss; everything after is exact zeros.
        assert!(out.pcm[kept + 160..].iter().all(|x| *x == 0.0));
        assert_eq!(tl.written(), out.pcm.len() as u64);
        // A stamp that is not later adds nothing.
        let step = tl.advance_to(ms(1_500));
        assert!(step.pcm.is_empty() && step.levels.is_empty());
    }

    #[test]
    fn a_lost_track_with_no_block_ever_still_fills_from_the_call_start() {
        let mut tl = Timeline::new(Track::System, ms(500));
        let mut out = Out::default();
        out.add(tl.lost(LossCause::NotAvailable, ms(500)));
        out.add(tl.advance_to(ms(1_500)));
        assert!(
            near(tl.written(), 1_000 * 16, 160),
            "written {}",
            tl.written()
        );
        assert!(out.pcm.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn a_live_track_keeps_a_late_block_30_ms_behind_the_other_track() {
        let mut tl = mic();
        let mut out = Out::default();
        feed(&mut tl, &mut out, 0, 100, 10, 0.5);
        // The other track is at 130 ms; this track's 100..130 block has not arrived yet.
        out.add(tl.advance_to(ms(130)));
        assert!(
            tl.written() <= 1_600 + 1,
            "zeros inside the hold-off: {}",
            tl.written()
        );
        push_mono(&mut tl, &mut out, 100, &vec![0.5; n(30, 16)]);
        feed(&mut tl, &mut out, 130, 230, 10, 0.5);
        assert!(
            out.pcm[160..3_500].iter().all(|x| *x > 0.4),
            "a sample was clipped or zeroed"
        );
        assert!(
            near(tl.written(), 230 * 16, 320),
            "written {}",
            tl.written()
        );
        assert_eq!(tl.reanchors(), 0);
    }

    #[test]
    fn a_live_track_silent_past_the_hold_off_is_filled_with_zeros() {
        let mut tl = mic();
        let mut out = Out::default();
        feed(&mut tl, &mut out, 0, 100, 10, 0.5);
        out.add(tl.advance_to(ms(1_100)));
        let w = tl.written();
        assert!(
            ((1_100 - 210) * 16..=1_100 * 16 + 160).contains(&w),
            "written {w}"
        );
        assert!(out.pcm[1_760..].iter().all(|x| *x == 0.0));
        assert!(!tl.is_lost());
    }

    // ---- full callback channel (AC-5) ----------------------------------------------------

    fn blk(i: u64) -> TrackEvent {
        TrackEvent::Block {
            interleaved: vec![0.5; 160],
            rate: 16_000,
            channels: 1,
            at: ms(i * 10),
        }
    }

    fn drain(rx: &TrackReceiver) -> Vec<TrackEvent> {
        let mut v = Vec::new();
        while let Some(e) = rx.try_recv() {
            v.push(e);
            assert!(v.len() < 100, "the channel never ends");
        }
        v
    }

    #[test]
    fn a_full_channel_drops_blocks_fills_exact_silence_and_reports_the_drop() {
        let (sink, mic_rx, sys_rx) = ChannelSink::new(2);
        for i in 0..5 {
            sink.block(Track::Mic, &[0.5; 160], 16_000, 1, ms(i * 10));
        }
        sink.block(Track::System, &[0.25; 160], 16_000, 1, ms(0));

        let events = drain(&mic_rx);
        assert_eq!(
            events,
            vec![blk(0), blk(1), TrackEvent::Dropped { blocks: 3, ms: 30 }]
        );
        assert!(drain(&mic_rx).is_empty(), "a drop is reported once");
        // The other track's queue is its own: nothing dropped there.
        assert_eq!(drain(&sys_rx).len(), 1);

        let mut tl = mic();
        let mut out = Out::default();
        let mut dropped_step = None;
        for e in events {
            match e {
                TrackEvent::Block {
                    interleaved,
                    rate,
                    channels,
                    at,
                } => {
                    out.add(tl.push(&interleaved, rate, channels, at).unwrap());
                }
                TrackEvent::Dropped { blocks, ms } => {
                    let step = tl.dropped(blocks, ms);
                    dropped_step = Some(step.clone());
                    out.add(step);
                }
                TrackEvent::Lost { .. } => unreachable!(),
            }
        }
        let step = dropped_step.unwrap();
        // 30 ms is below the 40 ms step threshold, yet the fill is exact, not left to the loop.
        assert_eq!(step.pcm.len(), 480);
        assert!(step.pcm.iter().all(|x| *x == 0.0));
        assert_eq!(step.reports, vec![Report::Dropped { blocks: 3, ms: 30 }]);
        // The next block lands where the dropped ones would have ended.
        feed(&mut tl, &mut out, 50, 100, 10, 0.5);
        assert!(
            near(tl.written(), 100 * 16, 160),
            "written {}",
            tl.written()
        );
        assert_eq!(tl.reanchors(), 0);
    }

    #[test]
    fn a_loss_is_never_dropped_even_when_the_channel_is_full() {
        let (sink, mic_rx, _sys) = ChannelSink::new(1);
        sink.block(Track::Mic, &[0.5; 160], 16_000, 1, ms(0));
        sink.block(Track::Mic, &[0.5; 160], 16_000, 1, ms(10)); // full: dropped
        sink.lost(Track::Mic, LossCause::Invalidated, ms(20));
        let events = drain(&mic_rx);
        assert_eq!(events[0], blk(0));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, TrackEvent::Lost { .. }))
                .count(),
            1,
            "{events:?}"
        );
        assert!(events.contains(&TrackEvent::Lost {
            cause: LossCause::Invalidated,
            at: ms(20)
        }));
        assert!(events.contains(&TrackEvent::Dropped { blocks: 1, ms: 10 }));

        // And through an empty queue.
        let (sink, _mic, sys_rx) = ChannelSink::new(4);
        sink.lost(Track::System, LossCause::NotAvailable, ms(5));
        assert_eq!(
            drain(&sys_rx),
            vec![TrackEvent::Lost {
                cause: LossCause::NotAvailable,
                at: ms(5)
            }]
        );
    }

    // ---- levels (AC-7) -------------------------------------------------------------------

    #[test]
    fn levels_are_one_rms_per_1600_output_samples() {
        let mut tl = mic();
        let mut out = Out::default();
        feed(&mut tl, &mut out, 0, 1_000, 100, 0.5);
        assert_eq!(out.levels.len() as u64, tl.written() / 1_600);
        assert!(out.levels.len() >= 9);
        assert!(
            out.levels[2..].iter().all(|l| (l - 0.5).abs() < 0.02),
            "{:?}",
            out.levels
        );
        assert!(out.levels.iter().all(|l| (0.0..=1.0).contains(l)));
    }

    #[test]
    fn levels_come_at_most_10_times_per_second_whatever_the_block_size() {
        let mut tl = mic();
        let mut out = Out::default();
        feed(&mut tl, &mut out, 0, 3_000, 1, 0.5); // 1 ms blocks: 3000 pushes
        assert!(out.levels.len() <= 30, "{} levels in 3 s", out.levels.len());
        assert_eq!(out.levels.len() as u64, tl.written() / 1_600);
    }

    #[test]
    fn a_lost_track_reads_level_zero() {
        let mut tl = mic();
        let mut out = Out::default();
        out.add(tl.lost(LossCause::Other, ms(0)));
        let step = tl.advance_to(ms(1_000));
        assert!(step.levels.len() >= 9, "{}", step.levels.len());
        assert!(step.levels.iter().all(|l| *l == 0.0));
    }

    // ---- OpenError (design point 8) --------------------------------------------------------

    #[test]
    fn an_open_error_maps_one_to_one_onto_the_refused_reason() {
        assert_eq!(
            RefusedReason::from(OpenError::NoDevice),
            RefusedReason::NoDevice
        );
        assert_eq!(
            RefusedReason::from(OpenError::AccessDenied),
            RefusedReason::AccessDenied
        );
        assert_eq!(RefusedReason::from(OpenError::Busy), RefusedReason::Busy);
        assert_eq!(RefusedReason::from(OpenError::Other), RefusedReason::Other);
    }
}
