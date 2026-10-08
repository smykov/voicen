//! T-008: `diag::format_line` and the diag tables (analysis approach 1 and the
//! format half of 4).
//!
//! The property: every `LogEvent`, built from every variant of every field type,
//! including engine kinds, detector names and failure codes that are planted
//! `Box::leak`ed strings, formats to one line that passes the hand-written grammar
//! and the closed value sets of `diag_support`, and no planted byte reaches it.
//! Every real `FailureReason` code maps to its own literal, never `other`.

mod diag_support;

use std::collections::BTreeSet;

use diag_support::{
    at, check_closed, pairs, parse, parse_ts, parsed, DETECTORS, DICTATION_OUTCOMES, ENGINES,
    FAILURES, MIC_CAUSES, NOON_UTC, WARNING_KINDS,
};
use voicen_core::autostart::ReconcileAction;
use voicen_core::connection_test::ConnectionTestResult;
use voicen_core::delivery::DeliveryResult;
use voicen_core::diag::{
    format_line, DetectorTag, DictationLine, DictationOutcome, EngineTag, FailureTag, LoadKind,
    LogEvent, SaveLine, WarningKind,
};
use voicen_core::engine::engine_for;
use voicen_core::failure::FailureReason;
use voicen_core::recording::MicCause;
use voicen_core::secrets::{FakeCredentialStore, KeySlot};
use voicen_core::settings::gate::Blocked;
use voicen_core::settings::service::{FormError, Warning, WarningCode as SettingsWarningCode};
use voicen_core::settings::{defaults, EngineKind, ErrorCode, FieldError, FieldId};
use voicen_core::vad::{EnergyDetector, SpeechDetector};
use voicen_core::BuildInfo;

/// Markers of the planted strings: none may appear in any line.
const PLANTED_MARKERS: &[&str] = &["PLANTED", "planted", "INJECT", "sk-test", "Planted"];

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// Strings from an open source (a foreign `Engine::kind`, `SpeechDetector::name`
/// or code), leaked to `&'static str` the way such a source can: spaces, CR/LF,
/// `key=value` injection, a lowercase word that passes the character grammar, a
/// near-miss of a real code, and the empty string.
fn planted() -> Vec<&'static str> {
    vec![
        leak("PLANTED-engine sk-test-PLANTED\nINJECT engine=api"),
        leak("planted_lower_snake"),
        leak("InvalidApiKey PLANTED"),
        leak("api\r\nPLANTED=1"),
        leak("Planted"),
        leak(""),
    ]
}

fn fake_build() -> BuildInfo {
    BuildInfo {
        version: "0.1.0",
        commit: "abc1234",
    }
}

fn line(
    engine: Option<EngineTag>,
    outcome: DictationOutcome,
    detector: Option<DetectorTag>,
    timings: Option<u64>,
) -> DictationLine {
    DictationLine {
        recording: 7,
        engine,
        outcome,
        detector,
        press_to_frame_ms: timings,
        duration_ms: timings,
        stop_to_text_ms: timings,
        text_to_paste_ms: timings,
    }
}

// ---- exhaustive enumerations (a new variant fails to compile here) ---------------

fn all_engine_tags() -> Vec<EngineTag> {
    let all = vec![
        EngineTag::Api,
        EngineTag::Builtin,
        EngineTag::LocalServer,
        EngineTag::Other,
    ];
    let seen: BTreeSet<usize> = all
        .iter()
        .map(|t| match t {
            EngineTag::Api => 0,
            EngineTag::Builtin => 1,
            EngineTag::LocalServer => 2,
            EngineTag::Other => 3,
        })
        .collect();
    assert_eq!(seen.len(), 4, "every EngineTag once");
    all
}

fn all_detector_tags() -> Vec<DetectorTag> {
    let all = vec![DetectorTag::Silero, DetectorTag::Energy, DetectorTag::Other];
    let seen: BTreeSet<usize> = all
        .iter()
        .map(|t| match t {
            DetectorTag::Silero => 0,
            DetectorTag::Energy => 1,
            DetectorTag::Other => 2,
        })
        .collect();
    assert_eq!(seen.len(), 3, "every DetectorTag once");
    all
}

