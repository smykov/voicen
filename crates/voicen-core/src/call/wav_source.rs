//! A call-track source that plays a WAV file (spec 007 R22, FR-014; T-124).
//!
//! Test tooling: compiled for tests and the `test-fakes` feature only. No thread and no wall
//! clock: [`WavTrackSource::pump`] pushes the blocks due by a given stamp on the caller's
//! thread, so a test runs an hour of audio as fast as the timeline can take it.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::capture::{CaptureStamp, LossCause, OpenError, TrackDevice, TrackSink, TrackSource};
use super::events::{EndpointId, Track};
use crate::platform::CaptureHandle;
use crate::recording::CaptureError;

/// The file is not a WAV this reader accepts (16-bit PCM or 32-bit IEEE float RIFF).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a readable 16-bit PCM or f32 WAV file")]
pub struct WavError;

/// A scripted irregularity of the feed; times are on the source's own timeline, in ms
/// after its start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WavGap {
    /// No blocks for `len_ms` from `at_ms`; the next block carries a stamp `len_ms` later
    /// and the file's audio continues where it was (the stalled time is simply missing).
    Stall { at_ms: u64, len_ms: u64 },
    /// At `at_ms` the sink gets `lost(track, cause, ..)`; no block follows.
    Lost { at_ms: u64, cause: LossCause },
}

impl WavGap {
    fn at_ms(&self) -> u64 {
        match *self {
            WavGap::Stall { at_ms, .. } | WavGap::Lost { at_ms, .. } => at_ms,
        }
    }
}

/// The source's progress.
struct State {
    sink: Option<(Track, Arc<dyn TrackSink>)>,
    /// Index of the next block (its stamp slot).
    next: u64,
    /// The next file frame to play.
    frame: usize,
    /// Stamp shift accumulated by stalls.
    shift_ns: u64,
    /// Scripted gaps already handled (they are sorted by time).
    fired: usize,
    lost: bool,
    now: CaptureStamp,
}

/// What one pump iteration hands to the sink, outside the lock.
enum Action {
    Block(Track, Arc<dyn TrackSink>, Vec<f32>, CaptureStamp),
    Lost(Track, Arc<dyn TrackSink>, LossCause, CaptureStamp),
}

/// Plays a WAV file as a call track. No thread and no wall clock: `pump` pushes the blocks
/// whose synthetic stamp is not after `until`, on the caller's thread.
pub struct WavTrackSource {
    /// Interleaved samples in `[-1, 1]`.
    samples: Vec<f32>,
    rate: u32,
    channels: u16,
    block_ms: u64,
    drift: f64,
    start: CaptureStamp,
    gaps: Vec<WavGap>,
    state: Mutex<State>,
}

impl WavTrackSource {
    pub fn from_bytes(bytes: &[u8]) -> Result<WavTrackSource, WavError> {
        let (samples, rate, channels) = read_wav(bytes)?;
        Ok(WavTrackSource {
            samples,
            rate,
            channels,
            block_ms: 10,
            drift: 0.0,
            start: CaptureStamp(0),
            gaps: Vec::new(),
            state: Mutex::new(State {
                sink: None,
                next: 0,
                frame: 0,
                shift_ns: 0,
                fired: 0,
                lost: false,
                now: CaptureStamp(0),
            }),
        })
    }

    /// Block length in ms (default 10; at least 1).
    pub fn with_block_ms(mut self, ms: u64) -> WavTrackSource {
        self.block_ms = ms.max(1);
        self
    }

    /// Source clock error: stamp_k = start + k * N / (rate * (1 + d)).
    pub fn with_drift(mut self, d: f64) -> WavTrackSource {
        self.drift = d;
        self
    }

    /// The stamp of block 0 (default 0); `now()` starts there.
    pub fn with_start(mut self, start: CaptureStamp) -> WavTrackSource {
        self.start = start;
        self.state_mut().now = start;
        self
    }

    pub fn with_gaps(mut self, mut gaps: Vec<WavGap>) -> WavTrackSource {
        gaps.sort_by_key(WavGap::at_ms);
        self.gaps = gaps;
        self
    }

    /// Push every not yet pushed block with stamp <= `until`; also moves `now()` to `until`.
    pub fn pump(&self, until: CaptureStamp) {
        while let Some(action) = self.next_action(until) {
            match action {
                Action::Block(track, sink, data, at) => {
                    sink.block(track, &data, self.rate, self.channels, at)
                }
                Action::Lost(track, sink, cause, at) => sink.lost(track, cause, at),
            }
        }
        let mut st = self.state();
        st.now = st.now.max(until);
    }

