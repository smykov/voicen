//! Energy speech detector: the fallback when Silero is unavailable, and the only
//! detector on Linux (spec 001 R-5, extended by T-041 with a clamped noise floor
//! and a minimum run of loud frames).
//!
//! RED SKELETON (T-041 test-writer): `detect`, `analyse` and the trait impl are
//! stubs that return wrong values on purpose. Pinned rule (T-041 Investigation):
//! 30 ms frames (480 samples, trailing partial frame dropped); RMS level in dBFS
//! (all-zero frame = -inf); floor = 10th percentile of frame levels clamped to
//! [-70, -40] dBFS; loud = level >= floor + 12 dB; only runs of >= 3 loud frames
//! count; speech = counted frames >= 10 (300 ms).

use super::{SpeechDetector, VadError};
use crate::audio::AudioBuffer;

/// The energy detector. Infallible.
#[derive(Debug, Clone, Copy, Default)]
pub struct EnergyDetector {
    _private: (),
}

impl EnergyDetector {
    pub fn new() -> EnergyDetector {
        EnergyDetector { _private: () }
    }

    /// True when the recording contains speech by the energy rule.
    pub fn detect(&self, audio: &AudioBuffer) -> bool {
        // Stub (T-041 red): lets everything through.
        let _ = analyse(audio.samples());
        true
    }
}

impl SpeechDetector for EnergyDetector {
    fn name(&self) -> &'static str {
        // Stub (T-041 red).
        ""
    }

    fn contains_speech(&self, _audio: &AudioBuffer) -> Result<bool, VadError> {
        // Stub (T-041 red): the energy detector never fails.
        Err(VadError::Failed)
    }
}

/// What the energy rule sees in a recording. Tests assert fixture premises with it
/// (P-005), so the fields beyond `counted` may be read by tests only.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct EnergyAnalysis {
    /// The noise floor after clamping, dBFS.
    pub(crate) floor_db: f64,
    /// Frames at least 12 dB above the floor.
    pub(crate) loud: usize,
    /// The longest run of consecutive loud frames.
    pub(crate) longest_run: usize,
    /// Loud frames in runs of 3 or more.
    pub(crate) counted: usize,
}

