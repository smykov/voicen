//! T-210: failure feedback in the shell (spec 001 US2, FR-027 / FR-028; contracts/messages.md
//! "Where"; decisions #274, #279, #280). Windows CI only (decision #5): the file does not
//! compile on Linux, and without the change it does not compile on Windows either (no
//! `DictationPorts::notifier`, no `win::toast`).
//!
//! What is under test, through the real wiring (`build_app` on the mock runtime, the real
//! core `DictationSession` over core fakes, the real tray and logs-folder handlers):
//!
//! - the toast notifier (`voicen_lib::win::toast::ShellNotifier`): every offer on the toast
//!   surface becomes at most one toast, rendered in the settings' `ui_language`, built and
//!   shown only on the one `toast` thread, with no action and no launch argument; the one
//!   overlay-only message (`notice.logs_unwritable`) gives none; a failed show writes one
//!   `toast_failed` line and changes nothing else;
//! - the tray Retry (`tray::on_menu_event`): handed off the main thread, run with the
//!   settings of the click;
//! - the logs-folder notice (`logs_folder`): a failed open shows `error.open_logs_folder`
//!   with a `logs_reason.*` id, keeps its one `logs_folder_failed` line.
//!
//! The API this file assumes (docs/tasks/T-210.md `## Tests`; the developer implements it):
//!
//! ```text
//! voicen_lib::dictation::DictationPorts { .., notifier: Arc<dyn Notifier> }
//! voicen_lib::win::toast::ToastSink: Send + Sync {
//!     fn show(&self, xml: &str) -> Result<(), windows::core::Error>;   // the toast's XML
//! }
//! voicen_lib::win::toast::SpawnFn = fn(&str, Box<dyn FnOnce() + Send>) -> io::Result<()>;
//! voicen_lib::win::toast::ShellNotifier: Notifier + Send + Sync
//!     ::new(settings: Arc<SettingsService>, log: Arc<Log>, sink: Arc<dyn ToastSink>)
//!     ::with_spawn(settings, log, sink, spawn: SpawnFn)   // the toast thread is spawned here
//! ```
//!
//! The toast thread is named `toast`. `ShellNotifier::new` over the WinRT sink is `release_ports`'
//! constructor; here a fake sink stands in, so no toast reaches the runner's Action Center
//! (the real `CreateToastNotifierWithId` needs the installed Start-menu shortcut: the owner's
//! manual check on the installed app, decisions #5).
//!
//! The clock is read only through `timing_support`; every wait is a state wait.
#![cfg(windows)]

mod timing_support;
mod win32_support;

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, ThreadId};
use std::time::Duration;

use tauri::menu::{MenuEvent, MenuId};
use tauri::test::MockRuntime;
use tauri::Manager;
use voicen_core::audio::AudioBuffer;
use voicen_core::diag::Log;
use voicen_core::dictation::{DictationSession, SessionDeps};
use voicen_core::engine::{Engine, TranscribeRequest};
use voicen_core::events::RecordingObserver;
use voicen_core::failure::FailureReason;
use voicen_core::i18n::{self, UiLanguage};
use voicen_core::pipeline::{EngineFactory, PipelineDeps};
use voicen_core::platform::{
    AudioSource, CaptureHandle, DeviceId, FakeAudioSource, FakeCancelKey, FakeClipboard,
    FakeIndicator, FakePaster, FakeShellRequests, FakeTempAudioStore, FrameChunk, FrameSink,
    Indicator, InputDevice, Notifier, StartWindow, WindowRef,
};
use voicen_core::post_process::PassThrough;
use voicen_core::recording::{CaptureError, OverlayState, TrayState, MESSAGE_DURATION};
use voicen_core::secrets::{CredentialStore, KeyEdits};
use voicen_core::settings::service::{SaveOutcome, SaveRequest};
use voicen_core::settings::{EngineKind, Settings};
use voicen_core::test_support::fixtures;
use voicen_core::tray::TrayAction;
use voicen_core::vad::{EnergyDetector, SpeechDetector, SpeechGate};
use voicen_lib::dictation::{start_dictation, DictationPorts};
use voicen_lib::logs_folder::{self, FolderOpener};
use voicen_lib::tray::{self, TrayPart};
use voicen_lib::win::toast::{ShellNotifier, SpawnFn, ToastSink};
use win32_support::{
    assert_hotkey_free, eventually, holds_for, serial, AppRig, HOLD_LIMIT, SETTLE,
};
use windows::core::HRESULT;

/// `E_ACCESSDENIED` as a Win32-facility HRESULT: the log's `os_code` is its Win32 code, 5.
const ACCESS_DENIED: i32 = 0x8007_0005_u32 as i32;

// ---- fakes ---------------------------------------------------------------------

/// What one `ToastSink::show` call saw.
#[derive(Clone, Debug)]
struct Shown {
    xml: String,
    thread: ThreadId,
    thread_name: Option<String>,
}

