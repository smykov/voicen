//! T-054: the one-time "logs cannot be written" notice latch
//! (`voicen_core::diag::LogsNotice`) and the reason -> `logs_reason.*` map, on the
//! Linux gate (analysis approach A, red tests 1 and 4).
//!
//! The latch sits between the log's `on_unwritable` and the dictation session: the
//! log may fail before the session exists (inside `Log::open`, at start), after it
//! attached, or while it attaches; and the failing write may run on a thread that
//! holds the session lock. Invariant (T-054 analysis): each `on_unwritable` call
//! ends in exactly one notice through the attached sink, at most one per latch,
//! whatever the order; the callback takes only the latch's own leaf lock and never
//! calls the sink inline; a delivery after the attach runs on another thread; a
//! delivery at the attach runs on the attaching thread before `attach` returns (so
//! the shell's startup `notice.hotkey_unavailable`, raised after the attach, wins
//! the one message slot).
//!
//! API pinned here (developer follows it):
//! - `diag::LogsNotice::new()`, `callback(&self) -> OnUnwritable` (called on an
//!   `Arc<LogsNotice>`; any receiver that autoderefs from it works),
//!   `attach(&self, sink: Arc<dyn LogsNoticeSink>)`;
//! - `diag::LogsNoticeSink: Send + Sync` with `fn logs_unwritable(&self, reason:
//!   LogsUnwritableReason)` (the dictation session implements it,
//!   `tests/dictation_session.rs`);
//! - `LogsUnwritableReason::message_id(self) -> i18n::MessageId`, a nested
//!   `logs_reason.*` id (precedent `MicCause::message_id`).
//!
//! Everything that waits on another thread is bounded by `BUDGET`; a hung latch
//! fails the test instead of hanging it.

mod common;

use std::sync::mpsc;
use std::sync::{Arc, Barrier, Mutex, MutexGuard, PoisonError};
use std::thread::{self, ThreadId};
use std::time::Duration;

use voicen_core::diag::{LogsNotice, LogsNoticeSink, LogsUnwritableReason, OnUnwritable};
use voicen_core::i18n::{self, MessageId};

/// Upper bound on anything that waits for another thread.
const BUDGET: Duration = Duration::from_secs(10);
/// How long a test keeps watching for a second (wrong) delivery after the first.
const SETTLE: Duration = Duration::from_millis(300);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn eventually(cond: impl FnMut() -> bool) -> bool {
    common::timing::eventually(BUDGET, cond)
}

/// What a sink received: each reason with the thread it was delivered on.
#[derive(Default)]
struct Seen {
    calls: Mutex<Vec<(LogsUnwritableReason, ThreadId)>>,
}

impl Seen {
    fn reasons(&self) -> Vec<LogsUnwritableReason> {
        lock(&self.calls).iter().map(|(r, _)| *r).collect()
    }
    fn threads(&self) -> Vec<ThreadId> {
        lock(&self.calls).iter().map(|(_, t)| *t).collect()
    }
    fn count(&self) -> usize {
        lock(&self.calls).len()
    }
}

impl LogsNoticeSink for Seen {
    fn logs_unwritable(&self, reason: LogsUnwritableReason) {
        lock(&self.calls).push((reason, thread::current().id()));
    }
}

fn sink(seen: &Arc<Seen>) -> Arc<dyn LogsNoticeSink> {
    Arc::clone(seen) as Arc<dyn LogsNoticeSink>
}

// ---- orders -----------------------------------------------------------------------

#[test]
fn a_failure_before_the_attach_is_delivered_once_by_the_attach_itself() {
    // Order 1 (the start: `Log::open` fails inside `release_parts`, before any
    // session exists). The reason is latched and delivered when the sink attaches,
    // on the attaching thread, before `attach` returns; a second call of the
    // callback (a second failure the log would never report, or a second log) adds
    // nothing. Bite: a latch that drops a call made before the attach (today's
    // `|_| {}`), one that delivers on a spawned thread at the attach (the hotkey
    // notice could then be overwritten), one that delivers every call, the reason
    // replaced or defaulted.
    let notice = Arc::new(LogsNotice::new());
    let callback = notice.callback();
    callback(LogsUnwritableReason::PermissionDenied);
    callback(LogsUnwritableReason::DiskFull);

    let seen = Arc::new(Seen::default());
    notice.attach(sink(&seen));
    assert_eq!(
        seen.reasons(),
        vec![LogsUnwritableReason::PermissionDenied],
        "delivered by the attach, before it returned, with the first reason"
    );
    assert_eq!(
        seen.threads(),
        vec![thread::current().id()],
        "a delivery at the attach runs on the attaching thread"
    );

    callback(LogsUnwritableReason::NotADirectory);
    thread::sleep(SETTLE);
    assert_eq!(
        seen.count(),
        1,
        "at most one notice per latch: {:?}",
        seen.reasons()
    );
}

