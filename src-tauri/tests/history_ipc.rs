//! T-022 (review 1 #1, #3): the History window, its three commands and the history part's
//! start, over the shell's app wiring (`build_app`, mock runtime) with the part started by
//! `voicen_lib::history::start_history` over a fake clipboard (and, in one test, the real
//! `WinClipboard`). Windows CI only (decision #5): none of this runs on the Linux dev host, so
//! the first observation is the `ci.yml` run on `wip/T-022` after the implement stage.
//!
//! The guarantees (docs/tasks/T-022.md Acceptance 1 "copy puts the text in the clipboard",
//! Acceptance 2 "history turned off -> stored entries deleted, nothing more written"; spec 005
//! contracts/ipc.md, T013):
//! - (b) `history_copy { id }` writes the listed entry's full text once through the one
//!   clipboard writer (`set_text_excluded_from_history`); an unknown id is `{code:"not_found"}`
//!   with no write; a failing clipboard is `{code:"clipboard_unavailable"}` after one write.
//! - (c) a saved `history.enabled = false` before `start_history` leaves no `history.json*`
//!   in the given dir and a later delivery writes none; a save of `false` after the start
//!   reaches the follower thread (the shell half of P-013).
//! - (d) `open_history` twice gives one window labelled `history` at `/history`; the tray
//!   `OpenHistory` id routes to it. A closed window built again is `lifecycle.rs` (real
//!   runtime: the mock one never removes a destroyed window from tauri's manager).
//! - #3: a failed `start_history` writes one closed-tag line (below).
//!
//! Entries are made through the pipeline's own `TranscriptSink` (`history::transcript_sink`);
//! the pipeline-to-recorder wiring and the required sink are `history_wiring.rs`.
//!
//! Signatures this file assumes: only what exists today (`voicen_lib::history::{start_history,
//! transcript_sink, open_history, LABEL}`, the three commands, `tray::on_menu_event`), plus the
//! line pinned for #3 (the one expected red here):
//!
//! ```text
//! start_history, on every `Err` it returns after the Log is managed (already started, a
//! thread that cannot start), writes exactly one line
//!     <timestamp> WARN warning kind=history_start_failed [os_code=<n>]
//! i.e. core `LogEvent::Warning { kind: WarningKind::HistoryStartFailed, os_code }` (the
//! shape of `dictation_start_failed`): the closed tag and the OS code only, no path, no
//! error text. crates/voicen-core/tests/history_start_log.rs pins the core formatting.
//! ```
//!
//! Waits are on state through `win32_support` / `timing_support` (F-031); a bounded negative
//! ("no second window", "nothing written") can only pass on a stall (T1, reviewed residual).
#![cfg(windows)]

mod history_support;
mod timing_support;
mod win32_support;

use std::fs;
use std::sync::Arc;

use history_support::{get, id_of, invoke, ok, texts_of, HistoryRig, LEFTOVER};
use serde_json::json;
use tauri::menu::{MenuEvent, MenuId};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
use voicen_core::platform::Clipboard;
use voicen_core::tray::TrayAction;
use voicen_lib::history::{self, LABEL};
use voicen_lib::tray;
use voicen_lib::win::clipboard::WinClipboard;
use win32_support::{
    eventually, holds_for, put_text, read_clipboard, serial, ClipboardState, SETTLE,
};

/// A long, multi-line, non-ASCII text: a preview of it would differ from the full text.
const LONG: &str = concat!(
    "FULL-TEXT-a41 first line of the dictation, не превью\n",
    "second line\nthird line\nfourth line with more words than a preview keeps\n",
    "ccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    "ccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    "ccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    " THE-END"
);
const SHORT: &str = "SHORT-TEXT-b72";

fn menu_item(action: TrayAction) -> MenuEvent {
    MenuEvent {
        id: MenuId::new(action.as_str()),
    }
}

// ---- the commands over a delivered entry ----------------------------------------------------

