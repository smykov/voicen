//! The tray icon (T-052; spec 001 FR-001, FR-002, FR-008 / FR-028; spec 004
//! FR-013, FR-019; T-052 invariants 3-5).
//!
//! - Built by `assemble` after tauri's `build()` (never from `tauri.conf.json`
//!   `app.trayIcon`, which tauri builds before the plugins decide), with id
//!   [`TRAY_ID`], the `TrayState::Idle` icon and tooltip, and the menu in the
//!   language of the settings snapshot taken after its own `subscribe()`.
//! - Menu items, ids, texts and icons come from core `voicen_core::tray` only.
//! - The tray changes only in one fire-and-forget main-thread task that applies the
//!   latest `(TrayState, retry_available, ui_language)`: [`TrayPart::set_tray`]
//!   (called by the dictation session under its lock, through T-006's composite
//!   `Indicator`) and the language follower only store the value and post that
//!   task; neither calls a `TrayIcon` setter (they wait for the main thread).
//! - The main thread never calls a session input: opening the menu stamps the
//!   instant and hands `DictationSession::tray_menu_opened` to another thread.
//! - "Settings" posts `settings_window::request(Front)`; "Exit" calls
//!   `AppHandle::exit(0)`, the only user exit.
//! - Only tauri keeps the `TrayIcon` (in its resources), so tauri's exit cleanup
//!   drops the last copy and tray-icon removes the icon from the notification
//!   area; this module looks the icon up for each render.

use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use tauri::image::Image;
use tauri::menu::{IsMenuItem, Menu, MenuEvent, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};
use voicen_core::diag::{Log, LogEvent, WarningKind};
use voicen_core::dictation::DictationSession;
use voicen_core::i18n::{self, UiLanguage};
use voicen_core::recording::TrayState;
use voicen_core::settings::gate::{second_launch_action, SecondLaunchAction};
use voicen_core::settings::service::SettingsService;
use voicen_core::settings::Settings;
use voicen_core::tray::{self as table, TrayAction, TrayIconKind};

use crate::diag::io_os_code;
use crate::settings_window::{self, OpenTarget, Receipt};

/// The id of the one tray icon (`Manager::tray_by_id`).
pub const TRAY_ID: &str = "voicen";

/// Whether a left click shows the menu, as a right click does (OQ-11 Q3 default).
/// The one flag for the builder (`show_menu_on_left_click`) and for the handler's
/// "menu opened" (P-010).
const MENU_ON_LEFT_CLICK: bool = true;

/// The view the last apply task rendered on the tray icon (its build counts as the
/// first apply). Written by the task after the setters returned: `state`,
/// `retry_available` and `lang` are what it asked the icon to show, also when a
/// setter returned an error (one `tray_failed` warning); `icon` and `tooltip` are
/// the last image and tooltip whose setter returned `Ok` (T-006; tauri 2.12.1 has no
/// getter to read them back), and `menu` the last menu that was set. After a failed
/// setter the icon still shows an older state until the next change renders again,
/// and tray-icon re-registers the icon after an Explorer restart (TaskbarCreated)
/// with the last image and tooltip that were set successfully, which nothing
/// re-applies. Written under a lock that is never held across a setter, so
/// [`applied`] never waits for an apply in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub state: TrayState,
    pub retry_available: bool,
    pub lang: UiLanguage,
    /// `voicen_core::tray::icon(state)` of the last state whose `set_icon` returned
    /// `Ok`: the image on the icon.
    pub icon: TrayIconKind,
    /// The tooltip text of the last `set_tooltip` that returned `Ok`.
    pub tooltip: String,
    /// The menu set on the icon, as `(item id, item text)` read back from its
    /// items, top to bottom.
    pub menu: Vec<(String, String)>,
}

/// What the tray is to show: the session's `(TrayState, retry_available)` and the
/// settings' UI language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct View {
    state: TrayState,
    retry_available: bool,
    lang: UiLanguage,
}

/// The latest wanted view, bumped by every change, and whether an apply task is
/// posted (or running) that will still read it.
struct Wanted {
    view: View,
    version: u64,
    posted: bool,
}

/// The tray of one app: the wanted view and the apply record. It holds no
/// `TrayIcon`: tray-icon removes the icon from the notification area only when its
/// last copy is dropped, and tauri's exit cleanup drops only its own (resources
/// table), so a copy kept here would leave the icon behind after Exit (review
/// round 1 #1). Each render looks the icon up with `tray_by_id`.
struct Shared<R: Runtime> {
    app: AppHandle<R>,
    log: Arc<Log>,
    wanted: Mutex<Wanted>,
    /// `(lang, retry_available)` of the menu on the icon; read and written by the
    /// apply task only.
    menu_key: Mutex<(UiLanguage, bool)>,
    applied: Mutex<Applied>,
}

