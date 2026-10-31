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
    at, check_closed, closed, pairs, parse, parse_ts, parsed, DETECTORS, DICTATION_OUTCOMES,
    ENGINES, FAILURES, MIC_CAUSES, NOON_UTC, PP_REASONS, PP_RESULTS, WARNING_KINDS,
};
use voicen_core::autostart::ReconcileAction;
use voicen_core::call::{
    CallSource, DeviceCause, EndpointId, FailReason, FinalOutcome, FinalStage, InvalidRule,
    PassMode, RefusedPart, RefusedReason, SilentRule, StopReason, StorageKind, SwitchRefusal,
    Track, VadFallbackCause,
};
use voicen_core::connection_test::ConnectionTestResult;
use voicen_core::delivery::DeliveryResult;
use voicen_core::diag::{
    format_line, CrashFileName, DetectorTag, DictationLine, DictationOutcome, EngineTag,
    FailureTag, LoadKind, LogEvent, OsVersion, PreviousSessionLine, SaveLine, WarningKind,
};
use voicen_core::engine::engine_for;
use voicen_core::events::{PostProcessResult, PostProcessTrace, SkipKind};
use voicen_core::failure::{FailureReason, TimeoutOrigin};
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

/// Windows 11 24H2, as `RtlGetVersion` reports it.
const WIN11: OsVersion = OsVersion {
    major: 10,
    minor: 0,
    build: 26100,
};

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
        post_processing: None,
        local: None,
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
        (FailureTag::NoLocalModel, "no_local_model"),
        (FailureTag::LocalEngineFailed, "local_engine_failed"),
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
            FailureTag::NoLocalModel => 10,
            FailureTag::LocalEngineFailed => 11,
            FailureTag::Other => 12,
        })
        .collect();
    assert_eq!(seen.len(), 13, "every FailureTag once");
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

/// Every `SkipKind` with its `pp_reason` literal (T-076; the six
/// `SkipReason::code()` spellings, written from format.rs's own table).
fn all_skip_kinds() -> Vec<(SkipKind, &'static str)> {
    let all = vec![
        (SkipKind::Timeout, "timeout"),
        (SkipKind::Unreachable, "unreachable"),
        (SkipKind::InvalidKey, "invalid_key"),
        (SkipKind::Http, "http"),
        (SkipKind::InvalidResponse, "invalid_response"),
        (SkipKind::NotConfigured, "not_configured"),
    ];
    let seen: BTreeSet<usize> = all
        .iter()
        .map(|(k, _)| match k {
            SkipKind::Timeout => 0,
            SkipKind::Unreachable => 1,
            SkipKind::InvalidKey => 2,
            SkipKind::Http => 3,
            SkipKind::InvalidResponse => 4,
            SkipKind::NotConfigured => 5,
        })
        .collect();
    assert_eq!(seen.len(), 6, "every SkipKind once");
    all
}

