//! The shared HTTP helpers (T-020, T-079): the one place a reqwest client is built
//! ([`client`], [`client_with_read_timeout`]), one `Authorization` rule and one
//! capped send-and-read, used by [`super::openai::OpenAiCompatibleEngine`]
//! (transcription and Test connection), [`crate::post_process::chat::ChatPostProcessor`]
//! (chat completions) and `local_models::download::Downloader` (model download).
//!
//! Every client resolves host names through [`DeadlineResolver`]: a lookup never
//! runs on the runtime's blocking pool, so a lookup with no answer is not awaited
//! when the client is dropped, and a send that fails while a lookup is unanswered
//! is a DNS failure (`TransportError::Send { dns: true, .. }`), whichever timer
//! ended it (decisions #106, #113; `docs/decisions/engine-http.md`).
//!
//! Every failure is a [`TransportError`], which `failure::classify` maps to a
//! reason; no `reqwest::Error`, URL, key or body leaves this module.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::future::Future;
use std::io::Read;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use reqwest::blocking::{Client, RequestBuilder, Response};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::HeaderValue;
use url::Url;
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

/// A blocking client built here, with the lookup record of its resolver. Every
/// reqwest client of the crate is one of these (T-079, decisions #106, #113;
/// `docs/decisions/engine-http.md`; `make check-http-client-builder`): its lookups
/// run on detached threads through [`DeadlineResolver`], so neither a send nor the
/// client's drop waits for a host-name lookup with no answer, and a send that
/// fails while a lookup is unanswered is a DNS failure.
pub(crate) struct HttpClient {
    client: Client,
    record: LookupRecord,
}

/// The client for one call, with `connect` as its connect timeout and reqwest's
/// default redirect policy. The whole-request deadline is set per request by the
/// caller (`RequestBuilder::timeout`), which bounds connect to the last body byte.
pub(crate) fn client(connect: Duration) -> Result<HttpClient, TransportError> {
    build(Client::builder().connect_timeout(connect))
}

/// The client for a download: `connect` as its connect timeout and `read` as
/// reqwest's per-read timeout (it also bounds the wait for the headers), never a
/// total one.
pub(crate) fn client_with_read_timeout(
    connect: Duration,
    read: Duration,
) -> Result<HttpClient, TransportError> {
    build(Client::builder().connect_timeout(connect).timeout(read))
}

fn build(builder: reqwest::blocking::ClientBuilder) -> Result<HttpClient, TransportError> {
    let resolver = DeadlineResolver::system();
    let record = resolver.record();
    let client = builder
        .dns_resolver(Arc::new(resolver))
        .build()
        .map_err(|_| TransportError::Setup)?;
    Ok(HttpClient { client, record })
}

impl HttpClient {
    pub(crate) fn get(&self, url: Url) -> RequestBuilder {
        self.client.get(url)
    }

    pub(crate) fn post(&self, url: Url) -> RequestBuilder {
        self.client.post(url)
    }

    /// Sends `request` (built from this client). A failure is a
    /// [`TransportError`]; it carries the DNS flag when reqwest says so or when a
    /// lookup of this client was unanswered at that moment, whichever timer ended
    /// the send.
    pub(crate) fn send(&self, request: RequestBuilder) -> Result<Response, TransportError> {
        request
            .send()
            .map_err(|e| send_error(&e, self.record.unanswered()))
    }

