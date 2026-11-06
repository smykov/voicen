# Call final pass (the runner that turns a stopped call into a transcript)

**Code:** `crates/voicen-core/src/call/final_pass.rs` (`run_pass`, `run_pass_with`, `TimedTranscriber`, `PassEvents`, `PassCall`, `PassDeps`, `PassOutcome`, `PassSources`; the T-137 planners `ChunkPlanner`, `RangePlanner`, `recut_timeless` live in the same file), `crates/voicen-core/src/call/storage.rs` (`TrackResult::{Pending, Done, Failed}`) · **Tests that pin it:** `call::final_pass::pass_tests` (unit, fakes), `crates/voicen-core/tests/final_pass_http.rs` (wiremock, real Opus, real key path)

Tasks: T-138 (class `call-final-pass`; the queue, cancel and "Transcribe again" are T-146, the shell wiring T-139). Contract: `specs/007-call-mode-record/contracts/core-call.md` "Built (T-138)". Decisions: #187, #188, #197. Class `call-final-pass` in `docs/failures.md` has no entry yet (first sighting).

## Invariants

### A pass is one function of its inputs, and everything it reaches goes through one path

- **Defect that produced it:** none yet (found in analysis, T-138). The building blocks (T-123, T-125, T-134, T-136, T-137) had no caller; the nearest history is review findings: T-134 r1#1 (a primary `Err` must never become empty spans), T-136 r1#2 (`EngineNotSetUp` needs a call-level test), T-137 r2#1 and r2#2 (planner contiguity).
- **What breaks if you violate it:** a settings or key change mid-pass makes one transcript from two engines; a track's chunks are cut or re-ordered by a second rule; a request goes out with no key.
- **Where it is enforced:** `run_pass` takes one `Settings` snapshot and makes one key read, builds the API engine once (not `engine_for`, which follows `settings.engine` and treats a missing key as "no header"), and passes the same `TranscribeRequest` to every chunk. Cutting is `ChunkPlanner` and `recut_timeless` only; lines are built by `transcript::merge` only; files go through `storage::write_atomic` only. Tests `a_key_saved_during_the_pass_is_not_picked_up`, `a_builtin_engine_with_no_api_key_fails_engine_not_set_up_before_any_request`.
- **Don't:** call `engine_for`, read the key or the settings again per chunk, build `SegmentV1` by hand, or cut a window outside the planner.

### At most 3 requests in flight, from both tracks, in call-time start order, with one window plus carry held per track

