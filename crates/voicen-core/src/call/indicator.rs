//! The call indicator model and its IPC wire (spec 007 contracts/ipc.md `IndicatorState`,
//! `IndicatorTick`, `IndicatorWarning`; T-126). Pure values: every text is rendered here in
//! Rust from the i18n catalog, so the UI shows what core decided. [`place_indicator`] is
//! the window's pure placement over the monitors' work areas (T-130).
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
use crate::settings::IndicatorPosition;

/// What the call indicator window shows (`indicator://state`, `call_indicator_state`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndicatorState {
    /// The language of every text in this state, and of the page's own labels: the call's
    /// language while it records; for idle, the settings language when it was built.
    pub lang: UiLanguage,
    pub recording: bool,
    pub reminder_visible: bool,
    /// The recording reminder; empty when `reminder_visible` is false.
    pub reminder_text: String,
    /// The headphones hint; empty when `reminder_visible` is false.
    pub headphones_hint: String,
    pub warnings: Vec<IndicatorWarning>,
}

impl IndicatorState {
    /// No call: nothing shown; `lang` is the language the page's own labels use.
    pub fn idle(lang: UiLanguage) -> IndicatorState {
        IndicatorState {
            lang,
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
            lang,
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
    /// A track judged silent by the FR-013 rule (T-149): `true` from the `cant_hear` warning
    /// of that track until the track's next speech second.
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

/// The warnings of one call (data-model "CallWarning and CallNotice", D4 of
/// docs/decisions/call-session.md): each entry has an id unique within the call, starting
/// at 1. A warning equal to one still listed (same kind, same track for `TrackLost`) is not
/// added again; a dismissed one is added again, with a new id, at its next raise.
#[derive(Debug, Clone)]
pub struct WarningList {
    items: Vec<(u32, CallWarning)>,
    next_id: u32,
}

impl Default for WarningList {
    fn default() -> WarningList {
        WarningList::new()
    }
}

impl WarningList {
    pub fn new() -> WarningList {
        WarningList {
            items: Vec::new(),
            next_id: 1,
        }
    }

    /// Adds `warning` at the end; its id, or `None` when an equal warning is listed.
    pub fn raise(&mut self, warning: CallWarning) -> Option<u32> {
        if self.items.iter().any(|(_, w)| *w == warning) {
            return None;
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.items.push((id, warning));
        Some(id)
    }

    /// Removes the entry with `id`; `false` when no entry has it.
    pub fn dismiss(&mut self, id: u32) -> bool {
        let before = self.items.len();
        self.items.retain(|(i, _)| *i != id);
        self.items.len() != before
    }

    /// The listed warnings in raise order, rendered in `lang`.
    pub fn items(&self, lang: UiLanguage) -> Vec<IndicatorWarning> {
        self.items
            .iter()
            .map(|(id, w)| IndicatorWarning::of(*id, w, lang))
            .collect()
    }
}

/// One monitor's work area in screen pixels (the monitor minus the taskbar); `right` and
/// `bottom` are exclusive. In a slice given to [`place_indicator`] the first entry is the
/// primary monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkArea {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// The inset of the default indicator placement from the primary work area's top-right.
pub const INDICATOR_DEFAULT_MARGIN: i32 = 16;

/// The window's top-left on screen for a window of `size` (width, height) pixels (T-130).
///
/// - `stored = None`: the top-right of the primary (first) work area, inset by
///   [`INDICATOR_DEFAULT_MARGIN`].
/// - `stored` fully inside some work area: returned unchanged.
/// - Otherwise the window is moved the least that puts it fully inside one work area:
///   the one it overlaps most (by pixels), or, when it overlaps none, the nearest one
///   (rect-to-rect gap; ties go to the earlier entry). A window larger than that area is
///   pinned to the area's left/top.
///
/// An empty `work_areas` keeps `stored` (or `(0, 0)` without one): nothing to clamp to.
pub fn place_indicator(
    stored: Option<IndicatorPosition>,
    size: (i32, i32),
    work_areas: &[WorkArea],
) -> IndicatorPosition {
    let Some(stored) = stored else {
        let Some(primary) = work_areas.first() else {
            return IndicatorPosition { x: 0, y: 0 };
        };
        let default = IndicatorPosition {
            x: primary
                .right
                .saturating_sub(size.0)
                .saturating_sub(INDICATOR_DEFAULT_MARGIN),
            y: primary.top.saturating_add(INDICATOR_DEFAULT_MARGIN),
        };
        return clamp_into(default, size, primary);
    };
    let rect = Rect::of(stored, size);
    if work_areas.iter().any(|a| rect.inside(a)) {
        return stored;
    }
    let mut best: Option<(&WorkArea, i64)> = None;
    for area in work_areas {
        let overlap = rect.overlap(area);
        if overlap > 0 && best.is_none_or(|(_, o)| overlap > o) {
            best = Some((area, overlap));
        }
    }
    let target = match best {
        Some((area, _)) => Some(area),
        None => {
            let mut nearest: Option<(&WorkArea, i64)> = None;
            for area in work_areas {
                let gap = rect.gap(area);
                if nearest.is_none_or(|(_, g)| gap < g) {
                    nearest = Some((area, gap));
                }
            }
            nearest.map(|(area, _)| area)
        }
    };
    match target {
        Some(area) => clamp_into(stored, size, area),
        None => stored,
    }
}

/// `p` moved the least that puts a `size` window inside `area`; pinned to its left/top
/// when the window is larger than the area.
fn clamp_into(p: IndicatorPosition, size: (i32, i32), area: &WorkArea) -> IndicatorPosition {
    let axis = |v: i32, low: i32, high_exclusive: i32, len: i32| {
        v.min(high_exclusive.saturating_sub(len)).max(low)
    };
    IndicatorPosition {
        x: axis(p.x, area.left, area.right, size.0),
        y: axis(p.y, area.top, area.bottom, size.1),
    }
}

/// A window rectangle in i64, so sums and products of screen coordinates never overflow.
struct Rect {
    left: i64,
    top: i64,
    right: i64,
    bottom: i64,
}

impl Rect {
    fn of(p: IndicatorPosition, size: (i32, i32)) -> Rect {
        Rect {
            left: i64::from(p.x),
            top: i64::from(p.y),
            right: i64::from(p.x) + i64::from(size.0),
            bottom: i64::from(p.y) + i64::from(size.1),
        }
    }

    fn inside(&self, a: &WorkArea) -> bool {
        self.left >= i64::from(a.left)
            && self.top >= i64::from(a.top)
            && self.right <= i64::from(a.right)
            && self.bottom <= i64::from(a.bottom)
    }

    /// The shared area in pixels (0 when apart).
    fn overlap(&self, a: &WorkArea) -> i64 {
        let w = self.right.min(i64::from(a.right)) - self.left.max(i64::from(a.left));
        let h = self.bottom.min(i64::from(a.bottom)) - self.top.max(i64::from(a.top));
        w.max(0) * h.max(0)
    }

    /// The squared distance between the rectangles (0 when they touch or overlap).
    fn gap(&self, a: &WorkArea) -> i64 {
        let dx = (i64::from(a.left) - self.right)
            .max(self.left - i64::from(a.right))
            .max(0);
        let dy = (i64::from(a.top) - self.bottom)
            .max(self.top - i64::from(a.bottom))
            .max(0);
        dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(list: &WarningList) -> Vec<(u32, WarningKind)> {
        list.items(UiLanguage::En)
            .into_iter()
            .map(|w| (w.id, w.kind))
            .collect()
    }

    #[test]
    fn every_constructor_carries_the_language_it_is_given() {
        // T-204: the field cannot drift from the texts rendered in the same language.
        // Bite: a fixed language, `idle` or `recording` ignoring its argument.
        for lang in [UiLanguage::En, UiLanguage::Ru] {
            assert_eq!(IndicatorState::idle(lang).lang, lang);
            for visible in [true, false] {
                let state = IndicatorState::recording(lang, visible, Vec::new());
                assert_eq!(state.lang, lang);
                if visible {
                    assert_eq!(state.reminder_text, text(lang, INDICATOR_REMINDER, &[]));
                }
            }
        }
        assert_ne!(
            IndicatorState::idle(UiLanguage::En),
            IndicatorState::idle(UiLanguage::Ru)
        );
    }

    #[test]
    fn ids_start_at_1_and_follow_the_raise_order() {
        let mut list = WarningList::new();
        assert_eq!(list.raise(CallWarning::MicChanged), Some(1));
        assert_eq!(list.raise(CallWarning::CantHearYou), Some(2));
        assert_eq!(
            kinds(&list),
            vec![(1, WarningKind::MicChanged), (2, WarningKind::CantHearYou)]
        );
    }

    #[test]
    fn a_listed_warning_is_not_added_again_but_the_other_track_is() {
        // Bite: a duplicate per raise; the two lost tracks merged into one entry.
        let mut list = WarningList::new();
        assert_eq!(
            list.raise(CallWarning::TrackLost {
                track: Track::System
            }),
            Some(1)
        );
        assert_eq!(
            list.raise(CallWarning::TrackLost {
                track: Track::System
            }),
            None
        );
        assert_eq!(
            list.raise(CallWarning::TrackLost { track: Track::Mic }),
            Some(2)
        );
        assert_eq!(list.items(UiLanguage::En).len(), 2);
    }

    #[test]
    fn dismiss_removes_by_id_and_the_next_raise_gets_a_new_id() {
        // Bite: dismiss by kind; a dismiss of an unknown id answering true; an id reused.
        let mut list = WarningList::new();
        let a = list.raise(CallWarning::MicChanged);
        let b = list.raise(CallWarning::LowDisk);
        assert_eq!((a, b), (Some(1), Some(2)));
        assert!(!list.dismiss(7), "unknown");
        assert!(list.dismiss(1));
        assert!(!list.dismiss(1), "already gone");
        assert_eq!(kinds(&list), vec![(2, WarningKind::LowDisk)]);
        assert_eq!(list.raise(CallWarning::MicChanged), Some(3));
        assert_eq!(
            kinds(&list),
            vec![(2, WarningKind::LowDisk), (3, WarningKind::MicChanged)]
        );
    }
}
