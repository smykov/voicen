//! T-008: `diag::LogObserver`, the release `PipelineObserver` (analysis approach 4
//! and the `DictationEvent` half of 1).
//!
//! One dictation line per recording with the engine, the outcome and the FR-20
//! timings plus `duration_ms`; keys whose event did not come are left out. Events
//! of one recording are joined by `RecordingId` (`JobFinished.recording`, T-008)
//! and `Delivered` by its job's `seq`, never by event order, so interleaved jobs
//! (T-011 runs `process` concurrently) keep their own timings. Recording ids come
//! from the real `RecordingController`; events are fed as T-006 and the pipeline
//! emit them.

mod common;
mod diag_support;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use common::timing::now;

use diag_support::{active_lines, closed, dictation_lines, open_log, pairs, Line, NOON_UTC};
use voicen_core::delivery::DeliveryResult;
use voicen_core::diag::{LogConfig, LogObserver};
use voicen_core::events::{DeviceKind, DictationEvent, OutcomeCode, PipelineObserver, WarningCode};
use voicen_core::failure::FailureReason;
use voicen_core::recording::{MicCause, Press, RecordingController, RecordingEnd, RecordingId};
use voicen_core::settings::gate::Blocked;
use voicen_core::test_support::TempDir;

/// `n` distinct recording ids from the real controller (press, discarded release).
fn ids(n: usize) -> Vec<RecordingId> {
    let mut ctrl = RecordingController::<()>::new();
    let t0 = now();
    (0..n)
        .map(|i| {
            let t = t0 + Duration::from_secs(i as u64);
            let id = match ctrl.press(t, ()) {
                Press::Start(id) => id,
                Press::Ignored => panic!("press {i} ignored"),
            };
            let _ = ctrl.release(t + Duration::from_millis(10));
            id
        })
        .collect()
}

/// Fields drop in this order: the observer (and its log's file handle) before the
/// temp dir, which Windows cannot remove while the file is open.
struct Fixture {
    obs: LogObserver,
    dir: std::path::PathBuf,
    _tmp: TempDir,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new();
    let dir = tmp.path().join("logs");
    let (log, _clock, _seen) =
        open_log(&dir, diag_support::at(NOON_UTC, 0), 0, LogConfig::default());
    let obs = LogObserver::new(Arc::clone(&log));
    Fixture {
        obs,
        dir,
        _tmp: tmp,
    }
}

impl Fixture {
    fn feed(&self, events: &[DictationEvent]) {
        for e in events {
            self.obs.event(e);
        }
    }

    fn lines(&self) -> Vec<String> {
        if self.dir.join(diag_support::ACTIVE).exists() {
            active_lines(&self.dir)
        } else {
            Vec::new()
        }
    }

    fn dictations(&self) -> Vec<Line> {
        dictation_lines(&self.lines())
    }
}

fn started(recording: RecordingId, ms: u64) -> DictationEvent {
    DictationEvent::RecordingStarted {
        recording,
        hotkey_to_first_frame_ms: ms,
        device: DeviceKind::Selected,
    }
}

fn ended(recording: RecordingId, duration_ms: u64, end: RecordingEnd) -> DictationEvent {
    DictationEvent::RecordingEnded {
        recording,
        duration_ms,
        end,
    }
}

fn gate(recording: RecordingId, detector: &'static str, speech: bool) -> DictationEvent {
    DictationEvent::SpeechGate {
        recording,
        detector,
        speech,
    }
}

fn finished(
    seq: u64,
    recording: RecordingId,
    engine: Option<&'static str>,
    stop_to_text_ms: u64,
    outcome: OutcomeCode,
    failure: Option<&'static str>,
    http_status: Option<u16>,
) -> DictationEvent {
    DictationEvent::JobFinished {
        seq,
        recording,
        engine,
        stop_to_text_ms,
        outcome,
        failure,
        http_status,
    }
}

fn delivered(seq: u64, text_to_paste_ms: u64, result: DeliveryResult) -> DictationEvent {
    DictationEvent::Delivered {
        seq,
        text_to_paste_ms,
        result,
    }
}

fn capture_failed(recording: RecordingId, cause: MicCause) -> DictationEvent {
    DictationEvent::CaptureFailed { recording, cause }
}

/// Every `MicCause` with its log literal; a new cause fails to compile here.
fn every_mic_cause() -> Vec<(MicCause, &'static str)> {
    let all = [
        MicCause::NoDevice,
        MicCause::AccessDenied,
        MicCause::Busy,
        MicCause::Other,
    ];
    all.into_iter()
        .map(|c| {
            let literal = match c {
                MicCause::NoDevice => "no_device",
                MicCause::AccessDenied => "access_denied",
                MicCause::Busy => "busy",
                MicCause::Other => "other",
            };
            (c, literal)
        })
        .collect()
}

