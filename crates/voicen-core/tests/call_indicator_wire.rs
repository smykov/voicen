//! T-126 (spec 007 T024/T025): the call indicator wire fixture
//! `e2e/fixtures/call-indicator-wire.json` is pinned by core (the overlay.rs
//! `e2e_overlay_wire_fixture_matches_core` pattern), and the pure indicator model
//! (`voicen_core::call::indicator`) renders its texts in Rust.
//!
//! The API these tests pin (the developer builds it; contracts/ipc.md):
//!
//! ```text
//! voicen_core::call::indicator::{IndicatorState, IndicatorTick, IndicatorWarning, WarningKind}
//! IndicatorState { recording: bool, reminder_visible: bool, reminder_text: String,
//!                  headphones_hint: String, warnings: Vec<IndicatorWarning> }  // Serialize camelCase
//! IndicatorState::idle() -> IndicatorState
//! IndicatorState::recording(lang: UiLanguage, reminder_visible: bool,
//!                           warnings: Vec<IndicatorWarning>) -> IndicatorState
//!     // reminder_text / headphones_hint = i18n INDICATOR_REMINDER / INDICATOR_HEADPHONES in `lang`
//!     // when reminder_visible, else both empty
//! IndicatorTick { elapsed_ms: u64, mic_level: f32, system_level: f32, mic_silent: bool,
//!                 system_silent: bool }  // Serialize camelCase
//! IndicatorWarning { id: u32, kind: WarningKind, text: String }  // Serialize
//! IndicatorWarning::of(id: u32, warning: &CallWarning, lang: UiLanguage) -> IndicatorWarning
//!     // text = the catalog text of the warning in `lang`; provider_notice carries {host}
//! WarningKind: Serialize camelCase: communicationsDevice | trackLost | micChanged |
//!     cantHearOtherParty | cantHearYou | lowDisk | providerNotice  (Debug, Clone, Copy, PartialEq, Eq)
//! ```

use serde_json::{json, Value};
use voicen_core::call::indicator::{IndicatorState, IndicatorTick, IndicatorWarning, WarningKind};
use voicen_core::call::{CallWarning, Track};
use voicen_core::i18n::UiLanguage;
use voicen_core::settings::url::{check_base_url, provider_host};

const FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../e2e/fixtures/call-indicator-wire.json"
));

fn host() -> voicen_core::settings::url::ProviderHost {
    let url = check_base_url("https://api.example.com/v1").expect("a valid base url");
    provider_host(&url).expect("a non-loopback host")
}

fn every_warning() -> Vec<CallWarning> {
    vec![
        CallWarning::CommunicationsDeviceDiffers,
        CallWarning::TrackLost { track: Track::Mic },
        CallWarning::TrackLost {
            track: Track::System,
        },
        CallWarning::MicChanged,
        CallWarning::CantHearOtherParty,
        CallWarning::CantHearYou,
        CallWarning::LowDisk,
        CallWarning::ProviderNotice { host: host() },
    ]
}

fn warnings(lang: UiLanguage) -> Vec<IndicatorWarning> {
    every_warning()
        .iter()
        .zip(1u32..)
        .map(|(w, id)| IndicatorWarning::of(id, w, lang))
        .collect()
}

fn wire<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).expect("serializes")
}

