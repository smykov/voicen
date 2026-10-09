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
//! own runtime); `RecordingStarted` / `RecordingEnded` are emitted on the same
//! observer the way the dictation session does (T-051: it emits them, and
//! `CaptureFailed`, on `PipelineDeps.observer`). Fake data only: 127.0.0.1,
//! `sk-test-SECRET`. The refused scenario uses `common::os_answer::OsAnswer::refused()`
//! and its deadlines (F-004, F-005, T-080 I1).

mod common;
mod diag_support;
/// The checks of `common::os_answer::OsAnswer::refused()`, in each binary that takes it
/// (T-047).
#[path = "common/refused_addr_tests.rs"]
mod refused_addr_tests;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::os_answer::OsAnswer;
use common::timing::{ago, at_least, now};
use diag_support::{
    all_lines, closed, contains, dictation_lines, files_under, open_log, utf16le, Line,
};
use serde_json::json;
use voicen_core::autostart::FakeAutostart;
use voicen_core::clock::FakeClock;
use voicen_core::delivery::DeliveryResult;
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
use voicen_core::post_process::chat::ChatPostProcessor;
use voicen_core::post_process::settings::PostProcessingSettings;
use voicen_core::post_process::{PassThrough, PostProcessor, SkipReason};
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
use wiremock::matchers::{any, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-test-SECRET";
// T-076 canaries of the post-processing run: the key, the prompt, the chat reply,
// the model, the base URL's query and a host that only a refused base URL holds.
// Lowercase ones pass the log's value grammar, so only the allowlist keeps them out.
const PP_KEY: &str = "sk-test-PP-SECRET";
const PP_PROMPT: &str = "PROMPT-SECRET-fix the text";
const PP_REPLY: &str = "REPLY-SECRET-the fixed text";
const PP_MODEL: &str = "pp_model_secret";
const PP_QUERY: &str = "ppqsecret";
const PP_HOST: &str = "pp-host-secret.example.com";
/// What the recording's stop lies before `run_job` starts: the press is 4 s ago,
/// the hold 3 s (see `dictate_with`). The stage starts after it, so every traced
/// job's stop_to_text_ms is at least this plus its pp_ms, under any load.
const STOP_BEFORE_JOB_MS: u64 = 1_000;
const QUERY_SECRET: &str = "SECRETQ";
const TRANSCRIPT: &str = "TRANSCRIPT-MARKER";
const OS: Option<&str> = Some("en-US");
/// Press -> first frame and the hold, as the dictation session (T-051) reports them.
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
    dictate_with(
        log,
        settings,
        timeouts,
        clipboard_fails,
        Arc::new(PassThrough),
    )
}

/// [`dictate`] with `post_processor` as the stage (T-076), and the
/// post-processing key stored next to the transcription key.
fn dictate_with(
    log: &Arc<Log>,
    settings: Settings,
    timeouts: Timeouts,
    clipboard_fails: bool,
    post_processor: Arc<dyn PostProcessor>,
) -> (RecordingId, JobReport, Vec<String>) {
    let clipboard = Arc::new(FakeClipboard::new());
    clipboard.set_fail(clipboard_fails);
    let observer = Arc::new(LogObserver::new(Arc::clone(log)));
    let deps = PipelineDeps {
        gate: energy_gate(),
        credentials: Arc::new(
            FakeCredentialStore::new()
                .with_key(KeySlot::TranscriptionApi, KEY)
                .with_key(KeySlot::PostProcessing, PP_KEY),
        ),
        clipboard: clipboard.clone(),
        paster: Arc::new(FakePaster::new()),
        temp_audio: Arc::new(FakeTempAudioStore::new()),
        observer: observer.clone(),
        post_processor,
    };
    let pipeline = Pipeline::with_timeouts(deps, timeouts);
    let mut ctrl = RecordingController::<PressContext>::new();
    let t0 = ago(Duration::from_secs(4));
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
    ctrl.job_finished(id, report.end.clone(), now());
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
    let refused_case = OsAnswer::refused();
    let refused = format!("http://{}/v1", refused_case.host);
    let ms = Timeouts {
        connect: Duration::from_secs(2),
        api_transcription: Duration::from_millis(300),
        ..Timeouts::default()
    };
    // As in api_pipeline (T-048): the refused scenario never runs under `ms`.
    let refused_t = refused_case.timeouts;

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
                host: refused_case.host.clone(),
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

// ---- T-076: the post-processing outcome on the dictation line ---------------------------

/// A chat endpoint (`.../chat/completions`) answering `response`.
async fn chat_with(response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(path_regex("/chat/completions$"))
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

fn chat_reply(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{ "message": { "role": "assistant", "content": text } }]
    }))
}

