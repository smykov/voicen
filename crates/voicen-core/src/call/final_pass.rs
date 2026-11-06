//! The final pass, chunk part (spec 007 FR-023, FR-024, research R9; T-137, decision #197):
//! cuts a call track, window by window, at speech pauses into bounded, disjoint, ordered
//! upload ranges, and slices the held audio into each range's [`AudioBuffer`].
//!
//! T-138 / T-146 add the pass runner and the queue to this file later; every cut, pad and bound
//! of call audio goes through the planners here.
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
//!   `max_chunk_ms(mode) - carry_ms()` ms next. `base` starts at 0 and moves forward only
//!   after a chunk is emitted or while nothing is pending, never on its own when spans
//!   arrive: after a chunk it is `max(prev_end, first pending span start - PAD_MS)` (the end
//!   of that chunk, unless the next span's lead-in starts later), and when nothing is
//!   pending `max(prev_end, known_end - PAD_MS)` (so a window without a span leaves
//!   `carry_ms() == PAD_MS`, the lead-in of a span that starts at the next window's first
//!   millisecond). `base` is never after the start of the next chunk.
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
//!   `TIMELESS_MAX_MS` is cut hard at `base + TIMELESS_MAX_MS` (after the first piece `base`
//!   is the chunk's start, so every `TIMELESS_MAX_MS` from it), and an open span is cut there
//!   too once the audio reaches that point, so `carry_ms()` stays below the limit.
//! * **Invariant.** `0 < end - start <= max_chunk_ms(mode)`; every chunk holds at least one
//!   span (`spans`: merged, track-relative, clipped to `[start, end)`); chunks are strictly
//!   ordered and disjoint; every speech millisecond lies in exactly one chunk; `index` counts
//!   from 0 over the planner's lifetime.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::File;
use std::io::{self, BufReader};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use crate::audio::{AudioBuffer, SAMPLE_RATE};
use crate::call::codec::{OpusOggReader, TrackDecoder};
use crate::call::storage::{
    write_atomic, write_meta, Failure, Results, TRANSCRIPT_FILE, TRANSCRIPT_MD_FILE,
};
use crate::call::{
    merge, render_markdown, CallId, CallMeta, CallNotice, CallState, ChunkInput, DecodeEnd,
    EngineRef, FailReason, FinalOutcome, FinalStage, MergeInput, PassMode, StorageKind, Track,
    TrackInput, TrackResult, TranscriptV1, VadFallbackCause,
};
use crate::clock::MonotonicClock;
use crate::diag::event::LogEvent;
use crate::engine::openai::{OpenAiCompatibleEngine, ResponseShape, Timed, TimedFailure};
use crate::engine::TranscribeRequest;
use crate::secrets::{CredentialStore, KeySlot};
use crate::settings::url::check_base_url;
use crate::settings::Settings;
use crate::timeouts::Timeouts;
use crate::vad::{EnergyDetector, SpeechSpan, SpeechSpans, VadError};

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
    match mode {
        PassMode::Timed => TIMED_MAX_MS,
        PassMode::Timeless => TIMELESS_MAX_MS,
    }
}

/// Samples per millisecond of 16 kHz audio.
const SAMPLES_PER_MS: usize = (SAMPLE_RATE / 1_000) as usize;

fn ms_to_samples(ms: u64) -> usize {
    usize::try_from(ms)
        .unwrap_or(usize::MAX)
        .saturating_mul(SAMPLES_PER_MS)
}

