//! T-017 (spec 002 US2, FR-07, NFR-01, NFR-03, spec FR-009-FR-013, FR-015): the
//! built-in engine wired end to end in the core: the dictation session, the
//! residency's settings hook and the local-models coordinator. The T-098 parts
//! (`local_residency`, `local_engine`, `local_model_delete`) are proven there; these
//! tests prove the parts are CONNECTED.
//!
//! Pinned API (analysis seam of docs/tasks/T-017.md; none of it exists yet, so this
//! file does not compile until it does):
//! - `voicen_core::dictation::SessionDeps { builtin: Option<BuiltinDeps>, .. }`.
//!   `DictationSession::start` builds the pipeline's factory from
//!   `engine_for(s, c, builtin.as_ref())` when `engine_factory` is `None`
//!   (`engine_factory` stays a test override). In `hotkey_pressed`, when the press
//!   snapshot has `engine == BuiltinLocal`, the session holds
//!   `residency.recording_started()` for the recording's life and, when
//!   `store.path_if_downloaded(id)` is `Some`, calls `residency.prewarm(id, path)`;
//!   any other engine never touches the residency at the press;
//! - `ModelResidency::on_settings(&self, settings: &Settings)`: pure, the selected
//!   model is `builtin_local.model_id` parsed iff `engine == BuiltinLocal`, then
//!   `on_engine_or_model_changed(selected)`;
//! - `LocalModels::with_residency(self, Arc<ModelResidency>) -> LocalModels`:
//!   `list()` fills `loaded` from `residency.loaded()`, and
//!   `LocalModels::delete_resident(&self, id: &str, settings: &SettingsService)` is
//!   `delete` released through that residency (the production path; `delete` with an
//!   explicit port stays for the T-019 tests).
//!
//! Every clock here is the fake monotonic port of `local_support`: "10 minutes" and
//! "600 s of ticks during a recording" take no wall time. Fake data only.

mod common;
mod local_support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use common::os_answer::in_time;
use common::timing::{ago, eventually};
use local_support::{file_name, local_catalog, place_models, FakeLoader, FakeMono, Script};
use voicen_core::audio::AudioBuffer;
use voicen_core::autostart::FakeAutostart;
use voicen_core::clock::{FakeClock, MonotonicClock};
use voicen_core::dictation::{DictationSession, SessionDeps};
use voicen_core::engine::builtin::BuiltinDeps;
use voicen_core::engine::speech_model::SpeechModelLoader;
use voicen_core::engine::{Engine, TranscribeRequest};
use voicen_core::events::{DictationEvent, OutcomeCode, RecordingObserver};
use voicen_core::failure::FailureReason;
use voicen_core::hotkey_registrar::FakeHotkeyRegistrar;
use voicen_core::i18n::{self, UiLanguage};
use voicen_core::local_models::catalog::ModelId;
use voicen_core::local_models::residency::ModelResidency;
use voicen_core::local_models::service::{DeleteOutcome, LocalModels};
use voicen_core::local_models::store::{LocalModelState, ModelStore};
use voicen_core::models::FakeDownloadedModels;
use voicen_core::pipeline::{EngineFactory, PipelineDeps};
use voicen_core::platform::{
    FakeAudioSource, FakeCancelKey, FakeClipboard, FakeIndicator, FakeNotifier, FakePaster,
    FakeShellRequests, FakeTempAudioStore, FrameChunk, PasterCall, StartWindow, ToastAction,
    WindowRef,
};
use voicen_core::post_process::PassThrough;
use voicen_core::recording::OverlayState;
use voicen_core::secrets::{CredentialStore, FakeCredentialStore, KeyEdits, KeySlot};
use voicen_core::settings::file::FakeSettingsFile;
use voicen_core::settings::service::{SaveOutcome, SaveRequest, SettingsDeps, SettingsService};
use voicen_core::settings::{defaults, EngineKind, LoadOutcome, Settings};
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::{fixtures, TempDir};
use voicen_core::timeouts::MODEL_IDLE_UNLOAD;
use voicen_core::vad::{EnergyDetector, SpeechDetector, SpeechGate};

const OS: Option<&str> = Some("en-US");
const KEY: &str = "fake-key-for-tests";
const WINDOW: WindowRef = WindowRef(0x0001_0017);
const BUDGET: Duration = Duration::from_secs(10);
const FIRST_FRAME: Duration = Duration::from_millis(40);
/// What the fake built-in model transcribes to.
const LOCAL_TEXT: &str = "local words from the fake model";
/// What the fake API engine answers.
const API_TEXT: &str = "api words from the fake engine";

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn past() -> Instant {
    ago(Duration::from_secs(60))
}

