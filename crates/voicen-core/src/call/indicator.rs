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
use crate::settings::IndicatorPosition;

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

/// A monitor work area in physical screen pixels (the shell reads it from Win32).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl MonitorRect {
    fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }
}

/// The indicator's saved `position` moved so that a window of `size` (width, height)
/// lies on a present monitor work area (T-130). A window fully inside one work area
/// is returned unchanged. Otherwise the target is the monitor with the largest
/// intersection with the window (none intersects: the nearest one; a tie: the first
/// in the slice), and the top-left is moved the least that puts the whole window
/// inside it; a window larger than the monitor is aligned to its left/top edge. No
/// monitors: unchanged. Pure.
pub fn clamp_to_monitors(
    position: IndicatorPosition,
    size: (i32, i32),
    monitors: &[MonitorRect],
) -> IndicatorPosition {
    let (x, y) = (i64::from(position.x), i64::from(position.y));
    let (w, h) = (i64::from(size.0.max(0)), i64::from(size.1.max(0)));
    let (right, bottom) = (x + w, y + h);
    let inside = |m: &MonitorRect| {
        x >= i64::from(m.x) && y >= i64::from(m.y) && right <= m.right() && bottom <= m.bottom()
    };
    if monitors.is_empty() || monitors.iter().any(inside) {
        return position;
    }
    let overlap = |m: &MonitorRect| {
        let dx = (right.min(m.right()) - x.max(i64::from(m.x))).max(0);
        let dy = (bottom.min(m.bottom()) - y.max(i64::from(m.y))).max(0);
        dx * dy
    };
    let gap = |m: &MonitorRect| {
        let dx = (i64::from(m.x) - right).max(x - m.right()).max(0);
        let dy = (i64::from(m.y) - bottom).max(y - m.bottom()).max(0);
        dx * dx + dy * dy
    };
    // The first best wins a tie: `fold` keeps the earlier one unless strictly better.
    let best = monitors
        .iter()
        .fold(None::<(&MonitorRect, i64, i64)>, |best, m| {
            let (o, g) = (overlap(m), gap(m));
            match best {
                Some((_, bo, bg)) if !(o > bo || (o == 0 && bo == 0 && g < bg)) => best,
                _ => Some((m, o, g)),
            }
        });
    let Some((m, _, _)) = best else {
        return position;
    };
    let fit = |pos: i64, len: i64, start: i32, end: i64| {
        // Larger than the monitor: aligned to its start edge.
        pos.min(end - len).max(i64::from(start))
    };
    let nx = fit(x, w, m.x, m.right());
    let ny = fit(y, h, m.y, m.bottom());
    IndicatorPosition {
        x: i32::try_from(nx).unwrap_or(position.x),
        y: i32::try_from(ny).unwrap_or(position.y),
    }
}

#[cfg(test)]
mod clamp_tests {
    //! T-130 (analysis `approach`): the saved indicator position is clamped to a visible
    //! monitor work area at each build. Assumed API, pure:
    //!
    //! ```text
    //! /// A monitor work area in screen pixels; Debug, Clone, Copy, PartialEq, Eq.
    //! pub struct MonitorRect { pub x: i32, pub y: i32, pub width: i32, pub height: i32 }
    //! pub fn clamp_to_monitors(position: IndicatorPosition, size: (i32, i32),
    //!                          monitors: &[MonitorRect]) -> IndicatorPosition
    //! ```
    //!
    //! `size` is the window's (width, height). Rule: a window rectangle fully inside one
    //! work area is returned unchanged; otherwise the target is the monitor with the
    //! largest intersection with the rectangle (none intersects: the monitor nearest to
    //! the rectangle; a tie: the first in the slice) and the position is moved the least
    //! that puts the whole rectangle inside it (a window larger than the monitor is
    //! aligned to its left/top edge); no monitors: unchanged.

    use super::*;
    use crate::settings::IndicatorPosition;

    const SIZE: (i32, i32) = (300, 80);

