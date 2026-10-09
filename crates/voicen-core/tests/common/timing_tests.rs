//! T-080 (I2, docs/decisions/core-tests.md): the contract of `common::timing`, the only
//! place a core test reads the wall clock. A wall-clock reading decides only a lower
//! bound ([`at_least`]: a deadline never fires early); the only ceiling is a
//! spec-stated tolerance ([`within_spec`]), whose failure names the spec id and the
//! reference load the tolerance holds under (OQ-23: the gate's own load, one
//! `make check`, `cargo test -j2`). [`measure`] is the stopwatch callers use instead
//! of `Instant::now()` / `.elapsed()` (refused outside `tests/common` by
//! `scripts/ci/core-test-clocks.sh`).
//!
//! Not a module of `tests/common`: included by `tests/common_helpers.rs` with
//! `#[path]`, as `refused_addr_tests.rs` is by its users.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Duration;

use crate::common::timing::{at_least, measure, within_spec};

/// SC-003's tolerance (spec 003: "measured tolerance <= 0.5 s").
const TOL: Duration = Duration::from_millis(500);
const WANT: Duration = Duration::from_secs(5);

/// The panic message of `f`, or `None` when it did not panic.
fn panic_message(f: impl FnOnce()) -> Option<String> {
    let payload = catch_unwind(AssertUnwindSafe(f)).err()?;
    Some(if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else {
        "<non-string panic payload>".to_string()
    })
}

#[test]
fn within_spec_refuses_a_stage_0_6_s_over_a_0_5_s_tolerance() {
    // The red case of the analysis (iv): 5.6 s against 5 s +- 0.5 s breaks SC-003.
    // The message names the spec id, the stage and the reference load (OQ-23), so a
    // red run says which promise broke and under which load it holds. Bite: a
    // helper that does not check the upper side, one that widens the tolerance
    // (T-078's + 3 s), a message without the spec id or the load.
    let msg = panic_message(|| within_spec(WANT + Duration::from_millis(600), WANT, TOL, "SC-003"))
        .expect("a stage 0.6 s over a 0.5 s tolerance must fail within_spec");
    assert!(
        msg.contains("SC-003"),
        "the message names the spec id: {msg}"
    );
    assert!(
        msg.contains("OQ-23"),
        "the message names the reference load (OQ-23): {msg}"
    );
    assert!(
        msg.contains(&format!("{:?}", WANT + Duration::from_millis(600))),
        "the message names the measured stage: {msg}"
    );
}

#[test]
fn within_spec_accepts_the_tolerance_edges_and_inside() {
    // "Tolerance <= 0.5 s" includes 0.5 s on either side. Bite: a strict `<`, a
    // one-sided check that fails a stage below `want`.
    for stage in [
        WANT,
        WANT + TOL,
        WANT - TOL,
        WANT + Duration::from_millis(499),
    ] {
        if let Some(msg) = panic_message(|| within_spec(stage, WANT, TOL, "SC-003")) {
            panic!("{stage:?} is within {WANT:?} +- {TOL:?}, but within_spec failed: {msg}");
        }
    }
}

#[test]
fn within_spec_refuses_a_stage_that_ended_before_the_tolerance() {
    // A deadline that fires 0.6 s early breaks SC-003 just as a late one does (the
    // existing check is two-sided: `stage + SLACK >= want && stage <= want + SLACK`).
    // Bite: an upper-bound-only helper.
    let msg = panic_message(|| within_spec(WANT - Duration::from_millis(600), WANT, TOL, "SC-003"))
        .expect("a stage 0.6 s under a 0.5 s tolerance must fail within_spec");
    assert!(msg.contains("SC-003"), "{msg}");
}

#[test]
fn within_spec_refuses_a_ceiling_without_a_spec_id() {
    // I2: the only ceiling allowed is a spec-stated tolerance, so a ceiling that
    // names no spec is refused even when the stage is inside it. Bite: a helper
    // that ignores its spec argument (any test could then add an undocumented
    // ceiling through it).
    for spec in ["", "  "] {
        assert!(
            panic_message(|| within_spec(WANT, WANT, TOL, spec)).is_some(),
            "within_spec with spec id {spec:?} must fail: a ceiling names its spec"
        );
    }
}

#[test]
fn at_least_accepts_the_deadline_and_later() {
    // A deadline never fires early: a measured time at or above it passes, however
    // much later (a wrong longer deadline is caught by the outcome, not by a
    // ceiling). Bite: an at_least that also caps the time from above.
    for took in [WANT, WANT + Duration::from_millis(1), WANT * 10] {
        if let Some(msg) = panic_message(|| at_least(took, WANT)) {
            panic!("{took:?} >= {WANT:?}, but at_least failed: {msg}");
        }
    }
}

#[test]
fn at_least_refuses_a_time_below_the_deadline() {
    // The lower bound is the one thing a wall-clock reading decides: 1 ms early is
    // a deadline that fired early (or a wrong shorter limit, e.g. the 2 s connect
    // used as the 5 s post-processing deadline). The message names both values.
    // Bite: a no-op helper, a slack below the deadline, an inverted comparison.
    let took = WANT - Duration::from_millis(1);
    let msg = panic_message(|| at_least(took, WANT))
        .expect("a time 1 ms below the deadline must fail at_least");
    assert!(
        msg.contains(&format!("{took:?}")),
        "names the measured time: {msg}"
    );
    assert!(
        msg.contains(&format!("{WANT:?}")),
        "names the deadline: {msg}"
    );
    assert!(
        panic_message(|| at_least(Duration::ZERO, WANT)).is_some(),
        "zero against a 5 s deadline must fail at_least"
    );
}

#[test]
fn measure_returns_the_value_and_a_duration_no_shorter_than_the_work() {
    // The stopwatch every caller uses instead of Instant::now()/.elapsed(). Bite:
    // a constant Duration::ZERO, timing that starts after the closure ran, a value
    // dropped or replaced.
    let work = Duration::from_millis(60);
    let (got, took) = measure(|| {
        std::thread::sleep(work);
        "VALUE-MARKER"
    });
    assert_eq!(got, "VALUE-MARKER");
    assert!(took >= work, "measured {took:?} for a {work:?} sleep");
}
