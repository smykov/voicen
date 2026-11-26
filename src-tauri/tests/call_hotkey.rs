//! T-129: the call hotkey (Ctrl+Alt+R by default; spec 007 FR-004, FR-040, US9 #3; T030, T031):
//! a second registration slot on the hotkey thread, registered at startup by `start_dictation_with`
//! and by the two-step registrar of a save (`prepare_call` / commit / abort), whose press only
//! posts `CallRequest::Record(CallSource::Hotkey)` and whose conflict never blocks dictation.
//! Through the real wiring: `AppRig` (`build_app`, `load_settings_with` over the shell's registrar
//! handle), `start_dictation_with` over fakes with one recording `FakeNotifier`, and
//! `voicen_lib::call::start_calls` over the same notifier (as `run()` does). Windows CI only
//! (decision #5); not compiled on the Linux gate.
//!
//! Red: this file does not compile until `voicen_lib::dictation::call_hotkey_mark` exists (the
//! unresolved import is the red). The surface these tests pin (the developer may rename, the
//! tests follow):
//! - `voicen_lib::dictation::call_hotkey_mark<R: Runtime>(app: &AppHandle<R>) -> Option<FieldId>`:
//!   the shell-held mark of the Calls tab: `Some(FieldId::CallHotkey)` while the call slot is
//!   failing (startup or reassert), `None` otherwise (I3). No wire carries it yet (the task's
//!   open question), so the tests read it here.
//! - `start_dictation_with` registers the call slot from `snapshot().calls.hotkey` and offers
//!   `notice.call_hotkey_conflict` (`NOTICE_CALL_HOTKEY_CONFLICT`) to its notifier once per
//!   transition to failing.
//!
//! Test rules: a combination is probed (`combo_is_free`, `held_by_someone_else`) only after a
//! synchronous answer of the thread (F-020: a probe's take can win a pending re-registration);
//! the call slot is exercised by single presses, not holds (F-014); every wait is a state, never
//! a sleep. A taken combination is taken by this thread (`TakenHotkey`, 1409 for the app).
//! Data: a `TempDir` per test, fake key; nothing is logged beyond the app's own lines.
#![cfg(windows)]

mod call_shell_support;
mod timing_support;
mod win32_support;

use std::sync::{Arc, Mutex};
use std::thread;

use call_shell_support::{plain_opener, FakeExit, FakeFolder, FakeSave, WavSource};
use serde_json::Value;
use tauri::{Listener, Manager};
use timing_support::HANG;
use voicen_core::autostart::{AutostartError, FakeAutostart};
use voicen_core::call::session::CallSession;
use voicen_core::clock::SystemMonotonic;
use voicen_core::platform::{
    FakeAudioSource, FakeClipboard, FakeIndicator, FakeNotifier, FakePaster,
};
use voicen_core::recording::TrayState;
use voicen_core::secrets as creds;
use voicen_core::settings::file::SETTINGS_FILE;
use voicen_core::settings::service::{SaveOutcome, SaveRequest};
use voicen_core::settings::{EngineKind, ErrorCode, FieldError, FieldId};
use voicen_core::test_support::local_models::FakeDisk;
use voicen_lib::call::{start_calls, CallPorts};
use voicen_lib::dictation::{
    call_hotkey_mark, start_dictation_with, DictationHandle, DictationPorts,
};
use voicen_lib::settings_window::LABEL;
use win32_support::{
    assert_hotkey_free, combo_is_free, engine_factory, eventually, holds_for, message_id, serial,
    shown, AppRig, Keys, Shown, TakenHotkey, TextEngine, HOLD, SETTLE, VK_CONTROL, VK_MENU,
};
use windows::core::{w, HRESULT, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetWindowThreadProcessId, SendMessageW, PBT_APMRESUMEAUTOMATIC,
    WM_POWERBROADCAST,
};

