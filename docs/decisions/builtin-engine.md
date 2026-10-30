# Built-in engine (whisper.cpp threads and abort)

**Code:** `crates/voicen-whisper/src/lib.rs` (`BEST_THREADS`, `thread_count_for`, `thread_count`, `WhisperModel::transcribe`), `crates/voicen-whisper/src/silero.rs` (`VAD_THREADS`) · **Tests that pin it:** `transcribe_wav.rs`: `thread_policy_is_at_least_one_and_below_parallelism`, `loader_uses_the_policy_and_with_threads_overrides_it`, `pre_requested_abort_returns_promptly`, `MODEL_LOCK` / `serial()` on every model test, print-only `whisper_timing_is_printed`

Why: the engine ran about 16 ggml threads on 4 vCPUs and one 1 s clip took about 90 s (T-109; `transcribe_wav` 280.73 s, run 40030080179). No test went red, so there is no F-entry for this class.

## Mechanism

ggml's CPU barrier (`ggml_barrier`, whisper.cpp 1.8.3 in whisper-rs-sys 0.15.0) is a pure spin, and OpenMP is off (`GGML_OPENMP` OFF unless the `openmp` feature, which is not enabled). A thread waiting at a barrier burns the CPU that the thread it waits for needs, at every node of every graph. With one thread the barrier returns at once. So the number of runnable ggml threads in the process must stay at or below the free CPUs, or the cost grows far beyond the gain.

## Invariants

### The VAD decision runs on 1 ggml thread

- **Defect that produced it:** T-043 review 1 #1: Silero at 4 threads cost minutes per clip set; on 4 pinned CPUs with 4 detectors at once about 48 s per decision against about 20 ms at 1 thread (figures and conditions in the `VAD_THREADS` doc comment).
- **What breaks if you violate it:** the decision, which runs next to a transcription, stalls both.
- **Where it is enforced:** one constant, `VAD_THREADS = 1`, in `silero.rs`; the detector is not routed through `thread_count`.
- **Don't:** give the detector the engine's count, or "a few threads for speed".

### The engine's thread count comes from one policy function

`thread_count() = max(1, min(B, P - 1))`, P = available parallelism (1 when unknown), B = `BEST_THREADS` = 2 (decision #213). Always below P when P > 1, at least 1.

B is the count with the lowest 30 s figure in the windows-latest `whisper-timing` table (decision #200 D1, ties to fewer threads). Run 40168449583 @ 282df633, tiny model, best of 3 runs, available parallelism 4, one transcription alone in the process:

| threads | 1 s clip best | 30 s best |
|---|---|---|
| 1 | 1366.6 ms | 4295.4 ms |
| 2 | 778.4 ms | 3077.8 ms |
| 3 | 746.1 ms | 3235.8 ms |
| 4 | 677.1 ms | 3182.2 ms |

Limit (#213): measured on 4 vCPUs only, with the tiny model. A stronger PC stays at 2 threads; raising B for it needs a new measurement on that class of machine, not a guess.

- **Defect that produced it:** none logged (cost, not a red run); T-109, T-043 r2#1.
- **What breaks if you violate it:** a count equal to P, or a policy that ignores `- 1`, makes a spinning thread compete with the UI, the VAD decision and the paste for the last core.
- **Where it is enforced:** the single function `thread_count_for` (`thread_count` is its only production caller, reached only through `WhisperLoader::load_whisper`); `thread_policy_is_at_least_one_and_below_parallelism` holds for any B >= 1. The value of B is deliberately not asserted (a statement about one machine class, F-013, F-021); it is shown by the printed line `whisper-timing: policy picks N threads, available parallelism P`.
- **Don't:** add a settings field or env variable for the count (#200 D2); `WhisperLoader::with_threads` is `doc(hidden)` and for tests only. Don't set B from a Linux surrogate.

### At most one multi-thread whisper.cpp computation runs at a time in a test process (I1)

- **What breaks if you violate it:** libtest runs 4 tests at once; each loads a model with its own threads and the spin barrier multiplies the time (about 90 s per 1 s clip; suite 280.73 s before, 91.26 s after, about 71 s of it the timing child).
- **Where it is enforced:** a file-level `MODEL_LOCK` taken by `serial()` (recovers a poisoned guard) first in every `transcribe_wav` test that loads a model, and held by the parent of a child process (quiet child, whisper-timing child) for the child's whole run. Load-failure tests build no graph and take no lock. No test asserts "only one at a time" (that would be a clock verdict); the evidence is the suite time in the Windows log.
- **Don't:** drop the lock for speed, or add a model test without `serial()`. Don't add `--test-threads=1` to `ci.yml` instead (it touches the owned windows job).

### The abort guarantee is asserted in units of work, not time (I3)

A run whose `Abort` is already requested leaves whisper.cpp's segment count for that run at 0; the same audio unaborted on the same model leaves more than 0. `pre_requested_abort_returns_promptly` asserts exactly that through `WhisperModel::last_segment_count()` (`WhisperState::full_n_segments`), on one concrete model from `WhisperLoader::load_whisper`.

- **Defect that produced it:** T-099 v3#N2: the old test decided by `aborted * 4 <= baseline`, two wall-clock readings; with both stop callbacks removed it passed 1 run in 4. Its other check, clock reads >= 1, was always met by `transcribe`'s own `abort.requested()` after `whisper_full`. Class: F-013, F-021.
- **What breaks if you violate it:** the test would pass without the stop callbacks, and a cancelled recording would keep the engine busy.
- **Where it is enforced:** the segment-count assertions. Mutant `01-no-stop-callbacks` (both `set_abort_callback` and `set_start_encoder_callback` removed): red on `last_segment_count() == 0`, run 40175631024, the aborted run left 12 segments, assertion expected 0. Honest limit of that observation: the baseline run printed 24 segments, the aborted one 12. The assertion relies on the aborted count being > 0 under the mutant, not on equality with the baseline; why it is 12 and not 24 is not established.
- **Mel caveat:** `whisper_full_with_state` computes the mel of the whole input before any callback runs, so that part is not abortable and grows with the audio length. The assertion does not cover it, by design. It also does not tell apart "only `encoder_may_start` removed" (the first encoder graph runs to its end, then the abort check after it returns with 0 segments) from the intact code; whisper.cpp exposes no per-state encode count.
- **Don't:** put a time bound back; count inside the stop callbacks (the counter vanishes with them); use `whisper_get_timings` (null under whisper-rs's no-state init, and wall-clock anyway).

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| Only the lock, engine count unmeasured | leaves the policy a guess; Silero's answer (1 thread) does not carry over | #200 |
| `--test-threads=1` as a `ci.yml` step | touches the owned windows job, and hides the production cost | T-109 analysis |
| Engine count = P (min(P, 8), the old code) | spin barrier with as many threads as CPUs stalls whenever anything else takes a CPU | #200 D1 |
| B = 4 | 3182 ms against 3078 ms at 2 threads for 30 s: no gain, one more spinning thread | #213 |
| Progress-callback counter or encoder counter for the abort test | adds a production callback only for the test; shows nothing the segment count misses | T-109 analysis |
| Counter inside the stop callbacks | disappears with them: the v3#N2 mutant would read 0 and pass | T-099 v3#N2 |
| Test-clock stepping (`Abort::requested` latches) | reads after the stop are not counted, so post-stop work is invisible | T-109 analysis |
| User setting or env variable for the thread count | product surface for a tuning knob | #200 D2 |

## Open

- A third ggml user next to a transcription (T-102 shell VAD gate, T-135 Silero speech spans) shows the same symptom: open an rca for a process-wide ggml thread budget.