/// Every `FailureTag` with its literal (the diag failure table).
fn all_failure_tags() -> Vec<(FailureTag, &'static str)> {
    let all = vec![
        (FailureTag::InvalidApiKey, "invalid_api_key"),
        (FailureTag::NetworkUnavailable, "network_unavailable"),
        (FailureTag::CannotReach, "cannot_reach"),
        (FailureTag::Timeout, "timeout"),
        (FailureTag::ServerError, "server_error"),
        (FailureTag::UnexpectedResponse, "unexpected_response"),
        (FailureTag::KeyStoreUnavailable, "key_store_unavailable"),
        (FailureTag::EngineNotConfigured, "engine_not_configured"),
        (FailureTag::ClipboardUnavailable, "clipboard_unavailable"),
        (FailureTag::MicrophoneUnavailable, "microphone_unavailable"),
        (FailureTag::Other, "other"),
    ];
    let seen: BTreeSet<usize> = all
        .iter()
        .map(|(t, _)| match t {
            FailureTag::InvalidApiKey => 0,
            FailureTag::NetworkUnavailable => 1,
            FailureTag::CannotReach => 2,
            FailureTag::Timeout => 3,
            FailureTag::ServerError => 4,
            FailureTag::UnexpectedResponse => 5,
            FailureTag::KeyStoreUnavailable => 6,
            FailureTag::EngineNotConfigured => 7,
            FailureTag::ClipboardUnavailable => 8,
            FailureTag::MicrophoneUnavailable => 9,
            FailureTag::Other => 10,
        })
        .collect();
    assert_eq!(seen.len(), 11, "every FailureTag once");
    all
}

/// Every `MicCause` with its literal (the diag microphone table, T-051).
fn all_mic_causes() -> Vec<(MicCause, &'static str)> {
    let all = vec![
        (MicCause::NoDevice, "no_device"),
        (MicCause::AccessDenied, "access_denied"),
        (MicCause::Busy, "busy"),
        (MicCause::Other, "other"),
    ];
    let seen: BTreeSet<usize> = all
        .iter()
        .map(|(c, _)| match c {
            MicCause::NoDevice => 0,
            MicCause::AccessDenied => 1,
            MicCause::Busy => 2,
            MicCause::Other => 3,
        })
        .collect();
    assert_eq!(seen.len(), 4, "every MicCause once");
    all
}

fn all_results() -> Vec<(DeliveryResult, &'static str)> {
    let all = vec![
        (DeliveryResult::Pasted, "pasted"),
        (DeliveryResult::CopiedOnly, "copied_only"),
        (DeliveryResult::CopyManual, "copy_manual"),
    ];
    for (r, _) in &all {
        match r {
            DeliveryResult::Pasted | DeliveryResult::CopiedOnly | DeliveryResult::CopyManual => {}
        }
    }
    all
}

/// Every `WarningKind` with its literal.
fn all_warning_kinds() -> Vec<(WarningKind, &'static str)> {
    let all = vec![
        (WarningKind::VadFallback, "vad_fallback"),
        (WarningKind::EscUnavailable, "esc_unavailable"),
        (WarningKind::ToastFailed, "toast_failed"),
        (WarningKind::ModelsCleanupFailed, "models_cleanup_failed"),
        (WarningKind::SettingsWindowFailed, "settings_window_failed"),
        (WarningKind::ChangeBridgeFailed, "change_bridge_failed"),
        (WarningKind::SettingsOpenerFailed, "settings_opener_failed"),
        (WarningKind::TrayFailed, "tray_failed"),
        (WarningKind::TrayFollowerFailed, "tray_follower_failed"),
        (WarningKind::HotkeyThreadFailed, "hotkey_thread_failed"),
        (WarningKind::HotkeyRegisterFailed, "hotkey_register_failed"),
        (WarningKind::DictationStartFailed, "dictation_start_failed"),
        (WarningKind::OverlayFailed, "overlay_failed"),
        (WarningKind::LogsFolderFailed, "logs_folder_failed"),
    ];
    let seen: BTreeSet<usize> = all
        .iter()
        .map(|(k, _)| match k {
            WarningKind::VadFallback => 0,
            WarningKind::EscUnavailable => 1,
            WarningKind::ToastFailed => 2,
            WarningKind::ModelsCleanupFailed => 3,
            WarningKind::SettingsWindowFailed => 4,
            WarningKind::ChangeBridgeFailed => 5,
            WarningKind::SettingsOpenerFailed => 6,
            WarningKind::TrayFailed => 7,
            WarningKind::TrayFollowerFailed => 8,
            WarningKind::HotkeyThreadFailed => 9,
            WarningKind::HotkeyRegisterFailed => 10,
            WarningKind::DictationStartFailed => 11,
            WarningKind::OverlayFailed => 12,
            WarningKind::LogsFolderFailed => 13,
        })
        .collect();
    assert_eq!(seen.len(), 14, "every WarningKind once");
    all
}

