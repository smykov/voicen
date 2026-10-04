//! T-008: the redaction run (analysis approach 3; FR-20, NFR-04, P-009).
//!
//! The eight leak scenarios of `api_pipeline::no_event_or_failure_carries_key_
//! query_or_transcript` (pasted, copied only, clipboard fails, 401, 500, bad body,
//! timeout, refused), each through a real `Pipeline` against a mock server, with
//! a `LogObserver` as the pipeline's observer, all writing into one `Log` over a
//! temp dir; plus the settings save and load lines of a configuration whose base
//! URL carries the query secret and whose keys are the key. No byte of the key,
//! the base URL's query or the transcript reaches any file under the logs dir.
//! Positive controls: the key and the query were sent, the transcript reached the
//! clipboard, and every scenario left its own dictation line.
//!
//! The harness is a compact copy of api_pipeline's (recordings made through the
//! real `RecordingController`, `run_job` on a plain std thread, wiremock on its
//! own runtime); T-006's `RecordingStarted` / `RecordingEnded` are emitted on the
//! same observer the way the capture worker will. Fake data only: 127.0.0.1,
//! `sk-test-SECRET`. The refused scenario uses `common::refused_addr()` and
//! `common::refused_timeouts()` (F-004, F-005).

mod common;
mod diag_support;
/// The checks of `common::refused_addr()`, in each binary that calls it (T-047).
#[path = "common/refused_addr_tests.rs"]
mod refused_addr_tests;

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use common::{refused_addr, refused_timeouts};
use diag_support::{
    all_lines, closed, contains, dictation_lines, files_under, open_log, utf16le, Line,
};
use serde_json::json;
use voicen_core::autostart::FakeAutostart;
use voicen_core::clock::FakeClock;
use voicen_core::diag::{Log, LogConfig, LogEvent, LogObserver};
use voicen_core::events::{DeviceKind, DictationEvent, PipelineObserver};
use voicen_core::failure::FailureReason;
use voicen_core::hotkey_registrar::FakeHotkeyRegistrar;
use voicen_core::i18n;
use voicen_core::models::FakeDownloadedModels;
use voicen_core::pipeline::{JobReport, Pipeline, PipelineDeps, PressContext};
use voicen_core::platform::{
    FakeClipboard, FakePaster, FakeTempAudioStore, StartWindow, WindowRef,
};
use voicen_core::post_process::PassThrough;
use voicen_core::recording::{
    JobEnd, Press, RecordingController, RecordingEnd, RecordingId, Release,
};
use voicen_core::secrets::{FakeCredentialStore, KeyEdit, KeyEdits, KeySlot, Secret};
use voicen_core::settings::file::FakeSettingsFile;
use voicen_core::settings::service::{SaveOutcome, SaveRequest, SettingsDeps, SettingsService};
use voicen_core::settings::{defaults, EngineKind, LoadOutcome, Settings};
use voicen_core::test_support::{fixtures, TempDir};
use voicen_core::timeouts::Timeouts;
use voicen_core::vad::{EnergyDetector, SpeechDetector, SpeechGate};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-test-SECRET";
const QUERY_SECRET: &str = "SECRETQ";
const TRANSCRIPT: &str = "TRANSCRIPT-MARKER";
const OS: Option<&str> = Some("en-US");
/// T-006's press -> first frame and the hold, as the capture worker reports them.
const PRESS_TO_FRAME_MS: u64 = 37;
const HOLD_MS: u64 = 3_000;

// ---- harness ------------------------------------------------------------------------

fn energy_gate() -> SpeechGate {
    SpeechGate::new(
        Ok(Box::new(EnergyDetector::new()) as Box<dyn SpeechDetector>),
        EnergyDetector::new(),
    )
}

fn api_settings(base: &str) -> Settings {
    let mut s = defaults(OS);
    s.engine = EngineKind::Api;
    s.api.base_url = base.to_string();
    s.api.model = "whisper-1".to_string();
    s.speech_language = None;
    s.auto_paste = true;
    s
}

