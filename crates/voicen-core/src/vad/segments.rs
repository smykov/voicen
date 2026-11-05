//! Native speech segments to [`SpeechSpan`]s (T-135; spec 007 FR-023, R8).
//!
//! The Silero detector gets segments from whisper.cpp in centiseconds as `f32`. This is the
//! one funnel from that answer to the port's spans, so the rules live here, testable on
//! Linux: an error stays an error, a malformed segment is an error, and nothing is ever
//! turned into "no speech" by accident.

use super::{SpeechSpan, VadError};

/// Milliseconds per centisecond: spans land on 10 ms steps.
const MS_PER_CS: u64 = 10;

/// The spans of a native answer over a buffer of `duration_ms`.
///
/// - An incoming `Err` comes back unchanged.
/// - A malformed segment (a non-finite or negative bound, `end <= start`, or a start before
///   the previous segment's end) makes the whole answer `Err(VadError::Failed)`; it is judged
///   on the native values, before the clamp.
/// - Each bound is rounded to whole centiseconds and turned into ms, so spans lie on 10 ms
///   steps; the end is clamped to the last 10 ms step within `duration_ms`.
/// - A span empty after rounding and the clamp (only padding past the buffer) is dropped.
///
/// The result is sorted, non-overlapping, non-empty and within `[0, duration_ms]`.
pub fn spans_from_centiseconds(
    native: Result<impl IntoIterator<Item = (f32, f32)>, VadError>,
    duration_ms: u64,
) -> Result<Vec<SpeechSpan>, VadError> {
    let limit_ms = duration_ms - duration_ms % MS_PER_CS;
    let mut spans = Vec::new();
    let mut previous_end = 0.0_f32;
    for (start, end) in native? {
        let well_formed = start.is_finite()
            && end.is_finite()
            && start >= 0.0
            && end > start
            && start >= previous_end;
        if !well_formed {
            return Err(VadError::Failed);
        }
        previous_end = end;
        let start_ms = to_ms(start).min(limit_ms);
        let end_ms = to_ms(end).min(limit_ms);
        if start_ms < end_ms {
            spans.push(SpeechSpan { start_ms, end_ms });
        }
    }
    Ok(spans)
}

/// A finite, non-negative centisecond value in ms on the 10 ms step (saturating).
fn to_ms(cs: f32) -> u64 {
    // `as` saturates a float above u64::MAX; the caller checked finite and non-negative.
    (cs.round() as u64).saturating_mul(MS_PER_CS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start_ms: u64, end_ms: u64) -> SpeechSpan {
        SpeechSpan { start_ms, end_ms }
    }

    fn run(segments: Vec<(f32, f32)>, duration_ms: u64) -> Result<Vec<SpeechSpan>, VadError> {
        spans_from_centiseconds(Ok(segments), duration_ms)
    }

    #[test]
    fn a_native_error_comes_back_unchanged_never_as_empty() {
        // Acceptance 1. Bite: mapping Err to Ok(empty), or to a fixed variant.
        for err in [VadError::Failed, VadError::Unavailable] {
            let native: Result<Vec<(f32, f32)>, VadError> = Err(err);
            assert_eq!(spans_from_centiseconds(native, 10_000), Err(err));
        }
    }

    #[test]
    fn centiseconds_become_milliseconds() {
        // Bite: no x10, or x100.
        assert_eq!(
            run(vec![(10.0, 35.0), (100.0, 150.0)], 10_000),
            Ok(vec![span(100, 350), span(1_000, 1_500)])
        );
    }

    #[test]
    fn no_segments_is_ok_empty() {
        assert_eq!(run(vec![], 10_000), Ok(vec![]));
    }

    #[test]
    fn an_end_past_the_buffer_is_clamped_to_its_duration() {
        // whisper.cpp pads the last segment up to 31 ms past the buffer. Bite: no clamp.
        assert_eq!(run(vec![(0.0, 31.0)], 300), Ok(vec![span(0, 300)]));
    }

    #[test]
    fn a_span_empty_after_the_clamp_is_dropped_not_an_error() {
        // Starts at the buffer end: nothing of it is inside. Bite: keeping [300, 300), or an
        // Err for a segment that is only padding.
        assert_eq!(
            run(vec![(0.0, 10.0), (30.0, 31.0)], 300),
            Ok(vec![span(0, 100)])
        );
        assert_eq!(run(vec![(35.0, 40.0)], 300), Ok(vec![]));
    }

    #[test]
    fn a_malformed_segment_is_failed_never_dropped() {
        // Acceptance 1. One bad segment among good ones fails the whole answer. Bite: filtering
        // the bad segment out (as contains_speech did), or Ok(empty).
        let bad = [
            ("NaN start", (f32::NAN, 20.0)),
            ("NaN end", (10.0, f32::NAN)),
            ("infinite end", (10.0, f32::INFINITY)),
            ("infinite start", (f32::NEG_INFINITY, 20.0)),
            ("negative start", (-1.0, 20.0)),
            ("negative end", (-5.0, -2.0)),
            ("end before start", (40.0, 20.0)),
            ("end equals start", (20.0, 20.0)),
        ];
        for (what, seg) in bad {
            assert_eq!(
                run(vec![(0.0, 10.0), seg, (60.0, 80.0)], 10_000),
                Err(VadError::Failed),
                "{what}, in the middle"
            );
            assert_eq!(
                run(vec![seg], 10_000),
                Err(VadError::Failed),
                "{what}, alone"
            );
        }
    }

    #[test]
    fn a_malformed_segment_past_the_buffer_is_still_failed() {
        // Malformed is judged before the clamp. Bite: clamping first (end == start == the
        // duration would then be dropped as empty instead of failing).
        assert_eq!(run(vec![(40.0, 40.0)], 300), Err(VadError::Failed));
    }

    #[test]
    fn an_overlapping_segment_is_failed_never_merged_or_kept() {
        // Invariant: spans are sorted and non-overlapping; a start before the previous end is
        // malformed. Bite: dropping the overlap check, or not tracking the previous end.
        assert_eq!(
            run(vec![(0.0, 30.0), (20.0, 40.0)], 10_000),
            Err(VadError::Failed),
            "overlap"
        );
        assert_eq!(
            run(vec![(50.0, 60.0), (10.0, 20.0)], 10_000),
            Err(VadError::Failed),
            "out of order"
        );
        // Touching (start == previous end) is not an overlap.
        assert_eq!(
            run(vec![(0.0, 30.0), (30.0, 40.0)], 10_000),
            Ok(vec![span(0, 300), span(300, 400)])
        );
    }

    #[test]
    fn the_clamp_lands_on_the_last_10_ms_step_within_the_buffer() {
        // Invariant: spans on 10 ms steps within [0, duration]. A 305 ms buffer clamps to 300.
        // Bite: clamping to the raw duration (305 is off the 10 ms step).
        assert_eq!(run(vec![(0.0, 31.0)], 305), Ok(vec![span(0, 300)]));
        assert_eq!(run(vec![(30.5, 31.0)], 309), Ok(vec![]));
    }

    #[test]
    fn fractional_centiseconds_round_to_the_nearest_10_ms_step() {
        // whisper.cpp's segments are f32 centiseconds. Bite: `(cs * 10) as u64` (off the
        // 10 ms step) or truncation instead of rounding.
        assert_eq!(
            run(vec![(10.4, 35.6), (50.5, 60.49)], 10_000),
            Ok(vec![span(100, 360), span(510, 600)])
        );
    }
}
