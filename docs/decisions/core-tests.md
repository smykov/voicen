# voicen-core test network and timing

**Code:** `crates/voicen-core/tests/common/`:
- `os_answer.rs`: `OsAnswer::refused()`, `OsAnswer::unresolvable()`, `OsAnswer::refused_released_port()`, `OS_ANSWER_BUDGET`, `UNANSWERED_HOST`;
- `timing.rs`: `at_least`, `within_spec`, `REFERENCE_LOAD`, `measure`, `now`, `ago`, `deadline`, `left`, `passed`, `eventually`;
- `refused_addr_tests.rs` and `download.rs`.

Also: `scripts/ci/core-test-clocks.sh` (the tripwire, run by `make check-core-test-clocks`; its self-test `scripts/ci/core-test-clocks.test.sh` uses the fixtures in `scripts/ci/fixtures/core-test-clocks/`), and `.github/workflows/ci.yml` (the windows job, `--no-fail-fast`).

**Users of `tests/common`:**
- Every voicen-core test binary that declares `mod common;`. Among them: `api_pipeline`, `openai_client`, `diag_pipeline`, `local_server`, `local_download`, `local_download_refused`, `post_process_chat`, `post_process_timeout`, `connection_test`, `dictation_session`, `fakes`, `local_models_service`, `diag_blocked_press`, `diag_observer`, `common_helpers`.
- The shell test `src-tauri/tests/settings_ipc.rs`. It includes `common/mod.rs` and `common/refused_addr_tests.rs` through `#[path]`, so a change to `common` must keep it building. It is type-checked by `check-shell-windows` and runs only in the Windows job.

**Tests that pin it:**
- `common_helpers` (`common/timing_tests.rs`, `common/os_answer_tests.rs`).
- `refused_addr_tests::*` in each binary that takes a refused case. They check:
  - the port is below the ephemeral range;
  - the address is IPv4 loopback;
  - the port is refused by a probe within `OS_ANSWER_BUDGET`;
  - every deadline of the case, destructured exhaustively.
- The tripwire self-test: 23 cases, including the real tests dir.

**Tasks:** T-016; T-047 (F-004); T-048 (F-005); T-078 (F-013; deferred and absorbed by T-080); T-080 (rca, class `os-answer-deadline-race`).
**Classes** in `docs/failures.md`: `test-port-race` and `os-answer-deadline-race`.
**Decisions:** #53, #56.
**Open questions:** OQ-23 (the reference load); OQ-22 / T-079 (which product reason a lookup cut by the connect timer gives).

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
- A measured time (`common::timing::measure`) is checked only with `at_least(took, deadline)`. That shows a deadline never fires early and no shorter wrong limit was used.
- A wrong *longer* deadline is caught by the outcome, for example:
  - the server answers between the right and the wrong deadline (`Ok` instead of `Timeout`);
  - a stall holds longer than the harness waits for the end event (no end event);
  - a request count;
  - the overlay sequence.
- The only ceiling is a tolerance a spec states. It goes through `within_spec(stage, want, tol, spec)`, which names the spec id and the reference load (`REFERENCE_LOAD`).
  - The reference load is OQ-23 proposal (a): the gate's own load. That is one `make check` at a time, `cargo test -j2`, test binaries one after another.
  - Its only use today is SC-003 in `post_process_timeout`: the stage, over the fastest of 3 baseline runs, lies within the deadline ± 0.5 s.

**Other clock uses in tests:**
- Instants handed to a product API as input (a press, a release, a frame's `at`) come from `common::timing::now()` or `ago()`.
- Waits that only end a hung test use `deadline`, `left` or `eventually`. They are sized far above the time the awaited outcome needs.
- The never-contacted TEST-NET-1 value `192.0.2.1` comes from `common::os_answer::UNANSWERED_HOST`. It has two uses:
  - a settings value a fake engine records (`dictation_session`);
  - the one test that pins the connect timer (the `api_pipeline` blackhole test). Its verdict accepts both endings the OS may give, and it bites on the reason, not on a ceiling.

**Defects that produced it:**
- F-005: a refused loopback connect takes about 2.17 s on windows-latest, so a 300 ms total gave `Timeout`, not `CannotReach`.
- F-013:
  - reqwest times the DNS lookup inside `connect_timeout`. When a UDP answer is lost, glibc retries for about 20.1 s, so a 2 s connect gave `CannotReach`, not `NetworkUnavailable`;
  - fixed wall-clock ceilings went red under host load.
- Also the ceilings of T-047 review rounds 1-2 and T-046 review round 1 #2, which have no F-entry.

**What breaks if you violate it:** a test is green on an idle host and red on a loaded one, on windows-latest, or behind a slow resolver. The verdict then reports the host, not the product.

**Where it is enforced:**
- T3: `make check-core-test-clocks` runs `scripts/ci/core-test-clocks.sh` over every `*.rs` under `crates/voicen-core/tests` outside the top-level `common/`.
  - It refuses an OS-answer literal: `<name>.invalid`, `127.0.0.1:1` (as text or as `([127, 0, 0, 1], 1)`), `192.0.2.1`.
  - It refuses a clock read: `Instant::now`, `.elapsed(`, an `Instant` alias.
  - It skips lines that start with `//`.
  - It decides on raw text (F-003). It does not catch:
    - a target built from parts at run time;
    - arithmetic on two instants handed out by `common`, including a ceiling written that way;
    - the shell tests.
  - Exit 3 ("cannot run") is never a pass.
- Automatic checks inside `common`:
  - the `OsAnswer` probes;
  - the exhaustive `Timeouts` destructure in `refused_addr_tests`: a new deadline field breaks the build until it is set;
  - `within_spec` refuses a ceiling without a spec id;
  - `common_helpers` pins both modules.
- By review only (T1):
  - no ceiling written as instant arithmetic slips past the tripwire;
  - when a ceiling is removed, an outcome catches its bite, named in the test's comment.
- `ci.yml` runs the Windows workspace tests with `--no-fail-fast`, so one red binary does not hide the others.

**Don't:**
- write an OS-answer literal or read the clock outside `tests/common`;
- put a deadline below `OS_ANSWER_BUDGET` on an OS-answer case;
- test a timeout with a refused or unroutable address. Use a server that accepts and answers late;
- add a `took <` ceiling. Find the outcome that catches the wrong limit, or name the spec tolerance through `within_spec`;
- remove `--no-fail-fast`.

### A refused address never comes from a released port

- **Defect that produced it:** F-004. Helpers bound `127.0.0.1:0`, read the port and released it. A sibling test's server in the same binary could get that port, so the address was no longer refused (about 1 in 7058 per bind).
- **What breaks if you violate it:** a rare flaky "refused" test.
- **Where it is enforced:**
  - The single seam is `OsAnswer::refused()`: `127.0.0.1:1`, below the Linux and Windows ephemeral ranges. Its probe asserts `ConnectionRefused` at each use, so a listener on port 1 fails loudly.
  - The tripwire keeps the literal inside `tests/common`.
  - The rule itself (no bound-and-released port) is enforced by review (T1).
  - `local_download_refused` keeps its own test binary because its test needs refused-then-serve on one port. It is the only user of `OsAnswer::refused_released_port()`.
- **Don't:** bind port 0 and release it to get a "closed" port.

## Open

- The ~2 s user-facing refusal on Windows: OQ-07 (non-blocking).
- Which reason a lookup cut off by the connect timer gives, and the ~20 s wait: OQ-22 / T-079 (product).
- `src-tauri/tests/settings_ipc.rs` keeps one `took < 30 s` ceiling on its refused invoke. The shell tests are outside the tripwire's scope.
