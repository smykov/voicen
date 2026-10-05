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

use tauri::menu::MenuEvent;
use tauri::tray::{TrayIcon, TrayIconEvent};
use tauri::{AppHandle, Runtime};
use voicen_core::i18n::UiLanguage;
use voicen_core::recording::TrayState;
use voicen_core::tray::TrayIconKind;

use crate::settings_window::Receipt;

/// The id of the one tray icon (`Manager::tray_by_id`).
pub const TRAY_ID: &str = "voicen";

/// What the last apply task set on the tray icon (its build counts as the first
/// apply). Written by the task after the setters returned, under a lock that is
/// never held across a setter, so [`applied`] never waits for an apply in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub state: TrayState,
    pub retry_available: bool,
    pub lang: UiLanguage,
    /// `voicen_core::tray::icon(state)`: the image set on the icon.
    pub icon: TrayIconKind,
    /// The tooltip text set on the icon.
    pub tooltip: String,
    /// The menu set on the icon, as `(item id, item text)` read back from its
    /// items, top to bottom.
    pub menu: Vec<(String, String)>,
}

/// The tray half of the core `Indicator` port (T-006's composite `Indicator`
/// forwards `set_tray` here).
pub struct TrayPart<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> Clone for TrayPart<R> {
    fn clone(&self) -> Self {
        TrayPart {
            app: self.app.clone(),
        }
    }
}

impl<R: Runtime> TrayPart<R> {
    /// Stores `(state, retry_available)` and, unless an apply task is already
    /// pending, posts one to the main thread (`run_on_main_thread`). Returns at once:
    /// it never waits for the main thread and never calls into the session
    /// (dictation-session.md: called under the session lock).
    pub fn set_tray(&self, state: TrayState, retry_available: bool) {
        // Skeleton (T-052 red tests): not implemented yet.
        let _ = (&self.app, state, retry_available);
        todo!("T-052: TrayPart::set_tray")
    }
}

/// The tray part of `app`; `None` when the tray was not built.
pub fn part<R: Runtime>(app: &AppHandle<R>) -> Option<TrayPart<R>> {
    // Skeleton (T-052 red tests): not implemented yet.
    let _ = app;
    todo!("T-052: tray::part")
}

/// The record of the last apply; `None` when the tray was not built.
pub fn applied<R: Runtime>(app: &AppHandle<R>) -> Option<Applied> {
    // Skeleton (T-052 red tests): not implemented yet.
    let _ = app;
    todo!("T-052: tray::applied")
}

/// The tray menu's handler (`TrayIconBuilder::on_menu_event`): the item id mapped by
/// `TrayAction::from_id`; `OpenSettings` posts `settings_window::request(Front)`
/// and drops the receipt, `Exit` calls `app.exit(0)`, any other id does nothing.
pub fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    // Skeleton (T-052 red tests): not implemented yet.
    let _ = (app, event);
    todo!("T-052: tray::on_menu_event")
}

/// The tray icon's handler (`TrayIconBuilder::on_tray_icon_event`). "Menu opened"
/// is `Click { Right, Up }`, or `Click { Left, Up }` while the menu shows on a left
/// click (OQ-11 Q3 default: it does, as for a right click): the instant is stamped
/// here and `tray_menu_opened(at)` runs on another thread, on the session managed
/// as `Arc<DictationSession>` (nothing before T-006 manages one). Every other event
/// does nothing.
pub fn on_tray_icon_event<R: Runtime>(tray: &TrayIcon<R>, event: TrayIconEvent) {
    // Skeleton (T-052 red tests): not implemented yet.
    let _ = (tray, event);
    todo!("T-052: tray::on_tray_icon_event")
}

/// The single-instance callback (in the primary, on its main thread, while the
/// second process waits): `second_launch_action(launched_by_autostart(argv))`;
/// `FrontSettings` posts `settings_window::request(Front)` and returns its receipt,
/// `TrayOnly` posts nothing (`None`). Writes no log line and touches no file.
pub fn on_second_instance<R: Runtime>(
    app: &AppHandle<R>,
    argv: Vec<String>,
    cwd: String,
) -> Option<Receipt> {
    // Skeleton (T-052 red tests): not implemented yet.
    let _ = (app, argv, cwd);
    todo!("T-052: tray::on_second_instance")
}
