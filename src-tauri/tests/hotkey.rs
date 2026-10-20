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
//!
//! T-009 (FR-22, OQ-21 (1)): the hotkey thread claims Esc while a recording is on, bare
//! and under the hotkey's modifiers, through `CancelKeyHandle` (the session's
//! `CancelKey` port, attached to the thread like T-055's registrar handle); Esc
//! discards the recording; idle, Esc reaches the focused window; a refused claim keeps
//! the recording and warns `esc_unavailable`.
//!
//! T-108: a check that Esc is claimed never probes while the claim is still pending
//! (an Esc probe registers Esc itself and can refuse the thread's one-shot claim):
//! [`esc_claim`] waits for the Recording indicator, makes one round trip on the hotkey
//! thread's request channel ([`hotkey_round_trip`]), then probes once. The release side
//! polls (`esc_released`): a probe cannot block an unregistration.
#![cfg(windows)]

mod win32_support;

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::Instant;

use voicen_core::diag::Log;
use voicen_core::dictation::{DictationSession, SessionDeps};
use voicen_core::events::{DictationEvent, RecordingObserver, WarningCode};
use voicen_core::hotkey_registrar::HotkeyRegistrar;
use voicen_core::pipeline::PipelineDeps;
use voicen_core::platform::{
    FakeAudioSource, FakeClipboard, FakeIndicator, FakePaster, FakeShellRequests,
    FakeTempAudioStore, Indicator,
};
use voicen_core::post_process::PassThrough;
use voicen_core::recording::{RecordingEnd, TrayState, MAX_LENGTH};
use voicen_core::secrets::KeyEdits;
use voicen_core::settings::hotkey::{parse_hotkey, Hotkey, HotkeyKey};
use voicen_core::settings::service::{SaveOutcome, SaveRequest};
use voicen_core::settings::{EngineKind, Mode};
use voicen_core::test_support::TempDir;
use voicen_core::vad::{EnergyDetector, SpeechDetector, SpeechGate};
use voicen_core::win32_data::{hotkey_codes, modifier_wait_keys};
use voicen_lib::win::hotkey::{CancelKeyHandle, HotkeyRegistrarHandle, HotkeyThread};
use win32_support::{
    assert_hotkey_free, combo_is_free, engine_factory, esc_is_free, eventually, has_pair, headed,
    holds_for, is_down, keyed_store, log_lines, precondition, serial, settings, Keys, Shape,
    TakenHotkey, TestWindow, TextEngine, HOLD, HOLD_LIMIT, HOTKEY_KEYS, SETTLE, VK_CONTROL,
    VK_ESCAPE, VK_MASK, VK_MENU, VK_SPACE, WAIT,
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
    /// T-009: the session's events (RecordingEnded, Warning, JobFinished).
    observer: Arc<RecordingObserver>,
    /// T-009: the Esc claim port, attached to the hotkey thread by `start_hotkey`.
    cancel: Arc<CancelKeyHandle>,
    /// T-108: a registrar on the hotkey thread's request channel, attached by
    /// `start_hotkey`; only [`hotkey_round_trip`] uses it (never a save).
    registrar: Arc<HotkeyRegistrarHandle>,
    engine: Arc<TextEngine>,
    /// T-010: the session's settings requests (one OpenSettings per transition to failed).
    requests: Arc<FakeShellRequests>,
}

impl Rig {
    fn new() -> Rig {
        Rig::with_mode(Mode::Hold)
    }

    /// T-009: the rig with `mode` saved before the session starts.
    fn with_mode(mode: Mode) -> Rig {
        Rig::build(mode, None)
    }

    /// T-108: the rig whose indicator holds the hotkey thread inside the session's
    /// first `set_tray(Recording)` until `gate` opens (see [`TrayGate`]).
    fn gated(gate: &Arc<TrayGate>) -> Rig {
        Rig::build(Mode::Hold, Some(Arc::clone(gate)))
    }

