//! T-165 (docs/decisions/core-tests.md, in-time rule; guard scripts/ci/core-test-deadlines.sh):
//! the download constructors of `common::download` take no `Duration`. A deadline or a hold of an
//! in-time leg comes from `os_answer::in_time()` and the named constructors in `tests/common`;
//! a caller cannot hand one in. The guard's tokens do not see `fixture(&server, Duration::..)`,
//! so rustc decides this shape: these pins stop compiling when a `Duration` parameter comes back
//! (T-111 review 1 #3, review 2 #2, decision #224), and `common::download::timeouts(<duration>)`
//! is gone.
//!
//! Not a module of `tests/common`: included by `tests/common_in_time.rs` with `#[path]`.

use std::sync::Arc;

use crate::common::download::{fixture, fixture_with, Fixture};
use crate::common::{FakeDisk, Server};
use voicen_core::local_models::catalog::CatalogEntry;

#[test]
fn fixture_takes_the_server_and_no_duration() {
    // Bite: `fixture(server, no_data: Duration)` back (the signature pin stops compiling).
    let _: fn(&Server) -> Fixture = fixture;
}

#[test]
fn fixture_with_takes_entries_and_disk_and_no_duration() {
    // Bite: `fixture_with(entries, disk, no_data: Duration)` back.
    let _: fn(Vec<CatalogEntry>, Arc<FakeDisk>) -> Fixture = fixture_with;
}
