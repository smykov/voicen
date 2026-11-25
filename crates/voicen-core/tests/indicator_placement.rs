//! T-130 (spec 007 T033): the pure placement of the call indicator, in
//! `voicen_core::call::indicator`. The shell reads the monitors' work areas from Win32
//! and passes plain numbers; core decides where the window goes. Red until the developer
//! adds (the signature these tests pin):
//!
//! ```text
//! pub struct WorkArea { pub left: i32, pub top: i32, pub right: i32, pub bottom: i32 }
//!     // Debug, Clone, Copy, PartialEq, Eq; right/bottom exclusive; the FIRST entry of the
//!     // slice is the primary monitor.
//! pub const INDICATOR_DEFAULT_MARGIN: i32 = 16;
//! pub fn place_indicator(stored: Option<IndicatorPosition>, size: (i32, i32),
//!                        work_areas: &[WorkArea]) -> IndicatorPosition
//!     // size = (width, height) of the window in pixels; the result is the window's
//!     // top-left. Rules:
//!     // 1. stored = None: the default, the top-right of the primary work area, inset by
//!     //    INDICATOR_DEFAULT_MARGIN: x = right - width - margin, y = top + margin.
//!     // 2. stored fully inside some work area: returned unchanged (negative coordinates
//!     //    of a monitor left of / above the primary included).
//!     // 3. Otherwise the window is moved the least that puts it fully inside ONE work
//!     //    area: the area it overlaps most (by pixels), when it overlaps none the
//!     //    nearest one (rect-to-rect gap, ties to the earlier entry); inside that area
//!     //    x is clamped to left..=right-width, y to top..=bottom-height. A window larger
//!     //    than the area is pinned to the area's left/top.
//!     // 4. A stored position whose monitor is gone (off every work area) is rule 3.
//! ```
//!
//! Not pinned: an empty `work_areas` slice (the shell never passes one; it keeps the
//! stored value or defaults on its own).
use voicen_core::call::indicator::{place_indicator, WorkArea, INDICATOR_DEFAULT_MARGIN};
use voicen_core::settings::IndicatorPosition;

const SIZE: (i32, i32) = (320, 96);

fn wa(left: i32, top: i32, right: i32, bottom: i32) -> WorkArea {
    WorkArea {
        left,
        top,
        right,
        bottom,
    }
}

fn at(x: i32, y: i32) -> IndicatorPosition {
    IndicatorPosition { x, y }
}

/// 1920x1040 (a taskbar below), primary.
fn primary() -> WorkArea {
    wa(0, 0, 1920, 1040)
}

/// 1280x1024 to the left of the primary: its coordinates are negative.
fn left_monitor() -> WorkArea {
    wa(-1280, 0, 0, 1024)
}

fn fully_inside(p: IndicatorPosition, size: (i32, i32), a: &WorkArea) -> bool {
    p.x >= a.left && p.y >= a.top && p.x + size.0 <= a.right && p.y + size.1 <= a.bottom
}

#[test]
fn no_stored_position_gets_the_default_placement_in_the_primary_work_area() {
    // Bite: the default at (0, 0), without the margin, on the left edge, or on the
    // second monitor (primary is the first entry).
    let m = INDICATOR_DEFAULT_MARGIN;
    assert_eq!(
        place_indicator(None, SIZE, &[primary()]),
        at(1920 - 320 - m, m)
    );
    assert_eq!(
        place_indicator(None, SIZE, &[wa(0, 40, 1600, 900), left_monitor()]),
        at(1600 - 320 - m, 40 + m),
        "the first entry is the primary, and its top is the work area's"
    );
}

#[test]
fn a_position_fully_on_a_monitor_is_kept_including_negative_coordinates() {
    // Bite: a clamp to (0, 0).. that drops a monitor left of the primary, or one that
    // rewrites a valid position.
    let areas = [primary(), left_monitor()];
    for p in [
        at(0, 0),
        at(100, 200),
        at(1920 - 320, 1040 - 96),
        at(-1280, 0),
        at(-700, 500),
    ] {
        assert_eq!(place_indicator(Some(p), SIZE, &areas), p, "{p:?}");
    }
}

