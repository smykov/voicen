//! T-016: the model downloader against a raw-TCP mock server (spec 002 US1,
//! contracts/core-traits.md "Downloader", T-016 analysis approach (1)-(6),
//! decision #49).
//!
//! Every test injects a catalog entry for the fake ~64 KiB model (tests/common) and
//! millisecond timeouts. The downloader runs on its own std thread; the tests call
//! it from plain `#[test]` threads (no tokio runtime: the blocking reqwest client
//! panics inside one, T-040). The harness (tests/common/download.rs) snapshots the
//! models dir at each event, so "`.part` removed before the end event" is checked
//! at the moment the event is emitted, not afterwards.
//!
//! The refused-port case is in `tests/local_download_refused.rs`, its own process
//! (T-016 review round 1 #1).

mod common;

use std::time::Duration;

use common::download::{
    assert_no_url_in, assert_retry_succeeds, assert_verified_file_only, events, fixture,
    fixture_with, fixture_with_timeouts, timeouts, Events, Fixture, Seen, END_WAIT, FILE, PART,
};
use common::held::Held;
use common::os_answer::OS_ANSWER_BUDGET;
use common::timing::{at_least, at_most_per_second, between, now, Took};
use common::{dir_entries, entry, model_bytes, FakeDisk, Serve, Server, NEEDED, SIZE};
use voicen_core::local_models::catalog::ModelId;
use voicen_core::local_models::download::{DownloadError, DownloadEvent, DownloadFailure};
use voicen_core::models::DownloadedModels;
use voicen_core::secrets::{KeyEdits, KeyPresence};
use voicen_core::settings::validate::{validate, KeyEditsWithPresence};
use voicen_core::settings::{defaults, EngineKind, ErrorCode, FieldId};
use voicen_core::timeouts::Timeouts;

/// How long a stalling or silent server holds the connection: three times
/// [`END_WAIT`], so a client that waits for the hold (no read timeout, reqwest's
/// 30 s default, a cancel that waits for the read) ends after `END_WAIT` and the
/// test fails on "no end event", not on a wall-clock ceiling (T-080 I2).
const HOLD: Duration = Duration::from_secs(30);

/// `base` served by `server`, with `download_no_data = no_data` and every other
/// deadline (connect included) longer than [`END_WAIT`]: a client that takes its
/// read timeout from another field ends after `END_WAIT`, and the test fails on
/// "no end event" (T-080 I2: the outcome, not a ceiling, catches a longer limit).
fn stall_fixture(server: &Server, no_data: Duration) -> Fixture {
    let t = Timeouts {
        connect: HOLD,
        ..timeouts(no_data)
    };
    assert!(
        HOLD > END_WAIT,
        "HOLD {HOLD:?} must exceed END_WAIT {END_WAIT:?}"
    );
    for (name, v) in [
        ("connect", t.connect),
        ("api_transcription", t.api_transcription),
        ("local_server", t.local_server),
        ("post_processing", t.post_processing),
        ("builtin", t.builtin),
    ] {
        assert!(
            v > END_WAIT,
            "stall fixture: {name} {v:?} is not above END_WAIT"
        );
    }
    fixture_with_timeouts(
        vec![entry(ModelId::Base, FILE, &server.url(FILE))],
        FakeDisk::with_available(10 * NEEDED),
        t,
    )
}

/// Settings validation accepts `builtin_local` with `base` through `store`.
fn validation_accepts_base(store: &dyn DownloadedModels) -> bool {
    let mut s = defaults(Some("en-US"));
    s.engine = EngineKind::BuiltinLocal;
    s.builtin_local.model_id = Some("base".to_string());
    let edits = KeyEdits::default();
    let keys = KeyEditsWithPresence {
        edits: &edits,
        presence: KeyPresence::default(),
    };
    !validate(&s, &keys, store).iter().any(|e| {
        e.field == FieldId::EngineBuiltinLocalModelId && e.code == ErrorCode::ModelNotDownloaded
    })
}

