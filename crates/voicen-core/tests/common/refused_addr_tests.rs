//! T-047 (decision #53): [`refused_addr`] can never be handed to another socket
//! of the test process, and nothing listens on it.
//!
//! These run in every test binary that compiles `tests/common`, so each binary
//! that uses the helper checks it in its own process. They bind no port (the
//! connect probe only takes a source port for an instant), so they keep the
//! "no other port bind" rule of `tests/local_download_refused.rs`.

use std::io::ErrorKind;
use std::net::{Ipv4Addr, TcpStream};
use std::time::Duration;

use super::refused_addr;

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
    // from inside the range by construction), port 0.
    if !cfg!(target_os = "linux") {
        eprintln!("skipped: the ephemeral range is read from {PORT_RANGE} on Linux only");
        return;
    }
    let low = match ephemeral_low_bound() {
        Ok(low) => low,
        Err(why) => {
            eprintln!("skipped: no ephemeral range ({why})");
            return;
        }
    };
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
    // Nothing listens there: a plain connect is refused at once, not accepted and
    // not timed out. Bite: a held listener's address, an unroutable address
    // (TimedOut), a non-loopback address.
    let addr = refused_addr();
    match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
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
