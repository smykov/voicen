//! The one way a voicen-core test gets a target whose outcome comes from an OS
//! answer (T-080, invariant I1; docs/decisions/core-tests.md): a refused loopback
//! port or a name the resolver fails. `scripts/ci/core-test-clocks.sh` (in
//! `make check`) refuses such a literal (`*.invalid`, `127.0.0.1:1`, `192.0.2.1`) in
//! every test file outside `tests/common`.
//!
//! I1: an [`OsAnswer`] case carries the target and its deadlines together. Every
//! network deadline of the case is at least [`OS_ANSWER_BUDGET`], and the case's own
//! probe, run when it is built, shows that the OS answered within half of that
//! budget, so the OS answer, not a timer, ends the connect. One budget for every
//! kind; a new kind adds a probe, not a budget or a timeouts helper. Earlier, one
//! budget per kind left every kind not yet listed open: F-005 (Windows refusal
//! ~2.17 s vs a 300 ms total), F-013 (a lost resolver answer retried for ~20 s under
//! reqwest's connect timer).
//!
//! Contract tests: `common/os_answer_tests.rs` (run by `tests/common_helpers.rs`) and,
//! in each binary that takes a refused case, `common/refused_addr_tests.rs`.

use std::io;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use voicen_core::failure::FailureReason;
use voicen_core::timeouts::Timeouts;

use super::timing::measure;

/// The time a test grants the OS to answer, for every kind of OS answer (T-080).
///
/// The slowest answers measured: a refused loopback connect on windows-latest,
/// ~2.17 s (Windows retries the SYN after the RST; CI run 37204170764; F-005); a
/// `.invalid` lookup whose UDP answer is lost, ~20.1 s (glibc retries 5 s per try,
/// 2 tries, A and AAAA; `--dns 192.0.2.1`, T-078; CI run 37783297296; F-013).
/// Each probe checks its own kind within half of this budget. 60 s is also the
/// largest connect limit the settings allow (`TimeoutRole::Connect`, decision #99),
/// so a case's deadlines survive the clamp of the settings-based paths.
pub const OS_ANSWER_BUDGET: Duration = Duration::from_secs(60);

/// A target whose outcome is an OS answer, with the deadlines that let that answer,
/// not a timer, decide it, and the answer time its probe measured.
#[derive(Debug, Clone)]
pub struct OsAnswer {
    /// The authority to put in a URL (`http://{host}/v1`) and to expect back in a
    /// `CannotReach { host }`: `127.0.0.1:1` or `voicen-test.invalid`.
    pub host: String,
    /// connect = [`OS_ANSWER_BUDGET`]; every whole-request and no-data deadline = 2 ×
    /// the budget (so it cannot fire before the connect ends); `builtin` (no
    /// network) keeps the production default.
    pub timeouts: Timeouts,
    /// How long the OS took to answer this case's probe (at most half the budget).
    pub answer_took: Duration,
}

impl OsAnswer {
    /// The one loopback address a test may expect to be refused (T-047, decision
    /// #53): `127.0.0.1:1`.
    ///
    /// Never a port that was bound and released (F-004): port 1 lies below the OS
    /// ephemeral range (Linux `ip_local_port_range`, 32768–60999 by default; Windows
    /// dynamic range, 49152–65535 by default), so no `bind(0)` or `connect()` of a
    /// sibling test in the same process can be handed it. Probed at each call: a
    /// plain connect (limit [`OS_ANSWER_BUDGET`]) must be refused, within half the
    /// budget, otherwise this panics (a test-environment problem, reported loudly
    /// instead of a misclassified reason).
    pub fn refused() -> OsAnswer {
        Self::refused_on(1)
    }

    /// A refused case on a loopback port the caller bound and released, probed and
    /// sized as [`OsAnswer::refused`]. Only for `tests/local_download_refused.rs`,
    /// whose single test needs the refused port to come up later (refused, then
    /// served) and whose process binds no other port meanwhile (F-004, T-016 review
    /// 1 #1); every other test takes [`OsAnswer::refused`].
    pub fn refused_released_port(port: u16) -> OsAnswer {
        Self::refused_on(port)
    }

