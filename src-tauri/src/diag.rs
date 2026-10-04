//! The shell side of the local log (T-008; FR-20, FR-18): opens the one
//! `voicen_core::diag::Log` of the process over the logs dir and writes `Started`
//! as its first line. Replaces the skeleton's `log_start`. The resolver of the
//! logs dir stays `paths::log_dir()` (P-010); `run()` passes it, tests pass a temp
//! dir. The local UTC offset comes from the Windows time-zone rules; the wall time
//! from core's `SystemClock`.

use std::path::PathBuf;
use std::sync::Arc;

use voicen_core::diag::{Log, OnUnwritable};

/// Opens the one log over `logs_dir` (`SystemClock`, the local UTC offset,
/// `LogConfig::default()`) and writes `LogEvent::Started { build_info(),
/// std::process::id() }` as its first line. Never fails and never panics: an
/// unwritable folder gives a degraded log and one `on_unwritable` call (T-054
/// turns it into the one-time notice).
pub fn start(logs_dir: PathBuf, on_unwritable: OnUnwritable) -> Arc<Log> {
    // Skeleton (T-008 red tests): not implemented yet.
    let _ = (logs_dir, on_unwritable);
    todo!("T-008: diag::start")
}
