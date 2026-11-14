//! T-128: the tray's call routes and call display (spec 007 FR-001, FR-003, FR-006; T028, T029;
//! decisions #197, #223, #277) over the shell's app wiring (`build_app`, mock runtime) with the
//! call part started by `voicen_lib::call::start_calls` over fakes. Windows CI only (decision #5).
//! The API these tests pin is in `call_shell_support/mod.rs`; `voicen_lib::call` and
//! `TrayPart::set_call_recording` do not exist yet, so this file fails to compile until they do
//! (that unresolved import is the red).
//!
//! The invariants (docs/tasks/T-128.md analysis): (I-a) the tray's Record, Stop and Calls items
//! only post `CallRequest`s; (I-b) the tray shows `table::shown(dictation, call_recording)` and
//! `table::menu(retry, call_recording)`, where `call_recording` is only what
//! `CallIndicator::set_call_recording` published, `menu_key` carries it, and the `CallRecord` arm
//! still bumps `menu_epoch` (#223).
#![cfg(windows)]

mod call_shell_support;
mod timing_support;

use std::sync::{Arc, Mutex, PoisonError};

use call_shell_support::{
    argv, eventually, holds_for_a_while, plain_opener, CallRig, Refusing, CWD, EXE,
};
use tauri::menu::{ContextMenu, MenuEvent, MenuId};
use tauri::test::MockRuntime;
use voicen_core::call::OpenError;
use voicen_core::i18n::{self, UiLanguage};
use voicen_core::recording::TrayState;
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::tray::{self as table, TrayAction, TrayRow};
use voicen_lib::tray::{self, Applied, AppliedRow};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumThreadWindows, GetClassNameW, GetMenuItemID, SendMessageW, HMENU, WM_COMMAND,
};

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

fn menu_item(action: TrayAction) -> MenuEvent {
    MenuEvent {
        id: MenuId::new(action.as_str()),
    }
}

/// What the tray shows for `(state, retry_available, call_recording)` in `lang`, by core's table.
fn expected(
    state: TrayState,
    retry_available: bool,
    lang: UiLanguage,
    call_recording: bool,
) -> Applied {
    Applied {
        state,
        retry_available,
        lang,
        icon: table::icon(state),
        tooltip: i18n::text(lang, table::tooltip(state), &[]),
        menu: applied_rows(&table::menu(retry_available, call_recording), lang),
    }
}

fn applied_rows(rows: &[TrayRow], lang: UiLanguage) -> Vec<AppliedRow> {
    rows.iter()
        .map(|row| match row {
            TrayRow::Item { action, enabled } => AppliedRow::Item {
                id: action.as_str().to_string(),
                text: i18n::text(lang, action.label(), &[]),
                enabled: *enabled,
            },
            TrayRow::Check {
                action,
                checked,
                enabled,
            } => AppliedRow::Check {
                id: action.as_str().to_string(),
                text: i18n::text(lang, action.label(), &[]),
                checked: *checked,
                enabled: *enabled,
            },
            TrayRow::Submenu { label, rows } => AppliedRow::Submenu {
                text: i18n::text(lang, *label, &[]),
                rows: applied_rows(rows, lang),
            },
        })
        .collect()
}

fn applied(rig: &CallRig) -> Option<Applied> {
    tray::applied(rig.handle())
}

#[test]
fn record_stop_and_the_switch_reach_the_same_session_path() {
    // AC4 (T028, T029; I-a): the tray's "Record" and "Stop", a second launch's `--call record`
    // and "Record" again on a recording call each end in the one session: a Record click starts
    // (`call_requested source=tray`), Stop stops (`call_stopped reason=user`), the switch starts
    // (`call_requested source=switch`) and a Record click on the recording call stops it (the
    // intent is read from the session, not from muda's mark). One `call_requested` per start,
    // nothing for a stop. "Calls" reaches `call::open_calls` once (proven as routing only: the
    // Calls window is T-142's). Bite: an arm that posts nothing (no line), Stop mapped to a
    // start or to Record, the click's intent taken from the mark (the second Record click would
    // start again), `OpenCalls` still an empty arm (`calls_opened` stays 0), a source mixed up.
    let _serial = serial();
    let (rig, source) = CallRig::with_wav();

    tray::on_menu_event(rig.handle(), menu_item(TrayAction::CallRecord));
    assert!(
        eventually(|| rig.recording()),
        "Record did not start a call"
    );
    assert_eq!(rig.count(&["call_requested", "source=tray"]), 1);
    source.pump();

    tray::on_menu_event(rig.handle(), menu_item(TrayAction::CallStop));
    assert!(
        eventually(|| !rig.recording()),
        "Stop did not stop the call"
    );
    assert_eq!(rig.count(&["call_stopped", "reason=user"]), 1);

    let posted = tray::on_second_instance(
        rig.handle(),
        argv(&[EXE, "--call", "record"]),
        CWD.to_string(),
    );
    assert!(posted.is_none());
    assert!(
        eventually(|| rig.recording()),
        "the switch did not start a call"
    );
    assert_eq!(rig.count(&["call_requested", "source=switch"]), 1);
    source.pump();

    tray::on_menu_event(rig.handle(), menu_item(TrayAction::CallRecord));
    assert!(
        eventually(|| !rig.recording()),
        "a Record click on a recording call did not stop it"
    );
    assert_eq!(rig.count(&["call_stopped", "reason=user"]), 2);

    tray::on_menu_event(rig.handle(), menu_item(TrayAction::OpenCalls));
    assert!(
        eventually(|| voicen_lib::call::calls_opened(rig.handle()) == 1),
        "Calls did not reach call::open_calls"
    );
    assert!(
        holds_for_a_while(
            || rig.count(&["call_requested"]) == 2 && rig.count(&["call_stopped"]) == 2
        ),
        "a click wrote an extra request or stop line"
    );
}