const CALL_HOTKEY: &str = "Ctrl+Alt+R";
const VK_R: u16 = 0x52;
const CALL_KEYS: [u16; 3] = [VK_CONTROL, VK_MENU, VK_R];
const NEW_CALL_HOTKEY: &str = "Ctrl+Alt+F10";
const VK_F10: u16 = 0x79;
const NEW_CALL_KEYS: [u16; 3] = [VK_CONTROL, VK_MENU, VK_F10];
const DEFAULT_HOTKEY: &str = "Ctrl+Alt+Space";
const DEFAULT_KEYS: [u16; 3] = win32_support::HOTKEY_KEYS;
const CALL_CONFLICT_NOTICE: &str = "notice.call_hotkey_conflict";
const HOTKEY_NOTICE: &str = "notice.hotkey_unavailable";
const FOCUS: &str = "settings://focus";
/// ERROR_HOTKEY_ALREADY_REGISTERED.
const ALREADY_REGISTERED: u32 = 1409;

/// The app, its started dictation and its started call part, over fakes.
struct Started {
    rig: AppRig,
    audio: Arc<FakeAudioSource>,
    indicator: Arc<FakeIndicator>,
    notifier: Arc<FakeNotifier>,
    source: Arc<WavSource>,
    focus: Arc<Mutex<Vec<Value>>>,
    dictation: Option<DictationHandle>,
}

fn start(rig: AppRig) -> Started {
    let audio = Arc::new(FakeAudioSource::new());
    let indicator = Arc::new(FakeIndicator::new());
    let notifier = Arc::new(FakeNotifier::new());
    let source = WavSource::new();
    let engine = TextEngine::new("call hotkey test transcript");
    let focus = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = focus.clone();
    rig.app.listen_any(FOCUS, move |event| {
        let payload = serde_json::from_str(event.payload()).expect("focus payload is JSON");
        sink.lock().expect("lock").push(payload);
    });
    // As `run()`: the dictation first (it registers the call slot), then the call part.
    let dictation = start_dictation_with(
        rig.app.handle(),
        DictationPorts {
            audio: audio.clone(),
            clipboard: Arc::new(FakeClipboard::new()),
            paster: Arc::new(FakePaster::new()),
            engine_factory: Some(engine_factory(&engine)),
            builtin: None,
            indicator: indicator.clone(),
            credentials: Arc::clone(&rig.creds),
            hotkeys: Arc::clone(&rig.hotkeys),
            detector: Err(voicen_core::vad::VadError::Unavailable),
            temp_audio: Arc::new(voicen_core::platform::FakeTempAudioStore::new()),
        },
        notifier.clone(),
    )
    .expect("start_dictation_with");
    let calls_root = rig.dir.path().join("calls");
    std::fs::create_dir_all(&calls_root).expect("the calls root");
    start_calls(
        rig.app.handle(),
        CallPorts {
            source: source.clone(),
            disk: FakeDisk::with_available(100_000_000_000),
            mono: Arc::new(SystemMonotonic),
            open_track: plain_opener(),
            notifier: notifier.clone(),
            calls_root,
            clipboard: Arc::new(FakeClipboard::new()),
            folder: Arc::new(FakeFolder::default()),
            save: Arc::new(FakeSave::default()),
            credentials: Arc::clone(&rig.creds),
            detector: None,
            exit_confirm: Arc::new(FakeExit::default()),
        },
    )
    .expect("the call part starts");
    Started {
        rig,
        audio,
        indicator,
        notifier,
        source,
        focus,
        dictation: Some(dictation),
    }
}

impl Started {
    /// Waits until the app holds `combo` (this thread can no longer take it).
    #[track_caller]
    fn assert_app_holds(&self, combo: &str, why: &str) {
        assert!(
            eventually(|| !combo_is_free(combo)),
            "{why}: the app does not hold {combo}"
        );
    }

