//! A call-track source that plays a WAV file (spec 007 R22, FR-014; T-124).
//!
//! RED-TESTS SKELETON (T-124): only the signatures the tests need; behaviour is `todo!()`.
//! Test tooling: compiled for tests and the `test-fakes` feature only.

use std::sync::Arc;

use super::capture::{CaptureStamp, LossCause, OpenError, TrackDevice, TrackSink, TrackSource};
use super::events::{EndpointId, Track};
use crate::platform::CaptureHandle;

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

/// Plays a WAV file as a call track. No thread and no wall clock: `pump` pushes the blocks
/// whose synthetic stamp is not after `until`, on the caller's thread.
#[allow(dead_code)]
pub struct WavTrackSource {
    start: CaptureStamp,
}

impl WavTrackSource {
    pub fn from_bytes(_bytes: &[u8]) -> Result<WavTrackSource, WavError> {
        todo!("T-124")
    }

    /// Block length in ms (default 10).
    pub fn with_block_ms(self, _ms: u64) -> WavTrackSource {
        todo!("T-124")
    }

    /// Source clock error: stamp_k = start + k * N / (rate * (1 + d)).
    pub fn with_drift(self, _d: f64) -> WavTrackSource {
        todo!("T-124")
    }

    /// The stamp of block 0 (default 0).
    pub fn with_start(self, _start: CaptureStamp) -> WavTrackSource {
        todo!("T-124")
    }

    pub fn with_gaps(self, _gaps: Vec<WavGap>) -> WavTrackSource {
        todo!("T-124")
    }

    /// Push every not yet pushed block with stamp <= `until`; also moves `now()` to `until`.
    pub fn pump(&self, _until: CaptureStamp) {
        todo!("T-124")
    }
}

impl TrackSource for WavTrackSource {
    fn open(
        &self,
        _track: Track,
        _device: TrackDevice,
        _sink: Arc<dyn TrackSink>,
    ) -> Result<(Box<dyn CaptureHandle>, EndpointId), OpenError> {
        todo!("T-124")
    }

    fn now(&self) -> CaptureStamp {
        todo!("T-124")
    }
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