fn s(v: u64) -> String {
    v.to_string()
}

#[test]
fn recording_id_get_is_the_controller_number() {
    // RecordingId::get() (T-001 Notes, T-008): the number behind the id, the
    // controller's counter from 0. Bite: a constant, a hash, an off-by-one.
    let got: Vec<u64> = ids(3).into_iter().map(RecordingId::get).collect();
    assert_eq!(got, vec![0, 1, 2]);
}

#[test]
fn delivered_dictation_is_one_line_with_engine_and_all_timings() {
    // Acceptance 1: one dictation writes one line with the engine and the FR-20
    // timings (press -> first frame, stop -> text, text -> paste) plus
    // duration_ms. The line is written at Delivered, which follows
    // JobFinished{Text}. Bite: a line per event (analysis option B), the line
    // written at JobFinished (no text_to_paste_ms), a timing under another key,
    // the detector or the result dropped.
    let f = fixture();
    let [a] = ids(1)[..] else { unreachable!() };
    f.feed(&[
        started(a, 41),
        ended(a, 3_000, RecordingEnd::Released),
        gate(a, "energy", true),
        finished(1, a, Some("api"), 812, OutcomeCode::Text, None, None),
    ]);
    assert_eq!(
        f.dictations(),
        Vec::<Line>::new(),
        "no line before the delivery is known"
    );
    f.feed(&[delivered(1, 95, DeliveryResult::Pasted)]);

    let lines = f.dictations();
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert_eq!(lines[0].level, "INFO");
    assert_eq!(
        lines[0].pairs,
        pairs(&[
            ("rec", s(a.get())),
            ("engine", "api".to_string()),
            ("outcome", "delivered".to_string()),
            ("result", "pasted".to_string()),
            ("detector", "energy".to_string()),
            ("press_to_frame_ms", s(41)),
            ("duration_ms", s(3_000)),
            ("stop_to_text_ms", s(812)),
            ("text_to_paste_ms", s(95)),
        ])
    );
    assert_eq!(f.lines().len(), 1, "nothing but the dictation line");
}

#[test]
fn failed_dictation_line_has_the_failure_literal_and_http_status() {
    // A failed job closes its record at JobFinished, WARN, with the failure
    // through the table and the ServerError status. Bite: the CamelCase code
    // echoed, http_status dropped, the line held back waiting for a Delivered
    // that never comes.
    let f = fixture();
    let [a] = ids(1)[..] else { unreachable!() };
    f.feed(&[
        started(a, 37),
        ended(a, 2_500, RecordingEnd::Released),
        gate(a, "silero", true),
        finished(
            1,
            a,
            Some("api"),
            30_001,
            OutcomeCode::Failed,
            Some("ServerError"),
            Some(503),
        ),
    ]);
    let lines = f.dictations();
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert_eq!(lines[0].level, "WARN");
    assert_eq!(
        lines[0].pairs,
        pairs(&[
            ("rec", s(a.get())),
            ("engine", "api".to_string()),
            ("outcome", "failed".to_string()),
            ("failure", "server_error".to_string()),
            ("http_status", s(503)),
            ("detector", "silero".to_string()),
            ("press_to_frame_ms", s(37)),
            ("duration_ms", s(2_500)),
            ("stop_to_text_ms", s(30_001)),
        ])
    );
}

#[test]
fn no_speech_line_has_no_engine_and_closes_at_job_finished() {
    // No speech: no engine was built (engine None), the record closes at
    // JobFinished{NoSpeech}. Bite: engine=other / engine= for None, the line never
    // written (waiting for Delivered).
    let f = fixture();
    let [a] = ids(1)[..] else { unreachable!() };
    f.feed(&[
        started(a, 25),
        ended(a, 1_800, RecordingEnd::Released),
        gate(a, "energy", false),
        finished(2, a, None, 4, OutcomeCode::NoSpeech, None, None),
    ]);
    let lines = f.dictations();
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert_eq!(
        lines[0].pairs,
        pairs(&[
            ("rec", s(a.get())),
            ("outcome", "no_speech".to_string()),
            ("detector", "energy".to_string()),
            ("press_to_frame_ms", s(25)),
            ("duration_ms", s(1_800)),
            ("stop_to_text_ms", s(4)),
        ])
    );
}

