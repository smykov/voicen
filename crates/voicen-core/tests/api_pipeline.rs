//! T-001: the dictation job (`Pipeline::run_job`) against a mock OpenAI-compatible
//! server (T-001 Acceptance lines 1, 2 and 4; decision #47).
//!
//! Recordings are made through the real `RecordingController` (press, release
//! 3 s later, finish) and closed with `job_finished`, as T-006 will do. wiremock
//! serves on its own thread and runtime; `run_job` runs on a plain std thread,
//! because the blocking reqwest client panics inside a tokio runtime context
//! (T-040). Fake data only: 127.0.0.1, 192.0.2.1 (RFC 5737), `sk-test-SECRET`.

mod common;
/// The checks of `common::refused_addr()`, in each binary that calls it (T-047).
#[path = "common/refused_addr_tests.rs"]
mod refused_addr_tests;

use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use common::{refused_addr, refused_timeouts, OVERHEAD_ALLOWANCE, REFUSAL_BUDGET};
use serde_json::json;
use voicen_core::audio::{wav, AudioBuffer};
use voicen_core::autostart::FakeAutostart;
use voicen_core::clock::FakeClock;
use voicen_core::delivery::{DeliveryResult, MODIFIER_WAIT};
use voicen_core::events::{DictationEvent, OutcomeCode, RecordingObserver, WarningCode};
use voicen_core::failure::FailureReason;
use voicen_core::hotkey_registrar::FakeHotkeyRegistrar;
use voicen_core::i18n::{self, text, MessageId, UiLanguage};
use voicen_core::models::FakeDownloadedModels;
use voicen_core::pipeline::{JobReport, Pipeline, PipelineDeps, PressContext};
use voicen_core::platform::{
    FakeClipboard, FakePaster, FakeTempAudioStore, PasterCall, PendingId, StartWindow, StoreCall,
    TempAudioStore, WindowRef,
};
use voicen_core::post_process::PassThrough;
use voicen_core::recording::{
    FinishedRecording, JobEnd, OverlayState, Press, RecordingController, RecordingId, Release,
    TrayState,
};
use voicen_core::secrets::{
    CredentialCall, CredentialError, CredentialOp, FakeCredentialStore, KeyEdit, KeyEdits, KeySlot,
    Secret,
};
use voicen_core::settings::file::FakeSettingsFile;
use voicen_core::settings::service::{SaveOutcome, SaveRequest, SettingsDeps, SettingsService};
use voicen_core::settings::{defaults, EngineKind, Settings};
use voicen_core::test_support::fixtures;
use voicen_core::timeouts::Timeouts;
use voicen_core::vad::{EnergyDetector, SpeechDetector, SpeechGate, VadError};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-test-SECRET";
const QUERY_SECRET: &str = "SECRETQ";
const TRANSCRIPT: &str = "TRANSCRIPT-MARKER";
const MOCK_TEXT: &str = "hello from the mock";
const OS: Option<&str> = Some("en-US");
const WINDOW: WindowRef = WindowRef(0x0001_0042);

// ---- harness ------------------------------------------------------------------

fn window() -> StartWindow {
    StartWindow {
        handle: WINDOW,
        process_id: 4242,
        elevated: false,
    }
}

/// A gate whose primary is the energy detector: decisions are the energy rule's
/// and no fallback warning is raised.
fn energy_gate() -> SpeechGate {
    SpeechGate::new(
        Ok(Box::new(EnergyDetector::new()) as Box<dyn SpeechDetector>),
        EnergyDetector::new(),
    )
}

fn creds_with_key() -> Arc<FakeCredentialStore> {
    Arc::new(FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, KEY))
}

struct Harness {
    clipboard: Arc<FakeClipboard>,
    paster: Arc<FakePaster>,
    store: Arc<FakeTempAudioStore>,
    observer: Arc<RecordingObserver>,
    creds: Arc<FakeCredentialStore>,
    pipeline: Pipeline,
    ctrl: RecordingController<PressContext>,
}

/// Production pipeline (`Pipeline::new`, FR-24 timeouts), energy gate, key stored.
fn harness() -> Harness {
    harness_with(energy_gate(), None, creds_with_key())
}

/// `timeouts: None` = `Pipeline::new`; `Some` = the test constructor.
fn harness_with(
    gate: SpeechGate,
    timeouts: Option<Timeouts>,
    creds: Arc<FakeCredentialStore>,
) -> Harness {
    let clipboard = Arc::new(FakeClipboard::new());
    let paster = Arc::new(FakePaster::new());
    let store = Arc::new(FakeTempAudioStore::new());
    let observer = Arc::new(RecordingObserver::new());
    let deps = PipelineDeps {
        gate,
        credentials: creds.clone(),
        clipboard: clipboard.clone(),
        paster: paster.clone(),
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
        paster,
        store,
        observer,
        creds,
        pipeline,
        ctrl: RecordingController::new(),
    }
}

