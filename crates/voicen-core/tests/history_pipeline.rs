//! T-022: what enters the history is decided in the one delivery point of the
//! pipeline (`Pipeline::release`, the `Ok(deliver)` branch): the final text as
//! delivered, once per job, and nothing for a job that did not deliver (spec 005
//! FR-001, tasks T013; Acceptance "failed/empty/cancelled dictations create no
//! entry"). A recording fake `TranscriptSink`, the new required `PipelineDeps`
//! field `transcript_sink`, stands for the recorder.
//!
//! Pinned API (the developer builds exactly this): `pipeline::TranscriptSink`
//! with `fn delivered(&self, text: &str, engine: history::EngineLabel)` (`Send +
//! Sync`), `PipelineDeps::transcript_sink: Arc<dyn TranscriptSink>`.
//!
//! The pipeline runs on plain std threads (blocking reqwest, T-040); wiremock
//! serves the engine. Fake data only: 127.0.0.1, `sk-test-SECRET`.

mod common;
#[path = "common/mock_http.rs"]
mod mock_http;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use common::timing::{ago, now};
use serde_json::json;
use voicen_core::audio::AudioBuffer;
use voicen_core::events::RecordingObserver;
use voicen_core::failure::FailureReason;
use voicen_core::history::{EngineKind as HistoryEngine, EngineLabel};
use voicen_core::pipeline::{
    JobReport, Pipeline, PipelineDeps, PressContext, RetryOutcome, TranscriptSink,
};
use voicen_core::platform::{
    FakeClipboard, FakePaster, FakeTempAudioStore, StartWindow, WindowRef,
};
use voicen_core::post_process::{PostProcessInput, PostProcessOutcome, PostProcessor, SkipReason};
use voicen_core::recording::{JobEnd, Press, RecordingController, Release};
use voicen_core::secrets::{FakeCredentialStore, KeySlot};
use voicen_core::settings::{defaults, EngineKind, Settings};
use voicen_core::test_support::fixtures;
use voicen_core::vad::{EnergyDetector, SpeechDetector, SpeechGate};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-test-SECRET";
/// Inner spacing, a tab and Cyrillic: "exactly the delivered text" is not
/// satisfied by a trimmed or normalised copy.
const RAW: &str = "привет  как\tдела, RAW-MARKER";
const PROCESSED: &str = "Привет, как дела? PROCESSED-MARKER";
const WINDOW: WindowRef = WindowRef(0x0001_0042);

// ---- fakes ----------------------------------------------------------------------

#[derive(Default)]
struct RecordingSink {
    calls: Mutex<Vec<(String, EngineLabel)>>,
}

impl RecordingSink {
    fn calls(&self) -> Vec<(String, EngineLabel)> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl TranscriptSink for RecordingSink {
    fn delivered(&self, text: &str, engine: EngineLabel) {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((text.to_string(), engine));
    }
}

/// A post-processor with a fixed outcome.
struct FixedPost(PostProcessOutcome);

impl PostProcessor for FixedPost {
    fn process(&self, _raw: &str, _input: &PostProcessInput<'_>) -> PostProcessOutcome {
        self.0.clone()
    }
}

// ---- harness ----------------------------------------------------------------------

fn window() -> StartWindow {
    StartWindow {
        handle: WINDOW,
        process_id: 4242,
        elevated: false,
    }
}

fn energy_gate() -> SpeechGate {
    SpeechGate::new(
        Ok(Box::new(EnergyDetector::new()) as Box<dyn SpeechDetector>),
        EnergyDetector::new(),
    )
}

struct Rig {
    clipboard: Arc<FakeClipboard>,
    sink: Arc<RecordingSink>,
    pipeline: Pipeline,
    ctrl: RecordingController<PressContext>,
}

fn rig() -> Rig {
    rig_with(Arc::new(voicen_core::post_process::PassThrough))
}

fn rig_with(post: Arc<dyn PostProcessor>) -> Rig {
    let clipboard = Arc::new(FakeClipboard::new());
    let sink = Arc::new(RecordingSink::default());
    let creds = Arc::new(FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, KEY));
    let pipeline = Pipeline::new(PipelineDeps {
        gate: energy_gate(),
        credentials: creds,
        clipboard: clipboard.clone(),
        paster: Arc::new(FakePaster::new()),
        temp_audio: Arc::new(FakeTempAudioStore::new()),
        observer: Arc::new(RecordingObserver::new()),
        post_processor: post,
        transcript_sink: sink.clone(),
    });
    Rig {
        clipboard,
        sink,
        pipeline,
        ctrl: RecordingController::new(),
    }
}

