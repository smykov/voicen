//! T-055: the two-step hotkey registration of a save (spec 004 R-3: prepare before the
//! autostart / keys / file, commit after the file, abort on every later refusal) as
//! requests into the hotkey thread (`RegisterHotKey` runs only on the thread that owns
//! the registration), and the startup failure branch (tray HotkeyError, the
//! `notice.hotkey_unavailable` overlay, settings opened at Recording / hotkey field).
//! Through the real wiring: `load_settings_with` with the shell's registrar handle
//! (`voicen_lib::win::hotkey::HotkeyRegistrarHandle`), `build_app` (as `run()`),
//! `start_dictation` with `DictationPorts::hotkeys` = the same handle. Windows CI only
//! (decision #5); not compiled on the Linux gate.
//!
//! The API these tests pin (the developer may rename, the tests follow):
//! - `HotkeyRegistrarHandle::new() -> Arc<HotkeyRegistrarHandle>`, a
//!   `voicen_core::hotkey_registrar::HotkeyRegistrar`; until `start_dictation` attaches
//!   the hotkey thread its `prepare` fails (fail closed: a save of a new hotkey is then
//!   refused with `hotkey.unavailable`, never saved unregistered);
//! - `settings_ipc::load_settings_with(.., hotkeys: Arc<dyn HotkeyRegistrar>, ..)`;
//! - `DictationPorts { .., hotkeys: Arc<HotkeyRegistrarHandle> }`.
//!
//! Runner capabilities (docs/decisions/windows-ci-runner.md): `hotkey`, `sendinput`,
//! `async_keys`, asserted loudly by `win32_support`. A taken combination is taken by
//! this test thread (`TakenHotkey`, thread-associated), so the app's `RegisterHotKey`
//! fails with 1409 as against another process. Every wait is at least 3 s.
//!
//! Data: a `TempDir` per test, the fake key of `win32_support`; nothing is logged
//! beyond the app's own lines.
#![cfg(windows)]

mod win32_support;

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::Value;
use tauri::{Listener, Manager};
use voicen_core::autostart::{AutostartError, FakeAutostart};
use voicen_core::platform::{FakeAudioSource, FakeClipboard, FakeIndicator, FakePaster};
use voicen_core::recording::TrayState;
use voicen_core::secrets::KeyEdits;
use voicen_core::settings::file::SETTINGS_FILE;
use voicen_core::settings::service::{SaveOutcome, SaveRequest};
use voicen_core::settings::{EngineKind, ErrorCode, FieldError, FieldId, LoadOutcome};
use voicen_lib::dictation::{start_dictation, DictationHandle, DictationPorts};
use voicen_lib::settings_window::{self, LABEL};
use win32_support::{
    assert_hotkey_free, combo_is_free, engine_factory, eventually, serial, shown, AppRig, Keys,
    Shown, TakenHotkey, TextEngine, HOLD, VK_CONTROL, VK_MENU, WAIT,
};

/// The new hotkey the tests save (free on the runner; checked per test).
const NEW_HOTKEY: &str = "Ctrl+Alt+F9";
const VK_F9: u16 = 0x78;
const NEW_KEYS: [u16; 3] = [VK_CONTROL, VK_MENU, VK_F9];
const DEFAULT_HOTKEY: &str = "Ctrl+Alt+Space";
const DEFAULT_KEYS: [u16; 3] = win32_support::HOTKEY_KEYS;
const HOTKEY_NOTICE: &str = "notice.hotkey_unavailable";
const FOCUS: &str = "settings://focus";
/// The opener thread's budget (F-005).
const OPEN_BUDGET: Duration = Duration::from_secs(10);

/// The app plus its started dictation over fakes (audio, clipboard, paster, indicator).
struct Started {
    rig: AppRig,
    audio: Arc<FakeAudioSource>,
    indicator: Arc<FakeIndicator>,
    dictation: Option<DictationHandle>,
}