#[test]
fn too_short_recording_is_one_line_at_recording_ended() {
    // A discarded hold (< 300 ms) has no job: RecordingEnded{TooShort} closes it.
    // Bite: no line for a too-short recording (analysis option C's gap), a line
    // without its duration.
    let f = fixture();
    let [a] = ids(1)[..] else { unreachable!() };
    f.feed(&[started(a, 30), ended(a, 120, RecordingEnd::TooShort)]);
    let lines = f.dictations();
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert_eq!(lines[0].level, "INFO");
    assert_eq!(
        lines[0].pairs,
        pairs(&[
            ("rec", s(a.get())),
            ("outcome", "too_short".to_string()),
            ("press_to_frame_ms", s(30)),
            ("duration_ms", s(120)),
        ])
    );
}

#[test]
fn a_cancelled_recording_is_one_info_line_with_outcome_cancelled() {
    // T-009 (FR-22, FR-20): an Esc-cancelled recording has no job, so
    // RecordingEnded{Cancelled} closes its record at once: one INFO line with
    // outcome=cancelled, the press timing and the duration, nothing else. Bite: no
    // line (the record waits for a job that never comes, then is evicted), the
    // outcome spelled as too_short or Debug-formatted ("Cancelled"), the line at
    // WARN.
    let f = fixture();
    let [a] = ids(1)[..] else { unreachable!() };
    f.feed(&[started(a, 35), ended(a, 2_000, RecordingEnd::Cancelled)]);
    let lines = f.dictations();
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert_eq!(lines[0].level, "INFO");
    assert_eq!(
        lines[0].pairs,
        pairs(&[
            ("rec", s(a.get())),
            ("outcome", "cancelled".to_string()),
            ("press_to_frame_ms", s(35)),
            ("duration_ms", s(2_000)),
        ])
    );
    assert_eq!(f.lines().len(), 1, "nothing but the dictation line");
}

#[test]
fn toggled_and_max_length_recordings_are_closed_by_their_job() {
    // T-009: a toggle stop and a max-length stop queue a job like a release does, so
    // RecordingEnded{Toggled | MaxLength} writes no line; the record closes at the
    // delivery with the recording's duration. Bite: Toggled or MaxLength closing the
    // record at RecordingEnded (two lines per dictation, or a line without the job's
    // timings).
    let f = fixture();
    let [a, b] = ids(2)[..] else { unreachable!() };
    f.feed(&[
        started(a, 41),
        ended(a, 10_000, RecordingEnd::Toggled),
        started(b, 42),
        ended(b, 600_000, RecordingEnd::MaxLength),
    ]);
    assert_eq!(f.dictations(), Vec::<Line>::new(), "a line before the job");
    f.feed(&[
        gate(a, "energy", true),
        finished(1, a, Some("api"), 812, OutcomeCode::Text, None, None),
        delivered(1, 95, DeliveryResult::Pasted),
        gate(b, "energy", true),
        finished(2, b, Some("api"), 900, OutcomeCode::Text, None, None),
        delivered(2, 90, DeliveryResult::Pasted),
    ]);
    let lines = f.dictations();
    assert_eq!(lines.len(), 2, "{lines:#?}");
    for (line, (rec, frame, duration, text, paste)) in lines
        .iter()
        .zip([(a, 41, 10_000, 812, 95), (b, 42, 600_000, 900, 90)])
    {
        assert_eq!(line.level, "INFO");
        assert_eq!(
            line.pairs,
            pairs(&[
                ("rec", s(rec.get())),
                ("engine", "api".to_string()),
                ("outcome", "delivered".to_string()),
                ("result", "pasted".to_string()),
                ("detector", "energy".to_string()),
                ("press_to_frame_ms", s(frame)),
                ("duration_ms", s(duration)),
                ("stop_to_text_ms", s(text)),
                ("text_to_paste_ms", s(paste)),
            ])
        );
    }
}

#[test]
fn a_capture_failure_at_press_is_one_warn_line_with_the_mic_literal() {
    // T-051: a capture that cannot open at press emits only CaptureFailed (no
    // RecordingStarted / RecordingEnded, no job), so CaptureFailed itself closes
    // the recording's line: outcome=capture_failed and the cause through the
    // closed microphone table, WARN. Bite: no line for a failed press (the record
    // waits for a JobFinished that never comes), the cause Debug-formatted
    // ("AccessDenied"), a second line (a warning beside the dictation line), the
    // line at INFO.
    let f = fixture();
    let causes = every_mic_cause();
    let rec = ids(causes.len());
    for ((cause, _), id) in causes.iter().zip(&rec) {
        f.feed(&[capture_failed(*id, *cause)]);
    }
    assert_eq!(f.lines().len(), causes.len(), "{:#?}", f.lines());
    let lines = f.dictations();
    assert_eq!(lines.len(), causes.len(), "{lines:#?}");
    for (line, ((cause, literal), id)) in lines.iter().zip(causes.iter().zip(&rec)) {
        assert_eq!(line.level, "WARN", "{cause:?}: {}", line.raw);
        assert_eq!(
            line.pairs,
            pairs(&[
                ("rec", s(id.get())),
                ("outcome", "capture_failed".to_string()),
                ("mic", literal.to_string()),
            ]),
            "{cause:?}: {}",
            line.raw
        );
    }
}

