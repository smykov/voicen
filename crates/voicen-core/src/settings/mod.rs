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
//! always contain the field.

pub mod file;
pub mod gate;
pub mod hotkey;
pub mod service;
pub mod url;
pub mod validate;

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
        ui_language: resolve_ui_language(os_tag),
    }
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
    PostProcessingBaseUrl,
    PostProcessingModel,
    PostProcessingPrompt,
    /// The post-processing key input (not persisted).
    PostProcessingKey,
    HistoryEnabled,
    HistorySize,
    GeneralStartWithWindows,
    GeneralUiLanguage,
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
            FieldId::PostProcessingBaseUrl => "post_processing.base_url",
            FieldId::PostProcessingModel => "post_processing.model",
            FieldId::PostProcessingPrompt => "post_processing.prompt",
            FieldId::PostProcessingKey => "post_processing.key",
            FieldId::HistoryEnabled => "history.enabled",
            FieldId::HistorySize => "history.size",
            FieldId::GeneralStartWithWindows => "general.start_with_windows",
            FieldId::GeneralUiLanguage => "general.ui_language",
        }
    }
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
    pub(crate) const ALL: [ErrorCode; 14] = [
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
    ];
}

/// On the wire a code is [`ErrorCode::as_str`], the one spelling.
impl Serialize for ErrorCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// On the wire: `{"field": "<FieldId::as_str>", "code": "<ErrorCode::as_str>"}`; the
/// UI shows message `error.<code>` on the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct FieldError {
    pub field: FieldId,
    pub code: ErrorCode,
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

    /// Every `FieldId`, once (`field_ids_match_data_model` refuses a variant listed
    /// twice). Completeness is by hand: `expected_field_id`'s exhaustive match makes
    /// a new variant's contract string compile-required, but nothing here fails if
    /// the variant is missing from this table (the length is part of the type).
    const ALL_FIELD_IDS: [FieldId; 21] = [
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
        FieldId::PostProcessingBaseUrl,
        FieldId::PostProcessingModel,
        FieldId::PostProcessingPrompt,
        FieldId::PostProcessingKey,
        FieldId::HistoryEnabled,
        FieldId::HistorySize,
        FieldId::GeneralStartWithWindows,
        FieldId::GeneralUiLanguage,
    ];

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
            FieldId::PostProcessingBaseUrl => "post_processing.base_url",
            FieldId::PostProcessingModel => "post_processing.model",
            FieldId::PostProcessingPrompt => "post_processing.prompt",
            FieldId::PostProcessingKey => "post_processing.key",
            FieldId::HistoryEnabled => "history.enabled",
            FieldId::HistorySize => "history.size",
            FieldId::GeneralStartWithWindows => "general.start_with_windows",
            FieldId::GeneralUiLanguage => "general.ui_language",
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
        for id in ALL_FIELD_IDS {
            assert_eq!(id.as_str(), expected_field_id(id), "{id:?}");
            assert!(is_wire_name(id.as_str()), "{id:?}: {:?}", id.as_str());
        }
        // Each variant listed once, and no two share a string.
        let ids: std::collections::HashSet<_> = ALL_FIELD_IDS.into_iter().collect();
        assert_eq!(ids.len(), ALL_FIELD_IDS.len(), "a FieldId listed twice");
        let texts: std::collections::HashSet<_> =
            ALL_FIELD_IDS.iter().map(|id| id.as_str()).collect();
        assert_eq!(
            texts.len(),
            ALL_FIELD_IDS.len(),
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
        }
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
}
