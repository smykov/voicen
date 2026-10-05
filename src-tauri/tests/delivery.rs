//! T-006 Acceptance line 3 (failure branch) and line 4 ("Test that fails without the
//! change: src-tauri/tests/delivery.rs"): the dictation wiring
//! (`voicen_lib::dictation::start_dictation` over `build_app`, as `run()`) with the real
//! `WinClipboard` and `WinPaster` where the branch needs them, and the session's inputs
//! called directly (no injected keys), so each failure branch is decided by the real
//! adapters and core's delivery table:
//! - the start window replaced by another before delivery -> no Ctrl+V, the text in the
//!   clipboard, `notice.copied_paste_manually`, `result=copy_manual` (row 19);
//! - an elevated start window -> the same, and no `send_ctrl_v` at all (row 20);
//! - microphone access denied -> no Recording, `failure.microphone_unavailable` with
//!   `mic_reason.access_denied`, `outcome=capture_failed mic=access_denied`, clipboard
//!   untouched, no engine call (row 21);
//! - engine none -> no capture, one settings window on Engine through the opener,
//!   `notice.choose_engine` (row 22);
//! - the session is managed as `Arc<DictationSession>` and `ShellIndicator` forwards the
//!   tray half to the tray (row 23).
//!
//! The visible notice reaches the screen only with T-057 (overlay) or T-007 (toast), so the
//! notice is asserted on the `Indicator` port (T-006 Q6). Windows CI only (decision #5).
//! Runner capabilities (docs/decisions/windows-ci-runner.md, `ok` in runs A and B):
//! `foreground` and `clipboard` (rows 19-21), asserted loudly by `win32_support`; rows 22
//! and 23 need none (tauri's mock runtime). Fake key, fake texts, a `TempDir` per test.
#![cfg(windows)]

mod win32_support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tauri::Manager;
use voicen_core::dictation::DictationSession;
use voicen_core::platform::{
    FakeAudioSource, FakeIndicator, FrameChunk, Indicator, PasteError, Paster, StartWindow,
    WindowRef,
};
use voicen_core::recording::{CaptureError, OverlayState, TrayState};
use voicen_core::settings::EngineKind;
use voicen_core::test_support::fixtures;
use voicen_lib::dictation::{start_dictation, DictationHandle, DictationPorts, ShellIndicator};
use voicen_lib::settings_window::LABEL;
use voicen_lib::tray;
use voicen_lib::win::clipboard::WinClipboard;
use voicen_lib::win::paste::WinPaster;
use win32_support::{
    assert_hotkey_free, clipboard_text, dictation_lines, engine_factory, eventually, has_pair,
    holds_for, lock, put_text, serial, shown, window_ref, AppRig, Gate, OpenOnDrop, Shape, Shown,
    TestWindow, TextEngine, BUDGET, HOLD, SETTLE,
};

const TEXT: &str = "delivery test text – текст";

/// `WinPaster` with a record of what it captured and how often Ctrl+V was sent; with
/// `elevated`, the captured start window is reported as elevated (row 20: a CI runner
/// cannot host a target above its own integrity level).
struct Watched {
    inner: WinPaster,
    elevated: bool,
    captured: Mutex<Vec<Option<StartWindow>>>,
    sends: AtomicUsize,
}

impl Watched {
    fn new(elevated: bool) -> Arc<Watched> {
        Arc::new(Watched {
            inner: WinPaster::new(),
            elevated,
            captured: Mutex::new(Vec::new()),
            sends: AtomicUsize::new(0),
        })
    }

    fn captured(&self) -> Vec<Option<StartWindow>> {
        lock(&self.captured).clone()
    }

    fn sends(&self) -> usize {
        self.sends.load(Ordering::SeqCst)
    }
}

impl Paster for Watched {
    fn capture_start_window(&self) -> Option<StartWindow> {
        let w = self.inner.capture_start_window().map(|w| StartWindow {
            elevated: w.elevated || self.elevated,
            ..w
        });
        lock(&self.captured).push(w.clone());
        w
    }

    fn wait_modifiers_released(&self, max_wait: Duration) -> bool {
        self.inner.wait_modifiers_released(max_wait)
    }

