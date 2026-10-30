//! Built-in engine of Voicen (T-099, decisions #12, #130, #142): implements
//! voicen-core's [`SpeechModelLoader`] and [`SpeechModel`] with whisper.cpp through
//! whisper-rs, and its [`SpeechDetector`](voicen_core::vad::SpeechDetector) with
//! whisper.cpp's Silero VAD ([`SileroDetector`], T-043).
//!
//! - CPU only (`use_gpu` off); whisper.cpp is one static build with the CPU baseline of
//!   decision #142 and the shell's C runtime (both set in `.cargo/config.toml` [env]).
//! - whisper.cpp and ggml print nothing: their log hooks are installed before the first
//!   model load, and every `print_*` parameter is off. Transcript text never reaches a log
//!   (FR-20, NFR-04); no error carries it either.
//! - Every failure is `Err(FailureReason::LocalEngineFailed)`, never a panic.
//!
//! Known cost: whisper-rs `FullParams::set_language` leaks its C string. A model keeps one
//! set of parameters per language it was asked for (at most the ~100 languages whisper
//! knows), so the leak is bounded per loaded model, not per transcription.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod silero;

pub use silero::SileroDetector;

use std::collections::HashMap;
use std::ffi::{c_int, c_void};
use std::num::NonZero;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use voicen_core::engine::speech_model::{Abort, SpeechModel, SpeechModelLoader};
use voicen_core::failure::FailureReason;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
    WhisperSysContext, WhisperSysState,
};

/// B of the thread policy ([`thread_count_for`], T-109 I2, decision #200 D1): the best ggml
/// thread count of one transcription. 2 by decision #213: the lowest 30 s figure of the
/// windows-latest `whisper-timing` table (ties to fewer threads). Measured on 4 vCPUs only;
/// a stronger PC stays at 2 threads.
const BEST_THREADS: usize = 2;

/// Loads a whisper model file (`ggml-*.bin`) for the CPU.
#[derive(Debug, Default)]
pub struct WhisperLoader {
    /// Test-only override of the policy ([`WhisperLoader::with_threads`]); `None` = policy.
    threads: Option<NonZero<usize>>,
}

impl WhisperLoader {
    /// A loader. Silences whisper.cpp and ggml for the whole process (whisper-rs
    /// `install_logging_hooks`, once).
    pub fn new() -> WhisperLoader {
        whisper_rs::install_logging_hooks();
        WhisperLoader { threads: None }
    }

    /// Test-only (T-109, decision #200 D2): a loader whose models use `threads` ggml threads
    /// instead of the policy's count. Not a user setting. Silences whisper.cpp like [`new`].
    ///
    /// [`new`]: WhisperLoader::new
    #[doc(hidden)]
    pub fn with_threads(threads: NonZero<usize>) -> WhisperLoader {
        whisper_rs::install_logging_hooks();
        WhisperLoader {
            threads: Some(threads),
        }
    }

    /// Test-only (T-109): the concrete load behind [`SpeechModelLoader::load`] (the one load
    /// path), so a test can read [`WhisperModel`]'s thread and segment counts.
    #[doc(hidden)]
    pub fn load_whisper(&self, path: &Path) -> Result<WhisperModel, FailureReason> {
        // Also here: a loader built through `Default` has not installed the hooks.
        whisper_rs::install_logging_hooks();
        let threads = match self.threads {
            None => thread_count(),
            Some(n) => c_int::try_from(n.get()).unwrap_or(c_int::MAX),
        };
        let mut params = WhisperContextParameters::new();
        params.use_gpu(false);
        let context = WhisperContext::new_with_params(path, params)
            .map_err(|_| FailureReason::LocalEngineFailed)?;
        let state = context
            .create_state()
            .map_err(|_| FailureReason::LocalEngineFailed)?;
        Ok(WhisperModel {
            state,
            threads,
            params: HashMap::new(),
            last_segments: 0,
        })
    }
}

impl SpeechModelLoader for WhisperLoader {
    fn load(&self, path: &Path) -> Result<Box<dyn SpeechModel>, FailureReason> {
        Ok(Box::new(self.load_whisper(path)?))
    }
}

/// A loaded whisper model with its decoding state (the state holds the context).
pub struct WhisperModel {
    state: WhisperState,
    threads: c_int,
    /// Parameters per language: `None` = auto-detect, `Some(code)` = whisper's code for it.
    params: HashMap<Option<&'static str>, FullParams<'static, 'static>>,
    /// whisper.cpp's segment count of the last `transcribe` (0 before the first, and after
    /// a run that ended before `whisper_full` or was stopped before its first window).
    last_segments: usize,
}

impl std::fmt::Debug for WhisperModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WhisperModel")
            .field("threads", &self.threads)
            .finish_non_exhaustive()
    }
}

impl WhisperModel {
    /// Test-only (T-109): the ggml thread count this model computes with.
    #[doc(hidden)]
    pub fn threads(&self) -> usize {
        usize::try_from(self.threads).unwrap_or(1)
    }

    /// Test-only (T-109 I3): whisper.cpp's segment count (`whisper_full_n_segments`) of the
    /// last [`SpeechModel::transcribe`], on every path (success, error and abort); 0 when
    /// that run never reached `whisper_full` or was stopped before its first window.
    #[doc(hidden)]
    pub fn last_segment_count(&self) -> usize {
        self.last_segments
    }

