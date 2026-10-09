//! The host-name lookup behind every HTTP client of the crate (T-079, decisions
//! #106, #113): [`Lookup`] is the one place a name becomes addresses;
//! [`SystemLookup`] (the OS resolver: getaddrinfo / GetAddrInfoW through std) is
//! the production one. `engine::http`'s resolver calls it; an IP-literal host
//! never reaches it (hyper-util skips the resolver for literals).
//!
//! Under `test-fakes` only: [`install`] puts a fake lookup in front of the system
//! one for one host name (exact, lowercase), so a test reaches every client path
//! (engine, post-processing, download, connection test, pipeline) without new
//! constructor parameters. The fakes are [`HeldLookup`] (no answer until the test
//! releases it) and [`FixedLookup`] (an answer or an error at once). They log
//! nothing.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::io;
use std::net::{SocketAddr, ToSocketAddrs};

/// Resolves a host name to addresses (blocking). Port `0` in an answer means "the
/// URL's port". Never logs the name.
pub trait Lookup: Send + Sync {
    fn lookup(&self, host: &str) -> io::Result<Vec<SocketAddr>>;
}

/// The OS resolver (std `ToSocketAddrs`, the same call hyper's `GaiResolver` makes).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemLookup;

impl Lookup for SystemLookup {
    fn lookup(&self, host: &str) -> io::Result<Vec<SocketAddr>> {
        (host, 0).to_socket_addrs().map(Iterator::collect)
    }
}

#[cfg(any(test, feature = "test-fakes"))]
pub use fakes::{install, FixedLookup, HeldLookup, Installed};

/// The fake installed for `host`, if any (always `None` without `test-fakes`).
#[cfg(any(test, feature = "test-fakes"))]
pub(crate) fn installed_for(host: &str) -> Option<std::sync::Arc<dyn Lookup>> {
    fakes::installed_for(host)
}

/// The fake installed for `host`, if any (always `None` without `test-fakes`).
#[cfg(not(any(test, feature = "test-fakes")))]
pub(crate) fn installed_for(_host: &str) -> Option<std::sync::Arc<dyn Lookup>> {
    None
}

#[cfg(any(test, feature = "test-fakes"))]
mod fakes {
    use std::io;
    use std::net::SocketAddr;
    use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
    use std::time::{Duration, Instant};

    use super::Lookup;

    type Registry = Mutex<Vec<(String, Arc<dyn Lookup>)>>;

    static INSTALLED: Registry = Mutex::new(Vec::new());

    fn registry() -> MutexGuard<'static, Vec<(String, Arc<dyn Lookup>)>> {
        INSTALLED.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn installed_for(host: &str) -> Option<Arc<dyn Lookup>> {
        registry()
            .iter()
            .rev()
            .find(|(name, _)| name == host)
            .map(|(_, lookup)| Arc::clone(lookup))
    }

    /// Puts `lookup` in front of the system lookup for `host` (a lowercase name, as
    /// a URL carries it) until the returned guard is dropped. Use a name unique to
    /// the test: the registry is process-wide.
    pub fn install(host: &str, lookup: Arc<dyn Lookup>) -> Installed {
        registry().push((host.to_string(), Arc::clone(&lookup)));
        Installed { lookup }
    }

    /// Removes its fake from the registry when dropped.
    #[must_use = "the fake is removed when the guard is dropped"]
    pub struct Installed {
        lookup: Arc<dyn Lookup>,
    }

    impl Drop for Installed {
        fn drop(&mut self) {
            registry().retain(|(_, l)| !Arc::ptr_eq(l, &self.lookup));
        }
    }

    #[derive(Debug, Default)]
    struct HeldState {
        asked: usize,
        answered: usize,
        released: bool,
    }

    #[derive(Debug)]
    struct HeldInner {
        state: Mutex<HeldState>,
        changed: Condvar,
        answer: Vec<SocketAddr>,
        backstop: Duration,
    }

    /// A lookup that gives no answer until [`HeldLookup::release`] (then `answer`),
    /// or until `backstop` passes (then a `TimedOut` error; the backstop only ends a
    /// hung test). Counts the lookups asked and answered, so a test can show that a
    /// call returned while the lookup was still held. Clones share the state.
    #[derive(Debug, Clone)]
    pub struct HeldLookup {
        inner: Arc<HeldInner>,
    }

    impl HeldLookup {
        pub fn answering(answer: Vec<SocketAddr>, backstop: Duration) -> HeldLookup {
            HeldLookup {
                inner: Arc::new(HeldInner {
                    state: Mutex::new(HeldState::default()),
                    changed: Condvar::new(),
                    answer,
                    backstop,
                }),
            }
        }

        fn state(&self) -> MutexGuard<'_, HeldState> {
            self.inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
        }

        /// Lets every held and later lookup answer. Returns how many lookups were
        /// still held at this moment.
        pub fn release(&self) -> usize {
            let mut state = self.state();
            state.released = true;
            let held = state.asked.saturating_sub(state.answered);
            self.inner.changed.notify_all();
            held
        }

        /// Lookups started so far.
        pub fn asked(&self) -> usize {
            self.state().asked
        }

        /// Lookups that returned so far (released or past the backstop).
        pub fn answered(&self) -> usize {
            self.state().answered
        }
    }

    impl Lookup for HeldLookup {
        fn lookup(&self, _host: &str) -> io::Result<Vec<SocketAddr>> {
            let end = Instant::now() + self.inner.backstop;
            let mut state = self.state();
            state.asked += 1;
            self.inner.changed.notify_all();
            while !state.released {
                let left = end.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                state = self
                    .inner
                    .changed
                    .wait_timeout(state, left)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            state.answered += 1;
            if state.released {
                Ok(self.inner.answer.clone())
            } else {
                Err(io::Error::from(io::ErrorKind::TimedOut))
            }
        }
    }

    /// A lookup that answers at once: the addresses, or an error of `kind`.
    #[derive(Debug)]
    pub struct FixedLookup {
        answer: Result<Vec<SocketAddr>, io::ErrorKind>,
        asked: Mutex<usize>,
    }

    impl FixedLookup {
        pub fn answer(addrs: Vec<SocketAddr>) -> FixedLookup {
            FixedLookup {
                answer: Ok(addrs),
                asked: Mutex::new(0),
            }
        }

        pub fn error(kind: io::ErrorKind) -> FixedLookup {
            FixedLookup {
                answer: Err(kind),
                asked: Mutex::new(0),
            }
        }

        /// Lookups started so far.
        pub fn asked(&self) -> usize {
            *self.asked.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    impl Lookup for FixedLookup {
        fn lookup(&self, _host: &str) -> io::Result<Vec<SocketAddr>> {
            *self.asked.lock().unwrap_or_else(PoisonError::into_inner) += 1;
            self.answer.clone().map_err(io::Error::from)
        }
    }
}