#[derive(Default)]
struct SinkState {
    shown: Vec<Shown>,
    /// 1-based number of the one call that fails (exactly one: F-024).
    fail_call: Option<usize>,
    /// The first call blocks until `released` (or `HOLD_LIMIT`).
    block_first: bool,
    entered: bool,
    released: bool,
    /// Calls that returned (a blocked one returns only after the release).
    returned: usize,
}

/// The toast sink port's fake: records the XML and the thread of every call.
#[derive(Default)]
struct FakeSink {
    state: Mutex<SinkState>,
    changed: Condvar,
}

impl FakeSink {
    fn failing_call(n: usize) -> Arc<FakeSink> {
        let sink = FakeSink::default();
        win32_support::lock(&sink.state).fail_call = Some(n);
        Arc::new(sink)
    }

    fn blocking_first() -> Arc<FakeSink> {
        let sink = FakeSink::default();
        win32_support::lock(&sink.state).block_first = true;
        Arc::new(sink)
    }

    fn shown(&self) -> Vec<Shown> {
        win32_support::lock(&self.state).shown.clone()
    }

    fn returned(&self) -> usize {
        win32_support::lock(&self.state).returned
    }

    fn entered(&self) -> bool {
        win32_support::lock(&self.state).entered
    }

    fn release(&self) {
        win32_support::lock(&self.state).released = true;
        self.changed.notify_all();
    }
}

impl ToastSink for FakeSink {
    fn show(&self, xml: &str) -> Result<(), windows::core::Error> {
        let mut st = win32_support::lock(&self.state);
        st.shown.push(Shown {
            xml: xml.to_string(),
            thread: thread::current().id(),
            thread_name: thread::current().name().map(str::to_string),
        });
        let n = st.shown.len();
        if st.block_first && n == 1 {
            st.entered = true;
            self.changed.notify_all();
            st = self
                .changed
                .wait_timeout_while(st, HOLD_LIMIT, |s| !s.released)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        st.returned += 1;
        if st.fail_call == Some(n) {
            return Err(windows::core::Error::from_hresult(HRESULT(ACCESS_DENIED)));
        }
        Ok(())
    }
}

/// Releases the sink when the test ends, also by a failed assertion.
struct ReleaseSinkOnDrop(Arc<FakeSink>);

impl Drop for ReleaseSinkOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// A spawner that refuses: the toast thread cannot start.
fn refuse_spawn(_name: &str, _body: Box<dyn FnOnce() + Send>) -> io::Result<()> {
    Err(io::Error::from_raw_os_error(1455))
}

/// The composite indicator in miniature: tray to the shell's `TrayPart`, overlay recorded.
struct Recorder {
    tray: TrayPart<MockRuntime>,
    overlays: Mutex<Vec<OverlayState>>,
}

impl Recorder {
    fn overlays(&self) -> Vec<OverlayState> {
        win32_support::lock(&self.overlays).clone()
    }

    /// The messages the overlay showed: id string, params, `until`.
    fn messages(&self) -> Vec<(String, Vec<(&'static str, String)>, std::time::Instant)> {
        self.overlays()
            .into_iter()
            .filter_map(|o| match o {
                OverlayState::Message { id, params, until } => Some((id_str(id), params, until)),
                _ => None,
            })
            .collect()
    }
}

impl Indicator for Recorder {
    fn set_tray(&self, state: TrayState, retry_available: bool) {
        self.tray.set_tray(state, retry_available);
    }

    fn set_overlay(&self, state: &OverlayState) {
        win32_support::lock(&self.overlays).push(state.clone());
    }
}

/// The catalog id string of a `MessageId` (its serialized form).
fn id_str(id: i18n::MessageId) -> String {
    serde_json::to_value(id)
        .expect("a MessageId serializes")
        .as_str()
        .expect("a JSON string")
        .to_string()
}

/// A microphone whose next `start` can be held (the session holds its lock across it) and
/// then refuses with `AccessDenied`; every other start is the inner fake's.
struct GatedMic {
    inner: FakeAudioSource,
    gate_next: AtomicBool,
    /// (entered, released, returned)
    state: Mutex<(bool, bool, usize)>,
    changed: Condvar,
}

impl GatedMic {
    fn new() -> Arc<GatedMic> {
        Arc::new(GatedMic {
            inner: FakeAudioSource::new(),
            gate_next: AtomicBool::new(false),
            state: Mutex::new((false, false, 0)),
            changed: Condvar::new(),
        })
    }

    fn hold_next_start(&self) {
        self.gate_next.store(true, Ordering::SeqCst);
    }

    fn entered(&self) -> bool {
        win32_support::lock(&self.state).0
    }

    /// How many held starts have returned.
    fn returned(&self) -> usize {
        win32_support::lock(&self.state).2
    }