fn builtin_settings(model_id: Option<&str>) -> Settings {
    let mut s = defaults(OS);
    s.engine = EngineKind::BuiltinLocal;
    s.builtin_local.model_id = model_id.map(str::to_string);
    s.api.base_url = "https://api.example.com/v1".to_string();
    s.speech_language = None;
    s.auto_paste = true;
    s
}

fn api_settings() -> Settings {
    let mut s = builtin_settings(Some("tiny"));
    s.engine = EngineKind::Api;
    s
}

fn energy_gate() -> SpeechGate {
    SpeechGate::new(
        Ok(Box::new(EnergyDetector::new()) as Box<dyn SpeechDetector>),
        EnergyDetector::new(),
    )
}

fn window() -> StartWindow {
    StartWindow {
        handle: WINDOW,
        process_id: 4242,
        elevated: false,
    }
}

/// A fake API engine for the "Api never touches the built-in engine" leg.
struct ApiFake;

impl Engine for ApiFake {
    fn kind(&self) -> &'static str {
        "fake"
    }
    fn transcribe(
        &self,
        _audio: &AudioBuffer,
        _req: &TranscribeRequest,
    ) -> Result<String, FailureReason> {
        Ok(API_TEXT.to_string())
    }
}

fn api_factory() -> Box<EngineFactory> {
    Box::new(|_: &Settings, _: &dyn CredentialStore| Ok(Box::new(ApiFake) as Box<dyn Engine>))
}

// ---- the rig ------------------------------------------------------------------------

struct Rig {
    _tmp: TempDir,
    session: DictationSession,
    audio: Arc<FakeAudioSource>,
    indicator: Arc<FakeIndicator>,
    clipboard: Arc<FakeClipboard>,
    paster: Arc<FakePaster>,
    temp: Arc<FakeTempAudioStore>,
    notifier: Arc<FakeNotifier>,
    observer: Arc<RecordingObserver>,
    settings: Arc<SettingsService>,
    clock: Arc<FakeMono>,
    loader: Arc<FakeLoader>,
    residency: Arc<ModelResidency>,
}

/// A session over fakes, with the one `BuiltinDeps` the shell builds: the fake loader
/// on a fake monotonic clock, the residency (`MODEL_IDLE_UNLOAD`), and a store over a
/// models dir holding `on_disk`. `engine_factory` is `None` unless a test overrides it.
fn rig(
    settings: &Settings,
    on_disk: &[ModelId],
    downloaded: &[&str],
    factory: Option<Box<EngineFactory>>,
) -> Rig {
    let tmp = TempDir::new();
    let dir = tmp.path().join("models");
    place_models(&dir, on_disk);
    let clock = FakeMono::starting_at(common::timing::now());
    let loader = FakeLoader::with(
        &clock,
        Script {
            text: LOCAL_TEXT.to_string(),
            ..Script::default()
        },
    );
    let residency = Arc::new(ModelResidency::new(
        Arc::clone(&loader) as Arc<dyn SpeechModelLoader>,
        Arc::clone(&clock) as Arc<dyn MonotonicClock>,
        MODEL_IDLE_UNLOAD,
    ));
    let store = Arc::new(ModelStore::new(dir, local_catalog()));

    let creds = Arc::new(FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, KEY));
    let bytes = serde_json::to_vec(settings).expect("serialize settings");
    let (service, outcome) = SettingsService::load_or_init(
        SettingsDeps {
            file: Arc::new(FakeSettingsFile::with_bytes(&bytes)),
            credentials: creds.clone(),
            autostart: Arc::new(FakeAutostart::new()),
            hotkeys: Arc::new(FakeHotkeyRegistrar::new()),
            local_models: Arc::new(FakeDownloadedModels::new(downloaded)),
            clock: Arc::new(FakeClock::at(
                UNIX_EPOCH + Duration::from_secs(1_709_251_199),
            )),
            call_roots: Arc::new(voicen_core::settings::roots::FakeCallRoots::new()),
        },
        OS,
    );
    assert!(
        matches!(&outcome, LoadOutcome::Loaded(s) if s == settings),
        "premise: the settings load as given: {outcome:?}"
    );
    let settings = Arc::new(service);

    let audio = Arc::new(FakeAudioSource::new());
    let indicator = Arc::new(FakeIndicator::new());
    let clipboard = Arc::new(FakeClipboard::new());
    let paster = Arc::new(FakePaster::new().with_start_window(Some(window())));
    let temp = Arc::new(FakeTempAudioStore::new());
    let notifier = Arc::new(FakeNotifier::new());
    let observer = Arc::new(RecordingObserver::new());
    let session = DictationSession::start(SessionDeps {
        pipeline: PipelineDeps {
            gate: energy_gate(),
            credentials: creds,
            clipboard: clipboard.clone(),
            paster: paster.clone(),
            temp_audio: temp.clone(),
            observer: observer.clone(),
            post_processor: Arc::new(PassThrough),
        },
        engine_factory: factory,
        builtin: Some(BuiltinDeps {
            residency: Arc::clone(&residency),
            store,
        }),
        audio: audio.clone(),
        indicator: indicator.clone(),
        requests: Arc::new(FakeShellRequests::new()),
        notifier: notifier.clone(),
        settings: settings.clone(),
        cancel_key: Arc::new(FakeCancelKey::new()),
    })
    .expect("the session starts");
    Rig {
        _tmp: tmp,
        session,
        audio,
        indicator,
        clipboard,
        paster,
        temp,
        notifier,
        observer,
        settings,
        clock,
        loader,
        residency,
    }
}

