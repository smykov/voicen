//! Startup and dictation gate: pure decisions the shell executes
//! (spec 004 FR-005..FR-007, FR-010, US5-3; decision #19).

use super::EngineKind;
use super::{LoadOutcome, Settings};
use crate::i18n::{MessageId, NOTICE_CHOOSE_ENGINE};

/// Why dictation cannot start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    /// Engine = none (FR-21 failure branch).
    NoEngine,
}

/// A tab of the settings window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Engine,
    Recording,
    Output,
    PostProcessing,
    History,
    General,
}

/// What the shell does instead of recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellAction {
    Notify(MessageId),
    OpenSettings(SettingsTab),
}

/// What the shell does at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupAction {
    OpenSettings(SettingsTab),
    TrayOnly,
}

/// Called by the hotkey handler before the microphone is opened. Only engine
/// `none` blocks; whether a configured engine works is the pipeline's question.
pub fn dictation_gate(s: &Settings) -> Result<(), Blocked> {
    match s.engine {
        EngineKind::None => Err(Blocked::NoEngine),
        EngineKind::Api | EngineKind::BuiltinLocal | EngineKind::LocalServer => Ok(()),
    }
}

/// What the shell does, in order, when dictation is blocked.
pub fn blocked_actions(b: Blocked) -> Vec<ShellAction> {
    match b {
        Blocked::NoEngine => vec![
            ShellAction::Notify(NOTICE_CHOOSE_ENGINE),
            ShellAction::OpenSettings(SettingsTab::Engine),
        ],
    }
}

/// Startup decision: the settings window on the Engine tab unless the settings were
/// loaded from an existing file. `launched_by_autostart` does not change it
/// (spec US5-3); the shell adds the reset or unavailable notice.
pub fn startup_action(o: &LoadOutcome, _launched_by_autostart: bool) -> StartupAction {
    match o {
        LoadOutcome::Loaded(_) => StartupAction::TrayOnly,
        LoadOutcome::FirstRun(_) | LoadOutcome::Reset { .. } | LoadOutcome::Unavailable(_) => {
            StartupAction::OpenSettings(SettingsTab::Engine)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::sample;
    use super::super::EngineKind;
    use super::*;
    use crate::i18n::NOTICE_CHOOSE_ENGINE;

    #[test]
    fn startup_action_table() {
        // FirstRun / Reset / Unavailable open Engine; Loaded stays in the tray; the
        // autostart flag never changes that (US5-3). Bite: any row swapped, or the
        // autostart flag consulted.
        let s = sample(EngineKind::Api);
        let table = [
            (
                LoadOutcome::FirstRun(s.clone()),
                StartupAction::OpenSettings(SettingsTab::Engine),
            ),
            (
                LoadOutcome::Reset {
                    settings: s.clone(),
                    backup_file_name: "settings.json.bad-20261003-120000".into(),
                },
                StartupAction::OpenSettings(SettingsTab::Engine),
            ),
            (
                LoadOutcome::Unavailable(s.clone()),
                StartupAction::OpenSettings(SettingsTab::Engine),
            ),
            (LoadOutcome::Loaded(s.clone()), StartupAction::TrayOnly),
        ];
        for (outcome, expected) in &table {
            for autostart in [false, true] {
                assert_eq!(
                    startup_action(outcome, autostart),
                    *expected,
                    "{outcome:?}, autostart {autostart}"
                );
            }
        }
    }

    #[test]
    fn startup_action_ignores_settings_content() {
        // Loaded with engine none still starts in the tray: engine none is handled
        // at the hotkey (FR-007), not at startup (FR-006).
        let none = sample(EngineKind::None);
        for autostart in [false, true] {
            assert_eq!(
                startup_action(&LoadOutcome::Loaded(none.clone()), autostart),
                StartupAction::TrayOnly
            );
        }
    }

    #[test]
    fn blocked_actions_no_engine() {
        // Bite: missing notice, wrong tab, other order, or an extra action.
        assert_eq!(
            blocked_actions(Blocked::NoEngine),
            vec![
                ShellAction::Notify(NOTICE_CHOOSE_ENGINE),
                ShellAction::OpenSettings(SettingsTab::Engine),
            ]
        );
    }

    #[test]
    fn dictation_gate_blocks_only_none() {
        // Bite: a gate that blocks a configured engine, or lets none through.
        assert_eq!(
            dictation_gate(&sample(EngineKind::None)),
            Err(Blocked::NoEngine)
        );
        for engine in [
            EngineKind::Api,
            EngineKind::BuiltinLocal,
            EngineKind::LocalServer,
        ] {
            assert_eq!(dictation_gate(&sample(engine)), Ok(()), "{engine:?}");
        }
    }
}