    fn refused_on(port: u16) -> OsAnswer {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let (probe, took) = measure(|| TcpStream::connect_timeout(&addr, OS_ANSWER_BUDGET));
        match probe {
            Ok(_) => panic!(
                "OsAnswer::refused: something listens on {addr}; the refused cases need \
                 nothing listening there (T-047, decision #53)"
            ),
            Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                Self::answered("refused", addr.to_string(), took.duration())
            }
            Err(e) => panic!(
                "OsAnswer::refused: a connect probe to {addr} was not refused within \
                 {OS_ANSWER_BUDGET:?} ({:?}: {e}, after {took:?}); the refused cases need \
                 nothing listening there and a refusal, not a timeout (T-047, decision #53)",
                e.kind()
            ),
        }
    }

    /// The one host name a test may expect never to resolve: `voicen-test.invalid`
    /// (RFC 6761 reserves `.invalid`; fake data, never sent anywhere but the system
    /// resolver).
    ///
    /// Probed at each call: a system lookup of the name (on its own thread, waited
    /// for at most [`OS_ANSWER_BUDGET`]) must fail, within half the budget,
    /// otherwise this panics (a resolver that answers `.invalid`, or one too slow
    /// for the budget: a test-environment problem).
    pub fn unresolvable() -> OsAnswer {
        const HOST: &str = "voicen-test.invalid";
        let (tx, rx) = mpsc::channel();
        let (got, took) = measure(|| {
            // Not joined: a lookup that outlives the budget must not hold the test.
            thread::spawn(move || {
                let got = (HOST, 80).to_socket_addrs().map(|addrs| addrs.count());
                let _ = tx.send(got);
            });
            rx.recv_timeout(OS_ANSWER_BUDGET)
        });
        match got {
            Ok(Ok(count)) => panic!(
                "OsAnswer::unresolvable: the system resolver resolved {HOST} ({count} \
                 address(es)); the DNS-failure cases need a resolver that fails `.invalid` \
                 (RFC 6761)"
            ),
            Ok(Err(_)) => Self::answered("unresolvable", HOST.to_string(), took.duration()),
            Err(_) => panic!(
                "OsAnswer::unresolvable: the lookup of {HOST} gave no answer within \
                 {OS_ANSWER_BUDGET:?}; the DNS-failure cases need the system resolver to \
                 fail it within half of that"
            ),
        }
    }

    /// The case for an answer that came after `took`; panics when that is more than
    /// half the budget (the budget must cover the answer twice over).
    fn answered(kind: &str, host: String, took: Duration) -> OsAnswer {
        assert!(
            2 * took <= OS_ANSWER_BUDGET,
            "OsAnswer::{kind}: the OS answered the probe of {host} only after {took:?}; \
             the cases budget {OS_ANSWER_BUDGET:?} for an OS answer and need it within \
             half of that (T-080 I1)"
        );
        OsAnswer {
            host,
            timeouts: budget_timeouts(),
            answer_took: took,
        }
    }
}

/// The deadlines of every OS-answer case (see [`OsAnswer::timeouts`]).
fn budget_timeouts() -> Timeouts {
    Timeouts {
        connect: OS_ANSWER_BUDGET,
        api_transcription: 2 * OS_ANSWER_BUDGET,
        local_server: 2 * OS_ANSWER_BUDGET,
        post_processing: 2 * OS_ANSWER_BUDGET,
        download_no_data: 2 * OS_ANSWER_BUDGET,
        ..Timeouts::default()
    }
}

/// A target that never answers (T-080 review 1 #2): a connect to it is ended by
/// the test's connect timer, or by the OS at once where the host has no route.
/// Not an [`OsAnswer`]: no OS answer is awaited, so there is no probe and no
/// budget; the case carries its deadlines and the endings it accepts, so the
/// verdict never depends on which of the two the OS gives. The host exists
/// outside `tests/common` only inside this case (the literal is refused there by
/// `scripts/ci/core-test-clocks.sh`).
#[derive(Debug, Clone)]
pub struct Unanswered {
    /// The authority to put in a URL: TEST-NET-1 (RFC 5737), `192.0.2.1`.
    pub host: String,
    /// connect = 300 ms; every whole-request and no-data deadline is ten times
    /// that (3 s), so a connect limit that does not reach the client ends the
    /// request as `Timeout`; `builtin` keeps the production default.
    pub timeouts: Timeouts,
}

impl Unanswered {
    /// The blackhole case of `api_pipeline::blackhole_connect_is_bounded_by_connect_timeout`
    /// (T-040 Notes, review 1 #3): it blackholes in voicen-rust:1.99 (probed), and
    /// may have no route on windows-latest.
    pub fn blackhole() -> Unanswered {
        let connect = Duration::from_millis(300);
        Unanswered {
            host: "192.0.2.1".to_string(),
            timeouts: Timeouts {
                connect,
                api_transcription: 10 * connect,
                local_server: 10 * connect,
                post_processing: 10 * connect,
                download_no_data: 10 * connect,
                ..Timeouts::default()
            },
        }
    }

    /// Whether `reason` is one of the two endings this case accepts: the connect
    /// timer fired (`CannotReach` for this host) or the OS had no route
    /// (`NetworkUnavailable`).
    pub fn ended_by_connect_or_os(&self, reason: &FailureReason) -> bool {
        match reason {
            FailureReason::CannotReach { host } => *host == self.host,
            FailureReason::NetworkUnavailable => true,
            _ => false,
        }
    }
}