    fn release(&self) {
        win32_support::lock(&self.state).1 = true;
        self.changed.notify_all();
    }
}

impl AudioSource for GatedMic {
    fn devices(&self) -> Result<Vec<InputDevice>, CaptureError> {
        self.inner.devices()
    }

    fn start(
        &self,
        device: &DeviceId,
        sink: Arc<dyn FrameSink>,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        if !self.gate_next.swap(false, Ordering::SeqCst) {
            return self.inner.start(device, sink);
        }
        let mut st = win32_support::lock(&self.state);
        st.0 = true;
        self.changed.notify_all();
        st = self
            .changed
            .wait_timeout_while(st, HOLD_LIMIT, |s| !s.1)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
        st.2 += 1;
        Err(CaptureError::AccessDenied)
    }
}

struct ReleaseMicOnDrop(Arc<GatedMic>);

impl Drop for ReleaseMicOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// An engine that fails its first call with the network off and then answers; its
/// factory records the local-server base URL of the settings each engine was built from.
struct FlakyEngine {
    calls: AtomicUsize,
}

struct EngineRef(Arc<FlakyEngine>);

impl Engine for EngineRef {
    fn kind(&self) -> &'static str {
        "local_server"
    }

    fn transcribe(
        &self,
        _audio: &AudioBuffer,
        _req: &TranscribeRequest,
    ) -> Result<String, FailureReason> {
        if self.0.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(FailureReason::NetworkUnavailable)
        } else {
            Ok("hello from the fake engine".to_string())
        }
    }
}

fn flaky_factory(engine: &Arc<FlakyEngine>, urls: &Arc<Mutex<Vec<String>>>) -> Box<EngineFactory> {
    let engine = Arc::clone(engine);
    let urls = Arc::clone(urls);
    Box::new(move |s: &Settings, _c: &dyn CredentialStore| {
        win32_support::lock(&urls).push(s.local_server.base_url.clone());
        Ok(Box::new(EngineRef(Arc::clone(&engine))) as Box<dyn Engine>)
    })
}

/// A folder opener that always fails with an access-denied OS error.
struct DeniedOpener;

impl FolderOpener for DeniedOpener {
    fn open(&self, _dir: &std::path::Path) -> io::Result<()> {
        Err(io::Error::from_raw_os_error(5))
    }
}

// ---- helpers -------------------------------------------------------------------

const URL_BEFORE: &str = "http://127.0.0.1:8080/v1";
const URL_AFTER: &str = "http://127.0.0.1:8081/v1";

fn save(rig: &AppRig, edit: impl FnOnce(&mut Settings)) {
    let mut s = (*rig.service.snapshot()).clone();
    edit(&mut s);
    match rig.service.save(SaveRequest {
        settings: s,
        keys: KeyEdits::default(),
    }) {
        SaveOutcome::Saved { .. } => {}
        other => panic!("premise: the save is refused: {other:?}"),
    }
}

/// Engine `local_server` over a loopback fake URL, so a press passes the dictation gate.
fn select_local_server(rig: &AppRig) {
    save(rig, |s| {
        s.engine = EngineKind::LocalServer;
        s.local_server.base_url = URL_BEFORE.to_string();
    });
}

fn app_log(rig: &AppRig) -> Arc<Log> {
    let state = rig
        .app
        .try_state::<Arc<Log>>()
        .expect("build_app manages the log");
    Arc::clone(&state)
}

/// The shipped notifier over `sink`.
fn notifier(rig: &AppRig, sink: &Arc<FakeSink>) -> Arc<ShellNotifier> {
    let sink: Arc<dyn ToastSink> = sink.clone();
    Arc::new(ShellNotifier::new(
        Arc::clone(&rig.service),
        app_log(rig),
        sink,
    ))
}

fn start_session(
    rig: &AppRig,
    audio: Arc<dyn AudioSource>,
    notifier: Arc<dyn Notifier>,
    engine_factory: Option<Box<EngineFactory>>,
) -> (Arc<DictationSession>, Arc<Recorder>) {
    let recorder = Arc::new(Recorder {
        tray: tray::part(rig.app.handle()).expect("the tray part of a built tray"),
        overlays: Mutex::new(Vec::new()),
    });
    let session = DictationSession::start(SessionDeps {
        pipeline: PipelineDeps {
            gate: SpeechGate::new(
                Ok(Box::new(EnergyDetector::new()) as Box<dyn SpeechDetector>),
                EnergyDetector::new(),
            ),
            credentials: Arc::clone(&rig.creds),
            clipboard: Arc::new(FakeClipboard::new()),
            paster: Arc::new(FakePaster::new().with_start_window(Some(StartWindow {
                handle: WindowRef(0x0001_0051),
                process_id: 4242,
                elevated: false,
            }))),
            temp_audio: Arc::new(FakeTempAudioStore::new()),
            observer: Arc::new(RecordingObserver::new()),
            post_processor: Arc::new(PassThrough),
        },
        engine_factory,
        audio,
        indicator: recorder.clone(),
        requests: Arc::new(FakeShellRequests::new()),
        settings: Arc::clone(&rig.service),
        cancel_key: Arc::new(FakeCancelKey::new()),
        notifier,
    })
    .expect("the session starts");
    let session = Arc::new(session);
    rig.app.manage(Arc::clone(&session));
    (session, recorder)
}