impl Harness {
    /// Press 4 s ago, release 3 s later (so `stopped_at` is about 1 s before the
    /// job starts), finish with `audio`. The snapshot is taken at press.
    fn record(
        &mut self,
        audio: AudioBuffer,
        settings: Arc<Settings>,
    ) -> FinishedRecording<PressContext> {
        let t0 = Instant::now()
            .checked_sub(Duration::from_secs(4))
            .expect("monotonic clock at least 4 s past its origin");
        let ctx = PressContext {
            start_window: Some(window()),
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

    /// `run_job` on a plain std thread (no tokio context there); also returns how
    /// long it took. A panic in `run_job` fails the test (NFR-07).
    fn run(&self, rec: FinishedRecording<PressContext>) -> (JobReport, Duration) {
        let pipeline = &self.pipeline;
        std::thread::scope(|s| {
            s.spawn(move || {
                let started = Instant::now();
                let report = pipeline.run_job(rec);
                (report, started.elapsed())
            })
            .join()
            .expect("run_job must not panic")
        })
    }

    /// The whole dictation as T-006 drives it: record, run, `job_finished`.
    fn dictate(&mut self, audio: AudioBuffer, settings: Settings) -> (RecordingId, JobReport) {
        let rec = self.record(audio, Arc::new(settings));
        let id = rec.id();
        let (report, _) = self.run(rec);
        self.ctrl
            .job_finished(id, report.end.clone(), Instant::now());
        (id, report)
    }

    fn events(&self) -> Vec<DictationEvent> {
        self.observer.events()
    }

    fn read_api_calls(&self) -> Vec<CredentialCall> {
        self.creds.calls()
    }
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

fn read_api() -> CredentialCall {
    CredentialCall {
        op: CredentialOp::Read,
        slot: KeySlot::TranscriptionApi,
    }
}

// ---- mock server helpers --------------------------------------------------------

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

fn part<'a>(parts: &'a [(String, Vec<u8>)], name: &str) -> Option<&'a [u8]> {
    parts
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, b)| b.as_slice())
}

// ---- event helpers --------------------------------------------------------------

fn job_finished(events: &[DictationEvent]) -> Vec<DictationEvent> {
    events
        .iter()
        .copied()
        .filter(|e| matches!(e, DictationEvent::JobFinished { .. }))
        .collect()
}

fn delivered(events: &[DictationEvent]) -> Vec<DictationEvent> {
    events
        .iter()
        .copied()
        .filter(|e| matches!(e, DictationEvent::Delivered { .. }))
        .collect()
}

#[track_caller]
fn assert_overlay_message(h: &Harness, want: MessageId) {
    match &h.ctrl.indicator().overlay {
        OverlayState::Message { id, .. } => assert_eq!(*id, want),
        other => panic!("expected overlay message {want:?}, got {other:?}"),
    }
}

// ---- Acceptance 1: speech -> mock text delivered ---------------------------------

