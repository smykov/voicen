//! The call indicator model and its IPC wire (spec 007 contracts/ipc.md `IndicatorState`,
//! `IndicatorTick`, `IndicatorWarning`; T-126). Pure values: every text is rendered here in
//! Rust from the i18n catalog, so the UI shows what core decided.
//!
//! `e2e/fixtures/call-indicator-wire.json` is pinned against these values by
//! `tests/call_indicator_wire.rs`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use serde::Serialize;

use crate::call::events::{CallWarning, Track};
use crate::i18n::{
    text, MessageId, UiLanguage, INDICATOR_HEADPHONES, INDICATOR_REMINDER,
    WARNING_CANT_HEAR_OTHER_PARTY, WARNING_CANT_HEAR_YOU, WARNING_COMMUNICATIONS_DEVICE,
    WARNING_LOW_DISK, WARNING_MIC_CHANGED, WARNING_PROVIDER_NOTICE, WARNING_TRACK_LOST_MIC,
    WARNING_TRACK_LOST_SYSTEM,
};

/// What the call indicator window shows (`indicator://state`, `call_indicator_state`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndicatorState {
    pub recording: bool,
    pub reminder_visible: bool,
    /// The recording reminder; empty when `reminder_visible` is false.
    pub reminder_text: String,
    /// The headphones hint; empty when `reminder_visible` is false.
    pub headphones_hint: String,
    pub warnings: Vec<IndicatorWarning>,
}

impl IndicatorState {
    /// No call: nothing shown.
    pub fn idle() -> IndicatorState {
        IndicatorState {
            recording: false,
            reminder_visible: false,
            reminder_text: String::new(),
            headphones_hint: String::new(),
            warnings: Vec::new(),
        }
    }

    /// A recording call; the reminder and the headphones hint are rendered in `lang` when
    /// `reminder_visible`, and both are empty otherwise.
    pub fn recording(
        lang: UiLanguage,
        reminder_visible: bool,
        warnings: Vec<IndicatorWarning>,
    ) -> IndicatorState {
        let shown = |id: MessageId| {
            if reminder_visible {
                text(lang, id, &[])
            } else {
                String::new()
            }
        };
        IndicatorState {
            recording: true,
            reminder_visible,
            reminder_text: shown(INDICATOR_REMINDER),
            headphones_hint: shown(INDICATOR_HEADPHONES),
            warnings,
        }
    }
}

/// The live levels of a recording call (`indicator://tick`, at most 10 per second).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndicatorTick {
    /// Time since the start of the call.
    pub elapsed_ms: u64,
    /// Linear RMS (0..1) of the latest 100 ms window of each track.
    pub mic_level: f32,
    pub system_level: f32,
    /// A track judged silent by the FR-013 rule. The session of T-126 has no silence rule
    /// yet and always sends `false`.
    pub mic_silent: bool,
    pub system_silent: bool,
}

/// The wire kind of an indicator warning (both lost tracks share `trackLost`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WarningKind {
    CommunicationsDevice,
    TrackLost,
    MicChanged,
    CantHearOtherParty,
    CantHearYou,
    LowDisk,
    ProviderNotice,
}

/// One warning line of the indicator: the caller's id, its kind and its rendered text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IndicatorWarning {
    pub id: u32,
    pub kind: WarningKind,
    pub text: String,
}

impl IndicatorWarning {
    /// `warning` rendered in `lang`; the provider notice carries its host in the text.
    pub fn of(id: u32, warning: &CallWarning, lang: UiLanguage) -> IndicatorWarning {
        let (kind, text) = match warning {
            CallWarning::CommunicationsDeviceDiffers => (
                WarningKind::CommunicationsDevice,
                text(lang, WARNING_COMMUNICATIONS_DEVICE, &[]),
            ),
            CallWarning::TrackLost { track } => (
                WarningKind::TrackLost,
                text(
                    lang,
                    match track {
                        Track::Mic => WARNING_TRACK_LOST_MIC,
                        Track::System => WARNING_TRACK_LOST_SYSTEM,
                    },
                    &[],
                ),
            ),
            CallWarning::MicChanged => (
                WarningKind::MicChanged,
                text(lang, WARNING_MIC_CHANGED, &[]),
            ),
            CallWarning::CantHearOtherParty => (
                WarningKind::CantHearOtherParty,
                text(lang, WARNING_CANT_HEAR_OTHER_PARTY, &[]),
            ),
            CallWarning::CantHearYou => (
                WarningKind::CantHearYou,
                text(lang, WARNING_CANT_HEAR_YOU, &[]),
            ),
            CallWarning::LowDisk => (WarningKind::LowDisk, text(lang, WARNING_LOW_DISK, &[])),
            CallWarning::ProviderNotice { host } => (
                WarningKind::ProviderNotice,
                text(lang, WARNING_PROVIDER_NOTICE, &[("host", host.as_str())]),
            ),
        };
        IndicatorWarning { id, kind, text }
    }
}
