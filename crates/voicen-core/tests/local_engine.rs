//! T-098 (spec 002 T020, T022; FR-07, FR-11, FR-24, NFR-01, spec FR-009–FR-012,
//! FR-015, FR-019a as reworded by #133): the built-in engine in the core.
//! Acceptance 2 ("`engine_for` with `BuiltinLocal` builds `BuiltinEngine`; the
//! transcription runs through the one `ModelResidency`") and Acceptance 3 (no model
//! downloaded -> `NoLocalModel`, loader never called, audio kept pending; a fake
//! transcription over `timeouts.builtin` -> `Timeout`; a slow load alone -> no
//! timeout).
//!
//! Pinned API (analysis seam (4)-(6); `local_support` pins the traits and the clock
//! port, `local_residency` the residency):
//! - `voicen_core::engine::builtin::BuiltinDeps { pub residency: Arc<ModelResidency>,
//!   pub store: Arc<ModelStore> }`;
//! - `voicen_core::engine::engine_for(settings, creds, builtin: Option<&BuiltinDeps>)`:
//!   the one factory (P-011). `BuiltinLocal` with deps builds the engine of kind
//!   `"builtin"` and reads no key; `None` stays `EngineNotConfigured`. The model is
//!   resolved from the job's settings snapshot (`builtin_local.model_id`) through
//!   `ModelStore::path_if_downloaded`; none -> `NoLocalModel` (from the factory or
//!   from `transcribe`, either), nothing loaded;
//! - the engine reads its limit from `TranscribeRequest.timeouts.builtin` and the
//!   time from the residency's monotonic port; the limit covers the transcription
//!   only and is enforced through the model's `Abort` hook (#133). A transcription
//!   the hook stopped is `Timeout`; any other load or transcription error is
//!   `FailureReason::LocalEngineFailed`;
//! - `FailureReason::NoLocalModel` (code `NoLocalModel`, id `failure.no_local_model`)
//!   and `FailureReason::LocalEngineFailed` (code `LocalEngineFailed`, id
//!   `failure.local_engine`), both retryable (the audio is kept, FR-11);
//! - the dictation log line of a built-in job carries `local=cold load_ms=<n>` or
//!   `local=warm` (spec FR-015, `n` from the monotonic port), never the transcript.
//!
//! Fake data only: no network, documentation hosts, `sk-test-` keys.

mod common;
mod diag_support;
mod local_support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::timing::{ago, now};
use diag_support::{
    all_lines, at, contains, dictation_lines, files_under, open_log, utf16le, NOON_UTC,
};
use local_support::{
    file_name, local_catalog, place_models, wrong_magic_bytes, FakeLoader, FakeMono, Script,
};
use voicen_core::audio::AudioBuffer;
use voicen_core::clock::MonotonicClock;
use voicen_core::diag::{LogConfig, LogObserver};
use voicen_core::engine::builtin::BuiltinDeps;
use voicen_core::engine::speech_model::SpeechModelLoader;
use voicen_core::engine::{engine_for, Engine, TranscribeRequest};
use voicen_core::events::{DeviceKind, DictationEvent, PipelineObserver, RecordingObserver};
use voicen_core::failure::{FailureReason, TimeoutSource};
use voicen_core::i18n::{self, UiLanguage, MESSAGE_IDS};
use voicen_core::local_models::catalog::ModelId;
use voicen_core::local_models::residency::ModelResidency;
use voicen_core::local_models::store::ModelStore;
use voicen_core::overlay::{overlay_payload, OverlayView};
use voicen_core::pipeline::{JobReport, Pipeline, PipelineDeps, PressContext};
use voicen_core::platform::{
    FakeClipboard, FakePaster, FakeTempAudioStore, StartWindow, StoreCall, WindowRef,
};
use voicen_core::post_process::PassThrough;
use voicen_core::recording::{
    JobEnd, OverlayState, Press, RecordingController, RecordingEnd, Release,
};
use voicen_core::secrets::{FakeCredentialStore, KeySlot};
use voicen_core::settings::{defaults, EngineKind, Settings};
use voicen_core::test_support::{fixtures, TempDir};
use voicen_core::timeouts::{Timeouts, MODEL_IDLE_UNLOAD};
use voicen_core::vad::{EnergyDetector, SpeechDetector, SpeechGate};

const OS: Option<&str> = Some("en-US");
const KEY: &str = "sk-test-SECRET";
/// A transcript canary: must reach the clipboard and never the log.
const TRANSCRIPT: &str = "TRANSCRIPT-MARKER local words";
/// A built-in limit other than the 120 s default, so a hard-coded constant or
/// another endpoint's limit gives another outcome.
const LIMIT: Duration = Duration::from_secs(45);
const STEP: Duration = Duration::from_secs(1);

