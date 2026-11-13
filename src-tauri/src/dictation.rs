//! The dictation wiring (T-006 invariant 5; design 6): [`start_dictation`] is the
//! one function `run()` and the tests use to build the only `DictationSession` of an
//! app. It builds the session from the managed `SettingsService`, one `LogObserver`
//! over the managed `Log`, the given ports (the credential store must be the
//! `SettingsService`'s own instance), the speech gate over the given detector with the
//! energy fallback (T-102), `ChatPostProcessor` (T-074: stateless,
//! settings, key and timeouts come from each job, so the app and the tests run the
//! same processor), the given pending-audio store (T-209: `run()` passes
//! [`pending_store`] over `paths::temp_audio_dir()`; its `delete_all` runs before
//! the session starts, a failure writes one `pending_audio_cleanup_failed` line, and
//! the store is managed as
//! [`PendingAudio`] for the `RunEvent::Exit` cleanup) and [`SettingsRequests`];
//! manages it as
//! `Arc<DictationSession>` (the type the tray's menu-open handler looks up),
//! starts the hotkey thread for the settings' hotkey and attaches it to the
//! `HotkeyRegistrarHandle` the `SettingsService` was given (T-055), so a save's
//! hotkey step is served by that thread. The session's Esc claim (T-009,
//! `CancelKey`) is a `CancelKeyHandle` built here and attached to the same thread;
//! it is not a port. `run()` and the tests differ only in the ports.
//!
//! [`ShellIndicator`] is `run()`'s `Indicator`: the tray half forwards to the
//! tray's `TrayPart` (nothing when the tray was not built), the overlay half to the
//! overlay's `OverlayPart` (T-057: the newest state into the overlay mailbox, then
//! the overlay thread builds, emits or destroys the window). Both only store and
//! post.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, Manager, Runtime};
use voicen_core::diag::{
    DetectorTag, Log, LogEvent, LogObserver, LogsNotice, LogsNoticeSink, WarningKind,
};
use voicen_core::dictation::{DictationSession, SessionDeps};
use voicen_core::pending::{CallRootsFn, FsTempAudioStore};
use voicen_core::pipeline::{EngineFactory, PipelineDeps};
use voicen_core::platform::{
    AudioSource, Clipboard, Indicator, Paster, ShellRequests, TempAudioStore,
};
use voicen_core::post_process::chat::ChatPostProcessor;
use voicen_core::recording::{OverlayState, TrayState};
use voicen_core::secrets::CredentialStore;
use voicen_core::settings::gate::SettingsTab;
use voicen_core::settings::service::SettingsService;
use voicen_core::settings::FieldId;
use voicen_core::vad::{EnergyDetector, SpeechDetector, SpeechGate, VadError};

use crate::overlay::{self, OverlayPart};
use crate::settings_window::{self, OpenTarget};
use crate::tray::{self, TrayPart};
use crate::win::hotkey::{CancelKeyHandle, HotkeyRegistrarHandle, HotkeyThread};

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
    /// The `SettingsService`'s own hotkey registrar (`SettingsDeps::hotkeys`, T-055):
    /// the started hotkey thread is attached to it.
    pub hotkeys: Arc<HotkeyRegistrarHandle>,
    /// The gate's primary detector: `vad::load_bundled` in `run()`, explicit in the tests.
    /// An `Err` makes the energy detector decide (one `vad_fallback` at the first decision).
    pub detector: Result<Box<dyn SpeechDetector>, VadError>,
    /// The pending recording's audio (T-209, FR-032): [`pending_store`] over
    /// `paths::temp_audio_dir()` in `run()`.
    pub temp_audio: Arc<dyn TempAudioStore>,
}

/// The pending-audio store of `app` (T-209, FR-032): `FsTempAudioStore` over `dir`,
/// guarded by the managed `Arc<SettingsService>`'s `call_roots`, looked up through
/// `try_state` at every write and delete (the one resolution of the roots, P-010).
/// Without a managed service the roots are unknown and the store fails closed: it
/// writes and deletes nothing. The one constructor of the shipped store: `run()`
/// passes `paths::temp_audio_dir()`, the shell tests their own folder.
pub fn pending_store<R: Runtime>(app: &AppHandle<R>, dir: PathBuf) -> Arc<dyn TempAudioStore> {
    let app = app.clone();
    let roots: CallRootsFn = Arc::new(move || {
        app.try_state::<Arc<SettingsService>>()
            .and_then(|service| service.call_roots())
    });
    Arc::new(FsTempAudioStore::new(dir, roots))
}