    fn is_in_front(&self, w: &StartWindow) -> bool {
        self.inner.is_in_front(w)
    }

    fn send_ctrl_v(&self) -> Result<(), PasteError> {
        self.sends.fetch_add(1, Ordering::SeqCst);
        self.inner.send_ctrl_v()
    }
}

/// Three seconds of speech, delivered at once by every capture.
fn speaking_mic() -> Arc<FakeAudioSource> {
    let audio = Arc::new(FakeAudioSource::new());
    audio.set_chunks(vec![FrameChunk::from_buffer(
        &fixtures::speech_3s(),
        Instant::now() + Duration::from_millis(10),
    )]);
    audio
}

fn start(
    rig: &AppRig,
    audio: Arc<FakeAudioSource>,
    paster: Arc<dyn Paster>,
    engine: &Arc<TextEngine>,
    indicator: Arc<dyn Indicator>,
) -> DictationHandle {
    start_dictation(
        rig.app.handle(),
        DictationPorts {
            audio,
            clipboard: Arc::new(WinClipboard::new()),
            paster,
            engine_factory: Some(engine_factory(engine)),
            indicator,
            credentials: Arc::clone(&rig.creds),
        },
    )
    .expect("start_dictation")
}

/// A hold of `HOLD` through the session's inputs.
fn hold(session: &DictationSession) {
    session.hotkey_pressed(Instant::now());
    thread::sleep(HOLD);
    session.hotkey_released(Instant::now());
}

/// The one dictation line, once written.
#[track_caller]
fn one_dictation_line(rig: &AppRig) -> String {
    assert!(
        eventually(BUDGET, || !dictation_lines(&rig.log_lines()).is_empty()),
        "no dictation line within {BUDGET:?}"
    );
    let lines = dictation_lines(&rig.log_lines());
    assert_eq!(lines.len(), 1, "{lines:?}");
    lines[0].clone()
}

fn copied_paste_manually() -> Shown {
    Shown::Message("notice.copied_paste_manually", vec![])
}

#[test]
fn start_window_replaced_before_delivery_gives_copy_manual_and_no_paste() {
    // T-006 row 19 (Acceptance line 3, first branch; FR-009): the start window A is
    // captured at the press; while the engine works, window B comes to the front; then
    // no window receives Ctrl+V, the clipboard holds the text, the indicator shows
    // `notice.copied_paste_manually`, the log says `result=copy_manual`. Premise checked:
    // A was captured (a missing start window would give copy-manual vacuously). Bite: a
    // paster whose front check accepts any window (the text lands in B), the wiring
    // using another paster or clipboard than the ports.
    let _serial = serial();
    let a = TestWindow::open(Shape::TopLevel);
    let b = TestWindow::open(Shape::TopLevel);
    a.front();
    let rig = AppRig::new(EngineKind::Api);
    let gate = Arc::new(Gate::default());
    let _open = OpenOnDrop(Arc::clone(&gate));
    let engine = TextEngine::gated(TEXT, Arc::clone(&gate));
    let paster = Watched::new(false);
    let indicator = Arc::new(FakeIndicator::new());
    let _dictation = start(
        &rig,
        speaking_mic(),
        paster.clone(),
        &engine,
        indicator.clone(),
    );
    let session = rig.session();

    hold(&session);
    assert!(
        gate.wait_entered(BUDGET),
        "premise: the engine was not called"
    );
    assert_eq!(
        paster.captured(),
        vec![Some(StartWindow {
            handle: WindowRef(window_ref(a.hwnd())),
            process_id: std::process::id(),
            elevated: false,
        })],
        "premise: A captured at the press"
    );
    b.front();
    gate.open();

    let line = one_dictation_line(&rig);
    assert!(has_pair(&line, "outcome", "delivered"), "{line}");
    assert!(has_pair(&line, "result", "copy_manual"), "{line}");
    assert!(
        holds_for(SETTLE, || a.text().is_empty() && b.text().is_empty()),
        "pasted: A {:?}, B {:?}",
        a.text(),
        b.text()
    );
    assert_eq!(paster.sends(), 0, "Ctrl+V sent");
    assert_eq!(clipboard_text().as_deref(), Some(TEXT), "clipboard");
    assert!(
        eventually(BUDGET, || shown(&indicator.overlays())
            .contains(&copied_paste_manually())),
        "overlays: {:?}",
        shown(&indicator.overlays())
    );
}

