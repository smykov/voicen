//! The tray icon's menu, tooltips and icons (T-052; spec 001 FR-001, FR-008 /
//! FR-028; spec 004 FR-013; contracts/messages.md `tray.*`).
//!
//! The one table the shell's tray adapter (`src-tauri/src/tray.rs`) renders: which
//! items the menu has and in which order, the id each item carries (its
//! `MenuEvent` id), the catalog id of each item's text and of each state's
//! tooltip, and which of the four icons a state shows. The shell renders the texts
//! with [`i18n::text`](crate::i18n::text) in the settings' `ui_language` and maps a
//! clicked id back with [`TrayAction::from_id`]; it decides nothing itself (P-010).
//!
//! The tray's state itself is the controller's ([`TrayState`], derived by
//! `RecordingController`, published by the dictation session together with
//! `retry_available`); nothing here keeps state.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use crate::i18n::{self, MessageId};
use crate::recording::TrayState;

/// An item of the tray menu. T-007 adds Retry (only while `retry_available`),
/// T-054 "Open logs folder", feature 005 History.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrayAction {
    /// "Settings": the settings window to the front on the tab it shows, or opened
    /// on Engine when none is open (OQ-11 Q2 default).
    OpenSettings,
    /// "Exit": ends the process (the only user exit, T-052 invariant 4).
    Exit,
}

impl TrayAction {
    /// The menu item id (`MenuEvent.id`): a closed set of `[a-z_]+` words, one per
    /// action.
    pub fn as_str(self) -> &'static str {
        match self {
            TrayAction::OpenSettings => "settings",
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
            TrayAction::OpenSettings => i18n::TRAY_SETTINGS,
            TrayAction::Exit => i18n::TRAY_EXIT,
        }
    }
}

/// Every action, so [`TrayAction::from_id`] maps back exactly the ids
/// [`TrayAction::as_str`] gives (a new variant is added here and to `as_str`).
const ALL: [TrayAction; 2] = [TrayAction::OpenSettings, TrayAction::Exit];

/// The four tray icons (OQ-11 Q1 default: the app icon plain, with a red dot, with
/// an amber "!", with an amber "!" and a key mark). The shell embeds one image per
/// kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrayIconKind {
    Idle,
    Recording,
    Error,
    HotkeyError,
}

/// The menu items, top to bottom. `retry_available` is the session's (the pending
/// slot, dictation-session.md); T-007 adds Retry while it is true. Pure.
pub fn menu(retry_available: bool) -> Vec<TrayAction> {
    // T-007 inserts Retry before Exit while `retry_available`; until then both rows
    // are the same.
    let _ = retry_available;
    vec![TrayAction::OpenSettings, TrayAction::Exit]
}

/// The catalog id of the tooltip `state` shows. Pure.
pub fn tooltip(state: TrayState) -> MessageId {
    match state {
        TrayState::Idle => i18n::TRAY_TOOLTIP_IDLE,
        TrayState::Recording => i18n::TRAY_TOOLTIP_RECORDING,
        TrayState::Error => i18n::TRAY_TOOLTIP_ERROR,
        TrayState::HotkeyError => i18n::TRAY_TOOLTIP_HOTKEY_ERROR,
    }
}

/// The icon `state` shows. Pure.
pub fn icon(state: TrayState) -> TrayIconKind {
    match state {
        TrayState::Idle => TrayIconKind::Idle,
        TrayState::Recording => TrayIconKind::Recording,
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
    fn all_states() -> [TrayState; 4] {
        let all = [
            TrayState::Idle,
            TrayState::Recording,
            TrayState::Error,
            TrayState::HotkeyError,
        ];
        for state in all {
            match state {
                TrayState::Idle
                | TrayState::Recording
                | TrayState::Error
                | TrayState::HotkeyError => {}
            }
        }
        all
    }

    /// Every menu action, by a wildcard-free match (as above).
    fn all_actions() -> [TrayAction; 2] {
        let all = [TrayAction::OpenSettings, TrayAction::Exit];
        for action in all {
            match action {
                TrayAction::OpenSettings | TrayAction::Exit => {}
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

    #[test]
    fn menu_is_settings_then_exit_whether_or_not_a_retry_is_available() {
        // spec 001 FR-001 / T-052 analysis design 3: the menu is [Settings, Exit] in
        // T-052, for both values of retry_available (T-007 changes the `true` row:
        // Retry while the pending slot holds a recording). Bite: Exit missing (the
        // process could never be ended once prevent_exit is in), the order swapped,
        // an item for a later task already present, or the two rows differing now.
        assert_eq!(
            menu(false),
            vec![TrayAction::OpenSettings, TrayAction::Exit],
            "retry_available = false"
        );
        assert_eq!(
            menu(true),
            vec![TrayAction::OpenSettings, TrayAction::Exit],
            "retry_available = true (T-007 adds Retry here)"
        );
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
            "open_logs",
            "retry",
            "history",
            " exit",
        ] {
            assert_eq!(
                TrayAction::from_id(unknown),
                None,
                "{unknown:?} is not an item of the T-052 menu"
            );
        }
    }

    #[test]
    fn menu_item_labels_are_the_contract_ids_and_declared() {
        // contracts/messages.md:32 and :36. Declared in MESSAGE_IDS, so the catalog
        // test checks both catalogs. Bite: a wrong id (`tray.open_settings`), the
        // two items swapped, or an id built outside the `messages!` list.
        let table = [
            (TrayAction::OpenSettings, "tray.settings"),
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
        let rows: [(MessageId, &str, &str); 6] = [
            (TrayAction::OpenSettings.label(), "Settings", "Настройки"),
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
            (TrayState::Error, TrayIconKind::Error),
            (TrayState::HotkeyError, TrayIconKind::HotkeyError),
        ];
        assert_eq!(table.map(|(s, _)| s), all_states());
        for (state, expected) in table {
            assert_eq!(icon(state), expected, "{state:?}");
        }
    }
}