#[test]
fn e2e_call_indicator_wire_fixture_matches_core() {
    // Acceptance "e2e/fixtures/call-indicator-wire.json is pinned by a core test".
    // Bite: any change of a field name, a kind name, a catalog text (en or ru), the
    // hidden-reminder blanking or the provider-notice host without regenerating the
    // fixture; a key in the fixture that core no longer produces (the key sets are
    // compared both ways).
    let fixture: Value = serde_json::from_str(FIXTURE)
        .unwrap_or_else(|e| panic!("call-indicator-wire.json is valid JSON: {e}"));
    let cases: Vec<(&str, Value)> = vec![
        ("idle", wire(&IndicatorState::idle())),
        (
            "recording_reminder_en",
            wire(&IndicatorState::recording(UiLanguage::En, true, Vec::new())),
        ),
        (
            "recording_reminder_ru",
            wire(&IndicatorState::recording(UiLanguage::Ru, true, Vec::new())),
        ),
        (
            "recording_no_reminder_en",
            wire(&IndicatorState::recording(
                UiLanguage::En,
                false,
                Vec::new(),
            )),
        ),
        (
            "warnings_en",
            wire(&IndicatorState::recording(
                UiLanguage::En,
                false,
                warnings(UiLanguage::En),
            )),
        ),
        (
            "warnings_ru",
            wire(&IndicatorState::recording(
                UiLanguage::Ru,
                false,
                warnings(UiLanguage::Ru),
            )),
        ),
        (
            "tick_levels",
            wire(&IndicatorTick {
                elapsed_ms: 61_000,
                mic_level: 0.5,
                system_level: 0.25,
                mic_silent: false,
                system_silent: false,
            }),
        ),
        (
            "tick_silent",
            wire(&IndicatorTick {
                elapsed_ms: 0,
                mic_level: 0.0,
                system_level: 0.0,
                mic_silent: true,
                system_silent: true,
            }),
        ),
    ];
    let object = fixture.as_object().expect("the fixture is an object");
    for (key, core) in &cases {
        let got = object
            .get(*key)
            .unwrap_or_else(|| panic!("the fixture has no `{key}`; core produces {core}"));
        assert_eq!(
            got, core,
            "fixture `{key}` differs from core; copy this value: {core}"
        );
    }
    let mut want: Vec<&str> = cases.iter().map(|(k, _)| *k).collect();
    want.push("_format");
    want.sort_unstable();
    let mut have: Vec<&str> = object.keys().map(String::as_str).collect();
    have.sort_unstable();
    assert_eq!(have, want, "the fixture holds exactly the core cases");
}

#[test]
fn the_reminder_is_blank_when_hidden_and_rendered_when_shown() {
    // Acceptance "the reminder and headphones hint show ... and not at all when
    // `reminder` is off": the model itself blanks both texts when not visible and
    // renders them in the UI language when visible. Bite: texts kept while hidden;
    // the texts in the wrong language.
    let shown = IndicatorState::recording(UiLanguage::En, true, Vec::new());
    assert!(shown.recording && shown.reminder_visible);
    assert_eq!(
        shown.reminder_text,
        "Tell the other party the call is being recorded"
    );
    assert_eq!(shown.headphones_hint, "Better with headphones");
    let ru = IndicatorState::recording(UiLanguage::Ru, true, Vec::new());
    assert_eq!(ru.headphones_hint, "Лучше в наушниках");
    let hidden = IndicatorState::recording(UiLanguage::En, false, Vec::new());
    assert!(hidden.recording && !hidden.reminder_visible);
    assert_eq!(hidden.reminder_text, "");
    assert_eq!(hidden.headphones_hint, "");
    let idle = IndicatorState::idle();
    assert!(!idle.recording && !idle.reminder_visible && idle.warnings.is_empty());
}

#[test]
fn every_warning_maps_to_its_own_kind_and_keeps_its_id() {
    // contracts/ipc.md IndicatorWarning: kind is one of seven names; both lost tracks
    // share `trackLost`; the id the caller passes is kept. Bite: a kind mapped to a
    // neighbour (cantHearYou for cantHearOtherParty), the id replaced by an index.
    let kinds: Vec<WarningKind> = warnings(UiLanguage::En).iter().map(|w| w.kind).collect();
    assert_eq!(
        kinds,
        vec![
            WarningKind::CommunicationsDevice,
            WarningKind::TrackLost,
            WarningKind::TrackLost,
            WarningKind::MicChanged,
            WarningKind::CantHearOtherParty,
            WarningKind::CantHearYou,
            WarningKind::LowDisk,
            WarningKind::ProviderNotice,
        ]
    );
    let w = IndicatorWarning::of(41, &CallWarning::LowDisk, UiLanguage::En);
    assert_eq!(w.id, 41);
    assert_eq!(wire(&w)["kind"], json!("lowDisk"));
}