/// The failure branch (Acceptance line 2): `Failed{reason}` with `reason` as
/// expected, `.part` gone and no final file at the moment of the event, exactly
/// one end event, no URL in it, then a retry succeeds.
fn fails_then_retry_succeeds(
    label: &str,
    f: &Fixture,
    expect: impl Fn(&DownloadFailure) -> bool,
) -> Took {
    let started = now();
    let mut ev = f
        .start(ModelId::Base)
        .unwrap_or_else(|e| panic!("{label}: start refused: {e:?}"));
    let end = ev.wait_end();
    let took = between(started, end.at);
    match &end.event {
        DownloadEvent::Failed { id, reason } => {
            assert_eq!(*id, ModelId::Base, "{label}: id");
            assert!(expect(reason), "{label}: unexpected reason {reason:?}");
        }
        other => panic!(
            "{label}: expected Failed, got {other:?}; seen {:?}",
            ev.seen
        ),
    }
    assert!(
        end.dir.is_empty(),
        "{label}: models dir at the Failed event (no .part, no final file): {:?}",
        end.dir
    );
    let more = ev.drain_for(Duration::from_millis(200));
    assert!(more.is_empty(), "{label}: events after the end: {more:?}");
    for s in &ev.seen {
        if let DownloadEvent::Progress {
            total, received, ..
        } = s.event
        {
            assert_eq!(total, SIZE, "{label}: progress total");
            assert!(received <= SIZE, "{label}: progress received {received}");
        }
    }
    assert_no_url_in(&ev.seen, label);
    assert!(
        dir_entries(&f.models).is_empty(),
        "{label}: models dir after"
    );
    assert!(
        !f.store.is_downloaded("base"),
        "{label}: store says downloaded"
    );
    assert_retry_succeeds(f, label);
    took
}

// ---- (1) happy path ---------------------------------------------------------

#[test]
fn happy_path_reports_progress_then_finished_and_the_verified_file_is_downloaded() {
    // Acceptance line 1. Bite: no Progress before Finished, a wrong total, received
    // going backwards or past the size, Finished before the rename (final file not
    // there yet, or `.part` still there), a second end event, the disk probe not
    // asked about the models dir, the store or validation not seeing the file.
    let server = Server::start(vec![]);
    let f = fixture(&server, Duration::from_secs(2));
    let mut ev = f.start(ModelId::Base).expect("start");
    let end = ev.wait_end();

    assert_eq!(end.event, DownloadEvent::Finished { id: ModelId::Base });
    assert_eq!(
        end.dir,
        vec![FILE.to_string()],
        "at Finished the final file is in place and no .part remains"
    );
    let progress = ev.progress();
    assert!(!progress.is_empty(), "no Progress before Finished");
    assert!(
        progress.iter().all(|(_, t)| *t == SIZE),
        "totals {progress:?}"
    );
    assert!(
        progress.windows(2).all(|w| w[0].0 <= w[1].0) && progress.iter().all(|(r, _)| *r <= SIZE),
        "received not monotonic within the size: {progress:?}"
    );
    assert!(
        ev.seen.iter().take_while(|s| !s.is_end()).count() == ev.seen.len() - 1,
        "Finished is not the last event: {:?}",
        ev.seen
    );
    let more = ev.drain_for(Duration::from_millis(200));
    assert!(more.is_empty(), "events after Finished: {more:?}");

    assert_verified_file_only(&f, "happy");
    assert_eq!(server.accepts(), 1, "requests");
    assert_eq!(f.disk.asked(), vec![f.models.clone()], "disk probe dir");
    assert!(f.store.is_downloaded("base"));
    assert_eq!(
        f.store.path_if_downloaded(ModelId::Base),
        Some(f.final_path())
    );
    assert!(
        validation_accepts_base(f.store.as_ref()),
        "settings validation refuses builtin_local with the downloaded model"
    );
}

// ---- (2) failure branches ---------------------------------------------------

#[test]
fn cut_off_body_fails_interrupted_then_retry_succeeds() {
    // Content-Length = size, half the bytes, FIN. Bite: the short file renamed, the
    // `.part` left behind, another reason, no retry possible (busy flag not cleared
    // before the end event).
    let server = Server::start(vec![Serve::CutOff]);
    let f = fixture(&server, Duration::from_secs(2));
    fails_then_retry_succeeds("cut off", &f, |r| {
        *r == DownloadFailure::DownloadInterrupted
    });
}