    /// The parameters for `language`, built on first use.
    fn params_for(
        &mut self,
        language: Option<&str>,
    ) -> Result<FullParams<'static, 'static>, FailureReason> {
        // whisper's own code for the language: a static string of whisper.cpp's table.
        let code: Option<&'static str> = match language {
            None => None,
            Some(asked) => {
                // whisper-rs panics on a NUL byte in `get_lang_id`.
                if asked.contains('\0') {
                    return Err(FailureReason::LocalEngineFailed);
                }
                let id = whisper_rs::get_lang_id(asked).ok_or(FailureReason::LocalEngineFailed)?;
                Some(whisper_rs::get_lang_str(id).ok_or(FailureReason::LocalEngineFailed)?)
            }
        };
        if let Some(p) = self.params.get(&code) {
            return Ok(p.clone());
        }
        let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        p.set_n_threads(self.threads);
        p.set_print_special(false);
        p.set_print_progress(false);
        p.set_print_realtime(false);
        p.set_print_timestamps(false);
        // Each dictation stands alone: no text of the previous one is fed back as a prompt.
        p.set_no_context(true);
        // None: a null language makes whisper.cpp detect it and transcribe in it (whisper-rs
        // `set_detect_language` would only detect). Some: leaked once (see the crate doc).
        p.set_language(code);
        self.params.insert(code, p.clone());
        Ok(p)
    }
}

impl SpeechModel for WhisperModel {
    fn transcribe(
        &mut self,
        pcm: &[f32],
        language: Option<&str>,
        abort: &Abort,
    ) -> Result<String, FailureReason> {
        self.last_segments = 0;
        // whisper-rs passes the length as a C int; an empty slice is refused there.
        if pcm.is_empty() || c_int::try_from(pcm.len()).is_err() {
            return Err(FailureReason::LocalEngineFailed);
        }
        let mut params = self.params_for(language)?;
        let user_data = abort as *const Abort as *mut c_void;
        // SAFETY: both callbacks only read the `Abort` behind `user_data`, which outlives the
        // synchronous `full` call below that is the only user of `params`; `Abort` is `Sync`,
        // so ggml's compute threads may call `abort_requested` concurrently.
        unsafe {
            params.set_abort_callback(Some(abort_requested));
            params.set_abort_callback_user_data(user_data);
            params.set_start_encoder_callback(Some(encoder_may_start));
            params.set_start_encoder_callback_user_data(user_data);
        }
        let ran = self.state.full(params, pcm);
        // Recorded before any early return: the abort and error paths count too (T-109 I3).
        self.last_segments = usize::try_from(self.state.full_n_segments()).unwrap_or(0);
        if abort.requested() {
            // Ignored by the engine, which reports Timeout (decision #133).
            return Err(FailureReason::LocalEngineFailed);
        }
        ran.map_err(|_| FailureReason::LocalEngineFailed)?;
        let mut text = String::new();
        for i in 0..self.state.full_n_segments() {
            let segment = self
                .state
                .get_segment(i)
                .ok_or(FailureReason::LocalEngineFailed)?;
            let part = segment
                .to_str_lossy()
                .map_err(|_| FailureReason::LocalEngineFailed)?;
            text.push_str(&part);
        }
        Ok(text.trim().to_owned())
    }
}

/// The engine's ggml thread policy (T-109 I2, decision #200 D1), its single source:
/// `max(1, min(B, P - 1))` for `parallelism` P, B = [`BEST_THREADS`]. Below P whenever
/// P > 1 (ggml's barrier spins, so a thread per CPU stalls whenever anything else needs a
/// core) and at least 1. Public only for the tests.
#[doc(hidden)]
pub fn thread_count_for(parallelism: usize) -> usize {
    BEST_THREADS.min(parallelism.saturating_sub(1)).max(1)
}

/// [`thread_count_for`] on this machine's available parallelism (1 when the OS does not say).
fn thread_count() -> c_int {
    let p = std::thread::available_parallelism().map_or(1, |n| n.get());
    c_int::try_from(thread_count_for(p)).unwrap_or(1)
}

/// Whether `user_data` (an `&Abort`) asks to stop; a panic counts as a stop.
fn requested(user_data: *mut c_void) -> bool {
    // SAFETY: `transcribe` sets `user_data` to an `&Abort` that lives through `full`.
    let Some(abort) = (unsafe { (user_data as *const Abort).as_ref() }) else {
        return true;
    };
    catch_unwind(AssertUnwindSafe(|| abort.requested())).unwrap_or(true)
}

/// ggml's abort callback: true stops the running graph. whisper.cpp checks it only after the
/// encoder graph, so [`encoder_may_start`] stops a transcription before the next encode.
unsafe extern "C" fn abort_requested(user_data: *mut c_void) -> bool {
    requested(user_data)
}

/// whisper.cpp's encoder-begin callback: false ends `whisper_full` before the next window.
unsafe extern "C" fn encoder_may_start(
    _ctx: *mut WhisperSysContext,
    _state: *mut WhisperSysState,
    user_data: *mut c_void,
) -> bool {
    !requested(user_data)
}
