//! T-020: the post-processing deadline (FR-24, spec 003 FR-004 / SC-003, decision
//! #99). Its own binary because both cases take real seconds: the production
//! default (15 s) against a server that accepts and stays silent, and a configured
//! `post_processing_s = 5` read from the job's settings snapshot through the
//! production pipeline (`Pipeline::new`), the reader T-073 left to T-020.
//!
//! The processor and the pipeline run on plain std threads (the blocking reqwest
//! client panics inside a tokio runtime context, T-040). Timeouts are tested with
//! servers that accept and answer late or never, never with a refused or
//! unroutable address (F-004, F-005). Fake data only: 127.0.0.1, `sk-test-...`.
//!
//! Wall-clock bounds (T-078): the lower side is the deadline minus [`SLACK`] (a
//! deadline never fires early); the upper side is the deadline plus
//! `common::OVERHEAD_ALLOWANCE`, below the nearest wrong-deadline bite, which each
//! test names. The baseline is the fastest of [`BASELINE_SAMPLES`] runs, so one
//! cold sample does not shift the stage.

mod common;

use std::io::Read;
use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::OVERHEAD_ALLOWANCE;
use serde_json::json;
use voicen_core::delivery::DeliveryResult;
use voicen_core::events::{DictationEvent, OutcomeCode, RecordingObserver};
use voicen_core::pipeline::{JobReport, Pipeline, PipelineDeps, PressContext};
use voicen_core::platform::{
    FakeClipboard, FakePaster, FakeTempAudioStore, StartWindow, WindowRef,
};
use voicen_core::post_process::chat::ChatPostProcessor;
use voicen_core::post_process::settings::PostProcessingSettings;
use voicen_core::post_process::{PostProcessInput, PostProcessOutcome, PostProcessor, SkipReason};
use voicen_core::recording::{JobEnd, Press, RecordingController, Release};
use voicen_core::secrets::{FakeCredentialStore, KeySlot};
use voicen_core::settings::{defaults, EngineKind, Settings};
use voicen_core::test_support::fixtures;
use voicen_core::timeouts::Timeouts;
use voicen_core::vad::{EnergyDetector, SpeechDetector, SpeechGate};
use wiremock::matchers::path_regex;
use wiremock::{Mock, MockServer, ResponseTemplate};

const API_KEY: &str = "sk-test-SECRET";
const PP_KEY: &str = "sk-test-pp-SECRET";
/// The engine's text (after its own trim): inner spacing, a tab, Cyrillic and a
/// marker, so "byte for byte" is not satisfied by a normalised copy.
const RAW: &str = "привет  как\tдела, TRANSCRIPT-MARKER";
const PROCESSED: &str = "Привет, как дела? PROCESSED";

/// Wall-clock tolerance of contract guarantee 4 (SC-003) on the lower side: the
/// stage may come out this much under the deadline (baseline noise), never more.
const SLACK: Duration = Duration::from_millis(500);

/// Baseline runs per test; the fastest one is the baseline (T-078: a single cold
/// sample gave 1.81 s once, against 0.46 s on another run).
const BASELINE_SAMPLES: usize = 3;

/// A listener that accepts one connection, reads the request and never answers;
/// held open for `hold`. The thread is not joined.
fn silent_server(hold: Duration) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
    let addr = listener.local_addr().expect("local addr");
    std::thread::spawn(move || {
        let Ok((mut s, _)) = listener.accept() else {
            return;
        };
        let started = Instant::now();
        let _ = s.set_read_timeout(Some(Duration::from_millis(200)));
        let mut buf = [0u8; 8192];
        while started.elapsed() < hold {
            if let Ok(0) = s.read(&mut buf) {
                return;
            }
        }
    });
    format!("http://{addr}/v1")
}

/// `ChatPostProcessor::process` with the production durations on a plain std
/// thread, timed.
fn process_timed(base: String) -> (PostProcessOutcome, Duration) {
    let settings = PostProcessingSettings {
        enabled: true,
        base_url: base,
        model: "gpt-test-mini".to_string(),
        prompt: "PROMPT-MARKER".to_string(),
    };
    let creds = FakeCredentialStore::new().with_key(KeySlot::PostProcessing, PP_KEY);
    let timeouts = Timeouts::default();
    std::thread::scope(|s| {
        s.spawn(|| {
            let input = PostProcessInput {
                settings: &settings,
                credentials: &creds,
                timeouts: &timeouts,
            };
            let started = Instant::now();
            let got = ChatPostProcessor::new().process(RAW, &input);
            (got, started.elapsed())
        })
        .join()
        .expect("process must not panic")
    })
}

