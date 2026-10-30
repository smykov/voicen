//! T-111: the new seam of `tests/common` that the in-time legs are routed through
//! (docs/decisions/core-tests.md): `os_answer::in_time()`, `timing::M`,
//! `timing::late_answer`, `timing::skip` and `timing::with_premise`.

mod common;

#[path = "common/in_time_tests.rs"]
mod in_time_tests;
#[path = "common/timing_seam_tests.rs"]
mod timing_seam_tests;
