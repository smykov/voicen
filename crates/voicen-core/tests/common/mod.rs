//! Shared helpers of the voicen-core test binaries (and of
//! `src-tauri/tests/settings_ipc.rs`, which includes this file by `#[path]`):
//! - for `tests/local_download.rs`, `tests/local_download_refused.rs` and
//!   `tests/local_store.rs` (T-016) the fake model, test catalog entries, a fake disk
//!   probe and a raw-TCP mock model server, re-exported from
//!   `voicen_core::test_support::local_models` (one copy for the core and the shell
//!   tests since T-044, P-010; the download harness is in [`download`]);
//! - [`os_answer`] (T-080, I1): the only source of a target whose outcome is an OS
//!   answer (refused, unresolvable), and of the never-answered blackhole
//!   (`Unanswered`), together with its deadlines;
//! - [`timing`] (T-080, I2): the only place a core test reads the wall clock;
//! - [`held`] (T-079): a lookup held without an answer until the test releases it,
//!   and the ordering check "the call returned while the lookup was held".
//!
//! `scripts/ci/core-test-clocks.sh` (in `make check`) refuses an OS-answer literal or
//! a clock reading in any test file outside this directory. These rules stay here,
//! not in `test_support`: they are core-test-only (docs/decisions/core-tests.md).
//! The checks of the helpers are not modules of `common`: `common/os_answer_tests.rs`
//! and `common/timing_tests.rs` run in `tests/common_helpers.rs`, and
//! `common/refused_addr_tests.rs` in each binary that takes a refused case (T-047
//! review 1 #3), so a binary that takes none, `tests/local_download_refused.rs`
//! above all, runs none of its connect probes.
#![allow(dead_code)]

pub mod download;
pub mod held;
pub mod os_answer;
pub mod timing;

pub use voicen_core::test_support::local_models::*;
