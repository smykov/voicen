//! T-006 Refresh 5, red test 1 (observer half): a press the dictation gate blocks
//! is one `DictationEvent::PressBlocked { reason }` (`reason` is the closed
//! `settings::gate::Blocked`, so no text can ride on it, FR-20), and the release
//! `diag::LogObserver` turns each one into exactly one line at once:
//!
//! ```text
//! <ts> WARN dictation outcome=blocked reason=no_engine
//! ```
//!
//! WARN like the other dictations that did not happen (`failed`,
//! `capture_failed`); no `rec=`, because a blocked press is not a recording and
//! takes no `RecordingId`; `reason` through a closed table (one literal per
//! `Blocked` variant). Kept in its own binary so the other diag tests still build
//! while the variant does not exist (this file fails to compile until it does).
//! The session half (one event per blocked press, no line at its release, rec
//! numbering unchanged) is in `tests/dictation_session.rs`.

mod common;
mod diag_support;

use std::sync::Arc;
use std::time::Duration;

use common::timing::now;

use diag_support::{active_lines, closed, open_log, NOON_UTC};
use voicen_core::diag::{LogConfig, LogObserver};
use voicen_core::events::{DeviceKind, DictationEvent, PipelineObserver};
use voicen_core::recording::{Press, RecordingController, RecordingEnd};
use voicen_core::settings::gate::Blocked;
use voicen_core::test_support::TempDir;

const BLOCKED_LINE: &str =
    "2026-10-04T12:00:00.000+00:00 WARN dictation outcome=blocked reason=no_engine";

fn blocked() -> DictationEvent {
    DictationEvent::PressBlocked {
        reason: Blocked::NoEngine,
    }
}

/// Every `Blocked` with its log literal; a new reason fails to compile here.
fn every_reason() -> Vec<(Blocked, &'static str)> {
    let all = vec![(Blocked::NoEngine, "no_engine")];
    for (b, _) in &all {
        match b {
            Blocked::NoEngine => {}
        }
    }
    all
}

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
    Fixture {
        obs: LogObserver::new(Arc::clone(&log)),
        dir,
        _tmp: tmp,
    }
}

impl Fixture {
    fn lines(&self) -> Vec<String> {
        if self.dir.join(diag_support::ACTIVE).exists() {
            active_lines(&self.dir)
        } else {
            Vec::new()
        }
    }
}

#[test]
fn each_blocked_press_is_exactly_one_warn_line_with_its_reason_and_no_rec() {
    // Bite: no line for PressBlocked (mapped to None), a line with rec=0 (a
    // recording id invented for it), the reason Debug-formatted ("NoEngine"),
    // INFO instead of WARN, one line for two presses (deduplicated), a second
    // line (a warning beside the dictation line).
    let f = fixture();
    f.obs.event(&blocked());
    assert_eq!(f.lines(), vec![BLOCKED_LINE.to_string()]);
    f.obs.event(&blocked());
    assert_eq!(f.lines(), vec![BLOCKED_LINE.to_string(); 2]);
    for raw in f.lines() {
        // The grammar and the closed sets of diag_support (blocked: only
        // outcome= and reason=).
        let line = closed(&raw);
        assert_eq!(line.head, "dictation", "{raw}");
    }
}

#[test]
fn every_block_reason_reaches_the_line_as_its_literal() {
    // One literal per Blocked variant, in the closed set. Bite: a reason
    // written as `other` or Debug text.
    let f = fixture();
    let reasons = every_reason();
    for (b, _) in &reasons {
        f.obs.event(&DictationEvent::PressBlocked { reason: *b });
    }
    let lines = f.lines();
    assert_eq!(lines.len(), reasons.len(), "{lines:#?}");
    for (raw, (b, literal)) in lines.iter().zip(&reasons) {
        let line = closed(raw);
        assert_eq!(line.get("reason"), Some(*literal), "{b:?}: {raw}");
        assert_eq!(line.get("outcome"), Some("blocked"), "{b:?}: {raw}");
        assert_eq!(line.get("rec"), None, "{b:?}: {raw}");
    }
}

#[test]
fn a_blocked_press_leaves_an_open_recording_untouched() {
    // A blocked event during an open record (it cannot happen from the session
    // today, but the observer joins by RecordingId, never by event order): the
    // blocked line comes at once, and the recording still closes with its own
    // timings. Bite: PressBlocked closing or resetting the newest open record,
    // or held back until the next close.
    let mut ctrl = RecordingController::<()>::new();
    let t0 = now();
    let id = match ctrl.press(t0, ()) {
        Press::Start(id) => id,
        Press::Ignored => panic!("premise: the press starts a recording"),
    };
    let _ = ctrl.release(t0 + Duration::from_millis(10));
    let f = fixture();
    f.obs.event(&DictationEvent::RecordingStarted {
        recording: id,
        hotkey_to_first_frame_ms: 30,
        device: DeviceKind::Selected,
    });
    f.obs.event(&blocked());
    assert_eq!(
        f.lines(),
        vec![BLOCKED_LINE.to_string()],
        "not written at once"
    );
    f.obs.event(&DictationEvent::RecordingEnded {
        recording: id,
        duration_ms: 120,
        end: RecordingEnd::TooShort,
    });
    let lines = f.lines();
    assert_eq!(lines.len(), 2, "{lines:#?}");
    assert_eq!(lines[0], BLOCKED_LINE);
    let rec = closed(&lines[1]);
    assert_eq!(
        rec.get("rec"),
        Some(id.get().to_string().as_str()),
        "{}",
        rec.raw
    );
    assert_eq!(rec.get("outcome"), Some("too_short"), "{}", rec.raw);
    assert_eq!(rec.get("press_to_frame_ms"), Some("30"), "{}", rec.raw);
    assert_eq!(rec.get("duration_ms"), Some("120"), "{}", rec.raw);
}
