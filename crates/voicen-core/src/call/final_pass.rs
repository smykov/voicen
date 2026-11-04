//! The final pass, chunk part (spec 007 FR-023, FR-024, research R9; T-137, decision #197):
//! cuts a call track, window by window, at speech pauses into bounded, disjoint, ordered
//! upload ranges, and slices the held audio into each range's [`AudioBuffer`].
//!
//! RED STAGE: the bodies below are `todo!()` stubs so the tests compile and fail for the
//! missing behaviour. The developer replaces them; the signatures and the doc comments are
//! what the tests pin. T-138 / T-146 add the pass runner and the queue to this file later.
//!
//! # Semantics (all times are track-relative milliseconds, spans are half-open `[start, end)`)
//!
//! * **Input.** A planner is fed contiguous windows in order (`window_start_ms` equals the end
//!   of the previous window; the first starts at 0). The spans of a window are window-relative,
//!   as the `SpeechSpans` port returns them. The planner normalises them in one place: offset
//!   by `window_start_ms`, clamp to the window, drop empty or inverted ones, sort, merge
//!   overlapping ones. It then merges spans whose gap is `< PAUSE_MS` (also across windows);
//!   a gap of exactly `PAUSE_MS` is a pause.
//! * **Window and carry.** `known_end` is the end of the last window fed. The planner keeps
//!   the audio from `base` to `known_end`; `carry_ms() = known_end - base` and is always
//!   `< max_chunk_ms(mode)`, so the caller asks the decoder for
//!   `max_chunk_ms(mode) - carry_ms()` ms next. `base` starts at 0, becomes the end of the
//!   last emitted chunk, and, when nothing is pending, `max(chunk end, known_end - PAD_MS)`
//!   (so a window without a span leaves `carry_ms() == PAD_MS`, the lead-in of a span that
//!   starts at the next window's first millisecond).
//! * **Timed.** The planner decides only when `known_end >= base + TIMED_MAX_MS` (a full
//!   window) or at `finish`. Let `prev_end` be the end of the previous chunk (0 at the start).
//!   If the last merged span is closed (`known_end - span.end >= PAUSE_MS`, always at
//!   `finish`) all pending spans form one chunk. Otherwise the chunk ends after the last
//!   pause before the open tail span; with no pause at all it is cut hard at
//!   `base + TIMED_MAX_MS` and the rest of the span is carried. A chunk is
//!   `[max(first span start - PAD_MS, prev_end), min(last span end + PAD_MS, known_end)]`,
//!   but a hard cut ends exactly at the limit (no pad). A window without a span yields no
//!   chunk.
//! * **Time-less.** Each merged span is its own chunk (never packed with another), emitted as
//!   soon as it is closed, with the same crop and clamps; a span longer than
//!   `TIMELESS_MAX_MS` is cut hard every `TIMELESS_MAX_MS` from the chunk's start.
//! * **Invariant.** `0 < end - start <= max_chunk_ms(mode)`; every chunk holds at least one
//!   span (`spans`: merged, track-relative, clipped to `[start, end)`); chunks are strictly
//!   ordered and disjoint; every speech millisecond lies in exactly one chunk; `index` counts
//!   from 0 over the planner's lifetime.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use crate::audio::AudioBuffer;
use crate::call::PassMode;
use crate::vad::SpeechSpan;

/// A gap between two spans shorter than this merges them.
pub const PAUSE_MS: u64 = 300;
/// A chunk is cropped to its first span start minus this and its last span end plus this.
pub const PAD_MS: u64 = 200;
/// The longest timed chunk (and the largest window plus carry).
pub const TIMED_MAX_MS: u64 = 120_000;
/// The longest time-less chunk.
pub const TIMELESS_MAX_MS: u64 = 30_000;

/// The chunk length limit of a pass mode: [`TIMED_MAX_MS`] or [`TIMELESS_MAX_MS`].
pub fn max_chunk_ms(mode: PassMode) -> u64 {
    let _ = mode;
    todo!("T-137")
}

