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
//! tray's `TrayPart` (nothing when the tray was not built), the overlay half to the
//! overlay's `OverlayPart` (T-057: the newest state into the overlay mailbox, then
//! the overlay thread builds, emits or destroys the window). Both only store and
//! post.

use std::io;
use std::sync::Arc;

use tauri::{AppHandle, Manager, Runtime};
use voicen_core::audio::AudioBuffer;
use voicen_core::diag::{Log, LogEvent, LogObserver, WarningKind};
use voicen_core::dictation::{DictationSession, SessionDeps};
use voicen_core::pipeline::{EngineFactory, PipelineDeps};
use voicen_core::platform::{
    AudioSource, Clipboard, Indicator, Paster, PendingId, ShellRequests, TempAudioStore,
};
use voicen_core::post_process::PassThrough;
use voicen_core::recording::{OverlayState, TrayState};
use voicen_core::secrets::CredentialStore;
use voicen_core::settings::gate::SettingsTab;
use voicen_core::settings::service::SettingsService;
use voicen_core::vad::{EnergyDetector, SpeechGate, VadError};

use crate::overlay::{self, OverlayPart};
use crate::settings_window::{self, OpenTarget};
use crate::tray::{self, TrayPart};
use crate::win::hotkey::HotkeyThread;

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
    _hotkey: HotkeyThread,
}

/// Builds the one dictation session of `app` over `ports`, manages it as
/// `Arc<DictationSession>` and starts the hotkey thread for the hotkey of the
/// settings snapshot. Needs the managed `Arc<SettingsService>` and `Arc<Log>`
/// (`assemble`'s wiring). `Err` when the session or the hotkey thread could not be
/// started, or when the app already has a session (then nothing is started): a
/// failed session start (or a second one) writes `dictation_start_failed`, a failed
/// hotkey thread `hotkey_thread_failed`.
pub fn start_dictation<R: Runtime>(
    app: &AppHandle<R>,
    ports: DictationPorts,
) -> io::Result<DictationHandle> {
    let log = app
        .try_state::<Arc<Log>>()
        .map(|log| Arc::clone(&log))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the app manages no log"))?;
    let failed = |err: io::Error| {
        log.write(LogEvent::Warning {
            kind: WarningKind::DictationStartFailed,
            os_code: err.raw_os_error(),
        });
        err
    };
    let service = app
        .try_state::<Arc<SettingsService>>()
        .map(|service| Arc::clone(&service))
        .ok_or_else(|| {
            failed(io::Error::new(
                io::ErrorKind::NotFound,
                "the app manages no settings service",
            ))
        })?;
    if app.try_state::<Arc<DictationSession>>().is_some() {
        return Err(failed(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "the app already has a dictation session",
        )));
    }
    let DictationPorts {
        audio,
        clipboard,
        paster,
        engine_factory,
        indicator,
        credentials,
    } = ports;
    let session = DictationSession::start(SessionDeps {
        pipeline: PipelineDeps {
            // Release 1 has no Silero (#62, T-043): the energy detector decides and
            // the first decision writes the one `vad_fallback` line (FR-016).
            gate: SpeechGate::new(Err(VadError::Unavailable), EnergyDetector::new()),
            credentials,
            clipboard,
            paster,
            temp_audio: Arc::new(NoPendingAudio),
            observer: Arc::new(LogObserver::new(Arc::clone(&log))),
            post_processor: Arc::new(PassThrough),
        },
        engine_factory,
        audio,
        indicator,
        requests: Arc::new(SettingsRequests::new(app)),
        settings: Arc::clone(&service),
    })
    .map_err(failed)?;
    let session = Arc::new(session);
    if !app.manage(Arc::clone(&session)) {
        return Err(failed(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "the app already has a dictation session",
        )));
    }
    let hotkey = service.snapshot().hotkey.clone();
    let hotkey = HotkeyThread::start(session, &hotkey, log)?;
    Ok(DictationHandle { _hotkey: hotkey })
}

/// The interim pending-audio store (until T-007's file store): keeps nothing, so a
/// failed dictation leaves no pending recording (T-001 Notes).
struct NoPendingAudio;

fn not_kept() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "no pending audio store until T-007",
    )
}

impl TempAudioStore for NoPendingAudio {
    fn put_pending(&self, _id: PendingId, _audio: &AudioBuffer) -> io::Result<()> {
        Err(not_kept())
    }

    fn get_pending(&self, _id: PendingId) -> io::Result<AudioBuffer> {
        Err(io::Error::from(io::ErrorKind::NotFound))
    }

    fn delete_pending(&self, _id: PendingId) -> io::Result<()> {
        Ok(())
    }

    fn delete_all(&self) -> io::Result<()> {
        Ok(())
    }
}

/// `run()`'s `Indicator`: the tray half to the tray part of the app, the overlay
/// half to its overlay part (T-057). Neither waits for another thread.
pub struct ShellIndicator<R: Runtime> {
    tray: Option<TrayPart<R>>,
    overlay: Option<OverlayPart>,
}

impl<R: Runtime> ShellIndicator<R> {
    /// The indicator of `app`: its tray part (`tray::part`), if the tray was built,
    /// and its overlay part (`overlay::part`), if `assemble` wired the app.
    pub fn new(app: &AppHandle<R>) -> ShellIndicator<R> {
        ShellIndicator {
            tray: tray::part(app),
            overlay: overlay::part(app),
        }
    }
}

impl<R: Runtime> Indicator for ShellIndicator<R> {
    fn set_tray(&self, state: TrayState, retry_available: bool) {
        if let Some(tray) = &self.tray {
            tray.set_tray(state, retry_available);
        }
    }

    fn set_overlay(&self, state: &OverlayState) {
        if let Some(overlay) = &self.overlay {
            overlay.set_overlay(state);
        }
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
        SettingsRequests { app: app.clone() }
    }
}

impl<R: Runtime> ShellRequests for SettingsRequests<R> {
    fn open_settings(&self, tab: SettingsTab) {
        let _ = settings_window::request(&self.app, OpenTarget::Tab(tab, None));
    }
}
