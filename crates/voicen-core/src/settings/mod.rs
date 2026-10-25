//! Settings model: one definition of every setting and of its defaults (FR-13, FR-21 v4).
//!
//! `Settings` has no key field (keys: [`crate::secrets`]). Every value missing from a
//! file, nested fields included, comes from [`defaults`] (container-level serde
//! default, not the field types' `Default`). The file, the service and the load
//! logic are T-032's; [`LoadOutcome`] is defined here because the startup gate
//! ([`gate::startup_action`]) decides on it.
//!
//! A container default cannot know the OS language, so a file without
//! `ui_language` loads the language of `defaults(None)` (`en`); release-1 files
//! always contain the field. For the same reason the container default of
//! `calls.primary_language` is `en`; the one exception to "every missing value comes
//! from `defaults`" is [`parse_file`], the load path of the settings file, which
//! fills a missing `calls.primary_language` from the file's `ui_language` (decision
//! #196 (1), spec 007 FR-038).

pub mod file;
pub mod gate;
pub mod hotkey;
pub mod roots;
pub mod service;
pub mod url;
pub mod validate;

use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};

use crate::i18n::{resolve_ui_language, UiLanguage};
use crate::post_process::settings::{self as post_process_settings, PostProcessingSettings};

/// Current (and only known) `schema_version`; a greater one is rejected.
pub const SCHEMA_VERSION: u32 = 1;

/// The accepted values of `speech_language` besides `null` (decisions #27(3), #28):
/// every two-letter (ISO 639-1) code of the Whisper language list, `LANGUAGES` in
/// openai/whisper `whisper/tokenizer.py` at commit 86098128c0b4 (the list
/// whisper.cpp copies), 97 codes. Left out: the three-letter `haw` and `yue` (not
/// ISO 639-1) and Whisper's Javanese `jw` (ISO 639-1 says `jv`). Compared exactly:
/// case-sensitive, no trimming.
pub const WHISPER_ISO_639_1: [&str; 97] = [
    "en", "zh", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr", "pl", "ca", "nl", "ar", "sv", "it",
    "id", "hi", "fi", "vi", "he", "uk", "el", "ms", "cs", "ro", "da", "hu", "ta", "no", "th", "ur",
    "hr", "bg", "lt", "la", "mi", "ml", "cy", "sk", "te", "fa", "lv", "bn", "sr", "az", "sl", "kn",
    "et", "mk", "br", "eu", "is", "hy", "ne", "mn", "bs", "kk", "sq", "sw", "gl", "mr", "pa", "si",
    "km", "sn", "yo", "so", "af", "oc", "ka", "be", "tg", "sd", "gu", "am", "yi", "lo", "uz", "fo",
    "ht", "ps", "tk", "nn", "mt", "sa", "lb", "my", "bo", "tl", "mg", "as", "tt", "ln", "ha", "ba",
    "su",
];

/// Whether `code` is a language code of [`WHISPER_ISO_639_1`]: the one membership
/// test of every language code (`speech_language`, `calls.primary_language`,
/// [`LangCode::parse`]). Exact match: case-sensitive, no trimming.
pub fn is_lang_code(code: &str) -> bool {
    WHISPER_ISO_639_1.contains(&code)
}

/// A language code of [`WHISPER_ISO_639_1`] (decision #196 (4)); serialized as the
/// bare code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LangCode(&'static str);

impl LangCode {
    /// `Some` exactly when [`is_lang_code`] accepts `code`.
    pub fn parse(code: &str) -> Option<LangCode> {
        if !is_lang_code(code) {
            return None;
        }
        // The list's own `'static` text of the accepted code.
        WHISPER_ISO_639_1
            .iter()
            .find(|known| **known == code)
            .map(|known| LangCode(known))
    }

    pub fn as_str(self) -> &'static str {
        self.0
    }
}

impl Serialize for LangCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

/// Selected transcription engine; serialized `none` | `api` | `builtin_local` | `local_server`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    None,
    Api,
    BuiltinLocal,
    LocalServer,
}

/// Recording mode; serialized `hold` | `toggle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Hold,
    Toggle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default = "default_api")]
pub struct ApiSettings {
    pub base_url: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default = "default_local_server")]
pub struct LocalServerSettings {
    pub base_url: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default = "default_builtin_local")]
pub struct BuiltinLocalSettings {
    pub model_id: Option<String>,
}

/// A selected microphone. Both fields are required: the default is no microphone
/// at all (`null`), so there is no default to fill a partial object from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Microphone {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default = "default_history")]
pub struct HistorySettings {
    pub enabled: bool,
    pub size: u32,
}

/// The dictation timeouts in whole seconds (FR-24, decisions #97, #99). Bounds and
/// defaults: [`crate::timeouts::TimeoutRole::bounds`]; the durations a job uses:
/// [`crate::timeouts::Timeouts::from_settings`] of its snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default = "default_timeouts")]
pub struct TimeoutSettings {
    pub connect_s: u32,
    pub api_transcription_s: u32,
    pub local_server_s: u32,
    pub post_processing_s: u32,
    pub builtin_local_s: u32,
}

/// The toggles of the call widget (spec 007 data-model "Toggles").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default = "default_toggles")]
pub struct Toggles {
    pub record: bool,
}

/// Where the call indicator was last placed (screen pixels). Both fields are
/// required: the default is no position (`null`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndicatorPosition {
    pub x: i32,
    pub y: i32,
}

/// The call-mode settings (spec 007 data-model "CallSettings").
///
/// `hotkey` and `primary_language` are stored as text, like `Settings.hotkey` and
/// `speech_language`, so a bad hand-edited value is a field error at save, not a
/// whole-file reset. `last_combination`, `earlier_roots`, `warned_hosts` and
/// `indicator_position` are server-owned: a save takes them from the snapshot in
/// force, never from the draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default = "default_calls")]
pub struct CallSettings {
    /// Canonical hotkey text (`Ctrl+Alt+R`); checked by `validate`, never equal to
    /// the dictation hotkey.
    pub hotkey: String,
    pub last_combination: Toggles,
    pub reminder: bool,
    /// One of [`WHISPER_ISO_639_1`]; checked by `validate`. `null` in a file reads
    /// as the default (and [`parse_file`] fills it from `ui_language`).
    #[serde(deserialize_with = "deserialize_primary_language")]
    pub primary_language: String,
    /// `None` = the default root (`<data_dir>/calls`, [`roots::CallRoots::default_root`]).
    pub storage_root: Option<PathBuf>,
    /// Every effective root ever replaced, no duplicates, never the current one.
    pub earlier_roots: Vec<PathBuf>,
    pub warned_hosts: Vec<String>,
    pub indicator_position: Option<IndicatorPosition>,
}

/// Every persisted setting (data-model.md › Settings). No key field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default = "default_settings")]
pub struct Settings {
    /// Never above [`SCHEMA_VERSION`]: a newer file is rejected, not half-read.
    #[serde(deserialize_with = "deserialize_schema_version")]
    pub schema_version: u32,
    pub engine: EngineKind,
    pub api: ApiSettings,
    pub local_server: LocalServerSettings,
    pub builtin_local: BuiltinLocalSettings,
    /// `null` (auto-detect) or one of [`WHISPER_ISO_639_1`]; checked by `validate`.
    pub speech_language: Option<String>,
    pub microphone: Option<Microphone>,
    /// Canonical hotkey text (`Ctrl+Alt+Space`); checked by `validate`.
    pub hotkey: String,
    pub mode: Mode,
    pub auto_paste: bool,
    pub post_processing: PostProcessingSettings,
    pub history: HistorySettings,
    pub start_with_windows: bool,
    pub ui_language: UiLanguage,
    /// Checked by `validate`; clamped when converted for a job.
    pub timeouts: TimeoutSettings,
    /// Call mode (spec 007); a file without it loads `defaults(None).calls`, with the
    /// primary language filled by [`parse_file`].
    pub calls: CallSettings,
}

