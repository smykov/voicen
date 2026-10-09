//! T-012 (FR-27, FR-04; spec 001 US6, FR-030/FR-031): the microphone choice, the
//! fallback notice and device loss through the one dictation session
//! (`voicen_core::dictation::DictationSession`) over fakes. Acceptance "Test that
//! fails without the change: crates/voicen-core audio-device tests".
//!
//! Every session is built from the real `SettingsService` (fake file, fake
//! credential store holding the key), `FakeAudioSource` with a scripted device list
//! (or a sink-keeping source of this file for the loss races), a fake engine
//! factory (no network), `FakeClipboard` / `FakePaster`, `FakeIndicator`,
//! `FakeShellRequests`, `FakeCancelKey` and one `RecordingObserver`.
//!
//! Pinned API (docs/tasks/T-012.md `## Tests`; analysis option A):
//! - `platform::DeviceId(pub String)` (the WASAPI endpoint id), `platform::InputDevice
//!   { id: DeviceId, name: String, is_default: bool }`;
//! - `AudioSource::devices(&self) -> Result<Vec<InputDevice>, CaptureError>` and
//!   `AudioSource::start(&self, device: &DeviceId, sink: Arc<dyn FrameSink>)`;
//! - `FrameSink::device_lost(&self, at: Instant)`;
//! - `RecordingEnd::DeviceLost`; `i18n::NOTICE_MIC_FALLBACK` ("notice.mic_fallback",
//!   param `device` = the device's display name);
//! - `FakeAudioSource`: `set_devices(Vec<InputDevice>)` (what `devices()` returns
//!   from now on; the default list must hold one device flagged default, so the
//!   existing session tests keep recording), `set_devices_error(Option<CaptureError>)`
//!   (while `Some`, `devices()` returns it), `started_with() -> Vec<DeviceId>` (the id
//!   of every `start` call, failed ones included), `set_device_loss(Option<Instant>)`
//!   (each later capture, after its scripted chunks, calls `sink.device_lost(at)` once
//!   from its capture thread); `start` with an id absent from the scripted list
//!   returns `Err(NoDevice)` (counted, nothing opened).
//!
//! Instants: every caller instant is a minute ahead of the real clock (`ahead()`,
//! from `common::timing::now`), so a message raised at a press (`until` = press + 3 s)
//! never expires on the session's timer or at the worker's real job end while a test
//! runs, and the notice is published at the stop's `finish` (the overlay hides a
//! message during a recording, data-model `OverlayState`). No wall-clock reading
//! decides a verdict (T-080 I2); waits are bounded by `BUDGET` and only end a hung
//! test. Fake data only: example.com, `sk-test-SECRET`, fake endpoint ids.

mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

use common::timing::now;

use voicen_core::audio::AudioBuffer;
use voicen_core::autostart::FakeAutostart;
use voicen_core::clock::FakeClock;
use voicen_core::dictation::{DictationSession, SessionDeps};
use voicen_core::engine::{Engine, TranscribeRequest};
use voicen_core::events::{DeviceKind, DictationEvent, RecordingObserver};
use voicen_core::failure::FailureReason;
use voicen_core::hotkey_registrar::FakeHotkeyRegistrar;
use voicen_core::i18n;
use voicen_core::models::FakeDownloadedModels;
use voicen_core::pipeline::{EngineFactory, PipelineDeps};
use voicen_core::platform::{
    AudioSource, CaptureHandle, DeviceId, FakeAudioSource, FakeCancelKey, FakeClipboard,
    FakeIndicator, FakePaster, FakeShellRequests, FakeTempAudioStore, FrameChunk, FrameSink,
    IndicatorCall, InputDevice, StartWindow, WindowRef,
};
use voicen_core::post_process::PassThrough;
use voicen_core::recording::{CaptureError, MicCause, OverlayState, RecordingEnd, TrayState};
use voicen_core::secrets::{CredentialStore, FakeCredentialStore, KeyEdits, KeySlot};
use voicen_core::settings::file::FakeSettingsFile;
use voicen_core::settings::service::{SaveOutcome, SaveRequest, SettingsDeps, SettingsService};
use voicen_core::settings::{defaults, EngineKind, LoadOutcome, Microphone, Mode, Settings};
use voicen_core::test_support::fixtures;
use voicen_core::vad::{EnergyDetector, SpeechDetector, SpeechGate};

const KEY: &str = "sk-test-SECRET";
const OS: Option<&str> = Some("en-US");
const TEXT: &str = "hello from the fake engine";
const W1: WindowRef = WindowRef(0x0001_0012);
/// Upper bound on anything that waits for the session's threads.
const BUDGET: Duration = Duration::from_secs(10);
/// Where the capture stamps the first frame, after the press.
const FIRST_FRAME: Duration = Duration::from_millis(40);