async fn server_with(response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

fn ok_text(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "text": text }))
}

fn base(server: &MockServer) -> String {
    format!("{}/v1", server.uri())
}

/// One scenario's dictation as T-006 drives it, observed by a `LogObserver` over
/// `log`: press 4 s ago, release 3 s later, `RecordingStarted` / `RecordingEnded`
/// on the observer, `run_job` on a plain std thread, `job_finished`. Returns the
/// recording, the report and what reached the clipboard.
fn dictate(
    log: &Arc<Log>,
    settings: Settings,
    timeouts: Timeouts,
    clipboard_fails: bool,
) -> (RecordingId, JobReport, Vec<String>) {
    let clipboard = Arc::new(FakeClipboard::new());
    clipboard.set_fail(clipboard_fails);
    let observer = Arc::new(LogObserver::new(Arc::clone(log)));
    let deps = PipelineDeps {
        gate: energy_gate(),
        credentials: Arc::new(FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, KEY)),
        clipboard: clipboard.clone(),
        paster: Arc::new(FakePaster::new()),
        temp_audio: Arc::new(FakeTempAudioStore::new()),
        observer: observer.clone(),
        post_processor: Arc::new(PassThrough),
    };
    let pipeline = Pipeline::with_timeouts(deps, timeouts);
    let mut ctrl = RecordingController::<PressContext>::new();
    let t0 = Instant::now()
        .checked_sub(Duration::from_secs(4))
        .expect("monotonic clock at least 4 s past its origin");
    let ctx = PressContext {
        start_window: Some(StartWindow {
            handle: WindowRef(0x0001_0042),
            process_id: 4242,
            elevated: false,
        }),
        settings: Arc::new(settings),
    };
    let id = match ctrl.press(t0, ctx) {
        Press::Start(id) => id,
        Press::Ignored => panic!("press ignored"),
    };
    observer.event(&DictationEvent::RecordingStarted {
        recording: id,
        hotkey_to_first_frame_ms: PRESS_TO_FRAME_MS,
        device: DeviceKind::Selected,
    });
    let stop = t0 + Duration::from_millis(HOLD_MS);
    let ticket = match ctrl.release(stop) {
        Release::Stop(ticket) => ticket,
        other => panic!("expected a stop ticket, got {other:?}"),
    };
    observer.event(&DictationEvent::RecordingEnded {
        recording: id,
        duration_ms: HOLD_MS,
        end: RecordingEnd::Released,
    });
    let rec = ctrl
        .finish(ticket, Ok(fixtures::speech_3s()), stop)
        .expect("finish(Ok) gives the recording");
    let report = std::thread::scope(|s| {
        s.spawn(|| pipeline.run_job(rec))
            .join()
            .expect("run_job must not panic")
    });
    ctrl.job_finished(id, report.end.clone(), Instant::now());
    (id, report, clipboard.texts())
}

/// A settings service over an in-memory file that holds `stored`.
fn settings_service(stored: &Settings) -> (SettingsService, LoadOutcome) {
    let bytes = serde_json::to_vec(stored).expect("serialize");
    let deps = SettingsDeps {
        file: Arc::new(FakeSettingsFile::with_bytes(&bytes)),
        credentials: Arc::new(FakeCredentialStore::new()),
        autostart: Arc::new(FakeAutostart::new()),
        hotkeys: Arc::new(FakeHotkeyRegistrar::new()),
        local_models: Arc::new(FakeDownloadedModels::new(&["base"])),
        clock: Arc::new(FakeClock::at(
            UNIX_EPOCH + Duration::from_secs(1_791_115_200),
        )),
    };
    SettingsService::load_or_init(deps, OS)
}

// ---- the run ----------------------------------------------------------------------------

