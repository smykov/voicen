//! T-018: the local OpenAI-compatible server engine (spec 002 US3; FR-17, FR-24,
//! v4 FR-13) against a mock server, through the one factory `engine_for` and the
//! production pipeline.
//!
//! Every engine here comes from `engine_for` with `EngineKind::LocalServer`: that
//! is the only path the shell and the pipeline take (decision #44), so a test that
//! built the client directly could pass while the user's path stays broken.
//! wiremock serves on its own thread and runtime; `transcribe` and `run_job` run
//! on a plain std thread, because the blocking reqwest client panics inside a
//! tokio runtime context (decision #42). Refused cases use `refused_addr()` and
//! `refused_timeouts()` (F-004, F-005; docs/decisions/core-tests.md); timeout
//! cases use a server that accepts and answers late. The "connection not accepted
//! in time" row (connect + timeout -> CannotReach) has no deterministic local
//! fixture and is pinned by `failure::tests` (T-018 Investigation).
//! Fake data only: 127.0.0.1, `sk-test-…`.

mod common;
/// The checks of `common::refused_addr()`, in each binary that calls it (T-047).
#[path = "common/refused_addr_tests.rs"]
mod refused_addr_tests;

use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use common::{refused_addr, refused_timeouts, OVERHEAD_ALLOWANCE};
use serde_json::json;
use voicen_core::audio::{wav, AudioBuffer};
use voicen_core::autostart::FakeAutostart;
use voicen_core::clock::FakeClock;
use voicen_core::engine::{engine_for, Engine, TranscribeRequest};
use voicen_core::events::{DictationEvent, OutcomeCode, RecordingObserver};
use voicen_core::failure::FailureReason;
use voicen_core::hotkey_registrar::FakeHotkeyRegistrar;
use voicen_core::models::FakeDownloadedModels;
use voicen_core::pipeline::{JobReport, Pipeline, PipelineDeps, PressContext};
use voicen_core::platform::{
    FakeClipboard, FakePaster, FakeTempAudioStore, StartWindow, TempAudioStore, WindowRef,
};
use voicen_core::post_process::PassThrough;
use voicen_core::recording::{FinishedRecording, JobEnd, Press, RecordingController, Release};
use voicen_core::secrets::{
    CredentialCall, CredentialOp, FakeCredentialStore, KeyEdit, KeyEdits, KeySlot, Secret,
};
use voicen_core::settings::file::FakeSettingsFile;
use voicen_core::settings::service::{SaveOutcome, SaveRequest, SettingsDeps, SettingsService};
use voicen_core::settings::{defaults, EngineKind, Settings};
use voicen_core::test_support::fixtures;
use voicen_core::timeouts::Timeouts;
use voicen_core::vad::{EnergyDetector, SpeechDetector, SpeechGate};
use wiremock::matchers::{any, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The key stored in the `local-server` slot.
const LOCAL_KEY: &str = "sk-test-LOCAL";
/// A key stored in the transcription-API slot: it must never reach the local server.
const API_KEY: &str = "sk-test-API-OTHER";
const OS: Option<&str> = Some("en-US");

// ---- helpers ------------------------------------------------------------------

/// 100 ms of a 16 kHz sawtooth.
fn audio() -> AudioBuffer {
    AudioBuffer::from_16k_mono((0..1600).map(|i| ((i % 64) as i16 - 32) * 512).collect())
}

/// Test durations: set explicitly, never the production defaults. The API and
/// local-server deadlines differ, so a client that reads the wrong field shows.
fn timeouts(api_transcription: Duration, local_server: Duration) -> Timeouts {
    Timeouts {
        connect: Duration::from_secs(2),
        api_transcription,
        local_server,
        ..Timeouts::default()
    }
}

fn req_with(t: Timeouts) -> TranscribeRequest {
    TranscribeRequest {
        language: None,
        timeouts: t,
    }
}

fn req() -> TranscribeRequest {
    req_with(timeouts(Duration::from_secs(5), Duration::from_secs(5)))
}

/// Settings with engine = local server at `base` and `model`; the API section
/// points elsewhere, so an engine built from it would never reach the mock.
fn local_settings(base: &str, model: &str) -> Settings {
    let mut s = defaults(OS);
    s.engine = EngineKind::LocalServer;
    s.local_server.base_url = base.to_string();
    s.local_server.model = model.to_string();
    s.api.base_url = "https://api.example.com/v1".to_string();
    s.api.model = "whisper-1".to_string();
    s.speech_language = None;
    s.auto_paste = true;
    s
}

/// The API slot always holds a key; the local-server slot only when `local` is set.
fn creds(local: Option<&str>) -> FakeCredentialStore {
    let c = FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, API_KEY);
    match local {
        Some(key) => c.with_key(KeySlot::LocalServer, key),
        None => c,
    }
}

