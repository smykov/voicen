//! T-051: the platform-free dictation session (`voicen_core::dictation`) over fakes
//! (T-051 Acceptance 1-3; Investigation red-test table, rows 1-19).
//!
//! Every session is built from the real `SettingsService` (fake file, fake
//! credential store holding the key), `FakeAudioSource`, a fake engine factory
//! (`SessionDeps::engine_factory`: no network, F-004/F-005), `FakeClipboard` /
//! `FakePaster` (a non-elevated start window), `FakeIndicator`,
//! `FakeShellRequests` and one `RecordingObserver`.
//!
//! Instants: deterministic tests stamp the caller's events in the past (`past()`),
//! so they stay ordered against the worker's real job-end instants. A message
//! raised at such an instant has already expired, so those tests match messages
//! without their `until`, or raise the message at `Instant::now()` and allow the
//! timer's expiry only once its 3 s have passed (`settled`). The real-time tests
//! (message expiry, timer re-arm, `RealtimeSource`) check recorded instants
//! against budgets sized for windows-latest (timer resolution ~15.6 ms, slower
//! thread start), never an exact deadline. Everything that waits on the session's
//! threads is bounded by `BUDGET`, so a broken session fails instead of hanging.
//! Fake data only: example.com, `sk-test-SECRET`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant, UNIX_EPOCH};

use voicen_core::audio::{wav, AudioBuffer};
use voicen_core::autostart::FakeAutostart;
use voicen_core::clock::FakeClock;
use voicen_core::delivery::{DeliveryResult, MODIFIER_WAIT};
use voicen_core::dictation::{DictationSession, SessionDeps};
use voicen_core::engine::{Engine, TranscribeRequest};
use voicen_core::events::{DeviceKind, DictationEvent, OutcomeCode, RecordingObserver};
use voicen_core::failure::FailureReason;
use voicen_core::hotkey_registrar::FakeHotkeyRegistrar;
use voicen_core::i18n;
use voicen_core::models::FakeDownloadedModels;
use voicen_core::pipeline::{EngineFactory, PipelineDeps};
use voicen_core::platform::{
    AudioSource, Clipboard, ClipboardError, FakeAudioSource, FakeClipboard, FakeIndicator,
    FakePaster, FakeShellRequests, FakeTempAudioStore, FrameChunk, FrameSink, Indicator,
    IndicatorCall, PasteError, Paster, PasterCall, PendingId, ShellRequestCall, ShellRequests,
    StartWindow, TempAudioStore, WindowRef,
};
use voicen_core::post_process::PassThrough;
use voicen_core::recording::{
    CaptureError, MicCause, OverlayState, RecordingEnd, TrayState, MESSAGE_DURATION,
};
use voicen_core::secrets::{CredentialStore, FakeCredentialStore, KeyEdits, KeySlot};
use voicen_core::settings::file::FakeSettingsFile;
use voicen_core::settings::gate::SettingsTab;
use voicen_core::settings::service::{SaveOutcome, SaveRequest, SettingsDeps, SettingsService};
use voicen_core::settings::{defaults, EngineKind, LoadOutcome, Mode, Settings};
use voicen_core::test_support::fixtures;
use voicen_core::test_support::realtime::{read_wav, RealtimeSource};
use voicen_core::vad::{EnergyDetector, SpeechDetector, SpeechGate};

const KEY: &str = "sk-test-SECRET";
const OS: Option<&str> = Some("en-US");
const TEXT: &str = "hello from the fake engine";
const W1: WindowRef = WindowRef(0x0001_0051);
const W2: WindowRef = WindowRef(0x0001_0052);
/// Upper bound on anything that waits for the session's threads.
const BUDGET: Duration = Duration::from_secs(10);
/// How late the timer may publish an expiry on windows-latest.
const EXPIRY_BUDGET: Duration = Duration::from_secs(2);
/// Where the fake capture stamps the first frame, after the press.
const FIRST_FRAME: Duration = Duration::from_millis(40);

// ---- harness ------------------------------------------------------------------

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// A caller instant one minute ago: later test instants (up to ~30 s on) stay
/// before the worker's real `Instant::now()`.
fn past() -> Instant {
    Instant::now()
        .checked_sub(Duration::from_secs(60))
        .expect("monotonic clock at least 60 s past its origin")
}

fn window(handle: WindowRef) -> StartWindow {
    StartWindow {
        handle,
        process_id: 4242,
        elevated: false,
    }
}

fn api_settings() -> Settings {
    let mut s = defaults(OS);
    s.engine = EngineKind::Api;
    s.api.base_url = "https://api.example.com/v1".to_string();
    s.speech_language = None;
    s.auto_paste = true;
    s
}

fn energy_gate() -> SpeechGate {
    SpeechGate::new(
        Ok(Box::new(EnergyDetector::new()) as Box<dyn SpeechDetector>),
        EnergyDetector::new(),
    )
}

/// Polls `cond` every 5 ms until it holds or `BUDGET` has passed.
fn eventually(mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + BUDGET;
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(ms(5));
    }
}

/// `calls` without a trailing timer expiry (`hidden`) once the message raised at
/// `raised` may have expired, so an immediate check does not race the timer on a
/// slow machine. Before its 3 s have passed an expiry cannot be there.
fn settled<T: PartialEq>(mut calls: Vec<T>, hidden: T, raised: Instant) -> Vec<T> {
    if raised.elapsed() >= MESSAGE_DURATION && calls.last() == Some(&hidden) {
        calls.pop();
    }
    calls
}

/// The "microphone unavailable" overlay with the catalog id of its reason
/// (contracts/messages.md), spelled here, not derived from the code under test.
fn mic_message(reason: &str, until: Instant) -> OverlayState {
    OverlayState::Message {
        id: i18n::FAILURE_MICROPHONE_UNAVAILABLE,
        params: vec![("reason", reason.to_string())],
        until,
    }
}

/// The same message at any `until` (raised at a past instant, it may be expired).
fn is_mic_message(o: &OverlayState, reason: &str) -> bool {
    matches!(o, OverlayState::Message { id, params, .. }
        if *id == i18n::FAILURE_MICROPHONE_UNAVAILABLE
            && *params == vec![("reason", reason.to_string())])
}

fn job_count(events: &[DictationEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, DictationEvent::JobFinished { .. }))
        .count()
}

/// `(duration_ms, end)` of every `RecordingEnded`.
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

fn delivery_events(events: &[DictationEvent]) -> Vec<DeliveryResult> {
    events
        .iter()
        .filter_map(|e| match e {
            DictationEvent::Delivered { result, .. } => Some(*result),
            _ => None,
        })
        .collect()
}

fn pasted_into(w: WindowRef) -> Vec<PasterCall> {
    vec![
        PasterCall::CaptureStartWindow,
        PasterCall::WaitModifiersReleased(MODIFIER_WAIT),
        PasterCall::IsInFront(w),
        PasterCall::SendCtrlV,
    ]
}

// ---- fake engine ----------------------------------------------------------------

type Reply = Box<dyn Fn(&AudioBuffer) -> Result<String, FailureReason> + Send + Sync>;

/// What the fake factory and its engines saw.
#[derive(Default)]
struct EngineLog {
    factory_calls: AtomicUsize,
    settings: Mutex<Vec<Settings>>,
    /// The sample count of each audio `transcribe` got, in call order.
    transcribed: Mutex<Vec<usize>>,
}

impl EngineLog {
    fn factory_calls(&self) -> usize {
        self.factory_calls.load(Ordering::SeqCst)
    }
    fn settings(&self) -> Vec<Settings> {
        lock(&self.settings).clone()
    }
    fn transcribed(&self) -> Vec<usize> {
        lock(&self.transcribed).clone()
    }
}

struct FnEngine {
    reply: Arc<Reply>,
    log: Arc<EngineLog>,
}

impl Engine for FnEngine {
    fn kind(&self) -> &'static str {
        "fake"
    }
    fn transcribe(
        &self,
        audio: &AudioBuffer,
        _req: &TranscribeRequest,
    ) -> Result<String, FailureReason> {
        lock(&self.log.transcribed).push(audio.samples().len());
        (self.reply)(audio)
    }
}

fn engine_factory(log: &Arc<EngineLog>, reply: Reply) -> Box<EngineFactory> {
    let log = Arc::clone(log);
    let reply = Arc::new(reply);
    Box::new(move |settings: &Settings, _creds: &dyn CredentialStore| {
        log.factory_calls.fetch_add(1, Ordering::SeqCst);
        lock(&log.settings).push(settings.clone());
        Ok(Box::new(FnEngine {
            reply: Arc::clone(&reply),
            log: Arc::clone(&log),
        }) as Box<dyn Engine>)
    })
}

