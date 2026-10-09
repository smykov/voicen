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
//! The stopwatch ([`measure`], [`between`]) returns an opaque [`Took`] that only
//! these helpers read, so a ceiling on a measured time does not compile (review 1
//! #1). Besides the stopwatch this module hands out instants that are not
//! measurements: [`now`] / [`ago`] (timestamps a product API takes as input, e.g.
//! `hotkey_pressed(at)`), and [`deadline`] / [`left`] / [`eventually`] (how long a
//! test waits for something that should already have happened; the verdict is the
//! awaited outcome, the wait only ends a hung test).
//! Contract tests: `common/timing_tests.rs`, run by `tests/common_helpers.rs`.

use std::thread;
use std::time::{Duration, Instant};

/// The host load a spec tolerance holds under. OQ-23 is still open; the tests
/// proceed on its proposal (a): the gate's own load, one `make check` at a time,
/// `cargo test -j2`, test binaries one after another, on the dev host and the CI
/// runners. Red under a heavier concurrent load is then not a defect. A different
/// answer from the owner changes this text and reopens T-080.
pub const REFERENCE_LOAD: &str = "OQ-23 (open; proposal (a) assumed): the gate's own \
     load, one `make check`, `cargo test -j2`, test binaries one after another";

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

/// A measured time: what [`measure`] and [`between`] return (T-080 review 1 #1).
///
/// Opaque on purpose: no comparison, no `==`, no deref or conversion back to a
/// [`Duration`], so a ceiling on a measured time (`assert!(took < ...)`) does not
/// compile. Only [`at_least`] (the lower bound), [`within_spec`] (a spec-stated
/// tolerance) and [`at_most_per_second`] (a count capped by the time) read it;
/// [`Took::less`] and [`fastest`] keep a baseline subtraction a `Took`. `Debug`
/// prints the duration for failure messages. `From<Duration>` exists for the
/// helpers' own contract tests and for times a product reports; it gives nothing
/// back. Pinned by `timing_tests::a_measured_time_cannot_be_compared_or_read_back_so_no_ceiling_compiles`.
#[derive(Clone, Copy)]
pub struct Took(Duration);

impl Took {
    /// This time less `baseline` (zero when the baseline is longer): the SC-003
    /// stage over the fastest baseline run.
    pub fn less(self, baseline: Took) -> Took {
        Took(self.0.saturating_sub(baseline.0))
    }

    /// The duration, for the other helpers of `tests/common` only (the OS-answer
    /// probes); never handed to a test file.
    pub(super) fn duration(self) -> Duration {
        self.0
    }
}

impl From<Duration> for Took {
    fn from(d: Duration) -> Took {
        Took(d)
    }
}

impl std::fmt::Debug for Took {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Runs `f` and returns its value and how long it took (the stopwatch every core
/// test uses instead of `Instant::now()` / `.elapsed()`), as an opaque [`Took`].
pub fn measure<T>(f: impl FnOnce() -> T) -> (T, Took) {
    let started = Instant::now();
    let got = f();
    (got, Took(started.elapsed()))
}

/// The time from `start` to `end`, two instants from [`now`] or from a harness in
/// `tests/common` (zero when `end` is before `start`).
pub fn between(start: Instant, end: Instant) -> Took {
    Took(end.saturating_duration_since(start))
}

/// The fastest of `runs` (a baseline taken as the minimum of several runs, so a
/// cold first run does not count). Panics on no run: a test bug.
pub fn fastest(runs: impl IntoIterator<Item = Took>) -> Took {
    runs.into_iter()
        .map(|t| t.0)
        .min()
        .map(Took)
        .unwrap_or_else(|| panic!("fastest: no baseline run"))
}

/// The lower bound a wall-clock reading may decide: `took` (a measured time) is at
/// least `deadline`, i.e. the deadline did not fire early and no shorter wrong limit
/// was used. No upper side: a wrong longer limit is caught by the outcome.
pub fn at_least(took: impl Into<Took>, deadline: Duration) {
    let took = took.into().0;
    assert!(
        took >= deadline,
        "took {took:?}, below the deadline {deadline:?}: a deadline fired early or a \
         shorter limit was used (T-080 I2: a wall-clock reading decides only this lower \
         bound)"
    );
}

/// A count capped by a measured time: `count` is at most `per_second` for every
/// started second of `took`, plus one (e.g. Progress events at <= 4/s). The time
/// only widens the cap, so host load can make the check pass, never fail.
pub fn at_most_per_second(count: usize, per_second: usize, took: Took, what: &str) {
    let secs = usize::try_from(took.0.as_millis().div_ceil(1_000)).unwrap_or(usize::MAX);
    let max = per_second.saturating_mul(secs).saturating_add(1);
    assert!(
        count <= max,
        "{count} {what} over {took:?}: more than {per_second}/s (+ 1)"
    );
}

/// A spec-stated tolerance, the only wall-clock ceiling a core test may assert
/// (I2): `stage` lies within `want ± tol`, both ends inclusive. `spec` names the
/// spec item that states the tolerance (e.g. `"SC-003"`); a ceiling without one is
/// refused. The failure names the spec id, the stage and [`REFERENCE_LOAD`].
pub fn within_spec(stage: impl Into<Took>, want: Duration, tol: Duration, spec: &str) {
    let stage = stage.into().0;
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