    /// Whether a call records; polls `try_is_recording` while pumping (never blocks on the
    /// session lock: the call worker may need this thread's messages, T-128).
    fn recording(&self) -> bool {
        let Some(session) = self.rig.app.try_state::<Arc<CallSession>>() else {
            return false;
        };
        let session = Arc::clone(&session);
        let mut known = None;
        let answered = eventually(|| {
            known = session.try_is_recording();
            known.is_some()
        });
        assert!(answered, "HANG: the session lock stayed held for {HANG:?}");
        known.unwrap_or(false)
    }

    /// One synthetic press of `keys`, released at once (F-014: a press, not a hold).
    #[track_caller]
    fn press(&self, keys: &[u16]) {
        let mut held = Keys::press(keys);
        held.release_all();
    }

    /// The lines of the app's log that have every one of `words` as a whole word.
    fn lines(&self, words: &[&str]) -> Vec<String> {
        self.rig
            .log_lines()
            .into_iter()
            .filter(|l| words.iter().all(|w| l.split_whitespace().any(|x| x == *w)))
            .collect()
    }

    fn save_with(&self, edit: impl FnOnce(&mut voicen_core::settings::Settings)) -> SaveOutcome {
        let mut draft = (*self.rig.service.snapshot()).clone();
        edit(&mut draft);
        self.rig.service.save(SaveRequest {
            settings: draft,
            keys: creds::KeyEdits::default(),
        })
    }

    fn save_call_hotkey(&self, text: &str) -> SaveOutcome {
        self.save_with(|s| s.calls.hotkey = text.to_string())
    }

    /// The offers to the notifier with the message id `id`.
    fn offers(&self, id: &str) -> usize {
        self.notifier
            .calls()
            .iter()
            .filter(|c| message_id(c.id) == id)
            .count()
    }

    fn has_hotkey_error(&self) -> bool {
        self.indicator
            .trays()
            .iter()
            .any(|(s, _)| *s == TrayState::HotkeyError)
    }

    fn settings_file(&self) -> Option<String> {
        std::fs::read_to_string(self.rig.dir.path().join(SETTINGS_FILE)).ok()
    }

    /// A dictation hold of the default hotkey opens one capture, and its release closes it.
    #[track_caller]
    fn assert_hold_records(&self, what: &str) {
        let before = self.audio.start_calls();
        let mut held = Keys::press(&DEFAULT_KEYS);
        assert!(
            eventually(|| self.audio.start_calls() == before + 1),
            "{what}: no capture within {HANG:?} of the press (start calls {} -> {})",
            before,
            self.audio.start_calls()
        );
        thread::sleep(HOLD);
        held.release_all();
        assert!(
            eventually(|| self.audio.open_handles() == 0),
            "{what}: the capture stayed open after the release"
        );
    }
}

fn call_hotkey_unavailable() -> SaveOutcome {
    SaveOutcome::Refused {
        errors: vec![FieldError::new(
            FieldId::CallHotkey,
            ErrorCode::HotkeyUnavailable,
        )],
        form_error: None,
    }
}

// ---- the hotkey thread's window (T-010: resume reaches it as WM_POWERBROADCAST) ------------

/// The one top-level window of this process titled "Voicen hotkey".
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
        "no single \"Voicen hotkey\" window: {found:?}"
    );
    found[0]
}

/// A simulated resume from sleep: sent, and returned once the window procedure ran.
fn resume(hwnd: HWND) {
    // SAFETY: a window of this process; plain integer arguments.
    let _ = unsafe {
        SendMessageW(
            hwnd,
            WM_POWERBROADCAST,
            Some(WPARAM(PBT_APMRESUMEAUTOMATIC as usize)),
            Some(LPARAM(0)),
        )
    };
}

