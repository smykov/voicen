//! Shared helpers of the voicen-core test binaries: for `tests/local_download.rs`,
//! `tests/local_download_refused.rs` and `tests/local_store.rs` (T-016) the fake
//! model, test catalog entries, a fake disk probe and a raw-TCP mock model server,
//! re-exported from `voicen_core::test_support::local_models` (one copy for the core
//! and the shell tests since T-044, P-010; the download harness is in [`download`]);
//! for `tests/openai_client.rs` and `tests/api_pipeline.rs` the refused address
//! [`refused_addr`] (T-047); for every test that expects a refused connect the
//! deadlines [`refused_timeouts`], sized by [`REFUSAL_BUDGET`] (T-048). The
//! refused-address rules stay here, not in `test_support`: they are core-test-only
//! (docs/decisions/core-tests.md). The
//! checks of [`refused_addr`] (`common/refused_addr_tests.rs`) are not a module of
//! `common`: only those two binaries include them (T-047 review 1 #3), so a binary
//! that does not use the helper, `tests/local_download_refused.rs` above all, runs
//! none of its connect probes.
#![allow(dead_code)]

pub mod download;

use std::io;
use std::net::{SocketAddr, TcpStream};
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
/// Tests: `refused_addr_tests.rs`, included by `tests/openai_client.rs` and
/// `tests/api_pipeline.rs`.
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