fn deserialize_schema_version<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let version = u32::deserialize(deserializer)?;
    if version > SCHEMA_VERSION {
        return Err(serde::de::Error::custom(format_args!(
            "unsupported schema_version {version} (known: {SCHEMA_VERSION})"
        )));
    }
    Ok(version)
}

/// The one source of defaults (FR-21 v4, spec 004 FR-005).
pub fn defaults(os_tag: Option<&str>) -> Settings {
    let ui_language = resolve_ui_language(os_tag);
    Settings {
        schema_version: SCHEMA_VERSION,
        engine: EngineKind::None,
        api: ApiSettings {
            base_url: "https://api.openai.com/v1".to_string(),
            model: "whisper-1".to_string(),
        },
        local_server: LocalServerSettings {
            base_url: "http://localhost:8000/v1".to_string(),
            model: String::new(),
        },
        builtin_local: BuiltinLocalSettings { model_id: None },
        speech_language: None,
        microphone: None,
        hotkey: "Ctrl+Alt+Space".to_string(),
        mode: Mode::Hold,
        auto_paste: true,
        post_processing: post_process_settings::defaults(),
        history: HistorySettings {
            enabled: true,
            size: 20,
        },
        start_with_windows: false,
        ui_language,
        timeouts: crate::timeouts::default_settings(),
        calls: CallSettings {
            hotkey: "Ctrl+Alt+R".to_string(),
            last_combination: Toggles { record: true },
            reminder: true,
            primary_language: ui_language_code(ui_language).to_string(),
            storage_root: None,
            earlier_roots: Vec::new(),
            warned_hosts: Vec::new(),
            indicator_position: None,
        },
    }
}

/// The language code of a UI language (both are in [`WHISPER_ISO_639_1`]).
fn ui_language_code(language: UiLanguage) -> &'static str {
    match language {
        UiLanguage::En => "en",
        UiLanguage::Ru => "ru",
    }
}

/// The one parse of a settings file (`SettingsService::load_or_init`): the plain
/// `Settings` parse, then, when the file has no `calls.primary_language` (absent or
/// `null`), the primary language from the file's `ui_language` (decision #196 (1)).
/// An unreadable file stays an error. A save request is never filled: the window
/// sends the whole view.
pub fn parse_file(bytes: &[u8]) -> Result<Settings, serde_json::Error> {
    let mut settings: Settings = serde_json::from_slice(bytes)?;
    let presence: FilePresence = serde_json::from_slice(bytes)?;
    let has_primary_language = presence
        .calls
        .is_some_and(|calls| calls.primary_language.is_some());
    if !has_primary_language {
        settings.calls.primary_language = ui_language_code(settings.ui_language).to_string();
    }
    Ok(settings)
}

/// What [`parse_file`] needs to know of the file beyond `Settings`: whether
/// `calls.primary_language` is there with a value.
#[derive(Deserialize)]
struct FilePresence {
    calls: Option<CallsPresence>,
}

#[derive(Deserialize)]
struct CallsPresence {
    primary_language: Option<serde::de::IgnoredAny>,
}

// Container defaults for serde: each one is a part of `defaults(None)`, so there is
// one source of default values.
fn default_settings() -> Settings {
    defaults(None)
}

fn default_api() -> ApiSettings {
    defaults(None).api
}

fn default_local_server() -> LocalServerSettings {
    defaults(None).local_server
}

fn default_builtin_local() -> BuiltinLocalSettings {
    defaults(None).builtin_local
}

fn default_history() -> HistorySettings {
    defaults(None).history
}

fn default_timeouts() -> TimeoutSettings {
    defaults(None).timeouts
}

fn default_calls() -> CallSettings {
    defaults(None).calls
}

/// `null` counts as missing: the default of `defaults(None)`; any other non-string
/// value stays an error.
fn deserialize_primary_language<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = Option::<String>::deserialize(deserializer)?;
    Ok(value.unwrap_or_else(|| defaults(None).calls.primary_language))
}

fn default_toggles() -> Toggles {
    defaults(None).calls.last_combination
}

/// A field as named in validation errors, the UI highlight and log lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldId {
    EngineKind,
    EngineApiBaseUrl,
    EngineApiModel,
    /// The API key input (not persisted); target of `key.required`.
    EngineApiKey,
    EngineLocalServerBaseUrl,
    EngineLocalServerModel,
    /// The local-server key input (not persisted).
    EngineLocalServerKey,
    EngineBuiltinLocalModelId,
    EngineSpeechLanguage,
    RecordingMicrophone,
    RecordingHotkey,
    RecordingMode,
    OutputAutoPaste,
    /// The post-processing toggle (a control's `data-field`; never refused).
    PostProcessingEnabled,
    PostProcessingBaseUrl,
    PostProcessingModel,
    PostProcessingPrompt,
    /// The post-processing key input (not persisted).
    PostProcessingKey,
    HistoryEnabled,
    HistorySize,
    GeneralStartWithWindows,
    GeneralUiLanguage,
    TimeoutsConnect,
    TimeoutsApiTranscription,
    TimeoutsLocalServer,
    TimeoutsPostProcessing,
    TimeoutsBuiltinLocal,
    CallHotkey,
    /// The reminder toggle (never refused).
    CallReminder,
    PrimaryLanguage,
    StorageRoot,
}

impl FieldId {
    /// The dotted id (`engine.api.base_url`).
    pub fn as_str(self) -> &'static str {
        match self {
            FieldId::EngineKind => "engine.kind",
            FieldId::EngineApiBaseUrl => "engine.api.base_url",
            FieldId::EngineApiModel => "engine.api.model",
            FieldId::EngineApiKey => "engine.api.key",
            FieldId::EngineLocalServerBaseUrl => "engine.local_server.base_url",
            FieldId::EngineLocalServerModel => "engine.local_server.model",
            FieldId::EngineLocalServerKey => "engine.local_server.key",
            FieldId::EngineBuiltinLocalModelId => "engine.builtin_local.model_id",
            FieldId::EngineSpeechLanguage => "engine.speech_language",
            FieldId::RecordingMicrophone => "recording.microphone",
            FieldId::RecordingHotkey => "recording.hotkey",
            FieldId::RecordingMode => "recording.mode",
            FieldId::OutputAutoPaste => "output.auto_paste",
            FieldId::PostProcessingEnabled => "post_processing.enabled",
            FieldId::PostProcessingBaseUrl => "post_processing.base_url",
            FieldId::PostProcessingModel => "post_processing.model",
            FieldId::PostProcessingPrompt => "post_processing.prompt",
            FieldId::PostProcessingKey => "post_processing.key",
            FieldId::HistoryEnabled => "history.enabled",
            FieldId::HistorySize => "history.size",
            FieldId::GeneralStartWithWindows => "general.start_with_windows",
            FieldId::GeneralUiLanguage => "general.ui_language",
            FieldId::TimeoutsConnect => "timeouts.connect",
            FieldId::TimeoutsApiTranscription => "timeouts.api_transcription",
            FieldId::TimeoutsLocalServer => "timeouts.local_server",
            FieldId::TimeoutsPostProcessing => "timeouts.post_processing",
            FieldId::TimeoutsBuiltinLocal => "timeouts.builtin_local",
            FieldId::CallHotkey => "calls.hotkey",
            FieldId::CallReminder => "calls.reminder",
            FieldId::PrimaryLanguage => "calls.primary_language",
            FieldId::StorageRoot => "calls.storage_root",
        }
    }
}

