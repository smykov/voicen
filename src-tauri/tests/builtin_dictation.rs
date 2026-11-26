//! T-017 (spec 002 US2 and T035; FR-07, NFR-01; decision #311): the built-in engine
//! through the shell's own wiring, offline, with the cached `tiny` model. Windows CI
//! only (decision #5): whisper.cpp builds and runs there, and the model dir comes from
//! the T-099 fetch step (`VOICEN_WHISPER_TEST_MODELS_DIR`, a `ModelStore` directory; a
//! missing variable or model is red, never a skip). No network is touched: no port is
//! opened, the model is copied, the audio is core's CC0 clip through a fake source.
//!
//! Pinned shell API (analysis seam (5) of docs/tasks/T-017.md; it does not exist yet):
//! - `voicen_lib::local_models::builtin_deps(store: Arc<ModelStore>) -> BuiltinDeps`:
//!   the one constructor `run()` uses: one `ModelResidency` over `WhisperLoader` and
//!   `SystemMonotonic` (idle = `MODEL_IDLE_UNLOAD`) and `store`;
//! - core's `SessionDeps::builtin` and `LocalModels::{with_residency, delete_resident}`
//!   (pinned by `crates/voicen-core/tests/builtin_session.rs`).
//!
//! The clock is read only through `timing_support`; no transcript text is asserted or
//! printed (FR-20): the pasted text is checked for being non-empty only.

mod timing_support;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use timing_support::{eventually, now};
use voicen_core::audio::AudioBuffer;
use voicen_core::autostart::FakeAutostart;
use voicen_core::clock::FakeClock;
use voicen_core::dictation::{DictationSession, SessionDeps};
use voicen_core::events::{DictationEvent, OutcomeCode, RecordingObserver};
use voicen_core::hotkey_registrar::FakeHotkeyRegistrar;
use voicen_core::local_models::catalog::{ModelId, MODELS};
use voicen_core::local_models::service::{DeleteOutcome, LocalModels};
use voicen_core::local_models::store::ModelStore;
use voicen_core::models::DownloadedModels;
use voicen_core::platform::{
    FakeAudioSource, FakeCancelKey, FakeClipboard, FakeIndicator, FakeNotifier, FakePaster,
    FakeShellRequests, FakeTempAudioStore, FrameChunk, PasterCall, StartWindow, WindowRef,
};
use voicen_core::post_process::PassThrough;
use voicen_core::secrets::{FakeCredentialStore, KeySlot};
use voicen_core::settings::file::FakeSettingsFile;
use voicen_core::settings::service::{SettingsDeps, SettingsService};
use voicen_core::settings::{defaults, EngineKind};
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_core::vad::{EnergyDetector, SpeechDetector, SpeechGate};
use voicen_lib::local_models::builtin_deps;

const MODELS_DIR_VAR: &str = "VOICEN_WHISPER_TEST_MODELS_DIR";
const OS: Option<&str> = Some("en-US");
const WINDOW: WindowRef = WindowRef(0x0001_0017);
/// About 1 s of 16 kHz mono s16le English speech, CC0 (licenses/manual.json).
const CLIP_S16LE: &[u8] =
    include_bytes!("../../crates/voicen-core/assets/connection-test-speech.s16le");

fn clip() -> AudioBuffer {
    AudioBuffer::from_16k_mono(
        CLIP_S16LE
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| i16::from_le_bytes(b))
            .collect(),
    )
}

/// The CI-cached tiny model file.
fn cached_tiny() -> PathBuf {
    let dir = std::env::var_os(MODELS_DIR_VAR)
        .unwrap_or_else(|| panic!("{MODELS_DIR_VAR} is not set: the CI fetch step must run first"));
    ModelStore::new(PathBuf::from(dir), MODELS)
        .path_if_downloaded(ModelId::Tiny)
        .unwrap_or_else(|| panic!("the store in {MODELS_DIR_VAR} has no downloaded tiny model"))
}

/// What the shell builds in `run()`: the coordinator over a models dir that holds a copy
/// of the cached tiny, its store, and the one `BuiltinDeps` over that store.
struct App {
    _tmp: TempDir,
    models_dir: PathBuf,
    models: LocalModels,
    service: Arc<SettingsService>,
    deps: voicen_core::engine::builtin::BuiltinDeps,
}

fn app() -> App {
    let tmp = TempDir::new();
    let models_dir = tmp.path().join("models");
    fs::create_dir_all(&models_dir).expect("create the models dir");
    let target = models_dir.join(
        cached_tiny()
            .file_name()
            .expect("the cached model has a file name"),
    );
    fs::copy(cached_tiny(), &target).expect("copy the cached tiny");

    let (models, cleanup) = LocalModels::open(
        models_dir.clone(),
        FakeDisk::with_available(1 << 40),
        Timeouts::default(),
        MODELS,
    );
    cleanup.expect("cleanup at open");
    let store = models.store();
    assert!(store.is_downloaded("tiny"), "premise: the copy is tiny");
    let deps = builtin_deps(Arc::clone(&store));
    let models = models.with_residency(Arc::clone(&deps.residency));

    let mut settings = defaults(OS);
    settings.engine = EngineKind::BuiltinLocal;
    settings.builtin_local.model_id = Some("tiny".to_string());
    settings.speech_language = None;
    settings.auto_paste = true;
    let bytes = serde_json::to_vec(&settings).expect("serialize settings");
    let (service, _) = SettingsService::load_or_init(
        SettingsDeps {
            file: Arc::new(FakeSettingsFile::with_bytes(&bytes)),
            credentials: Arc::new(
                FakeCredentialStore::new()
                    .with_key(KeySlot::TranscriptionApi, "fake-key-for-tests"),
            ),
            autostart: Arc::new(FakeAutostart::new()),
            hotkeys: Arc::new(FakeHotkeyRegistrar::new()),
            local_models: store,
            clock: Arc::new(FakeClock::at(
                std::time::UNIX_EPOCH + Duration::from_secs(1_709_251_199),
            )),
            call_roots: Arc::new(voicen_core::settings::roots::FakeCallRoots::new()),
        },
        OS,
    );
    App {
        _tmp: tmp,
        models_dir,
        models,
        service: Arc::new(service),
        deps,
    }
}

