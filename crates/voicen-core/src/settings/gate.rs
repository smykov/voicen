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
    /// The call-mode settings (spec 007 FR-040).
    Calls,
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
            SettingsTab::Calls => "calls",
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
    fn all_tabs() -> [SettingsTab; 7] {
        let all = [
            SettingsTab::Engine,
            SettingsTab::Recording,
            SettingsTab::Output,
            SettingsTab::PostProcessing,
            SettingsTab::History,
            SettingsTab::General,
            // T-120 (spec 007 FR-040): the Calls tab.
            SettingsTab::Calls,
        ];
        for tab in all {
            match tab {
                SettingsTab::Engine
                | SettingsTab::Recording
                | SettingsTab::Output
                | SettingsTab::PostProcessing
                | SettingsTab::History
                | SettingsTab::General
                | SettingsTab::Calls => {}
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
            (SettingsTab::Calls, "calls"),
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
        // T-121 (spec 007 FR-003, FR-043): rewritten over `LaunchArgs`; the pre-007
        // rows are kept, `--call record` wins over autostart, an unknown value is
        // no action. Bite: `Record` mapped to FrontSettings/TrayOnly, `Record`
        // losing to autostart, `Unknown` fronting settings.
        let rows: [(&str, bool, Option<CallSwitch>, SecondLaunchAction); 6] = [
            ("default", false, None, SecondLaunchAction::FrontSettings),
            ("autostart", true, None, SecondLaunchAction::TrayOnly),
            (
                "record",
                false,
                Some(CallSwitch::Record),
                SecondLaunchAction::StartCallRecord,
            ),
            (
                "record+autostart",
                true,
                Some(CallSwitch::Record),
                SecondLaunchAction::StartCallRecord,
            ),
            (
                "unknown",
                false,
                Some(CallSwitch::Unknown),
                SecondLaunchAction::TrayOnly,
            ),
            (
                "unknown+autostart",
                true,
                Some(CallSwitch::Unknown),
                SecondLaunchAction::TrayOnly,
            ),
        ];
        for (label, autostart, call, want) in rows {
            let args = LaunchArgs { autostart, call };
            assert_eq!(second_launch_action(&args), want, "row {label}");
        }
    }

    const EXE: &str = r"C:\Program Files\Voicen\voicen.exe";

    #[test]
    fn launch_args_parse_table() {
        // T-121 (contracts/core-call.md "Launch arguments", invariants I1, I3, I5).
        // Bite: a prefix/case-insensitive match, `--call=record` read as a switch,
        // the value of `--call` swallowed from the `--autostart` scan, a later
        // `--call` overriding the first, a missing value read as None or Record.
        use CallSwitch::{Record, Unknown};
        let rows: Vec<(&str, Vec<&str>, bool, Option<CallSwitch>)> = vec![
            ("empty argv", vec![], false, None),
            ("exe only", vec![EXE], false, None),
            ("autostart", vec![EXE, "--autostart"], true, None),
            (
                "call record",
                vec![EXE, "--call", "record"],
                false,
                Some(Record),
            ),
            (
                "autostart then call",
                vec![EXE, "--autostart", "--call", "record"],
                true,
                Some(Record),
            ),
            (
                "call then autostart",
                vec![EXE, "--call", "record", "--autostart"],
                true,
                Some(Record),
            ),
            (
                "call bogus",
                vec![EXE, "--call", "bogus"],
                false,
                Some(Unknown),
            ),
            (
                "call without value",
                vec![EXE, "--call"],
                false,
                Some(Unknown),
            ),
            (
                "call RECORD",
                vec![EXE, "--call", "RECORD"],
                false,
                Some(Unknown),
            ),
            (
                "call swallows --autostart as value, autostart still true",
                vec![EXE, "--call", "--autostart"],
                true,
                Some(Unknown),
            ),
            (
                "autostart-later",
                vec![EXE, "--autostart-later"],
                false,
                None,
            ),
            ("AUTOSTART", vec![EXE, "--AUTOSTART"], false, None),
            ("call=record", vec![EXE, "--call=record"], false, None),
            ("CALL record", vec![EXE, "--CALL", "record"], false, None),
            (
                "first call decides: record then bogus",
                vec![EXE, "--call", "record", "--call", "bogus"],
                false,
                Some(Record),
            ),
            (
                "first call decides: bogus then record",
                vec![EXE, "--call", "bogus", "--call", "record"],
                false,
                Some(Unknown),
            ),
            (
                "call --call record: value is the second --call",
                vec![EXE, "--call", "--call", "record"],
                false,
                Some(Unknown),
            ),
            // I5: argv[0] is scanned like any other argument.
            (
                "autostart as the only argument",
                vec!["--autostart"],
                true,
                None,
            ),
        ];
        for (label, argv, autostart, call) in rows {
            let parsed = LaunchArgs::parse(argv.iter().copied());
            assert_eq!(parsed.autostart, autostart, "row {label}: autostart");
            assert_eq!(parsed.call, call, "row {label}: call");
        }
    }

    #[test]
    fn launch_args_parse_accepts_owned_strings_and_os_strings() {
        // The generic bound: the plugin callback's `Vec<String>` (by reference) and
        // `std::env::args_os()` (`OsString`) both go through the one parser.
        let owned: Vec<String> = [EXE, "--autostart", "--call", "record"]
            .iter()
            .map(|a| a.to_string())
            .collect();
        let from_ref = LaunchArgs::parse(&owned);
        assert!(from_ref.autostart, "Vec<String> by reference: autostart");
        assert_eq!(
            from_ref.call,
            Some(CallSwitch::Record),
            "Vec<String> by ref"
        );
        let from_owned = LaunchArgs::parse(owned.clone());
        assert_eq!(
            from_owned.call,
            Some(CallSwitch::Record),
            "Vec<String> owned"
        );

        let os: Vec<std::ffi::OsString> = owned.iter().map(std::ffi::OsString::from).collect();
        let from_os = LaunchArgs::parse(os);
        assert!(from_os.autostart, "OsString: autostart");
        assert_eq!(from_os.call, Some(CallSwitch::Record), "OsString");
    }

    #[test]
    fn launch_args_parse_non_utf8_value_is_unknown() {
        // I3: compared as `OsStr`, never through a lossy conversion. A value that is
        // not valid Unicode is `Unknown`, never `Record` and never `None`; an
        // `--autostart` after it is still found (I5).
        #[cfg(unix)]
        let bad: std::ffi::OsString = {
            use std::os::unix::ffi::OsStringExt;
            std::ffi::OsString::from_vec(vec![b'r', 0xFF, b'c'])
        };
        #[cfg(windows)]
        let bad: std::ffi::OsString = {
            use std::os::windows::ffi::OsStringExt;
            std::ffi::OsString::from_wide(&[0x72, 0xD800, 0x63])
        };
        assert!(bad.to_str().is_none(), "premise: not valid Unicode");
        let argv: Vec<std::ffi::OsString> =
            vec![EXE.into(), "--call".into(), bad, "--autostart".into()];
        let parsed = LaunchArgs::parse(argv);
        assert_eq!(parsed.call, Some(CallSwitch::Unknown), "non-UTF-8 value");
        assert!(parsed.autostart, "autostart after a non-UTF-8 value");
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