    fn primary() -> MonitorRect {
        MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1040,
        }
    }

    fn right() -> MonitorRect {
        MonitorRect {
            x: 1920,
            y: 0,
            width: 1920,
            height: 1040,
        }
    }

    fn left() -> MonitorRect {
        MonitorRect {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
        }
    }

    fn at(x: i32, y: i32) -> IndicatorPosition {
        IndicatorPosition { x, y }
    }

    #[test]
    fn a_fully_visible_position_is_unchanged() {
        // Bite: a clamp that always moves to a corner or to the first monitor.
        for (monitors, saved) in [
            (vec![primary()], at(100, 100)),
            (vec![primary()], at(0, 0)),
            (vec![primary()], at(1620, 960)),
            (vec![primary(), right()], at(2000, 500)),
            (vec![left(), primary()], at(-1000, 100)),
        ] {
            assert_eq!(
                clamp_to_monitors(saved, SIZE, &monitors),
                saved,
                "{monitors:?}"
            );
        }
    }

    #[test]
    fn an_off_screen_position_moves_inside_the_nearest_monitor() {
        // Bite: no clamp, a clamp of x only, a clamp to the monitor's origin instead of
        // the nearest edge.
        let two = [primary(), right()];
        assert_eq!(clamp_to_monitors(at(5000, 100), SIZE, &two), at(3540, 100));
        assert_eq!(clamp_to_monitors(at(100, 9000), SIZE, &two), at(100, 960));
        assert_eq!(
            clamp_to_monitors(at(-4000, -4000), SIZE, &[primary()]),
            at(0, 0)
        );
    }

    #[test]
    fn a_partly_visible_position_moves_fully_inside() {
        // Bite: a clamp that accepts any overlap, or only tests the top-left corner.
        let one = [primary()];
        assert_eq!(clamp_to_monitors(at(100, 1000), SIZE, &one), at(100, 960));
        assert_eq!(clamp_to_monitors(at(-50, 10), SIZE, &one), at(0, 10));
        assert_eq!(clamp_to_monitors(at(1800, 10), SIZE, &one), at(1620, 10));
        // Straddling two monitors: the one with the larger part of the window wins.
        let two = [primary(), right()];
        assert_eq!(clamp_to_monitors(at(1800, 10), SIZE, &two), at(1920, 10));
        assert_eq!(clamp_to_monitors(at(1700, 10), SIZE, &two), at(1620, 10));
    }

    #[test]
    fn a_position_on_a_removed_monitor_returns_to_a_present_one() {
        // The window was saved on the second monitor, which is gone. Bite: the saved
        // position trusted as it is.
        assert_eq!(
            clamp_to_monitors(at(2500, 200), SIZE, &[primary()]),
            at(1620, 200)
        );
        assert_eq!(
            clamp_to_monitors(at(-500, 200), SIZE, &[primary()]),
            at(0, 200)
        );
    }

    #[test]
    fn negative_coordinates_on_a_monitor_left_of_the_origin_are_valid() {
        // Bite: a clamp that treats x < 0 as off-screen (max(0, x)), or an unsigned
        // conversion.
        let both = [left(), primary()];
        assert_eq!(clamp_to_monitors(at(-1900, 20), SIZE, &both), at(-1900, 20));
        assert_eq!(
            clamp_to_monitors(at(-3000, 100), SIZE, &both),
            at(-1920, 100)
        );
        assert_eq!(
            clamp_to_monitors(at(-3000, -200), SIZE, &both),
            at(-1920, 0)
        );
        // Straddling the origin with the larger part (290 px) on `left`: moved fully onto
        // `left`, to a negative x.
        assert_eq!(clamp_to_monitors(at(-290, 100), SIZE, &both), at(-300, 100));
    }

    #[test]
    fn an_oversized_window_aligns_to_the_edge_and_no_monitor_changes_nothing() {
        // Bite: a negative result from `monitor.width - width`; a panic on an empty list.
        assert_eq!(
            clamp_to_monitors(at(500, 10), (2000, 100), &[primary()]),
            at(0, 10)
        );
        assert_eq!(clamp_to_monitors(at(-77, 9000), SIZE, &[]), at(-77, 9000));
    }
}
