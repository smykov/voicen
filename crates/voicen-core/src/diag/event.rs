//! The typed log allowlist (T-008; spec 006 data-model "LogEvent", spec 004 R-11).
//!
//! Adding a variant or a field is the only way to log something new (P-009).
//! Every field is an integer, a closed enum of this crate, `BuildInfo`, or a validated
//! `call::EndpointId` (T-118).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::crash::{CrashFileName, OsVersion};
use crate::autostart::ReconcileAction;
use crate::build_info::BuildInfo;
use crate::call::{
    CallSource, DeviceCause, EndpointId, FinalOutcome, FinalStage, InvalidRule, PassMode,
    RefusedPart, RefusedReason, SilentRule, StopReason, StorageKind, SwitchRefusal, Track,
    VadFallbackCause,
};
use crate::connection_test::ConnectionTestResult;
use crate::delivery::DeliveryResult;
use crate::events::PostProcessTrace;
use crate::local_models::residency::Warmth;
use crate::recording::MicCause;
use crate::settings::gate::Blocked;
use crate::settings::service::{FormError, SaveOutcome, Warning};
use crate::settings::{FieldError, LoadOutcome};

/// One log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogEvent {
    /// The first line of a session:
    /// `voicen <version> (<commit>) started pid=<n> os=<major.minor.build>`
    /// (FR-18, spec 006 FR-003; the CI smoke greps `(<commit>) started`). `os` is
    /// always written; an unreadable OS version is [`OsVersion::UNKNOWN`] (`0.0.0`).
    Started {
        build: BuildInfo,
        pid: u32,
        os: OsVersion,
    },
    /// One dictation, written by [`LogObserver`](super::LogObserver).
    Dictation(DictationLine),
    /// A hotkey press the dictation gate blocked (T-006):
    /// `dictation outcome=blocked reason=<no_engine>`, written by
    /// [`LogObserver`](super::LogObserver). Not a recording, so no `rec=`.
    DictationBlocked { reason: Blocked },
    /// A warning with a closed kind and, where one exists, the OS error code.
    Warning {
        kind: WarningKind,
        os_code: Option<i32>,
    },
    /// `settings load outcome=<loaded|first_run|reset|unavailable>`.
    SettingsLoad(LoadKind),
    /// `settings save outcome=<ok|refused|failed> ...` (field ids and codes only).
    SettingsSave(SaveLine),
    /// `settings test_connection result=<...> [latency_ms=<n>]` (spec 004 R-11,
    /// T-046 choice (iv)): the result kind only, never the host or field errors.
    SettingsTestConnection(TestConnectionLine),
    /// `autostart reconcile action=<none|written|removed|failed>`.
    AutostartReconcile(ReconcileAction),
    /// The log was unwritable and is written again (written by the log itself).
    LogsRecovered,
    /// The verdict on the previous session (T-100, research R7), written by
    /// [`Session::begin`](super::Session::begin):
    /// `previous session outcome=<crashed|ended_by_installer|ended_abnormally>
    /// [crash_file=<name>]`. A clean previous session has no line (no variant).
    PreviousSession(PreviousSessionLine),
    /// A clean end of the session (`Session::end_clean`, T-100): `exited`.
    Exited,
    // ---- T-118: the call lines (contracts/core-call.md "Log lines"). Never the call
    // folder, a device name, a path or segment text.
    /// `call_requested source=<tray|hotkey|switch>`.
    CallRequested { source: CallSource },
    /// `call_refused part=<…> reason=<…>` (WARN).
    CallRefused {
        part: RefusedPart,
        reason: RefusedReason,
    },
    /// `call_started record=1 subtitles=0 mic=<EndpointId> system=<EndpointId>
    /// comms_differs=<0|1>` (the 1a mode is fixed: record on, subtitles off).
    CallStarted {
        mic: EndpointId,
        system: EndpointId,
        comms_differs: bool,
    },
    /// `call_device track=<…> cause=<…> gap_ms=<n> to=<EndpointId|none>` (WARN on `none`).
    CallDeviceChanged {
        track: Track,
        cause: DeviceCause,
        gap_ms: u64,
        to: Option<EndpointId>,
    },
    /// `call_track_silent track=<…> rule=<lost|cant_hear>` (WARN).
    CallTrackSilent { track: Track, rule: SilentRule },
    /// `call_stopped reason=<StopReason> duration_ms=<n> mic_bytes=<n> system_bytes=<n>`.
    CallStopped {
        reason: StopReason,
        duration_ms: u64,
        mic_bytes: u64,
        system_bytes: u64,
    },
    /// `call_storage_failed kind=<…> [os=<code>]` (WARN); `os` is left out when unknown.
    CallStorageFailed { kind: StorageKind, os: Option<i32> },
    /// `call_recovered calls=<n>`.
    CallRecovered { calls: u32 },
    /// `final_stage track=<…> stage=<…> ms=<n> chunks=<n>`.
    FinalPassStage {
        track: Track,
        stage: FinalStage,
        ms: u64,
        chunks: u32,
    },
    /// `final_finished outcome=<ready|failed> [reason=<FailReason>] mode=<timed|timeless>
    /// ms=<n>`; `reason` only on `failed` (WARN).
    FinalPassFinished {
        outcome: FinalOutcome,
        mode: PassMode,
        ms: u64,
    },
    /// `call_switch_refused reason=unknown_value` (WARN).
    CallSwitchRefused { reason: SwitchRefusal },
    /// `vad_fallback track=<…> cause=<unavailable|error>` (WARN; #188).
    VadFallback {
        track: Track,
        cause: VadFallbackCause,
    },
    /// `transcript_invalid rule=<…> segment=<n>` (WARN): the index, never the text.
    TranscriptInvalid { rule: InvalidRule, segment: u32 },
    /// `vad detector=<silero|energy|other>` (T-102, decision #198): the one start line of
    /// a process that starts dictation, naming the detector its speech gate holds. A
    /// closed tag, never the model's path.
    VadDetector { detector: DetectorTag },
}