fn always(text: &str) -> Reply {
    let text = text.to_string();
    Box::new(move |_| Ok(text.clone()))
}

/// One reply per `transcribe` call, in order (the last one repeats).
fn in_order(replies: Vec<Result<String, FailureReason>>) -> Reply {
    let queue = Mutex::new(VecDeque::from(replies));
    Box::new(move |_| {
        let mut q = lock(&queue);
        if q.len() > 1 {
            q.pop_front().expect("non-empty")
        } else {
            q.front().cloned().expect("at least one reply")
        }
    })
}

/// Replies by audio length (`texts`); the job whose audio has `slow_len` samples
/// signals `entered` and then blocks until `go` (bounded by `BUDGET`).
struct Gated {
    reply: Reply,
    entered: mpsc::Receiver<()>,
    go: mpsc::Sender<()>,
}

fn gated(slow_len: usize, texts: &[(usize, &str)]) -> Gated {
    let (entered_tx, entered) = mpsc::channel::<()>();
    let (go, go_rx) = mpsc::channel::<()>();
    let entered_tx = Mutex::new(entered_tx);
    let go_rx = Mutex::new(go_rx);
    let texts: Vec<(usize, String)> = texts.iter().map(|(n, t)| (*n, t.to_string())).collect();
    let reply: Reply = Box::new(move |audio: &AudioBuffer| {
        let n = audio.samples().len();
        if n == slow_len {
            let _ = lock(&entered_tx).send(());
            let _ = lock(&go_rx).recv_timeout(BUDGET);
        }
        Ok(texts
            .iter()
            .find(|(len, _)| *len == n)
            .map(|(_, t)| t.clone())
            .unwrap_or_else(|| format!("unexpected audio of {n} samples")))
    });
    Gated { reply, entered, go }
}

/// Three speech recordings of distinct lengths (3 s, 2 s, 1.5 s), so the engine can
/// tell them apart; each premise checked (P-005).
fn three_recordings() -> [AudioBuffer; 3] {
    let all = [
        fixtures::speech_3s(),
        fixtures::speech(11, 2.0, 0.0, -12.0),
        fixtures::speech(12, 1.5, 0.0, -12.0),
    ];
    for (i, a) in all.iter().enumerate() {
        assert!(
            EnergyDetector::new().detect(a),
            "premise: recording {i} is speech"
        );
    }
    assert_eq!(
        all.iter().map(|a| a.samples().len()).collect::<Vec<_>>(),
        vec![48_000, 32_000, 24_000],
        "premise: distinct lengths"
    );
    all
}

// ---- instrumented ports -----------------------------------------------------------

/// Clipboard and paster that count deliveries in flight (from the clipboard write
/// to Ctrl+V) and the threads they ran on. `wait_modifiers_released` sleeps 30 ms,
/// so two deliveries that run at once overlap inside it.
#[derive(Default)]
struct Instrumented {
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    texts: Mutex<Vec<String>>,
    threads: Mutex<Vec<ThreadId>>,
    pastes: AtomicUsize,
}

impl Instrumented {
    fn texts(&self) -> Vec<String> {
        lock(&self.texts).clone()
    }
    fn threads(&self) -> Vec<ThreadId> {
        lock(&self.threads).clone()
    }
}

impl Clipboard for Instrumented {
    fn set_text_excluded_from_history(&self, text: &str) -> Result<(), ClipboardError> {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        lock(&self.texts).push(text.to_string());
        lock(&self.threads).push(thread::current().id());
        Ok(())
    }
}

impl Paster for Instrumented {
    fn capture_start_window(&self) -> Option<StartWindow> {
        Some(window(W1))
    }
    fn wait_modifiers_released(&self, _max_wait: Duration) -> bool {
        thread::sleep(ms(30));
        true
    }
    fn is_in_front(&self, _w: &StartWindow) -> bool {
        true
    }
    fn send_ctrl_v(&self) -> Result<(), PasteError> {
        self.pastes.fetch_add(1, Ordering::SeqCst);
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }
}

/// The indicator and the shell requests in one log, for the order of the blocked
/// actions.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Tray(TrayState, bool),
    Overlay(OverlayState),
    OpenSettings(SettingsTab),
}

#[derive(Default)]
struct Sequence {
    steps: Mutex<Vec<Step>>,
}

impl Indicator for Sequence {
    fn set_tray(&self, state: TrayState, retry_available: bool) {
        lock(&self.steps).push(Step::Tray(state, retry_available));
    }
    fn set_overlay(&self, state: &OverlayState) {
        lock(&self.steps).push(Step::Overlay(state.clone()));
    }
}

impl ShellRequests for Sequence {
    fn open_settings(&self, tab: SettingsTab) {
        lock(&self.steps).push(Step::OpenSettings(tab));
    }
}

/// A pending-audio store whose `put_pending` signals `entered` and then blocks until
/// `go` (bounded by `BUDGET`): the pipeline holds its pending-slot lock meanwhile
/// (`Pipeline::keep_pending`).
struct BlockingStore {
    inner: FakeTempAudioStore,
    entered: Mutex<mpsc::Sender<()>>,
    go: Mutex<mpsc::Receiver<()>>,
}

impl BlockingStore {
    fn new() -> (Arc<BlockingStore>, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered_tx, entered) = mpsc::channel();
        let (go, go_rx) = mpsc::channel();
        let store = Arc::new(BlockingStore {
            inner: FakeTempAudioStore::new(),
            entered: Mutex::new(entered_tx),
            go: Mutex::new(go_rx),
        });
        (store, entered, go)
    }
}

impl TempAudioStore for BlockingStore {
    fn put_pending(&self, id: PendingId, audio: &AudioBuffer) -> std::io::Result<()> {
        let _ = lock(&self.entered).send(());
        let _ = lock(&self.go).recv_timeout(BUDGET);
        self.inner.put_pending(id, audio)
    }
    fn get_pending(&self, id: PendingId) -> std::io::Result<AudioBuffer> {
        self.inner.get_pending(id)
    }
    fn delete_pending(&self, id: PendingId) -> std::io::Result<()> {
        self.inner.delete_pending(id)
    }
    fn delete_all(&self) -> std::io::Result<()> {
        self.inner.delete_all()
    }
}

// ---- the rig ---------------------------------------------------------------------

/// Ports a test replaces in the default rig.
#[derive(Default)]
struct Custom {
    /// Clipboard and paster.
    output: Option<Arc<Instrumented>>,
    /// Indicator and shell requests.
    ui: Option<Arc<Sequence>>,
    /// The pending-audio store.
    store: Option<Arc<BlockingStore>>,
}

struct Rig {
    session: DictationSession,
    audio: Arc<FakeAudioSource>,
    indicator: Arc<FakeIndicator>,
    requests: Arc<FakeShellRequests>,
    clipboard: Arc<FakeClipboard>,
    paster: Arc<FakePaster>,
    store: Arc<FakeTempAudioStore>,
    observer: Arc<RecordingObserver>,
    settings: Arc<SettingsService>,
    engine: Arc<EngineLog>,
}

impl Rig {
    fn new(settings: Settings, reply: Reply) -> Rig {
        Rig::with(settings, reply, Custom::default())
    }

    fn with(settings: Settings, reply: Reply, custom: Custom) -> Rig {
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
        let requests = Arc::new(FakeShellRequests::new());
        let clipboard = Arc::new(FakeClipboard::new());
        let paster = Arc::new(FakePaster::new().with_start_window(Some(window(W1))));
        let store = Arc::new(FakeTempAudioStore::new());
        let observer = Arc::new(RecordingObserver::new());
        let engine = Arc::new(EngineLog::default());
        let (clipboard_port, paster_port): (Arc<dyn Clipboard>, Arc<dyn Paster>) =
            match &custom.output {
                Some(o) => (o.clone(), o.clone()),
                None => (clipboard.clone(), paster.clone()),
            };
        let (indicator_port, requests_port): (Arc<dyn Indicator>, Arc<dyn ShellRequests>) =
            match &custom.ui {
                Some(u) => (u.clone(), u.clone()),
                None => (indicator.clone(), requests.clone()),
            };
        let store_port: Arc<dyn TempAudioStore> = match &custom.store {
            Some(b) => b.clone(),
            None => store.clone(),
        };
        let session = DictationSession::start(SessionDeps {
            pipeline: PipelineDeps {
                gate: energy_gate(),
                credentials: creds,
                clipboard: clipboard_port,
                paster: paster_port,
                temp_audio: store_port,
                observer: observer.clone(),
                post_processor: Arc::new(PassThrough),
            },
            engine_factory: Some(engine_factory(&engine, reply)),
            audio: audio.clone(),
            indicator: indicator_port,
            requests: requests_port,
            settings: settings.clone(),
        })
        .expect("the session starts");
        Rig {
            session,
            audio,
            indicator,
            requests,
            clipboard,
            paster,
            store,
            observer,
            settings,
            engine,
        }
    }