/// The audio of `[start_ms, end_ms)` out of `samples`, which begin at track ms `origin_ms`.
/// Always exactly `(end_ms - start_ms) * 16` samples; the planners only ask for held audio,
/// so the silence fill for a range outside `samples` is never reached.
fn slice(samples: &[i16], origin_ms: u64, start_ms: u64, end_ms: u64) -> AudioBuffer {
    let from = ms_to_samples(start_ms.saturating_sub(origin_ms));
    let to = ms_to_samples(end_ms.saturating_sub(origin_ms));
    let mut out = samples
        .get(from..to.min(samples.len()))
        .unwrap_or_default()
        .to_vec();
    out.resize(to.saturating_sub(from), 0);
    AudioBuffer::from_16k_mono(out)
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
#[derive(Debug)]
pub struct RangePlanner {
    mode: PassMode,
    /// The index of the next chunk.
    next_index: usize,
    /// The end of the last emitted chunk (the start of the planned range before the first).
    prev_end: u64,
    /// The end of the last window fed.
    known_end: u64,
    /// The start of the held audio (module docs, "Window and carry").
    base: u64,
    /// Merged, track-relative, sorted spans not yet in a chunk; consecutive gaps `>= PAUSE_MS`.
    pending: Vec<SpeechSpan>,
}

impl RangePlanner {
    pub fn new(mode: PassMode) -> RangePlanner {
        RangePlanner::resume(mode, 0, 0, &[])
    }

    /// A planner over the already-known range `[start_ms, end_ms)` with its track-relative
    /// `spans` (normalised like a window's), nothing emitted yet.
    fn resume(mode: PassMode, start_ms: u64, end_ms: u64, spans: &[SpeechSpan]) -> RangePlanner {
        let mut planner = RangePlanner {
            mode,
            next_index: 0,
            prev_end: start_ms,
            known_end: start_ms,
            base: start_ms,
            pending: Vec::new(),
        };
        planner.take_spans(start_ms, end_ms, spans.iter().copied());
        planner.known_end = end_ms.max(start_ms);
        planner.rebase();
        planner
    }

    /// Feeds the next window `[window_start_ms, window_start_ms + window_len_ms)` with its
    /// window-relative spans; returns the chunks that are now decided (possibly none).
    pub fn feed(
        &mut self,
        window_start_ms: u64,
        window_len_ms: u64,
        spans: &[SpeechSpan],
    ) -> Vec<ChunkRange> {
        // The one place window-relative spans become track-relative.
        let window_end = window_start_ms.saturating_add(window_len_ms);
        let track_relative = spans.iter().map(|s| SpeechSpan {
            start_ms: window_start_ms.saturating_add(s.start_ms),
            end_ms: window_start_ms.saturating_add(s.end_ms),
        });
        // Clamped to the new part of the window, so nothing before `known_end` is reopened.
        self.take_spans(
            window_start_ms.max(self.known_end),
            window_end,
            track_relative,
        );
        self.known_end = self.known_end.max(window_end);
        self.decide_all(false)
    }

    /// The track ended: decides everything that is pending.
    pub fn finish(&mut self) -> Vec<ChunkRange> {
        self.decide_all(true)
    }

    /// The milliseconds of audio the planner keeps (see the module docs).
    pub fn carry_ms(&self) -> u64 {
        self.known_end.saturating_sub(self.base)
    }

    /// Moves `base` to the earliest track ms the next chunk can start at: the lead-in of the
    /// first pending span, or of a span starting at `known_end`, never before `prev_end`.
    /// Called after each chunk and when nothing is pending, so `base` only grows.
    fn rebase(&mut self) {
        let lead_in_of = match self.pending.first() {
            Some(first) => first.start_ms,
            None => self.known_end,
        };
        self.base = lead_in_of
            .saturating_sub(PAD_MS)
            .max(self.prev_end)
            .max(self.base);
    }

    /// Normalises `spans` (clamp to `[from, to)`, drop empty or inverted, sort) and merges
    /// them into `pending` (overlaps and gaps `< PAUSE_MS`, also with the last pending span).
    fn take_spans(&mut self, from: u64, to: u64, spans: impl Iterator<Item = SpeechSpan>) {
        let mut fresh: Vec<SpeechSpan> = spans
            .filter_map(|s| {
                let start_ms = s.start_ms.max(from);
                let end_ms = s.end_ms.min(to);
                (start_ms < end_ms).then_some(SpeechSpan { start_ms, end_ms })
            })
            .collect();
        fresh.sort_unstable_by_key(|s| s.start_ms);
        for span in fresh {
            match self.pending.last_mut() {
                Some(last) if span.start_ms < last.end_ms.saturating_add(PAUSE_MS) => {
                    last.end_ms = last.end_ms.max(span.end_ms);
                }
                _ => self.pending.push(span),
            }
        }
    }

    fn decide_all(&mut self, finished: bool) -> Vec<ChunkRange> {
        let mut out = Vec::new();
        while let Some(range) = self.decide(finished) {
            out.push(range);
            self.rebase();
        }
        if self.pending.is_empty() {
            self.rebase();
        }
        out
    }

    /// The next chunk, if it is decided now (module docs, "Timed" and "Time-less").
    fn decide(&mut self, finished: bool) -> Option<ChunkRange> {
        let first = *self.pending.first()?;
        let last = *self.pending.last()?;
        let max = max_chunk_ms(self.mode);
        let start = first.start_ms.saturating_sub(PAD_MS).max(self.prev_end);
        // `base <= start`, so a chunk ending by `base + max` is within the limit. Only a
        // window past the caller's budget can put the first span beyond it.
        let mut limit = self.base.saturating_add(max);
        if limit <= first.start_ms {
            limit = start.saturating_add(max);
        }
        let full = self.known_end >= limit;
        if self.mode == PassMode::Timed && !full && !finished {
            return None;
        }
        let closed = finished || self.known_end.saturating_sub(last.end_ms) >= PAUSE_MS;
        let count = self.pending.len();
        // A chunk may end after span `k` when `k` fits the limit and is followed by a pause.
        let ends_chunk = |k: usize, s: &SpeechSpan| s.end_ms <= limit && (k + 1 < count || closed);
        let pick = match self.mode {
            PassMode::Timed => self
                .pending
                .iter()
                .enumerate()
                .filter(|(k, s)| ends_chunk(*k, s))
                .map(|(k, _)| k)
                .next_back(),
            PassMode::Timeless => self.pending.first().filter(|s| ends_chunk(0, s)).map(|_| 0),
        };
        let (end, spans) = if let Some(k) = pick {
            let rest = self.pending.split_off(k + 1);
            let taken = std::mem::replace(&mut self.pending, rest);
            let last_end = taken.last().map_or(start, |s| s.end_ms);
            let end = last_end
                .saturating_add(PAD_MS)
                .min(self.known_end)
                .min(limit);
            (end, taken)
        } else if full {
            // No pause to cut at: hard cut at the limit, no pad, the rest is carried.
            let mut taken = Vec::new();
            let mut rest = Vec::new();
            for s in self.pending.drain(..) {
                if s.start_ms < limit {
                    taken.push(SpeechSpan {
                        start_ms: s.start_ms,
                        end_ms: s.end_ms.min(limit),
                    });
                }
                if s.end_ms > limit {
                    rest.push(SpeechSpan {
                        start_ms: s.start_ms.max(limit),
                        end_ms: s.end_ms,
                    });
                }
            }
            self.pending = rest;
            (limit, taken)
        } else {
            return None;
        };
        self.prev_end = end;
        let index = self.next_index;
        self.next_index += 1;
        Some(ChunkRange {
            index,
            start_ms: start,
            end_ms: end,
            mode: self.mode,
            spans,
        })
    }
}

/// [`RangePlanner`] plus the held audio: the same decisions, each with its [`AudioBuffer`].
/// It holds only `[base, known_end)`, i.e. at most one window plus its carry.
#[derive(Debug)]
pub struct ChunkPlanner {
    ranges: RangePlanner,
    /// The audio of `[held_start_ms, ranges.known_end)`.
    held: Vec<i16>,
    held_start_ms: u64,
}

impl ChunkPlanner {
    pub fn new(mode: PassMode) -> ChunkPlanner {
        ChunkPlanner {
            ranges: RangePlanner::new(mode),
            held: Vec::new(),
            held_start_ms: 0,
        }
    }

    /// Like [`RangePlanner::feed`]; the window length is `audio.samples().len() / 16` ms.
    pub fn feed(
        &mut self,
        window_start_ms: u64,
        audio: &AudioBuffer,
        spans: &[SpeechSpan],
    ) -> Vec<Chunk> {
        debug_assert_eq!(
            window_start_ms, self.ranges.known_end,
            "windows must be fed contiguously"
        );
        let samples = audio.samples();
        let len_ms = (samples.len() / SAMPLES_PER_MS) as u64;
        let whole_ms = samples.get(..ms_to_samples(len_ms)).unwrap_or(samples);
        // Keep `held` aligned with `[held_start_ms, known_end)` even if a caller breaks the
        // contiguous-window contract: a gap is held as silence, an overlap is dropped.
        let known_end = self.ranges.known_end;
        let gap = ms_to_samples(window_start_ms.saturating_sub(known_end));
        self.held.resize(self.held.len().saturating_add(gap), 0);
        let overlap = ms_to_samples(known_end.saturating_sub(window_start_ms));
        self.held
            .extend_from_slice(whole_ms.get(overlap..).unwrap_or_default());
        let ranges = self.ranges.feed(window_start_ms, len_ms, spans);
        self.attach(ranges)
    }

    pub fn finish(&mut self) -> Vec<Chunk> {
        let ranges = self.ranges.finish();
        self.attach(ranges)
    }

    pub fn carry_ms(&self) -> u64 {
        self.ranges.carry_ms()
    }

    /// Slices each range's audio out of `held`, then drops the audio before `base`.
    fn attach(&mut self, ranges: Vec<ChunkRange>) -> Vec<Chunk> {
        let chunks = ranges
            .into_iter()
            .map(|r| {
                let audio = slice(&self.held, self.held_start_ms, r.start_ms, r.end_ms);
                with_audio(r, audio)
            })
            .collect();
        let base = self.ranges.base.max(self.held_start_ms);
        let drop = ms_to_samples(base - self.held_start_ms).min(self.held.len());
        self.held.drain(..drop);
        self.held_start_ms = base;
        chunks
    }
}

fn with_audio(r: ChunkRange, audio: AudioBuffer) -> Chunk {
    Chunk {
        index: r.index,
        start_ms: r.start_ms,
        end_ms: r.end_ms,
        mode: r.mode,
        spans: r.spans,
        audio,
    }
}

/// The FR-024 fallback: re-cuts one already-cut (timed) chunk into time-less chunks from its
/// `spans` and `audio`. Each merged span is a chunk, cropped by [`PAD_MS`] and clamped to the
/// source chunk's range, a span over [`TIMELESS_MAX_MS`] cut hard; `index` counts from 0 in the
/// returned list, `start_ms`/`end_ms` stay track-relative, `mode` is `Timeless`.
pub fn recut_timeless(chunk: &Chunk) -> Vec<Chunk> {
    RangePlanner::resume(
        PassMode::Timeless,
        chunk.start_ms,
        chunk.end_ms,
        &chunk.spans,
    )
    .finish()
    .into_iter()
    .map(|r| {
        let audio = slice(chunk.audio.samples(), chunk.start_ms, r.start_ms, r.end_ms);
        with_audio(r, audio)
    })
    .collect()
}

// ---- the pass runner (T-138) -----------------------------------------------------------
//
// One pass is a function of (call folder, its `CallMeta`, one settings snapshot, one key
// read, the detectors). The calling thread is the dispatcher: it owns both decoders, one
// `ChunkPlanner` and a short queue of ready chunks per track, and sends at most
// `MAX_IN_FLIGHT` chunks at a time, each on its own scoped thread, in call-time start order
// (track offset + chunk start, mic first on a tie). Every window goes through the primary
// detector, or after its first `Err` (or with none) through energy spans for the rest of the
// track: an error is never an empty span list. Once a chunk answers `Timeless`, that chunk
// and every chunk dispatched after it are re-cut by `recut_timeless` and sent `Plain` (an
// answer with times of a later chunk is dropped and re-sent). A track's first failure stops
// its chunks; the other track runs on. The only files written are `transcript.md`, then
// `transcript.json`, then `meta.json`, each through `storage::write_atomic`, after
// `validate()` passed.

/// Requests in flight at once, drawn from both tracks (#187).
const MAX_IN_FLIGHT: usize = 3;

/// The one engine call the pass makes: a seam so scheduling tests need no HTTP.
pub trait TimedTranscriber: Send + Sync {
    fn transcribe_timed(
        &self,
        audio: &AudioBuffer,
        req: &TranscribeRequest,
        shape: ResponseShape,
    ) -> Result<Timed, TimedFailure>;
}

impl TimedTranscriber for OpenAiCompatibleEngine {
    fn transcribe_timed(
        &self,
        audio: &AudioBuffer,
        req: &TranscribeRequest,
        shape: ResponseShape,
    ) -> Result<Timed, TimedFailure> {
        // The inherent method (T-136); a path call resolves it before this trait method.
        OpenAiCompatibleEngine::transcribe_timed(self, audio, req, shape)
    }
}

/// Where the pass reports: log lines and the user notice.
pub trait PassEvents: Send + Sync {
    fn log(&self, event: LogEvent);
    fn notice(&self, notice: CallNotice);
}

/// The call folder and its `meta.json` as read (state `ready-to-transcribe`).
#[derive(Debug, Clone)]
pub struct PassCall {
    pub dir: PathBuf,
    pub meta: CallMeta,
}

/// What the pass needs besides the settings snapshot and the key store.
pub struct PassDeps<'a> {
    /// The primary detector; `None` = unavailable (energy fallback, cause `unavailable`).
    pub primary: Option<Arc<dyn SpeechSpans>>,
    pub events: &'a dyn PassEvents,
    pub clock: Arc<dyn MonotonicClock>,
}

