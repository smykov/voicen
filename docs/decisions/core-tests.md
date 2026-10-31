# voicen-core test network and timing

**Code:** `crates/voicen-core/tests/common/`:
- `os_answer.rs`: `OsAnswer::refused()`, `OsAnswer::unresolvable()`, `OsAnswer::refused_released_port()`, `OS_ANSWER_BUDGET`, `in_time()` (T-111: the deadlines of a leg that expects an answer, the I1 shape without a target or a probe), `Unanswered::blackhole()`;
- `timing.rs`: `Took` (an opaque measured time), `measure`, `between`, `at_least`, `at_most_per_second`, `now`, `ago`, `try_ago` (T-009's 10-minute timer test skips on a young clock), `deadline`, `left`, `passed`, `eventually`, and (T-111) `M`, `with_premise`, `skip`;
- `refused_addr_tests.rs` and `download.rs`.

Also: `scripts/ci/core-test-os-answer.sh` (the I1 tripwire, run by `make check-core-test-os-answer`; its self-test `scripts/ci/core-test-os-answer.test.sh` uses the fixtures in `scripts/ci/fixtures/core-test-os-answer/`), the I2 call ban (`crates/voicen-core/clippy.toml`, `[lints.clippy]` in `crates/voicen-core/Cargo.toml`, the owned crate-level allow of `tests/common/mod.rs`; `make check-core`, proven by `make check-ban-canary`, owned lines held by `make check-clippy-lines`; T-089, `docs/decisions/ci-toolchain.md`), and `.github/workflows/ci.yml` (the windows job, `--no-fail-fast`).

**Users of `tests/common`:**
- Every voicen-core test binary that declares `mod common;`. Among them: `api_pipeline`, `openai_client`, `diag_pipeline`, `local_server`, `local_download`, `local_download_refused`, `post_process_chat`, `post_process_timeout`, `connection_test`, `dictation_session`, `fakes`, `local_models_service`, `diag_blocked_press`, `diag_observer`, `common_helpers`.
- The shell test `src-tauri/tests/settings_ipc.rs`. It includes `common/mod.rs` and `common/refused_addr_tests.rs` through `#[path]`, so a change to `common` must keep it building. It is type-checked by `check-shell-windows` and runs only in the Windows job.

**Tests that pin it:**
- `common_helpers` (`common/timing_tests.rs`, `common/os_answer_tests.rs`) and `common_in_time` (`common/in_time_tests.rs`: the `in_time()` destructure; `common/timing_seam_tests.rs`: `M`, `skip` in a child process under libtest capture, `with_premise`).
- `refused_addr_tests::*` in each binary that takes a refused case. They check:
  - the port is below the ephemeral range;
  - the address is IPv4 loopback;
  - the port is refused by a probe within `OS_ANSWER_BUDGET`;
  - every deadline of the case, destructured exhaustively.
- `timing_tests::a_measured_time_cannot_be_compared_or_read_back_so_no_ceiling_compiles`: a compile-time check that `Took` has no comparison, `==`, deref or conversion back to a `Duration`.
- The I1 tripwire self-test `core-test-os-answer.test.sh`: 24 cases, including the real tests dir. The I2 ban's canary `ban-canary.sh` (probes k01-k13 and k15-k17, k19, k20 in `tests/ban_canary.rs`, feature `ban-canary-tests`; k14 and k18 in `src/ban_canary.rs`, feature `ban-canary-lib`).

**Tasks:** T-111 (in-time legs, both actors); T-016; T-047 (F-004); T-048 (F-005); T-078 (F-013; deferred and absorbed by T-080); T-080 (rca, class `os-answer-deadline-race`); T-089 (I2 moved to a compiler ban, I1 a raw-token tripwire); T-107 (F-021; deferred and absorbed by T-110); T-110 (rca, class `os-answer-deadline-race`: no wall-clock ceiling in any core test).
**Classes** in `docs/failures.md`: `test-port-race` and `os-answer-deadline-race`.
**Decisions:** #53, #56, #100, #106, #107, #112, #113, #149, #200, #201, #224 (the seven `silero_model.rs` fetch legs at `timeouts(5 s)` stay a follow-up).
**Open questions:** none. OQ-22 (which product reason a lookup cut by the connect timer gives) is answered by decisions #106, #113 and built in T-079 (`docs/decisions/engine-http.md`).

## Invariants

### No core test's verdict races a clock it does not control

This is one rule for every kind of uncontrolled clock: the OS refusal time, the resolver retry schedule, host scheduling and host load. Earlier fixes drew the rule once per kind: a refusal budget, then a resolver budget, then an overhead allowance. Every kind not yet listed stayed open.

**I1: OS answers.**
- A target whose outcome comes from an OS answer (a refused loopback port, a name the resolver fails) comes only from `tests/common::os_answer::OsAnswer`, together with its deadlines.
- There is one budget, `OS_ANSWER_BUDGET` (60 s). 60 s is also the largest connect limit the settings allow, so the settings-based paths keep it after the clamp.
- The case's deadlines:
  - connect = the budget;
  - every whole-request and no-data deadline = 2 × the budget;
  - `builtin` stays at its default.
- Each case runs its own probe when it is built. It panics unless the OS answered within half the budget, so the budget is at least 2 × the measured answer.
- A new kind of OS answer adds a constructor with its own probe. It never adds a new budget or a timeouts helper.

**I2: the wall clock decides only lower bounds.**
- A measured time (`common::timing::measure`, or `between` for two stamps) is checked only with `at_least(took, deadline)`. That shows a deadline never fires early and no shorter wrong limit was used.
- The measured time is an opaque `Took`: no comparison, no `==`, no deref or conversion back to a `Duration`, and its only accessor is private to `tests/common`. So `assert!(took < ...)` does not compile. Only `at_least` and `at_most_per_second` (a count capped by the time; the time only widens the cap) read it.
- A wrong *longer* deadline is caught by the outcome, for example:
  - the server answers between the right and the wrong deadline (`Ok` instead of `Timeout`);
  - a stall holds longer than the harness waits for the end event (no end event);
  - a request count;
  - the overlay sequence.
- There is no wall-clock ceiling in any core test. A spec tolerance (SC-003) is checked by its outcome: a server that answers 200 with a valid body at deadline + tolerance must still give the timeout outcome, and a sibling with a generous deadline proves the server answers (decision #149, which amends #107's measurement wording). A host stall can then delay such a test but cannot turn a correct deadline red. `within_spec`, `REFERENCE_LOAD`, `fastest` and `Took::less` were retired in T-110.
  - Its user: `post_process_timeout::default_deadline_skips_a_silent_server_at_15_s` (answer at 15.5 s, `Timeouts::default()` → `Skipped(Timeout)`; a 60 s sibling leg → `Applied`). `post_process_timeout::configured_post_processing_timeout_delivers_the_raw_transcript` is no longer a user (T-111): its limit is `PP_LIMIT_S` = 10 s and its answer `PP_LATE_ANSWER` = 15.5 s (limit + `M` + the tolerance, so a processor left on `Timeouts::default()` also skips), `post_processing_s = 10` → `DeliveredSkipped(Timeout)`; the control leg at `post_processing_s = 60` → `Delivered`. The 15 s default is pinned by the user above.

**Other clock uses in tests:**
- Instants handed to a product API as input (a press, a release, a frame's `at`) come from `common::timing::now()` or `ago()`.
- Waits that only end a hung test use `deadline`, `left` or `eventually`. They are sized far above the time the awaited outcome needs.
- A target that never answers (TEST-NET-1, `192.0.2.1`) comes only from `common::os_answer::Unanswered::blackhole()`, together with its deadlines (connect 300 ms, every total twenty times that, 6 s) and the endings it accepts (`ended_by_connect_or_os`: `CannotReach` for its host, or `NetworkUnavailable` where there is no route). Its users are the test that pins the connect timer (`api_pipeline::blackhole_connect_is_bounded_by_connect_timeout`) and `openai_client::answered_lookup_then_unanswered_connect_stays_cannot_reach` (T-079: a lookup answered at once with the blackhole address, so the connect timer, not the lookup, ends the call); both bite on the reason, not on a ceiling. It is not an `OsAnswer`: no OS answer is awaited, so there is no probe and no budget.
- A lookup with no answer (T-079) is not a host resolver target: it comes from `tests/common/held.rs` (`Held::install` puts a `test-fakes` `engine::lookup::HeldLookup` in front of the OS resolver for one test-only `*.t079.example.com` name). The verdict is the ordering "the call returned while the lookup was still held" (`returns_while_held`); the clock gives only the lower bound `at_least(took, connect)`. `HELD_WAIT` (`OS_ANSWER_BUDGET`) only ends a hung test.
- A settings value a fake engine records but nothing contacts (`dictation_session`) is a TEST-NET-2 literal (`198.51.100.9`), not an OS-answer target.
- `dictation_session::realtime_source_paces_frames_by_real_time_and_pads_with_silence` holds the capture until about 1 s of audio arrived (`eventually`), not for a fixed sleep, and checks only that the capture lasted at least the audio less one chunk (`at_least(between(..))`).
- `dictation_session::realtime_source_pushes_nothing_after_stop_or_drop` awaits its premise (the first push) with `eventually`, not after a fixed sleep (T-110); only the absence check after the stop sleeps, and a stall can only make an absence check pass.
- A real-time verdict that needs the test thread to act before a product timer fires, or before the end of a transfer, checks that premise, it does not time it (T-110 review 1 #1, T-111). Right after the act the test asks `passed(raised, ..)`; when a host stall made it miss the premise, `common::timing::with_premise(what, budget, run)` re-runs the scenario on a fresh rig while `budget` since the first start allows, and skips when every run missed, writing the reason to the real stderr through `common::timing::skip` (`std::io::stderr().write_all`; `eprintln!` is captured by libtest for a passing test, so a skip would look like a pass; pinned in a child process by `timing_seam_tests`). `passed` there decides only "no verdict", never a red, so it is not a ceiling. Its users: `dictation_session::timer_follows_the_latest_deadline` (B's press before A's `until`) and `a_job_failure_during_a_recording_shows_after_its_release_for_the_rest_of_its_3s` (B's release, and the overlay read after it, before A's `until`), both with the file's local `BUDGET` (10 s); `local_download::cancel_during_stall_*` (the cancel before `no_data`) with `OS_ANSWER_BUDGET`. The other real-time tests of `dictation_session` drop a possible expiry once its 3 s have passed (`settled`), stamp their instants in the past or ahead of the clock (`past()`, `audio_device`'s `ahead()`), or stamp a 10-minute deadline that the timer fires at once when it is already due (`the_session_timer_stops_a_recording_at_its_max_length`). That test skips through `skip` on a host whose monotonic clock is younger than 10 min (a fresh CI runner, Linux too). Coverage gap: on such a clock nothing tests the timer-to-stop path, and how often CI skips is not measured.

**Defects that produced it:**
- F-005: a refused loopback connect takes about 2.17 s on windows-latest, so a 300 ms total gave `Timeout`, not `CannotReach`.
- F-013:
  - reqwest times the DNS lookup inside `connect_timeout`. When a UDP answer is lost, glibc retries for about 20.1 s, so a 2 s connect gave `CannotReach`, not `NetworkUnavailable`;
  - fixed wall-clock ceilings went red under host load.
- Also the ceilings of T-047 review rounds 1-2 and T-046 review round 1 #2, which have no F-entry.
- F-021: SC-003's `within_spec` ceiling, the one T-080 kept, measured 16.39 s against 15 s ± 0.5 s on Windows CI while the product deadline held; a runner-wide stall of about 1.4 s sat inside the measured window.

**What breaks if you violate it:** a test is green on an idle host and red on a loaded one, on windows-latest, or behind a slow resolver. The verdict then reports the host, not the product: a correct deadline red once in about 57 Windows runs (F-021).

**Where it is enforced:**
- T3, I1 (T-089, `decides_on: token`): `make check-core-test-os-answer` runs `scripts/ci/core-test-os-answer.sh` over every `*.rs` under `crates/voicen-core/tests` outside the top-level `common/`.
  - It refuses a line holding an OS-answer token, in any case, comments included (no `//` skip): `.invalid` not followed by a letter, digit or `_`; `127.0.0.1:1` not followed by a digit; `192.0.2.1` not followed by a digit.
  - It reads no Rust syntax (F-003, P-020). It does not catch:
    - the array form `SocketAddr::from(([127, 0, 0, 1], 1))`, `Ipv4Addr::LOCALHOST`, `127.1`, `:01`;
    - a target built from parts at run time;
    - the `#[cfg(test)]` unit tests under `crates/voicen-core/src` (outside the dir it reads);
    - the shell tests.
  - Exit 3 ("cannot run") is never a pass.
- T3, I2 (T-089, `decides_on: switch`): rustc's path resolution. Clippy `disallowed-methods` (`std::time::Instant::now`, `std::time::Instant::elapsed`, `std::time::SystemTime::elapsed`, `tokio::time::Instant::now`, `tokio::time::Instant::elapsed` in `crates/voicen-core/clippy.toml`; tokio's `time` feature is on in voicen-core's build through feature unification, so tokio's clock is reachable), pinned at deny by `[lints.clippy]` in `crates/voicen-core/Cargo.toml`, fails `make check-core` on a clock read that check-core compiles (`--all-targets`, default features, the Linux target), whatever the spelling (whitespace, a split path, `use … as`, UFCS, a fn reference, a macro). The only exemptions are owned lines: `#![allow(dead_code, clippy::disallowed_methods)]` in `tests/common/mod.rs` (all of `common`) and the two allows on `crate::clock::instant` / `since` (the src seam, T-103). `make check-ban-canary` proves the ban bites (probes k01-k20), and `make check-clippy-lines` refuses any other line naming the lint tool, check-core's clippy command line in the `Makefile` included (an owned line, so dropping `--all-targets` or adding `-A clippy::…` there is refused).
  - It does not catch:
    - code under a cfg that check-core does not compile: `#[cfg(windows)]` or another target, a feature that is off (clippy never lints it; the retired text guard did read such lines);
    - a wall-clock measurement through a method the config does not name: `SystemTime::now()` stays allowed (log timestamps, `SystemClock`), so `SystemTime::now().duration_since(t)` passes, and so does another crate's clock; a new clock source needs its path in `clippy.toml` with a canary probe;
    - a make-level setting without the lint tool's name (`export RUSTFLAGS := --cap-lints allow`) or a `check:` that drops `check-core`: review only (T1);
    - arithmetic on two instants handed out by `common` (`now() - a`), including a ceiling written that way;
    - a clock read in src through `crate::clock::instant()` / `since()` (the unit tests under `src` stamp through it and hold no ceiling today);
    - the shell tests (another package, no such config).
  - A ceiling on what `measure` / `between` return is not text it reads: it does not compile (`Took`, above).
- Automatic checks inside `common`:
  - the `OsAnswer` probes;
  - the exhaustive `Timeouts` destructure in `refused_addr_tests`: a new deadline field breaks the build until it is set;
  - `Took` makes a ceiling on a measured time a compile error, pinned by a compile-time not-impl check in `timing_tests`;
  - `Unanswered`'s deadlines are destructured exhaustively in `os_answer_tests`;
  - `common_helpers` pins both modules.
- By review only (T1):
  - no ceiling written as instant arithmetic, as `!passed(..)` or `D - left(..)`, or in a unit test under `src` or a shell test slips in; no real-time verdict that needs the test thread to act before a product timer without a premise check (`with_premise`), and no premise check that goes red instead of re-running or skipping (T-110); no `catch_unwind(at_least)`; no in-time leg on a short value and no test-thread act that races a product timer without `with_premise` (class `os-answer-deadline-race`, until T-165); no edit of a case's `pub` deadline fields after construction (decisions #112); no new public reader of `Took` in `common::timing` besides `at_least` and `at_most_per_second` (T-110);
  - when a ceiling is removed, an outcome catches its bite, named in the test's comment.
- `ci.yml` runs the Windows workspace tests with `--no-fail-fast`, so one red binary does not hide the others.

**Don't:**
- write an OS-answer literal or read the clock outside `tests/common`;
- put a deadline below `OS_ANSWER_BUDGET` on an OS-answer case;
- test a timeout with a refused or unroutable address. Use a server that accepts and answers late;
- add a `took <` ceiling. Find the outcome that catches the wrong limit; a spec tolerance too is checked by an outcome bracket (decision #149);
- remove `--no-fail-fast`.

### An in-time leg never depends on a host-scheduled actor beating a short deadline

T-111 (class `os-answer-deadline-race`; T-110's Open item, F-021) extends the rule above to the actor that must act before a product deadline: a loopback mock or `common::Server` thread answering, or the test thread (a cancel). Every product deadline a core test sets is exactly one of the three kinds below, except the one named residual with a margin under `M` that decision #149 (not #200) accepted (list below) and the legs not yet moved to this rule: the seven fetch legs through `common::download::timeouts(Duration::from_secs(5))` in `silero_model.rs` (a follow-up, decision #224) and the in-time legs with connect 5 s in `connection_test.rs` and `post_process_timeout.rs` (T-111 review 2 #3). Their margin equals `M`:
1. **A timeout leg.** The counterparty acts later than the deadline by at least `M` (5 s, `common::timing::M`, decision #200) or never, and no other product timer it must precede lies within `M`. A stall only delays the outcome. Such a leg overrides only its own field of `in_time()`, and its file takes the answer time from `common::timing::late_answer(deadline)` (= deadline + `M`), pinned against the leg's deadline by a test in that file (`timing_seam_tests` pins `late_answer` itself).
2. **The in-time value.** `common::os_answer::in_time()` (connect `OS_ANSWER_BUDGET`, every total and no-data 2 x the budget, `builtin` at its default; pinned by an exhaustive destructure in `common/in_time_tests.rs`) serves every leg that expects an answer and whose deadline is not the subject, every helper's `timeouts` and `common::download::timeouts` included.
3. **A bite-bound deadline.** The deadline is the subject, and test cost or reqwest's 30 s default caps its size. Its in-time margin is at least `M`, and it is a T1 residual accepted by #200 (list below).

A test-thread act that must come before a product timer or the end of a transfer goes through `with_premise`, or the race is removed (a transfer that outlasts the budget: `local_download::long_trickle()`). Test-owned holds and hang-ending waits keep `M` above what they await (`END_WAIT` above the trickle, `HOLD` above `END_WAIT`).

**T1 residuals at `M`** (values as in the code):
- `local_download`: `stall_past_no_data_*`, `no_response_headers_*`, the stall legs and their retries (the retry shares the `Downloader` and its limit): `no_data` 5 s (`stall_no_data()`); `cancel_during_stall_*`: `no_data` 5 s (`cancel_no_data()`, with the premise check); `trickle_*`: `no_data` 5.25 s, 1000 bytes every 250 ms (about 16.5 s, at least 3 x `no_data`); `common::download::END_WAIT` 25 s (`M` above the trickle, below the 30 s default); `HOLD` stays 30 s.
- `api_pipeline::configured_api_timeout_*` and `post_process_timeout::configured_post_processing_timeout_*`: `connect_s` 5, limit 10 s, the answer 15 s = the limit + `M` for the API (`T073_DELAY`), 15.5 s = the limit + `M` + SC-003's 0.5 s for post-processing (`PP_LATE_ANSWER`) (a loopback connect must finish within the connect value; a connect read as the limit ends the job early and `at_least` sees it); the `API_*` / `PP_*` consts are pinned by `*_keep_the_in_time_margin_m`.
- `Unanswered::blackhole()`: connect 300 ms, totals 20 x connect (margin 5.7 s).
- `post_process_timeout::default_deadline_skips_a_silent_server_at_15_s`: the production 15 s default against an answer at 15.5 s, margin 0.5 s (SC-003's tolerance), under `M`. Accepted as a named residual by decision #149 (the spec's tolerance is checked by an outcome bracket), not by #200; widening it would drop the bracket. A host stall longer than the margin can turn it red on correct code (F-021, the case #149 recorded).

**Not caught by any test (T1 until T-165):** a single fixture site, redesigned test, helper value or trickle put back on a short value (`fixture(&server, Duration::from_secs(2))`, a `Serve::Stall`, a short `Serve::Trickle`). The helper values and `in_time()` are pinned and sibling tests carry the injected stall, but no test binds each of the 19 `local_download` fixture sites: catalog follow-ups 61 (`cancel_of_another_model_or_when_idle_*` back on a 3 s `Stall` under `no_data` 2 s) and 62 (`part_file_that_cannot_be_created_*` back on `fixture(.., 2 s)`) survive (T-111's Failure branches). Mechanical enforcement is T-165, but as first written it refuses only `Timeouts {` and `Timeouts::default()` outside `tests/common`; its Acceptance is extended to these shapes, and until that is met they stay T1 (review).

### A refused address never comes from a released port

- **Defect that produced it:** F-004. Helpers bound `127.0.0.1:0`, read the port and released it. A sibling test's server in the same binary could get that port, so the address was no longer refused (about 1 in 7058 per bind).
- **What breaks if you violate it:** a rare flaky "refused" test.
- **Where it is enforced:**
  - The single seam is `OsAnswer::refused()`: `127.0.0.1:1`, below the Linux and Windows ephemeral ranges. Its probe asserts `ConnectionRefused` at each use, so a listener on port 1 fails loudly.
  - The I1 tripwire (`core-test-os-answer.sh`) keeps the literal inside `tests/common`.
  - The rule itself (no bound-and-released port) is enforced by review (T1).
  - `local_download_refused` keeps its own test binary because its test needs refused-then-serve on one port. It is the only user of `OsAnswer::refused_released_port()`.
- **Don't:** bind port 0 and release it to get a "closed" port.

## Open

- The ~2 s user-facing refusal on Windows: OQ-07 (non-blocking).
- `src-tauri/tests/settings_ipc.rs` keeps one `took < 30 s` ceiling on its refused invoke. The shell tests are outside the I1 tripwire's and the I2 ban's scope.