    fn build(mode: Mode, gate: Option<Arc<TrayGate>>) -> Rig {
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
        if mode != service.snapshot().mode {
            let mut s = (*service.snapshot()).clone();
            s.mode = mode;
            let outcome = service.save(SaveRequest {
                settings: s,
                keys: KeyEdits::default(),
            });
            assert!(
                matches!(outcome, SaveOutcome::Saved { .. }),
                "premise: saving mode {mode:?} is refused: {outcome:?}"
            );
        }
        let observer = Arc::new(RecordingObserver::new());
        let cancel = CancelKeyHandle::new();
        let audio = Arc::new(FakeAudioSource::new());
        let indicator = Arc::new(FakeIndicator::new());
        let engine = TextEngine::new("hotkey test transcript");
        let requests = Arc::new(FakeShellRequests::new());
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
                observer: observer.clone(),
                post_processor: Arc::new(PassThrough),
            },
            engine_factory: Some(engine_factory(&engine)),
            audio: audio.clone(),
            indicator: match gate {
                None => indicator.clone() as Arc<dyn Indicator>,
                Some(gate) => Arc::new(GatedIndicator {
                    inner: indicator.clone(),
                    gate,
                }),
            },
            requests: requests.clone(),
            settings: service,
            cancel_key: cancel.clone(),
        })
        .expect("the session starts");
        Rig {
            _dir: dir,
            logs,
            log,
            session: Arc::new(session),
            audio,
            indicator,
            observer,
            cancel,
            registrar: HotkeyRegistrarHandle::new(),
            engine,
            requests,
        }
    }

    fn start_hotkey(&self) -> HotkeyThread {
        let thread = HotkeyThread::start(
            Arc::clone(&self.session),
            "Ctrl+Alt+Space",
            Arc::clone(&self.log),
        )
        .expect("the hotkey thread starts");
        self.cancel.attach(&thread);
        self.registrar.attach(&thread);
        thread
    }

    /// T-009: every `RecordingEnded`'s end, in order.
    fn ends(&self) -> Vec<RecordingEnd> {
        self.observer
            .events()
            .iter()
            .filter_map(|e| match e {
                DictationEvent::RecordingEnded { end, .. } => Some(*end),
                _ => None,
            })
            .collect()
    }

    /// T-009: jobs that ended (a cancelled recording must have none).
    fn jobs(&self) -> usize {
        self.observer
            .events()
            .iter()
            .filter(|e| matches!(e, DictationEvent::JobFinished { .. }))
            .count()
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

// ---- T-009: Esc cancel (FR-22), claimed only while recording -------------------------

/// The hotkey's modifiers as `MOD_*` bits (Ctrl+Alt for the default Ctrl+Alt+Space).
const HOTKEY_MODS: u32 = MOD_CONTROL.0 | MOD_ALT.0;

/// The thread holds Esc both bare and under the hotkey's modifiers (OQ-21 (1)).
///
/// A registration probe: it takes and frees each combination, so it must never overlap
/// a claim still pending on the hotkey thread (T-108). Call it only through
/// [`esc_claim`] (which orders it after the claim), or after `esc_claim` returned `Ok`.
fn esc_claimed() -> bool {
    !esc_is_free(0) && !esc_is_free(HOTKEY_MODS)
}

/// Esc is free both bare and under the hotkey's modifiers. Polling it is safe on the
/// release side: a probe cannot block an unregistration, only a registration (T-108).
fn esc_released() -> bool {
    esc_is_free(0) && esc_is_free(HOTKEY_MODS)
}

/// The window saw an Esc key-down (WM_KEYDOWN / WM_SYSKEYDOWN with VK_ESCAPE).
fn esc_reached(window: &TestWindow) -> bool {
    window
        .messages()
        .iter()
        .any(|(m, w)| (*m == WM_KEYDOWN || *m == WM_SYSKEYDOWN) && *w == usize::from(VK_ESCAPE))
}

#[track_caller]
fn assert_esc_free() {
    precondition(
        "hotkey (registration)",
        esc_released(),
        "Esc or Ctrl+Alt+Esc is already registered by someone",
    );
}

#[test]
fn esc_while_recording_in_hold_mode_discards_with_the_hotkey_held() {
    // T-009 Acceptance "Esc cancel" (FR-22, OQ-21 (1)): while a hold is on the thread
    // claims Esc bare and under Ctrl+Alt; Esc pressed with Ctrl+Alt+Space still held
    // closes the capture, ends the recording Cancelled, runs no job, does not reach the
    // focused window, and the claim is given back. Bite: only bare Esc claimed (Ctrl+
    // Alt+Esc is another combination: nothing happens), the claim not released after
    // the cancel, WM_HOTKEY for the Esc id routed to hotkey_pressed or ignored, the
    // recording delivered at the hotkey's release.
    let _serial = serial();
    assert_hotkey_free();
    assert_esc_free();
    let window = TestWindow::open(Shape::TopLevel);
    window.front();
    let rig = Rig::new();
    let _hotkey = rig.start_hotkey();

    let (mut keys, _pressed_at) = rig.press_hotkey();
    if let Err(why) = esc_claim(&rig) {
        panic!("Esc not claimed bare and under Ctrl+Alt while recording: {why}");
    }
    let mut esc = Keys::press(&[VK_ESCAPE]);
    assert!(
        eventually(WAIT, || rig.audio.open_handles() == 0),
        "the capture is still open {WAIT:?} after Esc"
    );
    esc.release_all();
    keys.release_all();
    thread::sleep(SETTLE);

    assert_eq!(rig.ends(), vec![RecordingEnd::Cancelled]);
    assert_eq!(rig.jobs(), 0, "a job ran for the cancelled recording");
    assert_eq!(rig.engine.calls(), 0, "the cancelled audio was sent");
    assert_eq!(rig.audio.start_calls(), 1);
    assert!(!esc_reached(&window), "the claimed Esc reached the window");
    assert!(
        eventually(WAIT, esc_released),
        "Esc still claimed after the cancel; observer events: {:?}",
        rig.observer.events()
    );
}

#[test]
fn esc_in_toggle_mode_discards_after_the_hotkey_is_released() {
    // T-009 (FR-03 toggle, FR-22): in toggle mode the hotkey's release keeps the
    // recording on, Esc stays claimed with no key held, and bare Esc discards it.
    // Bite: the claim tied to the hotkey being down (dropped at its release), the
    // toggle release ending the recording.
    let _serial = serial();
    assert_hotkey_free();
    assert_esc_free();
    let window = TestWindow::open(Shape::TopLevel);
    window.front();
    let rig = Rig::with_mode(Mode::Toggle);
    let _hotkey = rig.start_hotkey();

    let (mut keys, _pressed_at) = rig.press_hotkey();
    keys.release_all();
    assert!(
        holds_for(SETTLE, || rig.audio.open_handles() == 1),
        "the toggle recording ended at the hotkey's release"
    );
    if let Err(why) = esc_claim(&rig) {
        panic!("Esc not claimed in toggle mode after the hotkey's release: {why}");
    }
    let mut esc = Keys::press(&[VK_ESCAPE]);
    esc.release_all();
    assert!(
        eventually(WAIT, || rig.audio.open_handles() == 0),
        "the capture is still open {WAIT:?} after Esc"
    );
    thread::sleep(SETTLE);
    assert_eq!(rig.ends(), vec![RecordingEnd::Cancelled]);
    assert_eq!(rig.jobs(), 0);
    assert!(!esc_reached(&window), "the claimed Esc reached the window");
    assert!(
        eventually(WAIT, esc_released),
        "Esc still claimed after the cancel; observer events: {:?}",
        rig.observer.events()
    );
}

#[test]
fn esc_while_idle_reaches_the_focused_window() {
    // FR-22 "elsewhere Esc reaches the focused app": with the hotkey thread running and
    // no recording, Esc is not registered and the foreground window gets its key-down;
    // nothing is recorded. Bite: Esc claimed at thread start (or kept after a
    // recording), the window starved of Esc.
    let _serial = serial();
    assert_hotkey_free();
    assert_esc_free();
    let window = TestWindow::open(Shape::TopLevel);
    window.front();
    let rig = Rig::new();
    let _hotkey = rig.start_hotkey();
    // T-108: whatever the thread was asked at its start has run before the first probe.
    if let Err(why) = hotkey_round_trip(&rig) {
        panic!("{why}");
    }
    assert!(
        holds_for(SETTLE, esc_released),
        "Esc claimed with no recording on; observer events: {:?}",
        rig.observer.events()
    );
    let mut esc = Keys::press(&[VK_ESCAPE]);
    esc.release_all();
    assert!(
        eventually(WAIT, || esc_reached(&window)),
        "the window never saw Esc: {:x?}",
        window.messages()
    );
    assert_eq!(rig.audio.start_calls(), 0);
    assert!(
        rig.observer.events().is_empty(),
        "{:?}",
        rig.observer.events()
    );
}

#[test]
fn esc_is_released_after_a_max_length_stop() {
    // T-009 Acceptance failure branch through the shell: a hold reaches its 10 min (the
    // session's timer input driven by the test at the observed press + MAX_LENGTH); the
    // capture closes, the recording ends MaxLength and is processed, and Esc is given
    // back to the apps while the hotkey is still held. Bite: the claim released only on
    // the hotkey's release path (kept until the keys go up), no 10-min stop.
    let _serial = serial();
    assert_hotkey_free();
    assert_esc_free();
    let window = TestWindow::open(Shape::TopLevel);
    window.front();
    let rig = Rig::new();
    let _hotkey = rig.start_hotkey();

    let (mut keys, pressed_at) = rig.press_hotkey();
    if let Err(why) = esc_claim(&rig) {
        panic!("Esc not claimed while recording: {why}");
    }
    rig.session.tick(pressed_at + MAX_LENGTH);
    assert!(
        eventually(WAIT, || rig.audio.open_handles() == 0),
        "the capture is still open after the max-length stop"
    );
    assert!(
        eventually(WAIT, esc_released),
        "Esc still claimed after the max-length stop (hotkey still held); observer events: {:?}",
        rig.observer.events()
    );
    keys.release_all();
    assert!(
        eventually(WAIT, || rig.jobs() == 1),
        "the max-length recording was not processed"
    );
    assert_eq!(rig.ends(), vec![RecordingEnd::MaxLength]);
}

#[test]
fn a_refused_esc_claim_keeps_recording_and_warns_esc_unavailable() {
    // T-009 Acceptance "Esc unavailable" (FR-22): with bare Esc already registered by
    // another owner, the thread's claim fails; the session gets the result, emits one
    // Warning{EscUnavailable}, and the recording goes on to its normal end at the
    // release. Bite: the claim result never reported (no warning), the recording
    // stopped or discarded on the refusal, the thread failing on the refused claim (no
    // release handling afterwards).
    let _serial = serial();
    assert_hotkey_free();
    assert_esc_free();
    let _taken = TakenHotkey::take_esc(0).expect("premise: the test takes bare Esc");
    let window = TestWindow::open(Shape::TopLevel);
    window.front();
    let rig = Rig::new();
    let _hotkey = rig.start_hotkey();

    let (mut keys, pressed_at) = rig.press_hotkey();
    let warned = || {
        rig.observer
            .events()
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    DictationEvent::Warning {
                        code: WarningCode::EscUnavailable
                    }
                )
            })
            .count()
    };
    assert!(
        eventually(WAIT, || warned() >= 1),
        "no Warning{{EscUnavailable}}: {:?}",
        rig.observer.events()
    );
    assert!(
        holds_for(SETTLE, || rig.audio.open_handles() == 1),
        "the refused claim ended the recording"
    );
    wait_out(pressed_at);
    keys.release_all();
    assert!(
        eventually(WAIT, || rig.jobs() == 1),
        "the recording was not processed after the refused claim"
    );
    assert_eq!(rig.ends(), vec![RecordingEnd::Released]);
    assert_eq!(warned(), 1, "{:?}", rig.observer.events());
}

