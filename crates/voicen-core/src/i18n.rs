//! English/Russian message catalog shared by the shell and the UI (FR-15, T-005).
//!
//! The catalog lives at the repository root (`i18n/en.json`, `i18n/ru.json`,
//! decision #13): two flat JSON maps of id -> text. This module is the only Rust
//! code that parses, looks up and renders them; the UI (`src/lib/i18n`) follows the
//! same rule, and both are pinned by the shared fixture `i18n/conformance.json`.
//!
//! - Lookup: the text in the requested language if non-empty, else the English
//!   text if non-empty, else the id itself.
//! - Placeholders: `{name}` with `name` matching `[a-z][a-z0-9_]*`; no escaping.
//!   Any other `{` or `}` is a stray brace (rendered as is, reported by the
//!   test-only check `Catalog::parity_problems`).
//! - Rendering: one left-to-right pass. A placeholder with an argument is replaced
//!   by the value inserted literally (never re-expanded); a placeholder without an
//!   argument stays verbatim; extra arguments are ignored.

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
/// ```text
/// messages! {
///     /// Tray menu item.
///     TRAY_EXIT = "tray.exit",
/// }
/// ```
macro_rules! messages {
    ($($(#[$meta:meta])* $name:ident = $id:literal),* $(,)?) => {
        $(
            $(#[$meta])*
            pub const $name: MessageId = MessageId($id);
        )*

        /// Every `MessageId` declared in this module (generated from the same
        /// declaration).
        pub const MESSAGE_IDS: &[MessageId] = &[$($name),*];
    };
}

// Rust-originated ids. Each one must exist in both i18n/en.json and i18n/ru.json
// (test `message_ids_exist_in_both_catalogs`).
messages! {
    /// Hotkey pressed while engine = none (spec 004 FR-007; `settings::gate::blocked_actions`).
    NOTICE_CHOOSE_ENGINE = "notice.choose_engine",
    /// A save refused because the settings could not be read at startup (decision
    /// #19; `settings::service::FormError::SettingsUnavailable`).
    NOTICE_SETTINGS_UNAVAILABLE = "notice.settings_unavailable",
    /// A save refused because `settings.json` could not be written; everything was
    /// restored (`FormError::WriteFailed`).
    SETTINGS_WRITE_FAILED = "settings.write_failed",
    /// A refused save whose undo failed for some keys; the UI highlights the key
    /// fields named in `FormError::PartiallyRestored` (R-3).
    SETTINGS_PARTIALLY_RESTORED = "settings.partially_restored",
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

/// Render a Rust-originated message from the embedded catalog.
pub fn text(lang: UiLanguage, id: MessageId, args: &[(&str, &str)]) -> String {
    embedded().text(lang, id.0, args)
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

    // ---- T-004: every refusal reaches the user in the user's language (U2) --------

    /// Every `ErrorCode`, once. The match in [`every_error_code_has_catalog_text`]
    /// has no wildcard, so a variant added later (T-010, T-014, T-020, T-021) does
    /// not compile until it is listed here, and then the test needs its texts.
    const ALL_ERROR_CODES: [crate::settings::ErrorCode; 14] = {
        use crate::settings::ErrorCode::*;
        [
            Required,
            UrlMalformed,
            KeyRequired,
            ModelNotDownloaded,
            HotkeyNoModifier,
            HotkeyNoKey,
            HotkeyEscReserved,
            HotkeyInvalid,
            HotkeyUnavailable,
            HistorySizeRange,
            AutostartFailed,
            KeyStoreFailed,
            UrlCredentials,
            LanguageUnsupported,
        ]
    };

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
        use crate::settings::ErrorCode;
        for code in ALL_ERROR_CODES {
            // No wildcard: a new variant is a compile error here (see ALL_ERROR_CODES).
            match code {
                ErrorCode::Required
                | ErrorCode::UrlMalformed
                | ErrorCode::KeyRequired
                | ErrorCode::ModelNotDownloaded
                | ErrorCode::HotkeyNoModifier
                | ErrorCode::HotkeyNoKey
                | ErrorCode::HotkeyEscReserved
                | ErrorCode::HotkeyInvalid
                | ErrorCode::HotkeyUnavailable
                | ErrorCode::HistorySizeRange
                | ErrorCode::AutostartFailed
                | ErrorCode::KeyStoreFailed
                | ErrorCode::UrlCredentials
                | ErrorCode::LanguageUnsupported => {}
            }
        }
        let distinct: BTreeSet<&str> = ALL_ERROR_CODES.iter().map(|c| c.as_str()).collect();
        assert_eq!(
            distinct.len(),
            ALL_ERROR_CODES.len(),
            "an ErrorCode listed twice"
        );

        let c = catalog(EN_JSON, RU_JSON);
        let mut missing = Vec::new();
        for code in ALL_ERROR_CODES {
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
}