#[test]
fn a_capture_failure_at_stop_closes_the_open_record_in_one_line() {
    // T-051: a capture that fails at stop comes after the release's
    // RecordingStarted (only if a frame arrived) and RecordingEnded{Released},
    // and no job follows. CaptureFailed closes that recording's open record: one
    // line with its own timings, joined by RecordingId (b has no first frame and
    // ends between a's events). Bite: a line built from an empty record (timings
    // lost), the line joined to the last RecordingStarted, no line (the record
    // waits for a JobFinished), the record left open after its line (a later
    // event of that id would carry its timings again).
    let f = fixture();
    let [a, b] = ids(2)[..] else { unreachable!() };
    f.feed(&[
        started(a, 41),
        ended(b, 1_500, RecordingEnd::Released),
        ended(a, 2_000, RecordingEnd::Released),
        capture_failed(a, MicCause::Busy),
        capture_failed(b, MicCause::Other),
    ]);
    assert_eq!(f.lines().len(), 2, "{:#?}", f.lines());
    let lines = f.dictations();
    assert_eq!(lines.len(), 2, "{lines:#?}");
    assert_eq!(lines[0].level, "WARN", "{}", lines[0].raw);
    assert_eq!(
        lines[0].pairs,
        pairs(&[
            ("rec", s(a.get())),
            ("outcome", "capture_failed".to_string()),
            ("mic", "busy".to_string()),
            ("press_to_frame_ms", s(41)),
            ("duration_ms", s(2_000)),
        ])
    );
    assert_eq!(
        lines[1].pairs,
        pairs(&[
            ("rec", s(b.get())),
            ("outcome", "capture_failed".to_string()),
            ("mic", "other".to_string()),
            ("duration_ms", s(1_500)),
        ])
    );

    // Probe: a stray event of a after its line starts from an empty record.
    f.feed(&[finished(9, a, None, 5, OutcomeCode::NoSpeech, None, None)]);
    let lines = f.dictations();
    assert_eq!(lines.len(), 3, "{lines:#?}");
    assert_eq!(
        lines[2].pairs,
        pairs(&[
            ("rec", s(a.get())),
            ("outcome", "no_speech".to_string()),
            ("stop_to_text_ms", s(5)),
        ]),
        "a's record stayed open after its capture_failed line"
    );
}

#[test]
fn interleaved_jobs_are_joined_by_recording_id() {
    // T-011 runs process() concurrently, so A's and B's events interleave and B
    // can finish first. Each line must carry its own recording's timings. Bite: a
    // join by event order ("the last RecordingStarted"), Delivered joined to the
    // last JobFinished instead of its seq, one record for two recordings.
    let f = fixture();
    let [a, b] = ids(2)[..] else { unreachable!() };
    f.feed(&[
        started(a, 11),
        ended(a, 1_100, RecordingEnd::Released),
        started(b, 22),
        ended(b, 2_200, RecordingEnd::Released),
        gate(b, "energy", true),
        gate(a, "silero", true),
        finished(2, b, Some("api"), 222, OutcomeCode::Text, None, None),
        finished(
            1,
            a,
            Some("api"),
            111,
            OutcomeCode::Failed,
            Some("Timeout"),
            None,
        ),
        delivered(2, 33, DeliveryResult::CopiedOnly),
    ]);
    let lines = f.dictations();
    assert_eq!(lines.len(), 2, "{lines:#?}");
    let by_rec = |id: RecordingId| {
        lines
            .iter()
            .find(|l| l.get("rec") == Some(s(id.get()).as_str()))
            .unwrap_or_else(|| panic!("no line for {id:?}: {lines:#?}"))
            .pairs
            .clone()
    };
    assert_eq!(
        by_rec(a),
        pairs(&[
            ("rec", s(a.get())),
            ("engine", "api".to_string()),
            ("outcome", "failed".to_string()),
            ("failure", "timeout".to_string()),
            ("detector", "silero".to_string()),
            ("press_to_frame_ms", s(11)),
            ("duration_ms", s(1_100)),
            ("stop_to_text_ms", s(111)),
        ])
    );
    assert_eq!(
        by_rec(b),
        pairs(&[
            ("rec", s(b.get())),
            ("engine", "api".to_string()),
            ("outcome", "delivered".to_string()),
            ("result", "copied_only".to_string()),
            ("detector", "energy".to_string()),
            ("press_to_frame_ms", s(22)),
            ("duration_ms", s(2_200)),
            ("stop_to_text_ms", s(222)),
            ("text_to_paste_ms", s(33)),
        ])
    );
}

