//! T-020: the post-processing deadline (FR-24, spec 003 FR-004 / SC-003, decision
//! #99). Its own binary because both cases take real seconds: the production
//! default (15 s) against a server that answers after 15.5 s, and a configured
//! `post_processing_s = 10` read from the job's settings snapshot through the
//! production pipeline (`Pipeline::new`), the reader T-073 left to T-020.
//!
//! The processor and the pipeline run on plain std threads (the blocking reqwest
//! client panics inside a tokio runtime context, T-040). Timeouts are tested with
//! servers that accept and answer late or never, never with a refused or
//! unroutable address (F-004, F-005). Fake data only: 127.0.0.1, `sk-test-...`.
//!
//! The clock is read only through `common::timing` (T-080 I2), and only for the
//! lower bound (`at_least`: the deadline did not fire early). SC-003's tolerance
//! (± 0.5 s) is checked by its outcome, an outcome bracket (decision #149, T-110):
//! the server answers 200 with a valid body at deadline + tolerance, so the right
//! deadline gives the timeout outcome and a deadline past the tolerance gives
//! Applied; a sibling leg with a generous deadline proves the same server answers.
//! No wall-clock ceiling: a host stall can delay a case but not turn a correct
//! deadline red (F-021).

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::held::{returns_while_held, Held};
use common::timing::{ago, at_least, late_answer, measure, Took, M};

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

/// SC-003's tolerance over a deadline: the bracket server answers this much after
/// the deadline, so a deadline past the tolerance gets the answer (Applied).
const TOLERANCE: Duration = Duration::from_millis(500);

/// A chat server that answers every request `delay` after it has read it, with 200
/// and a valid `choices[0].message.content` of `PROCESSED`. wiremock reads the whole
/// request before the delay starts, so the answer comes at least `delay` after the
/// client's timers started (a raw listener that answers without draining the body
/// risks a RST on Windows).
async fn chat_answering_after(delay: Duration) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(path_regex("/chat/completions$"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({
                    "choices": [{ "message": { "role": "assistant", "content": PROCESSED } }]
                }))
                .set_delay(delay),
        )
        .mount(&server)
        .await;
    server
}

/// `ChatPostProcessor::process` with `timeouts` on a plain std thread, timed.
fn process_timed(base: String, timeouts: Timeouts) -> (PostProcessOutcome, Took) {
    let settings = PostProcessingSettings {
        enabled: true,
        base_url: base,
        model: "gpt-test-mini".to_string(),
        prompt: "PROMPT-MARKER".to_string(),
    };
    let creds = FakeCredentialStore::new().with_key(KeySlot::PostProcessing, PP_KEY);
    std::thread::scope(|s| {
        s.spawn(|| {
            let input = PostProcessInput {
                settings: &settings,
                credentials: &creds,
                timeouts: &timeouts,
            };
            measure(|| ChatPostProcessor::new().process(RAW, &input))
        })
        .join()
        .expect("process must not panic")
    })
}

