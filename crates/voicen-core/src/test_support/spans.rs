//! A scripted [`SpeechSpans`] for tests (T-134; feature `test-fakes`, never in a
//! release build): answers from a queue of span lists or [`VadError`]s, then with a
//! default forever, and counts its calls. T-137 and T-138 tests use it too.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use crate::audio::AudioBuffer;
use crate::vad::{SpeechSpan, SpeechSpans, VadError};

pub struct ScriptedSpans {
    script: Mutex<VecDeque<Result<Vec<SpeechSpan>, VadError>>>,
    then: Result<Vec<SpeechSpan>, VadError>,
    calls: AtomicUsize,
}

impl ScriptedSpans {
    /// Answers `script` in order, then `then` on every later call. Errors come back
    /// unchanged.
    pub fn new(
        script: Vec<Result<Vec<SpeechSpan>, VadError>>,
        then: Result<Vec<SpeechSpan>, VadError>,
    ) -> ScriptedSpans {
        ScriptedSpans {
            script: Mutex::new(script.into()),
            then,
            calls: AtomicUsize::new(0),
        }
    }

    /// How many times `speech_spans` was called.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl SpeechSpans for ScriptedSpans {
    fn name(&self) -> &'static str {
        "scripted"
    }

    fn speech_spans(&self, _audio: &AudioBuffer) -> Result<Vec<SpeechSpan>, VadError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.script
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(|| self.then.clone())
    }
}