/// A logged previous-session verdict (T-100): `PreviousSession` without `Clean`.
/// The file name is a [`CrashFileName`], held as integers and rendered by diag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviousSessionLine {
    /// A crash file of the previous session exists (no new record): WARN.
    Crashed { crash_file: CrashFileName },
    /// The installer ended it: INFO, no file.
    EndedByInstaller,
    /// It ended without a clean exit or a crash file; the new `abnormal_end`
    /// file, when it could be written: WARN.
    EndedAbnormally { crash_file: Option<CrashFileName> },
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

    /// The test-connection line of `result`; the host of `CannotReach` and the
    /// field errors of `Invalid` are dropped (T-008 Q1: no hosts, no values).
    pub fn settings_test_connection(result: &ConnectionTestResult) -> LogEvent {
        LogEvent::SettingsTestConnection(match result {
            ConnectionTestResult::Ok { latency_ms } => TestConnectionLine::Ok {
                latency_ms: *latency_ms,
            },
            ConnectionTestResult::CannotReach { host: _ } => TestConnectionLine::CannotReach,
            ConnectionTestResult::InvalidKey => TestConnectionLine::InvalidKey,
            ConnectionTestResult::Timeout => TestConnectionLine::Timeout,
            ConnectionTestResult::Http { status } => TestConnectionLine::Http { status: *status },
            ConnectionTestResult::UnexpectedResponse => TestConnectionLine::Unexpected,
            ConnectionTestResult::Invalid { errors: _ } => TestConnectionLine::Invalid,
            ConnectionTestResult::KeyStoreUnavailable => TestConnectionLine::KeyStoreUnavailable,
        })
    }
}

/// A connection test result without its host or field errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestConnectionLine {
    Ok { latency_ms: u64 },
    CannotReach,
    InvalidKey,
    Timeout,
    Http { status: u16 },
    Unexpected,
    Invalid,
    KeyStoreUnavailable,
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
/// start, settings-window, tray, hotkey, dictation-start, overlay and
/// logs-folder failures.
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
    /// The overlay window could not be built, emitted to or destroyed, or the
    /// overlay thread could not be started (T-057); the dictation itself goes on.
    OverlayFailed,
    /// The tray's "Open logs folder" could not create, check or open the logs
    /// folder, or its thread could not be started (T-071); the app keeps running.
    LogsFolderFailed,
    /// A `settings_test_connection` call could not finish: its blocking task
    /// panicked or was cancelled (T-046), so no `settings test_connection` line was
    /// written; the window gets `ipc.unavailable`.
    TestConnectionFailed,
}

/// One dictation: the engine, the outcome and the FR-20 timings plus the
/// recording's duration and the post-processing trace (spec 003 FR-014). A timing whose event did not come is `None` and its key
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
    /// The post-processing stage's result and duration (`JobFinished`, T-076);
    /// `None` when the job never called the stage: no `pp*` key is written.
    pub post_processing: Option<PostProcessTrace>,
    /// The built-in engine's model warmth (`JobFinished.local`, spec 002 FR-015,
    /// T-098): `local=cold load_ms=<n>` or `local=warm`, written only on an
    /// `engine=builtin` line; `None` writes neither key.
    pub local: Option<Warmth>,
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
    /// Esc cancelled the recording (T-009, FR-22): nothing was sent.
    Cancelled,
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
    NoLocalModel,
    LocalEngineFailed,
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
            "NoLocalModel" => FailureTag::NoLocalModel,
            "LocalEngineFailed" => FailureTag::LocalEngineFailed,
            _ => FailureTag::Other,
        }
    }
}