fn all_load_kinds() -> Vec<(LoadKind, &'static str)> {
    let all = vec![
        (LoadKind::Loaded, "loaded"),
        (LoadKind::FirstRun, "first_run"),
        (LoadKind::Reset, "reset"),
        (LoadKind::Unavailable, "unavailable"),
    ];
    for (k, _) in &all {
        match k {
            LoadKind::Loaded | LoadKind::FirstRun | LoadKind::Reset | LoadKind::Unavailable => {}
        }
    }
    all
}

fn all_reconcile_actions() -> Vec<ReconcileAction> {
    let all = vec![
        ReconcileAction::None,
        ReconcileAction::Written,
        ReconcileAction::Removed,
        ReconcileAction::Failed,
    ];
    for a in &all {
        match a {
            ReconcileAction::None
            | ReconcileAction::Written
            | ReconcileAction::Removed
            | ReconcileAction::Failed => {}
        }
    }
    all
}

fn save_lines() -> Vec<SaveLine> {
    vec![
        SaveLine::Saved { warnings: vec![] },
        SaveLine::Saved {
            warnings: vec![
                Warning {
                    field: FieldId::EngineApiBaseUrl,
                    code: SettingsWarningCode::EndpointInsecure,
                },
                Warning {
                    field: FieldId::PostProcessingBaseUrl,
                    code: SettingsWarningCode::EndpointInsecure,
                },
            ],
        },
        SaveLine::Refused {
            errors: vec![
                FieldError {
                    field: FieldId::EngineApiBaseUrl,
                    code: ErrorCode::UrlMalformed,
                },
                FieldError {
                    field: FieldId::HistorySize,
                    code: ErrorCode::HistorySizeRange,
                },
                FieldError {
                    field: FieldId::RecordingHotkey,
                    code: ErrorCode::HotkeyEscReserved,
                },
            ],
            form_error: None,
        },
        SaveLine::Refused {
            errors: vec![],
            form_error: Some(FormError::WriteFailed),
        },
        SaveLine::Refused {
            errors: vec![],
            form_error: Some(FormError::SettingsUnavailable),
        },
        SaveLine::Refused {
            errors: vec![FieldError {
                field: FieldId::EngineApiKey,
                code: ErrorCode::KeyStoreFailed,
            }],
            form_error: Some(FormError::PartiallyRestored {
                not_restored: vec![FieldId::GeneralStartWithWindows, FieldId::EngineApiKey],
            }),
        },
    ]
}

