//! A scripted [`SpeechSpans`] for tests (T-134; feature `test-fakes`, never in a
//! release build): answers from a queue of span lists or [`VadError`]s, then with a
//! default forever, and counts its calls. T-137 and T-138 tests use it too.

use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;
use std::sync::Mutex;

use crate::audio::AudioBuffer;
use crate::vad::{SpeechSpan, SpeechSpans, VadError};

pub struct ScriptedSpans {
    _script: Mutex<VecDeque<Result<Vec<SpeechSpan>, VadError>>>,
    _then: Result<Vec<SpeechSpan>, VadError>,
    _calls: AtomicUsize,
}

impl ScriptedSpans {
    pub fn new(
        _script: Vec<Result<Vec<SpeechSpan>, VadError>>,
        _then: Result<Vec<SpeechSpan>, VadError>,
    ) -> ScriptedSpans {
        todo!("T-134")
    }

    /// How many times `speech_spans` was called.
    pub fn calls(&self) -> usize {
        todo!("T-134")
    }
}

impl SpeechSpans for ScriptedSpans {
    fn name(&self) -> &'static str {
        todo!("T-134")
    }

    fn speech_spans(&self, _audio: &AudioBuffer) -> Result<Vec<SpeechSpan>, VadError> {
        todo!("T-134")
    }
}