/// How the pass ended and in which mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassOutcome {
    pub outcome: FinalOutcome,
    pub mode: PassMode,
}

/// The two decoded tracks (the lower entry takes them already open).
pub struct PassSources {
    pub mic: Box<dyn TrackDecoder>,
    pub system: Box<dyn TrackDecoder>,
}

/// The whole pass from `ready-to-transcribe`: builds the API engine (whatever engine is
/// active) once from `settings` and one read of the key, opens the tracks of `call.dir`, and
/// runs the pass of [`run_pass_with`] with them.
///
/// A base URL `check_base_url` refuses, or a missing or blank key, fails the pass with
/// `engine_not_set_up` before any request; a key store that cannot be read fails it with
/// `key`. Either way both track results are `failed{reason}` and `failure.track` is `None`.
pub fn run_pass(
    call: PassCall,
    settings: Arc<Settings>,
    creds: &dyn CredentialStore,
    deps: PassDeps<'_>,
) -> PassOutcome {
    let started = deps.clock.now();
    let engine = match api_engine(&settings, creds) {
        Ok(engine) => engine,
        Err(reason) => {
            let PassCall { dir, mut meta } = call;
            let failed = TrackResult::Failed { reason };
            meta.results = Results {
                mic: failed.clone(),
                system: failed,
            };
            let id = meta.id.clone();
            let reason = write_failed(&dir, meta, reason, None, PassMode::Timed, deps.events);
            return conclude(
                &deps,
                started,
                id,
                FinalOutcome::Failed { reason },
                PassMode::Timed,
            );
        }
    };
    let request = TranscribeRequest {
        language: settings.speech_language.clone(),
        timeouts: Timeouts::from_settings(&settings.timeouts),
    };
    let engine_ref = EngineRef::new("api", &settings.api.model);
    let sources = PassSources {
        mic: open_track(&call, Track::Mic),
        system: open_track(&call, Track::System),
    };
    pass(call, &engine, &request, engine_ref, sources, deps, started)
}

/// The pass from "write meta as transcribing" on, with the engine and the tracks given.
/// `engine_ref` is written to `meta.json` `engines.transcription` and `transcript.json`.
pub fn run_pass_with(
    call: PassCall,
    engine: &dyn TimedTranscriber,
    request: &TranscribeRequest,
    engine_ref: EngineRef,
    sources: PassSources,
    deps: PassDeps<'_>,
) -> PassOutcome {
    let started = deps.clock.now();
    pass(call, engine, request, engine_ref, sources, deps, started)
}

/// The API engine of the snapshot, or why there is none (D3).
fn api_engine(
    settings: &Settings,
    creds: &dyn CredentialStore,
) -> Result<OpenAiCompatibleEngine, FailReason> {
    let base_url =
        check_base_url(&settings.api.base_url).map_err(|_| FailReason::EngineNotSetUp)?;
    let key = creds
        .read(KeySlot::TranscriptionApi)
        .map_err(|_| FailReason::Key)?
        .filter(|key| !key.expose().trim().is_empty())
        .ok_or(FailReason::EngineNotSetUp)?;
    Ok(OpenAiCompatibleEngine::new(
        base_url,
        settings.api.model.clone(),
        Some(key),
    ))
}

