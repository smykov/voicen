//! The only place a `src-tauri/tests` crate reads the clock (T-201: the shell's I2, as
//! `crates/voicen-core/tests/common/timing.rs` is for voicen-core; docs/decisions/core-tests.md,
//! docs/decisions/windows-shell.md). The package's call ban (the ban list in `src-tauri`, denied
//! for every target by `src-tauri/Cargo.toml` `[lints]`, run by `make check-shell-windows`) refuses
//! `Instant::now`, `Instant::elapsed` and `SystemTime::elapsed` under any spelling in every test
//! crate; this module's allow below is the one shell-owned exemption. The other one is core's
//! `crates/voicen-core/tests/common`, which `settings_ipc.rs` includes via `#[path]`: it carries
//! its own allow and a budgeted `eventually` (a review-only residual, not refused by the ban).
//!
//! What a test can express through it, and what it cannot:
//! - a measured time ([`measure`], [`between`]) is an opaque [`Took`]: only [`at_least`] (a
//!   lower bound: a deadline did not fire early) reads it, so a ceiling on a `Took` does not
//!   compile. A ceiling races host scheduling and the OS (class `os-answer-deadline-race`,
//!   F-005, F-013, F-019, T-200); a wrong longer limit is caught by an outcome or an ordering
//!   instead. The type does not stop a ceiling written another way: [`now`] returns a plain
//!   `Instant`, so `now() - t0 < X` or `now().duration_since(t0) < X` still compile, as do an
//!   asserted `!passed(t0, X)` and a short [`deadline`] / [`passed`] loop used as a ceiling.
//!   That residual is reviewed, not refused (T1).
//! - a positive wait ([`eventually`], [`eventually_pumping`]) has no budget parameter: it returns
//!   as soon as its condition holds and gives up only at [`HANG`], the one hang bound, so a
//!   cold window build or a slow OS answer only delays a green run.
//! - a bounded negative ([`holds_for`], [`holds_for_pumping`]): a condition that must keep
//!   holding for a window; a stall can only make it pass.
//! - instants that are not measurements: [`now`] (a timestamp a product API takes, a before /
//!   after bracket), and [`deadline`] / [`left`] / [`passed`] for child-process kill bounds,
//!   reader grace, hold timers and drain windows (the residual: such a loop is reviewed, T1).
//! - [`refused_loopback`], the shell copy of the refused-address premise probe (decision #56).
#![allow(dead_code)]
#![allow(clippy::disallowed_methods)]

use std::io;
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

/// The one hang bound of a positive wait: far above any OS answer a green run waits for
/// (a cold WebView2 build took up to ~20 s on the runner, T-200), so it only ends a hung test.
pub const HANG: Duration = Duration::from_secs(120);

/// How often a wait re-checks its condition.
const POLL: Duration = Duration::from_millis(10);

/// The current instant: a timestamp handed to a product API (a press, a release, a frame's
/// `at`), or one side of a [`between`] bracket. Not a measurement.
pub fn now() -> Instant {
    Instant::now()
}

/// A measured time: what [`measure`] and [`between`] return. Opaque on purpose: no comparison,
/// no `==`, no conversion back to a [`Duration`], so a ceiling on a `Took` does not compile;
/// [`at_least`] is its only reader. It does not cover instants from [`now`], whose arithmetic
/// stays open (the module doc's reviewed residual). `Debug` prints it for messages; `Default`
/// (zero) fills a report not yet measured. Pinned at compile time by `took_stays_opaque` below.
#[derive(Clone, Copy, Default)]
pub struct Took(Duration);

impl std::fmt::Debug for Took {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Runs `f` and returns its value and how long it took, as an opaque [`Took`].
pub fn measure<T>(f: impl FnOnce() -> T) -> (T, Took) {
    let started = Instant::now();
    let got = f();
    (got, Took(started.elapsed()))
}

/// The time from `start` to `end`, two instants from [`now`] (zero when `end` is first).
pub fn between(start: Instant, end: Instant) -> Took {
    Took(end.saturating_duration_since(start))
}

/// The lower bound a clock reading may decide: `took` is at least `bound` (a deadline did not
/// fire early, no shorter wrong limit was used). There is no upper side.
#[track_caller]
pub fn at_least(took: Took, bound: Duration) {
    let took = took.0;
    assert!(
        took >= bound,
        "took {took:?}, below {bound:?}: a deadline fired early or a shorter limit was used \
         (T-201: a clock reading decides only this lower bound)"
    );
}

/// The instant `d` from now: the end of a kill bound, a reader grace, a hold or a drain window.
pub fn deadline(d: Duration) -> Instant {
    Instant::now() + d
}

/// What is left until `end` (zero once it has passed), e.g. for `recv_timeout`.
pub fn left(end: Instant) -> Duration {
    end.saturating_duration_since(Instant::now())
}

/// Whether `d` has passed since `since` (an instant from [`now`]). For bounding a loop; an
/// asserted `!passed(..)` is a ceiling the types do not refuse (reviewed, T1).
pub fn passed(since: Instant, d: Duration) -> bool {
    Instant::now() >= since + d
}

/// Polls `cond` until it holds (`true`), or [`HANG`] passed (`false`: a hung test). The verdict
/// is the awaited outcome; there is no budget to race.
pub fn eventually(cond: impl FnMut() -> bool) -> bool {
    eventually_pumping(|| {}, cond)
}

/// [`eventually`] re-checking every millisecond: for a test that acts the moment an OS answer
/// comes (a race it means to provoke). Same hang bound, no budget.
pub fn eventually_tight(mut cond: impl FnMut() -> bool) -> bool {
    let end = deadline(HANG);
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        thread::sleep(Duration::from_millis(1));
    }
}

