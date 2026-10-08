//! The public test fakes (feature `test-fakes`, decision #23 N4), used from outside
//! the crate the way T-032's and T-030's tests will use them.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use voicen_core::audio::AudioBuffer;
use voicen_core::autostart::{Autostart, AutostartCall, AutostartError, FakeAutostart};
use voicen_core::hotkey_registrar::{
    FakeHotkeyRegistrar, HotkeyRegistrar, RegistrarCall, Unavailable,
};
use voicen_core::models::{DownloadedModels, FakeDownloadedModels};
use voicen_core::platform::{
    AudioSource, FakeAudioSource, FakeIndicator, FakeShellRequests, FrameChunk, FrameSink,
    Indicator, IndicatorCall, ShellRequestCall, ShellRequests,
};
use voicen_core::recording::{CaptureError, OverlayState, TrayState};
use voicen_core::secrets::{
    CredentialCall, CredentialError, CredentialOp, CredentialStore, FakeCredentialStore, KeySlot,
    Secret,
};
use voicen_core::settings::gate::SettingsTab;
use voicen_core::settings::hotkey::{Hotkey, HotkeyKey};
use voicen_core::settings::{FieldId, Mode};

fn call(op: CredentialOp, slot: KeySlot) -> CredentialCall {
    CredentialCall { op, slot }
}

fn read_value(store: &dyn CredentialStore, slot: KeySlot) -> Option<String> {
    store
        .read(slot)
        .expect("read succeeds")
        .map(|s| s.expose().to_string())
}

#[test]
fn fake_credential_store_records_calls_per_slot() {
    // Bite: a call not recorded, recorded under another slot, or out of order.
    let store = FakeCredentialStore::new();
    store
        .write(KeySlot::TranscriptionApi, &Secret::new("sk-test-api"))
        .expect("write");
    let _ = store.read(KeySlot::LocalServer).expect("read");
    store.delete(KeySlot::PostProcessing).expect("delete");
    let _ = store.read(KeySlot::TranscriptionApi).expect("read");
    assert_eq!(
        store.calls(),
        vec![
            call(CredentialOp::Write, KeySlot::TranscriptionApi),
            call(CredentialOp::Read, KeySlot::LocalServer),
            call(CredentialOp::Delete, KeySlot::PostProcessing),
            call(CredentialOp::Read, KeySlot::TranscriptionApi),
        ]
    );
}

#[test]
fn fake_credential_store_keeps_slots_apart() {
    // Bite: one shared value for all slots; delete of one slot touching another.
    let store = FakeCredentialStore::new()
        .with_key(KeySlot::TranscriptionApi, "sk-test-api")
        .with_key(KeySlot::LocalServer, "sk-test-local");
    assert!(store.calls().is_empty(), "with_key is not a recorded call");

    store
        .write(KeySlot::PostProcessing, &Secret::new("sk-test-pp"))
        .expect("write");
    assert_eq!(
        read_value(&store, KeySlot::TranscriptionApi).as_deref(),
        Some("sk-test-api")
    );
    assert_eq!(
        read_value(&store, KeySlot::LocalServer).as_deref(),
        Some("sk-test-local")
    );
    assert_eq!(
        read_value(&store, KeySlot::PostProcessing).as_deref(),
        Some("sk-test-pp")
    );

    store.delete(KeySlot::LocalServer).expect("delete");
    assert_eq!(read_value(&store, KeySlot::LocalServer), None);
    assert_eq!(store.stored(KeySlot::LocalServer), None);
    assert_eq!(
        store.stored(KeySlot::TranscriptionApi).as_deref(),
        Some("sk-test-api")
    );

    // Deleting an absent key is Ok (contract).
    store
        .delete(KeySlot::LocalServer)
        .expect("absent delete is Ok");

    // Overwrite replaces the value.
    store
        .write(KeySlot::TranscriptionApi, &Secret::new("sk-test-api-2"))
        .expect("write");
    assert_eq!(
        store.stored(KeySlot::TranscriptionApi).as_deref(),
        Some("sk-test-api-2")
    );
}

