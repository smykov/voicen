//! The release `PipelineObserver` (T-008): one dictation line per recording,
//! joined by `RecordingId`, never by event order.
//!
//! Closing rules: `Delivered` for a text outcome (it follows `JobFinished{Text}`,
//! joined by `seq`); `JobFinished` for no-speech or failed; `RecordingEnded{TooShort}`
//! for a discarded recording. A pipeline `Warning` is its own line at once.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use super::log::Log;
use crate::events::{DictationEvent, PipelineObserver};

/// Aggregates the events of each recording into one [`LogEvent::Dictation`](super::LogEvent::Dictation).
pub struct LogObserver {
    // Skeleton (T-008 red tests): the open records are not implemented yet.
    _skeleton: (),
}

impl LogObserver {
    pub fn new(log: Arc<Log>) -> LogObserver {
        // Skeleton (T-008 red tests): not implemented yet.
        let _ = log;
        todo!("T-008: LogObserver::new")
    }
}

impl PipelineObserver for LogObserver {
    fn event(&self, e: &DictationEvent) {
        // Skeleton (T-008 red tests): not implemented yet.
        let _ = e;
        todo!("T-008: LogObserver::event")
    }
}
