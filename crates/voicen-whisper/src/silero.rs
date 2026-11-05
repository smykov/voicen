//! Silero speech detector over whisper.cpp's VAD (T-043; FR-12; decisions #12, #60, #136):
//! implements voicen-core's [`SpeechDetector`] and [`SpeechSpans`] (T-135; spec 007 FR-023,
//! R8) on the pinned Silero model (`voicen_core::vad::silero_model::SILERO_MODEL`).
//!
//! - [`SileroDetector::open`] is the only constructor ([`SileroDetector::load`] boxes it) and
//!   hands whisper.cpp only a file that `silero_model::verify` accepted: whisper.cpp's VAD
//!   loader throws C++ exceptions across `extern "C"` on bad bytes, which would abort the
//!   process (T-043 invariant 1).
//! - One load, many ports: `Clone` shares the one native context (`Arc`), so the dictation
//!   gate and the final pass use the same load (decision #175).
//! - Never panics and never reports an error as speech or as no spans: every native answer
//!   goes through `voicen_core::vad::segments::spans_from_centiseconds`; a native failure, a
//!   malformed segment or a poisoned lock is `Err(VadError::Failed)`; empty audio is no spans
//!   (`Ok(false)`) without a native call (invariant 2).
//! - Rule: speech iff the summed length of the spans whisper.cpp finds with fixed parameters
//!   (its defaults) is at least [`MIN_SPEECH_CS`]; `contains_speech` is derived from the
//!   spans, so the two ports never disagree. The constant holds for the whole clip set of
//!   `tests/silero_clips.rs` (speech 89-154 cs, non-speech 0 cs), never tuned per clip.
//! - Prints nothing: whisper.cpp's and ggml's log hooks are installed before the native load.
//! - One ggml thread per decision ([`VAD_THREADS`]): see the constant for the measurements.
//!
//! Known cost: whisper-rs 0.16.0 `WhisperVadContext::new` hands whisper.cpp its path through
//! `CString::into_raw` and never frees it, so every successful or refused native load leaks
//! the path string once (a few hundred bytes). Bounded per load, not per decision; the shell
//! loads the detector once (T-102).

use std::ffi::c_int;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::{Arc, Mutex};

use voicen_core::audio::{AudioBuffer, SAMPLE_RATE};
use voicen_core::vad::segments::spans_from_centiseconds;
use voicen_core::vad::silero_model;
use voicen_core::vad::{SpeechDetector, SpeechSpan, SpeechSpans, VadError};
use whisper_rs::{WhisperVadContext, WhisperVadContextParams, WhisperVadParams};

/// Minimum summed span length, in centiseconds, for a recording to count as speech.
/// whisper.cpp already drops segments under its 250 ms minimum; this is the detector's own
/// floor, fixed for the whole clip set.
const MIN_SPEECH_CS: u64 = 25;

/// ggml compute threads of one decision: 1, as upstream Silero runs. whisper.cpp runs one
/// graph per 512-sample chunk (~94 for 3 s of audio), and ggml's CPU barrier between graph
/// nodes is a pure spin (no yield), so a waiting thread burns the CPU the thread it waits for
/// needs; with 1 thread the barrier returns at once. Measured (T-043 review 1, Linux surrogate,
/// a 3 s clip, best of 5): 1 thread ~15-20 ms, 4 threads 29-105 ms alone; on 4 pinned CPUs
/// with 4 detectors deciding at once, 1 thread ~20 ms and 4 threads ~48 s per decision.
const VAD_THREADS: c_int = 1;

/// The Silero detector: one whisper.cpp VAD context behind a shared lock (`&self`,
/// `Send + Sync`); a clone is another handle to the same context, never a second load.
#[derive(Clone)]
pub struct SileroDetector {
    context: Arc<Mutex<WhisperVadContext>>,
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
    /// refuses the file. Never panics. Each call that reaches whisper.cpp leaks the path
    /// string once (whisper-rs, see the module doc).
    pub fn load(path: &Path) -> Result<Box<dyn SpeechDetector>, VadError> {
        SileroDetector::open(path).map(|d| Box::new(d) as Box<dyn SpeechDetector>)
    }

    /// Loads the Silero model at `path` as a shareable detector: clones share this one native
    /// context. Errors as [`SileroDetector::load`].
    pub fn open(path: &Path) -> Result<SileroDetector, VadError> {
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
        params.set_n_threads(VAD_THREADS);
        let context = catch_unwind(|| WhisperVadContext::new(path, params))
            .map_err(|_| VadError::Unavailable)?
            .map_err(|_| VadError::Unavailable)?;
        Ok(SileroDetector {
            context: Arc::new(Mutex::new(context)),
        })
    }
}

impl SpeechDetector for SileroDetector {
    fn name(&self) -> &'static str {
        "silero"
    }

    /// Derived from [`SpeechSpans::speech_spans`] on the same buffer, so "no spans" and "no
    /// speech" never disagree; an error stays an error.
    fn contains_speech(&self, audio: &AudioBuffer) -> Result<bool, VadError> {
        let spans = SpeechSpans::speech_spans(self, audio)?;
        let total_ms: u64 = spans.iter().map(|s| s.end_ms - s.start_ms).sum();
        Ok(total_ms >= MIN_SPEECH_CS * 10)
    }
}

impl SpeechSpans for SileroDetector {
    fn name(&self) -> &'static str {
        "silero"
    }

    /// One native call under the shared lock; its answer, error or not, goes through the one
    /// funnel [`spans_from_centiseconds`].
    fn speech_spans(&self, audio: &AudioBuffer) -> Result<Vec<SpeechSpan>, VadError> {
        let samples = audio.samples();
        let duration_ms = samples.len() as u64 * 1000 / SAMPLE_RATE as u64;
        spans_from_centiseconds(self.native_segments(samples), duration_ms)
    }
}

impl SileroDetector {
    /// whisper.cpp's segments of `samples` as `(start, end)` centiseconds, with its default
    /// parameters. Empty audio is `Ok(empty)` without a native call; a native failure, a
    /// panic inside whisper-rs or a poisoned lock is `Err(VadError::Failed)`.
    fn native_segments(&self, samples: &[i16]) -> Result<Vec<(f32, f32)>, VadError> {
        if samples.is_empty() {
            return Ok(Vec::new());
        }
        // whisper-rs passes the length as a C int.
        if c_int::try_from(samples.len()).is_err() {
            return Err(VadError::Failed);
        }
        let pcm: Vec<f32> = samples.iter().map(|&s| f32::from(s) / 32768.0).collect();
        let mut context = self.context.lock().map_err(|_| VadError::Failed)?;
        catch_unwind(AssertUnwindSafe(|| {
            context
                .segments_from_samples(WhisperVadParams::new(), &pcm)
                .map(|segments| segments.map(|s| (s.start, s.end)).collect())
                .map_err(|_| VadError::Failed)
        }))
        .map_err(|_| VadError::Failed)?
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
