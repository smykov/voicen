//! The tray icon's menu, tooltips and icons (T-052; spec 001 FR-001, FR-008 /
//! FR-028; spec 004 FR-013; spec 007 FR-003, FR-006; contracts/messages.md `tray.*`).
//!
//! The one table the shell's tray adapter (`src-tauri/src/tray.rs`) renders: which
//! rows the menu has and in which order (with their mark and enabled flag), the id
//! each item carries (its `MenuEvent` id), the catalog id of each row's text and of
//! each state's tooltip, and which of the four icons a state shows. The shell renders
//! the texts with [`i18n::text`](crate::i18n::text) in the settings' `ui_language` and
//! maps a clicked id back with [`TrayAction::from_id`]; it decides nothing itself
//! (P-010).
//!
//! The dictation side of the tray's state is the controller's ([`TrayState`], derived
//! by `RecordingController`, published by the dictation session together with
//! `retry_available`); [`shown`] combines it with the call session's recording flag.
//! Nothing here keeps state.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use crate::i18n::{self, MessageId};
use crate::recording::TrayState;

/// An item of the tray menu. T-007 adds Retry (only while `retry_available`),
/// feature 005 History.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrayAction {
    /// "Call" > "Record": the check item that starts or stops recording a call
    /// (spec 007 FR-003).
    CallRecord,
    /// "Call" > "Stop": stops the call recording.
    CallStop,
    /// "Calls": opens the calls list.
    OpenCalls,
    /// "Settings": the settings window to the front on the tab it shows, or opened
    /// on Engine when none is open (OQ-11 Q2 default).
    OpenSettings,
    /// "Open logs folder": the logs folder in Explorer, created when missing
    /// (T-071; spec 006 FR-017). The shell opens it off the main thread.
    OpenLogs,
    /// "Exit": ends the process (the only user exit, T-052 invariant 4).
    Exit,
}

impl TrayAction {
    /// The menu item id (`MenuEvent.id`): a closed set of `[a-z_]+` words, one per
    /// action.
    pub fn as_str(self) -> &'static str {
        match self {
            TrayAction::CallRecord => "call_record",
            TrayAction::CallStop => "call_stop",
            TrayAction::OpenCalls => "calls",
            TrayAction::OpenSettings => "settings",
            TrayAction::OpenLogs => "open_logs",
            TrayAction::Exit => "exit",
        }
    }

    /// The action whose [`as_str`](Self::as_str) is `id`; `None` for any other id.
    pub fn from_id(id: &str) -> Option<TrayAction> {
        ALL.into_iter().find(|action| action.as_str() == id)
    }

    /// The catalog id of the item's text (contracts/messages.md).
    pub fn label(self) -> MessageId {
        match self {
            TrayAction::CallRecord => i18n::TRAY_CALL_RECORD,
            TrayAction::CallStop => i18n::TRAY_CALL_STOP,
            TrayAction::OpenCalls => i18n::TRAY_CALLS,
            TrayAction::OpenSettings => i18n::TRAY_SETTINGS,
            TrayAction::OpenLogs => i18n::TRAY_OPEN_LOGS,
            TrayAction::Exit => i18n::TRAY_EXIT,
        }
    }
}

/// Every action, so [`TrayAction::from_id`] maps back exactly the ids
/// [`TrayAction::as_str`] gives (a new variant is added here and to `as_str`).
const ALL: [TrayAction; 6] = [
    TrayAction::CallRecord,
    TrayAction::CallStop,
    TrayAction::OpenCalls,
    TrayAction::OpenSettings,
    TrayAction::OpenLogs,
    TrayAction::Exit,
];

/// The four tray icons (OQ-11 Q1 default: the app icon plain, with a red dot, with
/// an amber "!", with an amber "!" and a key mark). The shell embeds one image per
/// kind. `TrayState::CallRecording` shares the red dot (decision #197 D4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrayIconKind {
    Idle,
    Recording,
    Error,
    HotkeyError,
}