fn read_local() -> CredentialCall {
    CredentialCall {
        op: CredentialOp::Read,
        slot: KeySlot::LocalServer,
    }
}

/// `engine_for` for these settings; panics with the reason when no engine is built.
fn local_engine(settings: &Settings, creds: &FakeCredentialStore) -> Box<dyn Engine> {
    match engine_for(settings, creds) {
        Ok(engine) => engine,
        Err(e) => panic!("local-server engine expected from engine_for, got {e:?}"),
    }
}

/// `transcribe` on a plain std thread (no runtime context there).
fn transcribe(engine: Box<dyn Engine>, req: TranscribeRequest) -> Result<String, FailureReason> {
    std::thread::spawn(move || engine.transcribe(&audio(), &req))
        .join()
        .expect("transcribe must not panic")
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

async fn received(server: &MockServer) -> Vec<wiremock::Request> {
    server
        .received_requests()
        .await
        .expect("request recording is on")
}

async fn only_request(server: &MockServer) -> wiremock::Request {
    let mut requests = received(server).await;
    assert_eq!(requests.len(), 1, "exactly one request (no retries)");
    requests.remove(0)
}

fn header<'a>(r: &'a wiremock::Request, name: &str) -> Option<&'a str> {
    r.headers.get(name).and_then(|v| v.to_str().ok())
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// The `(name, body)` parts of a `multipart/form-data` request body (RFC 7578).
fn multipart(r: &wiremock::Request) -> Vec<(String, Vec<u8>)> {
    let content_type = header(r, "content-type").expect("Content-Type header");
    assert!(
        content_type.starts_with("multipart/form-data"),
        "Content-Type {content_type}"
    );
    let boundary = content_type
        .split(';')
        .map(str::trim)
        .find_map(|p| p.strip_prefix("boundary="))
        .expect("multipart boundary")
        .trim_matches('"');
    let delim = format!("--{boundary}").into_bytes();
    let next_delim = [b"\r\n".as_slice(), delim.as_slice()].concat();
    let body = &r.body;
    let mut parts = Vec::new();
    let mut pos = find(body, &delim, 0).expect("first boundary");
    loop {
        let after = pos + delim.len();
        if body.get(after..after + 2) == Some(b"--".as_slice()) {
            break;
        }
        let head_start = after + 2;
        let head_end = find(body, b"\r\n\r\n", head_start).expect("end of part headers");
        let head = String::from_utf8_lossy(&body[head_start..head_end]).into_owned();
        let content_end = find(body, &next_delim, head_end + 4).expect("next boundary");
        let name = head
            .split("\r\n")
            .filter_map(|line| line.split_once(':'))
            .filter(|(k, _)| k.trim().eq_ignore_ascii_case("content-disposition"))
            .flat_map(|(_, v)| v.split(';').map(str::trim).collect::<Vec<_>>())
            .find_map(|p| {
                p.strip_prefix("name=")
                    .map(|n| n.trim_matches('"').to_string())
            })
            .unwrap_or_default();
        parts.push((name, body[head_end + 4..content_end].to_vec()));
        pos = content_end + 2;
    }
    parts
}

fn part_names(parts: &[(String, Vec<u8>)]) -> Vec<String> {
    let mut names: Vec<String> = parts.iter().map(|(n, _)| n.clone()).collect();
    names.sort_unstable();
    names
}

fn part_text(parts: &[(String, Vec<u8>)], name: &str) -> Option<String> {
    parts
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
}

// ---- Acceptance 1: request shape ----------------------------------------------