    /// `edit` applied to the current settings, saved through the real service
    /// (keys untouched: the key is stored).
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

    /// The frames of the next capture: `audio`, first frame stamped `first_at`.
    fn frames(&self, audio: &AudioBuffer, first_at: Instant) {
        self.audio
            .set_chunks(vec![FrameChunk::from_buffer(audio, first_at)]);
    }

    /// One hold: `audio`'s frames (first frame `FIRST_FRAME` after the press),
    /// press at `t`, release at `t + held`.
    fn hold(&self, audio: &AudioBuffer, t: Instant, held: Duration) {
        self.frames(audio, t + FIRST_FRAME);
        self.session.hotkey_pressed(t);
        self.session.hotkey_released(t + held);
    }

    fn events(&self) -> Vec<DictationEvent> {
        self.observer.events()
    }

    /// Waits until `n` jobs have ended and the overlay has left Processing.
    fn wait_jobs(&self, n: usize) {
        let done = eventually(|| {
            job_count(&self.events()) >= n
                && matches!(
                    self.indicator.overlays().last(),
                    Some(OverlayState::Hidden | OverlayState::Message { .. })
                )
        });
        assert!(
            done,
            "{n} job(s) not ended within {BUDGET:?}: events {:?}, indicator {:?}",
            self.events(),
            self.indicator.calls()
        );
    }
}

// ---- Acceptance 1: one hold, one delivery; jobs in order ------------------------------

#[test]
fn hold_gives_one_paste_and_overlay_recording_processing_hidden() {
    // Row 1, Acceptance 1. One hold through the session: the text reaches the
    // clipboard once, one Ctrl+V after the front check of the start window, the
    // overlay goes Recording -> Processing -> Hidden and the tray Recording ->
    // Idle, each state published once, and the recording and job events arrive on
    // the one observer for the one recording. Bite: no worker (no job), no
    // publish, a publish without dedupe (a state twice), Hidden flashed between
    // the release and the finish, a second observer (no RecordingStarted /
    // RecordingEnded here), the events of another recording.
    let rig = Rig::new(api_settings(), always(TEXT));
    let t0 = past();
    rig.frames(&fixtures::speech_3s(), t0 + FIRST_FRAME);
    rig.session.hotkey_pressed(t0);
    rig.session.hotkey_released(t0 + ms(3000));
    rig.wait_jobs(1);

    assert_eq!(rig.clipboard.texts(), vec![TEXT.to_string()]);
    assert_eq!(rig.paster.calls(), pasted_into(W1));
    assert_eq!(
        rig.indicator.overlays(),
        vec![
            OverlayState::Recording,
            OverlayState::Processing,
            OverlayState::Hidden
        ]
    );
    assert_eq!(
        rig.indicator.trays(),
        vec![(TrayState::Recording, false), (TrayState::Idle, false)]
    );
    assert_eq!(rig.engine.factory_calls(), 1);
    assert_eq!(rig.audio.start_calls(), 1);
    assert_eq!(rig.audio.open_handles(), 0);
    let events = rig.events();
    match events.as_slice() {
        [DictationEvent::RecordingStarted {
            recording: started,
            hotkey_to_first_frame_ms: 40,
            device: DeviceKind::Selected,
        }, DictationEvent::RecordingEnded {
            recording: ended,
            duration_ms: 3000,
            end: RecordingEnd::Released,
        }, DictationEvent::SpeechGate {
            recording: gated,
            detector: "energy",
            speech: true,
        }, DictationEvent::JobFinished {
            engine: Some("fake"),
            outcome: OutcomeCode::Text,
            failure: None,
            ..
        }, DictationEvent::Delivered {
            result: DeliveryResult::Pasted,
            ..
        }] => {
            assert_eq!(started, ended, "one recording");
            assert_eq!(ended, gated, "the job is that recording's");
        }
        other => panic!("events {other:?}"),
    }
}

#[test]
fn start_window_and_settings_are_taken_once_at_press() {
    // Row 2 (Clarification 4, P-013): the foreground window changes W1 -> W2 and
    // auto-paste is saved off between press and release; the job still pastes into
    // W1 with the press snapshot, and the start window is asked for once. Bite:
    // the start window captured at release or at delivery (IsInFront(W2)), the
    // snapshot re-read at release (copied only, no paster call), a second
    // CaptureStartWindow.
    let rig = Rig::new(api_settings(), always(TEXT));
    let t0 = past();
    rig.frames(&fixtures::speech_3s(), t0 + FIRST_FRAME);
    rig.session.hotkey_pressed(t0);
    rig.paster.set_start_window(Some(window(W2)));
    rig.save(|s| s.auto_paste = false);
    rig.session.hotkey_released(t0 + ms(3000));
    rig.wait_jobs(1);

    assert_eq!(rig.paster.calls(), pasted_into(W1));
    assert_eq!(rig.clipboard.texts(), vec![TEXT.to_string()]);
    assert_eq!(delivery_events(&rig.events()), vec![DeliveryResult::Pasted]);
    let seen: Vec<bool> = rig.engine.settings().iter().map(|s| s.auto_paste).collect();
    assert_eq!(seen, vec![true], "the job ran with the press snapshot");
}

#[test]
fn stereo_48k_capture_reaches_the_job_as_16k_mono() {
    // FR-06 through the session (analysis "Ports": the sink mixes down to mono as
    // frames arrive; `AudioBuffer::from_frames` resamples at the stop): 3 s
    // captured at 48 kHz stereo (L = R) reach the engine as about 3 s of 16 kHz
    // mono and are delivered. Bite: interleaved stereo taken as mono (twice the
    // length), no resampling (three times), the capture rate dropped (16 kHz
    // assumed).
    let speech = fixtures::speech_3s();
    // Each 16 kHz sample held for 3 frames at 48 kHz, both channels equal.
    let stereo_48k: Vec<f32> = speech
        .samples()
        .iter()
        .flat_map(|&s| [f32::from(s) / 32768.0; 6])
        .collect();
    let rig = Rig::new(api_settings(), always(TEXT));
    let t0 = past();
    rig.audio.set_chunks(vec![FrameChunk {
        samples: stereo_48k,
        rate: 48_000,
        channels: 2,
        at: t0 + FIRST_FRAME,
    }]);
    rig.session.hotkey_pressed(t0);
    rig.session.hotkey_released(t0 + ms(3000));
    rig.wait_jobs(1);
    let lengths = rig.engine.transcribed();
    assert_eq!(lengths.len(), 1, "one engine call: {lengths:?}");
    // spec T009: length within 1 sample per 10 ms of audio.
    assert!(
        lengths[0].abs_diff(48_000) <= 300,
        "the engine got {} samples, expected 48000 ± 300 (16 kHz mono)",
        lengths[0]
    );
    assert_eq!(rig.clipboard.texts(), vec![TEXT.to_string()]);
}

