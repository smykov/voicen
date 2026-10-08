//! T-052: the tray icon (spec 001 FR-001, FR-008 / FR-028; spec 004 FR-013;
//! T-052 invariants 3 and 5) over the shell's app wiring (`build_app`) through
//! Tauri's mock runtime, with the real core `DictationSession` over the core
//! fakes. Windows CI only (decision #5).
//!
//! What the tray shows is read from `voicen_lib::tray::applied` (what the last
//! main-thread apply task set on the icon: state, icon kind, tooltip text, menu
//! items read back from the menu) and from tauri's own `tray_by_id`. The expected
//! icons, ids and texts come from core `voicen_core::tray` and the embedded
//! catalogs, which core's tests pin against contracts/messages.md.
//!
//! Threads in the mock: `build_app` builds the tray on the test thread, which is
//! the mock's main thread, so tray-icon's window belongs to this thread and its
//! setters `SendMessageW` to it. The mock runs a main-thread task inline on
//! whichever thread posts it while the loop is not running (mock_runtime.rs:84-96),
//! so an apply posted from another thread (the session's menu-open thread, the
//! language follower) waits until this thread dispatches messages: every wait here
//! pumps this thread's queue ([`eventually`]). The real runtime's threading is
//! `lifecycle.rs`'s.
//!
//! Each test has its own `TempDir`; the runner's real `%LOCALAPPDATA%\Voicen` is
//! never written. No keys are used; the local server URL is a loopback fake.
//!
//! T-071 (Open logs folder): the click path is `on_menu_event` ->
//! `voicen_lib::logs_folder::request`, over a fake `FolderOpener` installed with
//! `logs_folder::install` on a `TempDir` path, so no Explorer window opens on the
//! runner (F-006). The Win32 `ShellOpener` is exercised only on its failure branch
//! (a missing path).
#![cfg(windows)]

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use tauri::menu::{MenuEvent, MenuId};
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconEvent, TrayIconId};
use tauri::{App, AppHandle, Context, Manager, PhysicalPosition, Rect, Runtime};
use voicen_core::autostart::{Autostart, FakeAutostart};
use voicen_core::diag::Log;
use voicen_core::dictation::{DictationSession, SessionDeps};
use voicen_core::events::RecordingObserver;
use voicen_core::i18n::{self, UiLanguage};
use voicen_core::local_models::catalog::MODELS;
use voicen_core::local_models::service::LocalModels;
use voicen_core::local_models::store::ModelStore;
use voicen_core::pipeline::PipelineDeps;
use voicen_core::platform::{
    AudioSource, CaptureHandle, FakeAudioSource, FakeCancelKey, FakeClipboard, FakePaster,
    FakeShellRequests, FakeTempAudioStore, FrameSink, Indicator,
};
use voicen_core::post_process::PassThrough;
use voicen_core::recording::{CaptureError, OverlayState, TrayState};
use voicen_core::secrets::{CredentialStore, FakeCredentialStore, KeyEdits};
use voicen_core::settings::service::{SaveOutcome, SaveRequest, SettingsService};
use voicen_core::settings::EngineKind;
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_core::tray::{self as table, TrayAction};
use voicen_core::vad::{EnergyDetector, SpeechDetector, SpeechGate};
use voicen_lib::logs_folder::{self, FolderOpener};
use voicen_lib::settings_ipc::load_settings;
use voicen_lib::settings_window::{self, OpenTarget, LABEL};
use voicen_lib::tray::{self, Applied, TrayPart, TRAY_ID};
use voicen_lib::win::shell_open::ShellOpener;
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::{Shell_NotifyIconW, NIM_DELETE, NOTIFYICONDATAW};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EnumThreadWindows, GetClassNameW, PeekMessageW, TranslateMessage, MSG,
    PM_REMOVE,
};

/// How long a test waits for an asynchronous change (Windows-sized, F-005).
const BUDGET: Duration = Duration::from_secs(10);

/// How long a bounded negative watches for a change that must not come.
const SETTLE: Duration = Duration::from_millis(500);

/// A handler that hands its work to another thread returns well within this.
const RETURNS_AT_ONCE: Duration = Duration::from_secs(2);

/// A held fake gives up after this, so a failing test cannot hang the exe.
const HOLD_LIMIT: Duration = Duration::from_secs(30);