#[test]
fn a_call_in_progress_shows_call_recording() {
    // AC7 (FR-006; I-b): a recording call shows `CallRecording` (icon, tooltip, Record checked,
    // Stop enabled), through the tray's `table::shown`; a dictation `Recording` still wins over
    // it, and when the dictation goes back to `Idle` the call is still shown; the end of the
    // call returns the tray to `Idle` with Record unchecked and Stop disabled. Bite: the tray
    // rendering `table::icon(view.state)` of the dictation state alone (the first expectation
    // reads `Idle`), `shown` with the arguments swapped, the flag not reset on a stop.
    let _serial = serial();
    let (rig, source) = CallRig::with_wav();
    assert_eq!(
        applied(&rig),
        Some(expected(TrayState::Idle, false, UiLanguage::En, false)),
        "premise"
    );

    voicen_lib::call::request(
        rig.handle(),
        voicen_lib::call::CallRequest::Record(voicen_core::call::CallSource::Tray),
    );

    assert!(
        eventually(|| applied(&rig)
            == Some(expected(
                TrayState::CallRecording,
                false,
                UiLanguage::En,
                true
            ))),
        "a call in progress is not shown: {:?}",
        applied(&rig)
    );
    let part = tray::part(rig.handle()).expect("the tray part");

    part.set_tray(TrayState::Recording, false);
    assert!(
        eventually(|| applied(&rig).is_some_and(|a| a.state == TrayState::Recording)),
        "a dictation recording did not win over the call: {:?}",
        applied(&rig)
    );

    part.set_tray(TrayState::Idle, false);
    assert!(
        eventually(|| applied(&rig)
            == Some(expected(
                TrayState::CallRecording,
                false,
                UiLanguage::En,
                true
            ))),
        "the call is not shown again once the dictation is idle: {:?}",
        applied(&rig)
    );

    source.pump();
    voicen_lib::call::request(
        rig.handle(),
        voicen_lib::call::CallRequest::Stop(voicen_core::call::CallSource::Tray),
    );
    assert!(
        eventually(
            || applied(&rig) == Some(expected(TrayState::Idle, false, UiLanguage::En, false))
        ),
        "the tray still shows a call that ended: {:?}",
        applied(&rig)
    );
}

#[test]
fn a_change_of_the_call_flag_rebuilds_the_menu() {
    // AC8 (#197 D2, D3; I-b): the call flag the indicator publishes is part of what the tray
    // wants and of the key of the menu on the icon, so a change of the flag alone (language,
    // Retry and the epoch unchanged) rebuilds the menu from core's table: Record checked and
    // Stop enabled with the flag on, unchecked and disabled again with it off. `Applied.menu` and
    // the live check item both show it. Bite: no flag in `View` / `menu_key` (the menu stays as
    // it was: Record unchecked, Stop disabled), the flag rebuilt only when the epoch moves, a
    // menu built with `call_recording = false` whatever the flag.
    let _serial = serial();
    let (rig, _source) = CallRig::with_wav();
    let part = tray::part(rig.handle()).expect("the tray part");
    assert_eq!(
        applied(&rig),
        Some(expected(TrayState::Idle, false, UiLanguage::En, false)),
        "premise"
    );

    part.set_call_recording(true);

    assert!(
        eventually(|| applied(&rig)
            .is_some_and(|a| a.menu == applied_rows(&table::menu(false, true), UiLanguage::En))),
        "the menu did not follow the call flag: {:?}",
        applied(&rig).map(|a| a.menu)
    );
    assert_eq!(
        applied(&rig),
        Some(expected(
            TrayState::CallRecording,
            false,
            UiLanguage::En,
            true
        ))
    );
    let (_, record) = live_record_item(&rig);
    assert!(
        record.is_checked().expect("is_checked"),
        "live Record is unchecked"
    );

    part.set_call_recording(false);

    assert!(
        eventually(
            || applied(&rig) == Some(expected(TrayState::Idle, false, UiLanguage::En, false))
        ),
        "the menu did not return with the flag: {:?}",
        applied(&rig)
    );
    let (_, record) = live_record_item(&rig);
    assert!(
        !record.is_checked().expect("is_checked"),
        "live Record stays checked"
    );
}