/// The energy rule's view of 16 kHz mono samples. Never panics.
pub(crate) fn analyse(samples: &[i16]) -> EnergyAnalysis {
    // Stub (T-041 red).
    let _ = samples;
    EnergyAnalysis {
        floor_db: 0.0,
        loud: 0,
        longest_run: 0,
        counted: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fixtures::{self, noise_floor, quantize, Lcg};

    /// 30 ms at 16 kHz.
    const FRAME: usize = 480;
    /// 500 Hz: exactly 15 periods per frame, so each tone frame has the same RMS.
    const TONE_HZ: f64 = 500.0;

    fn energy(audio: &AudioBuffer) -> bool {
        EnergyDetector::new().detect(audio)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// Frame-aligned segments over a continuous -60 dBFS noise floor: `(frames,
    /// None)` is floor only, `(frames, Some(rms_dbfs))` adds a 500 Hz sine with that
    /// RMS level.
    fn segments(seed: u64, parts: &[(usize, Option<f64>)]) -> Vec<f64> {
        let total: usize = parts.iter().map(|(f, _)| f * FRAME).sum();
        let mut rng = Lcg::new(seed);
        let mut out = noise_floor(&mut rng, total, -60.0);
        let mut pos = 0;
        for &(frames, tone) in parts {
            let len = frames * FRAME;
            if let Some(rms_dbfs) = tone {
                let a = fixtures::db_to_amplitude(rms_dbfs) * 2f64.sqrt();
                for (i, v) in out.iter_mut().skip(pos).take(len).enumerate() {
                    let t = i as f64 / f64::from(crate::audio::SAMPLE_RATE);
                    *v += a * (2.0 * std::f64::consts::PI * TONE_HZ * t).sin();
                }
            }
            pos += len;
        }
        out
    }

    fn buffer(seed: u64, parts: &[(usize, Option<f64>)]) -> AudioBuffer {
        AudioBuffer::from_16k_mono(quantize(&segments(seed, parts)))
    }

    const BURST: Option<f64> = Some(-20.0);

    // --- Fixtures (premise first, then the verdict) ---------------------------------

    #[test]
    fn speech_3s_is_speech() {
        // Positive control. Bite: an always-false detector.
        let audio = fixtures::speech_3s();
        let a = analyse(audio.samples());
        assert!(
            a.counted >= 20 && a.longest_run >= 3,
            "premise: well above the 10-frame minimum in counted runs, got {a:?}"
        );
        assert!((-70.0..=-40.0).contains(&a.floor_db), "premise: {a:?}");
        assert!(energy(&audio));
    }

    #[test]
    fn silence_at_minus_60_dbfs_is_not_speech() {
        // The threshold is relative to the floor, not "any nonzero sample".
        let audio = fixtures::silence_3s();
        let a = analyse(audio.samples());
        assert!(
            (-61.5..=-59.0).contains(&a.floor_db) && a.loud == 0,
            "premise: floor at the noise level, nothing loud, got {a:?}"
        );
        assert!(!energy(&audio));
    }

    #[test]
    fn digital_zero_is_not_speech_and_floor_clamps_to_minus_70() {
        // All-zero frames are -inf dBFS: without the lower clamp the threshold is
        // -inf + 12 = -inf and every frame is "loud"; NaN or a panic are the other
        // ways to get this wrong.
        let audio = fixtures::digital_zero_3s();
        let a = analyse(audio.samples());
        assert!(
            close(a.floor_db, -70.0) && a.loud == 0,
            "premise: floor clamped to -70, nothing loud, got {a:?}"
        );
        assert!(!energy(&audio));
    }

    #[test]
    fn one_second_cough_burst_is_not_speech() {
        // The burst is one counted run (the minimum run alone would let it through);
        // only the 10-frame total rejects it. Bite: dropping the total minimum.
        let audio = fixtures::cough_1s();
        let a = analyse(audio.samples());
        assert!(
            a.longest_run >= 3 && a.counted > 0 && a.counted < 10,
            "premise: one counted run, below the total, got {a:?}"
        );
        assert!(!energy(&audio));
    }

    #[test]
    fn keyboard_clicks_are_not_speech() {
        // R-5's total rule alone says speech here (>= 10 loud frames); no click spans
        // more than 2 frames. Bite: dropping the 3-frame minimum run.
        let audio = fixtures::keyboard_3s();
        let a = analyse(audio.samples());
        assert!(
            a.loud >= 10 && a.longest_run <= 2,
            "premise: many loud frames, all in runs of <= 2, got {a:?}"
        );
        assert!(!energy(&audio));
    }

    #[test]
    fn continuous_speech_without_pauses_is_speech() {
        // The quietest tenth of the frames is speech itself; the floor is capped at
        // -40 dBFS. Bite: dropping the upper clamp (floor ~ -26, nothing loud).
        let audio = fixtures::continuous_speech_1s();
        let a = analyse(audio.samples());
        assert!(
            close(a.floor_db, -40.0) && a.counted >= 10,
            "premise: floor at the -40 cap, enough counted frames, got {a:?}"
        );
        assert!(energy(&audio));
    }

    #[test]
    fn speech_shorter_than_300ms_is_not_speech() {
        let audio = fixtures::speech_250ms();
        let a = analyse(audio.samples());
        assert!(
            a.longest_run >= 3 && a.counted < 10,
            "premise: a counted run below the total, got {a:?}"
        );
        assert!(!energy(&audio));
    }

    // --- The pinned constants ------------------------------------------------------

    #[test]
    fn run_of_three_frames_counts_run_of_two_does_not() {
        // Bite: a minimum run of 2 (B passes) or 4 (A fails).
        let mut a_parts = vec![(30, None)];
        let mut b_parts = vec![(30, None)];
        for _ in 0..4 {
            a_parts.extend([(3, BURST), (5, None)]);
        }
        for _ in 0..8 {
            b_parts.extend([(2, BURST), (5, None)]);
        }
        let a = buffer(11, &a_parts);
        let b = buffer(12, &b_parts);
        let got_a = analyse(a.samples());
        let got_b = analyse(b.samples());
        assert_eq!(
            (got_a.loud, got_a.longest_run, got_a.counted),
            (12, 3, 12),
            "{got_a:?}"
        );
        assert_eq!(
            (got_b.loud, got_b.longest_run, got_b.counted),
            (16, 2, 0),
            "{got_b:?}"
        );
        assert!(energy(&a));
        assert!(!energy(&b));
    }

    #[test]
    fn ten_counted_frames_are_speech_nine_are_not() {
        // The total is over all counted runs, not the longest. Bite: a total of 9
        // or 11, or "longest run >= 10".
        let ten = buffer(13, &[(30, None), (10, BURST), (30, None)]);
        let nine = buffer(14, &[(30, None), (9, BURST), (30, None)]);
        let five_and_five = buffer(
            15,
            &[(30, None), (5, BURST), (4, None), (5, BURST), (30, None)],
        );
        assert_eq!(analyse(ten.samples()).counted, 10);
        assert_eq!(analyse(nine.samples()).counted, 9);
        assert_eq!(analyse(five_and_five.samples()).counted, 10);
        assert!(energy(&ten));
        assert!(!energy(&nine));
        assert!(energy(&five_and_five));
    }

    #[test]
    fn loud_means_twelve_db_above_the_floor() {
        // Tone frames about 14 dB above a -60 dBFS floor are loud, about 10 dB are
        // not. Bite: a margin of 10 or 14 dB.
        let above = buffer(16, &[(60, None), (20, Some(-46.0)), (20, None)]);
        let below = buffer(17, &[(60, None), (20, Some(-50.5)), (20, None)]);
        let a = analyse(above.samples());
        let b = analyse(below.samples());
        assert!((-61.0..=-59.5).contains(&a.floor_db), "premise: {a:?}");
        assert!((-61.0..=-59.5).contains(&b.floor_db), "premise: {b:?}");
        assert_eq!(a.loud, 20, "{a:?}");
        assert_eq!(b.loud, 0, "{b:?}");
        assert!(energy(&above));
        assert!(!energy(&below));
    }

    #[test]
    fn noise_floor_is_the_tenth_percentile_of_frame_levels() {
        // 15 % pause, 85 % steady tone at -45 dBFS: the 10th percentile is the
        // pause (~ -60), a median floor would be the tone itself (nothing loud).
        let audio = buffer(18, &[(15, None), (85, Some(-45.0))]);
        let a = analyse(audio.samples());
        assert!((-61.5..=-59.0).contains(&a.floor_db), "{a:?}");
        assert_eq!(a.loud, 85, "{a:?}");
        assert!(energy(&audio));
    }

    #[test]
    fn trailing_partial_frame_is_dropped() {
        // 9 full loud frames + 479 loud samples: a padded or short last frame would
        // make it 10 counted frames.
        let mut s = segments(19, &[(34, None), (9, BURST)]);
        let a = fixtures::db_to_amplitude(-20.0) * 2f64.sqrt();
        s.extend((0..FRAME - 1).map(|i| {
            let t = i as f64 / f64::from(crate::audio::SAMPLE_RATE);
            a * (2.0 * std::f64::consts::PI * TONE_HZ * t).sin()
        }));
        let audio = AudioBuffer::from_16k_mono(quantize(&s));
        let got = analyse(audio.samples());
        assert_eq!((got.loud, got.counted), (9, 9), "{got:?}");
        assert!(!energy(&audio));
    }

    #[test]
    fn full_scale_samples_do_not_overflow() {
        // 15 frames of i16::MIN (0 dBFS): squares summed in i32, or i16::abs, overflow
        // (a panic in a debug build, a wrong level in release).
        let mut rng = Lcg::new(20);
        let mut samples = quantize(&noise_floor(&mut rng, 40 * FRAME, -60.0));
        samples.extend(std::iter::repeat_n(i16::MIN, 15 * FRAME));
        let audio = AudioBuffer::from_16k_mono(samples);
        let got = analyse(audio.samples());
        assert_eq!((got.loud, got.counted), (15, 15), "{got:?}");
        assert!(energy(&audio));
    }

    #[test]
    fn empty_and_sub_frame_buffers_are_not_speech() {
        // No full frame, so nothing to count; must not panic on an empty level list.
        let empty = AudioBuffer::from_16k_mono(vec![]);
        let short = AudioBuffer::from_16k_mono(vec![i16::MAX; FRAME - 1]);
        for (label, audio) in [("empty", &empty), ("479 samples", &short)] {
            let a = analyse(audio.samples());
            assert_eq!((a.loud, a.counted), (0, 0), "{label}: {a:?}");
            assert!(!energy(audio), "{label}");
        }
    }

    #[test]
    fn trait_impl_is_named_energy_and_never_fails() {
        // The gate reports this name in the SpeechGate event; contains_speech is
        // detect() wrapped in Ok.
        let det = EnergyDetector::new();
        assert_eq!(SpeechDetector::name(&det), "energy");
        assert_eq!(det.contains_speech(&fixtures::speech_3s()), Ok(true));
        assert_eq!(det.contains_speech(&fixtures::silence_3s()), Ok(false));
        assert_eq!(det.contains_speech(&fixtures::digital_zero_3s()), Ok(false));
    }
}