#[test]
fn delivered_is_joined_to_its_job_by_seq() {
    // Delivered carries only its job's seq (events.rs): several text jobs can be
    // open when a Delivered arrives, once T-011 queues release. Each delivery
    // belongs to the job with its seq; the middle one is delivered first. Bite:
    // Delivered closing the most recent or the oldest open text job instead of
    // the one with its seq.
    let f = fixture();
    let [a, b, c] = ids(3)[..] else {
        unreachable!()
    };
    f.feed(&[
        finished(1, a, Some("api"), 101, OutcomeCode::Text, None, None),
        finished(2, b, Some("api"), 202, OutcomeCode::Text, None, None),
        finished(3, c, Some("api"), 303, OutcomeCode::Text, None, None),
        delivered(2, 22, DeliveryResult::CopyManual),
    ]);
    let lines = f.dictations();
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert_eq!(lines[0].get("rec"), Some(s(b.get()).as_str()), "{lines:#?}");
    assert_eq!(lines[0].get("result"), Some("copy_manual"));
    assert_eq!(lines[0].get("text_to_paste_ms"), Some("22"));
    assert_eq!(lines[0].get("stop_to_text_ms"), Some("202"));

    f.feed(&[
        delivered(3, 33, DeliveryResult::Pasted),
        delivered(1, 11, DeliveryResult::CopiedOnly),
    ]);
    let lines = f.dictations();
    assert_eq!(lines.len(), 3, "{lines:#?}");
    for (line, (id, result, paste, stop)) in lines[1..]
        .iter()
        .zip([(c, "pasted", "33", "303"), (a, "copied_only", "11", "101")])
    {
        assert_eq!(line.get("rec"), Some(s(id.get()).as_str()), "{lines:#?}");
        assert_eq!(line.get("result"), Some(result), "{}", line.raw);
        assert_eq!(line.get("text_to_paste_ms"), Some(paste), "{}", line.raw);
        assert_eq!(line.get("stop_to_text_ms"), Some(stop), "{}", line.raw);
    }
}

#[test]
fn a_job_without_recording_events_still_gets_its_line() {
    // Until T-006 emits RecordingStarted/Ended (and for T-007's retry from the
    // pending slot) a job's events are all there is: the line has what came and
    // omits the rest. Bite: a job dropped because no RecordingStarted opened a
    // record, absent timings written as 0.
    let f = fixture();
    let [a] = ids(1)[..] else { unreachable!() };
    f.feed(&[finished(
        5,
        a,
        Some("api"),
        640,
        OutcomeCode::Failed,
        Some("InvalidApiKey"),
        None,
    )]);
    let lines = f.dictations();
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert_eq!(
        lines[0].pairs,
        pairs(&[
            ("rec", s(a.get())),
            ("engine", "api".to_string()),
            ("outcome", "failed".to_string()),
            ("failure", "invalid_api_key".to_string()),
            ("stop_to_text_ms", s(640)),
        ])
    );
}

/// The documented bound on open records and on text jobs awaiting `Delivered`
/// (`observer.rs` `MAX_OPEN`, docs/decisions/diagnostics-log.md).
const OPEN_BOUND: usize = 64;
/// Recordings opened past the bound in the two tests below.
const PAST_BOUND: usize = 36;