#[test]
fn a_delivered_text_is_listed_newest_first_with_its_engine_and_stored() {
    // Acceptance 1 (T013): texts delivered to the pipeline's sink of the started app are what
    // `history_get` lists, newest first, in the IPC wire shape (camelCase, the engine label),
    // and what the file holds. Characterization of the shipped wiring (green today); it pins
    // that `transcript_sink` of a started app is the managed recorder. Bite: a sink that is
    // not the managed recorder (a second history, the keep-nothing sink); entries listed
    // oldest first; `ready_at_ms` snake-cased on the wire; the file never written.
    let _serial = serial();
    let rig = HistoryRig::started();
    rig.deliver(LONG);
    rig.deliver(SHORT);
    let window = rig.open_window();

    let view = get(&window);
    assert_eq!(texts_of(&view), vec![SHORT.to_string(), LONG.to_string()]);
    assert_eq!(view["enabled"], json!(true));
    assert_eq!(view["notice"], json!(null));
    let newest = &view["entries"][0];
    assert!(newest["readyAtMs"].is_i64(), "{newest}");
    assert_eq!(newest["engine"]["kind"], json!("api"), "{newest}");
    assert_eq!(newest["engine"]["model"], json!("whisper-1"), "{newest}");
    assert_eq!(rig.history_files(), vec!["history.json".to_string()]);
}

#[test]
fn copy_writes_exactly_the_entrys_full_text_once_through_the_one_clipboard_writer() {
    // Acceptance 1 (review 1 #1 b, I4): `history_copy { id }` answers null and the clipboard
    // port the part was given receives exactly one write with that entry's full text (the
    // older entry's too, not the newest's); the list is unchanged afterwards (a copy records
    // nothing). Bite: a command that copies the newest entry or the first row whatever the
    // id; the preview or a trimmed text instead of the full one; no write (a swallowed
    // call); two writes (a retry or a second writer); a copy that records itself.
    let _serial = serial();
    let rig = HistoryRig::started();
    rig.deliver(LONG);
    rig.deliver(SHORT);
    let window = rig.open_window();
    let view = get(&window);
    let (long_id, short_id) = (id_of(&view, LONG), id_of(&view, SHORT));

    assert_eq!(
        ok(&window, "history_copy", json!({ "id": long_id })),
        json!(null)
    );
    assert_eq!(rig.clipboard.texts(), vec![LONG.to_string()]);

    assert_eq!(
        ok(&window, "history_copy", json!({ "id": short_id })),
        json!(null)
    );
    assert_eq!(
        rig.clipboard.texts(),
        vec![LONG.to_string(), SHORT.to_string()],
        "one write per copy"
    );
    assert_eq!(
        texts_of(&get(&window)),
        vec![SHORT.to_string(), LONG.to_string()],
        "a copy changes no entry"
    );
}

#[test]
fn copy_of_an_unknown_id_is_not_found_and_writes_nothing() {
    // Acceptance 1 failure branch (contracts/ipc.md `history_copy`): an id that is not listed
    // (never issued, and the id of a cleared entry) answers exactly `{code:"not_found"}` and
    // the clipboard port is never called. Bite: an unknown id copying the newest entry or an
    // empty text; the error mapped to `clipboard_unavailable`; the error carrying text.
    let _serial = serial();
    let rig = HistoryRig::started();
    rig.deliver(SHORT);
    let window = rig.open_window();
    let known = id_of(&get(&window), SHORT);

    assert_eq!(
        invoke(&window, "history_copy", json!({ "id": known + 1000 })),
        Err(json!({ "code": "not_found" }))
    );
    ok(&window, "history_clear", json!({}));
    assert_eq!(
        invoke(&window, "history_copy", json!({ "id": known })),
        Err(json!({ "code": "not_found" })),
        "the id of a cleared entry"
    );
    assert_eq!(rig.clipboard.texts(), Vec::<String>::new(), "no write");
}

