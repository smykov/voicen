//! The typed log allowlist (T-008; spec 006 data-model "LogEvent", spec 004 R-11).
//!
//! Adding a variant or a field is the only way to log something new (P-009).
//! Every field is an integer, a closed enum of this crate, or `BuildInfo`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use crate::autostart::ReconcileAction;
use crate::build_info::BuildInfo;
use crate::delivery::DeliveryResult;
use crate::recording::MicCause;
use crate::settings::service::{FormError, SaveOutcome, Warning};
use crate::settings::{FieldError, LoadOutcome};

/// One log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogEvent {
    /// The first line of a session: `voicen <version> (<commit>) started pid=<n>`
    /// (FR-18; the CI smoke greps `(<commit>) started`).
    Started { build: BuildInfo, pid: u32 },
    /// One dictation, written by [`LogObserver`](super::LogObserver).
    Dictation(DictationLine),
    /// A warning with a closed kind and, where one exists, the OS error code.
    Warning {
        kind: WarningKind,
        os_code: Option<i32>,
    },
    /// `settings load outcome=<loaded|first_run|reset|unavailable>`.
    SettingsLoad(LoadKind),
    /// `settings save outcome=<ok|refused|failed> ...` (field ids and codes only).
    SettingsSave(SaveLine),
    /// `autostart reconcile action=<none|written|removed|failed>`.
    AutostartReconcile(ReconcileAction),
    /// The log was unwritable and is written again (written by the log itself).
    LogsRecovered,
}

impl LogEvent {
    /// The load line of `outcome`; the backup file name and the settings are
    /// dropped.
    pub fn settings_load(outcome: &LoadOutcome) -> LogEvent {
        LogEvent::SettingsLoad(match outcome {
            LoadOutcome::Loaded(_) => LoadKind::Loaded,
            LoadOutcome::FirstRun(_) => LoadKind::FirstRun,
            LoadOutcome::Reset { .. } => LoadKind::Reset,
            LoadOutcome::Unavailable(_) => LoadKind::Unavailable,
        })
    }

    /// The save line of `outcome`; the view (settings values, key presence) is
    /// dropped, only warnings, field errors and the form error are kept.
    pub fn settings_save(outcome: &SaveOutcome) -> LogEvent {
        LogEvent::SettingsSave(match outcome {
            SaveOutcome::Saved { view: _, warnings } => SaveLine::Saved {
                warnings: warnings.clone(),
            },
            SaveOutcome::Refused { errors, form_error } => SaveLine::Refused {
                errors: errors.clone(),
                form_error: form_error.clone(),
            },
        })
    }
}

/// How startup found the settings (`LoadOutcome` without its values).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadKind {
    Loaded,
    FirstRun,
    Reset,
    Unavailable,
}

/// A save outcome without its view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveLine {
    Saved {
        warnings: Vec<Warning>,
    },
    Refused {
        errors: Vec<FieldError>,
        form_error: Option<FormError>,
    },
}

/// The kinds of a `Warning` line: the pipeline's `WarningCode`s and the shell's
/// start, settings-window, tray, hotkey and dictation-start failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningKind {
    /// `events::WarningCode::VadFallback`.
    VadFallback,
    /// `events::WarningCode::EscUnavailable`.
    EscUnavailable,
    /// `events::WarningCode::ToastFailed`.
    ToastFailed,
    /// Unfinished model downloads could not be removed at start.
    ModelsCleanupFailed,
    /// The settings window could not be opened at start.
    SettingsWindowFailed,
    /// The `settings://changed` bridge thread could not be started.
    ChangeBridgeFailed,
    /// The settings window's opener thread could not be started (T-052); no
    /// settings window can open in this run.
    SettingsOpenerFailed,
    /// The tray icon could not be built at start, or a state could not be applied
    /// to it (T-052).
    TrayFailed,
    /// The tray's UI-language follower thread could not be started (T-052); the
    /// tray menu keeps its start language until restart.
    TrayFollowerFailed,
    /// The hotkey thread or its hidden window could not be started (T-006); no
    /// hotkey works in this run.
    HotkeyThreadFailed,
    /// `RegisterHotKey` refused the hotkey (T-006); the OS code says why (1409:
    /// another program holds the combination).
    HotkeyRegisterFailed,
    /// The dictation session could not be started (T-006); the app runs on
    /// without dictation.
    DictationStartFailed,
}

