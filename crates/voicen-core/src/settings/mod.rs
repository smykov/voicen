//! Settings model: one definition of every setting and of its defaults (FR-13, FR-21 v4).
//!
//! `Settings` has no key field (keys: [`crate::secrets`]). Every value missing from a
//! file, nested fields included, comes from [`defaults`] (container-level serde
//! default, not the field types' `Default`). The file, the service and the load
//! logic are T-032's; [`LoadOutcome`] is defined here because the startup gate
//! ([`gate::startup_action`]) decides on it.
//!
//! STUB (T-003 red tests): every body is `todo!()`; the developer implements them.

pub mod gate;
pub mod hotkey;
pub mod url;
pub mod validate;

use crate::i18n::UiLanguage;
use crate::post_process::settings::PostProcessingSettings;

/// Current (and only known) `schema_version`; a greater one is rejected.
pub const SCHEMA_VERSION: u32 = 1;

/// Selected transcription engine; serialized `none` | `api` | `builtin_local` | `local_server`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngineKind {
    None,
    Api,
    BuiltinLocal,
    LocalServer,
}

/// Recording mode; serialized `hold` | `toggle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    Hold,
    Toggle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiSettings {
    pub base_url: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalServerSettings {
    pub base_url: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltinLocalSettings {
    pub model_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Microphone {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistorySettings {
    pub enabled: bool,
    pub size: u32,
}

/// Every persisted setting (data-model.md › Settings). No key field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub schema_version: u32,
    pub engine: EngineKind,
    pub api: ApiSettings,
    pub local_server: LocalServerSettings,
    pub builtin_local: BuiltinLocalSettings,
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

// STUB: replace with derives (snake_case names, container-level default from
// `defaults`, `schema_version` > 1 rejected) — data-model.md, research R-2.
impl serde::Serialize for Settings {
    #[allow(unused_variables)] // STUB: body is todo!()
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        todo!("T-003: Settings Serialize")
    }
}

impl<'de> serde::Deserialize<'de> for Settings {
    #[allow(unused_variables)] // STUB: body is todo!()
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Settings, D::Error> {
        todo!("T-003: Settings Deserialize")
    }
}

/// The one source of defaults (FR-21 v4, spec 004 FR-005).
#[allow(unused_variables)] // STUB: body is todo!()
pub fn defaults(os_tag: Option<&str>) -> Settings {
    todo!("T-003: settings::defaults")
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
    EngineBuiltinLocalModelId,
    EngineSpeechLanguage,
    RecordingMicrophone,
    RecordingHotkey,
    RecordingMode,
    OutputAutoPaste,
    PostProcessingBaseUrl,
    PostProcessingModel,
    PostProcessingPrompt,
    HistoryEnabled,
    HistorySize,
    GeneralStartWithWindows,
    GeneralUiLanguage,
}

impl FieldId {
    /// The dotted id (`engine.api.base_url`).
    pub fn as_str(self) -> &'static str {
        todo!("T-003: FieldId::as_str")
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
    HotkeyUnavailable,
    HistorySizeRange,
    AutostartFailed,
    KeyStoreFailed,
}

impl ErrorCode {
    /// The wire code (`url.malformed`).
    pub fn as_str(self) -> &'static str {
        todo!("T-003: ErrorCode::as_str")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

    #[test]
    fn field_ids_match_data_model() {
        // Bite: a FieldId string that differs from data-model.md (UI highlight, logs).
        let table = [
            (FieldId::EngineKind, "engine.kind"),
            (FieldId::EngineApiBaseUrl, "engine.api.base_url"),
            (FieldId::EngineApiModel, "engine.api.model"),
            (FieldId::EngineApiKey, "engine.api.key"),
            (
                FieldId::EngineLocalServerBaseUrl,
                "engine.local_server.base_url",
            ),
            (FieldId::EngineLocalServerModel, "engine.local_server.model"),
            (
                FieldId::EngineBuiltinLocalModelId,
                "engine.builtin_local.model_id",
            ),
            (FieldId::EngineSpeechLanguage, "engine.speech_language"),
            (FieldId::RecordingMicrophone, "recording.microphone"),
            (FieldId::RecordingHotkey, "recording.hotkey"),
            (FieldId::RecordingMode, "recording.mode"),
            (FieldId::OutputAutoPaste, "output.auto_paste"),
            (FieldId::PostProcessingBaseUrl, "post_processing.base_url"),
            (FieldId::PostProcessingModel, "post_processing.model"),
            (FieldId::PostProcessingPrompt, "post_processing.prompt"),
            (FieldId::HistoryEnabled, "history.enabled"),
            (FieldId::HistorySize, "history.size"),
            (
                FieldId::GeneralStartWithWindows,
                "general.start_with_windows",
            ),
            (FieldId::GeneralUiLanguage, "general.ui_language"),
        ];
        for (id, text) in table {
            assert_eq!(id.as_str(), text, "{id:?}");
        }
    }

    #[test]
    fn error_codes_match_data_model() {
        // Bite: a code string that differs from data-model.md (message id error.<code>).
        let table = [
            (ErrorCode::Required, "required"),
            (ErrorCode::UrlMalformed, "url.malformed"),
            (ErrorCode::KeyRequired, "key.required"),
            (ErrorCode::ModelNotDownloaded, "model.not_downloaded"),
            (ErrorCode::HotkeyNoModifier, "hotkey.no_modifier"),
            (ErrorCode::HotkeyNoKey, "hotkey.no_key"),
            (ErrorCode::HotkeyEscReserved, "hotkey.esc_reserved"),
            (ErrorCode::HotkeyUnavailable, "hotkey.unavailable"),
            (ErrorCode::HistorySizeRange, "history.size_range"),
            (ErrorCode::AutostartFailed, "autostart.failed"),
            (ErrorCode::KeyStoreFailed, "key.store_failed"),
        ];
        for (code, text) in table {
            assert_eq!(code.as_str(), text, "{code:?}");
        }
    }
}