/// The managed tray of an app (absent when the tray could not be built).
struct Tray<R: Runtime>(Arc<Shared<R>>);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl<R: Runtime> Shared<R> {
    /// Stores a change of the wanted view and, unless an apply task is already
    /// posted, posts one (`run_on_main_thread`: from another thread it only posts;
    /// on the main thread it runs inline). Never calls a `TrayIcon` setter itself.
    fn want(self: &Arc<Self>, change: impl FnOnce(&mut View)) {
        let post = {
            let mut wanted = lock(&self.wanted);
            change(&mut wanted.view);
            wanted.version = wanted.version.wrapping_add(1);
            !std::mem::replace(&mut wanted.posted, true)
        };
        if post {
            let shared = Arc::clone(self);
            if self.app.run_on_main_thread(move || shared.apply()).is_err() {
                // The loop is gone (exit): nothing will render again.
                lock(&self.wanted).posted = false;
            }
        }
    }

    /// The apply task: renders the latest wanted view until no change came in
    /// during a render, then clears `posted`. So one task is in flight at a time
    /// and the last change is always rendered.
    fn apply(&self) {
        loop {
            let (view, version) = {
                let wanted = lock(&self.wanted);
                (wanted.view, wanted.version)
            };
            self.render(view);
            let mut wanted = lock(&self.wanted);
            if wanted.version == version {
                wanted.posted = false;
                return;
            }
        }
    }

    /// Sets the icon and the tooltip of `view`, and a new menu when its language or
    /// `retry_available` changed, then records the rendered view. Does nothing once
    /// tauri no longer has the icon (after the exit cleanup). No lock is held across
    /// a setter. The first failure writes one `tray_failed` warning: the kind, and
    /// the OS code only for a `tauri::Error::Io` (tray-icon's own OS errors arrive as
    /// `tauri::Error::Tray`, whose code the shell cannot reach without depending on
    /// tray-icon, so they carry none).
    fn render(&self, view: View) {
        let Some(icon) = self.app.tray_by_id(TRAY_ID) else {
            return;
        };
        let mut failure: Option<Option<i32>> = None;
        let mut note = |err: tauri::Error| {
            failure.get_or_insert(io_os_code(&err));
        };
        let kind = table::icon(view.state);
        let set_kind = match icon.set_icon(Some(image(kind))) {
            Ok(()) => Some(kind),
            Err(err) => {
                note(err);
                None
            }
        };
        let tooltip = i18n::text(view.lang, table::tooltip(view.state), &[]);
        let set_tooltip = match icon.set_tooltip(Some(&tooltip)) {
            Ok(()) => Some(tooltip),
            Err(err) => {
                note(err);
                None
            }
        };
        let key = (view.lang, view.retry_available);
        let previous_key = *lock(&self.menu_key);
        let new_menu = if key == previous_key {
            None
        } else {
            match build_menu(&self.app, view.lang, view.retry_available) {
                Ok((menu, items)) => match icon.set_menu(Some(menu)) {
                    Ok(()) => {
                        *lock(&self.menu_key) = key;
                        Some(items)
                    }
                    Err(err) => {
                        note(err);
                        None
                    }
                },
                Err(err) => {
                    note(err);
                    None
                }
            }
        };
        {
            let mut applied = lock(&self.applied);
            applied.state = view.state;
            applied.retry_available = view.retry_available;
            applied.lang = view.lang;
            if let Some(kind) = set_kind {
                applied.icon = kind;
            }
            if let Some(tooltip) = set_tooltip {
                applied.tooltip = tooltip;
            }
            if let Some(items) = new_menu {
                applied.menu = items;
            }
        }
        if let Some(os_code) = failure {
            self.log.write(LogEvent::Warning {
                kind: WarningKind::TrayFailed,
                os_code,
            });
        }
    }
}

/// The embedded image of `kind` (`src-tauri/icons/tray/`; OQ-11 Q1 default).
fn image(kind: TrayIconKind) -> Image<'static> {
    match kind {
        TrayIconKind::Idle => tauri::include_image!("icons/tray/idle.png"),
        TrayIconKind::Recording => tauri::include_image!("icons/tray/recording.png"),
        TrayIconKind::Error => tauri::include_image!("icons/tray/error.png"),
        TrayIconKind::HotkeyError => tauri::include_image!("icons/tray/hotkey_error.png"),
    }
}

