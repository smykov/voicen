//! T-111: the new seam of `tests/common` that the in-time legs are routed through
//! (docs/decisions/core-tests.md): `os_answer::in_time()`, `timing::M`,
//! `timing::skip` and `timing::with_premise`.
//!
//! Its own binary on purpose: these tests name symbols that do not exist before the
//! implementation, so they do not compile until then. Kept apart from
//! `tests/common_helpers.rs`, the red of the tests there (the `Unanswered` totals)
//! stays a run-time assertion instead of a build error. After the implementation the
//! two modules can move into `common_helpers.rs` (a developer's choice).

mod common;

#[path = "common/in_time_tests.rs"]
mod in_time_tests;
#[path = "common/timing_seam_tests.rs"]
mod timing_seam_tests;
