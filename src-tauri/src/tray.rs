//! The tray icon (T-052; spec 001 FR-001, FR-002, FR-008 / FR-028; spec 004
//! FR-013, FR-019; T-052 invariants 3-5).
//!
//! - Built by `assemble` after tauri's `build()` (never from `tauri.conf.json`
//!   `app.trayIcon`, which tauri builds before the plugins decide), with id
//!   [`TRAY_ID`], the `TrayState::Idle` icon and tooltip, and the menu in the
//!   language of the settings snapshot taken after its own `subscribe()`.
//! - Menu rows, ids, texts, marks, enabled flags and icons come from core
//!   `voicen_core::tray` only.
//! - The tray changes only in one fire-and-forget main-thread task that applies the
//!   latest `(TrayState, retry_available, call_recording, ui_language)`:
//!   [`TrayPart::set_tray`] (called by the dictation session under its lock, through
//!   T-006's composite `Indicator`), [`TrayPart::set_call_recording`] (called by the
//!   call session under its lock, through `call::ShellCallIndicator`; T-128) and the
//!   language follower only store the value and post that task; none calls a
//!   `TrayIcon` setter (they wait for the main thread). The icon and tooltip show
//!   `voicen_core::tray::shown(dictation, call_recording)`, the menu
//!   `voicen_core::tray::menu(retry_available, call_recording)`.
//! - The main thread never calls a session input: opening the menu stamps the
//!   instant and hands `DictationSession::tray_menu_opened` to another thread.
//! - "Settings" posts `settings_window::request(Front)`; "Open logs folder" calls
//!   `logs_folder::request` (its own thread, T-071); "Exit" calls
//!   `AppHandle::exit(0)`, the only user exit. "Record" and "Stop" post a
//!   `call::CallRequest` to the call worker (T-128; nothing without a call part) and
//!   "Calls" calls `call::open_calls`; "Record" also rebuilds the menu, because muda
//!   flips a check item's native mark on a click and only the table may decide it
//!   (decision #223).
//! - Only tauri keeps the `TrayIcon` (in its resources), so tauri's exit cleanup
//!   drops the last copy and tray-icon removes the icon from the notification
//!   area; this module looks the icon up for each render.

use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, MenuItemKind, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};
use voicen_core::call::CallSource;
use voicen_core::diag::{Log, LogEvent, WarningKind};
use voicen_core::dictation::DictationSession;
use voicen_core::i18n::{self, UiLanguage};
use voicen_core::recording::TrayState;
use voicen_core::settings::gate::{second_launch_action, LaunchArgs, SecondLaunchAction};
use voicen_core::settings::service::SettingsService;
use voicen_core::settings::Settings;
use voicen_core::tray::{self as table, TrayAction, TrayIconKind, TrayRow};

use crate::call::{self, CallRequest};
use crate::diag::io_os_code;
use crate::logs_folder;
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
    /// The menu set on the icon, as rows read back from its items, top to bottom.
    pub menu: Vec<AppliedRow>,
}

/// A row of the menu on the icon, read back from the built menu. The submenu has no
/// id here: its id is generated and no action maps to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppliedRow {
    Item {
        id: String,
        text: String,
        enabled: bool,
    },
    Check {
        id: String,
        text: String,
        checked: bool,
        enabled: bool,
    },
    Submenu {
        text: String,
        rows: Vec<AppliedRow>,
    },
}

/// What the tray is to show: the dictation session's `(TrayState, retry_available)`,
/// the call session's recording flag (only what `CallIndicator::set_call_recording`
/// last published) and the settings' UI language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct View {
    state: TrayState,
    retry_available: bool,
    call_recording: bool,
    lang: UiLanguage,
    /// Bumped to rebuild the menu from the table with nothing else changed (decision
    /// #223: a click on "Record" leaves muda's flipped mark on the item until then).
    menu_epoch: u64,
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
    /// `(lang, retry_available, call_recording, menu_epoch)` of the menu on the icon;
    /// read and written by the apply task only.
    menu_key: Mutex<MenuKey>,
    applied: Mutex<Applied>,
    /// The menu last set on the icon, for [`live_menu`] (tauri 2.12.1 has no getter
    /// for a tray's menu). Unlike a `TrayIcon` copy it keeps no icon in the
    /// notification area.
    live_menu: Mutex<Option<Menu<R>>>,
}

/// What the menu on the icon was built from: `(lang, retry_available, call_recording,
/// menu_epoch)`.
type MenuKey = (UiLanguage, bool, bool, u64);

impl View {
    /// The key of the menu this view asks for.
    fn menu_key(&self) -> MenuKey {
        (
            self.lang,
            self.retry_available,
            self.call_recording,
            self.menu_epoch,
        )
    }
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

