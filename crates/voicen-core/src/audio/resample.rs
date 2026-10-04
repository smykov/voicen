//! Capture rate -> 16 kHz conversion with rubato 5 (spec 001 T009). Used only by
//! [`AudioBuffer::from_frames`](super::AudioBuffer::from_frames); its behaviour is
//! tested there (`audio::tests`).

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};

use super::SAMPLE_RATE;

/// Chunk size handed to the FFT resampler (frames); the resampler rounds it to a
/// multiple of its minimum block for the rate pair.
const CHUNK: usize = 1024;

/// The resampler refused the input (not expected for a rate above 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ResampleFailed;

/// Mono samples at `rate` Hz resampled to [`SAMPLE_RATE`] as one whole clip
/// (`Resampler::process_all`: startup delay trimmed, no padding), so the output has
/// `ceil(len * 16000 / rate)` samples. `rate` must be above 0.
pub(super) fn mono_to_16k(mono: &[f32], rate: u32) -> Result<Vec<f32>, ResampleFailed> {
    if mono.is_empty() {
        return Ok(Vec::new());
    }
    let rate_in = usize::try_from(rate).map_err(|_| ResampleFailed)?;
    let rate_out = usize::try_from(SAMPLE_RATE).map_err(|_| ResampleFailed)?;
    let mut resampler = Fft::<f32>::new(rate_in, rate_out, CHUNK, 1, FixedSync::Input)
        .map_err(|_| ResampleFailed)?;
    let input = InterleavedSlice::new(mono, 1, mono.len()).map_err(|_| ResampleFailed)?;
    let output = resampler
        .process_all(&input, mono.len(), None)
        .map_err(|_| ResampleFailed)?;
    Ok(output.take_data())
}
