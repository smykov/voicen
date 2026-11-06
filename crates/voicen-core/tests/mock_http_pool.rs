//! T-195 I2: a core test reads only the request log of a mock HTTP server that no other
//! test has borrowed.
//!
//! wiremock 0.6.5 lends servers from one pool per process and clears a server's request
//! log when the next borrower checks it out, but the server thread keeps serving the
//! connections the previous borrower opened. A request that arrives after the next
//! checkout is then recorded in the next borrower's log (the "2 requests" flake of
//! `connection_test`, seen under host load). This binary holds the deterministic
//! reproduction (no load needed) and the structural test that keeps the pooled call out
//! of every core test.
//!
//! The reproduction is the only test of this binary that borrows a server, so no other
//! test takes the idle one. It borrows through `borrow()` below. Today `borrow()` is the
//! pooled call (red: the second borrower's log holds the late request). The developer
//! switches its body to the unpooled helper
//! (`#[path = "common/mock_http.rs"] mod mock_http;` and `mock_http::start().await`),
//! which turns both tests here green (the structural test also counts this file).
//!
//! Fake data only: 127.0.0.1.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use wiremock::MockServer;

#[path = "common/mock_http.rs"]
mod mock_http;

/// The one place this file takes a server. The developer replaces the body with the
/// unpooled helper; nothing else in the file changes.
async fn borrow() -> MockServer {
    mock_http::start().await
}

/// Where the late request must not show up: the log of a server the test just borrowed.
#[tokio::test]
async fn a_request_still_in_flight_from_the_previous_borrower_never_reaches_the_next_borrowers_log()
{
    const BODY: &str = "abcdefghij";
    let first = borrow().await;
    let addr = first.address().to_owned();

    // The head promises ten bytes; only five are sent, so the server has not recorded
    // the request yet.
    let mut raw = TcpStream::connect(addr).expect("connect to the mock server");
    raw.set_read_timeout(Some(Duration::from_secs(30)))
        .expect("read timeout");
    let head = format!(
        "POST /v1/late HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        BODY.len()
    );
    raw.write_all(head.as_bytes()).expect("write head");
    raw.write_all(&BODY.as_bytes()[..5])
        .expect("write half body");
    raw.flush().expect("flush");

    // The previous borrower is done: its server goes back (pooled) or is gone (unpooled).
    drop(first);
    // The next test borrows. Pooled: the same server comes back after a reset.
    let second = borrow().await;

    // The previous borrower's request completes after the checkout.
    // (Unpooled, the first server is already gone and the write may be refused.)
    let _ = raw.write_all(&BODY.as_bytes()[5..]);
    let _ = raw.flush();
    // Pooled: the server is alive and answers. Unpooled: the first server went down with
    // its borrower and the stream is reset. Either way the wait ends; what it left in the
    // second borrower's log is the assertion.
    let mut answer = Vec::new();
    let _ = raw.read_to_end(&mut answer);

    let seen = second
        .received_requests()
        .await
        .expect("request recording is on");
    assert_eq!(
        seen.len(),
        0,
        "the second borrower's log holds a request it never sent: {:?}",
        seen.iter()
            .map(|r| r.url.path().to_owned())
            .collect::<Vec<_>>()
    );
}

/// The raw token this crate's tests must not contain outside the helper. Built from parts
/// so that this file does not contain it itself.
fn token() -> String {
    ["MockServer", "::start"].concat()
}

/// True when `src` calls (or names) the pooled constructor. Whitespace is ignored, so
/// spaces around `::` or before `()` and a call split over lines count; `wiremock::` prefixed
/// paths contain the same text. `MockServer::builder().start()` does not match. An
/// alias import (`use wiremock::MockServer as Other`) is not seen: out of scope, the
/// import line would be reviewed with the helper.
fn names_pooled_start(src: &str) -> bool {
    let squeezed: String = src.chars().filter(|c| !c.is_whitespace()).collect();
    let tok = token();
    let mut from = 0;
    while let Some(at) = squeezed[from..].find(&tok) {
        let end = from + at + tok.len();
        let next = squeezed[end..].chars().next();
        if !next.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            return true;
        }
        from = end;
    }
    false
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| e.expect("dir entry").path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Every wiremock server in `tests/**` comes from the one unpooled helper
/// `tests/common/mock_http.rs`; the pooled constructor appears nowhere else.
#[test]
fn the_pooled_constructor_appears_in_no_core_test_outside_the_helper() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    assert!(
        files.len() > 20,
        "the scan must see the test tree, saw {} files",
        files.len()
    );
    let helper = root.join("common").join("mock_http.rs");
    let hits: Vec<String> = files
        .iter()
        .filter(|p| **p != helper)
        .filter(|p| {
            let src =
                std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
            names_pooled_start(&src)
        })
        .map(|p| p.strip_prefix(&root).unwrap().display().to_string())
        .collect();
    assert!(
        hits.is_empty(),
        "pooled mock servers still used in: {hits:?}"
    );
}

/// The scanner itself: the spellings it must catch and the unpooled form it must let by.
#[test]
fn the_scan_catches_every_spelling_of_the_pooled_call_and_lets_the_builder_by() {
    let t = token();
    for src in [
        format!("let s = {t}().await;"),
        format!("let s = {t} ().await;"),
        format!("let s = wiremock::{t}().await;"),
        format!("let s = {}\n    ::{}\n    ().await;", "MockServer", "start"),
        format!("let make = {t};"),
        format!("// {t}() in a comment still counts"),
    ] {
        assert!(names_pooled_start(&src), "missed: {src:?}");
    }
    for src in [
        "let s = MockServer::builder().start().await;".to_owned(),
        "let s = mock_http::start().await;".to_owned(),
        format!("let s = {t}_with_config().await;"),
    ] {
        assert!(!names_pooled_start(&src), "false hit: {src:?}");
    }
}