#[tokio::test]
async fn default_deadline_skips_a_silent_server_at_15_s() {
    // FR-24 / spec 003 FR-004, SC-003: with the production durations
    // (Timeouts::default(), post_processing 15 s) a server that accepts and never
    // answers ends the stage as Skipped(Timeout) 15 s +- 0.5 s after the call. The
    // time is taken against a baseline call to a server that answers at once
    // (client setup, connect, round trip), so a loaded debug build is not counted
    // against the deadline. Bite: no request timeout (hangs until the server
    // closes at 30 s), the connect timeout (5 s) or the transcription one (30 s)
    // as the deadline, a fixed deadline of its own.
    let prompt = MockServer::start().await;
    Mock::given(path_regex("/chat/completions$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "role": "assistant", "content": PROCESSED } }]
        })))
        .mount(&prompt)
        .await;
    let mut baseline = Duration::MAX;
    for _ in 0..BASELINE_SAMPLES {
        let (got, took) = process_timed(format!("{}/v1", prompt.uri()));
        assert_eq!(
            got,
            PostProcessOutcome::Applied(PROCESSED.to_string()),
            "baseline"
        );
        baseline = baseline.min(took);
    }

    let (got, elapsed) = process_timed(silent_server(Duration::from_secs(30)));
    assert_eq!(got, PostProcessOutcome::Skipped(SkipReason::Timeout));
    // Upper = 15 s + OVERHEAD_ALLOWANCE (3 s: client build, connect, drop under
    // host load), 18 s, below the nearest wrong-deadline bite: the transcription
    // deadline (30 s), the server closing at 30 s, a retry after the timeout
    // (2 x 15 s). Lower = 15 s - SLACK: catches the connect value (5 s).
    let want = Duration::from_secs(15);
    let upper = want + OVERHEAD_ALLOWANCE;
    assert!(
        upper < Timeouts::default().api_transcription,
        "bound {upper:?} not below the bite"
    );
    let stage = elapsed.saturating_sub(baseline);
    assert!(
        stage + SLACK >= want && stage <= upper,
        "took {elapsed:?} against a {baseline:?} baseline: {stage:?}, expected 15 s \
         (-{SLACK:?}, +{OVERHEAD_ALLOWANCE:?})"
    );
}

// ---- the configured value, through the production pipeline ------------------------