/// A row of the tray menu.
#[derive(Debug, Clone, PartialEq)]
pub enum TrayRow {
    /// A plain item.
    Item { action: TrayAction, enabled: bool },
    /// An item with a check mark.
    Check {
        action: TrayAction,
        checked: bool,
        enabled: bool,
    },
    /// A submenu; it carries no action, so no click maps to it.
    Submenu {
        label: MessageId,
        rows: Vec<TrayRow>,
    },
}

/// The menu rows, top to bottom (decision #197 D2, D3): "Call" > {"Record", checked
/// iff `call_recording`; "Stop", enabled iff `call_recording`}, "Calls", Settings,
/// Open logs folder, Exit last. `retry_available` is the session's (the pending slot,
/// dictation-session.md); T-007 adds Retry while it is true. Pure.
pub fn menu(retry_available: bool, call_recording: bool) -> Vec<TrayRow> {
    // T-007 inserts Retry before Exit while `retry_available`; until then both rows
    // are the same.
    let _ = retry_available;
    let item = |action| TrayRow::Item {
        action,
        enabled: true,
    };
    vec![
        TrayRow::Submenu {
            label: i18n::TRAY_CALL,
            rows: vec![
                TrayRow::Check {
                    action: TrayAction::CallRecord,
                    checked: call_recording,
                    enabled: true,
                },
                TrayRow::Item {
                    action: TrayAction::CallStop,
                    enabled: call_recording,
                },
            ],
        },
        item(TrayAction::OpenCalls),
        item(TrayAction::OpenSettings),
        item(TrayAction::OpenLogs),
        item(TrayAction::Exit),
    ]
}

/// The state the tray shows for the dictation session's state and the call session's
/// recording flag (spec 007 FR-006): dictation `HotkeyError` > dictation `Recording` >
/// `CallRecording` while `call_recording` > dictation `Error` > `Idle`. `HotkeyError`
/// is shown iff `dictation` is `HotkeyError`, so no call-side input can produce it
/// (FR-004). The dictation controller never produces `CallRecording`; passed in with
/// the flag off, it shows `Idle`. Pure.
pub fn shown(dictation: TrayState, call_recording: bool) -> TrayState {
    match (dictation, call_recording) {
        (TrayState::HotkeyError, _) => TrayState::HotkeyError,
        (TrayState::Recording, _) => TrayState::Recording,
        (_, true) => TrayState::CallRecording,
        (TrayState::Error, false) => TrayState::Error,
        (TrayState::Idle | TrayState::CallRecording, false) => TrayState::Idle,
    }
}

/// The catalog id of the tooltip `state` shows. Pure.
pub fn tooltip(state: TrayState) -> MessageId {
    match state {
        TrayState::Idle => i18n::TRAY_TOOLTIP_IDLE,
        TrayState::Recording => i18n::TRAY_TOOLTIP_RECORDING,
        TrayState::CallRecording => i18n::TRAY_TOOLTIP_CALL_RECORDING,
        TrayState::Error => i18n::TRAY_TOOLTIP_ERROR,
        TrayState::HotkeyError => i18n::TRAY_TOOLTIP_HOTKEY_ERROR,
    }
}

/// The icon `state` shows. Pure.
pub fn icon(state: TrayState) -> TrayIconKind {
    match state {
        TrayState::Idle => TrayIconKind::Idle,
        TrayState::Recording | TrayState::CallRecording => TrayIconKind::Recording,
        TrayState::Error => TrayIconKind::Error,
        TrayState::HotkeyError => TrayIconKind::HotkeyError,
    }
}

#[cfg(test)]
mod tests {
    //! T-052 red-test table row 1 (core half; Linux gate). Expected ids and texts are
    //! spelled here from contracts/messages.md (lines 32-40), not derived from the
    //! code under test. The catalog side of every id is also checked by
    //! `i18n::tests::message_ids_exist_in_both_catalogs` through `MESSAGE_IDS`.

    use std::collections::BTreeSet;

    use serde_json::Value;

    use super::*;
    use crate::i18n::{self, UiLanguage, MESSAGE_IDS};