#[test]
fn jobs_never_overlap_in_delivery_and_finish_in_recording_order() {
    // Row 3, Acceptance 1 (FR-029, decision #48). A's engine blocks until B and C
    // are recorded and queued, B and C answer at once. Deliveries never overlap
    // (max 1 in flight), the clipboard gets A, B, C in recording order, the jobs
    // run on one thread that is not the input thread, and the overlay stays
    // Processing until the last job ends. Bite: a thread per job (B and C before
    // A, or an overlap), LIFO (C before B), run_job on the input thread (release
    // blocks; deliveries on the test thread), the overlay hidden while jobs are
    // queued.
    let [a, b, c] = three_recordings();
    let g = gated(48_000, &[(48_000, "A"), (32_000, "B"), (24_000, "C")]);
    let out = Arc::new(Instrumented::default());
    let rig = Rig::with(
        api_settings(),
        g.reply,
        Custom {
            output: Some(out.clone()),
            ..Custom::default()
        },
    );
    let t0 = past();
    rig.hold(&a, t0, ms(3000));
    assert!(
        g.entered.recv_timeout(BUDGET).is_ok(),
        "A's job never reached the engine"
    );
    rig.hold(&b, t0 + ms(4000), ms(2000));
    rig.hold(&c, t0 + ms(7000), ms(1500));
    assert_eq!(out.texts(), Vec::<String>::new(), "A is still in its job");
    g.go.send(()).expect("A's engine is waiting");
    assert!(
        eventually(|| out.pastes.load(Ordering::SeqCst) == 3
            && rig.indicator.overlays().last() == Some(&OverlayState::Hidden)),
        "three deliveries not done: texts {:?}, indicator {:?}",
        out.texts(),
        rig.indicator.calls()
    );

    assert_eq!(out.texts(), vec!["A", "B", "C"]);
    assert_eq!(
        out.max_in_flight.load(Ordering::SeqCst),
        1,
        "deliveries overlapped"
    );
    let threads = out.threads();
    assert!(
        threads.windows(2).all(|w| w[0] == w[1]),
        "deliveries ran on several threads: {threads:?}"
    );
    assert!(
        !threads.contains(&thread::current().id()),
        "a delivery ran on the input thread"
    );
    let gates: Vec<_> = rig
        .events()
        .iter()
        .filter_map(|e| match e {
            DictationEvent::SpeechGate { recording, .. } => Some(*recording),
            _ => None,
        })
        .collect();
    assert_eq!(gates.len(), 3, "{gates:?}");
    assert!(
        gates.windows(2).all(|w| w[0] < w[1]),
        "jobs not in recording order: {gates:?}"
    );
    assert_eq!(
        rig.indicator.overlays(),
        vec![
            OverlayState::Recording,
            OverlayState::Processing,
            OverlayState::Recording,
            OverlayState::Processing,
            OverlayState::Recording,
            OverlayState::Processing,
            OverlayState::Hidden,
        ]
    );
}

#[test]
fn press_during_a_slow_job_starts_recording_at_once() {
    // Row 4 (FR-029): while A's job is blocked in the engine, B's press returns
    // and publishes Recording before A's job ends; both are delivered in order
    // afterwards. B's press runs on a helper thread, so a session that blocks it
    // fails here instead of hanging. Bite: run_job under the controller lock (the
    // press waits for A), run_job on the input thread (A's release blocks until
    // the engine's budget runs out and A is delivered first).
    let [a, b, _] = three_recordings();
    let g = gated(48_000, &[(48_000, "A"), (32_000, "B")]);
    let out = Arc::new(Instrumented::default());
    let rig = Rig::with(
        api_settings(),
        g.reply,
        Custom {
            output: Some(out.clone()),
            ..Custom::default()
        },
    );
    let t0 = past();
    rig.hold(&a, t0, ms(3000));
    assert!(
        g.entered.recv_timeout(BUDGET).is_ok(),
        "A's job never reached the engine"
    );
    let tb = t0 + ms(4000);
    rig.frames(&b, tb + FIRST_FRAME);
    let before = rig.indicator.calls().len();
    let (pressed_tx, pressed_rx) = mpsc::channel();
    let session = &rig.session;
    let (returned, a_running, after_press) = thread::scope(|s| {
        s.spawn(move || {
            session.hotkey_pressed(tb);
            let _ = pressed_tx.send(());
        });
        let returned = pressed_rx.recv_timeout(BUDGET).is_ok();
        let a_running = out.texts().is_empty() && job_count(&rig.events()) == 0;
        let after_press = rig.indicator.calls();
        let _ = g.go.send(());
        (returned, a_running, after_press)
    });
    assert!(
        returned,
        "B's press did not return within {BUDGET:?} while A was in its job"
    );
    assert!(a_running, "A's job ended before B's press returned");
    // Before B's press: tray Idle, overlay Processing (A queued). The press
    // changes both, once each (the order of the two ports is not specified).
    let mut new_calls = after_press.get(before..).unwrap_or(&[]).to_vec();
    new_calls.sort_by_key(|c| matches!(c, IndicatorCall::Overlay(_)));
    assert_eq!(
        new_calls,
        vec![
            IndicatorCall::Tray(TrayState::Recording, false),
            IndicatorCall::Overlay(OverlayState::Recording),
        ],
        "what B's press published while A was in its job"
    );

    rig.session.hotkey_released(tb + ms(2000));
    assert!(
        eventually(|| out.pastes.load(Ordering::SeqCst) == 2),
        "deliveries: {:?}",
        out.texts()
    );
    assert_eq!(out.texts(), vec!["A", "B"]);
}

// ---- Acceptance 2: failure branches ----------------------------------------------------

#[test]
fn access_denied_publishes_no_recording_state_and_queues_no_job() {
    // Row 5, Acceptance 2 (FR-009; data-model "Idle --press [device fails]-->
    // Idle + failure (no recording state)"). The capture cannot open: the
    // indicator never shows Recording; the tray shows Error and the overlay
    // "microphone unavailable: access denied" for 3 s from the press; one
    // CaptureFailed{AccessDenied}, no RecordingStarted; no device left open; the
    // stray release changes nothing; and no job: a later good hold is the first
    // and only job. Bite: a publish between the press and the capture result
    // (Recording flashes), no capture_failed (tray stays Recording), a job queued
    // for the failed press (two factory calls), the event missing.
    let rig = Rig::new(api_settings(), always(TEXT));
    rig.audio.set_start_error(Some(CaptureError::AccessDenied));
    let tp = Instant::now();
    rig.session.hotkey_pressed(tp);

    let calls = rig.indicator.calls();
    assert!(
        !calls.iter().any(|c| matches!(
            c,
            IndicatorCall::Tray(TrayState::Recording, _)
                | IndicatorCall::Overlay(OverlayState::Recording)
        )),
        "a Recording state was published: {calls:?}"
    );
    let expected = vec![
        IndicatorCall::Tray(TrayState::Error, false),
        IndicatorCall::Overlay(mic_message(
            "mic_reason.access_denied",
            tp + MESSAGE_DURATION,
        )),
    ];
    let hidden = IndicatorCall::Overlay(OverlayState::Hidden);
    let mut got = settled(calls, hidden.clone(), tp);
    got.sort_by_key(|c| matches!(c, IndicatorCall::Overlay(_)));
    assert_eq!(got, expected, "after the failed press");
    assert_eq!(rig.audio.start_calls(), 1);
    assert_eq!(rig.audio.open_handles(), 0);
    let events = rig.events();
    assert!(
        matches!(
            events.as_slice(),
            [DictationEvent::CaptureFailed {
                cause: MicCause::AccessDenied,
                ..
            }]
        ),
        "{events:?}"
    );

    rig.session.hotkey_released(Instant::now());
    let mut after = settled(rig.indicator.calls(), hidden, tp);
    after.sort_by_key(|c| matches!(c, IndicatorCall::Overlay(_)));
    assert_eq!(after, expected, "the stray release changed the indicator");
    assert_eq!(rig.events().len(), 1, "the stray release emitted an event");

    // Barrier: the FIFO runs jobs in order, so a job queued by the failed press
    // would run before this one.
    rig.audio.set_start_error(None);
    rig.hold(&fixtures::speech_3s(), past(), ms(3000));
    rig.wait_jobs(1);
    assert_eq!(
        rig.engine.factory_calls(),
        1,
        "a job ran for the failed press"
    );
    assert_eq!(rig.clipboard.texts(), vec![TEXT.to_string()]);
    assert_eq!(job_count(&rig.events()), 1);
    assert_eq!(
        capture_failures(&rig.events()),
        vec![MicCause::AccessDenied]
    );
}

#[test]
fn engine_none_opens_no_capture_and_asks_for_the_engine_tab() {
    // Row 6, Acceptance 2 (FR-007; blocked_actions: notice, then open settings on
    // Engine). No capture is started; the overlay shows notice.choose_engine for
    // 3 s from the press before the shell is asked to open settings on the Engine
    // tab; the tray is not touched; no event; the release does nothing. Bite: a
    // capture opened before the gate, the wrong tab, the request before the
    // notice, the notice shown outside the controller (no overlay message), a
    // tray change.
    let mut s = api_settings();
    s.engine = EngineKind::None;
    let ui = Arc::new(Sequence::default());
    let rig = Rig::with(
        s,
        always(TEXT),
        Custom {
            ui: Some(ui.clone()),
            ..Custom::default()
        },
    );
    let tp = Instant::now();
    rig.session.hotkey_pressed(tp);
    let expected = vec![
        Step::Overlay(OverlayState::Message {
            id: i18n::NOTICE_CHOOSE_ENGINE,
            params: vec![],
            until: tp + MESSAGE_DURATION,
        }),
        Step::OpenSettings(SettingsTab::Engine),
    ];
    let hidden = Step::Overlay(OverlayState::Hidden);
    assert_eq!(
        settled(lock(&ui.steps).clone(), hidden.clone(), tp),
        expected
    );
    assert_eq!(rig.audio.start_calls(), 0, "a capture was started");

    rig.session.hotkey_released(Instant::now());
    assert_eq!(
        settled(lock(&ui.steps).clone(), hidden, tp),
        expected,
        "the release did something"
    );
    assert_eq!(rig.audio.start_calls(), 0);
    assert_eq!(rig.events(), vec![], "no event for a blocked press");
    assert_eq!(rig.engine.factory_calls(), 0);
}