/// The 1409 probe: `Ok` when `combo` is held by someone else.
fn held_by_someone_else(combo: &str) -> Result<(), String> {
    match TakenHotkey::take_combo(combo) {
        Ok(_free) => Err(format!("{combo} is free (nobody holds it)")),
        Err(e) if e.code() == HRESULT::from_win32(ALREADY_REGISTERED) => Ok(()),
        Err(e) => Err(format!("refused, but not with 1409: {e:?}")),
    }
}

// ---- (a) startup registration and the press ---------------------------------------------

#[test]
fn the_call_hotkey_is_registered_at_startup_and_its_presses_start_and_stop_a_call() {
    // Acceptance 1 (T030, T031): after `start_dictation_with` returned, the app holds
    // Ctrl+Alt+R (probed only now: F-020). One press starts a call with
    // `last_combination` {record} (`call_requested source=hotkey`, both tracks opened,
    // nothing else started); the next press stops it (`call_stopped reason=hotkey`, one
    // `call_requested` in all). Dictation is unaffected (hotkey held, no HotkeyError, no
    // call notice). Bite: the call slot never registered (the app does not hold the
    // combination), the press routed to the dictation session (no `call_requested`), the
    // source mixed up (`source=tray`), a press that always starts (the second press does not
    // stop), a start that opens one track only.
    let _serial = serial();
    assert_hotkey_free();
    assert!(
        combo_is_free(CALL_HOTKEY),
        "premise: {CALL_HOTKEY} is free on the runner"
    );
    let app = start(AppRig::new(EngineKind::Api));

    app.assert_app_holds(CALL_HOTKEY, "after start");
    app.assert_app_holds(DEFAULT_HOTKEY, "the dictation hotkey");

    app.press(&CALL_KEYS);
    assert!(
        eventually(|| app.recording()),
        "the call hotkey did not start a call"
    );
    assert_eq!(app.lines(&["call_requested", "source=hotkey"]).len(), 1);
    app.source.pump();
    assert_eq!(app.source.opens(), 2, "both tracks open on a Record start");

    app.press(&CALL_KEYS);
    assert!(
        eventually(|| !app.recording()),
        "the second press did not stop the call"
    );
    assert_eq!(app.lines(&["call_stopped", "reason=hotkey"]).len(), 1);
    assert_eq!(
        app.lines(&["call_requested"]).len(),
        1,
        "a stop wrote a request line"
    );
    assert!(!app.has_hotkey_error(), "{:?}", app.indicator.trays());
    assert_eq!(app.offers(CALL_CONFLICT_NOTICE), 0);
    assert!(app.rig.session().hotkey_registered());
}

// ---- (b) failure branch: a taken call hotkey at startup ------------------------------------