/// Core's menu for `retry_available`, its texts in `lang`, and the `(id, text)` of
/// its items read back from the built menu.
fn build_menu<R: Runtime>(
    app: &AppHandle<R>,
    lang: UiLanguage,
    retry_available: bool,
) -> tauri::Result<(Menu<R>, Vec<(String, String)>)> {
    let items = table::menu(retry_available)
        .into_iter()
        .map(|action| {
            MenuItem::with_id(
                app,
                action.as_str(),
                i18n::text(lang, action.label(), &[]),
                true,
                None::<&str>,
            )
        })
        .collect::<tauri::Result<Vec<MenuItem<R>>>>()?;
    let refs: Vec<&dyn IsMenuItem<R>> = items.iter().map(|i| i as &dyn IsMenuItem<R>).collect();
    let menu = Menu::with_items(app, &refs)?;
    let read = menu
        .items()?
        .iter()
        .map(|kind| match kind.as_menuitem() {
            Some(item) => Ok((item.id().as_ref().to_string(), item.text()?)),
            None => Err(tauri::Error::UnexpectedMenuKind),
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    Ok((menu, read))
}

/// Builds the one tray icon of `app` (called once by `assemble`'s wiring, after
/// tauri's `build()`, on the thread that built the app): `TrayState::Idle`, no
/// Retry, the menu and tooltip in the language of `service.snapshot()`, read after
/// the follower's own `service.subscribe()`, so no saved language is missed. Then
/// manages it (`part`, `applied`) and starts the language follower thread
/// ("tray-language"), which acts only on a `ui_language` change. A failed build
/// writes one `tray_failed` warning (the kind, and the OS code only for a
/// `tauri::Error::Io`) and leaves the app without a tray, so nothing is prevented
/// at exit; a failed follower spawn writes `tray_follower_failed`. The icon itself
/// stays only in tauri's resources, so tauri's exit cleanup removes it from the
/// notification area.
pub(crate) fn install<R: Runtime>(app: &AppHandle<R>, service: &SettingsService, log: &Arc<Log>) {
    let changes = service.subscribe();
    let view = View {
        state: TrayState::Idle,
        retry_available: false,
        lang: service.snapshot().ui_language,
    };
    let applied = match build(app, view) {
        Ok(built) => built,
        Err(err) => {
            log.write(LogEvent::Warning {
                kind: WarningKind::TrayFailed,
                os_code: io_os_code(&err),
            });
            return;
        }
    };
    let shared = Arc::new(Shared {
        app: app.clone(),
        log: Arc::clone(log),
        wanted: Mutex::new(Wanted {
            view,
            version: 0,
            posted: false,
        }),
        menu_key: Mutex::new((view.lang, view.retry_available)),
        applied: Mutex::new(applied),
    });
    app.manage(Tray(Arc::clone(&shared)));
    if let Err(err) = spawn_follower(shared, changes, view.lang) {
        log.write(LogEvent::Warning {
            kind: WarningKind::TrayFollowerFailed,
            os_code: err.raw_os_error(),
        });
    }
}

/// Builds the tray icon for `view` with its handlers and returns the record of that
/// first apply. The `TrayIcon` the builder returns is dropped here, on the building
/// (main) thread: tauri's resources keep the only copy.
fn build<R: Runtime>(app: &AppHandle<R>, view: View) -> tauri::Result<Applied> {
    let (menu, items) = build_menu(app, view.lang, view.retry_available)?;
    let kind = table::icon(view.state);
    let tooltip = i18n::text(view.lang, table::tooltip(view.state), &[]);
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(image(kind))
        .tooltip(&tooltip)
        .menu(&menu)
        .show_menu_on_left_click(MENU_ON_LEFT_CLICK)
        .on_menu_event(|app, event| on_menu_event(app, event))
        .on_tray_icon_event(|tray, event| on_tray_icon_event(tray, event))
        .build(app)?;
    Ok(Applied {
        state: view.state,
        retry_available: view.retry_available,
        lang: view.lang,
        icon: kind,
        tooltip,
        menu: items,
    })
}

/// The language follower: one `SettingsService::subscribe()` of its own (decision
/// #34); on a saved `ui_language` other than the one shown it stores the language
/// and posts the apply task, nothing else.
fn spawn_follower<R: Runtime>(
    shared: Arc<Shared<R>>,
    changes: Receiver<Arc<Settings>>,
    mut lang: UiLanguage,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("tray-language".into())
        .spawn(move || {
            for settings in changes {
                if settings.ui_language != lang {
                    lang = settings.ui_language;
                    shared.want(|view| view.lang = lang);
                }
            }
        })
        .map(|_| ())
}

/// The tray half of the core `Indicator` port (T-006's composite `Indicator`
/// forwards `set_tray` here).
pub struct TrayPart<R: Runtime> {
    shared: Arc<Shared<R>>,
}

impl<R: Runtime> Clone for TrayPart<R> {
    fn clone(&self) -> Self {
        TrayPart {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<R: Runtime> TrayPart<R> {
    /// Stores `(state, retry_available)` and, unless an apply task is already
    /// pending, posts one to the main thread (`run_on_main_thread`). Returns at once:
    /// it never waits for the main thread and never calls into the session
    /// (dictation-session.md: called under the session lock).
    pub fn set_tray(&self, state: TrayState, retry_available: bool) {
        self.shared.want(|view| {
            view.state = state;
            view.retry_available = retry_available;
        });
    }
}

/// The tray part of `app`; `None` when the tray was not built.
pub fn part<R: Runtime>(app: &AppHandle<R>) -> Option<TrayPart<R>> {
    app.try_state::<Tray<R>>().map(|tray| TrayPart {
        shared: Arc::clone(&tray.0),
    })
}

/// The record of the last apply; `None` when the tray was not built.
pub fn applied<R: Runtime>(app: &AppHandle<R>) -> Option<Applied> {
    app.try_state::<Tray<R>>()
        .map(|tray| lock(&tray.0.applied).clone())
}

/// The tray menu's handler (`TrayIconBuilder::on_menu_event`): the item id mapped by
/// `TrayAction::from_id`; `OpenSettings` posts `settings_window::request(Front)`
/// and drops the receipt, `Exit` calls `app.exit(0)`, any other id does nothing.
pub fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    match TrayAction::from_id(event.id().as_ref()) {
        Some(TrayAction::OpenSettings) => {
            let _ = settings_window::request(app, OpenTarget::Front);
        }
        Some(TrayAction::Exit) => app.exit(0),
        None => {}
    }
}

/// `true` for the events after which the menu shows: `Click { Right, Up }`, and
/// `Click { Left, Up }` while [`MENU_ON_LEFT_CLICK`] (tray-icon sends the click
/// event, then shows the menu).
fn opens_menu(event: &TrayIconEvent) -> bool {
    match event {
        TrayIconEvent::Click {
            button,
            button_state: MouseButtonState::Up,
            ..
        } => match button {
            MouseButton::Right => true,
            MouseButton::Left => MENU_ON_LEFT_CLICK,
            MouseButton::Middle => false,
        },
        _ => false,
    }
}

/// The tray icon's handler (`TrayIconBuilder::on_tray_icon_event`). "Menu opened"
/// is `Click { Right, Up }`, or `Click { Left, Up }` while the menu shows on a left
/// click (OQ-11 Q3 default: it does, as for a right click): the instant is stamped
/// here and `tray_menu_opened(at)` runs on another thread, on the session managed
/// as `Arc<DictationSession>` (nothing before T-006 manages one). Every other event
/// does nothing.
pub fn on_tray_icon_event<R: Runtime>(tray: &TrayIcon<R>, event: TrayIconEvent) {
    if !opens_menu(&event) {
        return;
    }
    let at = Instant::now();
    let Some(session) = tray.app_handle().try_state::<Arc<DictationSession>>() else {
        return;
    };
    let session = Arc::clone(&session);
    // The session lock may be held across a slow `AudioSource::start`: the handler
    // (on the main thread) never takes it.
    let _ = tauri::async_runtime::spawn_blocking(move || session.tray_menu_opened(at));
}

/// The single-instance callback (in the primary, on its main thread, while the
/// second process waits): `second_launch_action(launched_by_autostart(argv))`;
/// `FrontSettings` posts `settings_window::request(Front)` and returns its receipt,
/// `TrayOnly` posts nothing (`None`). Writes no log line and touches no file.
pub fn on_second_instance<R: Runtime>(
    app: &AppHandle<R>,
    argv: Vec<String>,
    _cwd: String,
) -> Option<Receipt> {
    match second_launch_action(second_launched_by_autostart(&argv)) {
        SecondLaunchAction::FrontSettings => Some(settings_window::request(app, OpenTarget::Front)),
        SecondLaunchAction::TrayOnly => None,
    }
}

/// `true` when the second process's `argv` carries the Run value's `--autostart`
/// (T-014).
#[cfg(windows)]
fn second_launched_by_autostart(argv: &[String]) -> bool {
    crate::autostart::launched_by_autostart(argv)
}

#[cfg(not(windows))]
fn second_launched_by_autostart(_argv: &[String]) -> bool {
    compile_error!(
        "the Voicen app runs on Windows only: the autostart flag comes from the HKCU \
         Run value"
    )
}