#[test]
fn press_while_recording_runs_no_gate_and_opens_no_second_stream() {
    // Row 7 (invariant (1)): auto-repeat presses while recording, after engine
    // none was saved, neither run the gate (no request, no notice) nor start a
    // second capture; the one recording is delivered with the first press's
    // snapshot and measured from the first press. Bite: the gate checked before
    // the live check (open settings, notice), `start` called on Ignored, the hold
    // measured from a repeat.
    let rig = Rig::new(api_settings(), always(TEXT));
    let t0 = past();
    rig.frames(&fixtures::speech_3s(), t0 + FIRST_FRAME);
    rig.session.hotkey_pressed(t0);
    rig.save(|s| s.engine = EngineKind::None);
    rig.session.hotkey_pressed(t0 + ms(100));
    rig.session.hotkey_pressed(t0 + ms(200));
    assert_eq!(rig.audio.start_calls(), 1);
    assert_eq!(rig.audio.open_handles(), 1);
    assert_eq!(rig.requests.calls(), vec![]);
    assert!(
        !rig.indicator
            .overlays()
            .iter()
            .any(|o| matches!(o, OverlayState::Message { .. })),
        "{:?}",
        rig.indicator.overlays()
    );

    rig.session.hotkey_released(t0 + ms(3000));
    rig.wait_jobs(1);
    assert_eq!(rig.clipboard.texts(), vec![TEXT.to_string()]);
    assert_eq!(
        recording_ends(&rig.events()),
        vec![(3000, RecordingEnd::Released)]
    );
    let engines: Vec<EngineKind> = rig.engine.settings().iter().map(|s| s.engine).collect();
    assert_eq!(engines, vec![EngineKind::Api]);
}

#[test]
fn press_under_300ms_is_discarded_with_the_stream_closed() {
    // Row 8, Acceptance 2 (FR-02): a 299 ms hold ends TooShort with its duration,
    // closes the device, shows Recording then Hidden with no message, and queues
    // no job (a later good hold is the only job). Bite: a job for a short hold,
    // the stream left open, a message for the silent discard, the duration
    // measured elsewhere.
    let rig = Rig::new(api_settings(), always(TEXT));
    let t0 = past();
    rig.hold(&fixtures::speech_3s(), t0, ms(299));
    assert_eq!(rig.audio.open_handles(), 0, "stream left open");
    assert_eq!(
        recording_ends(&rig.events()),
        vec![(299, RecordingEnd::TooShort)]
    );
    assert_eq!(
        rig.indicator.overlays(),
        vec![OverlayState::Recording, OverlayState::Hidden]
    );
    assert_eq!(
        rig.indicator.trays(),
        vec![(TrayState::Recording, false), (TrayState::Idle, false)]
    );

    rig.hold(&fixtures::speech_3s(), t0 + ms(5000), ms(3000));
    rig.wait_jobs(1);
    assert_eq!(
        rig.engine.factory_calls(),
        1,
        "a job ran for the short hold"
    );
    assert_eq!(rig.clipboard.texts(), vec![TEXT.to_string()]);
    let gates = rig
        .events()
        .iter()
        .filter(|e| matches!(e, DictationEvent::SpeechGate { .. }))
        .count();
    assert_eq!(gates, 1);
}

#[test]
fn microphone_is_open_only_between_press_and_release() {
    // Row 9 (NFR-02): no device before the press; one while held; none once the
    // release returned, for a stop, a discard, a stop that fails, and a press whose
    // capture fails. Bite: the handle kept after release (stopped later, on the
    // worker, or leaked), a second stream.
    let rig = Rig::new(api_settings(), always(TEXT));
    let speech = fixtures::speech_3s();
    let t0 = past();
    assert_eq!(rig.audio.open_handles(), 0, "before any press");

    rig.frames(&speech, t0 + FIRST_FRAME);
    rig.session.hotkey_pressed(t0);
    assert_eq!(rig.audio.open_handles(), 1, "held (stop)");
    rig.session.hotkey_released(t0 + ms(3000));
    assert_eq!(rig.audio.open_handles(), 0, "after a stop");

    let t1 = t0 + ms(5000);
    rig.frames(&speech, t1 + FIRST_FRAME);
    rig.session.hotkey_pressed(t1);
    assert_eq!(rig.audio.open_handles(), 1, "held (discard)");
    rig.session.hotkey_released(t1 + ms(100));
    assert_eq!(rig.audio.open_handles(), 0, "after a discard");

    let t2 = t0 + ms(6000);
    rig.audio.set_stop_error(Some(CaptureError::DeviceBusy));
    rig.frames(&speech, t2 + FIRST_FRAME);
    rig.session.hotkey_pressed(t2);
    assert_eq!(rig.audio.open_handles(), 1, "held (failing stop)");
    rig.session.hotkey_released(t2 + ms(3000));
    assert_eq!(rig.audio.open_handles(), 0, "after a failing stop");

    rig.audio.set_stop_error(None);
    rig.audio.set_start_error(Some(CaptureError::NoDevice));
    rig.session.hotkey_pressed(t0 + ms(10_000));
    assert_eq!(rig.audio.open_handles(), 0, "after a failed start");
    rig.session.hotkey_released(t0 + ms(13_000));
    assert_eq!(rig.audio.open_handles(), 0);
    assert_eq!(rig.audio.start_calls(), 4, "one capture per press");
}

#[test]
fn first_frame_instant_gives_hotkey_to_first_frame_ms() {
    // Row 10 (T-008 touch point): RecordingStarted carries the adapter's first
    // frame instant minus the press instant (40 ms; later chunks do not move it)
    // and comes before that recording's RecordingEnded; a capture with no frame
    // emits no RecordingStarted but still RecordingEnded. Bite: measured from the
    // release or from `start` returning, core stamping its own instant (a minute
    // off here), the last chunk's instant, RecordingStarted without a frame.
    let rig = Rig::new(api_settings(), always(TEXT));
    let t0 = past();
    let all = FrameChunk::from_buffer(&fixtures::speech_3s(), t0 + ms(40));
    let (first, rest) = all.samples.split_at(all.samples.len() / 2);
    rig.audio.set_chunks(vec![
        FrameChunk {
            samples: first.to_vec(),
            at: t0 + ms(40),
            ..all.clone()
        },
        FrameChunk {
            samples: rest.to_vec(),
            at: t0 + ms(60),
            ..all.clone()
        },
    ]);
    rig.session.hotkey_pressed(t0);
    rig.session.hotkey_released(t0 + ms(3000));
    rig.wait_jobs(1);

    rig.audio.set_chunks(vec![]);
    let t1 = t0 + ms(10_000);
    rig.session.hotkey_pressed(t1);
    rig.session.hotkey_released(t1 + ms(1000));
    rig.wait_jobs(2);

    let events = rig.events();
    let started: Vec<(usize, u64, DeviceKind)> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            DictationEvent::RecordingStarted {
                hotkey_to_first_frame_ms,
                device,
                ..
            } => Some((i, *hotkey_to_first_frame_ms, *device)),
            _ => None,
        })
        .collect();
    let ended: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e, DictationEvent::RecordingEnded { .. }))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        started
            .iter()
            .map(|(_, ms, d)| (*ms, *d))
            .collect::<Vec<_>>(),
        vec![(40, DeviceKind::Selected)],
        "{events:?}"
    );
    assert_eq!(
        recording_ends(&events),
        vec![
            (3000, RecordingEnd::Released),
            (1000, RecordingEnd::Released)
        ]
    );
    assert!(
        matches!((started.first(), ended.first()), (Some((s, _, _)), Some(e)) if s < e),
        "RecordingStarted after RecordingEnded: {events:?}"
    );
}