#[test]
fn a_taken_call_hotkey_at_startup_is_a_notice_only_and_a_resume_recovers_it() {
    // Acceptance 2 (failure branch; US9 #3, FR-040; I3): with Ctrl+Alt+R taken by this
    // thread before start,
    // - exactly one `notice.call_hotkey_conflict` is offered to the notifier, no more while
    //   it stays taken (a resume retries silently: once per transition to failing);
    // - no settings window opens and no `settings://focus` arrives; the tray never shows
    //   HotkeyError; no `notice.hotkey_unavailable` overlay;
    // - the dictation hotkey works: `hotkey_registered()` is true and a hold opens one
    //   capture;
    // - the call-hotkey mark reads `FieldId::CallHotkey`;
    // then, freed and a simulated resume: the slot is registered again (probed only after
    // the mark cleared: F-020), the mark clears, no second notice, and a press starts a
    // call. Bite: the failure routed to `DictationSession::hotkey_registration(false)`
    // (HotkeyError, a hotkey_unavailable notice, a settings window), the failure swallowed
    // (no notice, no mark), a notice per retry, the call slot's failure failing the dictation
    // registration (hold dead), the mark never cleared by a success, no reassert of the call
    // slot on resume.
    let _serial = serial();
    assert_hotkey_free();
    let taken = TakenHotkey::take_combo(CALL_HOTKEY).expect("premise: the test takes Ctrl+Alt+R");
    let app = start(AppRig::new(EngineKind::Api));

    assert!(
        eventually(|| app.offers(CALL_CONFLICT_NOTICE) == 1),
        "no notice.call_hotkey_conflict offered: {:?}",
        app.notifier.calls()
    );
    assert_eq!(
        app.notifier.calls().len(),
        1,
        "exactly the one conflict notice: {:?}",
        app.notifier.calls()
    );
    assert_eq!(
        call_hotkey_mark(app.rig.app.handle()),
        Some(FieldId::CallHotkey)
    );
    assert!(
        holds_for(SETTLE, || {
            app.rig.app.get_webview_window(LABEL).is_none()
                && app.focus.lock().expect("lock").is_empty()
                && !app.has_hotkey_error()
                && !shown(&app.indicator.overlays())
                    .iter()
                    .any(|o| matches!(o, Shown::Message(id, _) if *id == HOTKEY_NOTICE))
        }),
        "a settings window, focus event, HotkeyError or hotkey notice for a call conflict: \
         trays {:?}, overlays {:?}",
        app.indicator.trays(),
        shown(&app.indicator.overlays())
    );
    assert!(app.rig.session().hotkey_registered());
    app.assert_app_holds(DEFAULT_HOTKEY, "the dictation hotkey");
    app.assert_hold_records("the dictation hold with the call hotkey taken");

    // Still taken: a resume retries and fails silently.
    let hwnd = hotkey_window();
    resume(hwnd);
    assert!(
        holds_for(SETTLE, || app.offers(CALL_CONFLICT_NOTICE) == 1
            && call_hotkey_mark(app.rig.app.handle())
                == Some(FieldId::CallHotkey)),
        "a retry while still taken offered another notice or cleared the mark: {:?}",
        app.notifier.calls()
    );

    drop(taken);
    resume(hwnd);
    // No probe before the mark clears: a probe's take could win the re-registration.
    assert!(
        eventually(|| call_hotkey_mark(app.rig.app.handle()).is_none()),
        "the mark stayed after the combination was freed and the resume"
    );
    assert_eq!(held_by_someone_else(CALL_HOTKEY), Ok(()));
    assert_eq!(
        app.offers(CALL_CONFLICT_NOTICE),
        1,
        "a notice for the success"
    );
    assert!(!app.has_hotkey_error());
    app.press(&CALL_KEYS);
    assert!(
        eventually(|| app.recording()),
        "the recovered call hotkey did not start a call"
    );
}

/// The `call_hotkey_unavailable` key of a serialized `SettingsView` (T-232); `Null` when the
/// key is missing, so a missing field is an assertion failure, not a compile error.
fn unavailable_on_the_wire(view: impl serde::Serialize) -> Value {
    serde_json::to_value(view).expect("serializes")["call_hotkey_unavailable"].clone()
}

