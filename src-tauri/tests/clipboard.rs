//! T-006: the clipboard adapter (`voicen_lib::win::clipboard::WinClipboard`; design 3,
//! research R-11, spec 001 FR-022, invariant 4). Windows CI only (decision #5).
//!
//! The clipboard is read back with the test's own `OpenClipboard` / `GetClipboardData`
//! (`win32_support`), never through the adapter. Runner capability: `clipboard`
//! (docs/decisions/windows-ci-runner.md, `ok` in runs A and B), asserted loudly by
//! `open_clipboard`. Fake texts only.
//!
//! Red-test table rows 8 (text and the three exclusion formats), 9 (a briefly held
//! clipboard is retried), 10 (held past the retries -> `ClipboardError`, nothing
//! written).
#![cfg(windows)]

mod win32_support;

use std::time::{Duration, Instant};

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
    // T-006 row 9 (R-11: OpenClipboard with up to 10 x 20 ms retries): another thread
    // holds the clipboard open for 60 ms when the write starts; the write still
    // succeeds. Bite: a single OpenClipboard attempt (Err while held).
    let _serial = serial();
    put_text("before the held write");
    let clipboard = WinClipboard::new();

    let holder = ClipboardHolder::hold(Duration::from_millis(60));
    let refused = try_open_clipboard().is_none();
    precondition(
        "clipboard",
        refused,
        "OpenClipboard succeeded while another thread held the clipboard open",
    );
    let result = clipboard.set_text_excluded_from_history("written after a short hold");
    holder.release();

    assert_eq!(result, Ok(()), "a clipboard held for 60 ms");
    assert_eq!(
        clipboard_text().as_deref(),
        Some("written after a short hold")
    );
}

#[test]
fn a_clipboard_held_past_the_retries_is_a_clipboard_error_and_nothing_is_written() {
    // T-006 row 10 (invariant 4; the pipeline turns it into
    // `Failed(ClipboardUnavailable)`): with the clipboard held for 3 s, the write gives
    // up with `ClipboardError` before the holder lets go (bounded, it never waits for
    // the holder), and the clipboard still holds what was there. Bite: an unbounded
    // retry loop (Ok once the holder lets go, or a hang), a panic or partial write.
    let _serial = serial();
    put_text("untouched by a failed write");
    let clipboard = WinClipboard::new();
    let hold = Duration::from_secs(3);

    let holder = ClipboardHolder::hold(hold);
    let started = Instant::now();
    let result = clipboard.set_text_excluded_from_history("must not be written");
    let took = started.elapsed();
    holder.release();

    assert_eq!(result, Err(ClipboardError), "held for {hold:?}");
    assert!(
        took < hold,
        "the write waited {took:?}, as long as the holder held the clipboard ({hold:?})"
    );
    assert_eq!(
        clipboard_text().as_deref(),
        Some("untouched by a failed write"),
        "the clipboard after a failed write"
    );
}