/// Every `LogEvent` variant, every value of every closed field, `Some`/`None` of
/// every optional, and the planted strings through the three tables.
fn every_event() -> Vec<LogEvent> {
    let mut all = vec![
        LogEvent::Started {
            build: fake_build(),
            pid: 4242,
        },
        LogEvent::Started {
            build: voicen_core::build_info(),
            pid: u32::MAX,
        },
        LogEvent::LogsRecovered,
    ];
    let mut outcomes = vec![DictationOutcome::NoSpeech, DictationOutcome::TooShort];
    for (r, _) in all_results() {
        outcomes.push(DictationOutcome::Delivered(r));
    }
    for (f, _) in all_failure_tags() {
        for http_status in [None, Some(503), Some(u16::MAX)] {
            outcomes.push(DictationOutcome::Failed {
                failure: f,
                http_status,
            });
        }
    }
    for (cause, _) in all_mic_causes() {
        outcomes.push(DictationOutcome::CaptureFailed { cause });
    }
    let mut engines: Vec<Option<EngineTag>> = all_engine_tags().into_iter().map(Some).collect();
    engines.push(None);
    let mut detectors: Vec<Option<DetectorTag>> =
        all_detector_tags().into_iter().map(Some).collect();
    detectors.push(None);
    for outcome in &outcomes {
        for engine in &engines {
            for detector in &detectors {
                for timings in [None, Some(0), Some(u64::MAX)] {
                    all.push(LogEvent::Dictation(line(
                        *engine, *outcome, *detector, timings,
                    )));
                }
            }
        }
    }
    for p in planted() {
        all.push(LogEvent::Dictation(line(
            Some(EngineTag::from_kind(p)),
            DictationOutcome::Failed {
                failure: FailureTag::from_code(p),
                http_status: Some(500),
            },
            Some(DetectorTag::from_name(p)),
            Some(12),
        )));
    }
    for (kind, _) in all_warning_kinds() {
        for os_code in [None, Some(5), Some(-2_147_024_891), Some(i32::MIN)] {
            all.push(LogEvent::Warning { kind, os_code });
        }
    }
    for (kind, _) in all_load_kinds() {
        all.push(LogEvent::SettingsLoad(kind));
    }
    for s in save_lines() {
        all.push(LogEvent::SettingsSave(s));
    }
    for a in all_reconcile_actions() {
        all.push(LogEvent::AutostartReconcile(a));
    }
    // T-046: every ConnectionTestResult kind, through the one constructor, with
    // the edge values of its numbers and planted hosts.
    let mut test_results = vec![
        ConnectionTestResult::Ok { latency_ms: 0 },
        ConnectionTestResult::Ok {
            latency_ms: u64::MAX,
        },
        ConnectionTestResult::InvalidKey,
        ConnectionTestResult::Timeout,
        ConnectionTestResult::Http { status: 100 },
        ConnectionTestResult::Http { status: 503 },
        ConnectionTestResult::Http { status: u16::MAX },
        ConnectionTestResult::UnexpectedResponse,
        ConnectionTestResult::Invalid { errors: vec![] },
        ConnectionTestResult::Invalid {
            errors: vec![FieldError {
                field: FieldId::EngineApiBaseUrl,
                code: ErrorCode::UrlMalformed,
            }],
        },
        ConnectionTestResult::KeyStoreUnavailable,
    ];
    for p in planted() {
        test_results.push(ConnectionTestResult::CannotReach {
            host: p.to_string(),
        });
    }
    for r in &test_results {
        all.push(LogEvent::settings_test_connection(r));
    }
    all.push(LogEvent::DictationBlocked {
        reason: Blocked::NoEngine,
    });

    let seen: BTreeSet<usize> = all
        .iter()
        .map(|e| match e {
            LogEvent::Started { .. } => 0,
            LogEvent::Dictation(_) => 1,
            LogEvent::Warning { .. } => 2,
            LogEvent::SettingsLoad(_) => 3,
            LogEvent::SettingsSave(_) => 4,
            LogEvent::AutostartReconcile(_) => 5,
            LogEvent::LogsRecovered => 6,
            LogEvent::DictationBlocked { .. } => 7,
            LogEvent::SettingsTestConnection(_) => 8,
        })
        .collect();
    assert_eq!(seen.len(), 9, "every LogEvent variant is enumerated");
    all
}

// ---- the closed output -----------------------------------------------------------

