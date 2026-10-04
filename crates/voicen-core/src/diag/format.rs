//! The one line formatter (T-008): `<local time with offset> <LEVEL> <message>`.
//!
//! Total over [`LogEvent`]; every value is an integer, a literal from this
//! module's tables, or `BuildInfo`. Std only (the date math of `clock.rs`).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::SystemTime;

use super::event::LogEvent;

/// One line for `event` at `at`, local time = UTC + `seconds_east`
/// (`YYYY-MM-DDTHH:MM:SS.mmm+HH:MM`). Pure; no `'\n'` or `'\r'` inside and no
/// terminating newline (the [`Log`](super::Log) adds it).
pub fn format_line(at: SystemTime, seconds_east: i32, event: &LogEvent) -> String {
    // Skeleton (T-008 red tests): not implemented yet.
    let _ = (at, seconds_east, event);
    todo!("T-008: format_line")
}
