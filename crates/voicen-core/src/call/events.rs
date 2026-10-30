//! The call value types and the closed field values of the call log lines (spec 007
//! data-model, contracts/core-call.md "Log lines"; T-118, decision #195).
//!
//! Every value here is a closed enum, an integer, a [`CallId`] or an [`EndpointId`]; none
//! holds free text, so a call log line (written only by `diag::format`) is closed. The
//! literal of each log field value is in `diag::format`'s tables, not here, except
//! [`Track::as_str`] and [`InvalidRule::as_str`], which `transcript.json` shares (T-123); the
//! serde names here are the `meta.json` / IPC wire of data-model.md.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One of the two tracks of a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Track {
    Mic,
    System,
}

impl Track {
    /// The wire name (`mic` | `system`) of the log lines and `transcript.json`.
    pub fn as_str(self) -> &'static str {
        match self {
            Track::Mic => "mic",
            Track::System => "system",
        }
    }
}

/// Where a call start request came from (`call_requested source=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallSource {
    Tray,
    Hotkey,
    /// The `--call record` launch switch.
    Switch,
}

/// The part of a call start that was refused (`call_refused part=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusedPart {
    Mic,
    System,
    /// The calls storage root.
    Root,
}

/// Why a call start was refused (`call_refused reason=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusedReason {
    NoDevice,
    AccessDenied,
    Busy,
    Unwritable,
    Missing,
    Other,
}

/// Why a track's device changed (`call_device cause=`; data-model "DeviceChange").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceCause {
    DefaultOutputChanged,
    MicLost,
    MicDefaultChanged,
    TrackLost,
    Reopened,
    NoDevice,
}

/// The FR-013 rule a silent track hit (`call_track_silent rule=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilentRule {
    Lost,
    CantHear,
}

/// Why a call stopped (data-model "Call"). 1a has no stop switch (#184).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    User,
    Hotkey,
    Exit,
    SystemEnd,
    Sleep,
    MaxLength,
    LowDisk,
    StorageError,
}

/// The storage step that failed (`call_storage_failed kind=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageKind {
    Write,
    Flush,
    Sync,
    Meta,
    Transcript,
    LowDisk,
}

/// A stage of the final pass (`final_stage stage=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalStage {
    Decode,
    Vad,
    Upload,
    Merge,
    Write,
}

/// Why a final pass failed (data-model "Call" `failure.reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailReason {
    Key,
    Network,
    Timeout,
    Provider,
    Storage,
    EngineNotSetUp,
}

/// How a final pass ended (`final_finished outcome= [reason=]`). The reason exists only
/// on a failed pass, so a ready pass cannot carry one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalOutcome {
    Ready,
    Failed { reason: FailReason },
}

/// Whether the final pass kept segment times (`final_finished mode=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassMode {
    Timed,
    Timeless,
}

/// Why a `--call` launch switch was refused (`call_switch_refused reason=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchRefusal {
    UnknownValue,
}

/// Why the final pass fell back to energy spans (`vad_fallback cause=`; #188).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadFallbackCause {
    Unavailable,
    Error,
}

/// The transcript rule `validate()` found broken (`transcript_invalid rule=`; research R17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidRule {
    Unsorted,
    OutOfRange,
    UnknownSpeaker,
    UnknownTrack,
    BadSource,
    BadLang,
    EmptyText,
}

impl InvalidRule {
    /// The closed rule name of the `transcript_invalid rule=` log line.
    pub fn as_str(self) -> &'static str {
        match self {
            InvalidRule::Unsorted => "unsorted",
            InvalidRule::OutOfRange => "out_of_range",
            InvalidRule::UnknownSpeaker => "unknown_speaker",
            InvalidRule::UnknownTrack => "unknown_track",
            InvalidRule::BadSource => "bad_source",
            InvalidRule::BadLang => "bad_lang",
            InvalidRule::EmptyText => "empty_text",
        }
    }
}

