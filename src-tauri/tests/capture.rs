//! T-006: the capture adapter's error map (`voicen_lib::win::capture::capture_error`;
//! design 2, invariant 4, P-009). Windows CI only (decision #5): cpal is a Windows-only
//! dependency of the shell.
//!
//! The default-device facts (frames with rate and channels, none after `stop`, NFR-02;
//! red-test table row 17) are not asserted here: the runner's capture endpoints were never
//! probed (docs/decisions/windows-ci-runner.md, "Open"), so they are the owner's manual
//! check (T-006 Notes, orchestrator decision on Refresh 2). This file needs no runner
//! capability.
//!
//! The bounded open (`voicen_lib::win::capture::open_bounded`, design 2, invariants 2
//! and 3; review 1 finding #1) is tested with plain-Rust openers, no device: an opener
//! that blocks past the budget, one that produces its value after the timeout, one that
//! fails, one that panics, and one that succeeds. No verdict is a ceiling on a measured time
//! (T-201, os-answer-deadline-race): "returned before the opener" is an ordering on the
//! opener's own state, and "at once, not after the budget" uses a budget far above
//! `timing_support::HANG` under that hang bound (`within_hang`), so a wait for the budget is
//! a hang. A clock reading decides only a lower bound (`at_least`).
//!
//! Red-test table row 16; review 1 finding #1.
#![cfg(windows)]

mod timing_support;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use cpal::{Error, ErrorKind};
use timing_support::{at_least, eventually, holds_for, measure, now, within_hang, HANG};
use voicen_core::recording::CaptureError;
use voicen_lib::win::capture::{capture_error, open_bounded};

/// OS-like text planted as cpal's message: it must never reach a `CaptureError`.
const CANARY: &str = "PLANTED-OS-TEXT 0x80070005 Zugriff verweigert";

/// Every cpal 0.18.2 `ErrorKind` (error.rs:7-91), by a match that lists each one, so a
/// kind added by a cpal update shows up here.
fn all_kinds() -> Vec<ErrorKind> {
    let all = vec![
        ErrorKind::DeviceBusy,
        ErrorKind::DeviceChanged,
        ErrorKind::DeviceNotAvailable,
        ErrorKind::HostUnavailable,
        ErrorKind::InvalidInput,
        ErrorKind::PermissionDenied,
        ErrorKind::RealtimeDenied,
        ErrorKind::ResourceExhausted,
        ErrorKind::StreamInvalidated,
        ErrorKind::UnsupportedConfig,
        ErrorKind::UnsupportedOperation,
        ErrorKind::Xrun,
        ErrorKind::BackendError,
        ErrorKind::Other,
    ];
    for kind in &all {
        match kind {
            ErrorKind::DeviceBusy
            | ErrorKind::DeviceChanged
            | ErrorKind::DeviceNotAvailable
            | ErrorKind::HostUnavailable
            | ErrorKind::InvalidInput
            | ErrorKind::PermissionDenied
            | ErrorKind::RealtimeDenied
            | ErrorKind::ResourceExhausted
            | ErrorKind::StreamInvalidated
            | ErrorKind::UnsupportedConfig
            | ErrorKind::UnsupportedOperation
            | ErrorKind::Xrun
            | ErrorKind::BackendError
            | ErrorKind::Other => {}
            _ => panic!("a cpal ErrorKind this test does not list: {kind:?}"),
        }
    }
    all
}

