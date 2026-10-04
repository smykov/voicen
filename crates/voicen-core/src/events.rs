//! What a dictation job reports to the log (spec 001 data-model "DictationEvent",
//! FR-033, T-001).
//!
//! [`DictationEvent`] is `Copy`: no variant can hold a `String`, a `Vec`, a
//! `Secret` or a `FailureReason`, so no event can carry transcript text, audio, a
//! URL query, a header or a key. Only `Pipeline::run_job` emits the job events
//! (`Warning`, `SpeechGate`, `JobFinished`, `Delivered`), on one
//! [`PipelineObserver`]; the dictation session (`crate::dictation`, T-051) emits
//! `RecordingStarted` / `RecordingEnded` / `CaptureFailed` on the same observer.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use crate::delivery::DeliveryResult;
use crate::recording::{MicCause, RecordingEnd, RecordingId};

/// Which device a recording used (data-model `RecordingStarted`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Selected,
    Fallback,
}

/// The outcome code of `JobFinished`: `text` / `no_speech` / `failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeCode {
    Text,
    NoSpeech,
    Failed,
}

/// The code of a `Warning`: `vad_fallback`, `esc_unavailable`, `toast_failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningCode {
    VadFallback,
    EscUnavailable,
    ToastFailed,
}

/// The log allowlist: exactly these variants and fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictationEvent {
    RecordingStarted {
        recording: RecordingId,
        hotkey_to_first_frame_ms: u64,
        device: DeviceKind,
    },
    RecordingEnded {
        recording: RecordingId,
        duration_ms: u64,
        end: RecordingEnd,
    },
    SpeechGate {
        recording: RecordingId,
        /// `GateDecision::detector`: "silero" | "energy".
        detector: &'static str,
        speech: bool,
    },
    JobFinished {
        seq: u64,
        /// The job's recording (T-008: the log joins a job to its recording's
        /// `RecordingStarted` / `RecordingEnded` by this id, not by event order).
        recording: RecordingId,
        /// `Engine::kind()`; `None` when no engine was built.
        engine: Option<&'static str>,
        /// From the recording's `stopped_at` to the outcome.
        stop_to_text_ms: u64,
        outcome: OutcomeCode,
        /// `FailureReason::code()` for a failed job.
        failure: Option<&'static str>,
        /// The status of a `ServerError` only.
        http_status: Option<u16>,
    },
    Delivered {
        seq: u64,
        text_to_paste_ms: u64,
        result: DeliveryResult,
    },
    Warning {
        code: WarningCode,
    },
    /// The capture of `recording` failed: at press (`RecordingController::capture_failed`)
    /// or at stop (`finish(Err)`). Only the closed [`MicCause`], never OS text (P-009).
    /// Emitted by the dictation session (T-051).
    CaptureFailed {
        recording: RecordingId,
        cause: MicCause,
    },
}

/// Receives every event (T-008 writes them to the log file).
pub trait PipelineObserver: Send + Sync {
    fn event(&self, e: &DictationEvent);
}

#[cfg(any(test, feature = "test-fakes"))]
pub use fake::RecordingObserver;

#[cfg(any(test, feature = "test-fakes"))]
mod fake {
    use std::sync::{Mutex, PoisonError};

    use super::{DictationEvent, PipelineObserver};

    /// A [`PipelineObserver`] that keeps every event in order.
    #[derive(Default)]
    pub struct RecordingObserver {
        events: Mutex<Vec<DictationEvent>>,
    }

    impl RecordingObserver {
        pub fn new() -> RecordingObserver {
            RecordingObserver::default()
        }

        /// Every event so far, in order.
        pub fn events(&self) -> Vec<DictationEvent> {
            self.events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl PipelineObserver for RecordingObserver {
        fn event(&self, e: &DictationEvent) {
            self.events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(*e);
        }
    }
}
