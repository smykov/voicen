//! T-080 (I2, docs/decisions/core-tests.md): the contract of `common::timing`, the only
//! place a core test reads the wall clock. A wall-clock reading decides only a lower
//! bound ([`at_least`]: a deadline never fires early); no core test asserts a
//! wall-clock ceiling, a spec tolerance included (SC-003 is checked by an outcome
//! bracket, decision #149; `within_spec` was retired in T-110). [`measure`] is the stopwatch callers use instead
//! of `Instant::now()` / `.elapsed()` (refused outside `tests/common` by the
//! package's call ban, rustc's path resolution, T-089).
//!
//! Not a module of `tests/common`: included by `tests/common_helpers.rs` with
//! `#[path]`, as `refused_addr_tests.rs` is by its users.

use std::borrow::Borrow;
use std::ops::Deref;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Duration;

use crate::common::timing::{at_least, at_most_per_second, between, measure, now, Took};

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
    // `at_least` and `at_most_per_second` (T-110 retired `within_spec`), so
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