/// A track that could not be opened: its first read is the error, so it fails the track
/// through the same path as a decode error.
struct Unopened(DecodeEnd);

impl TrackDecoder for Unopened {
    fn next_window(&mut self, _max_ms: u32) -> Result<Option<AudioBuffer>, DecodeEnd> {
        Err(self.0)
    }
}

/// The decoder of `track`'s file in the call folder (its `meta.json` file name, final
/// component only).
fn open_track(call: &PassCall, track: Track) -> Box<dyn TrackDecoder> {
    let name = call
        .meta
        .tracks
        .iter()
        .find(|t| t.track == track)
        .and_then(|t| Path::new(&t.file).file_name());
    let Some(name) = name else {
        return Box::new(Unopened(DecodeEnd::Unreadable));
    };
    match File::open(call.dir.join(name)) {
        Err(_) => Box::new(Unopened(DecodeEnd::Unreadable)),
        Ok(file) => match OpusOggReader::open(BufReader::new(file)) {
            Ok(reader) => Box::new(reader),
            Err(end) => Box::new(Unopened(end)),
        },
    }
}

fn ms_of(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

fn since(clock: &dyn MonotonicClock, t0: Instant) -> Duration {
    clock.now().saturating_duration_since(t0)
}

/// Logs `final_finished` and sends the notice: the one end of every pass.
fn conclude(
    deps: &PassDeps<'_>,
    started: Instant,
    call: CallId,
    outcome: FinalOutcome,
    mode: PassMode,
) -> PassOutcome {
    let ms = ms_of(since(&*deps.clock, started));
    deps.events
        .log(LogEvent::FinalPassFinished { outcome, mode, ms });
    deps.events.notice(match outcome {
        FinalOutcome::Ready => CallNotice::TranscriptReady { call },
        FinalOutcome::Failed { reason } => CallNotice::PassFailed { reason },
    });
    PassOutcome { outcome, mode }
}

fn storage_failed(events: &dyn PassEvents, kind: StorageKind, error: &io::Error) {
    events.log(LogEvent::CallStorageFailed {
        kind,
        os: error.raw_os_error(),
    });
}

/// Writes `meta` as `failed{reason, track}`; returns the reason the pass ends with:
/// `reason`, or `storage` when this write fails too (logged, never swallowed).
fn write_failed(
    dir: &Path,
    mut meta: CallMeta,
    reason: FailReason,
    track: Option<Track>,
    mode: PassMode,
    events: &dyn PassEvents,
) -> FailReason {
    meta.state = CallState::Failed;
    meta.failure = Some(Failure { reason, track });
    meta.pass_mode = Some(mode);
    match write_meta(dir, &meta) {
        Ok(()) => reason,
        Err(e) => {
            storage_failed(events, StorageKind::Meta, &e);
            FailReason::Storage
        }
    }
}

/// A chunk waiting to be sent, and how.
struct Work {
    chunk: Arc<Chunk>,
    shape: ResponseShape,
}

/// A request in flight.
struct Sent {
    track: Track,
    work: Work,
    at: Instant,
}

/// An answered chunk. `chunk` keeps the audio of a timed answer only while a `Timeless`
/// from an earlier dispatch could still make it be re-sent.
struct Answer {
    seq: u64,
    start_ms: u64,
    end_ms: u64,
    shape: ResponseShape,
    timed: Timed,
    chunk: Option<Arc<Chunk>>,
}

/// What a lane needs from the pass to read and cut its track.
struct Env<'a> {
    primary: Option<Arc<dyn SpeechSpans>>,
    energy: EnergyDetector,
    events: &'a dyn PassEvents,
    clock: Arc<dyn MonotonicClock>,
}

/// One track of the pass: its decoder, planner, ready chunks and answers.
struct Lane {
    track: Track,
    offset_ms: u64,
    decoder: Box<dyn TrackDecoder>,
    planner: ChunkPlanner,
    /// The track ms the next window starts at: the sum of the floored window lengths.
    pos_ms: u64,
    ended: bool,
    /// The track's first failure and its place in dispatch order.
    failed: Option<(FailReason, u64)>,
    /// The primary detector is no longer asked for this track (D2).
    fallen_back: bool,
    /// Ready chunks, sorted by start.
    queue: VecDeque<Work>,
    answers: Vec<Answer>,
    /// Chunks the planner cut for this track.
    chunks: u32,
    decode: Duration,
    vad: Duration,
    upload: Duration,
}

impl Lane {
    fn new(track: Track, offset_ms: u64, decoder: Box<dyn TrackDecoder>) -> Lane {
        Lane {
            track,
            offset_ms,
            decoder,
            planner: ChunkPlanner::new(PassMode::Timed),
            pos_ms: 0,
            ended: false,
            failed: None,
            fallen_back: false,
            queue: VecDeque::new(),
            answers: Vec::new(),
            chunks: 0,
            decode: Duration::ZERO,
            vad: Duration::ZERO,
            upload: Duration::ZERO,
        }
    }

    fn live(&self) -> bool {
        self.failed.is_none()
    }

    /// The call-time start of the next chunk to send, if any.
    fn head(&self) -> Option<u64> {
        if !self.live() {
            return None;
        }
        self.queue
            .front()
            .map(|w| self.offset_ms.saturating_add(w.chunk.start_ms))
    }

    fn fail(&mut self, reason: FailReason, order: u64) {
        if self.failed.is_none() {
            self.failed = Some((reason, order));
        }
        self.queue.clear();
    }

    /// Decodes window after window until a chunk is ready, the track ends or fails. One
    /// window at a time, asked for `TIMED_MAX_MS - carry_ms()` ms.
    fn refill(&mut self, env: &Env<'_>, timeless: bool, order: u64) {
        while self.queue.is_empty() && !self.ended && self.live() {
            let ask = TIMED_MAX_MS.saturating_sub(self.planner.carry_ms()).max(1);
            let ask = u32::try_from(ask).unwrap_or(u32::MAX);
            let t0 = env.clock.now();
            let window = self.decoder.next_window(ask);
            self.decode += since(&*env.clock, t0);
            let chunks = match window {
                Ok(Some(audio)) if !audio.samples().is_empty() => {
                    let spans = self.spans(env, &audio);
                    let at = self.pos_ms;
                    let len_ms = (audio.samples().len() / SAMPLES_PER_MS) as u64;
                    self.pos_ms = at.saturating_add(len_ms);
                    self.planner.feed(at, &audio, &spans)
                }
                // The end, or a damaged tail after the readable prefix (D5).
                Ok(_) | Err(DecodeEnd::Damaged) => {
                    self.ended = true;
                    self.planner.finish()
                }
                Err(DecodeEnd::Unreadable | DecodeEnd::Codec) => {
                    self.fail(FailReason::Storage, order);
                    Vec::new()
                }
            };
            self.chunks = self
                .chunks
                .saturating_add(u32::try_from(chunks.len()).unwrap_or(u32::MAX));
            for chunk in chunks {
                if timeless {
                    self.requeue(&chunk);
                } else {
                    self.queue.push_back(Work {
                        chunk: Arc::new(chunk),
                        shape: ResponseShape::Segments,
                    });
                }
            }
        }
    }

