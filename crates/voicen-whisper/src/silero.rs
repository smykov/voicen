//! Silero speech detector over whisper.cpp's VAD (T-043; FR-12; decisions #12, #60, #136):
//! implements voicen-core's [`SpeechDetector`] on the pinned Silero model
//! (`voicen_core::vad::silero_model::SILERO_MODEL`).
//!
//! - [`SileroDetector::load`] is the only constructor and hands whisper.cpp only a file that
//!   `silero_model::verify` accepted: whisper.cpp's VAD loader throws C++ exceptions across
//!   `extern "C"` on bad bytes, which would abort the process (T-043 invariant 1).
//! - Never panics and never reports an error as speech: a native failure or a poisoned lock
//!   is `Err(VadError::Failed)`; empty audio is `Ok(false)` without a native call (invariant 2).
//! - Rule: speech iff the summed length of the segments whisper.cpp finds with fixed
//!   parameters (its defaults) is at least [`MIN_SPEECH_CS`]. The constant holds for the
//!   whole clip set of `tests/silero_clips.rs` (speech 89-154 cs, non-speech 0 cs), never
//!   tuned per clip.
//! - Prints nothing: whisper.cpp's and ggml's log hooks are installed before the native load.

use std::ffi::c_int;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::Mutex;

use voicen_core::audio::AudioBuffer;
use voicen_core::vad::silero_model;
use voicen_core::vad::{SpeechDetector, VadError};
use whisper_rs::{WhisperVadContext, WhisperVadContextParams, WhisperVadParams};

/// Minimum summed segment length, in centiseconds, for a recording to count as speech.
/// whisper.cpp already drops segments under its 250 ms minimum; this is the detector's own
/// floor, fixed for the whole clip set.
const MIN_SPEECH_CS: f32 = 25.0;

/// Upper bound of the VAD threads (whisper-rs' default): the Silero model is tiny.
const MAX_THREADS: usize = 4;

/// The Silero detector: one whisper.cpp VAD context behind a lock (`&self`, `Send + Sync`).
pub struct SileroDetector {
    context: Mutex<WhisperVadContext>,
}

impl std::fmt::Debug for SileroDetector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SileroDetector").finish_non_exhaustive()
    }
}

impl SileroDetector {
    /// Loads the Silero model at `path`. `Err(VadError::Unavailable)` when the file is not
    /// the pinned model (checked before any whisper.cpp call), the path is not UTF-8 or holds
    /// a NUL, the CPU lacks AVX2 (the whisper.cpp baseline, decision #142), or whisper.cpp
    /// refuses the file. Never panics.
    pub fn load(path: &Path) -> Result<Box<dyn SpeechDetector>, VadError> {
        silero_model::verify(path)?;
        whisper_rs::install_logging_hooks();
        // whisper-rs panics on a NUL in the path.
        let path = path
            .to_str()
            .filter(|p| !p.contains('\0'))
            .ok_or(VadError::Unavailable)?;
        if !cpu_supported() {
            return Err(VadError::Unavailable);
        }
        let mut params = WhisperVadContextParams::new();
        params.set_use_gpu(false);
        params.set_n_threads(thread_count());
        let context = catch_unwind(|| WhisperVadContext::new(path, params))
            .map_err(|_| VadError::Unavailable)?
            .map_err(|_| VadError::Unavailable)?;
        Ok(Box::new(SileroDetector {
            context: Mutex::new(context),
        }))
    }
}

impl SpeechDetector for SileroDetector {
    fn name(&self) -> &'static str {
        "silero"
    }

    fn contains_speech(&self, audio: &AudioBuffer) -> Result<bool, VadError> {
        let samples = audio.samples();
        if samples.is_empty() {
            return Ok(false);
        }
        // whisper-rs passes the length as a C int.
        if c_int::try_from(samples.len()).is_err() {
            return Err(VadError::Failed);
        }
        let pcm: Vec<f32> = samples.iter().map(|&s| f32::from(s) / 32768.0).collect();
        let mut context = self.context.lock().map_err(|_| VadError::Failed)?;
        let total_cs = catch_unwind(AssertUnwindSafe(|| -> Result<f32, VadError> {
            let segments = context
                .segments_from_samples(WhisperVadParams::new(), &pcm)
                .map_err(|_| VadError::Failed)?;
            Ok(segments
                .map(|s| s.end - s.start)
                .filter(|len| len.is_finite() && *len > 0.0)
                .sum())
        }))
        .map_err(|_| VadError::Failed)??;
        Ok(total_cs >= MIN_SPEECH_CS)
    }
}

/// Whether this CPU runs the whisper.cpp build (AVX2 baseline on x86, decision #142).
fn cpu_supported() -> bool {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        std::arch::is_x86_feature_detected!("avx2")
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        true
    }
}

/// min(available parallelism, [`MAX_THREADS`]); 1 when the OS does not say.
fn thread_count() -> c_int {
    let n = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(MAX_THREADS);
    c_int::try_from(n).unwrap_or(1)
}
