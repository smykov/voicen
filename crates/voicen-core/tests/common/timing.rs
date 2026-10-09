//! The only place a voicen-core test reads the wall clock (T-080, invariant I2;
//! docs/decisions/core-tests.md). `scripts/ci/core-test-clocks.sh` (in `make check`)
//! refuses `Instant::now`, `.elapsed(` and an alias of `Instant` in every test file
//! outside `tests/common`.
//!
//! I2: a wall-clock reading decides only a lower bound ([`at_least`]: a deadline
//! never fires early). A wrong longer deadline is caught by the outcome (the server
//! answers between the right and the wrong deadline, or the mock's request count),
//! never by a ceiling, because a ceiling races host scheduling (F-013, T-046, T-047).
//! The only ceiling is a spec-stated tolerance, through [`within_spec`], which names
//! the spec id and the reference load the tolerance holds under ([`REFERENCE_LOAD`],
//! OQ-23 proposal (a)).
//!
//! Besides the stopwatch ([`measure`]) this module hands out instants that are not
//! measurements: [`now`] / [`ago`] (timestamps a product API takes as input, e.g.
//! `hotkey_pressed(at)`), and [`deadline`] / [`left`] / [`eventually`] (how long a
//! test waits for something that should already have happened; the verdict is the
//! awaited outcome, the wait only ends a hung test).
//! Contract tests: `common/timing_tests.rs`, run by `tests/common_helpers.rs`.

use std::thread;
use std::time::{Duration, Instant};

/// The host load a spec tolerance holds under (OQ-23, proposal (a)): the gate's own,
/// one `make check` at a time, `cargo test -j2`, test binaries one after another, on
/// the dev host and the CI runners. Red under a heavier concurrent load is not a
/// defect.
pub const REFERENCE_LOAD: &str = "OQ-23 (a): the gate's own load, one `make check`, \
     `cargo test -j2`, test binaries one after another";

/// The current instant, as a timestamp handed to a product API (a press, a
/// release, a frame's `at`). Not a measurement: a duration is taken with
/// [`measure`], and a bound is checked with [`at_least`] or [`within_spec`].
pub fn now() -> Instant {
    Instant::now()
}

/// The instant `d` before now (a press "4 s ago"), as a timestamp handed to a
/// product API. Panics when the monotonic clock is younger than `d` (a host that
/// just booted), a test-environment problem.
pub fn ago(d: Duration) -> Instant {
    Instant::now()
        .checked_sub(d)
        .unwrap_or_else(|| panic!("monotonic clock at least {d:?} past its origin"))
}

/// Runs `f` and returns its value and how long it took (the stopwatch every core
/// test uses instead of `Instant::now()` / `.elapsed()`).
pub fn measure<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let started = Instant::now();
    let got = f();
    (got, started.elapsed())
}

/// The lower bound a wall-clock reading may decide: `took` (a measured time) is at
/// least `deadline`, i.e. the deadline did not fire early and no shorter wrong limit
/// was used. No upper side: a wrong longer limit is caught by the outcome.
pub fn at_least(took: Duration, deadline: Duration) {
    assert!(
        took >= deadline,
        "took {took:?}, below the deadline {deadline:?}: a deadline fired early or a \
         shorter limit was used (T-080 I2: a wall-clock reading decides only this lower \
         bound)"
    );
}

/// A spec-stated tolerance, the only wall-clock ceiling a core test may assert
/// (I2): `stage` lies within `want ± tol`, both ends inclusive. `spec` names the
/// spec item that states the tolerance (e.g. `"SC-003"`); a ceiling without one is
/// refused. The failure names the spec id, the stage and [`REFERENCE_LOAD`].
pub fn within_spec(stage: Duration, want: Duration, tol: Duration, spec: &str) {
    assert!(
        !spec.trim().is_empty(),
        "within_spec: a wall-clock ceiling needs the id of the spec item that states \
         its tolerance (T-080 I2); got {spec:?} for stage {stage:?}"
    );
    assert!(
        stage + tol >= want && stage <= want + tol,
        "{spec}: stage {stage:?} is outside {want:?} ± {tol:?} (the spec's tolerance, \
         held under {REFERENCE_LOAD})"
    );
}

/// The instant `d` from now: the end of a wait (see [`left`]).
pub fn deadline(d: Duration) -> Instant {
    Instant::now() + d
}

/// What is left of a wait until `end` (zero once it has passed), e.g. for
/// `recv_timeout`. The wait only ends a hung test; the awaited outcome is the
/// verdict.
pub fn left(end: Instant) -> Duration {
    end.saturating_duration_since(Instant::now())
}

/// Whether `d` has passed since `since` (an instant from [`now`]).
pub fn passed(since: Instant, d: Duration) -> bool {
    Instant::now() >= since + d
}

/// Polls `cond` every 5 ms until it holds (`true`) or `budget` has passed
/// (`false`). The budget only ends a hung test; it is sized far above the time the
/// condition needs.
pub fn eventually(budget: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let end = deadline(budget);
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
}
