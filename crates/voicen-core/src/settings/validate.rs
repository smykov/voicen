//! Save-time validation: one rule per line of spec 004 FR-004 (core part).
//!
//! Pure: no I/O, no side effects. Only the selected engine is validated
//! (Clarification Q2). Post-processing rules (003's `validate`) are added by
//! T-020/T-021; `hotkey.unavailable`, `autostart.failed` and `key.store_failed`
//! come from the save steps (T-010, T-014, T-032), not from here.
//!
//! STUB (T-003 red tests): every body is `todo!()`; the developer implements them.

use super::{FieldError, Settings};
use crate::models::DownloadedModels;
use crate::secrets::{KeyEdits, KeyPresence};

/// The key edits of a save request and which slots hold a key now.
#[derive(Debug, Clone, Copy)]
pub struct KeyEditsWithPresence<'a> {
    pub edits: &'a KeyEdits,
    pub presence: KeyPresence,
}

/// Every field error of `s`, all at once (empty = valid).
#[allow(unused_variables)] // STUB: body is todo!()
pub fn validate(
    s: &Settings,
    keys: &KeyEditsWithPresence<'_>,
    models: &dyn DownloadedModels,
) -> Vec<FieldError> {
    todo!("T-003: validate")
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::sample;
    use super::super::{EngineKind, ErrorCode, FieldId};
    use super::*;
    use crate::models::FakeDownloadedModels;
    use crate::secrets::{KeyEdit, Secret};

    const MALFORMED: [&str; 4] = ["api.openai.com", "htp://x", "https://", "ftp://host"];

    fn no_keys() -> KeyEdits {
        KeyEdits::default()
    }

    fn api_key_stored() -> KeyPresence {
        KeyPresence {
            transcription_api: true,
            local_server: false,
            post_processing: false,
        }
    }

    fn run(
        s: &Settings,
        edits: &KeyEdits,
        presence: KeyPresence,
        downloaded: &[&str],
    ) -> Vec<FieldError> {
        let models = FakeDownloadedModels::new(downloaded);
        validate(s, &KeyEditsWithPresence { edits, presence }, &models)
    }

    /// Errors as (field id, code) strings, sorted: compared against the wire names.
    fn pairs(errors: &[FieldError]) -> Vec<(&'static str, &'static str)> {
        let mut out: Vec<_> = errors
            .iter()
            .map(|e| (e.field.as_str(), e.code.as_str()))
            .collect();
        out.sort();
        out
    }

    /// A valid API setup: the sample with a stored API key.
    fn api_ok() -> (Settings, KeyEdits, KeyPresence) {
        (sample(EngineKind::Api), no_keys(), api_key_stored())
    }

    #[test]
    fn valid_settings_have_no_errors() {
        // Baseline for every rule below: the sample is valid for each engine.
        let (s, edits, presence) = api_ok();
        assert_eq!(run(&s, &edits, presence, &[]), vec![]);
        assert_eq!(
            run(
                &sample(EngineKind::None),
                &no_keys(),
                KeyPresence::default(),
                &[]
            ),
            vec![]
        );
        assert_eq!(
            run(
                &sample(EngineKind::LocalServer),
                &no_keys(),
                KeyPresence::default(),
                &[]
            ),
            vec![]
        );
        assert_eq!(
            run(
                &sample(EngineKind::BuiltinLocal),
                &no_keys(),
                KeyPresence::default(),
                &["base"]
            ),
            vec![]
        );
    }

    #[test]
    fn api_empty_base_url_refused() {
        // Decision #18: the field is engine.api.base_url; empty (after trim) = required.
        for raw in ["", "   "] {
            let (mut s, edits, presence) = api_ok();
            s.api.base_url = raw.into();
            assert_eq!(
                pairs(&run(&s, &edits, presence, &[])),
                vec![("engine.api.base_url", "required")],
                "{raw:?}"
            );
        }
    }

    #[test]
    fn api_malformed_base_url_refused() {
        for raw in MALFORMED {
            let (mut s, edits, presence) = api_ok();
            s.api.base_url = raw.into();
            assert_eq!(
                pairs(&run(&s, &edits, presence, &[])),
                vec![("engine.api.base_url", "url.malformed")],
                "{raw:?}"
            );
        }
    }

    #[test]
    fn api_base_url_is_checked_after_normalization() {
        // Spaces and one trailing slash are not errors (they are normalized away).
        let (mut s, edits, presence) = api_ok();
        s.api.base_url = "  https://api.example.com/v1/  ".into();
        assert_eq!(run(&s, &edits, presence, &[]), vec![]);
    }

    #[test]
    fn api_empty_model_refused() {
        for raw in ["", "  \t"] {
            let (mut s, edits, presence) = api_ok();
            s.api.model = raw.into();
            assert_eq!(
                pairs(&run(&s, &edits, presence, &[])),
                vec![("engine.api.model", "required")],
                "{raw:?}"
            );
        }
    }

    #[test]
    fn api_key_required_unless_stored_or_entered() {
        let s = sample(EngineKind::Api);
        let key_required = vec![("engine.api.key", "key.required")];

        // Untouched and nothing stored.
        assert_eq!(
            pairs(&run(&s, &no_keys(), KeyPresence::default(), &[])),
            key_required
        );

        // Clear of the stored key while the API engine is selected.
        let clear = KeyEdits {
            transcription_api: KeyEdit::Clear,
            ..KeyEdits::default()
        };
        assert_eq!(pairs(&run(&s, &clear, api_key_stored(), &[])), key_required);

        // Keys of other slots do not count for the API engine.
        let others = KeyPresence {
            transcription_api: false,
            local_server: true,
            post_processing: true,
        };
        assert_eq!(pairs(&run(&s, &no_keys(), others, &[])), key_required);

        // Untouched with a stored key: ok.
        assert_eq!(run(&s, &no_keys(), api_key_stored(), &[]), vec![]);

        // A key entered in this save: ok, even with nothing stored.
        let entered = KeyEdits {
            transcription_api: KeyEdit::Replace(Secret::new("sk-test-entered")),
            ..KeyEdits::default()
        };
        assert_eq!(run(&s, &entered, KeyPresence::default(), &[]), vec![]);
    }

    #[test]
    fn local_server_base_url_refused_when_empty_or_malformed() {
        for (raw, code) in [("", "required"), ("  ", "required")]
            .into_iter()
            .chain(MALFORMED.map(|m| (m, "url.malformed")))
        {
            let mut s = sample(EngineKind::LocalServer);
            s.local_server.base_url = raw.into();
            assert_eq!(
                pairs(&run(&s, &no_keys(), KeyPresence::default(), &[])),
                vec![("engine.local_server.base_url", code)],
                "{raw:?}"
            );
        }
    }

    #[test]
    fn local_server_model_and_key_are_optional() {
        // 002 FR-017. Bite: the API rules applied to the local server.
        let mut s = sample(EngineKind::LocalServer);
        s.local_server.model = String::new();
        assert_eq!(run(&s, &no_keys(), KeyPresence::default(), &[]), vec![]);
    }

    #[test]
    fn builtin_model_must_be_downloaded() {
        let not_downloaded = vec![("engine.builtin_local.model_id", "model.not_downloaded")];

        // No model selected.
        let mut s = sample(EngineKind::BuiltinLocal);
        s.builtin_local.model_id = None;
        assert_eq!(
            pairs(&run(&s, &no_keys(), KeyPresence::default(), &["base"])),
            not_downloaded
        );

        // A model that is not on disk (through the DownloadedModels fake).
        let mut s = sample(EngineKind::BuiltinLocal);
        s.builtin_local.model_id = Some("small".into());
        assert_eq!(
            pairs(&run(
                &s,
                &no_keys(),
                KeyPresence::default(),
                &["base", "tiny"]
            )),
            not_downloaded
        );
        assert_eq!(
            pairs(&run(&s, &no_keys(), KeyPresence::default(), &[])),
            not_downloaded
        );

        // The same model once downloaded.
        assert_eq!(
            run(&s, &no_keys(), KeyPresence::default(), &["base", "small"]),
            vec![]
        );
    }

    #[test]
    fn history_size_must_be_1_to_100() {
        for size in [0u32, 101, 1000, u32::MAX] {
            let (mut s, edits, presence) = api_ok();
            s.history.size = size;
            assert_eq!(
                pairs(&run(&s, &edits, presence, &[])),
                vec![("history.size", "history.size_range")],
                "{size}"
            );
        }
        for size in [1u32, 20, 100] {
            let (mut s, edits, presence) = api_ok();
            s.history.size = size;
            assert_eq!(run(&s, &edits, presence, &[]), vec![], "{size}");
        }
        // Checked even when history is off (the value is still saved).
        let (mut s, edits, presence) = api_ok();
        s.history.enabled = false;
        s.history.size = 0;
        assert_eq!(
            pairs(&run(&s, &edits, presence, &[])),
            vec![("history.size", "history.size_range")]
        );
    }

    #[test]
    fn hotkey_grammar_errors_refused() {
        for (raw, code) in [
            ("Space", "hotkey.no_modifier"),
            ("Ctrl+Alt", "hotkey.no_key"),
            ("Ctrl+Esc", "hotkey.esc_reserved"),
        ] {
            let (mut s, edits, presence) = api_ok();
            s.hotkey = raw.into();
            assert_eq!(
                pairs(&run(&s, &edits, presence, &[])),
                vec![("recording.hotkey", code)],
                "{raw:?}"
            );
        }
        // Property: no string the grammar rejects passes validation.
        for raw in ["", "Ctrl+Tab", "Ctrl+A+B", "Ctrl+F25", "ctrl alt space"] {
            let (mut s, edits, presence) = api_ok();
            s.hotkey = raw.into();
            let errors = run(&s, &edits, presence, &[]);
            assert_eq!(errors.len(), 1, "{raw:?}: {errors:?}");
            assert_eq!(errors[0].field, FieldId::RecordingHotkey, "{raw:?}");
            assert!(
                matches!(
                    errors[0].code,
                    ErrorCode::HotkeyNoModifier
                        | ErrorCode::HotkeyNoKey
                        | ErrorCode::HotkeyEscReserved
                ),
                "{raw:?}: {errors:?}"
            );
        }
    }

    /// Settings whose API, local-server, built-in and post-processing fields are all
    /// invalid, with the given engine selected.
    fn everything_invalid(engine: EngineKind) -> Settings {
        let mut s = sample(engine);
        s.api.base_url = "htp://x".into();
        s.api.model = String::new();
        s.local_server.base_url = String::new();
        s.builtin_local.model_id = None;
        s.post_processing.enabled = false;
        s.post_processing.base_url = "not a url".into();
        s.post_processing.model = String::new();
        s.post_processing.prompt = String::new();
        s
    }

    #[test]
    fn unselected_engines_not_validated() {
        // Clarification Q2. Bite: a rule that ignores the selected engine.
        let none = KeyPresence::default();
        assert_eq!(
            run(&everything_invalid(EngineKind::None), &no_keys(), none, &[]),
            vec![]
        );
        assert_eq!(
            pairs(&run(
                &everything_invalid(EngineKind::LocalServer),
                &no_keys(),
                none,
                &[]
            )),
            vec![("engine.local_server.base_url", "required")]
        );
        assert_eq!(
            pairs(&run(
                &everything_invalid(EngineKind::BuiltinLocal),
                &no_keys(),
                none,
                &[]
            )),
            vec![("engine.builtin_local.model_id", "model.not_downloaded")]
        );
        assert_eq!(
            pairs(&run(
                &everything_invalid(EngineKind::Api),
                &no_keys(),
                none,
                &[]
            )),
            vec![
                ("engine.api.base_url", "url.malformed"),
                ("engine.api.key", "key.required"),
                ("engine.api.model", "required"),
            ]
        );
    }

    #[test]
    fn post_processing_off_not_validated() {
        // While off, its fields are kept as entered and never refused (FR-004).
        let (mut s, edits, presence) = api_ok();
        s.post_processing.enabled = false;
        s.post_processing.base_url = "ftp://host".into();
        s.post_processing.model = String::new();
        s.post_processing.prompt = "   ".into();
        assert_eq!(run(&s, &edits, presence, &[]), vec![]);
    }

    #[test]
    fn all_errors_returned_at_once() {
        // SC-003: every offending field highlighted in one refusal, no duplicates.
        let mut s = sample(EngineKind::Api);
        s.api.base_url = String::new();
        s.api.model = " ".into();
        s.hotkey = "Ctrl+Esc".into();
        s.history.size = 101;
        let errors = run(&s, &no_keys(), KeyPresence::default(), &[]);
        assert_eq!(
            pairs(&errors),
            vec![
                ("engine.api.base_url", "required"),
                ("engine.api.key", "key.required"),
                ("engine.api.model", "required"),
                ("history.size", "history.size_range"),
                ("recording.hotkey", "hotkey.esc_reserved"),
            ]
        );
        assert_eq!(errors.len(), 5, "duplicates: {errors:?}");
    }
}
