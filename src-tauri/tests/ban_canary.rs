//! T-201 ban canary, shell side (feature `ban-canary-tests`; test tooling, empty without the
//! feature). Each probe reads the wall clock in a shell test crate the way a clocked verdict would:
//! the deadline of a file-local wait (the T-200 / F-019 shape), a `took < X` ceiling, a fn
//! reference, the full path, under a blanket allow. `scripts/ci/ban-canary.sh`
//! (`make check-ban-canary`) lints this file with the package's lint config and compares every
//! diagnostic with the owned list `scripts/ci/ban-canary.expected.txt`; a line marked
//! `probe: <id>` must have a row there and a row must sit on a marked line. Rows are keyed by
//! line: edit this file, regenerate the list. Not a row of scripts/ci/shell-targets.txt: a test
//! target is not one of the package's lib/bin/build-script targets.
#![cfg(feature = "ban-canary-tests")]
#![allow(dead_code)]

use std::time::{Duration, Instant, SystemTime};

#[rustfmt::skip]
fn s01_deadline_of_a_wait() -> Instant {
    let deadline = Instant::now() + Duration::from_secs(20); // probe: s01
    deadline
}

#[rustfmt::skip]
fn s02_ceiling_on_a_measured_time(t: Instant) -> bool {
    t.elapsed() < Duration::from_secs(2) // probe: s02
}

#[rustfmt::skip]
fn s03_whitespace_in_path() -> Instant {
    Instant :: now() // probe: s03
}

#[rustfmt::skip]
fn s04_ufcs(t: Instant) -> Duration {
    Instant::elapsed(&t) // probe: s04
}

#[rustfmt::skip]
fn s05_fn_reference() -> fn() -> Instant {
    Instant::now // probe: s05
}

#[rustfmt::skip]
fn s06_full_path() -> std::time::Instant {
    std::time::Instant::now() // probe: s06
}

#[rustfmt::skip]
fn s07_system_time(t: SystemTime) -> Duration {
    t.elapsed().unwrap_or_default() // probe: s07
}

#[rustfmt::skip]
#[allow(warnings)]
fn s08_under_a_blanket_allow() -> Instant {
    Instant::now() // probe: s08
}

/// The poll loop of a file-local wait, as lifecycle.rs had it (the clock read is in the helper).
#[rustfmt::skip]
fn s09_poll_helper(mut cond: impl FnMut() -> bool) -> bool {
    let start = Instant::now(); // probe: s09
    while start.elapsed() < Duration::from_secs(20) { // probe: s10
        if cond() {
            return true;
        }
    }
    false
}