/// [`eventually`], calling `pump` before every check (a thread whose message queue the awaited
/// outcome needs, e.g. `win32_support::pump`).
pub fn eventually_pumping(mut pump: impl FnMut(), mut cond: impl FnMut() -> bool) -> bool {
    let end = deadline(HANG);
    loop {
        pump();
        if cond() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        thread::sleep(POLL);
    }
}

/// Runs `f` on its own thread and waits for it up to [`HANG`]: its value and how long it took,
/// or `None` when it is still running then (a hung call; the thread is left behind). The hang
/// bound of a call whose verdict is its result, not its duration. A panic in `f` is resumed
/// here.
pub fn within_hang<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<(T, Took)> {
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let _ = tx.send(measure(f));
    });
    match rx.recv_timeout(HANG) {
        Ok(got) => Some(got),
        Err(RecvTimeoutError::Timeout) => None,
        Err(RecvTimeoutError::Disconnected) => match worker.join() {
            Err(panic) => std::panic::resume_unwind(panic),
            Ok(()) => None,
        },
    }
}

/// A whole-test hang guard (T-128): ends the process with a named message when the test that
/// made it is still running `2 * HANG` later. It covers what no positive wait can: a call that
/// blocks for good in a rig build, a join or a drop. Dropping it (the test ended, in whatever
/// way) cancels it. It is a hang guard, never a verdict on a measured time.
pub struct HangWatch {
    cancel: Option<mpsc::Sender<()>>,
}

/// Starts a [`HangWatch`] for the test running on this thread (libtest names the thread after
/// the test); `what` names the stage that is guarded.
pub fn watch_for_hang(what: &str) -> HangWatch {
    let (cancel, ended) = mpsc::channel::<()>();
    let test = thread::current().name().unwrap_or("<unnamed>").to_string();
    let what = what.to_string();
    thread::spawn(move || {
        if let Err(RecvTimeoutError::Timeout) = ended.recv_timeout(HANG * 2) {
            eprintln!(
                "HANG: test `{test}` was still running {:?} after {what} started; a wait or a \
                 drop never returned (T-128 hang guard)",
                HANG * 2
            );
            std::process::exit(101);
        }
    });
    HangWatch {
        cancel: Some(cancel),
    }
}

impl Drop for HangWatch {
    fn drop(&mut self) {
        drop(self.cancel.take());
    }
}

/// A bounded negative: `cond` keeps holding for `within` (`false` as soon as it fails). A stall
/// only makes it pass, so `within` is sized by the bite, not by the host.
pub fn holds_for(within: Duration, cond: impl FnMut() -> bool) -> bool {
    holds_for_pumping(|| {}, within, cond)
}

/// [`holds_for`], calling `pump` before every check.
pub fn holds_for_pumping(
    mut pump: impl FnMut(),
    within: Duration,
    mut cond: impl FnMut() -> bool,
) -> bool {
    let end = deadline(within);
    while Instant::now() < end {
        pump();
        if !cond() {
            return false;
        }
        thread::sleep(POLL);
    }
    pump();
    cond()
}

/// `127.0.0.1:1`, the one loopback address a test may expect to be refused (decision #53:
/// below every ephemeral range, never bound and released). Probed at each call: a plain connect
/// must be refused within `budget / 2` (decision #56, a premise of the test, not a verdict on the
/// product), else this panics (a runner problem, not a product failure).
#[track_caller]
pub fn refused_loopback(budget: Duration) -> SocketAddr {
    let addr = SocketAddr::from(([127, 0, 0, 1], 1));
    let started = Instant::now();
    let probe = TcpStream::connect_timeout(&addr, budget);
    let took = started.elapsed();
    match probe {
        Ok(_) => panic!("premise: something listens on {addr} (decision #53)"),
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
            assert!(
                took < budget / 2,
                "premise: the connect to {addr} was refused only after {took:?} (decision #56)"
            );
            addr
        }
        Err(e) => panic!(
            "premise: the connect to {addr} was not refused within {budget:?} ({:?} after \
             {took:?})",
            e.kind()
        ),
    }
}

/// Compiles only while `T` does NOT implement the trait in the second impl: with it,
/// `<T as NotImpl<_>>::check` matches both impls and the type parameter cannot be inferred
/// (the `static_assertions::assert_not_impl_any` trick, written out, as in core's
/// `timing_tests`).
macro_rules! assert_not_impl {
    ($t:ty: $($tr:tt)+) => {{
        trait NotImpl<A> {
            fn check() {}
        }
        impl<T: ?Sized> NotImpl<()> for T {}
        struct Invalid;
        impl<T: ?Sized + $($tr)+> NotImpl<Invalid> for T {}
        let _ = <$t as NotImpl<_>>::check;
    }};
}

/// Type-checked in every crate that includes this module (never called): it stops compiling once
/// [`Took`] gains a comparison, an `==` or a way back to a [`Duration`], i.e. once a ceiling on
/// a measured time would compile.
fn took_stays_opaque() {
    use std::borrow::Borrow;
    use std::ops::Deref;
    assert_not_impl!(Took: PartialOrd<Duration>);
    assert_not_impl!(Took: PartialOrd<Took>);
    assert_not_impl!(Took: PartialEq<Duration>);
    assert_not_impl!(Took: PartialEq<Took>);
    assert_not_impl!(Took: Deref);
    assert_not_impl!(Took: Into<Duration>);
    assert_not_impl!(Took: From<Duration>);
    assert_not_impl!(Took: AsRef<Duration>);
    assert_not_impl!(Took: Borrow<Duration>);
    assert_not_impl!(Duration: PartialOrd<Took>);
    assert_not_impl!(Duration: PartialEq<Took>);
    assert_not_impl!(Duration: From<Took>);
}
