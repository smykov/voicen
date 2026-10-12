//! The one-time "logs cannot be written" notice (T-054; spec 006 FR-009,
//! Clarification Q3).
//!
//! The log reports that it became unwritable through its `on_unwritable` callback,
//! at most once per [`Log`](super::Log), and it may do so before the dictation
//! session exists (inside `Log::open`, at start), after it exists, or while it is
//! being attached, on whichever thread wrote the line, which may hold the session
//! lock. [`LogsNotice`] sits between the two.
//!
//! Invariant: at most one delivery per latch, and a reported failure is delivered
//! once a sink is attached, whatever the order. The callback takes only the latch's
//! own mutex (a leaf lock: nothing else is taken under it) and never calls the sink
//! inline. A delivery at [`LogsNotice::attach`] runs on the attaching thread before
//! `attach` returns; a delivery after the attach runs on one spawned thread. If that
//! thread cannot be spawned the notice is lost (there is nothing to report it to).
//!
//! The latch holds the attached sink strongly until it delivers (it may be the
//! sink's only owner). In release this is a reference cycle while nothing has been
//! delivered: session -> pipeline observer -> `Log` -> `on_unwritable` -> latch ->
//! session, so the session's `Drop` (which joins its threads) never runs. That is
//! harmless there, because the session lives for the whole process as Tauri managed
//! state. A caller that relies on dropping the session must not attach it to a log
//! it keeps alive, or must give that log a fresh latch.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use super::log::{LogsUnwritableReason, OnUnwritable};

/// Who shows the notice (the dictation session implements it).
pub trait LogsNoticeSink: Send + Sync {
    /// Shows `notice.logs_unwritable` with `reason`. Called at most once per latch,
    /// never from inside the log's callback.
    fn logs_unwritable(&self, reason: LogsUnwritableReason);
}

enum State {
    /// No failure, no sink.
    Empty,
    /// A failure reported before any sink was attached.
    Pending(LogsUnwritableReason),
    /// A sink waiting for a failure.
    Attached(Arc<dyn LogsNoticeSink>),
    /// The notice was handed to a sink; nothing more is delivered.
    Delivered,
}

/// The latch between the log's `on_unwritable` and the notice sink.
pub struct LogsNotice {
    state: Mutex<State>,
}

impl Default for LogsNotice {
    fn default() -> LogsNotice {
        LogsNotice::new()
    }
}

impl LogsNotice {
    /// An empty latch: no failure reported, no sink attached.
    pub fn new() -> LogsNotice {
        LogsNotice {
            state: Mutex::new(State::Empty),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The `on_unwritable` to give the log. A call before the attach latches the
    /// reason; a call after it hands the reason to the sink on a spawned thread;
    /// any call after the first adds nothing.
    pub fn callback(self: &Arc<Self>) -> OnUnwritable {
        let latch = Arc::clone(self);
        Box::new(move |reason| latch.report(reason))
    }

    fn report(&self, reason: LogsUnwritableReason) {
        let sink = {
            let mut st = self.lock();
            match std::mem::replace(&mut *st, State::Delivered) {
                State::Empty => {
                    *st = State::Pending(reason);
                    return;
                }
                State::Attached(sink) => sink,
                other @ (State::Pending(_) | State::Delivered) => {
                    *st = other;
                    return;
                }
            }
        };
        // Off the caller's thread: it may hold the lock the sink takes.
        let _ = thread::Builder::new()
            .name("voicen-logs-notice".into())
            .spawn(move || sink.logs_unwritable(reason));
    }

    /// Attaches the sink. A failure already reported is delivered here, on this
    /// thread, before this returns. A second attach before any failure replaces
    /// the sink; after the delivery it does nothing.
    pub fn attach(&self, sink: Arc<dyn LogsNoticeSink>) {
        let mut st = self.lock();
        match std::mem::replace(&mut *st, State::Delivered) {
            State::Pending(reason) => {
                drop(st);
                sink.logs_unwritable(reason);
            }
            State::Empty => *st = State::Attached(sink),
            State::Attached(previous) => {
                *st = State::Attached(sink);
                // The replaced sink is released outside the latch's lock.
                drop(st);
                drop(previous);
            }
            State::Delivered => {}
        }
    }
}