// ---- T-108: the Esc claim check never races the claim it checks ----------------------
//
// `esc_claimed` probes by registering Esc itself. The hotkey thread claims each Esc
// combination once (`Registrations::cancel_key`, no retry), so a probe that overlaps a
// claim still pending makes the thread's claim fail (1409) and then reads "not claimed"
// for the whole wait (CI 39299815752, T-108 Investigation). The tests below hold the
// hotkey thread inside the session's first `set_tray(Recording)` (`TrayGate`): the
// session has queued the CancelKey request (`Shared::publish` syncs the claim before
// the indicator) but the thread has not run it, so a collision is planted, not hoped for.

/// Holds the thread that calls the first `set_tray(Recording)` until [`open`](Self::open)
/// (or `HOLD_LIMIT`); `entered` once that call arrived.
#[derive(Default)]
struct TrayGate {
    /// (entered, open)
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}

impl TrayGate {
    fn enter_and_wait(&self) {
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        st.0 = true;
        self.changed.notify_all();
        let _ = self
            .changed
            .wait_timeout_while(st, HOLD_LIMIT, |s| !s.1)
            .unwrap_or_else(PoisonError::into_inner);
    }

    fn wait_entered(&self) -> bool {
        let st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let (st, _) = self
            .changed
            .wait_timeout_while(st, WAIT, |s| !s.0)
            .unwrap_or_else(PoisonError::into_inner);
        st.0
    }