#[tokio::test]
async fn no_key_request_goes_to_url_audio_transcriptions_without_authorization() {
    // Acceptance 1 / FR-13: engine = local server, no key in the local-server slot
    // (a key sits in the API slot), default empty model. One POST to
    // `<URL>/audio/transcriptions`, no Authorization header, the WAV as `file`,
    // `response_format=json` and no `model` part. Bite: the LocalServer arm still
    // EngineNotConfigured; the API slot's key read (Bearer sk-test-API-OTHER);
    // `model=""` sent; the API base URL used.
    let server = server_with(ok_text("hello local")).await;
    let creds = creds(None);
    let engine = local_engine(&local_settings(&base(&server), ""), &creds);
    assert_eq!(transcribe(engine, req()), Ok("hello local".to_string()));

    let r = only_request(&server).await;
    assert_eq!(r.method.as_str(), "POST");
    assert_eq!(r.url.path(), "/v1/audio/transcriptions");
    assert_eq!(header(&r, "authorization"), None);
    let parts = multipart(&r);
    assert_eq!(part_names(&parts), vec!["file", "response_format"]);
    assert!(
        parts
            .iter()
            .any(|(n, b)| n == "file" && *b == wav::encode(&audio())),
        "file part is not wav::encode(&buffer)"
    );
    assert_eq!(
        part_text(&parts, "response_format").as_deref(),
        Some("json")
    );
    assert_eq!(
        creds.calls(),
        vec![read_local()],
        "only the local-server slot, once"
    );
}

#[tokio::test]
async fn whitespace_only_model_omits_model_part() {
    // A hand-edited settings file bypasses the save-time trim: a whitespace-only
    // model counts as unset (T-018 analysis). Bite: the model sent untrimmed
    // ("   "), or only the exact empty string treated as unset.
    let server = server_with(ok_text("hi")).await;
    let engine = local_engine(&local_settings(&base(&server), " \t "), &creds(None));
    assert_eq!(transcribe(engine, req()), Ok("hi".to_string()));
    let parts = multipart(&only_request(&server).await);
    assert_eq!(
        part_text(&parts, "model"),
        None,
        "model part sent for an unset model"
    );
}

#[tokio::test]
async fn set_model_is_sent_trimmed() {
    // A set model is sent as the `model` part, trimmed. Bite: the model dropped
    // for the local server altogether, the API section's model ("whisper-1")
    // sent, or the stored text sent with its spaces.
    let server = server_with(ok_text("hi")).await;
    let engine = local_engine(
        &local_settings(&base(&server), "  Systran/faster-whisper-small "),
        &creds(None),
    );
    assert_eq!(transcribe(engine, req()), Ok("hi".to_string()));
    let parts = multipart(&only_request(&server).await);
    assert_eq!(
        part_text(&parts, "model").as_deref(),
        Some("Systran/faster-whisper-small")
    );
    assert_eq!(part_names(&parts), vec!["file", "model", "response_format"]);
}

#[tokio::test]
async fn stored_local_server_key_is_sent_as_bearer() {
    // FR-13: a key stored in the `local-server` slot is the Bearer; the API
    // slot's key never is. Bite: the API slot read, the key read but dropped.
    let server = server_with(ok_text("hi")).await;
    let creds = creds(Some(LOCAL_KEY));
    let engine = local_engine(&local_settings(&base(&server), ""), &creds);
    assert_eq!(transcribe(engine, req()), Ok("hi".to_string()));
    let r = only_request(&server).await;
    assert_eq!(header(&r, "authorization"), Some("Bearer sk-test-LOCAL"));
    assert_eq!(creds.calls(), vec![read_local()]);
}

#[tokio::test]
async fn empty_stored_key_sends_no_authorization() {
    // An empty stored key is "no key" (the existing authorization() rule), not
    // `Bearer ` with nothing after it. Bite: a second bearer_auth path for the
    // local server that skips the empty-key rule.
    let server = server_with(ok_text("hi")).await;
    let engine = local_engine(&local_settings(&base(&server), ""), &creds(Some("")));
    assert_eq!(transcribe(engine, req()), Ok("hi".to_string()));
    assert_eq!(header(&only_request(&server).await, "authorization"), None);
}

// ---- Acceptance 2: failure branches ---------------------------------------------

