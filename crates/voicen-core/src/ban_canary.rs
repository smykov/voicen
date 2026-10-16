//! T-089 ban canary, library side (feature `ban-canary-lib`; test tooling, never in a build that
//! ships, never called). Each probe builds a reqwest client outside `engine::http`, or reads
//! the wall clock outside `crate::clock`, in a spelling that the retired text guards
//! (scripts/ci/http-client-builder.sh, core-test-clocks.sh I2) did not read: whitespace in a
//! path, a split path, `Default`, `use ... as`, a fn reference, a macro, a glob import, a type
//! alias, the `get` shortcuts. `scripts/ci/ban-canary.sh` (`make check-ban-canary`) lints this
//! file with the package's lint config and compares every diagnostic with the owned list
//! `scripts/ci/ban-canary.expected.txt`; a line marked `probe: <id>` must have a row there and a
//! row must sit on a marked line. Rows are keyed by line: edit this file, regenerate the list.

use std::time::Instant;

#[rustfmt::skip]
pub fn h01_whitespace_in_path() {
    let _ = reqwest::blocking::Client :: builder(); // probe: h01
}

#[rustfmt::skip]
pub fn h02_path_split_over_lines() {
    let _ = reqwest::blocking::Client:: // probe: h02
        builder();
}

#[rustfmt::skip]
pub fn h03_client_default() {
    let _ = reqwest::blocking::Client::default(); // probe: h03
}

#[rustfmt::skip]
pub fn h04_use_as() {
    use reqwest::blocking::Client as Http; // probe: h04
    let _ = Http::builder(); // probe: h04
}

#[rustfmt::skip]
pub fn h05_blocking_get() {
    let _ = reqwest::blocking::get("http://example.com/"); // probe: h05
}

#[rustfmt::skip]
pub fn h06_client_builder_default() {
    let _ = reqwest::blocking::ClientBuilder::default(); // probe: h06
}

#[rustfmt::skip]
pub fn h07_typed_default() {
    let _c: reqwest::blocking::Client = Default::default(); // probe: h07
}

#[rustfmt::skip]
pub fn h08_generic_default() {
    fn make<T: Default>() -> T { T::default() }
    let _ = make::<reqwest::blocking::Client>(); // probe: h08
}

#[rustfmt::skip]
pub fn h09_derived_default_field() {
    #[derive(Default)]
    struct Holder { _c: reqwest::blocking::Client } // probe: h09
    let _ = Holder::default();
}

#[rustfmt::skip]
pub fn h10_fn_reference() {
    let make = reqwest::blocking::Client::new; // probe: h10
    let _ = make();
}

#[rustfmt::skip]
pub fn h11_local_macro() {
    macro_rules! make { ($t:ty) => { <$t>::new() } }
    let _ = make!(reqwest::blocking::Client); // probe: h11
}

#[rustfmt::skip]
pub fn h12_async_builder() {
    let _ = <reqwest::Client>::builder(); // probe: h12
}

#[rustfmt::skip]
pub fn h13_qualified_default() {
    let _ = <reqwest::blocking::Client as Default>::default(); // probe: h13
}

#[rustfmt::skip]
pub fn h14_async_whitespace_new() {
    let _ = reqwest::Client :: new(); // probe: h14
}

#[rustfmt::skip]
pub fn h15_async_client_builder_default() {
    let _ = reqwest::ClientBuilder::default(); // probe: h15
}

#[rustfmt::skip]
pub fn h16_client_builder_new_qualified() {
    let _ = <reqwest::blocking::ClientBuilder>::new(); // probe: h16
}

#[rustfmt::skip]
pub fn h17_async_get() -> impl std::future::Future<Output = reqwest::Result<reqwest::Response>> {
    reqwest::get("http://example.com/") // probe: h17
}

#[rustfmt::skip]
pub fn h18_module_alias_get() {
    use reqwest::blocking as rb;
    let _ = rb::get("http://example.com/"); // probe: h18
}

#[rustfmt::skip]
pub fn h19_glob_import_get() {
    use reqwest::blocking::*;
    let _ = get("http://example.com/"); // probe: h19
}

#[rustfmt::skip]
pub fn h20_type_alias() {
    type Http = reqwest::blocking::Client; // probe: h20
    let _ = Http::default();
}

// The ban holds under a blanket allow of every warning (the deny level is pinned by the
// package, not only by -D warnings on the command line).
#[rustfmt::skip]
#[allow(warnings)]
pub fn h21_under_allow_warnings() {
    let _ = reqwest::blocking::Client :: builder(); // probe: h21
}

// The clock ban holds in src too: only crate::clock reads the wall clock.
#[rustfmt::skip]
pub fn k14_lib_clock_read() -> Instant {
    Instant :: now() // probe: k14
}
