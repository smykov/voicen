//! Live apply and persistence through the public API (spec 004 T034, core part;
//! research R-4; decision #22): subscribers get one snapshot per `Saved` and none on
//! a refusal, a handed-out snapshot never changes, and a new service over the same
//! folder returns every saved value.

use std::sync::mpsc::TryRecvError;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use voicen_core::autostart::FakeAutostart;
use voicen_core::clock::FakeClock;
use voicen_core::hotkey_registrar::FakeHotkeyRegistrar;
use voicen_core::i18n::UiLanguage;
use voicen_core::models::FakeDownloadedModels;
use voicen_core::post_process::settings::PostProcessingSettings;
use voicen_core::secrets::{FakeCredentialStore, KeyEdit, KeyEdits, Secret};
use voicen_core::settings::file::{FakeSettingsFile, FsSettingsFile, SettingsFile};
use voicen_core::settings::service::{SaveOutcome, SaveRequest, SettingsDeps, SettingsService};
use voicen_core::settings::{
    defaults, ApiSettings, BuiltinLocalSettings, EngineKind, ErrorCode, FieldError, FieldId,
    HistorySettings, LoadOutcome, LocalServerSettings, Microphone, Mode, Settings,
};
use voicen_core::test_support::TempDir;

const OS: Option<&str> = Some("en-US");

fn deps(file: Arc<dyn SettingsFile>, creds: Arc<FakeCredentialStore>) -> SettingsDeps {
    SettingsDeps {
        file,
        credentials: creds,
        autostart: Arc::new(FakeAutostart::new()),
        hotkeys: Arc::new(FakeHotkeyRegistrar::new()),
        local_models: Arc::new(FakeDownloadedModels::new(&["base"])),
        clock: Arc::new(FakeClock::at(
            UNIX_EPOCH + Duration::from_secs(1_709_251_199),
        )),
    }
}

/// A service over an in-memory file holding valid settings.
fn service() -> SettingsService {
    let bytes = serde_json::to_vec(&defaults(OS)).expect("serialize");
    let file = Arc::new(FakeSettingsFile::with_bytes(&bytes));
    let (service, _) =
        SettingsService::load_or_init(deps(file, Arc::new(FakeCredentialStore::new())), OS);
    service
}

/// Valid for engine `api` and different from `defaults(OS)` in every field but
/// `schema_version`; fake hosts only.
fn sample() -> Settings {
    Settings {
        schema_version: 1,
        engine: EngineKind::Api,
        api: ApiSettings {
            base_url: "https://api.example.com/v1".into(),
            model: "whisper-test".into(),
        },
        local_server: LocalServerSettings {
            base_url: "http://192.0.2.10:8000/v1".into(),
            model: "local-test".into(),
        },
        builtin_local: BuiltinLocalSettings {
            model_id: Some("base".into()),
        },
        speech_language: Some("de".into()),
        microphone: Some(Microphone {
            id: "{0.0.1.00000000}.{fake-device-id}".into(),
            name: "Test Microphone (fake)".into(),
        }),
        hotkey: "Ctrl+Shift+F9".into(),
        mode: Mode::Toggle,
        auto_paste: false,
        post_processing: PostProcessingSettings {
            enabled: true,
            base_url: "https://llm.example.com/v1".into(),
            model: "llm-test".into(),
            prompt: "Fix the text.".into(),
        },
        history: HistorySettings {
            enabled: false,
            size: 7,
        },
        start_with_windows: true,
        ui_language: UiLanguage::Ru,
    }
}

/// Every key slot gets a fake key, so engine `api` (and any future enabled
/// post-processing rule) is satisfied.
fn with_keys(settings: Settings) -> SaveRequest {
    SaveRequest {
        settings,
        keys: KeyEdits {
            transcription_api: KeyEdit::Replace(Secret::new("sk-test-api-0000")),
            local_server: KeyEdit::Untouched,
            post_processing: KeyEdit::Replace(Secret::new("sk-test-pp-0000")),
        },
    }
}

/// A draft that `validate` refuses.
fn invalid() -> SaveRequest {
    let mut settings = sample();
    settings.history.size = 0;
    with_keys(settings)
}

#[track_caller]
fn assert_saved(outcome: SaveOutcome) {
    assert!(
        matches!(outcome, SaveOutcome::Saved { .. }),
        "expected Saved, got {outcome:?}"
    );
}