#[test]
fn copy_with_a_failing_clipboard_is_clipboard_unavailable_after_one_write() {
    // Acceptance 1 failure branch (swallowed error at the origin, one-shot fake): the
    // clipboard fails the one write; the command answers exactly
    // `{code:"clipboard_unavailable"}` (no text, no OS message), the clipboard was asked
    // once (no retry), the entry is still listed; with the clipboard back the next copy
    // answers null. Bite: `let _ = clipboard.set_text_excluded_from_history(..); Ok(())`;
    // the failure mapped to `not_found`; a retry (two writes); the entry dropped on failure.
    let _serial = serial();
    let rig = HistoryRig::started();
    rig.deliver(SHORT);
    let window = rig.open_window();
    let id = id_of(&get(&window), SHORT);
    rig.clipboard.set_fail(true);

    assert_eq!(
        invoke(&window, "history_copy", json!({ "id": id })),
        Err(json!({ "code": "clipboard_unavailable" }))
    );
    assert_eq!(
        rig.clipboard.texts(),
        vec![SHORT.to_string()],
        "one attempt"
    );
    assert_eq!(texts_of(&get(&window)), vec![SHORT.to_string()]);

    rig.clipboard.set_fail(false);
    assert_eq!(
        ok(&window, "history_copy", json!({ "id": id })),
        json!(null)
    );
    assert_eq!(rig.clipboard.texts().len(), 2);
}

#[test]
fn copy_through_the_real_clipboard_carries_the_text_and_the_three_exclusion_formats() {
    // Acceptance 1 (FR-10, spec 005 T013): with the real `WinClipboard` given to
    // `start_history`, the clipboard holds the entry's full text as CF_UNICODETEXT and the
    // three FR-10 formats (`ExcludeClipboardContentFromMonitorProcessing`,
    // `CanIncludeInClipboardHistory` = 0, `CanUploadToCloudClipboard` = 0), read back with
    // the test's own reader. Bite: a history copy that sets the text only; the part holding a
    // different writer than the one it was given; a format missing.
    let _serial = serial();
    put_text("an earlier owner's text");
    let rig = HistoryRig::unstarted();
    rig.start_with(Arc::new(WinClipboard::new()) as Arc<dyn Clipboard>)
        .expect("start_history");
    rig.deliver(LONG);
    let window = rig.open_window();
    let id = id_of(&get(&window), LONG);

    assert_eq!(
        ok(&window, "history_copy", json!({ "id": id })),
        json!(null)
    );
    assert_eq!(
        read_clipboard(),
        ClipboardState {
            text: Some(LONG.to_string()),
            exclude_from_monitor: true,
            can_include_in_history: Some(0),
            can_upload_to_cloud: Some(0),
        }
    );
}

// ---- off deletes, nothing more written ------------------------------------------------------

#[test]
fn a_saved_off_before_the_start_leaves_no_history_files_and_a_delivery_writes_none() {
    // Acceptance 2 (review 1 #1 c; C5 through the shell): `history.enabled = false` is saved
    // while `history.json` and `history.json.tmp` of an earlier run are in the dir; after
    // `start_history` the dir holds no `history.json*` (the leftovers are deleted, not
    // loaded), `history_get` is `{enabled:false, entries:[]}`, and a later delivery writes no
    // file and lists nothing. Bite: `start_history` opening with `enabled = true` (a constant,
    // or the defaults) instead of the saved snapshot; the snapshot read from another service;
    // a recorder that writes while off.
    let _serial = serial();
    let rig = HistoryRig::unstarted();
    fs::write(rig.data.join("history.json"), LEFTOVER).expect("leftover document");
    fs::write(rig.data.join("history.json.tmp"), LEFTOVER).expect("leftover tmp");
    rig.save_history_enabled(false);

    rig.start().expect("start_history");
    assert_eq!(rig.history_files(), Vec::<String>::new(), "leftovers kept");
    rig.deliver("NEVER-STORED-c90");
    let window = rig.open_window();

    let view = get(&window);
    assert_eq!(view["enabled"], json!(false), "{view}");
    assert_eq!(texts_of(&view), Vec::<String>::new());
    assert!(
        holds_for(SETTLE, || rig.history_files().is_empty()),
        "a delivery wrote {:?}",
        rig.history_files()
    );
}