    /// Sets the icon and the tooltip of `shown(view.state, view.call_recording)`, and a
    /// new menu when its language, `retry_available`, `call_recording` or `menu_epoch`
    /// changed, then records the rendered view. Does nothing once
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
        let state = table::shown(view.state, view.call_recording);
        let kind = table::icon(state);
        let set_kind = match icon.set_icon(Some(image(kind))) {
            Ok(()) => Some(kind),
            Err(err) => {
                note(err);
                None
            }
        };
        let tooltip = i18n::text(view.lang, table::tooltip(state), &[]);
        let set_tooltip = match icon.set_tooltip(Some(&tooltip)) {
            Ok(()) => Some(tooltip),
            Err(err) => {
                note(err);
                None
            }
        };
        let key = view.menu_key();
        let previous_key = *lock(&self.menu_key);
        let new_menu = if key == previous_key {
            None
        } else {
            match build_menu(
                &self.app,
                view.lang,
                view.retry_available,
                view.call_recording,
            ) {
                Ok((menu, items)) => match icon.set_menu(Some(menu.clone())) {
                    Ok(()) => {
                        *lock(&self.menu_key) = key;
                        // The replaced menu drops after the guard: its drop runs a
                        // main-thread task, never under this lock.
                        let _replaced = lock(&self.live_menu).replace(menu);
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
            applied.state = state;
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

/// Core's menu for `retry_available` and `call_recording`, its texts in `lang`, and its
/// rows read back from the built menu.
fn build_menu<R: Runtime>(
    app: &AppHandle<R>,
    lang: UiLanguage,
    retry_available: bool,
    call_recording: bool,
) -> tauri::Result<(Menu<R>, Vec<AppliedRow>)> {
    let items = render_rows(app, lang, &table::menu(retry_available, call_recording))?;
    let refs: Vec<&dyn IsMenuItem<R>> = items.iter().map(|i| i as &dyn IsMenuItem<R>).collect();
    let menu = Menu::with_items(app, &refs)?;
    let read = read_rows(&menu.items()?)?;
    Ok((menu, read))
}

/// The menu items of core's `rows`, texts in `lang`.
fn render_rows<R: Runtime>(
    app: &AppHandle<R>,
    lang: UiLanguage,
    rows: &[TrayRow],
) -> tauri::Result<Vec<MenuItemKind<R>>> {
    rows.iter()
        .map(|row| {
            Ok(match row {
                TrayRow::Item { action, enabled } => MenuItemKind::MenuItem(MenuItem::with_id(
                    app,
                    action.as_str(),
                    i18n::text(lang, action.label(), &[]),
                    *enabled,
                    None::<&str>,
                )?),
                TrayRow::Check {
                    action,
                    checked,
                    enabled,
                } => MenuItemKind::Check(CheckMenuItem::with_id(
                    app,
                    action.as_str(),
                    i18n::text(lang, action.label(), &[]),
                    *enabled,
                    *checked,
                    None::<&str>,
                )?),
                TrayRow::Submenu { label, rows } => {
                    let items = render_rows(app, lang, rows)?;
                    let refs: Vec<&dyn IsMenuItem<R>> =
                        items.iter().map(|i| i as &dyn IsMenuItem<R>).collect();
                    MenuItemKind::Submenu(Submenu::with_items(
                        app,
                        i18n::text(lang, *label, &[]),
                        true,
                        &refs,
                    )?)
                }
            })
        })
        .collect()
}

/// The rows of a built menu's `items`; a kind the table never asks for is an error.
fn read_rows<R: Runtime>(items: &[MenuItemKind<R>]) -> tauri::Result<Vec<AppliedRow>> {
    items
        .iter()
        .map(|kind| match kind {
            MenuItemKind::MenuItem(item) => Ok(AppliedRow::Item {
                id: item.id().as_ref().to_string(),
                text: item.text()?,
                enabled: item.is_enabled()?,
            }),
            MenuItemKind::Check(item) => Ok(AppliedRow::Check {
                id: item.id().as_ref().to_string(),
                text: item.text()?,
                checked: item.is_checked()?,
                enabled: item.is_enabled()?,
            }),
            MenuItemKind::Submenu(submenu) => Ok(AppliedRow::Submenu {
                text: submenu.text()?,
                rows: read_rows(&submenu.items()?)?,
            }),
            MenuItemKind::Predefined(_) | MenuItemKind::Icon(_) => {
                Err(tauri::Error::UnexpectedMenuKind)
            }
        })
        .collect()
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
        call_recording: false,
        lang: service.snapshot().ui_language,
        menu_epoch: 0,
    };
    let (applied, menu) = match build(app, view) {
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
        menu_key: Mutex::new(view.menu_key()),
        applied: Mutex::new(applied),
        live_menu: Mutex::new(Some(menu)),
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
/// first apply and the menu set on the icon. The `TrayIcon` the builder returns is dropped here, on the building
/// (main) thread: tauri's resources keep the only copy.
fn build<R: Runtime>(app: &AppHandle<R>, view: View) -> tauri::Result<(Applied, Menu<R>)> {
    let (menu, items) = build_menu(app, view.lang, view.retry_available, view.call_recording)?;
    let state = table::shown(view.state, view.call_recording);
    let kind = table::icon(state);
    let tooltip = i18n::text(view.lang, table::tooltip(state), &[]);
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(image(kind))
        .tooltip(&tooltip)
        .menu(&menu)
        .show_menu_on_left_click(MENU_ON_LEFT_CLICK)
        .on_menu_event(|app, event| on_menu_event(app, event))
        .on_tray_icon_event(|tray, event| on_tray_icon_event(tray, event))
        .build(app)?;
    let applied = Applied {
        state,
        retry_available: view.retry_available,
        lang: view.lang,
        icon: kind,
        tooltip,
        menu: items,
    };
    Ok((applied, menu))
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
/// forwards `set_tray` here) and of the call session's `CallIndicator`
/// (`call::ShellCallIndicator` forwards `set_call_recording` here; T-128).
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

    /// Stores the call session's recording flag and, unless an apply task is already
    /// pending, posts one, exactly like [`TrayPart::set_tray`]: it returns at once and
    /// never calls into a session (called under the call session lock). The flag is
    /// part of the menu key, so a change rebuilds the menu (Record's mark, Stop's
    /// enabled flag).
    pub fn set_call_recording(&self, recording: bool) {
        self.shared.want(|view| view.call_recording = recording);
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

/// The menu last set on the tray icon; `None` when the tray was not built. A test-read
/// accessor like [`applied`]: `Applied` is a value record, and tauri has no getter for
/// the tray's menu, so this is the only way to read the live items (decision #223).
pub fn live_menu<R: Runtime>(app: &AppHandle<R>) -> Option<Menu<R>> {
    app.try_state::<Tray<R>>()
        .and_then(|tray| lock(&tray.0.live_menu).clone())
}

/// The tray menu's handler (`TrayIconBuilder::on_menu_event`): the item id mapped by
/// `TrayAction::from_id`; `OpenSettings` posts `settings_window::request(Front)`
/// and drops the receipt, `OpenLogs` calls `logs_folder::request` (returns at
/// once), `Exit` calls `app.exit(0)`, any other id does nothing. The call items only
/// post (T-128; no session lock here): `CallRecord` rebuilds the menu from the table,
/// undoing muda's flip of the mark (so a refused start, which publishes no flag, leaves
/// it unchecked; decision #223), and posts `CallRequest::Record(Tray)`, whose intent the
/// call worker reads from the session; `CallStop` posts `CallRequest::Stop(Tray)`;
/// `OpenCalls` calls `call::open_calls`. Without a call part the posts do nothing.
/// `Retry` does nothing until T-210.
pub fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    match TrayAction::from_id(event.id().as_ref()) {
        Some(TrayAction::CallRecord) => {
            if let Some(part) = part(app) {
                part.shared
                    .want(|view| view.menu_epoch = view.menu_epoch.wrapping_add(1));
            }
            call::request(app, CallRequest::Record(CallSource::Tray));
        }
        Some(TrayAction::CallStop) => call::request(app, CallRequest::Stop(CallSource::Tray)),
        Some(TrayAction::OpenCalls) => call::open_calls(app),
        Some(TrayAction::OpenSettings) => {
            let _ = settings_window::request(app, OpenTarget::Front);
        }
        Some(TrayAction::OpenLogs) => logs_folder::request(app),
        // T-210: `DictationSession::retry(at, None)`.
        Some(TrayAction::Retry) => {}
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
    // (on the main thread) never takes it; dropping the handle detaches the task.
    drop(tauri::async_runtime::spawn_blocking(move || {
        session.tray_menu_opened(at)
    }));
}

/// The single-instance callback (in the primary, on its main thread, while the
/// second process waits): `second_launch_action(&LaunchArgs::parse(&argv))`;
/// `FrontSettings` posts `settings_window::request(Front)` and returns its receipt;
/// `StartCallRecord` and `TrayOnly` return `None` and hand the parsed arguments to
/// `call::apply_launch`, the one `--call` mapping of a first and a second launch
/// (T-128): `--call record` posts `CallRequest::Record(Switch)`, an unknown `--call`
/// value posts `CallRequest::RefuseSwitch` (the call worker writes its line), no
/// `--call` posts nothing. Writes no log line itself and touches no file.
pub fn on_second_instance<R: Runtime>(
    app: &AppHandle<R>,
    argv: Vec<String>,
    _cwd: String,
) -> Option<Receipt> {
    let args = LaunchArgs::parse(&argv);
    match second_launch_action(&args) {
        SecondLaunchAction::FrontSettings => Some(settings_window::request(app, OpenTarget::Front)),
        SecondLaunchAction::TrayOnly | SecondLaunchAction::StartCallRecord => {
            call::apply_launch(app, &args);
            None
        }
    }
}