#[test]
fn fake_credential_store_injected_failures() {
    // Bite: the failure ignored, applied to another slot or op, or changing the slot.
    let store = FakeCredentialStore::new().with_key(KeySlot::LocalServer, "sk-test-old");
    let err = CredentialError { os_code: 1312 };
    store.fail(CredentialOp::Write, KeySlot::LocalServer, err);
    store.fail(CredentialOp::Delete, KeySlot::PostProcessing, err);
    store.fail(CredentialOp::Read, KeySlot::TranscriptionApi, err);

    assert_eq!(
        store.write(KeySlot::LocalServer, &Secret::new("sk-test-new")),
        Err(err)
    );
    assert_eq!(
        store.stored(KeySlot::LocalServer).as_deref(),
        Some("sk-test-old")
    );
    // Persistent until cleared.
    assert_eq!(
        store.write(KeySlot::LocalServer, &Secret::new("sk-test-new")),
        Err(err)
    );
    assert_eq!(store.delete(KeySlot::PostProcessing), Err(err));
    assert!(matches!(store.read(KeySlot::TranscriptionApi), Err(e) if e == err));

    // Other ops on the same slot, and the same op on other slots, still work.
    assert_eq!(
        read_value(&store, KeySlot::LocalServer).as_deref(),
        Some("sk-test-old")
    );
    store
        .write(KeySlot::PostProcessing, &Secret::new("sk-test-pp"))
        .expect("write on another slot");
    store
        .delete(KeySlot::LocalServer)
        .expect("delete on the failing slot");

    // Failed calls are recorded too.
    let calls = store.calls();
    assert_eq!(calls.len(), 7, "{calls:?}");
    assert_eq!(calls[0], call(CredentialOp::Write, KeySlot::LocalServer));

    store.clear_failures();
    store
        .write(KeySlot::LocalServer, &Secret::new("sk-test-new"))
        .expect("write after clear");
    assert_eq!(
        store.stored(KeySlot::LocalServer).as_deref(),
        Some("sk-test-new")
    );
}

fn hotkey(key: HotkeyKey) -> Hotkey {
    Hotkey {
        ctrl: true,
        alt: true,
        shift: false,
        win: false,
        key,
    }
}

#[test]
fn fake_hotkey_registrar_two_phase_with_call_log() {
    // Bite: commit not making the hotkey active, abort changing it, calls not logged.
    let registrar = FakeHotkeyRegistrar::new();
    assert_eq!(registrar.active(), None);

    let first = registrar
        .prepare(hotkey(HotkeyKey::Space), Mode::Hold)
        .expect("prepare");
    assert_eq!(first.hotkey, hotkey(HotkeyKey::Space));
    assert_eq!(first.mode, Mode::Hold);
    assert_eq!(registrar.active(), None, "prepare alone does not activate");
    registrar.commit(first);
    assert_eq!(
        registrar.active(),
        Some((hotkey(HotkeyKey::Space), Mode::Hold))
    );

    let second = registrar
        .prepare(hotkey(HotkeyKey::F9), Mode::Toggle)
        .expect("prepare");
    registrar.abort(second);
    assert_eq!(
        registrar.active(),
        Some((hotkey(HotkeyKey::Space), Mode::Hold)),
        "abort keeps the old hotkey"
    );

    assert_eq!(
        registrar.calls(),
        vec![
            RegistrarCall::Prepare(hotkey(HotkeyKey::Space), Mode::Hold),
            RegistrarCall::Commit(hotkey(HotkeyKey::Space)),
            RegistrarCall::Prepare(hotkey(HotkeyKey::F9), Mode::Toggle),
            RegistrarCall::Abort(hotkey(HotkeyKey::F9)),
        ]
    );
}

