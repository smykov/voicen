//! The shared OpenAI-compatible HTTP helpers (T-020): one client setup, one
//! `Authorization` rule and one capped send-and-read, used by both
//! [`super::openai::OpenAiCompatibleEngine`] (transcription) and
//! [`crate::post_process::chat::ChatPostProcessor`] (chat completions).
//!
//! Every failure is a [`TransportError`], which `failure::classify` maps to a
//! reason; no `reqwest::Error`, URL, key or body leaves this module.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::future::Future;
use std::io::Read;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};
use std::thread::JoinHandle;
use std::time::Duration;

use reqwest::blocking::{Client, RequestBuilder};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::HeaderValue;
use zeroize::Zeroizing;

use super::lookup::{self, Lookup, SystemLookup};
use super::openai::{body_error, send_error};
use crate::failure::TransportError;
use crate::secrets::Secret;

/// Largest accepted response body (contract: "body > 1 MiB" -> `UnexpectedResponse`).
pub(crate) const MAX_BODY: u64 = 1024 * 1024;

/// The `Authorization` value for `key`: `None` without a key (or with an empty
/// one). The one rule for a usable key: `Bearer <key>` passes `HeaderValue`
/// validation (http 1.5: no control byte other than tab, no DEL); otherwise
/// [`TransportError::UnusableKey`]. Bytes of a non-ASCII key pass that rule and
/// are sent as UTF-8; the server's 401/403 then gives `InvalidApiKey`. The value
/// is marked sensitive (not printed by `Debug`, not HPACK-indexed, and dropped by
/// reqwest's redirect policy on a cross-origin redirect).
pub(crate) fn authorization(key: Option<&Secret>) -> Result<Option<HeaderValue>, TransportError> {
    let Some(key) = key.filter(|k| !k.expose().is_empty()) else {
        return Ok(None);
    };
    let text = Zeroizing::new(format!("Bearer {}", key.expose()));
    let mut value = HeaderValue::from_str(&text).map_err(|_| TransportError::UnusableKey)?;
    value.set_sensitive(true);
    Ok(Some(value))
}

/// The blocking client for one call, with `connect` as its connect timeout and
/// reqwest's default redirect policy. The whole-request deadline is set per
/// request by the caller (`RequestBuilder::timeout`), which bounds connect to the
/// last body byte.
pub(crate) fn client(connect: Duration) -> Result<Client, TransportError> {
    Client::builder()
        .connect_timeout(connect)
        .dns_resolver(Arc::new(DeadlineResolver::system()))
        .build()
        .map_err(|_| TransportError::Setup)
}

/// Sends `request` and reads a 2xx body through the [`MAX_BODY`] cap. A non-2xx
/// status is [`TransportError::Status`] and its body is never read (P-009); a body
/// over the cap is [`TransportError::BadBody`].
pub(crate) fn send_capped(request: RequestBuilder) -> Result<Vec<u8>, TransportError> {
    let response = request.send().map_err(|e| send_error(&e))?;
    let status = response.status();
    if !status.is_success() {
        return Err(TransportError::Status(status.as_u16()));
    }
    let mut body = Vec::new();
    response
        .take(MAX_BODY + 1)
        .read_to_end(&mut body)
        .map_err(|e| body_error(&e))?;
    if !u64::try_from(body.len()).is_ok_and(|len| len <= MAX_BODY) {
        return Err(TransportError::BadBody);
    }
    Ok(body)
}

// ---- T-079 red seam ------------------------------------------------------------
// The lookup seam with TODAY's behaviour, written by the test writer so the T-079
// tests compile and go red on behaviour (T-079 analysis "Write the seam first with
// today's behaviour"). It does what reqwest's default `GaiResolver` does: each
// lookup runs on its own thread, and every lookup thread is joined when the
// resolver (that is, the client and its runtime) is dropped, as tokio's blocking
// pool is at runtime drop. The record is never set. The developer replaces this
// with the real `DeadlineResolver` (detached thread, drop guard, record read by
// `send_error`).

/// Whether a lookup of the client is unanswered: started and not answered yet
/// (its future may still be alive: the blocking caller's own deadline can end the
/// send first), or dropped before its answer.
// T-079 red seam: read by `send_error` once the developer wires it.
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub(crate) struct LookupRecord(Arc<AtomicBool>);