#[tokio::test]
async fn speech_is_transcribed_and_delivered_as_mock_text() {
    // Acceptance 1. Bite: no request, the request without the WAV of the
    // recording / model / Bearer, a language part for auto, the text not written
    // to the clipboard, no Ctrl+V, the Ctrl+V without the wait or the front check,
    // the events missing or carrying another recording id, stop_to_text_ms
    // measured from the job start instead of the recording's stop.
    let server = server_with(ok_text(MOCK_TEXT)).await;
    let mut h = harness();
    let rec = h.record(
        fixtures::speech_3s(),
        Arc::new(api_settings(&base(&server))),
    );
    let id = rec.id();
    let (report, _) = h.run(rec);
    h.ctrl.job_finished(id, report.end.clone(), Instant::now());

    assert_eq!(
        report,
        JobReport {
            end: JobEnd::Delivered { notice: None },
            pending: None
        }
    );
    assert_eq!(h.clipboard.texts(), vec![MOCK_TEXT.to_string()]);
    assert_eq!(
        h.paster.calls(),
        vec![
            PasterCall::WaitModifiersReleased(MODIFIER_WAIT),
            PasterCall::IsInFront(WINDOW),
            PasterCall::SendCtrlV
        ]
    );
    assert_eq!(h.ctrl.indicator().tray, TrayState::Idle);
    assert_eq!(h.ctrl.indicator().overlay, OverlayState::Hidden);
    assert_eq!(h.read_api_calls(), vec![read_api()]);
    assert_eq!(h.store.calls(), vec![]);
    assert_eq!(h.pipeline.pending(), None);

    let requests = received(&server).await;
    assert_eq!(requests.len(), 1, "exactly one request");
    let r = &requests[0];
    assert_eq!(r.method.as_str(), "POST");
    assert_eq!(r.url.path(), "/v1/audio/transcriptions");
    assert_eq!(header(r, "authorization"), Some("Bearer sk-test-SECRET"));
    let parts = multipart(r);
    let file = part(&parts, "file").expect("file part");
    let expected_wav = wav::encode(&fixtures::speech_3s());
    assert!(
        file == expected_wav.as_slice(),
        "file part is not wav::encode(speech_3s)"
    );
    assert_eq!(
        file.get(22..24),
        Some(1u16.to_le_bytes().as_slice()),
        "mono"
    );
    assert_eq!(
        file.get(24..28),
        Some(16_000u32.to_le_bytes().as_slice()),
        "16 kHz"
    );
    assert_eq!(part(&parts, "model"), Some(b"whisper-1".as_slice()));
    assert_eq!(
        part(&parts, "language"),
        None,
        "auto omits the language part"
    );

    let events = h.events();
    match events.as_slice() {
        [DictationEvent::SpeechGate {
            recording,
            detector: "energy",
            speech: true,
        }, DictationEvent::JobFinished {
            seq,
            recording: finished_recording,
            engine: Some("api"),
            stop_to_text_ms,
            outcome: OutcomeCode::Text,
            failure: None,
            http_status: None,
        }, DictationEvent::Delivered {
            seq: delivered_seq,
            result: DeliveryResult::Pasted,
            ..
        }] => {
            assert_eq!(*recording, id);
            // T-008: JobFinished names its own recording, so the log can join
            // it to RecordingStarted/Ended without relying on event order.
            assert_eq!(*finished_recording, id);
            assert_eq!(delivered_seq, seq);
            assert!(
                (1_000..60_000).contains(stop_to_text_ms),
                "stop_to_text_ms {stop_to_text_ms} is not measured from the stop ~1 s ago"
            );
        }
        other => panic!("events {other:?}"),
    }
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
async fn saved_settings_reach_the_job() {
    // P-013: what the settings window saved (through the real SettingsService and
    // the same credential store) is what the job uses: speech language "de" is
    // sent, auto-paste off copies only, and the saved key is the Bearer. Bite:
    // language not taken from the snapshot, auto_paste ignored (Ctrl+V sent), a
    // second credential store, the copied notice missing.
    let server = server_with(ok_text(MOCK_TEXT)).await;
    let creds = Arc::new(FakeCredentialStore::new());
    let (service, _) = SettingsService::load_or_init(settings_deps(creds.clone()), OS);
    let mut s = api_settings(&base(&server));
    s.speech_language = Some("de".to_string());
    s.auto_paste = false;
    let outcome = service.save(SaveRequest {
        settings: s,
        keys: KeyEdits {
            transcription_api: KeyEdit::Replace(Secret::new(KEY)),
            local_server: KeyEdit::Untouched,
            post_processing: KeyEdit::Untouched,
        },
    });
    assert!(
        matches!(outcome, SaveOutcome::Saved { .. }),
        "save refused: {outcome:?}"
    );

    let mut h = harness_with(energy_gate(), None, creds);
    let rec = h.record(fixtures::speech_3s(), service.snapshot());
    let id = rec.id();
    let (report, _) = h.run(rec);
    h.ctrl.job_finished(id, report.end.clone(), Instant::now());

    assert_eq!(
        report,
        JobReport {
            end: JobEnd::Delivered {
                notice: Some(i18n::NOTICE_COPIED)
            },
            pending: None
        }
    );
    assert_eq!(h.clipboard.texts(), vec![MOCK_TEXT.to_string()]);
    assert_eq!(h.paster.calls(), vec![], "auto-paste off: no paster call");
    assert_overlay_message(&h, i18n::NOTICE_COPIED);
    let requests = received(&server).await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        header(&requests[0], "authorization"),
        Some("Bearer sk-test-SECRET")
    );
    assert_eq!(
        part(&multipart(&requests[0]), "language"),
        Some(b"de".as_slice())
    );
    let events = h.events();
    assert!(
        matches!(
            delivered(&events).as_slice(),
            [DictationEvent::Delivered {
                result: DeliveryResult::CopiedOnly,
                ..
            }]
        ),
        "{events:?}"
    );
}

#[tokio::test]
async fn blank_engine_text_is_no_speech() {
    // Step 3: Ok("") from the engine is NoSpeech: nothing is written or pasted,
    // and the pending recording of an earlier failure stays as it is. Bite: an
    // empty clipboard write / Delivered, or NoSpeech clearing the pending slot.
    let failing = server_with(ResponseTemplate::new(401)).await;
    let blank = server_with(ok_text("   ")).await;
    let mut h = harness();
    let (_, first) = h.dictate(fixtures::speech_3s(), api_settings(&base(&failing)));
    let Some(pending) = first.pending else {
        panic!("401 leaves a pending recording: {first:?}")
    };
    let before = h.events().len();

    let (_, report) = h.dictate(fixtures::speech_3s(), api_settings(&base(&blank)));
    assert_eq!(
        report,
        JobReport {
            end: JobEnd::Notice(i18n::NOTICE_NO_SPEECH),
            pending: None
        }
    );
    assert_eq!(h.clipboard.texts(), Vec::<String>::new());
    assert_eq!(h.paster.calls(), vec![]);
    assert_eq!(
        h.pipeline.pending(),
        Some((pending, FailureReason::InvalidApiKey))
    );
    assert_eq!(h.store.calls(), vec![StoreCall::Put(pending)]);
    assert_eq!(received(&blank).await.len(), 1);
    let events = h.events();
    let job = &events[before..];
    assert!(
        matches!(
            job,
            [
                DictationEvent::SpeechGate { speech: true, .. },
                DictationEvent::JobFinished {
                    engine: Some("api"),
                    outcome: OutcomeCode::NoSpeech,
                    failure: None,
                    ..
                }
            ]
        ),
        "{job:?}"
    );
    assert_overlay_message(&h, i18n::NOTICE_NO_SPEECH);
}

// ---- Acceptance 2: no speech -> no request -----------------------------------------

