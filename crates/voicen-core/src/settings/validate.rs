//! Save-time validation: one rule per line of spec 004 FR-004 (core part).
//!
//! Pure: no I/O, no side effects. Of the per-engine fields, only the selected
//! engine's are validated (Clarification Q2); `speech_language`, the hotkey, the
//! history size and the timeouts (decision #99) are checked for every engine.
//! Post-processing rules (003's `validate`) are added by T-020/T-021; `hotkey.unavailable`, `autostart.failed` and `key.store_failed`
//! come from the save steps (T-010, T-014, T-032), not from here.

use super::hotkey::{parse_hotkey, HotkeyError};
use super::url::{check_base_url, UrlError};
use super::{EngineKind, ErrorCode, FieldError, FieldId, Settings, WHISPER_ISO_639_1};
use crate::models::DownloadedModels;
use crate::secrets::{KeyEdit, KeyEdits, KeyPresence, KeySlot};
use crate::timeouts::TimeoutRole;

/// Valid values of `history.size` (FR-16).
const HISTORY_SIZE: std::ops::RangeInclusive<u32> = 1..=100;

/// The key edits of a save request and which slots hold a key now.
#[derive(Debug, Clone, Copy)]
pub struct KeyEditsWithPresence<'a> {
    pub edits: &'a KeyEdits,
    pub presence: KeyPresence,
}

impl KeyEditsWithPresence<'_> {
    /// Whether `slot` holds a key once the save is applied: a key entered now, or an
    /// untouched slot that holds one. A `Replace` that is empty after `trim()` is not
    /// an entered key (decision #27(1)); removing a key is the explicit `Clear`.
    fn has_key_after_save(&self, slot: KeySlot) -> bool {
        match self.edits.get(slot) {
            KeyEdit::Replace(key) => !key.expose().trim().is_empty(),
            KeyEdit::Untouched => self.presence.get(slot),
            KeyEdit::Clear => false,
        }
    }
}

/// Every field error of `s`, all at once (empty = valid).
pub fn validate(
    s: &Settings,
    keys: &KeyEditsWithPresence<'_>,
    models: &dyn DownloadedModels,
) -> Vec<FieldError> {
    let mut errors = Vec::new();
    let mut refuse = |field, code| errors.push(FieldError { field, code });

    match s.engine {
        EngineKind::None => {}
        EngineKind::Api => {
            if let Err(code) = base_url_rule(&s.api.base_url) {
                refuse(FieldId::EngineApiBaseUrl, code);
            }
            if s.api.model.trim().is_empty() {
                refuse(FieldId::EngineApiModel, ErrorCode::Required);
            }
            if !keys.has_key_after_save(KeySlot::TranscriptionApi) {
                refuse(FieldId::EngineApiKey, ErrorCode::KeyRequired);
            }
        }
        EngineKind::LocalServer => {
            if let Err(code) = base_url_rule(&s.local_server.base_url) {
                refuse(FieldId::EngineLocalServerBaseUrl, code);
            }
        }
        EngineKind::BuiltinLocal => {
            let downloaded = s
                .builtin_local
                .model_id
                .as_deref()
                .is_some_and(|id| models.is_downloaded(id));
            if !downloaded {
                refuse(
                    FieldId::EngineBuiltinLocalModelId,
                    ErrorCode::ModelNotDownloaded,
                );
            }
        }
    }

    // Not a field of one engine, so checked whatever engine is selected.
    if let Some(code) = s.speech_language.as_deref() {
        if !WHISPER_ISO_639_1.contains(&code) {
            refuse(
                FieldId::EngineSpeechLanguage,
                ErrorCode::LanguageUnsupported,
            );
        }
    }

    if let Err(e) = parse_hotkey(&s.hotkey) {
        refuse(FieldId::RecordingHotkey, hotkey_code(e));
    }

    if !HISTORY_SIZE.contains(&s.history.size) {
        refuse(FieldId::HistorySize, ErrorCode::HistorySizeRange);
    }

    // Every timeout, whatever engine is selected (decision #99): the group is
    // always shown, so each error has a control.
    for role in TimeoutRole::ALL {
        if !role.bounds().contains(role.get(&s.timeouts)) {
            refuse(timeout_field(role), ErrorCode::TimeoutRange);
        }
    }

    errors
}