#[allow(dead_code)]
impl LookupRecord {
    /// A lookup future of this client was dropped before its answer.
    pub(crate) fn unanswered(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// The resolver of every client built here.
#[allow(dead_code)]
pub(crate) struct DeadlineResolver {
    lookup: Arc<dyn Lookup>,
    record: LookupRecord,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

type LookupResult = std::io::Result<Vec<SocketAddr>>;

#[derive(Default)]
struct Slot {
    answer: Option<LookupResult>,
    waker: Option<Waker>,
}

struct Pending(Arc<Mutex<Slot>>);

impl Future for Pending {
    type Output = Result<Addrs, Box<dyn std::error::Error + Send + Sync>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match slot.answer.take() {
            Some(Ok(addrs)) => Poll::Ready(Ok(Box::new(addrs.into_iter()) as Addrs)),
            Some(Err(e)) => Poll::Ready(Err(Box::new(e))),
            None => {
                slot.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

impl DeadlineResolver {
    /// The resolver over the OS lookup.
    pub(crate) fn system() -> DeadlineResolver {
        DeadlineResolver::new(Arc::new(SystemLookup))
    }

    pub(crate) fn new(lookup: Arc<dyn Lookup>) -> DeadlineResolver {
        DeadlineResolver {
            lookup,
            record: LookupRecord::default(),
            threads: Mutex::new(Vec::new()),
        }
    }

    /// This resolver's lookup record.
    #[allow(dead_code)]
    pub(crate) fn record(&self) -> LookupRecord {
        self.record.clone()
    }
}

impl Resolve for DeadlineResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        let lookup = lookup::installed_for(&host).unwrap_or_else(|| Arc::clone(&self.lookup));
        let slot = Arc::new(Mutex::new(Slot::default()));
        let filled = Arc::clone(&slot);
        let spawned = std::thread::Builder::new()
            .name("voicen-lookup".to_string())
            .spawn(move || {
                let answer = lookup.lookup(&host);
                let mut slot = filled.lock().unwrap_or_else(PoisonError::into_inner);
                slot.answer = Some(answer);
                if let Some(waker) = slot.waker.take() {
                    waker.wake();
                }
            });
        match spawned {
            Ok(handle) => {
                self.threads
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(handle);
                Box::pin(Pending(slot))
            }
            Err(e) => Box::pin(std::future::ready(Err(
                Box::new(e) as Box<dyn std::error::Error + Send + Sync>
            ))),
        }
    }
}

impl Drop for DeadlineResolver {
    fn drop(&mut self) {
        let threads =
            std::mem::take(&mut *self.threads.lock().unwrap_or_else(PoisonError::into_inner));
        for handle in threads {
            let _ = handle.join();
        }
    }
}
// ---- end of the T-079 red seam ---------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_or_absent_key_is_no_header() {
        // The key rule with no engine around it (the chat path calls it directly).
        // Bite: `Bearer ` sent for an empty key.
        assert!(matches!(authorization(None), Ok(None)));
        let empty = Secret::new("");
        assert!(matches!(authorization(Some(&empty)), Ok(None)));
    }

    // ---- T-079: the lookup record (drop guard) -----------------------------------

    use crate::engine::lookup::{FixedLookup, HeldLookup};
    use std::task::Wake;

    /// Wakes the polling thread.
    struct Unpark(std::thread::Thread);

    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    /// Polls `fut` on this thread until it is ready.
    fn block_on<F: Future>(fut: F) -> F::Output {
        let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        let mut fut = std::pin::pin!(fut);
        loop {
            if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
                return out;
            }
            std::thread::park();
        }
    }

    fn name(host: &str) -> Name {
        match host.parse() {
            Ok(name) => name,
            Err(_) => panic!("test host {host:?} is not a valid name"),
        }
    }

    /// Releases the held lookup when the test ends, also on a failed assertion,
    /// so the resolver's drop never waits for the fake's backstop.
    struct ReleaseOnDrop(HeldLookup);

    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.0.release();
        }
    }

    #[test]
    fn a_lookup_future_dropped_before_its_answer_is_recorded_unanswered() {
        // T-079 invariant: a pending lookup dropped by a timer (reqwest's connect
        // timer or the whole-request deadline) is a DNS failure, recorded by the
        // resolver, not won by a timer race. Bite: no drop guard (record stays
        // false); the record set only on the resolver's own deadline.
        let held = HeldLookup::answering(Vec::new(), Duration::from_secs(10));
        let resolver = DeadlineResolver::new(Arc::new(held.clone()));
        let _release = ReleaseOnDrop(held.clone());
        let record = resolver.record();
        assert!(
            !record.unanswered(),
            "a new client has no unanswered lookup"
        );
        let mut fut = resolver.resolve(name("unit-held.t079.example.com"));
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        assert!(
            fut.as_mut().poll(&mut cx).is_pending(),
            "a held lookup has no answer"
        );
        drop(fut);
        assert!(
            record.unanswered(),
            "a lookup future dropped before its answer must set the record"
        );
        assert_eq!(
            held.answered(),
            0,
            "the record is set at the drop, not when the lookup ends"
        );
    }

    #[test]
    fn a_lookup_still_pending_is_recorded_unanswered_before_any_drop() {
        // The blocking client also applies the whole-request deadline on the
        // caller's thread (reqwest `blocking::wait::timeout`): `send_error` can run
        // while the runtime thread still holds the lookup future, before any drop.
        // Decision #113 needs "unanswered when the send failed", so a lookup that
        // started and has not answered counts. The bite check saw a drop-only
        // record turn the post-processing deadline twin
        // (`unanswered_pp_lookup_cut_by_the_post_processing_deadline_...`) into
        // Timeout. Bite: the record set only at the future's drop.
        let held = HeldLookup::answering(Vec::new(), Duration::from_secs(10));
        let resolver = DeadlineResolver::new(Arc::new(held.clone()));
        let _release = ReleaseOnDrop(held.clone());
        let record = resolver.record();
        let mut fut = resolver.resolve(name("unit-pending.t079.example.com"));
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        assert!(
            fut.as_mut().poll(&mut cx).is_pending(),
            "a held lookup has no answer"
        );
        assert!(
            record.unanswered(),
            "a lookup started and not answered is unanswered while its future lives"
        );
        drop(fut);
    }

    #[test]
    fn a_lookup_future_never_polled_and_dropped_is_recorded_unanswered() {
        // The timer can fire before the connector polls the lookup at all (a
        // deadline already past, a starved runtime). Bite: the guard armed only on
        // the first poll.
        let held = HeldLookup::answering(Vec::new(), Duration::from_secs(10));
        let resolver = DeadlineResolver::new(Arc::new(held.clone()));
        let _release = ReleaseOnDrop(held.clone());
        let record = resolver.record();
        drop(resolver.resolve(name("unit-unpolled.t079.example.com")));
        assert!(record.unanswered());
    }

    #[test]
    fn an_answered_lookup_dropped_afterwards_leaves_the_record_false() {
        // Pins the negative (green today): an answer, or the resolver's own error,
        // is never an unanswered lookup, so a refused or blackholed address behind
        // a name stays CannotReach. Bite: the record set on every drop.
        let addr = SocketAddr::from(([127, 0, 0, 1], 0));
        let resolver = DeadlineResolver::new(Arc::new(FixedLookup::answer(vec![addr])));
        let record = resolver.record();
        let got = block_on(resolver.resolve(name("unit-answered.t079.example.com")));
        let first = got.ok().and_then(|mut addrs| addrs.next());
        assert_eq!(
            first,
            Some(addr),
            "the lookup's answer is the resolver's answer"
        );
        assert!(!record.unanswered(), "an answered lookup is not unanswered");

        let resolver =
            DeadlineResolver::new(Arc::new(FixedLookup::error(std::io::ErrorKind::Other)));
        let record = resolver.record();
        let got = block_on(resolver.resolve(name("unit-failed.t079.example.com")));
        assert!(got.is_err(), "the lookup's error is the resolver's error");
        assert!(!record.unanswered(), "a failed lookup answered");
    }
}