#[test]
fn an_elevated_start_window_gets_no_ctrl_v_and_the_text_stays_in_the_clipboard() {
    // T-006 row 20 (Acceptance line 3, "the target runs elevated"; R-12): the start window
    // is reported elevated (a CI runner cannot host a higher-integrity target, so the
    // wrapper marks it); `send_ctrl_v` is never called, the window stays empty, the
    // clipboard holds the text, the notice and `result=copy_manual` follow. Premise: A
    // was captured. Bite: the wiring ignoring the paster port's start window, a paste
    // attempted into an elevated window.
    let _serial = serial();
    let a = TestWindow::open(Shape::TopLevel);
    a.front();
    let rig = AppRig::new(EngineKind::Api);
    let engine = TextEngine::new(TEXT);
    let paster = Watched::new(true);
    let indicator = Arc::new(FakeIndicator::new());
    let _dictation = start(
        &rig,
        speaking_mic(),
        paster.clone(),
        &engine,
        indicator.clone(),
    );
    let session = rig.session();

    hold(&session);

    let line = one_dictation_line(&rig);
    assert!(has_pair(&line, "result", "copy_manual"), "{line}");
    let captured = paster.captured();
    assert!(
        matches!(captured.as_slice(), [Some(w)] if w.handle == WindowRef(window_ref(a.hwnd())) && w.elevated),
        "premise: A captured as elevated: {captured:?}"
    );
    assert_eq!(paster.sends(), 0, "Ctrl+V sent to an elevated window");
    assert!(
        holds_for(SETTLE, || a.text().is_empty()),
        "pasted: {:?}",
        a.text()
    );
    assert_eq!(clipboard_text().as_deref(), Some(TEXT), "clipboard");
    assert!(
        eventually(BUDGET, || shown(&indicator.overlays())
            .contains(&copied_paste_manually())),
        "overlays: {:?}",
        shown(&indicator.overlays())
    );
}

#[test]
fn microphone_access_denied_shows_no_recording_and_the_microphone_unavailable_notice() {
    // T-006 row 21 (Acceptance line 3, second branch; FR-009, NFR-02): the source refuses
    // with AccessDenied; there is no Recording on either indicator, the overlay shows
    // `failure.microphone_unavailable` with reason `mic_reason.access_denied`, the log has
    // `outcome=capture_failed mic=access_denied`, the clipboard keeps what it held and the
    // engine is never called. Bite: a refused capture shown as recording, the reason lost,
    // a job queued for a capture that never opened.
    let _serial = serial();
    put_text("held before the press");
    let rig = AppRig::new(EngineKind::Api);
    let engine = TextEngine::new(TEXT);
    let audio = Arc::new(FakeAudioSource::new());
    audio.set_start_error(Some(CaptureError::AccessDenied));
    let indicator = Arc::new(FakeIndicator::new());
    let _dictation = start(
        &rig,
        audio.clone(),
        Watched::new(false),
        &engine,
        indicator.clone(),
    );
    let session = rig.session();

    hold(&session);

    let line = one_dictation_line(&rig);
    assert!(has_pair(&line, "outcome", "capture_failed"), "{line}");
    assert!(has_pair(&line, "mic", "access_denied"), "{line}");
    let want = Shown::Message(
        "failure.microphone_unavailable",
        vec![("reason", "mic_reason.access_denied".to_string())],
    );
    assert!(
        eventually(BUDGET, || shown(&indicator.overlays()).contains(&want)),
        "overlays: {:?}",
        shown(&indicator.overlays())
    );
    assert!(
        holds_for(SETTLE, || engine.calls() == 0),
        "the engine was called for a refused capture"
    );
    assert!(
        !shown(&indicator.overlays()).contains(&Shown::Recording),
        "overlay Recording: {:?}",
        indicator.overlays()
    );
    assert!(
        !indicator
            .trays()
            .iter()
            .any(|(s, _)| *s == TrayState::Recording),
        "tray Recording: {:?}",
        indicator.trays()
    );
    assert_eq!(audio.start_calls(), 1, "start calls");
    assert_eq!(audio.open_handles(), 0, "open handles");
    assert_eq!(
        clipboard_text().as_deref(),
        Some("held before the press"),
        "clipboard"
    );
}