    fn open(&self) {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).1 = true;
        self.changed.notify_all();
    }
}

/// Opens the tray gate when dropped, also by a failed assertion (the hotkey thread is
/// never left blocked).
struct OpenTrayOnDrop(Arc<TrayGate>);

impl Drop for OpenTrayOnDrop {
    fn drop(&mut self) {
        self.0.open();
    }
}

/// The rig's `FakeIndicator` (every call recorded first, so the Recording indicator is
/// visible to the test) that then blocks the caller on the gate for `Recording`.
struct GatedIndicator {
    inner: Arc<FakeIndicator>,
    gate: Arc<TrayGate>,
}

impl Indicator for GatedIndicator {
    fn set_tray(&self, state: TrayState, retry_available: bool) {
        self.inner.set_tray(state, retry_available);
        if state == TrayState::Recording {
            self.gate.enter_and_wait();
        }
    }

    fn set_overlay(&self, state: &OverlayState) {
        self.inner.set_overlay(state);
    }
}

/// T-108: the claim check of the Esc tests, for the recording just pressed.
///
/// Contract (T-108 invariant and approach, option A): `Ok(())` only when the hotkey
/// thread has run this recording's claim and holds Esc bare and under the hotkey's
/// modifiers, with no `Warning{EscUnavailable}` among the observer's events; otherwise
/// `Err` with a message that names a refused claim (`EscUnavailable`) and prints
/// `rig.observer.events()`. No registration probe of the check may overlap a claim that
/// is still pending: the check orders itself after the claim (the Recording indicator,
/// then a synchronous round trip on the hotkey thread's request channel) and probes
/// once, never by polling registrations.
///
/// Why the order holds: `Shared::publish` queues the CancelKey request before it calls
/// `set_tray(Recording)`, so once the indicator shows Recording the request is on the
/// thread's channel; the thread runs the channel in order, so when the round trip's
/// `prepare` is answered the claim has run and its `cancel_key_result` (the warning,
/// emitted on the hotkey thread) is in the observer.
fn esc_claim(rig: &Rig) -> Result<(), String> {
    let recording = || {
        rig.indicator
            .trays()
            .iter()
            .any(|(s, _)| *s == TrayState::Recording)
    };
    // A memory read of the fake, not a registration probe: polling it races nothing.
    if !eventually(WAIT, recording) {
        return Err(format!(
            "Esc not claimed: no Recording indicator within {WAIT:?} of the press \
             (indicator calls: {:?}); observer events: {:?}",
            rig.indicator.calls(),
            rig.observer.events()
        ));
    }
    hotkey_round_trip(rig)?;
    if esc_unavailable_warnings(rig) > 0 {
        return Err(format!(
            "the hotkey thread's Esc claim was refused (Warning{{EscUnavailable}}); \
             observer events: {:?}",
            rig.observer.events()
        ));
    }
    if esc_claimed() {
        Ok(())
    } else {
        Err(format!(
            "Esc not claimed bare and under the hotkey's modifiers after the claim ran \
             (bare free: {}, Ctrl+Alt free: {}); observer events: {:?}",
            esc_is_free(0),
            esc_is_free(HOTKEY_MODS),
            rig.observer.events()
        ))
    }
}

/// The unused combination of [`hotkey_round_trip`] (never the rig's hotkey, never Esc).
const ROUND_TRIP_COMBO: &str = "Ctrl+Alt+F9";

/// T-108: one synchronous round trip on the hotkey thread's request channel: a
/// registrar `prepare` of [`ROUND_TRIP_COMBO`] under the spare id, then its `abort`.
/// When it returns `Ok`, every request queued before it (an Esc claim or release
/// included) has run. Prepare and abort touch only the spare id (`Registrations::
/// handle`), so the active hotkey and the Esc ids stay as they were; the check reads
/// back that the hotkey is still held and the spare combination free again (neither
/// probe is an Esc registration). `Err` (with the observer's events) when the thread
/// did not answer within the registrar's budget, refused the spare combination, or
/// the round trip changed a registration.
fn hotkey_round_trip(rig: &Rig) -> Result<(), String> {
    let spare = parse_hotkey(ROUND_TRIP_COMBO)
        .map_err(|e| format!("premise: {ROUND_TRIP_COMBO} does not parse: {e:?}"))?;
    let prepared = rig.registrar.prepare(spare, Mode::Hold).map_err(|e| {
        format!(
            "no round trip on the hotkey thread's request channel: prepare of \
             {ROUND_TRIP_COMBO} was {e:?} (refused, or not answered in the budget); \
             observer events: {:?}",
            rig.observer.events()
        )
    })?;
    rig.registrar.abort(prepared);
    if TakenHotkey::take().is_ok() {
        return Err(format!(
            "the round trip released the active hotkey Ctrl+Alt+Space; observer events: {:?}",
            rig.observer.events()
        ));
    }
    if !combo_is_free(ROUND_TRIP_COMBO) {
        return Err(format!(
            "{ROUND_TRIP_COMBO} is still registered after the round trip's abort; \
             observer events: {:?}",
            rig.observer.events()
        ));
    }
    Ok(())
}

