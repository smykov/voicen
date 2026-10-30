//! T-111 (docs/decisions/core-tests.md): `common::os_answer::in_time()` is the one
//! deadline shape of every leg that expects an answer and whose deadline is not the
//! subject. It is the I1 shape (BUDGET scale) without an OS answer: no probe, no
//! target. A connect leg cannot be delayed on loopback, so this pin is what covers
//! every connect limit that is routed through it.
//!
//! Not a module of `tests/common`: included by `tests/common_in_time.rs` with `#[path]`.

use crate::common::os_answer::{in_time, OS_ANSWER_BUDGET};
use voicen_core::timeouts::Timeouts;

#[test]
fn in_time_is_the_budget_for_connect_and_twice_the_budget_for_every_total() {
    // The destructuring is exhaustive on purpose: a new deadline in `Timeouts` does
    // not compile here until it is sized. Bite: a connect below the budget (the
    // 2 s or 5 s of the old helpers), a total at the budget (the connect and the
    // total then fire together), a no-data limit left at a test value, a field left
    // at `Timeouts::default()` (30 s / 15 s / 60 s), `builtin` changed.
    let Timeouts {
        connect,
        api_transcription,
        local_server,
        post_processing,
        builtin,
        download_no_data,
    } = in_time();
    assert_eq!(connect, OS_ANSWER_BUDGET, "connect");
    assert_eq!(api_transcription, 2 * OS_ANSWER_BUDGET, "api_transcription");
    assert_eq!(local_server, 2 * OS_ANSWER_BUDGET, "local_server");
    assert_eq!(post_processing, 2 * OS_ANSWER_BUDGET, "post_processing");
    assert_eq!(download_no_data, 2 * OS_ANSWER_BUDGET, "download_no_data");
    // Not a network deadline: whisper.cpp runs in-process.
    assert_eq!(builtin, Timeouts::default().builtin, "builtin");
}
