//! T-080 (I1, docs/decisions/core-tests.md): an OS-answer target (refused,
//! unresolvable) exists in a core test only as a `common::os_answer::OsAnswer` case
//! that carries its own deadlines. One budget, [`OS_ANSWER_BUDGET`], for every kind;
//! one probe per kind, run when the case is built, whose measured answer time the case
//! carries; the budget is at least twice that time and every network deadline of the
//! case is at least the budget, so the OS answer, not a timer, ends the connect
//! (F-005: Windows refusal ~2.17 s; F-013: the resolver's retries under reqwest's
//! connect timer). A new kind adds a probe, not a budget or a timeouts helper.
//!
//! Not a module of `tests/common`: included by `tests/common_helpers.rs` with
//! `#[path]`, so the probes run in a binary of their own (the `refused_addr_tests.rs`
//! rule: never in `tests/local_download_refused.rs`).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use crate::common::os_answer::{OsAnswer, Unanswered, OS_ANSWER_BUDGET};
use voicen_core::failure::FailureReason;
use voicen_core::timeouts::Timeouts;

/// Every kind the seam offers today, built through its probe.
fn cases() -> Vec<(&'static str, OsAnswer)> {
    vec![
        ("refused", OsAnswer::refused()),
        ("unresolvable", OsAnswer::unresolvable()),
    ]
}

#[test]
fn os_answer_budget_is_at_least_twice_each_probed_answer_time() {
    // The probe's measured answer time travels with the case, and the one budget
    // covers it twice over for every kind. Bite: a case that does not run its
    // probe or does not record the time (answer_took zero), a budget sized for the
    // refusal only (the resolver kind then needs a second constant), a budget
    // below twice a measured answer.
    for (kind, case) in cases() {
        assert!(
            case.answer_took > Duration::ZERO,
            "{kind}: the probe's answer time was not measured ({:?})",
            case.answer_took
        );
        assert!(
            2 * case.answer_took <= OS_ANSWER_BUDGET,
            "{kind}: the OS answered after {:?}; OS_ANSWER_BUDGET {OS_ANSWER_BUDGET:?} \
             must be at least twice that",
            case.answer_took
        );
    }
}

#[test]
fn every_network_deadline_of_an_os_answer_case_is_at_least_the_budget() {
    // I1: no deadline of the case can fire before the OS answer. The destructuring
    // is exhaustive on purpose: a new deadline in `Timeouts` does not compile here
    // until it is sized. Bite: a test value left on any deadline (openai_client's 2 s
    // connect, the 300 ms leak-test total, the download harness's 2 s), a case built
    // with `Timeouts::default()` plus a budget-sized connect (30 s / 15 s totals,
    // below a budget that covers the resolver's 20 s retry twice). A per-kind
    // budget that each probe still covers is not visible here; the exhaustive
    // destructure and the shared constant are what keep it one.
    for (kind, case) in cases() {
        let Timeouts {
            connect,
            api_transcription,
            local_server,
            post_processing,
            builtin,
            download_no_data,
        } = case.timeouts;
        for (name, deadline) in [
            ("connect", connect),
            ("api_transcription", api_transcription),
            ("local_server", local_server),
            ("post_processing", post_processing),
            ("download_no_data", download_no_data),
        ] {
            assert!(
                deadline >= OS_ANSWER_BUDGET,
                "{kind}: {name} = {deadline:?} is below OS_ANSWER_BUDGET {OS_ANSWER_BUDGET:?}"
            );
        }
        // Not a network deadline: whisper.cpp runs in-process.
        assert_eq!(builtin, Timeouts::default().builtin, "{kind}: builtin");
    }
}

#[test]
fn refused_case_is_the_loopback_port_below_every_ephemeral_range() {
    // F-004 (decision #53): never a bound-and-released port; the host is the
    // authority callers put in a URL and expect back in `CannotReach { host }`.
    // Bite: 127.0.0.1:0-and-release, a LAN or non-loopback address, a host
    // without its port.
    let case = OsAnswer::refused();
    let addr: SocketAddr = case
        .host
        .parse()
        .unwrap_or_else(|e| panic!("refused host {:?} is not ip:port: {e}", case.host));
    assert_eq!(addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST), "{addr}");
    assert!(
        addr.port() != 0 && addr.port() < 1024,
        "{addr}: a port inside an ephemeral range can be handed to a sibling test"
    );
}

#[test]
fn unresolvable_case_is_an_rfc_6761_invalid_name() {
    // The lookup must fail by DNS and never reach a real domain. Bite: a real or
    // merely unlikely domain, `localhost`, an IP literal (no lookup at all).
    let case = OsAnswer::unresolvable();
    assert!(case.host.ends_with(".invalid"), "{}", case.host);
    assert!(
        case.host.parse::<IpAddr>().is_err(),
        "{}: an IP literal",
        case.host
    );
    assert!(
        !case.host.contains(':'),
        "{}: a bare host name, no port",
        case.host
    );
}

#[test]
fn unanswered_case_carries_a_connect_far_below_every_total() {
    // T-080 review 1 #2: the unrouted kind (a blackholed connect) exists outside
    // `tests/common` only as an `Unanswered` case with its deadlines, never as a
    // bare host. Its verdict is decided by two timers the test sets (the connect
    // timer against the whole-request total), so every total is at least ten
    // times the connect: a connect limit that does not reach the client lets the
    // total fire (Timeout), however loaded the host. Exhaustive destructure: a new
    // deadline does not compile here until it is sized. Bite: a total close to the
    // connect (the ending then races host scheduling), a test-chosen connect.
    let case = Unanswered::blackhole();
    let Timeouts {
        connect,
        api_transcription,
        local_server,
        post_processing,
        builtin,
        download_no_data,
    } = case.timeouts;
    assert!(connect > Duration::ZERO, "connect {connect:?}");
    for (name, total) in [
        ("api_transcription", api_transcription),
        ("local_server", local_server),
        ("post_processing", post_processing),
        ("download_no_data", download_no_data),
    ] {
        assert!(
            total >= 10 * connect,
            "{name} = {total:?} is not at least ten times the connect {connect:?}"
        );
    }
    assert_eq!(builtin, Timeouts::default().builtin, "builtin");
}

#[test]
fn unanswered_case_is_test_net_1_and_accepts_only_the_connect_or_os_ending() {
    // RFC 5737 TEST-NET-1, never answered. Both endings the OS may give are
    // accepted (the connect timer firing: CannotReach for this host; no route at
    // once: NetworkUnavailable), so no OS answer races the timer for the verdict.
    // Bite: Timeout accepted (the connect limit not reaching the client would
    // pass), CannotReach for another host accepted, a routable address.
    let case = Unanswered::blackhole();
    let ip: Ipv4Addr = case.host.parse().unwrap_or_else(|e| {
        panic!(
            "unanswered host {:?} is not an IPv4 address: {e}",
            case.host
        )
    });
    assert_eq!(ip.octets()[..3], [192, 0, 2], "{ip}: TEST-NET-1 (RFC 5737)");
    assert!(case.ended_by_connect_or_os(&FailureReason::CannotReach {
        host: case.host.clone()
    }));
    assert!(case.ended_by_connect_or_os(&FailureReason::NetworkUnavailable));
    assert!(!case.ended_by_connect_or_os(&FailureReason::Timeout));
    assert!(!case.ended_by_connect_or_os(&FailureReason::CannotReach {
        host: "127.0.0.1:1".to_string()
    }));
}