/// Each binary serialises its tests: the tray icons and their windows are per
/// session (T-052 analysis, red-test notes).
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Dispatches every message queued for this thread, sent messages included.
fn pump() {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid, writable MSG for the whole loop; the calls only
    // read and dispatch this thread's own queue.
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Polls `cond` every 5 ms, pumping this thread's messages, until it holds or
/// `BUDGET` has passed.
fn eventually(mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + BUDGET;
    loop {
        pump();
        if cond() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// Pumps for `SETTLE`; false as soon as `cond` fails (a bounded negative: it can
/// miss a change slower than that, never report one that did not happen).
fn holds_for_a_while(mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + SETTLE;
    while Instant::now() < deadline {
        pump();
        if !cond() {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
    pump();
    cond()
}

fn no_keys() -> Arc<dyn CredentialStore> {
    Arc::new(FakeCredentialStore::new())
}

fn no_autostart() -> Arc<dyn Autostart> {
    Arc::new(FakeAutostart::new())
}

fn idle_store(data_dir: &Path) -> Arc<ModelStore> {
    Arc::new(ModelStore::new(data_dir.join("models"), MODELS))
}

fn idle_models() -> Arc<LocalModels> {
    let (models, cleanup) = LocalModels::open(
        TempDir::new().path().join("models"),
        FakeDisk::with_available(u64::MAX),
        Timeouts::default(),
        MODELS,
    );
    cleanup.expect("cleanup over a missing models dir");
    Arc::new(models)
}

/// A degraded log under this test exe (a file), so it never writes anything.
fn discard_log() -> Arc<Log> {
    let exe = std::env::current_exe().expect("current_exe");
    voicen_lib::diag::start(exe.join("logs"), Box::new(|_| {}))
}

struct Rig {
    _dir: TempDir,
    app: App<MockRuntime>,
    service: Arc<SettingsService>,
}

impl Rig {
    fn handle(&self) -> &AppHandle<MockRuntime> {
        self.app.handle()
    }

    fn applied(&self) -> Option<Applied> {
        tray::applied(self.handle())
    }

    fn state(&self) -> Option<TrayState> {
        self.applied().map(|a| a.state)
    }

    fn tray_icon(&self) -> TrayIcon<MockRuntime> {
        self.app
            .tray_by_id(TRAY_ID)
            .unwrap_or_else(|| panic!("no tray icon `{TRAY_ID}`"))
    }

    fn part(&self) -> TrayPart<MockRuntime> {
        tray::part(self.handle()).expect("the tray part of a built tray")
    }

    fn labels(&self) -> Vec<String> {
        let set: BTreeSet<String> = self.app.webview_windows().into_keys().collect();
        set.into_iter().collect()
    }

    /// `edit` applied to the current settings, saved through the real service.
    fn save(&self, edit: impl FnOnce(&mut voicen_core::settings::Settings)) {
        let mut s = (*self.service.snapshot()).clone();
        edit(&mut s);
        match self.service.save(SaveRequest {
            settings: s,
            keys: KeyEdits::default(),
        }) {
            SaveOutcome::Saved { .. } => {}
            other => panic!("premise: the save is refused: {other:?}"),
        }
    }
}

/// The app as `run()` builds it (`build_app`) over a fresh data dir whose first run
/// resolves the UI language from `os_language`.
fn rig(os_language: Option<&str>) -> Rig {
    rig_with_log(os_language, discard_log())
}

/// [`rig`] writing its log to `log` (T-006: a test that reads the tray's warnings).
fn rig_with_log(os_language: Option<&str>, log: Arc<Log>) -> Rig {
    let dir = TempDir::new();
    let (service, _) = load_settings(
        dir.path().to_path_buf(),
        no_keys(),
        no_autostart(),
        idle_store(dir.path()),
        os_language,
        &log,
    );
    let app = voicen_lib::build_app(
        mock_builder(),
        mock_context(noop_assets()),
        service.clone(),
        idle_models(),
        log,
    )
    .expect("mock app builds");
    Rig {
        _dir: dir,
        app,
        service,
    }
}

/// What the tray shows for `(state, retry_available)` in `lang`, by core's table.
fn expected(state: TrayState, retry_available: bool, lang: UiLanguage) -> Applied {
    Applied {
        state,
        retry_available,
        lang,
        icon: table::icon(state),
        tooltip: i18n::text(lang, table::tooltip(state), &[]),
        menu: table::menu(retry_available)
            .into_iter()
            .map(|a| (a.as_str().to_string(), i18n::text(lang, a.label(), &[])))
            .collect(),
    }
}

/// Every tray state, by a wildcard-free match.
fn all_states() -> [TrayState; 4] {
    let all = [
        TrayState::Idle,
        TrayState::Recording,
        TrayState::Error,
        TrayState::HotkeyError,
    ];
    for state in all {
        match state {
            TrayState::Idle | TrayState::Recording | TrayState::Error | TrayState::HotkeyError => {}
        }
    }
    all
}

fn click(button: MouseButton, button_state: MouseButtonState) -> TrayIconEvent {
    TrayIconEvent::Click {
        id: TrayIconId::new(TRAY_ID),
        position: PhysicalPosition::new(0.0, 0.0),
        rect: Rect::default(),
        button,
        button_state,
    }
}

fn menu_item(action: TrayAction) -> MenuEvent {
    MenuEvent {
        id: MenuId::new(action.as_str()),
    }
}

// ---- the session, over fakes, behind a test-local composite Indicator ----------

/// T-006's composite `Indicator` in miniature: the tray half forwards to the shell's
/// `TrayPart`, the overlay half is T-057's and does nothing here.
struct TrayOnly(TrayPart<MockRuntime>);

impl Indicator for TrayOnly {
    fn set_tray(&self, state: TrayState, retry_available: bool) {
        self.0.set_tray(state, retry_available);
    }

    fn set_overlay(&self, _state: &OverlayState) {}
}

/// Saves engine `local_server` (loopback fake URL; no key needed), so a press
/// passes the dictation gate and opens the microphone.
fn select_local_server(rig: &Rig) {
    rig.save(|s| {
        s.engine = EngineKind::LocalServer;
        s.local_server.base_url = "http://127.0.0.1:8080/v1".to_string();
    });
}

/// The core session over fakes, its tray through `rig`'s `TrayPart`, managed as
/// `Arc<DictationSession>` (the type the tray's menu-open handler looks up).
fn start_session(rig: &Rig, audio: Arc<dyn AudioSource>) -> Arc<DictationSession> {
    let session = DictationSession::start(SessionDeps {
        pipeline: PipelineDeps {
            gate: SpeechGate::new(
                Ok(Box::new(EnergyDetector::new()) as Box<dyn SpeechDetector>),
                EnergyDetector::new(),
            ),
            credentials: no_keys(),
            clipboard: Arc::new(FakeClipboard::new()),
            paster: Arc::new(FakePaster::new()),
            temp_audio: Arc::new(FakeTempAudioStore::new()),
            observer: Arc::new(RecordingObserver::new()),
            post_processor: Arc::new(PassThrough),
        },
        engine_factory: None,
        audio,
        indicator: Arc::new(TrayOnly(rig.part())),
        requests: Arc::new(FakeShellRequests::new()),
        settings: Arc::clone(&rig.service),
        cancel_key: Arc::new(FakeCancelKey::new()),
    })
    .expect("the session starts");
    let session = Arc::new(session);
    rig.app.manage(Arc::clone(&session));
    session
}

/// A microphone whose `start` is held until released (or `HOLD_LIMIT`), then
/// refuses with `AccessDenied`. The session holds its lock across `start`.
#[derive(Default)]
struct HeldMic {
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}

impl HeldMic {
    fn wait_entered(&self) -> bool {
        let st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let (st, _) = self
            .changed
            .wait_timeout_while(st, BUDGET, |s| !s.0)
            .unwrap_or_else(PoisonError::into_inner);
        st.0
    }

    fn release(&self) {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).1 = true;
        self.changed.notify_all();
    }
}

impl AudioSource for HeldMic {
    fn start(&self, _sink: Arc<dyn FrameSink>) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        st.0 = true;
        self.changed.notify_all();
        let _released = self
            .changed
            .wait_timeout_while(st, HOLD_LIMIT, |s| !s.1)
            .unwrap_or_else(PoisonError::into_inner);
        Err(CaptureError::AccessDenied)
    }
}

/// Releases the mic when the test ends, also by a failed assertion.
struct ReleaseOnDrop(Arc<HeldMic>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

// ---- tests ---------------------------------------------------------------------

#[test]
fn the_tray_exists_after_build_idle_in_the_settings_language() {
    // T-052 red-test table row 8 (spec 001 FR-001): `build_app` (and so `run()`)
    // builds the one tray icon after tauri's `build()`; it starts Idle, with no
    // Retry, its tooltip and menu in the settings' UI language (the session
    // publishes nothing at start, dictation-session.md). Bite: no tray, a tray that
    // starts in another state, or texts in a hard-coded language.
    let _serial = serial();
    for (os, lang) in [(None, UiLanguage::En), (Some("ru-RU"), UiLanguage::Ru)] {
        let rig = rig(os);
        assert_eq!(rig.service.snapshot().ui_language, lang, "premise");
        assert!(
            rig.app.tray_by_id(TRAY_ID).is_some(),
            "{lang:?}: no tray icon `{TRAY_ID}` after build_app"
        );
        assert_eq!(
            rig.applied(),
            Some(expected(TrayState::Idle, false, lang)),
            "{lang:?}"
        );
        // T-057 (T-052 validation F2): Shell_NotifyIconGetRect is Some only when the
        // shell knows the icon. The runner has a notification area (fact `taskbar`,
        // ok in runs A and B, docs/decisions/windows-ci-runner.md), so a None here is
        // an icon the shell never got. Bite: a tray built but never added to the
        // notification area.
        let rect = rig.tray_icon().rect();
        assert!(
            matches!(rect, Ok(Some(_))),
            "{lang:?}: the shell knows no rect for the tray icon (Shell_NotifyIconGetRect); \
             runner fact `taskbar` is ok in runs A and B (docs/decisions/windows-ci-runner.md), \
             so either the icon was not added or the runner image changed (then this \
             Acceptance moves to the owner): {rect:?}"
        );
    }
}

#[test]
fn each_state_and_retry_flag_reaches_the_tray_with_its_icon_tooltip_and_menu() {
    // T-052 invariant 3 and 5: `TrayPart::set_tray` (the session's Indicator call)
    // gets every (TrayState, retry_available) onto the icon, with core's icon,
    // tooltip and menu. Bite: a state not applied, the wrong icon or tooltip for a
    // state (Error shown as Idle), retry_available dropped, or the menu not
    // following core's `menu(retry_available)`.
    let _serial = serial();
    let rig = rig(None);
    let part = rig.part();
    for retry in [false, true] {
        for state in all_states() {
            part.set_tray(state, retry);
            let want = expected(state, retry, UiLanguage::En);
            assert!(
                eventually(|| rig.applied() == Some(want.clone())),
                "after set_tray({state:?}, {retry}): applied {:?}",
                rig.applied()
            );
        }
    }
}

#[test]
fn opening_the_menu_clears_error_through_the_session_and_keeps_hotkey_error() {
    // T-052 red-test table row 9 (FR-008 / FR-028): the tray shows what the
    // session publishes. A press whose capture is refused shows Error; opening the
    // menu (right click, button up) goes to `tray_menu_opened` and the tray turns
    // Idle; a failed hotkey registration shows HotkeyError, which opening the menu
    // does not clear (only a successful registration does). Bite: the menu-open
    // event not routed to the session, or the tray keeping a state of its own
    // (clearing on click whatever the session says).
    let _serial = serial();
    let rig = rig(None);
    select_local_server(&rig);
    let audio = Arc::new(FakeAudioSource::new());
    let session = start_session(&rig, audio.clone());

    audio.set_start_error(Some(CaptureError::AccessDenied));
    session.hotkey_pressed(Instant::now());
    assert!(
        eventually(|| rig.state() == Some(TrayState::Error)),
        "a refused capture: tray {:?}",
        rig.state()
    );

    tray::on_tray_icon_event(
        &rig.tray_icon(),
        click(MouseButton::Right, MouseButtonState::Up),
    );
    assert!(
        eventually(|| rig.state() == Some(TrayState::Idle)),
        "the menu opened over Error: tray {:?}",
        rig.state()
    );

    session.hotkey_registration(false, Instant::now());
    assert!(
        eventually(|| rig.state() == Some(TrayState::HotkeyError)),
        "a failed registration: tray {:?}",
        rig.state()
    );
    tray::on_tray_icon_event(
        &rig.tray_icon(),
        click(MouseButton::Right, MouseButtonState::Up),
    );
    assert!(
        holds_for_a_while(|| rig.state() == Some(TrayState::HotkeyError)),
        "opening the menu cleared HotkeyError: tray {:?}",
        rig.state()
    );
    session.hotkey_registration(true, Instant::now());
    assert!(
        eventually(|| rig.state() == Some(TrayState::Idle)),
        "a successful registration: tray {:?}",
        rig.state()
    );
}

#[test]
fn a_left_click_opens_the_menu_and_clears_error() {
    // OQ-11 Q3 default: a left click shows the same menu as a right click (tauri's
    // default `show_menu_on_left_click`), so it counts as opening the menu. Bite:
    // only right clicks routed, or the builder and the handler disagreeing on the
    // left-click flag.
    let _serial = serial();
    let rig = rig(None);
    select_local_server(&rig);
    let audio = Arc::new(FakeAudioSource::new());
    let session = start_session(&rig, audio.clone());
    audio.set_start_error(Some(CaptureError::AccessDenied));
    session.hotkey_pressed(Instant::now());
    assert!(
        eventually(|| rig.state() == Some(TrayState::Error)),
        "premise: Error; tray {:?}",
        rig.state()
    );

    tray::on_tray_icon_event(
        &rig.tray_icon(),
        click(MouseButton::Left, MouseButtonState::Up),
    );

    assert!(
        eventually(|| rig.state() == Some(TrayState::Idle)),
        "a left click over Error: tray {:?}",
        rig.state()
    );
}

#[test]
fn hovering_over_the_icon_keeps_the_error() {
    // FR-028: opening the menu clears Error; moving the mouse over the icon does
    // not. Bite: every tray event treated as "menu opened".
    let _serial = serial();
    let rig = rig(None);
    select_local_server(&rig);
    let audio = Arc::new(FakeAudioSource::new());
    let session = start_session(&rig, audio.clone());
    audio.set_start_error(Some(CaptureError::AccessDenied));
    session.hotkey_pressed(Instant::now());
    assert!(
        eventually(|| rig.state() == Some(TrayState::Error)),
        "premise: Error; tray {:?}",
        rig.state()
    );

    let id = TrayIconId::new(TRAY_ID);
    let at = PhysicalPosition::new(0.0, 0.0);
    let icon = rig.tray_icon();
    for event in [
        TrayIconEvent::Enter {
            id: id.clone(),
            position: at,
            rect: Rect::default(),
        },
        TrayIconEvent::Move {
            id: id.clone(),
            position: at,
            rect: Rect::default(),
        },
        TrayIconEvent::Leave {
            id,
            position: at,
            rect: Rect::default(),
        },
    ] {
        tray::on_tray_icon_event(&icon, event);
    }

    assert!(
        holds_for_a_while(|| rig.state() == Some(TrayState::Error)),
        "hovering cleared Error: tray {:?}",
        rig.state()
    );
}

#[test]
fn opening_the_menu_never_waits_for_the_session() {
    // T-052 design 3: the tray handler runs on the main thread, which must never
    // take the session lock (a press holds it across `AudioSource::start`, slow
    // with cpal). With a press held inside `start`, the menu-open handler returns at
    // once; after the release the press shows Error and the handed-over
    // `tray_menu_opened` then clears it. The handler is called from a helper thread
    // here so that a failing implementation (waiting for the lock) cannot deadlock
    // this thread's message pump. The press is waited on until it has ended, so
    // it has published Error under the lock before the handed-over call can take
    // the lock; only then is Idle asserted (the tray starts Idle, so an earlier
    // check would pass on the initial state; review round 1 #2). Bite:
    // `tray_menu_opened` called inline in the handler, or the hand-over dropped
    // while the session is busy (`try_lock`, no spawn).
    let _serial = serial();
    let rig = rig(None);
    select_local_server(&rig);
    let mic = Arc::new(HeldMic::default());
    let _release = ReleaseOnDrop(Arc::clone(&mic));
    let session = start_session(&rig, mic.clone());

    let presser = Arc::clone(&session);
    let press = thread::spawn(move || presser.hotkey_pressed(Instant::now()));
    assert!(mic.wait_entered(), "premise: the press reached start");

    let (tx, rx) = mpsc::channel();
    let icon = rig.tray_icon();
    thread::spawn(move || {
        tray::on_tray_icon_event(&icon, click(MouseButton::Right, MouseButtonState::Up));
        let _ = tx.send(());
    });
    let returned = rx.recv_timeout(RETURNS_AT_ONCE).is_ok();

    mic.release();
    assert!(
        returned,
        "the menu-open handler waited for the session lock held by a press"
    );
    // The press renders Error through this thread's tray window, so wait pumping.
    assert!(
        eventually(|| press.is_finished()),
        "premise: the press did not end after the release"
    );
    assert!(
        eventually(|| rig.state() == Some(TrayState::Idle)),
        "the handed-over menu open did not reach the session after the press: tray {:?}",
        rig.state()
    );
}

#[test]
fn a_ui_language_save_rebuilds_the_menu_and_the_tooltip() {
    // T-052 red-test table row 10 (spec 004 FR-013, decision #34): the tray follows
    // a saved UI language through its own `SettingsService::subscribe()`. Bite: no
    // follower (the menu stays English until restart), or a follower that changes
    // the language but not the menu or the tooltip.
    let _serial = serial();
    let rig = rig(None);
    assert_eq!(
        rig.applied(),
        Some(expected(TrayState::Idle, false, UiLanguage::En)),
        "premise"
    );

    rig.save(|s| s.ui_language = UiLanguage::Ru);

    assert!(
        eventually(|| rig.applied() == Some(expected(TrayState::Idle, false, UiLanguage::Ru))),
        "after saving ui_language = ru: applied {:?}",
        rig.applied()
    );
}

#[test]
fn the_settings_item_opens_one_window_and_then_fronts_it() {
    // T-052 red-test table row 11 (spec 001 FR-001; OQ-11 Q2 default): "Settings"
    // posts to the opener: with no window it opens one on Engine; again, it fronts
    // that window (still one, URL unchanged, no `settings://focus`). The second
    // click is waited on with a later request (the opener is a FIFO). Bite: the
    // item id mapped to another action, the item opening outside the opener, a
    // second window, or a tab switch.
    let _serial = serial();
    let rig = rig(None);
    let focus = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&focus);
    tauri::Listener::listen_any(&rig.app, settings_window::FOCUS_EVENT, move |event| {
        sink.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event.payload().to_string());
    });

    tray::on_menu_event(rig.handle(), menu_item(TrayAction::OpenSettings));
    assert!(
        eventually(|| rig.labels() == vec![LABEL.to_string()]),
        "the Settings item: windows {:?}",
        rig.labels()
    );
    let url = rig
        .app
        .get_webview_window(LABEL)
        .expect("the settings window")
        .url()
        .expect("url");
    assert_eq!(url.query(), Some("tab=engine"), "url {url}");

    tray::on_menu_event(rig.handle(), menu_item(TrayAction::OpenSettings));
    settings_window::request(rig.handle(), OpenTarget::Front)
        .wait_timeout(BUDGET)
        .expect("the opener ran the later request")
        .expect("front");

    assert_eq!(rig.labels(), vec![LABEL.to_string()], "windows");
    assert_eq!(
        rig.app
            .get_webview_window(LABEL)
            .expect("the settings window")
            .url()
            .expect("url"),
        url,
        "url changed"
    );
    assert_eq!(
        focus.lock().unwrap_or_else(PoisonError::into_inner).clone(),
        Vec::<String>::new(),
        "the Settings item switched the tab"
    );
}

#[test]
fn an_unknown_menu_id_does_nothing() {
    // Only core's ids are mapped (`TrayAction::from_id`); another task's item or a
    // stray event does nothing. Bite: a catch-all arm (opening settings, opening the
    // logs folder, or exiting: the mock's `request_exit` is unimplemented and would
    // panic here). T-071: `open_logs` is an item now, so it left this list; an
    // installed opener must still see no call for any of these ids.
    let _serial = serial();
    let rig = rig(None);
    let dir = TempDir::new();
    let opener = Arc::new(FakeOpener::default());
    logs_folder::install(rig.handle(), dir.path().join("logs"), opener.clone());

    for id in ["retry", "", "Exit", "OPEN_LOGS", "open_logs_folder", "logs"] {
        tray::on_menu_event(
            rig.handle(),
            MenuEvent {
                id: MenuId::new(id),
            },
        );
    }

    assert!(
        holds_for_a_while(|| rig.labels().is_empty()),
        "an unknown id opened a window: {:?}",
        rig.labels()
    );
    assert_eq!(
        rig.applied(),
        Some(expected(TrayState::Idle, false, UiLanguage::En)),
        "an unknown id changed the tray"
    );
    assert!(
        opener.calls().is_empty(),
        "an unknown id opened the logs folder: {:?}",
        opener.calls()
    );
}

// ---- T-006: the applied record follows only setters that succeeded --------------

/// Collects the `tray_icon_app` windows (tray-icon's window class) of the enumerated
/// thread into the `Vec<isize>` behind `lparam`.
unsafe extern "system" fn collect_tray_windows(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let mut buf = [0u16; 64];
    // SAFETY: `buf` is writable; GetClassNameW sends no message.
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    let class = String::from_utf16_lossy(&buf[..usize::try_from(len).unwrap_or(0).min(buf.len())]);
    if class == "tray_icon_app" {
        // SAFETY: `lparam` is the `&mut Vec<isize>` of `remove_tray_icons`, alive for the
        // whole synchronous enumeration.
        unsafe { (*(lparam.0 as *mut Vec<isize>)).push(hwnd.0 as isize) };
    }
    BOOL(1)
}

/// Deletes from the notification area every icon of this thread's tray-icon windows
/// (`Shell_NotifyIconW(NIM_DELETE)` for each id tray-icon may have given: its counter is
/// per process and small). tray-icon's own window and data stay; only the shell forgets
/// the icon, so a later `NIM_MODIFY` (`set_icon`, `set_tooltip`) fails. Returns how many
/// icons the shell removed.
fn remove_tray_icons() -> usize {
    let mut windows: Vec<isize> = Vec::new();
    // SAFETY: the callback only reads class names and pushes into `windows`, which
    // outlives this synchronous call.
    let _ = unsafe {
        EnumThreadWindows(
            GetCurrentThreadId(),
            Some(collect_tray_windows),
            LPARAM(&mut windows as *mut Vec<isize> as isize),
        )
    };
    let mut removed = 0;
    for hwnd in windows {
        for id in 0..=256u32 {
            let nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: HWND(hwnd as *mut std::ffi::c_void),
                uID: id,
                ..Default::default()
            };
            // SAFETY: a valid NOTIFYICONDATAW naming one icon of a window of this thread.
            if unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) }.as_bool() {
                removed += 1;
            }
        }
    }
    removed
}

