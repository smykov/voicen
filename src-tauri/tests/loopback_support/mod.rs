//! T-175: the one loopback accept loop that `dictation_e2e.rs::ChatMock` and
//! `call_chain.rs::Mock` share (each keeps its own answer function). Windows CI only.
//!
//! RED STUB (test-writer, T-175): this is the loop as both mocks have it today, an accept
//! thread over `listener.incoming()` with no stop. The exit flag exists so the test
//! `loopback_server_stops_on_drop` compiles, and nothing ever sets it, so that test is red for
//! the missing behaviour. The developer replaces the body: a stop flag, a wake-up connect to the
//! server's own still-bound port (or a non-blocking accept), a join in `impl Drop`, and the exit
//! flag set by the accept thread as its last act. The API (`start`, `addr`, `exit_flag`) stays.
#![allow(dead_code)]

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;

/// A loopback server on `127.0.0.1:<free port>` that hands each connection to `serve` on its
/// own thread. Stopped when dropped (the guarantee under test).
pub struct LoopbackServer {
    addr: SocketAddr,
    exited: Arc<AtomicBool>,
}

impl LoopbackServer {
    pub fn start(serve: impl Fn(TcpStream) + Send + Sync + 'static) -> LoopbackServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
        let addr = listener.local_addr().expect("local addr");
        let serve = Arc::new(serve);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let serve = Arc::clone(&serve);
                thread::spawn(move || serve(stream));
            }
        });
        LoopbackServer {
            addr,
            exited: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Set to `true` by the accept thread when it has returned (its listener closed). A clone
    /// stays readable after the server is dropped.
    pub fn exit_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.exited)
    }
}