    /// The window's speech spans (D2): the primary's `Ok`; on its first `Err` (or with no
    /// primary) one `vad_fallback` and energy spans for this window and the rest of the
    /// track. An energy error keeps the whole window as speech, never drops it.
    fn spans(&mut self, env: &Env<'_>, audio: &AudioBuffer) -> Vec<SpeechSpan> {
        if !self.fallen_back {
            let cause = match &env.primary {
                Some(primary) => {
                    let t0 = env.clock.now();
                    let got = primary.speech_spans(audio);
                    self.vad += since(&*env.clock, t0);
                    match got {
                        Ok(spans) => return spans,
                        Err(VadError::Unavailable) => VadFallbackCause::Unavailable,
                        Err(_) => VadFallbackCause::Error,
                    }
                }
                None => VadFallbackCause::Unavailable,
            };
            env.events.log(LogEvent::VadFallback {
                track: self.track,
                cause,
            });
            self.fallen_back = true;
        }
        let t0 = env.clock.now();
        let got = env.energy.speech_spans(audio);
        self.vad += since(&*env.clock, t0);
        got.unwrap_or_else(|_| {
            vec![SpeechSpan {
                start_ms: 0,
                end_ms: (audio.samples().len() / SAMPLES_PER_MS) as u64,
            }]
        })
    }

    /// Queues `chunk` again as time-less pieces sent `Plain`, in start order.
    fn requeue(&mut self, chunk: &Chunk) {
        if !self.live() {
            return;
        }
        for piece in recut_timeless(chunk) {
            self.queue.push_back(Work {
                chunk: Arc::new(piece),
                shape: ResponseShape::Plain,
            });
        }
        self.queue
            .make_contiguous()
            .sort_by_key(|w| w.chunk.start_ms);
    }

    /// The chunks of the answers, in start order, as `merge` takes them.
    fn inputs(&self) -> Vec<ChunkInput> {
        let mut answers: Vec<&Answer> = self.answers.iter().collect();
        answers.sort_by_key(|a| a.start_ms);
        answers
            .into_iter()
            .map(|a| ChunkInput {
                start_ms: a.start_ms,
                end_ms: a.end_ms,
                text: a.timed.text.clone(),
                lang: a.timed.language,
                segments: match a.shape {
                    ResponseShape::Segments => a.timed.segments.clone(),
                    ResponseShape::Plain => None,
                },
            })
            .collect()
    }

    fn log_stage(&self, events: &dyn PassEvents, stage: FinalStage, d: Duration) {
        events.log(LogEvent::FinalPassStage {
            track: self.track,
            stage,
            ms: ms_of(d),
            chunks: self.chunks,
        });
    }
}

/// The dispatcher's state over both lanes.
struct Run {
    mic: Lane,
    system: Lane,
    mode: PassMode,
    next_seq: u64,
    in_flight: BTreeMap<u64, Sent>,
    /// In-flight requests whose answer is dropped (they were re-queued time-less).
    discarded: BTreeSet<u64>,
}

type Reply = (u64, std::thread::Result<Result<Timed, TimedFailure>>);

impl Run {
    fn lane(&mut self, track: Track) -> &mut Lane {
        match track {
            Track::Mic => &mut self.mic,
            Track::System => &mut self.system,
        }
    }

    /// The track whose next chunk starts first on the call timeline; mic first on a tie.
    fn pick(&self) -> Option<Track> {
        match (self.mic.head(), self.system.head()) {
            (Some(mic), Some(system)) if system < mic => Some(Track::System),
            (Some(_), _) => Some(Track::Mic),
            (None, Some(_)) => Some(Track::System),
            (None, None) => None,
        }
    }

    fn drive(&mut self, env: &Env<'_>, engine: &dyn TimedTranscriber, request: &TranscribeRequest) {
        std::thread::scope(|scope| {
            let (tx, rx) = mpsc::channel::<Reply>();
            loop {
                while self.in_flight.len() < MAX_IN_FLIGHT {
                    let timeless = self.mode == PassMode::Timeless;
                    let order = self.next_seq;
                    self.mic.refill(env, timeless, order);
                    self.system.refill(env, timeless, order);
                    let Some(track) = self.pick() else { break };
                    let Some(work) = self.lane(track).queue.pop_front() else {
                        break;
                    };
                    let seq = self.next_seq;
                    self.next_seq += 1;
                    let (chunk, shape, tx) = (work.chunk.clone(), work.shape, tx.clone());
                    scope.spawn(move || {
                        let reply = panic::catch_unwind(AssertUnwindSafe(|| {
                            engine.transcribe_timed(&chunk.audio, request, shape)
                        }));
                        // The receiver lives until every request is answered.
                        let _ = tx.send((seq, reply));
                    });
                    let at = env.clock.now();
                    self.in_flight.insert(seq, Sent { track, work, at });
                }
                if self.in_flight.is_empty() {
                    break;
                }
                let Ok((seq, reply)) = rx.recv() else { break };
                match reply {
                    Ok(answer) => self.on_answer(env, seq, answer),
                    // An engine panic is the engine's bug: re-raised, not turned into a reason.
                    Err(payload) => panic::resume_unwind(payload),
                }
            }
        });
    }

    fn on_answer(&mut self, env: &Env<'_>, seq: u64, answer: Result<Timed, TimedFailure>) {
        let Some(sent) = self.in_flight.remove(&seq) else {
            return;
        };
        let took = since(&*env.clock, sent.at);
        let discarded = self.discarded.remove(&seq);
        let lane = self.lane(sent.track);
        lane.upload += took;
        if !discarded && lane.live() {
            match answer {
                Ok(timed) => {
                    let keep = sent.work.shape == ResponseShape::Segments;
                    lane.answers.push(Answer {
                        seq,
                        start_ms: sent.work.chunk.start_ms,
                        end_ms: sent.work.chunk.end_ms,
                        shape: sent.work.shape,
                        timed,
                        chunk: keep.then_some(sent.work.chunk),
                    });
                }
                Err(TimedFailure::Timeless) if sent.work.shape == ResponseShape::Segments => {
                    self.go_timeless(seq, &sent);
                }
                // A plain request has no times to refuse: the server is at fault.
                Err(TimedFailure::Timeless) => lane.fail(FailReason::Provider, seq),
                Err(TimedFailure::Failed(reason)) => lane.fail(reason, seq),
            }
        }
        self.release_audio();
    }