// Fake endpoint ids shaped like WASAPI's; never a real device.
const USB_ID: &str = "{0.0.1.00000000}.{fake-usb-headset-0001}";
const USB_NAME: &str = "USB Headset (fake)";
const ARRAY_ID: &str = "{0.0.1.00000000}.{fake-mic-array-0002}";
const ARRAY_NAME: &str = "Microphone Array (fake)";
const DOCK_ID: &str = "{0.0.1.00000000}.{fake-dock-mic-0003}";
const DOCK_NAME: &str = "Dock Mic (fake)";

// ---- harness ----------------------------------------------------------------------

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// A caller instant one minute ahead of the real clock (see the header).
fn ahead() -> Instant {
    now() + Duration::from_secs(60)
}

fn id(s: &str) -> DeviceId {
    DeviceId(s.to_string())
}

fn device(s: &str, name: &str, is_default: bool) -> InputDevice {
    InputDevice {
        id: id(s),
        name: name.to_string(),
        is_default,
    }
}

/// The selected USB headset, plugged in, not the Windows default.
fn usb() -> InputDevice {
    device(USB_ID, USB_NAME, false)
}

/// The built-in array, the Windows default.
fn array_default() -> InputDevice {
    device(ARRAY_ID, ARRAY_NAME, true)
}

/// A dock microphone that became the Windows default.
fn dock_default() -> InputDevice {
    device(DOCK_ID, DOCK_NAME, true)
}

fn selected_usb() -> Option<Microphone> {
    Some(Microphone {
        id: USB_ID.to_string(),
        name: USB_NAME.to_string(),
    })
}

fn settings(mode: Mode, microphone: Option<Microphone>) -> Settings {
    let mut s = defaults(OS);
    s.engine = EngineKind::Api;
    s.api.base_url = "https://api.example.com/v1".to_string();
    s.speech_language = None;
    s.auto_paste = true;
    s.mode = mode;
    s.microphone = microphone;
    s
}

fn energy_gate() -> SpeechGate {
    SpeechGate::new(
        Ok(Box::new(EnergyDetector::new()) as Box<dyn SpeechDetector>),
        EnergyDetector::new(),
    )
}

fn eventually(cond: impl FnMut() -> bool) -> bool {
    common::timing::eventually(BUDGET, cond)
}

/// Five seconds of speech (80 000 samples at 16 kHz); premise checked (P-005).
fn speech_5s() -> AudioBuffer {
    let a = fixtures::speech(13, 5.0, 0.0, -12.0);
    assert!(EnergyDetector::new().detect(&a), "premise: speech");
    assert_eq!(a.samples().len(), 80_000, "premise: 5 s at 16 kHz");
    a
}

// ---- event and overlay views --------------------------------------------------------

/// The `device` param of every published `notice.mic_fallback` overlay, in order. A
/// notice with other params shows up as its params' debug text, so it cannot match.
fn fallback_notices(indicator: &FakeIndicator) -> Vec<String> {
    indicator
        .overlays()
        .into_iter()
        .filter_map(|o| match o {
            OverlayState::Message { id, params, .. } if id == i18n::NOTICE_MIC_FALLBACK => {
                match params.as_slice() {
                    [("device", name)] => Some(name.clone()),
                    other => Some(format!("unexpected params {other:?}")),
                }
            }
            _ => None,
        })
        .collect()
}

fn started_kinds(events: &[DictationEvent]) -> Vec<DeviceKind> {
    events
        .iter()
        .filter_map(|e| match e {
            DictationEvent::RecordingStarted { device, .. } => Some(*device),
            _ => None,
        })
        .collect()
}

fn recording_ends(events: &[DictationEvent]) -> Vec<(u64, RecordingEnd)> {
    events
        .iter()
        .filter_map(|e| match e {
            DictationEvent::RecordingEnded {
                duration_ms, end, ..
            } => Some((*duration_ms, *end)),
            _ => None,
        })
        .collect()
}

fn capture_failures(events: &[DictationEvent]) -> Vec<MicCause> {
    events
        .iter()
        .filter_map(|e| match e {
            DictationEvent::CaptureFailed { cause, .. } => Some(*cause),
            _ => None,
        })
        .collect()
}

fn job_count(events: &[DictationEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, DictationEvent::JobFinished { .. }))
        .count()
}

// ---- fake engine ----------------------------------------------------------------------

/// The sample count of every audio the fake engine transcribed, in call order.
#[derive(Default)]
struct EngineLog {
    transcribed: Mutex<Vec<usize>>,
}

struct FnEngine(Arc<EngineLog>);

impl Engine for FnEngine {
    fn kind(&self) -> &'static str {
        "fake"
    }
    fn transcribe(
        &self,
        audio: &AudioBuffer,
        _req: &TranscribeRequest,
    ) -> Result<String, FailureReason> {
        lock(&self.0.transcribed).push(audio.samples().len());
        Ok(TEXT.to_string())
    }
}

fn engine_factory(log: &Arc<EngineLog>) -> Box<EngineFactory> {
    let log = Arc::clone(log);
    Box::new(move |_s: &Settings, _c: &dyn CredentialStore| {
        Ok(Box::new(FnEngine(Arc::clone(&log))) as Box<dyn Engine>)
    })
}

