//! T-079 (decisions #106, #113): a lookup that gets no answer, without the host
//! resolver and without a wall-clock ceiling (T-080 I1, I2).
//!
//! [`Held`] installs a `voicen_core::engine::lookup::HeldLookup` for one test-only
//! host name (a subdomain of `example.com`, unique per test): every client path
//! that resolves that name gets no answer until the test releases it.
//! [`returns_while_held`] runs the call and decides by ordering, not by a clock:
//! the call must return while its lookup is still held. A client that waits for
//! the lookup (today: the runtime's blocking pool is joined at client drop) does
//! not return, and the test ends red after [`HELD_WAIT`], which only ends a hung
//! test.

use std::net::SocketAddr;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use voicen_core::engine::lookup::{install, HeldLookup, Installed};

use super::os_answer::OS_ANSWER_BUDGET;
use super::timing::{measure, Took};

/// How long a test waits for a call that must return while its lookup is held.
/// Only ends a hung (red) test: far above every deadline these tests set.
pub const HELD_WAIT: Duration = OS_ANSWER_BUDGET;

/// The fake's own backstop, past [`HELD_WAIT`], so the test, not the fake, ends a
/// wait.
pub const HELD_BACKSTOP: Duration = Duration::from_secs(2 * OS_ANSWER_BUDGET.as_secs());

/// A held lookup installed for `host`. Released when dropped, so a failing test
/// never leaves a client thread waiting for it.
pub struct Held {
    pub host: String,
    pub lookup: HeldLookup,
    _installed: Installed,
}

impl Held {
    /// Holds every lookup of `host` (lowercase); `answer` is what a released
    /// lookup returns.
    pub fn install(host: &str, answer: Vec<SocketAddr>) -> Held {
        let lookup = HeldLookup::answering(answer, HELD_BACKSTOP);
        let installed = install(host, Arc::new(lookup.clone()));
        Held {
            host: host.to_string(),
            lookup,
            _installed: installed,
        }
    }

    /// The lookup was asked at least once and has not answered yet.
    pub fn assert_still_held(&self, label: &str) {
        assert!(
            self.lookup.asked() >= 1,
            "{label}: the lookup of {} was never asked: the client did not go through \
             the lookup seam (engine::lookup), so this test proves nothing",
            self.host
        );
        assert_eq!(
            self.lookup.answered(),
            0,
            "{label}: the lookup of {} had already answered when the call returned",
            self.host
        );
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.lookup.release();
    }
}

/// Runs `call` on its own thread and returns its result and duration, after
/// checking that it returned while `held`'s lookup was still unanswered. When the
/// call does not return within [`HELD_WAIT`], the lookup is released (so the call
/// can end) and the test fails: the call waited for the lookup.
pub fn returns_while_held<T: Send>(held: &Held, call: impl FnOnce() -> T + Send) -> (T, Took) {
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|s| {
        s.spawn(move || {
            let _ = tx.send(measure(call));
        });
        match rx.recv_timeout(HELD_WAIT) {
            Ok(got) => {
                held.assert_still_held("returns_while_held");
                held.lookup.release();
                got
            }
            Err(_) => {
                held.lookup.release();
                panic!(
                    "the call did not return within {HELD_WAIT:?} while the lookup of {} \
                     was held: it waits for the OS resolver instead of ending at its \
                     deadline (T-079)",
                    held.host
                );
            }
        }
    })
}