impl Rig {
    /// Record, run the job on a plain thread, close it as the session does.
    fn dictate(&mut self, audio: AudioBuffer, settings: Settings) -> JobReport {
        let t0 = ago(Duration::from_secs(4));
        let ctx = PressContext {
            start_window: Some(window()),
            settings: Arc::new(settings),
        };
        assert!(matches!(self.ctrl.press(t0, ctx), Press::Start(_)));
        let stop = t0 + Duration::from_secs(3);
        let Release::Stop(ticket) = self.ctrl.release(stop) else {
            panic!("expected a stop ticket")
        };
        let rec = self
            .ctrl
            .finish(ticket, Ok(audio), stop)
            .expect("finish(Ok) gives the recording");
        let id = rec.id();
        let pipeline = &self.pipeline;
        let report = std::thread::scope(|s| {
            s.spawn(move || pipeline.run_job(rec))
                .join()
                .expect("run_job must not panic")
        });
        self.ctrl.job_finished(id, report.end.clone(), now());
        report
    }

    fn retry(&self, id: voicen_core::platform::PendingId, settings: &Settings) -> RetryOutcome {
        let pipeline = &self.pipeline;
        std::thread::scope(|s| {
            s.spawn(move || pipeline.retry(Some(id), settings, now()))
                .join()
                .expect("retry must not panic")
        })
    }
}

fn api_settings(base: &str, auto_paste: bool) -> Settings {
    let mut s = defaults(Some("en-US"));
    s.engine = EngineKind::Api;
    s.api.base_url = base.to_string();
    s.api.model = "whisper-1".to_string();
    s.speech_language = None;
    s.auto_paste = auto_paste;
    s
}