#[test]
fn a_save_of_off_after_the_start_deletes_the_files_and_stops_the_writing() {
    // Acceptance 2 (review 1 #1 c; the shell half of P-013): with history on, one delivery
    // writes `history.json`; saving `history.enabled = false` through the real
    // `SettingsService::save` reaches the part's follower thread: the files go, the list is
    // empty and `enabled` false, and a later delivery writes nothing. Bite: the follower
    // thread never spawned or fed from a subscription taken after the save, a follower that
    // applies the inverted flag, `start_history` not calling `follow_settings` with the
    // service's own channel.
    let _serial = serial();
    let rig = HistoryRig::started();
    rig.deliver("STORED-THEN-DELETED-d11");
    assert_eq!(rig.history_files(), vec!["history.json".to_string()]);
    let window = rig.open_window();

    rig.save_history_enabled(false);

    assert!(
        eventually(|| rig.history_files().is_empty()),
        "files after the save: {:?}",
        rig.history_files()
    );
    let view = get(&window);
    assert_eq!(view["enabled"], json!(false), "{view}");
    assert_eq!(texts_of(&view), Vec::<String>::new());
    rig.deliver("NEVER-STORED-e22");
    assert!(
        holds_for(SETTLE, || rig.history_files().is_empty()),
        "a delivery after the off wrote {:?}",
        rig.history_files()
    );
    assert_eq!(texts_of(&get(&window)), Vec::<String>::new());
}

// ---- the window -----------------------------------------------------------------------------

#[test]
fn open_history_twice_gives_one_history_window_at_the_history_page() {
    // Acceptance 1 (review 1 #1 d, I2, T013): `open_history` builds the window labelled
    // `history` at `/history` on the opener thread; a second request makes no second window
    // (the count of windows and the label count stay). Bite: a window per request (a second
    // label, or a replaced window); the wrong label or page; no window at all.
    let _serial = serial();
    let rig = HistoryRig::started();
    assert!(rig.handle().get_webview_window(LABEL).is_none());

    let window = rig.open_window();
    assert_eq!(window.label(), LABEL);
    assert_eq!(window.url().expect("url").path(), "/history");
    let windows = rig.handle().webview_windows().len();

    history::open_history(rig.handle());
    assert!(
        holds_for(SETTLE, || rig.history_windows() == 1
            && rig.handle().webview_windows().len() == windows),
        "a second open made another window"
    );
}

#[test]
fn the_tray_history_item_opens_the_history_window_and_no_other() {
    // Acceptance 1 (review 1 #1 d): the tray's `OpenHistory` menu id routes to
    // `history::open_history`: the `history` window appears at `/history`, and neither the
    // Calls nor the Settings window is made. Bite: the item routed to `open_calls` or the
    // settings window, or to nothing (the id unknown to `on_menu_event`).
    let _serial = serial();
    let rig = HistoryRig::started();

    tray::on_menu_event(rig.handle(), menu_item(TrayAction::OpenHistory));

    let window = rig.wait_window();
    assert_eq!(window.url().expect("url").path(), "/history");
    assert!(
        holds_for(SETTLE, || rig
            .handle()
            .get_webview_window("calls")
            .is_none()
            && rig
                .handle()
                .get_webview_window(voicen_lib::settings_window::LABEL)
                .is_none()),
        "another window was opened"
    );
}

// "A closed window is built again" (T013 "close destroys it") is
// `lifecycle.rs::a_destroyed_history_window_is_built_again_by_the_next_open`, on the real Wry
// runtime: tauri drops a window from its manager only on the loop's `WindowEvent::Destroyed`,
// which the mock runtime never emits, so a destroyed mock window stays listed for good
// (Windows CI run 45649199635).

// ---- an app without a history part (characterization) ---------------------------------------