impl Rig {
    /// Presses at `t` with three seconds of speech queued as the capture.
    fn press(&self, t: Instant) {
        self.audio.set_chunks(vec![FrameChunk::from_buffer(
            &fixtures::speech_3s(),
            t + FIRST_FRAME,
        )]);
        self.session.hotkey_pressed(t);
    }

    fn release(&self, t: Instant) {
        self.session.hotkey_released(t);
    }

    fn jobs(&self) -> Vec<DictationEvent> {
        self.observer
            .events()
            .into_iter()
            .filter(|e| matches!(e, DictationEvent::JobFinished { .. }))
            .collect()
    }

    /// Waits until the first job has ended and the overlay has left Processing.
    fn wait_job(&self) {
        let done = eventually(BUDGET, || {
            !self.jobs().is_empty()
                && matches!(
                    self.indicator.overlays().last(),
                    Some(OverlayState::Hidden | OverlayState::Message { .. })
                )
        });
        assert!(
            done,
            "no job ended within {BUDGET:?}: events {:?}",
            self.observer.events()
        );
    }

    /// A hold of 3 s, ended and waited for.
    fn hold(&self) {
        let t = past();
        self.press(t);
        self.release(t + ms(3000));
        self.wait_job();
    }

    /// The absence check of "nothing loaded": a stall can only make it pass, so the
    /// positive control of each test (a load that does happen elsewhere) guards it.
    fn assert_no_load_ever(&self) {
        std::thread::sleep(ms(150));
        assert_eq!(self.loader.loads(), Vec::<PathBuf>::new(), "a load ran");
        assert_eq!(self.residency.loaded(), None);
    }
}

// ---- 1. the shipped wiring: builtin deps, no factory override -----------------------

#[test]
fn a_builtin_session_with_no_factory_override_transcribes_through_the_builtin_engine() {
    // Acceptance 1 (core half) / invariant "every built-in consumer reaches the one
    // BuiltinDeps": `engine_factory: None` + `builtin: Some(..)` must build
    // `engine_for(.., Some(deps))`. Today the default factory gets `None` and the job
    // ends `EngineNotConfigured`. The text on the clipboard is the fake model's, one
    // Ctrl+V, and the log's engine tag is "builtin" with the warmth carried.
    // Bite: the session keeps passing `None` to `engine_for` (EngineNotConfigured, no
    // text); a factory that ignores the deps' residency (the fake loader unused).
    let r = rig(
        &builtin_settings(Some("tiny")),
        &[ModelId::Tiny],
        &["tiny"],
        None,
    );
    r.hold();

    assert_eq!(r.clipboard.texts(), vec![LOCAL_TEXT.to_string()]);
    assert_eq!(
        r.paster
            .calls()
            .iter()
            .filter(|c| **c == PasterCall::SendCtrlV)
            .count(),
        1,
        "one Ctrl+V: {:?}",
        r.paster.calls()
    );
    match r.jobs().as_slice() {
        [DictationEvent::JobFinished {
            engine: Some("builtin"),
            outcome: OutcomeCode::Text,
            failure: None,
            local: Some(_),
            ..
        }] => {}
        other => panic!("jobs {other:?}"),
    }
    assert_eq!(
        r.loader.transcriptions().len(),
        1,
        "the job ran through the residency's fake model"
    );
}