fn start(rig: AppRig) -> Started {
    let audio = Arc::new(FakeAudioSource::new());
    let indicator = Arc::new(FakeIndicator::new());
    let engine = TextEngine::new("registrar test transcript");
    let dictation = start_dictation(
        rig.app.handle(),
        DictationPorts {
            audio: audio.clone(),
            clipboard: Arc::new(FakeClipboard::new()),
            paster: Arc::new(FakePaster::new()),
            engine_factory: Some(engine_factory(&engine)),
            indicator: indicator.clone(),
            credentials: Arc::clone(&rig.creds),
            hotkeys: Arc::clone(&rig.hotkeys),
        },
    )
    .expect("start_dictation");
    Started {
        rig,
        audio,
        indicator,
        dictation: Some(dictation),
    }
}

impl Started {
    /// Waits until the app holds `combo` (this thread can no longer take it).
    #[track_caller]
    fn assert_app_holds(&self, combo: &str, why: &str) {
        assert!(
            eventually(WAIT, || !combo_is_free(combo)),
            "{why}: the app does not hold {combo}"
        );
    }

    fn save_hotkey(&self, text: &str) -> SaveOutcome {
        let mut draft = (*self.rig.service.snapshot()).clone();
        draft.hotkey = text.to_string();
        self.rig.service.save(SaveRequest {
            settings: draft,
            keys: KeyEdits::default(),
        })
    }

    /// A hold of `keys` opens one capture, and its release closes it.
    #[track_caller]
    fn assert_hold_records(&self, keys: &[u16], what: &str) {
        let before = self.audio.start_calls();
        let mut held = Keys::press(keys);
        assert!(
            eventually(WAIT, || self.audio.start_calls() == before + 1),
            "{what}: no capture within {WAIT:?} of the press (start calls {} → {})",
            before,
            self.audio.start_calls()
        );
        thread::sleep(HOLD);
        held.release_all();
        assert!(
            eventually(WAIT, || self.audio.open_handles() == 0),
            "{what}: the capture stayed open after the release"
        );
    }

    fn settings_file(&self) -> Option<String> {
        std::fs::read_to_string(self.rig.dir.path().join(SETTINGS_FILE)).ok()
    }
}

fn hotkey_unavailable() -> SaveOutcome {
    SaveOutcome::Refused {
        errors: vec![FieldError {
            field: FieldId::RecordingHotkey,
            code: ErrorCode::HotkeyUnavailable,
        }],
        form_error: None,
    }
}

#[test]
fn a_saved_new_hotkey_is_registered_and_the_old_one_freed() {
    // Acceptance "a changed hotkey takes effect on save, without restart" (FR-005,
    // spec 004 R-3 commit): saving Ctrl+Alt+F9 is Saved; afterwards the app holds
    // Ctrl+Alt+F9, Ctrl+Alt+Space is free, and a Ctrl+Alt+F9 hold opens one capture
    // that its release closes (the new combination reaches the session; mode unchanged).
    // Bite: the registrar handle not wired into the hotkey thread (prepare fails → a
    // refusal, or a no-op → the old combination still registered), commit not freeing
    // the old registration, WM_HOTKEY still matched against the old id only.
    let _serial = serial();
    assert_hotkey_free();
    assert!(
        combo_is_free(NEW_HOTKEY),
        "premise: {NEW_HOTKEY} is free on the runner"
    );
    let app = start(AppRig::new(EngineKind::Api));
    app.assert_app_holds(DEFAULT_HOTKEY, "premise");

    let outcome = app.save_hotkey(NEW_HOTKEY);
    assert!(matches!(outcome, SaveOutcome::Saved { .. }), "{outcome:?}");

    app.assert_app_holds(NEW_HOTKEY, "after the save");
    assert!(
        eventually(WAIT, || combo_is_free(DEFAULT_HOTKEY)),
        "the old {DEFAULT_HOTKEY} is still registered after the save"
    );
    app.assert_hold_records(&NEW_KEYS, "the saved hotkey");
    assert!(
        !app.indicator
            .trays()
            .iter()
            .any(|(s, _)| *s == TrayState::HotkeyError),
        "trays: {:?}",
        app.indicator.trays()
    );
}