/// `n` toasts shown within the hang bound.
#[track_caller]
fn wait_toasts(sink: &FakeSink, n: usize) {
    assert!(
        eventually(|| sink.shown().len() >= n),
        "{n} toast(s) not shown: {:?}",
        sink.shown()
    );
}

/// The XML text-node form of `text` for the characters a device name can carry.
fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A toast carries text only: no action, no launch argument, no retry argument.
#[track_caller]
fn assert_no_action(xml: &str) {
    let lower = xml.to_lowercase();
    for banned in ["<actions", "<action ", "launch=", "arguments=", "retry"] {
        assert!(
            !lower.contains(banned),
            "the toast carries `{banned}`: {xml}"
        );
    }
}

fn lines_of(rig: &AppRig, kind: &str) -> Vec<String> {
    let want = format!("kind={kind}");
    rig.log_lines()
        .into_iter()
        .filter(|l| l.split_whitespace().any(|w| w == want))
        .collect()
}

/// The words of a log line after its timestamp and level.
fn words(line: &str) -> Vec<&str> {
    line.split_whitespace().skip(2).collect()
}

fn mic_fallback(notifier: &dyn Notifier, device: &str) {
    notifier.notify(
        i18n::NOTICE_MIC_FALLBACK,
        &[("device", device.to_string())],
        None,
    );
}

/// A caller instant `by` before now (a hold ended well before the worker's clock).
fn ago(by: Duration) -> std::time::Instant {
    timing_support::now()
        .checked_sub(by)
        .expect("premise: the host has been up for longer than the test's backdating")
}

// ---- toasts ----------------------------------------------------------------------

#[test]
fn a_shown_failure_becomes_one_toast_in_the_ui_language_with_no_action() {
    // T-210 I5 / Acceptance 1 (FR-027): a refused microphone shows the failure on the
    // overlay and, through the Notifier, as exactly one toast whose text is the
    // catalog text of the very message the overlay shows, in the settings' ui_language
    // (en, then ru after a save), built and shown on the one `toast` thread (never the
    // test thread, which here is also the caller), with no action and no launch argument.
    // Bite: `NoToasts` still wired (no toast), the toast shown on the calling thread, text
    // in a fixed language or from the language at start, an action or launch argument in
    // the XML, two toasts for one message (one per publish), the message not rendered.
    let _serial = serial();
    let rig = AppRig::new(EngineKind::Api);
    select_local_server(&rig);
    let sink = Arc::new(FakeSink::default());
    let audio = Arc::new(FakeAudioSource::new());
    audio.set_start_error(Some(CaptureError::AccessDenied));
    let (session, recorder) = start_session(&rig, audio.clone(), notifier(&rig, &sink), None);
    let caller = thread::current().id();

    for (n, lang) in [(1, UiLanguage::En), (2, UiLanguage::Ru)] {
        if lang == UiLanguage::Ru {
            save(&rig, |s| s.ui_language = UiLanguage::Ru);
        }
        session.hotkey_pressed(timing_support::now());
        wait_toasts(&sink, n);
        let (id, params, _) = recorder
            .messages()
            .pop()
            .expect("the failure is on the overlay");
        let args: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let message = recorder
            .overlays()
            .into_iter()
            .rev()
            .find_map(|o| match o {
                OverlayState::Message { id, .. } => Some(id),
                _ => None,
            })
            .expect("a message id");
        assert_eq!(id_str(message), id);
        let text = i18n::text(lang, message, &args);
        let shown = sink.shown();
        assert!(
            shown[n - 1].xml.contains(&escaped(&text)),
            "toast {n} lacks the {lang:?} text {text:?}: {}",
            shown[n - 1].xml
        );
        assert_no_action(&shown[n - 1].xml);
        assert_ne!(shown[n - 1].thread, caller, "toast {n} shown on the caller");
        assert_eq!(
            shown[n - 1].thread_name.as_deref(),
            Some("toast"),
            "toast {n} not on the toast thread"
        );
    }
    assert!(
        holds_for(SETTLE, || sink.shown().len() == 2),
        "more than one toast per message: {:?}",
        sink.shown()
    );
}