// ---- the rig ------------------------------------------------------------------------

struct Rig {
    /// `None` only while the rig is dropped (the session is dropped within `BUDGET`).
    session: Option<Arc<DictationSession>>,
    audio: Arc<FakeAudioSource>,
    indicator: Arc<FakeIndicator>,
    clipboard: Arc<FakeClipboard>,
    observer: Arc<RecordingObserver>,
    settings: Arc<SettingsService>,
    engine: Arc<EngineLog>,
    cancel: Arc<FakeCancelKey>,
}

impl Rig {
    fn new(settings: Settings) -> Rig {
        Rig::with_source(settings, None)
    }

    /// A rig whose microphone is `source` instead of the rig's `FakeAudioSource`.
    fn with_source(settings: Settings, source: Option<Arc<dyn AudioSource>>) -> Rig {
        let creds = Arc::new(FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, KEY));
        let bytes = serde_json::to_vec(&settings).expect("serialize settings");
        let (service, outcome) = SettingsService::load_or_init(
            SettingsDeps {
                file: Arc::new(FakeSettingsFile::with_bytes(&bytes)),
                credentials: creds.clone(),
                autostart: Arc::new(FakeAutostart::new()),
                hotkeys: Arc::new(FakeHotkeyRegistrar::new()),
                local_models: Arc::new(FakeDownloadedModels::new(&["base"])),
                clock: Arc::new(FakeClock::at(
                    UNIX_EPOCH + Duration::from_secs(1_709_251_199),
                )),
            },
            OS,
        );
        assert!(
            matches!(&outcome, LoadOutcome::Loaded(s) if *s == settings),
            "premise: the settings load as given: {outcome:?}"
        );
        let settings = Arc::new(service);
        let audio = Arc::new(FakeAudioSource::new());
        let indicator = Arc::new(FakeIndicator::new());
        let clipboard = Arc::new(FakeClipboard::new());
        let paster = Arc::new(FakePaster::new().with_start_window(Some(StartWindow {
            handle: W1,
            process_id: 4242,
            elevated: false,
        })));
        let observer = Arc::new(RecordingObserver::new());
        let engine = Arc::new(EngineLog::default());
        let cancel = Arc::new(FakeCancelKey::new());
        let session = DictationSession::start(SessionDeps {
            pipeline: PipelineDeps {
                gate: energy_gate(),
                credentials: creds,
                clipboard: clipboard.clone(),
                paster,
                temp_audio: Arc::new(FakeTempAudioStore::new()),
                observer: observer.clone(),
                post_processor: Arc::new(PassThrough),
            },
            engine_factory: Some(engine_factory(&engine)),
            audio: match source {
                Some(s) => s,
                None => audio.clone(),
            },
            indicator: indicator.clone(),
            requests: Arc::new(FakeShellRequests::new()),
            settings: settings.clone(),
            cancel_key: cancel.clone(),
        })
        .expect("the session starts");
        Rig {
            session: Some(Arc::new(session)),
            audio,
            indicator,
            clipboard,
            observer,
            settings,
            engine,
            cancel,
        }
    }

    fn s(&self) -> &DictationSession {
        self.session.as_deref().expect("the session is on")
    }

    /// The session itself, for a call on another thread.
    fn shared(&self) -> Arc<DictationSession> {
        Arc::clone(self.session.as_ref().expect("the session is on"))
    }

    fn save(&self, edit: impl FnOnce(&mut Settings)) {
        let mut s = (*self.settings.snapshot()).clone();
        edit(&mut s);
        let outcome = self.settings.save(SaveRequest {
            settings: s,
            keys: KeyEdits::default(),
        });
        assert!(
            matches!(outcome, SaveOutcome::Saved { .. }),
            "save refused: {outcome:?}"
        );
    }

    /// The frames of every later capture: `audio`, first frame at `first_at`.
    fn frames(&self, audio: &AudioBuffer, first_at: Instant) {
        self.audio
            .set_chunks(vec![FrameChunk::from_buffer(audio, first_at)]);
    }

    /// One 1 s hold of 3 s of speech at `t`, then waits for its job (unless this
    /// press failed to capture, or left the device open).
    fn hold(&self, t: Instant) {
        self.frames(&fixtures::speech_3s(), t + FIRST_FRAME);
        let jobs = job_count(&self.events());
        let failures = capture_failures(&self.events()).len();
        self.s().hotkey_pressed(t);
        self.s().hotkey_released(t + ms(1000));
        if self.audio.open_handles() == 0 && capture_failures(&self.events()).len() == failures {
            self.wait_jobs(jobs + 1);
        }
    }

    fn events(&self) -> Vec<DictationEvent> {
        self.observer.events()
    }

    fn wait_jobs(&self, n: usize) {
        assert!(
            eventually(|| job_count(&self.events()) >= n),
            "{n} job(s) not ended within {BUDGET:?}: events {:?}, indicator {:?}",
            self.events(),
            self.indicator.calls()
        );
    }

    fn wait_ends(&self, n: usize) {
        assert!(
            eventually(|| recording_ends(&self.events()).len() >= n),
            "{n} RecordingEnded not within {BUDGET:?}: events {:?}",
            self.events()
        );
    }
}

