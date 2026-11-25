//! T-175: the one loopback accept loop that `dictation_e2e.rs::ChatMock` and
//! `call_chain.rs::Mock` share (each keeps its own answer function). Windows CI only.
//!
//! The server stops when its owner drops it: `Drop` sets the stop flag, wakes the accept that
//! blocks in `listener.incoming()` with one connect to the server's own port (still bound, the
//! listener lives in the accept thread until it returns), and joins the accept thread. The
//! accept thread drops its listener and sets the exit flag as its last act, so a set flag means
//! the loop has returned. Connections already handed to `serve` run on their own threads and
//! are not joined (each ends with its connection). Pinned by
//! `dictation_e2e.rs::loopback_server_stops_on_drop`.
#![allow(dead_code)]

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crate::timing_support::HANG;

/// A loopback server on `127.0.0.1:<free port>` that hands each connection to `serve` on its
/// own thread. Stopped when dropped.
pub struct LoopbackServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    exited: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl LoopbackServer {
    pub fn start(serve: impl Fn(TcpStream) + Send + Sync + 'static) -> LoopbackServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
        let addr = listener.local_addr().expect("local addr");
        let serve = Arc::new(serve);
        let stop = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));
        let accept = {
            let stop = Arc::clone(&stop);
            let exited = Arc::clone(&exited);
            thread::spawn(move || {
                for stream in listener.incoming() {
                    // Checked after every accept: the wake-up connect of `Drop` lands here.
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let serve = Arc::clone(&serve);
                    thread::spawn(move || serve(stream));
                }
                drop(listener);
                exited.store(true, Ordering::SeqCst);
            })
        };
        LoopbackServer {
            addr,
            stop,
            exited,
            accept: Some(accept),
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

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // The wake-up: the listener is still bound (the accept thread owns it until it
        // returns), so this connect is accepted and the loop sees the stop flag. When it
        // fails, the accept is not woken and a join would block for good: the thread is
        // left behind and its exit flag stays false (a test of the stop sees that).
        if TcpStream::connect_timeout(&self.addr, HANG).is_err() {
            return;
        }
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}
