//! The shared history of a process (spec 005 T013, T022): the pipeline's production
//! `TranscriptSink`, the settings follower's target and the History window commands'
//! source, all over one locked [`History`].

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::UNIX_EPOCH;

use super::error::HistoryError;
use super::model::{EngineLabel, HistoryView};
use super::store::HistoryStore;
use super::{size_of, History};
use crate::clock::Clock;
use crate::pipeline::TranscriptSink;

/// Called with the new view after each change (the shell emits it to the History
/// window). Runs under the recorder's lock, so views arrive in order; it must not call
/// back into the recorder.
pub type OnChange = Box<dyn Fn(HistoryView) + Send + Sync>;

/// Called with each disk error (the shell writes one log line: op and kind only).
/// Runs under the recorder's lock, like [`OnChange`].
pub type OnError = Box<dyn Fn(&HistoryError) + Send + Sync>;

/// One [`History`] behind a mutex, stamped by `clock`, announcing every change.
/// Never fails a caller: a disk failure is in the view's notice and goes to the
/// error callback (NFR-07, SC-004).
pub struct HistoryRecorder<S: HistoryStore> {
    history: Mutex<History<S>>,
    clock: Arc<dyn Clock>,
    on_change: OnChange,
    on_error: OnError,
}

impl<S: HistoryStore> HistoryRecorder<S> {
    /// The recorder over `history`; disk errors are dropped until
    /// [`with_errors`](Self::with_errors) gives them a sink.
    pub fn new(history: History<S>, clock: Arc<dyn Clock>, on_change: OnChange) -> Self {
        HistoryRecorder {
            history: Mutex::new(history),
            clock,
            on_change,
            on_error: Box::new(|_| {}),
        }
    }

    /// This recorder with `on_error` called for every disk error.
    pub fn with_errors(self, on_error: OnError) -> Self {
        HistoryRecorder { on_error, ..self }
    }

    fn lock(&self) -> MutexGuard<'_, History<S>> {
        self.history.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `op` on the history; reports its error and, when the view changed,
    /// announces the new one. Returns the view after `op`.
    fn change(&self, op: impl FnOnce(&mut History<S>) -> Option<HistoryError>) -> HistoryView {
        let mut history = self.lock();
        let before = history.view();
        if let Some(error) = op(&mut history) {
            (self.on_error)(&error);
        }
        let after = history.view();
        if after != before {
            (self.on_change)(after.clone());
        }
        after
    }

    /// The current view (memory only).
    pub fn view(&self) -> HistoryView {
        self.lock().view()
    }

    /// The full text of the listed entry `id`, for the clipboard; `None` otherwise.
    pub fn text_of(&self, id: u64) -> Option<String> {
        self.lock().text_of(id).map(str::to_string)
    }

    /// [`History::clear`]; returns the view after it.
    pub fn clear(&self) -> HistoryView {
        self.change(History::clear)
    }

    /// [`History::apply_settings`] with the settings' `history.size`; returns the view.
    pub fn apply_settings(&self, enabled: bool, size: u32) -> HistoryView {
        self.change(|history| history.apply_settings(enabled, size_of(size)))
    }
}

impl<S: HistoryStore> TranscriptSink for HistoryRecorder<S> {
    /// Records `text` at the clock's current UTC milliseconds.
    fn delivered(&self, text: &str, engine: EngineLabel) {
        let ready_at_ms = self
            .clock
            .now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        self.change(|history| history.record(text.to_string(), ready_at_ms, engine));
    }
}