#[test]
fn open_records_are_bounded_and_the_oldest_lose_their_timings_without_a_line() {
    // The open-record map is bounded (64): a record that never closes (a job that
    // never runs, a press without a later event) must not grow memory for the
    // life of the tray process. 100 recordings start and none closes; the oldest
    // 36 are dropped without a line. Each recording is then closed by its job,
    // newest first (a closed record never re-opens one that is still held), so
    // every line shows whether its record survived: exactly the 64 newest keep
    // press_to_frame_ms (the map held 64, never more and never fewer), the 36
    // oldest have only what their job carries (a_job_without_recording_events_
    // still_gets_its_line). Bite: no eviction (all 100 keep their timing), a
    // bound other than 64, the newest evicted instead of the oldest, a line
    // written at eviction.
    let f = fixture();
    let rec = ids(OPEN_BOUND + PAST_BOUND);
    for (i, id) in rec.iter().enumerate() {
        f.obs.event(&started(*id, 1_000 + i as u64));
    }
    assert_eq!(f.lines(), Vec::<String>::new(), "eviction writes no line");

    for (i, id) in rec.iter().enumerate().rev() {
        f.obs.event(&finished(
            i as u64 + 1,
            *id,
            None,
            7,
            OutcomeCode::NoSpeech,
            None,
            None,
        ));
    }
    let lines = f.dictations();
    assert_eq!(
        lines.len(),
        rec.len(),
        "one line per closed job: {lines:#?}"
    );
    let kept: Vec<u64> = lines
        .iter()
        .filter(|l| l.get("press_to_frame_ms").is_some())
        .filter_map(|l| l.get("rec").and_then(|r| r.parse().ok()))
        .collect();
    assert_eq!(kept.len(), OPEN_BOUND, "records held at once: {kept:?}");
    for (i, id) in rec.iter().enumerate() {
        let line = lines
            .iter()
            .find(|l| l.get("rec") == Some(s(id.get()).as_str()))
            .unwrap_or_else(|| panic!("no line for {id:?}"));
        if i < PAST_BOUND {
            assert_eq!(
                line.pairs,
                pairs(&[
                    ("rec", s(id.get())),
                    ("outcome", "no_speech".to_string()),
                    ("stop_to_text_ms", s(7)),
                ]),
                "recording {i} is among the oldest and was evicted"
            );
        } else {
            assert_eq!(
                line.pairs,
                pairs(&[
                    ("rec", s(id.get())),
                    ("outcome", "no_speech".to_string()),
                    ("press_to_frame_ms", s(1_000 + i as u64)),
                    ("stop_to_text_ms", s(7)),
                ]),
                "recording {i} is among the {OPEN_BOUND} newest and kept its record"
            );
        }
    }
}

#[test]
fn text_jobs_awaiting_delivery_are_bounded_and_the_oldest_get_no_line() {
    // The map of text jobs waiting for their Delivered is bounded (64): a
    // Delivered that never comes must not grow memory. 100 text jobs finish and
    // none is delivered; then every seq is delivered, newest first. Exactly the
    // 64 newest get their line, with their own record (engine, stop_to_text_ms);
    // the 36 oldest were dropped without a line, and their late Delivered writes
    // none. Bite: no eviction in the awaiting map (100 lines), a bound other than
    // 64, the newest evicted instead of the oldest, a line written at eviction.
    let f = fixture();
    let rec = ids(OPEN_BOUND + PAST_BOUND);
    for (i, id) in rec.iter().enumerate() {
        f.obs.event(&finished(
            i as u64 + 1,
            *id,
            Some("api"),
            500 + i as u64,
            OutcomeCode::Text,
            None,
            None,
        ));
    }
    assert_eq!(f.lines(), Vec::<String>::new(), "eviction writes no line");

    for i in (0..rec.len()).rev() {
        f.obs
            .event(&delivered(i as u64 + 1, 9, DeliveryResult::Pasted));
    }
    let lines = f.dictations();
    assert_eq!(
        lines.len(),
        OPEN_BOUND,
        "only the {OPEN_BOUND} newest text jobs were still awaiting delivery: {lines:#?}"
    );
    for (line, i) in lines.iter().zip((PAST_BOUND..rec.len()).rev()) {
        let id = rec[i];
        assert_eq!(
            line.pairs,
            pairs(&[
                ("rec", s(id.get())),
                ("engine", "api".to_string()),
                ("outcome", "delivered".to_string()),
                ("result", "pasted".to_string()),
                ("stop_to_text_ms", s(500 + i as u64)),
                ("text_to_paste_ms", s(9)),
            ]),
            "seq {} (the newest are delivered first)",
            i + 1
        );
    }
}

