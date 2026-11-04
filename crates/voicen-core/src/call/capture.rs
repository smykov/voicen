//! Call-track capture ports and the per-track timeline (spec 007 R2, R3, R21; T-124).
//!
//! RED-TESTS SKELETON (T-124): only the signatures the tests need. Every behaviour is
//! `todo!()` until T-124's implementation (spec task T021) replaces this file's non-test part.

use std::sync::Arc;

use super::events::{EndpointId, RefusedReason, Track};
use crate::audio::AudioError;
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
    fn from(_: OpenError) -> RefusedReason {
        RefusedReason::Other
    }
}

/// Why a running track was lost (R4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossCause {
    Invalidated,
    NotAvailable,
    Other,
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

/// The bounded callback channel (one queue and one drop counter per track).
#[allow(dead_code)]
pub struct ChannelSink {
    capacity: usize,
}

/// The consumer end of one track's channel.
pub struct TrackReceiver;

impl ChannelSink {
    /// A sink with `capacity` queued events per track, and the mic and system receivers.
    pub fn new(_capacity: usize) -> (ChannelSink, TrackReceiver, TrackReceiver) {
        todo!("T-124")
    }
}

impl TrackSink for ChannelSink {
    fn block(&self, _: Track, _: &[f32], _: u32, _: u16, _: CaptureStamp) {
        todo!("T-124")
    }
    fn lost(&self, _: Track, _: LossCause, _: CaptureStamp) {
        todo!("T-124")
    }
}

impl TrackReceiver {
    /// The next event, or `None` when nothing is pending. A pending drop comes after the
    /// blocks queued before it.
    pub fn try_recv(&self) -> Option<TrackEvent> {
        todo!("T-124")
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

/// One track's 16 kHz axis, placed from capture stamps.
#[allow(dead_code)]
pub struct Timeline {
    track: Track,
    call_start: CaptureStamp,
}

impl Timeline {
    pub fn new(track: Track, call_start: CaptureStamp) -> Timeline {
        Timeline { track, call_start }
    }

    /// Place a block whose first frame was captured at `at`.
    pub fn push(
        &mut self,
        _interleaved: &[f32],
        _rate: u32,
        _channels: u16,
        _at: CaptureStamp,
    ) -> Result<Step, AudioError> {
        todo!("T-124")
    }

    /// Zero-fill exactly `ms` for `blocks` blocks the channel dropped.
    pub fn dropped(&mut self, _blocks: u32, _ms: u32) -> Step {
        todo!("T-124")
    }

    /// Mark the track lost at `at`.
    pub fn lost(&mut self, _cause: LossCause, _at: CaptureStamp) -> Step {
        todo!("T-124")
    }

    /// The other track's stamp, or the session tick: advance this track to `at`.
    pub fn advance_to(&mut self, _at: CaptureStamp) -> Step {
        todo!("T-124")
    }

    /// 16 kHz samples emitted so far.
    pub fn written(&self) -> u64 {
        todo!("T-124")
    }

    /// The slow loop's relative ratio trim, in ppm (positive = output stretched).
    pub fn ratio_adj_ppm(&self) -> f64 {
        todo!("T-124")
    }

    /// How many times the offset exceeded 40 ms and the timeline re-anchored at once.
    pub fn reanchors(&self) -> u32 {
        todo!("T-124")
    }

    pub fn is_lost(&self) -> bool {
        todo!("T-124")
    }
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
        let (wf, ws) = (
            lock(&fast.timeline).written(),
            lock(&slow.timeline).written(),
        );
        assert!(
            wf.abs_diff(ws) <= 320,
            "tracks differ by {} samples",
            wf.abs_diff(ws)
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
