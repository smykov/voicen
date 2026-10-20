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
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use voicen_core::engine::speech_model::{Abort, SpeechModel, SpeechModelLoader};
use voicen_core::failure::FailureReason;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
    WhisperSysContext, WhisperSysState,
};

/// Upper bound of the decoder threads (T-099): more threads stop paying off on whisper's
/// small models and starve the rest of the desktop.
const MAX_THREADS: usize = 8;

/// Loads a whisper model file (`ggml-*.bin`) for the CPU.
#[derive(Debug, Default)]
pub struct WhisperLoader {
    _private: (),
}

impl WhisperLoader {
    /// A loader. Silences whisper.cpp and ggml for the whole process (whisper-rs
    /// `install_logging_hooks`, once).
    pub fn new() -> WhisperLoader {
        whisper_rs::install_logging_hooks();
        WhisperLoader { _private: () }
    }
}

impl SpeechModelLoader for WhisperLoader {
    fn load(&self, path: &Path) -> Result<Box<dyn SpeechModel>, FailureReason> {
        // Also here: a loader built through `Default` has not installed the hooks.
        whisper_rs::install_logging_hooks();
        let mut params = WhisperContextParameters::new();
        params.use_gpu(false);
        let context = WhisperContext::new_with_params(path, params)
            .map_err(|_| FailureReason::LocalEngineFailed)?;
        let state = context
            .create_state()
            .map_err(|_| FailureReason::LocalEngineFailed)?;
        Ok(Box::new(WhisperModel {
            state,
            threads: thread_count(),
            params: HashMap::new(),
        }))
    }
}

/// A loaded whisper model with its decoding state (the state holds the context).
pub struct WhisperModel {
    state: WhisperState,
    threads: c_int,
    /// Parameters per language: `None` = auto-detect, `Some(code)` = whisper's code for it.
    params: HashMap<Option<&'static str>, FullParams<'static, 'static>>,
}

impl std::fmt::Debug for WhisperModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WhisperModel")
            .field("threads", &self.threads)
            .finish_non_exhaustive()
    }
}

impl WhisperModel {
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

/// min(available parallelism, [`MAX_THREADS`]); 1 when the OS does not say.
fn thread_count() -> c_int {
    let n = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(MAX_THREADS);
    c_int::try_from(n).unwrap_or(1)
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