#[test]
fn dropped_chunked_body_fails_interrupted_then_retry_succeeds() {
    // A chunked body closed without its last chunk (connection drop mid-body).
    let server = Server::start(vec![Serve::DropChunked]);
    let f = fixture(&server, Duration::from_secs(2));
    fails_then_retry_succeeds("drop mid-body", &f, |r| {
        *r == DownloadFailure::DownloadInterrupted
    });
}

#[test]
fn one_altered_byte_fails_checksum_mismatch_then_retry_succeeds() {
    // All SIZE bytes arrive, one flipped. Bite: no hash check, a hash over another
    // range, a comparison against anything but the entry's pinned value.
    let server = Server::start(vec![Serve::Altered]);
    let f = fixture(&server, Duration::from_secs(2));
    fails_then_retry_succeeds("altered byte", &f, |r| {
        *r == DownloadFailure::ChecksumMismatch
    });
}

#[test]
fn short_complete_body_fails_interrupted_then_retry_succeeds() {
    // A clean end 100 bytes short of the catalog size (a consistent Content-Length).
    // contracts/core-traits.md: fewer bytes than the catalog size is a cut body ->
    // DownloadInterrupted (retry is the user's remedy), not ChecksumMismatch.
    // Bite: trusting the server's length instead of the catalog size; the short end
    // reported as ChecksumMismatch (hashing a short file instead of stopping at the
    // byte count).
    let server = Server::start(vec![Serve::Short]);
    let f = fixture(&server, Duration::from_secs(2));
    fails_then_retry_succeeds("short body", &f, |r| {
        *r == DownloadFailure::DownloadInterrupted
    });
}

#[test]
fn overlong_body_fails_checksum_mismatch_then_retry_succeeds() {
    // The model plus one byte. contracts/core-traits.md: a body longer than the
    // catalog size is ChecksumMismatch (it cannot be the pinned file). Bite:
    // stopping at the catalog size and accepting a body that is not the pinned file,
    // hashing only the first SIZE bytes, the overrun reported as DownloadInterrupted.
    let server = Server::start(vec![Serve::Long]);
    let f = fixture(&server, Duration::from_secs(2));
    fails_then_retry_succeeds("overlong body", &f, |r| {
        *r == DownloadFailure::ChecksumMismatch
    });
}

#[test]
fn http_404_fails_http_status_then_retry_succeeds() {
    // Bite: the error body written as the model, a status not reported, a 4xx
    // mapped to another reason.
    let server = Server::start(vec![Serve::Status(404)]);
    let f = fixture(&server, Duration::from_secs(2));
    fails_then_retry_succeeds("HTTP 404", &f, |r| {
        *r == DownloadFailure::HttpStatus { code: 404 }
    });
}

#[test]
fn http_500_fails_http_status_then_retry_succeeds() {
    let server = Server::start(vec![Serve::Status(500)]);
    let f = fixture(&server, Duration::from_secs(2));
    fails_then_retry_succeeds("HTTP 500", &f, |r| {
        *r == DownloadFailure::HttpStatus { code: 500 }
    });
}

#[test]
fn stall_past_no_data_fails_interrupted_within_the_timeout_then_retry_succeeds() {
    // 4 KiB, then HOLD (30 s) of silence; no_data = 300 ms. Bite: no read timeout
    // (reqwest's 30 s default, or none) or the timeout taken from another field (all
    // longer than END_WAIT here): no end event within END_WAIT; another reason; a
    // shorter limit (the end before no_data).
    let no_data = Duration::from_millis(300);
    let server = Server::start(vec![Serve::Stall {
        after: 4096,
        hold: HOLD,
    }]);
    let f = stall_fixture(&server, no_data);
    let took =
        fails_then_retry_succeeds("stall", &f, |r| *r == DownloadFailure::DownloadInterrupted);
    at_least(took, no_data);
}

#[test]
fn no_response_headers_within_no_data_fails_then_retry_succeeds() {
    // The server accepts and never answers for HOLD (30 s); no_data = 300 ms also
    // bounds the wait for the headers (ClientBuilder::timeout). Bite: a client
    // without a timeout on the response wait, or one taken from another field (all
    // longer than END_WAIT here): no end event within END_WAIT; a shorter limit (the end
    // before no_data).
    let no_data = Duration::from_millis(300);
    let server = Server::start(vec![Serve::SilentHeaders { hold: HOLD }]);
    let f = stall_fixture(&server, no_data);
    let took = fails_then_retry_succeeds("silent headers", &f, |r| {
        matches!(
            r,
            DownloadFailure::DownloadInterrupted | DownloadFailure::SourceUnreachable { .. }
        )
    });
    at_least(took, no_data);
}