#[test]
fn a_taken_call_hotkey_at_startup_reaches_the_settings_view_until_a_save_moves_it() {
    // T-232 (decisions #306, #373): the failure branch's field. With Ctrl+Alt+R taken by this
    // thread before start, the managed `SettingsService`'s `view()` (what `settings_get` and
    // `settings://changed` send) has `call_hotkey_unavailable == true`, and it stays true while
    // a resume retries and fails; a save that leaves the call hotkey alone keeps it true in
    // its Saved view (no re-registration); a save that moves `calls.hotkey` to the free
    // Ctrl+Alt+F10 is Saved with the field false, the next `view()` reads false too and the
    // shell's mark is cleared. Bite: the view field never filled from the shell's mark (always
    // false), the handle's mark a different `Arc` than the thread's (the field stays false),
    // the Saved view built before the commit (true after the move), the field cleared by a
    // save that did not register the call slot, the field never cleared by the commit.
    let _serial = serial();
    assert_hotkey_free();
    assert!(
        combo_is_free(NEW_CALL_HOTKEY),
        "premise: {NEW_CALL_HOTKEY} is free on the runner"
    );
    let taken = TakenHotkey::take_combo(CALL_HOTKEY).expect("premise: the test takes Ctrl+Alt+R");
    let app = start(AppRig::new(EngineKind::Api));

    assert!(
        eventually(|| unavailable_on_the_wire(app.rig.service.view()) == Value::Bool(true)),
        "view().call_hotkey_unavailable is not true while {CALL_HOTKEY} is taken: {}",
        unavailable_on_the_wire(app.rig.service.view())
    );
    assert_eq!(
        call_hotkey_mark(app.rig.app.handle()),
        Some(FieldId::CallHotkey),
        "the view and the shell's mark read one state"
    );

    // A resume while still taken fails again: the field stays true.
    resume(hotkey_window());
    assert!(
        holds_for(SETTLE, || unavailable_on_the_wire(app.rig.service.view())
            == Value::Bool(true)),
        "a failing retry cleared the field"
    );

    // A save that does not touch the call hotkey does not re-register it: still true.
    let outcome = app.save_with(|s| s.start_with_windows = !s.start_with_windows);
    let SaveOutcome::Saved { view, .. } = outcome else {
        panic!("expected Saved, got {outcome:?}");
    };
    assert_eq!(
        unavailable_on_the_wire(view),
        Value::Bool(true),
        "the Saved view of an unrelated save dropped a mark that is still true"
    );
    assert_eq!(
        call_hotkey_mark(app.rig.app.handle()),
        Some(FieldId::CallHotkey)
    );

    // A save that moves the call hotkey to a free combination registers it and clears the mark.
    let outcome = app.save_call_hotkey(NEW_CALL_HOTKEY);
    let SaveOutcome::Saved { view, .. } = outcome else {
        panic!("expected Saved, got {outcome:?}");
    };
    assert_eq!(
        unavailable_on_the_wire(view),
        Value::Bool(false),
        "the Saved view after the call-slot commit still says unavailable"
    );
    assert_eq!(
        unavailable_on_the_wire(app.rig.service.view()),
        Value::Bool(false),
        "the next view() after the successful save"
    );
    assert_eq!(call_hotkey_mark(app.rig.app.handle()), None);
    app.assert_app_holds(NEW_CALL_HOTKEY, "after the save");
    drop(taken);
}

// ---- (c) a save of calls.hotkey ------------------------------------------------------------

#[test]
fn a_saved_new_call_hotkey_is_registered_and_the_old_one_freed() {
    // Acceptance 3 (T030): saving Ctrl+Alt+F10 as the call hotkey is Saved; afterwards the app
    // holds Ctrl+Alt+F10, Ctrl+Alt+R is free, the dictation hotkey is still held, and a press
    // of the new combination starts a call (without a restart). Bite: the save only written
    // (the old combination still held, the new one free), commit not freeing the old slot,
    // WM_HOTKEY matched against the old id only, the dictation slot touched.
    let _serial = serial();
    assert_hotkey_free();
    assert!(
        combo_is_free(NEW_CALL_HOTKEY),
        "premise: {NEW_CALL_HOTKEY} is free on the runner"
    );
    let app = start(AppRig::new(EngineKind::Api));
    app.assert_app_holds(CALL_HOTKEY, "premise");

    let outcome = app.save_call_hotkey(NEW_CALL_HOTKEY);
    assert!(matches!(outcome, SaveOutcome::Saved { .. }), "{outcome:?}");

    app.assert_app_holds(NEW_CALL_HOTKEY, "after the save");
    assert!(
        eventually(|| combo_is_free(CALL_HOTKEY)),
        "the old {CALL_HOTKEY} is still registered after the save"
    );
    app.assert_app_holds(DEFAULT_HOTKEY, "the dictation hotkey");
    app.press(&NEW_CALL_KEYS);
    assert!(
        eventually(|| app.recording()),
        "the saved call hotkey did not start a call"
    );
    assert_eq!(app.lines(&["call_requested", "source=hotkey"]).len(), 1);
    assert!(!app.has_hotkey_error(), "{:?}", app.indicator.trays());
}

