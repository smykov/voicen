//! T-006: the hotkey thread (`voicen_lib::win::hotkey::HotkeyThread`; design 1, research
//! R-1, R-2; invariant 1) over the real core `DictationSession` with core fakes
//! (`FakeAudioSource`, `FakeIndicator`, `FakeClipboard`, `FakePaster`) and a fixed-text
//! test engine. Windows CI only (decision #5).
//!
//! Runner capabilities (docs/decisions/windows-ci-runner.md, `ok` in runs A and B):
//! `hotkey` (registration and `WM_HOTKEY` from injected keys), `sendinput`, `async_keys`;
//! and `foreground_again` (test 5; re-measured by the probe in every job): the window is
//! brought to the front through an injected Alt, whose keys `bring_to_front` clears from
//! the windows' message record before the test reads it. Each is asserted as a loud
//! precondition by the helpers in `win32_support`. A hold is timed from the observed press (the fake's start count),
//! never from `SendInput` (run B: ~1 s to `WM_HOTKEY`); every wait is at least 3 s.
//!
//! Red-test table rows (T-006 Investigation): 2 (win32_data equals the windows crate),
//! 3 (one capture per hold, closed on release), 4 (release on any key), 5 (no WM_CHAR,
//! no menu, the mask key sent), 6 (a taken hotkey is reported and logged with 1409),
//! 7 (dropping the handle frees the hotkey).
#![cfg(windows)]

mod win32_support;

use std::sync::Arc;
use std::thread;
use std::time::Instant;

