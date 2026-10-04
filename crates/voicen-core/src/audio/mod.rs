//! Recorded audio as the engines take it: 16 kHz mono 16-bit PCM (spec 001 T009,
//! FR-06, decisions #1).
//!
//! An [`AudioBuffer`] exists only at 16 kHz mono: built from 16 kHz mono samples
//! ([`AudioBuffer::from_16k_mono`]) or from captured interleaved frames at any rate
//! and channel count ([`AudioBuffer::from_frames`]: mix-down, then resampling with
//! rubato in [`resample`]). [`wav::encode`] writes it as a RIFF WAV for upload.
//! Audio is never logged (FR-20, NFR-04), so `Debug` does not print samples.

pub mod resample;
pub mod wav;

/// The one sample rate of an [`AudioBuffer`].
pub const SAMPLE_RATE: u32 = 16_000;

/// 16 kHz mono signed 16-bit samples.
// T-040 skeleton: the derived Debug prints the samples (red: audio must not reach a log).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioBuffer {
    samples: Vec<i16>,
}

/// Captured frames that cannot be converted. Never a panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioError {
    /// Sample rate 0.
    ZeroRate,
    /// Channel count 0.
    ZeroChannels,
    /// The interleaved sample count is not a multiple of the channel count.
    RaggedFrames,
}

impl AudioBuffer {
    /// Samples that are already 16 kHz mono.
    pub fn from_16k_mono(samples: Vec<i16>) -> AudioBuffer {
        AudioBuffer { samples }
    }

    /// Interleaved `f32` frames in `[-1.0, 1.0]` at `rate` Hz with `channels`
    /// channels: channels averaged to mono, resampled to 16 kHz, converted to
    /// `i16` with saturation.
    pub fn from_frames(
        samples: &[f32],
        rate: u32,
        channels: u16,
    ) -> Result<AudioBuffer, AudioError> {
        // T-040 skeleton: wrong on purpose until implemented (red tests first).
        let _ = (samples, rate, channels);
        Err(AudioError::ZeroRate)
    }

    /// The 16 kHz mono samples.
    pub fn samples(&self) -> &[i16] {
        &self.samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AMPLITUDE: f32 = 0.5;

    /// Interleaved frames of a sine of `freq` Hz, `seconds` long, at `rate` Hz;
    /// channel `c` gets `sign[c] * sine`.
    fn tone(freq: f64, seconds: f64, rate: u32, signs: &[f32]) -> Vec<f32> {
        let frames = (seconds * f64::from(rate)).round() as usize;
        let mut out = Vec::with_capacity(frames * signs.len());
        for n in 0..frames {
            let t = n as f64 / f64::from(rate);
            let v = (AMPLITUDE as f64 * (2.0 * std::f64::consts::PI * freq * t).sin()) as f32;
            for sign in signs {
                out.push(sign * v);
            }
        }
        out
    }

    /// Goertzel power of `samples` (16 kHz) at `freq` Hz.
    fn power_at(samples: &[i16], freq: f64) -> f64 {
        let w = 2.0 * std::f64::consts::PI * freq / f64::from(SAMPLE_RATE);
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f64, 0.0f64);
        for &x in samples {
            let s0 = f64::from(x) + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        s1 * s1 + s2 * s2 - coeff * s1 * s2
    }

    fn rms(samples: &[i16]) -> f64 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum: f64 = samples.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
        (sum / samples.len() as f64).sqrt()
    }

    /// The samples without the first and last 50 ms (resampler edge transients).
    fn steady(samples: &[i16]) -> &[i16] {
        let edge = (SAMPLE_RATE / 20) as usize;
        samples
            .get(edge..samples.len().saturating_sub(edge))
            .unwrap_or(&[])
    }

    /// Spec T009: "length ±1 sample per 10 ms" of audio.
    fn assert_length(rate: u32, channels: u16, seconds: f64) {
        let signs = vec![1.0f32; usize::from(channels)];
        let frames = tone(440.0, seconds, rate, &signs);
        let buf = AudioBuffer::from_frames(&frames, rate, channels).expect("valid frames");
        let expected = seconds * f64::from(SAMPLE_RATE);
        let tolerance = (seconds * 100.0).ceil();
        let got = buf.samples().len() as f64;
        assert!(
            (got - expected).abs() <= tolerance,
            "{rate} Hz x{channels}, {seconds} s: {got} samples, expected {expected} ± {tolerance}"
        );
    }

    #[test]
    fn from_frames_48k_stereo_to_16k_mono_length() {
        // Bite: no resampling (3x the length), resampling per interleaved sample
        // instead of per frame (2x), or a dropped tail chunk (short inputs).
        assert_length(48_000, 2, 2.0);
        assert_length(48_000, 2, 0.1);
        assert_length(48_000, 1, 1.0);
    }

    #[test]
    fn from_frames_44k1_stereo_to_16k_mono_length() {
        // Non-integer ratio 160/441. Bite: an integer decimation (44100/16000 -> 2 or 3).
        assert_length(44_100, 2, 2.0);
        assert_length(44_100, 2, 0.1);
        assert_length(44_100, 2, 1.2345);
    }

    #[test]
    fn from_frames_empty_input_is_empty_buffer() {
        // A zero-length capture is not an error and not a panic.
        let buf = AudioBuffer::from_frames(&[], 48_000, 2).expect("empty frames are valid");
        assert!(buf.samples().is_empty());
    }