/// One dictation: the engine, the outcome and the FR-20 timings plus the
/// recording's duration. A timing whose event did not come is `None` and its key
/// is left out of the line (never `0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DictationLine {
    /// `RecordingId::get()`.
    pub recording: u64,
    pub engine: Option<EngineTag>,
    pub outcome: DictationOutcome,
    pub detector: Option<DetectorTag>,
    /// Hotkey press -> first audio frame (`RecordingStarted`).
    pub press_to_frame_ms: Option<u64>,
    /// The recording's length (`RecordingEnded`).
    pub duration_ms: Option<u64>,
    /// Stop -> outcome (`JobFinished`).
    pub stop_to_text_ms: Option<u64>,
    /// Text -> paste (`Delivered`).
    pub text_to_paste_ms: Option<u64>,
}

/// How a dictation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictationOutcome {
    Delivered(DeliveryResult),
    Failed {
        failure: FailureTag,
        http_status: Option<u16>,
    },
    NoSpeech,
    TooShort,
    /// The capture failed at press or at stop (`DictationEvent::CaptureFailed`,
    /// T-051); only the closed `MicCause`, which carries no OS text.
    CaptureFailed {
        cause: MicCause,
    },
}

/// `Engine::kind()` through the table: `api`, `builtin`, `local_server`, else
/// `other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineTag {
    Api,
    Builtin,
    LocalServer,
    Other,
}

impl EngineTag {
    /// Exact match on the whole string; anything else (a near miss, an injected
    /// `key=value`, the empty string) is `Other`.
    pub fn from_kind(kind: &str) -> EngineTag {
        match kind {
            "api" => EngineTag::Api,
            "builtin" => EngineTag::Builtin,
            "local_server" => EngineTag::LocalServer,
            _ => EngineTag::Other,
        }
    }
}

/// `SpeechDetector::name()` through the table: `silero`, `energy`, else `other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectorTag {
    Silero,
    Energy,
    Other,
}

impl DetectorTag {
    /// Exact match on the whole string; anything else is `Other`.
    pub fn from_name(name: &str) -> DetectorTag {
        match name {
            "silero" => DetectorTag::Silero,
            "energy" => DetectorTag::Energy,
            _ => DetectorTag::Other,
        }
    }
}

/// `FailureReason::code()` through the table; an unknown code is `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureTag {
    InvalidApiKey,
    NetworkUnavailable,
    CannotReach,
    Timeout,
    ServerError,
    UnexpectedResponse,
    KeyStoreUnavailable,
    EngineNotConfigured,
    ClipboardUnavailable,
    MicrophoneUnavailable,
    Other,
}

impl FailureTag {
    /// Exact match on `FailureReason::code()`'s spellings; anything else is
    /// `Other`. `tests/diag_format.rs` pins one tag per reason, so a new reason or
    /// a renamed code fails there instead of logging `other`.
    pub fn from_code(code: &str) -> FailureTag {
        match code {
            "InvalidApiKey" => FailureTag::InvalidApiKey,
            "NetworkUnavailable" => FailureTag::NetworkUnavailable,
            "CannotReach" => FailureTag::CannotReach,
            "Timeout" => FailureTag::Timeout,
            "ServerError" => FailureTag::ServerError,
            "UnexpectedResponse" => FailureTag::UnexpectedResponse,
            "KeyStoreUnavailable" => FailureTag::KeyStoreUnavailable,
            "EngineNotConfigured" => FailureTag::EngineNotConfigured,
            "ClipboardUnavailable" => FailureTag::ClipboardUnavailable,
            "MicrophoneUnavailable" => FailureTag::MicrophoneUnavailable,
            _ => FailureTag::Other,
        }
    }
}
