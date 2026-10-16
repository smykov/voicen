//! T-098 (spec 002 T021; NFR-05, FR-07, spec FR-010, SC-003): with the built-in
//! engine and post-processing off, dictation makes no network request at all.
//!
//! The witness is a loopback listener that counts every accepted connection. Every
//! URL in the settings snapshot (transcription API, local server, post-processing)
//! points at it and every key is stored, so an engine or stage that reached for the
//! network would land there. A positive control runs one API dictation through the
//! same pipeline and listener and sees it counted, so a dead listener cannot pass
//! the test.
//!
//! The `crates/voicen-whisper/Cargo.toml` half of T021 (no HTTP crate) belongs to
//! T-099, which creates that crate.
//!
//! Fake data only: 127.0.0.1, `sk-test-` keys.

mod common;
mod local_support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::timing::{ago, now};
use local_support::{local_catalog, place_models, FakeLoader, FakeMono, Script};
use voicen_core::clock::MonotonicClock;
use voicen_core::engine::builtin::BuiltinDeps;
use voicen_core::engine::engine_for;
use voicen_core::engine::speech_model::SpeechModelLoader;
use voicen_core::events::RecordingObserver;
use voicen_core::local_models::catalog::ModelId;
use voicen_core::local_models::residency::ModelResidency;
use voicen_core::local_models::store::ModelStore;
use voicen_core::pipeline::{JobReport, Pipeline, PipelineDeps, PressContext};
use voicen_core::platform::{FakeClipboard, FakePaster, FakeTempAudioStore};
use voicen_core::post_process::chat::ChatPostProcessor;
use voicen_core::recording::{JobEnd, Press, RecordingController, Release};
use voicen_core::secrets::{FakeCredentialStore, KeySlot};
use voicen_core::settings::{defaults, EngineKind, Settings};
use voicen_core::test_support::local_models::Server;
use voicen_core::test_support::{fixtures, TempDir};
use voicen_core::timeouts::{Timeouts, MODEL_IDLE_UNLOAD};
use voicen_core::vad::{EnergyDetector, SpeechDetector, SpeechGate};

const KEY: &str = "sk-test-SECRET";
const DICTATIONS: usize = 3;

fn settings(engine: EngineKind, server: &Server) -> Settings {
    let url = format!("http://127.0.0.1:{}/v1", server.port);
    let mut s = defaults(Some("en-US"));
    s.engine = engine;
    s.builtin_local.model_id = Some("tiny".to_string());
    s.api.base_url = url.clone();
    s.local_server.base_url = url.clone();
    s.post_processing.enabled = false;
    s.post_processing.base_url = url;
    s.post_processing.model = "gpt-test".to_string();
    s.speech_language = None;
    s.auto_paste = true;
    s
}

fn dictate(pipeline: &Pipeline, settings: Settings) -> JobReport {
    let mut ctrl = RecordingController::<PressContext>::new();
    let t0 = ago(Duration::from_secs(4));
    let ctx = PressContext {
        start_window: None,
        settings: Arc::new(settings),
    };
    let id = match ctrl.press(t0, ctx) {
        Press::Start(id) => id,
        Press::Ignored => panic!("press ignored"),
    };
    let stop = t0 + Duration::from_secs(3);
    let ticket = match ctrl.release(stop) {
        Release::Stop(ticket) => ticket,
        other => panic!("expected a stop ticket, got {other:?}"),
    };
    let rec = ctrl
        .finish(ticket, Ok(fixtures::speech_3s()), stop)
        .expect("finish(Ok) gives the recording");
    let report = std::thread::scope(|s| {
        s.spawn(|| pipeline.run_job(rec))
            .join()
            .expect("run_job must not panic")
    });
    ctrl.job_finished(id, report.end.clone(), now());
    report
}

#[test]
fn builtin_dictations_with_post_processing_off_make_no_request() {
    // Bite: the BuiltinLocal arm building an OpenAI-compatible engine (API or
    // local server: both URLs lead to the listener), the post-processing stage
    // called although it is off, any "is the model online" probe or download in
    // the job path. The positive control rules out a listener nobody could reach.
    let server = Server::start(Vec::new());
    let tmp = TempDir::new();
    let dir: PathBuf = tmp.path().join("models");
    place_models(&dir, &[ModelId::Tiny]);
    let clock = FakeMono::starting_at(now());
    let loader = FakeLoader::with(
        &clock,
        Script {
            text: "local words".to_string(),
            ..Script::default()
        },
    );
    let residency = Arc::new(ModelResidency::new(
        Arc::clone(&loader) as Arc<dyn SpeechModelLoader>,
        Arc::clone(&clock) as Arc<dyn MonotonicClock>,
        MODEL_IDLE_UNLOAD,
    ));
    let store = Arc::new(ModelStore::new(dir, local_catalog()));
    let clipboard = Arc::new(FakeClipboard::new());
    let deps = PipelineDeps {
        gate: SpeechGate::new(
            Ok(Box::new(EnergyDetector::new()) as Box<dyn SpeechDetector>),
            EnergyDetector::new(),
        ),
        credentials: Arc::new(
            FakeCredentialStore::new()
                .with_key(KeySlot::TranscriptionApi, KEY)
                .with_key(KeySlot::LocalServer, KEY)
                .with_key(KeySlot::PostProcessing, KEY),
        ),
        clipboard: clipboard.clone(),
        paster: Arc::new(FakePaster::new()),
        temp_audio: Arc::new(FakeTempAudioStore::new()),
        observer: Arc::new(RecordingObserver::new()),
        post_processor: Arc::new(ChatPostProcessor::new()),
    };
    let factory_residency = Arc::clone(&residency);
    let factory_store = Arc::clone(&store);
    let pipeline = Pipeline::with_timeouts(
        deps,
        Timeouts {
            api_transcription: Duration::from_secs(10),
            ..Timeouts::default()
        },
    )
    .with_engine_factory(Box::new(move |settings, creds| {
        let builtin = BuiltinDeps {
            residency: Arc::clone(&factory_residency),
            store: Arc::clone(&factory_store),
        };
        engine_for(settings, creds, Some(&builtin))
    }));

    for n in 0..DICTATIONS {
        let report = dictate(&pipeline, settings(EngineKind::BuiltinLocal, &server));
        assert!(
            matches!(report.end, JobEnd::Delivered { .. }),
            "dictation {n}: {report:?}"
        );
    }
    assert_eq!(
        clipboard.texts().len(),
        DICTATIONS,
        "every dictation delivered"
    );
    assert_eq!(loader.loads().len(), 1, "one load, then warm");
    assert_eq!(
        server.accepts(),
        0,
        "no connection from a built-in dictation"
    );

    // Positive control: the same pipeline with the API engine reaches the listener.
    let control = dictate(&pipeline, settings(EngineKind::Api, &server));
    assert!(
        matches!(control.end, JobEnd::Failed(_)),
        "the listener is no transcription API: {control:?}"
    );
    assert!(server.accepts() >= 1, "the listener counts connections");
}
