//! The dictation wiring (T-006 invariant 5; design 6): [`start_dictation`] is the
//! one function `run()` and the tests use to build the only `DictationSession` of an
//! app. It builds the session from the managed `SettingsService`, one `LogObserver`
//! over the managed `Log`, the given ports (the credential store must be the
//! `SettingsService`'s own instance), the release-1 speech gate
//! (`SpeechGate(Err(Unavailable), energy)`), `PassThrough`, an interim pending-audio
//! store whose `put` fails (until T-007) and [`SettingsRequests`]; manages it as
//! `Arc<DictationSession>` (the type the tray's menu-open handler looks up) and
//! starts the hotkey thread for the settings' hotkey. `run()` and the tests differ
//! only in the ports.
//!
//! [`ShellIndicator`] is `run()`'s `Indicator`: the tray half forwards to the
//! tray's `TrayPart` (nothing when the tray was not built), the overlay half does
//! nothing until T-057. Both only store and post.

use std::io;
use std::sync::Arc;

use tauri::{AppHandle, Runtime};
use voicen_core::pipeline::EngineFactory;
use voicen_core::platform::{AudioSource, Clipboard, Indicator, Paster, ShellRequests};
use voicen_core::recording::{OverlayState, TrayState};
use voicen_core::secrets::CredentialStore;
use voicen_core::settings::gate::SettingsTab;

use crate::tray::TrayPart;

/// What the tests replace; `run()` passes the Windows adapters.
pub struct DictationPorts {
    pub audio: Arc<dyn AudioSource>,
    pub clipboard: Arc<dyn Clipboard>,
    pub paster: Arc<dyn Paster>,
    /// `None` in `run()` (`engine::engine_for`); a test engine in the tests.
    pub engine_factory: Option<Box<EngineFactory>>,
    pub indicator: Arc<dyn Indicator>,
    /// The `SettingsService`'s own instance (`PipelineDeps::credentials`).
    pub credentials: Arc<dyn CredentialStore>,
}

/// The started dictation of an app: owns the hotkey thread. Dropping it stops the
/// hotkey thread and frees the hotkey; the session stays managed by the app.
pub struct DictationHandle {
    _private: (),
}

/// Builds the one dictation session of `app` over `ports`, manages it as
/// `Arc<DictationSession>` and starts the hotkey thread for the hotkey of the
/// settings snapshot. Needs the managed `Arc<SettingsService>` and `Arc<Log>`
/// (`assemble`'s wiring). `Err` when the session or the hotkey thread could not be
/// started.
pub fn start_dictation<R: Runtime>(
    app: &AppHandle<R>,
    ports: DictationPorts,
) -> io::Result<DictationHandle> {
    let _ = (app, ports);
    todo!("T-006: start_dictation")
}

/// `run()`'s `Indicator`: the tray half to the tray part of the app, the overlay
/// half a no-op until T-057.
pub struct ShellIndicator<R: Runtime> {
    tray: Option<TrayPart<R>>,
}

impl<R: Runtime> ShellIndicator<R> {
    /// The indicator of `app`: its tray part (`tray::part`), if the tray was built.
    pub fn new(app: &AppHandle<R>) -> ShellIndicator<R> {
        let _ = app;
        todo!("T-006: ShellIndicator::new")
    }
}

impl<R: Runtime> Indicator for ShellIndicator<R> {
    fn set_tray(&self, state: TrayState, retry_available: bool) {
        let _ = (&self.tray, state, retry_available);
        todo!("T-006: ShellIndicator::set_tray")
    }

    fn set_overlay(&self, state: &OverlayState) {
        let _ = state;
        todo!("T-006: ShellIndicator::set_overlay")
    }
}

/// The session's `ShellRequests`: `open_settings(tab)` posts
/// `settings_window::request(app, OpenTarget::Tab(tab, None))` and drops the
/// receipt (never waits).
pub struct SettingsRequests<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> SettingsRequests<R> {
    pub fn new(app: &AppHandle<R>) -> SettingsRequests<R> {
        let _ = app;
        todo!("T-006: SettingsRequests::new")
    }
}

impl<R: Runtime> ShellRequests for SettingsRequests<R> {
    fn open_settings(&self, tab: SettingsTab) {
        let _ = (&self.app, tab);
        todo!("T-006: SettingsRequests::open_settings")
    }
}