    /// Chunk `k` answered without times (D1): it and every chunk dispatched after it go
    /// time-less, answered, in flight or still queued; chunks before it keep their times.
    fn go_timeless(&mut self, k: u64, sent: &Sent) {
        self.mode = PassMode::Timeless;
        self.lane(sent.track).requeue(&sent.work.chunk);
        let later: Vec<(u64, Track, Arc<Chunk>)> = self
            .in_flight
            .iter()
            .filter(|(seq, s)| {
                **seq > k
                    && s.work.shape == ResponseShape::Segments
                    && !self.discarded.contains(seq)
            })
            .map(|(seq, s)| (*seq, s.track, s.work.chunk.clone()))
            .collect();
        for (seq, track, chunk) in later {
            self.discarded.insert(seq);
            self.lane(track).requeue(&chunk);
        }
        for lane in [&mut self.mic, &mut self.system] {
            let answers = std::mem::take(&mut lane.answers);
            for answer in answers {
                match &answer.chunk {
                    Some(chunk) if answer.seq > k && answer.shape == ResponseShape::Segments => {
                        let chunk = chunk.clone();
                        lane.requeue(&chunk);
                    }
                    _ => lane.answers.push(answer),
                }
            }
            let queued = std::mem::take(&mut lane.queue);
            for work in queued {
                match work.shape {
                    ResponseShape::Segments => lane.requeue(&work.chunk),
                    ResponseShape::Plain => lane.queue.push_back(work),
                }
            }
            lane.queue
                .make_contiguous()
                .sort_by_key(|w| w.chunk.start_ms);
        }
    }

    /// Drops the audio of timed answers no `Timeless` can reach any more: those dispatched
    /// before the earliest timed request still in flight (or all, with none in flight).
    fn release_audio(&mut self) {
        let floor = self
            .in_flight
            .iter()
            .find(|(seq, s)| {
                s.work.shape == ResponseShape::Segments && !self.discarded.contains(seq)
            })
            .map(|(seq, _)| *seq);
        for lane in [&mut self.mic, &mut self.system] {
            for answer in &mut lane.answers {
                if floor.is_none_or(|f| answer.seq < f) {
                    answer.chunk = None;
                }
            }
        }
    }
}

/// The pass from "write meta as transcribing" on.
fn pass(
    call: PassCall,
    engine: &dyn TimedTranscriber,
    request: &TranscribeRequest,
    engine_ref: EngineRef,
    sources: PassSources,
    deps: PassDeps<'_>,
    started: Instant,
) -> PassOutcome {
    let PassCall { dir, mut meta } = call;
    let events = deps.events;
    let id = meta.id.clone();
    meta.state = CallState::Transcribing;
    meta.engines.transcription = Some(engine_ref.clone());
    if let Err(e) = write_meta(&dir, &meta) {
        storage_failed(events, StorageKind::Meta, &e);
        let _ = write_failed(
            &dir,
            meta,
            FailReason::Storage,
            None,
            PassMode::Timed,
            events,
        );
        let outcome = FinalOutcome::Failed {
            reason: FailReason::Storage,
        };
        return conclude(&deps, started, id, outcome, PassMode::Timed);
    }

    let offset = |track: Track| {
        meta.tracks
            .iter()
            .find(|t| t.track == track)
            .map_or(0, |t| t.start_offset_ms)
    };
    let mut run = Run {
        mic: Lane::new(Track::Mic, offset(Track::Mic), sources.mic),
        system: Lane::new(Track::System, offset(Track::System), sources.system),
        mode: PassMode::Timed,
        next_seq: 0,
        in_flight: BTreeMap::new(),
        discarded: BTreeSet::new(),
    };
    let env = Env {
        primary: deps.primary.clone(),
        energy: EnergyDetector::new(),
        events,
        clock: deps.clock.clone(),
    };
    run.drive(&env, engine, request);
    let mode = run.mode;
    for lane in [&run.mic, &run.system] {
        lane.log_stage(events, FinalStage::Decode, lane.decode);
        lane.log_stage(events, FinalStage::Vad, lane.vad);
        lane.log_stage(events, FinalStage::Upload, lane.upload);
    }

    // Merge the tracks that are done; the lines of each are its result (D4).
    let t0 = deps.clock.now();
    let duration_ms = meta.duration_ms.unwrap_or_else(|| {
        [&run.mic, &run.system]
            .iter()
            .map(|l| l.offset_ms.saturating_add(l.pos_ms))
            .min()
            .unwrap_or(0)
    });
    let merged = merge(&MergeInput {
        call_id: meta.id.clone(),
        title: meta.title.clone(),
        started_at: meta.started_at.clone(),
        duration_ms,
        speakers: meta.speakers.clone(),
        engine: engine_ref,
        tracks: [&run.mic, &run.system]
            .into_iter()
            .filter(|l| l.live())
            .map(|l| TrackInput {
                track: l.track,
                start_offset_ms: l.offset_ms,
                chunks: l.inputs(),
            })
            .collect(),
    });
    let result_of = |lane: &Lane| match lane.failed {
        Some((reason, _)) => TrackResult::Failed { reason },
        None => TrackResult::Done {
            segments: merged
                .segments
                .iter()
                .filter(|s| s.track == lane.track.as_str())
                .cloned()
                .collect(),
        },
    };
    meta.results = Results {
        mic: result_of(&run.mic),
        system: result_of(&run.system),
    };
    let merge_took = since(&*deps.clock, t0);
    for lane in [&run.mic, &run.system] {
        lane.log_stage(events, FinalStage::Merge, merge_took);
    }

    let t0 = deps.clock.now();
    let outcome = match (run.mic.failed, run.system.failed) {
        (None, None) => write_ready(&dir, meta, &merged, mode, events),
        (failed_mic, failed_system) => {
            let (reason, track) = match (failed_mic, failed_system) {
                (Some((reason, _)), None) => (reason, Some(Track::Mic)),
                (None, Some((reason, _))) => (reason, Some(Track::System)),
                (Some(m), Some(s)) => (if s.1 < m.1 { s.0 } else { m.0 }, None),
                (None, None) => (FailReason::Storage, None),
            };
            let reason = write_failed(&dir, meta, reason, track, mode, events);
            FinalOutcome::Failed { reason }
        }
    };
    let write_took = since(&*deps.clock, t0);
    for lane in [&run.mic, &run.system] {
        lane.log_stage(events, FinalStage::Write, write_took);
    }
    conclude(&deps, started, id, outcome, mode)
}

/// Both tracks are done (D6): validate, render, then `transcript.md`, `transcript.json` and
/// `meta.json` (`ready`), each whole through `write_atomic`. Any failure is logged and gives
/// `failed: storage` with a best-effort failed `meta.json`.
fn write_ready(
    dir: &Path,
    mut meta: CallMeta,
    transcript: &TranscriptV1,
    mode: PassMode,
    events: &dyn PassEvents,
) -> FinalOutcome {
    let failed = |meta: CallMeta| {
        let _ = write_failed(dir, meta, FailReason::Storage, None, mode, events);
        FinalOutcome::Failed {
            reason: FailReason::Storage,
        }
    };
    if let Err((rule, segment)) = transcript.validate() {
        events.log(LogEvent::TranscriptInvalid { rule, segment });
        return failed(meta);
    }
    let markdown = render_markdown(transcript, &meta.title, &meta.speakers);
    for (name, bytes) in [
        (TRANSCRIPT_MD_FILE, markdown.into_bytes()),
        (TRANSCRIPT_FILE, transcript.to_json().into_bytes()),
    ] {
        if let Err(e) = write_atomic(dir, name, &bytes) {
            storage_failed(events, StorageKind::Transcript, &e);
            return failed(meta);
        }
    }
    meta.state = CallState::Ready;
    meta.failure = None;
    meta.pass_mode = Some(mode);
    match write_meta(dir, &meta) {
        Ok(()) => FinalOutcome::Ready,
        Err(e) => {
            storage_failed(events, StorageKind::Meta, &e);
            failed(meta)
        }
    }
}