    /// Every tray state, by a wildcard-free match: a new variant fails to compile
    /// here until it is added to the tables below.
    fn all_states() -> [TrayState; 5] {
        let all = [
            TrayState::Idle,
            TrayState::Recording,
            TrayState::CallRecording,
            TrayState::Error,
            TrayState::HotkeyError,
        ];
        for state in all {
            match state {
                TrayState::Idle
                | TrayState::Recording
                | TrayState::CallRecording
                | TrayState::Error
                | TrayState::HotkeyError => {}
            }
        }
        all
    }

    /// Every menu action, by a wildcard-free match (as above).
    fn all_actions() -> [TrayAction; 6] {
        let all = [
            TrayAction::CallRecord,
            TrayAction::CallStop,
            TrayAction::OpenCalls,
            TrayAction::OpenSettings,
            TrayAction::OpenLogs,
            TrayAction::Exit,
        ];
        for action in all {
            match action {
                TrayAction::CallRecord
                | TrayAction::CallStop
                | TrayAction::OpenCalls
                | TrayAction::OpenSettings
                | TrayAction::OpenLogs
                | TrayAction::Exit => {}
            }
        }
        all
    }

    /// The id string of `id` (its wire form; the field is private to `i18n`).
    fn id_str(id: MessageId) -> String {
        match serde_json::to_value(id) {
            Ok(Value::String(s)) => s,
            other => panic!("a MessageId serialized as {other:?}"),
        }
    }

    /// The expected menu of D2/D3 (decision #197), spelled out from the spec, not
    /// derived from the code under test.
    fn want_menu(call_recording: bool) -> Vec<TrayRow> {
        vec![
            TrayRow::Submenu {
                label: i18n::TRAY_CALL,
                rows: vec![
                    TrayRow::Check {
                        action: TrayAction::CallRecord,
                        checked: call_recording,
                        enabled: true,
                    },
                    TrayRow::Item {
                        action: TrayAction::CallStop,
                        enabled: call_recording,
                    },
                ],
            },
            TrayRow::Item {
                action: TrayAction::OpenCalls,
                enabled: true,
            },
            TrayRow::Item {
                action: TrayAction::OpenSettings,
                enabled: true,
            },
            TrayRow::Item {
                action: TrayAction::OpenLogs,
                enabled: true,
            },
            TrayRow::Item {
                action: TrayAction::Exit,
                enabled: true,
            },
        ]
    }

    #[test]
    fn menu_is_call_submenu_calls_settings_open_logs_then_exit_for_every_input_pair() {
        // Spec 007 FR-003, decision #197 D2/D3: Submenu "Call" {Record = Check,
        // checked iff call_recording; Stop = Item, enabled iff call_recording},
        // "Calls", Settings, Open logs folder, Exit last; for both values of
        // retry_available (T-007 changes the `true` rows) and of call_recording.
        // Bite: Record's mark inverted or constant, Stop always enabled, the order
        // changed (Exit no longer last), a row missing, Record outside the submenu,
        // the call input ignored.
        for retry in [false, true] {
            for call in [false, true] {
                assert_eq!(
                    menu(retry, call),
                    want_menu(call),
                    "retry_available = {retry}, call_recording = {call}"
                );
            }
        }
    }

