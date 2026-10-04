//! T-016: a refused port ends the download as `SourceUnreachable{host[:port]}`,
//! and a retry succeeds once a server listens on that port (Acceptance line 2,
//! contracts/core-traits.md "Downloader").
//!
//! Why its own binary (T-016 review round 1 #1): "nothing listens on this port"
//! must hold for the whole refused phase. A port that is bound, read and released
//! is free for the kernel to hand out again, and the parallel tests of
//! `tests/local_download.rs` bind `127.0.0.1:0` all the time: one of their mock
//! servers took the port, the "refused" download reached it and finished, and that
//! server counted an extra request (1 failure in 37 runs). Cargo runs test binaries
//! one after another, each in its own process, so here no sibling test binds a
//! port. Keep exactly one test in this file: a second test that binds a port would
//! bring the race back.

mod common;

use std::net::TcpListener;
use std::time::Duration;

use common::download::{assert_no_url_in, assert_retry_succeeds, fixture_with, FILE};
use common::{entry, url_for, FakeDisk, Server, NEEDED};
use voicen_core::local_models::catalog::ModelId;
use voicen_core::local_models::download::{DownloadEvent, DownloadFailure};

/// A loopback port with nothing listening (bound, read, released). Only safe in a
/// process that binds no other port meanwhile (module docs).
fn refused_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
    listener.local_addr().expect("local addr").port()
}

#[test]
fn refused_port_fails_source_unreachable_with_host_port_then_retry_succeeds() {
    // Nothing listens; then a server comes up on the same port. Bite: another
    // reason, the host without the port, the whole URL (path, query) in the reason.
    let port = refused_port();
    let f = fixture_with(
        vec![entry(ModelId::Base, FILE, &url_for(port, FILE))],
        FakeDisk::with_available(10 * NEEDED),
        Duration::from_secs(2),
    );
    let mut ev = f.start(ModelId::Base).expect("start");
    let end = ev.wait_end();
    assert_eq!(
        end.event,
        DownloadEvent::Failed {
            id: ModelId::Base,
            reason: DownloadFailure::SourceUnreachable {
                host: format!("127.0.0.1:{port}")
            }
        }
    );
    assert!(end.dir.is_empty(), "models dir at Failed: {:?}", end.dir);
    assert_no_url_in(&ev.seen, "refused");

    let server = Server::start_on(port, vec![]);
    assert_retry_succeeds(&f, "refused");
    assert_eq!(
        server.accepts(),
        1,
        "the retry's request reached the new server"
    );
}