#[test]
fn the_mic_fallback_notice_is_a_toast_with_the_device_name_built_through_the_dom() {
    // T-210 Acceptance 3 (decisions #108, #214): `notice.mic_fallback` "Using {device}"
    // toasts with the device in it. A device name holding XML metacharacters changes
    // nothing of the toast's structure (built through the DOM, P-009 / FR-20): it is in
    // the XML escaped, never raw. Bite: the message dropped by a filter that is too
    // narrow, the param not substituted, the XML built by string concatenation (a raw
    // `<A&B>` in the text).
    let _serial = serial();
    let rig = AppRig::new(EngineKind::Api);
    let sink = Arc::new(FakeSink::default());
    let n = notifier(&rig, &sink);

    mic_fallback(n.as_ref(), "USB Mic");
    wait_toasts(&sink, 1);
    let want = i18n::text(
        UiLanguage::En,
        i18n::NOTICE_MIC_FALLBACK,
        &[("device", "USB Mic")],
    );
    assert_eq!(want, "Using USB Mic", "premise: the contract text");
    assert!(
        sink.shown()[0].xml.contains(&want),
        "{}",
        sink.shown()[0].xml
    );
    assert_no_action(&sink.shown()[0].xml);

    mic_fallback(n.as_ref(), "Mic <A&B>");
    wait_toasts(&sink, 2);
    let xml = sink.shown()[1].xml.clone();
    assert!(
        !xml.contains("<A&B>"),
        "a raw device name in the XML: {xml}"
    );
    assert!(
        xml.contains("Using Mic &lt;A&amp;B"),
        "the escaped device name is not in the XML: {xml}"
    );
}

#[test]
fn the_overlay_only_logs_notice_gives_no_toast_and_the_next_offer_still_does() {
    // T-210 I5 (contracts/messages.md: `notice.logs_unwritable` is overlay only): offered
    // to the Notifier (T-007 I1), it is not shown. A later offer is (so the first was
    // filtered, not merely slow). Bite: no filter (a toast for it), a filter that also
    // drops the next offer, a filter by position.
    let _serial = serial();
    let rig = AppRig::new(EngineKind::Api);
    let sink = Arc::new(FakeSink::default());
    let n = notifier(&rig, &sink);

    n.notify(
        i18n::NOTICE_LOGS_UNWRITABLE,
        &[("reason", "logs_reason.permission_denied".to_string())],
        None,
    );
    mic_fallback(n.as_ref(), "USB Mic");
    wait_toasts(&sink, 1);

    assert!(
        holds_for(SETTLE, || sink.shown().len() == 1),
        "the logs notice became a toast: {:?}",
        sink.shown()
    );
    assert!(
        sink.shown()[0].xml.contains("Using USB Mic"),
        "the one toast is not the later offer: {}",
        sink.shown()[0].xml
    );
}

#[test]
fn a_ui_language_change_between_offers_changes_the_next_toasts_language() {
    // T-210 I5: the language is read from the settings at each toast, not at start. Bite:
    // the snapshot taken once when the notifier is built (the second toast stays English),
    // a hard-coded language.
    let _serial = serial();
    let rig = AppRig::new(EngineKind::Api);
    let sink = Arc::new(FakeSink::default());
    let n = notifier(&rig, &sink);

    mic_fallback(n.as_ref(), "USB Mic");
    wait_toasts(&sink, 1);
    save(&rig, |s| s.ui_language = UiLanguage::Ru);
    mic_fallback(n.as_ref(), "USB Mic");
    wait_toasts(&sink, 2);

    let en = i18n::text(
        UiLanguage::En,
        i18n::NOTICE_MIC_FALLBACK,
        &[("device", "USB Mic")],
    );
    let ru = i18n::text(
        UiLanguage::Ru,
        i18n::NOTICE_MIC_FALLBACK,
        &[("device", "USB Mic")],
    );
    assert_ne!(en, ru, "premise: the two catalogs differ");
    let shown = sink.shown();
    assert!(shown[0].xml.contains(&en), "{}", shown[0].xml);
    assert!(shown[1].xml.contains(&ru), "{}", shown[1].xml);
}