#[tokio::test]
async fn request_deadline_is_local_server_not_api_transcription() {
    // FR-24: the local server gets `Timeouts::local_server` (60 s), not the API's
    // 30 s. The answer comes after 1.5 s: past the API deadline (300 ms), inside
    // the local-server one (5 s). Bite: the engine still uses api_transcription
    // (Timeout after 300 ms).
    let server =
        server_with(ok_text("late but in time").set_delay(Duration::from_millis(1500))).await;
    let engine = local_engine(&local_settings(&base(&server), ""), &creds(None));
    let got = transcribe(
        engine,
        req_with(timeouts(Duration::from_millis(300), Duration::from_secs(5))),
    );
    assert_eq!(got, Ok("late but in time".to_string()));
}

#[tokio::test]
async fn no_answer_within_local_server_deadline_is_timeout() {
    // Acceptance 2 "no answer in 60 s -> timeout", with the deadline injected: a
    // 5 s delay against a 300 ms local-server deadline is Timeout well before 5 s
    // (the API deadline is 10 s here). Bite: api_transcription or
    // Timeouts::default() used (the call succeeds after 5 s), or the timeout
    // mapped to CannotReach.
    let server = server_with(ok_text("too late").set_delay(Duration::from_secs(5))).await;
    let engine = local_engine(&local_settings(&base(&server), ""), &creds(None));
    let started = Instant::now();
    let got = transcribe(
        engine,
        req_with(timeouts(
            Duration::from_secs(10),
            Duration::from_millis(300),
        )),
    );
    let elapsed = started.elapsed();
    assert_eq!(got, Err(FailureReason::Timeout));
    assert!(elapsed < Duration::from_secs(3), "took {elapsed:?}");
}

#[test]
fn refused_server_is_cannot_reach_host_port() {
    // Acceptance 2 "server down -> cannot reach <host:port>": a refused loopback
    // port gives CannotReach with the local URL's host:port (the default URL gives
    // "localhost:8000": openai::tests). Bite: EngineNotConfigured (no arm), the
    // API URL's host, NetworkUnavailable, the whole URL as host.
    let addr = refused_addr();
    let engine = local_engine(
        &local_settings(&format!("http://{addr}/v1"), ""),
        &creds(None),
    );
    let got = transcribe(engine, req_with(refused_timeouts()));
    assert_eq!(
        got,
        Err(FailureReason::CannotReach {
            host: addr.to_string()
        })
    );
}

#[tokio::test]
async fn status_401_and_403_are_invalid_api_key() {
    // A local server that wants a key it did not get (or rejects the one sent)
    // answers 401/403: InvalidApiKey, one request. Bite: a local-server path that
    // maps statuses itself (ServerError) or retries without the key.
    for status in [401u16, 403] {
        let server = server_with(ResponseTemplate::new(status)).await;
        let engine = local_engine(&local_settings(&base(&server), ""), &creds(Some(LOCAL_KEY)));
        let got = transcribe(engine, req());
        assert_eq!(got, Err(FailureReason::InvalidApiKey), "{status}");
        only_request(&server).await;
    }
}

// ---- the production pipeline ------------------------------------------------------

struct Harness {
    clipboard: Arc<FakeClipboard>,
    store: Arc<FakeTempAudioStore>,
    observer: Arc<RecordingObserver>,
    pipeline: Pipeline,
    ctrl: RecordingController<PressContext>,
}

/// Energy gate; `timeouts: None` = `Pipeline::new` (FR-24 defaults).
fn harness(creds: Arc<FakeCredentialStore>, timeouts: Option<Timeouts>) -> Harness {
    let clipboard = Arc::new(FakeClipboard::new());
    let store = Arc::new(FakeTempAudioStore::new());
    let observer = Arc::new(RecordingObserver::new());
    let deps = PipelineDeps {
        gate: SpeechGate::new(
            Ok(Box::new(EnergyDetector::new()) as Box<dyn SpeechDetector>),
            EnergyDetector::new(),
        ),
        credentials: creds,
        clipboard: clipboard.clone(),
        paster: Arc::new(FakePaster::new()),
        temp_audio: store.clone(),
        observer: observer.clone(),
        post_processor: Arc::new(PassThrough),
    };
    let pipeline = match timeouts {
        None => Pipeline::new(deps),
        Some(t) => Pipeline::with_timeouts(deps, t),
    };
    Harness {
        clipboard,
        store,
        observer,
        pipeline,
        ctrl: RecordingController::new(),
    }
}

