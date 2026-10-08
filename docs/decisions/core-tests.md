# voicen-core test network

**Code:** `crates/voicen-core/tests/common/mod.rs` (`refused_addr()`, `REFUSAL_BUDGET`, `refused_timeouts()`; `unresolvable_host()`, `RESOLVER_BUDGET`, `resolver_timeouts()`, `OVERHEAD_ALLOWANCE`), `crates/voicen-core/tests/common/refused_addr_tests.rs`, `crates/voicen-core/tests/common/unresolvable_host_tests.rs` (included by `tests/openai_client.rs`), `tests/post_process_timeout.rs` (bounds), users: `tests/api_pipeline.rs`, `tests/openai_client.rs`, `tests/diag_pipeline.rs` (T-008's redaction run; includes `refused_addr_tests.rs` and its refused scenario uses `refused_timeouts()`), `tests/local_server.rs` (T-018; includes `refused_addr_tests.rs`), `tests/local_download_refused.rs`, `.github/workflows/ci.yml` (windows job, `--no-fail-fast`) · **Tests that pin it:** `refused_addr_tests::*` (port below the ephemeral range, loopback IPv4, refused by a connect probe within `REFUSAL_BUDGET / 2`, `refused_timeouts_leave_the_refusal_room_on_every_deadline`); `unresolvable_host_tests::*` (an RFC 6761 `.invalid` name, its lookup fails within `RESOLVER_BUDGET / 2`, `resolver_timeouts_leave_the_lookup_room_on_every_deadline`)

Tasks: T-016, T-047 (F-004), T-048 (F-005), T-078 (F-013). Classes `test-port-race` and `windows-refused-connect-timing` in `docs/failures.md`. Decisions: #53, #56.

## Invariants

### A refused address never comes from a released port

- **Defect that produced it:** F-004. Helpers bound `127.0.0.1:0`, read the port and released it; a sibling test's server in the same binary could get that port, so the address was no longer refused (about 1 in 7058 per bind).
- **What breaks if you violate it:** a rare flaky "refused" test.
- **Where it is enforced:** the single seam `common::refused_addr()` (127.0.0.1:1, below the Linux and Windows ephemeral ranges); it asserts a `ConnectionRefused` probe at each use, so a listener on port 1 fails loudly. That is the only automatic check, and it covers the environment, not the rule. The rule itself is enforced by review (T1); there is no grep guard. `local_download_refused` keeps its own test binary (refused-then-serve).
- **Don't:** bind port 0 and release it to get a "closed" port.

### A deadline never races an OS answer (refusal or resolver)

Widened by T-078 (F-013) from "every deadline on a refused path leaves the Windows refusal time": the same shape on the resolver path.

- **Defect that produced it:** F-005 (refusal), F-013 (resolver). F-005: On windows-latest a refused loopback connect returns after about 2.17 s; Linux returns at once. A whole-request deadline below that gives `Timeout` (download: `DownloadInterrupted`), not `CannotReach` (`SourceUnreachable`); only the refusal or the connect timeout gives the latter. F-013: reqwest 0.13.5 times the DNS lookup inside `connect_timeout`; a lookup cut off by that timer has no dns flag and gives `CannotReach`, not `NetworkUnavailable`. A normal lookup of `.invalid` fails in ~0.2 s, but a lost UDP answer makes glibc retry (~20 s with the resolver blackholed), past the 2 s connect the `.invalid` cases had.
- **What breaks if you violate it:** green on the Linux gate, red only in the Windows job, which blocks every `deploys: true` task behind it.
- **Where it is enforced:**
  - Constants: every refused case takes its `Timeouts` from `common::refused_timeouts()` (connect = `REFUSAL_BUDGET` 5 s; whole-request and no-data = 2 × budget); the leak test's refused scenario uses it, not the 300 ms `ms`. Every unresolvable-host case takes its host from `common::unresolvable_host()` and its `Timeouts` from `common::resolver_timeouts()` (connect = `RESOLVER_BUDGET` 60 s; whole-request and no-data = 2 × budget).
  - Automatic: the `refused_addr()` probe asserts refusal within `REFUSAL_BUDGET / 2`, so a slower platform fails at the probe; the `unresolvable_host()` probe asserts the lookup fails (never resolves) within `RESOLVER_BUDGET / 2`, waiting at most the budget, so a slower resolver fails at the probe as an environment problem; `refused_timeouts_leave_the_refusal_room_on_every_deadline` and `resolver_timeouts_leave_the_lookup_room_on_every_deadline` destructure `Timeouts` exhaustively, so a new deadline field fails to compile until it is set.
  - By review only (T1): that a new refused-path test calls `refused_timeouts()`, a new DNS-failure test calls `unresolvable_host()` and `resolver_timeouts()`, and that test-side `took <` bounds stay under the connect budget.
  - `ci.yml` runs the Windows workspace tests with `--no-fail-fast`, so one red binary does not hide the others.
- **Don't:** put a short deadline on a refused address or an unresolvable host; hard-code `.invalid` hosts in a test; test a timeout with a refused or unroutable address (use a server that accepts and answers slowly); remove `--no-fail-fast`.
- **Not decided here:** which reason a product lookup cut off by the connect timeout gives, and the ~20 s client-drop wait behind it: T-079 (OQ-22).

### A wall-clock bound lies between the deadline and the nearest wrong-deadline bite

- **Defect that produced it:** F-013 (with T-020's and T-046's local overruns). Timeout tests asserted fixed margins of 0.4–1.5 s over a window that also holds a per-call `engine::http::client` build (a runtime thread and a CA-bundle parse, 64–781 ms under parallel load), client drop, VAD and WAV, and the post-processing cases subtracted a single cold baseline sample.
- **What breaks if you violate it:** a timing test red under host load with the product correct, or a bound so loose it no longer catches the wrong deadline it was written for.
- **Where it is enforced:** each timed bound states its lower side (the deadline minus timer slack: a deadline never fires early) and its upper side (the deadline plus `common::OVERHEAD_ALLOWANCE`, 3 s), and names the wrong-deadline bite above that bound; the test asserts the bound lies below the bite (server delay, production default) where it is a value. Server delays sit above the bound (T-073 cases and the configured post-processing case: 10 s against a 5 s limit, bound 8 s); the blackhole connect test has a 10 s total and a 3.3 s bound under the 5 s production connect. Post-processing baselines are the fastest of `BASELINE_SAMPLES` (3) runs. Review only (T1) for new tests.
- **Don't:** assert a fixed margin under a second over a window that includes a client build; put a server delay within `OVERHEAD_ALLOWANCE` of the deadline; time a stage against one baseline sample.

## Open

- The ~2 s user-facing refusal on Windows: OQ-07 (non-blocking).
