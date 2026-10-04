//! The log writer (T-008): `logs/voicen.log`, rolled into `voicen-*.log`, retained
//! by size and age, and a degraded mode when the folder cannot be written.
//!
//! - Size: the active file stays `<= roll_bytes` (rolled before a line would cross
//!   it, or at the local date change); the rolled `voicen-*.log` files total
//!   `<= total_bytes - roll_bytes`, so the whole set is `<= total_bytes` at every
//!   instant.
//! - Age: rolled files older than `max_age` are deleted at open and after each
//!   roll. Retention touches only `voicen-*.log`, never `voicen.log` or any other
//!   file in the folder.
//! - Failure: `write` never panics and never surfaces an I/O error. An unwritable
//!   log is degraded; `on_unwritable` is called at most once per `Log`; reopening
//!   is tried at most every `reopen_every`, and a successful reopen writes
//!   `LogsRecovered` first.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::event::LogEvent;
use crate::clock::{Clock, LocalOffset};

/// Sizes and intervals of the log (FR-20: 7 days, 10 MB).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogConfig {
    /// The active file is rolled before a line would make it larger (2 MiB).
    pub roll_bytes: u64,
    /// The active file plus the rolled files never exceed this (10 MiB).
    pub total_bytes: u64,
    /// Rolled files older than this are deleted (7 days).
    pub max_age: Duration,
    /// A degraded log tries to reopen at most this often (60 s).
    pub reopen_every: Duration,
}

impl Default for LogConfig {
    /// 2 MiB, 10 MiB, 7 days, 60 s.
    fn default() -> LogConfig {
        // Skeleton (T-008 red tests): not implemented yet.
        todo!("T-008: LogConfig::default")
    }
}

/// Why the log cannot be written (T-054 shows it to the user once).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogsUnwritableReason {
    PermissionDenied,
    DiskFull,
    /// The logs path, or a part of it, is not a directory.
    NotADirectory,
    /// Any other error, with its raw OS code if it has one.
    Other(Option<i32>),
}

/// Called at most once per [`Log`] when it becomes unwritable.
pub type OnUnwritable = Box<dyn Fn(LogsUnwritableReason) + Send + Sync>;

/// The one log of the process. `Send + Sync`; whole lines under one mutex.
pub struct Log {
    // Skeleton (T-008 red tests): the writer state is not implemented yet.
    _skeleton: (),
}

impl Log {
    /// Opens `dir/voicen.log` (creating `dir`), runs retention, and never fails:
    /// an unwritable folder gives a degraded log and one `on_unwritable` call.
    /// Writes nothing by itself (the shell writes `Started` first).
    pub fn open(
        dir: PathBuf,
        clock: Arc<dyn Clock>,
        offset: Arc<dyn LocalOffset>,
        cfg: LogConfig,
        on_unwritable: OnUnwritable,
    ) -> Log {
        // Skeleton (T-008 red tests): not implemented yet.
        let _ = (dir, clock, offset, cfg, on_unwritable);
        todo!("T-008: Log::open")
    }

    /// Appends one whole line for `event` (rolling and retaining first when due).
    /// Never panics; an I/O error degrades the log instead of surfacing.
    pub fn write(&self, event: LogEvent) {
        // Skeleton (T-008 red tests): not implemented yet.
        let _ = event;
        todo!("T-008: Log::write")
    }
}