// ---- rig -----------------------------------------------------------------------------

struct Rig {
    _tmp: TempDir,
    dir: PathBuf,
    clock: Arc<FakeMono>,
    loader: Arc<FakeLoader>,
    residency: Arc<ModelResidency>,
    store: Arc<ModelStore>,
    creds: FakeCredentialStore,
}

/// A models dir holding `on_disk`, the store over it, the fake loader and the one
/// residency on the fake clock.
fn rig(on_disk: &[ModelId], script: Script) -> Rig {
    let tmp = TempDir::new();
    let dir = tmp.path().join("models");
    place_models(&dir, on_disk);
    let clock = FakeMono::starting_at(now());
    let loader = FakeLoader::with(&clock, script);
    let residency = Arc::new(ModelResidency::new(
        Arc::clone(&loader) as Arc<dyn SpeechModelLoader>,
        Arc::clone(&clock) as Arc<dyn MonotonicClock>,
        MODEL_IDLE_UNLOAD,
    ));
    let store = Arc::new(ModelStore::new(dir.clone(), local_catalog()));
    let creds = FakeCredentialStore::new()
        .with_key(KeySlot::TranscriptionApi, KEY)
        .with_key(KeySlot::LocalServer, KEY);
    Rig {
        _tmp: tmp,
        dir,
        clock,
        loader,
        residency,
        store,
        creds,
    }
}

fn builtin(model_id: Option<&str>) -> Settings {
    let mut s = defaults(OS);
    s.engine = EngineKind::BuiltinLocal;
    s.builtin_local.model_id = model_id.map(str::to_string);
    // Valid API and local-server sections: an arm that builds from them would work.
    s.api.base_url = "https://api.example.com/v1".to_string();
    s.local_server.base_url = "http://localhost:8000/v1".to_string();
    s.speech_language = None;
    s.auto_paste = true;
    s
}

fn request(language: Option<&str>) -> TranscribeRequest {
    TranscribeRequest {
        language: language.map(str::to_string),
        timeouts: Timeouts {
            builtin: LIMIT,
            ..Timeouts::default()
        },
    }
}

impl Rig {
    fn deps(&self) -> BuiltinDeps {
        BuiltinDeps {
            residency: Arc::clone(&self.residency),
            store: Arc::clone(&self.store),
        }
    }

    fn engine(&self, settings: &Settings) -> Box<dyn Engine> {
        match engine_for(settings, &self.creds, Some(&self.deps())) {
            Ok(engine) => engine,
            Err(e) => panic!("built-in engine expected, got {e:?}"),
        }
    }

    /// The whole job's engine part: build from the snapshot, then transcribe.
    fn job(&self, settings: &Settings, audio: &AudioBuffer) -> Result<String, FailureReason> {
        engine_for(settings, &self.creds, Some(&self.deps()))
            .and_then(|engine| engine.transcribe(audio, &request(None)))
    }

    fn path(&self, id: ModelId) -> PathBuf {
        self.dir.join(file_name(id))
    }

    /// The pipeline as the app runs it, with the one factory over this rig.
    fn pipeline(
        &self,
        observer: Arc<dyn PipelineObserver>,
        clipboard: Arc<FakeClipboard>,
        temp: Arc<FakeTempAudioStore>,
        paster: Arc<FakePaster>,
    ) -> Pipeline {
        let deps = PipelineDeps {
            gate: SpeechGate::new(
                Ok(Box::new(EnergyDetector::new()) as Box<dyn SpeechDetector>),
                EnergyDetector::new(),
            ),
            credentials: Arc::new(
                FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, KEY),
            ),
            clipboard,
            paster,
            temp_audio: temp,
            observer,
            post_processor: Arc::new(PassThrough),
        };
        let residency = Arc::clone(&self.residency);
        let store = Arc::clone(&self.store);
        Pipeline::with_timeouts(
            deps,
            Timeouts {
                builtin: LIMIT,
                ..Timeouts::default()
            },
        )
        .with_engine_factory(Box::new(move |settings, creds| {
            let builtin = BuiltinDeps {
                residency: Arc::clone(&residency),
                store: Arc::clone(&store),
            };
            engine_for(settings, creds, Some(&builtin))
        }))
    }
}

/// One dictation as the session drives it (press 4 s ago, 3 s hold, 3 s of speech),
/// with `RecordingStarted` / `RecordingEnded` on `observer`, `run_job` on a plain
/// std thread.
fn dictate(pipeline: &Pipeline, observer: &dyn PipelineObserver, settings: Settings) -> JobReport {
    let mut ctrl = RecordingController::<PressContext>::new();
    dictate_on(&mut ctrl, pipeline, observer, settings)
}

