//! English/Russian message catalog shared by the shell and the UI (FR-15, T-005).
//!
//! The catalog lives at the repository root (`i18n/en.json`, `i18n/ru.json`,
//! decision #13): two flat JSON maps of id -> text. This module is the only Rust
//! code that parses, looks up and renders them. Lookup and rendering
//! (`Catalog::text` / `render`) follow the same rule as the UI's `lookup + render`
//! in `src/lib/i18n`; the shared fixture `i18n/conformance.json` pins that rule in
//! both suites. The embedded [`text`] adds one Rust-only step on top, nested-id
//! resolution (below): the fixture has no case for it and the UI's `t()` does not
//! resolve nested ids, so params carrying a nested id render the same only in the
//! shell. The UI does not mirror it: such a message crosses IPC as text rendered
//! here (the overlay: [`crate::overlay::overlay_payload`]; decision #64).
//!
//! - Lookup: the text in the requested language if non-empty, else the English
//!   text if non-empty, else the id itself.
//! - Placeholders: `{name}` with `name` matching `[a-z][a-z0-9_]*`; no escaping.
//!   Any other `{` or `}` is a stray brace (rendered as is, reported by the
//!   test-only check `Catalog::parity_problems`).
//! - Rendering: one left-to-right pass. A placeholder with an argument is replaced
//!   by the value inserted literally (never re-expanded); a placeholder without an
//!   argument stays verbatim; extra arguments are ignored.
//! - Nested ids (Rust [`text`] only): an argument whose value is the id of a
//!   message declared in the `nested:` group of [`messages!`] (`mic_reason.*`) is
//!   first replaced by that message's text in the same language, rendered without
//!   arguments, one level deep. Any other value, including the id of a message
//!   outside that group, is inserted as is. So a lang-independent parameter such
//!   as `FailureReason::message_params`'s `reason` renders in the user's
//!   language, and no `&str` id outside [`MESSAGE_IDS`] is ever looked up.

use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::BTreeSet;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// UI language; serialized as `"en"` / `"ru"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiLanguage {
    En,
    Ru,
}

/// An id of a message that originates in Rust (shell text or ids sent over IPC).
///
/// Constructed only by the [`messages!`] declaration below, so every Rust id is
/// listed in [`MESSAGE_IDS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageId(&'static str);

/// On the wire (IPC) a `MessageId` is its catalog id string (`"settings.write_failed"`);
/// the UI renders it with its own `t(id)`.
impl Serialize for MessageId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

