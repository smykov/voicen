//! English/Russian message catalog shared by the shell and the UI (FR-15, T-005).
//!
//! STUB written by the test writer: the public surface below exists only so the
//! tests compile. Every body is `todo!()`; every `#[allow]` here is for the stub and
//! is removed by the implementation.

use serde::{Deserialize, Serialize};

/// UI language; serialized as `"en"` / `"ru"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiLanguage {
    En,
    Ru,
}

/// An id of a message that originates in Rust (shell text or ids sent over IPC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // stub: field is read by the implementation
pub struct MessageId(&'static str);

/// Every `MessageId` declared in this module (generated from the same declaration).
pub const MESSAGE_IDS: &[MessageId] = &[];

/// Both catalogs (en, ru) as parsed flat id -> text maps.
#[derive(Debug)]
pub struct Catalog {
    _stub: (),
}

/// A catalog file that is not a flat JSON object of strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogError {
    /// Which file is broken.
    pub lang: UiLanguage,
    pub message: String,
}

/// One invariant violation, naming the offending id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogProblem {
    pub id: String,
    pub kind: ProblemKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProblemKind {
    /// The id is absent from this language's catalog.
    Missing(UiLanguage),
    /// The id's text is empty in this language's catalog.
    Empty(UiLanguage),
    /// en and ru texts of the id use different sets of placeholder names.
    PlaceholdersDiffer,
    /// A `{` or `}` in this language's text is not part of a `{[a-z][a-z0-9_]*}` placeholder.
    StrayBrace(UiLanguage),
}

impl Catalog {
    /// Parse the two catalogs. Lenient about content (empty texts, missing ids are
    /// allowed here and reported by `parity_problems`); rejects anything that is not
    /// a flat JSON object of strings.
    pub fn from_json(_en: &str, _ru: &str) -> Result<Catalog, CatalogError> {
        todo!("T-005: Catalog::from_json")
    }

    /// The single lookup + render rule (see `i18n/conformance.json`).
    pub fn text(&self, _lang: UiLanguage, _id: &str, _args: &[(&str, &str)]) -> String {
        todo!("T-005: Catalog::text")
    }

    /// Invariant (1): same id set, non-empty texts, same placeholder set per id,
    /// no stray braces.
    pub fn parity_problems(&self) -> Vec<CatalogProblem> {
        todo!("T-005: Catalog::parity_problems")
    }

    /// Ids of `ids` absent from either catalog (kind `Missing(lang)`).
    pub fn missing_ids(&self, _ids: &[MessageId]) -> Vec<CatalogProblem> {
        todo!("T-005: Catalog::missing_ids")
    }
}

/// The catalog embedded from `i18n/en.json` and `i18n/ru.json`, parsed once.
pub fn embedded() -> &'static Catalog {
    todo!("T-005: embedded catalog")
}

/// Render a Rust-originated message from the embedded catalog.
pub fn text(_lang: UiLanguage, _id: MessageId, _args: &[(&str, &str)]) -> String {
    todo!("T-005: text")
}

/// OS language tag -> UI language: primary subtag `ru` (any ASCII case) -> Ru, else En.
pub fn resolve_ui_language(_os_tag: Option<&str>) -> UiLanguage {
    todo!("T-005: resolve_ui_language")
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
}
