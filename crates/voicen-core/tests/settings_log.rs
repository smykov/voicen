//! T-008: the settings and autostart log lines (analysis approach 8; spec 004
//! R-11 / T035, decisions #23 N6, #30).
//!
//! `LogEvent::settings_load(&LoadOutcome)` and `LogEvent::settings_save(&SaveOutcome)`
//! keep only the closed parts of an outcome (the outcome kind, field ids, error /
//! warning codes, the form error); `LogEvent::AutostartReconcile` carries the
//! `ReconcileAction`. The outcomes come from the real `SettingsService` over fakes.
//! No settings value reaches a line: not the base URL or its query string
//! (decision #30), not the host, model, prompt, microphone name, backup file name
//! or a key. Spec 004 R-11's `changed=<field ids>` is not logged (T-008 Q4,
//! decision #64).

mod diag_support;

use std::io;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use diag_support::{at, closed, pairs, NOON_UTC};
use voicen_core::autostart::{FakeAutostart, ReconcileAction};
use voicen_core::clock::FakeClock;
use voicen_core::connection_test::ConnectionTestResult;
use voicen_core::diag::{format_line, LoadKind, LogEvent, SaveLine};
use voicen_core::hotkey_registrar::FakeHotkeyRegistrar;
use voicen_core::models::FakeDownloadedModels;
use voicen_core::secrets::{FakeCredentialStore, KeyEdit, KeyEdits, Secret};
use voicen_core::settings::file::FakeSettingsFile;
use voicen_core::settings::service::{
    FormError, SaveOutcome, SaveRequest, SettingsDeps, SettingsService, Warning, WarningCode,
};
use voicen_core::settings::{
    defaults, EngineKind, ErrorCode, FieldError, FieldId, LoadOutcome, Microphone, Settings,
};

const OS: Option<&str> = Some("en-US");
const KEY: &str = "sk-test-SECRET";
const QUERY_SECRET: &str = "SECRETQ";
/// Values that must never reach a line, with the planted settings that hold them.
const VALUES: &[&str] = &[
    KEY,
    QUERY_SECRET,
    "example.com",
    "192.0.2.10",
    "MODEL-MARKER",
    "PROMPT-MARKER",
    "MIC-MARKER",
    "settings.json.bad",
    "PLANTED",
];

fn deps(file: Arc<FakeSettingsFile>, autostart: Arc<FakeAutostart>) -> SettingsDeps {
    SettingsDeps {
        file,
        credentials: Arc::new(FakeCredentialStore::new()),
        autostart,
        hotkeys: Arc::new(FakeHotkeyRegistrar::new()),
        local_models: Arc::new(FakeDownloadedModels::new(&["base"])),
        clock: Arc::new(FakeClock::at(
            UNIX_EPOCH + Duration::from_secs(1_791_115_200),
        )),
    }
}

/// Settings that hold every value of [`VALUES`] but the key.
fn planted(base_url: &str) -> Settings {
    let mut s = defaults(OS);
    s.engine = EngineKind::Api;
    s.api.base_url = base_url.to_string();
    s.api.model = "MODEL-MARKER".to_string();
    s.post_processing.prompt = "PROMPT-MARKER".to_string();
    s.microphone = Some(Microphone {
        id: "{0.0.1.00000000}.{MIC-MARKER}".to_string(),
        name: "MIC-MARKER".to_string(),
    });
    s
}

fn with_query(base: &str) -> String {
    format!("{base}?api-version={QUERY_SECRET}")
}

fn keys() -> KeyEdits {
    KeyEdits {
        transcription_api: KeyEdit::Replace(Secret::new(KEY)),
        local_server: KeyEdit::Untouched,
        post_processing: KeyEdit::Untouched,
    }
}

fn service_over(file: FakeSettingsFile) -> (SettingsService, LoadOutcome) {
    SettingsService::load_or_init(deps(Arc::new(file), Arc::new(FakeAutostart::new())), OS)
}

/// The formatted line of `event`, checked against the grammar and the closed sets
/// and for every planted value.
#[track_caller]
fn line_of(event: &LogEvent) -> diag_support::Line {
    let raw = format_line(at(NOON_UTC, 0), 0, event);
    for v in VALUES {
        assert!(!raw.contains(v), "{v} reached the line {raw:?}");
    }
    closed(&raw)
}