// ---- 2. prewarm at the press, hold while recording, unload after idle ----------------

#[test]
fn a_builtin_press_prewarms_once_and_the_model_survives_a_long_recording_then_unloads_after_idle() {
    // Acceptance 2 / spec FR-013, FR-015, NFR-03. The press with BuiltinLocal and a
    // downloaded model loads the model while the user speaks (the loader is called
    // before the release); 601 s of fake time and idle ticks during the recording
    // never unload it (the recording hold); after the job it is warm (no second
    // load), still loaded 599 s after the last activity and gone at 601 s.
    // Bite: no prewarm (no load before the release), prewarm per press without the
    // dedupe (two loads), no `recording_started` hold (the tick at 601 s unloads
    // during the recording), the hold released at the job end only or never (the
    // unload point moves), an unload that ignores the 10-minute idle.
    let r = rig(
        &builtin_settings(Some("tiny")),
        &[ModelId::Tiny],
        &["tiny"],
        None,
    );
    let t = past();
    r.press(t);

    assert!(
        eventually(BUDGET, || r.residency.loaded() == Some(ModelId::Tiny)),
        "the press did not prewarm tiny while recording; loads {:?}",
        r.loader.loads()
    );
    assert_eq!(r.loader.loads().len(), 1, "one load at the press");
    assert!(
        r.jobs().is_empty(),
        "premise: still recording, no job yet: {:?}",
        r.observer.events()
    );

    r.clock.advance(Duration::from_secs(601));
    r.residency.tick();
    assert_eq!(
        r.residency.loaded(),
        Some(ModelId::Tiny),
        "unloaded during the recording"
    );
    assert_eq!(r.loader.dropped(), 0);

    r.release(t + ms(3000));
    r.wait_job();
    assert_eq!(r.clipboard.texts(), vec![LOCAL_TEXT.to_string()]);
    assert_eq!(
        r.loader.loads().len(),
        1,
        "the job used the prewarmed model, no second load"
    );

    // Idle countdown starts at the last activity (the job's end).
    r.clock.advance(Duration::from_secs(599));
    r.residency.tick();
    assert_eq!(
        r.residency.loaded(),
        Some(ModelId::Tiny),
        "unloaded before 10 min idle"
    );
    r.clock.advance(Duration::from_secs(2));
    r.residency.tick();
    assert_eq!(r.residency.loaded(), None, "still loaded 10 min after idle");
    assert_eq!(r.loader.dropped(), 1);
    assert_eq!(r.loader.live(), 0);
}

// ---- 3. an Api press never loads ------------------------------------------------------

#[test]
fn an_api_press_never_prewarms_or_loads_even_with_a_downloaded_builtin_model() {
    // Invariant "nothing loads at app start / for another engine". The built-in deps
    // exist, tiny is on disk and `builtin_local.model_id` names it, but the engine is
    // Api: the press touches no loader. Positive control: the job delivers the API
    // engine's text. Bite: a prewarm that ignores the engine kind (the snapshot's
    // model id alone decides), a prewarm at session start.
    let r = rig(
        &api_settings(),
        &[ModelId::Tiny],
        &["tiny"],
        Some(api_factory()),
    );
    r.hold();

    assert_eq!(r.clipboard.texts(), vec![API_TEXT.to_string()]);
    r.assert_no_load_ever();
}

// ---- 4. failure branch: no model downloaded --------------------------------------------