    #[test]
    fn from_frames_preserves_tone_frequency() {
        // A 440 Hz tone stays at 440 Hz (not 400/480: a wrong ratio shifts it) and
        // keeps its level (RMS of a 0.5 sine = 0.5/sqrt(2) of full scale, ±10%).
        // Bite: silence, a wrong rate ratio, or a lost channel average.
        for rate in [48_000u32, 44_100] {
            let frames = tone(440.0, 1.0, rate, &[1.0, 1.0]);
            let buf = AudioBuffer::from_frames(&frames, rate, 2).expect("valid frames");
            let s = steady(buf.samples());
            let at = power_at(s, 440.0);
            for other in [400.0, 480.0] {
                let off = power_at(s, other);
                assert!(
                    at > 100.0 * off && at > 0.0,
                    "{rate} Hz: power at 440 Hz {at} not > 100x power at {other} Hz {off}"
                );
            }
            let expected = f64::from(AMPLITUDE) / 2f64.sqrt() * 32767.0;
            let got = rms(s);
            assert!(
                (got - expected).abs() <= 0.1 * expected,
                "{rate} Hz: RMS {got}, expected {expected} ± 10%"
            );
        }
    }

    #[test]
    fn from_frames_mixes_down_channels() {
        // 16 kHz stereo: mix-down only, no resampling, so values are checked one
        // by one. L = R keeps the value; L = -R cancels. Bite: taking only the left
        // channel (L = -R not cancelled), summing without averaging (L = R doubled).
        let values: Vec<f32> = (0..1600).map(|i| (i as f32 / 1600.0) - 0.5).collect();
        let same: Vec<f32> = values.iter().flat_map(|&v| [v, v]).collect();
        let buf = AudioBuffer::from_frames(&same, 16_000, 2).expect("valid frames");
        assert_eq!(
            buf.samples().len(),
            values.len(),
            "L = R: one sample per frame"
        );
        for (i, (&got, &v)) in buf.samples().iter().zip(&values).enumerate() {
            let expected = f64::from(v) * 32767.0;
            assert!(
                (f64::from(got) - expected).abs() <= 2.0,
                "L = R frame {i}: {got}, expected {expected}"
            );
        }

        let opposite: Vec<f32> = values.iter().flat_map(|&v| [v, -v]).collect();
        let buf = AudioBuffer::from_frames(&opposite, 16_000, 2).expect("valid frames");
        assert_eq!(
            buf.samples().len(),
            values.len(),
            "L = -R: one sample per frame"
        );
        assert!(
            buf.samples().iter().all(|&x| x.abs() <= 2),
            "L = -R must cancel: {:?}",
            buf.samples().iter().map(|x| x.abs()).max()
        );

        // Also through the resampler: 48 kHz L = -R is near silence.
        let frames = tone(440.0, 1.0, 48_000, &[1.0, -1.0]);
        let buf = AudioBuffer::from_frames(&frames, 48_000, 2).expect("valid frames");
        assert!(
            !buf.samples().is_empty(),
            "48 kHz L = -R: no samples at all"
        );
        assert!(
            rms(buf.samples()) < 0.01 * 32767.0,
            "48 kHz L = -R: RMS {}",
            rms(buf.samples())
        );
    }

    #[test]
    fn from_frames_16k_mono_is_identity() {
        // 16 kHz mono needs no resampling: same count, each value scaled to i16
        // (±2 LSB for 32767/32768 scaling and rounding); out-of-range input
        // saturates instead of wrapping. Bite: a resampler run anyway (edge
        // transients, length change), or `as i16` on an unscaled/unclamped value.
        let values: Vec<f32> = (0..800).map(|i| ((i as f32) * 0.37).sin() * 0.9).collect();
        let buf = AudioBuffer::from_frames(&values, 16_000, 1).expect("valid frames");
        assert_eq!(buf.samples().len(), values.len());
        for (i, (&got, &v)) in buf.samples().iter().zip(&values).enumerate() {
            let expected = f64::from(v) * 32767.0;
            assert!(
                (f64::from(got) - expected).abs() <= 2.0,
                "sample {i}: {got}, expected {expected}"
            );
        }

        let buf =
            AudioBuffer::from_frames(&[2.0, -2.0, 1.0, -1.0], 16_000, 1).expect("valid frames");
        let s = buf.samples();
        assert_eq!(s.len(), 4);
        assert!(
            s.first() == Some(&i16::MAX) && s.get(1).is_some_and(|&x| x <= -32767),
            "out-of-range input must saturate: {s:?}"
        );
        assert!(
            s.get(2).is_some_and(|&x| x >= 32766) && s.get(3).is_some_and(|&x| x <= -32766),
            "full scale: {s:?}"
        );
    }

    #[test]
    fn from_frames_rejects_zero_rate_or_channels_or_ragged_frames() {
        // Err, never a panic (NFR-07: no panic on capture data). Bite: a division
        // by zero / `chunks_exact` remainder silently dropped / rubato built with
        // ratio 0.
        assert_eq!(
            AudioBuffer::from_frames(&[0.0; 4], 0, 1),
            Err(AudioError::ZeroRate)
        );
        assert_eq!(
            AudioBuffer::from_frames(&[0.0; 4], 48_000, 0),
            Err(AudioError::ZeroChannels)
        );
        assert_eq!(
            AudioBuffer::from_frames(&[0.0; 5], 48_000, 2),
            Err(AudioError::RaggedFrames)
        );
    }

    #[test]
    fn debug_does_not_print_samples() {
        // FR-20/NFR-04: audio never reaches a log. Bite: derived Debug.
        let buf = AudioBuffer::from_16k_mono(vec![12345; 8]);
        let debug = format!("{buf:?}");
        assert!(!debug.contains("12345"), "Debug prints samples: {debug}");
    }
}