    /// The next thing due by `until`, with the state advanced past it.
    fn next_action(&self, until: CaptureStamp) -> Option<Action> {
        let mut st = self.state();
        if st.lost {
            return None;
        }
        let (track, sink) = st.sink.clone()?;
        loop {
            let at = self.stamp(&st);
            let rel_ns = at.0.saturating_sub(self.start.0);
            match self.gaps.get(st.fired) {
                Some(&WavGap::Stall { at_ms, len_ms }) if rel_ns >= at_ms * 1_000_000 => {
                    st.shift_ns += len_ms * 1_000_000;
                    st.fired += 1;
                    continue;
                }
                Some(&WavGap::Lost { at_ms, cause }) if rel_ns >= at_ms * 1_000_000 => {
                    let lost_at = CaptureStamp(self.start.0 + at_ms * 1_000_000);
                    if lost_at > until {
                        return None;
                    }
                    st.fired += 1;
                    st.lost = true;
                    return Some(Action::Lost(track, sink, cause, lost_at));
                }
                _ => {}
            }
            if at > until {
                return None;
            }
            let data = self.block_at(st.frame);
            st.frame += self.block_frames();
            st.next += 1;
            return Some(Action::Block(track, sink, data, at));
        }
    }

    fn block_frames(&self) -> usize {
        let frames = u64::from(self.rate) * self.block_ms / 1_000;
        usize::try_from(frames).unwrap_or(usize::MAX).max(1)
    }

    /// One block from file frame `frame`; past the end of the file it is silence.
    fn block_at(&self, frame: usize) -> Vec<f32> {
        let channels = usize::from(self.channels);
        let len = self.block_frames() * channels;
        let from = frame.saturating_mul(channels).min(self.samples.len());
        let to = from.saturating_add(len).min(self.samples.len());
        let mut data = self.samples[from..to].to_vec();
        data.resize(len, 0.0);
        data
    }

