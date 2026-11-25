//! T-266 (F-036 draft; docs/tasks/T-266.md): the WAV harness of the shell call tests feeds the
//! two call tracks in stamp order. `WavSource::pump_for` and `WavSource::pump` step the opened
//! tracks together, so the latest stamp fed to one track never leads the other's by more than one
//! block (20 ms). That is below the writer's 200 ms hold-off: after a wall-clock pause longer
//! than its STALL, `Writer::advance` can then never zero-fill a live track, and the recorded
//! durations depend only on the pumped stamps (the flake of `call_dictation`, CI 45465360769).
//! Windows CI only (decision #5); not compiled on the Linux gate.
//!
//! Red today: the pump feeds the whole span of the system track (opened first, as
//! `Opener::open_tracks` does), then the whole span of the mic, so the system leads by the span.
//!
//! Deterministic: no thread, no wall clock, no sleep; a recording sink takes the blocks on the
//! test thread, in the order the pump pushes them. The data is the harness's own tone.
#![cfg(windows)]

mod call_shell_support;
mod timing_support;

use std::sync::{Arc, Mutex, PoisonError};

use call_shell_support::{WavSource, PUMP_MS, START_MS};
use voicen_core::call::{CaptureStamp, LossCause, Track, TrackDevice, TrackSink, TrackSource};

/// One block of audio is 20 ms (`WavSource::open`: `with_block_ms(20)`).
const BLOCK_MS: u64 = 20;

/// Every block the pump pushed, in push order: the track and its stamp in ms.
#[derive(Default)]
struct Recording {
    pushed: Mutex<Vec<(Track, u64)>>,
    lost: Mutex<usize>,
}

impl TrackSink for Recording {
    fn block(&self, track: Track, _data: &[f32], _rate: u32, _channels: u16, at: CaptureStamp) {
        self.pushed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((track, at.ns() / 1_000_000));
    }

    fn lost(&self, _track: Track, _cause: LossCause, _at: CaptureStamp) {
        *self.lost.lock().unwrap_or_else(PoisonError::into_inner) += 1;
    }
}

impl Recording {
    fn pushed(&self) -> Vec<(Track, u64)> {
        self.pushed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// A source with its tracks opened in the order `Opener::open_tracks` uses (system, then mic),
/// both on one recording sink.
fn opened() -> (Arc<WavSource>, Arc<Recording>) {
    let source = WavSource::new();
    let sink = Arc::new(Recording::default());
    for (track, device) in [
        (Track::System, TrackDevice::DefaultOutputLoopback),
        (Track::Mic, TrackDevice::DefaultInput),
    ] {
        let sink: Arc<dyn TrackSink> = sink.clone();
        source.open(track, device, sink).expect("the WAV open");
    }
    assert_eq!(source.opens(), 2, "premise: two tracks opened");
    (source, sink)
}

/// The invariant: at every pushed block, the latest stamp of one track leads the other's by at
/// most one block; a track not yet fed counts as at the start stamp.
#[track_caller]
fn assert_stamp_interleaved(pushed: &[(Track, u64)], what: &str) {
    let (mut system, mut mic) = (START_MS, START_MS);
    for (i, &(track, at)) in pushed.iter().enumerate() {
        match track {
            Track::System => system = at,
            Track::Mic => mic = at,
        }
        let lead = system.abs_diff(mic);
        assert!(
            lead <= BLOCK_MS,
            "{what}: after block #{i} ({track:?} at {at} ms) the {} track leads by {lead} ms \
             (system at {system}, mic at {mic}); a lead of 200 ms or more lets the writer \
             zero-fill the lagging live track",
            if system > mic { "system" } else { "mic" }
        );
    }
}

/// Both tracks got exactly the blocks due by `START_MS + ms`, in stamp order per track.
#[track_caller]
fn assert_complete(pushed: &[(Track, u64)], ms: u64, what: &str) {
    for track in [Track::System, Track::Mic] {
        let stamps: Vec<u64> = pushed
            .iter()
            .filter(|(t, _)| *t == track)
            .map(|&(_, at)| at)
            .collect();
        let want: Vec<u64> = (0..=ms / BLOCK_MS)
            .map(|k| START_MS + k * BLOCK_MS)
            .collect();
        assert_eq!(stamps, want, "{what}: the {track:?} blocks pushed");
    }
}

#[test]
fn pump_for_feeds_the_two_tracks_in_stamp_order() {
    // Guarantee: the lead of one track over the other is at most one block, across the three
    // cumulative pumps of `call_dictation` (1000, 1500 and PUMP_MS), and nothing is dropped or
    // doubled. Bite: a pump that feeds each opened track's whole span in turn (the defect: the
    // system leads by the span); a pump that steps by more than one block per track.
    let (source, sink) = opened();
    for ms in [1_000, 1_500, PUMP_MS] {
        source.pump_for(ms);
        assert_stamp_interleaved(&sink.pushed(), &format!("after pump_for({ms})"));
    }
    assert_complete(&sink.pushed(), PUMP_MS, "pump_for");
}

#[test]
fn pump_feeds_the_two_tracks_in_stamp_order() {
    // Guarantee: the same for `pump` (the feed of call_chain, call_hotkey, call_ipc, call_launch,
    // call_lifecycle, call_tray). Bite: interleaving only in `pump_for`.
    let (source, sink) = opened();
    source.pump();
    assert_stamp_interleaved(&sink.pushed(), "pump");
    assert_complete(&sink.pushed(), PUMP_MS, "pump");
}

#[test]
fn a_single_long_pump_for_is_interleaved_from_the_first_block() {
    // Guarantee: the order holds for one pump over the whole span too (the lead is checked at
    // every block, not at the end of a pump), and the lost-callback path is untouched: the feed
    // pushes no `lost`. Bite: a pump that interleaves only the tail.
    let (source, sink) = opened();
    source.pump_for(PUMP_MS);
    assert_stamp_interleaved(&sink.pushed(), "pump_for(PUMP_MS)");
    assert_eq!(
        *sink.lost.lock().unwrap_or_else(PoisonError::into_inner),
        0,
        "the feed pushed a lost event"
    );
}
