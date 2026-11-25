//! T-210: failure feedback in the shell (spec 001 US2, FR-027 / FR-028; contracts/messages.md
//! "Where"; decisions #274, #279, #280). Windows CI only (decision #5): the file does not
//! compile on Linux, and without the change it does not compile on Windows either (no
//! `win::toast`).
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
//! voicen_lib::dictation::start_dictation_with(app, ports: DictationPorts, notifier: Arc<dyn Notifier>)
//! voicen_lib::win::toast::ToastSink: Send + Sync {
//!     fn show(&self, xml: &str) -> Result<(), windows::core::Error>;   // the toast's XML
//! }
//! voicen_lib::win::toast::SpawnFn = fn(&str, Box<dyn FnOnce() + Send>) -> io::Result<()>;
//! voicen_lib::win::toast::ShellNotifier: Notifier + Send + Sync
//!     ::new(settings: Arc<SettingsService>, log: Arc<Log>, sink: Arc<dyn ToastSink>)
//!     ::with_spawn(settings, log, sink, spawn: SpawnFn)   // the toast thread is spawned here
//! voicen_lib::win::toast::setting_gate(NotificationSetting) -> Result<(), windows::core::Error>
//!     // Enabled -> Ok; any other value -> Err(HRESULT_FROM_WIN32(ERROR_CANCELLED))
//! voicen_lib::win::toast::REFUSED_OS_CODE: i32 = 1223   // the logged code of a refused toast
//! ```
//!
//! T-213 adds the toast's buttons and the COM activator behind them (spec 001 US2 FR-027,
//! spec 007 FR-030; decisions #274 Q5, #279, #298, #312, #313). The API it assumes (the
//! developer implements it; the first compile error on a missing item is the red):
//!
//! ```text
//! voicen_core::platform::{ToastAction, ToastArg}              // tests/toast_support.rs (core)
//! voicen_core::platform::Notifier::notify(id, params, action: Option<ToastAction>)
//! voicen_lib::win::toast::
//!   TOAST_ACTIVATOR_CLSID: windows::core::GUID                // the one CLSID; the NSIS `!define` equals it
//!   type ToastRoute                                           // what an activation is handed to (opaque here)
//!   app_route<R: Runtime>(app: &AppHandle<R>) -> ToastRoute   // Retry -> DictationSession::retry via
//!       // spawn_blocking, Open -> call::open_calls(app, Some(CallRef { root: wire_root(root), id }))
//!   type RegisterFn = fn(&IClassFactory) -> Result<(), windows::core::Error>
//!   register_class_object: RegisterFn                         // the shipped one: CoRegisterClassObject
//!   ShellNotifier::attach(&self, route: ToastRoute)           // = attach_with(route, register_class_object)
//!   ShellNotifier::attach_with(&self, route: ToastRoute, register: RegisterFn)
//!       // posts to the toast thread, which registers the activator through `register`; offers after
//!       // it in the channel carry a button only when the register fn returned Ok
//! ```
//! The activator object is `IClassFactory::CreateInstance`d as `INotificationActivationCallback`
//! (needs the `Win32_UI_Notifications` feature of `windows`, a normal dependency of the shell; the
//! test binary sees the union of features). The logic tests take the factory from a fake
//! `RegisterFn` and call `Activate` on the object directly; only the one registration test goes
//! through `CoRegisterClassObject` + `CoCreateInstance(CLSCTX_LOCAL_SERVER)`, once per test
//! binary (F-012). Whether the same-process local-server lookup works on the runner is proven only
//! by the `wip/T-213` run.
//!
//! The toast thread is named `toast`. `ShellNotifier::new` over the WinRT sink is `release_ports`'
//! constructor; here a fake sink stands in, so no toast reaches the runner's Action Center
//! (the real `CreateToastNotifierWithId` needs the installed Start-menu shortcut: the owner's
//! manual check on the installed app, decisions #5).
//!
//! The clock is read only through `timing_support`; every wait is a state wait.
#![cfg(windows)]

mod call_shell_support;
mod timing_support;
mod win32_support;

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::thread::{self, ThreadId};
use std::time::Duration;