fn s(v: &str) -> String {
    v.to_string()
}

#[test]
fn every_load_outcome_maps_to_its_load_line() {
    // R-11 `settings load outcome=<...>` for each LoadOutcome of the real
    // load_or_init: first run (no file), loaded, reset (unreadable JSON, moved
    // aside), unavailable (read error). Bite: Debug of the outcome on the line
    // (settings values, the backup file name), two outcomes on one literal.
    let stored =
        serde_json::to_vec(&planted(&with_query("https://api.example.com/v1"))).expect("serialize");
    let unreadable = FakeSettingsFile::new();
    unreadable.fail_read(io::ErrorKind::PermissionDenied);
    let cases: Vec<(FakeSettingsFile, LoadKind, &str)> = vec![
        (FakeSettingsFile::new(), LoadKind::FirstRun, "first_run"),
        (
            FakeSettingsFile::with_bytes(&stored),
            LoadKind::Loaded,
            "loaded",
        ),
        (
            FakeSettingsFile::with_bytes(b"{ PLANTED not json"),
            LoadKind::Reset,
            "reset",
        ),
        (unreadable, LoadKind::Unavailable, "unavailable"),
    ];
    for (file, kind, literal) in cases {
        let (_, outcome) = service_over(file);
        let event = LogEvent::settings_load(&outcome);
        assert_eq!(event, LogEvent::SettingsLoad(kind), "{outcome:?}");
        let l = line_of(&event);
        assert_eq!(l.head, "settings load");
        assert_eq!(l.pairs, pairs(&[("outcome", s(literal))]), "{}", l.raw);
    }
}

#[test]
fn a_saved_outcome_is_ok_with_its_warnings_and_no_values() {
    // R-11 `settings save outcome=ok warnings=<field:code>`; decision #30: the
    // base URL's query string never appears. The view (settings, key presence)
    // is dropped. Bite: the SettingsView or the request on the line, a warning
    // without its field, the URL in a warning.
    let stored = serde_json::to_vec(&defaults(OS)).expect("serialize");
    let (service, _) = service_over(FakeSettingsFile::with_bytes(&stored));
    let outcome = service.save(SaveRequest {
        settings: planted(&with_query("http://192.0.2.10:8000/v1")),
        keys: keys(),
    });
    let SaveOutcome::Saved { warnings, .. } = &outcome else {
        panic!("expected Saved: {outcome:?}");
    };
    assert_eq!(
        warnings,
        &vec![Warning {
            field: FieldId::EngineApiBaseUrl,
            code: WarningCode::EndpointInsecure
        }],
        "premise: the plain-http remote URL gives one warning"
    );

    let event = LogEvent::settings_save(&outcome);
    assert_eq!(
        event,
        LogEvent::SettingsSave(SaveLine::Saved {
            warnings: warnings.clone()
        })
    );
    let l = line_of(&event);
    assert_eq!(l.head, "settings save");
    assert_eq!(
        l.pairs,
        pairs(&[
            ("outcome", s("ok")),
            ("warnings", s("engine.api.base_url:endpoint.insecure")),
        ]),
        "{}",
        l.raw
    );
    // The message as the shell test (src-tauri/tests/diag.rs) reads it: outcome
    // first.
    assert!(
        l.raw
            .ends_with(" settings save outcome=ok warnings=engine.api.base_url:endpoint.insecure"),
        "{}",
        l.raw
    );

    let quiet = service.save(SaveRequest {
        settings: planted(&with_query("https://api.example.com/v1")),
        keys: keys(),
    });
    let l = line_of(&LogEvent::settings_save(&quiet));
    assert_eq!(
        l.pairs,
        pairs(&[("outcome", s("ok"))]),
        "no warnings key: {}",
        l.raw
    );
}