#[test]
fn the_dictation_ports_notifier_is_the_one_the_session_offers_to() {
    // T-210 wiring (DictationPorts gains `notifier`, `release_ports` builds the
    // ShellNotifier): through `start_dictation`, a press with a refused microphone is
    // shown on the ports' indicator and arrives at the ports' notifier as one toast
    // with that message's text. Bite: `start_dictation` ignoring `ports.notifier` (a
    // built-in no-op: no toast), a second inline notifier.
    let _serial = serial();
    assert_hotkey_free();
    let rig = AppRig::new(EngineKind::Api);
    select_local_server(&rig);
    let sink = Arc::new(FakeSink::default());
    let audio = Arc::new(FakeAudioSource::new());
    audio.set_start_error(Some(CaptureError::AccessDenied));
    let indicator = Arc::new(FakeIndicator::new());
    let _dictation = start_dictation(
        rig.app.handle(),
        DictationPorts {
            audio,
            clipboard: Arc::new(FakeClipboard::new()),
            paster: Arc::new(FakePaster::new()),
            engine_factory: None,
            indicator: indicator.clone(),
            credentials: Arc::clone(&rig.creds),
            hotkeys: Arc::clone(&rig.hotkeys),
            detector: Err(voicen_core::vad::VadError::Unavailable),
            temp_audio: Arc::new(FakeTempAudioStore::new()),
            notifier: notifier(&rig, &sink),
        },
    )
    .expect("start_dictation");

    rig.session().hotkey_pressed(timing_support::now());
    wait_toasts(&sink, 1);
    let message = indicator
        .overlays()
        .into_iter()
        .find_map(|o| match o {
            OverlayState::Message { id, params, .. } => Some((id, params)),
            _ => None,
        })
        .expect("the failure is on the overlay");
    let args: Vec<(&str, &str)> = message.1.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let text = i18n::text(UiLanguage::En, message.0, &args);
    assert!(
        sink.shown()[0].xml.contains(&escaped(&text)),
        "{}",
        sink.shown()[0].xml
    );
}

// ---- the tray's Retry ---------------------------------------------------------------

#[test]
fn the_tray_retry_returns_while_the_session_lock_is_held_and_runs_with_the_click_settings() {
    // T-210 I6 (tray half) / Acceptance 2: a failed recording is pending (tray Error with
    // Retry). The settings are saved (a new base URL), and a press is held inside
    // `AudioSource::start`, with the session lock held (windows-shell.md invariant 2).
    // The main thread's `on_menu_event` for the `retry` item must return while that
    // start has not (ordering, not a ceiling): it never takes the lock, it stamps `at` and
    // hands off with `spawn_blocking`. After the release the retry runs: a second engine
    // is built from the settings saved after the failure (the URL of the click), not from
    // the failed recording's. Bite: the Retry arm still empty (no second engine), the call
    // run inline on the main thread (the handler waits for the held start: `returned`
    // is 1 when it comes back), `retry(at, Some(id))` with an invented id, a snapshot of
    // the settings taken at the failure (the old URL).
    let _serial = serial();
    let rig = AppRig::new(EngineKind::Api);
    select_local_server(&rig);
    let mic = GatedMic::new();
    let _release = ReleaseMicOnDrop(Arc::clone(&mic));
    let engine = Arc::new(FlakyEngine {
        calls: AtomicUsize::new(0),
    });
    let urls = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::new(FakeSink::default());
    let (session, _recorder) = start_session(
        &rig,
        mic.clone(),
        notifier(&rig, &sink),
        Some(flaky_factory(&engine, &urls)),
    );

    // One recording that fails with the network off: pending, tray Error with Retry.
    let t0 = ago(Duration::from_secs(10));
    mic.inner.set_chunks(vec![FrameChunk::from_buffer(
        &fixtures::speech_3s(),
        t0 + Duration::from_millis(40),
    )]);
    session.hotkey_pressed(t0);
    session.hotkey_released(t0 + Duration::from_secs(3));
    assert!(
        eventually(|| tray::applied(rig.app.handle())
            .is_some_and(|a| a.state == TrayState::Error && a.retry_available)),
        "premise: tray Error with Retry: {:?}",
        tray::applied(rig.app.handle())
    );
    assert_eq!(*win32_support::lock(&urls), vec![URL_BEFORE.to_string()]);

    // The settings change after the failure; a press now holds the session lock.
    save(&rig, |s| s.local_server.base_url = URL_AFTER.to_string());
    mic.hold_next_start();
    let pressing = {
        let session = Arc::clone(&session);
        thread::spawn(move || session.hotkey_pressed(timing_support::now()))
    };
    assert!(
        eventually(|| mic.entered()),
        "premise: the press did not reach the held start"
    );

    tray::on_menu_event(
        rig.app.handle(),
        MenuEvent {
            id: MenuId::new(TrayAction::Retry.as_str()),
        },
    );
    let returned_before_release = mic.returned();
    mic.release();
    pressing.join().expect("the press thread");

    assert_eq!(
        returned_before_release, 0,
        "the Retry handler returned only after the held start did (it took the session lock \
         on the main thread)"
    );
    assert!(
        eventually(|| win32_support::lock(&urls).len() == 2),
        "the retry did not run after the release: engines built {:?}",
        win32_support::lock(&urls)
    );
    assert_eq!(
        *win32_support::lock(&urls),
        vec![URL_BEFORE.to_string(), URL_AFTER.to_string()],
        "the retry's engine is not built from the settings of the click"
    );
    assert!(
        eventually(|| engine.calls.load(Ordering::SeqCst) == 2),
        "the retry did not transcribe the pending recording"
    );
}

// ---- the logs-folder notice ----------------------------------------------------------