#[tokio::test]
async fn default_deadline_skips_a_silent_server_at_15_s() {
    // FR-24 / spec 003 FR-004, SC-003 (15 s +- 0.5 s), checked by its outcome
    // (decision #149, T-110): with the production durations (Timeouts::default(),
    // post_processing 15 s) a server that stays silent until 15.5 s, the end of the
    // tolerance, and then answers 200 with a valid body ends the stage as
    // Skipped(Timeout), and not before 15 s. The sibling leg sends the same server
    // a 60 s deadline and gets Applied(PROCESSED): the server does answer, so the
    // Timeout leg is not passed by a broken body or a mock that never answers. No
    // wall-clock ceiling (F-021: a 1.4 s host stall in the window is not a defect).
    // Bite: no request timeout, the transcription (30 s) or local-server (60 s)
    // value or any deadline past 15.5 s as the limit (the answer is applied); the
    // connect timeout (5 s) or any deadline below 15 s (at_least); the answer read
    // as a timeout, or every error read as Timeout (the 60 s leg).
    let want = Duration::from_secs(15);
    let server = chat_answering_after(want + TOLERANCE).await;
    let base = format!("{}/v1", server.uri());

    let (got, took) = process_timed(base.clone(), Timeouts::default());
    assert_eq!(
        got,
        PostProcessOutcome::Skipped(SkipReason::Timeout),
        "the default deadline against an answer at {:?}",
        want + TOLERANCE
    );
    at_least(took, want);

    let generous = Timeouts {
        post_processing: Duration::from_secs(60),
        ..Timeouts::default()
    };
    let (got, _) = process_timed(base, generous);
    assert_eq!(
        got,
        PostProcessOutcome::Applied(PROCESSED.to_string()),
        "the same server, post_processing 60 s: it answers"
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
    took: Took,
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
    let t0 = ago(Duration::from_secs(4));
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
        s.spawn(move || measure(|| pipeline.run_job(rec)))
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

/// The configured post-processing limit and connect limit of the timeout leg below
/// (seconds). T-111: the connect limit must stay M below the limit, so a connect
/// value read as the request limit ends the job early and `at_least` sees it, and a
/// host stall of less than M cannot make the connect timer the verdict.
const PP_LIMIT_S: u32 = 10;
const PP_CONNECT_S: u32 = 5;
/// When the server of `configured_post_processing_timeout_*` answers. M after the limit
/// (kind (1) of the T-111 invariant), plus SC-003's tolerance so the answer also comes
/// after the 15 s default: a processor on `Timeouts::default()` ends the same test's
/// control leg as Skipped(Timeout).
const PP_LATE_ANSWER: Duration =
    late_answer(Duration::from_secs(PP_LIMIT_S as u64)).saturating_add(TOLERANCE);

/// The snapshot of the control leg: the same server, a post-processing limit that
/// the answer must beat. T-111 widens it (the answer is not the subject there; the
/// 15 s default is pinned by `default_deadline_skips_*`).
fn control_snapshot(base: &str) -> Settings {
    let mut s = snapshot(base);
    s.timeouts.post_processing_s = 60;
    s
}

#[test]
fn configured_timeout_legs_keep_the_in_time_margin_m() {
    // T-111: connect >= M (a loopback connect finishing within it is not a race) and
    // the limit M above the connect; the answer M above the limit and past the 15 s
    // default. Bite: the old connect_s = 2, the old answer at 10.5 s.
    let (connect, limit) = (
        Duration::from_secs(u64::from(PP_CONNECT_S)),
        Duration::from_secs(u64::from(PP_LIMIT_S)),
    );
    assert!(connect >= M, "connect {connect:?} < M {M:?}");
    assert!(
        limit >= connect + M,
        "limit {limit:?} is not M above connect {connect:?}"
    );
    assert!(
        PP_LATE_ANSWER >= limit + M,
        "answer {PP_LATE_ANSWER:?} is not M above limit {limit:?}"
    );
    assert!(
        PP_LATE_ANSWER > Timeouts::default().post_processing,
        "answer {PP_LATE_ANSWER:?} does not come after the default limit"
    );
}

#[tokio::test]
async fn control_leg_survives_a_chat_answer_stalled_past_the_default_limit() {
    // T-111 D3: an answer 15.5 s late (past the 15 s default limit) is applied by the
    // control snapshot, whose limit is far above any answer the test waits for.
    // Bite: the control snapshot left on the default 15 s (Skipped(Timeout)).
    let server = transcribe_then_chat(Duration::from_millis(15_500)).await;
    let run = dictate(control_snapshot(&format!("{}/v1", server.uri())));
    assert_eq!(
        run.report,
        JobReport {
            end: JobEnd::Delivered { notice: None },
            pending: None,
        }
    );
    assert_eq!(run.clipboard, vec![PROCESSED.to_string()]);
}

#[tokio::test]
async fn configured_post_processing_timeout_delivers_the_raw_transcript() {
    // Acceptance 2 with #99: a job whose snapshot says post_processing_s = 10
    // (T-111: M above connect_s; the #99 minimum is pinned by `TimeoutRole::bounds`),
    // against a chat endpoint answering after 15.5 s (`PP_LATE_ANSWER`; T-111: SC-003's
    // bracket is `default_deadline_skips_*`), is
    // delivered with the raw transcript byte for byte, ends as
    // DeliveredSkipped(Timeout), no pending recording, JobFinished outcome text,
    // and takes at least 10 s; no wall-clock ceiling (F-021). connect_s = 5, so
    // the connect value read as the request limit would end it at about 5 s
    // (at_least). The same server with post_processing_s = 60 is applied, so
    // the server does answer. Bite: the processor on Timeouts::default() (its 15 s
    // ends the control leg as Skipped), on any limit past 15.5 s (the answer applied),
    // on the pipeline's test override field (None in production), on the connect
    // value, or the pipeline deriving a second Timeouts for the processor that ignores
    // the snapshot; the processed text or nothing delivered on a skip; the skip ended
    // as Delivered or Failed.
    let want = Duration::from_secs(u64::from(PP_LIMIT_S));
    let server = transcribe_then_chat(PP_LATE_ANSWER).await;
    let base = format!("{}/v1", server.uri());
    let mut s = snapshot(&base);
    s.timeouts.post_processing_s = PP_LIMIT_S;
    s.timeouts.connect_s = PP_CONNECT_S;

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
    at_least(run.took, want);
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

    // The same server, the control snapshot (post_processing 60 s): applied.
    let run = dictate(control_snapshot(&base));
    assert_eq!(
        run.report,
        JobReport {
            end: JobEnd::Delivered { notice: None },
            pending: None,
        },
        "control limit"
    );
    assert_eq!(run.clipboard, vec![PROCESSED.to_string()]);
}

// ---- T-079: a lookup with no answer (decisions #106, #113) -------------------------

/// A snapshot whose transcription goes to `transcription` (an IP literal, no
/// lookup) and whose post-processing goes to the held name `pp_host`.
fn snapshot_with_held_pp(transcription: &MockServer, pp_host: &str) -> Settings {
    let mut s = snapshot(&format!("{}/v1", transcription.uri()));
    s.post_processing.base_url = format!("http://{pp_host}/v1");
    s
}

#[tokio::test]
async fn unanswered_pp_lookup_delivers_the_raw_transcript_at_the_connect_deadline() {
    // T-079 twin through the production pipeline (#106): the chat host's lookup
    // never answers while the job runs; connect_s = 1 (the minimum), post-
    // processing far longer. The job ends while the lookup is still held, with the
    // raw transcript pasted byte for byte and the skip reason Unreachable{host}.
    // Today the job returns only after the release (the client drop waits for the
    // lookup). Bite: the lookup joined at drop; the processor's client built
    // without the shared resolver (the seam never asked).
    let server = transcribe_then_chat(Duration::ZERO).await;
    let held = Held::install("pp-pipeline.t079.example.com", Vec::new());
    let mut s = snapshot_with_held_pp(&server, &held.host);
    s.timeouts.connect_s = 1;
    s.timeouts.post_processing_s = 60;
    let (run, _) = returns_while_held(&held, || dictate(s));
    assert_eq!(
        run.report,
        JobReport {
            end: JobEnd::DeliveredSkipped {
                reason: SkipReason::Unreachable {
                    host: held.host.clone()
                },
                delivery: DeliveryResult::Pasted,
            },
            pending: None,
        }
    );
    assert!(
        run.clipboard
            .first()
            .is_some_and(|t| t.as_bytes() == RAW.as_bytes())
            && run.clipboard.len() == 1,
        "raw transcript byte for byte: {:?}",
        run.clipboard
    );
    at_least(run.took, Duration::from_secs(1));
}

#[tokio::test]
async fn unanswered_pp_lookup_cut_by_the_post_processing_deadline_is_unreachable_not_timeout() {
    // Decision #113 (T-079 open question 1, the owner's example): connect_s = 60,
    // post_processing_s = 5, the chat host's lookup still unanswered when the 5 s
    // whole-request deadline fires -> a DNS failure whichever timer ended the send
    // -> Unreachable{host}, not Timeout; the raw transcript is delivered. Today:
    // DeliveredSkipped(Timeout), returned only after the release. Bite: the
    // lookup record read only for errors with the connect flag (Timeout).
    let server = transcribe_then_chat(Duration::ZERO).await;
    let held = Held::install("pp-total.t079.example.com", Vec::new());
    let mut s = snapshot_with_held_pp(&server, &held.host);
    s.timeouts.connect_s = 60;
    s.timeouts.post_processing_s = 5;
    let (run, _) = returns_while_held(&held, || dictate(s));
    assert_eq!(
        run.report,
        JobReport {
            end: JobEnd::DeliveredSkipped {
                reason: SkipReason::Unreachable {
                    host: held.host.clone()
                },
                delivery: DeliveryResult::Pasted,
            },
            pending: None,
        }
    );
    assert_eq!(run.clipboard, vec![RAW.to_string()]);
    at_least(run.took, Duration::from_secs(5));
}