    /// The stamp of the next block: start + stall shift + k * N / (rate * (1 + d)).
    fn stamp(&self, st: &State) -> CaptureStamp {
        let per_block_ns = self.block_frames() as f64 * 1e9
            / (f64::from(self.rate) * (1.0 + self.drift).max(1e-9));
        let offset = (st.next as f64 * per_block_ns).round() as u64;
        CaptureStamp(self.start.0 + st.shift_ns + offset)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn state_mut(&mut self) -> &mut State {
        self.state.get_mut().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The handle of an open WAV track: nothing runs, so stopping always succeeds.
struct WavHandle;

impl CaptureHandle for WavHandle {
    fn stop(self: Box<Self>) -> Result<(), CaptureError> {
        Ok(())
    }
}

impl TrackSource for WavTrackSource {
    fn open(
        &self,
        track: Track,
        _device: TrackDevice,
        sink: Arc<dyn TrackSink>,
    ) -> Result<(Box<dyn CaptureHandle>, EndpointId), OpenError> {
        self.state().sink = Some((track, sink));
        let endpoint = match track {
            Track::Mic => "{0.0.1.00000000}.{00000000-0000-0000-0000-000000000001}",
            Track::System => "{0.0.0.00000000}.{00000000-0000-0000-0000-000000000002}",
        };
        Ok((Box::new(WavHandle), EndpointId::new(endpoint)))
    }

    fn now(&self) -> CaptureStamp {
        self.state().now
    }
}

/// The `fmt ` fields the reader uses.
#[derive(Clone, Copy)]
struct Format {
    float: bool,
    channels: u16,
    rate: u32,
}

/// Interleaved `f32` samples, rate and channels of a 16-bit PCM or 32-bit float RIFF WAV.
/// Chunks other than `fmt ` and `data` are skipped; anything malformed is a [`WavError`].
fn read_wav(bytes: &[u8]) -> Result<(Vec<f32>, u32, u16), WavError> {
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err(WavError);
    }
    let mut pos = 12usize;
    let mut format = None;
    loop {
        let header = bytes
            .get(pos..pos.checked_add(8).ok_or(WavError)?)
            .ok_or(WavError)?;
        let size = usize::try_from(le_u32(&header[4..8])).map_err(|_| WavError)?;
        let body_start = pos + 8;
        let body_end = body_start.checked_add(size).ok_or(WavError)?;
        let body = bytes.get(body_start..body_end).ok_or(WavError)?;
        match &header[..4] {
            b"fmt " => format = Some(read_format(body)?),
            b"data" => {
                let format = format.ok_or(WavError)?;
                return Ok((decode(format, body)?, format.rate, format.channels));
            }
            _ => {}
        }
        pos = body_end.checked_add(size & 1).ok_or(WavError)?;
    }
}

fn read_format(body: &[u8]) -> Result<Format, WavError> {
    if body.len() < 16 {
        return Err(WavError);
    }
    let tag = le_u16(&body[0..2]);
    let channels = le_u16(&body[2..4]);
    let rate = le_u32(&body[4..8]);
    let block_align = le_u16(&body[12..14]);
    let bits = le_u16(&body[14..16]);
    let float = match (tag, bits) {
        (1, 16) => false,
        (3, 32) => true,
        _ => return Err(WavError),
    };
    let frame_bytes = u32::from(channels) * u32::from(bits / 8);
    if channels == 0 || rate == 0 || u32::from(block_align) != frame_bytes {
        return Err(WavError);
    }
    Ok(Format {
        float,
        channels,
        rate,
    })
}

fn decode(format: Format, data: &[u8]) -> Result<Vec<f32>, WavError> {
    let sample_bytes = if format.float { 4 } else { 2 };
    let frame_bytes = sample_bytes * usize::from(format.channels);
    if !data.len().is_multiple_of(frame_bytes) {
        return Err(WavError);
    }
    Ok(if format.float {
        data.as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect()
    } else {
        data.as_chunks::<2>()
            .0
            .iter()
            .map(|b| f32::from(i16::from_le_bytes(*b)) / 32_768.0)
            .collect()
    })
}

fn le_u16(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

fn le_u32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, PoisonError};

    use super::*;

    fn ms(n: u64) -> CaptureStamp {
        CaptureStamp::from_ms(n)
    }

    // ---- WAV builders -------------------------------------------------------------------

    fn wav(tag: u16, bits: u16, channels: u16, rate: u32, data: &[u8]) -> Vec<u8> {
        let block = channels.wrapping_mul(bits / 8);
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&tag.to_le_bytes());
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * u32::from(block)).to_le_bytes());
        v.extend_from_slice(&block.to_le_bytes());
        v.extend_from_slice(&bits.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(data);
        v
    }

    fn pcm16(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        wav(1, 16, channels, rate, &data)
    }

    fn f32wav(rate: u32, channels: u16, samples: &[f32]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        wav(3, 32, channels, rate, &data)
    }

    // ---- a recording sink ---------------------------------------------------------------

    #[derive(Debug, Clone, PartialEq)]
    struct Got {
        track: Track,
        samples: Vec<f32>,
        rate: u32,
        channels: u16,
        at: CaptureStamp,
    }

    #[derive(Default)]
    struct Rec {
        blocks: Mutex<Vec<Got>>,
        losses: Mutex<Vec<(Track, LossCause, CaptureStamp)>>,
    }

    impl Rec {
        fn blocks(&self) -> Vec<Got> {
            self.blocks
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
        fn losses(&self) -> Vec<(Track, LossCause, CaptureStamp)> {
            self.losses
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
        fn stamps(&self) -> Vec<u64> {
            self.blocks().iter().map(|b| b.at.ns()).collect()
        }
    }

    impl TrackSink for Rec {
        fn block(
            &self,
            track: Track,
            interleaved: &[f32],
            rate: u32,
            channels: u16,
            at: CaptureStamp,
        ) {
            self.blocks
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Got {
                    track,
                    samples: interleaved.to_vec(),
                    rate,
                    channels,
                    at,
                });
        }
        fn lost(&self, track: Track, cause: LossCause, at: CaptureStamp) {
            self.losses
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((track, cause, at));
        }
    }

    fn opened(src: &WavTrackSource, track: Track) -> Arc<Rec> {
        let rec = Arc::new(Rec::default());
        src.open(track, TrackDevice::DefaultInput, rec.clone())
            .map(|_| ())
            .unwrap_or_else(|e| panic!("open refused: {e:?}"));
        rec
    }

    fn ramp(len: usize) -> Vec<i16> {
        (0..len).map(|i| ((i % 300) as i16 - 150) * 100).collect()
    }

    // ---- reading ------------------------------------------------------------------------

    #[test]
    fn a_pcm16_file_is_read_and_scaled_to_f32() {
        let samples = ramp(320);
        let src = WavTrackSource::from_bytes(&pcm16(16_000, 1, &samples)).unwrap();
        let rec = opened(&src, Track::Mic);
        src.pump(ms(10));
        let blocks = rec.blocks();
        assert_eq!(blocks.len(), 2, "blocks at 0 and 10 ms");
        let got: Vec<f32> = blocks.iter().flat_map(|b| b.samples.clone()).collect();
        assert_eq!(got.len(), 320);
        for (g, s) in got.iter().zip(&samples) {
            assert!((g - f32::from(*s) / 32_768.0).abs() < 1e-4, "{g} vs {s}");
        }
        assert!(blocks
            .iter()
            .all(|b| b.rate == 16_000 && b.channels == 1 && b.track == Track::Mic));
    }

    #[test]
    fn a_stereo_48k_pcm16_file_keeps_its_rate_channels_and_frame_blocks() {
        let samples = ramp(960 * 2);
        let src = WavTrackSource::from_bytes(&pcm16(48_000, 2, &samples)).unwrap();
        let rec = opened(&src, Track::System);
        src.pump(ms(10));
        let blocks = rec.blocks();
        assert_eq!(blocks.len(), 2);
        assert!(blocks
            .iter()
            .all(|b| b.rate == 48_000 && b.channels == 2 && b.samples.len() == 960));
        assert_eq!(blocks[0].track, Track::System);
        assert!((blocks[1].samples[0] - f32::from(samples[960]) / 32_768.0).abs() < 1e-4);
    }

    #[test]
    fn an_f32_file_is_read_exactly() {
        let samples: Vec<f32> = (0..320).map(|i| (i as f32 - 160.0) / 200.0).collect();
        let src = WavTrackSource::from_bytes(&f32wav(16_000, 1, &samples)).unwrap();
        let rec = opened(&src, Track::Mic);
        src.pump(ms(10));
        let got: Vec<f32> = rec
            .blocks()
            .iter()
            .flat_map(|b| b.samples.clone())
            .collect();
        assert_eq!(got, samples);
    }

    #[test]
    fn other_formats_are_refused_with_a_wav_error_never_a_panic() {
        let some = [0u8; 64];
        let bad: Vec<(&str, Vec<u8>)> = vec![
            ("8-bit pcm", wav(1, 8, 1, 16_000, &some)),
            ("24-bit pcm", wav(1, 24, 1, 16_000, &some)),
            ("32-bit int pcm", wav(1, 32, 1, 16_000, &some)),
            ("64-bit float", wav(3, 64, 1, 16_000, &some)),
            ("16-bit float tag", wav(3, 16, 1, 16_000, &some)),
            ("mp3 tag", wav(0x55, 16, 1, 16_000, &some)),
            ("extensible", wav(0xFFFE, 16, 1, 16_000, &some)),
            ("zero channels", wav(1, 16, 0, 16_000, &some)),
            ("zero rate", wav(1, 16, 1, 0, &some)),
            ("ragged frames", wav(1, 16, 2, 16_000, &some[..62])),
            (
                "not riff",
                b"OggS this is not a wav file at all, but it is long enough".to_vec(),
            ),
            ("empty", Vec::new()),
        ];
        for (name, bytes) in bad {
            assert_eq!(
                WavTrackSource::from_bytes(&bytes).err(),
                Some(WavError),
                "{name}"
            );
        }
    }

    #[test]
    fn a_wav_without_data_or_with_a_data_chunk_past_the_end_is_refused() {
        let mut no_data = pcm16(16_000, 1, &[1, 2, 3, 4]);
        no_data.truncate(36);
        assert!(WavTrackSource::from_bytes(&no_data).is_err());
        let mut past = pcm16(16_000, 1, &[1, 2, 3, 4]);
        let len = past.len();
        past[len - 12..len - 8].copy_from_slice(b"data");
        past[len - 8..len - 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(WavTrackSource::from_bytes(&past).is_err());
    }

    #[test]
    fn a_file_cut_at_any_length_is_an_error_not_a_panic() {
        for full in [pcm16(16_000, 1, &ramp(64)), f32wav(16_000, 1, &[0.5; 64])] {
            for cut in 0..full.len() {
                assert!(
                    WavTrackSource::from_bytes(&full[..cut]).is_err(),
                    "cut at {cut} of {}",
                    full.len()
                );
            }
        }
    }

    #[test]
    fn unknown_chunks_before_data_are_skipped() {
        let base = pcm16(16_000, 1, &ramp(160));
        let mut v = base[..36].to_vec();
        v.extend_from_slice(b"LIST");
        v.extend_from_slice(&3u32.to_le_bytes());
        v.extend_from_slice(&[1, 2, 3, 0]); // odd length, padded to even
        v.extend_from_slice(&base[36..]);
        let src = WavTrackSource::from_bytes(&v).unwrap();
        let rec = opened(&src, Track::Mic);
        src.pump(ms(0));
        assert_eq!(rec.blocks().len(), 1);
        assert_eq!(rec.blocks()[0].samples.len(), 160);
    }

    // ---- stamps, pump, drift, gaps, silence ---------------------------------------------------

    #[test]
    fn pump_pushes_each_block_once_with_synthetic_stamps_and_moves_now() {
        let src = WavTrackSource::from_bytes(&pcm16(16_000, 1, &ramp(1_600)))
            .unwrap()
            .with_block_ms(100)
            .with_start(ms(7_000));
        let rec = opened(&src, Track::Mic);
        src.pump(ms(7_250));
        assert_eq!(
            rec.stamps(),
            vec![7_000_000_000, 7_100_000_000, 7_200_000_000]
        );
        assert_eq!(src.now(), ms(7_250));
        src.pump(ms(7_450));
        assert_eq!(rec.stamps().len(), 5, "no block is pushed twice");
        src.pump(ms(7_450));
        assert_eq!(rec.stamps().len(), 5);
        assert!(rec.blocks().iter().all(|b| b.samples.len() == 1_600));
    }

    #[test]
    fn open_returns_a_handle_that_stops_cleanly_and_an_endpoint() {
        let src = WavTrackSource::from_bytes(&pcm16(16_000, 1, &ramp(160))).unwrap();
        let (handle, _endpoint) = src
            .open(
                Track::Mic,
                TrackDevice::DefaultOutputLoopback,
                Arc::new(Rec::default()),
            )
            .unwrap();
        assert!(handle.stop().is_ok());
    }

    #[test]
    fn a_drift_factor_scales_the_stamps() {
        let d = 1e-3;
        let src = WavTrackSource::from_bytes(&pcm16(16_000, 1, &ramp(160)))
            .unwrap()
            .with_drift(d)
            .with_start(ms(100));
        let rec = opened(&src, Track::Mic);
        src.pump(ms(1_100));
        let stamps = rec.stamps();
        assert_eq!(stamps.len(), 101, "blocks 0..=100 are stamped within 1 s");
        for (k, s) in stamps.iter().enumerate() {
            let want = 100_000_000.0 + (k as f64) * 10_000_000.0 / (1.0 + d);
            assert!((*s as f64 - want).abs() <= 2.0, "block {k}: {s} vs {want}");
        }
    }

    #[test]
    fn a_stall_leaves_a_hole_in_the_stamps_and_the_audio_continues() {
        let samples = ramp(3_200);
        let src = WavTrackSource::from_bytes(&pcm16(16_000, 1, &samples))
            .unwrap()
            .with_gaps(vec![WavGap::Stall {
                at_ms: 100,
                len_ms: 300,
            }]);
        let rec = opened(&src, Track::Mic);
        src.pump(ms(600));
        let want: Vec<u64> = (0..10).chain(40..61).map(|k| k * 10_000_000).collect();
        assert_eq!(rec.stamps(), want);
        // The stalled time is missing, not replayed: block 10 carries the file's next audio.
        let after = &rec.blocks()[10];
        assert!((after.samples[0] - f32::from(samples[1_600]) / 32_768.0).abs() < 1e-4);
    }

    #[test]
    fn a_scripted_loss_calls_lost_once_and_no_block_follows() {
        let src = WavTrackSource::from_bytes(&pcm16(16_000, 1, &ramp(3_200)))
            .unwrap()
            .with_gaps(vec![WavGap::Lost {
                at_ms: 50,
                cause: LossCause::NotAvailable,
            }]);
        let rec = opened(&src, Track::System);
        src.pump(ms(200));
        assert_eq!(
            rec.stamps(),
            vec![0, 10_000_000, 20_000_000, 30_000_000, 40_000_000]
        );
        assert_eq!(
            rec.losses(),
            vec![(Track::System, LossCause::NotAvailable, ms(50))]
        );
        src.pump(ms(400));
        assert_eq!(rec.stamps().len(), 5);
        assert_eq!(rec.losses().len(), 1);
    }

    #[test]
    fn past_the_end_of_the_file_the_source_pushes_silence_like_a_live_device() {
        let src = WavTrackSource::from_bytes(&f32wav(16_000, 1, &[0.5; 1_600])).unwrap();
        let rec = opened(&src, Track::Mic);
        src.pump(ms(300));
        let blocks = rec.blocks();
        assert_eq!(blocks.len(), 31);
        assert!(blocks[..10]
            .iter()
            .all(|b| b.samples.iter().all(|x| *x == 0.5)));
        assert!(blocks[10..]
            .iter()
            .all(|b| b.samples.len() == 160 && b.samples.iter().all(|x| *x == 0.0)));
    }
}
