//! Shared helpers of the voicen-core test binaries: for `tests/local_download.rs`,
//! `tests/local_download_refused.rs` and `tests/local_store.rs` (T-016) the fake
//! model, test catalog entries, a fake disk probe and a raw-TCP mock model server,
//! re-exported from `voicen_core::test_support::local_models` (one copy for the core
//! and the shell tests since T-044, P-010; the download harness is in [`download`]);
//! for `tests/openai_client.rs`, `tests/api_pipeline.rs`, `tests/diag_pipeline.rs`
//! (T-008) and `tests/local_server.rs` (T-018) the refused address [`refused_addr`] (T-047); for every test that
//! expects a refused connect the
//! deadlines [`refused_timeouts`], sized by [`REFUSAL_BUDGET`] (T-048); for
//! `tests/openai_client.rs` the unresolvable host [`unresolvable_host`] and its
//! deadlines [`resolver_timeouts`], sized by [`RESOLVER_BUDGET`] (T-078); for
//! every wall-clock bound of a timeout test the [`OVERHEAD_ALLOWANCE`] (T-078). The
//! refused-address and resolver rules stay here, not in `test_support`: they are
//! core-test-only (docs/decisions/core-tests.md). The
//! checks of [`refused_addr`] (`common/refused_addr_tests.rs`) are not a module of
//! `common`: only the binaries that call it include them (T-047 review 1 #3), so a binary
//! that does not use the helper, `tests/local_download_refused.rs` above all, runs
//! none of its connect probes. The checks of [`unresolvable_host`]
//! (`common/unresolvable_host_tests.rs`) follow the same rule.
#![allow(dead_code)]

pub mod download;

use std::io;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub use voicen_core::test_support::local_models::*;
use voicen_core::timeouts::Timeouts;

/// The time a test grants a refused loopback connect to come back refused
/// (T-048, decision #56).
///
/// On Linux the refusal is immediate. On windows-latest a refused loopback connect
/// returns `ConnectionRefused` only after ~2.17 s (Windows retries the SYN after
/// the RST; CI run 37204170764: 2.17 s per `refused_addr()` probe). A deadline
/// below that on a refused path fires first and decides the reason (`Timeout`,
/// `DownloadInterrupted`) instead of the refusal (`CannotReach`,
/// `SourceUnreachable`). So every deadline on such a path comes from
/// [`refused_timeouts`], and the [`refused_addr`] probe checks that the refusal
/// arrives within half of this budget: a platform whose refusal grows past that
/// fails loudly at the probe instead of being misclassified later.
pub const REFUSAL_BUDGET: Duration = Duration::from_secs(5);

/// The deadlines of every test that expects a refused connect (T-048): connect =
/// [`REFUSAL_BUDGET`], every whole-request and no-data deadline = 2 ×
/// [`REFUSAL_BUDGET`], so the refusal itself, not a timer, ends the connect on
/// Linux and on Windows. `builtin` (no network) keeps the production default.
/// Timeout tests do not use this: they use a server that accepts and answers
/// slowly, never a refused or unroutable address.
pub fn refused_timeouts() -> Timeouts {
    Timeouts {
        connect: REFUSAL_BUDGET,
        api_transcription: 2 * REFUSAL_BUDGET,
        local_server: 2 * REFUSAL_BUDGET,
        post_processing: 2 * REFUSAL_BUDGET,
        download_no_data: 2 * REFUSAL_BUDGET,
        ..Timeouts::default()
    }
}

/// The time a test grants the system resolver to fail a lookup of an
/// unresolvable name (T-078).
///
/// reqwest times the DNS lookup as part of the connect (`connect_timeout` wraps
/// the whole connector), and a lookup cut off by that timer is not a DNS error:
/// it comes back as a connect timeout (`CannotReach`), not `NetworkUnavailable`.
/// With a normal resolver the `.invalid` lookup fails in ~0.2 s; when a UDP answer
/// is lost, glibc retries (5 s per try, 2 tries, A and AAAA): 20.1 s measured with
/// the resolver blackholed (`--dns 192.0.2.1`, T-078 Investigation; CI run
/// 37783297296 lost one answer). Windows' GetAddrInfoW schedule is checked by the
/// probe on the Windows CI job. So every deadline on a path that waits for a
/// lookup comes from [`resolver_timeouts`], and the [`unresolvable_host`] probe
/// checks that the lookup fails within half of this budget: a resolver slower than
/// that fails loudly at the probe instead of being misclassified later.
pub const RESOLVER_BUDGET: Duration = Duration::from_secs(60);

/// The deadlines of every test that expects a failed DNS lookup (T-078): connect =
/// [`RESOLVER_BUDGET`], every whole-request and no-data deadline = 2 ×
/// [`RESOLVER_BUDGET`], so the resolver's own answer, not a timer, ends the
/// lookup, even after the OS retries. `builtin` (no network) keeps the
/// production default. The same pattern as [`refused_timeouts`].
pub fn resolver_timeouts() -> Timeouts {
    Timeouts {
        connect: RESOLVER_BUDGET,
        api_transcription: 2 * RESOLVER_BUDGET,
        local_server: 2 * RESOLVER_BUDGET,
        post_processing: 2 * RESOLVER_BUDGET,
        download_no_data: 2 * RESOLVER_BUDGET,
        ..Timeouts::default()
    }
}