impl Harness {
    /// Press 4 s ago, release 3 s later, finish with `audio`; snapshot at press.
    fn record(
        &mut self,
        audio: AudioBuffer,
        settings: Arc<Settings>,
    ) -> FinishedRecording<PressContext> {
        let t0 = Instant::now()
            .checked_sub(Duration::from_secs(4))
            .expect("monotonic clock at least 4 s past its origin");
        let ctx = PressContext {
            start_window: Some(StartWindow {
                handle: WindowRef(0x0001_0042),
                process_id: 4242,
                elevated: false,
            }),
            settings,
        };
        assert!(matches!(self.ctrl.press(t0, ctx), Press::Start(_)));
        let stop = t0 + Duration::from_secs(3);
        let ticket = match self.ctrl.release(stop) {
            Release::Stop(ticket) => ticket,
            other => panic!("expected a stop ticket, got {other:?}"),
        };
        self.ctrl
            .finish(ticket, Ok(audio), stop)
            .expect("finish(Ok) gives the recording")
    }

    /// `run_job` on a plain std thread (no tokio context there).
    fn run(&self, rec: FinishedRecording<PressContext>) -> JobReport {
        let pipeline = &self.pipeline;
        std::thread::scope(|s| {
            s.spawn(move || pipeline.run_job(rec))
                .join()
                .expect("run_job must not panic")
        })
    }

    fn job_engines(&self) -> Vec<Option<&'static str>> {
        self.observer
            .events()
            .iter()
            .filter_map(|e| match e {
                DictationEvent::JobFinished { engine, .. } => Some(*engine),
                _ => None,
            })
            .collect()
    }

    fn job_outcomes(&self) -> Vec<OutcomeCode> {
        self.observer
            .events()
            .iter()
            .filter_map(|e| match e {
                DictationEvent::JobFinished { outcome, .. } => Some(*outcome),
                _ => None,
            })
            .collect()
    }
}

#[test]
fn server_down_is_cannot_reach_and_audio_kept() {
    // Acceptance 2 on the dictation path: engine = local server, nothing listening.
    // The job fails CannotReach{host:port}, the recording is kept as pending (for
    // retry, FR-17), nothing is copied, and the job is tagged "local_server" in the
    // diag events. Bite: EngineNotConfigured (not retryable: nothing kept), kind()
    // still "api", the API host named.
    let addr = refused_addr();
    let mut h = harness(Arc::new(creds(None)), Some(refused_timeouts()));
    let rec = h.record(
        fixtures::speech_3s(),
        Arc::new(local_settings(&format!("http://{addr}/v1"), "")),
    );
    let report = h.run(rec);
    let reason = FailureReason::CannotReach {
        host: addr.to_string(),
    };
    assert_eq!(report.end, JobEnd::Failed(reason.clone()));
    let Some(id) = report.pending else {
        panic!("a refused local server keeps the recording: {report:?}")
    };
    assert_eq!(h.pipeline.pending(), Some((id, reason)));
    assert_eq!(
        h.store.get_pending(id).expect("pending audio stored"),
        fixtures::speech_3s()
    );
    assert_eq!(h.clipboard.texts(), Vec::<String>::new());
    assert_eq!(h.job_engines(), vec![Some("local_server")]);
    assert_eq!(h.job_outcomes(), vec![OutcomeCode::Failed]);
}