#[test]
fn an_app_without_a_history_part_answers_an_empty_list_and_not_found_and_opens_nothing() {
    // Characterization (green today; `build_app` alone manages no history part, as for the
    // call part): `open_history` opens no window; a `history` window made by hand gets the
    // empty list and `not_found` for a copy, no write, not a panic or a refusal of the
    // invoke. Bite: the commands taking `State<HistoryPart>` (they would refuse); `open_history`
    // building a window with nothing behind it.
    let _serial = serial();
    let rig = HistoryRig::unstarted();
    history::open_history(rig.handle());
    assert!(
        holds_for(SETTLE, || rig.handle().get_webview_window(LABEL).is_none()),
        "a window was opened without a history part"
    );
    let window = WebviewWindowBuilder::new(rig.handle(), LABEL, WebviewUrl::App("history".into()))
        .build()
        .expect("a history window");

    let view = get(&window);
    assert_eq!(texts_of(&view), Vec::<String>::new());
    assert_eq!(view["notice"], json!(null));
    assert_eq!(
        invoke(&window, "history_copy", json!({ "id": 1 })),
        Err(json!({ "code": "not_found" }))
    );
    assert_eq!(rig.clipboard.texts(), Vec::<String>::new());
}

// ---- a failed start is logged (review 1 #3) -------------------------------------------------

#[test]
fn a_failed_start_history_writes_one_closed_tag_line_and_leaves_the_first_part_working() {
    // Review 1 #3 (silent degradation; expected RED today: nothing is logged): a second
    // `start_history` on an app that already has a history part returns `Err` (kind
    // `AlreadyExists`) and writes exactly one line `WARN warning kind=history_start_failed`
    // (an OS code would be `os_code=<n>`; none here): no path (the history dir), no error
    // text; the first start wrote none. The first part stays the app's: entries delivered
    // before and after the failed start are both listed. Bite: nothing logged (today's
    // `let _ =` at the call site and no line inside); the error's text or the dir in the
    // line; one line per step (two lines); the second start replacing the first part (the
    // earlier entry lost, or the sink bound to the failed part).
    let _serial = serial();
    let rig = HistoryRig::started();
    assert_eq!(
        warning_lines(&rig),
        Vec::<String>::new(),
        "premise: the first start logged no history_start_failed"
    );
    rig.deliver("BEFORE-THE-FAILED-START");

    let other_dir = rig.rig.dir.path().join("other-history-dir");
    fs::create_dir_all(&other_dir).expect("other dir");
    let failed = history::start_history(
        rig.handle(),
        other_dir.clone(),
        Arc::new(voicen_core::platform::FakeClipboard::new()) as Arc<dyn Clipboard>,
    )
    .expect_err("a second start is refused");
    assert_eq!(failed.kind(), std::io::ErrorKind::AlreadyExists);

    assert!(
        eventually(|| !warning_lines(&rig).is_empty()),
        "no history_start_failed line after a failed start"
    );
    assert!(
        holds_for(SETTLE, || warning_lines(&rig).len() == 1),
        "want exactly one line: {:?}",
        warning_lines(&rig)
    );
    let line = warning_lines(&rig).remove(0);
    let words: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(
        words.get(1..).map(<[&str]>::to_vec),
        Some(vec!["WARN", "warning", "kind=history_start_failed"]),
        "closed tag only, no OS code for a non-OS error: {line}"
    );
    for forbidden in [
        other_dir.to_string_lossy().as_ref(),
        rig.data.to_string_lossy().as_ref(),
        "history part",
        "already",
    ] {
        assert!(!line.contains(forbidden), "{forbidden:?} in {line}");
    }

    rig.deliver("AFTER-THE-FAILED-START");
    let window = rig.open_window();
    assert_eq!(
        texts_of(&get(&window)),
        vec![
            "AFTER-THE-FAILED-START".to_string(),
            "BEFORE-THE-FAILED-START".to_string()
        ]
    );
    assert_eq!(
        history_support::history_files(&other_dir),
        Vec::<String>::new(),
        "the failed start wrote into its own dir"
    );
}

/// The `warning` lines of the log that carry `kind=history_start_failed`.
fn warning_lines(rig: &HistoryRig) -> Vec<String> {
    rig.lines_headed("warning")
        .into_iter()
        .filter(|l| {
            l.split_whitespace()
                .any(|w| w == "kind=history_start_failed")
        })
        .collect()
}