/// The post-processing base URL of `server`, with the query canary.
fn pp_base(server: &MockServer) -> String {
    format!("{}/v1?api-version={PP_QUERY}", server.uri())
}

/// api_settings for transcription at `api_base`, with post-processing `enabled`
/// at `pp_base_url`, the planted model and prompt.
fn pp_settings(api_base: &str, enabled: bool, pp_base_url: &str) -> Settings {
    let mut s = api_settings(api_base);
    s.post_processing = PostProcessingSettings {
        enabled,
        base_url: pp_base_url.to_string(),
        model: PP_MODEL.to_string(),
        prompt: PP_PROMPT.to_string(),
    };
    s
}

/// One T-076 scenario and what its dictation line must say.
struct PpScenario {
    label: &'static str,
    settings: Settings,
    timeouts: Timeouts,
    clipboard_fails: bool,
    end: JobEnd,
    /// What reaches the clipboard (`None`: the write fails).
    clipboard: Option<&'static str>,
    outcome: &'static str,
    /// `pp=`, `pp_reason=`; `pp_ms=` is required exactly for applied / skipped.
    pp: Option<&'static str>,
    pp_reason: Option<&'static str>,
    /// pp_ms is at least this (the chat's delay or the stage's deadline).
    pp_ms_at_least: Duration,
}