#[test]
fn a_failed_open_logs_folder_shows_the_notice_for_three_seconds_as_one_toast() {
    // T-210 I8 / Acceptance 4: the folder opener fails with an access-denied OS error on
    // the `logs-folder` thread. The overlay shows `error.open_logs_folder` with
    // `{reason}` = the `logs_reason.permission_denied` id (the core map, never OS text) for
    // 3 s from the failure, once; one toast carries its text; the tray is as it was;
    // the failure keeps its one `logs_folder_failed os_code=5` line. Bite: the notice not
    // raised (log only, as before), the OS text as the reason, a second map in the shell,
    // the notice raised on the main thread, a second toast, the tray set to Error, the
    // log line dropped when the notice is raised, a duration other than 3 s.
    let _serial = serial();
    let rig = AppRig::new(EngineKind::Api);
    let sink = Arc::new(FakeSink::default());
    let (_session, recorder) = start_session(
        &rig,
        Arc::new(FakeAudioSource::new()),
        notifier(&rig, &sink),
        None,
    );
    let target: PathBuf = rig.dir.path().join("opened");
    logs_folder::install(rig.app.handle(), target, Arc::new(DeniedOpener));
    let tray_before = tray::applied(rig.app.handle());
    let before = timing_support::now();

    tray::on_menu_event(
        rig.app.handle(),
        MenuEvent {
            id: MenuId::new(TrayAction::OpenLogs.as_str()),
        },
    );
    assert!(
        eventually(|| !recorder.messages().is_empty()),
        "no message after the failed open: {:?}",
        recorder.overlays()
    );
    let seen = timing_support::now();

    let shown = recorder.messages();
    assert_eq!(shown.len(), 1, "exactly one message: {shown:?}");
    let (id, params, until) = &shown[0];
    assert_eq!(id, "error.open_logs_folder");
    assert_eq!(
        params,
        &vec![("reason", "logs_reason.permission_denied".to_string())],
        "the reason is the logs_reason id of the error kind"
    );
    assert!(
        *until >= before + MESSAGE_DURATION && *until <= seen + MESSAGE_DURATION,
        "the notice is not shown for 3 s from the failure"
    );

    wait_toasts(&sink, 1);
    let text = i18n::text(
        UiLanguage::En,
        i18n::ERROR_OPEN_LOGS_FOLDER,
        &[("reason", "logs_reason.permission_denied")],
    );
    assert!(
        sink.shown()[0].xml.contains(&escaped(&text)),
        "{}",
        sink.shown()[0].xml
    );
    assert!(
        holds_for(SETTLE, || sink.shown().len() == 1
            && tray::applied(rig.app.handle()) == tray_before),
        "more than one toast, or the tray changed: {:?}, {:?}",
        sink.shown(),
        tray::applied(rig.app.handle())
    );

    assert!(
        eventually(|| !lines_of(&rig, "logs_folder_failed").is_empty()),
        "the failure's log line is gone"
    );
    let lines = lines_of(&rig, "logs_folder_failed");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(
        words(&lines[0]),
        vec!["warning", "kind=logs_folder_failed", "os_code=5"]
    );
}

// ---- failure branches ------------------------------------------------------------------

#[test]
fn a_toast_that_cannot_be_shown_writes_one_line_and_leaves_the_overlay_tray_and_next_toast() {
    // T-210 I5 / Acceptance 5 (FR-028): the sink fails exactly one call (the first, with
    // E_ACCESSDENIED; F-024: a sink that kept failing would let a swallowed error pass).
    // The refused microphone is still on the overlay and the tray shows Error; the log
    // has exactly one `warning kind=toast_failed os_code=5` line (kind and code, nothing
    // else); the next offer is shown. Bite: the error swallowed (no line), the line
    // written twice (per layer), the OS text or the XML in the line, the code dropped, a
    // panic or a stopped toast thread (the next offer is not shown), the overlay or tray
    // not set when the toast fails, a latch "failed once, never again".
    let _serial = serial();
    let rig = AppRig::new(EngineKind::Api);
    select_local_server(&rig);
    let sink = FakeSink::failing_call(1);
    let n = notifier(&rig, &sink);
    let audio = Arc::new(FakeAudioSource::new());
    audio.set_start_error(Some(CaptureError::AccessDenied));
    let (session, recorder) = start_session(&rig, audio.clone(), n.clone(), None);

    session.hotkey_pressed(timing_support::now());
    assert!(
        eventually(|| !lines_of(&rig, "toast_failed").is_empty()),
        "no toast_failed line after the failed show: {:?}",
        sink.shown()
    );
    let lines = lines_of(&rig, "toast_failed");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(
        words(&lines[0]),
        vec!["warning", "kind=toast_failed", "os_code=5"],
        "{:?}",
        lines[0]
    );
    assert!(
        !recorder.messages().is_empty(),
        "the failure is not on the overlay: {:?}",
        recorder.overlays()
    );
    assert!(
        eventually(|| tray::applied(rig.app.handle()).is_some_and(|a| a.state == TrayState::Error)),
        "the tray is not Error: {:?}",
        tray::applied(rig.app.handle())
    );

    mic_fallback(n.as_ref(), "Next Mic");
    wait_toasts(&sink, 2);
    assert!(
        sink.shown()[1].xml.contains("Using Next Mic"),
        "the next offer is not shown: {}",
        sink.shown()[1].xml
    );
    assert!(
        holds_for(SETTLE, || lines_of(&rig, "toast_failed").len() == 1),
        "a second toast_failed line: {:?}",
        lines_of(&rig, "toast_failed")
    );
}