// The refused-port case lives in its own binary, `tests/local_download_refused.rs`:
// a released port cannot stay free while the parallel tests here bind 127.0.0.1:0
// (T-016 review round 1 #1).

#[test]
fn part_file_that_cannot_be_created_fails_disk_error_then_retry_succeeds() {
    // The models "dir" is a regular file, so `<file>.part` cannot be created. Bite:
    // a panic or a hang on the write error, another reason, a final file anyway.
    let server = Server::start(vec![]);
    let f = fixture(&server, Duration::from_secs(2));
    std::fs::remove_dir(&f.models).expect("remove models dir");
    std::fs::write(&f.models, b"not a dir").expect("file in place of the dir");

    let mut ev = f.start(ModelId::Base).expect("start");
    let end = ev.wait_end();
    assert_eq!(
        end.event,
        DownloadEvent::Failed {
            id: ModelId::Base,
            reason: DownloadFailure::DiskError
        }
    );
    assert_eq!(
        std::fs::read(&f.models).expect("the blocking file stays"),
        b"not a dir"
    );

    std::fs::remove_file(&f.models).expect("remove the blocking file");
    std::fs::create_dir(&f.models).expect("create models dir");
    assert_retry_succeeds(&f, "disk error");
}

// ---- (3) trickle vs stall -----------------------------------------------------

#[test]
fn trickle_longer_than_no_data_succeeds_and_stall_fails() {
    // T-016 Investigation "Recommended" first test. A chunk every 50 ms for ~1.6 s
    // with no_data = 400 ms must finish: the no-data timeout is per read, not a
    // total. The same no_data fails a stall. Bite: RequestBuilder::timeout(no_data)
    // (a total timeout ends the trickle at ~400 ms), no timeout at all (the stall
    // half gives no end event within END_WAIT: the hold is HOLD), progress only at
    // the end, a temp name other than `<file>.part` (cleanup_at_start would miss
    // it).
    //
    // Margins: each gap leaves 350 ms of scheduling slack below no_data, and the
    // whole trickle lasts 4x no_data. The earlier 60 ms / 150 ms pair left 90 ms,
    // and a loaded host stretched one gap past 150 ms (2 failures in 50 runs,
    // T-016 review round 1 loop).
    let no_data = Duration::from_millis(400);
    let server = Server::start(vec![Serve::Trickle {
        chunk: 2_000,
        every: Duration::from_millis(50),
    }]);
    let f = fixture(&server, no_data);
    let started = now();
    let mut ev = f.start(ModelId::Base).expect("start");
    let first = ev.wait_progress();
    assert!(
        first.dir.contains(&PART.to_string()),
        "while downloading, the bytes go to {PART}: dir {:?}",
        first.dir
    );
    let end = ev.wait_end();
    let took = between(started, end.at);
    assert_eq!(
        end.event,
        DownloadEvent::Finished { id: ModelId::Base },
        "trickle with no_data {no_data:?}; seen {:?}",
        ev.seen
    );
    // The trickle outlasts no_data several times over (a lower bound, I2).
    at_least(took, 3 * no_data);
    let n = ev.progress().len();
    assert!(
        n >= 2,
        "only {n} Progress events over {took:?} (≥ 1/s while data arrives)"
    );
    at_most_per_second(n, 4, took, "Progress events");
    assert_verified_file_only(&f, "trickle");

    let stall = Server::start(vec![Serve::Stall {
        after: 4096,
        hold: HOLD,
    }]);
    let g = stall_fixture(&stall, no_data);
    let started = now();
    let mut ev = g.start(ModelId::Base).expect("start stall");
    let end = ev.wait_end();
    assert_eq!(
        end.event,
        DownloadEvent::Failed {
            id: ModelId::Base,
            reason: DownloadFailure::DownloadInterrupted
        }
    );
    at_least(between(started, end.at), no_data);
    assert!(end.dir.is_empty(), "dir at Failed: {:?}", end.dir);
}