#[tokio::test]
async fn configured_local_server_timeout_fails_the_job_with_timeout() {
    // T-073 (the owner's case: long dictations on a slow local server), through
    // the production pipeline (`Pipeline::new`, no test override): a snapshot with
    // local_server_s = 5 (the #99 minimum) fails with Timeout against a server
    // answering after 10 s (T-078: was 7 s; now above the took bound), about 5 s
    // after the request started, failure=Timeout in the job event, the audio kept; the API limit in the same snapshot is set long
    // (600 s) so reading it instead shows. The same server with the default
    // snapshot (60 s) is delivered. Bite: the pipeline's fixed Timeouts::default(),
    // the API value used for the local server, or milliseconds.
    let delay = Duration::from_secs(10);
    let server = server_with(ok_text("late local text").set_delay(delay)).await;

    let mut h = harness(Arc::new(creds(None)), None);
    let mut s = local_settings(&base(&server), "");
    s.timeouts.local_server_s = 5;
    s.timeouts.api_transcription_s = 600;
    let rec = h.record(fixtures::speech_3s(), Arc::new(s));
    let started = Instant::now();
    let report = h.run(rec);
    let took = started.elapsed();
    assert_eq!(report.end, JobEnd::Failed(FailureReason::Timeout));
    // Bounds (T-078): lower = the 5 s deadline minus 100 ms of timer slack (a
    // deadline never fires early; catches milliseconds). Upper = 5 s +
    // OVERHEAD_ALLOWANCE (3 s: client build, VAD, WAV, drop under host load),
    // 8 s, below the nearest wrong-deadline bite: a retry after the timeout
    // (2 x 5 s = 10 s, Timeout) and the server's 10 s answer; a limit above that
    // (the 600 s API value, the 60 s default) is caught by the reason (Delivered).
    let want = Duration::from_secs(5);
    let upper = want + OVERHEAD_ALLOWANCE;
    assert!(upper < delay, "bound {upper:?} not below the bite");
    assert!(
        took >= want - Duration::from_millis(100) && took < upper,
        "took {took:?}: the 5 s limit of the snapshot (bound {upper:?})"
    );
    assert!(report.pending.is_some(), "{report:?}");
    assert_eq!(h.clipboard.texts(), Vec::<String>::new());
    assert_eq!(h.job_engines(), vec![Some("local_server")]);
    let failures: Vec<Option<&'static str>> = h
        .observer
        .events()
        .iter()
        .filter_map(|e| match e {
            DictationEvent::JobFinished { failure, .. } => Some(*failure),
            _ => None,
        })
        .collect();
    assert_eq!(failures, vec![Some("Timeout")]);

    // The same server, the default snapshot: within the default 60 s.
    let mut h = harness(Arc::new(creds(None)), None);
    let rec = h.record(
        fixtures::speech_3s(),
        Arc::new(local_settings(&base(&server), "")),
    );
    let report = h.run(rec);
    assert!(
        matches!(report.end, JobEnd::Delivered { .. }),
        "default limit: {report:?}"
    );
    assert_eq!(h.clipboard.texts(), vec!["late local text".to_string()]);
}

fn settings_deps(creds: Arc<FakeCredentialStore>) -> SettingsDeps {
    let bytes = serde_json::to_vec(&defaults(OS)).expect("serialize defaults");
    SettingsDeps {
        file: Arc::new(FakeSettingsFile::with_bytes(&bytes)),
        credentials: creds,
        autostart: Arc::new(FakeAutostart::new()),
        hotkeys: Arc::new(FakeHotkeyRegistrar::new()),
        local_models: Arc::new(FakeDownloadedModels::new(&["base"])),
        clock: Arc::new(FakeClock::at(
            UNIX_EPOCH + Duration::from_secs(1_709_251_199),
        )),
    }
}

#[tokio::test]
async fn saved_local_server_settings_reach_the_next_job_without_restart() {
    // Spec 002 T029: URL, model and key saved through the real SettingsService
    // (same credential store) are used by the next job, with the production
    // pipeline: the request goes to the saved URL with the saved model and the
    // saved key as Bearer, the text is delivered, the job is tagged
    // "local_server". Bite: the arm reading the API section or slot, kind()
    // "api", the model dropped.
    let server = server_with(ok_text("hello from the local server")).await;
    let creds = Arc::new(FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, API_KEY));
    let (service, _) = SettingsService::load_or_init(settings_deps(creds.clone()), OS);
    let outcome = service.save(SaveRequest {
        settings: local_settings(&base(&server), "whisper-local"),
        keys: KeyEdits {
            transcription_api: KeyEdit::Untouched,
            local_server: KeyEdit::Replace(Secret::new(LOCAL_KEY)),
            post_processing: KeyEdit::Untouched,
        },
    });
    assert!(
        matches!(outcome, SaveOutcome::Saved { .. }),
        "save refused: {outcome:?}"
    );

    let mut h = harness(creds, None);
    let rec = h.record(fixtures::speech_3s(), service.snapshot());
    let report = h.run(rec);
    assert!(
        matches!(report.end, JobEnd::Delivered { .. }),
        "local-server job not delivered: {report:?}"
    );
    assert_eq!(
        h.clipboard.texts(),
        vec!["hello from the local server".to_string()]
    );
    assert_eq!(h.job_engines(), vec![Some("local_server")]);

    let requests = received(&server).await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), "/v1/audio/transcriptions");
    assert_eq!(
        header(&requests[0], "authorization"),
        Some("Bearer sk-test-LOCAL")
    );
    assert_eq!(
        part_text(&multipart(&requests[0]), "model").as_deref(),
        Some("whisper-local")
    );
}

