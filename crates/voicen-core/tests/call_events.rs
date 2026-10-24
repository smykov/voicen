//! T-118: the call value types (spec 007 data-model, contracts/core-call.md; decision #195).
//!
//! Wire names of `Track`, `StopReason`, `FailReason`, `CallState`; the `CallId` grammar
//! `YYYY-MM-DD_HHMMSS[-n]`; the `EndpointId` charset rule (the only constructor of the value
//! the log writes for a device); `CallNotice` / `CallWarning` hold closed ids only.
//!
//! These are integration tests, not `call/events.rs` unit tests, because a test module cannot
//! exist before its file does; the types are expected at `voicen_core::call::*`.

use voicen_core::call::{
    CallId, CallNotice, CallState, CallWarning, EndpointId, FailReason, StopReason, Track,
};

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|e| panic!("serialize: {e}"))
}

fn wire<T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
    value: T,
    name: &str,
) {
    assert_eq!(
        json(&value),
        format!("\"{name}\""),
        "{value:?} serializes as {name}"
    );
    let back: T = serde_json::from_str(&format!("\"{name}\""))
        .unwrap_or_else(|e| panic!("{name} must deserialize: {e}"));
    assert_eq!(back, value, "{name} round-trips");
}

fn refused<T: serde::de::DeserializeOwned + std::fmt::Debug>(name: &str) {
    let r = serde_json::from_str::<T>(&format!("\"{name}\""));
    assert!(r.is_err(), "{name:?} must be refused, got {r:?}");
}

#[test]
fn track_names() {
    // Bite: PascalCase default (no rename_all), or a swapped pair.
    wire(Track::Mic, "mic");
    wire(Track::System, "system");
    refused::<Track>("Mic");
    refused::<Track>("other");
}

#[test]
fn stop_reason_names_are_the_data_model_set() {
    let all = [
        (StopReason::User, "user"),
        (StopReason::Hotkey, "hotkey"),
        (StopReason::Exit, "exit"),
        (StopReason::SystemEnd, "system_end"),
        (StopReason::Sleep, "sleep"),
        (StopReason::MaxLength, "max_length"),
        (StopReason::LowDisk, "low_disk"),
        (StopReason::StorageError, "storage_error"),
    ];
    for (v, name) in all {
        // An exhaustive match: a new variant must be added to this list.
        match v {
            StopReason::User
            | StopReason::Hotkey
            | StopReason::Exit
            | StopReason::SystemEnd
            | StopReason::Sleep
            | StopReason::MaxLength
            | StopReason::LowDisk
            | StopReason::StorageError => {}
        }
        wire(v, name);
    }
    // 1a has no stop switch (#184): `switch` is a start source, never a stop reason.
    refused::<StopReason>("switch");
    refused::<StopReason>("SystemEnd");
}

#[test]
fn fail_reason_names_are_the_data_model_set() {
    let all = [
        (FailReason::Key, "key"),
        (FailReason::Network, "network"),
        (FailReason::Timeout, "timeout"),
        (FailReason::Provider, "provider"),
        (FailReason::Storage, "storage"),
        (FailReason::EngineNotSetUp, "engine_not_set_up"),
    ];
    for (v, name) in all {
        match v {
            FailReason::Key
            | FailReason::Network
            | FailReason::Timeout
            | FailReason::Provider
            | FailReason::Storage
            | FailReason::EngineNotSetUp => {}
        }
        wire(v, name);
    }
    refused::<FailReason>("engine-not-set-up");
}

#[test]
fn call_state_keeps_the_hyphenated_name_of_the_data_model() {
    // Decision #195 (1): `ready-to-transcribe` with hyphens through an explicit rename; the
    // other states are single words. Bite: rename_all = "snake_case" alone gives
    // ready_to_transcribe; rename_all = "kebab-case" would pass this but the next line is
    // the guard against the snake spelling being accepted too.
    let all = [
        (CallState::Recording, "recording"),
        (CallState::ReadyToTranscribe, "ready-to-transcribe"),
        (CallState::Transcribing, "transcribing"),
        (CallState::Ready, "ready"),
        (CallState::Failed, "failed"),
        (CallState::Recovered, "recovered"),
    ];
    for (v, name) in all {
        match v {
            CallState::Recording
            | CallState::ReadyToTranscribe
            | CallState::Transcribing
            | CallState::Ready
            | CallState::Failed
            | CallState::Recovered => {}
        }
        wire(v, name);
    }
    refused::<CallState>("ready_to_transcribe");
    refused::<CallState>("ReadyToTranscribe");
}