/// The observer's `Warning{EscUnavailable}` count.
fn esc_unavailable_warnings(rig: &Rig) -> usize {
    rig.observer
        .events()
        .iter()
        .filter(|e| {
            matches!(
                e,
                DictationEvent::Warning {
                    code: WarningCode::EscUnavailable
                }
            )
        })
        .count()
}

/// Ends the gated hold and waits until the capture closed and Esc is given back, so
/// the next test starts with Esc free.
fn end_gated_hold(rig: &Rig, mut keys: Keys) {
    keys.release_all();
    assert!(
        eventually(WAIT, || rig.audio.open_handles() == 0),
        "cleanup: the capture is still open {WAIT:?} after the hotkey's release"
    );
    assert!(
        eventually(WAIT, esc_released),
        "cleanup: Esc still claimed after the hold ended; observer events: {:?}",
        rig.observer.events()
    );
}

#[test]
fn a_collision_planted_at_the_esc_claim_is_named_esc_unavailable_by_the_claim_check() {
    // T-108 reproduction (Acceptance "Test that fails without the change"): the CI
    // 39299815752 collision, planted. While the claim is pending the test takes
    // Ctrl+Alt+Esc, so the thread's one-shot claim gets bare Esc and is refused the
    // modified one (Warning{EscUnavailable}); then the test frees Ctrl+Alt+Esc. The
    // claim check must not say "not claimed" with no reason: it names the refusal.
    // Red before T-108: `eventually(WAIT, esc_claimed)` reads Ctrl+Alt+Esc free for the
    // whole WAIT and returns "Esc not claimed" (the CI message) with no events. Bite:
    // a check that does not read the observer's events (no EscUnavailable in its
    // message), a check that reads `Ok` while one combination is free.
    let _serial = serial();
    assert_hotkey_free();
    assert_esc_free();
    let window = TestWindow::open(Shape::TopLevel);
    window.front();
    let gate = Arc::new(TrayGate::default());
    let opener = OpenTrayOnDrop(Arc::clone(&gate));
    let rig = Rig::gated(&gate);
    let _hotkey = rig.start_hotkey();

    let (keys, _pressed_at) = rig.press_hotkey();
    assert!(
        gate.wait_entered(),
        "premise: the session's set_tray(Recording) never came: {:?}",
        rig.indicator.calls()
    );
    let planted = TakenHotkey::take_esc(HOTKEY_MODS)
        .expect("premise: Ctrl+Alt+Esc is free while the hotkey thread's claim is pending");
    drop(opener);
    assert!(
        eventually(WAIT, || esc_unavailable_warnings(&rig) >= 1),
        "premise: the planted Ctrl+Alt+Esc did not refuse the thread's claim: {:?}",
        rig.observer.events()
    );
    drop(planted);

    let verdict = esc_claim(&rig);
    let named = matches!(&verdict, Err(message) if message.contains("EscUnavailable"));
    assert!(
        named,
        "the claim check does not name the refused claim (want Err naming EscUnavailable, \
         got {verdict:?}); observer events: {:?}",
        rig.observer.events()
    );
    end_gated_hold(&rig, keys);
}

#[test]
fn an_esc_claim_check_started_while_the_claim_is_pending_waits_for_the_claim() {
    // T-108 invariant: no probe of the check overlaps a pending claim, and the verdict
    // is the shell's claim, never someone else's registration. While the hotkey thread
    // is held before its claim, the test takes both Esc combinations itself and starts
    // the check; it frees the thread after SETTLE. The thread's claim is then refused
    // both (Warning{EscUnavailable}). Red before T-108: the polling check reads the
    // test's own registrations as "claimed" at its first probe and returns Ok while
    // the claim is still pending. Bite: a check that probes before the round trip
    // (returns during the pending claim), a check that ignores the observer's warning
    // (Ok, because both combinations read taken).
    let _serial = serial();
    assert_hotkey_free();
    assert_esc_free();
    let window = TestWindow::open(Shape::TopLevel);
    window.front();
    let gate = Arc::new(TrayGate::default());
    let opener = OpenTrayOnDrop(Arc::clone(&gate));
    let rig = Rig::gated(&gate);
    let _hotkey = rig.start_hotkey();

    let (keys, _pressed_at) = rig.press_hotkey();
    assert!(
        gate.wait_entered(),
        "premise: the session's set_tray(Recording) never came: {:?}",
        rig.indicator.calls()
    );
    let bare = TakenHotkey::take_esc(0)
        .expect("premise: bare Esc is free while the hotkey thread's claim is pending");
    let modded = TakenHotkey::take_esc(HOTKEY_MODS)
        .expect("premise: Ctrl+Alt+Esc is free while the hotkey thread's claim is pending");

    let (verdict, returned_while_pending) = thread::scope(|scope| {
        let check = scope.spawn(|| esc_claim(&rig));
        let returned_while_pending = !holds_for(SETTLE, || !check.is_finished());
        drop(opener);
        let verdict = check.join().expect("the claim check panicked");
        (verdict, returned_while_pending)
    });
    assert!(
        eventually(WAIT, || esc_unavailable_warnings(&rig) >= 1),
        "premise: the test's own Esc registrations did not refuse the thread's claim: {:?}",
        rig.observer.events()
    );
    drop(modded);
    drop(bare);

    assert!(
        !returned_while_pending,
        "the claim check returned {verdict:?} while the hotkey thread's claim was still \
         pending (it probed before the claim ran)"
    );
    let named = matches!(&verdict, Err(message) if message.contains("EscUnavailable"));
    assert!(
        named,
        "the claim check took the test's own Esc registrations for the shell's claim \
         (want Err naming EscUnavailable, got {verdict:?}); observer events: {:?}",
        rig.observer.events()
    );
    end_gated_hold(&rig, keys);
}