#[tokio::test]
async fn silence_cough_keyboard_make_no_request() {
    // FR-12 / invariant (1): no credential read and no HTTP request unless the
    // gate said speech. Bite: building the engine (reads the key) or transcribing
    // before the gate, a clipboard write or a pending recording for no speech, a
    // JobFinished naming an engine that was never built.
    let server = server_with(ok_text("must never be asked")).await;
    for (label, audio) in [
        ("silence", fixtures::silence_3s()),
        ("cough", fixtures::cough_1s()),
        ("keyboard", fixtures::keyboard_3s()),
    ] {
        let mut h = harness();
        let (id, report) = h.dictate(audio, api_settings(&base(&server)));
        assert_eq!(
            report,
            JobReport {
                end: JobEnd::Notice(i18n::NOTICE_NO_SPEECH),
                pending: None
            },
            "{label}"
        );
        assert_eq!(h.read_api_calls(), vec![], "{label}: no credential read");
        assert_eq!(h.clipboard.texts(), Vec::<String>::new(), "{label}");
        assert_eq!(h.paster.calls(), vec![], "{label}");
        assert_eq!(h.store.calls(), vec![], "{label}");
        assert_eq!(h.pipeline.pending(), None, "{label}");
        assert_overlay_message(&h, i18n::NOTICE_NO_SPEECH);
        let events = h.events();
        match events.as_slice() {
            [DictationEvent::SpeechGate {
                recording,
                detector: "energy",
                speech: false,
            }, DictationEvent::JobFinished {
                engine: None,
                outcome: OutcomeCode::NoSpeech,
                failure: None,
                http_status: None,
                ..
            }] => assert_eq!(*recording, id, "{label}"),
            other => panic!("{label}: events {other:?}"),
        }
    }
    assert_eq!(
        received(&server).await.len(),
        0,
        "no request for any fixture"
    );
}

#[tokio::test]
async fn vad_fallback_warns_once_across_jobs() {
    // The gate returns fallback_warning once; run_job turns it into the one
    // Warning{vad_fallback}, before that job's SpeechGate event. Bite: no
    // warning, a warning per job (run_job tracking its own flag wrongly), or the
    // warning after the job's other events.
    let server = server_with(ok_text(MOCK_TEXT)).await;
    let gate = SpeechGate::new(Err(VadError::Unavailable), EnergyDetector::new());
    let mut h = harness_with(gate, None, creds_with_key());
    let settings = api_settings(&base(&server));
    let (first, _) = h.dictate(fixtures::speech_3s(), settings.clone());
    let _ = h.dictate(fixtures::silence_3s(), settings.clone());
    let _ = h.dictate(fixtures::speech_3s(), settings);

    let events = h.events();
    let warnings: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, DictationEvent::Warning { .. }))
        .collect();
    assert_eq!(
        warnings,
        vec![&DictationEvent::Warning {
            code: WarningCode::VadFallback
        }],
        "{events:?}"
    );
    assert!(
        matches!(
            events.get(..4),
            Some([
                DictationEvent::Warning {
                    code: WarningCode::VadFallback
                },
                DictationEvent::SpeechGate {
                    detector: "energy",
                    speech: true,
                    ..
                },
                DictationEvent::JobFinished { .. },
                DictationEvent::Delivered { .. },
            ])
        ),
        "{events:?}"
    );
    assert!(matches!(
        events.get(1),
        Some(DictationEvent::SpeechGate { recording, .. }) if *recording == first
    ));
    assert_eq!(
        received(&server).await.len(),
        2,
        "two speech jobs, one silence"
    );
}

// ---- Acceptance 4: failure branches -------------------------------------------------

#[tokio::test]
async fn status_401_is_invalid_api_key_nothing_pasted_audio_pending() {
    // Acceptance 4. Bite: anything pasted or copied on a failure, the audio not
    // stored as pending (or another recording's audio), a retry inside the job,
    // the controller not told (tray Error / overlay message).
    let server = server_with(ResponseTemplate::new(401).set_body_string("invalid key")).await;
    let mut h = harness();
    let (_, report) = h.dictate(fixtures::speech_3s(), api_settings(&base(&server)));

    assert_eq!(report.end, JobEnd::Failed(FailureReason::InvalidApiKey));
    let Some(id) = report.pending else {
        panic!("401 leaves a pending recording: {report:?}")
    };
    assert_eq!(
        h.pipeline.pending(),
        Some((id, FailureReason::InvalidApiKey))
    );
    assert_eq!(h.store.stored_ids(), vec![id]);
    assert_eq!(
        h.store.get_pending(id).expect("pending audio stored"),
        fixtures::speech_3s()
    );
    assert_eq!(h.clipboard.texts(), Vec::<String>::new());
    assert_eq!(h.paster.calls(), vec![]);
    assert_eq!(received(&server).await.len(), 1, "no retry inside the job");
    assert_eq!(h.ctrl.indicator().tray, TrayState::Error);
    assert_overlay_message(&h, i18n::FAILURE_INVALID_API_KEY);
    let events = h.events();
    assert!(
        matches!(
            job_finished(&events).as_slice(),
            [DictationEvent::JobFinished {
                engine: Some("api"),
                outcome: OutcomeCode::Failed,
                failure: Some("InvalidApiKey"),
                http_status: None,
                ..
            }]
        ),
        "{events:?}"
    );
    assert_eq!(delivered(&events), vec![]);
}

