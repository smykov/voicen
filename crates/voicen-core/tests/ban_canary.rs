//! T-089 ban canary, integration-test side (feature `ban-canary-tests`; test tooling, empty without
//! the feature). Each probe reads the wall clock outside tests/common in a spelling the retired
//! text guard (scripts/ci/core-test-clocks.sh I2) did not read: whitespace in a path, a split
//! alias or call, UFCS, a fn reference or pointer, a macro, inside `#[tokio::test]`, `assert!`
//! or `format!`, the full path, under a blanket allow. `scripts/ci/ban-canary.sh`
//! (`make check-ban-canary`) lints this file with the package's lint config and compares every
//! diagnostic with the owned list `scripts/ci/ban-canary.expected.txt`; a line marked
//! `probe: <id>` must have a row there and a row must sit on a marked line. Rows are keyed by
//! line: edit this file, regenerate the list.
#![cfg(feature = "ban-canary-tests")]
#![allow(dead_code)]

use std::time::{Duration, Instant, SystemTime};

#[rustfmt::skip]
fn k01_whitespace_in_path() -> Instant {
    Instant :: now() // probe: k01
}

#[rustfmt::skip]
fn k02_whitespace_before_call(t: Instant) -> Duration {
    t.elapsed () // probe: k02
}

#[rustfmt::skip]
fn k03_alias_split_over_lines() -> Instant {
    use std::time::{
        Instant
        as Clock};
    Clock::now() // probe: k03
}

#[rustfmt::skip]
fn k04_ufcs(t: Instant) -> Duration {
    Instant::elapsed(&t) // probe: k04
}

#[rustfmt::skip]
fn k05_method_as_value(ts: &[Instant]) -> Vec<Duration> {
    ts.iter().map(Instant::elapsed).collect() // probe: k05
}

#[rustfmt::skip]
#[tokio::test]
async fn k06_inside_tokio_test() {
    let _ = Instant :: now(); // probe: k06
}

#[rustfmt::skip]
fn k07_inside_assert(t: Instant) {
    assert!(Instant :: now() >= t); // probe: k07
}

#[rustfmt::skip]
fn k08_local_macro() -> Instant {
    macro_rules! read { ($t:ty) => { <$t>::now() } } // probe: k08
    read!(Instant)
}

#[rustfmt::skip]
fn k09_call_split_over_lines(t: Instant) -> Duration {
    t.elapsed // probe: k09
        ()
}

#[rustfmt::skip]
fn k10_fn_pointer() -> Instant {
    let read: fn() -> Instant = Instant :: now; // probe: k10
    read()
}

#[rustfmt::skip]
#[tokio::test]
async fn k11_elapsed_inside_tokio_test() {
    let _ = Instant :: now().elapsed (); // probe: k11
}

#[rustfmt::skip]
fn k12_inside_format(t: Instant) -> String {
    format!("{:?}", t.elapsed ()) // probe: k12
}

#[rustfmt::skip]
fn k13_full_path() -> ::std::time::Instant {
    ::std::time::Instant:: now() // probe: k13
}

// The ban holds under a blanket allow of every warning (the deny level is pinned by the
// package, not only by -D warnings on the command line).
#[rustfmt::skip]
#[allow(warnings)]
fn k15_under_allow_warnings() -> Instant {
    Instant :: now() // probe: k15
}

// A wall-clock measurement on SystemTime (T-089 review 1 #2): the retired text guard refused
// any `.elapsed(`, so SystemTime::elapsed was banned before the switch and must stay banned.
#[rustfmt::skip]
fn k16_system_time_elapsed(t: std::time::SystemTime) -> bool {
    t.elapsed ().is_ok() // probe: k16
}

#[rustfmt::skip]
fn k17_system_time_elapsed_ufcs(t: SystemTime) -> bool {
    SystemTime :: elapsed(&t).is_ok() // probe: k17
}