/// `dictate` on the caller's controller, so the test can read what it shows after
/// `job_finished`.
fn dictate_on(
    ctrl: &mut RecordingController<PressContext>,
    pipeline: &Pipeline,
    observer: &dyn PipelineObserver,
    settings: Settings,
) -> JobReport {
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
        hotkey_to_first_frame_ms: 37,
        device: DeviceKind::Selected,
    });
    let stop = t0 + Duration::from_secs(3);
    let ticket = match ctrl.release(stop) {
        Release::Stop(ticket) => ticket,
        other => panic!("expected a stop ticket, got {other:?}"),
    };
    observer.event(&DictationEvent::RecordingEnded {
        recording: id,
        duration_ms: 3_000,
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
    report
}

// ---- the factory ---------------------------------------------------------------------

#[test]
fn engine_for_builtin_local_builds_the_builtin_engine_and_reads_no_key() {
    // Acceptance 2, first half: the one factory maps BuiltinLocal to the built-in
    // engine (kind "builtin", the diag EngineTag), with no credential read and no
    // load at build time. Bite: the arm left at EngineNotConfigured, an API or
    // local-server engine built instead, a key read, a load when the engine is
    // built (a job that ends early would still have loaded the model).
    let r = rig(&[ModelId::Tiny], Script::default());
    let engine = r.engine(&builtin(Some("tiny")));
    assert_eq!(engine.kind(), "builtin");
    assert_eq!(r.creds.calls(), vec![], "no credential call");
    assert_eq!(r.loader.loads(), Vec::<PathBuf>::new(), "no load at build");
}

#[test]
fn the_transcription_runs_through_the_one_residency() {
    // Acceptance 2, second half: the text comes from the model the shared
    // residency loaded from the store's path of the snapshot's model; an engine
    // built for the next job finds it warm (one load). Bite: an engine with its
    // own residency or a direct loader call (the shared residency stays empty, a
    // second load), the wrong path (another model, a path not from the store).
    let r = rig(
        &[ModelId::Tiny, ModelId::Base],
        Script {
            text: TRANSCRIPT.to_string(),
            ..Script::default()
        },
    );
    let s = builtin(Some("base"));
    assert_eq!(
        r.job(&s, &fixtures::speech_3s()),
        Ok(TRANSCRIPT.to_string())
    );
    assert_eq!(r.residency.loaded(), Some(ModelId::Base));
    assert_eq!(r.loader.loads(), vec![r.path(ModelId::Base)]);

    assert_eq!(
        r.job(&s, &fixtures::speech_3s()),
        Ok(TRANSCRIPT.to_string())
    );
    assert_eq!(
        r.loader.loads().len(),
        1,
        "the next job reuses the loaded model"
    );
}

#[test]
fn the_speech_language_is_passed_to_the_model() {
    // Spec FR-009 / T020: `None` (auto-detect) reaches the model as None, a fixed
    // language as its code. Bite: the language dropped, a default "en" sent.
    let r = rig(&[ModelId::Tiny], Script::default());
    let engine = r.engine(&builtin(Some("tiny")));
    let audio = fixtures::speech_3s();
    engine.transcribe(&audio, &request(None)).expect("auto");
    assert_eq!(r.loader.last_transcription().language, None);
    engine.transcribe(&audio, &request(Some("ru"))).expect("ru");
    assert_eq!(
        r.loader.last_transcription().language,
        Some("ru".to_string())
    );
}

#[test]
fn the_audio_reaches_the_model_as_f32_scaled_by_32768() {
    // T025 "i16 -> f32 conversion": 16 kHz mono i16 becomes f32 in [-1, 1], one
    // sample per sample, s / 32768 (whisper.cpp's input). Bite: unscaled samples,
    // a /32767 scale, a resampled or truncated buffer.
    let r = rig(&[ModelId::Tiny], Script::default());
    let engine = r.engine(&builtin(Some("tiny")));
    let audio = AudioBuffer::from_16k_mono(vec![i16::MIN, -16_384, 0, 16_384, i16::MAX]);
    engine
        .transcribe(&audio, &request(None))
        .expect("transcribe");
    let pcm = r.loader.last_transcription().pcm;
    assert_eq!(pcm.len(), 5);
    assert_eq!(&pcm[..4], &[-1.0, -0.5, 0.0, 0.5]);
    assert!(pcm[4] > 0.999 && pcm[4] < 1.0, "{pcm:?}");
}

// ---- failure branches: no model, errors -----------------------------------------------

#[test]
fn no_model_selected_or_not_downloaded_is_no_local_model_and_nothing_is_loaded() {
    // Acceptance 3 / spec FR-011: no model selected, a selected model whose file is
    // missing, an id outside the catalog, and a file of the wrong size (a partial
    // copy) all give NoLocalModel and the loader is never called. Bite: a load
    // attempted on a missing path (LocalEngineFailed), EngineNotConfigured,
    // `small` loaded because some model is on disk, the size check skipped.
    let r = rig(&[ModelId::Tiny], Script::default());
    std::fs::write(r.path(ModelId::Base), b"too short").expect("write a partial base");
    for model in [None, Some("small"), Some("not-a-model"), Some("base")] {
        let got = r.job(&builtin(model), &fixtures::speech_3s());
        assert_eq!(got, Err(FailureReason::NoLocalModel), "{model:?}");
    }
    assert_eq!(
        r.loader.loads(),
        Vec::<PathBuf>::new(),
        "the loader is never called"
    );
    assert_eq!(r.residency.loaded(), None);
}

#[test]
fn a_load_error_is_local_engine_failed_and_the_next_job_loads_again() {
    // T020 "load error -> engine failure through the FR-11 path, app continues".
    // Bite: Timeout or NoLocalModel for a load error, a panic, a cached failure.
    let r = rig(
        &[ModelId::Tiny],
        Script {
            load_fails: true,
            ..Script::default()
        },
    );
    let s = builtin(Some("tiny"));
    assert_eq!(
        r.job(&s, &fixtures::speech_3s()),
        Err(FailureReason::LocalEngineFailed)
    );
    r.loader.script(|s| s.load_fails = false);
    assert!(r.job(&s, &fixtures::speech_3s()).is_ok());
    assert_eq!(r.loader.loads().len(), 2);
}

#[test]
fn a_transcription_error_is_local_engine_failed_not_timeout() {
    // An error well inside the limit is the engine failure, not Timeout. Bite:
    // every model error mapped to Timeout.
    let r = rig(
        &[ModelId::Tiny],
        Script {
            transcribe_fails: true,
            ..Script::default()
        },
    );
    assert_eq!(
        r.job(&builtin(Some("tiny")), &fixtures::speech_3s()),
        Err(FailureReason::LocalEngineFailed)
    );
}

// ---- the built-in limit (FR-24, #133) -----------------------------------------------

#[test]
fn a_transcription_over_timeouts_builtin_is_timeout_through_the_abort_hook() {
    // Acceptance 3: a transcription that would take 10x `timeouts.builtin` (45 s
    // here) is stopped by the model's abort hook as soon as the limit has passed on
    // the injected clock, and the job is Timeout. Bite: no hook (the fake runs all
    // 450 s, Ok), a hook that never fires, a post-hoc check after the model
    // returned (ran 450 s), the 120 s default or another endpoint's limit instead of
    // `req.timeouts.builtin` (stops at 120 s or 60 s), an abort before the limit,
    // the aborted error passed on as LocalEngineFailed.
    let r = rig(
        &[ModelId::Tiny],
        Script {
            transcribe_takes: LIMIT * 10,
            step: STEP,
            ..Script::default()
        },
    );
    let got = r.job(&builtin(Some("tiny")), &fixtures::speech_3s());
    assert_eq!(
        got,
        Err(FailureReason::Timeout {
            origin: TimeoutSource::Builtin
        })
    );
    let call = r.loader.last_transcription();
    assert!(
        call.aborted,
        "the model was stopped through its hook: {call:?}"
    );
    assert!(
        call.ran_for >= LIMIT && call.ran_for <= LIMIT + STEP,
        "stopped at the first poll past {LIMIT:?} of fake time, ran {:?}",
        call.ran_for
    );
}

#[test]
fn a_transcription_just_inside_timeouts_builtin_is_ok() {
    // The limit is not shorter than `timeouts.builtin`: 44 s of a 45 s limit ends
    // with the text. Bite: a shorter limit (connect, 5 s; or the load counted in).
    let r = rig(
        &[ModelId::Tiny],
        Script {
            transcribe_takes: LIMIT - STEP,
            step: STEP,
            text: TRANSCRIPT.to_string(),
            ..Script::default()
        },
    );
    let got = r.job(&builtin(Some("tiny")), &fixtures::speech_3s());
    assert_eq!(got, Ok(TRANSCRIPT.to_string()));
    assert!(!r.loader.last_transcription().aborted);
}

#[test]
fn a_slow_load_alone_never_times_out() {
    // Acceptance 3 / #133 (OQ-31 (a)): the limit covers the transcription only. A
    // cold load of 10x the limit followed by a 1 s transcription is Ok. Bite: the
    // deadline taken before the acquire (the hook fires at once: Timeout), the load
    // run under the same budget.
    let r = rig(
        &[ModelId::Tiny],
        Script {
            load_takes: LIMIT * 10,
            transcribe_takes: STEP,
            step: STEP,
            text: TRANSCRIPT.to_string(),
            ..Script::default()
        },
    );
    let got = r.job(&builtin(Some("tiny")), &fixtures::speech_3s());
    assert_eq!(got, Ok(TRANSCRIPT.to_string()));
    assert!(!r.loader.last_transcription().aborted);
}

// ---- T-106: the built-in timeout's own text (OQ-36, decision #147 (a)) -------------

/// contracts/messages.md `failure.local_timeout` (decision #147), spelled here, not
/// derived from the code under test.
const LOCAL_TIMEOUT_EN: &str = "Transcription took too long";
const LOCAL_TIMEOUT_RU: &str = "Распознавание заняло слишком много времени";
/// contracts/messages.md `failure.timeout`: the server's text, never shown for a
/// built-in timeout.
const SERVER_TIMEOUT_EN: &str = "The server did not answer in time";
const SERVER_TIMEOUT_RU: &str = "Сервер не ответил вовремя";

#[test]
fn a_builtin_timeout_renders_failure_local_timeout_with_code_timeout() {
    // T-106 Acceptance 1 at the reason: the abort of a transcription over
    // `timeouts.builtin` gives a reason whose code stays "Timeout" (what
    // JobFinished and the log carry, spec 002 FR-019a), whose message id is the
    // declared `failure.local_timeout` with no params, and which renders the
    // decision-#147 text in en and ru. Still retryable (FR-11). Bite:
    // `FailureReason::message_id` still mapping every Timeout to
    // `failure.timeout` (the server text), the id declared but missing from a
    // catalog (text() falls back to the id), a new code for the built-in timeout.
    let r = rig(
        &[ModelId::Tiny],
        Script {
            transcribe_takes: LIMIT * 10,
            step: STEP,
            ..Script::default()
        },
    );
    let reason = r
        .job(&builtin(Some("tiny")), &fixtures::speech_3s())
        .expect_err("a transcription over the limit fails");
    assert!(r.loader.last_transcription().aborted, "stopped by the hook");
    assert_eq!(reason.code(), "Timeout", "the log reason stays Timeout");
    assert!(reason.retryable(), "the audio is kept");
    assert_eq!(
        serde_json::to_value(reason.message_id()).expect("id serializes"),
        serde_json::Value::String("failure.local_timeout".to_string())
    );
    assert!(MESSAGE_IDS.contains(&reason.message_id()), "declared");
    assert_eq!(reason.message_params(), Vec::new(), "no params");
    assert_eq!(
        i18n::text(UiLanguage::En, reason.message_id(), &[]),
        LOCAL_TIMEOUT_EN
    );
    assert_eq!(
        i18n::text(UiLanguage::Ru, reason.message_id(), &[]),
        LOCAL_TIMEOUT_RU
    );
}

#[test]
fn a_builtin_timeout_dictation_shows_transcription_took_too_long_and_logs_failure_timeout() {
    // T-106 Acceptance 1 through the app's path: a built-in dictation whose
    // transcription runs past `timeouts.builtin` ends Failed with code "Timeout",
    // keeps the audio pending, and the controller's overlay message (rendered by
    // `overlay_payload`, the text the overlay window shows) is "Transcription took
    // too long" / "Распознавание заняло слишком много времени", never the server
    // text. The dictation line still says `engine=builtin outcome=failed
    // failure=timeout` (the log reason stays Timeout). Bite: the old
    // `Timeout => FAILURE_TIMEOUT` mapping (server text on the overlay), a new code
    // for the built-in timeout (`failure=` changes or JobFinished.failure differs),
    // the audio dropped.
    let r = rig(
        &[ModelId::Tiny],
        Script {
            transcribe_takes: LIMIT * 10,
            step: STEP,
            ..Script::default()
        },
    );
    let tmp = TempDir::new();
    let logs = tmp.path().join("logs");
    let (log, _clock, seen) = open_log(&logs, at(NOON_UTC, 0), 0, LogConfig::default());
    let observer = Arc::new(LogObserver::new(Arc::clone(&log)));
    let clipboard = Arc::new(FakeClipboard::new());
    let temp = Arc::new(FakeTempAudioStore::new());
    let paster = Arc::new(FakePaster::new());
    let pipeline = r.pipeline(
        observer.clone(),
        clipboard.clone(),
        temp.clone(),
        paster.clone(),
    );
    let mut ctrl = RecordingController::<PressContext>::new();

    let report = dictate_on(&mut ctrl, &pipeline, &*observer, builtin(Some("tiny")));

    let reason = match &report.end {
        JobEnd::Failed(reason) => reason.clone(),
        other => panic!("expected a failed job, got {other:?}"),
    };
    assert_eq!(reason.code(), "Timeout");
    assert!(r.loader.last_transcription().aborted, "stopped by the hook");
    let pending = report.pending.expect("the audio is kept pending");
    assert_eq!(temp.calls(), vec![StoreCall::Put(pending)]);
    assert_eq!(clipboard.texts(), Vec::<String>::new(), "nothing copied");
    assert_eq!(paster.calls(), vec![], "nothing pasted");

    let shown = ctrl.indicator().overlay.clone();
    assert!(
        matches!(shown, OverlayState::Message { .. }),
        "a message is shown: {shown:?}"
    );
    for (lang, want, server) in [
        (UiLanguage::En, LOCAL_TIMEOUT_EN, SERVER_TIMEOUT_EN),
        (UiLanguage::Ru, LOCAL_TIMEOUT_RU, SERVER_TIMEOUT_RU),
    ] {
        let view = overlay_payload(&shown, lang, 1, Duration::ZERO).state;
        assert_ne!(
            view,
            OverlayView::Message {
                text: server.to_string()
            },
            "{lang:?}: the server text for a built-in timeout"
        );
        assert_eq!(
            view,
            OverlayView::Message {
                text: want.to_string()
            },
            "{lang:?}"
        );
    }

    let lines = all_lines(&logs);
    let dictations = dictation_lines(&lines);
    assert_eq!(dictations.len(), 1, "{lines:#?}");
    let key = |k: &str| dictations[0].get(k).map(str::to_string);
    assert_eq!(key("engine").as_deref(), Some("builtin"), "{lines:#?}");
    assert_eq!(key("outcome").as_deref(), Some("failed"), "{lines:#?}");
    assert_eq!(key("failure").as_deref(), Some("timeout"), "{lines:#?}");
    for file in files_under(&logs) {
        let bytes = std::fs::read(&file).expect("read");
        for text in [LOCAL_TIMEOUT_EN, LOCAL_TIMEOUT_RU] {
            assert!(
                !contains(&bytes, text.as_bytes()) && !contains(&bytes, &utf16le(text)),
                "the message text, not the reason, in {}",
                file.display()
            );
        }
    }
    assert_eq!(seen.count(), 0, "the log stayed writable");
}

/// Lets a spawned job reach the model's lock. Too short only weakens
/// `a_job_waiting_for_the_model_gets_its_whole_limit_from_when_it_holds_it`; it
/// never turns it red.
const SETTLE: Duration = Duration::from_millis(300);

#[test]
fn a_job_waiting_for_the_model_gets_its_whole_limit_from_when_it_holds_it() {
    // T-098 review 1 #1 / #133 ("the limit covers transcription only"): two jobs
    // run at once on the one warm model (`Pipeline::process` may run concurrently,
    // T-011). Job 1 holds the model for 2x the limit of fake time and is aborted
    // (Timeout). Job 2 waited for the model all that time; its 45 s budget starts
    // when it holds the model, so its 2 s transcription ends Ok, not aborted.
    // Bite: the abort start taken after `acquire` but before the model's lock
    // (job 2 is Timeout at once, its wait counted), the start taken at submit.
    let r = rig(
        &[ModelId::Tiny],
        Script {
            transcribe_takes: STEP * 2,
            step: STEP,
            text: TRANSCRIPT.to_string(),
            ..Script::default()
        },
    );
    let s = builtin(Some("tiny"));
    assert_eq!(
        r.job(&s, &fixtures::speech_3s()),
        Ok(TRANSCRIPT.to_string()),
        "warm-up"
    );
    let first = r.engine(&s);
    let second = r.engine(&s);
    let local_support::Gate { entered, release } = r.loader.gate_transcriptions();
    let audio = fixtures::speech_3s();

    let (job1, job2) = std::thread::scope(|scope| {
        let job1 = scope.spawn(|| first.transcribe(&audio, &request(None)));
        entered
            .recv_timeout(Duration::from_secs(30))
            .expect("job 1 holds the model");
        let job2 = scope.spawn(|| second.transcribe(&audio, &request(None)));
        std::thread::sleep(SETTLE);
        // Job 1 runs 2x the limit while job 2 waits for the model.
        r.clock.advance(LIMIT * 2);
        // One release for job 1; the dropped sender lets job 2 through.
        let _ = release.send(());
        drop(release);
        (
            job1.join().expect("job 1 must not panic"),
            job2.join().expect("job 2 must not panic"),
        )
    });

    assert_eq!(
        job1,
        Err(FailureReason::Timeout {
            origin: TimeoutSource::Builtin
        }),
        "job 1 ran past the limit"
    );
    assert_eq!(
        job2,
        Ok(TRANSCRIPT.to_string()),
        "job 2's wait for the model was counted against its limit"
    );
    let calls = r.loader.transcriptions();
    let last = calls.last().expect("job 2 reached the model");
    assert!(!last.aborted, "job 2 was aborted: {last:?}");
    assert_eq!(last.ran_for, STEP * 2, "job 2 ran its whole transcription");
    assert_eq!(r.loader.loads().len(), 1, "one model, one load");
}

// ---- R-8: the file is checked before the load (review 1 #3) ---------------------------

#[test]
fn a_downloaded_file_without_the_ggml_magic_is_local_engine_failed_and_never_loaded() {
    // T-098 review 1 #3 / spec 002 T025 ("size and ggml-magic check before load",
    // research R-8, NFR-07): a file with the catalog size (so the store calls it
    // downloaded) whose header is not the ggml magic is refused before the loader
    // runs: the job is LocalEngineFailed (spec FR-012: a load that fails inside the
    // engine), retryable, nothing loaded. whisper.cpp may abort the process on such
    // a file. Bite: no magic check (the loader gets the file and the job is Ok),
    // the magic compared in the wrong byte order, NoLocalModel or Timeout instead.
    let r = rig(&[ModelId::Tiny], Script::default());
    std::fs::write(r.path(ModelId::Tiny), wrong_magic_bytes()).expect("corrupt tiny");
    assert!(
        r.store.path_if_downloaded(ModelId::Tiny).is_some(),
        "the store's size rule still calls it downloaded"
    );
    let got = r.job(&builtin(Some("tiny")), &fixtures::speech_3s());
    assert_eq!(got, Err(FailureReason::LocalEngineFailed));
    assert_eq!(r.loader.loads(), Vec::<PathBuf>::new(), "never loaded");
    assert_eq!(r.residency.loaded(), None);
}

// ---- failure reasons -------------------------------------------------------------------

#[test]
fn the_local_failure_reasons_are_retryable_with_their_own_codes_and_messages() {
    // FR-11: both keep the audio as the pending recording. Each has its own code
    // (what JobFinished and the log carry), its own declared `failure.*` id, and a
    // text in en and ru (FR-15, spec FR-025). Bite: not retryable, a shared or
    // reused code / id (`failure.timeout`, `failure.engine_not_configured`), an id
    // missing from a catalog (text() falls back to the id).
    let rows = [
        (
            FailureReason::NoLocalModel,
            "NoLocalModel",
            "failure.no_local_model",
        ),
        (
            FailureReason::LocalEngineFailed,
            "LocalEngineFailed",
            "failure.local_engine",
        ),
    ];
    for (reason, code, id) in rows {
        assert!(reason.retryable(), "{reason:?} keeps the audio");
        assert_eq!(reason.code(), code);
        assert_eq!(
            serde_json::to_value(reason.message_id()).expect("id serializes"),
            serde_json::Value::String(id.to_string())
        );
        assert!(MESSAGE_IDS.contains(&reason.message_id()), "{id} declared");
        assert_eq!(reason.message_params(), Vec::new(), "{id} has no params");
        for lang in [UiLanguage::En, UiLanguage::Ru] {
            let text = i18n::text(lang, reason.message_id(), &[]);
            assert!(
                !text.is_empty() && text != id,
                "{id} {lang:?} has a catalog text: {text:?}"
            );
        }
    }
    let en = |r: FailureReason| i18n::text(UiLanguage::En, r.message_id(), &[]);
    assert_ne!(
        en(FailureReason::NoLocalModel),
        en(FailureReason::EngineNotConfigured)
    );
}

// ---- through the pipeline ----------------------------------------------------------

#[test]
fn no_local_model_pastes_nothing_and_keeps_the_audio_pending() {
    // Acceptance 3 / spec FR-011 through the real pipeline: nothing on the
    // clipboard, no Ctrl+V, the job ends Failed(NoLocalModel) with the audio stored
    // as the one pending recording, and the loader was never called. Bite: a
    // non-retryable reason (no pending), a load attempt, a paste of anything.
    let r = rig(&[], Script::default());
    let clipboard = Arc::new(FakeClipboard::new());
    let temp = Arc::new(FakeTempAudioStore::new());
    let paster = Arc::new(FakePaster::new());
    let observer = Arc::new(RecordingObserver::new());
    let pipeline = r.pipeline(
        observer.clone(),
        clipboard.clone(),
        temp.clone(),
        paster.clone(),
    );

    let report = dictate(&pipeline, &*observer, builtin(Some("small")));

    assert_eq!(report.end, JobEnd::Failed(FailureReason::NoLocalModel));
    let pending = report.pending.expect("the audio is kept pending");
    assert_eq!(temp.calls(), vec![StoreCall::Put(pending)]);
    assert_eq!(
        pipeline.pending().map(|(_, reason)| reason),
        Some(FailureReason::NoLocalModel)
    );
    assert_eq!(clipboard.texts(), Vec::<String>::new(), "nothing copied");
    assert_eq!(paster.calls(), vec![], "nothing pasted");
    assert_eq!(r.loader.loads(), Vec::<PathBuf>::new(), "no load attempted");
}

#[test]
fn the_dictation_log_marks_cold_with_load_ms_then_warm_and_holds_no_transcript() {
    // T022 / spec FR-015 / NFR-01 / FR-20: the first built-in dictation's line says
    // `local=cold load_ms=1234` (the load's duration on the injected clock), the
    // second `local=warm` with no load_ms; a no-model job logs its failure and no
    // `local`. No byte of the transcript reaches any log file. Bite: warmth not
    // carried to the line, always cold/warm, load_ms from the wall clock, load_ms
    // on a warm line, the transcript or the model path written.
    let r = rig(
        &[ModelId::Tiny],
        Script {
            load_takes: Duration::from_millis(1_234),
            text: TRANSCRIPT.to_string(),
            ..Script::default()
        },
    );
    let tmp = TempDir::new();
    let logs = tmp.path().join("logs");
    let (log, _clock, seen) = open_log(&logs, at(NOON_UTC, 0), 0, LogConfig::default());
    let observer = Arc::new(LogObserver::new(Arc::clone(&log)));
    let clipboard = Arc::new(FakeClipboard::new());
    let pipeline = r.pipeline(
        observer.clone(),
        clipboard.clone(),
        Arc::new(FakeTempAudioStore::new()),
        Arc::new(FakePaster::new()),
    );

    let first = dictate(&pipeline, &*observer, builtin(Some("tiny")));
    let second = dictate(&pipeline, &*observer, builtin(Some("tiny")));
    let missing = dictate(&pipeline, &*observer, builtin(Some("small")));
    assert!(matches!(first.end, JobEnd::Delivered { .. }), "{first:?}");
    assert!(matches!(second.end, JobEnd::Delivered { .. }), "{second:?}");
    assert_eq!(missing.end, JobEnd::Failed(FailureReason::NoLocalModel));
    assert_eq!(
        clipboard.texts(),
        vec![TRANSCRIPT.to_string(), TRANSCRIPT.to_string()],
        "positive control: the transcript was delivered"
    );

    let lines = all_lines(&logs);
    let dictations = dictation_lines(&lines);
    assert_eq!(dictations.len(), 3, "{lines:#?}");
    let keys = |i: usize, key: &str| dictations[i].get(key).map(str::to_string);
    // The no-model job may fail in the factory (no engine built, no engine=) or
    // in transcribe (engine=builtin); either way it carries no `local`.
    for i in 0..2 {
        assert_eq!(keys(i, "engine").as_deref(), Some("builtin"), "{lines:#?}");
    }
    assert_eq!(keys(0, "outcome").as_deref(), Some("delivered"));
    assert_eq!(keys(0, "local").as_deref(), Some("cold"), "{lines:#?}");
    assert_eq!(keys(0, "load_ms").as_deref(), Some("1234"), "{lines:#?}");
    assert_eq!(keys(1, "local").as_deref(), Some("warm"), "{lines:#?}");
    assert_eq!(keys(1, "load_ms"), None, "no load_ms on a warm line");
    assert_eq!(keys(2, "outcome").as_deref(), Some("failed"));
    assert_eq!(
        keys(2, "failure").as_deref(),
        Some("no_local_model"),
        "{lines:#?}"
    );
    assert_eq!(keys(2, "local"), None, "no model was used: {lines:#?}");

    let files = files_under(&logs);
    assert!(!files.is_empty(), "the log was written");
    let model_path = r.path(ModelId::Tiny).display().to_string();
    for file in files {
        let bytes = std::fs::read(&file).expect("read");
        for secret in ["TRANSCRIPT-MARKER", "local words", model_path.as_str()] {
            assert!(
                !contains(&bytes, secret.as_bytes()) && !contains(&bytes, &utf16le(secret)),
                "{secret} in {}",
                file.display()
            );
        }
    }
    assert_eq!(seen.count(), 0, "the log stayed writable");
}