#[test]
fn an_esc_claim_check_started_while_the_claim_is_pending_sees_the_claim_once_it_ran() {
    // T-108 happy path through the pending window (guard, green before and after the
    // change): the check starts while the hotkey thread is held before its claim and
    // nobody else holds Esc; once the thread runs the claim, the check is Ok and the
    // session never warned EscUnavailable. Bite: a check that probes once before the
    // claim ran (Err "not claimed" during the pending claim), a check that always fails,
    // a check whose probe collides with the claim (EscUnavailable in the events).
    let _serial = serial();
    assert_hotkey_free();
    assert_esc_free();
    let window = TestWindow::open(Shape::TopLevel);
    window.front();
    let gate = Arc::new(TrayGate::default());
    let opener = OpenTrayOnDrop(Arc::clone(&gate));
    let rig = Rig::gated(&gate);
    let _hotkey = rig.start_hotkey();

    let (keys, _pressed_at) = rig.press_hotkey();
    assert!(
        gate.wait_entered(),
        "premise: the session's set_tray(Recording) never came: {:?}",
        rig.indicator.calls()
    );
    let verdict = thread::scope(|scope| {
        let check = scope.spawn(|| esc_claim(&rig));
        let _ = holds_for(SETTLE, || !check.is_finished());
        drop(opener);
        check.join().expect("the claim check panicked")
    });
    assert_eq!(
        verdict,
        Ok(()),
        "observer events: {:?}",
        rig.observer.events()
    );
    assert_eq!(
        esc_unavailable_warnings(&rig),
        0,
        "the check collided with the claim: {:?}",
        rig.observer.events()
    );
    assert!(esc_claimed(), "Esc not held after an Ok claim check");
    end_gated_hold(&rig, keys);
}

// ---- T-010: resume, unlock and suspend reach the hotkey thread ------------------------
//
// The OS sends WM_POWERBROADCAST and WM_WTSSESSION_CHANGE to the hotkey thread's own
// hidden top-level window (titled "Voicen hotkey"; option A gives it a class of our own,
// option B keeps STATIC with a subclass, so the test finds it by title in this process).
// The runner cannot sleep or lock, so the test sends the same messages with SendMessageW.
// The registration is proven with the 1409 probe (`TakenHotkey::take` refused), never with
// a synthetic hold (F-014); only the suspend test uses one hold, and sends the suspend at
// once after the observed press.

use voicen_core::i18n;
use voicen_core::recording::OverlayState;
use windows::core::{w, HRESULT, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetWindowThreadProcessId, SendMessageW, PBT_APMRESUMEAUTOMATIC, PBT_APMSUSPEND,
    WM_POWERBROADCAST, WM_WTSSESSION_CHANGE, WTS_SESSION_UNLOCK,
};

/// ERROR_HOTKEY_ALREADY_REGISTERED.
const ALREADY_REGISTERED: u32 = 1409;

/// `FakeShellRequests::calls()` (Debug) after the one request of a transition to failed.
const ONE_HOTKEY_FIELD_REQUEST: &str = "[OpenSettings(Recording, Some(RecordingHotkey))]";

/// The hotkey thread's window: the one top-level window of this process titled
/// "Voicen hotkey" (hidden windows included).
#[track_caller]
fn hotkey_window() -> HWND {
    let pid = std::process::id();
    let mut found = Vec::new();
    let mut after: Option<HWND> = None;
    loop {
        // SAFETY: static strings; `after` is a window FindWindowExW just returned.
        let next = unsafe { FindWindowExW(None, after, PCWSTR::null(), w!("Voicen hotkey")) };
        let Ok(hwnd) = next else { break };
        if hwnd.is_invalid() {
            break;
        }
        let mut owner = 0u32;
        // SAFETY: a window handle and a writable u32.
        let _ = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
        if owner == pid {
            found.push(hwnd);
        }
        after = Some(hwnd);
    }
    assert_eq!(
        found.len(),
        1,
        "the running hotkey thread has no single top-level window titled \"Voicen hotkey\" in \
         this process (the test finds it by title): {found:?}"
    );
    found[0]
}

/// Sends `msg` to `hwnd` and waits until its procedure returned (the hotkey thread
/// dispatches sent messages inside GetMessageW).
fn deliver(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) {
    // SAFETY: a window of this process; plain integer arguments.
    let _ = unsafe { SendMessageW(hwnd, msg, Some(WPARAM(wparam)), Some(LPARAM(lparam))) };
}

/// The session id of this process (the `lParam` of WM_WTSSESSION_CHANGE).
fn this_session() -> isize {
    let mut id = 0u32;
    // SAFETY: this process's id and a writable u32.
    unsafe { ProcessIdToSessionId(std::process::id(), &mut id) }
        .expect("ProcessIdToSessionId for this process");
    id as isize
}

/// A simulated resume from sleep.
fn resume(hwnd: HWND) {
    deliver(hwnd, WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC as usize, 0);
}

/// A simulated unlock of this session.
fn unlock(hwnd: HWND) {
    deliver(
        hwnd,
        WM_WTSSESSION_CHANGE,
        WTS_SESSION_UNLOCK as usize,
        this_session(),
    );
}

