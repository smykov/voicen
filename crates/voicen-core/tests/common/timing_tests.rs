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

use std::borrow::Borrow;
use std::ops::Deref;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Duration;

use crate::common::timing::{
    at_least, at_most_per_second, between, fastest, measure, now, within_spec, Took,
};

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
    at_least(took, work);
}

/// Compiles only while `T` does NOT implement the trait in the second
/// impl: with it, `<T as NotImpl<_>>::check` matches both impls and the
/// type parameter cannot be inferred (the `static_assertions::assert_not_impl_any`
/// trick, written out: no new dependency).
macro_rules! assert_not_impl {
    ($t:ty: $($tr:tt)+) => {{
        trait NotImpl<A> {
            fn check() {}
        }
        impl<T: ?Sized> NotImpl<()> for T {}
        #[allow(dead_code)]
        struct Invalid;
        impl<T: ?Sized + $($tr)+> NotImpl<Invalid> for T {}
        let _ = <$t as NotImpl<_>>::check;
    }};
}

#[test]
fn a_measured_time_cannot_be_compared_or_read_back_so_no_ceiling_compiles() {
    // T-080 review 1 #1: `measure` returns an opaque `Took`, taken only by
    // `at_least`, `within_spec` and `at_most_per_second`, so
    // `assert!(took < Duration::from_millis(1500))` on its result does not compile.
    // Each line below fails to compile once `Took` gains the trait (a comparison,
    // an `==`, a deref or conversion back to a Duration). Bite: `measure` returning
    // a `Duration` again, or `Took` deriving `PartialOrd` / `PartialEq` / a
    // getter-like conversion. A plain method returning the Duration is not visible
    // to this check; `Took`'s inner field is private and its only accessor is
    // `pub(super)` (inside `tests/common`).
    assert_not_impl!(Took: PartialOrd<Duration>);
    assert_not_impl!(Took: PartialOrd<Took>);
    assert_not_impl!(Took: PartialEq<Duration>);
    assert_not_impl!(Took: PartialEq<Took>);
    assert_not_impl!(Took: Deref);
    assert_not_impl!(Took: Into<Duration>);
    assert_not_impl!(Took: AsRef<Duration>);
    assert_not_impl!(Took: Borrow<Duration>);
    assert_not_impl!(Duration: PartialOrd<Took>);
    assert_not_impl!(Duration: PartialEq<Took>);
}

#[test]
fn between_two_stamps_is_a_measured_time_from_the_first_to_the_second() {
    // Two instants from `common::timing::now()` (e.g. a download's start and its
    // end event) give a `Took` for `at_least`, not a Duration a ceiling could read.
    // An end before the start is zero, not a panic. Bite: the order swapped, a
    // constant, an absolute difference.
    let a = now();
    let b = a + Duration::from_millis(50);
    at_least(between(a, b), Duration::from_millis(50));
    assert!(
        panic_message(|| at_least(between(a, b), Duration::from_millis(51))).is_some(),
        "50 ms between the stamps is below 51 ms"
    );
    assert!(
        panic_message(|| at_least(between(b, a), Duration::from_nanos(1))).is_some(),
        "an end before the start is zero"
    );
}

#[test]
fn fastest_and_less_give_the_sc_003_stage_over_the_baseline() {
    // SC-003's stage is the measured time over the fastest of the baseline runs
    // (post_process_timeout), kept a `Took`. Bite: the slowest or the first run as
    // the baseline, a subtraction that panics or wraps below zero.
    let runs = [6, 2, 4].map(|s| Took::from(Duration::from_secs(s)));
    let base = fastest(runs);
    within_spec(base, Duration::from_secs(2), Duration::ZERO, "SC-003");
    let stage = Took::from(Duration::from_millis(7_400)).less(base);
    within_spec(stage, Duration::from_millis(5_400), Duration::ZERO, "SC-003");
    assert!(
        panic_message(|| within_spec(
            Took::from(Duration::from_secs(1)).less(base),
            Duration::from_millis(1),
            Duration::ZERO,
            "SC-003"
        ))
        .is_some(),
        "1 s less a 2 s baseline is zero, not 1 ms"
    );
    assert!(
        panic_message(|| {
            fastest(Vec::<Took>::new());
        })
        .is_some(),
        "no baseline run is a test bug"
    );
}

#[test]
fn at_most_per_second_caps_a_count_by_the_measured_time_never_the_time() {
    // A rate check (local_download: <= 4 Progress events per started second, + 1):
    // the measured time widens the allowed count, so host load can only make it
    // pass. Bite: the cap rounded down, the count ignored.
    let two_s = Took::from(Duration::from_secs(2));
    at_most_per_second(9, 4, two_s, "Progress events");
    let msg = panic_message(|| at_most_per_second(10, 4, two_s, "Progress events"))
        .expect("10 events in 2 s is over 4/s + 1");
    assert!(msg.contains("Progress events"), "{msg}");
    at_most_per_second(5, 4, Took::from(Duration::from_millis(1)), "events");
}