/// Declares `MessageId` constants and generates `MESSAGE_IDS` from the same list,
/// so a Rust id cannot exist without being checked against both catalogs.
///
/// Ids after `nested:` are texts used as an argument value of another message
/// (see the module docs); they are in `MESSAGE_IDS` too, and `NESTED_IDS` is
/// generated from that group.
///
/// ```text
/// messages! {
///     /// Tray menu item.
///     TRAY_EXIT = "tray.exit",
///     ;
///     nested:
///     /// `{reason}` of `failure.microphone_unavailable`.
///     MIC_REASON_BUSY = "mic_reason.busy",
/// }
/// ```
macro_rules! messages {
    (
        $($(#[$meta:meta])* $name:ident = $id:literal),* $(,)?
        ;
        nested: $($(#[$nmeta:meta])* $nname:ident = $nid:literal),* $(,)?
    ) => {
        $(
            $(#[$meta])*
            pub const $name: MessageId = MessageId($id);
        )*
        $(
            $(#[$nmeta])*
            pub const $nname: MessageId = MessageId($nid);
        )*

        /// Every `MessageId` declared in this module (generated from the same
        /// declaration).
        pub const MESSAGE_IDS: &[MessageId] = &[$($name,)* $($nname),*];

        /// The ids [`text`] resolves when they arrive as an argument value
        /// (the `nested:` group of the same declaration).
        const NESTED_IDS: &[MessageId] = &[$($nname),*];
    };
}

// Rust-originated ids. Each one must exist in both i18n/en.json and i18n/ru.json
// (test `message_ids_exist_in_both_catalogs`).
messages! {
    /// Hotkey pressed while engine = none (spec 004 FR-007; `settings::gate::blocked_actions`).
    NOTICE_CHOOSE_ENGINE = "notice.choose_engine",
    /// The hotkey could not be registered (taken by another program or reserved by
    /// Windows): the overlay notice of `DictationSession::hotkey_registration(false)`
    /// (T-055), shown before settings open on the hotkey field.
    NOTICE_HOTKEY_UNAVAILABLE = "notice.hotkey_unavailable",
    /// A save refused because the settings could not be read at startup (decision
    /// #19; `settings::service::FormError::SettingsUnavailable`).
    NOTICE_SETTINGS_UNAVAILABLE = "notice.settings_unavailable",
    /// The settings file could not be read and was moved aside; the defaults are in
    /// use (`LoadOutcome::Reset`). Shown by the settings window from
    /// `SettingsView.reset_notice` and by T-006's notifier.
    NOTICE_SETTINGS_RESET = "notice.settings_reset",
    /// A save refused because `settings.json` could not be written; everything was
    /// restored (`FormError::WriteFailed`).
    SETTINGS_WRITE_FAILED = "settings.write_failed",
    /// A refused save whose undo failed for some keys; the UI highlights the key
    /// fields named in `FormError::PartiallyRestored` (R-3).
    SETTINGS_PARTIALLY_RESTORED = "settings.partially_restored",
    /// A saved base URL in use is `http` on a non-loopback host: the key travels
    /// unencrypted (`WarningCode::EndpointInsecure`, T-015; decision #52).
    SETTINGS_WARNING_ENDPOINT_INSECURE = "settings.warning.endpoint_insecure",
    /// `FailureReason::InvalidApiKey` (HTTP 401/403).
    FAILURE_INVALID_API_KEY = "failure.invalid_api_key", // a catalog id, not a key: teamwright:allow-secret
    /// `FailureReason::NetworkUnavailable` (DNS failure, network/host unreachable).
    FAILURE_NETWORK_UNAVAILABLE = "failure.network_unavailable",
    /// `FailureReason::CannotReach`; placeholder `{host}` = `host[:port]`.
    FAILURE_CANNOT_REACH = "failure.cannot_reach",
    /// `FailureReason::Timeout` from an HTTP engine (`TimeoutOrigin::Server`: the
    /// whole request exceeded its duration).
    FAILURE_TIMEOUT = "failure.timeout",
    /// `FailureReason::ServerError`; placeholder `{status}` = the HTTP status.
    FAILURE_SERVER_ERROR = "failure.server_error",
    /// `FailureReason::UnexpectedResponse` (bad, oversized or cut-off body).
    FAILURE_UNEXPECTED_RESPONSE = "failure.unexpected_response",
    /// `FailureReason::KeyStoreUnavailable` (credential read failed; decision #44).
    FAILURE_KEY_STORE_UNAVAILABLE = "failure.key_store_unavailable",
    /// `FailureReason::EngineNotConfigured` (no engine for the settings; decision #44).
    FAILURE_ENGINE_NOT_CONFIGURED = "failure.engine_not_configured",
    /// `FailureReason::MicrophoneUnavailable` (T-042); placeholder `{reason}` = a
    /// `mic_reason.*` id, rendered in the same language.
    FAILURE_MICROPHONE_UNAVAILABLE = "failure.microphone_unavailable",
    /// `FailureReason::ClipboardUnavailable` (the clipboard write failed; T-001).
    FAILURE_CLIPBOARD_UNAVAILABLE = "failure.clipboard_unavailable",
    /// `FailureReason::NoLocalModel` (the built-in engine has no downloaded model
    /// for the settings; T-098, spec 002 FR-011).
    FAILURE_NO_LOCAL_MODEL = "failure.no_local_model",
    /// `FailureReason::LocalEngineFailed` (the built-in engine's load or
    /// transcription failed; T-098).
    FAILURE_LOCAL_ENGINE = "failure.local_engine",
    /// `FailureReason::Timeout` from the built-in engine (`TimeoutOrigin::Builtin`:
    /// the transcription ran past `timeouts.builtin`; T-106, decision #147).
    FAILURE_LOCAL_TIMEOUT = "failure.local_timeout",
    /// A job without speech (`JobEnd::Notice`, T-001).
    NOTICE_NO_SPEECH = "notice.no_speech",
    /// A recording stopped at `recording::MAX_LENGTH` and sent on (T-009, FR-03).
    NOTICE_MAX_LENGTH = "notice.max_length",
    /// The selected microphone is absent and the press records from the Windows
    /// default instead (T-012, FR-27): shown once per change of the fallback
    /// device; placeholder `{device}` = that device's display name.
    NOTICE_MIC_FALLBACK = "notice.mic_fallback",
    /// Delivered with auto-paste off (`DeliveryResult::CopiedOnly`, T-001).
    NOTICE_COPIED = "notice.copied",
    /// Delivered but not pasted (`DeliveryResult::CopyManual`, T-001).
    NOTICE_COPIED_PASTE_MANUALLY = "notice.copied_paste_manually",
    /// `SkipReason::Timeout`: post-processing exceeded its deadline (T-020).
    NOTICE_POST_PROCESSING_SKIPPED_TIMEOUT = "notice.post_processing_skipped.timeout",
    /// `SkipReason::Unreachable`; placeholder `{host}` = `host[:port]` (T-020).
    NOTICE_POST_PROCESSING_SKIPPED_UNREACHABLE = "notice.post_processing_skipped.unreachable",
    /// `SkipReason::InvalidKey` (HTTP 401/403 or an unusable key; T-020).
    NOTICE_POST_PROCESSING_SKIPPED_INVALID_KEY = "notice.post_processing_skipped.invalid_key", // a catalog id, not a key: teamwright:allow-secret
    /// `SkipReason::Http`; placeholder `{status}` = the HTTP status (T-020).
    NOTICE_POST_PROCESSING_SKIPPED_HTTP = "notice.post_processing_skipped.http",
    /// `SkipReason::InvalidResponse` (empty, bad or oversized reply; T-020).
    NOTICE_POST_PROCESSING_SKIPPED_INVALID_RESPONSE = "notice.post_processing_skipped.invalid_response",
    /// `SkipReason::NotConfigured` (enabled with an unusable base URL; #91(2), T-020).
    NOTICE_POST_PROCESSING_SKIPPED_NOT_CONFIGURED = "notice.post_processing_skipped.not_configured",
    /// `DownloadFailure::DownloadInterrupted` (body cut, connection dropped, or no
    /// data for `Timeouts::download_no_data`; T-016).
    DOWNLOAD_INTERRUPTED = "download.interrupted",
    /// `DownloadFailure::ChecksumMismatch` (all bytes arrived, SHA-256 not the
    /// pinned one).
    DOWNLOAD_CHECKSUM_MISMATCH = "download.checksum_mismatch",
    /// `DownloadFailure::NotEnoughDiskSpace`; placeholder `{needed}` = bytes.
    DOWNLOAD_NOT_ENOUGH_DISK_SPACE = "download.not_enough_disk_space",
    /// `DownloadFailure::SourceUnreachable`; placeholder `{host}` = `host[:port]`.
    DOWNLOAD_SOURCE_UNREACHABLE = "download.source_unreachable",
    /// `DownloadFailure::DiskError` (the `.part` file could not be created,
    /// written or renamed).
    DOWNLOAD_DISK_ERROR = "download.disk_error",
    /// `DownloadFailure::HttpStatus`; placeholder `{code}` = the HTTP status.
    DOWNLOAD_HTTP_STATUS = "download.http_status",
    /// `DownloadError::Busy`, wire code `download_busy`: another download runs
    /// (T-044).
    DOWNLOAD_BUSY = "download.busy",
    /// `DownloadError::AlreadyDownloaded`, wire code `already_downloaded` (T-044).
    DOWNLOAD_ALREADY_DOWNLOADED = "download.already_downloaded",
    /// `DownloadError::NotInCatalog` or an id string `ModelId::parse` rejects, wire
    /// code `not_in_catalog` (T-044).
    DOWNLOAD_NOT_IN_CATALOG = "download.not_in_catalog",
    /// `DownloadError::CannotStart` (the OS refused the download thread), wire code
    /// `download_cannot_start` (T-044).
    DOWNLOAD_CANNOT_START = "download.cannot_start",
    /// `DeleteError::ModelInUse`, wire code `model_in_use`: a transcription holds
    /// the model (T-019).
    DELETE_MODEL_IN_USE = "delete.model_in_use",
    /// `DeleteError::NotDownloaded`, wire code `not_downloaded` (T-019).
    DELETE_NOT_DOWNLOADED = "delete.not_downloaded",
    /// `DeleteError::DeleteFailed`, wire code `delete_failed`: the file could not
    /// be removed (T-019).
    DELETE_FAILED = "delete.failed",
    /// `LocalModelView.nameKey` of `tiny` (T-044).
    LOCAL_MODEL_NAME_TINY = "local_model.name.tiny",
    /// `LocalModelView.nameKey` of `base` (T-044).
    LOCAL_MODEL_NAME_BASE = "local_model.name.base",
    /// `LocalModelView.nameKey` of `small` (T-044).
    LOCAL_MODEL_NAME_SMALL = "local_model.name.small",
    /// `LocalModelView.nameKey` of `medium-q5_0` (T-044).
    LOCAL_MODEL_NAME_MEDIUM_Q5_0 = "local_model.name.medium-q5_0",
    /// `LocalModelView.nameKey` of `large-v3-turbo-q5_0` (T-044).
    LOCAL_MODEL_NAME_LARGE_V3_TURBO_Q5_0 = "local_model.name.large-v3-turbo-q5_0",
    /// Tray menu item `TrayAction::OpenSettings` (T-052).
    TRAY_SETTINGS = "tray.settings",
    /// Tray menu item `TrayAction::OpenLogs` (T-071).
    TRAY_OPEN_LOGS = "tray.open_logs",
    /// Tray menu item `TrayAction::Exit` (T-052).
    TRAY_EXIT = "tray.exit",
    /// Tray tooltip of `TrayState::Idle` (T-052).
    TRAY_TOOLTIP_IDLE = "tray.tooltip.idle",
    /// Tray tooltip of `TrayState::Recording` (T-052).
    TRAY_TOOLTIP_RECORDING = "tray.tooltip.recording",
    /// Tray tooltip of `TrayState::Error` (T-052).
    TRAY_TOOLTIP_ERROR = "tray.tooltip.error",
    /// Tray tooltip of `TrayState::HotkeyError` (T-052).
    TRAY_TOOLTIP_HOTKEY_ERROR = "tray.tooltip.hotkey_error",
    /// The log cannot be written (T-054, spec 006 FR-009): shown once per process
    /// through `diag::LogsNotice` and `DictationSession::notice_with`; placeholder
    /// `{reason}` = a `logs_reason.*` id, rendered in the same language.
    NOTICE_LOGS_UNWRITABLE = "notice.logs_unwritable",
    // ---- Call mode (T-119, spec 007 contracts/ipc.md "Message catalog", #197, #198).
    // The enum-to-id mapping of refusals, notices and warnings is T-126's. ----
    /// Tray submenu `Call` (T-122).
    TRAY_CALL = "tray.call",
    /// Tray item `Call › Record` (T-122).
    TRAY_CALL_RECORD = "tray.call.record",
    /// Tray item `Call › Stop` (T-122).
    TRAY_CALL_STOP = "tray.call.stop",
    /// Tray item `Calls`, opens the Calls window (T-122).
    TRAY_CALLS = "tray.calls",
    /// Tray tooltip while a call is recording (#197).
    TRAY_TOOLTIP_CALL_RECORDING = "tray.tooltip.call_recording",
    /// `IndicatorState.reminderText`, rendered in Rust.
    INDICATOR_REMINDER = "indicator.reminder",
    /// `IndicatorState.headphonesHint`, rendered in Rust.
    INDICATOR_HEADPHONES = "indicator.headphones",
    /// `IndicatorWarning.text`: calls may use another output device than the recorded one.
    WARNING_COMMUNICATIONS_DEVICE = "warning.communications_device",
    /// `IndicatorWarning.text`: `Track::Mic` lost, silence is being recorded.
    WARNING_TRACK_LOST_MIC = "warning.track_lost.mic",
    /// `IndicatorWarning.text`: `Track::System` lost, silence is being recorded.
    WARNING_TRACK_LOST_SYSTEM = "warning.track_lost.system",
    /// `IndicatorWarning.text`: the mic track moved to the Windows default.
    WARNING_MIC_CHANGED = "warning.mic_changed",
    /// `IndicatorWarning.text`: the system track stays silent.
    WARNING_CANT_HEAR_OTHER_PARTY = "warning.cant_hear_other_party",
    /// `IndicatorWarning.text`: the mic track stays silent.
    WARNING_CANT_HEAR_YOU = "warning.cant_hear_you",
    /// `IndicatorWarning.text`: under 1 GB free in the calls folder.
    WARNING_LOW_DISK = "warning.low_disk",
    /// `IndicatorWarning.text` of kind `providerNotice`; placeholder `{host}` =
    /// `host[:port]`.
    WARNING_PROVIDER_NOTICE = "warning.provider_notice",
    /// `RefusedPart::Mic` x `RefusedReason::NoDevice`.
    NOTICE_CALL_REFUSED_MIC_NO_DEVICE = "notice.call_refused.mic.no_device",
    /// `RefusedPart::Mic` x `RefusedReason::AccessDenied`.
    NOTICE_CALL_REFUSED_MIC_ACCESS_DENIED = "notice.call_refused.mic.access_denied",
    /// `RefusedPart::Mic` x `RefusedReason::Busy`.
    NOTICE_CALL_REFUSED_MIC_BUSY = "notice.call_refused.mic.busy",
    /// `RefusedPart::Mic` x `RefusedReason::Unwritable`.
    NOTICE_CALL_REFUSED_MIC_UNWRITABLE = "notice.call_refused.mic.unwritable",
    /// `RefusedPart::Mic` x `RefusedReason::Missing`.
    NOTICE_CALL_REFUSED_MIC_MISSING = "notice.call_refused.mic.missing",
    /// `RefusedPart::Mic` x `RefusedReason::Other`.
    NOTICE_CALL_REFUSED_MIC_OTHER = "notice.call_refused.mic.other",
    /// `RefusedPart::System` x `RefusedReason::NoDevice`.
    NOTICE_CALL_REFUSED_SYSTEM_NO_DEVICE = "notice.call_refused.system.no_device",
    /// `RefusedPart::System` x `RefusedReason::AccessDenied`.
    NOTICE_CALL_REFUSED_SYSTEM_ACCESS_DENIED = "notice.call_refused.system.access_denied",
    /// `RefusedPart::System` x `RefusedReason::Busy`.
    NOTICE_CALL_REFUSED_SYSTEM_BUSY = "notice.call_refused.system.busy",
    /// `RefusedPart::System` x `RefusedReason::Unwritable`.
    NOTICE_CALL_REFUSED_SYSTEM_UNWRITABLE = "notice.call_refused.system.unwritable",
    /// `RefusedPart::System` x `RefusedReason::Missing`.
    NOTICE_CALL_REFUSED_SYSTEM_MISSING = "notice.call_refused.system.missing",
    /// `RefusedPart::System` x `RefusedReason::Other`.
    NOTICE_CALL_REFUSED_SYSTEM_OTHER = "notice.call_refused.system.other",
    /// `RefusedPart::Root` x `RefusedReason::NoDevice`.
    NOTICE_CALL_REFUSED_ROOT_NO_DEVICE = "notice.call_refused.root.no_device",
    /// `RefusedPart::Root` x `RefusedReason::AccessDenied`.
    NOTICE_CALL_REFUSED_ROOT_ACCESS_DENIED = "notice.call_refused.root.access_denied",
    /// `RefusedPart::Root` x `RefusedReason::Busy`.
    NOTICE_CALL_REFUSED_ROOT_BUSY = "notice.call_refused.root.busy",
    /// `RefusedPart::Root` x `RefusedReason::Unwritable`.
    NOTICE_CALL_REFUSED_ROOT_UNWRITABLE = "notice.call_refused.root.unwritable",
    /// `RefusedPart::Root` x `RefusedReason::Missing`.
    NOTICE_CALL_REFUSED_ROOT_MISSING = "notice.call_refused.root.missing",
    /// `RefusedPart::Root` x `RefusedReason::Other`.
    NOTICE_CALL_REFUSED_ROOT_OTHER = "notice.call_refused.root.other",
    /// A call was saved and its final pass is queued (T-139).
    NOTICE_CALL_SAVED = "notice.call_saved",
    /// A final pass ended ready (T-139).
    NOTICE_TRANSCRIPT_READY = "notice.transcript_ready",
    /// The action of the transcript-ready toast (T-139).
    NOTICE_OPEN = "notice.open",
    /// A final pass failed: `FailReason::Key`.
    NOTICE_PASS_FAILED_KEY = "notice.pass_failed.key", // a catalog id, not a key: teamwright:allow-secret
    /// A final pass failed: `FailReason::Network`.
    NOTICE_PASS_FAILED_NETWORK = "notice.pass_failed.network",
    /// A final pass failed: `FailReason::Timeout`.
    NOTICE_PASS_FAILED_TIMEOUT = "notice.pass_failed.timeout",
    /// A final pass failed: `FailReason::Provider`.
    NOTICE_PASS_FAILED_PROVIDER = "notice.pass_failed.provider",
    /// A final pass failed: `FailReason::Storage`.
    NOTICE_PASS_FAILED_STORAGE = "notice.pass_failed.storage",
    /// A final pass failed: `FailReason::EngineNotSetUp`.
    NOTICE_PASS_FAILED_ENGINE_NOT_SET_UP = "notice.pass_failed.engine_not_set_up",
    /// Recording stopped: `StopReason::Sleep`.
    NOTICE_STOPPED_SLEEP = "notice.stopped.sleep",
    /// Recording stopped: `StopReason::MaxLength`.
    NOTICE_STOPPED_MAX_LENGTH = "notice.stopped.max_length",
    /// Recording stopped: `StopReason::LowDisk`.
    NOTICE_STOPPED_LOW_DISK = "notice.stopped.low_disk",
    /// Recording stopped: `StopReason::StorageError`.
    NOTICE_STOPPED_STORAGE = "notice.stopped.storage",
    /// The call hotkey could not be registered (T-129).
    NOTICE_CALL_HOTKEY_CONFLICT = "notice.call_hotkey_conflict",
    /// The question of the exit dialog while a call records (T-145).
    EXIT_RECORDING_QUESTION = "exit.recording.question",
    /// Default `Call.title`, rendered once at creation (T-125); placeholders `{day}`,
    /// `{month}`, `{hour}`, `{minute}` = two-digit values the caller pads.
    CALLS_TITLE_DEFAULT = "calls.title_default",
    /// Default name of the mic speaker, rendered once at creation (T-125).
    SPEAKER_ME = "speaker.me",
    /// Default name of the system-track speaker, rendered once at creation (T-125).
    SPEAKER_OTHER = "speaker.other",
    ;
    nested:
    /// `MicCause::NoDevice`.
    MIC_REASON_NO_DEVICE = "mic_reason.no_device",
    /// `MicCause::AccessDenied`.
    MIC_REASON_ACCESS_DENIED = "mic_reason.access_denied",
    /// `MicCause::Busy`.
    MIC_REASON_BUSY = "mic_reason.busy",
    /// `MicCause::Other`.
    MIC_REASON_OTHER = "mic_reason.other",
    /// `diag::LogsUnwritableReason::PermissionDenied`.
    LOGS_REASON_PERMISSION_DENIED = "logs_reason.permission_denied",
    /// `diag::LogsUnwritableReason::DiskFull`.
    LOGS_REASON_DISK_FULL = "logs_reason.disk_full",
    /// `diag::LogsUnwritableReason::NotADirectory`.
    LOGS_REASON_NOT_A_DIRECTORY = "logs_reason.not_a_directory",
    /// `diag::LogsUnwritableReason::Other` (any OS code; the code is not shown).
    LOGS_REASON_OTHER = "logs_reason.other",
}

impl MessageId {
    /// This id as an argument value of another message; [`text`] renders it in
    /// the same language when the id is in the `nested:` group of [`messages!`].
    pub(crate) fn nested_arg(self) -> String {
        self.0.to_string()
    }
}

/// Both catalogs (en, ru) as parsed flat id -> text maps.
///
/// Crate-private: `Catalog::text` takes a `&str` id, so outside this crate catalog
/// text is rendered only through [`text`], which takes a [`MessageId`].
#[derive(Debug, Default)]
pub(crate) struct Catalog {
    en: BTreeMap<String, String>,
    ru: BTreeMap<String, String>,
}

/// A catalog file that is not a flat JSON object of strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CatalogError {
    /// Which file is broken.
    pub lang: UiLanguage,
    pub message: String,
}

/// One invariant violation, naming the offending id (test-only check).
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct CatalogProblem {
    pub id: String,
    pub kind: ProblemKind,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProblemKind {
    /// The id is absent from this language's catalog.
    Missing(UiLanguage),
    /// The id's text is empty in this language's catalog.
    Empty(UiLanguage),
    /// en and ru texts of the id use different sets of placeholder names.
    PlaceholdersDiffer,
    /// A `{` or `}` in this language's text is not part of a `{[a-z][a-z0-9_]*}` placeholder.
    StrayBrace(UiLanguage),
}

#[cfg(test)]
const LANGS: [UiLanguage; 2] = [UiLanguage::En, UiLanguage::Ru];

impl Catalog {
    /// Parse the two catalogs. Lenient about content (empty texts, missing ids are
    /// allowed here and reported by `parity_problems`); rejects anything that is not
    /// a flat JSON object of strings.
    pub(crate) fn from_json(en: &str, ru: &str) -> Result<Catalog, CatalogError> {
        Ok(Catalog {
            en: parse_flat(UiLanguage::En, en)?,
            ru: parse_flat(UiLanguage::Ru, ru)?,
        })
    }

    /// The single lookup + render rule (see `i18n/conformance.json`).
    pub(crate) fn text(&self, lang: UiLanguage, id: &str, args: &[(&str, &str)]) -> String {
        let found = self
            .non_empty(lang, id)
            .or_else(|| self.non_empty(UiLanguage::En, id));
        match found {
            Some(template) => render(template, args),
            None => id.to_string(),
        }
    }

    /// Invariant (1): same id set, non-empty texts, same placeholder set per id,
    /// no stray braces.
    #[cfg(test)]
    fn parity_problems(&self) -> Vec<CatalogProblem> {
        let ids: BTreeSet<&str> = self
            .en
            .keys()
            .chain(self.ru.keys())
            .map(String::as_str)
            .collect();
        let mut problems = Vec::new();
        for id in ids {
            let mut report = |kind| {
                problems.push(CatalogProblem {
                    id: id.to_string(),
                    kind,
                })
            };
            for lang in LANGS {
                match self.map(lang).get(id) {
                    None => report(ProblemKind::Missing(lang)),
                    Some(text) if text.is_empty() => report(ProblemKind::Empty(lang)),
                    Some(text) => {
                        if tokens(text).iter().any(|t| matches!(t, Token::Stray(_))) {
                            report(ProblemKind::StrayBrace(lang));
                        }
                    }
                }
            }
            if let (Some(en), Some(ru)) = (
                self.non_empty(UiLanguage::En, id),
                self.non_empty(UiLanguage::Ru, id),
            ) {
                if placeholder_names(en) != placeholder_names(ru) {
                    report(ProblemKind::PlaceholdersDiffer);
                }
            }
        }
        problems
    }

    /// Ids of `ids` absent from either catalog (kind `Missing(lang)`).
    #[cfg(test)]
    fn missing_ids(&self, ids: &[MessageId]) -> Vec<CatalogProblem> {
        ids.iter()
            .flat_map(|id| {
                LANGS
                    .into_iter()
                    .filter(|lang| !self.map(*lang).contains_key(id.0))
                    .map(|lang| CatalogProblem {
                        id: id.0.to_string(),
                        kind: ProblemKind::Missing(lang),
                    })
            })
            .collect()
    }

    fn map(&self, lang: UiLanguage) -> &BTreeMap<String, String> {
        match lang {
            UiLanguage::En => &self.en,
            UiLanguage::Ru => &self.ru,
        }
    }

    fn non_empty(&self, lang: UiLanguage, id: &str) -> Option<&str> {
        self.map(lang)
            .get(id)
            .map(String::as_str)
            .filter(|t| !t.is_empty())
    }
}

fn parse_flat(lang: UiLanguage, json: &str) -> Result<BTreeMap<String, String>, CatalogError> {
    serde_json::from_str(json).map_err(|e| CatalogError {
        lang,
        message: e.to_string(),
    })
}

/// One piece of a catalog text.
#[derive(Debug, PartialEq, Eq)]
enum Token<'a> {
    /// Plain text, rendered as is.
    Literal(&'a str),
    /// `{name}`; holds `name`.
    Placeholder(&'a str),
    /// A `{` or `}` that is not part of a placeholder; rendered as is.
    Stray(&'a str),
}

/// Split `text` into literals, placeholders and stray braces in one left-to-right
/// scan. All delimiters are ASCII, so every slice falls on a char boundary.
fn tokens(text: &str) -> Vec<Token<'_>> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut literal_start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let brace = bytes[i];
        if brace != b'{' && brace != b'}' {
            i += 1;
            continue;
        }
        if literal_start < i {
            out.push(Token::Literal(&text[literal_start..i]));
        }
        let name_len = if brace == b'{' {
            placeholder_name_len(&bytes[i + 1..])
        } else {
            None
        };
        match name_len {
            Some(len) => {
                out.push(Token::Placeholder(&text[i + 1..i + 1 + len]));
                i += len + 2;
            }
            None => {
                out.push(Token::Stray(&text[i..i + 1]));
                i += 1;
            }
        }
        literal_start = i;
    }
    if literal_start < bytes.len() {
        out.push(Token::Literal(&text[literal_start..]));
    }
    out
}

/// Length of `name` if `rest` starts with `name}` and `name` is `[a-z][a-z0-9_]*`.
fn placeholder_name_len(rest: &[u8]) -> Option<usize> {
    if !rest.first()?.is_ascii_lowercase() {
        return None;
    }
    let len = 1 + rest[1..]
        .iter()
        .take_while(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || **b == b'_')
        .count();
    (rest.get(len) == Some(&b'}')).then_some(len)
}

#[cfg(test)]
fn placeholder_names(text: &str) -> BTreeSet<&str> {
    tokens(text)
        .into_iter()
        .filter_map(|t| match t {
            Token::Placeholder(name) => Some(name),
            _ => None,
        })
        .collect()
}

fn render(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    for token in tokens(template) {
        match token {
            Token::Literal(s) | Token::Stray(s) => out.push_str(s),
            Token::Placeholder(name) => match args.iter().find(|(k, _)| *k == name) {
                Some((_, value)) => out.push_str(value),
                None => {
                    out.push('{');
                    out.push_str(name);
                    out.push('}');
                }
            },
        }
    }
    out
}

const EMBEDDED_EN: &str = include_str!("../../../i18n/en.json");
const EMBEDDED_RU: &str = include_str!("../../../i18n/ru.json");

/// The catalog embedded from `i18n/en.json` and `i18n/ru.json`, parsed once.
///
/// Private on purpose: the only public way to render embedded text is [`text`],
/// which takes a [`MessageId`], so every Rust id goes through [`MESSAGE_IDS`]
/// (invariant 3). `Catalog::text` takes a `&str` id, so neither this accessor nor
/// `Catalog` itself (crate-private) is reachable from another crate.
///
/// The files are compiled in. The test `catalog_parity_holds_for_the_real_catalogs`
/// parses the same files and fails if they do not parse. If parsing failed anyway,
/// this function would not panic: the catalog would be empty and every message
/// would render as its id. That fallback itself is not covered by a test.
fn embedded() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| Catalog::from_json(EMBEDDED_EN, EMBEDDED_RU).unwrap_or_default())
}

/// Render a Rust-originated message from the embedded catalog. An argument value
/// that is a nested id (`mic_reason.*`) is rendered as that message's text in
/// `lang` first (module docs).
pub fn text(lang: UiLanguage, id: MessageId, args: &[(&str, &str)]) -> String {
    render_with_nested(embedded(), lang, id, args)
}

fn render_with_nested(
    catalog: &Catalog,
    lang: UiLanguage,
    id: MessageId,
    args: &[(&str, &str)],
) -> String {
    let resolved: Vec<(&str, String)> = args
        .iter()
        .map(
            |(name, value)| match NESTED_IDS.iter().find(|n| n.0 == *value) {
                Some(nested) => (*name, catalog.text(lang, nested.0, &[])),
                None => (*name, (*value).to_string()),
            },
        )
        .collect();
    let resolved: Vec<(&str, &str)> = resolved.iter().map(|(k, v)| (*k, v.as_str())).collect();
    catalog.text(lang, id.0, &resolved)
}

/// OS language tag -> UI language: primary subtag `ru` (any ASCII case) -> Ru, else En.
///
/// The primary subtag is the text before the first `-` or `_`.
pub fn resolve_ui_language(os_tag: Option<&str>) -> UiLanguage {
    let primary = os_tag
        .and_then(|tag| tag.split(['-', '_']).next())
        .unwrap_or("");
    if primary.eq_ignore_ascii_case("ru") {
        UiLanguage::Ru
    } else {
        UiLanguage::En
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EN_JSON: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../i18n/en.json"));
    const RU_JSON: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../i18n/ru.json"));
    const CONFORMANCE_JSON: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../i18n/conformance.json"
    ));

    fn catalog(en: &str, ru: &str) -> Catalog {
        Catalog::from_json(en, ru).expect("test catalog must parse")
    }

    fn problem(id: &str, kind: ProblemKind) -> CatalogProblem {
        CatalogProblem {
            id: id.to_string(),
            kind,
        }
    }

    // ---- AC1: one rendering rule, pinned by the shared fixture -------------------

    /// Fixture format: see the `_format` key of `i18n/conformance.json`. The UI
    /// suite (src/lib/i18n/i18n.test.ts) iterates the same file.
    #[test]
    fn conformance_fixture() {
        let fixture: serde_json::Value =
            serde_json::from_str(CONFORMANCE_JSON).expect("conformance.json is valid JSON");
        let cases = fixture["cases"].as_array().expect("`cases` is an array");
        assert!(cases.len() >= 15, "fixture lost cases: {}", cases.len());

        for case in cases {
            let name = case["name"].as_str().expect("case has a name");
            let en = serde_json::to_string(&case["en"]).unwrap();
            let ru = serde_json::to_string(&case["ru"]).unwrap();
            let lang: UiLanguage = serde_json::from_value(case["lang"].clone())
                .unwrap_or_else(|e| panic!("case {name:?}: bad lang: {e}"));
            let id = case["id"].as_str().expect("case has an id");
            let args: Vec<(&str, &str)> = case["args"]
                .as_object()
                .expect("args is an object")
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str().expect("arg values are strings")))
                .collect();
            let expected = case["expected"].as_str().expect("case has expected");

            let catalog = Catalog::from_json(&en, &ru)
                .unwrap_or_else(|e| panic!("case {name:?}: catalog rejected: {e:?}"));
            assert_eq!(catalog.text(lang, id, &args), expected, "case {name:?}");
        }
    }

    #[test]
    fn ui_language_tags_are_en_and_ru() {
        assert_eq!(serde_json::to_string(&UiLanguage::En).unwrap(), "\"en\"");
        assert_eq!(serde_json::to_string(&UiLanguage::Ru).unwrap(), "\"ru\"");
    }

    #[test]
    fn from_json_rejects_a_catalog_that_is_not_a_flat_string_map() {
        let good = r#"{"a":"A"}"#;
        for bad in [
            r#"{"a":"A""#,        // malformed
            r#"{"a":{"b":"B"}}"#, // nested
            r#"{"a":1}"#,         // non-string value
            r#"["a"]"#,           // not an object
        ] {
            let err = Catalog::from_json(good, bad).expect_err(bad);
            assert_eq!(err.lang, UiLanguage::Ru, "ru file is the broken one: {bad}");
            let err = Catalog::from_json(bad, good).expect_err(bad);
            assert_eq!(err.lang, UiLanguage::En, "en file is the broken one: {bad}");
        }
    }

    #[test]
    fn text_renders_from_the_embedded_catalog() {
        let id = MessageId("app.build_info_error");
        assert_eq!(
            text(UiLanguage::En, id, &[("reason", "disk")]),
            "Cannot read build info: disk"
        );
        assert_eq!(
            text(UiLanguage::Ru, id, &[("reason", "диск")]),
            "Не удалось прочитать сведения о сборке: диск"
        );
        assert_eq!(
            text(
                UiLanguage::En,
                MessageId("app.build_info"),
                &[("version", "1.2.3"), ("commit", "abc1234")]
            ),
            "Voicen 1.2.3 (abc1234)"
        );
    }

    #[test]
    fn nested_id_arguments_render_in_the_same_language_and_nothing_else_resolves() {
        // T-042: `{reason}` carries a `mic_reason.*` id (lang-independent params);
        // text() renders it in the requested language. Bite: the raw id rendered,
        // the nested text taken from English for ru, any declared id (or any
        // catalog key) resolved, which would reopen the `&str` id path.
        let c = catalog(
            r#"{"m":"Mic: {reason}","mic_reason.busy":"busy {x}","failure.timeout":"T"}"#,
            r#"{"m":"Мик: {reason}","mic_reason.busy":"занято {x}","failure.timeout":"Т"}"#,
        );
        let m = MessageId("m");
        let rows = [
            (UiLanguage::En, "mic_reason.busy", "Mic: busy {x}"),
            (UiLanguage::Ru, "mic_reason.busy", "Мик: занято {x}"),
            (UiLanguage::En, "failure.timeout", "Mic: failure.timeout"),
            (UiLanguage::Ru, "disk", "Мик: disk"),
        ];
        for (lang, value, want) in rows {
            let got = render_with_nested(&c, lang, m, &[("reason", value), ("x", "X")]);
            assert_eq!(got, want, "{lang:?} {value:?}");
        }
        assert!(NESTED_IDS.iter().all(|n| MESSAGE_IDS.contains(n)));
    }

    // ---- AC2: OS language -> UI language, in one place ---------------------------

    #[test]
    fn resolve_ui_language_table() {
        let table: &[(Option<&str>, UiLanguage)] = &[
            (Some("ru"), UiLanguage::Ru),
            (Some("ru-RU"), UiLanguage::Ru),
            (Some("RU-ua"), UiLanguage::Ru),
            (Some("Ru"), UiLanguage::Ru),
            (Some("ru-Latn"), UiLanguage::Ru),
            (Some("ru_RU"), UiLanguage::Ru),
            (Some("ru-"), UiLanguage::Ru),
            (Some("en-US"), UiLanguage::En),
            (Some("en-GB"), UiLanguage::En),
            (Some("en-RU"), UiLanguage::En),
            (Some("de-DE"), UiLanguage::En),
            (Some("uk-UA"), UiLanguage::En),
            (Some("be-BY"), UiLanguage::En),
            (Some("rus"), UiLanguage::En),
            (Some("russian"), UiLanguage::En),
            (Some("r"), UiLanguage::En),
            (Some("-ru"), UiLanguage::En),
            (Some("_ru"), UiLanguage::En),
            (Some(""), UiLanguage::En),
            (None, UiLanguage::En),
        ];
        for (tag, expected) in table {
            assert_eq!(resolve_ui_language(*tag), *expected, "tag {tag:?}");
        }
    }

    // ---- AC3: catalog invariants name the offending id ---------------------------

    #[test]
    fn catalog_parity_accepts_a_consistent_pair() {
        let c = catalog(
            r#"{"a":"Hello","b":"{x} and {y}","c":"{x}{x} {n_2}"}"#,
            r#"{"a":"Привет","b":"{y} и {x} и {y}","c":"{n_2}: {x}"}"#,
        );
        assert_eq!(c.parity_problems(), vec![]);
    }

    #[test]
    fn catalog_parity_names_an_id_missing_from_ru() {
        let c = catalog(r#"{"a":"A","b":"B"}"#, r#"{"a":"А"}"#);
        assert_eq!(
            c.parity_problems(),
            vec![problem("b", ProblemKind::Missing(UiLanguage::Ru))]
        );
    }

    #[test]
    fn catalog_parity_names_an_id_missing_from_en() {
        let c = catalog(r#"{"a":"A"}"#, r#"{"a":"А","c":"В"}"#);
        assert_eq!(
            c.parity_problems(),
            vec![problem("c", ProblemKind::Missing(UiLanguage::En))]
        );
    }

    #[test]
    fn catalog_parity_names_an_empty_text() {
        let c = catalog(r#"{"a":"A","b":"B"}"#, r#"{"a":"А","b":""}"#);
        assert_eq!(
            c.parity_problems(),
            vec![problem("b", ProblemKind::Empty(UiLanguage::Ru))]
        );
        let c = catalog(r#"{"a":"","b":"B"}"#, r#"{"a":"А","b":"Б"}"#);
        assert_eq!(
            c.parity_problems(),
            vec![problem("a", ProblemKind::Empty(UiLanguage::En))]
        );
    }

    #[test]
    fn catalog_parity_names_a_different_placeholder_set() {
        for (en, ru) in [
            ("Hi {name}", "Привет {user}"),
            ("{a} {b}", "{a}"),
            ("{a}", "{a} {b}"),
            ("Hi", "Привет {name}"),
        ] {
            let c = catalog(
                &format!(r#"{{"ok":"Fine","bad":"{en}"}}"#),
                &format!(r#"{{"ok":"Хорошо","bad":"{ru}"}}"#),
            );
            assert_eq!(
                c.parity_problems(),
                vec![problem("bad", ProblemKind::PlaceholdersDiffer)],
                "en {en:?} / ru {ru:?}"
            );
        }
    }

    #[test]
    fn catalog_parity_names_a_stray_brace() {
        for stray in [
            "{", "}", "{}", "{x", "x}", "{ x }", "{Name}", "{1a}", "{a-b}", "{{x}}",
        ] {
            let c = catalog(
                &format!(r#"{{"ok":"Fine {{x}}","bad":"Use {{x}} {stray}"}}"#),
                r#"{"ok":"Хорошо {x}","bad":"Используйте {x}"}"#,
            );
            let problems = c.parity_problems();
            assert!(
                problems.contains(&problem("bad", ProblemKind::StrayBrace(UiLanguage::En))),
                "stray {stray:?} not reported: {problems:?}"
            );
            assert!(
                problems.iter().all(|p| p.id == "bad"),
                "stray {stray:?}: only the offending id is named: {problems:?}"
            );
        }
        // and in ru
        let c = catalog(r#"{"bad":"Use {x}"}"#, r#"{"bad":"Используйте {x} }"}"#);
        assert_eq!(
            c.parity_problems(),
            vec![problem("bad", ProblemKind::StrayBrace(UiLanguage::Ru))]
        );
    }

    #[test]
    fn catalog_parity_holds_for_the_real_catalogs() {
        let problems = catalog(EN_JSON, RU_JSON).parity_problems();
        assert!(
            problems.is_empty(),
            "i18n/en.json vs i18n/ru.json: {problems:?}"
        );
        let problems = embedded().parity_problems();
        assert!(problems.is_empty(), "embedded catalog: {problems:?}");
    }

    #[test]
    fn message_id_serializes_as_id() {
        // Bite: MessageId serialized as a struct ({"0":..}/{"id":..}), as a
        // placeholder, or as its Debug form (`MessageId("...")`). The UI renders
        // FormError.message with t(id), so the wire value must be the catalog id.
        assert_eq!(
            serde_json::to_string(&SETTINGS_WRITE_FAILED).expect("MessageId serializes"),
            r#""settings.write_failed""#
        );
        for id in MESSAGE_IDS {
            assert_eq!(
                serde_json::to_value(id).expect("MessageId serializes"),
                serde_json::Value::String(id.0.to_string()),
                "{id:?}"
            );
        }
    }

    #[test]
    fn message_ids_exist_in_both_catalogs() {
        // Bite: a synthetic list against a synthetic catalog.
        let c = catalog(r#"{"a":"A","b":"B"}"#, r#"{"a":"А"}"#);
        let found = c.missing_ids(&[MessageId("a"), MessageId("b"), MessageId("d")]);
        let expected = [
            problem("b", ProblemKind::Missing(UiLanguage::Ru)),
            problem("d", ProblemKind::Missing(UiLanguage::En)),
            problem("d", ProblemKind::Missing(UiLanguage::Ru)),
        ];
        assert_eq!(found.len(), expected.len(), "{found:?}");
        for p in &expected {
            assert!(found.contains(p), "{p:?} not reported: {found:?}");
        }
        assert_eq!(c.missing_ids(&[MessageId("a")]), vec![]);

        // The real list against the embedded catalog.
        let missing = embedded().missing_ids(MESSAGE_IDS);
        assert!(
            missing.is_empty(),
            "MESSAGE_IDS not in i18n/*.json: {missing:?}"
        );
    }

    #[test]
    fn notice_max_length_is_declared_with_its_contract_text() {
        // T-009 (FR-03, decision #1): the max-length stop shows `notice.max_length`;
        // it is a declared MessageId (so `message_ids_exist_in_both_catalogs` covers
        // it) and has the contracts/messages.md text in both catalogs. Bite: the id
        // missing from `messages!` or from a catalog (the overlay shows the raw id),
        // the text of another notice.
        assert_eq!(NOTICE_MAX_LENGTH.0, "notice.max_length");
        assert!(
            MESSAGE_IDS.contains(&NOTICE_MAX_LENGTH),
            "notice.max_length not in MESSAGE_IDS"
        );
        let c = catalog(EN_JSON, RU_JSON);
        assert_eq!(
            own_text(&c, UiLanguage::En, "notice.max_length"),
            Some("Maximum length reached")
        );
        assert_eq!(
            own_text(&c, UiLanguage::Ru, "notice.max_length"),
            Some("Достигнута максимальная длительность")
        );
    }

    #[test]
    fn failure_local_timeout_is_declared_with_its_contract_text_and_listed_in_messages_md() {
        // T-106 Acceptance 3 (decision #147): the built-in engine's timeout id
        // `failure.local_timeout` is declared with messages! (so
        // `message_ids_exist_in_both_catalogs` and the parity tests cover it), has
        // the decision-#147 text in en and ru (spelled here), and
        // specs/001 contracts/messages.md lists it with both texts. Bite: the id
        // missing from `messages!`, from a catalog or from the contract table;
        // the server text (`failure.timeout`) reused under the new id.
        assert!(
            MESSAGE_IDS.iter().any(|id| id.0 == "failure.local_timeout"),
            "failure.local_timeout not in MESSAGE_IDS"
        );
        let c = catalog(EN_JSON, RU_JSON);
        assert_eq!(
            own_text(&c, UiLanguage::En, "failure.local_timeout"),
            Some("Transcription took too long")
        );
        assert_eq!(
            own_text(&c, UiLanguage::Ru, "failure.local_timeout"),
            Some("Распознавание заняло слишком много времени")
        );
        const MESSAGES_MD: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/001-dictation-via-api/contracts/messages.md"
        ));
        let row = MESSAGES_MD
            .lines()
            .find(|l| l.starts_with("| `failure.local_timeout` |"))
            .expect("contracts/messages.md has a failure.local_timeout row");
        let cells: Vec<&str> = row.split('|').map(str::trim).collect();
        assert_eq!(
            cells.get(2..4),
            Some(
                &[
                    "Transcription took too long",
                    "Распознавание заняло слишком много времени"
                ][..]
            ),
            "{row}"
        );
    }

    // ---- T-004: every refusal reaches the user in the user's language (U2) --------

    /// The non-empty text of `id` in `lang`, straight from the catalog map (no
    /// fallback to English or to the id, which `text` would apply).
    fn own_text<'a>(c: &'a Catalog, lang: UiLanguage, id: &str) -> Option<&'a str> {
        let map = match lang {
            UiLanguage::En => &c.en,
            UiLanguage::Ru => &c.ru,
        };
        map.get(id).map(String::as_str).filter(|t| !t.is_empty())
    }

    #[test]
    fn every_error_code_has_catalog_text() {
        // Bite: any ErrorCode core can return without a non-empty `error.<code>` in
        // i18n/en.json or i18n/ru.json; the UI would show the raw id (text() falls
        // back to it). T-004 Acceptance; spec T044.
        // The codes are core's one hand-kept list, `ErrorCode::ALL` (completeness by
        // hand, next to the enum; settings/mod.rs).
        use crate::settings::ErrorCode;
        let c = catalog(EN_JSON, RU_JSON);
        let mut missing = Vec::new();
        for code in ErrorCode::ALL {
            let id = format!("error.{}", code.as_str());
            for lang in LANGS {
                if own_text(&c, lang, &id).is_none() {
                    missing.push(format!("{id} ({lang:?})"));
                }
            }
        }
        assert!(missing.is_empty(), "error.<code> without text: {missing:?}");
    }

    #[test]
    fn every_field_id_has_label_text() {
        // Bite: any FieldId core can name (a FieldError, FormError.not_restored) without
        // a non-empty `settings.field_label.<FieldId>` in i18n/en.json or i18n/ru.json;
        // the settings window lists a not-restored field it does not render by that
        // label (T-039 (L)), and would show the raw id (text() falls back to it).
        // The fields are core's one hand-kept list, `FieldId::ALL` (completeness by
        // hand, next to the enum; settings/mod.rs), as for `error.<code>` above.
        use crate::settings::FieldId;
        let c = catalog(EN_JSON, RU_JSON);
        let mut missing = Vec::new();
        // T-021: the post-processing toggle is a FieldId (the not-restored list
        // names it by its label). Bite: the variant left out of FieldId::ALL, so the
        // loop below never asks for `settings.field_label.post_processing.enabled`.
        assert!(
            FieldId::ALL.contains(&FieldId::PostProcessingEnabled),
            "PostProcessingEnabled not in FieldId::ALL"
        );
        for field in FieldId::ALL {
            let id = format!("settings.field_label.{}", field.as_str());
            for lang in LANGS {
                if own_text(&c, lang, &id).is_none() {
                    missing.push(format!("{id} ({lang:?})"));
                }
            }
        }
        assert!(
            missing.is_empty(),
            "settings.field_label.<FieldId> without text: {missing:?}"
        );
    }

    #[test]
    fn call_settings_ids_have_text_without_a_reason_placeholder() {
        // T-120 (decisions #196, #198): the four `calls.*` labels and the two new error
        // codes have EN and RU text. `error.storage.unwritable` takes no `{reason}`:
        // FieldMessage.svelte renders `t(id)` without arguments, and the per-reason
        // texts belong to the Calls tab (G41). Bite: a label or error text
        // missing in one language, a placeholder in the error text (the UI would show
        // `{reason}`).
        let c = catalog(EN_JSON, RU_JSON);
        for lang in LANGS {
            for id in [
                "settings.field_label.calls.hotkey",
                "settings.field_label.calls.reminder",
                "settings.field_label.calls.primary_language",
                "settings.field_label.calls.storage_root",
                "error.hotkey.equals_dictation",
                "error.storage.unwritable",
            ] {
                assert!(own_text(&c, lang, id).is_some(), "{id} ({lang:?})");
            }
            for id in ["error.hotkey.equals_dictation", "error.storage.unwritable"] {
                let text = own_text(&c, lang, id).expect("checked above");
                assert!(
                    !text.contains('{'),
                    "{id} ({lang:?}) has a placeholder: {text}"
                );
            }
        }
    }

    /// T-073: the id suffix of a timeouts field (`settings.field_label.timeouts.<suffix>`,
    /// `settings.timeouts.<suffix>.hint`). An exhaustive match, so a new role does not
    /// compile until it is named here.
    fn timeout_suffix(role: crate::timeouts::TimeoutRole) -> &'static str {
        use crate::timeouts::TimeoutRole;
        match role {
            TimeoutRole::Connect => "connect",
            TimeoutRole::ApiTranscription => "api_transcription",
            TimeoutRole::LocalServer => "local_server",
            TimeoutRole::PostProcessing => "post_processing",
            TimeoutRole::BuiltinLocal => "builtin_local",
        }
    }

    /// T-073: every timeouts role, from the one table (`TimeoutRole::ALL`). Pinned to
    /// the five distinct roles so the tests below cannot pass on an empty or short
    /// list.
    fn timeout_roles() -> [crate::timeouts::TimeoutRole; 5] {
        use crate::timeouts::TimeoutRole;
        let all = TimeoutRole::ALL;
        let distinct: BTreeSet<&str> = all.iter().map(|r| timeout_suffix(*r)).collect();
        assert_eq!(
            distinct,
            [
                "connect",
                "api_transcription",
                "local_server",
                "post_processing",
                "builtin_local"
            ]
            .into_iter()
            .collect::<BTreeSet<_>>(),
            "TimeoutRole::ALL must hold exactly the five roles"
        );
        all
    }

    #[test]
    fn timeout_texts_exist_in_both_catalogs() {
        // T-073: the refusal and each control reach the user in en and ru, named
        // here so they are checked even if a variant is left out of FieldId::ALL /
        // ErrorCode::ALL. Bite: `error.timeout.range`, a
        // `settings.field_label.timeouts.<role>` or a `settings.timeouts.<role>.hint`
        // missing or empty in a catalog.
        let c = catalog(EN_JSON, RU_JSON);
        let mut ids = vec!["error.timeout.range".to_string()];
        for role in timeout_roles() {
            let role = timeout_suffix(role);
            ids.push(format!("settings.field_label.timeouts.{role}"));
            ids.push(format!("settings.timeouts.{role}.hint"));
        }
        let mut missing = Vec::new();
        for id in &ids {
            for lang in LANGS {
                if own_text(&c, lang, id).is_none() {
                    missing.push(format!("{id} ({lang:?})"));
                }
            }
        }
        assert!(missing.is_empty(), "ids without text: {missing:?}");
    }

    #[test]
    fn timeout_hints_state_the_bounds_table() {
        // T-073: FieldError carries no parameters, so each control's range is in a
        // static hint ("1–60 s, default 5"); the numbers in it must be exactly the
        // min, max and default of that role in the one bounds table
        // (`TimeoutRole::bounds`, decision #99), in en and ru, so a hint and the
        // table cannot drift (P-010). Bite: a hint with another field's range, a
        // changed bound in one language only, a hint without the default, or a
        // bound changed in `TimeoutRole::bounds` with the hints left alone.
        let c = catalog(EN_JSON, RU_JSON);
        for role in timeout_roles() {
            let b = role.bounds();
            let (default, min, max) = (b.default, b.min, b.max);
            let id = format!("settings.timeouts.{}.hint", timeout_suffix(role));
            for lang in LANGS {
                let text = own_text(&c, lang, &id).unwrap_or("");
                let numbers: BTreeSet<u32> = text
                    .split(|ch: char| !ch.is_ascii_digit())
                    .filter(|part| !part.is_empty())
                    .filter_map(|part| part.parse().ok())
                    .collect();
                let want: BTreeSet<u32> = [default, min, max].into_iter().collect();
                assert_eq!(numbers, want, "{id} ({lang:?}): {text:?}");
            }
        }
    }

    #[test]
    fn settings_window_message_ids_exist() {
        // T-004 owns two more ids. `notice.settings_reset` (the window's reset banner,
        // and T-006's toast) is a Rust id, so it is declared with messages! and lands
        // in MESSAGE_IDS; `error.ipc_unavailable` is UI-only (a rejected invoke,
        // contracts/ipc.md "Errors"). Bite: either id missing or empty in a catalog,
        // or `notice.settings_reset` not declared in messages!.
        assert!(
            MESSAGE_IDS.iter().any(|m| m.0 == "notice.settings_reset"),
            "notice.settings_reset is not declared in messages!"
        );
        let c = catalog(EN_JSON, RU_JSON);
        let mut missing = Vec::new();
        for id in ["notice.settings_reset", "error.ipc_unavailable"] {
            for lang in LANGS {
                if own_text(&c, lang, id).is_none() {
                    missing.push(format!("{id} ({lang:?})"));
                }
            }
        }
        assert!(missing.is_empty(), "ids without text: {missing:?}");
    }

    #[test]
    fn post_processing_skip_ids_are_declared_with_text_and_their_placeholders() {
        // T-020, contracts/ipc.md "Message catalog entries", #91(2): the six
        // notice.post_processing_skipped.* ids are declared with messages! (so
        // the parity and existence tests cover them), have their own non-empty
        // text in en and ru, and name exactly their placeholders: {host} for
        // unreachable, {status} for http, none for the rest. Bite: an id missing
        // from a catalog, a placeholder dropped or misspelled, an id served only
        // by text()'s fallback.
        let c = catalog(EN_JSON, RU_JSON);
        let rows: [(&str, &[&str]); 6] = [
            ("notice.post_processing_skipped.timeout", &[]),
            ("notice.post_processing_skipped.unreachable", &["host"]),
            ("notice.post_processing_skipped.invalid_key", &[]),
            ("notice.post_processing_skipped.http", &["status"]),
            ("notice.post_processing_skipped.invalid_response", &[]),
            ("notice.post_processing_skipped.not_configured", &[]),
        ];
        let mut wrong = Vec::new();
        for (id, names) in rows {
            if !MESSAGE_IDS.iter().any(|m| m.0 == id) {
                wrong.push(format!("{id}: not declared with messages!"));
            }
            let want: BTreeSet<&str> = names.iter().copied().collect();
            for lang in LANGS {
                match own_text(&c, lang, id) {
                    None => wrong.push(format!("{id} ({lang:?}): no text")),
                    Some(t) if placeholder_names(t) != want => wrong.push(format!(
                        "{id} ({lang:?}): placeholders {:?}, expected {want:?}",
                        placeholder_names(t)
                    )),
                    Some(_) => {}
                }
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    #[test]
    fn logs_unwritable_ids_are_declared_with_text_and_the_reason_renders_nested() {
        // T-054 (contracts/ipc.md:41, OQ-35): `notice.logs_unwritable` and the four
        // `logs_reason.*` ids are declared with messages! (so parity and existence
        // cover them), have their own non-empty text in en and ru, the notice names
        // exactly {reason} and a reason names nothing; the EN notice text is the
        // contract's; `{reason}` carrying a `logs_reason.*` id renders that id's
        // text in the same language (the ids are in the nested group, as
        // `mic_reason.*`). Bite: an id missing from messages! or a catalog, the
        // reasons outside the nested group (the overlay shows "logs_reason.disk_full"),
        // a placeholder dropped or renamed.
        let c = catalog(EN_JSON, RU_JSON);
        let notice = "notice.logs_unwritable";
        let reasons = [
            "logs_reason.permission_denied",
            "logs_reason.disk_full",
            "logs_reason.not_a_directory",
            "logs_reason.other",
        ];
        let mut wrong = Vec::new();
        let rows =
            std::iter::once((notice, &["reason"][..])).chain(reasons.iter().map(|r| (*r, &[][..])));
        for (id, names) in rows {
            if !MESSAGE_IDS.iter().any(|m| m.0 == id) {
                wrong.push(format!("{id}: not declared with messages!"));
            }
            let want: BTreeSet<&str> = names.iter().copied().collect();
            for lang in LANGS {
                match own_text(&c, lang, id) {
                    None => wrong.push(format!("{id} ({lang:?}): no text")),
                    Some(t) if placeholder_names(t) != want => wrong.push(format!(
                        "{id} ({lang:?}): placeholders {:?}, expected {want:?}",
                        placeholder_names(t)
                    )),
                    Some(_) => {}
                }
            }
        }
        for r in reasons {
            if !NESTED_IDS.iter().any(|m| m.0 == r) {
                wrong.push(format!("{r}: not in the nested group (NESTED_IDS)"));
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
        assert_eq!(
            own_text(&c, UiLanguage::En, notice),
            Some("Logs cannot be written: {reason}")
        );
        for lang in LANGS {
            for r in reasons {
                let got = text(lang, MessageId(notice), &[("reason", r)]);
                let reason_text = own_text(&c, lang, r).unwrap_or("<none>");
                assert!(
                    !got.contains("logs_reason.") && got.contains(reason_text),
                    "{lang:?} {r}: rendered {got:?}"
                );
            }
        }
    }

    // ---- T-119: the call-mode vocabulary (contracts/ipc.md "Message catalog", #198) ----
    //
    // Every lookup below is by string, through the catalog map and MESSAGE_IDS, so
    // each test fails by its own assertion while the ids are missing and the
    // existing tests keep running (no `MessageId` constant is named here).

    /// Fixed (not rule-built) call ids: `(id, placeholders, rust)`. `rust` = the id
    /// is Rust-originated, so it is declared with `messages!` (and is in
    /// `MESSAGE_IDS`); `false` = a UI-only `t()` literal, JSON only (i18n.md,
    /// #198 D4). 25 Rust + 28 UI-only here; the four families add 26 + 6.
    const CALL_FIXED_IDS: [(&str, &[&str], bool); 53] = [
        ("tray.call", &[], true),
        ("tray.call.record", &[], true),
        ("tray.call.stop", &[], true),
        ("tray.calls", &[], true),
        ("tray.tooltip.call_recording", &[], true),
        ("indicator.reminder", &[], true),
        ("indicator.headphones", &[], true),
        ("warning.communications_device", &[], true),
        ("warning.mic_changed", &[], true),
        ("warning.cant_hear_other_party", &[], true),
        ("warning.cant_hear_you", &[], true),
        ("warning.low_disk", &[], true),
        ("warning.provider_notice", &["host"], true),
        ("notice.call_saved", &[], true),
        ("notice.transcript_ready", &[], true),
        ("notice.open", &[], true),
        ("notice.stopped.sleep", &[], true),
        ("notice.stopped.max_length", &[], true),
        ("notice.stopped.low_disk", &[], true),
        ("notice.stopped.storage", &[], true),
        ("notice.call_hotkey_conflict", &[], true),
        ("exit.recording.question", &[], true),
        (
            "calls.title_default",
            &["day", "month", "hour", "minute"],
            true,
        ),
        ("speaker.me", &[], true),
        ("speaker.other", &[], true),
        ("indicator.recording", &[], false),
        ("indicator.stop", &[], false),
        ("indicator.dismiss", &[], false),
        ("calls.empty", &["hotkey"], false),
        ("calls.state.recording", &[], false),
        ("calls.state.queued", &[], false),
        ("calls.state.transcribing", &[], false),
        ("calls.state.ready", &[], false),
        ("calls.state.failed", &[], false),
        ("calls.state.recovered", &[], false),
        ("calls.state.damaged", &[], false),
        ("calls.root_unavailable", &["root"], false),
        ("calls.search", &[], false),
        ("calls.no_matches", &[], false),
        ("calls.transcribe", &[], false),
        ("calls.transcribe_again", &[], false),
        ("calls.copy", &[], false),
        ("calls.export_md", &[], false),
        ("calls.export_json", &[], false),
        ("calls.open_folder", &[], false),
        ("calls.delete", &[], false),
        ("calls.delete_confirm", &[], false),
        ("calls.error.in_use", &["file"], false),
        ("calls.error.empty_name", &[], false),
        ("calls.error.recording", &[], false),
        ("calls.error.io", &[], false),
        ("settings.tab.calls", &[], false),
        ("settings.calls.provider_notice", &["host"], false),
    ];

    /// Problems of a family's ids: origin (`rust`: declared with messages!) and text.
    fn family_problems(c: &Catalog, ids: &[String], rust: bool) -> Vec<String> {
        let mut wrong = Vec::new();
        for id in ids {
            let declared = MESSAGE_IDS.iter().any(|m| m.0 == id);
            if rust && !declared {
                wrong.push(format!("{id}: not declared with messages!"));
            }
            if !rust && declared {
                wrong.push(format!("{id}: UI-only, but declared with messages!"));
            }
            for lang in LANGS {
                match own_text(c, lang, id) {
                    None => wrong.push(format!("{id} ({lang:?}): no text")),
                    Some(t) if !placeholder_names(t).is_empty() => wrong.push(format!(
                        "{id} ({lang:?}): placeholders {:?}, expected none",
                        placeholder_names(t)
                    )),
                    Some(_) => {}
                }
            }
        }
        wrong
    }

    /// The family is exactly `expected` ids: every id is present with text in both
    /// languages, and no other key of either catalog carries the prefix (a typo, a
    /// stale pair).
    fn assert_family(prefix: &str, ids: Vec<String>, expected: usize, rust: bool) {
        let distinct: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
        assert_eq!(
            distinct.len(),
            expected,
            "{prefix}*: the variants must map to {expected} distinct ids: {ids:?}"
        );
        let c = catalog(EN_JSON, RU_JSON);
        let wrong = family_problems(&c, &ids, rust);
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
        for lang in LANGS {
            let map = match lang {
                UiLanguage::En => &c.en,
                UiLanguage::Ru => &c.ru,
            };
            let stray: Vec<&str> = map
                .keys()
                .map(String::as_str)
                .filter(|k| k.starts_with(prefix) && !distinct.contains(k))
                .collect();
            assert!(
                stray.is_empty(),
                "{prefix}* ({lang:?}) stray ids: {stray:?}"
            );
        }
    }

    fn refused_part_suffix(part: crate::call::RefusedPart) -> &'static str {
        use crate::call::RefusedPart;
        match part {
            RefusedPart::Mic => "mic",
            RefusedPart::System => "system",
            RefusedPart::Root => "root",
        }
    }

    fn refused_reason_suffix(reason: crate::call::RefusedReason) -> &'static str {
        use crate::call::RefusedReason;
        match reason {
            RefusedReason::NoDevice => "no_device",
            RefusedReason::AccessDenied => "access_denied",
            RefusedReason::Busy => "busy",
            RefusedReason::Unwritable => "unwritable",
            RefusedReason::Missing => "missing",
            RefusedReason::Other => "other",
        }
    }

    fn fail_reason_suffix(reason: crate::call::FailReason) -> &'static str {
        use crate::call::FailReason;
        match reason {
            FailReason::Key => "key",
            FailReason::Network => "network",
            FailReason::Timeout => "timeout",
            FailReason::Provider => "provider",
            FailReason::Storage => "storage",
            FailReason::EngineNotSetUp => "engine_not_set_up",
        }
    }

    fn track_suffix(track: crate::call::Track) -> &'static str {
        use crate::call::Track;
        match track {
            Track::Mic => "mic",
            Track::System => "system",
        }
    }

    fn refused_parts() -> [crate::call::RefusedPart; 3] {
        use crate::call::RefusedPart;
        [RefusedPart::Mic, RefusedPart::System, RefusedPart::Root]
    }

    fn refused_reasons() -> [crate::call::RefusedReason; 6] {
        use crate::call::RefusedReason;
        [
            RefusedReason::NoDevice,
            RefusedReason::AccessDenied,
            RefusedReason::Busy,
            RefusedReason::Unwritable,
            RefusedReason::Missing,
            RefusedReason::Other,
        ]
    }

    fn fail_reasons() -> [crate::call::FailReason; 6] {
        use crate::call::FailReason;
        [
            FailReason::Key,
            FailReason::Network,
            FailReason::Timeout,
            FailReason::Provider,
            FailReason::Storage,
            FailReason::EngineNotSetUp,
        ]
    }

    fn tracks() -> [crate::call::Track; 2] {
        use crate::call::Track;
        [Track::Mic, Track::System]
    }

    #[test]
    fn call_fixed_ids_have_own_text_with_their_placeholders_and_the_right_origin() {
        // T-119 Acceptance 1 (contracts/ipc.md "Message catalog", #198 D4): each
        // fixed call id has its own non-empty, different text in en and ru, naming
        // exactly its placeholders; a Rust-originated id is declared with messages!
        // (so it is in MESSAGE_IDS and the parity tests cover it), a UI-only id is
        // not. Bite: an id missing from a catalog, a placeholder dropped or renamed
        // in one language, a RU text copied from EN, a UI-only id moved into
        // messages!, a Rust id left out of it.
        assert_eq!(CALL_FIXED_IDS.iter().filter(|r| r.2).count(), 25);
        assert_eq!(CALL_FIXED_IDS.iter().filter(|r| !r.2).count(), 28);
        let c = catalog(EN_JSON, RU_JSON);
        let mut wrong = Vec::new();
        for (id, names, rust) in CALL_FIXED_IDS {
            let declared = MESSAGE_IDS.iter().any(|m| m.0 == id);
            if rust && !declared {
                wrong.push(format!(
                    "{id}: Rust-originated, not declared with messages!"
                ));
            }
            if !rust && declared {
                wrong.push(format!("{id}: UI-only, but declared with messages!"));
            }
            let want: BTreeSet<&str> = names.iter().copied().collect();
            for lang in LANGS {
                match own_text(&c, lang, id) {
                    None => wrong.push(format!("{id} ({lang:?}): no text")),
                    Some(t) if placeholder_names(t) != want => wrong.push(format!(
                        "{id} ({lang:?}): placeholders {:?}, expected {want:?}",
                        placeholder_names(t)
                    )),
                    Some(_) => {}
                }
            }
            if let (Some(en), Some(ru)) = (
                own_text(&c, UiLanguage::En, id),
                own_text(&c, UiLanguage::Ru, id),
            ) {
                if en == ru {
                    wrong.push(format!("{id}: the ru text is the en text: {en:?}"));
                }
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    #[test]
    fn call_texts_the_contract_spells_out_are_exact() {
        // T-119 Acceptance 1 and 2: the English texts the contract table spells
        // out (the ones it leaves as "…" are the developer's wording, D5) and the
        // two RU speaker labels. Bite: any of these drifting from the contract, the
        // RU labels translated differently.
        let c = catalog(EN_JSON, RU_JSON);
        let en: [(&str, &str); 25] = [
            ("tray.call", "Call"),
            ("tray.call.record", "Record"),
            ("tray.call.stop", "Stop"),
            ("tray.calls", "Calls"),
            ("tray.tooltip.call_recording", "Voicen — recording a call"),
            ("indicator.recording", "● Recording"),
            ("indicator.stop", "Stop"),
            (
                "indicator.reminder",
                "Tell the other party the call is being recorded",
            ),
            ("indicator.headphones", "Better with headphones"),
            ("indicator.dismiss", "Dismiss"),
            (
                "warning.communications_device",
                "Calls may use a different output device than the one being recorded",
            ),
            (
                "warning.track_lost.mic",
                "Microphone lost — recording silence",
            ),
            (
                "warning.track_lost.system",
                "Output device lost — recording silence",
            ),
            (
                "warning.mic_changed",
                "Microphone changed to the Windows default",
            ),
            (
                "warning.cant_hear_other_party",
                "Can't hear the other party — check the output device",
            ),
            (
                "warning.cant_hear_you",
                "Can't hear you — check the microphone",
            ),
            (
                "warning.low_disk",
                "Less than 1 GB free in the calls folder",
            ),
            (
                "warning.provider_notice",
                "The other party's audio goes to provider {host}",
            ),
            ("notice.call_saved", "Call saved — transcribing"),
            ("notice.transcript_ready", "Transcript ready"),
            ("notice.open", "Open"),
            (
                "notice.call_hotkey_conflict",
                "The call hotkey is in use by another program",
            ),
            (
                "exit.recording.question",
                "A call is being recorded. Save it and exit?",
            ),
            ("speaker.me", "Me"),
            ("speaker.other", "Other party"),
        ];
        let mut wrong = Vec::new();
        for (id, want) in en {
            if own_text(&c, UiLanguage::En, id) != Some(want) {
                wrong.push(format!(
                    "{id} (En): {:?}, expected {want:?}",
                    own_text(&c, UiLanguage::En, id)
                ));
            }
        }
        for (id, want) in [("speaker.me", "Я"), ("speaker.other", "Собеседник")] {
            if own_text(&c, UiLanguage::Ru, id) != Some(want) {
                wrong.push(format!(
                    "{id} (Ru): {:?}, expected {want:?}",
                    own_text(&c, UiLanguage::Ru, id)
                ));
            }
        }
        for (id, want) in [
            ("calls.search", "Search"),
            ("calls.no_matches", "No matches"),
            ("calls.root_unavailable", "Folder not available: {root}"),
            (
                "settings.calls.provider_notice",
                "The other party's audio goes to provider {host}",
            ),
        ] {
            if own_text(&c, UiLanguage::En, id) != Some(want) {
                wrong.push(format!(
                    "{id} (En): {:?}, expected {want:?}",
                    own_text(&c, UiLanguage::En, id)
                ));
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    #[test]
    fn call_default_title_renders_the_day_month_and_time_in_both_languages() {
        // T-119 Acceptance 3 (data-model Call.title; the two-digit padding is the
        // caller's, T-125): a call of 5 March 09:07 is "Call 05.03 09:07". The RU
        // text carries the same four values in the same order. Looked up by
        // string, through the catalog, so a missing id fails here and not as a
        // compile error. Bite: a placeholder renamed or dropped, the day and month
        // swapped, a separator changed, the RU text without a value.
        let c = catalog(EN_JSON, RU_JSON);
        let args = [
            ("day", "05"),
            ("month", "03"),
            ("hour", "09"),
            ("minute", "07"),
        ];
        assert_eq!(
            own_text(&c, UiLanguage::En, "calls.title_default"),
            Some("Call {day}.{month} {hour}:{minute}"),
            "the EN template is the contract's"
        );
        assert_eq!(
            c.text(UiLanguage::En, "calls.title_default", &args),
            "Call 05.03 09:07"
        );
        assert!(
            own_text(&c, UiLanguage::Ru, "calls.title_default").is_some(),
            "calls.title_default has no ru text"
        );
        let ru = c.text(UiLanguage::Ru, "calls.title_default", &args);
        assert!(!ru.contains('{'), "unrendered placeholder: {ru:?}");
        let digits: String = ru.chars().filter(char::is_ascii_digit).collect();
        assert_eq!(digits, "05030907", "ru render: {ru:?}");
        let order: Vec<usize> = ["05", "03", "09", "07"]
            .iter()
            .map(|v| ru.find(v).unwrap_or(usize::MAX))
            .collect();
        assert!(
            order.windows(2).all(|w| w[0] < w[1]),
            "ru render {ru:?} does not show day, month, hour, minute in order"
        );
    }

    #[test]
    fn every_refused_part_and_reason_has_a_call_refused_notice() {
        // T-119 Acceptance 4, I3: `notice.call_refused.<part>.<reason>` for every
        // RefusedPart x RefusedReason (3 x 6 = 18), Rust-originated (the toast takes
        // a MessageId), text in en and ru without placeholders (D6: generic text
        // for pairs the session cannot produce). The suffix fns are exhaustive
        // matches: a new variant does not compile until it is named here. Bite: a
        // dropped pair, a pair missing from messages!, a pair missing in one
        // language, an id outside the product.
        let mut ids = Vec::new();
        for part in refused_parts() {
            for reason in refused_reasons() {
                ids.push(format!(
                    "notice.call_refused.{}.{}",
                    refused_part_suffix(part),
                    refused_reason_suffix(reason)
                ));
            }
        }
        assert_family("notice.call_refused.", ids, 18, true);
    }

    #[test]
    fn every_fail_reason_has_a_pass_failed_notice() {
        // T-119 I3: `notice.pass_failed.<FailReason>` (6), Rust-originated (the
        // transcription-failed toast), en and ru text, no placeholder. Bite: a
        // reason without an id, an id missing from messages! or a catalog.
        let ids = fail_reasons()
            .into_iter()
            .map(|r| format!("notice.pass_failed.{}", fail_reason_suffix(r)))
            .collect();
        assert_family("notice.pass_failed.", ids, 6, true);
    }

    #[test]
    fn every_fail_reason_has_a_calls_failed_text_that_is_ui_only() {
        // T-119 I3, #198 D4: `calls.failed.<FailReason>` (6) is shown by the Calls
        // page from the wire code, so it is a UI-only `t()` literal: in both JSON
        // files, not in messages!. Bite: a reason without text, the family moved
        // into messages!.
        let ids = fail_reasons()
            .into_iter()
            .map(|r| format!("calls.failed.{}", fail_reason_suffix(r)))
            .collect();
        assert_family("calls.failed.", ids, 6, false);
    }

    #[test]
    fn every_track_has_a_track_lost_warning() {
        // T-119 I3: `warning.track_lost.<Track>` (2), Rust-originated
        // (`IndicatorWarning.text` is rendered in Rust), en and ru text, no
        // placeholder. Bite: a track without an id, an id missing from messages!
        // or a catalog.
        let ids = tracks()
            .into_iter()
            .map(|t| format!("warning.track_lost.{}", track_suffix(t)))
            .collect();
        assert_family("warning.track_lost.", ids, 2, true);
    }

    #[test]
    fn dropped_call_storage_unwritable_id_stays_out_of_the_catalog() {
        // Decision #198 D2: `settings.calls.storage_unwritable` {reason} is dropped
        // (`error.storage.unwritable` of T-120 covers the refusal). Guard, green
        // today: it must not come back in a catalog or in messages!.
        let c = catalog(EN_JSON, RU_JSON);
        let id = "settings.calls.storage_unwritable";
        assert!(!MESSAGE_IDS.iter().any(|m| m.0 == id));
        assert!(!c.en.contains_key(id), "{id} is in en.json");
        assert!(!c.ru.contains_key(id), "{id} is in ru.json");
    }
}
