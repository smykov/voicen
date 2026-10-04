//! The typed log allowlist (T-008; spec 006 data-model "LogEvent", spec 004 R-11).
//!
//! Adding a variant or a field is the only way to log something new (P-009).
//! Every field is an integer, a closed enum of this crate, or `BuildInfo`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use crate::autostart::ReconcileAction;
use crate::build_info::BuildInfo;
use crate::delivery::DeliveryResult;
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
        // Skeleton (T-008 red tests): not implemented yet.
        let _ = outcome;
        todo!("T-008: LogEvent::settings_load")
    }

    /// The save line of `outcome`; the view (settings values, key presence) is
    /// dropped, only warnings, field errors and the form error are kept.
    pub fn settings_save(outcome: &SaveOutcome) -> LogEvent {
        // Skeleton (T-008 red tests): not implemented yet.
        let _ = outcome;
        todo!("T-008: LogEvent::settings_save")
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
/// start and settings-window failures.
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
    pub fn from_kind(kind: &str) -> EngineTag {
        // Skeleton (T-008 red tests): not implemented yet.
        let _ = kind;
        todo!("T-008: EngineTag::from_kind")
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
    pub fn from_name(name: &str) -> DetectorTag {
        // Skeleton (T-008 red tests): not implemented yet.
        let _ = name;
        todo!("T-008: DetectorTag::from_name")
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
    pub fn from_code(code: &str) -> FailureTag {
        // Skeleton (T-008 red tests): not implemented yet.
        let _ = code;
        todo!("T-008: FailureTag::from_code")
    }
}