use voicen_core::diag::Log;
use voicen_core::dictation::{DictationSession, SessionDeps};
use voicen_core::events::RecordingObserver;
use voicen_core::pipeline::PipelineDeps;
use voicen_core::platform::{
    FakeAudioSource, FakeClipboard, FakeIndicator, FakePaster, FakeShellRequests,
    FakeTempAudioStore,
};
use voicen_core::post_process::PassThrough;
use voicen_core::recording::TrayState;
use voicen_core::settings::hotkey::{Hotkey, HotkeyKey};
use voicen_core::settings::EngineKind;
use voicen_core::test_support::TempDir;
use voicen_core::vad::{EnergyDetector, SpeechDetector, SpeechGate};
use voicen_core::win32_data::{hotkey_codes, modifier_wait_keys};
use voicen_lib::win::hotkey::HotkeyThread;
use win32_support::{
    assert_hotkey_free, engine_factory, eventually, has_pair, headed, holds_for, is_down,
    keyed_store, log_lines, precondition, serial, settings, Keys, Shape, TakenHotkey, TestWindow,
    TextEngine, HOLD, HOTKEY_KEYS, SETTLE, VK_CONTROL, VK_MASK, VK_MENU, VK_SPACE, WAIT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, VIRTUAL_KEY, VK_0, VK_1, VK_2, VK_3,
    VK_4, VK_5, VK_6, VK_7, VK_8, VK_9, VK_A, VK_B, VK_C, VK_CONTROL as WIN_VK_CONTROL, VK_D,
    VK_DELETE, VK_DOWN, VK_E, VK_END, VK_F, VK_F1, VK_F10, VK_F11, VK_F12, VK_F13, VK_F14, VK_F15,
    VK_F16, VK_F17, VK_F18, VK_F19, VK_F2, VK_F20, VK_F21, VK_F22, VK_F23, VK_F24, VK_F3, VK_F4,
    VK_F5, VK_F6, VK_F7, VK_F8, VK_F9, VK_G, VK_H, VK_HOME, VK_I, VK_INSERT, VK_J, VK_K, VK_L,
    VK_LEFT, VK_LWIN as WIN_VK_LWIN, VK_M, VK_MENU as WIN_VK_MENU, VK_N, VK_NEXT, VK_NUMPAD0,
    VK_NUMPAD1, VK_NUMPAD2, VK_NUMPAD3, VK_NUMPAD4, VK_NUMPAD5, VK_NUMPAD6, VK_NUMPAD7, VK_NUMPAD8,
    VK_NUMPAD9, VK_O, VK_P, VK_PAUSE, VK_PRIOR, VK_Q, VK_R, VK_RIGHT, VK_RWIN as WIN_VK_RWIN, VK_S,
    VK_SHIFT as WIN_VK_SHIFT, VK_SPACE as WIN_VK_SPACE, VK_T, VK_U, VK_UP, VK_V, VK_W, VK_X, VK_Y,
    VK_Z,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SC_KEYMENU, WM_CHAR, WM_ENTERMENULOOP, WM_INITMENU, WM_KEYDOWN, WM_KEYUP, WM_SYSCHAR,
    WM_SYSCOMMAND, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

/// The core session over fakes, its settings in `dir` (engine `api`, the default
/// hotkey Ctrl+Alt+Space), logging to `<dir>/logs`.
struct Rig {
    _dir: TempDir,
    logs: std::path::PathBuf,
    log: Arc<Log>,
    session: Arc<DictationSession>,
    audio: Arc<FakeAudioSource>,
    indicator: Arc<FakeIndicator>,
}

impl Rig {
    fn new() -> Rig {
        let dir = TempDir::new();
        let logs = dir.path().join("logs");
        let log = voicen_lib::diag::start(logs.clone(), Box::new(|_| {}));
        let creds = keyed_store();
        let service = settings(dir.path(), &creds, &log, EngineKind::Api);
        assert_eq!(
            service.snapshot().hotkey,
            "Ctrl+Alt+Space",
            "premise: the default hotkey"
        );
        let audio = Arc::new(FakeAudioSource::new());
        let indicator = Arc::new(FakeIndicator::new());
        let engine = TextEngine::new("hotkey test transcript");
        let session = DictationSession::start(SessionDeps {
            pipeline: PipelineDeps {
                gate: SpeechGate::new(
                    Ok(Box::new(EnergyDetector::new()) as Box<dyn SpeechDetector>),
                    EnergyDetector::new(),
                ),
                credentials: creds,
                clipboard: Arc::new(FakeClipboard::new()),
                paster: Arc::new(FakePaster::new()),
                temp_audio: Arc::new(FakeTempAudioStore::new()),
                observer: Arc::new(RecordingObserver::new()),
                post_processor: Arc::new(PassThrough),
            },
            engine_factory: Some(engine_factory(&engine)),
            audio: audio.clone(),
            indicator: indicator.clone(),
            requests: Arc::new(FakeShellRequests::new()),
            settings: service,
        })
        .expect("the session starts");
        Rig {
            _dir: dir,
            logs,
            log,
            session: Arc::new(session),
            audio,
            indicator,
        }
    }

    fn start_hotkey(&self) -> HotkeyThread {
        HotkeyThread::start(
            Arc::clone(&self.session),
            "Ctrl+Alt+Space",
            Arc::clone(&self.log),
        )
        .expect("the hotkey thread starts")
    }

    /// Presses Ctrl+Alt+Space and waits for the session's press (the capture start):
    /// the observed press, from which a hold is timed.
    #[track_caller]
    fn press_hotkey(&self) -> (Keys, Instant) {
        let keys = Keys::press(&HOTKEY_KEYS);
        // The keys read down (preconditions `sendinput`, `async_keys` in Keys::press) and
        // the combination was free (`assert_hotkey_free`): a missing press is the code's.
        assert!(
            eventually(WAIT, || self.audio.start_calls() >= 1),
            "no press reached the session within {WAIT:?} of the injected Ctrl+Alt+Space"
        );
        (keys, Instant::now())
    }
}

fn wait_out(since: Instant) {
    let left = HOLD.saturating_sub(since.elapsed());
    thread::sleep(left);
}

// ---- row 2 ----------------------------------------------------------------------------

/// Every key of the closed set with its windows-crate constant.
fn windows_vk(key: HotkeyKey) -> VIRTUAL_KEY {
    use HotkeyKey::*;
    match key {
        A => VK_A,
        B => VK_B,
        C => VK_C,
        D => VK_D,
        E => VK_E,
        F => VK_F,
        G => VK_G,
        H => VK_H,
        I => VK_I,
        J => VK_J,
        K => VK_K,
        L => VK_L,
        M => VK_M,
        N => VK_N,
        O => VK_O,
        P => VK_P,
        Q => VK_Q,
        R => VK_R,
        S => VK_S,
        T => VK_T,
        U => VK_U,
        V => VK_V,
        W => VK_W,
        X => VK_X,
        Y => VK_Y,
        Z => VK_Z,
        Digit0 => VK_0,
        Digit1 => VK_1,
        Digit2 => VK_2,
        Digit3 => VK_3,
        Digit4 => VK_4,
        Digit5 => VK_5,
        Digit6 => VK_6,
        Digit7 => VK_7,
        Digit8 => VK_8,
        Digit9 => VK_9,
        F1 => VK_F1,
        F2 => VK_F2,
        F3 => VK_F3,
        F4 => VK_F4,
        F5 => VK_F5,
        F6 => VK_F6,
        F7 => VK_F7,
        F8 => VK_F8,
        F9 => VK_F9,
        F10 => VK_F10,
        F11 => VK_F11,
        F12 => VK_F12,
        F13 => VK_F13,
        F14 => VK_F14,
        F15 => VK_F15,
        F16 => VK_F16,
        F17 => VK_F17,
        F18 => VK_F18,
        F19 => VK_F19,
        F20 => VK_F20,
        F21 => VK_F21,
        F22 => VK_F22,
        F23 => VK_F23,
        F24 => VK_F24,
        Space => WIN_VK_SPACE,
        PageUp => VK_PRIOR,
        PageDown => VK_NEXT,
        End => VK_END,
        Home => VK_HOME,
        ArrowLeft => VK_LEFT,
        ArrowUp => VK_UP,
        ArrowRight => VK_RIGHT,
        ArrowDown => VK_DOWN,
        Insert => VK_INSERT,
        Delete => VK_DELETE,
        Pause => VK_PAUSE,
        Numpad0 => VK_NUMPAD0,
        Numpad1 => VK_NUMPAD1,
        Numpad2 => VK_NUMPAD2,
        Numpad3 => VK_NUMPAD3,
        Numpad4 => VK_NUMPAD4,
        Numpad5 => VK_NUMPAD5,
        Numpad6 => VK_NUMPAD6,
        Numpad7 => VK_NUMPAD7,
        Numpad8 => VK_NUMPAD8,
        Numpad9 => VK_NUMPAD9,
    }
}

#[test]
fn win32_data_equals_the_windows_crate_constants() {
    // T-006 row 2 (dictation-session.md: Win32 facts come from win32_data only, and
    // T-006 cross-checks them on Windows): every key's VK, every MOD_* bit with
    // MOD_NOREPEAT, and the modifier-wait set equal the `windows` 0.62.2 constants the
    // adapters would otherwise use. Needs no runner capability. Bite: a table value that
    // differs from Windows (a swapped arrow, VK_PRIOR/VK_NEXT, MOD_ALT/MOD_CONTROL), the
    // wait set missing VK_RWIN, or the wait set missing (red until T-006 adds it).
    let mut wrong = Vec::new();
    for &key in HotkeyKey::all() {
        let h = Hotkey {
            ctrl: true,
            alt: false,
            shift: false,
            win: false,
            key,
        };
        let got = hotkey_codes(&h).vk;
        if got != windows_vk(key).0 {
            wrong.push(format!(
                "{key:?}: {got:#04x}, windows {:#04x}",
                windows_vk(key).0
            ));
        }
    }
    for (ctrl, alt, shift, win) in [
        (true, false, false, false),
        (false, true, false, false),
        (false, false, true, false),
        (false, false, false, true),
        (true, true, true, true),
    ] {
        let h = Hotkey {
            ctrl,
            alt,
            shift,
            win,
            key: HotkeyKey::Space,
        };
        let mut want = MOD_NOREPEAT.0;
        if ctrl {
            want |= MOD_CONTROL.0;
        }
        if alt {
            want |= MOD_ALT.0;
        }
        if shift {
            want |= MOD_SHIFT.0;
        }
        if win {
            want |= MOD_WIN.0;
        }
        let got = hotkey_codes(&h).modifiers;
        if got != want {
            wrong.push(format!("{h}: modifiers {got:#06x}, windows {want:#06x}"));
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));

    let mut set = modifier_wait_keys().to_vec();
    set.sort_unstable();
    let mut want = vec![
        WIN_VK_SHIFT.0,
        WIN_VK_CONTROL.0,
        WIN_VK_MENU.0,
        WIN_VK_LWIN.0,
        WIN_VK_RWIN.0,
    ];
    want.sort_unstable();
    assert_eq!(
        set, want,
        "the modifier-wait set against windows VK_SHIFT/CONTROL/MENU/LWIN/RWIN"
    );
}

// ---- rows 3-7 ---------------------------------------------------------------------------

#[test]
fn a_synthetic_hold_opens_one_capture_and_closes_it_on_release() {
    // T-006 row 3 (invariant 1, FR-003/FR-007, NFR-02): a Ctrl+Alt+Space hold injected
    // with SendInput reaches the session as one press: one `AudioSource::start`, the
    // device open while the keys are held (the release poll does not end the hold
    // early), closed after the key-ups. Bite: no hotkey thread (no press), no release
    // poll (the capture stays open), a poll that reads the wrong keys (closes at once),
    // MOD_NOREPEAT dropped and a press per repeat (more than one start).
    let _serial = serial();
    assert_hotkey_free();
    let window = TestWindow::open(Shape::TopLevel);
    window.front();
    let rig = Rig::new();
    let _hotkey = rig.start_hotkey();

    let (mut keys, pressed_at) = rig.press_hotkey();
    assert!(
        holds_for(HOLD.saturating_sub(pressed_at.elapsed()), || rig
            .audio
            .open_handles()
            == 1),
        "the capture closed while Ctrl+Alt+Space was still held (open handles {})",
        rig.audio.open_handles()
    );
    wait_out(pressed_at);
    keys.release(&[VK_SPACE, VK_CONTROL, VK_MENU]);

    assert!(
        eventually(WAIT, || rig.audio.open_handles() == 0),
        "the capture is still open {WAIT:?} after the key-ups"
    );
    assert_eq!(rig.audio.start_calls(), 1, "captures opened by one hold");
}

#[test]
fn releasing_any_key_of_the_hotkey_ends_the_hold() {
    // T-006 row 4 (R-1: the first key found up counts as the release): with Ctrl and
    // Space still held, releasing only Alt closes the capture. Bite: only the hotkey's
    // main key polled (Space still down: the capture stays open), all keys required up.
    let _serial = serial();
    assert_hotkey_free();
    let window = TestWindow::open(Shape::TopLevel);
    window.front();
    let rig = Rig::new();
    let _hotkey = rig.start_hotkey();

    let (mut keys, pressed_at) = rig.press_hotkey();
    wait_out(pressed_at);
    keys.release(&[VK_MENU]);

    assert!(
        eventually(WAIT, || rig.audio.open_handles() == 0),
        "the capture is still open {WAIT:?} after Alt went up"
    );
    precondition(
        "async_keys",
        is_down(VK_CONTROL) && is_down(VK_SPACE),
        "Ctrl and Space no longer read down after the Alt key-up",
    );
    keys.release_all();
    assert_eq!(rig.audio.start_calls(), 1, "captures opened by one hold");
}

#[test]
fn the_hotkey_reaches_no_app_and_alt_opens_no_menu() {
    // T-006 row 5 (FR-010 "hotkey not passed on"; R-2): during a whole hold the
    // foreground test window gets no character (WM_CHAR / WM_SYSCHAR) and no menu
    // activation (WM_SYSCOMMAND SC_KEYMENU, WM_INITMENU, WM_ENTERMENULOOP), and it sees
    // the mask key 0xE8 go down and up (sent by the hotkey thread; the test never sends
    // it during the hold). Alt is released last, alone: the case that opens the menu
    // without the mask. Bite: no mask key (SC_KEYMENU on Alt's release), the hotkey not
    // registered (Space typed into the window as a character).
    let _serial = serial();
    assert_hotkey_free();
    let window = TestWindow::open(Shape::TopLevel);
    window.front();
    let rig = Rig::new();
    let _hotkey = rig.start_hotkey();

    let (mut keys, pressed_at) = rig.press_hotkey();
    wait_out(pressed_at);
    keys.release(&[VK_SPACE]);
    keys.release(&[VK_CONTROL]);
    keys.release(&[VK_MENU]);
    assert!(
        eventually(WAIT, || rig.audio.open_handles() == 0),
        "premise: the hold did not end"
    );
    thread::sleep(SETTLE);

    let messages = window.messages();
    let chars: Vec<_> = messages
        .iter()
        .filter(|(m, _)| *m == WM_CHAR || *m == WM_SYSCHAR)
        .collect();
    assert!(
        chars.is_empty(),
        "characters reached the window: {chars:x?}"
    );
    let menu: Vec<_> = messages
        .iter()
        .filter(|(m, w)| {
            (*m == WM_SYSCOMMAND && (w & 0xFFF0) == SC_KEYMENU as usize)
                || *m == WM_INITMENU
                || *m == WM_ENTERMENULOOP
        })
        .collect();
    assert!(
        menu.is_empty(),
        "the Alt release activated the menu: {menu:x?}"
    );
    let mask = usize::from(VK_MASK);
    let mask_down = messages
        .iter()
        .any(|(m, w)| (*m == WM_KEYDOWN || *m == WM_SYSKEYDOWN) && *w == mask);
    let mask_up = messages
        .iter()
        .any(|(m, w)| (*m == WM_KEYUP || *m == WM_SYSKEYUP) && *w == mask);
    assert!(
        mask_down && mask_up,
        "the window did not see VK 0xE8 down ({mask_down}) and up ({mask_up}): {messages:x?}"
    );
}

#[test]
fn a_taken_hotkey_is_reported_to_the_session_and_logged_with_1409() {
    // T-006 row 6 (FR-005 / FR-028 tray HotkeyError; #45 typed warnings): when another
    // owner holds Ctrl+Alt+Space, the hotkey thread reports `hotkey_registration(false)`
    // (the tray shows HotkeyError) and writes one `warning kind=hotkey_register_failed
    // os_code=1409` line (ERROR_HOTKEY_ALREADY_REGISTERED; no OS text). Bite: the
    // registration result ignored (no HotkeyError), a failure that is not logged, the
    // HRESULT instead of the Win32 code, the thread failing to start (Err) instead of
    // running on.
    let _serial = serial();
    assert_hotkey_free();
    let _taken = TakenHotkey::take().expect("premise: the test takes Ctrl+Alt+Space");
    let rig = Rig::new();
    let _hotkey = rig.start_hotkey();

    assert!(
        eventually(WAIT, || rig
            .indicator
            .trays()
            .iter()
            .any(|(s, _)| *s == TrayState::HotkeyError)),
        "no HotkeyError on the indicator: {:?}",
        rig.indicator.trays()
    );
    let failed = || {
        headed(&log_lines(&rig.logs), "warning")
            .into_iter()
            .filter(|l| has_pair(l, "kind", "hotkey_register_failed"))
            .collect::<Vec<_>>()
    };
    assert!(
        eventually(WAIT, || !failed().is_empty()),
        "no hotkey_register_failed line"
    );
    let lines = failed();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(has_pair(&lines[0], "os_code", "1409"), "{}", lines[0]);
}

#[test]
fn dropping_the_hotkey_thread_frees_the_hotkey() {
    // T-006 row 7: the handle owns the registration; once `drop` returns, the
    // combination is free (the next test, a restart of the thread or T-055's
    // re-registration can take it). Also pins that a running thread holds it. Bite: no
    // UnregisterHotKey, a thread left running, a drop that returns before the
    // unregistration.
    let _serial = serial();
    assert_hotkey_free();
    let rig = Rig::new();
    let hotkey = rig.start_hotkey();
    assert!(
        eventually(WAIT, || TakenHotkey::take().is_err()),
        "the running hotkey thread does not hold Ctrl+Alt+Space"
    );

    drop(hotkey);

    let taken = TakenHotkey::take();
    assert!(
        taken.is_ok(),
        "Ctrl+Alt+Space is still registered after the hotkey thread was dropped: {:?}",
        taken.err()
    );
}