// ---- (4) cancel ---------------------------------------------------------------

/// Cancel after the first Progress; returns the end event and the events. How soon
/// the end comes is not measured (T-080 I2): a cancel that waits for the hold or
/// for the transfer shows in the outcome (no end within END_WAIT, or Finished).
fn cancel_after_first_progress(f: &Fixture, label: &str) -> (Seen, Events) {
    let mut ev = f.start(ModelId::Base).expect("start");
    let first = ev.wait_progress();
    assert!(
        first.dir.contains(&PART.to_string()),
        "{label}: no .part while downloading"
    );
    assert!(
        f.dl.cancel(ModelId::Base),
        "{label}: cancel of the running download"
    );
    let end = ev.wait_end();
    (end, ev)
}

#[test]
fn cancel_during_stall_ends_cancelled_only_after_the_part_is_gone() {
    // Design point 4: the flag takes effect when the current read returns
    // (≤ no_data = 800 ms here); the end is Cancelled, not Failed, and comes after
    // the `.part` is removed. Bite: Cancelled emitted before the remove, the read
    // timeout reported as Failed{DownloadInterrupted}, the `.part` left, cancel
    // waiting for the hold (HOLD, so no end event within END_WAIT).
    let server = Server::start(vec![Serve::Stall {
        after: 4096,
        hold: HOLD,
    }]);
    let f = stall_fixture(&server, Duration::from_millis(800));
    let (end, mut ev) = cancel_after_first_progress(&f, "stall");
    assert_eq!(end.event, DownloadEvent::Cancelled { id: ModelId::Base });
    assert!(end.dir.is_empty(), "dir at Cancelled: {:?}", end.dir);
    let more = ev.drain_for(Duration::from_millis(300));
    assert!(more.is_empty(), "events after Cancelled: {more:?}");
    assert!(
        !ev.seen
            .iter()
            .any(|s| matches!(s.event, DownloadEvent::Failed { .. })),
        "a retry reason after cancel: {:?}",
        ev.seen
    );
    assert!(!f.dl.cancel(ModelId::Base), "cancel when nothing runs");
    assert!(!f.store.is_downloaded("base"));
    assert_retry_succeeds(&f, "after cancel");
}

#[test]
fn cancel_during_steady_transfer_ends_cancelled_quickly() {
    // Small chunks every 60 ms (~4 s in total): the flag is seen between reads.
    // Bite: cancel not checked in the read loop (the download finishes instead, so
    // the end is Finished, not Cancelled).
    let server = Server::start(vec![Serve::Trickle {
        chunk: 1_000,
        every: Duration::from_millis(60),
    }]);
    let f = fixture(&server, Duration::from_secs(2));
    let (end, _ev) = cancel_after_first_progress(&f, "steady");
    assert_eq!(end.event, DownloadEvent::Cancelled { id: ModelId::Base });
    assert!(end.dir.is_empty(), "dir at Cancelled: {:?}", end.dir);
}

#[test]
fn cancel_of_another_model_or_when_idle_returns_false_and_changes_nothing() {
    // Bite: cancel ignoring the id (stops the running `base` when asked for `tiny`),
    // cancel returning true with nothing running.
    let server = Server::start(vec![Serve::Stall {
        after: 4096,
        hold: Duration::from_secs(3),
    }]);
    let f = fixture(&server, Duration::from_secs(2));
    assert!(!f.dl.cancel(ModelId::Base), "idle cancel");
    let mut ev = f.start(ModelId::Base).expect("start");
    ev.wait_progress();
    assert!(
        !f.dl.cancel(ModelId::Tiny),
        "cancel of a model that is not running"
    );
    let more = ev.drain_for(Duration::from_millis(300));
    assert!(
        more.iter().all(|s| !s.is_end()),
        "cancel(tiny) ended the base download: {more:?}"
    );
    assert!(f.dl.cancel(ModelId::Base));
    assert_eq!(
        ev.wait_end().event,
        DownloadEvent::Cancelled { id: ModelId::Base }
    );
}

// ---- (5) one at a time ------------------------------------------------------