#[test]
fn fake_hotkey_registrar_injected_prepare_failure() {
    // Bite: the injected failure ignored, or a failed prepare changing the active key.
    let registrar = FakeHotkeyRegistrar::new();
    let ok = registrar
        .prepare(hotkey(HotkeyKey::Space), Mode::Hold)
        .expect("prepare");
    registrar.commit(ok);

    registrar.fail_prepare(true);
    assert_eq!(
        registrar.prepare(hotkey(HotkeyKey::A), Mode::Toggle),
        Err(Unavailable)
    );
    assert_eq!(
        registrar.active(),
        Some((hotkey(HotkeyKey::Space), Mode::Hold))
    );
    assert_eq!(
        registrar.calls().last(),
        Some(&RegistrarCall::Prepare(hotkey(HotkeyKey::A), Mode::Toggle)),
        "a failed prepare is recorded"
    );

    registrar.fail_prepare(false);
    assert!(registrar
        .prepare(hotkey(HotkeyKey::A), Mode::Toggle)
        .is_ok());
}

#[test]
fn fake_downloaded_models_reports_only_its_ids() {
    // Bite: is_downloaded true for an unknown id, or list() not the given ids.
    let models = FakeDownloadedModels::new(&["base", "small"]);
    assert!(models.is_downloaded("base"));
    assert!(models.is_downloaded("small"));
    assert!(!models.is_downloaded("tiny"));
    assert!(!models.is_downloaded(""));
    assert!(!models.is_downloaded("Base"), "ids are exact strings");
    let mut list = models.list();
    list.sort();
    assert_eq!(list, vec!["base".to_string(), "small".to_string()]);

    let empty = FakeDownloadedModels::new(&[]);
    assert!(!empty.is_downloaded("base"));
    assert!(empty.list().is_empty());
}

#[test]
fn traits_are_object_safe_and_shareable() {
    // SettingsDeps holds them as Arc<dyn Trait> across threads (contracts/core-traits.md).
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    assert_send_sync::<dyn CredentialStore>();
    assert_send_sync::<dyn HotkeyRegistrar>();
    assert_send_sync::<dyn DownloadedModels>();
    assert_send_sync::<dyn Autostart>();
    let _shared: Vec<std::sync::Arc<dyn CredentialStore>> =
        vec![std::sync::Arc::new(FakeCredentialStore::new())];
}

#[test]
fn fake_autostart_records_calls_and_fails_per_value() {
    // The fake the T-014 service tests rely on. Bite: a call not recorded, a
    // failed set changing the state, or a failure injected for one value hitting
    // the other.
    let fake = FakeAutostart::new();
    assert!(!fake.is_on());
    assert_eq!(fake.is_enabled(), Ok(false));
    fake.set(true).expect("set(true)");
    assert!(fake.is_on());

    let err = AutostartError { os_code: 5 };
    fake.fail_set(false, err);
    assert_eq!(fake.set(false), Err(err));
    assert!(fake.is_on(), "a failed set changed the state");
    fake.set(true).expect("set(true) still succeeds");

    fake.fail_is_enabled(err);
    assert_eq!(fake.is_enabled(), Err(err));
    fake.clear_failures();
    fake.set(false).expect("cleared");
    assert!(!fake.is_on());

    assert_eq!(
        fake.calls(),
        vec![
            AutostartCall::IsEnabled,
            AutostartCall::Set(true),
            AutostartCall::Set(false),
            AutostartCall::Set(true),
            AutostartCall::IsEnabled,
            AutostartCall::Set(false),
        ]
    );

    let on = FakeAutostart::enabled();
    assert!(on.is_on());
    assert!(on.calls().is_empty(), "enabled() recorded a call");
    on.fail_set(true, err);
    on.set(false)
        .expect("set(false) unaffected by fail_set(true)");
}

/// One `frames` call: samples, rate, channels, instant.
type FramesCall = (Vec<f32>, u32, u16, Instant);

/// Keeps every `frames` call.
#[derive(Default)]
struct LogSink {
    calls: Mutex<Vec<FramesCall>>,
}

impl FrameSink for LogSink {
    fn frames(&self, interleaved: &[f32], rate: u32, channels: u16, at: Instant) {
        self.calls
            .lock()
            .expect("lock")
            .push((interleaved.to_vec(), rate, channels, at));
    }
}