#[test]
fn a_builtin_press_with_no_model_never_loads_and_keeps_the_audio_with_the_engine_settings_notice() {
    // Acceptance 3 / spec FR-011 under decision #311 (text-only notice): no model
    // selected and none on disk. The press prewarms nothing, the job ends
    // `NoLocalModel`, nothing is copied or pasted, the audio is the one pending
    // recording, and the toast offered is the #147 text naming the Engine settings
    // with the Retry action for that pending id.
    // Bite: a prewarm with a made-up path (a load attempt), a swallowed
    // `NoLocalModel` (a pasted empty text, no pending), a wrong notice id, a retry
    // action missing.
    let r = rig(&builtin_settings(None), &[], &[], None);
    r.hold();

    match r.jobs().as_slice() {
        [DictationEvent::JobFinished {
            outcome: OutcomeCode::Failed,
            failure: Some("NoLocalModel"),
            local: None,
            ..
        }] => {}
        other => panic!("jobs {other:?}"),
    }
    assert_eq!(r.clipboard.texts(), Vec::<String>::new(), "nothing copied");
    assert!(
        !r.paster.calls().contains(&PasterCall::SendCtrlV),
        "nothing pasted: {:?}",
        r.paster.calls()
    );
    let pending = r.temp.stored_ids();
    assert_eq!(pending.len(), 1, "the audio is kept pending: {pending:?}");

    assert!(
        eventually(BUDGET, || !r.notifier.calls().is_empty()),
        "no notice was offered"
    );
    let notice = r.notifier.calls();
    let [call] = notice.as_slice() else {
        panic!("one notice expected: {notice:?}")
    };
    assert_eq!(call.id, FailureReason::NoLocalModel.message_id());
    assert_eq!(call.action, Some(ToastAction::Retry(pending[0])));
    let text = i18n::text(UiLanguage::En, call.id, &[]);
    assert!(text.contains("Engine settings"), "notice text: {text:?}");

    r.assert_no_load_ever();
}

#[test]
fn a_builtin_press_with_a_selected_model_missing_on_disk_never_loads_and_fails_no_local_model() {
    // The same failure branch with a model SELECTED but its file absent (the store
    // answers `None`): no prewarm of a made-up path, and `transcribe` fails
    // `NoLocalModel` at the missing file instead of delivering an empty text.
    // Bite: a swallowed `NoLocalModel` at `path_if_downloaded` in
    // `BuiltinEngine::transcribe` (an empty text delivered). A prewarm with a made-up
    // path is not observable here: `ModelResidency::load` refuses a missing file
    // (`check_file`) before the loader is called.
    let r = rig(&builtin_settings(Some("tiny")), &[], &["tiny"], None);
    r.hold();

    match r.jobs().as_slice() {
        [DictationEvent::JobFinished {
            outcome: OutcomeCode::Failed,
            failure: Some("NoLocalModel"),
            local: None,
            ..
        }] => {}
        other => panic!("jobs {other:?}"),
    }
    assert_eq!(r.clipboard.texts(), Vec::<String>::new(), "nothing copied");
    assert!(
        !r.paster.calls().contains(&PasterCall::SendCtrlV),
        "nothing pasted: {:?}",
        r.paster.calls()
    );
    assert_eq!(r.temp.stored_ids().len(), 1, "the audio is kept pending");
    r.assert_no_load_ever();
}

// ---- 5. ModelResidency::on_settings ------------------------------------------------------

fn residency() -> (Arc<ModelResidency>, Arc<FakeLoader>, Arc<FakeMono>, TempDir) {
    let tmp = TempDir::new();
    let clock = FakeMono::starting_at(common::timing::now());
    let loader = FakeLoader::new(&clock);
    let residency = Arc::new(ModelResidency::new(
        Arc::clone(&loader) as Arc<dyn SpeechModelLoader>,
        Arc::clone(&clock) as Arc<dyn MonotonicClock>,
        MODEL_IDLE_UNLOAD,
    ));
    place_models(tmp.path(), &[ModelId::Tiny, ModelId::Base]);
    (residency, loader, clock, tmp)
}

fn load(r: &ModelResidency, tmp: &TempDir, id: ModelId) {
    r.prewarm(id, tmp.path().join(file_name(id)));
    assert!(
        eventually(BUDGET, || r.loaded() == Some(id)),
        "{id:?} did not load"
    );
}

#[test]
fn on_settings_keeps_the_model_for_the_same_id_and_unloads_for_another_id() {
    // Spec FR-012 / T027: a save that keeps engine BuiltinLocal and the loaded model
    // id keeps it warm; a save that selects another model unloads at once.
    // Bite: `on_settings` that always unloads (the same-id leg is red) or never
    // unloads (the other-id leg is red); a selected id read from the wrong field.
    let (r, loader, _clock, tmp) = residency();
    load(&r, &tmp, ModelId::Tiny);

    r.on_settings(&builtin_settings(Some("tiny")));
    assert_eq!(r.loaded(), Some(ModelId::Tiny), "same id: stays loaded");
    assert_eq!(loader.dropped(), 0);

    r.on_settings(&builtin_settings(Some("base")));
    assert_eq!(r.loaded(), None, "another model id unloads");
    assert_eq!(loader.dropped(), 1);
    assert_eq!(loader.live(), 0);
}

