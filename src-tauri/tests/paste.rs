//! T-006: the paste adapter (`voicen_lib::win::paste::WinPaster`; design 4, research
//! R-12, spec 001 FR-009/FR-010, invariant 2) and its use by core's `delivery::deliver`
//! with the real `WinClipboard`. Windows CI only (decision #5).
//!
//! Runner capabilities (docs/decisions/windows-ci-runner.md): `foreground_again` (the test
//! windows are brought to the front through the injected-Alt path, re-measured by the
//! probe in every job; only the first window of a test exe has foreground rights without
//! it, T-006 verify 1), `sendinput` and `async_keys` (row 15 and the Alt path),
//! `clipboard` (`ok` in runs A and B). Each is asserted loudly by `win32_support`. Every
//! wait is at least 3 s (run B: ~1 s for injected input).
//!
//! Red-test table rows 11 (start window = root owner, not elevated at our level), 12
//! (Ctrl+V reaches the start window), 13 (another window in front -> copy-manual, nothing
//! pasted), 14 (`is_in_front` checks the process too; a closed window is not in front),
//! 15 (the modifier wait is bounded and polls both Win keys).
#![cfg(windows)]

mod win32_support;

use std::thread;
use std::time::{Duration, Instant};

use voicen_core::delivery::{self, DeliveryResult};
use voicen_core::platform::{Paster, StartWindow, WindowRef};
use voicen_lib::win::clipboard::WinClipboard;
use voicen_lib::win::paste::WinPaster;
use win32_support::{
    bring_to_front, clipboard_text, eventually, holds_for, put_text, serial, window_ref, Keys,
    Shape, TestWindow, SETTLE, VK_CONTROL, VK_MASK, VK_RWIN, WAIT,
};

#[test]
fn the_start_window_is_the_root_owner_of_the_foreground_window_and_not_elevated() {
    // T-006 row 11 (data-model "StartWindow": root owner, process id, elevated): with a
    // popup owned by window A in front, the start window is A (GA_ROOTOWNER), with this
    // process's id, and not elevated (the same integrity level as ours). Bite: the
    // foreground window itself instead of its root owner (the popup), the thread id or 0
    // as the process id, a reversed or unknown-level comparison (elevated true).
    let _serial = serial();
    let windows = TestWindow::open(Shape::WithOwnedPopup);
    bring_to_front(windows.popup());
    let paster = WinPaster::new();

    let start = paster.capture_start_window();

    assert_eq!(
        start,
        Some(StartWindow {
            handle: WindowRef(window_ref(windows.hwnd())),
            process_id: std::process::id(),
            elevated: false,
        }),
        "popup {:?} owned by {:?} in front",
        windows.popup(),
        windows.hwnd()
    );
}

#[test]
fn ctrl_v_pastes_the_clipboard_into_the_start_window() {
    // T-006 row 12 (FR-010: Ctrl+V into the start window): with the clipboard set and
    // the test window in front, `send_ctrl_v` makes the EDIT paste the clipboard text.
    // Bite: the wrong keys (V without Ctrl, a VK_PASTE), keys sent one by one with a
    // gap, Ctrl left down.
    let _serial = serial();
    let window = TestWindow::open(Shape::TopLevel);
    window.front();
    put_text("pasted by T-006 – вставка");
    let paster = WinPaster::new();

    assert_eq!(paster.send_ctrl_v(), Ok(()));

    assert!(
        eventually(WAIT, || window.text() == "pasted by T-006 – вставка"),
        "the window text after Ctrl+V: {:?}",
        window.text()
    );
    assert!(
        eventually(WAIT, || !win32_support::is_down(VK_CONTROL)),
        "Ctrl is still down after send_ctrl_v"
    );
}