#[test]
fn a_failure_after_the_attach_is_delivered_once_off_the_callback_thread() {
    // Order 2 (a write fails while the session runs). Nothing is delivered by an
    // attach without a failure; the first call is delivered exactly once, on a
    // thread other than the caller's (the caller may hold the session lock); later
    // calls add nothing. Bite: an inline delivery (same thread id), a delivery at
    // an attach with no failure, a notice per call.
    let notice = Arc::new(LogsNotice::new());
    let callback = notice.callback();
    let seen = Arc::new(Seen::default());
    notice.attach(sink(&seen));
    thread::sleep(SETTLE);
    assert_eq!(
        seen.count(),
        0,
        "no failure, no notice: {:?}",
        seen.reasons()
    );

    callback(LogsUnwritableReason::DiskFull);
    assert!(
        eventually(|| seen.count() >= 1),
        "the failure after the attach was not delivered within {BUDGET:?}"
    );
    callback(LogsUnwritableReason::Other(Some(32)));
    thread::sleep(SETTLE);
    assert_eq!(seen.reasons(), vec![LogsUnwritableReason::DiskFull]);
    assert_ne!(
        seen.threads(),
        vec![thread::current().id()],
        "delivered on the thread that called the callback"
    );
}

#[test]
fn the_callback_never_calls_the_sink_inline_under_the_callers_lock() {
    // The lock re-entry of the analysis (root cause (2)): the session publishes to
    // the indicator under its own mutex, and the tray's render may write a log line
    // there; the sink then takes that same mutex. Here the caller holds `session`
    // while it calls the callback, and the sink locks `session`. An inline delivery
    // never returns (std Mutex is not re-entrant), so the call runs on a helper
    // thread bounded by BUDGET. Bite: the sink called from inside the callback.
    let session = Arc::new(Mutex::new(()));
    struct Locking {
        session: Arc<Mutex<()>>,
        seen: Arc<Seen>,
    }
    impl LogsNoticeSink for Locking {
        fn logs_unwritable(&self, reason: LogsUnwritableReason) {
            let _held = lock(&self.session);
            self.seen.logs_unwritable(reason);
        }
    }
    let seen = Arc::new(Seen::default());
    let notice = Arc::new(LogsNotice::new());
    notice.attach(Arc::new(Locking {
        session: Arc::clone(&session),
        seen: Arc::clone(&seen),
    }));
    let callback = notice.callback();

    let (done_tx, done_rx) = mpsc::channel();
    let caller_lock = Arc::clone(&session);
    let caller = thread::spawn(move || {
        let _held = lock(&caller_lock);
        callback(LogsUnwritableReason::PermissionDenied);
        let _ = done_tx.send(thread::current().id());
    });
    let Ok(caller_id) = done_rx.recv_timeout(BUDGET) else {
        // The helper holds `session` forever; leave it, do not join it.
        drop(caller);
        panic!("the callback did not return within {BUDGET:?}: the sink was called inline under the caller's lock");
    };
    let _ = caller.join();
    assert!(
        eventually(|| seen.count() == 1),
        "not delivered once the caller released its lock: {:?}",
        seen.reasons()
    );
    assert_eq!(seen.reasons(), vec![LogsUnwritableReason::PermissionDenied]);
    assert_ne!(
        seen.threads(),
        vec![caller_id],
        "delivered on the caller's thread"
    );
}