/// The one host name a test may expect never to resolve (T-078):
/// `voicen-test.invalid` (RFC 6761 reserves `.invalid`; fake data, never sent
/// anywhere but the system resolver).
///
/// Checked at each call: a system lookup of the name (on its own thread, waited
/// for at most [`RESOLVER_BUDGET`]) must fail, and within `RESOLVER_BUDGET / 2`;
/// otherwise this panics as a test-environment problem (a resolver that answers
/// `.invalid`, or one too slow for the budget), reported loudly instead of a flaky
/// reason. Tests: `unresolvable_host_tests.rs`, included by
/// `tests/openai_client.rs`.
pub fn unresolvable_host() -> &'static str {
    const HOST: &str = "voicen-test.invalid";
    let (tx, rx) = mpsc::channel();
    let started = Instant::now();
    // Not joined: a lookup that outlives the budget must not hold the test past it.
    std::thread::spawn(move || {
        let got = (HOST, 80).to_socket_addrs().map(|addrs| addrs.count());
        let _ = tx.send(got);
    });
    match rx.recv_timeout(RESOLVER_BUDGET) {
        Ok(Ok(count)) => panic!(
            "unresolvable_host: the system resolver resolved {HOST} ({count} \
             address(es)); the DNS-failure tests need a resolver that fails `.invalid` \
             (RFC 6761; T-078)"
        ),
        Ok(Err(e)) => {
            let took = started.elapsed();
            assert!(
                took < RESOLVER_BUDGET / 2,
                "unresolvable_host: the lookup of {HOST} failed only after {took:?} \
                 ({e}); the DNS-failure tests budget {RESOLVER_BUDGET:?} for a lookup \
                 and need it within half of that (T-078)"
            );
            HOST
        }
        Err(_) => panic!(
            "unresolvable_host: the lookup of {HOST} gave no answer within \
             {RESOLVER_BUDGET:?}; the DNS-failure tests need the system resolver to \
             fail it within half of that (T-078)"
        ),
    }
}

/// The overhead a wall-clock bound of a timeout test grants above the expected
/// deadline (T-078): the timed window includes more than the request timer
/// (`engine::http::client` built per call: a runtime thread and a parse of the
/// system CA bundle, 64-781 ms measured under parallel load; client drop, VAD, WAV,
/// thread start). Measured overruns under load: 1.48 s over a 300 ms connect
/// limit, 2.29 s over a 5 s post-processing deadline (T-078 Investigation). Every
/// upper bound is `deadline + OVERHEAD_ALLOWANCE`, and the test's server delays and
/// other wrong deadlines lie above that bound, so it still catches them.
pub const OVERHEAD_ALLOWANCE: Duration = Duration::from_secs(3);

/// The one loopback address a test may expect to be refused (T-047, decision #53):
/// `127.0.0.1:1`.
///
/// Invariant: never a port that was bound and released. Port 1 lies below the OS
/// ephemeral range (Linux `ip_local_port_range`, 32768–60999 by default; Windows
/// dynamic range, 49152–65535 by default), so no `bind(0)` or `connect()` of a
/// sibling test in the same process can be handed it. That nothing listens there
/// is checked at each call: a plain connect (limit [`REFUSAL_BUDGET`]) must be
/// refused, and within `REFUSAL_BUDGET / 2` (T-048), otherwise this panics (a
/// test-environment problem, reported loudly instead of a flaky result).
/// Tests: `refused_addr_tests.rs`, included by `tests/openai_client.rs`,
/// `tests/api_pipeline.rs`, `tests/diag_pipeline.rs` and `tests/local_server.rs`.
pub fn refused_addr() -> SocketAddr {
    let addr = SocketAddr::from(([127, 0, 0, 1], 1));
    let started = Instant::now();
    let probe = TcpStream::connect_timeout(&addr, REFUSAL_BUDGET);
    let took = started.elapsed();
    match probe {
        Ok(_) => panic!(
            "refused_addr: something listens on {addr}; the refused-port tests need \
             nothing listening there (T-047, decision #53)"
        ),
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
            assert!(
                took < REFUSAL_BUDGET / 2,
                "refused_addr: the connect probe to {addr} was refused only after \
                 {took:?}; the refused-port tests budget {REFUSAL_BUDGET:?} for a \
                 refusal and need it within half of that (T-048, decision #56)"
            );
            addr
        }
        Err(e) => panic!(
            "refused_addr: a connect probe to {addr} was not refused within \
             {REFUSAL_BUDGET:?} ({:?}: {e}, after {took:?}); the refused-port tests \
             need nothing listening there and a refusal, not a timeout (T-047, \
             decision #53)",
            e.kind()
        ),
    }
}