async fn server_with(response: ResponseTemplate) -> MockServer {
    let server = mock_http::start().await;
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

fn api_label() -> EngineLabel {
    EngineLabel {
        kind: HistoryEngine::Api,
        model: "whisper-1".to_string(),
    }
}

// ---- delivered jobs record exactly one entry --------------------------------------------

#[tokio::test]
async fn a_pasted_job_records_the_delivered_text_once_with_the_engine_label() {
    // Bite: no call, a call before delivery/for every outcome, two calls, the
    // text trimmed or altered, the wrong engine label.
    let server = server_with(ok_text(RAW)).await;
    let mut r = rig();
    let report = r.dictate(fixtures::speech_3s(), api_settings(&base(&server), true));
    assert_eq!(report.end, JobEnd::Delivered { notice: None });
    assert_eq!(r.sink.calls(), vec![(RAW.to_string(), api_label())]);
    assert_eq!(
        r.clipboard.texts(),
        vec![RAW.to_string()],
        "the same text as delivered"
    );
}

#[tokio::test]
async fn a_copied_only_job_records_the_delivered_text() {
    // auto-paste off: CopiedOnly is a delivery too (spec 005 T013: delivered or
    // copied-only). Bite: recording only when Ctrl+V was sent.
    let server = server_with(ok_text(RAW)).await;
    let mut r = rig();
    let report = r.dictate(fixtures::speech_3s(), api_settings(&base(&server), false));
    assert!(
        matches!(report.end, JobEnd::Delivered { notice: Some(_) }),
        "{report:?}"
    );
    assert_eq!(r.sink.calls(), vec![(RAW.to_string(), api_label())]);
}

#[tokio::test]
async fn skipped_post_processing_records_the_raw_text() {
    // The delivered text is the raw one when the stage failed. Bite: recording the
    // processed text / an empty text when the stage was skipped.
    let server = server_with(ok_text(RAW)).await;
    let mut r = rig_with(Arc::new(FixedPost(PostProcessOutcome::Skipped(
        SkipReason::Timeout,
    ))));
    let report = r.dictate(fixtures::speech_3s(), api_settings(&base(&server), true));
    assert!(
        matches!(report.end, JobEnd::DeliveredSkipped { .. }),
        "{report:?}"
    );
    assert_eq!(r.clipboard.texts(), vec![RAW.to_string()]);
    assert_eq!(r.sink.calls(), vec![(RAW.to_string(), api_label())]);
}

#[tokio::test]
async fn applied_post_processing_records_the_processed_text_not_the_raw_one() {
    // Bite: recording the engine's raw text instead of what was delivered.
    let server = server_with(ok_text(RAW)).await;
    let mut r = rig_with(Arc::new(FixedPost(PostProcessOutcome::Applied(
        PROCESSED.to_string(),
    ))));
    r.dictate(fixtures::speech_3s(), api_settings(&base(&server), true));
    assert_eq!(r.clipboard.texts(), vec![PROCESSED.to_string()]);
    assert_eq!(r.sink.calls(), vec![(PROCESSED.to_string(), api_label())]);
}

#[tokio::test]
async fn two_delivered_jobs_record_two_entries_in_order() {
    let a = server_with(ok_text("first text")).await;
    let b = server_with(ok_text("second text")).await;
    let mut r = rig();
    r.dictate(fixtures::speech_3s(), api_settings(&base(&a), true));
    r.dictate(fixtures::speech_3s(), api_settings(&base(&b), true));
    let texts: Vec<String> = r.sink.calls().into_iter().map(|(t, _)| t).collect();
    assert_eq!(
        texts,
        vec!["first text".to_string(), "second text".to_string()]
    );
}

// ---- retry ---------------------------------------------------------------------------------

#[tokio::test]
async fn a_successful_retry_records_exactly_one_entry_and_the_failed_run_none() {
    // Bite: recording at the first (failed) run, no call on the retry path,
    // a second call for the same job.
    let failing = server_with(ResponseTemplate::new(401)).await;
    let working = server_with(ok_text(RAW)).await;
    let mut r = rig();
    let first = r.dictate(fixtures::speech_3s(), api_settings(&base(&failing), true));
    let Some(id) = first.pending else {
        panic!("401 leaves a pending recording: {first:?}")
    };
    assert_eq!(r.sink.calls(), vec![], "a failed job records nothing");

    let outcome = r.retry(id, &api_settings(&base(&working), true));
    assert!(
        matches!(
            &outcome,
            RetryOutcome::Ran(JobReport {
                end: JobEnd::Delivered { .. },
                ..
            })
        ),
        "{outcome:?}"
    );
    assert_eq!(r.sink.calls(), vec![(RAW.to_string(), api_label())]);
}

#[tokio::test]
async fn a_retry_that_fails_again_records_nothing() {
    let failing = server_with(ResponseTemplate::new(500)).await;
    let mut r = rig();
    let first = r.dictate(fixtures::speech_3s(), api_settings(&base(&failing), true));
    let Some(id) = first.pending else {
        panic!("500 leaves a pending recording: {first:?}")
    };
    let outcome = r.retry(id, &api_settings(&base(&failing), true));
    assert!(
        matches!(
            &outcome,
            RetryOutcome::Ran(JobReport {
                end: JobEnd::Failed(_),
                ..
            })
        ),
        "{outcome:?}"
    );
    assert_eq!(r.sink.calls(), vec![]);
}

// ---- jobs that do not deliver record nothing -------------------------------------------------

#[tokio::test]
async fn no_speech_records_nothing() {
    // Silence stopped by the gate, and a blank engine text. Bite: recording the
    // blank text, recording on every JobFinished.
    let blank = server_with(ok_text("   ")).await;
    let mut r = rig();
    let silent = r.dictate(fixtures::silence_3s(), api_settings(&base(&blank), true));
    let empty = r.dictate(fixtures::speech_3s(), api_settings(&base(&blank), true));
    assert!(matches!(silent.end, JobEnd::Notice(_)), "{silent:?}");
    assert!(matches!(empty.end, JobEnd::Notice(_)), "{empty:?}");
    assert_eq!(r.sink.calls(), vec![]);
}

#[tokio::test]
async fn a_failed_job_records_nothing() {
    let server = server_with(ResponseTemplate::new(401)).await;
    let mut r = rig();
    let report = r.dictate(fixtures::speech_3s(), api_settings(&base(&server), true));
    assert_eq!(report.end, JobEnd::Failed(FailureReason::InvalidApiKey));
    assert_eq!(r.sink.calls(), vec![]);
}

#[tokio::test]
async fn clipboard_unavailable_records_nothing() {
    // The text was transcribed but never delivered (spec 005 T013). Bite:
    // recording before `deliver`, ahead of its outcome.
    let server = server_with(ok_text(RAW)).await;
    let mut r = rig();
    r.clipboard.set_fail(true);
    let report = r.dictate(fixtures::speech_3s(), api_settings(&base(&server), true));
    assert_eq!(
        report.end,
        JobEnd::Failed(FailureReason::ClipboardUnavailable)
    );
    assert_eq!(r.sink.calls(), vec![]);
}

#[tokio::test]
async fn a_cancelled_recording_records_nothing() {
    let mut r = rig();
    let t0 = ago(Duration::from_secs(2));
    let ctx = PressContext {
        start_window: Some(window()),
        settings: Arc::new(api_settings("http://127.0.0.1:9/v1", true)),
    };
    assert!(matches!(r.ctrl.press(t0, ctx), Press::Start(_)));
    assert!(
        r.ctrl.cancel(t0 + Duration::from_secs(1)).is_some(),
        "premise: a live recording was cancelled"
    );
    assert_eq!(r.sink.calls(), vec![]);
    assert_eq!(r.clipboard.texts(), Vec::<String>::new());
}