#[test]
fn a_taken_new_call_hotkey_is_refused_and_the_old_one_keeps_working() {
    // Acceptance 3, failure branch (contracts/ipc.md: a save-time conflict on calls.hotkey
    // is hotkey.unavailable): with Ctrl+Alt+F10 taken by this thread, saving it as the call
    // hotkey is Refused [calls.hotkey: hotkey.unavailable]; settings.json is byte-for-byte
    // unchanged; the app still holds Ctrl+Alt+R and a press of it starts a call. Bite: the
    // 1409 swallowed (Saved with a dead hotkey), the old slot released before the new one
    // succeeded, the file written before the prepare, the error on recording.hotkey.
    let _serial = serial();
    assert_hotkey_free();
    let taken = TakenHotkey::take_combo(NEW_CALL_HOTKEY).expect("premise: the test takes F10");
    let app = start(AppRig::new(EngineKind::Api));
    app.assert_app_holds(CALL_HOTKEY, "premise");
    let file_before = app.settings_file();

    assert_eq!(
        app.save_call_hotkey(NEW_CALL_HOTKEY),
        call_hotkey_unavailable()
    );

    assert_eq!(app.settings_file(), file_before, "settings.json changed");
    assert_eq!(app.rig.service.snapshot().calls.hotkey, CALL_HOTKEY);
    app.assert_app_holds(CALL_HOTKEY, "after the refusal");
    app.press(&CALL_KEYS);
    assert!(
        eventually(|| app.recording()),
        "the old call hotkey stopped working after the refusal"
    );
    drop(taken);
}

#[test]
fn a_later_refusal_frees_the_prepared_call_hotkey() {
    // A save that changes the call hotkey and turns autostart on, with the autostart write
    // failing (one shot), is refused; Ctrl+Alt+F10 is free again (the prepared call-slot
    // registration was aborted on the hotkey thread) and Ctrl+Alt+R is still held. Bite:
    // abort not sent for the call slot (F10 stays registered until restart), abort freeing
    // the active slot instead of the prepared one.
    let _serial = serial();
    assert_hotkey_free();
    assert!(
        combo_is_free(NEW_CALL_HOTKEY),
        "premise: {NEW_CALL_HOTKEY} is free on the runner"
    );
    let autostart = Arc::new(FakeAutostart::new());
    autostart.fail_set(true, AutostartError { os_code: 5 });
    let app = start(AppRig::with_autostart(EngineKind::Api, autostart));
    app.assert_app_holds(CALL_HOTKEY, "premise");

    let outcome = app.save_with(|s| {
        s.calls.hotkey = NEW_CALL_HOTKEY.to_string();
        s.start_with_windows = true;
    });
    assert!(
        matches!(outcome, SaveOutcome::Refused { .. }),
        "{outcome:?}"
    );

    assert!(
        eventually(|| combo_is_free(NEW_CALL_HOTKEY)),
        "the aborted {NEW_CALL_HOTKEY} is still registered by the app"
    );
    app.assert_app_holds(CALL_HOTKEY, "after the abort");
    assert_eq!(app.rig.service.snapshot().calls.hotkey, CALL_HOTKEY);
}

// ---- (d) independence ----------------------------------------------------------------------