/// The app's pending-audio store, managed by [`start_dictation`] so the
/// `RunEvent::Exit` arm of `on_run_event` can run its `delete_all` (FR-032).
pub struct PendingAudio(pub Arc<dyn TempAudioStore>);

/// The started dictation of an app: owns the hotkey thread. Dropping it stops the
/// hotkey thread and frees the hotkey (also one a save committed); the session
/// stays managed by the app, and the registrar handle fails closed again.
pub struct DictationHandle {
    _hotkey: HotkeyThread,
}

/// Builds the one dictation session of `app` over `ports`, manages it as
/// `Arc<DictationSession>`, starts the hotkey thread for the hotkey of the
/// settings snapshot (it reports the registration result before this returns) and
/// attaches it to `ports.hotkeys`. Between the two it attaches the session to the
/// managed `Arc<LogsNotice>`, if any (T-054: a logs failure latched at start is
/// shown then, before the hotkey thread can raise its own notice). Needs the managed `Arc<SettingsService>` and `Arc<Log>`
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
        hotkeys,
        detector,
        temp_audio,
    } = ports;
    // FR-032: the stale files of an earlier session go before the session exists, so
    // no put can race it. A failure (the calls-roots guard refused, an I/O error) is
    // not fatal: one warning line with the kind and the OS code only (no path, no OS
    // text, P-009), and the next start retries.
    if let Err(err) = temp_audio.delete_all() {
        log.write(LogEvent::Warning {
            kind: WarningKind::PendingAudioCleanupFailed,
            os_code: err.raw_os_error(),
        });
    }
    let _ = app.manage(PendingAudio(Arc::clone(&temp_audio)));
    let cancel_key = CancelKeyHandle::new();
    let gate = SpeechGate::new(detector, EnergyDetector::new());
    let start_line = LogEvent::VadDetector {
        detector: DetectorTag::from_name(gate.detector_name()),
    };
    let session = DictationSession::start(SessionDeps {
        pipeline: PipelineDeps {
            gate,
            credentials,
            clipboard,
            paster,
            temp_audio,
            observer: Arc::new(LogObserver::new(Arc::clone(&log))),
            post_processor: Arc::new(ChatPostProcessor::new()),
        },
        engine_factory,
        audio,
        indicator,
        requests: Arc::new(SettingsRequests::new(app)),
        settings: Arc::clone(&service),
        cancel_key: cancel_key.clone(),
    })
    .map_err(failed)?;
    let session = Arc::new(session);
    if !app.manage(Arc::clone(&session)) {
        return Err(failed(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "the app already has a dictation session",
        )));
    }
    // T-102 (#198): the one start line. The runner has no capture endpoint, so a hold
    // never reaches the gate there; this line is the install smoke's observable.
    log.write(start_line);
    // T-054: the logs notice reaches the user through this session. A failure the
    // log reported before (at start) is shown here, on this thread; it comes before
    // the hotkey thread starts, so a startup `notice.hotkey_unavailable` replaces it
    // in the one message slot.
    if let Some(logs_notice) = app.try_state::<Arc<LogsNotice>>() {
        logs_notice.attach(Arc::clone(&session) as Arc<dyn LogsNoticeSink>);
    }
    let hotkey = service.snapshot().hotkey.clone();
    let hotkey = HotkeyThread::start(session, &hotkey, log)?;
    hotkeys.attach(&hotkey);
    cancel_key.attach(&hotkey);
    Ok(DictationHandle { _hotkey: hotkey })
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

/// The session's `ShellRequests`: `open_settings(tab, field)` posts
/// `settings_window::request(app, OpenTarget::Tab(tab, field))` and drops the
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
    fn open_settings(&self, tab: SettingsTab, field: Option<FieldId>) {
        let _ = settings_window::request(&self.app, OpenTarget::Tab(tab, field));
    }
}
