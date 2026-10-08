//! T-006 Acceptance line 1: "Windows CI: integration test pastes a transcript (mock
//! engine, injected WAV source) into a test window and the clipboard holds it, excluded
//! from clipboard history". Through the real wiring: `build_app` (as `run()`), then
//! `voicen_lib::dictation::start_dictation` with the injected ports (the real-time WAV
//! source `RealtimeSource` over `fixtures::speech_3s`, a fixed-text test engine of kind
//! `api`, the real `WinClipboard` and `WinPaster`, `FakeIndicator`), the real hotkey
//! thread and a synthetic Ctrl+Alt+Space hold. Windows CI only (decision #5).
//!
//! Runner capabilities (docs/decisions/windows-ci-runner.md): `sendinput`, `hotkey`,
//! `async_keys`, `clipboard` (`ok` in runs A and B) and `foreground_again` (re-measured by
//! the probe in every job); asserted loudly by `win32_support`. The hold is timed from the observed press (the indicator's
//! Recording), and every wait is at least 3 s (run B: ~1 s to `WM_HOTKEY`).
//!
//! Red-test table row 18. The data dir and the log are a `TempDir`; the key is fake; the
//! transcript carries a canary that must never reach the log (FR-020, NFR-04).
#![cfg(windows)]

mod win32_support;

use std::sync::Arc;
use std::thread;

use voicen_core::platform::FakeIndicator;
use voicen_core::settings::EngineKind;
use voicen_core::test_support::fixtures;
use voicen_core::test_support::realtime::RealtimeSource;
use voicen_lib::dictation::{start_dictation, DictationPorts};
use voicen_lib::win::clipboard::WinClipboard;
use voicen_lib::win::paste::WinPaster;
use win32_support::{
    assert_hotkey_free, dictation_lines, engine_factory, eventually, has_pair, read_clipboard,
    serial, shown, AppRig, ClipboardState, Keys, Shape, Shown, TestWindow, TextEngine, BUDGET,
    HOLD, HOTKEY_KEYS, VK_CONTROL, VK_MENU, VK_SPACE, WAIT,
};

/// The test engine's transcript: non-ASCII, and a canary the log must never contain.
const TRANSCRIPT: &str = "Voicen e2e TRANSCRIPT-CANARY-5d1 проверка";
const CANARY: &str = "TRANSCRIPT-CANARY-5d1";

#[test]
fn a_hotkey_hold_through_the_real_wiring_pastes_the_transcript_into_the_window() {
    // T-006 row 18 (Acceptance line 1; invariant 5): a synthetic Ctrl+Alt+Space hold of
    // 1.5 s from the observed press, with engine `api` (a test engine) and the WAV source
    // injected through `start_dictation`'s ports, ends with
    // - the transcript in the foreground test window's text (Ctrl+V into the start window);
    // - the clipboard holding it with CanIncludeInClipboardHistory = 0,
    //   CanUploadToCloudClipboard = 0 and ExcludeClipboardContentFromMonitorProcessing;
    // - exactly one `dictation … engine=api outcome=delivered result=pasted` line, and no
    //   log line containing the transcript;
    // - the overlay going Recording, Processing, Hidden (nothing else).
    // Bite: no wiring or no hotkey thread (nothing happens), a second observer (two
    // dictation lines), the injected ports not used (no paste), a transcript logged.
    let _serial = serial();
    assert_hotkey_free();
    let window = TestWindow::open(Shape::TopLevel);
    window.front();
    let rig = AppRig::new(EngineKind::Api);
    let engine = TextEngine::new(TRANSCRIPT);
    let indicator = Arc::new(FakeIndicator::new());
    let _dictation = start_dictation(
        rig.app.handle(),
        DictationPorts {
            audio: Arc::new(RealtimeSource::from_buffer(&fixtures::speech_3s())),
            clipboard: Arc::new(WinClipboard::new()),
            paster: Arc::new(WinPaster::new()),
            engine_factory: Some(engine_factory(&engine)),
            indicator: indicator.clone(),
            credentials: Arc::clone(&rig.creds),
            hotkeys: Arc::clone(&rig.hotkeys),
        },
    )
    .expect("start_dictation");

    let mut keys = Keys::press(&HOTKEY_KEYS);
    assert!(
        eventually(WAIT, || shown(&indicator.overlays())
            .contains(&Shown::Recording)),
        "no Recording within {WAIT:?} of the injected Ctrl+Alt+Space: {:?}",
        indicator.overlays()
    );
    thread::sleep(HOLD);
    keys.release(&[VK_SPACE, VK_CONTROL, VK_MENU]);

    assert!(
        eventually(BUDGET, || window.text() == TRANSCRIPT),
        "the window text {BUDGET:?} after the release: {:?} (engine calls {})",
        window.text(),
        engine.calls()
    );
    assert_eq!(
        read_clipboard(),
        ClipboardState {
            text: Some(TRANSCRIPT.to_string()),
            exclude_from_monitor: true,
            can_include_in_history: Some(0),
            can_upload_to_cloud: Some(0),
        }
    );
    assert!(
        eventually(WAIT, || !dictation_lines(&rig.log_lines()).is_empty()),
        "no dictation line in the log"
    );
    let lines = rig.log_lines();
    let dictation = dictation_lines(&lines);
    assert_eq!(dictation.len(), 1, "dictation lines: {dictation:?}");
    for (key, value) in [
        ("engine", "api"),
        ("outcome", "delivered"),
        ("result", "pasted"),
    ] {
        assert!(
            has_pair(&dictation[0], key, value),
            "{key}={value}: {}",
            dictation[0]
        );
    }
    let leaked: Vec<&String> = lines.iter().filter(|l| l.contains(CANARY)).collect();
    assert!(
        leaked.is_empty(),
        "the transcript reached the log: {leaked:?}"
    );
    assert!(
        eventually(WAIT, || shown(&indicator.overlays())
            == vec![Shown::Recording, Shown::Processing, Shown::Hidden]),
        "overlays: {:?}",
        shown(&indicator.overlays())
    );
    assert_eq!(engine.calls(), 1, "engine calls");
}
