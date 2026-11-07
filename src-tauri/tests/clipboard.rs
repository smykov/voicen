//! T-006: the clipboard adapter (`voicen_lib::win::clipboard::WinClipboard`; design 3,
//! research R-11, spec 001 FR-022, invariant 4). Windows CI only (decision #5).
//!
//! The clipboard is read back with the test's own `OpenClipboard` / `GetClipboardData`
//! (`win32_support`), never through the adapter. Runner capability: `clipboard`
//! (docs/decisions/windows-ci-runner.md, `ok` in runs A and B), asserted loudly by
//! `open_clipboard`. Rows 9 and 10 hold the clipboard with `ClipboardHolder` (its own
//! message-only window, not NULL: T-006 verify 1) and first assert that a second open is
//! refused while it holds, so neither can pass on a hold that blocks nobody. Fake texts
//! only.
//!
//! Red-test table rows 8 (text and the three exclusion formats), 9 (a briefly held
//! clipboard is retried), 10 (held past the retries -> `ClipboardError`, nothing
//! written).
#![cfg(windows)]

mod timing_support;
mod win32_support;

use std::time::Duration;

use timing_support::{at_least, measure, now, within_hang, HANG};
use voicen_core::platform::{Clipboard, ClipboardError};
use voicen_lib::win::clipboard::WinClipboard;
use win32_support::{
    clipboard_text, precondition, put_text, read_clipboard, serial, try_open_clipboard,
    ClipboardHolder, ClipboardState,
};

#[test]
fn the_text_is_written_with_the_three_history_exclusion_formats() {
    // T-006 row 8 (Acceptance line 1: "excluded from clipboard history"; FR-022, R-11):
    // after one write the clipboard holds the text as CF_UNICODETEXT (non-ASCII intact),
    // `ExcludeClipboardContentFromMonitorProcessing` is present, and
    // `CanIncludeInClipboardHistory` and `CanUploadToCloudClipboard` are the DWORD 0. A
    // second write replaces all of it. Bite: a format missing or set to 1, the text as
    // CF_TEXT (non-ASCII lost), no EmptyClipboard (the earlier owner's formats stay).
    let _serial = serial();
    put_text("an earlier owner's text");
    let clipboard = WinClipboard::new();

    for text in ["Voicen clipboard тест ✓ 42", "второй текст – second"] {
        assert_eq!(
            clipboard.set_text_excluded_from_history(text),
            Ok(()),
            "{text}"
        );
        assert_eq!(
            read_clipboard(),
            ClipboardState {
                text: Some(text.to_string()),
                exclude_from_monitor: true,
                can_include_in_history: Some(0),
                can_upload_to_cloud: Some(0),
            },
            "after writing {text:?}"
        );
    }
}

#[test]
fn a_briefly_held_clipboard_is_retried() {
    // T-006 row 9 (R-11: OpenClipboard with up to 10 x 20 ms retries, ~180-200 ms):
    // another thread holds the clipboard open for 120 ms; the write starts while it is
    // still held, succeeds, and took at least the part of the hold that was left when it
    // started (it really waited across the hold). The hold ends no earlier than
    // `asked + HOLD` (the holder opens after `asked` and holds HOLD from then), so the
    // hold left at the write's start is at least `asked + HOLD - write_start`. If the
    // runner was so slow that less than MIN_LEFT was left, that is a named precondition
    // failure, not a pass (review 1 finding #2). Bite: a single OpenClipboard attempt
    // (Err while held), too few or too short retries (Err before the holder lets go).
    const HOLD: Duration = Duration::from_millis(120);
    const MIN_LEFT: Duration = Duration::from_millis(60);
    const TICK: Duration = Duration::from_millis(5);
    let _serial = serial();
    put_text("before the held write");
    let clipboard = WinClipboard::new();

    let asked = now();
    let holder = ClipboardHolder::hold(HOLD);
    let refused = try_open_clipboard().is_none();
    precondition(
        "clipboard",
        refused,
        "OpenClipboard succeeded while another thread held the clipboard open",
    );
    let write_start = now();
    let left = (asked + HOLD).saturating_duration_since(write_start);
    precondition(
        "clipboard",
        left >= MIN_LEFT,
        format!(
            "the hold was (nearly) over before the write started: at most {left:?} of \
             {HOLD:?} left, need {MIN_LEFT:?} (runner too slow for this test)"
        ),
    );
    let (result, took) =
        measure(|| clipboard.set_text_excluded_from_history("written after a short hold"));
    holder.release();

    assert_eq!(result, Ok(()), "a clipboard held for {HOLD:?}");
    // The write took at least the hold left when it started (less one tick): it waited
    // across the hold.
    at_least(took, left.saturating_sub(TICK));
    assert_eq!(
        clipboard_text().as_deref(),
        Some("written after a short hold")
    );
}

#[test]
fn a_clipboard_held_past_the_retries_is_a_clipboard_error_and_nothing_is_written() {
    // T-006 row 10 (invariant 4; the pipeline turns it into
    // `Failed(ClipboardUnavailable)`): with the clipboard held until released (the hold
    // outlasts the hang bound), the write gives up with `ClipboardError` and returns while
    // the holder still holds (an ordering, T-201: no ceiling on the time), and the
    // clipboard still holds what was there. Bite: an unbounded retry loop (a hang), a
    // write that waits for the holder, a panic or partial write.
    let _serial = serial();
    put_text("untouched by a failed write");
    let hold = HANG * 2;

    let holder = ClipboardHolder::hold(hold);
    let refused = try_open_clipboard().is_none();
    precondition(
        "clipboard",
        refused,
        "OpenClipboard succeeded while another thread held the clipboard open",
    );
    let written =
        within_hang(|| WinClipboard::new().set_text_excluded_from_history("must not be written"));
    let still_held = try_open_clipboard().is_none();
    holder.release();

    let Some((result, took)) = written else {
        panic!("the write did not return within {HANG:?} while the clipboard was held")
    };
    assert_eq!(result, Err(ClipboardError), "held, the write took {took:?}");
    assert!(
        still_held,
        "the clipboard was free when the write returned (after {took:?}): it waited for \
         the holder"
    );
    assert_eq!(
        clipboard_text().as_deref(),
        Some("untouched by a failed write"),
        "the clipboard after a failed write"
    );
}
