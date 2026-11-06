//! The one way a core test takes a wiremock server: unpooled (T-195 I2).
//!
//! wiremock's default constructor lends servers from one pool per process. When a test
//! ends its server goes back to the pool, and the next borrower gets the same server
//! after its request log is cleared, but the server thread keeps serving the
//! connections the previous borrower opened. A request from the previous test that
//! completes after the checkout lands in the next test's log (the "2 requests" flake of
//! `connection_test` under host load). A server built here is the caller's alone and
//! goes down with it, so its log holds only the requests the caller sent.
//!
//! Not a module of `tests/common`: `src-tauri/tests/settings_ipc.rs` compiles
//! `common/mod.rs` and has no wiremock. Users include it with
//! `#[path = "common/mock_http.rs"] mod mock_http;`.
//! `tests/mock_http_pool.rs` keeps every other way of taking a server out of `tests/**`.

use wiremock::MockServer;

/// A fresh mock server on an OS-assigned 127.0.0.1 port, request recording on, shared
/// with no other test.
pub async fn start() -> MockServer {
    MockServer::builder().start().await
}