// ---- T-072: a full endpoint URL (decision #96) -------------------------------------

/// The endpoint the mock serves: POST at `/v1/audio/transcriptions` only; any
/// other path gets wiremock's unmatched answer, 404 (a server with that one route).
async fn endpoint_only_server(response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn full_endpoint_local_server_url_is_requested_as_is() {
    // T-072, the owner's case (rec=4, http_status=404): the local-server URL set to
    // the full endpoint `…/v1/audio/transcriptions` (as copied from another client),
    // through `engine_for`. One POST at that path, nothing appended, the text
    // delivered. Bite: the unconditional append (`…/audio/transcriptions/audio/
    // transcriptions` -> the mock's 404 -> ServerError{404}); a fix only in the
    // API arm or only in the UI (engine_for's LocalServer arm still doubles).
    let server = endpoint_only_server(ok_text("hello full endpoint")).await;
    for suffix in ["/v1/audio/transcriptions", "/v1/audio/transcriptions/"] {
        let url = format!("{}{suffix}", server.uri());
        let engine = local_engine(&local_settings(&url, ""), &creds(None));
        assert_eq!(
            transcribe(engine, req()),
            Ok("hello full endpoint".to_string()),
            "{suffix:?}"
        );
    }
    let requests = received(&server).await;
    let paths: Vec<&str> = requests.iter().map(|r| r.url.path()).collect();
    assert_eq!(
        paths,
        vec!["/v1/audio/transcriptions", "/v1/audio/transcriptions"],
        "one request per URL, each at the endpoint as typed"
    );
}

#[tokio::test]
async fn wrong_path_reports_server_404() {
    // T-072 Acceptance, failure branch: a URL with a doubled path or any other
    // wrong path still reaches the server at that path and reports its 404 as
    // today, on the production pipeline: JobEnd::Failed(ServerError{404}), the
    // recording kept as pending (retryable), nothing copied, the job tagged
    // "local_server". The request goes to exactly the path the rule gives (the
    // doubled URL as typed; `/v2` + `/audio/transcriptions`). Bite: a "repair"
    // that strips repeated `/audio/transcriptions` pairs (the doubled row then
    // succeeds at the mock's endpoint); a rewrite to a fixed `/v1` path (the `/v2`
    // row succeeds); a 404 swallowed or mapped to another reason; the audio
    // dropped; today's unconditional append (the doubled row's path quadruples).
    for (suffix, requested) in [
        (
            "/v1/audio/transcriptions/audio/transcriptions",
            "/v1/audio/transcriptions/audio/transcriptions",
        ),
        ("/v2", "/v2/audio/transcriptions"),
    ] {
        let server = endpoint_only_server(ok_text("must not be delivered")).await;
        let mut h = harness(Arc::new(creds(None)), None);
        let url = format!("{}{suffix}", server.uri());
        let rec = h.record(fixtures::speech_3s(), Arc::new(local_settings(&url, "")));
        let report = h.run(rec);
        let reason = FailureReason::ServerError { status: 404 };
        assert_eq!(report.end, JobEnd::Failed(reason.clone()), "{suffix:?}");
        let Some(id) = report.pending else {
            panic!("{suffix:?}: a server 404 keeps the recording: {report:?}")
        };
        assert_eq!(h.pipeline.pending(), Some((id, reason)), "{suffix:?}");
        assert_eq!(
            h.store.get_pending(id).expect("pending audio stored"),
            fixtures::speech_3s(),
            "{suffix:?}"
        );
        assert_eq!(h.clipboard.texts(), Vec::<String>::new(), "{suffix:?}");
        assert_eq!(h.job_engines(), vec![Some("local_server")], "{suffix:?}");
        assert_eq!(h.job_outcomes(), vec![OutcomeCode::Failed], "{suffix:?}");
        let r = only_request(&server).await;
        assert_eq!(r.method.as_str(), "POST", "{suffix:?}");
        assert_eq!(r.url.path(), requested, "{suffix:?}");
    }
}