#[test]
fn fake_audio_source_delivers_before_stop_and_counts_open_handles() {
    // The fake the T-051 session tests rely on (T-051 analysis "Fakes"). Bite:
    // chunks arriving after stop returned, a handle not closed by stop, by drop or
    // by a failing stop, a failed start counted as open or not counted as a call.
    let source = FakeAudioSource::new();
    let sink = Arc::new(LogSink::default());
    let at = Instant::now();
    let chunk = FrameChunk {
        samples: vec![0.5, -0.5, 0.25, -0.25],
        rate: 48_000,
        channels: 2,
        at,
    };
    source.set_chunks(vec![chunk.clone(), chunk.clone()]);
    assert_eq!(source.open_handles(), 0);

    let handle = source.start(sink.clone()).expect("start");
    assert_eq!(source.open_handles(), 1);
    assert_eq!(handle.stop(), Ok(()));
    assert_eq!(source.open_handles(), 0);
    let calls = sink.calls.lock().expect("lock").clone();
    assert_eq!(
        calls,
        vec![
            (chunk.samples.clone(), 48_000, 2, at),
            (chunk.samples.clone(), 48_000, 2, at)
        ],
        "every chunk delivered, in order, before stop returned"
    );

    let handle = source.start(sink.clone()).expect("start");
    assert_eq!(source.open_handles(), 1);
    drop(handle);
    assert_eq!(source.open_handles(), 0, "drop closes");

    source.set_stop_error(Some(CaptureError::DeviceBusy));
    let handle = source.start(sink.clone()).expect("start");
    assert_eq!(handle.stop(), Err(CaptureError::DeviceBusy));
    assert_eq!(source.open_handles(), 0, "a failing stop still closes");

    source.set_start_error(Some(CaptureError::AccessDenied));
    assert!(matches!(
        source.start(sink.clone()),
        Err(CaptureError::AccessDenied)
    ));
    assert_eq!(source.open_handles(), 0);
    assert_eq!(source.start_calls(), 4);
}

#[test]
fn frame_chunk_from_buffer_is_16k_mono_scaled_to_unit_range() {
    // Bite: a wrong scale (the session's audio would change level), another rate.
    let at = Instant::now();
    let chunk = FrameChunk::from_buffer(&AudioBuffer::from_16k_mono(vec![16_384, -32_768, 0]), at);
    assert_eq!(chunk.samples, vec![0.5, -1.0, 0.0]);
    assert_eq!((chunk.rate, chunk.channels, chunk.at), (16_000, 1, at));
}

#[test]
fn fake_indicator_and_shell_requests_record_in_order() {
    // Bite: a call not recorded, the per-port views mixing the two ports.
    let indicator = FakeIndicator::new();
    indicator.set_tray(TrayState::Recording, false);
    indicator.set_overlay(&OverlayState::Recording);
    indicator.set_tray(TrayState::Error, true);
    assert_eq!(
        indicator.calls(),
        vec![
            IndicatorCall::Tray(TrayState::Recording, false),
            IndicatorCall::Overlay(OverlayState::Recording),
            IndicatorCall::Tray(TrayState::Error, true),
        ]
    );
    assert_eq!(
        indicator.trays(),
        vec![(TrayState::Recording, false), (TrayState::Error, true)]
    );
    assert_eq!(indicator.overlays(), vec![OverlayState::Recording]);
    let timed = indicator.timed_calls();
    assert!(timed.windows(2).all(|w| w[0].0 <= w[1].0));

    let requests = FakeShellRequests::new();
    requests.open_settings(SettingsTab::Engine, None);
    requests.open_settings(SettingsTab::Recording, Some(FieldId::RecordingHotkey));
    assert_eq!(
        requests.calls(),
        vec![
            ShellRequestCall::OpenSettings(SettingsTab::Engine, None),
            ShellRequestCall::OpenSettings(SettingsTab::Recording, Some(FieldId::RecordingHotkey)),
        ]
    );
}

#[test]
fn session_ports_are_object_safe_and_shareable() {
    // The session holds them as Arc<dyn Trait> across threads.
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    assert_send_sync::<dyn AudioSource>();
    assert_send_sync::<dyn FrameSink>();
    assert_send_sync::<dyn Indicator>();
    assert_send_sync::<dyn ShellRequests>();
}