    #[test]
    fn menu_item_ids_are_distinct_words_that_map_back_to_their_action() {
        // The shell gives each item `as_str` as its MenuEvent id and maps a click
        // back with `from_id` (one table, P-010). Bite: an empty id or one with a
        // character outside [a-z_], two items sharing an id (a Settings click
        // exiting), `from_id` mapping an id to another action, or an unknown id
        // (another task's item, a stray event) mapped to an action.
        let ids: Vec<&str> = all_actions().iter().map(|a| a.as_str()).collect();
        for (action, id) in all_actions().iter().zip(&ids) {
            assert!(
                !id.is_empty() && id.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{action:?}: {id:?} is not a [a-z_]+ id"
            );
            assert_eq!(TrayAction::from_id(id), Some(*action), "{action:?}: {id:?}");
        }
        let unique: BTreeSet<&str> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "ids not distinct: {ids:?}");
        for unknown in [
            "",
            "Exit",
            "SETTINGS",
            "OPEN_LOGS",
            "open_logs_folder",
            "logs",
            "retry",
            "history",
            "call",
            "record",
            "stop",
            "CALLS",
            " exit",
        ] {
            assert_eq!(
                TrayAction::from_id(unknown),
                None,
                "{unknown:?} is not an item of the menu"
            );
        }
    }

    #[test]
    fn the_open_logs_item_has_id_open_logs_and_maps_back() {
        // T-071: the Open logs folder item carries the MenuEvent id `open_logs` (the
        // word the shell's `on_menu_event` maps back, and the id T-052's tests held
        // as "another task's item"), and that id maps back to OpenLogs only. Bite:
        // another spelling (`logs`, `open_logs_folder`), or `from_id("open_logs")`
        // missing from ALL (the click would do nothing).
        assert_eq!(TrayAction::OpenLogs.as_str(), "open_logs");
        assert_eq!(TrayAction::from_id("open_logs"), Some(TrayAction::OpenLogs));
    }

    #[test]
    fn menu_item_labels_are_the_contract_ids_and_declared() {
        // contracts/messages.md:32, :36 and the tray.open_logs row (T-071). Declared in MESSAGE_IDS, so the catalog
        // test checks both catalogs. Bite: a wrong id (`tray.open_settings`), the
        // two items swapped, or an id built outside the `messages!` list.
        let table = [
            (TrayAction::CallRecord, "tray.call.record"),
            (TrayAction::CallStop, "tray.call.stop"),
            (TrayAction::OpenCalls, "tray.calls"),
            (TrayAction::OpenSettings, "tray.settings"),
            (TrayAction::OpenLogs, "tray.open_logs"),
            (TrayAction::Exit, "tray.exit"),
        ];
        assert_eq!(table.map(|(a, _)| a), all_actions());
        for (action, expected) in table {
            let label = action.label();
            assert_eq!(id_str(label), expected, "{action:?}");
            assert!(
                MESSAGE_IDS.contains(&label),
                "{expected} is not in MESSAGE_IDS"
            );
        }
    }

    #[test]
    fn each_state_has_its_own_tooltip_id_from_the_contract() {
        // contracts/messages.md:37-40 (FR-008 / FR-028: the tooltip names the state).
        // Bite: a state without a tooltip, two states sharing one (Error and
        // HotkeyError told apart only by the icon), a wrong id, or an id that is not
        // declared.
        let table = [
            (TrayState::Idle, "tray.tooltip.idle"),
            (TrayState::Recording, "tray.tooltip.recording"),
            (TrayState::CallRecording, "tray.tooltip.call_recording"),
            (TrayState::Error, "tray.tooltip.error"),
            (TrayState::HotkeyError, "tray.tooltip.hotkey_error"),
        ];
        assert_eq!(table.map(|(s, _)| s), all_states());
        for (state, expected) in table {
            let id = tooltip(state);
            assert_eq!(id_str(id), expected, "{state:?}");
            assert!(
                MESSAGE_IDS.contains(&id),
                "{expected} is not in MESSAGE_IDS"
            );
        }
    }

    #[test]
    fn tray_texts_are_the_contract_texts_in_en_and_ru() {
        // i18n conformance of the new ids (contracts/messages.md:32-40), through the
        // embedded catalogs the shell renders with. Bite: an id missing from a
        // catalog (rendered as the id itself, or ru falling back to the English
        // text), or a text that is not the contract's. "Voicen" is the product name
        // in both languages (T-005).
        let rows: [(MessageId, &str, &str); 12] = [
            (i18n::TRAY_CALL, "Call", "Звонок"),
            (TrayAction::CallRecord.label(), "Record", "Записать"),
            (TrayAction::CallStop.label(), "Stop", "Остановить"),
            (TrayAction::OpenCalls.label(), "Calls", "Звонки"),
            (
                tooltip(TrayState::CallRecording),
                "Voicen — recording a call",
                "Voicen — запись звонка",
            ),
            (TrayAction::OpenSettings.label(), "Settings", "Настройки"),
            (
                TrayAction::OpenLogs.label(),
                "Open logs folder",
                "Открыть папку журналов",
            ),
            (TrayAction::Exit.label(), "Exit", "Выход"),
            (tooltip(TrayState::Idle), "Voicen", "Voicen"),
            (
                tooltip(TrayState::Recording),
                "Voicen — recording",
                "Voicen — запись",
            ),
            (
                tooltip(TrayState::Error),
                "Voicen — last dictation failed",
                "Voicen — последняя диктовка не удалась",
            ),
            (
                tooltip(TrayState::HotkeyError),
                "Voicen — hotkey not registered",
                "Voicen — сочетание клавиш не зарегистрировано",
            ),
        ];
        for (id, en, ru) in rows {
            assert_eq!(i18n::text(UiLanguage::En, id, &[]), en, "{} en", id_str(id));
            assert_eq!(i18n::text(UiLanguage::Ru, id, &[]), ru, "{} ru", id_str(id));
        }
    }

    #[test]
    fn each_state_shows_its_own_icon() {
        // OQ-11 Q1 default: four icon variants, one per state, so Error and
        // HotkeyError are visible without hovering. Bite: Error or HotkeyError
        // shown with the idle icon, Recording not marked, two states sharing an
        // icon.
        let table = [
            (TrayState::Idle, TrayIconKind::Idle),
            (TrayState::Recording, TrayIconKind::Recording),
            (TrayState::CallRecording, TrayIconKind::Recording),
            (TrayState::Error, TrayIconKind::Error),
            (TrayState::HotkeyError, TrayIconKind::HotkeyError),
        ];
        assert_eq!(table.map(|(s, _)| s), all_states());
        for (state, expected) in table {
            assert_eq!(icon(state), expected, "{state:?}");
        }
    }

    #[test]
    fn menu_submenu_label_is_the_call_id_and_rows_are_in_message_ids() {
        // contracts/ipc.md `tray.call`: the submenu's text id is declared; every
        // leaf action of the table is declared and has the id `as_str` maps back.
        // Bite: a submenu with another label, an undeclared id.
        for row in menu(false, false) {
            if let TrayRow::Submenu { label, .. } = row {
                assert_eq!(id_str(label), "tray.call");
                assert!(MESSAGE_IDS.contains(&label));
            }
        }
    }

    #[test]
    fn shown_follows_fr006_precedence_over_every_dictation_and_call_state() {
        // Spec 007 FR-006: dictation HotkeyError > dictation Recording >
        // CallRecording > Error > Idle. All 4 x 2 values spelled out. Bite:
        // CallRecording above dictation Recording or HotkeyError, below Error, the
        // flag ignored, a dictation state ignored.
        use TrayState::{
            CallRecording as C, Error as E, HotkeyError as H, Idle as I, Recording as R,
        };
        let table = [
            (I, false, I),
            (I, true, C),
            (R, false, R),
            (R, true, R),
            (E, false, E),
            (E, true, C),
            (H, false, H),
            (H, true, H),
        ];
        for (dictation, call, want) in table {
            assert_eq!(
                shown(dictation, call),
                want,
                "dictation {dictation:?}, call_recording {call}"
            );
        }
    }

    #[test]
    fn a_call_side_input_never_yields_hotkey_error() {
        // Spec 007 FR-004 failure branch: a call-hotkey conflict never sets the
        // hotkey-error state. HotkeyError is shown iff the dictation state is
        // HotkeyError, whatever the call flag; the dictation controller's own four
        // states are the only dictation inputs. Bite: the call flag mapped to
        // HotkeyError, HotkeyError shown for a non-HotkeyError dictation state,
        // HotkeyError hidden by the flag.
        for dictation in all_states() {
            for call in [false, true] {
                assert_eq!(
                    shown(dictation, call) == TrayState::HotkeyError,
                    dictation == TrayState::HotkeyError,
                    "dictation {dictation:?}, call_recording {call}"
                );
            }
        }
    }

    #[test]
    fn the_dictation_side_never_shows_call_recording_without_the_call_flag() {
        // CallRecording is shown only while the call flag is set, whatever the
        // dictation input (CallRecording included: the flag, not the input, decides).
        // Bite: CallRecording returned for a dictation state with the flag off.
        for dictation in all_states() {
            assert_ne!(
                shown(dictation, false),
                TrayState::CallRecording,
                "{dictation:?}"
            );
        }
    }
}