/// Drops the session on another thread and fails if that takes longer than
/// `BUDGET` (a session whose shutdown waits for a loss route that never ends hangs
/// here instead of hanging the test binary).
impl Drop for Rig {
    fn drop(&mut self) {
        let Some(session) = self.session.take() else {
            return;
        };
        let (done, dropped) = mpsc::channel::<()>();
        let _ = thread::Builder::new()
            .name("rig-drop".to_string())
            .spawn(move || {
                drop(session);
                let _ = done.send(());
            });
        if dropped.recv_timeout(BUDGET).is_err() && !thread::panicking() {
            panic!("dropping the session did not return within {BUDGET:?}");
        }
    }
}

// ---- Acceptance 1: the selected microphone, the fallback and its notice -------------------

#[test]
fn the_selected_microphone_is_opened_by_its_id_with_no_notice() {
    // FR-27 / spec FR-030: the saved microphone (the USB headset, present, not the
    // Windows default) is the one opened, by its endpoint id, once per press; the
    // recording is Selected and no notice is raised. Bite: the session (or the
    // fake) still opening the default device (started_with = [array]), a notice on
    // every press, RecordingStarted.device hard-coded.
    let rig = Rig::new(settings(Mode::Hold, selected_usb()));
    rig.audio.set_devices(vec![array_default(), usb()]);
    rig.hold(ahead());

    assert_eq!(rig.audio.started_with(), vec![id(USB_ID)]);
    assert_eq!(fallback_notices(&rig.indicator), Vec::<String>::new());
    assert_eq!(started_kinds(&rig.events()), vec![DeviceKind::Selected]);
    assert_eq!(rig.clipboard.texts(), vec![TEXT.to_string()]);
}

#[test]
fn selected_microphone_missing_falls_back_to_the_default_with_one_notice_naming_it() {
    // T-012 Acceptance 1 (FR-27, spec FR-030): the saved USB headset is not in the
    // device list; the press opens the Windows default device by its id, with one
    // `start` call (no try of the missing id first), raises notice.mic_fallback once
    // with device = the default's display name, records as Fallback and the dictation
    // still goes through. Bite: start with the saved id (the adapter deciding),
    // no notice, a notice naming the saved device or carrying the id, two notices,
    // RecordingStarted.device Selected, the press failing as NoDevice.
    let rig = Rig::new(settings(Mode::Hold, selected_usb()));
    rig.audio.set_devices(vec![array_default()]);
    rig.hold(ahead());

    assert_eq!(rig.audio.started_with(), vec![id(ARRAY_ID)]);
    assert_eq!(
        fallback_notices(&rig.indicator),
        vec![ARRAY_NAME.to_string()]
    );
    assert_eq!(started_kinds(&rig.events()), vec![DeviceKind::Fallback]);
    assert_eq!(capture_failures(&rig.events()), vec![]);
    assert_eq!(rig.clipboard.texts(), vec![TEXT.to_string()]);
    assert_eq!(rig.audio.open_handles(), 0);
}

#[test]
fn the_same_fallback_again_raises_no_second_notice() {
    // FR-27 "once per device change": three presses on the same fallback device
    // give one notice in all, and every one of them records as Fallback. Bite: a
    // notice per fallback press, the memory reset by every press.
    let rig = Rig::new(settings(Mode::Hold, selected_usb()));
    rig.audio.set_devices(vec![array_default()]);
    let t = ahead();
    for i in 0..3u64 {
        rig.hold(t + ms(5000 * i));
    }

    assert_eq!(rig.audio.started_with(), vec![id(ARRAY_ID); 3]);
    assert_eq!(
        fallback_notices(&rig.indicator),
        vec![ARRAY_NAME.to_string()]
    );
    assert_eq!(started_kinds(&rig.events()), vec![DeviceKind::Fallback; 3]);
}

#[test]
fn a_different_fallback_device_raises_a_new_notice_naming_it() {
    // FR-27: the fallback device changed (the dock's microphone became the Windows
    // default while the headset is still missing): a second notice naming the new
    // device. Bite: a memory of "on the fallback" only (no second notice), the old
    // device's name, the default read once at start instead of at each press.
    let rig = Rig::new(settings(Mode::Hold, selected_usb()));
    rig.audio.set_devices(vec![array_default()]);
    let t = ahead();
    rig.hold(t);
    rig.audio
        .set_devices(vec![device(ARRAY_ID, ARRAY_NAME, false), dock_default()]);
    rig.hold(t + ms(5000));

    assert_eq!(rig.audio.started_with(), vec![id(ARRAY_ID), id(DOCK_ID)]);
    assert_eq!(
        fallback_notices(&rig.indicator),
        vec![ARRAY_NAME.to_string(), DOCK_NAME.to_string()]
    );
}