#[test]
fn engine_none_opens_one_settings_window_on_engine_and_never_opens_the_microphone() {
    // T-006 row 22 (spec 001 FR-001 / blocked_actions; T-052 invariant 2): with no engine
    // chosen, a press opens no capture, shows `notice.choose_engine`, and opens exactly
    // one settings window on the Engine tab through the opener (`settings_window::request`
    // with `OpenTarget::Tab(Engine, None)`). Bite: the open-settings request dropped or
    // sent to another tab, the capture opened, a window per press.
    let _serial = serial();
    let rig = AppRig::new(EngineKind::None);
    let engine = TextEngine::new(TEXT);
    let audio = Arc::new(FakeAudioSource::new());
    let indicator = Arc::new(FakeIndicator::new());
    let _dictation = start(
        &rig,
        audio.clone(),
        Watched::new(false),
        &engine,
        indicator.clone(),
    );
    let session = rig.session();

    session.hotkey_pressed(Instant::now());
    session.hotkey_released(Instant::now());

    assert!(
        eventually(BUDGET, || rig.app.get_webview_window(LABEL).is_some()),
        "no settings window"
    );
    let url = rig
        .app
        .get_webview_window(LABEL)
        .expect("the settings window")
        .url()
        .expect("url");
    assert_eq!(url.query(), Some("tab=engine"), "url {url}");
    assert_eq!(
        rig.app.webview_windows().len(),
        1,
        "windows: {:?}",
        rig.app.webview_windows().keys().collect::<Vec<_>>()
    );
    assert!(
        shown(&indicator.overlays()).contains(&Shown::Message("notice.choose_engine", vec![])),
        "overlays: {:?}",
        shown(&indicator.overlays())
    );
    assert_eq!(audio.start_calls(), 0, "capture opened with no engine");
}

#[test]
fn the_session_is_managed_and_the_shell_indicator_drives_the_tray() {
    // T-006 row 23 (invariant 5; T-052's menu-open handler looks the session up as
    // `Arc<DictationSession>`): `build_app` manages no session; after `start_dictation`
    // one is managed. `ShellIndicator` (run()'s Indicator) forwards the tray half to the
    // app's tray: a refused capture through the session turns the tray to Error, and a
    // direct `set_tray` reaches it too; the overlay half changes nothing on the tray
    // (a no-op until T-057). Bite: the session not managed (the tray menu cannot clear
    // Error), ShellIndicator dropping the tray half.
    let _serial = serial();
    // A taken hotkey would put the tray in HotkeyError, which outranks Error.
    assert_hotkey_free();
    let rig = AppRig::new(EngineKind::Api);
    assert!(
        rig.app.try_state::<Arc<DictationSession>>().is_none(),
        "premise: build_app manages no session"
    );
    let engine = TextEngine::new(TEXT);
    let audio = Arc::new(FakeAudioSource::new());
    audio.set_start_error(Some(CaptureError::AccessDenied));
    let indicator = Arc::new(ShellIndicator::new(rig.app.handle()));
    let _dictation = start(&rig, audio, Watched::new(false), &engine, indicator.clone());
    let session = rig.session();
    let tray_state = || tray::applied(rig.app.handle()).map(|a| (a.state, a.retry_available));

    session.hotkey_pressed(Instant::now());
    assert!(
        eventually(BUDGET, || tray_state() == Some((TrayState::Error, false))),
        "a refused capture through ShellIndicator: tray {:?}",
        tray_state()
    );

    indicator.set_tray(TrayState::Recording, true);
    assert!(
        eventually(BUDGET, || tray_state()
            == Some((TrayState::Recording, true))),
        "ShellIndicator::set_tray: tray {:?}",
        tray_state()
    );
    indicator.set_overlay(&OverlayState::Processing);
    assert!(
        holds_for(SETTLE, || tray_state()
            == Some((TrayState::Recording, true))),
        "set_overlay changed the tray: {:?}",
        tray_state()
    );
}