#[test]
fn capture_error_at_stop_is_microphone_unavailable_and_no_job() {
    // Row 11 (FR-009, P-009, NFR-07): a stop that fails (DeviceBusy), frames that
    // cannot be converted (rate 0), an OS error with text and a format change
    // within one capture each give tray Error, the "microphone unavailable"
    // message with the closed reason, one CaptureFailed with that cause, and no
    // job; the OS text reaches no event and no indicator state; the session keeps
    // working (no panic): a later good hold is the only job. Bite: an empty job
    // built from the Err, `unwrap` on `from_frames` (panics on rate 0), the OS
    // text copied into a message or event, the wrong cause, frames of two rates
    // concatenated into one job.
    let rig = Rig::new(api_settings(), always(TEXT));
    let speech = fixtures::speech_3s();
    let t0 = past();

    rig.audio.set_stop_error(Some(CaptureError::DeviceBusy));
    rig.hold(&speech, t0, ms(3000));
    assert_eq!(capture_failures(&rig.events()), vec![MicCause::Busy]);
    assert!(
        rig.indicator
            .overlays()
            .iter()
            .any(|o| is_mic_message(o, "mic_reason.busy")),
        "{:?}",
        rig.indicator.overlays()
    );
    assert_eq!(
        rig.indicator.trays().last(),
        Some(&(TrayState::Error, false))
    );

    rig.audio.set_stop_error(None);
    let t1 = t0 + ms(5000);
    rig.audio.set_chunks(vec![FrameChunk {
        samples: vec![0.1; 1600],
        rate: 0,
        channels: 1,
        at: t1 + FIRST_FRAME,
    }]);
    rig.session.hotkey_pressed(t1);
    rig.session.hotkey_released(t1 + ms(3000));
    assert_eq!(
        capture_failures(&rig.events()),
        vec![MicCause::Busy, MicCause::Other]
    );
    assert!(
        rig.indicator
            .overlays()
            .iter()
            .any(|o| is_mic_message(o, "mic_reason.other")),
        "{:?}",
        rig.indicator.overlays()
    );

    let canary = "0x88890004 example-os-text-canary";
    rig.audio
        .set_stop_error(Some(CaptureError::Other(canary.to_string())));
    rig.hold(&speech, t0 + ms(10_000), ms(3000));
    assert_eq!(
        capture_failures(&rig.events()),
        vec![MicCause::Busy, MicCause::Other, MicCause::Other]
    );
    let seen = format!("{:?} {:?}", rig.events(), rig.indicator.calls());
    assert!(!seen.contains("canary"), "OS text leaked: {seen}");

    // A format change within one capture (16 kHz, then 48 kHz) is an error, not
    // audio mixed at two rates (analysis "Ports": FrameSink).
    rig.audio.set_stop_error(None);
    let t3 = t0 + ms(15_000);
    let first = FrameChunk::from_buffer(&speech, t3 + FIRST_FRAME);
    rig.audio.set_chunks(vec![
        first.clone(),
        FrameChunk {
            rate: 48_000,
            at: t3 + ms(80),
            ..first
        },
    ]);
    rig.session.hotkey_pressed(t3);
    rig.session.hotkey_released(t3 + ms(3000));
    assert_eq!(
        capture_failures(&rig.events()),
        vec![
            MicCause::Busy,
            MicCause::Other,
            MicCause::Other,
            MicCause::Other
        ]
    );

    rig.hold(&speech, t0 + ms(20_000), ms(3000));
    rig.wait_jobs(1);
    assert_eq!(
        rig.engine.factory_calls(),
        1,
        "a job ran for a failed capture"
    );
    assert_eq!(rig.clipboard.texts(), vec![TEXT.to_string()]);
}

#[test]
fn tray_menu_opened_clears_tray_error_and_keeps_the_message() {
    // Session input `tray_menu_opened` (data-model: Error is cleared by opening
    // the tray menu): after a capture failure the tray goes Error -> Idle and the
    // message stays. Bite: the input not wired to the controller, no publish, the
    // message dropped with the tray Error.
    let rig = Rig::new(api_settings(), always(TEXT));
    rig.audio.set_start_error(Some(CaptureError::AccessDenied));
    let tp = Instant::now();
    rig.session.hotkey_pressed(tp);
    rig.session.tray_menu_opened(Instant::now());
    assert_eq!(
        rig.indicator.trays(),
        vec![(TrayState::Error, false), (TrayState::Idle, false)]
    );
    assert_eq!(
        settled(rig.indicator.overlays(), OverlayState::Hidden, tp),
        vec![mic_message(
            "mic_reason.access_denied",
            tp + MESSAGE_DURATION
        )]
    );
}

// ---- the message timer (real time) --------------------------------------------------

#[test]
fn message_expires_without_input() {
    // Row 12 (real time): after a capture failure the overlay goes Hidden at the
    // message's `until` (press + 3 s) with no further input, not before and
    // within a Windows-sized budget after; tray Error stays. Bite: no timer
    // thread, tick never armed (no Hidden), the timer firing early.
    let rig = Rig::new(api_settings(), always(TEXT));
    rig.audio.set_start_error(Some(CaptureError::AccessDenied));
    let tp = Instant::now();
    rig.session.hotkey_pressed(tp);
    assert!(
        eventually(|| rig.indicator.overlays().contains(&OverlayState::Hidden)),
        "the message never expired: {:?}",
        rig.indicator.calls()
    );
    let hidden_at = rig
        .indicator
        .timed_calls()
        .iter()
        .find(|(_, c)| *c == IndicatorCall::Overlay(OverlayState::Hidden))
        .map(|(at, _)| *at)
        .expect("Hidden was published");
    let until = tp + MESSAGE_DURATION;
    assert!(
        hidden_at >= until,
        "expired {:?} before its until",
        until - hidden_at
    );
    assert!(
        hidden_at <= until + EXPIRY_BUDGET,
        "expired {:?} after its until",
        hidden_at - until
    );
    assert_eq!(
        rig.indicator.overlays(),
        vec![
            mic_message("mic_reason.access_denied", until),
            OverlayState::Hidden
        ]
    );
    assert_eq!(rig.indicator.trays(), vec![(TrayState::Error, false)]);
}

#[test]
fn timer_follows_the_latest_deadline() {
    // Row 13 (real time): failure A, then failure B 1 s later (B's press drops
    // A's message); the overlay is hidden at B's `until`, not at A's. Bite: the
    // timer not re-armed on a mutation (it wakes at A's deadline, the tick changes
    // nothing, and it never wakes again: no Hidden), a timer that waits only for
    // the first deadline it saw.
    let rig = Rig::new(api_settings(), always(TEXT));
    rig.audio.set_start_error(Some(CaptureError::AccessDenied));
    let ta = Instant::now();
    rig.session.hotkey_pressed(ta);
    rig.session.hotkey_released(Instant::now());
    thread::sleep(Duration::from_secs(1));
    let tb = Instant::now();
    rig.session.hotkey_pressed(tb);
    rig.session.hotkey_released(Instant::now());
    assert!(
        eventually(|| rig.indicator.overlays().contains(&OverlayState::Hidden)),
        "the message never expired: {:?}",
        rig.indicator.calls()
    );
    let hidden_at = rig
        .indicator
        .timed_calls()
        .iter()
        .find(|(_, c)| *c == IndicatorCall::Overlay(OverlayState::Hidden))
        .map(|(at, _)| *at)
        .expect("Hidden was published");
    let until_b = tb + MESSAGE_DURATION;
    assert!(
        hidden_at >= until_b,
        "hidden {:?} before B's until (at A's deadline?)",
        until_b - hidden_at
    );
    assert!(
        hidden_at <= until_b + EXPIRY_BUDGET,
        "hidden {:?} after B's until",
        hidden_at - until_b
    );
    assert_eq!(
        rig.indicator.overlays(),
        vec![
            mic_message("mic_reason.access_denied", ta + MESSAGE_DURATION),
            mic_message("mic_reason.access_denied", until_b),
            OverlayState::Hidden,
        ]
    );
    assert_eq!(rig.indicator.trays(), vec![(TrayState::Error, false)]);
}

// ---- retry, toggle, settings, hotkey registration -------------------------------------

#[test]
fn retry_available_follows_the_pending_slot() {
    // Row 14 (analysis hypothesis 4; data-model "PendingRecording": Text and
    // NoSpeech do not touch the slot). 503 keeps the audio pending: tray (Error,
    // retry). A later delivered text leaves the slot: (Idle, retry) - the tray
    // Retry must stay. A retryable failure whose audio cannot be stored empties
    // the slot: (Error, no retry). Bite: retry taken from the last
    // JobReport.pending (the delivered job hides Retry), retry never published
    // (always false), retry not updated after a failed store.
    let rig = Rig::new(
        api_settings(),
        in_order(vec![
            Err(FailureReason::ServerError { status: 503 }),
            Ok(TEXT.to_string()),
            Err(FailureReason::Timeout),
        ]),
    );
    let speech = fixtures::speech_3s();
    let t0 = past();
    let last_tray =
        |want: (TrayState, bool)| eventually(|| rig.indicator.trays().last() == Some(&want));

    rig.hold(&speech, t0, ms(3000));
    assert!(
        last_tray((TrayState::Error, true)),
        "after the 503: {:?}",
        rig.indicator.trays()
    );
    rig.hold(&speech, t0 + ms(10_000), ms(3000));
    assert!(
        last_tray((TrayState::Idle, true)),
        "after the delivered text: {:?}",
        rig.indicator.trays()
    );
    assert_eq!(rig.clipboard.texts(), vec![TEXT.to_string()]);
    rig.store.set_fail_put(true);
    rig.hold(&speech, t0 + ms(20_000), ms(3000));
    assert!(
        last_tray((TrayState::Error, false)),
        "after the failure that could not be stored: {:?}",
        rig.indicator.trays()
    );
}