#[test]
fn the_selected_microphone_back_is_used_without_notice_and_a_later_fallback_notifies_again() {
    // FR-27 / spec US6 "until the selected one returns": headset missing (fallback,
    // one notice), plugged back in (opened by its id, Selected, no notice), unplugged
    // again (fallback, a new notice). Bite: a press that keeps using the fallback
    // after the headset returned, a notice when the selected one returns, a memory
    // that never clears (no third notice).
    let rig = Rig::new(settings(Mode::Hold, selected_usb()));
    let t = ahead();
    rig.audio.set_devices(vec![array_default()]);
    rig.hold(t);
    rig.audio.set_devices(vec![usb(), array_default()]);
    rig.hold(t + ms(5000));
    rig.audio.set_devices(vec![array_default()]);
    rig.hold(t + ms(10_000));

    assert_eq!(
        rig.audio.started_with(),
        vec![id(ARRAY_ID), id(USB_ID), id(ARRAY_ID)]
    );
    assert_eq!(
        started_kinds(&rig.events()),
        vec![
            DeviceKind::Fallback,
            DeviceKind::Selected,
            DeviceKind::Fallback
        ]
    );
    assert_eq!(
        fallback_notices(&rig.indicator),
        vec![ARRAY_NAME.to_string(), ARRAY_NAME.to_string()]
    );
}

#[test]
fn no_microphone_selected_records_from_the_default_as_selected_without_notice() {
    // data-model `MicrophoneChoice`: nothing selected means the Windows default,
    // which is the user's choice, not a fallback: no notice, device Selected. Bite:
    // Fallback (and a notice) whenever no microphone is saved.
    let rig = Rig::new(settings(Mode::Hold, None));
    rig.audio.set_devices(vec![usb(), array_default()]);
    rig.hold(ahead());

    assert_eq!(rig.audio.started_with(), vec![id(ARRAY_ID)]);
    assert_eq!(started_kinds(&rig.events()), vec![DeviceKind::Selected]);
    assert_eq!(fallback_notices(&rig.indicator), Vec::<String>::new());
}

#[test]
fn the_selection_is_read_from_the_settings_at_each_press() {
    // P-013: the saved microphone is read from the press's snapshot: a press with
    // nothing saved records from the default (Selected), a press after the user
    // saved the headset records from it. Bite: the selection read once at start.
    let rig = Rig::new(settings(Mode::Hold, None));
    rig.audio.set_devices(vec![usb(), array_default()]);
    let t = ahead();
    rig.hold(t);
    rig.save(|s| s.microphone = selected_usb());
    rig.hold(t + ms(5000));

    assert_eq!(rig.audio.started_with(), vec![id(ARRAY_ID), id(USB_ID)]);
    assert_eq!(fallback_notices(&rig.indicator), Vec::<String>::new());
}

#[test]
fn a_fallback_that_fails_to_open_notifies_on_the_next_one_that_opens() {
    // T-012 invariant (2): the notice is raised only for a press whose capture
    // opened. The first fallback press fails to open (busy): "microphone
    // unavailable" and no fallback notice; the next fallback press opens and
    // notifies once. Bite: the tracker updated before `start` (the second press
    // stays silent), a notice shown for a press that recorded nothing.
    let rig = Rig::new(settings(Mode::Hold, selected_usb()));
    rig.audio.set_devices(vec![array_default()]);
    rig.audio.set_start_error(Some(CaptureError::DeviceBusy));
    let t = ahead();
    rig.s().hotkey_pressed(t);
    rig.s().hotkey_released(t + ms(1000));
    assert_eq!(fallback_notices(&rig.indicator), Vec::<String>::new());
    assert_eq!(capture_failures(&rig.events()), vec![MicCause::Busy]);

    rig.audio.set_start_error(None);
    rig.hold(t + ms(5000));
    assert_eq!(rig.audio.started_with(), vec![id(ARRAY_ID), id(ARRAY_ID)]);
    assert_eq!(
        fallback_notices(&rig.indicator),
        vec![ARRAY_NAME.to_string()]
    );
    assert_eq!(rig.clipboard.texts(), vec![TEXT.to_string()]);
}

// ---- Acceptance 2, failure branch: no input device at all (FR-04) ---------------------------