#[test]
fn on_settings_unloads_when_the_engine_moves_away_from_builtin_local() {
    // The same model id stays in `builtin_local`, but the engine is Api (or the
    // model is unselected): nothing may stay in memory (NFR-03 spirit, spec FR-012).
    // Bite: the selected id taken from `builtin_local.model_id` without looking at
    // the engine kind.
    for engine in [EngineKind::Api, EngineKind::LocalServer] {
        let (r, loader, _clock, tmp) = residency();
        load(&r, &tmp, ModelId::Tiny);
        let mut s = builtin_settings(Some("tiny"));
        s.engine = engine;
        s.local_server.base_url = "http://localhost:8000/v1".to_string();

        r.on_settings(&s);

        assert_eq!(r.loaded(), None, "engine {engine:?} keeps a model loaded");
        assert_eq!(loader.dropped(), 1, "{engine:?}");
    }
    let (r, _loader, _clock, tmp) = residency();
    load(&r, &tmp, ModelId::Tiny);
    r.on_settings(&builtin_settings(None));
    assert_eq!(r.loaded(), None, "no model selected unloads");
}

// ---- 6. LocalModels with the residency ---------------------------------------------------

struct Models {
    _tmp: TempDir,
    dir: PathBuf,
    models: LocalModels,
    service: SettingsService,
    residency: Arc<ModelResidency>,
    loader: Arc<FakeLoader>,
    _clock: Arc<FakeMono>,
}

fn models(on_disk: &[ModelId]) -> Models {
    let tmp = TempDir::new();
    let dir = tmp.path().join("models");
    place_models(&dir, on_disk);
    let clock = FakeMono::starting_at(common::timing::now());
    let loader = FakeLoader::new(&clock);
    let residency = Arc::new(ModelResidency::new(
        Arc::clone(&loader) as Arc<dyn SpeechModelLoader>,
        Arc::clone(&clock) as Arc<dyn MonotonicClock>,
        MODEL_IDLE_UNLOAD,
    ));
    let (models, cleanup) = LocalModels::open(
        dir.clone(),
        FakeDisk::with_available(1 << 30),
        in_time(),
        local_catalog(),
    );
    cleanup.expect("cleanup at open");
    let models = models.with_residency(Arc::clone(&residency));

    let stored = {
        let mut s = defaults(OS);
        s.engine = EngineKind::Api;
        s.api.base_url = "https://api.example.com/v1".to_string();
        s
    };
    let bytes = serde_json::to_vec(&stored).expect("serialize settings");
    let (service, _) = SettingsService::load_or_init(
        SettingsDeps {
            file: Arc::new(FakeSettingsFile::with_bytes(&bytes)),
            credentials: Arc::new(
                FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, KEY),
            ),
            autostart: Arc::new(FakeAutostart::new()),
            hotkeys: Arc::new(FakeHotkeyRegistrar::new()),
            local_models: Arc::new(FakeDownloadedModels::new(&[])),
            clock: Arc::new(FakeClock::at(
                UNIX_EPOCH + Duration::from_secs(1_709_251_199),
            )),
            call_roots: Arc::new(voicen_core::settings::roots::FakeCallRoots::new()),
        },
        OS,
    );
    Models {
        _tmp: tmp,
        dir,
        models,
        service,
        residency,
        loader,
        _clock: clock,
    }
}

impl Models {
    fn loaded_flags(&self) -> Vec<(ModelId, bool)> {
        self.models
            .list()
            .iter()
            .map(|v| (v.id, v.loaded))
            .collect()
    }
}