/// The one list of every `FieldId`, for tests (`tests::field_ids_match_data_model`,
/// `i18n::tests::every_field_id_has_label_text`).
///
/// Completeness is kept by hand, as for `ErrorCode::ALL`: a variant added to `FieldId`
/// must be added here as well. The wildcard-free matches (`as_str`, the tests'
/// `expected_field_id`) force a new match arm, not an entry in this list (the length is
/// part of the type); `field_ids_match_data_model` checks that no entry is listed twice.
#[cfg(test)]
impl FieldId {
    pub(crate) const ALL: [FieldId; 31] = [
        FieldId::EngineKind,
        FieldId::EngineApiBaseUrl,
        FieldId::EngineApiModel,
        FieldId::EngineApiKey,
        FieldId::EngineLocalServerBaseUrl,
        FieldId::EngineLocalServerModel,
        FieldId::EngineLocalServerKey,
        FieldId::EngineBuiltinLocalModelId,
        FieldId::EngineSpeechLanguage,
        FieldId::RecordingMicrophone,
        FieldId::RecordingHotkey,
        FieldId::RecordingMode,
        FieldId::OutputAutoPaste,
        FieldId::PostProcessingEnabled,
        FieldId::PostProcessingBaseUrl,
        FieldId::PostProcessingModel,
        FieldId::PostProcessingPrompt,
        FieldId::PostProcessingKey,
        FieldId::HistoryEnabled,
        FieldId::HistorySize,
        FieldId::GeneralStartWithWindows,
        FieldId::GeneralUiLanguage,
        FieldId::TimeoutsConnect,
        FieldId::TimeoutsApiTranscription,
        FieldId::TimeoutsLocalServer,
        FieldId::TimeoutsPostProcessing,
        FieldId::TimeoutsBuiltinLocal,
        FieldId::CallHotkey,
        FieldId::CallReminder,
        FieldId::PrimaryLanguage,
        FieldId::StorageRoot,
    ];
}

