//! T-022 (review 1 #3): the log line of a failed `start_history`, formatted by core.
//!
//! The shell's `start_history` writes one `LogEvent::Warning { kind:
//! WarningKind::HistoryStartFailed, os_code }` on every `Err` it returns (already started, a
//! thread that cannot start; `src-tauri/tests/history_ipc.rs` pins the shell side). This file
//! pins the closed tag, the level and that the line carries nothing else.
//!
//! Pinned API (the developer builds it): `diag::WarningKind::HistoryStartFailed`, literal
//! `history_start_failed`, WARN, formatted by the existing `warning kind=<kind> [os_code=<n>]`
//! arm. Adding the variant also makes the wildcard-free tables of `diag_format.rs`
//! (`all_warning_kinds`: the literal and the index match, count 20 -> 21) and `diag_support`
//! (`WARNING_KINDS`) fail to compile until they list it: the developer extends them in the
//! same change (that extension is not a weakening).
//!
//! Expected red today: `no variant HistoryStartFailed in WarningKind` (the missing API).

mod diag_support;

use diag_support::{at, parsed, NOON_UTC, WARNING_KINDS};
use voicen_core::diag::{format_line, LogEvent, WarningKind};

/// `"<LEVEL> <message>"` of an event: what follows the 29-character timestamp.
fn level_and_body(event: &LogEvent) -> String {
    let raw = format_line(at(NOON_UTC, 0), 0, event);
    let line = parsed(&raw);
    assert_eq!(line.ts.len(), 29, "{raw}");
    raw.get(30..)
        .unwrap_or_else(|| panic!("short line {raw:?}"))
        .to_string()
}

#[test]
fn a_failed_history_start_is_one_warn_line_with_the_closed_tag_and_no_os_code() {
    // Review 1 #3: no OS code, as a non-OS failure (already started) has none. Bite: a
    // literal swapped or misspelt (`history_failed` is the disk-error line, not this one),
    // INFO instead of WARN, a free-text field appended.
    assert_eq!(
        level_and_body(&LogEvent::Warning {
            kind: WarningKind::HistoryStartFailed,
            os_code: None,
        }),
        "WARN warning kind=history_start_failed"
    );
}

#[test]
fn a_failed_history_start_carries_the_os_code_and_nothing_else() {
    // Review 1 #3: a thread that cannot start has a raw OS code; the line holds the number
    // only (no OS text, no path: P-009). Bite: the code dropped; the code written as text.
    assert_eq!(
        level_and_body(&LogEvent::Warning {
            kind: WarningKind::HistoryStartFailed,
            os_code: Some(5),
        }),
        "WARN warning kind=history_start_failed os_code=5"
    );
}

#[test]
fn the_history_start_literal_is_in_the_closed_warning_set() {
    // Review 1 #3: the grammar check of every log line takes its closed values from
    // `diag_support::WARNING_KINDS`; the new literal belongs to that set, so a line with it
    // passes the project's closed-value validator. Bite: a literal in the formatter that the
    // closed set (and so the validator of the install smoke) does not know.
    assert!(
        WARNING_KINDS.contains(&"history_start_failed"),
        "WARNING_KINDS lacks history_start_failed"
    );
}