#[tokio::test]
async fn no_answer_within_timeout_is_timeout() {
    // FR-24 through the pipeline: the pipeline's Timeouts reach the request (a
    // 300 ms limit against a 5 s delay). Bite: run_job using Timeouts::default()
    // (the 5 s answer would arrive and be delivered), Timeout not kept pending.
    let server = server_with(ok_text("late").set_delay(Duration::from_secs(5))).await;
    let t = Timeouts {
        connect: Duration::from_secs(2),
        api_transcription: Duration::from_millis(300),
        ..Timeouts::default()
    };
    let mut h = harness_with(energy_gate(), Some(t), creds_with_key());
    let rec = h.record(
        fixtures::speech_3s(),
        Arc::new(api_settings(&base(&server))),
    );
    let (report, took) = h.run(rec);
    assert_eq!(report.end, JobEnd::Failed(FailureReason::Timeout));
    assert!(took < Duration::from_secs(3), "took {took:?}");
    assert!(report.pending.is_some(), "{report:?}");
    assert_eq!(h.clipboard.texts(), Vec::<String>::new());
    assert_eq!(h.paster.calls(), vec![]);
}

/// The server's answer delay for the T-073 cases: above the configured 5 s limit
/// (the API minimum of decision #99) plus [`OVERHEAD_ALLOWANCE`], so the took
/// bound lies below it (T-078; was 7 s, 0.5 s above the old bound), and well
/// under the 30 s default.
const T073_DELAY: Duration = Duration::from_secs(10);

#[tokio::test]
async fn configured_api_timeout_fails_the_job_with_timeout() {
    // T-073 Acceptance failure branch, through the production pipeline
    // (`Pipeline::new`, no test override): a job whose settings snapshot says
    // api_transcription_s = 5 fails with Timeout against a server answering after
    // 10 s, about 5 s after the request started, with failure=Timeout in the job
    // event and the audio kept; the same server with the default snapshot (30 s)
    // is delivered. connect_s = 2 (in range, not 5) so that the connect value
    // read as the request limit ends the job at about 2 s, outside the window.
    // Bite: the pipeline's fixed Timeouts::default() (the 10 s answer arrives and
    // is delivered), the local-server value used for the API request (60 s:
    // delivered), the connect value used for it (2 s: too early), or
    // milliseconds.
    let server = server_with(ok_text("late but in time").set_delay(T073_DELAY)).await;

    let mut h = harness();
    let mut s = api_settings(&base(&server));
    s.timeouts.api_transcription_s = 5;
    s.timeouts.connect_s = 2;
    let rec = h.record(fixtures::speech_3s(), Arc::new(s));
    let (report, took) = h.run(rec);
    assert_eq!(report.end, JobEnd::Failed(FailureReason::Timeout));
    // Bounds (T-078): lower = the 5 s deadline minus 100 ms of timer slack (a
    // deadline never fires early; catches the 2 s connect value used as the
    // limit). Upper = 5 s + OVERHEAD_ALLOWANCE (3 s: client build, VAD, WAV,
    // drop under host load), 8 s, below the nearest wrong-deadline bite: a retry
    // after the timeout (2 x 5 s = 10 s, Timeout) and the server's 10 s answer;
    // a limit above 10 s (the 30 s default, the 60 s local-server value) is
    // caught by the reason (Delivered).
    let want = Duration::from_secs(5);
    let upper = want + OVERHEAD_ALLOWANCE;
    assert!(upper < T073_DELAY, "bound {upper:?} not below the bite");
    assert!(
        took >= want - Duration::from_millis(100) && took < upper,
        "took {took:?}: the 5 s limit of the snapshot (bound {upper:?})"
    );
    assert!(report.pending.is_some(), "{report:?}");
    assert_eq!(h.clipboard.texts(), Vec::<String>::new());
    assert_eq!(h.paster.calls(), vec![]);
    let events = h.events();
    assert!(
        matches!(
            job_finished(&events).as_slice(),
            [DictationEvent::JobFinished {
                outcome: OutcomeCode::Failed,
                failure: Some("Timeout"),
                ..
            }]
        ),
        "{events:?}"
    );

    // The same server, the default snapshot: within the FR-24 default 30 s.
    let mut h = harness();
    let rec = h.record(
        fixtures::speech_3s(),
        Arc::new(api_settings(&base(&server))),
    );
    let (report, _) = h.run(rec);
    assert!(
        matches!(report.end, JobEnd::Delivered { .. }),
        "default limit: {report:?}"
    );
    assert_eq!(h.clipboard.texts(), vec!["late but in time".to_string()]);
}