/// A cut without audio: what the planner decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkRange {
    /// 0-based, strictly increasing over the planner's lifetime.
    pub index: usize,
    /// Track-relative, the cropped start.
    pub start_ms: u64,
    /// Track-relative, the cropped end (exclusive).
    pub end_ms: u64,
    pub mode: PassMode,
    /// The merged speech spans inside the range, track-relative, sorted, non-empty list.
    pub spans: Vec<SpeechSpan>,
}

/// An upload chunk: its range and the audio of exactly `[start_ms, end_ms)`
/// (`(end_ms - start_ms) * 16` samples).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub index: usize,
    pub start_ms: u64,
    pub end_ms: u64,
    pub mode: PassMode,
    pub spans: Vec<SpeechSpan>,
    pub audio: AudioBuffer,
}

/// The pure range planner: no audio, so a 1 h track is planned without holding it.
#[allow(dead_code)]
#[derive(Debug)]
pub struct RangePlanner {
    mode: PassMode,
}

impl RangePlanner {
    pub fn new(mode: PassMode) -> RangePlanner {
        RangePlanner { mode }
    }

    /// Feeds the next window `[window_start_ms, window_start_ms + window_len_ms)` with its
    /// window-relative spans; returns the chunks that are now decided (possibly none).
    pub fn feed(
        &mut self,
        window_start_ms: u64,
        window_len_ms: u64,
        spans: &[SpeechSpan],
    ) -> Vec<ChunkRange> {
        let _ = (window_start_ms, window_len_ms, spans);
        todo!("T-137")
    }

    /// The track ended: decides everything that is pending.
    pub fn finish(&mut self) -> Vec<ChunkRange> {
        todo!("T-137")
    }

    /// The milliseconds of audio the planner keeps (see the module docs).
    pub fn carry_ms(&self) -> u64 {
        todo!("T-137")
    }
}

/// [`RangePlanner`] plus the held audio: the same decisions, each with its [`AudioBuffer`].
#[allow(dead_code)]
#[derive(Debug)]
pub struct ChunkPlanner {
    ranges: RangePlanner,
}

impl ChunkPlanner {
    pub fn new(mode: PassMode) -> ChunkPlanner {
        ChunkPlanner {
            ranges: RangePlanner::new(mode),
        }
    }

    /// Like [`RangePlanner::feed`]; the window length is `audio.samples().len() / 16` ms.
    pub fn feed(
        &mut self,
        window_start_ms: u64,
        audio: &AudioBuffer,
        spans: &[SpeechSpan],
    ) -> Vec<Chunk> {
        let _ = (window_start_ms, audio, spans);
        todo!("T-137")
    }

    pub fn finish(&mut self) -> Vec<Chunk> {
        todo!("T-137")
    }

    pub fn carry_ms(&self) -> u64 {
        todo!("T-137")
    }
}