/// The 1409 probe: `Ok` when Ctrl+Alt+Space is held by someone else (the test cannot take
/// it: ERROR_HOTKEY_ALREADY_REGISTERED), `Err` when it is free or refused otherwise.
fn held_by_someone_else() -> Result<(), String> {
    match TakenHotkey::take() {
        Ok(_free) => Err("Ctrl+Alt+Space is free (nobody holds it)".to_string()),
        Err(e) if e.code() == HRESULT::from_win32(ALREADY_REGISTERED) => Ok(()),
        Err(e) => Err(format!("refused, but not with 1409: {e:?}")),
    }
}

impl Rig {
    /// The `warning kind=hotkey_register_failed` lines so far.
    fn register_failed(&self) -> Vec<String> {
        headed(&log_lines(&self.logs), "warning")
            .into_iter()
            .filter(|l| has_pair(l, "kind", "hotkey_register_failed"))
            .collect()
    }

    /// How many times the hotkey notice was published.
    fn hotkey_notices(&self) -> usize {
        self.indicator
            .overlays()
            .iter()
            .filter(|o| {
                matches!(o, OverlayState::Message { id, .. } if *id == i18n::NOTICE_HOTKEY_UNAVAILABLE)
            })
            .count()
    }

    fn last_tray(&self) -> Option<TrayState> {
        self.indicator.trays().last().map(|(s, _)| *s)
    }

    /// The thread started with Ctrl+Alt+Space taken by the test: HotkeyError, one
    /// failed line, one notice, one settings request (T-055's startup branch).
    #[track_caller]
    fn assert_started_failing(&self) {
        assert!(
            eventually(WAIT, || self.last_tray() == Some(TrayState::HotkeyError)),
            "premise: no HotkeyError at start with the hotkey taken: {:?}",
            self.indicator.trays()
        );
        assert!(
            eventually(WAIT, || self.register_failed().len() == 1),
            "premise: not one hotkey_register_failed line at start: {:?}",
            self.register_failed()
        );
        assert_eq!(
            format!("{:?}", self.requests.calls()),
            ONE_HOTKEY_FIELD_REQUEST,
            "premise"
        );
        assert_eq!(self.hotkey_notices(), 1, "premise");
    }
}

/// Acceptance 3 and 4: a hotkey taken at start and freed since is registered again by
/// `event`: the tray leaves HotkeyError, the test's own take fails with 1409, the session
/// reads it as registered, and no new failed line, notice or settings request appears.
fn a_freed_hotkey_is_registered_again_on(event: &str, send: fn(HWND)) {
    let _serial = serial();
    assert_hotkey_free();
    let taken = TakenHotkey::take().expect("premise: the test takes Ctrl+Alt+Space");
    let rig = Rig::new();
    let _hotkey = rig.start_hotkey();
    rig.assert_started_failing();
    let hwnd = hotkey_window();
    drop(taken);

    send(hwnd);

    // No probe before the tray changes: a probe's take could win the re-registration.
    assert!(
        eventually(WAIT, || rig.last_tray() == Some(TrayState::Idle)),
        "{event}: the tray is still HotkeyError {WAIT:?} after the {event} (no re-register): {:?}",
        rig.indicator.trays()
    );
    assert_eq!(
        held_by_someone_else(),
        Ok(()),
        "{event}: the hotkey thread does not hold Ctrl+Alt+Space after the {event}"
    );
    assert!(rig.session.hotkey_registered(), "{event}");
    assert_eq!(
        rig.register_failed().len(),
        1,
        "{event}: a failed line for a retry that succeeded: {:?}",
        rig.register_failed()
    );
    assert_eq!(rig.hotkey_notices(), 1, "{event}: a notice for the success");
    assert_eq!(
        format!("{:?}", rig.requests.calls()),
        ONE_HOTKEY_FIELD_REQUEST,
        "{event}: a settings request for the success"
    );
}

#[test]
fn a_resume_re_registers_a_hotkey_taken_at_start_and_freed_since() {
    // T-010 Acceptance 3 (FR-26, spec 001 US4; invariant 1): PBT_APMRESUMEAUTOMATIC to the
    // hotkey window re-makes the active registration from the thread's own codes and
    // reports it, so a success clears HotkeyError. Bite: no window procedure of ours (the
    // STATIC procedure swallows the message: today), the event re-posted but no arm in
    // the loop, the re-register skipped while `registered == false`, the result not
    // reported to the session (the tray stays HotkeyError).
    a_freed_hotkey_is_registered_again_on("resume", resume);
}

#[test]
fn an_unlock_re_registers_a_hotkey_taken_at_start_and_freed_since() {
    // T-010 Acceptance 4: the same for WM_WTSSESSION_CHANGE / WTS_SESSION_UNLOCK. Bite:
    // only WM_POWERBROADCAST handled, the session-change code read from lParam instead of
    // wParam.
    a_freed_hotkey_is_registered_again_on("unlock", unlock);
}