#[test]
fn refused_host_is_cannot_reach() {
    // Acceptance 4 "unreachable host": a refused loopback port is CannotReach
    // with host:port, at once, with the production pipeline. Bite: the error
    // turned into another reason, the whole URL as host, nothing kept pending.
    let addr = refused_addr();
    let mut h = harness();
    let rec = h.record(
        fixtures::speech_3s(),
        Arc::new(api_settings(&format!("http://{addr}/v1"))),
    );
    let (report, took) = h.run(rec);
    assert_eq!(
        report.end,
        JobEnd::Failed(FailureReason::CannotReach {
            host: addr.to_string()
        })
    );
    // "At once" = well before the connect timeout (FR-24: 5 s, the production
    // pipeline here) and the 30 s total: a refusal is not waited out. The bound is
    // the connect timeout minus 1 s, not a tighter one, because on windows-latest
    // one refused loopback connect itself takes ~2.1 s (Windows retries the SYN
    // after the RST; CI run 37183609259). Bite: a wait for the connect timeout or
    // the total, a sleep/backoff of ~4 s before failing, and on Windows a second
    // connect attempt (~2 x 2.1 s).
    // The bound stays tied to the production connect timeout, not to
    // REFUSAL_BUDGET (equal today, 5 s), so "a wait for the connect timeout" keeps
    // biting if the test budget ever grows. It must still leave room for the one
    // refusal that `refused_addr()` accepted (within REFUSAL_BUDGET / 2; T-048).
    let at_once = Timeouts::default().connect - Duration::from_secs(1);
    assert!(
        REFUSAL_BUDGET / 2 < at_once,
        "the bound {at_once:?} leaves no room for a refusal the probe accepts \
         (up to {:?})",
        REFUSAL_BUDGET / 2
    );
    assert!(took < at_once, "took {took:?}, limit {at_once:?}");
    assert!(report.pending.is_some(), "{report:?}");
    assert_eq!(h.clipboard.texts(), Vec::<String>::new());
    assert_eq!(h.paster.calls(), vec![]);
}

#[test]
fn blackhole_connect_is_bounded_by_connect_timeout() {
    // T-040 Notes (review 1 #3): that the connect limit reaches the client is
    // pinned here. 192.0.2.1 (RFC 5737) blackholes in voicen-rust:1.99 (probed),
    // so a 300 ms connect limit gives CannotReach well inside the 10 s total.
    // Where the host has no route at all (possibly windows-latest) the OS answers
    // at once with NetworkUnavailable and this test is vacuous there. Bite: no
    // connect_timeout on the client (the 10 s total fires: Timeout, caught by the
    // reason), the production connect value (5 s) instead of the job's (caught by
    // the bound below).
    let connect = Duration::from_millis(300);
    let t = Timeouts {
        connect,
        api_transcription: Duration::from_secs(10),
        ..Timeouts::default()
    };
    let mut h = harness_with(energy_gate(), Some(t), creds_with_key());
    let rec = h.record(
        fixtures::speech_3s(),
        Arc::new(api_settings("http://192.0.2.1/v1")),
    );
    let (report, took) = h.run(rec);
    assert!(
        matches!(
            &report.end,
            JobEnd::Failed(FailureReason::CannotReach { host }) if host == "192.0.2.1"
        ) || report.end == JobEnd::Failed(FailureReason::NetworkUnavailable),
        "expected CannotReach(192.0.2.1) or NetworkUnavailable, got {:?}",
        report.end
    );
    // Bound (T-078; was 1.5 s, overrun at 1.78 s under host load): the 300 ms
    // connect limit + OVERHEAD_ALLOWANCE (3 s: client build, VAD, WAV, drop),
    // 3.3 s, below the nearest wrong-deadline bite, the production connect
    // timeout (5 s, CannotReach at ~5 s); the 10 s total is caught by the reason.
    let upper = connect + OVERHEAD_ALLOWANCE;
    assert!(
        upper < Timeouts::default().connect,
        "bound {upper:?} not below the production connect timeout"
    );
    assert!(took < upper, "took {took:?}, bound {upper:?}");
}

#[tokio::test]
async fn key_store_error_and_unset_engine_send_nothing() {
    // Decision #44 through the pipeline: no engine -> no request, no clipboard,
    // and the recording is kept (both reasons are retryable). Bite: sending
    // without a key, a request for engine = none, the failure not kept pending,
    // a JobFinished naming an engine that was not built.
    let server = server_with(ok_text("must never be asked")).await;

    let creds = creds_with_key();
    creds.fail(
        CredentialOp::Read,
        KeySlot::TranscriptionApi,
        CredentialError { os_code: 1312 },
    );
    let mut h = harness_with(energy_gate(), None, creds);
    let (_, report) = h.dictate(fixtures::speech_3s(), api_settings(&base(&server)));
    assert_eq!(
        report.end,
        JobEnd::Failed(FailureReason::KeyStoreUnavailable)
    );
    assert!(report.pending.is_some(), "{report:?}");
    assert_eq!(h.clipboard.texts(), Vec::<String>::new());
    let events = h.events();
    assert!(
        matches!(
            job_finished(&events).as_slice(),
            [DictationEvent::JobFinished {
                engine: None,
                outcome: OutcomeCode::Failed,
                failure: Some("KeyStoreUnavailable"),
                ..
            }]
        ),
        "{events:?}"
    );

    let mut h = harness();
    let mut s = api_settings(&base(&server));
    s.engine = EngineKind::None;
    let (_, report) = h.dictate(fixtures::speech_3s(), s);
    assert_eq!(
        report.end,
        JobEnd::Failed(FailureReason::EngineNotConfigured)
    );
    assert!(report.pending.is_some(), "{report:?}");
    assert_eq!(h.read_api_calls(), vec![], "no key read for engine none");
    assert_eq!(h.clipboard.texts(), Vec::<String>::new());

    assert_eq!(received(&server).await.len(), 0);
}