#[test]
fn callbacks_and_the_attach_racing_on_eight_threads_deliver_exactly_once() {
    // Order 3 (concurrently with the attach): 7 threads call the callback (one
    // shared `OnUnwritable`, as the log's writers would) and 1 attaches, all
    // released by one barrier; 200 rounds, each a new latch. Every round delivers
    // exactly one notice, with one of the reasons the callers passed. Bite: a
    // check-then-act latch (state read, lock released, delivered, then marked), a
    // callback that only records when no sink is attached yet (lost when the
    // attach wins between the check and the store), a delivery per call.
    const ROUNDS: usize = 200;
    const CALLERS: usize = 7;
    let reasons = [
        LogsUnwritableReason::PermissionDenied,
        LogsUnwritableReason::DiskFull,
        LogsUnwritableReason::NotADirectory,
        LogsUnwritableReason::Other(Some(32)),
        LogsUnwritableReason::Other(None),
        LogsUnwritableReason::Other(Some(5)),
        LogsUnwritableReason::PermissionDenied,
    ];
    let mut all = Vec::with_capacity(ROUNDS);
    for round in 0..ROUNDS {
        let notice = Arc::new(LogsNotice::new());
        let callback: Arc<OnUnwritable> = Arc::new(notice.callback());
        let seen = Arc::new(Seen::default());
        let start = Arc::new(Barrier::new(CALLERS + 1));
        let mut threads = Vec::with_capacity(CALLERS + 1);
        for reason in reasons.iter().copied().take(CALLERS) {
            let callback = Arc::clone(&callback);
            let start = Arc::clone(&start);
            threads.push(thread::spawn(move || {
                start.wait();
                callback(reason);
            }));
        }
        {
            let notice = Arc::clone(&notice);
            let seen = Arc::clone(&seen);
            let start = Arc::clone(&start);
            threads.push(thread::spawn(move || {
                start.wait();
                notice.attach(sink(&seen));
            }));
        }
        for t in threads {
            t.join().expect("a racing thread panicked");
        }
        assert!(
            eventually(|| seen.count() >= 1),
            "round {round}: no notice within {BUDGET:?}"
        );
        all.push((notice, callback, seen));
    }
    thread::sleep(SETTLE);
    let wrong: Vec<(usize, Vec<LogsUnwritableReason>)> = all
        .iter()
        .enumerate()
        .filter(|(_, (_, _, seen))| seen.count() != 1)
        .map(|(round, (_, _, seen))| (round, seen.reasons()))
        .collect();
    assert!(
        wrong.is_empty(),
        "rounds without exactly one notice: {wrong:?}"
    );
    for (round, (_, _, seen)) in all.iter().enumerate() {
        let got = seen.reasons()[0];
        assert!(
            reasons.contains(&got),
            "round {round}: a reason no caller passed: {got:?}"
        );
    }
}

// ---- reason -> logs_reason.* --------------------------------------------------------

/// The catalog id of a `MessageId` (its serialized form, contracts/ipc.md).
fn catalog_id(id: MessageId) -> String {
    serde_json::to_value(id)
        .expect("a MessageId serializes")
        .as_str()
        .expect("a JSON string")
        .to_string()
}

/// The `logs_reason.*` id each reason must map to, spelled here (contracts:
/// specs/006 contracts/ipc.md, OQ-35 (1)). Exhaustive without a wildcard: a new
/// reason does not compile here until it has a row.
fn expected_id(reason: LogsUnwritableReason) -> &'static str {
    match reason {
        LogsUnwritableReason::PermissionDenied => "logs_reason.permission_denied",
        LogsUnwritableReason::DiskFull => "logs_reason.disk_full",
        LogsUnwritableReason::NotADirectory => "logs_reason.not_a_directory",
        LogsUnwritableReason::Other(_) => "logs_reason.other",
    }
}

#[test]
fn every_unwritable_reason_maps_to_its_declared_logs_reason_id() {
    // T-054 seam: `LogsUnwritableReason` has a total map to the nested
    // `logs_reason.*` ids, declared with `messages!` (so the catalog tests cover
    // them); the OS code of `Other` never reaches the text (OQ-35 (1): "no OS code
    // shown"), so every `Other` is `logs_reason.other`. Bite: a reason mapped to
    // another's id, `Other(Some(32))` given its own id or the code, an id not in
    // MESSAGE_IDS (the overlay would show the raw id).
    let reasons = [
        LogsUnwritableReason::PermissionDenied,
        LogsUnwritableReason::DiskFull,
        LogsUnwritableReason::NotADirectory,
        LogsUnwritableReason::Other(None),
        LogsUnwritableReason::Other(Some(32)),
        LogsUnwritableReason::Other(Some(5)),
        LogsUnwritableReason::Other(Some(-1)),
    ];
    let mut wrong = Vec::new();
    for reason in reasons {
        let id = reason.message_id();
        if catalog_id(id) != expected_id(reason) {
            wrong.push(format!(
                "{reason:?} -> {}, expected {}",
                catalog_id(id),
                expected_id(reason)
            ));
        }
        if !i18n::MESSAGE_IDS.contains(&id) {
            wrong.push(format!(
                "{reason:?} -> {}: not in MESSAGE_IDS",
                catalog_id(id)
            ));
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}