- **Defect that produced it:** none yet (decision #187).
- **What breaks if you violate it:** serial or per-track workers take longer than the call; a longer queue or a whole-track decode holds an hour of audio in memory; a track with a late start offset is sent before an earlier chunk of the other.
- **Where it is enforced:** `Run::drive` (the dispatcher is the calling thread, `MAX_IN_FLIGHT` = 3, `Run::pick` compares offset + chunk start, mic first on a tie); `Lane::refill` asks the decoder for `TIMED_MAX_MS - carry_ms()` and advances `pos_ms` by the floored window length (the planner's own rule), so a ragged decoder cannot drift. Tests `three_requests_in_flight_from_both_tracks_in_call_time_order_with_mic_first_on_a_tie`, `dispatch_order_uses_the_system_start_offset`, `decoding_asks_one_window_at_a_time_and_keeps_audio_whole`, `a_ragged_decoder_still_gives_whole_chunks`, `a_pauseless_track_is_read_in_windows_that_leave_room_for_the_carry`, `a_thirty_minute_pair_is_decoded_in_windows_and_uploaded_in_bounded_chunks`.
- **Don't:** ask for a window without subtracting the carry, give each track its own pool, or decode ahead more than one window.

### A primary detector `Err` is never empty spans or dropped audio

- **Defect that produced it:** T-134 r1#1 (a speech port whose error reads as "no speech" drops audio silently).
- **What breaks if you violate it:** a call where Silero failed transcribes as silence, with no log line.
- **Where it is enforced:** `Lane::spans`: the primary's `Ok`, else `EnergyDetector` spans for the same window and the rest of the track, with one `vad_fallback` per track (D2). Tests `no_primary_detector_uses_energy_spans_and_logs_the_fallback_per_track`, `a_primary_error_gives_energy_spans_for_the_window_and_the_rest_of_the_track`, `an_unavailable_primary_on_a_window_logs_cause_unavailable`.
- **Don't:** `unwrap_or_default()` a span result, or retry the primary after its first `Err`.

### Once a chunk answers `Timeless`, that chunk and every chunk after it in dispatch order is time-less, never back

- **Defect that produced it:** none yet (FR-024, decision #197).
- **What breaks if you violate it:** lines of a later chunk carry segment times the server does not support, or one chunk is time-less and its neighbours timed so lines overlap or jump.
- **Where it is enforced:** `Run::go_timeless` (D1). Tests `a_400_on_chunk_k_makes_chunk_k_and_every_later_chunk_timeless`, `a_chunk_answered_with_times_before_chunk_k_said_no_times_is_sent_again_time_less`, `a_plain_request_that_is_refused_for_times_fails_the_track_with_provider`.
- **Don't:** flip only chunk k, keep an answer with times from a chunk dispatched after k, or re-cut with a second cutting rule (the planners stay `Timed`; later chunks are re-cut before dispatch).

### A track's first failure stops that track only; no `transcript.*` unless both tracks are done

- **Defect that produced it:** none yet (FR-027).
- **What breaks if you violate it:** a half transcript is presented as the call's, or one track's failure throws away the other track's finished work.
- **Where it is enforced:** `Lane::fail` (D4); `pass` merges only live tracks, stores `done{segments}` for the good track and `failed{reason}` for the other, and writes `transcript.*` only when neither failed. Tests `a_tracks_first_failure_cancels_its_remaining_chunks_and_the_other_track_finishes`, `when_both_tracks_fail_the_reason_is_the_first_failure_in_dispatch_order`, `a_codec_error_fails_the_track_with_storage`, `an_unreadable_track_fails_with_storage`, `a_damaged_tail_ends_the_track_with_the_prefix_it_returned`.
- **Don't:** keep dispatching a failed track's queue, report a half transcript as `ready`, or turn a decode error into the end of the track.

### The only files written are `transcript.md`, `transcript.json`, `meta.json`, in that order, each whole, after `validate()`; a write failure is `failed: storage`

- **Defect that produced it:** none yet (FR-018; swallowed write errors are the class every swallow test in the task bites).
- **What breaks if you violate it:** a partial or unvalidated transcript on disk; `ready` in `meta.json` for a transcript that was not written; a failure that nobody sees.
- **Where it is enforced:** `write_ready` (D6) and `write_failed`; every error is logged as `call_storage_failed kind=transcript | meta` or `transcript_invalid`, never swallowed. Tests `a_failed_markdown_write_leaves_failed_storage_and_no_transcript`, `a_failed_json_write_leaves_failed_storage_and_no_transcript_json`, `a_failed_meta_write_is_reported_not_swallowed`, `a_failed_meta_write_after_a_track_failure_is_reported_and_ends_with_storage`, `a_transcript_that_fails_validation_is_logged_and_not_written`.
- **Don't:** write a file in place, write `meta.json` `ready` before both transcript files, `let _ =` a write result without a log, or log transcript text.

### One `final_finished` and one notice end every pass

- **Defect that produced it:** none yet.
- **What breaks if you violate it:** the Calls window never learns the pass ended; a second line double-counts a pass.
- **Where it is enforced:** `conclude` is the only place that writes `FinalPassFinished` and the notice (`TranscriptReady { call }` or `PassFailed { reason }`); every exit of `run_pass` and `pass` goes through it.
- **Don't:** return a `PassOutcome` without `conclude`.

## Developer-level choices (no owner decision; recorded so a reviewer can overturn them)

- **D1 "Every later chunk" is strict, in dispatch order.** Chunk k (pass-global dispatch order) answering `Timeless` flips the pass mode. A later chunk in flight as `Segments` has its answer dropped and is re-sent `Plain` (at most 2 extra requests); an answered later chunk keeps its audio until no earlier timed request is in flight, so it can be re-sent. Chunks before k keep their times.
- **D2 VAD fallback is per window and sticky per track.** The primary's `Err` on a window gives energy spans for that window, one `vad_fallback track= cause=` (`Unavailable` gives `unavailable`; `Failed` and any later `VadError` variant give `error`), and energy spans for the rest of the track. `primary: None` logs `unavailable` for each track at its first window.
- **D3 Engine setup.** A base URL refused by `check_base_url`, or a key that is `None` or blank after trim, is `failed: engine_not_set_up` (`failure.track` null, both results `failed{engine_not_set_up}`, no request). A credential-store read `Err` is `key` ("Transcribe again" retries). This is not T-136's private `fail_reason` default (`KeyStoreUnavailable` to `provider`), which says nothing about the cause. Either is an existing FR-027 reason, not a product change.
- **D4 Track and call failure.** The first `Failed` of a track stops its dispatch and decoding and drops its in-flight answers. One failed track gives `failure = {reason, track: Some(track)}`; both failed gives the first failure in dispatch order and `track: None`. The good track's `done{segments}` is call-relative (`merge` over that track alone), so T-146 can redo only the failed one.
- **D5 Decode ends.** `Ok(None)` and `Err(Damaged)` end the track (the CRC-valid prefix was already returned, R7). `Unreadable` and `Codec` fail the track with `storage`.
- **D6 Write order.** Validate, render, `transcript.md`, `transcript.json` (the commit point; T-141 reads lines from it), then `meta.json` (`ready`). A failure at any step gives `CallStorageFailed { kind: transcript | meta, os }`, a best-effort failed `meta.json`, and `PassFailed { storage }`. A `validate()` `Err` logs `TranscriptInvalid { rule, segment }` and gives `failed: storage`.
- **D7 Log lines.** `final_stage` for `decode`, `vad` and `upload` per track (summed ms, `chunks` = the track's chunks); `merge` and `write` run once over both tracks and are logged once per track with the shared ms, because `FinalPassStage.track` is required. One `final_finished` per pass, `mode` = the mode the pass ended in.

### Deviations found in the implementation (verified against `final_pass.rs` at cb66041e)

1. **`meta.duration_ms: None`** (should not happen after stop) uses the smaller of (track offset + decoded length) over the two tracks, which is data-model's "the shorter readable track" (`pass`). It has no test.
2. **A blank model is not `engine_not_set_up`.** `api_engine` checks the base URL and the key only; an empty `api.model` is sent as is, and the server decides (`EngineRef` is `api:` plus the empty model).
3. **Failing to write the failed `meta.json`** logs `CallStorageFailed { kind: meta }` and the pass ends with `storage`, replacing the reason it was writing (`write_failed` returns `Storage`); this also covers the first `transcribing` write.
4. **An `EnergyDetector` `Err` after the fallback keeps the whole window as speech** (one span over the window); it is never empty spans and never dropped audio. The detector is infallible today, so no test reaches it (marked equivalent in the T-138 mutant catalog).
5. **An empty decoded window ends the track** (`Ok(Some(empty))` is handled like `Ok(None)`), so a decoder that returns zero samples cannot spin the loop.
6. **A `Timeless` answer to a `Plain` request fails the track with `provider`:** a request with no times to refuse means the server is at fault. Test `a_plain_request_that_is_refused_for_times_fails_the_track_with_provider`.
7. **A worker panic is re-raised on the dispatcher thread** (`catch_unwind` in the scoped worker, `resume_unwind` in `drive`), not mapped to a `FailReason`: an engine panic is the engine's bug and must not read as a provider failure.

## Reused, not copied

| Concern | The one place |
|---|---|
| Engine call | `OpenAiCompatibleEngine::transcribe_timed` (T-136), through `TimedTranscriber` |
| Cutting | `ChunkPlanner`, `recut_timeless` (T-137) |
| Speech spans | the `SpeechSpans` port; `vad::EnergyDetector` for the fallback (T-134, #188) |
| Lines, validation, Markdown | `transcript::{merge, validate, render_markdown}` (T-123) |
| Atomic files | `storage::write_atomic`, `write_meta` (T-125, T-141) |
| Base URL rule | `settings::url::check_base_url` |
| Timeouts | `Timeouts::from_settings`, once per pass |
| Monotonic time | the injected `clock::MonotonicClock` |

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| `engine_for` for the pass | follows `settings.engine`; a `None` key gives an engine with no header, but FR-022 needs the API engine and `engine_not_set_up` for no key | T-138 analysis |
| Two workers, one per track | breaks "3 in flight from both tracks in start order" | #187 |
| A tokio runtime | the engine is blocking reqwest (#42); a runtime for nothing | T-138 analysis |
| `SpeechGate` for the spans | it answers yes or no; the pass needs spans | #197 |

## Open

- Not covered by tests: held-audio memory is not measured; the temp-file check covers the call folder only; a `duration_ms: None` call has no test; Windows-only parts are out of scope. Queue, cancel and "Transcribe again" are T-146.