#[test]
fn a_registered_hotkey_stays_registered_across_a_resume_and_an_unlock() {
    // T-010 Acceptance 3, second half (invariant 1 "unregister if registered, then
    // register under the same id"): with the hotkey registered, a resume and then an
    // unlock leave it registered (1409 from the test after each), with no
    // hotkey_register_failed line, no HotkeyError, no notice and no settings request.
    // Expected green today (nothing reacts yet); it guards the re-register. Bite: a
    // re-register without the unregister (RegisterHotKey on the same id refuses: a
    // failed line and HotkeyError), an unregister with no register after it (the probe
    // finds the hotkey free), the spare id used instead of the active one.
    let _serial = serial();
    assert_hotkey_free();
    let rig = Rig::new();
    let _hotkey = rig.start_hotkey();
    assert!(
        eventually(WAIT, || held_by_someone_else().is_ok()),
        "premise: the running hotkey thread does not hold Ctrl+Alt+Space"
    );
    let hwnd = hotkey_window();
    for (event, send) in [("resume", resume as fn(HWND)), ("unlock", unlock)] {
        send(hwnd);
        // The re-register runs from the loop after the procedure re-posted it.
        thread::sleep(SETTLE);
        assert_eq!(
            held_by_someone_else(),
            Ok(()),
            "{event}: Ctrl+Alt+Space is not held by the hotkey thread after the {event}"
        );
    }
    assert_eq!(rig.register_failed(), Vec::<String>::new());
    assert!(
        !rig.indicator
            .trays()
            .iter()
            .any(|(s, _)| *s == TrayState::HotkeyError),
        "{:?}",
        rig.indicator.trays()
    );
    assert_eq!(rig.hotkey_notices(), 0);
    assert_eq!(format!("{:?}", rig.requests.calls()), "[]");
    assert!(rig.session.hotkey_registered());
}

#[test]
fn a_retry_while_the_hotkey_is_still_taken_keeps_hotkey_error_and_notifies_once() {
    // T-010 Acceptance 5, failure branch (decision #131): taken at start and still taken,
    // a resume and then an unlock each retry: one more `hotkey_register_failed
    // os_code=1409` line per retry, the tray stays HotkeyError (never Idle), and no
    // second notice and no second OpenSettings(Recording, RecordingHotkey). Bite: no
    // retry (today: one line only), the retry's result not logged or logged without the
    // Win32 code, the session notifying on every failure, a retry that reports success
    // without registering.
    let _serial = serial();
    assert_hotkey_free();
    let _taken = TakenHotkey::take().expect("premise: the test takes Ctrl+Alt+Space");
    let rig = Rig::new();
    let _hotkey = rig.start_hotkey();
    rig.assert_started_failing();
    let hwnd = hotkey_window();

    for (n, (event, send)) in [("resume", resume as fn(HWND)), ("unlock", unlock)]
        .into_iter()
        .enumerate()
    {
        let want = n + 2;
        send(hwnd);
        assert!(
            eventually(WAIT, || rig.register_failed().len() >= want),
            "{event}: no hotkey_register_failed line for the retry: {:?}",
            rig.register_failed()
        );
        assert!(
            holds_for(SETTLE, || rig.register_failed().len() == want),
            "{event}: not exactly one more failed line: {:?}",
            rig.register_failed()
        );
        let lines = rig.register_failed();
        assert!(
            has_pair(&lines[want - 1], "os_code", "1409"),
            "{event}: {}",
            lines[want - 1]
        );
    }
    assert_eq!(
        rig.indicator.trays(),
        vec![(TrayState::HotkeyError, false)],
        "the tray left HotkeyError during the failed retries"
    );
    assert!(!rig.session.hotkey_registered());
    assert_eq!(
        rig.hotkey_notices(),
        1,
        "a second hotkey notice while already failing: {:?}",
        rig.indicator.overlays()
    );
    assert_eq!(
        format!("{:?}", rig.requests.calls()),
        ONE_HOTKEY_FIELD_REQUEST,
        "a second settings request while already failing"
    );
}

#[test]
fn a_suspend_during_a_hold_closes_the_capture_as_suspended() {
    // T-010 Acceptance 6 (invariant 3; spec 001 edge case "sleep while recording"):
    // PBT_APMSUSPEND to the hotkey window during a hold, with the keys still down,
    // closes the capture at once and ends the recording Suspended; the audio goes on to
    // one job; the later key-up starts no second stop (the poll was stopped). Bite: no
    // suspend arm (today: the capture stays open until the key-up, end Released), the
    // poll left running (a second session call at the key-up: harmless in the core, but
    // the end must still be Suspended), suspend mapped to `hotkey_released` (TooShort or
    // Released). The end is compared through its Debug form, so this binary builds
    // before `RecordingEnd::Suspended` exists.
    let _serial = serial();
    assert_hotkey_free();
    let window = TestWindow::open(Shape::TopLevel);
    window.front();
    let rig = Rig::new();
    let _hotkey = rig.start_hotkey();
    let hwnd = hotkey_window();

    let (mut keys, _pressed_at) = rig.press_hotkey();
    precondition(
        "async_keys",
        HOTKEY_KEYS.iter().all(|&vk| is_down(vk)),
        "Ctrl+Alt+Space no longer reads down right after the observed press (F-014)",
    );
    deliver(hwnd, WM_POWERBROADCAST, PBT_APMSUSPEND as usize, 0);
    assert!(
        eventually(WAIT, || rig.audio.open_handles() == 0),
        "the capture is still open {WAIT:?} after PBT_APMSUSPEND with the hotkey held"
    );
    keys.release_all();
    assert!(
        eventually(WAIT, || rig.jobs() == 1),
        "the suspended recording was not processed"
    );
    thread::sleep(SETTLE);
    assert_eq!(
        format!("{:?}", rig.ends()),
        "[Suspended]",
        "the recording did not end Suspended (an early end of the synthetic hold, F-014, \
         would show Released or TooShort here)"
    );
    assert_eq!(rig.jobs(), 1);
    assert_eq!(rig.audio.start_calls(), 1);
}
