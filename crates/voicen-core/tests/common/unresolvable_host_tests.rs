//! T-078: [`unresolvable_host`] names a host the system resolver fails, and the
//! deadlines of a DNS-failure case, [`resolver_timeouts`], leave the resolver's
//! retries room.
//!
//! Not a module of `tests/common`: each binary that calls [`unresolvable_host`]
//! (`tests/openai_client.rs`) includes this file with `#[path]`, so the probe runs
//! in the process that relies on it and a binary that does not use it makes no DNS
//! lookup (the same rule as `refused_addr_tests.rs`).

use crate::common::{resolver_timeouts, unresolvable_host, RESOLVER_BUDGET};
use voicen_core::timeouts::Timeouts;

#[test]
fn unresolvable_host_is_an_rfc_6761_invalid_name() {
    // The call sites expect `NetworkUnavailable` for a name no resolver may
    // answer, and the lookup must never reach a real domain. Bite: a real or
    // merely unlikely domain, `localhost`, an IP literal (no lookup at all).
    let host = unresolvable_host();
    assert!(host.ends_with(".invalid"), "{host}");
    assert!(
        host.parse::<std::net::IpAddr>().is_err(),
        "{host}: an IP literal"
    );
}

#[test]
fn resolver_timeouts_leave_the_lookup_room_on_every_deadline() {
    // T-078: on a path that waits for a lookup, the resolver's answer, not a
    // timer, must end it (reqwest times the lookup inside the connect timeout; a
    // lookup cut off by it is CannotReach, not NetworkUnavailable). The connect
    // limit is the whole budget, which `unresolvable_host()` checks is twice the
    // measured lookup time; every whole-request and no-data deadline is twice
    // that again. The destructuring is exhaustive on purpose: a new deadline in
    // `Timeouts` does not compile here until it is sized for the lookup. Bite:
    // any of them left at a test value below the budget (openai_client's 2 s
    // connect, the 5 s total), or the probe budget not tied to these deadlines.
    let Timeouts {
        connect,
        api_transcription,
        local_server,
        post_processing,
        builtin,
        download_no_data,
    } = resolver_timeouts();
    assert_eq!(connect, RESOLVER_BUDGET, "connect");
    for (name, total) in [
        ("api_transcription", api_transcription),
        ("local_server", local_server),
        ("post_processing", post_processing),
        ("download_no_data", download_no_data),
    ] {
        assert_eq!(total, 2 * RESOLVER_BUDGET, "{name}");
    }
    // Not a network deadline: whisper.cpp runs in-process.
    assert_eq!(builtin, Timeouts::default().builtin, "builtin");
}