#[tokio::test]
async fn post_processing_outcome_is_logged_without_prompt_reply_host_or_key() {
    // T-076 Acceptance 1 and 2 (spec 003 FR-014, US3-2; FR-20, NFR-04): real
    // dictations through Pipeline + ChatPostProcessor + LogObserver into one Log.
    // Each post-processed job's line carries pp=applied|skipped|off, pp_reason=
    // for a skip (all six kinds through the real processor), pp_ms= for applied and
    // skipped; a job whose transcription failed carries no pp key; a clipboard
    // failure after the stage keeps it. pp_ms is at least the chat's delay (applied)
    // and the stage deadline (timeout), and the stage lies inside stop -> text
    // (pp_ms + 1 s <= stop_to_text_ms, an interval relation no load can break). No
    // byte of the post-processing key, prompt, reply, model, base-URL query or a
    // refused/unparsable host, nor the transcript, reaches any file under the logs
    // dir, though each was sent (positive controls). Bite: no pp keys (today), the
    // skip reason written from SkipReason::code() of a value with the host,
    // Debug-formatted, every skip as one kind, pp=off for a job that never reached
    // the stage, pp_ms = 0 or the whole job's time, the trace dropped on the
    // clipboard-failure line, the reply or prompt kept anywhere the log reads.
    let transcribe = server_with(ok_text(TRANSCRIPT)).await;
    let api = base(&transcribe);
    let failing_api = server_with(ResponseTemplate::new(500)).await;
    let echo = format!("{TRANSCRIPT} {PP_PROMPT} {PP_KEY} {PP_REPLY} {PP_MODEL} {PP_QUERY}");
    let chat_delay = Duration::from_millis(150);
    let chat_ok = chat_with(chat_reply(PP_REPLY).set_delay(chat_delay)).await;
    let chat_500 = chat_with(ResponseTemplate::new(500).set_body_string(echo.clone())).await;
    let chat_401 = chat_with(ResponseTemplate::new(401).set_body_string(echo.clone())).await;
    let chat_bad =
        chat_with(ResponseTemplate::new(200).set_body_json(json!({ "choices": [], "echo": echo })))
            .await;
    let chat_slow = chat_with(chat_reply(PP_REPLY).set_delay(Duration::from_secs(3))).await;
    let chat_off = chat_with(chat_reply(PP_REPLY)).await;
    let chat_unused = chat_with(chat_reply(PP_REPLY)).await;
    let refused_case = OsAnswer::refused();
    let refused = format!("http://{}/v1?api-version={PP_QUERY}", refused_case.host);
    let unparsable = format!("ftp://{PP_HOST}/v1?api-version={PP_QUERY}");

    let t = Timeouts {
        connect: Duration::from_secs(2),
        api_transcription: Duration::from_secs(5),
        post_processing: Duration::from_secs(5),
        ..Timeouts::default()
    };
    let pp_deadline = Duration::from_millis(300);
    let t_pp_short = Timeouts {
        post_processing: pp_deadline,
        ..t
    };
    let skipped = |reason: SkipReason| JobEnd::DeliveredSkipped {
        reason,
        delivery: DeliveryResult::Pasted,
    };
    let zero = Duration::ZERO;
    let scenarios = vec![
        PpScenario {
            label: "applied",
            settings: pp_settings(&api, true, &pp_base(&chat_ok)),
            timeouts: t,
            clipboard_fails: false,
            end: JobEnd::Delivered { notice: None },
            clipboard: Some(PP_REPLY),
            outcome: "delivered",
            pp: Some("applied"),
            pp_reason: None,
            pp_ms_at_least: chat_delay,
        },
        PpScenario {
            label: "skipped http 500",
            settings: pp_settings(&api, true, &pp_base(&chat_500)),
            timeouts: t,
            clipboard_fails: false,
            end: skipped(SkipReason::Http { status: 500 }),
            clipboard: Some(TRANSCRIPT),
            outcome: "delivered",
            pp: Some("skipped"),
            pp_reason: Some("http"),
            pp_ms_at_least: zero,
        },
        PpScenario {
            label: "skipped invalid key (401)",
            settings: pp_settings(&api, true, &pp_base(&chat_401)),
            timeouts: t,
            clipboard_fails: false,
            end: skipped(SkipReason::InvalidKey),
            clipboard: Some(TRANSCRIPT),
            outcome: "delivered",
            pp: Some("skipped"),
            pp_reason: Some("invalid_key"),
            pp_ms_at_least: zero,
        },
        PpScenario {
            label: "skipped invalid response",
            settings: pp_settings(&api, true, &pp_base(&chat_bad)),
            timeouts: t,
            clipboard_fails: false,
            end: skipped(SkipReason::InvalidResponse),
            clipboard: Some(TRANSCRIPT),
            outcome: "delivered",
            pp: Some("skipped"),
            pp_reason: Some("invalid_response"),
            pp_ms_at_least: zero,
        },
        PpScenario {
            label: "skipped timeout",
            settings: pp_settings(&api, true, &pp_base(&chat_slow)),
            timeouts: t_pp_short,
            clipboard_fails: false,
            end: skipped(SkipReason::Timeout),
            clipboard: Some(TRANSCRIPT),
            outcome: "delivered",
            pp: Some("skipped"),
            pp_reason: Some("timeout"),
            pp_ms_at_least: pp_deadline,
        },
        PpScenario {
            label: "skipped not configured",
            settings: pp_settings(&api, true, &unparsable),
            timeouts: t,
            clipboard_fails: false,
            end: skipped(SkipReason::NotConfigured),
            clipboard: Some(TRANSCRIPT),
            outcome: "delivered",
            pp: Some("skipped"),
            pp_reason: Some("not_configured"),
            pp_ms_at_least: zero,
        },
        PpScenario {
            label: "skipped unreachable (refused)",
            settings: pp_settings(&api, true, &refused),
            timeouts: refused_case.timeouts,
            clipboard_fails: false,
            end: skipped(SkipReason::Unreachable {
                host: refused_case.host.clone(),
            }),
            clipboard: Some(TRANSCRIPT),
            outcome: "delivered",
            pp: Some("skipped"),
            pp_reason: Some("unreachable"),
            pp_ms_at_least: zero,
        },
        PpScenario {
            label: "off",
            settings: pp_settings(&api, false, &pp_base(&chat_off)),
            timeouts: t,
            clipboard_fails: false,
            end: JobEnd::Delivered { notice: None },
            clipboard: Some(TRANSCRIPT),
            outcome: "delivered",
            pp: Some("off"),
            pp_reason: None,
            pp_ms_at_least: zero,
        },
        PpScenario {
            label: "clipboard fails after applied",
            settings: pp_settings(&api, true, &pp_base(&chat_ok)),
            timeouts: t,
            clipboard_fails: true,
            end: JobEnd::Failed(FailureReason::ClipboardUnavailable),
            clipboard: None,
            outcome: "failed",
            pp: Some("applied"),
            pp_reason: None,
            pp_ms_at_least: chat_delay,
        },
        PpScenario {
            label: "transcription fails, post-processing on",
            settings: pp_settings(&base(&failing_api), true, &pp_base(&chat_unused)),
            timeouts: t,
            clipboard_fails: false,
            end: JobEnd::Failed(FailureReason::ServerError { status: 500 }),
            clipboard: None,
            outcome: "failed",
            pp: None,
            pp_reason: None,
            pp_ms_at_least: zero,
        },
    ];

    let tmp = TempDir::new();
    let dir = tmp.path().join("logs");
    let (log, _clock, seen) = open_log(&dir, SystemTime::now(), 7_200, LogConfig::default());

    let mut wrong = Vec::new();
    let mut ran = Vec::new();
    for sc in &scenarios {
        let (id, report, texts) = dictate_with(
            &log,
            sc.settings.clone(),
            sc.timeouts,
            sc.clipboard_fails,
            Arc::new(ChatPostProcessor::new()),
        );
        if report.end != sc.end {
            wrong.push(format!(
                "{}: {:?}, expected {:?}",
                sc.label, report.end, sc.end
            ));
        }
        if let Some(want) = sc.clipboard {
            if texts != vec![want.to_string()] {
                wrong.push(format!(
                    "{}: clipboard {texts:?}, expected {want:?}",
                    sc.label
                ));
            }
        }
        ran.push((sc, id));
    }
    assert!(
        wrong.is_empty(),
        "paths not exercised:\n{}",
        wrong.join("\n")
    );

    // Positive controls: every planted input was really sent where it belongs.
    let requests = chat_ok
        .received_requests()
        .await
        .expect("request recording");
    let r = requests
        .first()
        .expect("the applied chat endpoint was called");
    let body = String::from_utf8_lossy(&r.body);
    for sent in [TRANSCRIPT, PP_PROMPT, PP_MODEL] {
        assert!(body.contains(sent), "{sent} in the chat request: {body}");
    }
    assert_eq!(r.url.query(), Some("api-version=ppqsecret"));
    assert_eq!(
        r.headers.get("authorization").and_then(|v| v.to_str().ok()),
        Some("Bearer sk-test-PP-SECRET")
    );
    for (server, label) in [
        (&chat_500, "500"),
        (&chat_401, "401"),
        (&chat_bad, "bad body"),
    ] {
        let n = server.received_requests().await.map_or(0, |r| r.len());
        assert_eq!(n, 1, "the {label} chat endpoint was called once");
    }
    for (server, label) in [(&chat_off, "off"), (&chat_unused, "transcription failed")] {
        let n = server.received_requests().await.map_or(0, |r| r.len());
        assert_eq!(n, 0, "{label}: no chat request");
    }

    // Each scenario has its own closed line, in run order (every dictation runs on
    // a fresh controller, so rec= repeats), with exactly its pp keys.
    let lines = all_lines(&dir);
    for l in &lines {
        closed(l);
    }
    let dictations: Vec<Line> = dictation_lines(&lines);
    assert_eq!(dictations.len(), scenarios.len(), "{lines:#?}");
    for ((sc, id), l) in ran.iter().zip(&dictations) {
        let rec = id.get().to_string();
        assert_eq!(l.get("rec"), Some(rec.as_str()), "{}: {}", sc.label, l.raw);
        let ctx = format!("{}: {}", sc.label, l.raw);
        assert_eq!(l.get("outcome"), Some(sc.outcome), "{ctx}");
        assert_eq!(l.get("engine"), Some("api"), "{ctx}");
        assert_eq!(l.get("pp"), sc.pp, "{ctx}");
        assert_eq!(l.get("pp_reason"), sc.pp_reason, "{ctx}");
        let timed = matches!(sc.pp, Some("applied" | "skipped"));
        assert_eq!(l.get("pp_ms").is_some(), timed, "pp_ms presence: {ctx}");
        if let Some(pp_ms) = l.get("pp_ms") {
            let pp_ms: u64 = pp_ms.parse().unwrap_or_else(|e| panic!("{ctx}: {e}"));
            at_least(Duration::from_millis(pp_ms), sc.pp_ms_at_least);
            let stop_to_text: u64 = l
                .get("stop_to_text_ms")
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| panic!("no stop_to_text_ms: {ctx}"));
            assert!(
                pp_ms + STOP_BEFORE_JOB_MS <= stop_to_text,
                "the stage lies inside stop -> text: pp_ms {pp_ms} + {STOP_BEFORE_JOB_MS} > \
                 stop_to_text_ms {stop_to_text}: {ctx}"
            );
        }
    }

    // ...and none of the planted bytes, in any file under the logs dir.
    let files = files_under(&dir);
    assert!(!files.is_empty(), "the log was written");
    let canaries = [
        KEY,
        PP_KEY,
        "sk-test",
        PP_PROMPT,
        "PROMPT-SECRET",
        PP_REPLY,
        "REPLY-SECRET",
        PP_MODEL,
        PP_QUERY,
        PP_HOST,
        "pp-host-secret",
        refused_case.host.as_str(),
        TRANSCRIPT,
    ];
    for file in files {
        let bytes = std::fs::read(&file).expect("read");
        for secret in canaries {
            assert!(
                !contains(&bytes, secret.as_bytes()) && !contains(&bytes, &utf16le(secret)),
                "{secret} in {}",
                file.display()
            );
        }
    }
    assert_eq!(seen.count(), 0);
}