/// A server that answers the transcription at once with `{"text": RAW}` and the
/// chat request after `chat_delay` with `PROCESSED`.
async fn transcribe_then_chat(chat_delay: Duration) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(path_regex("/audio/transcriptions$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "text": RAW })))
        .mount(&server)
        .await;
    Mock::given(path_regex("/chat/completions$"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({
                    "choices": [{ "message": { "role": "assistant", "content": PROCESSED } }]
                }))
                .set_delay(chat_delay),
        )
        .mount(&server)
        .await;
    server
}

fn snapshot(base: &str) -> Settings {
    let mut s = defaults(Some("en-US"));
    s.engine = EngineKind::Api;
    s.api.base_url = base.to_string();
    s.api.model = "whisper-1".to_string();
    s.speech_language = None;
    s.auto_paste = true;
    s.post_processing = PostProcessingSettings {
        enabled: true,
        base_url: base.to_string(),
        model: "gpt-test-mini".to_string(),
        prompt: "PROMPT-MARKER".to_string(),
    };
    s
}

struct Run {
    report: JobReport,
    took: Duration,
    clipboard: Vec<String>,
    events: Vec<DictationEvent>,
}

/// One dictation through `Pipeline::new` with `ChatPostProcessor`: press, release
/// 3 s later, finish, `run_job` on a plain thread (timed).
fn dictate(settings: Settings) -> Run {
    let clipboard = Arc::new(FakeClipboard::new());
    let observer = Arc::new(RecordingObserver::new());
    let creds = Arc::new(
        FakeCredentialStore::new()
            .with_key(KeySlot::TranscriptionApi, API_KEY)
            .with_key(KeySlot::PostProcessing, PP_KEY),
    );
    let pipeline = Pipeline::new(PipelineDeps {
        gate: SpeechGate::new(
            Ok(Box::new(EnergyDetector::new()) as Box<dyn SpeechDetector>),
            EnergyDetector::new(),
        ),
        credentials: creds,
        clipboard: clipboard.clone(),
        paster: Arc::new(FakePaster::new()),
        temp_audio: Arc::new(FakeTempAudioStore::new()),
        observer: observer.clone(),
        post_processor: Arc::new(ChatPostProcessor::new()),
    });
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
    assert!(matches!(ctrl.press(t0, ctx), Press::Start(_)));
    let stop = t0 + Duration::from_secs(3);
    let ticket = match ctrl.release(stop) {
        Release::Stop(t) => t,
        other => panic!("expected a stop ticket, got {other:?}"),
    };
    let rec = ctrl
        .finish(ticket, Ok(fixtures::speech_3s()), stop)
        .expect("finish(Ok) gives the recording");
    let pipeline = &pipeline;
    let (report, took) = std::thread::scope(|s| {
        s.spawn(move || {
            let started = Instant::now();
            let report = pipeline.run_job(rec);
            (report, started.elapsed())
        })
        .join()
        .expect("run_job must not panic")
    });
    Run {
        report,
        took,
        clipboard: clipboard.texts(),
        events: observer.events(),
    }
}

#[tokio::test]
async fn configured_post_processing_timeout_delivers_the_raw_transcript() {
    // Acceptance 2 with #99: a job whose snapshot says post_processing_s = 5 (the
    // #99 minimum), against a chat endpoint answering after 7 s, is delivered
    // with the raw transcript byte for byte, ends as DeliveredSkipped(Timeout),
    // no pending recording, JobFinished outcome text; the job takes 5 s (-0.5 s,
    // +OVERHEAD_ALLOWANCE) longer than the same job against a chat endpoint that
    // answers at once (the baseline, fastest of BASELINE_SAMPLES: gate, WAV,
    // transcription, chat round trip; about 0.6 s in a debug build, so the whole
    // job is not timed against 5 s). connect_s = 2, so the connect value
    // read as the request limit would end it at about 2 s. The same server with
    // the default snapshot (15 s) is applied. Bite: the processor on
    // Timeouts::default() (the 10 s answer applied), on the pipeline's test
    // override field (None in production), on the connect value, or the pipeline
    // deriving a second Timeouts for the processor that ignores the snapshot; the
    // processed text or nothing delivered on a skip; the skip ended as Delivered
    // or Failed.
    // 10 s (T-078: was 7 s), above the upper bound below.
    let chat_delay = Duration::from_secs(10);
    let server = transcribe_then_chat(chat_delay).await;
    let base = format!("{}/v1", server.uri());
    let mut s = snapshot(&base);
    s.timeouts.post_processing_s = 5;
    s.timeouts.connect_s = 2;

    // The baseline: the same snapshot against a chat endpoint answering at once.
    let prompt = transcribe_then_chat(Duration::ZERO).await;
    let prompt_base = format!("{}/v1", prompt.uri());
    let mut fast = snapshot(&prompt_base);
    fast.timeouts = s.timeouts;
    let mut baseline = Duration::MAX;
    for _ in 0..BASELINE_SAMPLES {
        let run = dictate(fast.clone());
        assert_eq!(
            run.report.end,
            JobEnd::Delivered { notice: None },
            "baseline"
        );
        assert_eq!(run.clipboard, vec![PROCESSED.to_string()], "baseline");
        baseline = baseline.min(run.took);
    }

    let run = dictate(s);
    assert_eq!(
        run.report,
        JobReport {
            end: JobEnd::DeliveredSkipped {
                reason: SkipReason::Timeout,
                delivery: DeliveryResult::Pasted,
            },
            pending: None,
        }
    );
    assert_eq!(run.clipboard, vec![RAW.to_string()]);
    assert!(
        run.clipboard
            .first()
            .is_some_and(|t| t.as_bytes() == RAW.as_bytes()),
        "raw transcript byte for byte: {:?}",
        run.clipboard
    );
    // Upper = 5 s + OVERHEAD_ALLOWANCE (3 s: two client builds, VAD, WAV, drop
    // under host load), 8 s, below the nearest wrong-deadline bite: a retry after
    // the timeout (2 x 5 s = 10 s, still skipped) and the chat's 10 s answer; a
    // limit above that (the 15 s default) is caught by the outcome (Applied).
    // Lower = 5 s - SLACK: catches the connect value (2 s).
    let want = Duration::from_secs(5);
    let upper = want + OVERHEAD_ALLOWANCE;
    assert!(upper < chat_delay, "bound {upper:?} not below the bite");
    let stage = run.took.saturating_sub(baseline);
    assert!(
        stage + SLACK >= want && stage <= upper,
        "took {:?} against a {baseline:?} baseline: {stage:?}, expected the snapshot's \
         5 s (-{SLACK:?}, +{OVERHEAD_ALLOWANCE:?})",
        run.took
    );
    let finished: Vec<&DictationEvent> = run
        .events
        .iter()
        .filter(|e| matches!(e, DictationEvent::JobFinished { .. }))
        .collect();
    assert!(
        matches!(
            finished.as_slice(),
            [DictationEvent::JobFinished {
                outcome: OutcomeCode::Text,
                failure: None,
                http_status: None,
                ..
            }]
        ),
        "{finished:?}"
    );

    // The same server, the default snapshot (post_processing 15 s): applied.
    let run = dictate(snapshot(&base));
    assert_eq!(
        run.report,
        JobReport {
            end: JobEnd::Delivered { notice: None },
            pending: None,
        },
        "default limit"
    );
    assert_eq!(run.clipboard, vec![PROCESSED.to_string()]);
}