/// Every `PostProcessResult` with its `pp` literal and, for a skip, its
/// `pp_reason` literal (T-076).
fn all_pp_results() -> Vec<(PostProcessResult, &'static str, Option<&'static str>)> {
    let mut all = vec![
        (PostProcessResult::Off, "off", None),
        (PostProcessResult::Applied, "applied", None),
    ];
    for (kind, reason) in all_skip_kinds() {
        all.push((PostProcessResult::Skipped(kind), "skipped", Some(reason)));
    }
    let seen: BTreeSet<usize> = all
        .iter()
        .map(|(r, _, _)| match r {
            PostProcessResult::Off => 0,
            PostProcessResult::Applied => 1,
            PostProcessResult::Skipped(_) => 2,
        })
        .collect();
    assert_eq!(seen.len(), 3, "every PostProcessResult kind");
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
        (WarningKind::TestConnectionFailed, "test_connection_failed"),
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
            WarningKind::TestConnectionFailed => 14,
        })
        .collect();
    assert_eq!(seen.len(), 15, "every WarningKind once");
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
                FieldError::new(FieldId::EngineApiBaseUrl, ErrorCode::UrlMalformed),
                FieldError::new(FieldId::HistorySize, ErrorCode::HistorySizeRange),
                FieldError::new(FieldId::RecordingHotkey, ErrorCode::HotkeyEscReserved),
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
            errors: vec![FieldError::new(
                FieldId::EngineApiKey,
                ErrorCode::KeyStoreFailed,
            )],
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
            os: WIN11,
        },
        LogEvent::Started {
            build: voicen_core::build_info(),
            pid: u32::MAX,
            os: OsVersion {
                major: u32::MAX,
                minor: u32::MAX,
                build: u32::MAX,
            },
        },
        LogEvent::Started {
            build: fake_build(),
            pid: 0,
            os: OsVersion::UNKNOWN,
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
    // T-076: every post-processing result on every outcome a job closes (delivered,
    // failed, no speech: the stage ran before each), with the edge values of its
    // duration. A too-short, cancelled or capture-failed recording has no job, so
    // the observer never puts a trace on it.
    let job_outcomes = outcomes.iter().filter(|o| {
        matches!(
            o,
            DictationOutcome::Delivered(_)
                | DictationOutcome::Failed { .. }
                | DictationOutcome::NoSpeech
        )
    });
    for outcome in job_outcomes {
        for (result, _, _) in all_pp_results() {
            for ms in [0, 15_004, u64::MAX] {
                let mut l = line(
                    Some(EngineTag::Api),
                    *outcome,
                    Some(DetectorTag::Energy),
                    Some(3),
                );
                l.post_processing = Some(PostProcessTrace { result, ms });
                all.push(LogEvent::Dictation(l));
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
            errors: vec![FieldError::new(
                FieldId::EngineApiBaseUrl,
                ErrorCode::UrlMalformed,
            )],
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
    all.extend(call_events());
    // T-100 (review 1 #2): the session lines. A `CrashFileName` is built in tests
    // through the `test-fakes` constructor `CrashFileName::fake`, which accepts
    // only a name diag itself writes; the edge values of every part.
    all.push(LogEvent::Exited);
    all.push(LogEvent::PreviousSession(
        PreviousSessionLine::EndedByInstaller,
    ));
    all.push(LogEvent::PreviousSession(
        PreviousSessionLine::EndedAbnormally { crash_file: None },
    ));
    for name in [
        "crash-20261004-140000-4242.txt",
        "crash-00000101-000000-0.txt",
        "crash-99991231-235959-4294967295.txt",
    ] {
        let crash_file =
            CrashFileName::fake(name).unwrap_or_else(|| panic!("{name} is a diag name"));
        assert_eq!(crash_file.to_string(), name, "the name renders back");
        all.push(LogEvent::PreviousSession(PreviousSessionLine::Crashed {
            crash_file,
        }));
        all.push(LogEvent::PreviousSession(
            PreviousSessionLine::EndedAbnormally {
                crash_file: Some(crash_file),
            },
        ));
    }
    for foreign in [
        "crash-x.txt",
        "crash-20261004-140000-4242.txt\nkind: panic",
        "",
    ] {
        assert_eq!(
            CrashFileName::fake(foreign),
            None,
            "the fake constructor refuses {foreign:?}"
        );
    }
    let session_lines: BTreeSet<usize> = all
        .iter()
        .filter_map(|e| match e {
            LogEvent::PreviousSession(PreviousSessionLine::Crashed { .. }) => Some(0),
            LogEvent::PreviousSession(PreviousSessionLine::EndedByInstaller) => Some(1),
            LogEvent::PreviousSession(PreviousSessionLine::EndedAbnormally {
                crash_file: None,
            }) => Some(2),
            LogEvent::PreviousSession(PreviousSessionLine::EndedAbnormally {
                crash_file: Some(_),
            }) => Some(3),
            _ => None,
        })
        .collect();
    assert_eq!(
        session_lines.len(),
        4,
        "every PreviousSessionLine arm, with and without a file"
    );

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
            LogEvent::PreviousSession(_) => 9,
            LogEvent::Exited => 10,
            LogEvent::CallRequested { .. } => 11,
            LogEvent::CallRefused { .. } => 12,
            LogEvent::CallStarted { .. } => 13,
            LogEvent::CallDeviceChanged { .. } => 14,
            LogEvent::CallTrackSilent { .. } => 15,
            LogEvent::CallStopped { .. } => 16,
            LogEvent::CallStorageFailed { .. } => 17,
            LogEvent::CallRecovered { .. } => 18,
            LogEvent::FinalPassStage { .. } => 19,
            LogEvent::FinalPassFinished { .. } => 20,
            LogEvent::CallSwitchRefused { .. } => 21,
            LogEvent::VadFallback { .. } => 22,
            LogEvent::TranscriptInvalid { .. } => 23,
            LogEvent::VadDetector { .. } => 24,
        })
        .collect();
    assert_eq!(seen.len(), 25, "every LogEvent variant is enumerated");
    all
}

// ---- T-118: the call lines --------------------------------------------------------

const MIC_ID: &str = "{0.0.1.00000000}.{8a1c2f3e-0b4d-4c5a-9e6f-1a2b3c4d5e6f}";
const SYS_ID: &str = "{0.0.0.00000000}.{ABCDEF01-2345-6789-abcd-EF0123456789}";

const TRACKS: [(Track, &str); 2] = [(Track::Mic, "mic"), (Track::System, "system")];

const REFUSED_PARTS_ALL: [(RefusedPart, &str); 3] = [
    (RefusedPart::Mic, "mic"),
    (RefusedPart::System, "system"),
    (RefusedPart::Root, "root"),
];
const REFUSED_REASONS_ALL: [(RefusedReason, &str); 6] = [
    (RefusedReason::NoDevice, "no_device"),
    (RefusedReason::AccessDenied, "access_denied"),
    (RefusedReason::Busy, "busy"),
    (RefusedReason::Unwritable, "unwritable"),
    (RefusedReason::Missing, "missing"),
    (RefusedReason::Other, "other"),
];
const CAUSES_ALL: [(DeviceCause, &str); 6] = [
    (DeviceCause::DefaultOutputChanged, "default_output_changed"),
    (DeviceCause::MicLost, "mic_lost"),
    (DeviceCause::MicDefaultChanged, "mic_default_changed"),
    (DeviceCause::TrackLost, "track_lost"),
    (DeviceCause::Reopened, "reopened"),
    (DeviceCause::NoDevice, "no_device"),
];
const STOPS_ALL: [(StopReason, &str); 8] = [
    (StopReason::User, "user"),
    (StopReason::Hotkey, "hotkey"),
    (StopReason::Exit, "exit"),
    (StopReason::SystemEnd, "system_end"),
    (StopReason::Sleep, "sleep"),
    (StopReason::MaxLength, "max_length"),
    (StopReason::LowDisk, "low_disk"),
    (StopReason::StorageError, "storage_error"),
];
const STORAGE_ALL: [(StorageKind, &str); 6] = [
    (StorageKind::Write, "write"),
    (StorageKind::Flush, "flush"),
    (StorageKind::Sync, "sync"),
    (StorageKind::Meta, "meta"),
    (StorageKind::Transcript, "transcript"),
    (StorageKind::LowDisk, "low_disk"),
];
const STAGES_ALL: [(FinalStage, &str); 5] = [
    (FinalStage::Decode, "decode"),
    (FinalStage::Vad, "vad"),
    (FinalStage::Upload, "upload"),
    (FinalStage::Merge, "merge"),
    (FinalStage::Write, "write"),
];
const FAILS_ALL: [(FailReason, &str); 6] = [
    (FailReason::Key, "key"),
    (FailReason::Network, "network"),
    (FailReason::Timeout, "timeout"),
    (FailReason::Provider, "provider"),
    (FailReason::Storage, "storage"),
    (FailReason::EngineNotSetUp, "engine_not_set_up"),
];
const RULES_ALL: [(InvalidRule, &str); 7] = [
    (InvalidRule::Unsorted, "unsorted"),
    (InvalidRule::OutOfRange, "out_of_range"),
    (InvalidRule::UnknownSpeaker, "unknown_speaker"),
    (InvalidRule::UnknownTrack, "unknown_track"),
    (InvalidRule::BadSource, "bad_source"),
    (InvalidRule::BadLang, "bad_lang"),
    (InvalidRule::EmptyText, "empty_text"),
];

/// Every call variant with every value of every closed field, the edge integers, both
/// `Some`/`None` optionals, and the planted strings through `EndpointId`.
fn call_events() -> Vec<LogEvent> {
    let mut all = Vec::new();
    for source in [CallSource::Tray, CallSource::Hotkey, CallSource::Switch] {
        all.push(LogEvent::CallRequested { source });
    }
    for (part, _) in REFUSED_PARTS_ALL {
        for (reason, _) in REFUSED_REASONS_ALL {
            all.push(LogEvent::CallRefused { part, reason });
        }
    }
    let mut ids = vec![MIC_ID, SYS_ID];
    ids.extend(planted());
    for a in &ids {
        for b in &ids {
            for comms_differs in [false, true] {
                all.push(LogEvent::CallStarted {
                    mic: EndpointId::new(a),
                    system: EndpointId::new(b),
                    comms_differs,
                });
            }
        }
    }
    for (track, _) in TRACKS {
        for (cause, _) in CAUSES_ALL {
            for gap_ms in [0, 1_500, u64::MAX] {
                all.push(LogEvent::CallDeviceChanged {
                    track,
                    cause,
                    gap_ms,
                    to: None,
                });
                for id in &ids {
                    all.push(LogEvent::CallDeviceChanged {
                        track,
                        cause,
                        gap_ms,
                        to: Some(EndpointId::new(id)),
                    });
                }
            }
        }
        for rule in [SilentRule::Lost, SilentRule::CantHear] {
            all.push(LogEvent::CallTrackSilent { track, rule });
        }
        for (stage, _) in STAGES_ALL {
            for ms in [0, u64::MAX] {
                for chunks in [0, u32::MAX] {
                    all.push(LogEvent::FinalPassStage {
                        track,
                        stage,
                        ms,
                        chunks,
                    });
                }
            }
        }
        for cause in [VadFallbackCause::Unavailable, VadFallbackCause::Error] {
            all.push(LogEvent::VadFallback { track, cause });
        }
    }
    for (reason, _) in STOPS_ALL {
        for n in [0, u64::MAX] {
            all.push(LogEvent::CallStopped {
                reason,
                duration_ms: n,
                mic_bytes: n,
                system_bytes: u64::MAX - n,
            });
        }
    }
    for (kind, _) in STORAGE_ALL {
        for os in [None, Some(0), Some(5), Some(i32::MIN), Some(i32::MAX)] {
            all.push(LogEvent::CallStorageFailed { kind, os });
        }
    }
    for calls in [0, 1, u32::MAX] {
        all.push(LogEvent::CallRecovered { calls });
    }
    for mode in [PassMode::Timed, PassMode::Timeless] {
        for ms in [0, u64::MAX] {
            all.push(LogEvent::FinalPassFinished {
                outcome: FinalOutcome::Ready,
                mode,
                ms,
            });
            for (reason, _) in FAILS_ALL {
                all.push(LogEvent::FinalPassFinished {
                    outcome: FinalOutcome::Failed { reason },
                    mode,
                    ms,
                });
            }
        }
    }
    all.push(LogEvent::CallSwitchRefused {
        reason: SwitchRefusal::UnknownValue,
    });
    for (rule, _) in RULES_ALL {
        for segment in [0, 7, u32::MAX] {
            all.push(LogEvent::TranscriptInvalid { rule, segment });
        }
    }
    for detector in [DetectorTag::Silero, DetectorTag::Energy, DetectorTag::Other] {
        all.push(LogEvent::VadDetector { detector });
    }
    all
}

/// `"<LEVEL> <message>"` of an event: what follows the 29-character timestamp.
fn level_and_body(event: &LogEvent) -> String {
    let raw = format_line(at(NOON_UTC, 0), 0, event);
    let l = parsed(&raw);
    assert_eq!(l.ts.len(), 29, "{raw}");
    raw.get(30..)
        .unwrap_or_else(|| panic!("short line {raw:?}"))
        .to_string()
}

#[test]
fn every_call_line_formats_exactly_as_the_contract_with_its_level() {
    // Acceptance 1 (contracts/core-call.md "Log lines"; levels: the T-118 investigation,
    // pinned here so a change is visible). Bite: a renamed head or key, a changed key order,
    // `record=`/`subtitles=` dropped, `to=none` written as an empty value, an `os=` written
    // when unknown, a reason on a ready pass, a level moved.
    let mic = || EndpointId::new(MIC_ID);
    let sys = || EndpointId::new(SYS_ID);
    let ep = |t: &str| EndpointId::new(t);
    let cases: Vec<(LogEvent, String)> = vec![
        (
            LogEvent::CallRequested { source: CallSource::Tray },
            "INFO call_requested source=tray".into(),
        ),
        (
            LogEvent::CallRequested { source: CallSource::Hotkey },
            "INFO call_requested source=hotkey".into(),
        ),
        (
            LogEvent::CallRequested { source: CallSource::Switch },
            "INFO call_requested source=switch".into(),
        ),
        (
            LogEvent::CallRefused { part: RefusedPart::System, reason: RefusedReason::NoDevice },
            "WARN call_refused part=system reason=no_device".into(),
        ),
        (
            LogEvent::CallRefused { part: RefusedPart::Root, reason: RefusedReason::Unwritable },
            "WARN call_refused part=root reason=unwritable".into(),
        ),
        (
            LogEvent::CallStarted { mic: mic(), system: sys(), comms_differs: true },
            format!("INFO call_started record=1 subtitles=0 mic={MIC_ID} system={SYS_ID} comms_differs=1"),
        ),
        (
            LogEvent::CallStarted { mic: ep("Headset (Realtek)"), system: sys(), comms_differs: false },
            format!("INFO call_started record=1 subtitles=0 mic=invalid system={SYS_ID} comms_differs=0"),
        ),
        (
            LogEvent::CallDeviceChanged {
                track: Track::System,
                cause: DeviceCause::DefaultOutputChanged,
                gap_ms: 340,
                to: Some(sys()),
            },
            format!("INFO call_device track=system cause=default_output_changed gap_ms=340 to={SYS_ID}"),
        ),
        (
            LogEvent::CallDeviceChanged {
                track: Track::Mic,
                cause: DeviceCause::MicLost,
                gap_ms: 2000,
                to: None,
            },
            "WARN call_device track=mic cause=mic_lost gap_ms=2000 to=none".into(),
        ),
        (
            LogEvent::CallDeviceChanged {
                track: Track::Mic,
                cause: DeviceCause::Reopened,
                gap_ms: 0,
                to: Some(ep("bad name")),
            },
            "INFO call_device track=mic cause=reopened gap_ms=0 to=invalid".into(),
        ),
        (
            LogEvent::CallTrackSilent { track: Track::System, rule: SilentRule::CantHear },
            "WARN call_track_silent track=system rule=cant_hear".into(),
        ),
        (
            LogEvent::CallTrackSilent { track: Track::Mic, rule: SilentRule::Lost },
            "WARN call_track_silent track=mic rule=lost".into(),
        ),
        (
            LogEvent::CallStopped {
                reason: StopReason::SystemEnd,
                duration_ms: 61_000,
                mic_bytes: 1_234,
                system_bytes: 5_678,
            },
            "INFO call_stopped reason=system_end duration_ms=61000 mic_bytes=1234 system_bytes=5678".into(),
        ),
        (
            LogEvent::CallStorageFailed { kind: StorageKind::Sync, os: Some(112) },
            "WARN call_storage_failed kind=sync os=112".into(),
        ),
        (
            LogEvent::CallStorageFailed { kind: StorageKind::LowDisk, os: None },
            "WARN call_storage_failed kind=low_disk".into(),
        ),
        (
            LogEvent::CallStorageFailed { kind: StorageKind::Meta, os: Some(-5) },
            "WARN call_storage_failed kind=meta os=-5".into(),
        ),
        (LogEvent::CallRecovered { calls: 3 }, "INFO call_recovered calls=3".into()),
        (
            LogEvent::FinalPassStage { track: Track::Mic, stage: FinalStage::Upload, ms: 4_200, chunks: 12 },
            "INFO final_stage track=mic stage=upload ms=4200 chunks=12".into(),
        ),
        (
            LogEvent::FinalPassFinished { outcome: FinalOutcome::Ready, mode: PassMode::Timed, ms: 9_000 },
            "INFO final_finished outcome=ready mode=timed ms=9000".into(),
        ),
        (
            LogEvent::FinalPassFinished {
                outcome: FinalOutcome::Failed { reason: FailReason::EngineNotSetUp },
                mode: PassMode::Timeless,
                ms: 5,
            },
            "WARN final_finished outcome=failed reason=engine_not_set_up mode=timeless ms=5".into(),
        ),
        (
            LogEvent::CallSwitchRefused { reason: SwitchRefusal::UnknownValue },
            "WARN call_switch_refused reason=unknown_value".into(),
        ),
        (
            LogEvent::VadFallback { track: Track::System, cause: VadFallbackCause::Unavailable },
            "WARN vad_fallback track=system cause=unavailable".into(),
        ),
        (
            LogEvent::VadFallback { track: Track::Mic, cause: VadFallbackCause::Error },
            "WARN vad_fallback track=mic cause=error".into(),
        ),
        (
            LogEvent::TranscriptInvalid { rule: InvalidRule::OutOfRange, segment: 17 },
            "WARN transcript_invalid rule=out_of_range segment=17".into(),
        ),
        (
            LogEvent::VadDetector { detector: DetectorTag::Silero },
            "INFO vad detector=silero".into(),
        ),
        (
            LogEvent::VadDetector { detector: DetectorTag::Energy },
            "INFO vad detector=energy".into(),
        ),
    ];
    for (event, want) in cases {
        assert_eq!(level_and_body(&event), want, "{event:?}");
    }
}

#[test]
fn every_value_of_every_call_field_has_its_own_literal() {
    // Acceptance 1, per value: a table that maps two values to one literal, or to `other`,
    // is caught here, which the single examples above would not.
    let line = |e: LogEvent| {
        let s = level_and_body(&e);
        s.split_once(' ').map(|(_, b)| b.to_string()).unwrap_or(s)
    };
    for (part, p) in REFUSED_PARTS_ALL {
        for (reason, r) in REFUSED_REASONS_ALL {
            assert_eq!(
                line(LogEvent::CallRefused { part, reason }),
                format!("call_refused part={p} reason={r}")
            );
        }
    }
    for (track, t) in TRACKS {
        for (cause, c) in CAUSES_ALL {
            assert_eq!(
                line(LogEvent::CallDeviceChanged {
                    track,
                    cause,
                    gap_ms: 1,
                    to: None
                }),
                format!("call_device track={t} cause={c} gap_ms=1 to=none")
            );
        }
        for (stage, s) in STAGES_ALL {
            assert_eq!(
                line(LogEvent::FinalPassStage {
                    track,
                    stage,
                    ms: 2,
                    chunks: 3
                }),
                format!("final_stage track={t} stage={s} ms=2 chunks=3")
            );
        }
    }
    for (reason, r) in STOPS_ALL {
        assert_eq!(
            line(LogEvent::CallStopped {
                reason,
                duration_ms: 1,
                mic_bytes: 2,
                system_bytes: 3
            }),
            format!("call_stopped reason={r} duration_ms=1 mic_bytes=2 system_bytes=3")
        );
    }
    for (kind, k) in STORAGE_ALL {
        assert_eq!(
            line(LogEvent::CallStorageFailed { kind, os: Some(1) }),
            format!("call_storage_failed kind={k} os=1")
        );
    }
    for (reason, r) in FAILS_ALL {
        assert_eq!(
            line(LogEvent::FinalPassFinished {
                outcome: FinalOutcome::Failed { reason },
                mode: PassMode::Timed,
                ms: 1
            }),
            format!("final_finished outcome=failed reason={r} mode=timed ms=1")
        );
    }
    for (rule, r) in RULES_ALL {
        assert_eq!(
            line(LogEvent::TranscriptInvalid { rule, segment: 0 }),
            format!("transcript_invalid rule={r} segment=0")
        );
    }
}

#[test]
fn an_endpoint_outside_the_charset_or_length_formats_as_invalid() {
    // Acceptance 2, the failure branch, end to end through both lines that carry an
    // endpoint. Bite: Display echoing the text, a length check of 129 or 256, a charset
    // that also lets space, `=`, `\` or a letter past `f` through, `to=` written for None.
    let long = "a".repeat(129);
    let longest = "a".repeat(128);
    let bad: Vec<&str> = vec![
        "",
        "Headset (Realtek Audio)",
        "{abc} key=value",
        "{abc}\nINJECT=1",
        "{abcg}",
        "\\\\?\\SWD#MMDEVAPI#{abc}",
        &long,
    ];
    for text in bad {
        let started = level_and_body(&LogEvent::CallStarted {
            mic: EndpointId::new(text),
            system: EndpointId::new(MIC_ID),
            comms_differs: false,
        });
        assert_eq!(
            started,
            format!("INFO call_started record=1 subtitles=0 mic=invalid system={MIC_ID} comms_differs=0"),
            "{text:?}"
        );
        let device = level_and_body(&LogEvent::CallDeviceChanged {
            track: Track::Mic,
            cause: DeviceCause::MicDefaultChanged,
            gap_ms: 9,
            to: Some(EndpointId::new(text)),
        });
        assert_eq!(
            device, "INFO call_device track=mic cause=mic_default_changed gap_ms=9 to=invalid",
            "{text:?}"
        );
    }
    // The limit is inclusive and the kept text is written whole.
    let kept = level_and_body(&LogEvent::CallDeviceChanged {
        track: Track::System,
        cause: DeviceCause::TrackLost,
        gap_ms: 9,
        to: Some(EndpointId::new(&longest)),
    });
    assert_eq!(
        kept,
        format!("INFO call_device track=system cause=track_lost gap_ms=9 to={longest}")
    );
}

#[test]
fn transcript_invalid_names_the_segment_by_index_only() {
    // Acceptance 1 "never with segment text": the line has exactly two keys, whatever the
    // values. Bite: an extra key (text=, speaker=) or a title.
    for (rule, _) in RULES_ALL {
        let raw = format_line(
            at(NOON_UTC, 0),
            0,
            &LogEvent::TranscriptInvalid {
                rule,
                segment: u32::MAX,
            },
        );
        let l = closed(&raw);
        assert_eq!(l.keys(), ["rule", "segment"], "{raw}");
        assert_eq!(l.get("segment"), Some("4294967295"));
    }
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
    let engine = match engine_for(&s, &creds, None) {
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
        FailureReason::Timeout {
            origin: TimeoutOrigin::Server,
        },
        FailureReason::ServerError { status: 503 },
        FailureReason::UnexpectedResponse,
        FailureReason::KeyStoreUnavailable,
        FailureReason::EngineNotConfigured,
        FailureReason::ClipboardUnavailable,
        FailureReason::MicrophoneUnavailable {
            cause: MicCause::NoDevice,
        },
        FailureReason::NoLocalModel,
        FailureReason::LocalEngineFailed,
    ];
    all.into_iter()
        .map(|r| {
            let (tag, literal) = match &r {
                FailureReason::InvalidApiKey => (FailureTag::InvalidApiKey, "invalid_api_key"),
                FailureReason::NetworkUnavailable => {
                    (FailureTag::NetworkUnavailable, "network_unavailable")
                }
                FailureReason::CannotReach { .. } => (FailureTag::CannotReach, "cannot_reach"),
                FailureReason::Timeout { .. } => (FailureTag::Timeout, "timeout"),
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
                FailureReason::NoLocalModel => (FailureTag::NoLocalModel, "no_local_model"),
                FailureReason::LocalEngineFailed => {
                    (FailureTag::LocalEngineFailed, "local_engine_failed")
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
fn started_line_carries_version_commit_pid_and_os() {
    // FR-18, spec 006 FR-003 (T-105) and the Windows CI smoke (`(<commit>) started`,
    // `started pid=<n>` then a non-digit). Bite: another word order, the commit
    // without parentheses, no pid, no os, os before pid, os written in another form
    // (Debug, two parts), the real build info not used verbatim.
    let raw = format_line(
        at(NOON_UTC, 0),
        0,
        &LogEvent::Started {
            build: fake_build(),
            pid: 4242,
            os: WIN11,
        },
    );
    assert!(
        raw.contains(" voicen 0.1.0 (abc1234) started"),
        "start line {raw:?}"
    );
    let l = parsed(&raw);
    assert_eq!(l.head, "started");
    assert_eq!(l.level, "INFO");
    assert_eq!(
        l.pairs,
        pairs(&[
            ("pid", "4242".to_string()),
            ("os", "10.0.26100".to_string())
        ])
    );
    assert_eq!(check_closed(&l), Ok(()), "{raw}");
    // The message as the shell test (src-tauri/tests/diag.rs) reads it.
    assert!(
        raw.ends_with(" INFO voicen 0.1.0 (abc1234) started pid=4242 os=10.0.26100"),
        "{raw}"
    );

    let info = voicen_core::build_info();
    let raw = format_line(
        at(NOON_UTC, 0),
        0,
        &LogEvent::Started {
            build: info.clone(),
            pid: 1,
            os: OsVersion {
                major: 6,
                minor: 3,
                build: 9600,
            },
        },
    );
    assert!(
        raw.contains(&format!("voicen {info} started")),
        "start line {raw:?} for {info}"
    );
    assert!(raw.contains(&format!("({}) started", info.commit)), "{raw}");
    // The os value is the event's, not a constant.
    assert!(raw.ends_with(" started pid=1 os=6.3.9600"), "{raw}");
}

#[test]
fn started_line_with_an_unreadable_os_version_writes_the_placeholder() {
    // T-105 failure branch: RtlGetVersion fails -> the shell passes the closed
    // placeholder OsVersion::UNKNOWN; the line is still written, with os=0.0.0 (the
    // value the same session's crash files carry), no free text and no missing key.
    // Bite: UNKNOWN another value, os= dropped or written as `unknown` for it.
    assert_eq!(
        OsVersion::UNKNOWN,
        OsVersion {
            major: 0,
            minor: 0,
            build: 0,
        }
    );
    let raw = format_line(
        at(NOON_UTC, 0),
        0,
        &LogEvent::Started {
            build: fake_build(),
            pid: 4242,
            os: OsVersion::UNKNOWN,
        },
    );
    assert!(
        raw.ends_with(" INFO voicen 0.1.0 (abc1234) started pid=4242 os=0.0.0"),
        "{raw}"
    );
    let l = parsed(&raw);
    assert_eq!(check_closed(&l), Ok(()), "{raw}");
    assert_eq!(l.get("os"), Some("0.0.0"));
}

#[test]
fn the_started_grammar_requires_os_in_u32_dotted_form() {
    // The T-105 rows of diag_support are not toothless: a start line without os=,
    // with os before pid, or with an os value that is not three u32 parts is
    // refused; the well-formed ones (the placeholder included) pass. Bite (on
    // diag_support): os not required, os accepted as any value, the pid-first rule
    // dropped.
    let ts = "2026-10-04T12:00:00.000+00:00";
    let head = "INFO voicen 0.1.0 (abc1234) started";
    let ok = [
        format!("{ts} {head} pid=4242 os=10.0.26100"),
        format!("{ts} {head} pid=4242 os=0.0.0"),
        format!("{ts} {head} pid=1 os=4294967295.4294967295.4294967295"),
    ];
    for raw in &ok {
        let l = parsed(raw);
        assert_eq!(check_closed(&l), Ok(()), "{raw}");
    }
    let refused = [
        format!("{ts} {head} pid=4242"),
        format!("{ts} {head} os=10.0.26100"),
        format!("{ts} {head} os=10.0.26100 pid=4242"),
        format!("{ts} {head} pid=4242 os=unknown"),
        format!("{ts} {head} pid=4242 os=10.0"),
        format!("{ts} {head} pid=4242 os=10.0.26100.1"),
        format!("{ts} {head} pid=4242 os=10..26100"),
        format!("{ts} {head} pid=4242 os=10.0.x"),
        format!("{ts} {head} pid=4242 os=-1.0.26100"),
        format!("{ts} {head} pid=4242 os=+10.0.26100"),
        format!("{ts} {head} pid=4242 os=4294967296.0.0"),
        format!("{ts} {head} pid=4242 os=10.0.26100 build=1"),
    ];
    for raw in &refused {
        let refused_here = parse(raw).and_then(|l| check_closed(&l)).is_err();
        assert!(refused_here, "the grammar accepted {raw:?}");
    }
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
        post_processing: None,
        local: None,
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
        post_processing: None,
        local: None,
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
                post_processing: None,
                local: None,
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
                post_processing: None,
                local: None,
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

// ---- T-076: the post-processing keys ---------------------------------------------

fn delivered_line(post_processing: Option<PostProcessTrace>) -> LogEvent {
    LogEvent::Dictation(DictationLine {
        recording: 3,
        engine: Some(EngineTag::Api),
        outcome: DictationOutcome::Delivered(DeliveryResult::Pasted),
        detector: Some(DetectorTag::Energy),
        press_to_frame_ms: Some(41),
        duration_ms: Some(3_000),
        stop_to_text_ms: Some(15_900),
        text_to_paste_ms: Some(95),
        post_processing,
        local: None,
    })
}

fn delivered_pairs() -> Vec<(&'static str, String)> {
    vec![
        ("rec", "3".to_string()),
        ("engine", "api".to_string()),
        ("outcome", "delivered".to_string()),
        ("result", "pasted".to_string()),
        ("detector", "energy".to_string()),
        ("press_to_frame_ms", "41".to_string()),
        ("duration_ms", "3000".to_string()),
        ("stop_to_text_ms", "15900".to_string()),
        ("text_to_paste_ms", "95".to_string()),
    ]
}

#[test]
fn dictation_line_carries_pp_keys_per_post_processing_result() {
    // T-076 / spec 003 FR-014, US3-2: a traced job's line has pp=<off|applied|
    // skipped>, pp_reason=<one of the six kinds> exactly for a skip, pp_ms=<n>
    // exactly for applied or skipped (off has a trace but no step to time), each
    // value from its own literal; a skip on a delivered line stays INFO. Bite: no
    // pp keys written, the result or kind Debug-formatted ("Skipped(Timeout)",
    // "InvalidKey"), two kinds on one literal, pp_reason on every line, pp_ms on
    // an off line or missing on a skip, a skip raising the level to WARN.
    let mut pp_literals = BTreeSet::new();
    let mut reason_literals = BTreeSet::new();
    for (result, pp, reason) in all_pp_results() {
        let raw = format_line(
            at(NOON_UTC, 0),
            0,
            &delivered_line(Some(PostProcessTrace { result, ms: 15_004 })),
        );
        let l = parsed(&raw);
        assert_eq!(l.level, "INFO", "{result:?}: {raw}");
        let mut want = delivered_pairs();
        want.push(("pp", pp.to_string()));
        if let Some(reason) = reason {
            want.push(("pp_reason", reason.to_string()));
            assert!(reason_literals.insert(reason), "{reason} used twice");
        }
        if result != PostProcessResult::Off {
            want.push(("pp_ms", "15004".to_string()));
        }
        assert_eq!(l.pairs, pairs(&want), "{result:?}: {raw}");
        check_closed(&l).unwrap_or_else(|e| panic!("{result:?}: {e}"));
        pp_literals.insert(pp);
    }
    assert_eq!(
        pp_literals.len(),
        3,
        "off, applied, skipped: {pp_literals:?}"
    );
    let want_pp: BTreeSet<&str> = PP_RESULTS.iter().copied().collect();
    assert_eq!(pp_literals, want_pp);
    let want_reasons: BTreeSet<&str> = PP_REASONS.iter().copied().collect();
    assert_eq!(reason_literals, want_reasons);
}

#[test]
fn a_line_without_a_trace_has_no_pp_key() {
    // T-076 interpretation: no trace (the stage was not called: no speech before
    // it, a failed transcription, no engine) writes no pp key at all, not pp=off.
    // Bite: None written as pp=off or pp_ms=0.
    let raw = format_line(at(NOON_UTC, 0), 0, &delivered_line(None));
    let l = parsed(&raw);
    assert_eq!(l.pairs, pairs(&delivered_pairs()), "{raw}");
    assert!(
        l.keys().iter().all(|k| !k.starts_with("pp")),
        "no pp key without a trace: {raw}"
    );
}

#[test]
fn pp_keys_ride_on_failed_and_no_speech_lines_too() {
    // T-076: a job that reached the stage keeps its trace whatever closes it: a
    // clipboard failure after post-processing (failed, WARN) and a reply that was
    // blank after post-processing (no_speech). A real 0 ms is written. Bite: the
    // trace written only on delivered lines, the level keyed on the pp result,
    // pp_ms=0 dropped.
    let failed = LogEvent::Dictation(DictationLine {
        recording: 4,
        engine: Some(EngineTag::Api),
        outcome: DictationOutcome::Failed {
            failure: FailureTag::ClipboardUnavailable,
            http_status: None,
        },
        detector: Some(DetectorTag::Energy),
        press_to_frame_ms: None,
        duration_ms: Some(2_000),
        stop_to_text_ms: Some(900),
        text_to_paste_ms: None,
        post_processing: Some(PostProcessTrace {
            result: PostProcessResult::Applied,
            ms: 812,
        }),
        local: None,
    });
    let raw = format_line(at(NOON_UTC, 0), 0, &failed);
    let l = parsed(&raw);
    assert_eq!(l.level, "WARN", "{raw}");
    assert_eq!(
        l.pairs,
        pairs(&[
            ("rec", "4".to_string()),
            ("engine", "api".to_string()),
            ("outcome", "failed".to_string()),
            ("failure", "clipboard_unavailable".to_string()),
            ("detector", "energy".to_string()),
            ("duration_ms", "2000".to_string()),
            ("stop_to_text_ms", "900".to_string()),
            ("pp", "applied".to_string()),
            ("pp_ms", "812".to_string()),
        ]),
        "{raw}"
    );

    let no_speech = LogEvent::Dictation(DictationLine {
        recording: 5,
        engine: Some(EngineTag::LocalServer),
        outcome: DictationOutcome::NoSpeech,
        detector: Some(DetectorTag::Silero),
        press_to_frame_ms: None,
        duration_ms: None,
        stop_to_text_ms: Some(700),
        text_to_paste_ms: None,
        post_processing: Some(PostProcessTrace {
            result: PostProcessResult::Skipped(SkipKind::Http),
            ms: 0,
        }),
        local: None,
    });
    let raw = format_line(at(NOON_UTC, 0), 0, &no_speech);
    let l = parsed(&raw);
    assert_eq!(l.level, "INFO", "{raw}");
    assert_eq!(
        l.pairs,
        pairs(&[
            ("rec", "5".to_string()),
            ("engine", "local_server".to_string()),
            ("outcome", "no_speech".to_string()),
            ("detector", "silero".to_string()),
            ("stop_to_text_ms", "700".to_string()),
            ("pp", "skipped".to_string()),
            ("pp_reason", "http".to_string()),
            ("pp_ms", "0".to_string()),
        ]),
        "{raw}"
    );
}

#[test]
fn the_pp_grammar_refuses_keys_outside_their_sets_and_pairings() {
    // The grammar rows of T-076 in diag_support are not toothless: each line below
    // breaks exactly one pp rule and is refused; the well-formed ones pass. Bite (on
    // diag_support): pp or pp_reason accepted as any word, pp_ms accepted on off,
    // pp_reason without pp=skipped, pp on a recording that had no job.
    let ts = "2026-10-04T12:00:00.000+00:00";
    let base = "dictation rec=3 engine=api outcome=delivered result=pasted";
    let ok = [
        format!("{ts} INFO {base} pp=off"),
        format!("{ts} INFO {base} pp=applied pp_ms=12"),
        format!("{ts} INFO {base} pp=skipped pp_reason=timeout pp_ms=15004"),
        format!("{ts} WARN dictation rec=3 outcome=failed failure=clipboard_unavailable pp=applied pp_ms=0"),
        format!("{ts} INFO dictation rec=3 outcome=no_speech pp=skipped pp_reason=http pp_ms=9"),
        format!("{ts} INFO {base}"),
    ];
    for raw in &ok {
        let l = parsed(raw);
        assert_eq!(check_closed(&l), Ok(()), "{raw}");
    }
    let refused = [
        format!("{ts} INFO {base} pp=bogus"),
        format!("{ts} INFO {base} pp=skipped pp_reason=api.example.com pp_ms=1"),
        format!("{ts} INFO {base} pp=skipped pp_reason=http_500 pp_ms=1"),
        format!("{ts} INFO {base} pp=skipped pp_ms=1"),
        format!("{ts} INFO {base} pp=applied pp_reason=timeout pp_ms=1"),
        format!("{ts} INFO {base} pp=off pp_ms=3"),
        format!("{ts} INFO {base} pp=applied"),
        format!("{ts} INFO {base} pp=skipped pp_reason=timeout"),
        format!("{ts} INFO {base} pp_ms=3"),
        format!("{ts} INFO {base} pp_reason=timeout"),
        format!("{ts} INFO {base} pp=applied pp_ms=x1"),
        format!("{ts} INFO dictation rec=3 outcome=too_short pp=off"),
        format!("{ts} INFO dictation rec=3 outcome=cancelled pp=applied pp_ms=1"),
        format!("{ts} WARN dictation rec=3 outcome=capture_failed mic=busy pp=off"),
        format!("{ts} WARN dictation outcome=blocked reason=no_engine pp=off"),
    ];
    for raw in &refused {
        let refused_here = parse(raw).and_then(|l| check_closed(&l)).is_err();
        assert!(refused_here, "the grammar accepted {raw:?}");
    }
    assert_eq!(PP_RESULTS.len(), 3);
    assert_eq!(PP_REASONS.len(), all_skip_kinds().len());
}