#[test]
fn second_start_while_one_runs_is_busy_and_sends_no_request() {
    // At most one download (invariant). Bite: a second thread started, Busy only for
    // the same id, a request sent before the refusal.
    let first = Server::start(vec![Serve::Stall {
        after: 4096,
        hold: Duration::from_secs(3),
    }]);
    let other = Server::start(vec![]);
    let f = fixture_with(
        vec![
            entry(ModelId::Tiny, "ggml-tiny.bin", &other.url("ggml-tiny.bin")),
            entry(ModelId::Base, FILE, &first.url(FILE)),
        ],
        FakeDisk::with_available(10 * NEEDED),
        Duration::from_secs(2),
    );
    let mut ev = f.start(ModelId::Base).expect("start");
    ev.wait_progress();
    assert_eq!(
        f.start(ModelId::Base).err(),
        Some(DownloadError::Busy),
        "same id"
    );
    assert_eq!(
        f.start(ModelId::Tiny).err(),
        Some(DownloadError::Busy),
        "other id"
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(first.accepts(), 1, "requests to the running model's server");
    assert_eq!(other.accepts(), 0, "requests for the refused model");
    assert!(f.dl.cancel(ModelId::Base));
    assert_eq!(
        ev.wait_end().event,
        DownloadEvent::Cancelled { id: ModelId::Base }
    );
}

// ---- (6) disk space ---------------------------------------------------------

#[test]
fn disk_below_size_plus_1_percent_refuses_with_needed_and_sends_no_request() {
    // R-9: needed = size + 1 % = 66 256 bytes; available one byte less. Bite: no
    // check, the check without the margin, the check after the request, `needed`
    // not the required total, an event or a `.part` on refusal.
    let server = Server::start(vec![]);
    let f = fixture_with(
        vec![entry(ModelId::Base, FILE, &server.url(FILE))],
        FakeDisk::with_available(NEEDED - 1),
        Duration::from_secs(2),
    );
    let (cb, mut ev) = events(&f.models);
    assert_eq!(
        f.dl.start(ModelId::Base, cb).err(),
        Some(DownloadError::NotEnoughDiskSpace { needed: NEEDED })
    );
    let more = ev.drain_for(Duration::from_millis(300));
    assert!(more.is_empty(), "events after a refused start: {more:?}");
    assert_eq!(server.accepts(), 0, "a request was sent");
    assert!(
        dir_entries(&f.models).is_empty(),
        "models dir {:?}",
        dir_entries(&f.models)
    );
    assert_eq!(f.disk.asked(), vec![f.models.clone()], "disk probe dir");
    // The refusal leaves the downloader idle.
    assert!(!f.dl.cancel(ModelId::Base));
}

#[test]
fn disk_with_exactly_size_plus_1_percent_downloads() {
    // Boundary of R-9. Bite: `>` instead of `>=`, a margin above 1 %.
    let server = Server::start(vec![]);
    let f = fixture_with(
        vec![entry(ModelId::Base, FILE, &server.url(FILE))],
        FakeDisk::with_available(NEEDED),
        Duration::from_secs(2),
    );
    let mut ev = f
        .start(ModelId::Base)
        .expect("start with exactly the needed space");
    assert_eq!(
        ev.wait_end().event,
        DownloadEvent::Finished { id: ModelId::Base }
    );
    assert_verified_file_only(&f, "exact space");
}

#[test]
fn disk_probe_error_lets_the_download_proceed() {
    // R-9: a probe error is not a refusal. Bite: mapping the probe error to
    // NotEnoughDiskSpace or DiskError.
    let server = Server::start(vec![]);
    let f = fixture_with(
        vec![entry(ModelId::Base, FILE, &server.url(FILE))],
        FakeDisk::failing(),
        Duration::from_secs(2),
    );
    let mut ev = f.start(ModelId::Base).expect("start despite a probe error");
    assert_eq!(
        ev.wait_end().event,
        DownloadEvent::Finished { id: ModelId::Base }
    );
    assert_verified_file_only(&f, "probe error");
}

// ---- already downloaded / not in catalog / wrong-size file -------------------

#[test]
fn model_missing_from_the_catalog_is_refused_without_a_request_or_event() {
    // contracts/core-traits.md DownloadError::NotInCatalog: the injected catalog has
    // only `base`; `start(tiny)` refuses before any request, emits no event, writes
    // nothing and leaves the downloader idle (a `base` download still starts).
    // Bite: another refusal (or none: a request to some URL, a panic on the missing
    // entry), the active slot taken before the catalog check (Busy afterwards).
    let server = Server::start(vec![]);
    let f = fixture(&server, Duration::from_secs(2));
    let (cb, mut ev) = events(&f.models);
    assert_eq!(
        f.dl.start(ModelId::Tiny, cb).err(),
        Some(DownloadError::NotInCatalog)
    );
    let more = ev.drain_for(Duration::from_millis(300));
    assert!(more.is_empty(), "events after a refused start: {more:?}");
    assert_eq!(server.accepts(), 0, "a request was sent");
    assert!(
        dir_entries(&f.models).is_empty(),
        "models dir {:?}",
        dir_entries(&f.models)
    );
    assert!(
        !f.dl.cancel(ModelId::Tiny),
        "cancel(tiny) after the refusal"
    );
    let mut ev = f
        .start(ModelId::Base)
        .expect("the refusal left the downloader busy");
    assert_eq!(
        ev.wait_end().event,
        DownloadEvent::Finished { id: ModelId::Base }
    );
    assert_eq!(server.accepts(), 1, "requests: only the base download");
}

#[test]
fn already_downloaded_model_is_refused_without_a_request() {
    // contracts/core-traits.md DownloadError::AlreadyDownloaded. Bite: downloading
    // again over a good file.
    let server = Server::start(vec![]);
    let f = fixture(&server, Duration::from_secs(2));
    std::fs::write(f.final_path(), model_bytes()).expect("place the model");
    assert_eq!(
        f.start(ModelId::Base).err(),
        Some(DownloadError::AlreadyDownloaded)
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(server.accepts(), 0, "a request was sent");
}

#[test]
fn wrong_size_final_file_is_downloaded_again_and_replaced() {
    // data-model: a final-name file of the wrong size is not downloaded; a
    // re-download overwrites it via rename. Bite: AlreadyDownloaded for any file
    // with the final name, a rename that fails because the target exists.
    let server = Server::start(vec![]);
    let f = fixture(&server, Duration::from_secs(2));
    std::fs::write(f.final_path(), b"stale partial model").expect("place a wrong file");
    let mut ev = f
        .start(ModelId::Base)
        .expect("start over a wrong-size file");
    assert_eq!(
        ev.wait_end().event,
        DownloadEvent::Finished { id: ModelId::Base }
    );
    assert_verified_file_only(&f, "replaced");
}

// ---- T-079: a lookup with no answer (decisions #106, #113) -------------------------

#[test]
fn unanswered_lookup_fails_source_unreachable_while_the_lookup_is_held() {
    // T-079 twin (model download, #106): the catalog host's lookup never answers
    // while the download runs; connect 300 ms, download_no_data far longer. The
    // download ends (the end event emitted) while the lookup is still held, as
    // Failed(SourceUnreachable{host:port}), with no `.part` and no model file in
    // the dir at that moment. Today the reason is the same but the end event comes only after the release (the
    // client drop waits for the lookup), so no end event within END_WAIT. Bite:
    // the downloader's own client without the shared resolver (the seam never
    // asked); the lookup joined at drop.
    let held = Held::install("models.t079.example.com", Vec::new());
    let host = format!("{}:8443", held.host);
    let t = Timeouts {
        connect: Duration::from_millis(300),
        download_no_data: OS_ANSWER_BUDGET,
        ..timeouts(OS_ANSWER_BUDGET)
    };
    let f = fixture_with_timeouts(
        vec![entry(
            ModelId::Base,
            FILE,
            &format!("http://{host}/models/{FILE}"),
        )],
        FakeDisk::with_available(10 * NEEDED),
        t,
    );
    let mut ev = f.start(ModelId::Base).expect("start accepted");
    let end = ev.wait_end();
    held.assert_still_held("download end event");
    assert_eq!(
        end.event,
        DownloadEvent::Failed {
            id: ModelId::Base,
            reason: DownloadFailure::SourceUnreachable { host: host.clone() },
        }
    );
    assert!(
        !end.dir.iter().any(|n| n == PART || n == FILE),
        "no .part or model file at the end event: {:?}",
        end.dir
    );
    assert_no_url_in(&ev.seen, "held lookup");
    held.lookup.release();
}