#[test]
fn a_partly_visible_window_is_pulled_fully_inside_its_monitor() {
    // Each edge, one at a time. Bite: an unchanged position (still half off-screen),
    // a clamp of only one axis, an off-by-width clamp (x = right instead of
    // right - width).
    let areas = [primary()];
    assert_eq!(
        place_indicator(Some(at(1800, 100)), SIZE, &areas),
        at(1600, 100),
        "right"
    );
    assert_eq!(
        place_indicator(Some(at(-100, 100)), SIZE, &areas),
        at(0, 100),
        "left"
    );
    assert_eq!(
        place_indicator(Some(at(500, -50)), SIZE, &areas),
        at(500, 0),
        "top"
    );
    assert_eq!(
        place_indicator(Some(at(500, 1000)), SIZE, &areas),
        at(500, 1040 - 96),
        "bottom"
    );
    assert_eq!(
        place_indicator(Some(at(1900, 1030)), SIZE, &areas),
        at(1600, 944),
        "both axes"
    );
}

#[test]
fn a_position_on_a_removed_monitor_moves_to_the_nearest_remaining_one() {
    // The left monitor was unplugged: (-700, 500) is on no work area. Bite: kept as
    // stored (invisible window), the default instead of the nearest, the first area
    // regardless of distance.
    assert_eq!(
        place_indicator(Some(at(-700, 500)), SIZE, &[primary()]),
        at(0, 500)
    );
    // Far right of everything, two monitors: the nearer (right) one wins, not the first.
    let right = wa(1920, 0, 3840, 1080);
    let areas = [primary(), left_monitor(), right];
    assert_eq!(
        place_indicator(Some(at(5000, 300)), SIZE, &areas),
        at(3840 - 320, 300),
        "nearest, not first"
    );
    // Far below: nearest by gap, y clamped to the bottom.
    assert_eq!(
        place_indicator(Some(at(500, 5000)), SIZE, &[primary()]),
        at(500, 1040 - 96)
    );
}

#[test]
fn a_window_straddling_two_monitors_goes_to_the_one_it_overlaps_most() {
    // x = 1920 - 100: 100 px on the primary, 220 on the right one. Bite: always the
    // first area, or the one containing the top-left corner.
    let right = wa(1920, 0, 3840, 1080);
    assert_eq!(
        place_indicator(Some(at(1820, 100)), SIZE, &[primary(), right]),
        at(1920, 100)
    );
    // The mirror: 220 px on the primary, 100 on the right one.
    assert_eq!(
        place_indicator(Some(at(1700, 100)), SIZE, &[primary(), right]),
        at(1600, 100)
    );
}

#[test]
fn a_window_larger_than_the_work_area_is_pinned_to_its_top_left() {
    // No panic (clamp with min > max), no negative drift. Bite: `i32::clamp` with an
    // inverted range, or a result outside the area.
    let tiny = wa(10, 20, 110, 70);
    assert_eq!(
        place_indicator(Some(at(500, 500)), SIZE, &[tiny]),
        at(10, 20)
    );
}

#[test]
fn placement_always_ends_fully_inside_an_area_and_is_idempotent() {
    // The invariant, not the cases: for a grid of stored positions around and beyond
    // two monitors, the result is fully inside one work area, and placing the result
    // again changes nothing. Bite: any clamp that leaves a corner off-screen or one that
    // oscillates.
    let areas = [primary(), left_monitor()];
    for x in (-3000..=3000).step_by(137) {
        for y in (-1500..=2500).step_by(113) {
            let p = place_indicator(Some(at(x, y)), SIZE, &areas);
            assert!(
                areas.iter().any(|a| fully_inside(p, SIZE, a)),
                "({x}, {y}) -> {p:?} is off every monitor"
            );
            assert_eq!(
                place_indicator(Some(p), SIZE, &areas),
                p,
                "({x}, {y}) -> {p:?}"
            );
        }
    }
    let d = place_indicator(None, SIZE, &areas);
    assert!(
        fully_inside(d, SIZE, &areas[0]),
        "the default is inside the primary: {d:?}"
    );
}