#[test]
fn no_input_device_at_all_is_the_microphone_unavailable_branch_with_no_recording_state() {
    // T-012 Acceptance 2 / FR-04 failure branch (spec "no device at all"): an empty
    // device list, with a saved microphone and without one. No `start` call, one
    // CaptureFailed{NoDevice}, the tray Error and the overlay "microphone
    // unavailable: no input device", never a Recording state, no Esc claim, no
    // job, no fallback notice. Bite: `start` called anyway (with an empty or the
    // saved id), the press dropped silently (no failure shown), a Recording flash,
    // a job queued for nothing.
    for microphone in [selected_usb(), None] {
        let rig = Rig::new(settings(Mode::Hold, microphone.clone()));
        rig.audio.set_devices(vec![]);
        let t = ahead();
        rig.s().hotkey_pressed(t);
        rig.s().hotkey_released(t + ms(1000));

        let what = format!("selected {microphone:?}");
        assert_eq!(rig.audio.start_calls(), 0, "{what}: start was called");
        assert_eq!(
            capture_failures(&rig.events()),
            vec![MicCause::NoDevice],
            "{what}"
        );
        let calls = rig.indicator.calls();
        assert!(
            !calls.iter().any(|c| matches!(
                c,
                IndicatorCall::Tray(TrayState::Recording, _)
                    | IndicatorCall::Overlay(OverlayState::Recording)
            )),
            "{what}: a Recording state was published: {calls:?}"
        );
        assert!(
            rig.indicator.trays().contains(&(TrayState::Error, false)),
            "{what}: no tray Error: {calls:?}"
        );
        assert!(
            rig.indicator.overlays().iter().any(|o| matches!(o,
                OverlayState::Message { id, params, .. }
                    if *id == i18n::FAILURE_MICROPHONE_UNAVAILABLE
                        && *params == vec![("reason", "mic_reason.no_device".to_string())])),
            "{what}: no 'microphone unavailable: no input device': {calls:?}"
        );
        assert_eq!(
            rig.cancel.calls(),
            Vec::<bool>::new(),
            "{what}: Esc claimed"
        );
        assert_eq!(recording_ends(&rig.events()), vec![], "{what}");
        assert_eq!(job_count(&rig.events()), 0, "{what}");
        assert_eq!(fallback_notices(&rig.indicator), Vec::<String>::new());
    }
}

#[test]
fn a_device_list_that_cannot_be_read_fails_the_press_with_its_cause() {
    // Failure branch of the new port call: `devices()` itself fails (access
    // denied by Windows privacy settings): the press ends in the same FR-04 branch
    // with that cause, without a `start` call and without a recording state. Bite:
    // an enumeration error read as "no devices" (NoDevice), `start` called on a
    // guessed device, a panic on the error.
    let rig = Rig::new(settings(Mode::Hold, selected_usb()));
    rig.audio.set_devices(vec![array_default()]);
    rig.audio
        .set_devices_error(Some(CaptureError::AccessDenied));
    let t = ahead();
    rig.s().hotkey_pressed(t);
    rig.s().hotkey_released(t + ms(1000));

    assert_eq!(rig.audio.start_calls(), 0);
    assert_eq!(
        capture_failures(&rig.events()),
        vec![MicCause::AccessDenied]
    );
    assert!(!rig.indicator.overlays().contains(&OverlayState::Recording));
    assert_eq!(job_count(&rig.events()), 0);
}

// ---- Acceptance 2, failure branch: the microphone lost mid-recording ------------------------

#[test]
fn device_lost_at_second_5_of_a_toggle_recording_processes_the_5_s_captured() {
    // T-012 Acceptance 2 (spec FR-031, "mic unplugged at second 5 -> 5 s
    // processed"): a toggle recording (no release can end it) whose capture
    // delivers 5 s of speech and then reports the device lost at press + 5 s. With
    // no further input: the mic is closed, RecordingEnded (5000, DeviceLost), one
    // job over exactly the 5 s captured, the text pasted, Esc released; the next
    // press starts a new recording (it is not taken as the toggle's stop). Bite: the
    // loss ignored (the recording stays on until the 10-min stop: no job), the audio
    // dropped (a cancel), end Toggled/Released, the mic left open, the session's
    // controller left live (the next press stops instead of starting).
    let rig = Rig::new(settings(Mode::Toggle, selected_usb()));
    rig.audio.set_devices(vec![usb(), array_default()]);
    let t = ahead();
    rig.frames(&speech_5s(), t + FIRST_FRAME);
    rig.audio.set_device_loss(Some(t + ms(5000)));
    rig.s().hotkey_pressed(t);

    rig.wait_jobs(1);
    assert_eq!(
        recording_ends(&rig.events()),
        vec![(5000, RecordingEnd::DeviceLost)]
    );
    assert_eq!(lock(&rig.engine.transcribed).clone(), vec![80_000]);
    assert_eq!(rig.clipboard.texts(), vec![TEXT.to_string()]);
    assert_eq!(rig.audio.open_handles(), 0, "the lost device is still open");
    assert_eq!(rig.cancel.calls(), vec![true, false]);
    assert_eq!(capture_failures(&rig.events()), vec![]);

    rig.audio.set_device_loss(None);
    rig.s().hotkey_pressed(t + ms(8000));
    assert_eq!(rig.audio.start_calls(), 2, "the next press did not start");
    assert_eq!(rig.audio.open_handles(), 1);
    rig.s().hotkey_pressed(t + ms(11_000));
    rig.wait_jobs(2);
    assert_eq!(
        recording_ends(&rig.events()),
        vec![
            (5000, RecordingEnd::DeviceLost),
            (3000, RecordingEnd::Toggled)
        ]
    );
}