// ---- CallId ----------------------------------------------------------------------

#[test]
fn call_id_accepts_the_folder_grammar_and_keeps_the_text() {
    for ok in [
        "2026-10-24_093015",
        "2026-10-24_093015-2",
        "2026-01-01_000000-17",
        "1999-12-31_235959",
    ] {
        let id = CallId::parse(ok).unwrap_or_else(|| panic!("{ok} is a call id"));
        assert_eq!(id.to_string(), ok, "Display writes the folder name back");
        assert_eq!(
            json(&id),
            format!("\"{ok}\""),
            "serialized as the folder name"
        );
        let back: CallId = serde_json::from_str(&format!("\"{ok}\""))
            .unwrap_or_else(|e| panic!("{ok} must deserialize: {e}"));
        assert_eq!(back, id);
    }
}

#[test]
fn call_id_refuses_everything_outside_the_grammar() {
    // Bite: a prefix-only check, a lax split on '-', accepting a path (the id names a
    // folder, so a separator or `..` would escape the calls root), non-ASCII digits.
    for bad in [
        "",
        "2026-10-24",
        "2026-10-24_09301",
        "2026-10-24_0930155",
        "2026-10-24T093015",
        "2026-10-24 093015",
        "2026-13-24_093015",
        "2026-10-32_093015",
        "2026-00-10_093015",
        "2026-10-24_240000",
        "2026-10-24_096015",
        "2026-10-24_093060",
        "26-10-24_093015",
        "2026-10-24_093015-",
        "2026-10-24_093015-x",
        "2026-10-24_093015-1-2",
        "2026-10-24_093015_1",
        "2026-10-24_093015 ",
        " 2026-10-24_093015",
        "2026-10-24_093015\n",
        "../2026-10-24_093015",
        "2026-10-24_093015/..",
        "2026-10-24_093015\\x",
        "2026-10-24_093015-\u{663}",
        "\u{662}026-10-24_093015",
        "２０２６-10-24_093015",
    ] {
        assert!(CallId::parse(bad).is_none(), "{bad:?} is not a call id");
        let r = serde_json::from_str::<CallId>(&json(&bad));
        assert!(
            r.is_err(),
            "{bad:?} must not deserialize (meta.json is untrusted): {r:?}"
        );
    }
}

// ---- EndpointId ------------------------------------------------------------------

const REAL: &str = "{0.0.1.00000000}.{8a1c2f3e-0b4d-4c5a-9e6f-1a2b3c4d5e6f}";

fn text_of(id: &EndpointId) -> String {
    id.to_string()
}

#[test]
fn endpoint_id_keeps_a_wasapi_id_and_hex_in_either_case() {
    assert_eq!(text_of(&EndpointId::new(REAL)), REAL);
    let upper = "{0.0.0.00000000}.{ABCDEF01-2345-6789-abcd-EF0123456789}";
    assert_eq!(text_of(&EndpointId::new(upper)), upper);
    // The length limit is inclusive: 128 characters pass.
    let longest = "a".repeat(128);
    assert_eq!(text_of(&EndpointId::new(&longest)), longest);
    let one = "0";
    assert_eq!(text_of(&EndpointId::new(one)), one);
}