#[test]
fn save_round_trips_through_a_new_service() {
    // P-013: every field written by one service is read back by the next. Bite: a
    // field not serialized, written from the old snapshot instead of the request,
    // or the file not written at all.
    let d = defaults(OS);
    let s = sample();
    // Guard: the round trip is not vacuous for any field.
    assert_ne!(s.engine, d.engine);
    assert_ne!(s.api.base_url, d.api.base_url);
    assert_ne!(s.api.model, d.api.model);
    assert_ne!(s.local_server.base_url, d.local_server.base_url);
    assert_ne!(s.local_server.model, d.local_server.model);
    assert_ne!(s.builtin_local, d.builtin_local);
    assert_ne!(s.speech_language, d.speech_language);
    assert_ne!(s.microphone, d.microphone);
    assert_ne!(s.hotkey, d.hotkey);
    assert_ne!(s.mode, d.mode);
    assert_ne!(s.auto_paste, d.auto_paste);
    assert_ne!(s.post_processing.enabled, d.post_processing.enabled);
    assert_ne!(s.post_processing.base_url, d.post_processing.base_url);
    assert_ne!(s.post_processing.model, d.post_processing.model);
    assert_ne!(s.post_processing.prompt, d.post_processing.prompt);
    assert_ne!(s.history.enabled, d.history.enabled);
    assert_ne!(s.history.size, d.history.size);
    assert_ne!(s.start_with_windows, d.start_with_windows);
    assert_ne!(s.ui_language, d.ui_language);

    let dir = TempDir::new();
    let creds = Arc::new(FakeCredentialStore::new());
    let fs_deps = || {
        deps(
            Arc::new(FsSettingsFile::new(dir.path().to_path_buf())),
            creds.clone(),
        )
    };

    let (first, outcome) = SettingsService::load_or_init(fs_deps(), OS);
    assert_eq!(outcome, LoadOutcome::FirstRun(d.clone()));
    assert_saved(first.save(with_keys(s.clone())));
    drop(first);

    let (second, outcome) = SettingsService::load_or_init(fs_deps(), OS);
    assert_eq!(outcome, LoadOutcome::Loaded(s.clone()));
    assert_eq!(*second.snapshot(), s);
    let view = second.view();
    assert_eq!(view.settings, s);
    assert!(view.keys.transcription_api);
    assert!(view.keys.post_processing);
    assert!(!view.first_run);
    assert!(!view.reset_notice);
}

#[test]
fn subscriber_receives_snapshot_after_saved() {
    // Bite: no publish on Saved, the old snapshot published, a different Arc than
    // snapshot(), or more than one message per save.
    let service = service();
    let rx = service.subscribe();
    assert_saved(service.save(with_keys(sample())));

    let got = rx.try_recv().expect("one snapshot after Saved");
    assert_eq!(*got, sample());
    assert!(Arc::ptr_eq(&got, &service.snapshot()));
    assert!(
        matches!(rx.try_recv(), Err(TryRecvError::Empty)),
        "exactly one message per Saved"
    );
}

#[test]
fn refused_save_sends_nothing() {
    // Bite: a publish before validation / on a refusal, or the sender dropped
    // while the service lives (the receiver would see Disconnected).
    let service = service();
    let rx = service.subscribe();
    match service.save(invalid()) {
        SaveOutcome::Refused { errors, form_error } => {
            assert_eq!(
                errors,
                vec![FieldError {
                    field: FieldId::HistorySize,
                    code: ErrorCode::HistorySizeRange,
                }]
            );
            assert_eq!(form_error, None);
        }
        other => panic!("expected Refused, got {other:?}"),
    }
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));

    // The same receiver still gets the next Saved.
    assert_saved(service.save(with_keys(sample())));
    assert_eq!(*rx.try_recv().expect("snapshot after Saved"), sample());
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
}

#[test]
fn earlier_snapshot_unchanged() {
    // An in-progress dictation keeps the settings it started with. Bite: the
    // snapshot mutated in place instead of swapping the Arc.
    let service = service();
    let before = service.snapshot();
    let before_value = (*before).clone();
    assert_eq!(before_value, defaults(OS));

    assert_saved(service.save(with_keys(sample())));
    assert_eq!(*before, before_value);
    assert_eq!(*service.snapshot(), sample());
    assert!(!Arc::ptr_eq(&before, &service.snapshot()));
}

#[test]
fn dropped_receiver_does_not_fail_save() {
    // Bite: a send error to a dropped receiver turned into a refusal or a panic,
    // or the publish loop stopping at the first dead receiver.
    let service = service();
    let dropped = service.subscribe();
    let live = service.subscribe();
    drop(dropped);

    assert_saved(service.save(with_keys(sample())));
    assert_eq!(*live.try_recv().expect("live receiver"), sample());

    let mut next = sample();
    next.history.size = 8;
    assert_saved(service.save(with_keys(next.clone())));
    assert_eq!(*live.try_recv().expect("live receiver, second save"), next);
}

#[test]
fn service_is_send_and_sync() {
    // Compile-time: the shell shares one service across threads (decision #22).
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SettingsService>();
}
