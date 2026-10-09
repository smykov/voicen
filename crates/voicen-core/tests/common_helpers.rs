//! T-080: the contract of the shared core-test helpers in `tests/common` that every
//! timing and OS-answer test relies on (docs/decisions/core-tests.md): the timing
//! module (`common::timing`, I2) and the OS-answer case (`common::os_answer`, I1).
//! Its own binary, so the OS-answer probes run in a process of their own.

mod common;

#[path = "common/os_answer_tests.rs"]
mod os_answer_tests;
#[path = "common/timing_tests.rs"]
mod timing_tests;