#[test]
fn device_lost_mid_hold_ends_it_and_the_later_release_does_nothing() {
    // Spec FR-031 in hold mode: the loss at press + 2 s ends the recording through
    // the stop path (2000, DeviceLost) and its audio is processed; the user's
    // release at + 3 s then finds no recording (no event, no second job). Bite: a
    // loss that only closes the device and waits for the release (end Released,
    // 3000), the release starting a second stop.
    let rig = Rig::new(settings(Mode::Hold, selected_usb()));
    rig.audio.set_devices(vec![usb(), array_default()]);
    let t = ahead();
    rig.frames(&fixtures::speech_3s(), t + FIRST_FRAME);
    rig.audio.set_device_loss(Some(t + ms(2000)));
    rig.s().hotkey_pressed(t);
    rig.wait_ends(1);
    rig.wait_jobs(1);
    let before = rig.events().len();
    rig.s().hotkey_released(t + ms(3000));

    assert_eq!(
        recording_ends(&rig.events()),
        vec![(2000, RecordingEnd::DeviceLost)]
    );
    assert_eq!(
        rig.events().len(),
        before,
        "the release after the loss emitted"
    );
    assert_eq!(job_count(&rig.events()), 1);
    assert_eq!(rig.clipboard.texts(), vec![TEXT.to_string()]);
    assert_eq!(rig.audio.open_handles(), 0);
}

// ---- device loss races (a source that keeps its sinks) ------------------------------------

/// A microphone that keeps every sink it was given (an adapter may hold one past
/// its handle), delivers no frames, and can report a loss from inside `start`
/// (the session lock is held there) or from its handle's drop (the session drops
/// a live handle under its lock at shutdown).
struct KeptSinks {
    sinks: Mutex<Vec<Arc<dyn FrameSink>>>,
    open: Arc<AtomicUsize>,
    lose_in_start: AtomicBool,
    lose_on_drop: AtomicBool,
    /// The instant every loss is stamped with.
    loss_at: Instant,
}

impl KeptSinks {
    fn new(loss_at: Instant) -> Arc<KeptSinks> {
        Arc::new(KeptSinks {
            sinks: Mutex::new(Vec::new()),
            open: Arc::new(AtomicUsize::new(0)),
            lose_in_start: AtomicBool::new(false),
            lose_on_drop: AtomicBool::new(false),
            loss_at,
        })
    }

    fn sink(&self, i: usize) -> Arc<dyn FrameSink> {
        lock(&self.sinks)
            .get(i)
            .cloned()
            .unwrap_or_else(|| panic!("no capture #{i} was started"))
    }

    fn open(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }
}

struct KeptCapture {
    open: Arc<AtomicUsize>,
    lose: Option<(Arc<dyn FrameSink>, Instant)>,
}

impl CaptureHandle for KeptCapture {
    fn stop(self: Box<Self>) -> Result<(), CaptureError> {
        Ok(())
    }
}

impl Drop for KeptCapture {
    fn drop(&mut self) {
        if let Some((sink, at)) = self.lose.take() {
            sink.device_lost(at);
        }
        self.open.fetch_sub(1, Ordering::SeqCst);
    }
}

impl AudioSource for KeptSinks {
    fn devices(&self) -> Result<Vec<InputDevice>, CaptureError> {
        Ok(vec![array_default()])
    }

    fn start(
        &self,
        device: &DeviceId,
        sink: Arc<dyn FrameSink>,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        assert_eq!(device, &id(ARRAY_ID), "opened a device not in the list");
        if self.lose_in_start.load(Ordering::SeqCst) {
            sink.device_lost(self.loss_at);
        }
        lock(&self.sinks).push(Arc::clone(&sink));
        self.open.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(KeptCapture {
            open: Arc::clone(&self.open),
            lose: self
                .lose_on_drop
                .load(Ordering::SeqCst)
                .then_some((sink, self.loss_at)),
        }))
    }
}

/// Runs `f` on another thread and fails if it does not return within `BUDGET` (a
/// deadlock on the session lock fails instead of hanging).
fn returns_within_budget(what: &str, f: impl FnOnce() + Send + 'static) {
    let (done, returned) = mpsc::channel::<()>();
    let _ = thread::Builder::new()
        .name("bounded-call".to_string())
        .spawn(move || {
            f();
            let _ = done.send(());
        });
    assert!(
        returned.recv_timeout(BUDGET).is_ok(),
        "{what} did not return within {BUDGET:?} (a device loss waiting for the session lock?)"
    );
}

