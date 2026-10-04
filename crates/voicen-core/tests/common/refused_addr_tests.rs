//! T-047 (decision #53): [`refused_addr`] can never be handed to another socket
//! of the test process, and nothing listens on it. T-048 (decision #56): the
//! deadlines of a refused case, [`refused_timeouts`], leave the refusal room.
//!
//! Not a module of `tests/common`: each binary that calls [`refused_addr`]
//! (`tests/openai_client.rs`, `tests/api_pipeline.rs`) includes this file with
//! `#[path]`, so the helper is checked in the process that relies on it, and a
//! binary that does not use it runs none of these connect probes (each probe takes
//! an ephemeral source port for an instant; T-047 review 1 #3). Keep it out of
//! `tests/local_download_refused.rs`, whose refused phase needs a process that
//! takes no other port.

use std::io::ErrorKind;
use std::net::{Ipv4Addr, TcpStream};

use crate::common::{refused_addr, refused_timeouts, REFUSAL_BUDGET};
use voicen_core::timeouts::Timeouts;

/// Linux: the range `bind(0)` and `connect()` pick local ports from.
const PORT_RANGE: &str = "/proc/sys/net/ipv4/ip_local_port_range";

/// The low bound of [`PORT_RANGE`] (`"32768\t60999"`), or why it cannot be read.
fn ephemeral_low_bound() -> Result<u16, String> {
    let raw = std::fs::read_to_string(PORT_RANGE).map_err(|e| format!("{PORT_RANGE}: {e}"))?;
    let low = raw
        .split_whitespace()
        .next()
        .ok_or_else(|| format!("{PORT_RANGE}: empty"))?;
    low.parse()
        .map_err(|e| format!("{PORT_RANGE}: {low:?}: {e}"))
}

#[test]
fn refused_addr_port_is_below_the_ephemeral_range() {
    // The invariant itself: the OS never gives a port outside the ephemeral
    // range to a sibling's bind(0) (wiremock, partial_body_server) or to a
    // connect's source port. Bite: the bind-and-release helper (its port comes
    // from inside the range by construction), port 0. On Linux (the gate) the
    // range must be readable: a check that silently did not run would be a green
    // that proves nothing (T-047 review 1 #4). Other OSes (the Windows CI) have no
    // such file and skip; the doc of `refused_addr()` covers their range.
    if !cfg!(target_os = "linux") {
        eprintln!("skipped: the ephemeral range is read from {PORT_RANGE} on Linux only");
        return;
    }
    let low = ephemeral_low_bound()
        .unwrap_or_else(|why| panic!("cannot check the invariant on Linux: {why}"));
    let addr = refused_addr();
    assert_ne!(addr.port(), 0, "{addr}: port 0 is not an address");
    assert!(
        addr.port() < low,
        "{addr}: port inside the ephemeral range {low}.. (from {PORT_RANGE}); \
         a sibling test's bind(0) or connect() can be handed it"
    );
}

#[test]
fn refused_addr_is_refused_by_a_connect_probe() {
    // Nothing listens there: a plain connect is refused, not accepted and not
    // timed out (immediately on Linux, after ~2.17 s on windows-latest; T-048).
    // Bite: a held listener's address, an unroutable address (TimedOut), a
    // non-loopback address.
    let addr = refused_addr();
    match TcpStream::connect_timeout(&addr, REFUSAL_BUDGET) {
        Ok(_) => panic!("{addr}: something listens there"),
        Err(e) => assert_eq!(e.kind(), ErrorKind::ConnectionRefused, "{addr}: {e}"),
    }
}

#[test]
fn refused_addr_is_ipv4_loopback() {
    // The call sites expect `CannotReach{host: "127.0.0.1:<port>"}`, and a refused
    // request must never leave the machine. Bite: 0.0.0.0, [::1], a LAN address.
    let addr = refused_addr();
    assert_eq!(addr.ip(), Ipv4Addr::LOCALHOST, "{addr}");
}

#[test]
fn refused_timeouts_leave_the_refusal_room_on_every_deadline() {
    // T-048 (decision #56): on a refused path the refusal, not a timer, must end
    // the connect. The connect limit is the whole budget, which `refused_addr()`
    // checks is twice the measured refusal time; every whole-request and no-data
    // deadline is twice that again, so it cannot fire before the connect ends.
    // The destructuring is exhaustive on purpose: a new deadline in `Timeouts`
    // does not compile here until it is sized for the refusal. Bite: any of them
    // left at a test value below the budget (the leak test's 300 ms total, the
    // download harness's 2 s connect / no-data), or the probe budget not tied to
    // these deadlines.
    let Timeouts {
        connect,
        api_transcription,
        local_server,
        post_processing,
        builtin,
        download_no_data,
    } = refused_timeouts();
    assert_eq!(connect, REFUSAL_BUDGET, "connect");
    for (name, total) in [
        ("api_transcription", api_transcription),
        ("local_server", local_server),
        ("post_processing", post_processing),
        ("download_no_data", download_no_data),
    ] {
        assert_eq!(total, 2 * REFUSAL_BUDGET, "{name}");
    }
    // Not a network deadline: whisper.cpp runs in-process.
    assert_eq!(builtin, Timeouts::default().builtin, "builtin");
}