fn timeout_field(role: TimeoutRole) -> FieldId {
    match role {
        TimeoutRole::Connect => FieldId::TimeoutsConnect,
        TimeoutRole::ApiTranscription => FieldId::TimeoutsApiTranscription,
        TimeoutRole::LocalServer => FieldId::TimeoutsLocalServer,
        TimeoutRole::PostProcessing => FieldId::TimeoutsPostProcessing,
        TimeoutRole::BuiltinLocal => FieldId::TimeoutsBuiltinLocal,
    }
}

fn base_url_rule(raw: &str) -> Result<(), ErrorCode> {
    match check_base_url(raw) {
        Ok(_) => Ok(()),
        Err(UrlError::Empty) => Err(ErrorCode::Required),
        Err(UrlError::Malformed) => Err(ErrorCode::UrlMalformed),
        Err(UrlError::Credentials) => Err(ErrorCode::UrlCredentials),
    }
}

fn hotkey_code(e: HotkeyError) -> ErrorCode {
    match e {
        HotkeyError::NoModifier => ErrorCode::HotkeyNoModifier,
        HotkeyError::NoKey => ErrorCode::HotkeyNoKey,
        HotkeyError::EscReserved => ErrorCode::HotkeyEscReserved,
        HotkeyError::Invalid => ErrorCode::HotkeyInvalid,
    }
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
        // Every other grammar error is `hotkey.invalid` (decision #25(c)): empty part,
        // unknown key, two keys, out-of-range key; non-canonical text too (#26).
        // Bite: `HotkeyError::Invalid` mapped to any of the three codes above.
        for raw in ["", "Ctrl+Tab", "Ctrl+A+B", "Ctrl+F25", "ctrl alt space"] {
            let (mut s, edits, presence) = api_ok();
            s.hotkey = raw.into();
            assert_eq!(
                run(&s, &edits, presence, &[]),
                vec![FieldError {
                    field: FieldId::RecordingHotkey,
                    code: ErrorCode::HotkeyInvalid,
                }],
                "{raw:?}"
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
    fn post_processing_on_is_validated_for_every_engine() {
        // T-021 / 003 FR-011: post-processing is not a field of one engine, so with
        // it on its rule runs whatever engine is selected (`none` included), with
        // the engine URLs' codes; no key is required for it. Each engine is
        // otherwise valid, so the three post-processing errors are the only ones.
        // Bite: no call to post_process::settings::validate, the call inside one
        // engine's arm, or a post_processing.key error.
        for engine in EVERY_ENGINE {
            let mut s = sample(engine);
            s.post_processing.enabled = true;
            s.post_processing.base_url = String::new();
            s.post_processing.model = String::new();
            s.post_processing.prompt = String::new();
            assert_eq!(
                run(&s, &no_keys(), api_key_stored(), &["base"]),
                vec![
                    FieldError {
                        field: FieldId::PostProcessingBaseUrl,
                        code: ErrorCode::Required
                    },
                    FieldError {
                        field: FieldId::PostProcessingModel,
                        code: ErrorCode::Required
                    },
                    FieldError {
                        field: FieldId::PostProcessingPrompt,
                        code: ErrorCode::Required
                    },
                ],
                "{engine:?}"
            );
            s.post_processing.base_url = "https://user:pass@llm.example.com/v1".into();
            s.post_processing.model = "llm-test".into();
            s.post_processing.prompt = "Fix the text.".into();
            assert_eq!(
                pairs(&run(&s, &no_keys(), api_key_stored(), &["base"])),
                vec![("post_processing.base_url", "url.credentials")],
                "{engine:?}"
            );
        }
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

    // ---- T-073: timeouts (decisions #97, #99) ----

    /// One timeouts field: its wire id, its #99 bounds (default, min, max) and how to
    /// set it on a draft.
    struct TimeoutField {
        id: &'static str,
        default: u32,
        min: u32,
        max: u32,
        set: fn(&mut Settings, u32),
    }

    const TIMEOUT_FIELDS: [TimeoutField; 5] = [
        TimeoutField {
            id: "timeouts.connect",
            default: 5,
            min: 1,
            max: 60,
            set: |s, v| s.timeouts.connect_s = v,
        },
        TimeoutField {
            id: "timeouts.api_transcription",
            default: 30,
            min: 5,
            max: 600,
            set: |s, v| s.timeouts.api_transcription_s = v,
        },
        TimeoutField {
            id: "timeouts.local_server",
            default: 60,
            min: 5,
            max: 1800,
            set: |s, v| s.timeouts.local_server_s = v,
        },
        TimeoutField {
            id: "timeouts.post_processing",
            default: 15,
            min: 5,
            max: 300,
            set: |s, v| s.timeouts.post_processing_s = v,
        },
        TimeoutField {
            id: "timeouts.builtin_local",
            default: 120,
            min: 10,
            max: 1800,
            set: |s, v| s.timeouts.builtin_local_s = v,
        },
    ];

    /// The sample for `engine`, valid as it is (API key stored, `base` downloaded).
    fn valid_for(engine: EngineKind) -> Vec<FieldError> {
        run(&sample(engine), &no_keys(), api_key_stored(), &["base"])
    }

    #[test]
    fn timeouts_out_of_range_are_refused_for_every_engine() {
        // Decision #99 Q3 bounds, checked on every save whatever engine is selected
        // (`none` included), like history.size; the post-processing limit is checked
        // with post-processing on and off. Out of range (min - 1, max + 1, 0,
        // u32::MAX) -> exactly that field's `timeout.range`; min, max and the default
        // pass. Bite: a rule missing for one field, an exclusive bound (off by one at
        // min or max), the bounds of another field, a check only for the selected
        // engine's limit, a code other than timeout.range, or the error on another
        // field.
        for engine in EVERY_ENGINE {
            assert_eq!(valid_for(engine), vec![], "{engine:?} baseline");
            for f in &TIMEOUT_FIELDS {
                for bad in [f.min - 1, f.max + 1, 0, u32::MAX] {
                    for pp_on in [true, false] {
                        let mut s = sample(engine);
                        s.post_processing.enabled = pp_on;
                        (f.set)(&mut s, bad);
                        assert_eq!(
                            pairs(&run(&s, &no_keys(), api_key_stored(), &["base"])),
                            vec![(f.id, "timeout.range")],
                            "{engine:?} {} = {bad} (post-processing on: {pp_on})",
                            f.id
                        );
                    }
                }
                for good in [f.min, f.max, f.default] {
                    let mut s = sample(engine);
                    (f.set)(&mut s, good);
                    assert_eq!(
                        run(&s, &no_keys(), api_key_stored(), &["base"]),
                        vec![],
                        "{engine:?} {} = {good}",
                        f.id
                    );
                }
            }
        }
    }

    #[test]
    fn every_timeout_error_is_returned_at_once() {
        // SC-003 for the timeouts: five offending fields, five errors in one refusal,
        // beside the other rules' errors. Bite: validation stopping at the first
        // timeouts error, or one error for the whole group.
        let mut s = sample(EngineKind::None);
        for f in &TIMEOUT_FIELDS {
            (f.set)(&mut s, 0);
        }
        s.history.size = 0;
        let mut want: Vec<(&str, &str)> = TIMEOUT_FIELDS
            .iter()
            .map(|f| (f.id, "timeout.range"))
            .chain([("history.size", "history.size_range")])
            .collect();
        want.sort();
        let errors = run(&s, &no_keys(), KeyPresence::default(), &[]);
        assert_eq!(pairs(&errors), want);
        assert_eq!(errors.len(), 6, "duplicates: {errors:?}");
    }

    // ---- Decision #27 (T-003 review round 1) ----

    /// The FieldId of `speech_language`: `engine.speech_language`, per decision
    /// #28(a) (data-model.md and the spec's Engine tab; #27(3)'s
    /// `recording.speech_language` was a drafting slip).
    const SPEECH_LANGUAGE: FieldId = FieldId::EngineSpeechLanguage;

    /// Every engine, `none` included: `speech_language` is not a field of one
    /// engine, so it is checked whatever engine is selected (#28, data-model.md).
    const EVERY_ENGINE: [EngineKind; 4] = [
        EngineKind::None,
        EngineKind::Api,
        EngineKind::LocalServer,
        EngineKind::BuiltinLocal,
    ];

    fn replace(key: &str) -> KeyEdit {
        KeyEdit::Replace(Secret::new(key))
    }

    #[test]
    fn api_empty_or_whitespace_replace_is_key_required() {
        // #27(1): `Replace` with an empty or whitespace-only key is not an entered
        // key -> `key.required` on `engine.api.key`, with or without a stored key
        // (removing a key is the explicit `Clear`). Bite: `Replace(_) => true` in
        // `has_key_after_save` (today's behaviour, Was #26).
        let s = sample(EngineKind::Api);
        for raw in ["", " ", "\t", " \n\t ", "\u{3000}"] {
            for presence in [KeyPresence::default(), api_key_stored()] {
                let edits = KeyEdits {
                    transcription_api: replace(raw),
                    ..KeyEdits::default()
                };
                assert_eq!(
                    run(&s, &edits, presence, &[]),
                    vec![FieldError {
                        field: FieldId::EngineApiKey,
                        code: ErrorCode::KeyRequired,
                    }],
                    "{raw:?}, stored = {}",
                    presence.transcription_api
                );
            }
        }
        // A non-empty key around spaces is still a key entered now.
        let edits = KeyEdits {
            transcription_api: replace("  sk-test-padded  "),
            ..KeyEdits::default()
        };
        assert_eq!(run(&s, &edits, KeyPresence::default(), &[]), vec![]);
    }

    #[test]
    fn optional_key_slots_accept_an_empty_replace() {
        // The local-server key (002 FR-017) and the post-processing key (003 FR-011)
        // are optional: an empty `Replace` there behaves like no key, not like an
        // error (#27(1) read with the optional slots; what the save step stores for
        // it is T-032's). An empty `Replace` in another slot never makes the API
        // key look missing either.
        // Bite: the #27(1) empty-key rule applied to every slot instead of the
        // required one.
        for raw in ["", "   "] {
            let edits = KeyEdits {
                local_server: replace(raw),
                post_processing: replace(raw),
                ..KeyEdits::default()
            };
            for presence in [
                KeyPresence::default(),
                KeyPresence {
                    transcription_api: false,
                    local_server: true,
                    post_processing: true,
                },
            ] {
                // Local server selected, post-processing on with valid fields.
                let s = sample(EngineKind::LocalServer);
                assert!(s.post_processing.enabled);
                assert_eq!(run(&s, &edits, presence, &[]), vec![], "{raw:?}");
            }
            // API selected with a stored API key: the other slots' empty edits do
            // not matter.
            let s = sample(EngineKind::Api);
            assert_eq!(run(&s, &edits, api_key_stored(), &[]), vec![], "{raw:?}");
        }
    }

    /// Base URLs with userinfo; fake credentials, documentation hosts only.
    const WITH_USERINFO: [&str; 5] = [
        "https://user:pass@api.example.com/v1", // teamwright:allow-secret (fake test value)
        "http://user@api.example.com",
        "https://:fake-pass@api.example.com/v1", // teamwright:allow-secret (fake test value)
        "http://user:fake@192.0.2.10:8000/v1",   // teamwright:allow-secret (fake test value)
        "  https://user:pass@api.example.com/v1/  ", // teamwright:allow-secret (fake test value)
    ];

    #[test]
    fn api_base_url_with_credentials_refused() {
        // #27(2): userinfo in a base URL is a second path for key bytes into the
        // settings file -> `url.credentials` on the field, exactly. Bite: no
        // `username()`/`password()` check (today: accepted).
        for raw in WITH_USERINFO {
            let (mut s, edits, presence) = api_ok();
            s.api.base_url = raw.into();
            assert_eq!(
                pairs(&run(&s, &edits, presence, &[])),
                vec![("engine.api.base_url", "url.credentials")],
                "{raw:?}"
            );
        }
    }

    #[test]
    fn local_server_base_url_with_credentials_refused() {
        // #27(2), the same rule for the local-server base URL.
        for raw in WITH_USERINFO {
            let mut s = sample(EngineKind::LocalServer);
            s.local_server.base_url = raw.into();
            assert_eq!(
                pairs(&run(&s, &no_keys(), KeyPresence::default(), &[])),
                vec![("engine.local_server.base_url", "url.credentials")],
                "{raw:?}"
            );
        }
    }

    #[test]
    fn base_url_with_query_accepted() {
        // #27(2): a query string is allowed (Azure-style `?api-version=`).
        // Bite: a credentials rule that refuses any URL with a query.
        let raw = "https://api.example.com/v1?api-version=2024-06-01";
        let (mut s, edits, presence) = api_ok();
        s.api.base_url = raw.into();
        assert_eq!(run(&s, &edits, presence, &[]), vec![]);
        let mut s = sample(EngineKind::LocalServer);
        s.local_server.base_url = "http://192.0.2.10:8000/v1?mode=fast".into();
        assert_eq!(run(&s, &no_keys(), KeyPresence::default(), &[]), vec![]);
    }

    #[test]
    fn speech_language_null_or_whisper_code_accepted() {
        // #27(3), #28: `null` is auto-detect; every two-letter Whisper code is
        // accepted, for every engine, `none` included. Bite: a hand-written subset
        // (e.g. only the UI languages) or a case-folded comparison gone wrong. The
        // list's own shape is pinned by `whisper_list_is_unique_two_letter_lowercase`.
        for engine in EVERY_ENGINE {
            let mut s = sample(engine);
            s.speech_language = None;
            assert_eq!(
                run(&s, &no_keys(), api_key_stored(), &["base"]),
                vec![],
                "{engine:?} null"
            );
            for code in WHISPER_ISO_639_1 {
                let mut s = sample(engine);
                s.speech_language = Some(code.into());
                assert_eq!(
                    run(&s, &no_keys(), api_key_stored(), &["base"]),
                    vec![],
                    "{engine:?} {code}"
                );
            }
        }
    }

    #[test]
    fn speech_language_outside_whisper_list_refused() {
        // #27(3), #28: anything but `null` or a two-letter Whisper code ->
        // `language.unsupported` on the speech-language field, exactly, for every
        // engine, `none` included. Bite: no check, a length-only check (`xx`), a
        // case-insensitive match (`RU`), accepting the ISO 639-2 or BCP 47 form
        // (`rus`, `en-US`), trimming, or skipping the check when engine = `none`
        // (M12).
        for raw in [
            "xx", "zz", "rus", "RU", "Ru", "", " ", "en-US", "en_US", "ru ", " de", "haw", "yue",
            "english", "auto",
        ] {
            for engine in EVERY_ENGINE {
                let mut s = sample(engine);
                s.speech_language = Some(raw.into());
                assert_eq!(
                    run(&s, &no_keys(), api_key_stored(), &["base"]),
                    vec![FieldError {
                        field: SPEECH_LANGUAGE,
                        code: ErrorCode::LanguageUnsupported,
                    }],
                    "{engine:?} {raw:?}"
                );
            }
        }
    }
}