#[tokio::test]
async fn log_holds_no_key_query_or_transcript_on_any_path() {
    // Analysis approach 3. Bite: an engine kind, detector, failure code or any
    // other &str echoed into the line, a FailureReason (CannotReach's host,
    // Display text) formatted into the log, a settings value (base URL, model)
    // on the save or load line, the transcript kept anywhere the log reads.
    let ok = server_with(ok_text(TRANSCRIPT)).await;
    let marker_body = format!("{TRANSCRIPT} {KEY}");
    let s401 = server_with(ResponseTemplate::new(401).set_body_string(marker_body.clone())).await;
    let s500 = server_with(ResponseTemplate::new(500).set_body_string(marker_body.clone())).await;
    let bad = server_with(
        ResponseTemplate::new(200).set_body_json(json!({ "txt": TRANSCRIPT, "key": KEY })),
    )
    .await;
    let slow = server_with(ok_text(TRANSCRIPT).set_delay(Duration::from_secs(5))).await;
    let with_query = |base: &str| format!("{base}?api-version={QUERY_SECRET}");
    let refused = format!("http://{}/v1", refused_addr());
    let ms = Timeouts {
        connect: Duration::from_secs(2),
        api_transcription: Duration::from_millis(300),
        ..Timeouts::default()
    };
    // As in api_pipeline (T-048): the refused scenario never runs under `ms`.
    let refused_t = refused_timeouts();

    let tmp = TempDir::new();
    let dir = tmp.path().join("logs");
    let (log, _clock, seen) = open_log(&dir, SystemTime::now(), 7_200, LogConfig::default());
    log.write(LogEvent::Started {
        build: voicen_core::build_info(),
        pid: 4242,
    });

    // (label, base URL, auto_paste, clipboard fails, expected end, timeouts,
    //  expected outcome, expected result-or-failure key and value)
    type Scenario = (
        &'static str,
        String,
        bool,
        bool,
        JobEnd,
        Timeouts,
        &'static str,
        (&'static str, &'static str),
    );
    let scenarios: Vec<Scenario> = vec![
        (
            "pasted",
            with_query(&base(&ok)),
            true,
            false,
            JobEnd::Delivered { notice: None },
            ms,
            "delivered",
            ("result", "pasted"),
        ),
        (
            "copied only",
            with_query(&base(&ok)),
            false,
            false,
            JobEnd::Delivered {
                notice: Some(i18n::NOTICE_COPIED),
            },
            ms,
            "delivered",
            ("result", "copied_only"),
        ),
        (
            "clipboard fails",
            with_query(&base(&ok)),
            true,
            true,
            JobEnd::Failed(FailureReason::ClipboardUnavailable),
            ms,
            "failed",
            ("failure", "clipboard_unavailable"),
        ),
        (
            "401",
            with_query(&base(&s401)),
            true,
            false,
            JobEnd::Failed(FailureReason::InvalidApiKey),
            ms,
            "failed",
            ("failure", "invalid_api_key"),
        ),
        (
            "500",
            with_query(&base(&s500)),
            true,
            false,
            JobEnd::Failed(FailureReason::ServerError { status: 500 }),
            ms,
            "failed",
            ("failure", "server_error"),
        ),
        (
            "bad body",
            with_query(&base(&bad)),
            true,
            false,
            JobEnd::Failed(FailureReason::UnexpectedResponse),
            ms,
            "failed",
            ("failure", "unexpected_response"),
        ),
        (
            "timeout",
            with_query(&base(&slow)),
            true,
            false,
            JobEnd::Failed(FailureReason::Timeout),
            ms,
            "failed",
            ("failure", "timeout"),
        ),
        (
            "refused",
            with_query(&refused),
            true,
            false,
            JobEnd::Failed(FailureReason::CannotReach {
                host: refused
                    .trim_start_matches("http://")
                    .trim_end_matches("/v1")
                    .to_string(),
            }),
            refused_t,
            "failed",
            ("failure", "cannot_reach"),
        ),
    ];

    let mut wrong = Vec::new();
    let mut clipboard_texts = Vec::new();
    let mut expected_lines = Vec::new();
    for (label, base_url, auto_paste, clipboard_fails, want, t, outcome, extra) in scenarios {
        let mut s = api_settings(&base_url);
        s.auto_paste = auto_paste;
        let (id, report, texts) = dictate(&log, s, t, clipboard_fails);
        if report.end != want {
            wrong.push(format!("{label}: {:?}, expected {want:?}", report.end));
        }
        clipboard_texts.extend(texts);
        expected_lines.push((label, id, outcome, extra));
    }
    assert!(
        wrong.is_empty(),
        "paths not exercised:\n{}",
        wrong.join("\n")
    );

    // The settings lines of a configuration that holds every secret.
    let mut stored = api_settings(&with_query("https://api.example.com/v1"));
    stored.api.model = format!("{TRANSCRIPT}-model");
    let (service, load) = settings_service(&stored);
    assert!(matches!(load, LoadOutcome::Loaded(_)), "{load:?}");
    log.write(LogEvent::settings_load(&load));
    let mut draft = api_settings(&with_query("http://192.0.2.10:8000/v1"));
    draft.post_processing.prompt = format!("{TRANSCRIPT} prompt");
    let saved = service.save(SaveRequest {
        settings: draft,
        keys: KeyEdits {
            transcription_api: KeyEdit::Replace(Secret::new(KEY)),
            local_server: KeyEdit::Replace(Secret::new(KEY)),
            post_processing: KeyEdit::Replace(Secret::new(KEY)),
        },
    });
    assert!(matches!(saved, SaveOutcome::Saved { .. }), "{saved:?}");
    log.write(LogEvent::settings_save(&saved));

    // Positive controls: the inputs did flow.
    assert!(
        clipboard_texts.iter().any(|t| t == TRANSCRIPT),
        "the transcript reached the clipboard: {clipboard_texts:?}"
    );
    let requests = ok.received_requests().await.expect("request recording");
    let r = requests.first().expect("the ok server was called");
    assert_eq!(r.url.query(), Some("api-version=SECRETQ"));
    assert_eq!(
        r.headers.get("authorization").and_then(|v| v.to_str().ok()),
        Some("Bearer sk-test-SECRET")
    );

    // Every line is closed; each scenario has its own dictation line.
    let lines = all_lines(&dir);
    let heads: Vec<String> = lines.iter().map(|l| closed(l).head).collect();
    assert_eq!(
        heads.first().map(String::as_str),
        Some("started"),
        "{lines:#?}"
    );
    assert!(heads.iter().any(|h| h == "settings load"), "{heads:?}");
    assert!(heads.iter().any(|h| h == "settings save"), "{heads:?}");
    let dictations: Vec<Line> = dictation_lines(&lines);
    assert_eq!(dictations.len(), expected_lines.len(), "{lines:#?}");
    for (label, id, outcome, (key, value)) in expected_lines {
        let rec = id.get().to_string();
        let found = dictations.iter().any(|l| {
            l.get("rec") == Some(rec.as_str())
                && l.get("engine") == Some("api")
                && l.get("outcome") == Some(outcome)
                && l.get(key) == Some(value)
                && l.get("press_to_frame_ms") == Some("37")
                && l.get("duration_ms") == Some("3000")
        });
        assert!(
            found,
            "{label}: no line rec={rec} outcome={outcome} {key}={value}:\n{lines:#?}"
        );
    }

    // ...and none of the secrets, in any file under the logs dir.
    let files = files_under(&dir);
    assert!(!files.is_empty(), "the log was written");
    for file in files {
        let bytes = std::fs::read(&file).expect("read");
        for secret in [KEY, QUERY_SECRET, TRANSCRIPT] {
            assert!(
                !contains(&bytes, secret.as_bytes()) && !contains(&bytes, &utf16le(secret)),
                "{secret} in {}",
                file.display()
            );
        }
    }
    assert_eq!(seen.count(), 0);
}