#[test]
fn a_taken_new_hotkey_is_refused_and_the_old_one_keeps_working() {
    // Acceptance "a taken hotkey on save → field error hotkey.unavailable, nothing saved,
    // the old hotkey stays" (FR-005, spec 004 R-3 prepare fails): with Ctrl+Alt+F9 taken
    // by this thread, saving it is Refused [recording.hotkey: hotkey.unavailable];
    // settings.json is byte-for-byte unchanged; the app still holds Ctrl+Alt+Space and
    // a hold of it still records. Bite: the 1409 swallowed (Saved with a dead hotkey),
    // the old registration dropped before the new one succeeded (unregister-then-register),
    // the file written before the prepare.
    let _serial = serial();
    assert_hotkey_free();
    let taken = TakenHotkey::take_combo(NEW_HOTKEY).expect("premise: the test takes F9");
    let app = start(AppRig::new(EngineKind::Api));
    app.assert_app_holds(DEFAULT_HOTKEY, "premise");
    let file_before = app.settings_file();

    assert_eq!(app.save_hotkey(NEW_HOTKEY), hotkey_unavailable());

    assert_eq!(app.settings_file(), file_before, "settings.json changed");
    assert_eq!(app.rig.service.snapshot().hotkey, DEFAULT_HOTKEY);
    app.assert_app_holds(DEFAULT_HOTKEY, "after the refusal");
    app.assert_hold_records(&DEFAULT_KEYS, "the old hotkey after the refusal");
    drop(taken);
}

#[test]
fn a_later_refusal_frees_the_prepared_hotkey() {
    // Acceptance "nothing changes on a refused save" (spec 004 R-3 undo: abort after
    // the autostart undo): a save that changes the hotkey and turns autostart on, with
    // the autostart write failing, is refused; Ctrl+Alt+F9 is free again (the prepared
    // registration was aborted on the hotkey thread) and Ctrl+Alt+Space is still held.
    // Bite: abort not sent (F9 stays registered by the app until restart), abort freeing
    // the old registration instead of the prepared one.
    let _serial = serial();
    assert_hotkey_free();
    assert!(
        combo_is_free(NEW_HOTKEY),
        "premise: {NEW_HOTKEY} is free on the runner"
    );
    let autostart = Arc::new(FakeAutostart::new());
    autostart.fail_set(true, AutostartError { os_code: 5 });
    let app = start(AppRig::with_autostart(EngineKind::Api, autostart));
    app.assert_app_holds(DEFAULT_HOTKEY, "premise");

    let mut draft = (*app.rig.service.snapshot()).clone();
    draft.hotkey = NEW_HOTKEY.to_string();
    draft.start_with_windows = true;
    let outcome = app.rig.service.save(SaveRequest {
        settings: draft,
        keys: KeyEdits::default(),
    });
    assert!(
        matches!(outcome, SaveOutcome::Refused { .. }),
        "{outcome:?}"
    );

    assert!(
        eventually(WAIT, || combo_is_free(NEW_HOTKEY)),
        "the aborted {NEW_HOTKEY} is still registered by the app"
    );
    app.assert_app_holds(DEFAULT_HOTKEY, "after the abort");
    assert_eq!(app.rig.service.snapshot().hotkey, DEFAULT_HOTKEY);
}