#[test]
fn endpoint_id_outside_the_charset_or_length_is_invalid() {
    // Failure branch: no friendly name, path, key=value, control byte or oversize text may
    // be kept. Bite: a check on the first character only, `<= 129`, a missing empty check,
    // `is_alphanumeric` instead of hex, byte length instead of characters (the multi-byte
    // case is 40 chars but 80 bytes and must still be refused for its charset).
    let long = "a".repeat(129);
    let long_braces = format!("{{{}}}", "0".repeat(127));
    let cases: Vec<&str> = vec![
        "",
        "Headset (Realtek Audio)",
        "Speakers",
        "{0.0.1.00000000}.{guid-not-hex}",
        "{0.0.1.00000000}.{8a1c2f3e} ",
        " {0.0.1.00000000}",
        "{0.0.1.00000000}\n",
        "{0.0.1.00000000}\r\nINJECT=1",
        "{0.0.1.00000000}.{abc}=1",
        "mic={0.0.1.00000000}",
        "\\\\?\\SWD#MMDEVAPI#{0.0.1.00000000}.{abc}",
        "C:\\Users\\ivan\\Calls\\2026-10-24_093015",
        "{0.0.1.00000000}.{abc}/..",
        "{０.0.1.00000000}",
        "ééééééééééééééééééééééééééééééééééééééé",
        "invalid",
        "none",
        &long,
        &long_braces,
    ];
    for c in cases {
        assert_eq!(text_of(&EndpointId::new(c)), "invalid", "{c:?}");
    }
}

#[test]
fn endpoint_id_deserialize_goes_through_the_same_rule() {
    // A hand-edited meta.json must not carry free text into a later log line. Either the
    // value is refused or it becomes the invalid state; it is never kept.
    for bad in ["Headset (Realtek Audio)", "", "a\nb", "x=1"] {
        if let Ok(id) = serde_json::from_str::<EndpointId>(&json(&bad)) {
            assert_eq!(text_of(&id), "invalid", "{bad:?}");
        }
    }
    let id: EndpointId = serde_json::from_str(&json(&REAL))
        .unwrap_or_else(|e| panic!("a real id deserializes: {e}"));
    assert_eq!(text_of(&id), REAL);
    assert_eq!(id, EndpointId::new(REAL));
}

// ---- CallNotice / CallWarning ------------------------------------------------------

#[test]
fn notices_and_warnings_are_built_from_closed_values() {
    // Decision #195 (2): no `ProviderNotice` (T-126 adds it with a typed host). Every
    // variant is built from enums and a `CallId`; an exhaustive match pins the set.
    let call = CallId::parse("2026-10-24_093015").unwrap_or_else(|| panic!("id"));
    let warnings = [
        CallWarning::CommunicationsDeviceDiffers,
        CallWarning::TrackLost { track: Track::Mic },
        CallWarning::TrackLost {
            track: Track::System,
        },
        CallWarning::MicChanged,
        CallWarning::CantHearOtherParty,
        CallWarning::CantHearYou,
        CallWarning::LowDisk,
    ];
    for w in &warnings {
        match w {
            CallWarning::CommunicationsDeviceDiffers
            | CallWarning::TrackLost { .. }
            | CallWarning::MicChanged
            | CallWarning::CantHearOtherParty
            | CallWarning::CantHearYou
            | CallWarning::LowDisk => {}
        }
        assert_eq!(*w, w.clone());
        assert!(!json(w).is_empty());
    }
    let notices = [
        CallNotice::CallSavedTranscribing,
        CallNotice::TranscriptReady { call: call.clone() },
        CallNotice::PassFailed {
            reason: FailReason::Network,
        },
        CallNotice::Stopped {
            reason: StopReason::Sleep,
        },
    ];
    for n in &notices {
        match n {
            CallNotice::CallSavedTranscribing
            | CallNotice::TranscriptReady { .. }
            | CallNotice::PassFailed { .. }
            | CallNotice::Stopped { .. } => {}
        }
        assert_eq!(*n, n.clone());
    }
    // The ready notice names the call by its id and nothing else of the call.
    let text = json(&notices[1]);
    assert!(text.contains("2026-10-24_093015"), "{text}");
    // Distinct warnings stay distinct on the wire (a collapsed enum would not).
    let wire: std::collections::BTreeSet<String> = warnings.iter().map(json).collect();
    assert_eq!(wire.len(), warnings.len());
}
