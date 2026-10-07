//! The release `PipelineObserver` (T-008): one dictation line per recording,
//! joined by `RecordingId`, never by event order.
//!
//! Closing rules: `Delivered` for a text outcome (it follows `JobFinished{Text}`,
//! joined by `seq`); `JobFinished` for no-speech or failed; `RecordingEnded{TooShort}`
//! for a discarded recording; `CaptureFailed` for a capture that failed at press
//! (its only event) or at stop (after `RecordingStarted` / `RecordingEnded`, no
//! job follows). A pipeline `Warning` and a blocked press (`PressBlocked`, no
//! recording, T-006) are each their own line at once and touch no open record.
//!
//! Every match over the event types is exhaustive: a new `DictationEvent`,
//! `OutcomeCode`, `RecordingEnd` or `WarningCode` variant does not compile until
//! it has a log mapping here. The open records are bounded (64): a
//! record that never closes (a recording that never ends, a text job whose
//! `Delivered` never comes) is dropped without a line once newer ones push it out.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use super::event::{
    DetectorTag, DictationLine, DictationOutcome, EngineTag, FailureTag, LogEvent, WarningKind,
};
use super::log::Log;
use crate::events::{DictationEvent, OutcomeCode, PipelineObserver, WarningCode};
use crate::recording::{RecordingEnd, RecordingId};

/// Open records (and text jobs awaiting `Delivered`) kept at most; the oldest
/// recording id (smallest seq) goes first.
const MAX_OPEN: usize = 64;

/// Aggregates the events of each recording into one [`LogEvent::Dictation`](super::LogEvent::Dictation).
pub struct LogObserver {
    log: Arc<Log>,
    open: Mutex<Open>,
}

#[derive(Default)]
struct Open {
    records: BTreeMap<RecordingId, Record>,
    /// Text jobs by `seq`, waiting for their `Delivered`.
    awaiting_delivery: BTreeMap<u64, RecordingId>,
}

/// What has come for one recording so far.
#[derive(Default, Clone, Copy)]
struct Record {
    engine: Option<EngineTag>,
    detector: Option<DetectorTag>,
    press_to_frame_ms: Option<u64>,
    duration_ms: Option<u64>,
    stop_to_text_ms: Option<u64>,
}

impl Record {
    fn line(
        self,
        recording: RecordingId,
        outcome: DictationOutcome,
        text_to_paste_ms: Option<u64>,
    ) -> LogEvent {
        LogEvent::Dictation(DictationLine {
            recording: recording.get(),
            engine: self.engine,
            outcome,
            detector: self.detector,
            press_to_frame_ms: self.press_to_frame_ms,
            duration_ms: self.duration_ms,
            stop_to_text_ms: self.stop_to_text_ms,
            text_to_paste_ms,
        })
    }
}

impl Open {
    /// The record of `recording`, opened if new (evicting the oldest past
    /// [`MAX_OPEN`]).
    fn record(&mut self, recording: RecordingId) -> &mut Record {
        if !self.records.contains_key(&recording) && self.records.len() >= MAX_OPEN {
            if let Some(oldest) = self.records.keys().next().copied() {
                self.records.remove(&oldest);
            }
        }
        self.records.entry(recording).or_default()
    }

    /// Removes and returns the record of `recording` (empty if none was open).
    fn close(&mut self, recording: RecordingId) -> Record {
        self.records.remove(&recording).unwrap_or_default()
    }

    fn await_delivery(&mut self, seq: u64, recording: RecordingId) {
        if !self.awaiting_delivery.contains_key(&seq) && self.awaiting_delivery.len() >= MAX_OPEN {
            if let Some(oldest) = self.awaiting_delivery.keys().next().copied() {
                self.awaiting_delivery.remove(&oldest);
            }
        }
        self.awaiting_delivery.insert(seq, recording);
    }

    /// The line `e` closes or is, if any, after updating the open records.
    fn apply(&mut self, e: &DictationEvent) -> Option<LogEvent> {
        match *e {
            DictationEvent::RecordingStarted {
                recording,
                hotkey_to_first_frame_ms,
                device: _,
            } => {
                self.record(recording).press_to_frame_ms = Some(hotkey_to_first_frame_ms);
                None
            }
            DictationEvent::RecordingEnded {
                recording,
                duration_ms,
                end,
            } => {
                self.record(recording).duration_ms = Some(duration_ms);
                match end {
                    RecordingEnd::Released => None,
                    RecordingEnd::TooShort => Some(self.close(recording).line(
                        recording,
                        DictationOutcome::TooShort,
                        None,
                    )),
                }
            }
            DictationEvent::SpeechGate {
                recording,
                detector,
                speech: _,
            } => {
                self.record(recording).detector = Some(DetectorTag::from_name(detector));
                None
            }
            DictationEvent::JobFinished {
                seq,
                recording,
                engine,
                stop_to_text_ms,
                outcome,
                failure,
                http_status,
            } => {
                let record = self.record(recording);
                record.engine = engine.map(EngineTag::from_kind);
                record.stop_to_text_ms = Some(stop_to_text_ms);
                match outcome {
                    OutcomeCode::Text => {
                        self.await_delivery(seq, recording);
                        None
                    }
                    OutcomeCode::NoSpeech => Some(self.close(recording).line(
                        recording,
                        DictationOutcome::NoSpeech,
                        None,
                    )),
                    OutcomeCode::Failed => Some(self.close(recording).line(
                        recording,
                        DictationOutcome::Failed {
                            failure: failure.map_or(FailureTag::Other, FailureTag::from_code),
                            http_status,
                        },
                        None,
                    )),
                }
            }
            DictationEvent::Delivered {
                seq,
                text_to_paste_ms,
                result,
            } => {
                // A Delivered without its JobFinished has no recording: no line.
                let recording = self.awaiting_delivery.remove(&seq)?;
                Some(self.close(recording).line(
                    recording,
                    DictationOutcome::Delivered(result),
                    Some(text_to_paste_ms),
                ))
            }
            // At press it is the recording's only event; at stop it follows
            // RecordingStarted / RecordingEnded{Released} and no job comes.
            DictationEvent::CaptureFailed { recording, cause } => Some(self.close(recording).line(
                recording,
                DictationOutcome::CaptureFailed { cause },
                None,
            )),
            // Not a recording: its own line at once, no open record touched.
            DictationEvent::PressBlocked { reason } => Some(LogEvent::DictationBlocked { reason }),
            DictationEvent::Warning { code } => Some(LogEvent::Warning {
                kind: match code {
                    WarningCode::VadFallback => WarningKind::VadFallback,
                    WarningCode::EscUnavailable => WarningKind::EscUnavailable,
                    WarningCode::ToastFailed => WarningKind::ToastFailed,
                },
                os_code: None,
            }),
        }
    }
}

impl LogObserver {
    pub fn new(log: Arc<Log>) -> LogObserver {
        LogObserver {
            log,
            open: Mutex::new(Open::default()),
        }
    }
}

impl PipelineObserver for LogObserver {
    fn event(&self, e: &DictationEvent) {
        let line = self
            .open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .apply(e);
        if let Some(line) = line {
            self.log.write(line);
        }
    }
}