#[test]
fn a_stale_device_lost_does_not_end_the_next_recording() {
    // T-012 invariant (3), "stale ids ignored": the sink of an ended recording
    // reports a loss while the next recording is on; the next recording goes on
    // (device open, no RecordingEnded) and ends at its own release. Bite: a loss
    // route that ends whatever is live, a sink that does not know its recording.
    let t = ahead();
    let source = KeptSinks::new(t + ms(6000));
    let rig = Rig::with_source(settings(Mode::Hold, None), Some(source.clone()));
    rig.s().hotkey_pressed(t);
    rig.s().hotkey_released(t + ms(1000));
    rig.wait_jobs(1);
    rig.s().hotkey_pressed(t + ms(5000));
    assert_eq!(source.open(), 1, "premise: the second recording is on");

    source.sink(0).device_lost(t + ms(6000));
    // The loss route is asynchronous: give it the time it would need, then check
    // that it did nothing (the verdict is the state, not the wait).
    let _ = eventually(|| recording_ends(&rig.events()).len() > 1);
    assert_eq!(
        recording_ends(&rig.events()).len(),
        1,
        "the stale loss ended the second recording: {:?}",
        rig.events()
    );
    assert_eq!(source.open(), 1, "the stale loss closed the second device");

    rig.s().hotkey_released(t + ms(7000));
    rig.wait_jobs(2);
    assert_eq!(
        recording_ends(&rig.events())
            .into_iter()
            .map(|(_, end)| end)
            .collect::<Vec<_>>(),
        vec![RecordingEnd::Released, RecordingEnd::Released]
    );
}

#[test]
fn a_device_lost_reported_twice_ends_the_recording_once() {
    // T-012 invariant (3), "one FrameSink::device_lost": a capture whose error
    // callback fires twice (two errors before its thread ends) ends the recording
    // once: one RecordingEnded{DeviceLost}, one job. Bite: every loss call queued
    // as a stop (a second end, or a later recording stopped by the duplicate).
    let t = ahead();
    let source = KeptSinks::new(t + ms(2000));
    let rig = Rig::with_source(settings(Mode::Toggle, None), Some(source.clone()));
    rig.s().hotkey_pressed(t);
    source.sink(0).device_lost(t + ms(2000));
    source.sink(0).device_lost(t + ms(2001));
    rig.wait_jobs(1);
    rig.s().hotkey_pressed(t + ms(5000));
    assert_eq!(source.open(), 1, "premise: the next recording is on");
    let _ = eventually(|| recording_ends(&rig.events()).len() > 1);

    assert_eq!(
        recording_ends(&rig.events()),
        vec![(2000, RecordingEnd::DeviceLost)],
        "the duplicate loss ended something else"
    );
    assert_eq!(job_count(&rig.events()), 1);
    assert_eq!(source.open(), 1);
}

#[test]
fn a_device_lost_inside_start_under_the_session_lock_does_not_deadlock() {
    // Lock contract (platform.rs, docs/decisions/windows-shell.md invariant 2;
    // T-012 invariant (3) "never under the session lock"): the device dies while it
    // opens, so the loss is reported from inside `start`, where the session holds
    // its lock. The press returns, and the recording then ends through the stop
    // path (DeviceLost) with its device closed. Bite: a `device_lost` that takes the
    // session lock (the press never returns), a loss dropped because it came before
    // the capture was stored (the toggle recording stays on).
    let t = ahead();
    let source = KeptSinks::new(t + ms(10));
    source.lose_in_start.store(true, Ordering::SeqCst);
    let rig = Rig::with_source(settings(Mode::Toggle, None), Some(source.clone()));
    let session = rig.shared();
    returns_within_budget("the press", move || session.hotkey_pressed(t));

    rig.wait_ends(1);
    assert_eq!(
        recording_ends(&rig.events()),
        vec![(10, RecordingEnd::DeviceLost)]
    );
    assert!(
        eventually(|| source.open() == 0),
        "the lost device stayed open"
    );
}

#[test]
fn a_device_lost_from_a_handle_dropped_at_shutdown_does_not_deadlock() {
    // Lock contract (platform.rs `CaptureHandle`): the session drops a live handle
    // under its lock at shutdown, and dropping a cpal stream joins the thread that
    // runs the error callback, which reports the loss. Dropping the session while a
    // recording is on must return. Also: the session's loss route must end at
    // shutdown even though the adapter still holds the sinks (this source keeps
    // them). Bite: a `device_lost` that takes the session lock, a loss thread joined
    // at shutdown that waits for every sink to be dropped.
    let t = ahead();
    let source = KeptSinks::new(t + ms(500));
    source.lose_on_drop.store(true, Ordering::SeqCst);
    let mut rig = Rig::with_source(settings(Mode::Toggle, None), Some(source.clone()));
    rig.s().hotkey_pressed(t);
    assert_eq!(source.open(), 1, "premise: the recording is on");

    let session = rig.session.take().expect("the session is on");
    returns_within_budget("dropping the session", move || drop(session));
    assert_eq!(source.open(), 0, "the device is still open after shutdown");
}