/// The FR-024 fallback: re-cuts one already-cut (timed) chunk into time-less chunks from its
/// `spans` and `audio`. Each merged span is a chunk, cropped by [`PAD_MS`] and clamped to the
/// source chunk's range, a span over [`TIMELESS_MAX_MS`] cut hard; `index` counts from 0 in the
/// returned list, `start_ms`/`end_ms` stay track-relative, `mode` is `Timeless`.
pub fn recut_timeless(chunk: &Chunk) -> Vec<Chunk> {
    let _ = chunk;
    todo!("T-137")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::audio::wav;

    fn sp(start_ms: u64, end_ms: u64) -> SpeechSpan {
        SpeechSpan { start_ms, end_ms }
    }

    /// `ms` of audio starting at track ms `from_ms`: sample at absolute index `i` is a
    /// function of `i` only, so a slice can be compared with its source.
    fn ramp(from_ms: u64, ms: u64) -> AudioBuffer {
        let first = from_ms * 16;
        AudioBuffer::from_16k_mono(
            (first..first + ms * 16)
                .map(|i| ((i % 20_011) as i16).wrapping_sub(10_000))
                .collect(),
        )
    }

    /// What a detector returns for the window `[pos, pos + len)`: spans clipped to it,
    /// window-relative.
    fn window_spans(track: &[SpeechSpan], pos: u64, len: u64) -> Vec<SpeechSpan> {
        track
            .iter()
            .filter_map(|s| {
                let a = s.start_ms.max(pos);
                let b = s.end_ms.min(pos + len);
                (a < b).then(|| sp(a - pos, b - pos))
            })
            .collect()
    }

    /// Plays T-138: asks for `max - carry` ms at a time, feeds, finishes.
    fn plan(mode: PassMode, track_len: u64, track: &[SpeechSpan]) -> Vec<ChunkRange> {
        let mut planner = RangePlanner::new(mode);
        let mut out = Vec::new();
        let mut pos = 0;
        while pos < track_len {
            let carry = planner.carry_ms();
            assert!(carry < max_chunk_ms(mode), "carry {carry} leaves no budget");
            let len = (max_chunk_ms(mode) - carry).min(track_len - pos);
            out.extend(planner.feed(pos, len, &window_spans(track, pos, len)));
            pos += len;
        }
        out.extend(planner.finish());
        out
    }

    fn bounds(chunks: &[ChunkRange]) -> Vec<(u64, u64)> {
        chunks.iter().map(|c| (c.start_ms, c.end_ms)).collect()
    }

    /// The invariant of the module docs over a whole track.
    fn assert_invariant(mode: PassMode, track_len: u64, track: &[SpeechSpan], got: &[ChunkRange]) {
        let max = max_chunk_ms(mode);
        let mut prev_end = 0;
        for (i, c) in got.iter().enumerate() {
            assert_eq!(c.index, i, "indices count from 0 in call order");
            assert_eq!(c.mode, mode);
            assert!(c.start_ms < c.end_ms, "empty chunk {c:?}");
            assert!(c.end_ms - c.start_ms <= max, "chunk over the limit {c:?}");
            assert!(c.end_ms <= track_len, "chunk past the track {c:?}");
            assert!(
                c.start_ms >= prev_end,
                "chunks overlap or are out of order {c:?}"
            );
            prev_end = c.end_ms;
            assert!(!c.spans.is_empty(), "chunk without a span {c:?}");
            let mut span_end = c.start_ms;
            for s in &c.spans {
                assert!(s.start_ms < s.end_ms);
                assert!(s.start_ms >= c.start_ms && s.end_ms <= c.end_ms);
                assert!(
                    s.start_ms >= span_end,
                    "spans unsorted or overlapping {c:?}"
                );
                span_end = s.end_ms;
            }
            if mode == PassMode::Timeless {
                assert_eq!(c.spans.len(), 1, "a time-less chunk is one merged span");
            }
        }
        // Every speech millisecond of the track lies in exactly one chunk.
        for s in track {
            let (a, b) = (s.start_ms, s.end_ms.min(track_len));
            if a >= b {
                continue;
            }
            let covered: u64 = got
                .iter()
                .map(|c| c.end_ms.min(b).saturating_sub(c.start_ms.max(a)))
                .sum();
            assert_eq!(covered, b - a, "speech [{a}, {b}) not covered exactly once");
        }
    }

    // ---- timed: merge, cut, crop ------------------------------------------------------

    #[test]
    fn timed_gap_under_300_ms_merges_and_a_gap_of_300_does_not() {
        let merged = plan(
            PassMode::Timed,
            10_000,
            &[sp(1_000, 2_000), sp(2_299, 3_000)],
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].spans, vec![sp(1_000, 3_000)], "gap 299 merges");

        let apart = plan(
            PassMode::Timed,
            10_000,
            &[sp(1_000, 2_000), sp(2_300, 3_000)],
        );
        assert_eq!(apart.len(), 1, "timed packs both into one chunk");
        assert_eq!(
            apart[0].spans,
            vec![sp(1_000, 2_000), sp(2_300, 3_000)],
            "gap 300 is a pause: two spans"
        );
    }

    #[test]
    fn timed_cut_falls_at_the_end_of_the_last_pause_before_the_limit() {
        // S1 [0, 50 s), pause 1 s, S2 [51 s, 100 s), pause 400 ms, S3 [100.4 s, 130 s).
        let mut p = RangePlanner::new(PassMode::Timed);
        let first = p.feed(
            0,
            120_000,
            &[sp(0, 50_000), sp(51_000, 100_000), sp(100_400, 120_000)],
        );
        assert_eq!(first.len(), 1, "the full window decides at once");
        assert_eq!(first[0].index, 0);
        assert_eq!((first[0].start_ms, first[0].end_ms), (0, 100_200));
        assert_eq!(first[0].spans, vec![sp(0, 50_000), sp(51_000, 100_000)]);
        assert_eq!(p.carry_ms(), 120_000 - 100_200, "the rest of S3 is carried");

        // The decoder is asked for 120 000 - carry; the track has 20 s left.
        let more = p.feed(120_000, 20_000, &[sp(0, 10_000)]);
        assert!(
            more.is_empty(),
            "a window short of the limit waits for more"
        );
        let last = p.finish();
        assert_eq!(last.len(), 1);
        assert_eq!(last[0].index, 1);
        assert_eq!((last[0].start_ms, last[0].end_ms), (100_200, 130_200));
        assert_eq!(
            last[0].spans,
            vec![sp(100_400, 130_000)],
            "carried part and new window are one span"
        );
    }

    #[test]
    fn timed_cut_picks_the_last_pause_not_the_first() {
        // Pauses at 10-11 s and 60-61 s; the tail runs on to the limit.
        let got = plan(
            PassMode::Timed,
            130_000,
            &[sp(0, 10_000), sp(11_000, 60_000), sp(61_000, 125_000)],
        );
        assert_eq!(bounds(&got), vec![(0, 60_200), (60_800, 125_200)]);
    }

    #[test]
    fn timed_full_window_with_only_closed_spans_emits_them_at_once() {
        let mut p = RangePlanner::new(PassMode::Timed);
        let got = p.feed(0, 120_000, &[sp(1_000, 5_000)]);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].start_ms, got[0].end_ms), (800, 5_200));
        assert_eq!(
            p.carry_ms(),
            PAD_MS,
            "nothing pending: only the lead-in is kept"
        );
        assert!(p.finish().is_empty());
    }

    #[test]
    fn timed_crop_is_200_ms_each_side_clamped_to_the_track() {
        let mid = plan(PassMode::Timed, 20_000, &[sp(5_000, 6_000)]);
        assert_eq!(bounds(&mid), vec![(4_800, 6_200)]);
        let head = plan(PassMode::Timed, 20_000, &[sp(100, 900)]);
        assert_eq!(bounds(&head), vec![(0, 1_100)], "start clamps at 0");
        let tail = plan(PassMode::Timed, 20_000, &[sp(19_500, 20_000)]);
        assert_eq!(
            bounds(&tail),
            vec![(19_300, 20_000)],
            "end clamps at the audio end"
        );
    }

    #[test]
    fn timed_crop_is_clamped_by_the_previous_chunk_end() {
        // A pause of exactly 300 ms: the pads (200 + 200) would overlap by 100 ms.
        let got = plan(
            PassMode::Timed,
            130_000,
            &[sp(0, 60_000), sp(60_300, 125_000)],
        );
        assert_eq!(bounds(&got), vec![(0, 60_200), (60_200, 125_200)]);
    }

    // ---- timed: failure branch --------------------------------------------------------

    #[test]
    fn pauseless_five_minutes_is_cut_hard_at_120_s_with_the_rest_carried() {
        let mut p = ChunkPlanner::new(PassMode::Timed);
        let whole = sp(0, 120_000);

        let c1 = p.feed(0, &ramp(0, 120_000), &[whole]);
        assert_eq!(
            c1.len(),
            1,
            "the first hard cut comes with the first full window"
        );
        assert_eq!(
            (c1[0].start_ms, c1[0].end_ms),
            (0, 120_000),
            "no pad past the limit"
        );
        assert_eq!(c1[0].spans, vec![sp(0, 120_000)]);
        assert_eq!(p.carry_ms(), 0);

        let c2 = p.feed(120_000, &ramp(120_000, 120_000), &[whole]);
        assert_eq!(c2.len(), 1);
        assert_eq!(c2[0].index, 1);
        assert_eq!(
            (c2[0].start_ms, c2[0].end_ms),
            (120_000, 240_000),
            "no pad into chunk 1"
        );
        assert_eq!(p.carry_ms(), 0);

        let c3 = p.feed(240_000, &ramp(240_000, 60_000), &[sp(0, 60_000)]);
        assert!(c3.is_empty(), "the last 60 s wait for the end of the track");
        let last = p.finish();
        assert_eq!(last.len(), 1);
        assert_eq!(last[0].index, 2);
        assert_eq!((last[0].start_ms, last[0].end_ms), (240_000, 300_000));
        assert_eq!(last[0].audio.samples().len(), 60_000 * 16);
        assert_eq!(
            last[0].audio,
            ramp(240_000, 60_000),
            "the carry kept the right audio"
        );
        assert_eq!(c1[0].audio, ramp(0, 120_000));
        assert_eq!(c2[0].audio, ramp(120_000, 120_000));
    }

    #[test]
    fn a_window_with_no_span_yields_no_chunk_in_both_modes() {
        for mode in [PassMode::Timed, PassMode::Timeless] {
            let mut p = ChunkPlanner::new(mode);
            assert!(p.feed(0, &ramp(0, 30_000), &[]).is_empty());
            assert!(p.feed(30_000, &ramp(30_000, 5_000), &[]).is_empty());
            assert!(
                p.finish().is_empty(),
                "span-less audio is never sent ({mode:?})"
            );
        }
        let mut full = RangePlanner::new(PassMode::Timed);
        assert!(full.feed(0, 120_000, &[]).is_empty());
        assert_eq!(
            full.carry_ms(),
            PAD_MS,
            "only the lead-in of the next span is kept"
        );
    }

    #[test]
    fn a_span_at_the_first_millisecond_of_the_next_window_keeps_its_lead_in() {
        let mut p = ChunkPlanner::new(PassMode::Timed);
        assert!(p.feed(0, &ramp(0, 120_000), &[]).is_empty());
        assert!(p
            .feed(120_000, &ramp(120_000, 10_000), &[sp(0, 1_000)])
            .is_empty());
        let got = p.finish();
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].start_ms, got[0].end_ms), (119_800, 121_200));
        assert_eq!(
            got[0].audio,
            ramp(119_800, 1_400),
            "the pad comes from the previous window"
        );
    }

    #[test]
    fn window_relative_spans_are_offset_to_track_time_in_one_place() {
        let mut p = RangePlanner::new(PassMode::Timed);
        assert!(p.feed(0, 120_000, &[]).is_empty());
        assert!(p.feed(120_000, 10_000, &[sp(500, 1_000)]).is_empty());
        let got = p.finish();
        assert_eq!(bounds(&got), vec![(120_300, 121_200)]);
        assert_eq!(got[0].spans, vec![sp(120_500, 121_000)]);
    }

    #[test]
    fn a_span_touching_the_window_edge_continues_into_the_next_window() {
        // 60 s of speech split by the decoder's window at 50 s: one span, one chunk.
        let mut p = RangePlanner::new(PassMode::Timed);
        assert!(p.feed(0, 50_000, &[sp(10_000, 50_000)]).is_empty());
        assert!(p.feed(50_000, 40_000, &[sp(0, 10_000)]).is_empty());
        let got = p.finish();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].spans, vec![sp(10_000, 60_000)]);
    }

    // ---- input normalisation ----------------------------------------------------------

    #[test]
    fn unsorted_overlapping_empty_and_out_of_window_spans_are_normalised() {
        let messy = [
            sp(5_000, 6_000),
            sp(1_000, 2_000),
            sp(1_500, 2_500),  // overlaps the previous one
            sp(7_000, 7_000),  // empty
            sp(9_000, 8_000),  // inverted
            sp(9_500, 12_000), // runs past the window
        ];
        let mut p = RangePlanner::new(PassMode::Timed);
        assert!(p.feed(0, 10_000, &messy).is_empty());
        let got = p.finish();
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].spans,
            vec![sp(1_000, 2_500), sp(5_000, 6_000), sp(9_500, 10_000)]
        );
        assert_eq!((got[0].start_ms, got[0].end_ms), (800, 10_000));
    }

    #[test]
    fn only_empty_or_inverted_spans_are_no_speech() {
        for mode in [PassMode::Timed, PassMode::Timeless] {
            let mut p = RangePlanner::new(mode);
            assert!(p
                .feed(0, 10_000, &[sp(3_000, 3_000), sp(5_000, 4_000)])
                .is_empty());
            assert!(p.finish().is_empty());
        }
    }

    // ---- time-less --------------------------------------------------------------------

    #[test]
    fn timeless_makes_each_merged_span_a_chunk() {
        let got = plan(
            PassMode::Timeless,
            10_000,
            &[sp(1_000, 2_000), sp(5_000, 6_000)],
        );
        assert_eq!(
            bounds(&got),
            vec![(800, 2_200), (4_800, 6_200)],
            "not packed together"
        );
        assert!(got.iter().all(|c| c.mode == PassMode::Timeless));
    }

    #[test]
    fn timeless_merges_a_gap_of_299_ms_and_splits_at_300_with_a_clamped_pad() {
        let merged = plan(
            PassMode::Timeless,
            10_000,
            &[sp(1_000, 2_000), sp(2_299, 3_000)],
        );
        assert_eq!(bounds(&merged), vec![(800, 3_200)]);
        let apart = plan(
            PassMode::Timeless,
            10_000,
            &[sp(1_000, 2_000), sp(2_300, 3_000)],
        );
        assert_eq!(
            bounds(&apart),
            vec![(800, 2_200), (2_200, 3_200)],
            "pad clamped by chunk 0"
        );
    }

    #[test]
    fn timeless_70_s_span_is_cut_hard_into_30_30_10() {
        let got = plan(PassMode::Timeless, 70_000, &[sp(0, 70_000)]);
        assert_eq!(
            bounds(&got),
            vec![(0, 30_000), (30_000, 60_000), (60_000, 70_000)]
        );
        // More audio after the span: the last piece gets its 200 ms pad.
        let padded = plan(PassMode::Timeless, 71_000, &[sp(0, 70_000)]);
        assert_eq!(
            bounds(&padded),
            vec![(0, 30_000), (30_000, 60_000), (60_000, 70_200)]
        );
    }

    #[test]
    fn timeless_streaming_emits_a_hard_cut_with_the_window_that_reaches_it() {
        let mut p = RangePlanner::new(PassMode::Timeless);
        let c1 = p.feed(0, 30_000, &[sp(0, 30_000)]);
        assert_eq!(bounds(&c1), vec![(0, 30_000)]);
        assert_eq!(p.carry_ms(), 0);
        // The span ends 100 ms before the window does: it may still continue, so it is held.
        let c2 = p.feed(30_000, 10_000, &[sp(0, 9_900)]);
        assert!(c2.is_empty(), "an open span is held");
        assert_eq!(p.carry_ms(), 10_000);
        let rest = p.finish();
        assert_eq!(bounds(&rest), vec![(30_000, 40_000)]);
    }

    #[test]
    fn recut_of_one_timed_chunk_gives_timeless_chunks_from_its_own_audio() {
        let track = [sp(1_000, 20_000), sp(20_500, 60_000)];
        let mut p = ChunkPlanner::new(PassMode::Timed);
        assert!(p.feed(0, &ramp(0, 61_000), &track).is_empty());
        let timed = p.finish();
        assert_eq!(timed.len(), 1);
        assert_eq!((timed[0].start_ms, timed[0].end_ms), (800, 60_200));
        assert_eq!(timed[0].spans, track.to_vec());

        let pieces = recut_timeless(&timed[0]);
        let got: Vec<_> = pieces.iter().map(|c| (c.start_ms, c.end_ms)).collect();
        assert_eq!(got, vec![(800, 20_200), (20_300, 50_300), (50_300, 60_200)]);
        for (i, c) in pieces.iter().enumerate() {
            assert_eq!(c.index, i);
            assert_eq!(c.mode, PassMode::Timeless);
            assert!(c.end_ms - c.start_ms <= TIMELESS_MAX_MS);
            assert_eq!(
                c.audio,
                ramp(c.start_ms, c.end_ms - c.start_ms),
                "slice of the source"
            );
        }
        assert_eq!(pieces[1].spans, vec![sp(20_500, 50_300)]);
        assert_eq!(pieces[2].spans, vec![sp(50_300, 60_000)]);
    }

    // ---- WAV --------------------------------------------------------------------------

    #[test]
    fn a_maximal_chunk_is_a_3_840_044_byte_16k_mono_pcm16_wav() {
        let mut p = ChunkPlanner::new(PassMode::Timed);
        let got = p.feed(0, &ramp(0, 120_000), &[sp(0, 120_000)]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].audio.samples().len(), 1_920_000);
        let bytes = wav::encode(&got[0].audio);
        assert_eq!(bytes.len(), 3_840_044);
        assert_eq!(
            bytes.len() as u64,
            44 + 32 * (got[0].end_ms - got[0].start_ms)
        );
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes([bytes[20], bytes[21]]), 1, "PCM");
        assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 1, "mono");
        assert_eq!(
            u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
            16_000
        );
        assert_eq!(u16::from_le_bytes([bytes[34], bytes[35]]), 16, "16-bit");
        assert!(bytes.len() < 25_000_000);

        let mut t = ChunkPlanner::new(PassMode::Timeless);
        let tl = t.feed(0, &ramp(0, 30_000), &[sp(0, 30_000)]);
        assert_eq!(
            wav::encode(&tl[0].audio).len(),
            960_044,
            "44 + 32 B/ms at 30 s"
        );
    }

    // ---- properties and the 1 h track -------------------------------------------------

    /// xorshift64*; no dependency, a fixed seed per case.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n.max(1)
        }
    }

    fn random_track(rng: &mut Rng) -> (u64, Vec<SpeechSpan>) {
        let track_len = 1 + rng.below(600_000);
        let n = rng.below(40);
        let spans = (0..n)
            .map(|_| {
                let start = rng.below(track_len + 2_000);
                let len = match rng.below(4) {
                    0 => rng.below(300),
                    1 => rng.below(3_000),
                    2 => rng.below(40_000),
                    _ => rng.below(200_000),
                };
                if rng.below(20) == 0 {
                    sp(start + len, start) // inverted
                } else {
                    sp(start, start + len)
                }
            })
            .collect();
        (track_len, spans)
    }

    #[test]
    fn every_chunk_is_bounded_disjoint_ordered_and_covers_all_speech() {
        for seed in 1..=300u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let (track_len, track) = random_track(&mut rng);
            for mode in [PassMode::Timed, PassMode::Timeless] {
                let got = plan(mode, track_len, &track);
                assert_invariant(mode, track_len, &track, &got);
            }
        }
    }

    #[test]
    fn one_hour_track_gives_bounded_ordered_uploads_planned_without_its_audio() {
        let mut track = vec![sp(0, 1_000_000)]; // a pause-less 1000 s
        let mut t = 1_000_400;
        while t + 7_000 <= 3_600_000 {
            track.push(sp(t, t + 7_000));
            t += 7_400; // 400 ms pauses
        }
        let got = plan(PassMode::Timed, 3_600_000, &track);
        assert!(got.len() >= 30, "only {} chunks", got.len());
        assert_invariant(PassMode::Timed, 3_600_000, &track, &got);
        let mut prev_start = None;
        for (i, c) in got.iter().enumerate() {
            let wav_bytes = 44 + 32 * (c.end_ms - c.start_ms);
            assert!(wav_bytes <= 3_840_044, "{c:?}");
            assert!(wav_bytes < 25_000_000);
            assert_eq!(c.index, i);
            if let Some(prev) = prev_start {
                assert!(c.start_ms > prev, "start_ms strictly increasing");
            }
            prev_start = Some(c.start_ms);
        }
    }
}