#[test]
fn a_refused_outcome_lists_field_ids_and_codes_in_order() {
    // R-11 `errors=<field ids:codes>`, in the outcome's order. A base URL with
    // userinfo (decision #27(2)) carrying a planted key and the query, and an
    // out-of-range history size. Bite: the refused URL or its userinfo on the
    // line, the codes without their fields, the list re-ordered.
    let stored = serde_json::to_vec(&defaults(OS)).expect("serialize");
    let (service, _) = service_over(FakeSettingsFile::with_bytes(&stored));
    let mut draft = planted(&with_query(
        "https://user:sk-test-PLANTED@api.example.com/v1",
    ));
    draft.history.size = 0;
    let outcome = service.save(SaveRequest {
        settings: draft,
        keys: keys(),
    });
    let SaveOutcome::Refused { errors, form_error } = &outcome else {
        panic!("expected Refused: {outcome:?}");
    };
    assert_eq!(form_error, &None);
    assert!(
        errors.contains(&FieldError {
            field: FieldId::EngineApiBaseUrl,
            code: ErrorCode::UrlCredentials
        }) && errors.contains(&FieldError {
            field: FieldId::HistorySize,
            code: ErrorCode::HistorySizeRange
        }),
        "premise: {errors:?}"
    );

    let event = LogEvent::settings_save(&outcome);
    assert_eq!(
        event,
        LogEvent::SettingsSave(SaveLine::Refused {
            errors: errors.clone(),
            form_error: None
        })
    );
    let want = errors
        .iter()
        .map(|e| format!("{}:{}", e.field.as_str(), e.code.as_str()))
        .collect::<Vec<_>>()
        .join(",");
    let l = line_of(&event);
    assert_eq!(
        l.pairs,
        pairs(&[("outcome", s("refused")), ("errors", want.clone())]),
        "{}",
        l.raw
    );
    assert!(
        l.raw
            .ends_with(&format!(" settings save outcome=refused errors={want}")),
        "outcome first: {}",
        l.raw
    );
}

#[test]
fn form_errors_map_to_failed_or_refused_with_their_kind() {
    // R-11 outcome=<ok|refused|failed>: a file that cannot be written and an
    // undo that did not restore everything are `failed`; the Unavailable
    // service's refusal is `refused`. form_error uses FormError's wire kind;
    // not_restored lists field ids. Bite: every refusal logged as refused (or as
    // failed), the form error dropped, not_restored missing.
    let stored = serde_json::to_vec(&defaults(OS)).expect("serialize");
    let file = FakeSettingsFile::with_bytes(&stored);
    file.fail_write(io::ErrorKind::PermissionDenied);
    let (service, _) = service_over(file);
    let write_failed = service.save(SaveRequest {
        settings: planted(&with_query("https://api.example.com/v1")),
        keys: keys(),
    });
    assert_eq!(
        write_failed,
        SaveOutcome::Refused {
            errors: vec![],
            form_error: Some(FormError::WriteFailed)
        },
        "premise"
    );
    let l = line_of(&LogEvent::settings_save(&write_failed));
    assert_eq!(
        l.pairs,
        pairs(&[("outcome", s("failed")), ("form_error", s("write_failed"))]),
        "{}",
        l.raw
    );

    let unreadable = FakeSettingsFile::new();
    unreadable.fail_read(io::ErrorKind::PermissionDenied);
    let (service, _) = service_over(unreadable);
    let unavailable = service.save(SaveRequest {
        settings: planted(&with_query("https://api.example.com/v1")),
        keys: keys(),
    });
    assert_eq!(
        unavailable,
        SaveOutcome::Refused {
            errors: vec![],
            form_error: Some(FormError::SettingsUnavailable)
        },
        "premise"
    );
    let l = line_of(&LogEvent::settings_save(&unavailable));
    assert_eq!(
        l.pairs,
        pairs(&[
            ("outcome", s("refused")),
            ("form_error", s("settings_unavailable"))
        ]),
        "{}",
        l.raw
    );

    // The undo failure is built directly: the fakes reach it only through a
    // multi-step failure sequence that settings tests already pin.
    let partial = SaveOutcome::Refused {
        errors: vec![FieldError {
            field: FieldId::PostProcessingKey,
            code: ErrorCode::KeyStoreFailed,
        }],
        form_error: Some(FormError::PartiallyRestored {
            not_restored: vec![FieldId::GeneralStartWithWindows, FieldId::EngineApiKey],
        }),
    };
    let l = line_of(&LogEvent::settings_save(&partial));
    assert_eq!(
        l.pairs,
        pairs(&[
            ("outcome", s("failed")),
            ("errors", s("post_processing.key:key.store_failed")),
            ("form_error", s("partially_restored")),
            (
                "not_restored",
                s("general.start_with_windows,engine.api.key")
            ),
        ]),
        "{}",
        l.raw
    );
}