/// The stored state of a call (data-model "Call" `state`). `ready-to-transcribe` keeps the
/// data-model's hyphens (decision #195).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallState {
    Recording,
    #[serde(rename = "ready-to-transcribe")]
    ReadyToTranscribe,
    Transcribing,
    Ready,
    Failed,
    Recovered,
}

/// A warning shown in the call indicator (data-model "CallWarning and CallNotice"):
/// catalog ids and enums only. `provider_notice{host}` is added by T-126 (decision #195).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallWarning {
    CommunicationsDeviceDiffers,
    TrackLost { track: Track },
    MicChanged,
    CantHearOtherParty,
    CantHearYou,
    LowDisk,
}

/// A toast about a call (data-model "CallWarning and CallNotice"): catalog ids, enums and
/// a [`CallId`] only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallNotice {
    CallSavedTranscribing,
    TranscriptReady { call: CallId },
    PassFailed { reason: FailReason },
    Stopped { reason: StopReason },
}

// ---- CallId ----------------------------------------------------------------------

/// The id of a call, its folder name: `YYYY-MM-DD_HHMMSS[-n]` with a real date and time
/// and `n` one or more ASCII digits. The only constructors ([`CallId::parse`] and
/// `Deserialize`) refuse anything else, so the id never carries a path separator or `..`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CallId(String);

impl CallId {
    /// The id of `text`, or `None` when `text` is not exactly the folder grammar.
    pub fn parse(text: &str) -> Option<CallId> {
        call_id_grammar(text).map(|()| CallId(text.to_string()))
    }

    /// The folder name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn call_id_grammar(text: &str) -> Option<()> {
    let b = text.as_bytes();
    let (stamp, suffix) = (b.get(..17)?, b.get(17..)?);
    let digits = |r: std::ops::Range<usize>| -> Option<u32> {
        stamp.get(r)?.iter().try_fold(0u32, |acc, c| {
            c.is_ascii_digit().then(|| acc * 10 + u32::from(c - b'0'))
        })
    };
    for (i, sep) in [(4, b'-'), (7, b'-'), (10, b'_')] {
        if stamp.get(i) != Some(&sep) {
            return None;
        }
    }
    let (year, month, day) = (digits(0..4)?, digits(5..7)?, digits(8..10)?);
    let (hour, minute, second) = (digits(11..13)?, digits(13..15)?, digits(15..17)?);
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let month_days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if !(1..=month_days).contains(&day) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    match suffix.split_first() {
        None => Some(()),
        Some((b'-', n)) if !n.is_empty() && n.iter().all(u8::is_ascii_digit) => Some(()),
        Some(_) => None,
    }
}

impl fmt::Display for CallId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for CallId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CallId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        CallId::parse(&text).ok_or_else(|| serde::de::Error::custom("not a call id"))
    }
}

// ---- EndpointId ------------------------------------------------------------------

/// The longest WASAPI endpoint id kept, in characters.
const ENDPOINT_MAX: usize = 128;

/// A WASAPI endpoint id as the log and `meta.json` may carry it: 1..=128 characters of
/// `{`, `}`, `.`, hex digits and `-`, or the state Invalid, written `invalid`. A friendly
/// name, a device path or any other text becomes Invalid; the text is never kept.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EndpointId(Option<String>);

impl EndpointId {
    /// The id of `text` when it is in the charset and length; otherwise Invalid.
    pub fn new(text: &str) -> EndpointId {
        let len = text.chars().count();
        let ok = (1..=ENDPOINT_MAX).contains(&len)
            && text
                .chars()
                .all(|c| c.is_ascii_hexdigit() || matches!(c, '{' | '}' | '.' | '-'));
        EndpointId(ok.then(|| text.to_string()))
    }

    /// The kept id, or `None` for Invalid.
    pub fn as_str(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

impl fmt::Display for EndpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_deref().unwrap_or("invalid"))
    }
}

impl Serialize for EndpointId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for EndpointId {
    /// Through [`EndpointId::new`]: a hand-edited `meta.json` value outside the rule becomes
    /// Invalid, never kept text.
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Ok(EndpointId::new(&text))
    }
}