#[test]
fn list_reports_loaded_for_exactly_the_model_the_residency_holds() {
    // Spec FR-008/US3 view: `loaded` is true for the one model in memory and false
    // for every other (a model on disk, not loaded, included), and it follows an
    // unload. Today `loaded` is hard-coded false.
    // Bite: `loaded: false` constant; `loaded: true` for every downloaded model; a
    // flag that is never cleared after the unload.
    let m = models(&[ModelId::Tiny, ModelId::Base]);
    assert!(
        m.loaded_flags().iter().all(|(_, loaded)| !loaded),
        "premise: nothing loaded: {:?}",
        m.loaded_flags()
    );

    m.residency
        .prewarm(ModelId::Tiny, m.dir.join(file_name(ModelId::Tiny)));
    assert!(eventually(BUDGET, || m.residency.loaded() == Some(ModelId::Tiny)));

    let flags = m.loaded_flags();
    let loaded: Vec<ModelId> = flags
        .iter()
        .filter(|(_, l)| *l)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(loaded, vec![ModelId::Tiny], "{flags:?}");

    m.residency.on_settings(&builtin_settings(None));
    assert!(
        m.loaded_flags().iter().all(|(_, loaded)| !loaded),
        "the flag follows the unload: {:?}",
        m.loaded_flags()
    );
}

#[test]
fn delete_of_the_loaded_model_unloads_it_through_the_residency_and_removes_the_file() {
    // Spec T035 / FR-021 (core half): the production delete releases through the
    // coordinator's own residency. A loaded, unheld model is dropped, then the file
    // is removed, then the model lists as not downloaded and not loaded. Today
    // production passes `NoResidency`, which never unloads.
    // Bite: a delete that ignores the residency (`dropped` stays 0, the model stays
    // in memory), a release after the removal.
    let m = models(&[ModelId::Tiny, ModelId::Base]);
    m.residency
        .prewarm(ModelId::Tiny, m.dir.join(file_name(ModelId::Tiny)));
    assert!(eventually(BUDGET, || m.residency.loaded() == Some(ModelId::Tiny)));

    let got = m.models.delete_resident("tiny", &m.service);

    assert_eq!(
        got,
        Ok(DeleteOutcome {
            engine_reset: false,
            reset_failed: false
        })
    );
    assert_eq!(m.residency.loaded(), None, "the model is still in memory");
    assert_eq!(m.loader.dropped(), 1);
    assert_eq!(m.loader.live(), 0);
    assert!(!m.dir.join(file_name(ModelId::Tiny)).exists(), "file left");
    assert!(m.dir.join(file_name(ModelId::Base)).exists(), "base kept");
    let remaining: Vec<ModelId> = m
        .models
        .list()
        .into_iter()
        .filter(|v| v.state == LocalModelState::Downloaded)
        .map(|v| v.id)
        .collect();
    assert_eq!(remaining, vec![ModelId::Base]);
}

#[test]
fn delete_of_a_model_a_transcription_holds_is_refused_as_in_use_and_nothing_changes() {
    // The same production path, the held branch: `model_in_use`, the file stays,
    // the model stays loaded. Bite: a delete that removes the file while the
    // residency says in use (the release result ignored).
    let m = models(&[ModelId::Tiny]);
    let (guard, _warmth) = m
        .residency
        .acquire(ModelId::Tiny, m.dir.join(file_name(ModelId::Tiny)))
        .expect("acquire tiny");

    let got = m.models.delete_resident("tiny", &m.service);

    let reason = got.expect_err("a held model is not deleted");
    assert_eq!(reason.code, "model_in_use");
    assert!(
        m.dir.join(file_name(ModelId::Tiny)).exists(),
        "file removed"
    );
    assert_eq!(m.residency.loaded(), Some(ModelId::Tiny));
    drop(guard);
}

// ---- the press snapshot, not the start-up settings ------------------------------------

#[test]
fn a_saved_engine_change_reaches_the_next_press_snapshot() {
    // The session reads the engine from the press snapshot, not from start-up: a
    // session started on Api and saved to BuiltinLocal prewarms at the next press.
    // Bite: the engine kind or model cached at `DictationSession::start`.
    let r = rig(&api_settings(), &[ModelId::Tiny], &["tiny"], None);
    let mut s = (*r.settings.snapshot()).clone();
    s.engine = EngineKind::BuiltinLocal;
    s.builtin_local.model_id = Some("tiny".to_string());
    let untouched = KeyEdits::default();
    let saved = r.settings.save(SaveRequest {
        settings: s,
        keys: untouched,
    });
    assert!(matches!(saved, SaveOutcome::Saved { .. }), "{saved:?}");

    let t = past();
    r.press(t);
    assert!(
        eventually(BUDGET, || r.residency.loaded() == Some(ModelId::Tiny)),
        "no prewarm after the engine change"
    );
    r.release(t + ms(3000));
    r.wait_job();
    assert_eq!(r.clipboard.texts(), vec![LOCAL_TEXT.to_string()]);
}
