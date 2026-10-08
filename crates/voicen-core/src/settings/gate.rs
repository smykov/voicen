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

impl SettingsTab {
    /// The tab token of contracts/ipc.md (`settings?tab=<token>`, `settings://focus`):
    /// a closed set of `[a-z_]+` words, so it goes into the window URL unencoded.
    pub fn as_str(self) -> &'static str {
        match self {
            SettingsTab::Engine => "engine",
            SettingsTab::Recording => "recording",
            SettingsTab::Output => "output",
            SettingsTab::PostProcessing => "post_processing",
            SettingsTab::History => "history",
            SettingsTab::General => "general",
        }
    }
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

/// What the running instance does when a second launch reaches it (spec 001 FR-002,
/// spec 004 FR-019; T-052, OQ-11 Q2 default). The second process itself exits
/// inside tauri's `build()` and does nothing else (T-052 invariant 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondLaunchAction {
    /// The settings window to the front on the tab it shows (unminimized, shown,
    /// focused, no tab switch), or opened on Engine when none is open.
    FrontSettings,
    /// Nothing: a logon start (`--autostart`) opens no window.
    TrayOnly,
}

/// The second-launch decision, from whether the second process was started by the
/// Run value. Pure.
pub fn second_launch_action(launched_by_autostart: bool) -> SecondLaunchAction {
    if launched_by_autostart {
        SecondLaunchAction::TrayOnly
    } else {
        SecondLaunchAction::FrontSettings
    }
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

/// The startup decision once the hotkey registration is known (T-055 Q1): a failed
/// registration has already asked for settings on the Recording tab with the
/// hotkey field focused (`DictationSession::hotkey_registration(false)`), so the
/// startup tab must not take the window from it: `TrayOnly`. Otherwise
/// [`startup_action`]. Pure.
pub fn startup_action_after_hotkey(
    o: &LoadOutcome,
    launched_by_autostart: bool,
    hotkey_registered: bool,
) -> StartupAction {
    if hotkey_registered {
        startup_action(o, launched_by_autostart)
    } else {
        StartupAction::TrayOnly
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
    fn a_failed_hotkey_registration_keeps_the_startup_tab_away() {
        // T-055 Q1 (the hotkey field wins over the first-run Engine tab): with the
        // hotkey not registered no outcome posts a startup tab; with it registered
        // the decision is `startup_action`'s, row for row. Bite: the Engine tab
        // posted after a failed registration (the focus moves off the hotkey field),
        // or the registered case not delegating.
        let s = sample(EngineKind::Api);
        let outcomes = [
            LoadOutcome::FirstRun(s.clone()),
            LoadOutcome::Reset {
                settings: s.clone(),
                backup_file_name: "settings.json.bad-20261003-120000".into(),
            },
            LoadOutcome::Unavailable(s.clone()),
            LoadOutcome::Loaded(s.clone()),
        ];
        for outcome in &outcomes {
            for autostart in [false, true] {
                assert_eq!(
                    startup_action_after_hotkey(outcome, autostart, false),
                    StartupAction::TrayOnly,
                    "{outcome:?}, autostart {autostart}"
                );
                assert_eq!(
                    startup_action_after_hotkey(outcome, autostart, true),
                    startup_action(outcome, autostart),
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

    /// Every tab, by a wildcard-free match: a new variant fails to compile here until
    /// it is added to the token table below (T-037).
    fn all_tabs() -> [SettingsTab; 6] {
        let all = [
            SettingsTab::Engine,
            SettingsTab::Recording,
            SettingsTab::Output,
            SettingsTab::PostProcessing,
            SettingsTab::History,
            SettingsTab::General,
        ];
        for tab in all {
            match tab {
                SettingsTab::Engine
                | SettingsTab::Recording
                | SettingsTab::Output
                | SettingsTab::PostProcessing
                | SettingsTab::History
                | SettingsTab::General => {}
            }
        }
        all
    }

    #[test]
    fn settings_tab_tokens_are_the_ipc_contract() {
        // T-037: the tab token of `settings?tab=<token>` and of the `settings://focus`
        // payload is contracts/ipc.md's closed set, one spelling per tab. Bite: any
        // token misspelled, swapped between tabs, empty, or in another case
        // (`PostProcessing`, `post-processing`).
        let table = [
            (SettingsTab::Engine, "engine"),
            (SettingsTab::Recording, "recording"),
            (SettingsTab::Output, "output"),
            (SettingsTab::PostProcessing, "post_processing"),
            (SettingsTab::History, "history"),
            (SettingsTab::General, "general"),
        ];
        assert_eq!(table.map(|(tab, _)| tab), all_tabs());
        for (tab, token) in table {
            assert_eq!(tab.as_str(), token, "{tab:?}");
        }
    }

    #[test]
    fn settings_tab_tokens_are_distinct_url_safe_words() {
        // T-037 (S1): the token goes into the window URL unencoded, so it must be a
        // non-empty word of [a-z_] and no two tabs may share one. Bite: an empty
        // token, a character that needs URL encoding (`&`, `=`, space), or two tabs
        // that open the same page.
        let tokens: Vec<&str> = all_tabs().iter().map(|t| t.as_str()).collect();
        for (tab, token) in all_tabs().iter().zip(&tokens) {
            assert!(
                !token.is_empty() && token.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{tab:?}: {token:?} is not a [a-z_]+ token"
            );
        }
        let mut unique = tokens.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            tokens.len(),
            "tokens not distinct: {tokens:?}"
        );
    }

    #[test]
    fn second_launch_action_table() {
        // T-052 red-test table row 2 (spec 001 FR-002: a second launch brings the
        // settings window to the front; spec 004 FR-019: a logon start opens no
        // window). Bite: a second launch with `--autostart` opening a window (the
        // Run value firing while the app already runs, e.g. after a sign-out
        // without exit), or a plain second launch ignored.
        assert_eq!(
            second_launch_action(false),
            SecondLaunchAction::FrontSettings
        );
        assert_eq!(second_launch_action(true), SecondLaunchAction::TrayOnly);
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