/// On the wire a field is its dotted id ([`FieldId::as_str`]), the one spelling.
impl Serialize for FieldId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Why a field was refused; each maps to message id `error.<code>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    Required,
    UrlMalformed,
    KeyRequired,
    ModelNotDownloaded,
    HotkeyNoModifier,
    HotkeyNoKey,
    HotkeyEscReserved,
    /// Any other hotkey grammar error: unknown token, a second key, an empty part,
    /// a repeated or out-of-order modifier (decision #25).
    HotkeyInvalid,
    HotkeyUnavailable,
    HistorySizeRange,
    AutostartFailed,
    KeyStoreFailed,
    /// A base URL with userinfo (`user:pass@`), decision #27(2).
    UrlCredentials,
    /// `speech_language` not `null` and not a Whisper ISO 639-1 code, decision #27(3).
    LanguageUnsupported,
    /// A timeouts field outside its bounds (decision #99); the range is in the
    /// field's hint, the error has no parameters.
    TimeoutRange,
    /// The call hotkey equals the dictation hotkey (spec 007 FR-004).
    HotkeyEqualsDictation,
    /// The storage root failed the write probe; the error carries an
    /// [`roots::UnwritableReason`].
    StorageUnwritable,
}

impl ErrorCode {
    /// The wire code (`url.malformed`).
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Required => "required",
            ErrorCode::UrlMalformed => "url.malformed",
            ErrorCode::KeyRequired => "key.required",
            ErrorCode::ModelNotDownloaded => "model.not_downloaded",
            ErrorCode::HotkeyNoModifier => "hotkey.no_modifier",
            ErrorCode::HotkeyNoKey => "hotkey.no_key",
            ErrorCode::HotkeyEscReserved => "hotkey.esc_reserved",
            ErrorCode::HotkeyInvalid => "hotkey.invalid",
            ErrorCode::HotkeyUnavailable => "hotkey.unavailable",
            ErrorCode::HistorySizeRange => "history.size_range",
            ErrorCode::AutostartFailed => "autostart.failed",
            ErrorCode::KeyStoreFailed => "key.store_failed",
            ErrorCode::UrlCredentials => "url.credentials",
            ErrorCode::LanguageUnsupported => "language.unsupported",
            ErrorCode::TimeoutRange => "timeout.range",
            ErrorCode::HotkeyEqualsDictation => "hotkey.equals_dictation",
            ErrorCode::StorageUnwritable => "storage.unwritable",
        }
    }
}

/// The one list of every `ErrorCode`, for tests (`tests::error_codes_match_data_model`,
/// `i18n::tests::every_error_code_has_catalog_text`).
///
/// Completeness is kept by hand: a variant added to `ErrorCode` must be added here as
/// well. The wildcard-free matches (`as_str`, the tests' `expected_error_code`) force a
/// new match arm, not an entry in this list; `error_codes_match_data_model` checks that
/// no entry is listed twice.
#[cfg(test)]
impl ErrorCode {
    pub(crate) const ALL: [ErrorCode; 17] = [
        ErrorCode::Required,
        ErrorCode::UrlMalformed,
        ErrorCode::KeyRequired,
        ErrorCode::ModelNotDownloaded,
        ErrorCode::HotkeyNoModifier,
        ErrorCode::HotkeyNoKey,
        ErrorCode::HotkeyEscReserved,
        ErrorCode::HotkeyInvalid,
        ErrorCode::HotkeyUnavailable,
        ErrorCode::HistorySizeRange,
        ErrorCode::AutostartFailed,
        ErrorCode::KeyStoreFailed,
        ErrorCode::UrlCredentials,
        ErrorCode::LanguageUnsupported,
        ErrorCode::TimeoutRange,
        ErrorCode::HotkeyEqualsDictation,
        ErrorCode::StorageUnwritable,
    ];
}

/// On the wire a code is [`ErrorCode::as_str`], the one spelling.
impl Serialize for ErrorCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// On the wire: `{"field": "<FieldId::as_str>", "code": "<ErrorCode::as_str>"}`, plus
/// `"reason": "<UnwritableReason>"` for `storage.unwritable` only; the UI shows
/// message `error.<code>` on the field. The log line names `field:code` only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct FieldError {
    pub field: FieldId,
    pub code: ErrorCode,
    /// Set only by [`FieldError::storage_unwritable`] (a closed reason, never a path
    /// or OS text).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<roots::UnwritableReason>,
}

impl FieldError {
    /// An error without a reason.
    pub fn new(field: FieldId, code: ErrorCode) -> FieldError {
        FieldError {
            field,
            code,
            reason: None,
        }
    }

    /// `calls.storage_root` / `storage.unwritable` with its reason.
    pub fn storage_unwritable(reason: roots::UnwritableReason) -> FieldError {
        FieldError {
            field: FieldId::StorageRoot,
            code: ErrorCode::StorageUnwritable,
            reason: Some(reason),
        }
    }
}

/// How startup found the settings (data-model.md › LoadOutcome). Produced by T-032's
/// `SettingsService::load_or_init`; consumed by [`gate::startup_action`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadOutcome {
    Loaded(Settings),
    FirstRun(Settings),
    Reset {
        settings: Settings,
        backup_file_name: String,
    },
    Unavailable(Settings),
}

/// Test fixtures shared by the settings modules: a valid, fully populated value
/// built without `defaults()`, so rule tests do not depend on it.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    /// Valid for every engine; every optional value set; fake hosts only.
    pub(crate) fn sample(engine: EngineKind) -> Settings {
        Settings {
            schema_version: 1,
            engine,
            api: ApiSettings {
                base_url: "https://api.example.com/v1".into(),
                model: "whisper-test".into(),
            },
            local_server: LocalServerSettings {
                base_url: "http://192.0.2.10:8000/v1".into(),
                model: "local-test".into(),
            },
            builtin_local: BuiltinLocalSettings {
                model_id: Some("base".into()),
            },
            speech_language: Some("de".into()),
            microphone: Some(Microphone {
                id: "{0.0.1.00000000}.{fake-device-id}".into(),
                name: "Test Microphone (fake)".into(),
            }),
            hotkey: "Ctrl+Shift+F9".into(),
            mode: Mode::Toggle,
            auto_paste: false,
            post_processing: PostProcessingSettings {
                enabled: true,
                base_url: "https://llm.example.com/v1".into(),
                model: "llm-test".into(),
                prompt: "Fix the text.".into(),
            },
            history: HistorySettings {
                enabled: false,
                size: 7,
            },
            start_with_windows: true,
            ui_language: UiLanguage::Ru,
            // T-073: in range, and no value equal to its default (decision #99).
            timeouts: TimeoutSettings {
                connect_s: 7,
                api_transcription_s: 45,
                local_server_s: 90,
                post_processing_s: 20,
                builtin_local_s: 150,
            },
            calls: sample_calls(),
        }
    }

    /// T-120: a valid `calls` section whose hotkey differs from the sample's
    /// dictation hotkey (`Ctrl+Shift+F9`). The server-owned fields (`last_combination`,
    /// `earlier_roots`, `warned_hosts`, `indicator_position`) and `storage_root` hold
    /// their defaults, so a test that saves the sample is not rewritten by the
    /// server-owned rule and triggers no root probe; `hotkey`, `reminder` and
    /// `primary_language` differ from the defaults.
    pub(crate) fn sample_calls() -> CallSettings {
        CallSettings {
            hotkey: "Ctrl+Shift+F11".into(),
            last_combination: Toggles { record: true },
            reminder: false,
            primary_language: "de".into(),
            storage_root: None,
            earlier_roots: Vec::new(),
            warned_hosts: Vec::new(),
            indicator_position: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::sample;
    use super::*;
    use crate::post_process::settings::{self as pp, STARTER_PROMPT};
    use serde_json::{json, Value};

    fn parse(text: &str) -> Settings {
        serde_json::from_str(text).unwrap_or_else(|e| panic!("{text} must load: {e}"))
    }

    #[test]
    fn defaults_match_fr21_v4() {
        // Bite: any single default changed in defaults().
        let d = defaults(Some("ru-RU"));
        assert_eq!(d.schema_version, 1);
        assert_eq!(d.engine, EngineKind::None);
        assert_eq!(d.hotkey, "Ctrl+Alt+Space");
        assert_eq!(d.mode, Mode::Hold);
        assert!(d.auto_paste);
        assert_eq!(d.speech_language, None);
        assert!(d.history.enabled);
        assert_eq!(d.history.size, 20);
        assert!(!d.start_with_windows);
        assert_eq!(d.microphone, None);
        assert_eq!(d.api.base_url, "https://api.openai.com/v1");
        assert_eq!(d.api.model, "whisper-1");
        assert_eq!(d.local_server.base_url, "http://localhost:8000/v1");
        assert_eq!(d.local_server.model, "");
        assert_eq!(d.builtin_local.model_id, None);
        assert_eq!(d.post_processing, pp::defaults());
        assert!(!d.post_processing.enabled);
        assert_eq!(d.post_processing.base_url, "");
        assert_eq!(d.post_processing.model, "");
        assert_eq!(d.post_processing.prompt, STARTER_PROMPT);
        assert_eq!(d.ui_language, UiLanguage::Ru);
    }

    #[test]
    fn defaults_hold_fr24_timeouts() {
        // Decision #97: FR-24's durations become settings whose defaults are
        // today's values (connect 5, API 30, local server 60, post-processing 15,
        // built-in 120; whole seconds). Bite: any value changed, two roles swapped,
        // or milliseconds stored.
        let t = defaults(Some("ru-RU")).timeouts;
        assert_eq!(t.connect_s, 5, "connect_s");
        assert_eq!(t.api_transcription_s, 30, "api_transcription_s");
        assert_eq!(t.local_server_s, 60, "local_server_s");
        assert_eq!(t.post_processing_s, 15, "post_processing_s");
        assert_eq!(t.builtin_local_s, 120, "builtin_local_s");
        assert_eq!(
            defaults(None).timeouts,
            t,
            "the OS language changes no timeout"
        );
        // The wire / file shape (contracts/ipc.md, data-model.md): one `timeouts`
        // object with the five `_s` fields, numbers.
        let json = serde_json::to_value(defaults(None)).expect("serializes");
        assert_eq!(
            json["timeouts"],
            json!({
                "connect_s": 5,
                "api_transcription_s": 30,
                "local_server_s": 60,
                "post_processing_s": 15,
                "builtin_local_s": 120,
            })
        );
    }

    #[test]
    fn defaults_take_ui_language_from_resolve_ui_language() {
        // Bite: ui_language hard-coded instead of resolve_ui_language(os_tag).
        for (tag, lang) in [
            (Some("ru-RU"), UiLanguage::Ru),
            (Some("ru"), UiLanguage::Ru),
            (Some("en-US"), UiLanguage::En),
            (Some("de-DE"), UiLanguage::En),
            (None, UiLanguage::En),
        ] {
            assert_eq!(defaults(tag).ui_language, lang, "{tag:?}");
        }
        // Nothing but the language depends on the OS tag.
        let mut ru = defaults(Some("ru-RU"));
        ru.ui_language = UiLanguage::En;
        // T-120: the UI language also sets the call primary language (FR-038).
        ru.calls.primary_language = "en".into();
        assert_eq!(ru, defaults(None));
    }

    #[test]
    fn round_trip_keeps_every_field() {
        // Bite: a field skipped or renamed asymmetrically in serde.
        let original = sample(EngineKind::BuiltinLocal);
        let text = serde_json::to_string(&original).expect("serializes");
        let back: Settings = serde_json::from_str(&text).expect("loads");
        assert_eq!(back, original);
        // Each engine kind and mode survives the trip.
        for engine in [
            EngineKind::None,
            EngineKind::Api,
            EngineKind::BuiltinLocal,
            EngineKind::LocalServer,
        ] {
            for mode in [Mode::Hold, Mode::Toggle] {
                let mut s = sample(engine);
                s.mode = mode;
                s.microphone = None;
                s.speech_language = None;
                s.builtin_local.model_id = None;
                let back: Settings =
                    serde_json::from_value(serde_json::to_value(&s).expect("serializes"))
                        .expect("loads");
                assert_eq!(back, s);
            }
        }
    }

    #[test]
    fn missing_fields_take_defaults() {
        // Bite: field-level #[serde(default)] (type defaults: false, 0, "") instead
        // of the container default from defaults().
        let d = defaults(None);
        assert_eq!(parse("{}"), d, "{{}}");
        assert_eq!(parse(r#"{"schema_version":1}"#), d, "schema_version only");

        // Values whose type default differs from defaults(): proven explicitly.
        let empty = parse("{}");
        assert!(empty.auto_paste);
        assert_eq!(empty.history.size, 20);
        assert!(empty.history.enabled);
        assert_eq!(empty.api.base_url, "https://api.openai.com/v1");
        assert_eq!(empty.hotkey, "Ctrl+Alt+Space");
        assert_eq!(empty.post_processing.prompt, STARTER_PROMPT);

        // Partial nested objects: the missing nested fields come from defaults() too.
        let s = parse(r#"{"history":{"enabled":false}}"#);
        assert!(!s.history.enabled);
        assert_eq!(s.history.size, 20);

        let s = parse(r#"{"api":{}}"#);
        assert_eq!(s.api, d.api);

        let s = parse(r#"{"api":{"model":"m-test"}}"#);
        assert_eq!(s.api.model, "m-test");
        assert_eq!(s.api.base_url, "https://api.openai.com/v1");

        let s = parse(r#"{"local_server":{"model":"local-test"}}"#);
        assert_eq!(s.local_server.model, "local-test");
        assert_eq!(s.local_server.base_url, "http://localhost:8000/v1");

        let s = parse(r#"{"post_processing":{"enabled":true}}"#);
        assert!(s.post_processing.enabled);
        assert_eq!(s.post_processing.prompt, STARTER_PROMPT);
        assert_eq!(s.post_processing.base_url, "");

        let s = parse(r#"{"builtin_local":{}}"#);
        assert_eq!(s.builtin_local.model_id, None);

        // T-073: a file written before the timeouts existed (no `timeouts` key)
        // loads the FR-24 defaults, not zeros; schema_version stays 1 (research
        // R-2). Bite: field-level type defaults (0 s: every request fails at once),
        // or no container default (the old file becomes unreadable and is reset).
        assert_eq!(empty.timeouts, d.timeouts);
        assert_eq!(empty.timeouts.connect_s, 5);
        assert_eq!(empty.timeouts.api_transcription_s, 30);
        assert_eq!(empty.timeouts.local_server_s, 60);
        assert_eq!(empty.timeouts.post_processing_s, 15);
        assert_eq!(empty.timeouts.builtin_local_s, 120);
        let s = parse(r#"{"timeouts":{}}"#);
        assert_eq!(s.timeouts, d.timeouts);
        // A partial timeouts object changes only what it names.
        let s = parse(r#"{"timeouts":{"local_server_s":900}}"#);
        assert_eq!(s.timeouts.local_server_s, 900);
        assert_eq!(s.timeouts.connect_s, 5);
        assert_eq!(s.timeouts.api_transcription_s, 30);
        assert_eq!(s.timeouts.post_processing_s, 15);
        assert_eq!(s.timeouts.builtin_local_s, 120);
        let s =
            parse(r#"{"engine":"local_server","timeouts":{"connect_s":2,"builtin_local_s":600}}"#);
        let mut expected = d.clone();
        expected.engine = EngineKind::LocalServer;
        expected.timeouts.connect_s = 2;
        expected.timeouts.builtin_local_s = 600;
        assert_eq!(s, expected);

        // A partial file changes only what it names.
        let s = parse(r#"{"engine":"api","history":{"size":5}}"#);
        let mut expected = d.clone();
        expected.engine = EngineKind::Api;
        expected.history.size = 5;
        assert_eq!(s, expected);
    }

    #[test]
    fn unknown_fields_ignored() {
        // Bite: #[serde(deny_unknown_fields)] on Settings or a nested struct.
        let s = parse(
            r#"{"schema_version":1,"future_setting":42,"engine":"local_server",
                "local_server":{"base_url":"http://192.0.2.1:8000/v1","extra":true},
                "history":{"size":3,"retention":"week"},"api_key":"sk-test-ignored"}"#, // teamwright:allow-secret (fake test value)
        );
        assert_eq!(s.engine, EngineKind::LocalServer);
        assert_eq!(s.local_server.base_url, "http://192.0.2.1:8000/v1");
        assert_eq!(s.history.size, 3);
        // An unknown field is dropped: it is not written back.
        let back = serde_json::to_string(&s).expect("serializes");
        assert!(!back.contains("future_setting"));
        assert!(!back.contains("sk-test-ignored"));
    }

    #[test]
    fn schema_version_above_1_rejected() {
        // Bite: schema_version read but not checked. A full, otherwise valid file is
        // used so the rejection cannot come from a missing field.
        let mut full = serde_json::to_value(sample(EngineKind::Api)).expect("serializes");
        assert!(serde_json::from_value::<Settings>(full.clone()).is_ok());
        for version in [2u32, 3, 99, u32::MAX] {
            full["schema_version"] = json!(version);
            assert!(
                serde_json::from_value::<Settings>(full.clone()).is_err(),
                "schema_version {version} must be rejected"
            );
        }
        assert!(serde_json::from_str::<Settings>(r#"{"schema_version":2}"#).is_err());
    }

    /// Every object key at every depth, as dotted paths.
    fn key_paths(value: &Value, prefix: &str, out: &mut Vec<String>) {
        if let Value::Object(map) = value {
            for (k, v) in map {
                let path = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                out.push(path.clone());
                key_paths(v, &path, out);
            }
        }
    }

    #[test]
    fn settings_json_has_no_key_field() {
        // Invariant (NFR-04): the serialized shape is exactly the data model, which
        // has no key field. Bite: any added field (api_key, key, secret, ...).
        let json = serde_json::to_value(sample(EngineKind::Api)).expect("serializes");
        let mut paths = Vec::new();
        key_paths(&json, "", &mut paths);
        paths.sort();
        let mut expected: Vec<String> = [
            "api",
            "api.base_url",
            "api.model",
            "auto_paste",
            "builtin_local",
            "builtin_local.model_id",
            "calls",
            "calls.earlier_roots",
            "calls.hotkey",
            "calls.indicator_position",
            "calls.last_combination",
            "calls.last_combination.record",
            "calls.primary_language",
            "calls.reminder",
            "calls.storage_root",
            "calls.warned_hosts",
            "engine",
            "history",
            "history.enabled",
            "history.size",
            "hotkey",
            "local_server",
            "local_server.base_url",
            "local_server.model",
            "microphone",
            "microphone.id",
            "microphone.name",
            "mode",
            "post_processing",
            "post_processing.base_url",
            "post_processing.enabled",
            "post_processing.model",
            "post_processing.prompt",
            "schema_version",
            "speech_language",
            "start_with_windows",
            "timeouts",
            "timeouts.api_transcription_s",
            "timeouts.builtin_local_s",
            "timeouts.connect_s",
            "timeouts.local_server_s",
            "timeouts.post_processing_s",
            "ui_language",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        expected.sort();
        assert_eq!(paths, expected);
    }

    #[test]
    fn wire_values_match_data_model() {
        // Bite: a rename of an enum value the UI and the file depend on.
        for (engine, wire) in [
            (EngineKind::None, "none"),
            (EngineKind::Api, "api"),
            (EngineKind::BuiltinLocal, "builtin_local"),
            (EngineKind::LocalServer, "local_server"),
        ] {
            let json = serde_json::to_value(sample(engine)).expect("serializes");
            assert_eq!(json["engine"], json!(wire), "{engine:?}");
        }
        let mut s = defaults(None);
        assert_eq!(
            serde_json::to_value(&s).expect("ser")["mode"],
            json!("hold")
        );
        s.mode = Mode::Toggle;
        let json = serde_json::to_value(&s).expect("serializes");
        assert_eq!(json["mode"], json!("toggle"));
        assert_eq!(json["ui_language"], json!("en"));
        assert_eq!(json["speech_language"], Value::Null);
        assert_eq!(json["microphone"], Value::Null);
        assert_eq!(json["builtin_local"]["model_id"], Value::Null);
        assert_eq!(json["schema_version"], json!(1));
    }

    /// The data-model string of each FieldId. No wildcard: a variant added to
    /// `FieldId` fails to compile here until its contract string is written down.
    fn expected_field_id(id: FieldId) -> &'static str {
        match id {
            FieldId::EngineKind => "engine.kind",
            FieldId::EngineApiBaseUrl => "engine.api.base_url",
            FieldId::EngineApiModel => "engine.api.model",
            FieldId::EngineApiKey => "engine.api.key",
            FieldId::EngineLocalServerBaseUrl => "engine.local_server.base_url",
            FieldId::EngineLocalServerModel => "engine.local_server.model",
            FieldId::EngineLocalServerKey => "engine.local_server.key",
            FieldId::EngineBuiltinLocalModelId => "engine.builtin_local.model_id",
            FieldId::EngineSpeechLanguage => "engine.speech_language",
            FieldId::RecordingMicrophone => "recording.microphone",
            FieldId::RecordingHotkey => "recording.hotkey",
            FieldId::RecordingMode => "recording.mode",
            FieldId::OutputAutoPaste => "output.auto_paste",
            // T-021: the post-processing toggle (a control's data-field, like
            // history.enabled).
            FieldId::PostProcessingEnabled => "post_processing.enabled",
            FieldId::PostProcessingBaseUrl => "post_processing.base_url",
            FieldId::PostProcessingModel => "post_processing.model",
            FieldId::PostProcessingPrompt => "post_processing.prompt",
            FieldId::PostProcessingKey => "post_processing.key",
            FieldId::HistoryEnabled => "history.enabled",
            FieldId::HistorySize => "history.size",
            FieldId::GeneralStartWithWindows => "general.start_with_windows",
            FieldId::GeneralUiLanguage => "general.ui_language",
            // T-073 (analysis seam): the five timeouts fields.
            FieldId::TimeoutsConnect => "timeouts.connect",
            FieldId::TimeoutsApiTranscription => "timeouts.api_transcription",
            FieldId::TimeoutsLocalServer => "timeouts.local_server",
            FieldId::TimeoutsPostProcessing => "timeouts.post_processing",
            FieldId::TimeoutsBuiltinLocal => "timeouts.builtin_local",
            // T-120 (decision #196): the four call-settings fields.
            FieldId::CallHotkey => "calls.hotkey",
            FieldId::CallReminder => "calls.reminder",
            FieldId::PrimaryLanguage => "calls.primary_language",
            FieldId::StorageRoot => "calls.storage_root",
        }
    }

    /// A data-model wire string: dot-separated segments, each `[a-z][a-z_]*`.
    fn is_wire_name(s: &str) -> bool {
        s.split('.').all(|seg| {
            seg.starts_with(|c: char| c.is_ascii_lowercase())
                && seg.chars().all(|c| c.is_ascii_lowercase() || c == '_')
        })
    }

    #[test]
    fn field_ids_match_data_model() {
        // Bite: a FieldId string that differs from data-model.md (UI highlight, logs),
        // including the key inputs of #25(a) (`engine.api.key`,
        // `engine.local_server.key`, `post_processing.key`).
        for id in FieldId::ALL {
            assert_eq!(id.as_str(), expected_field_id(id), "{id:?}");
            assert!(is_wire_name(id.as_str()), "{id:?}: {:?}", id.as_str());
        }
        // T-021: the toggle's FieldId is in the one list, so the catalog check
        // (i18n every_field_id_has_label_text) and the wire check above cover it.
        // Bite: PostProcessingEnabled left out of FieldId::ALL.
        assert!(
            FieldId::ALL.contains(&FieldId::PostProcessingEnabled),
            "PostProcessingEnabled not in FieldId::ALL"
        );
        assert_eq!(
            FieldId::PostProcessingEnabled.as_str(),
            "post_processing.enabled"
        );
        // Each variant listed once, and no two share a string.
        let ids: std::collections::HashSet<_> = FieldId::ALL.into_iter().collect();
        assert_eq!(ids.len(), FieldId::ALL.len(), "a FieldId listed twice");
        let texts: std::collections::HashSet<_> =
            FieldId::ALL.iter().map(|id| id.as_str()).collect();
        assert_eq!(
            texts.len(),
            FieldId::ALL.len(),
            "two FieldIds share a string"
        );
    }

    /// The data-model code of each ErrorCode. No wildcard (see `expected_field_id`).
    fn expected_error_code(code: ErrorCode) -> &'static str {
        match code {
            ErrorCode::Required => "required",
            ErrorCode::UrlMalformed => "url.malformed",
            ErrorCode::KeyRequired => "key.required",
            ErrorCode::ModelNotDownloaded => "model.not_downloaded",
            ErrorCode::HotkeyNoModifier => "hotkey.no_modifier",
            ErrorCode::HotkeyNoKey => "hotkey.no_key",
            ErrorCode::HotkeyEscReserved => "hotkey.esc_reserved",
            ErrorCode::HotkeyInvalid => "hotkey.invalid",
            ErrorCode::HotkeyUnavailable => "hotkey.unavailable",
            ErrorCode::HistorySizeRange => "history.size_range",
            ErrorCode::AutostartFailed => "autostart.failed",
            ErrorCode::KeyStoreFailed => "key.store_failed",
            ErrorCode::UrlCredentials => "url.credentials",
            ErrorCode::LanguageUnsupported => "language.unsupported",
            // T-073: one code for every timeouts field (the range is in each hint).
            ErrorCode::TimeoutRange => "timeout.range",
            // T-120 (decision #196).
            ErrorCode::HotkeyEqualsDictation => "hotkey.equals_dictation",
            ErrorCode::StorageUnwritable => "storage.unwritable",
        }
    }

    #[test]
    fn timeout_ids_are_listed() {
        // T-073: FieldId::ALL / ErrorCode::ALL drive the catalog checks
        // (i18n::tests::every_field_id_has_label_text, every_error_code_has_catalog_text)
        // and the wire checks above; a new variant left out of them is checked by
        // nothing. Bite: a timeouts FieldId or TimeoutRange missing from ALL.
        for id in [
            FieldId::TimeoutsConnect,
            FieldId::TimeoutsApiTranscription,
            FieldId::TimeoutsLocalServer,
            FieldId::TimeoutsPostProcessing,
            FieldId::TimeoutsBuiltinLocal,
        ] {
            assert!(FieldId::ALL.contains(&id), "{id:?} not in FieldId::ALL");
        }
        assert!(
            ErrorCode::ALL.contains(&ErrorCode::TimeoutRange),
            "TimeoutRange not in ErrorCode::ALL"
        );
    }

    #[test]
    fn error_codes_match_data_model() {
        // Bite: a code string that differs from data-model.md / decisions #25(c), #27
        // (message id error.<code>), `hotkey.invalid` included.
        for code in ErrorCode::ALL {
            assert_eq!(code.as_str(), expected_error_code(code), "{code:?}");
            assert!(is_wire_name(code.as_str()), "{code:?}: {:?}", code.as_str());
        }
        let codes: std::collections::HashSet<_> = ErrorCode::ALL.into_iter().collect();
        assert_eq!(
            codes.len(),
            ErrorCode::ALL.len(),
            "an ErrorCode listed twice"
        );
        let texts: std::collections::HashSet<_> =
            ErrorCode::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(
            texts.len(),
            ErrorCode::ALL.len(),
            "two codes share a string"
        );
    }

    #[test]
    fn whisper_list_is_unique_two_letter_lowercase() {
        // The one speech-language list (#27(3), #28): each entry exactly two ASCII
        // lowercase letters (ISO 639-1 form, compared case-sensitively by
        // `validate`), no entry twice, and none of the Whisper codes left out on
        // purpose (`jw`: ISO 639-1 says `jv`; `haw`, `yue`: not ISO 639-1). The
        // length is fixed by the array type, so it is not asserted.
        // Bite: an entry typed in upper case (`"SU"`), with a space or a third
        // letter, a typo that duplicates another entry (`"su"` -> `"sw"`), or `jw`
        // put back.
        let mut seen = std::collections::HashSet::new();
        for code in WHISPER_ISO_639_1.iter() {
            assert!(
                code.len() == 2 && code.bytes().all(|b| b.is_ascii_lowercase()),
                "{code:?} is not two ASCII lowercase letters"
            );
            assert!(seen.insert(*code), "{code:?} listed twice");
        }
        for left_out in ["jw", "haw", "yue"] {
            assert!(
                !WHISPER_ISO_639_1.contains(&left_out),
                "{left_out:?} must not be listed"
            );
        }
    }

    #[test]
    fn schema_version_0_loads() {
        // data-model: only `> 1` is rejected; `0` loads and keeps its value.
        // Bite: (M9) `version > SCHEMA_VERSION` -> `version != SCHEMA_VERSION`.
        let mut full = serde_json::to_value(sample(EngineKind::Api)).expect("serializes");
        full["schema_version"] = json!(0);
        let loaded: Settings = serde_json::from_value(full).expect("schema_version 0 loads");
        assert_eq!(loaded.schema_version, 0);
        let mut expected = sample(EngineKind::Api);
        expected.schema_version = 0;
        assert_eq!(loaded, expected);
        assert_eq!(parse(r#"{"schema_version":0}"#).schema_version, 0);
    }

    #[test]
    fn partial_microphone_object_is_unreadable() {
        // A `microphone` object missing `id` or `name` fails to parse, so T-032's
        // loader treats the file as unreadable (spec Clarification Q4: backup +
        // defaults). There is no default device to fill a partial object from.
        // Bite: `#[serde(default)]` (or a container default) on `Microphone`.
        let mut full = serde_json::to_value(sample(EngineKind::Api)).expect("serializes");
        for partial in [
            json!({"id": "x"}),
            json!({"name": "Test Microphone (fake)"}),
            json!({}),
        ] {
            full["microphone"] = partial.clone();
            assert!(
                serde_json::from_value::<Settings>(full.clone()).is_err(),
                "microphone {partial} must not load"
            );
        }
        assert!(serde_json::from_str::<Settings>(r#"{"microphone":{"id":"x"}}"#).is_err());
        // Whole object and null both load.
        full["microphone"] = json!({"id": "x", "name": "y"});
        assert!(serde_json::from_value::<Settings>(full.clone()).is_ok());
        full["microphone"] = Value::Null;
        assert_eq!(
            serde_json::from_value::<Settings>(full)
                .expect("null loads")
                .microphone,
            None
        );
    }

    // ---- T-120: the `calls` section (spec 007 G06; decisions #196, #198) ----------

    fn calls_json_of_defaults(primary_language: &str) -> Value {
        json!({
            "hotkey": "Ctrl+Alt+R",
            "last_combination": { "record": true },
            "reminder": true,
            "primary_language": primary_language,
            "storage_root": null,
            "earlier_roots": [],
            "warned_hosts": [],
            "indicator_position": null,
        })
    }

    #[test]
    fn calls_defaults_match_data_model() {
        // data-model.md "CallSettings". Bite: any single default changed in
        // defaults(); primary_language not taken from the resolved UI language; a
        // field renamed or skipped in serde (the file shape).
        let d = defaults(None).calls;
        assert_eq!(d.hotkey, "Ctrl+Alt+R");
        assert_eq!(d.last_combination, Toggles { record: true });
        assert!(d.reminder);
        assert_eq!(d.primary_language, "en");
        assert_eq!(d.storage_root, None);
        assert!(d.earlier_roots.is_empty());
        assert!(d.warned_hosts.is_empty());
        assert!(d.indicator_position.is_none());
        assert_eq!(
            serde_json::to_value(defaults(None)).expect("serializes")["calls"],
            calls_json_of_defaults("en")
        );
        // The primary language follows the resolved UI language, nothing else of
        // the section does.
        let ru = defaults(Some("ru-RU")).calls;
        assert_eq!(ru.primary_language, "ru");
        assert_eq!(
            serde_json::to_value(&ru).expect("serializes"),
            calls_json_of_defaults("ru")
        );
        assert_eq!(defaults(Some("de-DE")).calls.primary_language, "en");
        // The version of the file format does not move (data-model: container
        // default, no migration).
        assert_eq!(SCHEMA_VERSION, 1);
        assert_eq!(defaults(Some("ru-RU")).schema_version, 1);
        assert_eq!(
            serde_json::to_value(defaults(None)).expect("serializes")["schema_version"],
            json!(1)
        );
    }

    #[test]
    fn older_file_without_calls_loads_with_defaults() {
        // Bite: no container default on `calls` (a release-1 file fails to parse and
        // is reset), field-level `#[serde(default)]` (type defaults: "", false), or a
        // bumped SCHEMA_VERSION.
        let d = defaults(None);
        for text in [
            "{}",
            r#"{"schema_version":1}"#,
            r#"{"calls":{}}"#,
            r#"{"schema_version":1,"calls":{}}"#,
        ] {
            assert_eq!(parse(text), d, "{text}");
            assert_eq!(parse(text).calls, d.calls, "{text}");
        }
        // A full release-1 file: the sample as it was written before `calls` existed.
        let mut release1 = serde_json::to_value(sample(EngineKind::Api)).expect("serializes");
        release1
            .as_object_mut()
            .expect("object")
            .remove("calls")
            .expect("the sample serializes a calls section");
        let loaded: Settings = serde_json::from_value(release1).expect("a release-1 file loads");
        let mut expected = sample(EngineKind::Api);
        expected.calls = defaults(None).calls;
        assert_eq!(loaded, expected);
        assert_eq!(loaded.schema_version, 1);
    }

    #[test]
    fn partial_calls_object_changes_only_what_it_names() {
        // Bite: field-level type defaults inside `calls` or `last_combination`
        // (reminder false, hotkey "", record false), or a missing nested default.
        type Change = fn(&mut CallSettings);
        let cases: [(&str, Change); 8] = [
            (r#"{"calls":{"reminder":false}}"#, |c| c.reminder = false),
            (r#"{"calls":{"hotkey":"Ctrl+Shift+F10"}}"#, |c| {
                c.hotkey = "Ctrl+Shift+F10".into()
            }),
            (r#"{"calls":{"primary_language":"de"}}"#, |c| {
                c.primary_language = "de".into()
            }),
            (r#"{"calls":{"storage_root":"calls-x"}}"#, |c| {
                c.storage_root = Some("calls-x".into())
            }),
            (r#"{"calls":{"earlier_roots":["a","b"]}}"#, |c| {
                c.earlier_roots = vec!["a".into(), "b".into()]
            }),
            (r#"{"calls":{"warned_hosts":["api.example.com"]}}"#, |c| {
                c.warned_hosts = vec!["api.example.com".into()]
            }),
            (r#"{"calls":{"last_combination":{}}}"#, |_| {}),
            (r#"{"calls":{"last_combination":{"record":false}}}"#, |c| {
                c.last_combination = Toggles { record: false }
            }),
        ];
        for (text, change) in cases {
            let mut expected = defaults(None);
            change(&mut expected.calls);
            assert_eq!(parse(text), expected, "{text}");
        }
        // An unknown key of the section is dropped, not written back.
        let s = parse(r#"{"calls":{"future_flag":true}}"#);
        assert_eq!(s, defaults(None));
        assert!(!serde_json::to_string(&s)
            .expect("serializes")
            .contains("future_flag"));
    }

    #[test]
    fn calls_section_round_trips_with_every_field_set() {
        // Bite: a field of the section skipped or renamed asymmetrically in serde.
        let mut value = serde_json::to_value(sample(EngineKind::Api)).expect("serializes");
        value["calls"] = json!({
            "hotkey": "Ctrl+Shift+F10",
            "last_combination": { "record": false },
            "reminder": false,
            "primary_language": "de",
            "storage_root": "calls-x",
            "earlier_roots": ["calls-a", "calls-b"],
            "warned_hosts": ["api.example.com"],
            "indicator_position": { "x": -5, "y": 12 },
        });
        let s: Settings = serde_json::from_value(value.clone()).expect("loads");
        assert_eq!(s.calls.storage_root, Some("calls-x".into()));
        assert_eq!(s.calls.earlier_roots.len(), 2);
        assert_eq!(s.calls.last_combination, Toggles { record: false });
        assert_eq!(
            serde_json::to_value(&s).expect("serializes")["calls"],
            value["calls"]
        );
    }

    #[test]
    fn parse_file_fills_a_missing_primary_language_from_ui_language() {
        // Decision #196 (1), FR-038: the one load path fills `calls.primary_language`
        // from the loaded `ui_language` when the file lacks it. Bite: no fill (the
        // plain container default `en` stays for a Russian user), the fill ignoring
        // `calls` that exist without the key, an explicit value overwritten, or `null`
        // not counted as missing.
        let cases = [
            (r#"{"ui_language":"ru"}"#, "ru"),
            (r#"{"ui_language":"ru","calls":{}}"#, "ru"),
            (r#"{"calls":{},"ui_language":"ru"}"#, "ru"),
            (
                r#"{"ui_language":"ru","calls":{"primary_language":null}}"#,
                "ru",
            ),
            (r#"{"ui_language":"ru","calls":{"reminder":false}}"#, "ru"),
            (r#"{"ui_language":"en"}"#, "en"),
            (r#"{}"#, "en"),
            // An explicit value is kept, valid or not (validate decides at save).
            (
                r#"{"ui_language":"ru","calls":{"primary_language":"en"}}"#,
                "en",
            ),
            (
                r#"{"ui_language":"en","calls":{"primary_language":"de"}}"#,
                "de",
            ),
            (
                r#"{"ui_language":"ru","calls":{"primary_language":"xx"}}"#,
                "xx",
            ),
            (
                r#"{"ui_language":"ru","calls":{"primary_language":""}}"#,
                "",
            ),
        ];
        for (text, language) in cases {
            let loaded =
                parse_file(text.as_bytes()).unwrap_or_else(|e| panic!("{text} must load: {e}"));
            assert_eq!(loaded.calls.primary_language, language, "{text}");
            // Nothing but primary_language differs from the plain parse.
            let mut plain = parse(text);
            plain.calls.primary_language = language.into();
            assert_eq!(loaded, plain, "{text}");
        }
        // A full release-1 file (the sample is Russian) gets Russian.
        let mut release1 = serde_json::to_value(sample(EngineKind::Api)).expect("serializes");
        release1.as_object_mut().expect("object").remove("calls");
        let loaded = parse_file(&serde_json::to_vec(&release1).expect("bytes")).expect("loads");
        assert_eq!(loaded.ui_language, UiLanguage::Ru);
        assert_eq!(loaded.calls.primary_language, "ru");
        assert_eq!(loaded.calls.hotkey, "Ctrl+Alt+R");
    }

    #[test]
    fn parse_file_refuses_what_the_plain_parse_refuses() {
        // The fill is not a repair: unreadable stays unreadable (the service resets
        // it). Bite: errors swallowed into defaults, a wrong type for
        // primary_language treated as missing, schema_version above 1 accepted.
        for bytes in [
            &b"{not json"[..],
            b"",
            br#"{"schema_version":2}"#,
            br#"{"calls":{"primary_language":5}}"#,
            br#"{"calls":{"reminder":"yes"}}"#,
            br#"{"microphone":{"id":"x"}}"#,
        ] {
            assert!(
                parse_file(bytes).is_err(),
                "{} must not load",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    #[test]
    fn container_default_for_primary_language_stays_en() {
        // The limitation settings/mod.rs documents: a container default cannot see
        // `ui_language`. The fill is `parse_file`'s alone; a plain parse (a save
        // request, a consumer) never derives the value. Bite: the fill moved into
        // Deserialize / defaults().
        assert_eq!(
            parse(r#"{"ui_language":"ru"}"#).calls.primary_language,
            "en"
        );
        // An explicit `null` counts as missing: the `defaults(None)` value, not a
        // second literal (bite: `deserialize_primary_language`'s fallback changed).
        assert_eq!(
            parse(r#"{"ui_language":"ru","calls":{"primary_language":null}}"#)
                .calls
                .primary_language,
            defaults(None).calls.primary_language
        );
    }

    #[test]
    fn is_lang_code_is_exactly_the_whisper_list() {
        // One membership test for every language code (invariant I4; decision #196
        // (4)). Bite: case-insensitive or trimmed matching, a prefix match, `jw`.
        for code in WHISPER_ISO_639_1 {
            assert!(is_lang_code(code), "{code}");
            let lang = LangCode::parse(code).unwrap_or_else(|| panic!("{code} must parse"));
            assert_eq!(lang.as_str(), code);
            assert_eq!(
                serde_json::to_value(lang).expect("serializes"),
                json!(code),
                "LangCode serializes as the bare code"
            );
        }
        for bad in [
            "",
            " ",
            "EN",
            "En",
            "en ",
            " en",
            "en\n",
            "eng",
            "e",
            "jw",
            "haw",
            "yue",
            "xx",
            "ру",
            "русский",
            "en-US",
        ] {
            assert!(!is_lang_code(bad), "{bad:?}");
            assert!(LangCode::parse(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn call_ids_are_listed_and_named() {
        // Decision #196 names. Bite: a variant missing from the one lists (the catalog
        // checks and the wire checks iterate them), a wire string other than the
        // dotted id, ALL not grown with the enum.
        for (id, wire) in [
            (FieldId::CallHotkey, "calls.hotkey"),
            (FieldId::CallReminder, "calls.reminder"),
            (FieldId::PrimaryLanguage, "calls.primary_language"),
            (FieldId::StorageRoot, "calls.storage_root"),
        ] {
            assert_eq!(id.as_str(), wire);
            assert!(FieldId::ALL.contains(&id), "{id:?} not in FieldId::ALL");
            assert_eq!(serde_json::to_value(id).expect("serializes"), json!(wire));
        }
        for (code, wire) in [
            (ErrorCode::HotkeyEqualsDictation, "hotkey.equals_dictation"),
            (ErrorCode::StorageUnwritable, "storage.unwritable"),
        ] {
            assert_eq!(code.as_str(), wire);
            assert!(
                ErrorCode::ALL.contains(&code),
                "{code:?} not in ErrorCode::ALL"
            );
            assert_eq!(serde_json::to_value(code).expect("serializes"), json!(wire));
        }
        assert_eq!(FieldId::ALL.len(), 31);
        assert_eq!(ErrorCode::ALL.len(), 17);
    }

    #[test]
    fn field_error_carries_a_reason_only_for_storage_unwritable() {
        // Invariant I5, decision #196 (2). Bite: `reason` serialized as null when
        // absent (every existing wire shape changes), the reason dropped, the reason
        // spelled in camel case, a path or OS text in place of the closed enum.
        use super::roots::UnwritableReason as R;
        let plain = FieldError::new(FieldId::EngineKind, ErrorCode::Required);
        assert_eq!(plain.field, FieldId::EngineKind);
        assert_eq!(plain.code, ErrorCode::Required);
        assert_eq!(plain.reason, None);
        assert_eq!(
            serde_json::to_value(plain).expect("serializes"),
            json!({ "field": "engine.kind", "code": "required" }),
            "no `reason` key when there is none"
        );
        let table = [
            (R::Missing, "missing"),
            (R::NotAFolder, "not_a_folder"),
            (R::Relative, "relative"),
            (R::AccessDenied, "access_denied"),
            (R::DiskFull, "disk_full"),
            (R::Other, "other"),
        ];
        for (reason, wire) in table {
            let e = FieldError::storage_unwritable(reason);
            assert_eq!(e.field, FieldId::StorageRoot);
            assert_eq!(e.code, ErrorCode::StorageUnwritable);
            assert_eq!(e.reason, Some(reason));
            assert_eq!(
                serde_json::to_value(e).expect("serializes"),
                json!({
                    "field": "calls.storage_root",
                    "code": "storage.unwritable",
                    "reason": wire,
                })
            );
        }
        // The reason is part of the identity, and the type stays a plain value.
        assert_ne!(
            FieldError::storage_unwritable(R::Missing),
            FieldError::storage_unwritable(R::Other)
        );
        fn plain_value<T: Copy + Eq + std::hash::Hash + std::fmt::Debug>() {}
        plain_value::<FieldError>();
        plain_value::<R>();
    }
}