struct Dictation {
    session: DictationSession,
    observer: Arc<RecordingObserver>,
    clipboard: Arc<FakeClipboard>,
    paster: Arc<FakePaster>,
}

/// A session with the shell's `BuiltinDeps` and no factory override, fake audio and paster.
fn dictation(app: &App) -> Dictation {
    let audio = Arc::new(FakeAudioSource::new());
    let clipboard = Arc::new(FakeClipboard::new());
    let paster = Arc::new(FakePaster::new().with_start_window(Some(StartWindow {
        handle: WINDOW,
        process_id: 4242,
        elevated: false,
    })));
    let observer = Arc::new(RecordingObserver::new());
    let creds = Arc::new(FakeCredentialStore::new());
    let session = DictationSession::start(SessionDeps {
        pipeline: voicen_core::pipeline::PipelineDeps {
            gate: SpeechGate::new(
                Ok(Box::new(EnergyDetector::new()) as Box<dyn SpeechDetector>),
                EnergyDetector::new(),
            ),
            credentials: creds,
            clipboard: clipboard.clone(),
            paster: paster.clone(),
            temp_audio: Arc::new(FakeTempAudioStore::new()),
            observer: observer.clone(),
            post_processor: Arc::new(PassThrough),
            transcript_sink: Arc::new(voicen_core::pipeline::DiscardTranscripts),
        },
        engine_factory: None,
        builtin: Some(app.deps.clone()),
        audio: audio.clone(),
        indicator: Arc::new(FakeIndicator::new()),
        requests: Arc::new(FakeShellRequests::new()),
        notifier: Arc::new(FakeNotifier::new()),
        settings: Arc::clone(&app.service),
        cancel_key: Arc::new(FakeCancelKey::new()),
    })
    .expect("the session starts");
    let t = now();
    audio.set_chunks(vec![FrameChunk::from_buffer(
        &clip(),
        t + Duration::from_millis(40),
    )]);
    session.hotkey_pressed(t);
    session.hotkey_released(t + Duration::from_millis(1500));
    Dictation {
        session,
        observer,
        clipboard,
        paster,
    }
}

fn wait_for_job(d: &Dictation) {
    assert!(
        eventually(|| d
            .observer
            .events()
            .iter()
            .any(|e| matches!(e, DictationEvent::JobFinished { .. }))),
        "no job ended: {:?}",
        d.observer.events()
    );
}

#[test]
fn the_shells_builtin_wiring_transcribes_the_clip_offline_and_pastes_non_empty_text() {
    // Acceptance 1 (Windows CI): `engine_factory: None` + the shell's `BuiltinDeps` over
    // the real WhisperLoader and the cached tiny. The job is the built-in engine's
    // (`engine=builtin`), delivered as text, the clipboard holds non-empty text and one
    // Ctrl+V went out. Bite: the factory passing no deps (EngineNotConfigured, no text),
    // a builder over a loader other than WhisperLoader, the prewarm skipped (still green,
    // so the load is cold: `local` is Some either way; see the unload test for the load).
    let app = app();
    let d = dictation(&app);
    wait_for_job(&d);

    match d
        .observer
        .events()
        .into_iter()
        .find(|e| matches!(e, DictationEvent::JobFinished { .. }))
    {
        Some(DictationEvent::JobFinished {
            engine: Some("builtin"),
            outcome: OutcomeCode::Text,
            failure: None,
            local: Some(_),
            ..
        }) => {}
        other => panic!("job {other:?}"),
    }
    let texts = d.clipboard.texts();
    assert_eq!(texts.len(), 1, "one text on the clipboard");
    assert!(
        texts.iter().all(|t| !t.trim().is_empty()),
        "the transcript is empty"
    );
    assert_eq!(
        d.paster
            .calls()
            .iter()
            .filter(|c| **c == PasterCall::SendCtrlV)
            .count(),
        1,
        "one Ctrl+V"
    );
    drop(d.session);
}

#[test]
fn deleting_the_model_after_a_real_load_unloads_it_first_and_removes_the_file() {
    // Spec T035 / FR-021: Windows refuses to delete a file a loaded whisper model still
    // maps. After a real load (the dictation above), the production delete releases the
    // model through the residency, then the file goes. Bite: `NoResidency` on the
    // delete (the removal fails with a sharing violation or leaves the model loaded),
    // a release after the removal.
    let app = app();
    let d = dictation(&app);
    wait_for_job(&d);
    drop(d.session);
    assert_eq!(
        app.deps.residency.loaded(),
        Some(ModelId::Tiny),
        "premise: the real model is in memory"
    );
    let listed: Vec<(ModelId, bool)> = app.models.list().iter().map(|v| (v.id, v.loaded)).collect();
    assert!(listed.contains(&(ModelId::Tiny, true)), "list: {listed:?}");

    let got = app.models.delete_resident("tiny", &app.service);

    assert_eq!(
        got,
        Ok(DeleteOutcome {
            engine_reset: true,
            reset_failed: false
        }),
        "the selected model was deleted: the engine resets"
    );
    assert_eq!(app.deps.residency.loaded(), None, "still loaded");
    let name = cached_tiny().file_name().expect("file name").to_owned();
    assert!(
        !app.models_dir.join(name).exists(),
        "the file is still there"
    );
}