#[tokio::test]
async fn newer_failure_replaces_pending_and_deletes_old_audio() {
    // Invariant (4): one pending slot; a newer retryable failure holds exactly its
    // own id and audio and the older audio is deleted; Text and NoSpeech leave it.
    // Bite: two pending recordings, the old audio left on disk, the slot keeping
    // the old id or reason, a reused id, Text/NoSpeech clearing the slot.
    let s401 = server_with(ResponseTemplate::new(401)).await;
    let s500 = server_with(ResponseTemplate::new(500)).await;
    let s200 = server_with(ok_text(MOCK_TEXT)).await;
    let second_audio = fixtures::speech(11, 3.0, 0.0, -12.0);
    assert_ne!(second_audio, fixtures::speech_3s(), "premise: other audio");
    assert!(
        EnergyDetector::new().detect(&second_audio),
        "premise: the second recording is speech"
    );
    let mut h = harness();

    let (_, first) = h.dictate(fixtures::speech_3s(), api_settings(&base(&s401)));
    let (_, second) = h.dictate(second_audio.clone(), api_settings(&base(&s500)));
    let (Some(id1), Some(id2)) = (first.pending, second.pending) else {
        panic!("both failures are retryable: {first:?} {second:?}")
    };
    assert!(id2 > id1, "pending ids are monotonic: {id1:?} {id2:?}");
    assert_eq!(
        second.end,
        JobEnd::Failed(FailureReason::ServerError { status: 500 })
    );
    let after_failures = vec![
        StoreCall::Put(id1),
        StoreCall::Put(id2),
        StoreCall::Delete(id1),
    ];
    assert_eq!(h.store.calls(), after_failures);
    assert_eq!(h.store.stored_ids(), vec![id2]);
    assert_eq!(
        h.pipeline.pending(),
        Some((id2, FailureReason::ServerError { status: 500 }))
    );
    let events = h.events();
    assert!(
        matches!(
            job_finished(&events).as_slice(),
            [
                DictationEvent::JobFinished {
                    http_status: None,
                    ..
                },
                DictationEvent::JobFinished {
                    failure: Some("ServerError"),
                    http_status: Some(500),
                    ..
                }
            ]
        ),
        "{events:?}"
    );

    let (_, text) = h.dictate(fixtures::speech_3s(), api_settings(&base(&s200)));
    let (_, none) = h.dictate(fixtures::silence_3s(), api_settings(&base(&s200)));
    assert_eq!(text.end, JobEnd::Delivered { notice: None });
    assert_eq!(none.end, JobEnd::Notice(i18n::NOTICE_NO_SPEECH));
    assert_eq!((text.pending, none.pending), (None, None));
    assert_eq!(
        h.store.calls(),
        after_failures,
        "Text/NoSpeech leave the slot"
    );
    assert_eq!(
        h.pipeline.pending(),
        Some((id2, FailureReason::ServerError { status: 500 }))
    );
    assert_eq!(
        h.store.get_pending(id2).expect("second audio kept"),
        second_audio
    );
}

#[tokio::test]
async fn clipboard_failure_is_clipboard_unavailable_and_pending() {
    // data-model DeliveryDecision row 1: the write fails -> Failed(ClipboardUnavailable),
    // no paste, the recording kept; JobFinished is emitted after the write, so it
    // says failed and there is no Delivered. Bite: Ctrl+V after a failed write
    // (pastes the old clipboard), reporting Delivered, not keeping the audio.
    let server = server_with(ok_text(MOCK_TEXT)).await;
    let mut h = harness();
    h.clipboard.set_fail(true);
    let (_, report) = h.dictate(fixtures::speech_3s(), api_settings(&base(&server)));

    assert_eq!(
        report.end,
        JobEnd::Failed(FailureReason::ClipboardUnavailable)
    );
    let Some(id) = report.pending else {
        panic!("ClipboardUnavailable is retryable: {report:?}")
    };
    assert_eq!(
        h.pipeline.pending(),
        Some((id, FailureReason::ClipboardUnavailable))
    );
    assert_eq!(
        h.store.get_pending(id).expect("pending audio stored"),
        fixtures::speech_3s()
    );
    assert_eq!(
        h.clipboard.texts(),
        vec![MOCK_TEXT.to_string()],
        "one attempt"
    );
    assert_eq!(h.paster.calls(), vec![]);
    assert_eq!(h.ctrl.indicator().tray, TrayState::Error);
    assert_overlay_message(&h, i18n::FAILURE_CLIPBOARD_UNAVAILABLE);
    let events = h.events();
    assert!(
        matches!(
            job_finished(&events).as_slice(),
            [DictationEvent::JobFinished {
                engine: Some("api"),
                outcome: OutcomeCode::Failed,
                failure: Some("ClipboardUnavailable"),
                ..
            }]
        ),
        "{events:?}"
    );
    assert_eq!(delivered(&events), vec![]);
}