#[test]
fn a_pipeline_warning_is_its_own_line_at_once() {
    // Warning events (vad_fallback, esc_unavailable, toast_failed) are not part
    // of a dictation record. Bite: a warning dropped, folded into the next
    // dictation line, or held until a record closes.
    let f = fixture();
    let codes = [
        (WarningCode::VadFallback, "vad_fallback"),
        (WarningCode::EscUnavailable, "esc_unavailable"),
        (WarningCode::ToastFailed, "toast_failed"),
    ];
    for (code, _) in codes {
        match code {
            WarningCode::VadFallback | WarningCode::EscUnavailable | WarningCode::ToastFailed => {}
        }
        f.obs.event(&DictationEvent::Warning { code });
    }
    let lines: Vec<Line> = f.lines().iter().map(|l| closed(l)).collect();
    assert_eq!(lines.len(), 3, "{lines:#?}");
    for (line, (_, literal)) in lines.iter().zip(codes) {
        assert_eq!(line.head, "warning");
        assert_eq!(line.level, "WARN");
        assert_eq!(line.pairs, pairs(&[("kind", literal.to_string())]));
    }
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

#[test]
fn every_dictation_event_with_planted_strings_gives_closed_lines() {
    // Analysis approach 1 through the observer: every DictationEvent variant and
    // every value of its closed fields, with Box::leak'ed strings planted where
    // the event carries an outside &'static str (SpeechGate.detector,
    // JobFinished.engine, JobFinished.failure). Every line passes the grammar and
    // the closed sets and no planted byte reaches the file. Bite: an echoed
    // detector / engine / failure, a panic on an unknown seq or a repeated event.
    let f = fixture();
    let planted = [
        leak("PLANTED detector sk-test-PLANTED\nINJECT"),
        leak("planted_lower_snake"),
        leak("energy\r\nPLANTED=1"),
    ];
    let rec = ids(48);
    let mut next = rec.iter().copied();
    let mut take = || next.next().expect("enough ids");
    let mut planted_recs = BTreeSet::new();
    let mut seq = 0u64;
    let mut emitted: Vec<DictationEvent> = Vec::new();

    for p in planted {
        for (outcome, failure, http) in [
            (OutcomeCode::Text, None, None),
            (OutcomeCode::NoSpeech, None, None),
            (OutcomeCode::Failed, Some(p), Some(500)),
            (OutcomeCode::Failed, Some(p), None),
        ] {
            for device in [DeviceKind::Selected, DeviceKind::Fallback] {
                let id = take();
                planted_recs.insert(id.get().to_string());
                seq += 1;
                emitted.extend([
                    DictationEvent::RecordingStarted {
                        recording: id,
                        hotkey_to_first_frame_ms: 7,
                        device,
                    },
                    ended(id, 900, RecordingEnd::Released),
                    gate(id, p, outcome != OutcomeCode::NoSpeech),
                    finished(seq, id, Some(p), 70, outcome, failure, http),
                ]);
                if outcome == OutcomeCode::Text {
                    emitted.push(delivered(seq, 9, DeliveryResult::CopyManual));
                }
            }
        }
        let short = take();
        emitted.extend([started(short, 3), ended(short, 50, RecordingEnd::TooShort)]);
    }
    for result in [
        DeliveryResult::Pasted,
        DeliveryResult::CopiedOnly,
        DeliveryResult::CopyManual,
    ] {
        let id = take();
        seq += 1;
        emitted.extend([
            gate(id, "energy", true),
            finished(seq, id, Some("api"), 70, OutcomeCode::Text, None, None),
            delivered(seq, 9, result),
        ]);
    }
    // Every capture failure (T-051), at press (CaptureFailed alone) and at stop
    // (after RecordingStarted / RecordingEnded{Released}).
    let mut capture_recs = BTreeSet::new();
    for (cause, _) in every_mic_cause() {
        let at_press = take();
        let at_stop = take();
        capture_recs.extend([at_press.get().to_string(), at_stop.get().to_string()]);
        emitted.extend([
            capture_failed(at_press, cause),
            started(at_stop, 8),
            ended(at_stop, 800, RecordingEnd::Released),
            capture_failed(at_stop, cause),
        ]);
    }
    // A Delivered with no JobFinished, a JobFinished repeated for a closed
    // record, a recording that never ends, and every warning.
    let first = rec[0];
    let lonely = take();
    emitted.extend([
        delivered(9_999, 1, DeliveryResult::Pasted),
        finished(
            1,
            first,
            Some(planted[0]),
            70,
            OutcomeCode::Failed,
            Some(planted[0]),
            None,
        ),
        started(lonely, 5),
        DictationEvent::Warning {
            code: WarningCode::VadFallback,
        },
        DictationEvent::Warning {
            code: WarningCode::EscUnavailable,
        },
        DictationEvent::Warning {
            code: WarningCode::ToastFailed,
        },
        DictationEvent::PressBlocked {
            reason: Blocked::NoEngine,
        },
    ]);

    let mut seen = BTreeSet::new();
    for e in &emitted {
        seen.insert(match e {
            DictationEvent::RecordingStarted { .. } => 0,
            DictationEvent::RecordingEnded { .. } => 1,
            DictationEvent::SpeechGate { .. } => 2,
            DictationEvent::JobFinished { .. } => 3,
            DictationEvent::Delivered { .. } => 4,
            DictationEvent::Warning { .. } => 5,
            DictationEvent::CaptureFailed { .. } => 6,
            DictationEvent::PressBlocked { .. } => 7,
        });
        f.obs.event(e);
    }
    assert_eq!(seen.len(), 8, "every DictationEvent variant was fed");

    // Every line passes the grammar and the closed sets (dictation_lines checks
    // each one on the way).
    let dictations = dictation_lines(&f.lines());
    // 3 planted x 4 outcomes x 2 devices, 3 too-short, 3 results, 4 causes x
    // (press, stop).
    assert!(
        dictations.len() >= 3 * 4 * 2 + 3 + 3 + 4 * 2,
        "{} dictation lines",
        dictations.len()
    );
    // One capture_failed line per capture failure, each with its mic literal.
    let capture_lines: Vec<&Line> = dictations
        .iter()
        .filter(|l| l.get("rec").is_some_and(|r| capture_recs.contains(r)))
        .collect();
    assert_eq!(capture_lines.len(), 4 * 2, "{capture_lines:#?}");
    for l in &capture_lines {
        assert_eq!(l.get("outcome"), Some("capture_failed"), "{}", l.raw);
        assert!(l.get("mic").is_some(), "{}", l.raw);
    }
    let mut checked = 0;
    for l in &dictations {
        if l.get("rec").is_some_and(|r| planted_recs.contains(r)) {
            for key in ["engine", "detector", "failure"] {
                if let Some(v) = l.get(key) {
                    checked += 1;
                    assert_eq!(v, "other", "{key} of a planted dictation: {}", l.raw);
                }
            }
        }
    }
    assert!(
        checked >= 3 * 2 * 4,
        "planted values were checked: {checked}"
    );
    let bytes = std::fs::read(f.dir.join(diag_support::ACTIVE)).expect("log written");
    for marker in ["PLANTED", "planted", "INJECT", "sk-test"] {
        assert!(
            !diag_support::contains(&bytes, marker.as_bytes()),
            "{marker} reached the log"
        );
    }
}

/// Every `FailureReason` with its log literal; a new reason fails to compile here.
fn every_reason() -> Vec<(FailureReason, &'static str)> {
    let all = vec![
        FailureReason::InvalidApiKey,
        FailureReason::NetworkUnavailable,
        FailureReason::CannotReach {
            host: "api.example.com:8443".to_string(),
        },
        FailureReason::Timeout,
        FailureReason::ServerError { status: 503 },
        FailureReason::UnexpectedResponse,
        FailureReason::KeyStoreUnavailable,
        FailureReason::EngineNotConfigured,
        FailureReason::ClipboardUnavailable,
        FailureReason::MicrophoneUnavailable {
            cause: MicCause::Busy,
        },
    ];
    all.into_iter()
        .map(|r| {
            let literal = match &r {
                FailureReason::InvalidApiKey => "invalid_api_key",
                FailureReason::NetworkUnavailable => "network_unavailable",
                FailureReason::CannotReach { .. } => "cannot_reach",
                FailureReason::Timeout => "timeout",
                FailureReason::ServerError { .. } => "server_error",
                FailureReason::UnexpectedResponse => "unexpected_response",
                FailureReason::KeyStoreUnavailable => "key_store_unavailable",
                FailureReason::EngineNotConfigured => "engine_not_configured",
                FailureReason::ClipboardUnavailable => "clipboard_unavailable",
                FailureReason::MicrophoneUnavailable { .. } => "microphone_unavailable",
            };
            (r, literal)
        })
        .collect()
}

#[test]
fn every_failure_code_reaches_the_line_as_its_literal() {
    // Analysis approach 1, end to end: JobFinished.failure = FailureReason::code()
    // -> failure=<own literal>, never other, for every reason. Bite: a code missing
    // from the observer's table, the code echoed.
    let f = fixture();
    let reasons = every_reason();
    let rec = ids(reasons.len());
    for (i, ((reason, _), id)) in reasons.iter().zip(&rec).enumerate() {
        f.obs.event(&finished(
            i as u64 + 1,
            *id,
            Some("api"),
            10,
            OutcomeCode::Failed,
            Some(reason.code()),
            None,
        ));
    }
    let lines = f.dictations();
    assert_eq!(lines.len(), reasons.len(), "{lines:#?}");
    for ((reason, literal), id) in reasons.iter().zip(&rec) {
        let line = lines
            .iter()
            .find(|l| l.get("rec") == Some(s(id.get()).as_str()))
            .unwrap_or_else(|| panic!("no line for {reason:?}"));
        assert_eq!(
            line.get("failure"),
            Some(*literal),
            "{reason:?}: {}",
            line.raw
        );
    }
}