    /// Sends `request` and reads a 2xx body through the [`MAX_BODY`] cap. A
    /// non-2xx status is [`TransportError::Status`] and its body is never read
    /// (P-009); a body over the cap is [`TransportError::BadBody`].
    pub(crate) fn send_capped(&self, request: RequestBuilder) -> Result<Vec<u8>, TransportError> {
        let response = self.send(request)?;
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
}

/// Whether a lookup of one client is unanswered: started and not answered yet
/// (its future may still be alive: the blocking client applies the
/// whole-request deadline on the caller's thread, so the send can fail first),
/// or dropped before its answer. Shared by the resolver and its client.
#[derive(Debug, Clone, Default)]
pub(crate) struct LookupRecord(Arc<RecordInner>);

#[derive(Debug, Default)]
struct RecordInner {
    /// Lookups started and neither answered nor dropped.
    open: AtomicUsize,
    /// A lookup future was dropped before its answer.
    dropped: AtomicBool,
}

impl LookupRecord {
    /// A lookup of this client is open, or one was dropped before its answer.
    pub(crate) fn unanswered(&self) -> bool {
        self.0.open.load(Ordering::SeqCst) > 0 || self.0.dropped.load(Ordering::SeqCst)
    }
}

/// The resolver of every client built here. Each lookup runs on its own named,
/// detached thread through [`Lookup`] (the OS resolver unless a `test-fakes`
/// lookup is installed for the name) and wakes its future through a std-only
/// one-shot slot. Nothing joins the thread: it ends when the lookup returns, after
/// the client may be gone. Logs nothing (never the host name).
pub(crate) struct DeadlineResolver {
    lookup: Arc<dyn Lookup>,
    record: LookupRecord,
}

type LookupResult = std::io::Result<Vec<SocketAddr>>;

/// The one-shot between a lookup thread and its future.
struct Slot {
    answer: Option<LookupResult>,
    waker: Option<Waker>,
    /// Still counted in the record's `open`: closed by the answer or by the
    /// future's drop, whichever comes first, under this slot's lock.
    open: bool,
}

/// Closes `slot`'s count in `record` once; `unanswered` also marks the record.
fn close(slot: &mut Slot, record: &LookupRecord, unanswered: bool) {
    if !slot.open {
        return;
    }
    slot.open = false;
    if unanswered {
        record.0.dropped.store(true, Ordering::SeqCst);
    }
    record.0.open.fetch_sub(1, Ordering::SeqCst);
}

/// A lookup's future. Dropped before the answer, it marks the record (the drop
/// guard of T-079): reqwest's connect timer or the request deadline cut it.
struct Pending {
    slot: Arc<Mutex<Slot>>,
    record: LookupRecord,
}

impl Future for Pending {
    type Output = Result<Addrs, Box<dyn std::error::Error + Send + Sync>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
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

impl Drop for Pending {
    fn drop(&mut self) {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        close(&mut slot, &self.record, true);
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
        }
    }

    /// This resolver's lookup record.
    pub(crate) fn record(&self) -> LookupRecord {
        self.record.clone()
    }
}

impl Resolve for DeadlineResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        let lookup = lookup::installed_for(&host).unwrap_or_else(|| Arc::clone(&self.lookup));
        // Counted open before the thread starts, so a send failing at any later
        // moment sees it until the answer or the drop closes it.
        self.record.0.open.fetch_add(1, Ordering::SeqCst);
        let slot = Arc::new(Mutex::new(Slot {
            answer: None,
            waker: None,
            open: true,
        }));
        let filled = Arc::clone(&slot);
        let record = self.record.clone();
        let spawned = std::thread::Builder::new()
            .name("voicen-lookup".to_string())
            .spawn(move || {
                let answer = lookup.lookup(&host);
                let mut slot = filled.lock().unwrap_or_else(PoisonError::into_inner);
                slot.answer = Some(answer);
                close(&mut slot, &record, false);
                if let Some(waker) = slot.waker.take() {
                    waker.wake();
                }
            });
        let pending = Pending {
            slot,
            record: self.record.clone(),
        };
        match spawned {
            // Detached: the handle is dropped, nothing waits for the thread.
            Ok(_detached) => Box::pin(pending),
            Err(e) => {
                // No lookup ran: an answer (the spawn error), not an unanswered one.
                let mut slot = pending.slot.lock().unwrap_or_else(PoisonError::into_inner);
                close(&mut slot, &pending.record, false);
                drop(slot);
                Box::pin(std::future::ready(Err(
                    Box::new(e) as Box<dyn std::error::Error + Send + Sync>
                )))
            }
        }
    }
}

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