#[test]
fn input_threads_never_wait_for_the_pending_slot() {
    // Invariant (4) (analysis hypothesis 2(b), T-007 note): retry_available is
    // read from the pending slot on the worker, never through Pipeline::pending()
    // on an input thread, because `keep_pending` holds the slot lock across
    // `put_pending`. While a 503's audio is being stored (the store blocks), the
    // tray menu, a registration result and a press each return at once; once the
    // store returns, the tray shows Error with Retry. The inputs run on a helper
    // thread, so a session that blocks them fails here instead of hanging. Bite:
    // `Pipeline::pending()` called from the publish of an input, or from an input.
    let (store, entered, go) = BlockingStore::new();
    let rig = Rig::with(
        api_settings(),
        in_order(vec![Err(FailureReason::ServerError { status: 503 })]),
        Custom {
            store: Some(store),
            ..Custom::default()
        },
    );
    let t0 = past();
    rig.hold(&fixtures::speech_3s(), t0, ms(3000));
    assert!(
        entered.recv_timeout(BUDGET).is_ok(),
        "the 503's audio was never stored"
    );
    rig.audio.set_chunks(vec![]);
    let (done_tx, done_rx) = mpsc::channel();
    let session = &rig.session;
    let returned = thread::scope(|s| {
        s.spawn(move || {
            session.tray_menu_opened(t0 + ms(4000));
            session.hotkey_registration(true, t0 + ms(4100));
            session.hotkey_pressed(t0 + ms(5000));
            let _ = done_tx.send(());
        });
        let returned = done_rx.recv_timeout(BUDGET).is_ok();
        let _ = go.send(());
        returned
    });
    assert!(
        returned,
        "an input waited for the pending slot while its audio was being stored"
    );
    rig.session.hotkey_released(t0 + ms(5100));
    assert!(
        eventually(|| rig.indicator.trays().last() == Some(&(TrayState::Error, true))),
        "tray after the stored 503: {:?}",
        rig.indicator.trays()
    );
}

#[test]
fn toggle_mode_behaves_as_hold() {
    // Row 15 (decision #63, until T-009): with mode = Toggle, press and release
    // after 1 s give one delivery. Bite: the release ignored in toggle mode (no
    // job until a second press).
    let mut s = api_settings();
    s.mode = Mode::Toggle;
    let rig = Rig::new(s, always(TEXT));
    rig.hold(&fixtures::speech_3s(), past(), ms(1000));
    rig.wait_jobs(1);
    assert_eq!(rig.clipboard.texts(), vec![TEXT.to_string()]);
    assert_eq!(rig.paster.calls(), pasted_into(W1));
    assert_eq!(
        recording_ends(&rig.events()),
        vec![(1000, RecordingEnd::Released)]
    );
}

#[test]
fn saved_settings_reach_the_next_press() {
    // Row 16 (P-013): engine none blocks the first press; after engine = Api is
    // saved through the real SettingsService, the next hold records and delivers
    // with the saved engine. Bite: the snapshot cached when the session started
    // (still blocked), the settings read from somewhere else.
    let mut s = api_settings();
    s.engine = EngineKind::None;
    let rig = Rig::new(s, always(TEXT));
    let t0 = past();
    rig.session.hotkey_pressed(t0);
    rig.session.hotkey_released(t0 + ms(1000));
    assert_eq!(
        rig.requests.calls(),
        vec![ShellRequestCall::OpenSettings(SettingsTab::Engine)]
    );
    assert_eq!(rig.audio.start_calls(), 0);

    rig.save(|s| s.engine = EngineKind::Api);
    rig.hold(&fixtures::speech_3s(), t0 + ms(5000), ms(3000));
    rig.wait_jobs(1);
    assert_eq!(rig.clipboard.texts(), vec![TEXT.to_string()]);
    assert_eq!(rig.audio.start_calls(), 1);
    assert_eq!(rig.requests.calls().len(), 1, "blocked again");
    let engines: Vec<EngineKind> = rig.engine.settings().iter().map(|s| s.engine).collect();
    assert_eq!(engines, vec![EngineKind::Api]);
}

#[test]
fn hotkey_registration_sets_and_clears_the_tray_hotkey_error() {
    // Row 17 (FR-011, FR-028): a failed registration shows HotkeyError; it stays
    // through a recording, a delivered job and the tray menu (none of them
    // publishes another tray state); a successful registration brings back Idle.
    // Bite: the input not wired, HotkeyError cleared by the delivery or the menu,
    // Recording shown over HotkeyError.
    let rig = Rig::new(api_settings(), always(TEXT));
    let t0 = past();
    rig.session.hotkey_registration(false, t0);
    assert_eq!(rig.indicator.trays(), vec![(TrayState::HotkeyError, false)]);
    rig.hold(&fixtures::speech_3s(), t0 + ms(1000), ms(3000));
    rig.wait_jobs(1);
    rig.session.tray_menu_opened(t0 + ms(10_000));
    assert_eq!(rig.clipboard.texts(), vec![TEXT.to_string()]);
    assert_eq!(
        rig.indicator.trays(),
        vec![(TrayState::HotkeyError, false)],
        "kept through a recording, a delivery and the tray menu"
    );
    assert_eq!(
        rig.indicator.overlays(),
        vec![
            OverlayState::Recording,
            OverlayState::Processing,
            OverlayState::Hidden
        ]
    );
    rig.session.hotkey_registration(true, t0 + ms(11_000));
    assert_eq!(
        rig.indicator.trays(),
        vec![(TrayState::HotkeyError, false), (TrayState::Idle, false)]
    );
}

// ---- shutdown --------------------------------------------------------------------------

#[test]
fn drop_finishes_the_job_in_flight_drops_the_queued_one_and_joins_threads() {
    // Row 18 (analysis Q3; core-traits "shutdown: drop in-flight results"): drop
    // with A in its job and B queued returns once A is done (A delivered), B's
    // engine is never called, and every port the session held is released (its
    // threads are gone: a thread still running would hold the pipeline). The
    // drop runs on a helper thread, bounded by BUDGET. Bite: a detached worker
    // (drop returns before A, ports still held), a hang, queued jobs run after
    // drop (mpsc still hands out B after the sender is dropped).
    let [a, b, _] = three_recordings();
    let g = gated(48_000, &[(48_000, "A"), (32_000, "B")]);
    let rig = Rig::new(api_settings(), g.reply);
    let t0 = past();
    rig.hold(&a, t0, ms(3000));
    assert!(
        g.entered.recv_timeout(BUDGET).is_ok(),
        "A's job never reached the engine"
    );
    rig.hold(&b, t0 + ms(4000), ms(2000));

    let Rig {
        session,
        audio,
        indicator,
        requests,
        clipboard,
        paster,
        store,
        observer,
        settings,
        engine,
    } = rig;
    // The dropper reports what the clipboard held at the moment drop returned.
    let (dropped_tx, dropped_rx) = mpsc::channel();
    let clipboard_seen = Arc::clone(&clipboard);
    let dropper = thread::spawn(move || {
        drop(session);
        let _ = dropped_tx.send(clipboard_seen.texts());
    });
    // Let the drop begin before A's job can end; A is still blocked, so a drop
    // that waits for its threads cannot have returned yet.
    thread::sleep(ms(300));
    let early = dropped_rx.try_recv().is_ok();
    let _ = g.go.send(());
    let at_return = if early {
        None
    } else {
        dropped_rx.recv_timeout(BUDGET).ok()
    };
    dropper.join().expect("drop must not panic");
    assert!(
        !early,
        "drop returned while A was still in its job (threads not joined)"
    );
    assert_eq!(
        at_return,
        Some(vec!["A".to_string()]),
        "drop did not return within {BUDGET:?}, or returned before A was delivered"
    );

    assert_eq!(clipboard.texts(), vec!["A".to_string()]);
    assert_eq!(engine.transcribed(), vec![48_000], "B's engine was called");
    let held = [
        ("audio", Arc::strong_count(&audio)),
        ("indicator", Arc::strong_count(&indicator)),
        ("requests", Arc::strong_count(&requests)),
        ("clipboard", Arc::strong_count(&clipboard)),
        ("paster", Arc::strong_count(&paster)),
        ("store", Arc::strong_count(&store)),
        ("observer", Arc::strong_count(&observer)),
        ("settings", Arc::strong_count(&settings)),
        ("engine factory", Arc::strong_count(&engine)),
    ];
    let still: Vec<_> = held.iter().filter(|(_, n)| *n != 1).collect();
    assert!(still.is_empty(), "still held after drop: {still:?}");
}

