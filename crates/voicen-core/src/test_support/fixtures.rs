//! Synthetic audio fixtures for the speech gate (T-041) and the pipeline (T-001):
//! 16 kHz mono i16, generated in code from a fixed-seed generator, so no binary
//! files and no licence (`rand` is not consented; T-001 Investigation "Fixtures").
//!
//! The recipes and seeds match the host simulation recorded in the T-041
//! Investigation table. They model the sounds only as far as the energy rule sees
//! them; real-audio FR-12 is T-043's (Silero on real clips) and the owner's. Each
//! test that uses a fixture also asserts the fixture's premise (frame levels, runs),
//! so a change here cannot silently stop a fixture from biting (P-005).

use std::f64::consts::PI;

use crate::audio::{AudioBuffer, SAMPLE_RATE};

const SR: f64 = SAMPLE_RATE as f64;

/// A 64-bit linear congruential generator (Knuth's MMIX constants). Deterministic
/// and good enough for noise in test fixtures; not for anything else.
#[derive(Debug, Clone)]
pub struct Lcg {
    state: u64,
}

impl Lcg {
    pub fn new(seed: u64) -> Lcg {
        Lcg { state: seed }
    }

    /// Uniform in `[0, 1)`: the top 53 bits of the next state.
    pub fn uniform(&mut self) -> f64 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.state >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in `[-1, 1)`.
    pub fn signed(&mut self) -> f64 {
        2.0 * self.uniform() - 1.0
    }
}

/// Full-scale amplitude for a level in dBFS.
pub fn db_to_amplitude(dbfs: f64) -> f64 {
    10f64.powf(dbfs / 20.0)
}

/// Uniform noise with the given RMS level in dBFS (uniform RMS = a / sqrt 3), as
/// floating-point samples in `[-1, 1]`.
pub fn noise_floor(rng: &mut Lcg, samples: usize, rms_dbfs: f64) -> Vec<f64> {
    let a = db_to_amplitude(rms_dbfs) * 3f64.sqrt();
    (0..samples).map(|_| a * rng.signed()).collect()
}

/// Floating-point samples to i16 (x 32767, rounded, saturated).
pub fn quantize(signal: &[f64]) -> Vec<i16> {
    signal
        .iter()
        .map(|v| (v * 32767.0).round().clamp(-32768.0, 32767.0) as i16)
        .collect()
}

/// A harmonic "voiced" signal over a -60 dBFS floor: f0 gliding 110 -> 220 Hz,
/// 7 harmonics (1/k), a 4 Hz raised-cosine syllable envelope that never drops
/// below `trough` (0..1), peak `peak_dbfs`.
pub fn speech(seed: u64, seconds: f64, trough: f64, peak_dbfs: f64) -> AudioBuffer {
    let mut rng = Lcg::new(seed);
    let n = (seconds * SR) as usize;
    let mut out = noise_floor(&mut rng, n, -60.0);
    let a = db_to_amplitude(peak_dbfs);
    let mut phase = 0.0f64;
    for (i, v) in out.iter_mut().enumerate() {
        let t = i as f64 / SR;
        let f0 = 110.0 + 110.0 * (t / seconds);
        phase += 2.0 * PI * f0 / SR;
        let env = trough + (1.0 - trough) * (0.5 - 0.5 * (2.0 * PI * 4.0 * t).cos());
        let s: f64 = (1..8)
            .map(|k| (k as f64 * phase).sin() / k as f64)
            .sum::<f64>()
            / 1.6;
        *v += a * env * s;
    }
    AudioBuffer::from_16k_mono(quantize(&out))
}

/// 3 s of syllabic speech, peak -12 dBFS, pauses down to the -60 dBFS floor.
/// Positive control: speech.
pub fn speech_3s() -> AudioBuffer {
    speech(1, 3.0, 0.0, -12.0)
}

/// 1 s of speech without pauses (envelope never below 0.6), peak -20 dBFS: the
/// quietest tenth of its frames is the speech itself. Speech only because the noise
/// floor estimate is capped at -40 dBFS.
pub fn continuous_speech_1s() -> AudioBuffer {
    speech(5, 1.0, 0.6, -20.0)
}

/// 0.25 s of syllabic speech: one loud run, shorter than the 300 ms minimum.
pub fn speech_250ms() -> AudioBuffer {
    speech(6, 0.25, 0.0, -12.0)
}

/// 3 s of uniform noise at -60 dBFS RMS (a quiet room, not digital zero).
pub fn silence_3s() -> AudioBuffer {
    let mut rng = Lcg::new(2);
    AudioBuffer::from_16k_mono(quantize(&noise_floor(
        &mut rng,
        3 * SAMPLE_RATE as usize,
        -60.0,
    )))
}

/// 3 s of digital zero (a muted device).
pub fn digital_zero_3s() -> AudioBuffer {
    AudioBuffer::from_16k_mono(vec![0; 3 * SAMPLE_RATE as usize])
}

/// 1 s: the -60 dBFS floor with one broadband burst at 0.2 s, peak -6 dBFS, 10 ms
/// linear attack, then -50 dB over 150 ms (300 ms in all).
pub fn cough_1s() -> AudioBuffer {
    let mut rng = Lcg::new(3);
    let n = SAMPLE_RATE as usize;
    let mut out = noise_floor(&mut rng, n, -60.0);
    let a = db_to_amplitude(-6.0);
    let start = (0.2 * SR) as usize;
    let decay_ln = db_to_amplitude(50.0).ln();
    let burst = (0.3 * SR) as usize;
    for (i, v) in out.iter_mut().skip(start).take(burst).enumerate() {
        let t = i as f64 / SR;
        let env = if t < 0.010 {
            t / 0.010
        } else {
            (-(t - 0.010) / 0.150 * decay_ln).exp()
        };
        *v += a * env * rng.signed();
    }
    AudioBuffer::from_16k_mono(quantize(&out))
}

/// 3 s: the -60 dBFS floor with 10-20 ms exponentially decaying clicks at
/// -10 dBFS peak, one every 125-167 ms (6-8 per second), the first at 50 ms.
pub fn keyboard_3s() -> AudioBuffer {
    let mut rng = Lcg::new(4);
    let n = 3 * SAMPLE_RATE as usize;
    let mut out = noise_floor(&mut rng, n, -60.0);
    let a = db_to_amplitude(-10.0);
    let mut pos = (0.05 * SR) as usize;
    while pos < n {
        let len = ((0.010 + 0.010 * rng.uniform()) * SR) as usize;
        for i in 0..len {
            if let Some(v) = out.get_mut(pos + i) {
                *v += a * (-5.0 * i as f64 / len as f64).exp() * rng.signed();
            }
        }
        pos += ((0.125 + 0.042 * rng.uniform()) * SR) as usize;
    }
    AudioBuffer::from_16k_mono(quantize(&out))
}