/// The `warning kind=tray_failed` lines of `<logs>/voicen.log`.
fn tray_failed_lines(logs: &Path) -> usize {
    std::fs::read_to_string(logs.join("voicen.log"))
        .map(|t| {
            t.lines()
                .filter(|l| l.split_whitespace().any(|w| w == "kind=tray_failed"))
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn a_failed_icon_and_tooltip_setter_leaves_the_applied_icon_and_tooltip_unchanged() {
    // T-006 (from T-052 validation 1, F1; orchestrator decision on Refresh 2): `Applied`
    // records the icon and the tooltip only from a setter that returned Ok (tauri 2.12.1
    // has no getter to read them back). The shell's icon is removed from the notification
    // area (as Explorer would lose it), so `set_icon` and `set_tooltip` fail on the next
    // render; the render writes `tray_failed`, and `Applied.icon` / `.tooltip` still show
    // the Idle icon and tooltip that are on screen, not Recording's. Together with
    // `each_state_and_retry_flag_reaches_the_tray_with_its_icon_tooltip_and_menu` this
    // makes deleting either setter call red. Bite: `Applied.icon` / `.tooltip` written
    // from core's table whatever the setter returned (the code at T-052's commit).
    let _serial = serial();
    let dir = TempDir::new();
    let logs = dir.path().join("logs");
    let rig = rig_with_log(
        None,
        voicen_lib::diag::start(logs.clone(), Box::new(|_| {})),
    );
    assert_eq!(
        rig.applied(),
        Some(expected(TrayState::Idle, false, UiLanguage::En)),
        "premise"
    );
    let removed = remove_tray_icons();
    assert!(
        removed >= 1,
        "runner precondition `taskbar` (docs/decisions/windows-ci-runner.md, ok in runs A \
         and B): no tray icon of this thread could be removed from the notification area"
    );

    rig.part().set_tray(TrayState::Recording, false);

    assert!(
        eventually(|| tray_failed_lines(&logs) >= 1),
        "premise: no tray_failed line after rendering Recording on a removed icon"
    );
    let applied = rig.applied().expect("the applied record");
    assert_eq!(
        applied.icon,
        table::icon(TrayState::Idle),
        "Applied.icon recorded from a failed set_icon"
    );
    assert_eq!(
        applied.tooltip,
        i18n::text(UiLanguage::En, table::tooltip(TrayState::Idle), &[]),
        "Applied.tooltip recorded from a failed set_tooltip"
    );
}

// ---- T-071: the Open logs folder item ---------------------------------------------

/// One `FolderOpener::open` call as the fake saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenCall {
    dir: PathBuf,
    /// Whether `dir` was a directory when the opener was called.
    was_dir: bool,
    thread: ThreadId,
}

/// The fake `FolderOpener` port: records every call; returns the OS error code
/// queued in `fail_next` (once), `Ok` otherwise; while `held`, waits in `open` until
/// released (or `HOLD_LIMIT`). Opens nothing.
#[derive(Default)]
struct FakeOpener {
    calls: Mutex<Vec<OpenCall>>,
    fail_next: Mutex<Option<i32>>,
    /// `(held, released)`.
    hold: Mutex<(bool, bool)>,
    changed: Condvar,
}

impl FakeOpener {
    fn failing_once(os_code: i32) -> FakeOpener {
        let fake = FakeOpener::default();
        *fake
            .fail_next
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(os_code);
        fake
    }

    fn held() -> FakeOpener {
        let fake = FakeOpener::default();
        fake.hold.lock().unwrap_or_else(PoisonError::into_inner).0 = true;
        fake
    }

    fn release(&self) {
        self.hold.lock().unwrap_or_else(PoisonError::into_inner).1 = true;
        self.changed.notify_all();
    }

    fn calls(&self) -> Vec<OpenCall> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl FolderOpener for FakeOpener {
    fn open(&self, dir: &Path) -> io::Result<()> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(OpenCall {
                dir: dir.to_path_buf(),
                was_dir: dir.is_dir(),
                thread: thread::current().id(),
            });
        let hold = self.hold.lock().unwrap_or_else(PoisonError::into_inner);
        if hold.0 {
            let _released = self
                .changed
                .wait_timeout_while(hold, HOLD_LIMIT, |h| !h.1)
                .unwrap_or_else(PoisonError::into_inner);
        }
        match self
            .fail_next
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            Some(code) => Err(io::Error::from_raw_os_error(code)),
            None => Ok(()),
        }
    }
}

/// Releases a held opener when the test ends, also by a failed assertion.
struct ReleaseOpenerOnDrop(Arc<FakeOpener>);

impl Drop for ReleaseOpenerOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// The `warning kind=logs_folder_failed` lines of `<logs>/voicen.log`.
fn logs_folder_failed_lines(logs: &Path) -> Vec<String> {
    std::fs::read_to_string(logs.join("voicen.log"))
        .map(|t| {
            t.lines()
                .filter(|l| l.split_whitespace().any(|w| w == "kind=logs_folder_failed"))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The words of a log line after its timestamp and level (`warning kind=… …`).
fn message_words(line: &str) -> Vec<&str> {
    line.split_whitespace().skip(2).collect()
}

/// A rig whose log is `<dir>/diag` (readable), plus that path.
fn rig_logging_under(dir: &TempDir) -> (Rig, PathBuf) {
    let diag_logs = dir.path().join("diag");
    let rig = rig_with_log(
        None,
        voicen_lib::diag::start(diag_logs.clone(), Box::new(|_| {})),
    );
    (rig, diag_logs)
}

#[test]
fn the_open_logs_item_creates_and_opens_the_logs_dir_off_the_main_thread() {
    // T-071 Acceptance 1 and the invariant: a click on Open logs folder goes
    // on_menu_event -> logs_folder::request, which creates the installed logs dir
    // when it is missing (here two levels are missing, as after the folder was
    // deleted while the app runs) and hands exactly that dir to the FolderOpener
    // port, on a thread that is not the main thread (the test thread is the mock's
    // main thread); the handler returns at once while the opener is still busy
    // (ShellExecuteEx can block on shell extensions). A success writes no warning.
    // Bite: OpenLogs not routed in on_menu_event (no call), the opener called
    // inline on the main thread (the handler blocks on the held opener, and the
    // thread is the main one), no create_dir_all (or create_dir only) before the
    // open (`was_dir` false), another path handed over (the log file, the parent,
    // `paths::log_dir()` instead of the installed dir), a warning on success.
    let _serial = serial();
    let dir = TempDir::new();
    let (rig, diag_logs) = rig_logging_under(&dir);
    let target = dir.path().join("deleted").join("logs");
    assert!(!target.exists(), "premise: the logs dir is missing");
    let opener = Arc::new(FakeOpener::held());
    let _release = ReleaseOpenerOnDrop(Arc::clone(&opener));
    logs_folder::install(rig.handle(), target.clone(), opener.clone());
    let main = thread::current().id();

    let started = Instant::now();
    tray::on_menu_event(rig.handle(), menu_item(TrayAction::OpenLogs));
    let took = started.elapsed();
    opener.release();

    assert!(
        took < RETURNS_AT_ONCE,
        "the Open logs handler waited {took:?} for the opener on the main thread"
    );
    assert!(
        eventually(|| opener.calls().len() == 1),
        "the Open logs item did not reach the opener: {:?}",
        opener.calls()
    );
    let call = opener.calls().remove(0);
    assert_eq!(call.dir, target, "the opener got another path");
    assert!(
        call.was_dir,
        "the opener was called before the logs dir was created"
    );
    assert_ne!(call.thread, main, "the opener ran on the main thread");
    assert!(target.is_dir(), "the logs dir does not exist afterwards");
    assert!(
        holds_for_a_while(
            || opener.calls().len() == 1 && logs_folder_failed_lines(&diag_logs).is_empty()
        ),
        "after a successful open: calls {:?}, warnings {:?}",
        opener.calls(),
        logs_folder_failed_lines(&diag_logs)
    );
}

#[test]
fn a_failing_opener_writes_one_logs_folder_failed_line_and_the_app_keeps_working() {
    // T-071 Acceptance 2 (failure branch: Explorer cannot be started): the opener's
    // error ends in exactly one `warning kind=logs_folder_failed os_code=5` line,
    // with nothing else in it (no path, no user name, no OS text such as "Access is
    // denied"); the tray still applies a state, and a second click reaches the
    // opener again and, succeeding, adds no line. Bite: the error swallowed (no
    // line), logged twice (once per layer), logged without its code, the path or
    // the OS message put in the line, a panic in the worker that poisons or
    // removes the managed state so the second click does nothing, or a "failed
    // once, never again" latch.
    let _serial = serial();
    let dir = TempDir::new();
    let (rig, diag_logs) = rig_logging_under(&dir);
    let target = dir.path().join("logs");
    let opener = Arc::new(FakeOpener::failing_once(5));
    logs_folder::install(rig.handle(), target.clone(), opener.clone());

    tray::on_menu_event(rig.handle(), menu_item(TrayAction::OpenLogs));

    assert!(
        eventually(|| !logs_folder_failed_lines(&diag_logs).is_empty()),
        "no logs_folder_failed line after the opener failed; calls {:?}",
        opener.calls()
    );
    let lines = logs_folder_failed_lines(&diag_logs);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(
        message_words(&lines[0]),
        vec!["warning", "kind=logs_folder_failed", "os_code=5"],
        "{:?}",
        lines[0]
    );
    let tmp_name = dir
        .path()
        .file_name()
        .and_then(|n| n.to_str())
        .expect("the temp dir has a name");
    assert!(
        !lines[0].contains(tmp_name),
        "the path is in {:?}",
        lines[0]
    );

    rig.part().set_tray(TrayState::Recording, false);
    assert!(
        eventually(|| rig.applied() == Some(expected(TrayState::Recording, false, UiLanguage::En))),
        "the tray stopped applying after the failed open: {:?}",
        rig.applied()
    );

    tray::on_menu_event(rig.handle(), menu_item(TrayAction::OpenLogs));
    assert!(
        eventually(|| opener.calls().len() == 2),
        "a second click did not reach the opener: {:?}",
        opener.calls()
    );
    assert!(
        holds_for_a_while(|| logs_folder_failed_lines(&diag_logs).len() == 1),
        "a successful second open changed the warnings: {:?}",
        logs_folder_failed_lines(&diag_logs)
    );
}

#[test]
fn a_file_at_the_logs_path_is_never_opened_and_gives_one_warning() {
    // T-071 invariant (guard before ShellExecuteEx "open", which would execute a
    // file): when the logs path is a file, creating the dir fails and the opener is
    // never called; one `logs_folder_failed` line (an os_code if the OS gave one,
    // nothing else) and the file is left as it was. Bite: the create error ignored
    // and no is_dir check (the file handed to the opener), the file removed or
    // replaced to make room for the dir, no warning, or more than one.
    let _serial = serial();
    let dir = TempDir::new();
    let (rig, diag_logs) = rig_logging_under(&dir);
    let target = dir.path().join("logs");
    std::fs::write(&target, b"not a folder").expect("premise: a file at the logs path");
    let opener = Arc::new(FakeOpener::default());
    logs_folder::install(rig.handle(), target.clone(), opener.clone());

    tray::on_menu_event(rig.handle(), menu_item(TrayAction::OpenLogs));

    assert!(
        eventually(|| !logs_folder_failed_lines(&diag_logs).is_empty()),
        "no logs_folder_failed line for a file at the logs path"
    );
    assert!(
        holds_for_a_while(
            || opener.calls().is_empty() && logs_folder_failed_lines(&diag_logs).len() == 1
        ),
        "calls {:?}, warnings {:?}",
        opener.calls(),
        logs_folder_failed_lines(&diag_logs)
    );
    let lines = logs_folder_failed_lines(&diag_logs);
    let words = message_words(&lines[0]);
    assert_eq!(
        words.get(..2),
        Some(&["warning", "kind=logs_folder_failed"][..])
    );
    assert!(
        words.len() == 2
            || (words.len() == 3
                && words[2]
                    .strip_prefix("os_code=")
                    .is_some_and(|c| c.parse::<i32>().is_ok())),
        "only kind and os_code may follow `warning`: {:?}",
        lines[0]
    );
    assert_eq!(
        std::fs::read(&target).ok().as_deref(),
        Some(&b"not a folder"[..]),
        "the file at the logs path was changed"
    );
}

#[test]
fn the_shell_opener_refuses_a_missing_folder_with_code_2_and_no_window() {
    // T-071 option A: the Win32 adapter (ShellExecuteExW "open",
    // SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI) reports a missing path as Err with
    // Win32 code 2 (ERROR_FILE_NOT_FOUND), the code the warning carries, and shows
    // no error dialog: without FLAG_NO_UI the call shows a modal "Windows cannot
    // find" box and does not return, which the bounded wait turns red. It creates
    // nothing (creating is logs_folder's). Only the failure branch is run: a
    // success would open an Explorer window on the runner (F-006). Bite: the error
    // dropped (Ok for a missing path), the HRESULT (0x80070002) or a ShellExecute
    // instance code (<= 32) passed as the os code, FLAG_NO_UI left out, the adapter
    // creating the folder.
    let _serial = serial();
    let dir = TempDir::new();
    let missing = dir.path().join("missing");

    let (tx, rx) = mpsc::channel();
    let path = missing.clone();
    thread::spawn(move || {
        let _ = tx.send(ShellOpener.open(&path));
    });
    let result = rx.recv_timeout(BUDGET).unwrap_or_else(|_| {
        panic!("ShellOpener did not return within {BUDGET:?} for a missing path (an error dialog?)")
    });

    let err = result.expect_err("ShellOpener opened a missing folder");
    assert_eq!(err.raw_os_error(), Some(2), "{err:?}");
    assert!(!missing.exists(), "ShellOpener created the folder");
}

/// The release context (tauri.conf.json as tauri-build resolved it).
fn release_context<R: Runtime>() -> Context<R> {
    tauri::generate_context!(test = true)
}

#[test]
fn config_declares_no_tray_icon() {
    // Characterization (green before T-052; T-052 analysis hypothesis 2(e)): a
    // `tauri.conf.json` `app.trayIcon` would be built by tauri inside `build()`,
    // before the plugins decide, so a second instance would flash an icon before it
    // exits; the tray is built in code after `build()`. Decided on tauri's own parse
    // of the config (F-003). Bite: a trayIcon added to the config.
    let context: Context<MockRuntime> = release_context();
    assert!(
        context.config().app.tray_icon.is_none(),
        "tauri.conf.json declares app.trayIcon"
    );
}