/// Collects the `tray_icon_app` windows of the enumerated thread into the `Vec<isize>` behind
/// `lparam`.
unsafe extern "system" fn collect_tray_windows(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let mut buf = [0u16; 64];
    // SAFETY: `buf` is writable; GetClassNameW sends no message.
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    let class = String::from_utf16_lossy(&buf[..usize::try_from(len).unwrap_or(0).min(buf.len())]);
    if class == "tray_icon_app" {
        // SAFETY: `lparam` is the `&mut Vec<isize>` of `tray_windows`, alive for the whole
        // synchronous enumeration.
        unsafe { (*(lparam.0 as *mut Vec<isize>)).push(hwnd.0 as isize) };
    }
    BOOL(1)
}

/// The `tray_icon_app` windows (tray-icon's class) of this thread.
fn tray_windows() -> Vec<isize> {
    let mut windows: Vec<isize> = Vec::new();
    // SAFETY: the callback only reads class names and pushes into `windows`, which outlives this
    // synchronous call.
    let _ = unsafe {
        EnumThreadWindows(
            GetCurrentThreadId(),
            Some(collect_tray_windows),
            LPARAM(&mut windows as *mut Vec<isize> as isize),
        )
    };
    windows
}

/// The live "Record" check item and its submenu: `live_menu` > submenu > check item.
fn live_record_item(
    rig: &CallRig,
) -> (
    tauri::menu::Submenu<MockRuntime>,
    tauri::menu::CheckMenuItem<MockRuntime>,
) {
    let menu = tray::live_menu(rig.handle()).expect("the menu last set on the tray icon");
    for kind in menu.items().expect("menu items") {
        if let Some(submenu) = kind.as_submenu() {
            for inner in submenu.items().expect("submenu items") {
                if let Some(check) = inner.as_check_menuitem() {
                    return (submenu.clone(), check.clone());
                }
            }
        }
    }
    panic!("no Call submenu with a check item in the live menu");
}

#[test]
fn a_refused_record_click_clears_the_mark_muda_flipped() {
    // AC9 (#223; I-b): muda flips a check item's native mark on WM_COMMAND before it sends the
    // `MenuEvent`; a refused start publishes no flag, so only the `CallRecord` arm's forced
    // rebuild can undo the flip. Order: premise unchecked; flip it for real with WM_COMMAND (the
    // mock drops the resulting MenuEvent) and check the flip; run the handler over a source
    // that refuses (the system track has no device); once the refusal is logged and a main
    // thread pass ran, the live Record item is unchecked and the tray shows the table's Idle.
    // Bite: the arm without `menu_epoch` bump (the item stays checked: nothing else rebuilds
    // the menu), a rebuild that keeps the old Menu in `live_menu`.
    let _serial = serial();
    let rig = CallRig::started(
        Arc::new(Refusing(OpenError::NoDevice)),
        FakeDisk::with_available(100_000_000_000),
        plain_opener(),
    );
    let (submenu, record) = live_record_item(&rig);
    assert!(
        !record.is_checked().expect("is_checked"),
        "premise: unchecked"
    );

    let hmenu = submenu.hpopupmenu().expect("the submenu's HMENU");
    // SAFETY: the HMENU is valid as long as `submenu` is; position 0 is Record.
    let command_id = unsafe { GetMenuItemID(HMENU(hmenu as *mut std::ffi::c_void), 0) };
    let windows = tray_windows();
    let hwnd = *windows.first().expect("runner precondition: a tray window");
    // SAFETY: a window of this thread; muda's subclass procedure runs inline.
    let _ = unsafe {
        SendMessageW(
            HWND(hwnd as *mut std::ffi::c_void),
            WM_COMMAND,
            Some(WPARAM(command_id as usize)),
            Some(LPARAM(0)),
        )
    };
    assert!(
        record.is_checked().expect("is_checked"),
        "premise: muda's click flip did not mark Record"
    );

    tray::on_menu_event(rig.handle(), menu_item(TrayAction::CallRecord));

    assert!(
        eventually(|| rig.count(&["call_refused", "part=system", "reason=no_device"]) == 1),
        "the refused start never ran"
    );
    assert!(
        eventually(|| {
            let (_, after) = live_record_item(&rig);
            !after.is_checked().expect("is_checked")
        }),
        "Record stays checked after a refused start"
    );
    assert_eq!(
        applied(&rig),
        Some(expected(TrayState::Idle, false, UiLanguage::En, false)),
        "the rebuilt menu differs from the table"
    );
}