#[test]
fn a_taken_hotkey_at_startup_reports_and_opens_the_hotkey_field_and_a_save_recovers() {
    // Acceptance "startup: hotkey taken → tray HotkeyError, a notice, settings opened at
    // the hotkey field" (FR-005, FR-028; T-055 Q1: the hotkey field wins over the
    // first-run Engine tab) and "saving a free hotkey clears the error":
    // - with Ctrl+Alt+Space taken before start, the tray goes HotkeyError, the overlay
    //   shows `notice.hotkey_unavailable`, and one `settings` window opens at
    //   `settings?tab=recording&field=recording.hotkey`;
    // - the startup executor's FirstRun request afterwards does not move it to Engine
    //   (no window URL change, no `settings://focus` with tab engine);
    // - saving the free Ctrl+Alt+F9 is Saved and the tray leaves HotkeyError (Idle).
    // Bite: the failure only on the tray (no notice, no window), the window at Engine or
    // without the field, on_ready's Engine request winning, a save that registers but
    // never reports `hotkey_registration(true)`.
    let _serial = serial();
    assert_hotkey_free();
    assert!(
        combo_is_free(NEW_HOTKEY),
        "premise: {NEW_HOTKEY} is free on the runner"
    );
    let taken = TakenHotkey::take().expect("premise: the test takes Ctrl+Alt+Space");
    let rig = AppRig::new(EngineKind::None);
    let focus = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let sink = focus.clone();
    rig.app.listen_any(FOCUS, move |event| {
        let payload = serde_json::from_str(event.payload()).expect("focus payload is JSON");
        sink.lock().expect("lock").push(payload);
    });
    let app = start(rig);

    assert!(
        eventually(WAIT, || app
            .indicator
            .trays()
            .iter()
            .any(|(s, _)| *s == TrayState::HotkeyError)),
        "no HotkeyError: {:?}",
        app.indicator.trays()
    );
    assert!(
        eventually(WAIT, || shown(&app.indicator.overlays()).iter().any(
            |o| matches!(o, Shown::Message(id, _) if *id == HOTKEY_NOTICE)
        )),
        "no {HOTKEY_NOTICE} overlay: {:?}",
        shown(&app.indicator.overlays())
    );
    let hotkey_query = vec![
        ("tab".to_string(), "recording".to_string()),
        ("field".to_string(), "recording.hotkey".to_string()),
    ];
    let query = || {
        app.rig.app.get_webview_window(LABEL).map(|w| {
            w.url()
                .expect("window url")
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect::<Vec<_>>()
        })
    };
    assert!(
        eventually(OPEN_BUDGET, || query().as_ref() == Some(&hotkey_query)),
        "the settings window is not at the hotkey field: {:?}",
        query()
    );

    // Q1: the startup executor runs on RunEvent::Ready, after the hotkey thread's report.
    let outcome = LoadOutcome::FirstRun((*app.rig.service.snapshot()).clone());
    if let Some(receipt) = settings_window::on_ready(app.rig.app.handle(), &outcome, false) {
        let _ = receipt.wait_timeout(OPEN_BUDGET);
    }
    assert_eq!(query().as_ref(), Some(&hotkey_query), "Q1: the URL moved");
    let engine_focus: Vec<Value> = focus
        .lock()
        .expect("lock")
        .iter()
        .filter(|p| p.get("tab").and_then(Value::as_str) == Some("engine"))
        .cloned()
        .collect();
    assert!(
        engine_focus.is_empty(),
        "Q1: the first-run Engine tab took the focus from the hotkey field: {engine_focus:?}"
    );

    let saved = app.save_hotkey(NEW_HOTKEY);
    assert!(matches!(saved, SaveOutcome::Saved { .. }), "{saved:?}");
    app.assert_app_holds(NEW_HOTKEY, "after the recovering save");
    assert!(
        eventually(WAIT, || app.indicator.trays().last().map(|(s, _)| *s)
            == Some(TrayState::Idle)),
        "the tray did not leave HotkeyError after the save: {:?}",
        app.indicator.trays()
    );
    drop(taken);
}

#[test]
fn dropping_the_dictation_frees_a_hotkey_committed_by_a_save() {
    // T-006 row 7 carried to T-055: the handle owns the registration made by a commit
    // too; after the drop Ctrl+Alt+F9 is free. Bite: the committed registration made on
    // another thread or under an id the drop does not unregister.
    let _serial = serial();
    assert_hotkey_free();
    assert!(
        combo_is_free(NEW_HOTKEY),
        "premise: {NEW_HOTKEY} is free on the runner"
    );
    let mut app = start(AppRig::new(EngineKind::Api));
    let outcome = app.save_hotkey(NEW_HOTKEY);
    assert!(matches!(outcome, SaveOutcome::Saved { .. }), "{outcome:?}");
    app.assert_app_holds(NEW_HOTKEY, "premise");

    drop(app.dictation.take());

    assert!(
        combo_is_free(NEW_HOTKEY),
        "{NEW_HOTKEY} is still registered after the dictation handle was dropped"
    );
}