#[cfg(test)]
mod pass_tests;

#[cfg(test)]
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
        let [m] = merged.as_slice() else {
            panic!("expected one chunk: {merged:?}")
        };
        assert_eq!(m.spans, vec![sp(1_000, 3_000)], "gap 299 merges");

        let apart = plan(
            PassMode::Timed,
            10_000,
            &[sp(1_000, 2_000), sp(2_300, 3_000)],
        );
        assert_eq!(apart.len(), 1, "timed packs both into one chunk");
        let [a] = apart.as_slice() else {
            panic!("expected one chunk: {apart:?}")
        };
        assert_eq!(
            a.spans,
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
        let [f] = first.as_slice() else {
            panic!("expected one chunk: {first:?}")
        };
        assert_eq!(f.index, 0);
        assert_eq!((f.start_ms, f.end_ms), (0, 100_200));
        assert_eq!(f.spans, vec![sp(0, 50_000), sp(51_000, 100_000)]);
        assert_eq!(p.carry_ms(), 120_000 - 100_200, "the rest of S3 is carried");

        // The decoder is asked for 120 000 - carry; the track has 20 s left.
        let more = p.feed(120_000, 20_000, &[sp(0, 10_000)]);
        assert!(
            more.is_empty(),
            "a window short of the limit waits for more"
        );
        let last = p.finish();
        assert_eq!(last.len(), 1);
        let [l] = last.as_slice() else {
            panic!("expected one chunk: {last:?}")
        };
        assert_eq!(l.index, 1);
        assert_eq!((l.start_ms, l.end_ms), (100_200, 130_200));
        assert_eq!(
            l.spans,
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
        assert_eq!(bounds(&got), vec![(800, 5_200)]);
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
        let [c1] = c1.as_slice() else {
            panic!("expected one chunk: {c1:?}")
        };
        assert_eq!(
            (c1.start_ms, c1.end_ms),
            (0, 120_000),
            "no pad past the limit"
        );
        assert_eq!(c1.spans, vec![sp(0, 120_000)]);
        assert_eq!(p.carry_ms(), 0);

        let c2 = p.feed(120_000, &ramp(120_000, 120_000), &[whole]);
        assert_eq!(c2.len(), 1);
        let [c2] = c2.as_slice() else {
            panic!("expected one chunk: {c2:?}")
        };
        assert_eq!(c2.index, 1);
        assert_eq!(
            (c2.start_ms, c2.end_ms),
            (120_000, 240_000),
            "no pad into chunk 1"
        );
        assert_eq!(p.carry_ms(), 0);

        let c3 = p.feed(240_000, &ramp(240_000, 60_000), &[sp(0, 60_000)]);
        assert!(c3.is_empty(), "the last 60 s wait for the end of the track");
        let last = p.finish();
        assert_eq!(last.len(), 1);
        let [l] = last.as_slice() else {
            panic!("expected one chunk: {last:?}")
        };
        assert_eq!(l.index, 2);
        assert_eq!((l.start_ms, l.end_ms), (240_000, 300_000));
        assert_eq!(l.audio.samples().len(), 60_000 * 16);
        assert_eq!(
            l.audio,
            ramp(240_000, 60_000),
            "the carry kept the right audio"
        );
        assert_eq!(c1.audio, ramp(0, 120_000));
        assert_eq!(c2.audio, ramp(120_000, 120_000));
    }

    #[test]
    fn a_hard_cut_carries_the_rest_of_a_span_beyond_the_limit() {
        // A window larger than the caller's budget (never asked for, but fed): the span's rest
        // after the hard cut is kept and comes out at the end of the track.
        let mut p = ChunkPlanner::new(PassMode::Timed);
        let c1 = p.feed(0, &ramp(0, 150_000), &[sp(0, 150_000)]);
        assert_eq!(c1.len(), 1);
        let [c1] = c1.as_slice() else {
            panic!("expected one chunk: {c1:?}")
        };
        assert_eq!((c1.start_ms, c1.end_ms), (0, 120_000));
        assert_eq!(c1.spans, vec![sp(0, 120_000)]);
        assert_eq!(p.carry_ms(), 30_000, "the rest of the span is carried");
        let last = p.finish();
        assert_eq!(last.len(), 1, "the carried rest is not lost");
        let [l] = last.as_slice() else {
            panic!("expected one chunk: {last:?}")
        };
        assert_eq!((l.start_ms, l.end_ms), (120_000, 150_000));
        assert_eq!(l.spans, vec![sp(120_000, 150_000)]);
        assert_eq!(l.audio, ramp(120_000, 30_000));
    }

    #[test]
    fn a_trailing_silence_of_exactly_300_ms_closes_the_tail_span_and_299_does_not() {
        let mut closed = RangePlanner::new(PassMode::Timed);
        let got = closed.feed(0, 120_000, &[sp(1_000, 2_000), sp(50_000, 119_700)]);
        assert_eq!(
            bounds(&got),
            vec![(800, 119_900)],
            "300 ms of silence closes"
        );
        assert_eq!(
            got.iter().map(|c| c.spans.clone()).collect::<Vec<_>>(),
            vec![vec![sp(1_000, 2_000), sp(50_000, 119_700)]]
        );

        let mut open = RangePlanner::new(PassMode::Timed);
        let got = open.feed(0, 120_000, &[sp(1_000, 2_000), sp(50_000, 119_701)]);
        assert_eq!(
            bounds(&got),
            vec![(800, 2_200)],
            "299 ms leaves the tail span open: cut at the last pause before it"
        );
        assert_eq!(
            got.iter().map(|c| c.spans.clone()).collect::<Vec<_>>(),
            vec![vec![sp(1_000, 2_000)]]
        );
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
        let [g] = got.as_slice() else {
            panic!("expected one chunk: {got:?}")
        };
        assert_eq!((g.start_ms, g.end_ms), (119_800, 121_200));
        assert_eq!(
            g.audio,
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
        assert_eq!(
            got.iter().map(|c| c.spans.clone()).collect::<Vec<_>>(),
            vec![vec![sp(120_500, 121_000)]]
        );
    }

    #[test]
    fn a_span_touching_the_window_edge_continues_into_the_next_window() {
        // 60 s of speech split by the decoder's window at 50 s: one span, one chunk.
        let mut p = RangePlanner::new(PassMode::Timed);
        assert!(p.feed(0, 50_000, &[sp(10_000, 50_000)]).is_empty());
        assert!(p.feed(50_000, 40_000, &[sp(0, 10_000)]).is_empty());
        let got = p.finish();
        assert_eq!(got.len(), 1);
        assert_eq!(
            got.iter().map(|c| c.spans.clone()).collect::<Vec<_>>(),
            vec![vec![sp(10_000, 60_000)]]
        );
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
            got.iter().map(|c| c.spans.clone()).collect::<Vec<_>>(),
            vec![vec![sp(1_000, 2_500), sp(5_000, 6_000), sp(9_500, 10_000)]]
        );
        assert_eq!(bounds(&got), vec![(800, 10_000)]);
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
        let [t] = timed.as_slice() else {
            panic!("expected one chunk: {timed:?}")
        };
        assert_eq!((t.start_ms, t.end_ms), (800, 60_200));
        assert_eq!(t.spans, track.to_vec());

        let pieces = recut_timeless(t);
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
        let [_, p1, p2] = pieces.as_slice() else {
            panic!("expected three pieces: {pieces:?}")
        };
        assert_eq!(p1.spans, vec![sp(20_500, 50_300)]);
        assert_eq!(p2.spans, vec![sp(50_300, 60_000)]);
    }

    // ---- WAV --------------------------------------------------------------------------

    #[test]
    fn a_maximal_chunk_is_a_3_840_044_byte_16k_mono_pcm16_wav() {
        let mut p = ChunkPlanner::new(PassMode::Timed);
        let got = p.feed(0, &ramp(0, 120_000), &[sp(0, 120_000)]);
        assert_eq!(got.len(), 1);
        let [g] = got.as_slice() else {
            panic!("expected one chunk: {got:?}")
        };
        assert_eq!(g.audio.samples().len(), 1_920_000);
        let bytes = wav::encode(&g.audio);
        assert_eq!(bytes.len(), 3_840_044);
        assert_eq!(bytes.len() as u64, 44 + 32 * (g.end_ms - g.start_ms));
        assert_eq!(bytes.get(0..4), Some(&b"RIFF"[..]));
        assert_eq!(bytes.get(8..12), Some(&b"WAVE"[..]));
        let u16_at = |i: usize| {
            bytes
                .get(i..i + 2)
                .and_then(|b| <[u8; 2]>::try_from(b).ok())
                .map(u16::from_le_bytes)
        };
        let u32_at = |i: usize| {
            bytes
                .get(i..i + 4)
                .and_then(|b| <[u8; 4]>::try_from(b).ok())
                .map(u32::from_le_bytes)
        };
        assert_eq!(u16_at(20), Some(1), "PCM");
        assert_eq!(u16_at(22), Some(1), "mono");
        assert_eq!(u32_at(24), Some(16_000));
        assert_eq!(u16_at(34), Some(16), "16-bit");
        assert!(bytes.len() < 25_000_000);

        let mut t = ChunkPlanner::new(PassMode::Timeless);
        let tl = t.feed(0, &ramp(0, 30_000), &[sp(0, 30_000)]);
        let [tl] = tl.as_slice() else {
            panic!("expected one chunk: {tl:?}")
        };
        assert_eq!(
            wav::encode(&tl.audio).len(),
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

    /// Feeds a `ChunkPlanner` budget-sized windows like T-138 (`max - carry` ms each) and
    /// checks after every `feed` and after `finish` that it holds only `[base, known_end)`:
    /// at most one window plus its carry, i.e. `<= max_chunk_ms(mode)` of audio, exactly
    /// `carry_ms()` of it. Returns the peak held ms.
    fn assert_held_audio_is_bounded(mode: PassMode, track_len: u64, track: &[SpeechSpan]) -> u64 {
        let max = max_chunk_ms(mode);
        let check = |p: &ChunkPlanner, when: &str| -> u64 {
            let held_ms = (p.held.len() / SAMPLES_PER_MS) as u64;
            assert_eq!(p.held.len() % SAMPLES_PER_MS, 0, "{when}: whole ms held");
            assert!(held_ms <= max, "{when}: {held_ms} ms held, limit {max} ms");
            assert_eq!(held_ms, p.carry_ms(), "{when}: held audio is the carry");
            assert_eq!(
                p.held_start_ms, p.ranges.base,
                "{when}: held starts at base"
            );
            assert_eq!(
                p.held_start_ms + held_ms,
                p.ranges.known_end,
                "{when}: held ends at the end of the last window"
            );
            held_ms
        };
        let mut planner = ChunkPlanner::new(mode);
        let (mut pos, mut peak, mut chunks) = (0, 0, 0);
        while pos < track_len {
            let len = (max - planner.carry_ms()).min(track_len - pos);
            chunks += planner
                .feed(pos, &ramp(pos, len), &window_spans(track, pos, len))
                .len();
            pos += len;
            peak = peak.max(check(&planner, &format!("after feed to {pos} ms")));
        }
        chunks += planner.finish().len();
        peak = peak.max(check(&planner, "after finish"));
        assert!(chunks > 0, "the track has speech, so chunks came out");
        peak
    }

    /// 12 minutes: dense speech, a long span-less stretch, a pause-less run over the limit,
    /// more silence, then speech with pauses to the end.
    fn long_track_with_silences() -> (u64, Vec<SpeechSpan>) {
        let mut track = Vec::new();
        let mut t = 500;
        while t + 800 <= 20_000 {
            track.push(sp(t, t + 800));
            t += 1_200;
        }
        track.push(sp(200_000, 340_000)); // pause-less 140 s
        let mut t = 400_000;
        while t + 6_000 <= 720_000 {
            track.push(sp(t, t + 6_000));
            t += 6_500;
        }
        (720_000, track)
    }

    #[test]
    fn timed_planner_never_holds_more_than_one_window_plus_its_carry() {
        let (track_len, track) = long_track_with_silences();
        let peak = assert_held_audio_is_bounded(PassMode::Timed, track_len, &track);
        assert!(peak > 0, "the planner held something");
        // And a track of silence only: nothing is emitted, the lead-in is all that is kept.
        let mut planner = ChunkPlanner::new(PassMode::Timed);
        let mut pos = 0;
        while pos < 600_000 {
            let len = (TIMED_MAX_MS - planner.carry_ms()).min(600_000 - pos);
            assert!(planner.feed(pos, &ramp(pos, len), &[]).is_empty());
            pos += len;
            assert!(
                planner.held.len() <= ms_to_samples(PAD_MS),
                "silence is not kept"
            );
        }
        assert!(planner.finish().is_empty());
        assert!(planner.held.len() <= ms_to_samples(PAD_MS));
    }

    #[test]
    fn timeless_planner_never_holds_more_than_one_window_plus_its_carry() {
        let (track_len, track) = long_track_with_silences();
        let peak = assert_held_audio_is_bounded(PassMode::Timeless, track_len, &track);
        assert!(peak > 0, "the planner held something");
    }
}