#[test]
fn reconcile_actions_map_to_their_codes() {
    // R-11 `autostart reconcile action=<none|written|removed|failed>`
    // (ReconcileAction::as_str), which load_settings discarded before T-008
    // (T-014 review round 1 #2). The real reconcile of a fresh install with a
    // leftover Run value gives `removed`. Bite: Debug spelling ("Removed"), the
    // action dropped.
    for action in [
        ReconcileAction::None,
        ReconcileAction::Written,
        ReconcileAction::Removed,
        ReconcileAction::Failed,
    ] {
        let l = line_of(&LogEvent::AutostartReconcile(action));
        assert_eq!(l.head, "autostart reconcile");
        assert_eq!(
            l.pairs,
            pairs(&[("action", s(action.as_str()))]),
            "{}",
            l.raw
        );
    }

    let (service, outcome) = SettingsService::load_or_init(
        deps(
            Arc::new(FakeSettingsFile::new()),
            Arc::new(FakeAutostart::enabled()),
        ),
        OS,
    );
    assert!(matches!(outcome, LoadOutcome::FirstRun(_)), "{outcome:?}");
    let action = service.reconcile_autostart();
    assert_eq!(action, ReconcileAction::Removed, "premise");
    let l = line_of(&LogEvent::AutostartReconcile(action));
    assert_eq!(l.get("action"), Some("removed"));
}

#[test]
fn every_test_connection_result_maps_to_its_line_without_host() {
    // R-11 / T-046 choice (iv): `settings test_connection result=<kind>
    // [latency_ms=<n>]`, one literal per ConnectionTestResult kind, the latency
    // only on `ok`, no host (T-008 Q1: "no values, no URLs, no hosts"), no status
    // text, no field values. Bite: Debug of the result on the line (the host),
    // http without its status, two kinds on one literal, the latency dropped or
    // put on a failure line.
    let cases: Vec<(ConnectionTestResult, Vec<(&str, String)>)> = vec![
        (
            ConnectionTestResult::Ok { latency_ms: 842 },
            vec![("result", s("ok")), ("latency_ms", s("842"))],
        ),
        (
            ConnectionTestResult::CannotReach {
                host: "api.example.com:8443".to_string(),
            },
            vec![("result", s("cannot_reach"))],
        ),
        (
            ConnectionTestResult::CannotReach {
                host: "192.0.2.10".to_string(),
            },
            vec![("result", s("cannot_reach"))],
        ),
        (
            ConnectionTestResult::InvalidKey,
            vec![("result", s("invalid_key"))],
        ),
        (
            ConnectionTestResult::Timeout,
            vec![("result", s("timeout"))],
        ),
        (
            ConnectionTestResult::Http { status: 503 },
            vec![("result", s("http_503"))],
        ),
        (
            ConnectionTestResult::UnexpectedResponse,
            vec![("result", s("unexpected"))],
        ),
        (
            ConnectionTestResult::Invalid {
                errors: vec![FieldError {
                    field: FieldId::EngineApiBaseUrl,
                    code: ErrorCode::UrlMalformed,
                }],
            },
            vec![("result", s("invalid"))],
        ),
        (
            ConnectionTestResult::KeyStoreUnavailable,
            vec![("result", s("key_store_unavailable"))],
        ),
    ];
    for (result, expected) in cases {
        let l = line_of(&LogEvent::settings_test_connection(&result));
        assert_eq!(l.head, "settings test_connection", "{}", l.raw);
        assert_eq!(l.pairs, pairs(&expected), "{result:?}: {}", l.raw);
        assert!(
            !l.raw.contains("8443"),
            "the port reached the line {:?}",
            l.raw
        );
    }
}