#[test]
fn session_is_send_and_sync() {
    // Compile-time guard: shell threads (hotkey, tray, timer) share one session.
    fn shared<T: Send + Sync>() {}
    shared::<DictationSession>();
    shared::<SessionDeps>();
}

// ---- RealtimeSource (test_support) -------------------------------------------------------

/// A sink that keeps every frame and counts frames that arrive after `stopped`.
#[derive(Default)]
struct CollectingSink {
    /// `(interleaved sample count, rate, channels, at)` per call.
    calls: Mutex<Vec<(usize, u32, u16, Instant)>>,
    samples: Mutex<Vec<f32>>,
    stopped: AtomicBool,
    after_stop: AtomicUsize,
}

impl CollectingSink {
    fn calls(&self) -> Vec<(usize, u32, u16, Instant)> {
        lock(&self.calls).clone()
    }
    fn seconds(&self) -> f64 {
        self.calls()
            .iter()
            .map(|(n, rate, channels, _)| *n as f64 / f64::from(*channels) / f64::from(*rate))
            .sum()
    }
}

impl FrameSink for CollectingSink {
    fn frames(&self, interleaved: &[f32], rate: u32, channels: u16, at: Instant) {
        if self.stopped.load(Ordering::SeqCst) {
            self.after_stop.fetch_add(1, Ordering::SeqCst);
        }
        lock(&self.calls).push((interleaved.len(), rate, channels, at));
        lock(&self.samples).extend_from_slice(interleaved);
    }
}

#[test]
fn realtime_source_paces_frames_by_real_time_and_pads_with_silence() {
    // Row 19 (analysis Q5): a 250 ms clip held for ~1 s yields about the hold's
    // length of audio (a Windows-sized budget), the clip first and then silence,
    // stamped with instants inside the capture. Bite: not paced (the whole clip,
    // or endless silence, at once), stopping when the data runs out (only
    // 250 ms), instants not stamped at push time.
    let clip = fixtures::speech_250ms();
    let source = RealtimeSource::from_buffer(&clip);
    let sink = Arc::new(CollectingSink::default());
    let started = Instant::now();
    let handle = source.start(sink.clone()).expect("start");
    thread::sleep(Duration::from_secs(1));
    let stop_called = Instant::now();
    handle.stop().expect("stop");
    let stopped = Instant::now();
    let held = (stop_called - started).as_secs_f64();
    let got = sink.seconds();
    assert!(
        got <= held + 0.25,
        "not paced: {got:.3} s of audio in a {held:.3} s hold"
    );
    assert!(
        got >= held * 0.5,
        "too little: {got:.3} s of audio in a {held:.3} s hold"
    );
    let calls = sink.calls();
    assert!(
        calls
            .iter()
            .all(|(_, rate, channels, _)| (*rate, *channels) == (16_000, 1)),
        "format: {:?}",
        calls.first()
    );
    assert!(
        calls.windows(2).all(|w| w[0].3 <= w[1].3)
            && calls.iter().all(|c| c.3 >= started && c.3 <= stopped),
        "instants not stamped at push time"
    );
    let samples = lock(&sink.samples).clone();
    let n = clip.samples().len();
    let head = AudioBuffer::from_frames(&samples[..n], 16_000, 1).expect("valid frames");
    assert!(
        head.samples()
            .iter()
            .zip(clip.samples())
            .all(|(a, b)| (i32::from(*a) - i32::from(*b)).abs() <= 1),
        "the clip is not the first {n} samples"
    );
    assert!(
        samples[n..].iter().all(|v| v.abs() < 1e-6),
        "not silence after the clip"
    );
}

#[test]
fn realtime_source_pushes_nothing_after_stop_or_drop() {
    // Row 19: once `stop` returns, or the handle is dropped, no frame reaches the
    // sink. Bite: a push thread that is not joined, a drop that does not stop.
    let source = RealtimeSource::from_buffer(&fixtures::speech_3s());
    for how in ["stop", "drop"] {
        let sink = Arc::new(CollectingSink::default());
        let handle = source.start(sink.clone()).expect("start");
        thread::sleep(ms(200));
        if how == "stop" {
            handle.stop().expect("stop");
        } else {
            drop(handle);
        }
        sink.stopped.store(true, Ordering::SeqCst);
        let count = sink.calls().len();
        assert!(count > 0, "{how}: nothing pushed in 200 ms");
        thread::sleep(ms(150));
        assert_eq!(
            sink.after_stop.load(Ordering::SeqCst),
            0,
            "{how}: frames after the capture ended"
        );
        assert_eq!(sink.calls().len(), count, "{how}");
    }
}

/// A PCM 16-bit little-endian WAV, header written out here (independent of
/// `wav::encode`).
fn pcm16_wav(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
    let data_len = u32::try_from(samples.len() * 2).expect("small");
    let block_align = channels * 2;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * u32::from(block_align)).to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[test]
fn read_wav_inverts_wav_encode_and_reads_stereo_at_any_rate() {
    // Row 19: wav::encode(speech_3s) reads back as 16 kHz mono with the same
    // samples (within 1 LSB after AudioBuffer::from_frames); a 44.1 kHz stereo
    // file keeps its rate, channel count and interleaving. Bite: a reader that
    // assumes 16 kHz mono, swaps channels, reads big-endian, or scales wrongly.
    let speech = fixtures::speech_3s();
    let encoded = wav::encode(&speech);
    assert_eq!(
        pcm16_wav(16_000, 1, speech.samples()),
        encoded,
        "premise: the hand-written header is wav::encode's"
    );
    let frames = read_wav(&encoded).expect("wav::encode output reads");
    assert_eq!((frames.rate, frames.channels), (16_000, 1));
    assert_eq!(frames.samples.len(), speech.samples().len());
    let back = AudioBuffer::from_frames(&frames.samples, 16_000, 1).expect("valid frames");
    assert!(
        back.samples()
            .iter()
            .zip(speech.samples())
            .all(|(a, b)| (i32::from(*a) - i32::from(*b)).abs() <= 1),
        "samples changed in the round trip"
    );
    assert!(RealtimeSource::from_wav(&encoded).is_ok());

    let stereo: [i16; 8] = [1000, -1000, 2000, -2000, 32767, -32768, 0, 7];
    let frames = read_wav(&pcm16_wav(44_100, 2, &stereo)).expect("stereo reads");
    assert_eq!((frames.rate, frames.channels), (44_100, 2));
    assert_eq!(frames.samples.len(), stereo.len());
    for (i, (got, want)) in frames.samples.iter().zip(stereo).enumerate() {
        assert!(
            (f64::from(*got) * 32767.0 - f64::from(want)).abs() <= 1.5,
            "sample {i}: {got}, expected about {want}/32768"
        );
    }
}

#[test]
fn read_wav_rejects_what_is_not_pcm16() {
    // Row 19: not RIFF, cut short, float or 8-bit samples, or zero channels is an
    // error, never a panic and never garbage audio. Bite: indexing past the end,
    // a division by a zero channel count, float bytes read as PCM16.
    let good = pcm16_wav(16_000, 1, &[1, 2, 3, 4]);
    let with = |offset: usize, bytes: &[u8]| {
        let mut w = good.clone();
        w[offset..offset + bytes.len()].copy_from_slice(bytes);
        w
    };
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty", Vec::new()),
        (
            "not RIFF",
            b"not a wav file, only some bytes here......".to_vec(),
        ),
        ("cut in the header", good[..30].to_vec()),
        ("float format", with(20, &3u16.to_le_bytes())),
        ("8-bit samples", with(34, &8u16.to_le_bytes())),
        ("zero channels", with(22, &0u16.to_le_bytes())),
    ];
    for (label, bytes) in cases {
        // A panic fails the test too.
        let got = read_wav(&bytes);
        assert!(got.is_err(), "{label}: accepted as {got:?}");
    }
    assert!(RealtimeSource::from_wav(b"junk").is_err());
}