#[test]
fn every_log_event_formats_to_one_closed_line() {
    // Analysis approach 1, the invariant of the task. Bite: a field echoed
    // instead of mapped through its table (a planted engine kind, detector name
    // or failure code reaching the line), a value outside its literal set, a CR
    // or LF inside a line, a key the grammar does not know, an absent optional
    // written as a key, format_line panicking on an extreme value (u64::MAX,
    // i32::MIN).
    let times = [
        (at(NOON_UTC, 0), 0),
        (at(1_709_251_199, 999), 3_600),
        (at(1_791_151_140, 123), -12_600),
        (at(1_791_151_230, 1), 50_400),
    ];
    let mut wrong = Vec::new();
    let mut count = 0usize;
    for event in every_event() {
        for (t, offset) in times {
            count += 1;
            let raw = format_line(t, offset, &event);
            match parse(&raw).and_then(|l| check_closed(&l)) {
                Ok(()) => {}
                Err(e) => wrong.push(format!("{event:?}: {e}")),
            }
            for marker in PLANTED_MARKERS {
                if raw.contains(marker) {
                    wrong.push(format!("{event:?}: planted {marker:?} reached {raw:?}"));
                }
            }
        }
    }
    assert!(count > 1_000, "the enumeration ran: {count}");
    assert!(
        wrong.is_empty(),
        "{} of {count} lines break the closed output:\n{}",
        wrong.len(),
        wrong
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn foreign_engine_detector_and_failure_strings_become_other() {
    // The three tables are closed: a string not in a table is `other`, whatever
    // it contains. Bite: an echo arm, a prefix / contains match that lets
    // "api\r\nPLANTED=1" through as itself, a table that panics on "".
    for p in planted() {
        assert_eq!(EngineTag::from_kind(p), EngineTag::Other, "{p:?}");
        assert_eq!(DetectorTag::from_name(p), DetectorTag::Other, "{p:?}");
        assert_eq!(FailureTag::from_code(p), FailureTag::Other, "{p:?}");
        let raw = format_line(
            at(NOON_UTC, 0),
            0,
            &LogEvent::Dictation(line(
                Some(EngineTag::from_kind(p)),
                DictationOutcome::Failed {
                    failure: FailureTag::from_code(p),
                    http_status: None,
                },
                Some(DetectorTag::from_name(p)),
                None,
            )),
        );
        let l = parsed(&raw);
        assert_eq!(l.get("engine"), Some("other"), "{raw}");
        assert_eq!(l.get("detector"), Some("other"), "{raw}");
        assert_eq!(l.get("failure"), Some("other"), "{raw}");
    }
}

#[test]
fn real_engine_and_detector_names_map_to_their_own_tags() {
    // The values that exist: Engine::kind() of the API engine ("api"), the names
    // T-017/T-018 will use ("builtin", "local_server", engine/mod.rs:22), the
    // energy detector's name and Silero's ("silero", vad/mod.rs:22). Bite: a
    // table that maps everything to other, or swaps two entries.
    let creds = FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, "sk-test-0000");
    let mut s = defaults(None);
    s.engine = EngineKind::Api;
    s.api.base_url = "https://api.example.com/v1".to_string();
    let engine = match engine_for(&s, &creds) {
        Ok(engine) => engine,
        Err(e) => panic!("API engine expected, got {e:?}"),
    };
    assert_eq!(EngineTag::from_kind(engine.kind()), EngineTag::Api);
    assert_eq!(EngineTag::from_kind("api"), EngineTag::Api);
    assert_eq!(EngineTag::from_kind("builtin"), EngineTag::Builtin);
    assert_eq!(EngineTag::from_kind("local_server"), EngineTag::LocalServer);
    let energy = EnergyDetector::new();
    assert_eq!(
        DetectorTag::from_name(SpeechDetector::name(&energy)),
        DetectorTag::Energy
    );
    assert_eq!(DetectorTag::from_name("silero"), DetectorTag::Silero);

    let tags = [
        (EngineTag::Api, "api"),
        (EngineTag::Builtin, "builtin"),
        (EngineTag::LocalServer, "local_server"),
        (EngineTag::Other, "other"),
    ];
    for (tag, literal) in tags {
        let raw = format_line(
            at(NOON_UTC, 0),
            0,
            &LogEvent::Dictation(line(Some(tag), DictationOutcome::NoSpeech, None, None)),
        );
        assert_eq!(parsed(&raw).get("engine"), Some(literal), "{raw}");
    }
    for (tag, literal) in [
        (DetectorTag::Silero, "silero"),
        (DetectorTag::Energy, "energy"),
        (DetectorTag::Other, "other"),
    ] {
        let raw = format_line(
            at(NOON_UTC, 0),
            0,
            &LogEvent::Dictation(line(None, DictationOutcome::NoSpeech, Some(tag), None)),
        );
        assert_eq!(parsed(&raw).get("detector"), Some(literal), "{raw}");
    }
    assert_eq!(ENGINES.len(), 4);
    assert_eq!(DETECTORS.len(), 3);
}

/// Every `FailureReason` once; the exhaustive match makes a new reason a compile
/// error here until it gets a log literal.
fn every_reason() -> Vec<(FailureReason, FailureTag, &'static str)> {
    let all = vec![
        FailureReason::InvalidApiKey,
        FailureReason::NetworkUnavailable,
        FailureReason::CannotReach {
            host: "api.example.com:8443".to_string(),
        },
        FailureReason::Timeout,
        FailureReason::ServerError { status: 503 },
        FailureReason::UnexpectedResponse,
        FailureReason::KeyStoreUnavailable,
        FailureReason::EngineNotConfigured,
        FailureReason::ClipboardUnavailable,
        FailureReason::MicrophoneUnavailable {
            cause: MicCause::NoDevice,
        },
    ];
    all.into_iter()
        .map(|r| {
            let (tag, literal) = match &r {
                FailureReason::InvalidApiKey => (FailureTag::InvalidApiKey, "invalid_api_key"),
                FailureReason::NetworkUnavailable => {
                    (FailureTag::NetworkUnavailable, "network_unavailable")
                }
                FailureReason::CannotReach { .. } => (FailureTag::CannotReach, "cannot_reach"),
                FailureReason::Timeout => (FailureTag::Timeout, "timeout"),
                FailureReason::ServerError { .. } => (FailureTag::ServerError, "server_error"),
                FailureReason::UnexpectedResponse => {
                    (FailureTag::UnexpectedResponse, "unexpected_response")
                }
                FailureReason::KeyStoreUnavailable => {
                    (FailureTag::KeyStoreUnavailable, "key_store_unavailable")
                }
                FailureReason::EngineNotConfigured => {
                    (FailureTag::EngineNotConfigured, "engine_not_configured")
                }
                FailureReason::ClipboardUnavailable => {
                    (FailureTag::ClipboardUnavailable, "clipboard_unavailable")
                }
                FailureReason::MicrophoneUnavailable { .. } => {
                    (FailureTag::MicrophoneUnavailable, "microphone_unavailable")
                }
            };
            (r, tag, literal)
        })
        .collect()
}

#[test]
fn every_failure_reason_code_maps_to_its_own_literal() {
    // Analysis approach 1, last clause: every real FailureReason::code() maps to
    // a non-"other" literal, one per reason, and the literal reaches the line.
    // Bite: a code missing from the table (-> other), two codes on one literal,
    // the CamelCase code echoed, the CannotReach host on the line.
    let mut literals = BTreeSet::new();
    for (reason, tag, literal) in every_reason() {
        assert_eq!(FailureTag::from_code(reason.code()), tag, "{reason:?}");
        assert_ne!(tag, FailureTag::Other, "{reason:?}");
        let raw = format_line(
            at(NOON_UTC, 0),
            0,
            &LogEvent::Dictation(line(
                Some(EngineTag::Api),
                DictationOutcome::Failed {
                    failure: tag,
                    http_status: None,
                },
                None,
                None,
            )),
        );
        let l = parsed(&raw);
        assert_eq!(l.get("failure"), Some(literal), "{reason:?}: {raw}");
        assert!(!raw.contains("example.com"), "{raw}");
        assert!(literals.insert(literal), "{literal} used twice");
    }
    for (tag, literal) in all_failure_tags() {
        let raw = format_line(
            at(NOON_UTC, 0),
            0,
            &LogEvent::Dictation(line(
                None,
                DictationOutcome::Failed {
                    failure: tag,
                    http_status: None,
                },
                None,
                None,
            )),
        );
        assert_eq!(parsed(&raw).get("failure"), Some(literal), "{tag:?}: {raw}");
    }
    assert_eq!(FAILURES.len(), every_reason().len() + 1);
}

// ---- the timestamp ---------------------------------------------------------------

#[test]
fn timestamp_is_local_time_with_offset_and_milliseconds() {
    // FR-004 / spec 006 FR-004: local time with UTC offset and milliseconds.
    // Bite: UTC printed as local, the offset added with the wrong sign, the
    // offset minutes dropped (+05:45, -03:30), the milliseconds rounded up into the
    // next second, a leap-day or month-end error, "Z" instead of +00:00.
    let table = [
        (
            at(1_791_151_140, 123),
            7_200,
            "2026-10-04T23:59:00.123+02:00",
        ),
        (at(1_791_151_230, 0), 7_200, "2026-10-05T00:00:30.000+02:00"),
        (
            at(1_791_151_140, 123),
            -12_600,
            "2026-10-04T18:29:00.123-03:30",
        ),
        (at(1_791_151_140, 999), 0, "2026-10-04T21:59:00.999+00:00"),
        (
            at(1_709_251_199, 999),
            3_600,
            "2024-03-01T00:59:59.999+01:00",
        ),
        (
            at(1_709_251_199, 0),
            -18_000,
            "2024-02-29T18:59:59.000-05:00",
        ),
        (at(NOON_UTC, 5), 20_700, "2026-10-04T17:45:00.005+05:45"),
    ];
    for (t, offset, want) in table {
        let raw = format_line(t, offset, &LogEvent::LogsRecovered);
        let l = parsed(&raw);
        assert_eq!(l.ts, want, "{t:?} at {offset}: {raw}");
        let (.., parsed_offset) = parse_ts(&l.ts).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(parsed_offset, offset);
    }
}

// ---- line contents ---------------------------------------------------------------

#[test]
fn started_line_carries_version_commit_and_pid() {
    // FR-18 and the Windows CI smoke (`(<commit>) started`). Bite: another word
    // order, the commit without parentheses, no pid, the real build info not used
    // verbatim.
    let raw = format_line(
        at(NOON_UTC, 0),
        0,
        &LogEvent::Started {
            build: fake_build(),
            pid: 4242,
        },
    );
    assert!(
        raw.contains(" voicen 0.1.0 (abc1234) started"),
        "start line {raw:?}"
    );
    let l = parsed(&raw);
    assert_eq!(l.head, "started");
    assert_eq!(l.level, "INFO");
    assert_eq!(l.pairs, pairs(&[("pid", "4242".to_string())]));
    // The message as the shell test (src-tauri/tests/diag.rs) reads it.
    assert!(
        raw.ends_with(" INFO voicen 0.1.0 (abc1234) started pid=4242"),
        "{raw}"
    );

    let info = voicen_core::build_info();
    let raw = format_line(
        at(NOON_UTC, 0),
        0,
        &LogEvent::Started {
            build: info.clone(),
            pid: 1,
        },
    );
    assert!(
        raw.contains(&format!("voicen {info} started")),
        "start line {raw:?} for {info}"
    );
    assert!(raw.contains(&format!("({}) started", info.commit)), "{raw}");
}

#[test]
fn dictation_line_keys_per_outcome() {
    // Analysis approach 4, the format half: engine and the FR-20 timings plus
    // duration_ms, result only on delivered, failure (+ http_status) only on
    // failed; INFO for a delivered line, WARN for a failed one (data-model
    // "Dictation"). Bite: a timing under another key, result/failure on the wrong
    // outcome, no level difference.
    let delivered = LogEvent::Dictation(DictationLine {
        recording: 7,
        engine: Some(EngineTag::Api),
        outcome: DictationOutcome::Delivered(DeliveryResult::Pasted),
        detector: Some(DetectorTag::Energy),
        press_to_frame_ms: Some(41),
        duration_ms: Some(3_000),
        stop_to_text_ms: Some(812),
        text_to_paste_ms: Some(95),
    });
    let l = parsed(&format_line(at(NOON_UTC, 0), 0, &delivered));
    assert_eq!(l.head, "dictation");
    assert_eq!(l.level, "INFO");
    assert_eq!(
        l.pairs,
        pairs(&[
            ("rec", "7".to_string()),
            ("engine", "api".to_string()),
            ("outcome", "delivered".to_string()),
            ("result", "pasted".to_string()),
            ("detector", "energy".to_string()),
            ("press_to_frame_ms", "41".to_string()),
            ("duration_ms", "3000".to_string()),
            ("stop_to_text_ms", "812".to_string()),
            ("text_to_paste_ms", "95".to_string()),
        ])
    );

    let failed = LogEvent::Dictation(DictationLine {
        recording: 8,
        engine: Some(EngineTag::Api),
        outcome: DictationOutcome::Failed {
            failure: FailureTag::ServerError,
            http_status: Some(503),
        },
        detector: Some(DetectorTag::Silero),
        press_to_frame_ms: Some(30),
        duration_ms: Some(2_500),
        stop_to_text_ms: Some(30_001),
        text_to_paste_ms: None,
    });
    let l = parsed(&format_line(at(NOON_UTC, 0), 0, &failed));
    assert_eq!(l.level, "WARN");
    assert_eq!(
        l.pairs,
        pairs(&[
            ("rec", "8".to_string()),
            ("engine", "api".to_string()),
            ("outcome", "failed".to_string()),
            ("failure", "server_error".to_string()),
            ("http_status", "503".to_string()),
            ("detector", "silero".to_string()),
            ("press_to_frame_ms", "30".to_string()),
            ("duration_ms", "2500".to_string()),
            ("stop_to_text_ms", "30001".to_string()),
        ])
    );

    for (outcome, literal) in [
        (DictationOutcome::NoSpeech, "no_speech"),
        (DictationOutcome::TooShort, "too_short"),
    ] {
        let raw = format_line(
            at(NOON_UTC, 0),
            0,
            &LogEvent::Dictation(DictationLine {
                recording: 9,
                engine: None,
                outcome,
                detector: None,
                press_to_frame_ms: Some(12),
                duration_ms: Some(120),
                stop_to_text_ms: None,
                text_to_paste_ms: None,
            }),
        );
        let l = parsed(&raw);
        assert_eq!(l.level, "INFO", "{raw}");
        assert_eq!(
            l.pairs,
            pairs(&[
                ("rec", "9".to_string()),
                ("outcome", literal.to_string()),
                ("press_to_frame_ms", "12".to_string()),
                ("duration_ms", "120".to_string()),
            ]),
            "{raw}"
        );
    }

    for (result, literal) in all_results() {
        let raw = format_line(
            at(NOON_UTC, 0),
            0,
            &LogEvent::Dictation(line(None, DictationOutcome::Delivered(result), None, None)),
        );
        assert_eq!(parsed(&raw).get("result"), Some(literal), "{raw}");
    }
}

#[test]
fn capture_failed_line_has_the_mic_literal_at_warn() {
    // T-051: a capture failure (DictationEvent::CaptureFailed) is a dictation line
    // `outcome=capture_failed mic=<cause>`, WARN like a failed job, the cause
    // through the closed microphone table, no failure= / result= / http_status=.
    // Bite: the cause Debug-formatted ("AccessDenied"), two causes on one
    // literal, the line at INFO, the cause written as failure=microphone_unavailable.
    let mut literals = BTreeSet::new();
    for (cause, literal) in all_mic_causes() {
        let raw = format_line(
            at(NOON_UTC, 0),
            0,
            &LogEvent::Dictation(DictationLine {
                recording: 11,
                engine: None,
                outcome: DictationOutcome::CaptureFailed { cause },
                detector: None,
                press_to_frame_ms: Some(41),
                duration_ms: Some(2_000),
                stop_to_text_ms: None,
                text_to_paste_ms: None,
            }),
        );
        let l = parsed(&raw);
        assert_eq!(l.head, "dictation", "{raw}");
        assert_eq!(l.level, "WARN", "{cause:?}: {raw}");
        assert_eq!(
            l.pairs,
            pairs(&[
                ("rec", "11".to_string()),
                ("outcome", "capture_failed".to_string()),
                ("mic", literal.to_string()),
                ("press_to_frame_ms", "41".to_string()),
                ("duration_ms", "2000".to_string()),
            ]),
            "{cause:?}: {raw}"
        );
        assert!(literals.insert(literal), "{literal} used twice");
    }
    assert_eq!(MIC_CAUSES.len(), all_mic_causes().len());
    assert!(DICTATION_OUTCOMES.contains(&"capture_failed"));
}

#[test]
fn absent_timings_are_omitted_and_zero_is_kept() {
    // Spec 006 FR-005: a timing that did not happen is absent, not 0; a real 0 ms
    // is a value. Bite: None written as 0 (or as an empty key), Some(0) dropped.
    let none = format_line(
        at(NOON_UTC, 0),
        0,
        &LogEvent::Dictation(line(None, DictationOutcome::TooShort, None, None)),
    );
    let l = parsed(&none);
    assert_eq!(l.keys(), vec!["outcome", "rec"], "{none}");

    let zero = format_line(
        at(NOON_UTC, 0),
        0,
        &LogEvent::Dictation(line(None, DictationOutcome::TooShort, None, Some(0))),
    );
    let l = parsed(&zero);
    for key in [
        "press_to_frame_ms",
        "duration_ms",
        "stop_to_text_ms",
        "text_to_paste_ms",
    ] {
        assert_eq!(l.get(key), Some("0"), "{key} in {zero}");
    }
}

#[test]
fn warning_line_has_its_kind_and_the_os_code_when_known() {
    // FR-006 and the typed shell warnings (analysis seam: the eprintln sites
    // become typed warnings). Bite: a kind literal swapped or echoed via Debug
    // ("VadFallback"), os_code written when absent, a negative code mangled, a
    // warning logged at INFO.
    for (kind, literal) in all_warning_kinds() {
        let raw = format_line(
            at(NOON_UTC, 0),
            0,
            &LogEvent::Warning {
                kind,
                os_code: None,
            },
        );
        let l = parsed(&raw);
        assert_eq!(l.head, "warning", "{raw}");
        assert_eq!(l.level, "WARN", "{raw}");
        assert_eq!(l.pairs, pairs(&[("kind", literal.to_string())]), "{raw}");

        let raw = format_line(
            at(NOON_UTC, 0),
            0,
            &LogEvent::Warning {
                kind,
                os_code: Some(-2_147_024_891),
            },
        );
        assert_eq!(parsed(&raw).get("os_code"), Some("-2147024891"), "{raw}");
    }
    assert_eq!(WARNING_KINDS.len(), all_warning_kinds().len());
}

#[test]
fn logs_recovered_line_has_no_keys() {
    // Bite: a reason or a path on the recovery line.
    let raw = format_line(at(NOON_UTC, 0), 0, &LogEvent::LogsRecovered);
    let l = parsed(&raw);
    assert_eq!(l.head, "logs recovered", "{raw}");
    assert!(l.pairs.is_empty(), "{raw}");
}