#[test]
fn cpal_errors_map_to_capture_causes_and_never_carry_cpal_text() {
    // T-006 row 16 (invariant 4, P-009; spec 001 FR-009 reasons): DeviceNotAvailable ->
    // NoDevice, PermissionDenied -> AccessDenied, DeviceBusy -> DeviceBusy, every other
    // kind -> Other with a fixed literal: the same with and without cpal's message, never
    // containing that message (OS text) or cpal's own Display text, and not empty. The
    // message never changes the cause. Bite: `Other(err.to_string())` (the canary leaks),
    // DeviceBusy or PermissionDenied falling to Other, a cause read from the message.
    let mut wrong = Vec::new();
    for kind in all_kinds() {
        let plain = Error::new(kind);
        let planted = Error::with_message(kind, CANARY);
        let (a, b) = (capture_error(&plain), capture_error(&planted));
        let want = match kind {
            ErrorKind::DeviceNotAvailable => Some(CaptureError::NoDevice),
            ErrorKind::PermissionDenied => Some(CaptureError::AccessDenied),
            ErrorKind::DeviceBusy => Some(CaptureError::DeviceBusy),
            _ => None,
        };
        match want {
            Some(want) => {
                if a != want || b != want {
                    wrong.push(format!("{kind:?}: {a:?} / {b:?}, expected {want:?}"));
                }
            }
            None => match (&a, &b) {
                (CaptureError::Other(x), CaptureError::Other(y)) => {
                    if x != y {
                        wrong.push(format!(
                            "{kind:?}: the message changed the literal: {x:?} / {y:?}"
                        ));
                    }
                    if y.contains("PLANTED") || y.contains("0x80070005") {
                        wrong.push(format!("{kind:?}: cpal's message copied: {y:?}"));
                    }
                    if x.is_empty() || *x == plain.to_string() || *x == kind.to_string() {
                        wrong.push(format!("{kind:?}: not a fixed literal of ours: {x:?}"));
                    }
                }
                _ => wrong.push(format!(
                    "{kind:?}: {a:?} / {b:?}, expected Other(<literal>)"
                )),
            },
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

// ---- the bounded open (review 1 finding #1) -------------------------------------------

/// The fixed literal of a timed-out open (`OPEN_TIMED_OUT` in src/win/capture.rs).
const TIMED_OUT: &str = "the microphone did not open in time";

/// How long a blocking opener waits for the test's release before it gives up and
/// produces its value anyway: far above the budgets of the tests that hold it, so an
/// unbounded or inline open returns only after the opener did (an ordering the test reads),
/// short enough that such a regression fails, not hangs.
const OPENER_WAIT: Duration = Duration::from_secs(10);

/// The budget of a test whose opener answers at once: far above `HANG`, so an
/// `open_bounded` that waits for its budget instead of the opener's answer is a hang that
/// `within_hang` reports, not a slow pass.
const LONG_BUDGET: Duration = Duration::from_secs(HANG.as_secs() * 2);

/// A value that counts its drops (stands in for the cpal stream: dropping it closes
/// the device).
struct Opened {
    drops: Arc<AtomicUsize>,
}

impl Drop for Opened {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn an_opener_blocking_past_the_budget_is_a_timeout_within_the_budget() {
    // Invariant 2 (finding #1 a): an opener that does not finish within the budget ->
    // `Err(Other(TIMED_OUT))`, returned after about the budget and while the opener was
    // still held (an ordering, T-201), and the opener ran on another thread. Bite: the
    // opener run on the caller's thread (blocks OPENER_WAIT, then Ok; also the thread-id
    // check and the ordering), `recv()` instead of `recv_timeout` (returns only after the
    // opener did), a zero or wrong budget (elapsed below half the budget), another literal.
    let budget = Duration::from_millis(300);
    let (release, released) = mpsc::channel::<()>();
    let (ran_on_tx, ran_on) = mpsc::channel::<ThreadId>();
    let drops = Arc::new(AtomicUsize::new(0));
    let opener_drops = drops.clone();
    let opener_done = Arc::new(AtomicBool::new(false));
    let done = opener_done.clone();

    let (result, took) = measure(|| {
        open_bounded(budget, move || {
            let _ = ran_on_tx.send(thread::current().id());
            let _ = released.recv_timeout(OPENER_WAIT);
            done.store(true, Ordering::SeqCst);
            Ok(Opened {
                drops: opener_drops,
            })
        })
    });
    let returned_while_held = !opener_done.load(Ordering::SeqCst);
    let _ = release.send(());

    let ran_on = ran_on.recv_timeout(HANG).expect("the opener never ran");
    assert_ne!(
        ran_on,
        thread::current().id(),
        "the opener ran on the caller's thread"
    );
    match result {
        Err(CaptureError::Other(text)) => assert_eq!(text, TIMED_OUT),
        Err(other) => panic!("expected Other({TIMED_OUT:?}), got {other:?}"),
        Ok(_) => panic!("a value handed out after {took:?} with a budget of {budget:?}"),
    }
    at_least(took, budget / 2);
    assert!(
        returned_while_held,
        "returned after {took:?}, only once the held opener had given up (budget {budget:?}, \
         opener wait {OPENER_WAIT:?})"
    );
}

#[test]
fn a_value_produced_after_the_timeout_is_dropped_at_once_and_never_handed_out() {
    // Invariant 3, NFR-02 (finding #1 b): the opener produces its value only after
    // `open_bounded` gave up; the value is dropped exactly once after being produced (the
    // device closes; awaited up to the hang bound, T-201), and `open_bounded` returned the timeout, not
    // the value. Bite: no thread or no timeout (the value is handed out after
    // OPENER_WAIT), late values kept alive (a stored receiver, a leaked channel, a
    // `mem::forget`: the drop count stays 0), a value dropped twice.
    let budget = Duration::from_millis(200);
    let (release, released) = mpsc::channel::<()>();
    let (produced_tx, produced) = mpsc::channel::<()>();
    let drops = Arc::new(AtomicUsize::new(0));
    let opener_drops = drops.clone();

    let result = open_bounded(budget, move || {
        let _ = released.recv_timeout(OPENER_WAIT);
        let value = Opened {
            drops: opener_drops,
        };
        let _ = produced_tx.send(());
        Ok(value)
    });
    let handed_out = result.is_ok();
    // Drop whatever came back before observing the count: only a late drop counts.
    let returned = result.map(|_| ());
    let _ = release.send(());

    assert!(
        !handed_out,
        "the late value was handed out by open_bounded ({returned:?})"
    );
    assert_eq!(returned, Err(CaptureError::Other(TIMED_OUT.to_owned())));
    produced
        .recv_timeout(HANG)
        .expect("the opener never produced its value after the release");
    assert!(
        eventually(|| drops.load(Ordering::SeqCst) != 0),
        "the late value was not dropped within {HANG:?} of being produced (kept alive)"
    );
    // Exactly once, and nothing drops it a second time later.
    assert!(
        holds_for(Duration::from_millis(100), || drops.load(Ordering::SeqCst)
            == 1),
        "the late value's drop count is {} (must be dropped exactly once)",
        drops.load(Ordering::SeqCst)
    );
}

#[test]
fn a_failing_opener_gives_its_error_and_a_succeeding_one_its_value() {
    // Invariant 2 (finding #1 c, and the happy path): an opener's own error comes back
    // unchanged and its value is handed out, not dropped, both before a budget far above the
    // hang bound (`LONG_BUDGET` under `within_hang`), and the opener ran on another thread.
    // Bite: the opener run on the caller's thread (thread-id check), errors remapped to
    // Other, a result that waits for the budget (a hang), the value dropped on the opener
    // thread.
    let budget = LONG_BUDGET;
    let errors = [
        CaptureError::NoDevice,
        CaptureError::AccessDenied,
        CaptureError::DeviceBusy,
        CaptureError::Other("an opener's own fixed text".to_owned()),
    ];
    for err in errors {
        let (ran_on_tx, ran_on) = mpsc::channel::<ThreadId>();
        let opener_err = err.clone();
        let got = within_hang(move || {
            let result = open_bounded(budget, move || -> Result<(), CaptureError> {
                let _ = ran_on_tx.send(thread::current().id());
                Err(opener_err)
            });
            (result, thread::current().id())
        });
        let Some(((result, called_on), _)) = got else {
            panic!("{err:?}: open_bounded did not return within {HANG:?} (budget {budget:?})")
        };
        assert_eq!(result, Err(err.clone()));
        assert_ne!(
            ran_on.recv_timeout(HANG).expect("the opener never ran"),
            called_on,
            "{err:?}: the opener ran on the caller's thread"
        );
    }

    let drops = Arc::new(AtomicUsize::new(0));
    let opener_drops = drops.clone();
    let (ran_on_tx, ran_on) = mpsc::channel::<ThreadId>();
    let got = within_hang(move || {
        let result = open_bounded(budget, move || {
            let _ = ran_on_tx.send(thread::current().id());
            Ok(Opened {
                drops: opener_drops,
            })
        });
        (result, thread::current().id())
    });
    let Some(((result, called_on), _)) = got else {
        panic!("the value did not come back within {HANG:?} (budget {budget:?})")
    };
    assert_ne!(
        ran_on.recv_timeout(HANG).expect("the opener never ran"),
        called_on,
        "the opener ran on the caller's thread"
    );
    let value = match result {
        Ok(value) => value,
        Err(err) => panic!("a succeeding opener gave {err:?}"),
    };
    assert_eq!(
        drops.load(Ordering::SeqCst),
        0,
        "the handed-out value was dropped"
    );
    drop(value);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn a_panicking_opener_is_an_other_error_not_a_hang() {
    // Finding #1 c: an opener that panics -> `Err(Other(<fixed literal>))` before a budget
    // far above the hang bound (`LONG_BUDGET` under `within_hang`), never the timeout
    // literal, never the panic's text (P-009), and the caller does not panic. Bite: the
    // opener run on the caller's thread (the panic unwinds into the caller, which
    // `within_hang` resumes here), waiting out the budget after the opener died (a hang),
    // the panic payload copied into the error.
    let budget = LONG_BUDGET;
    let got = within_hang(move || {
        open_bounded(budget, || -> Result<(), CaptureError> {
            panic!("PLANTED-PANIC-TEXT opener died")
        })
    });
    let Some((result, _)) = got else {
        panic!("a dead opener did not come back within {HANG:?} (budget {budget:?})")
    };

    match result {
        Err(CaptureError::Other(text)) => {
            assert!(!text.is_empty(), "an empty literal");
            assert_ne!(text, TIMED_OUT, "a dead opener reported as a timeout");
            assert!(!text.contains("PLANTED"), "the panic text copied: {text:?}");
        }
        other => panic!("expected Other(<literal>), got {other:?}"),
    }
}

// ---- T-012: devices by endpoint id and device loss (FR-27, spec 001 FR-030/FR-031) ----
//
// Pinned API (docs/tasks/T-012.md `## Tests`, analysis option A): `CpalSource` implements
// core's `AudioSource::devices()` (cpal `input_devices()`: `id()` = the WASAPI endpoint
// id, `description()?.name()`, `is_default` by comparing the default's id; under
// `open_bounded`) and `start(&DeviceId, sink)` (`device_by_id`; missing -> `NoDevice`);
// `voicen_lib::win::capture::device_loss_callback(sink) -> impl FnMut(cpal::Error) + Send
// + 'static`, the stream's error callback: the first call of a kind that ends the stream
// reports `sink.device_lost(now)` and later calls report nothing; a kind after which cpal
// keeps the stream running (T-104: `keeps_running`) reports nothing. The runner has no capture
// endpoint probed (docs/decisions/windows-ci-runner.md), so these tests need none: the
// list is checked for its shape only, the open by id on an id no machine has, and the
// error callback without a stream. A real unplug is the owner's check (quickstart §3).

use std::sync::Mutex;

use voicen_core::platform::{AudioSource, DeviceId, FrameSink};
use voicen_lib::win::capture::{device_loss_callback, CpalSource, OPEN_BUDGET};

/// A sink that records what reached it.
#[derive(Default)]
struct RecordingSink {
    frames: AtomicUsize,
    lost: Mutex<Vec<Instant>>,
}

impl RecordingSink {
    fn lost(&self) -> Vec<Instant> {
        self.lost.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl FrameSink for RecordingSink {
    fn frames(&self, _interleaved: &[f32], _rate: u32, _channels: u16, _at: Instant) {
        self.frames.fetch_add(1, Ordering::SeqCst);
    }
    fn device_lost(&self, at: Instant) {
        self.lost.lock().unwrap_or_else(|e| e.into_inner()).push(at);
    }
}

/// The error kinds after which cpal 0.18.2 keeps the stream running (T-104): `Xrun`
/// (`process_input` emits it for a discontinuous packet and goes on capturing),
/// `RealtimeDenied` ("Audio will still play") and `DeviceChanged` ("The stream remains
/// active").
fn keeps_running(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::Xrun | ErrorKind::RealtimeDenied | ErrorKind::DeviceChanged
    )
}

#[test]
fn the_error_callback_reports_device_lost_once_for_a_kind_that_ends_the_stream() {
    // T-012 invariant (3), corrected by T-104: a kind that ends the stream (every kind
    // but `keeps_running`) is "capture ended", reported once as `device_lost(at)` with
    // `at` stamped in the callback; a second and third error on the same stream report
    // nothing; no frame is invented. Bite: the T-006 no-op `|_err| {}` (nothing
    // reported), a filter on DeviceNotAvailable only (other kinds lost), a report per
    // call (no once flag), an `at` taken before the callback ran (a constant captured at
    // build).
    let mut wrong = Vec::new();
    for kind in all_kinds().into_iter().filter(|k| !keeps_running(*k)) {
        let sink = Arc::new(RecordingSink::default());
        let mut callback = device_loss_callback(sink.clone() as Arc<dyn FrameSink>);
        let before = now();
        callback(Error::with_message(kind, CANARY));
        let after = now();
        callback(Error::new(ErrorKind::DeviceNotAvailable));
        callback(Error::new(ErrorKind::StreamInvalidated));
        let lost = sink.lost();
        match lost.as_slice() {
            [at] if *at >= before && *at <= after => {}
            other => wrong.push(format!(
                "{kind:?}: device_lost calls {other:?}, expected one between {before:?} and {after:?}"
            )),
        }
        if sink.frames.load(Ordering::SeqCst) != 0 {
            wrong.push(format!("{kind:?}: frames reported by the error callback"));
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

#[test]
fn an_error_that_keeps_the_stream_running_is_not_a_device_loss() {
    // T-104 (the owner's microphone: an Xrun on the first packets ended every hold as
    // DeviceLost ~70 ms after the press, "no speech", and the auto-repeat restarted it):
    // a `keeps_running` kind reports nothing, also repeated, and does not use up the
    // stream's one report: a later DeviceNotAvailable (an unplug, the failure branch)
    // still reports exactly one loss, stamped at that call, and later errors nothing.
    // Bite: the T-012 "any kind" callback (the Xrun reports the loss), a filter that lets
    // one of the three through, a once flag set by an ignored kind (the unplug lost).
    let mut wrong = Vec::new();
    for kind in all_kinds().into_iter().filter(|k| keeps_running(*k)) {
        let sink = Arc::new(RecordingSink::default());
        let mut callback = device_loss_callback(sink.clone() as Arc<dyn FrameSink>);
        callback(Error::with_message(kind, CANARY));
        callback(Error::new(kind));
        if !sink.lost().is_empty() {
            wrong.push(format!("{kind:?}: reported as a device loss"));
            continue;
        }
        let before = now();
        callback(Error::new(ErrorKind::DeviceNotAvailable));
        let after = now();
        callback(Error::new(kind));
        callback(Error::new(ErrorKind::StreamInvalidated));
        match sink.lost().as_slice() {
            [at] if *at >= before && *at <= after => {}
            other => wrong.push(format!(
                "{kind:?} then DeviceNotAvailable: device_lost calls {other:?}, expected one between {before:?} and {after:?}"
            )),
        }
        if sink.frames.load(Ordering::SeqCst) != 0 {
            wrong.push(format!("{kind:?}: frames reported by the error callback"));
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

#[test]
fn the_device_list_has_unique_endpoint_ids_and_at_most_one_default() {
    // FR-27 / research R-3 / contracts/ipc.md `[{ id, name, is_default }]`: whatever the
    // runner has (possibly nothing), `devices()` returns within the open budget without
    // panicking; every entry has a non-empty endpoint id and a non-empty display name, the
    // ids are unique, and at most one entry is the default. An enumeration error carries a
    // fixed literal, never OS text. Bite: an unbounded enumeration (a hang past
    // `timing_support::HANG`, T-201: no ceiling on the time), the name used as the id (duplicates when two devices share a name, as two
    // identical USB headsets do), every device flagged default, cpal's text in an error.
    let Some((listed, _)) = within_hang(|| CpalSource::new().devices()) else {
        panic!("devices() did not return within {HANG:?} (open budget {OPEN_BUDGET:?})")
    };
    match listed {
        Ok(devices) => {
            let mut ids: Vec<&str> = devices.iter().map(|d| d.id.0.as_str()).collect();
            assert!(
                devices
                    .iter()
                    .all(|d| !d.id.0.is_empty() && !d.name.is_empty()),
                "an entry without id or name: {devices:?}"
            );
            ids.sort_unstable();
            let n = ids.len();
            ids.dedup();
            assert_eq!(ids.len(), n, "duplicate endpoint ids: {devices:?}");
            assert!(
                devices.iter().filter(|d| d.is_default).count() <= 1,
                "more than one default: {devices:?}"
            );
        }
        Err(CaptureError::Other(text)) => {
            assert!(!text.is_empty(), "an empty literal");
        }
        Err(_) => {}
    }
}

#[test]
fn start_on_an_id_no_machine_has_is_no_device_and_reports_nothing() {
    // FR-04 branch / `## Investigation` (`device_by_id` missing -> `NoDevice`): a
    // well-formed endpoint id of no device and a malformed id both give `NoDevice` (under
    // the hang bound, T-201), and the sink gets neither frames nor a loss (the session reports
    // the press's failure itself). Bite: falling back to the default device inside the
    // adapter (Ok, or frames), an unparsable id mapped to Other, a loss reported for a
    // capture that never opened, an unbounded lookup.
    for raw in [
        "{0.0.1.00000000}.{00000000-0000-0000-0000-000000000000}",
        "not-an-endpoint-id",
    ] {
        let sink = Arc::new(RecordingSink::default());
        let into_start = sink.clone();
        let id = DeviceId(raw.to_string());
        let Some((result, _)) =
            within_hang(move || CpalSource::new().start(&id, into_start).map(|_capture| ()))
        else {
            panic!("{raw}: start did not return within {HANG:?} (open budget {OPEN_BUDGET:?})")
        };
        match result {
            Err(CaptureError::NoDevice) => {}
            Err(other) => panic!("{raw}: expected NoDevice, got {other:?}"),
            Ok(_) => panic!("{raw}: a capture opened on a device that does not exist"),
        }
        thread::sleep(Duration::from_millis(100));
        assert_eq!(sink.frames.load(Ordering::SeqCst), 0, "{raw}: frames");
        assert_eq!(sink.lost(), Vec::<Instant>::new(), "{raw}: a loss reported");
    }
}