use tauri::menu::{MenuEvent, MenuId};
use tauri::test::MockRuntime;
use tauri::Manager;
use voicen_core::audio::AudioBuffer;
use voicen_core::call::storage::wire_root;
use voicen_core::call::CallId;
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
    Indicator, InputDevice, Notifier, PendingId, StartWindow, ToastAction, ToastArg, WindowRef,
};
use voicen_core::post_process::PassThrough;
use voicen_core::recording::{CaptureError, OverlayState, TrayState, MESSAGE_DURATION};
use voicen_core::secrets::{CredentialStore, KeyEdits};
use voicen_core::settings::service::{SaveOutcome, SaveRequest};
use voicen_core::settings::{EngineKind, Settings};
use voicen_core::test_support::fixtures;
use voicen_core::tray::TrayAction;
use voicen_core::vad::{EnergyDetector, SpeechDetector, SpeechGate};
use voicen_lib::dictation::{start_dictation_with, DictationPorts};
use voicen_lib::logs_folder::{self, FolderOpener};
use voicen_lib::tray::{self, TrayPart};
use voicen_lib::win::toast::{
    app_route, setting_gate, RegisterFn, ShellNotifier, SpawnFn, ToastRoute, ToastSink,
    REFUSED_OS_CODE, TOAST_ACTIVATOR_CLSID,
};
use win32_support::{
    assert_hotkey_free, eventually, holds_for, serial, AppRig, HOLD_LIMIT, SETTLE,
};
use windows::core::{w, Error as WinError, IUnknown, HRESULT, PCWSTR};
use windows::Win32::Foundation::ERROR_CANCELLED;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, IClassFactory, CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Notifications::INotificationActivationCallback;
use windows::UI::Notifications::NotificationSetting;

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
    /// The code that call fails with; `None` = `E_ACCESSDENIED`.
    fail_code: Option<HRESULT>,
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

    /// Fails exactly call `n` with `err`'s code (a refused show: the setting gate's error).
    fn refusing_call(n: usize, err: &windows::core::Error) -> Arc<FakeSink> {
        let sink = FakeSink::default();
        {
            let mut st = win32_support::lock(&sink.state);
            st.fail_call = Some(n);
            st.fail_code = Some(err.code());
        }
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
            let code = st.fail_code.unwrap_or(HRESULT(ACCESS_DENIED));
            return Err(windows::core::Error::from_hresult(code));
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

/// One overlay message as recorded: id string, params, `until`.
type ShownMessage = (String, Vec<(&'static str, String)>, std::time::Instant);

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
    fn messages(&self) -> Vec<ShownMessage> {
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
            transcript_sink: Arc::new(voicen_core::pipeline::DiscardTranscripts),
        },
        engine_factory,
        builtin: None,
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

/// The messages the call part offers to the one shared notifier (T-128, spec 007:
/// `CallPorts.notifier` is `run()`'s `SharedPorts.notifier`, the same instance the
/// dictation session gets through `start_dictation_with`).
const CALL_NOTICES: [i18n::MessageId; 30] = [
    i18n::NOTICE_CALL_REFUSED_MIC_NO_DEVICE,
    i18n::NOTICE_CALL_REFUSED_MIC_ACCESS_DENIED,
    i18n::NOTICE_CALL_REFUSED_MIC_BUSY,
    i18n::NOTICE_CALL_REFUSED_MIC_UNWRITABLE,
    i18n::NOTICE_CALL_REFUSED_MIC_MISSING,
    i18n::NOTICE_CALL_REFUSED_MIC_OTHER,
    i18n::NOTICE_CALL_REFUSED_SYSTEM_NO_DEVICE,
    i18n::NOTICE_CALL_REFUSED_SYSTEM_ACCESS_DENIED,
    i18n::NOTICE_CALL_REFUSED_SYSTEM_BUSY,
    i18n::NOTICE_CALL_REFUSED_SYSTEM_UNWRITABLE,
    i18n::NOTICE_CALL_REFUSED_SYSTEM_MISSING,
    i18n::NOTICE_CALL_REFUSED_SYSTEM_OTHER,
    i18n::NOTICE_CALL_REFUSED_ROOT_NO_DEVICE,
    i18n::NOTICE_CALL_REFUSED_ROOT_ACCESS_DENIED,
    i18n::NOTICE_CALL_REFUSED_ROOT_BUSY,
    i18n::NOTICE_CALL_REFUSED_ROOT_UNWRITABLE,
    i18n::NOTICE_CALL_REFUSED_ROOT_MISSING,
    i18n::NOTICE_CALL_REFUSED_ROOT_OTHER,
    i18n::NOTICE_CALL_SAVED,
    i18n::NOTICE_TRANSCRIPT_READY,
    i18n::NOTICE_PASS_FAILED_KEY,
    i18n::NOTICE_PASS_FAILED_NETWORK,
    i18n::NOTICE_PASS_FAILED_TIMEOUT,
    i18n::NOTICE_PASS_FAILED_PROVIDER,
    i18n::NOTICE_PASS_FAILED_STORAGE,
    i18n::NOTICE_PASS_FAILED_ENGINE_NOT_SET_UP,
    i18n::NOTICE_STOPPED_SLEEP,
    i18n::NOTICE_STOPPED_MAX_LENGTH,
    i18n::NOTICE_STOPPED_LOW_DISK,
    i18n::NOTICE_STOPPED_STORAGE,
];

#[test]
fn every_call_notice_offered_to_the_shared_notifier_becomes_one_toast_with_its_text() {
    // T-210 over T-128's shared notifier: the call part's offers (`notice.call_refused.*`,
    // `notice.call_saved`, `notice.transcript_ready`, `notice.pass_failed.*`,
    // `notice.stopped.*`) are on the toast surface; each becomes exactly one toast, in
    // order, with its catalog text in the settings' language and no action. Bite: a
    // toast-surface filter that keeps only the dictation's ids (no toast), a filter by
    // prefix that drops one family (fewer toasts), a toast per family (wrong text).
    let _serial = serial();
    let rig = AppRig::new(EngineKind::Api);
    let sink = Arc::new(FakeSink::default());
    let n = notifier(&rig, &sink);
    let lang = rig.service.snapshot().ui_language;

    for id in CALL_NOTICES {
        n.notify(id, &[], None);
    }
    wait_toasts(&sink, CALL_NOTICES.len());

    assert!(
        holds_for(SETTLE, || sink.shown().len() == CALL_NOTICES.len()),
        "not one toast per call notice: {:?}",
        sink.shown()
    );
    let shown = sink.shown();
    for (id, toast) in CALL_NOTICES.iter().zip(&shown) {
        let text = escaped(&i18n::text(lang, *id, &[]));
        assert!(
            toast.xml.contains(&text),
            "{}: the toast lacks {text:?}: {}",
            id_str(*id),
            toast.xml
        );
        assert_no_action(&toast.xml);
    }
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
    // T-210 wiring over T-128's shape (`run()` builds the one ShellNotifier as
    // `SharedPorts.notifier` and hands it to `start_dictation_with`): a press with a
    // refused microphone is shown on the ports' indicator and arrives at the given
    // notifier as one toast with that message's text. Bite: `start_dictation_with`
    // ignoring its notifier (a built-in no-op: no toast), a second inline notifier.
    let _serial = serial();
    assert_hotkey_free();
    let rig = AppRig::new(EngineKind::Api);
    select_local_server(&rig);
    let sink = Arc::new(FakeSink::default());
    let audio = Arc::new(FakeAudioSource::new());
    audio.set_start_error(Some(CaptureError::AccessDenied));
    let indicator = Arc::new(FakeIndicator::new());
    let _dictation = start_dictation_with(
        rig.app.handle(),
        DictationPorts {
            audio,
            clipboard: Arc::new(FakeClipboard::new()),
            paster: Arc::new(FakePaster::new()),
            engine_factory: None,
            builtin: None,
            indicator: indicator.clone(),
            credentials: Arc::clone(&rig.creds),
            hotkeys: Arc::clone(&rig.hotkeys),
            detector: Err(voicen_core::vad::VadError::Unavailable),
            temp_audio: Arc::new(FakeTempAudioStore::new()),
            history: voicen_lib::dictation::HistorySink::Discard,
        },
        notifier(&rig, &sink),
    )
    .expect("start_dictation_with");

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
fn the_setting_gate_lets_only_enabled_through_with_one_fixed_code() {
    // T-210 I5 / Acceptance 5 (review 1 #1): with notifications off for the app `Show`
    // does not fail, so `WinRtSink::show` reads `ToastNotifier::Setting()` and passes it
    // through `setting_gate` before `Show`. Only `Enabled` lets the toast through; every
    // other value is refused with the one documented code, `HRESULT_FROM_WIN32(
    // ERROR_CANCELLED)`, logged as `os_code=1223` (`REFUSED_OS_CODE`). Bite: the gate
    // `Ok` for every value (or `Err` for `Enabled`), a gate that lets one disabled value
    // through (a match on some variants only), a per-value or a generic code (`E_FAIL`,
    // `E_ACCESSDENIED`), a constant other than 1223.
    assert_eq!(REFUSED_OS_CODE, 1223);
    assert!(
        setting_gate(NotificationSetting::Enabled).is_ok(),
        "Enabled is refused"
    );
    let refused = HRESULT::from_win32(ERROR_CANCELLED.0);
    assert_eq!(
        refused,
        HRESULT(0x8007_04C7_u32 as i32),
        "premise: the Win32 HRESULT form"
    );
    for setting in [
        NotificationSetting::DisabledForApplication,
        NotificationSetting::DisabledForUser,
        NotificationSetting::DisabledByGroupPolicy,
        NotificationSetting::DisabledByManifest,
    ] {
        match setting_gate(setting) {
            Ok(()) => panic!("{setting:?} is let through"),
            Err(e) => assert_eq!(e.code(), refused, "{setting:?} is refused with {e:?}"),
        }
    }
}

#[test]
fn a_toast_refused_by_the_notification_setting_writes_one_line_and_leaves_the_overlay_tray_and_next_toast(
) {
    // T-210 I5 / Acceptance 5 "notifications off for the app" (review 1 #1): the sink
    // refuses exactly one call (the first, F-024) with the setting gate's error for
    // `DisabledForApplication`, the error `WinRtSink::show` returns when the app's
    // notifications are off. The refused microphone is still on the overlay and the tray
    // shows Error; the log has exactly one `warning kind=toast_failed os_code=1223` line
    // (one line per refused toast, like a failed one); the next offer is shown. Bite: the
    // gate's code not the Win32 ERROR_CANCELLED (another `os_code`, or none), the refused
    // show swallowed by the toast thread (no line), a line per layer (two), a latch
    // "refused once, never again" (the next offer not shown). The call of `setting_gate`
    // inside `WinRtSink::show` is not reached here (the fake sink stands in for the WinRT
    // one, which needs the installed shortcut): review and the owner's manual check.
    let _serial = serial();
    let rig = AppRig::new(EngineKind::Api);
    select_local_server(&rig);
    let refusal = setting_gate(NotificationSetting::DisabledForApplication)
        .expect_err("premise: DisabledForApplication is refused");
    let sink = FakeSink::refusing_call(1, &refusal);
    let n = notifier(&rig, &sink);
    let audio = Arc::new(FakeAudioSource::new());
    audio.set_start_error(Some(CaptureError::AccessDenied));
    let (session, recorder) = start_session(&rig, audio.clone(), n.clone(), None);

    session.hotkey_pressed(timing_support::now());
    assert!(
        eventually(|| !lines_of(&rig, "toast_failed").is_empty()),
        "no toast_failed line after the refused show: {:?}",
        sink.shown()
    );
    let lines = lines_of(&rig, "toast_failed");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(
        words(&lines[0]),
        vec!["warning", "kind=toast_failed", "os_code=1223"],
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

// ---- T-213: the toast's buttons and the COM activator ------------------------------------------

const CALL_ID: &str = "2026-11-14_101500";
const OTHER_CALL_ID: &str = "2026-11-15_090000";

/// A COM interface pointer moved across threads by the test.
struct SendCom<T>(T);
// SAFETY: the test only calls the free-threaded activator and the agile class factory (both
// created in the multithreaded apartment), so using them from another thread is sound.
unsafe impl<T> Send for SendCom<T> {}

/// The class factories the fake register fn was handed, in order.
static CAPTURED: Mutex<Vec<SendCom<IClassFactory>>> = Mutex::new(Vec::new());
/// How often `failing_register` ran.
static FAILING_REGISTERS: AtomicUsize = AtomicUsize::new(0);
/// The one notifier that registered the activator through COM (once per test binary, F-012).
static REGISTERED: OnceLock<Arc<ShellNotifier>> = OnceLock::new();

/// A `RegisterFn` that succeeds without touching COM and keeps the factory for the test.
fn capture_register(factory: &IClassFactory) -> Result<(), WinError> {
    win32_support::lock(&CAPTURED).push(SendCom(factory.clone()));
    Ok(())
}

/// A `RegisterFn` that fails the way a refused `CoRegisterClassObject` does (REGDB_E_CLASSNOTREG);
/// it fails exactly the calls it gets, one per attach (F-024).
fn failing_register(_factory: &IClassFactory) -> Result<(), WinError> {
    FAILING_REGISTERS.fetch_add(1, Ordering::SeqCst);
    Err(WinError::from_hresult(HRESULT(0x8004_0154_u32 as i32)))
}

/// Attaches `route` with the capturing register fn and waits until the toast thread called it.
#[track_caller]
fn attach_capturing(n: &ShellNotifier, route: ToastRoute) {
    let before = win32_support::lock(&CAPTURED).len();
    let register: RegisterFn = capture_register;
    n.attach_with(route, register);
    assert!(
        eventually(|| win32_support::lock(&CAPTURED).len() > before),
        "the toast thread did not register the activator after attach"
    );
}

/// The activator object, made by the last captured factory: the COM interface a click reaches.
#[track_caller]
fn activator() -> INotificationActivationCallback {
    let factory = win32_support::lock(&CAPTURED)
        .last()
        .map(|f| f.0.clone())
        .expect("premise: a register fn was handed the class factory");
    // SAFETY: a plain COM call on a live factory with no aggregation.
    unsafe { factory.CreateInstance(None::<&IUnknown>) }
        .expect("the class factory makes an INotificationActivationCallback")
}

/// `Activate` with the argument as Windows hands it: a NUL-terminated wide string that is valid
/// only for the call, the user-input array empty.
fn activate(cb: &INotificationActivationCallback, arg: &str) -> Result<(), WinError> {
    let wide: Vec<u16> = arg.encode_utf16().chain(Some(0)).collect();
    let aumid: PCWSTR = w!("dev.voicen.app");
    // SAFETY: `wide` outlives the call and is NUL-terminated; the input data is empty.
    unsafe { cb.Activate(aumid, PCWSTR(wide.as_ptr()), &[]) }
}

/// One attribute value of an XML start tag.
fn attr(tag: &str, name: &str) -> String {
    tag.split_once(&format!("{name}=\""))
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(value, _)| value.to_string())
        .unwrap_or_default()
}

/// `(content, arguments)` of every `<action>` element of a toast.
fn actions(xml: &str) -> Vec<(String, String)> {
    xml.split("<action ")
        .skip(1)
        .map(|segment| {
            let tag = segment.split('>').next().unwrap_or_default();
            (attr(tag, "content"), attr(tag, "arguments"))
        })
        .collect()
}

/// No button: no `<action>`, no launch argument.
#[track_caller]
fn assert_no_button(xml: &str) {
    let lower = xml.to_lowercase();
    for banned in ["<action", "launch=", "arguments="] {
        assert!(
            !lower.contains(banned),
            "the toast carries `{banned}`: {xml}"
        );
    }
}

fn call_id() -> CallId {
    CallId::parse(CALL_ID).expect("premise: a call id")
}

fn retry_of(n: u64) -> Option<ToastAction> {
    let id = PendingId::from_toast_arg(&format!("retry:{n}")).expect("premise: a pending id");
    Some(ToastAction::Retry(id))
}

fn network_down() -> (i18n::MessageId, Vec<(&'static str, String)>) {
    let reason = FailureReason::NetworkUnavailable;
    (reason.message_id(), reason.message_params())
}

/// A session whose first recording fails retryably, offered to a `ShellNotifier` whose
/// activator is registered through the capturing fake.
struct Retryable {
    rig: AppRig,
    sink: Arc<FakeSink>,
    notifier: Arc<ShellNotifier>,
    mic: Arc<GatedMic>,
    engine: Arc<FlakyEngine>,
    urls: Arc<Mutex<Vec<String>>>,
    session: Arc<DictationSession>,
    _release: ReleaseMicOnDrop,
}

/// Which register fn the fixture's notifier gets.
#[derive(Clone, Copy, PartialEq)]
enum Attach {
    Capture,
    /// `ShellNotifier::attach`: the shipped `CoRegisterClassObject`.
    Com,
}

fn retryable(lang: UiLanguage, attach: Attach) -> Retryable {
    let rig = AppRig::new(EngineKind::Api);
    select_local_server(&rig);
    if lang != UiLanguage::En {
        save(&rig, |s| s.ui_language = lang);
    }
    let mic = GatedMic::new();
    let release = ReleaseMicOnDrop(Arc::clone(&mic));
    let engine = Arc::new(FlakyEngine {
        calls: AtomicUsize::new(0),
    });
    let urls = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::new(FakeSink::default());
    let n = notifier(&rig, &sink);
    match attach {
        Attach::Capture => attach_capturing(&n, app_route(rig.app.handle())),
        Attach::Com => {
            n.attach(app_route(rig.app.handle()));
            assert!(
                REGISTERED.set(Arc::clone(&n)).is_ok(),
                "premise: the COM registration runs once per test binary"
            );
        }
    }
    let (session, _recorder) = start_session(
        &rig,
        mic.clone(),
        n.clone(),
        Some(flaky_factory(&engine, &urls)),
    );
    Retryable {
        rig,
        sink,
        notifier: n,
        mic,
        engine,
        urls,
        session,
        _release: release,
    }
}

impl Retryable {
    /// One recording that fails with the network off: pending, tray Error with Retry. Returns the
    /// failure's toast XML.
    #[track_caller]
    fn fail_once(&self) -> String {
        let t0 = ago(Duration::from_secs(10));
        self.mic.inner.set_chunks(vec![FrameChunk::from_buffer(
            &fixtures::speech_3s(),
            t0 + Duration::from_millis(40),
        )]);
        self.session.hotkey_pressed(t0);
        self.session.hotkey_released(t0 + Duration::from_secs(3));
        assert!(
            eventually(|| tray::applied(self.rig.app.handle())
                .is_some_and(|a| a.state == TrayState::Error && a.retry_available)),
            "premise: tray Error with Retry: {:?}",
            tray::applied(self.rig.app.handle())
        );
        wait_toasts(&self.sink, 1);
        self.sink.shown()[0].xml.clone()
    }

    /// The one button's arguments of the failure's toast.
    #[track_caller]
    fn offered_arg(&self, xml: &str) -> String {
        let found = actions(xml);
        assert_eq!(found.len(), 1, "exactly one action expected: {xml}");
        found[0].1.clone()
    }
}

#[test]
fn a_retryable_failure_toast_has_exactly_one_retry_button_in_the_ui_language() {
    // T-213 I10 / Acceptance 2: the failure's toast carries one `<action>` whose arguments are
    // `retry:<the offered pending id>` (the core codec's output, parsed back by the one reader) and
    // whose content is `action.retry` in the settings' ui_language (en, then ru), activationType
    // foreground. Bite: no `<actions>` (the button never built), the content in a fixed language,
    // two buttons, arguments other than the codec's (an id of 1, a launch argument), a background
    // activation.
    let _serial = serial();
    for lang in [UiLanguage::En, UiLanguage::Ru] {
        let t = retryable(lang, Attach::Capture);
        let xml = t.fail_once();
        let found = actions(&xml);
        assert_eq!(found.len(), 1, "{lang:?}: exactly one action: {xml}");
        assert_eq!(
            found[0].0,
            i18n::text(lang, i18n::ACTION_RETRY, &[]),
            "{lang:?}: the button's text"
        );
        assert!(
            matches!(ToastArg::parse(&found[0].1), Some(ToastArg::Retry(_))),
            "{lang:?}: the arguments {:?} are not a retry argument",
            found[0].1
        );
        assert!(
            xml.contains("activationType=\"foreground\""),
            "{lang:?}: the action is not a foreground activation: {xml}"
        );
        assert_eq!(
            xml.matches("arguments=").count(),
            1,
            "{lang:?}: an argument besides the button's (a launch argument): {xml}"
        );
        drop(t);
    }
}

#[test]
fn toast_buttons_follow_the_offer_and_the_attach() {
    // T-213 I10: a button only when the offer carries an action AND the activator is registered
    // in this process. Before attach, a retry offer shows plain text (a dead button is never
    // shown); after attach, a retry offer gets one `retry:<id>` button, an offer with no action
    // none, an Open offer (transcript ready) one `open:<call id>` button whose arguments hold no
    // root, with `action.open` in the ui_language. Bite: the action built whatever the
    // registration state, the button built for `None`, Retry and Open sharing one label, the
    // root written into the Open arguments.
    let _serial = serial();
    let rig = AppRig::new(EngineKind::Api);
    let sink = Arc::new(FakeSink::default());
    let n = notifier(&rig, &sink);
    let (id, params) = network_down();
    let open = || {
        Some(ToastAction::Open {
            call: call_id(),
            root: PathBuf::from(r"C:\kept-root"),
        })
    };

    n.notify(id, &params, retry_of(7));
    wait_toasts(&sink, 1);
    assert_no_button(&sink.shown()[0].xml);

    attach_capturing(&n, app_route(rig.app.handle()));
    n.notify(id, &params, retry_of(7));
    n.notify(id, &params, None);
    n.notify(i18n::NOTICE_TRANSCRIPT_READY, &[], open());
    wait_toasts(&sink, 4);
    let shown = sink.shown();
    assert_eq!(
        actions(&shown[1].xml),
        vec![("Retry".to_string(), "retry:7".to_string())],
        "{}",
        shown[1].xml
    );
    assert_no_button(&shown[2].xml);
    assert_eq!(
        actions(&shown[3].xml),
        vec![("Open".to_string(), format!("open:{CALL_ID}"))],
        "{}",
        shown[3].xml
    );
    assert!(
        !shown[3].xml.contains("kept-root"),
        "the root is in the toast: {}",
        shown[3].xml
    );

    save(&rig, |s| s.ui_language = UiLanguage::Ru);
    n.notify(id, &params, retry_of(7));
    n.notify(i18n::NOTICE_TRANSCRIPT_READY, &[], open());
    wait_toasts(&sink, 6);
    let shown = sink.shown();
    assert_eq!(actions(&shown[4].xml)[0].0, "Повторить", "{}", shown[4].xml);
    assert_eq!(actions(&shown[5].xml)[0].0, "Открыть", "{}", shown[5].xml);
}

#[test]
fn activating_the_offered_retry_argument_reruns_the_pending_recording_with_the_click_settings() {
    // T-213 I6 / Acceptance 2: through the COM interface (`Activate` on the object the class
    // factory makes), the argument the toast carried re-runs the pending recording with the
    // settings saved after the failure (a second engine from the new base URL). `Activate` is
    // called while a press holds the session lock inside `AudioSource::start`, and returns while
    // that start has not (ordering, not a ceiling: it never takes the lock, it stamps the instant
    // and hands off with `spawn_blocking`). Bite: the route empty (no second engine), the retry
    // run inline in `Activate` (`returned` is 1 when it comes back), a snapshot of the settings
    // taken at the failure (the old URL), an invented id.
    let _serial = serial();
    let t = retryable(UiLanguage::En, Attach::Capture);
    let xml = t.fail_once();
    let arg = t.offered_arg(&xml);
    assert_eq!(*win32_support::lock(&t.urls), vec![URL_BEFORE.to_string()]);
    let cb = activator();

    save(&t.rig, |s| s.local_server.base_url = URL_AFTER.to_string());
    t.mic.hold_next_start();
    let pressing = {
        let session = Arc::clone(&t.session);
        thread::spawn(move || session.hotkey_pressed(timing_support::now()))
    };
    assert!(
        eventually(|| t.mic.entered()),
        "premise: the press did not reach the held start"
    );
    let done = Arc::new(AtomicBool::new(false));
    let activating = {
        let (cb, arg, done) = (SendCom(cb.clone()), arg.clone(), Arc::clone(&done));
        thread::spawn(move || {
            let cb = cb;
            let result = activate(&cb.0, &arg);
            done.store(true, Ordering::SeqCst);
            result
        })
    };
    assert!(
        eventually(|| done.load(Ordering::SeqCst)),
        "Activate did not return while the session lock was held"
    );
    let returned_before_release = t.mic.returned();
    t.mic.release();
    pressing.join().expect("the press thread");
    activating
        .join()
        .expect("the activate thread did not panic")
        .expect("Activate returns S_OK");

    assert_eq!(
        returned_before_release, 0,
        "Activate returned only after the held start did (it took the session lock)"
    );
    assert!(
        eventually(|| win32_support::lock(&t.urls).len() == 2),
        "the retry did not run: engines built {:?}",
        win32_support::lock(&t.urls)
    );
    assert_eq!(
        *win32_support::lock(&t.urls),
        vec![URL_BEFORE.to_string(), URL_AFTER.to_string()],
        "the retry's engine is not built from the settings of the click"
    );
    assert!(
        eventually(|| t.engine.calls.load(Ordering::SeqCst) == 2),
        "the retry did not transcribe the pending recording"
    );
}

#[test]
fn stale_foreign_malformed_and_empty_arguments_run_nothing_and_do_not_panic() {
    // T-213 I6 / Acceptance 3: with one recording pending, every argument but the offered one
    // returns S_OK and runs nothing: no engine built, no request. That covers an empty argument
    // (a click on the toast's body), a malformed one, another prefix or case, trailing text, a
    // neighbouring id, `retry:1` (an id an earlier process's toast may carry: ids rise across
    // processes), an `open:` argument for a call this process never offered, and a very long
    // string. The offered argument then still runs (the route is live: the silence is the
    // filter's), and once the recording is delivered the same argument runs nothing again.
    // Bite: `Activate` retrying on any argument or on an empty one, the codec bypassed
    // (`retry:` read as id 0, a prefix match), `retry(at, None)` (retries whatever is pending),
    // a panic on a malformed argument, a non-S_OK answer.
    let _serial = serial();
    let t = retryable(UiLanguage::En, Attach::Capture);
    let xml = t.fail_once();
    let arg = t.offered_arg(&xml);
    let Some(ToastArg::Retry(_)) = ToastArg::parse(&arg) else {
        panic!("premise: a retry argument: {arg:?}")
    };
    let n: u64 = arg
        .strip_prefix("retry:")
        .and_then(|d| d.parse().ok())
        .expect("premise: retry:<u64>");
    let cb = activator();

    let mut bad: Vec<String> = vec![
        String::new(),
        "retry:".into(),
        "retry:x".into(),
        "retry:-1".into(),
        "retry:1".into(),
        format!("retry:{}", n.wrapping_add(1)),
        format!("retry:{}", n.wrapping_sub(1)),
        format!("Retry:{n}"),
        format!("retry:{n} "),
        format!(" retry:{n}"),
        format!("retry:{n}:{n}"),
        "open:".into(),
        "open:not-a-call-id".into(),
        format!("open:{CALL_ID}"),
        "garbage".into(),
        "retry".into(),
        "x".repeat(10_000),
        "\u{0}".into(),
    ];
    bad.retain(|b| *b != arg);
    for b in &bad {
        activate(&cb, b).unwrap_or_else(|e| panic!("Activate({:.30?}) is not S_OK: {e:?}", b));
    }
    assert!(
        holds_for(SETTLE, || t.engine.calls.load(Ordering::SeqCst) == 1
            && win32_support::lock(&t.urls).len() == 1),
        "an argument that must run nothing ran a retry: engines {:?}, calls {}",
        win32_support::lock(&t.urls),
        t.engine.calls.load(Ordering::SeqCst)
    );

    activate(&cb, &arg).expect("Activate of the offered argument");
    assert!(
        eventually(|| t.engine.calls.load(Ordering::SeqCst) == 2),
        "the offered argument did not run after the silent ones: the route is dead"
    );
    assert!(
        eventually(|| tray::applied(t.rig.app.handle()).is_some_and(|a| !a.retry_available)),
        "premise: the delivered retry leaves nothing pending: {:?}",
        tray::applied(t.rig.app.handle())
    );
    activate(&cb, &arg).expect("Activate of a consumed argument");
    assert!(
        holds_for(SETTLE, || t.engine.calls.load(Ordering::SeqCst) == 2),
        "a consumed (stale) argument ran a retry again"
    );
}

/// Every payload of `event` heard anywhere in the app, in order.
fn any_payloads(app: &tauri::AppHandle<MockRuntime>, event: &str) -> Receiver<serde_json::Value> {
    use tauri::Listener;
    let (tx, rx) = mpsc::channel();
    app.listen_any(event, move |e| {
        let _ = tx.send(serde_json::from_str(e.payload()).expect("event payload is JSON"));
    });
    rx
}

fn drain(rx: &Receiver<serde_json::Value>, heard: &mut Vec<serde_json::Value>) {
    loop {
        match rx.try_recv() {
            Ok(v) => heard.push(v),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
        }
    }
}

/// `calls_list` invoked from the Calls window, like its page does after registering its
/// listeners.
fn list_from(window: &tauri::WebviewWindow<MockRuntime>) {
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::get_ipc_response;
    use tauri::webview::InvokeRequest;
    get_ipc_response(
        window,
        InvokeRequest {
            cmd: "calls_list".into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: window.url().expect("window url"),
            body: InvokeBody::Json(serde_json::json!({})),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    )
    .unwrap_or_else(|e| panic!("calls_list rejected: {e:?}"));
}

#[test]
fn activating_open_with_no_calls_window_builds_it_and_selects_the_call_under_the_notices_root() {
    // T-213 I6 / Acceptance 4 (decisions #298, #312, #313; spec 007 FR-030): the "Transcript
    // ready" toast carries `open:<call id>` only. With no Calls window (the usual case after a
    // call), `Activate` of that argument builds the window through `call::open_calls` and, after
    // the page's first `calls_list`, emits exactly one `calls://open {root, id}` with the root the
    // TranscriptReady notice carried (a root that is not the settings' root). An `open:` for a call
    // this process never offered builds nothing and emits nothing. Bite: `open_transcript` only
    // (no window, nothing shown), the settings' root instead of the notice's, the root read from
    // the argument, a second emit, an unoffered call opening a window.
    let _serial = serial();
    let (rig, _source) = call_shell_support::CallRig::with_wav();
    let sink = Arc::new(FakeSink::default());
    let dyn_sink: Arc<dyn ToastSink> = sink.clone();
    let log = rig
        .handle()
        .try_state::<Arc<Log>>()
        .expect("build_app manages the log");
    let n = ShellNotifier::new(Arc::clone(&rig.service), Arc::clone(&log), dyn_sink);
    attach_capturing(&n, app_route(rig.handle()));
    let offered_root = rig.dir.path().join("kept-root");
    let settings_root = rig.dir.path().join("calls");
    assert_ne!(offered_root, settings_root, "premise: two roots");

    n.notify(
        i18n::NOTICE_TRANSCRIPT_READY,
        &[],
        Some(ToastAction::Open {
            call: call_id(),
            root: offered_root.clone(),
        }),
    );
    wait_toasts(&sink, 1);
    let found = actions(&sink.shown()[0].xml);
    assert_eq!(found.len(), 1, "{}", sink.shown()[0].xml);
    let arg = found[0].1.clone();
    assert_eq!(arg, format!("open:{CALL_ID}"));
    let cb = activator();
    let heard_any = any_payloads(rig.handle(), "calls://open");
    let mut heard = Vec::new();

    activate(&cb, &format!("open:{OTHER_CALL_ID}")).expect("Activate of an unoffered call");
    assert!(
        call_shell_support::holds_for_a_while(|| {
            drain(&heard_any, &mut heard);
            heard.is_empty()
                && voicen_lib::call::calls_opened(rig.handle()) == 0
                && rig.handle().get_webview_window("calls").is_none()
        }),
        "an unoffered call opened the Calls window or emitted: {heard:?}"
    );

    activate(&cb, &arg).expect("Activate of the offered open argument");
    assert!(
        call_shell_support::eventually(|| rig.handle().get_webview_window("calls").is_some()),
        "the Calls window was not built by the Open click"
    );
    let window = rig
        .handle()
        .get_webview_window("calls")
        .expect("the calls window");
    list_from(&window);
    let want = serde_json::json!({ "root": wire_root(&offered_root), "id": CALL_ID });
    assert!(
        call_shell_support::eventually(|| {
            drain(&heard_any, &mut heard);
            heard.contains(&want)
        }),
        "no calls://open {want} after the page's first list; heard {heard:?}"
    );
    assert!(
        call_shell_support::holds_for_a_while(|| {
            drain(&heard_any, &mut heard);
            heard == vec![want.clone()]
        }),
        "calls://open must be heard exactly once with the notice's root: {heard:?}"
    );
}

#[test]
fn the_registered_activator_is_reachable_through_com_and_runs_the_offered_retry() {
    // T-213 Acceptance 2 (registration): `ShellNotifier::attach` registers the activator with
    // `CoRegisterClassObject(CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE)` on the toast thread; a
    // `CoCreateInstance(TOAST_ACTIVATOR_CLSID, CLSCTX_LOCAL_SERVER)` in this process then yields
    // the activator, and its `Activate` with the offered argument re-runs the pending recording.
    // Registered once per test binary (the notifier is kept in a static, F-012). Bite: no
    // registration (the class is not found), another CLSID than the const, a registration that
    // is not multi-use or not local-server, the register call on the caller's thread, a factory
    // that hands out an object whose `Activate` does nothing. Whether the same-process
    // local-server lookup works on the runner is proven only by the `wip/T-213` run.
    let _serial = serial();
    let t = retryable(UiLanguage::En, Attach::Com);
    // SAFETY: initialising COM for this thread; an already-initialised apartment is fine.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let mut made: Option<INotificationActivationCallback> = None;
    assert!(
        eventually(|| {
            // SAFETY: a plain COM call with no aggregation.
            made = unsafe {
                CoCreateInstance(
                    &TOAST_ACTIVATOR_CLSID,
                    None::<&IUnknown>,
                    CLSCTX_LOCAL_SERVER,
                )
            }
            .ok();
            made.is_some()
        }),
        "CoCreateInstance(TOAST_ACTIVATOR_CLSID, CLSCTX_LOCAL_SERVER) never found the activator"
    );
    let cb = made.expect("the activator");
    let xml = t.fail_once();
    let arg = t.offered_arg(&xml);
    save(&t.rig, |s| s.local_server.base_url = URL_AFTER.to_string());

    activate(&cb, &arg).expect("Activate through the registered class");
    assert!(
        eventually(|| win32_support::lock(&t.urls).len() == 2),
        "Activate through COM did not re-run the pending recording: {:?}",
        win32_support::lock(&t.urls)
    );
    assert_eq!(
        win32_support::lock(&t.urls)[1],
        URL_AFTER,
        "the retry is not built from the settings of the click"
    );
    assert!(REGISTERED
        .get()
        .is_some_and(|n| Arc::ptr_eq(n, &t.notifier)));
}

#[test]
fn an_activator_that_cannot_be_registered_writes_one_line_and_toasts_show_without_buttons() {
    // T-213 Acceptance 7 (#279): the register fn fails once (REGDB_E_CLASSNOTREG; one call, F-024).
    // The toast thread writes exactly one new `warning` line (kind and `os_code=<code>`, no OS
    // text) and keeps buttons off: a retry offer and an Open offer are still shown as plain
    // toasts, the next offer is shown too, nothing crashes and no second line is written. Bite:
    // the error swallowed (no line), a line per offer, an unwrap (the toast thread dies, the next
    // offer is not shown), buttons built anyway (a dead Retry), the register fn retried on every
    // offer.
    let _serial = serial();
    let rig = AppRig::new(EngineKind::Api);
    let sink = Arc::new(FakeSink::default());
    let n = notifier(&rig, &sink);
    let warnings = |rig: &AppRig| -> Vec<String> {
        rig.log_lines()
            .into_iter()
            .filter(|l| words(l).first() == Some(&"warning"))
            .collect()
    };
    let before = warnings(&rig).len();
    let calls_before = FAILING_REGISTERS.load(Ordering::SeqCst);

    let register: RegisterFn = failing_register;
    n.attach_with(app_route(rig.app.handle()), register);
    assert!(
        eventually(|| warnings(&rig).len() == before + 1),
        "no warning line after the failed registration: {:?}",
        rig.log_lines()
    );
    let lines = warnings(&rig);
    let line = lines.last().expect("the new line");
    let w = words(line);
    assert!(
        w.len() == 3 && w[2].starts_with("os_code="),
        "the line is not `warning kind=<kind> os_code=<code>`: {line:?}"
    );

    let (id, params) = network_down();
    n.notify(id, &params, retry_of(7));
    n.notify(
        i18n::NOTICE_TRANSCRIPT_READY,
        &[],
        Some(ToastAction::Open {
            call: call_id(),
            root: PathBuf::from(r"C:\kept-root"),
        }),
    );
    mic_fallback(n.as_ref(), "Next Mic");
    wait_toasts(&sink, 3);
    let shown = sink.shown();
    for toast in &shown {
        assert_no_button(&toast.xml);
    }
    assert!(
        shown[2].xml.contains("Using Next Mic"),
        "the next offer is not shown: {}",
        shown[2].xml
    );
    assert!(
        holds_for(SETTLE, || warnings(&rig).len() == before + 1
            && FAILING_REGISTERS.load(Ordering::SeqCst) == calls_before + 1),
        "more than one line or one registration attempt: {:?}, attempts {}",
        warnings(&rig),
        FAILING_REGISTERS.load(Ordering::SeqCst) - calls_before
    );
}

#[test]
fn the_installer_hooks_define_exactly_the_clsid_the_activator_registers() {
    // T-213 I7: one CLSID, a Rust const and one `!define VOICEN_TOAST_CLSID` in
    // `src-tauri/windows/installer-hooks.nsh`, which the shortcut property and the HKCU key are
    // written from. Bite: the define missing or twice, a CLSID in the hooks that is not the
    // const (the installed shortcut then names a class nothing registers), a second literal.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("windows")
        .join("installer-hooks.nsh");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let defines: Vec<Vec<&str>> = text
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>())
        .filter(|t| t.first() == Some(&"!define") && t.get(1) == Some(&"VOICEN_TOAST_CLSID"))
        .collect();
    assert_eq!(
        defines.len(),
        1,
        "exactly one `!define VOICEN_TOAST_CLSID`: {defines:?}"
    );
    let hex = |s: &str| -> String {
        s.chars()
            .filter(char::is_ascii_hexdigit)
            .collect::<String>()
            .to_lowercase()
    };
    let in_hooks = hex(defines[0].get(2).copied().unwrap_or_default());
    let in_rust = hex(&format!("{TOAST_ACTIVATOR_CLSID:?}"));
    assert_eq!(in_hooks.len(), 32, "the define holds no GUID: {defines:?}");
    assert_ne!(in_rust, "0".repeat(32), "the const is the zero GUID");
    assert_eq!(
        in_hooks, in_rust,
        "the installer's CLSID is not TOAST_ACTIVATOR_CLSID"
    );
}