#[test]
fn another_window_in_front_gives_copy_manual_and_pastes_nothing() {
    // T-006 row 13 (Acceptance line 3, "start window replaced by another before
    // delivery"; FR-009): the start window is captured with A in front, then B comes to
    // the front; `deliver` with auto-paste returns CopyManual, the text is in the
    // clipboard, and neither A nor B received it. Bite: `is_in_front` true for any
    // window (Ctrl+V lands in B), a paste into A by focusing it (A gets the text).
    let _serial = serial();
    let a = TestWindow::open(Shape::TopLevel);
    let b = TestWindow::open(Shape::TopLevel);
    a.front();
    let paster = WinPaster::new();
    let start = paster
        .capture_start_window()
        .expect("premise: a start window with A in front");
    assert_eq!(
        start.handle,
        WindowRef(window_ref(a.hwnd())),
        "premise: A captured"
    );
    b.front();

    let result = delivery::deliver(
        "copy-manual text T-006",
        true,
        Some(&start),
        &WinClipboard::new(),
        &paster,
    );

    assert_eq!(result, Ok(DeliveryResult::CopyManual));
    assert!(
        holds_for(SETTLE, || a.text().is_empty() && b.text().is_empty()),
        "pasted: A {:?}, B {:?}",
        a.text(),
        b.text()
    );
    assert_eq!(clipboard_text().as_deref(), Some("copy-manual text T-006"));
}

#[test]
fn is_in_front_checks_the_window_and_its_process_and_a_closed_window_is_not_in_front() {
    // T-006 row 14 (R-12: a reused HWND fails the process check): the captured start
    // window is in front; the same handle with another process id is not; after the
    // window is destroyed, it is not in front. Bite: the HWND compared alone, a constant
    // answer (true or false).
    let _serial = serial();
    let a = TestWindow::open(Shape::TopLevel);
    a.front();
    let paster = WinPaster::new();
    let start = paster
        .capture_start_window()
        .expect("premise: a start window with A in front");

    assert!(paster.is_in_front(&start), "A in front: {start:?}");
    let other_process = StartWindow {
        process_id: start.process_id.wrapping_add(4),
        ..start.clone()
    };
    assert!(
        !paster.is_in_front(&other_process),
        "the same HWND with another process id counted as in front"
    );

    drop(a);
    assert!(
        !paster.is_in_front(&start),
        "a destroyed start window counted as in front"
    );
}

#[test]
fn the_modifier_wait_is_bounded_and_polls_ctrl_and_the_right_win_key() {
    // T-006 row 15 (data-model "DeliveryDecision": wait <= 1 s for Shift/Ctrl/Alt/Win):
    // with Ctrl held, `wait_modifiers_released(1 s)` returns false after at least 1 s
    // and well before the 3 s budget; with the right Win key held the same; with
    // everything up it returns true at once. Bite: an unbounded wait, an immediate
    // false (no wait), only the hotkey's modifiers or only VK_LWIN polled (RWIN held
    // reads as released).
    let _serial = serial();
    let window = TestWindow::open(Shape::TopLevel);
    window.front();
    let paster = WinPaster::new();
    let max = Duration::from_secs(1);

    for vk in [VK_CONTROL, VK_RWIN] {
        let mut keys = Keys::press(&[vk]);
        let started = Instant::now();
        let released = paster.wait_modifiers_released(max);
        let took = started.elapsed();
        // The mask key first: a Win key released alone would open the Start menu.
        win32_support::send(&[
            win32_support::key(VK_MASK, false),
            win32_support::key(VK_MASK, true),
        ]);
        keys.release(&[vk]);
        assert!(!released, "{vk:#04x} held: reported released");
        assert!(
            took >= max,
            "{vk:#04x} held: gave up after {took:?}, before {max:?}"
        );
        assert!(took < WAIT, "{vk:#04x} held: waited {took:?}");
    }

    thread::sleep(Duration::from_millis(100));
    let started = Instant::now();
    assert!(
        paster.wait_modifiers_released(max),
        "nothing held: reported held"
    );
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "nothing held: took {:?}",
        started.elapsed()
    );
}