#[test]
fn a_toast_sink_blocked_in_show_delays_neither_notify_nor_the_session() {
    // T-210 I5 (Notifier: must not block; `Show` is a COM call): the sink is held inside
    // its first `show`. Then `notify`, `DictationSession::notice_with` (the session lock)
    // and a failing press all return and take effect (the overlay shows them) while that
    // `show` has not returned (ordering, not a ceiling). After the release every offer is
    // shown, the first one first. Bite: `notify` showing inline (it waits for the held
    // sink: `returned` is 1 when it comes back), a bounded channel that fills and blocks
    // the caller, the show called under the session lock (the worker waits).
    let _serial = serial();
    let rig = AppRig::new(EngineKind::Api);
    select_local_server(&rig);
    let sink = FakeSink::blocking_first();
    let _release = ReleaseSinkOnDrop(Arc::clone(&sink));
    let n = notifier(&rig, &sink);
    let audio = Arc::new(FakeAudioSource::new());
    audio.set_start_error(Some(CaptureError::AccessDenied));
    let (session, recorder) = start_session(&rig, audio.clone(), n.clone(), None);

    mic_fallback(n.as_ref(), "First Mic");
    assert!(
        eventually(|| sink.entered()),
        "premise: the toast thread did not reach the sink"
    );
    mic_fallback(n.as_ref(), "Second Mic");
    session.notice_with(
        i18n::NOTICE_CHOOSE_ENGINE,
        Vec::new(),
        timing_support::now(),
    );
    session.hotkey_pressed(timing_support::now());
    assert!(
        eventually(|| recorder.messages().len() >= 2),
        "the session did not show its messages while the sink was blocked: {:?}",
        recorder.overlays()
    );
    let returned_before_release = sink.returned();
    sink.release();

    assert_eq!(
        returned_before_release, 0,
        "a caller returned only after the blocked show did"
    );
    wait_toasts(&sink, 3);
    let shown = sink.shown();
    assert!(shown[0].xml.contains("Using First Mic"), "{}", shown[0].xml);
    assert!(
        shown[1].xml.contains("Using Second Mic"),
        "{}",
        shown[1].xml
    );
}

#[test]
fn a_toast_thread_that_cannot_start_drops_the_offers_with_one_line_and_no_crash() {
    // T-210 I5 / Acceptance 5 (the toast thread cannot be started, the spawner refuses with
    // an OS error): one `toast_failed` line for the failed start, no toast, offers do not
    // crash or block, and no line per dropped offer; the overlay still shows messages. Bite:
    // an unwrap on the spawn (a panic), the failure silent (no line), a line per offer, the
    // offers shown inline on the caller when there is no thread.
    let _serial = serial();
    let rig = AppRig::new(EngineKind::Api);
    select_local_server(&rig);
    let sink = Arc::new(FakeSink::default());
    let dyn_sink: Arc<dyn ToastSink> = sink.clone();
    let spawn: SpawnFn = refuse_spawn;
    let n = Arc::new(ShellNotifier::with_spawn(
        Arc::clone(&rig.service),
        app_log(&rig),
        dyn_sink,
        spawn,
    ));

    assert!(
        eventually(|| !lines_of(&rig, "toast_failed").is_empty()),
        "no toast_failed line for the thread that did not start"
    );
    let audio = Arc::new(FakeAudioSource::new());
    audio.set_start_error(Some(CaptureError::AccessDenied));
    let (session, recorder) = start_session(&rig, audio.clone(), n.clone(), None);
    for _ in 0..3 {
        mic_fallback(n.as_ref(), "USB Mic");
    }
    session.hotkey_pressed(timing_support::now());

    assert!(
        eventually(|| !recorder.messages().is_empty()),
        "the overlay shows nothing: {:?}",
        recorder.overlays()
    );
    assert!(
        holds_for(SETTLE, || sink.shown().is_empty()
            && lines_of(&rig, "toast_failed").len() == 1),
        "toasts {:?}, toast_failed lines {:?}",
        sink.shown(),
        lines_of(&rig, "toast_failed")
    );
}