#[test]
fn a_dictation_hold_during_a_recording_call_leaves_the_call_recording() {
    // Acceptance 4 (T030): with a call recording (started by the call hotkey), a dictation
    // hold opens exactly one dictation capture and its release closes it; the call is still
    // recording and no call stop line was written. Bite: the dictation press routed to the
    // call (a stop), the call recording ended by a dictation start, no capture for the hold.
    let _serial = serial();
    assert_hotkey_free();
    let app = start(AppRig::new(EngineKind::Api));
    app.assert_app_holds(CALL_HOTKEY, "premise");
    app.press(&CALL_KEYS);
    assert!(eventually(|| app.recording()), "premise: the call started");

    app.assert_hold_records("the dictation hold during a recording call");

    assert!(app.recording(), "the dictation hold ended the call");
    assert_eq!(app.lines(&["call_stopped"]).len(), 0);
    assert_eq!(app.audio.start_calls(), 1, "one dictation capture");
}

#[test]
fn a_call_press_during_a_dictation_hold_leaves_the_hold_running_and_toggles_the_call() {
    // Acceptance 4 (T030; I2: a call-slot press never calls a DictationSession input, so the
    // Esc premise holds): with a dictation hold running, one press of R (Ctrl+Alt still down)
    // starts the call; the hold keeps its capture (still one open handle, no second start)
    // until it is released; a second press of R stops the call. Bite: the call press routed to
    // `hotkey_pressed` (a toggle or a second capture), a release poll started for the call
    // slot (the hold ends with the R key up), the call press cancelling the hold.
    let _serial = serial();
    assert_hotkey_free();
    let app = start(AppRig::new(EngineKind::Api));
    app.assert_app_holds(CALL_HOTKEY, "premise");

    let mut hold = Keys::press(&DEFAULT_KEYS);
    assert!(
        eventually(|| app.audio.start_calls() == 1),
        "premise: the dictation hold opened no capture"
    );

    app.press(&[VK_R]);
    assert!(
        eventually(|| app.recording()),
        "the call press during a dictation hold did not start a call"
    );
    assert!(
        holds_for(SETTLE, || app.audio.open_handles() == 1
            && app.audio.start_calls() == 1),
        "the call press disturbed the dictation hold: open {}, starts {}",
        app.audio.open_handles(),
        app.audio.start_calls()
    );
    app.press(&[VK_R]);
    assert!(
        eventually(|| !app.recording()),
        "the second call press did not stop the call"
    );
    assert_eq!(app.audio.open_handles(), 1, "the hold lost its capture");

    hold.release_all();
    assert!(
        eventually(|| app.audio.open_handles() == 0),
        "the hold's capture stayed open after its release"
    );
}

#[test]
fn dropping_the_dictation_frees_both_hotkeys() {
    // I1 (the hotkey thread is the only code that releases either hotkey; `release_all`
    // covers both slots): after the handle is dropped neither Ctrl+Alt+Space nor Ctrl+Alt+R
    // is registered; a call hotkey committed by a save is freed too. Bite: release_all
    // freeing the dictation slot only (the call slot leaks), the committed registration under
    // an id the drop does not unregister.
    let _serial = serial();
    assert_hotkey_free();
    assert!(
        combo_is_free(NEW_CALL_HOTKEY),
        "premise: {NEW_CALL_HOTKEY} is free on the runner"
    );
    let mut app = start(AppRig::new(EngineKind::Api));
    app.assert_app_holds(CALL_HOTKEY, "premise");
    let outcome = app.save_call_hotkey(NEW_CALL_HOTKEY);
    assert!(matches!(outcome, SaveOutcome::Saved { .. }), "{outcome:?}");
    app.assert_app_holds(NEW_CALL_HOTKEY, "premise: the committed one");

    drop(app.dictation.take());

    assert!(
        combo_is_free(NEW_CALL_HOTKEY),
        "{NEW_CALL_HOTKEY} is still registered after the dictation handle was dropped"
    );
    assert!(
        combo_is_free(CALL_HOTKEY),
        "{CALL_HOTKEY} is still registered after the dictation handle was dropped"
    );
    assert!(
        combo_is_free(DEFAULT_HOTKEY),
        "{DEFAULT_HOTKEY} is still registered after the dictation handle was dropped"
    );
}