#[tokio::test]
async fn pending_store_failure_reports_without_pending() {
    // Decision #47 (4): a failed put gives no pending recording, and the older
    // pending audio is still deleted (the tray never offers a dictation older than
    // the last failure). Bite: report.pending set although nothing was stored,
    // the old slot kept, the old audio left behind.
    let server = server_with(ResponseTemplate::new(401)).await;
    let mut h = harness();
    let (_, first) = h.dictate(fixtures::speech_3s(), api_settings(&base(&server)));
    let Some(id1) = first.pending else {
        panic!("401 leaves a pending recording: {first:?}")
    };

    h.store.set_fail_put(true);
    let (_, second) = h.dictate(fixtures::speech_3s(), api_settings(&base(&server)));
    assert_eq!(second.end, JobEnd::Failed(FailureReason::InvalidApiKey));
    assert_eq!(second.pending, None);
    assert_eq!(h.pipeline.pending(), None);
    assert_eq!(h.store.stored_ids(), Vec::<PendingId>::new());
    let calls = h.store.calls();
    assert!(
        matches!(
            calls.as_slice(),
            [StoreCall::Put(a), StoreCall::Put(b), StoreCall::Delete(c)] if *a == id1 && *b != id1 && *c == id1
        ),
        "{calls:?}"
    );
    assert_eq!(h.ctrl.indicator().tray, TrayState::Error);
}

// ---- redaction ----------------------------------------------------------------------

#[tokio::test]
async fn no_event_or_failure_carries_key_query_or_transcript() {
    // FR-20 / NFR-04 / P-009: the key, the base URL's query and the transcript
    // reach no event, no JobReport and no failure text, on every path. Positive
    // controls: the key and the query were sent, and the transcript did reach the
    // clipboard. Bite: a String field in an event, a FailureReason built from a
    // reqwest error or a body, the text kept in JobReport.
    let marker_body = format!("{TRANSCRIPT} {KEY}");
    let ok = server_with(ok_text(TRANSCRIPT)).await;
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
    // The refused scenario must not run under `ms`: on windows-latest the refusal
    // takes ~2.17 s, so the 300 ms total would fire first and end it `Timeout`
    // (T-048, CI run 37204170764). The "timeout" scenario keeps `ms` against the
    // slow server.
    let refused_t = refused_timeouts();

    // (label, base URL, auto_paste, clipboard fails, expected end, timeouts)
    let scenarios: Vec<(&str, String, bool, bool, JobEnd, Timeouts)> = vec![
        (
            "pasted",
            with_query(&base(&ok)),
            true,
            false,
            JobEnd::Delivered { notice: None },
            ms,
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
        ),
        (
            "clipboard fails",
            with_query(&base(&ok)),
            true,
            true,
            JobEnd::Failed(FailureReason::ClipboardUnavailable),
            ms,
        ),
        (
            "401",
            with_query(&base(&s401)),
            true,
            false,
            JobEnd::Failed(FailureReason::InvalidApiKey),
            ms,
        ),
        (
            "500",
            with_query(&base(&s500)),
            true,
            false,
            JobEnd::Failed(FailureReason::ServerError { status: 500 }),
            ms,
        ),
        (
            "bad body",
            with_query(&base(&bad)),
            true,
            false,
            JobEnd::Failed(FailureReason::UnexpectedResponse),
            ms,
        ),
        (
            "timeout",
            with_query(&base(&slow)),
            true,
            false,
            JobEnd::Failed(FailureReason::Timeout),
            ms,
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
        ),
    ];

    let mut texts: Vec<String> = Vec::new();
    let mut ends = Vec::new();
    let mut clipboard_texts = Vec::new();
    for (label, base_url, auto_paste, clipboard_fails, want, t) in scenarios {
        let mut h = harness_with(energy_gate(), Some(t), creds_with_key());
        h.clipboard.set_fail(clipboard_fails);
        let mut s = api_settings(&base_url);
        s.auto_paste = auto_paste;
        let (_, report) = h.dictate(fixtures::speech_3s(), s);
        ends.push((label, report.end.clone(), want));
        texts.push(format!("{label}: report {report:?}"));
        texts.push(format!("{label}: pending {:?}", h.pipeline.pending()));
        for e in h.events() {
            texts.push(format!("{label}: event {e:?}"));
        }
        if let JobEnd::Failed(r) = &report.end {
            texts.push(format!("{label}: reason {r:?} / {r}"));
            let params = r.message_params();
            let args: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
            for lang in [UiLanguage::En, UiLanguage::Ru] {
                texts.push(format!("{label}: {}", text(lang, r.message_id(), &args)));
            }
        }
        texts.push(format!("{label}: indicator {:?}", h.ctrl.indicator()));
        clipboard_texts.extend(h.clipboard.texts());
    }

    let wrong: Vec<String> = ends
        .iter()
        .filter(|(_, got, want)| got != want)
        .map(|(label, got, want)| format!("{label}: {got:?}, expected {want:?}"))
        .collect();
    assert!(
        wrong.is_empty(),
        "paths not exercised:\n{}",
        wrong.join("\n")
    );

    // Positive controls: the inputs did flow.
    assert!(
        clipboard_texts.iter().any(|t| t == TRANSCRIPT),
        "the transcript reached the clipboard: {clipboard_texts:?}"
    );
    let r = &received(&ok).await[0];
    assert_eq!(r.url.query(), Some("api-version=SECRETQ"));
    assert_eq!(header(r, "authorization"), Some("Bearer sk-test-SECRET"));

    let leaks: Vec<&String> = texts
        .iter()
        .filter(|t| t.contains(KEY) || t.contains(QUERY_SECRET) || t.contains(TRANSCRIPT))
        .collect();
    assert!(leaks.is_empty(), "leaked:\n{leaks:#?}");
}
