# voicen-core test network

**Code:** `crates/voicen-core/tests/common/mod.rs` (`refused_addr()`, `REFUSAL_BUDGET`, `refused_timeouts()`), `crates/voicen-core/tests/common/refused_addr_tests.rs`, users: `tests/api_pipeline.rs`, `tests/openai_client.rs`, `tests/diag_pipeline.rs` (T-008's redaction run; includes `refused_addr_tests.rs` and its refused scenario uses `refused_timeouts()`), `tests/local_server.rs` (T-018; includes `refused_addr_tests.rs`), `tests/local_download_refused.rs`, `.github/workflows/ci.yml` (windows job, `--no-fail-fast`) · **Tests that pin it:** `refused_addr_tests::*` (port below the ephemeral range, loopback IPv4, refused by a connect probe within `REFUSAL_BUDGET / 2`, `refused_timeouts_leave_the_refusal_room_on_every_deadline`)

Tasks: T-016, T-047 (F-004), T-048 (F-005). Classes `test-port-race` and `windows-refused-connect-timing` in `docs/failures.md`. Decisions: #53, #56.

## Invariants

### A refused address never comes from a released port

- **Defect that produced it:** F-004. Helpers bound `127.0.0.1:0`, read the port and released it; a sibling test's server in the same binary could get that port, so the address was no longer refused (about 1 in 7058 per bind).
- **What breaks if you violate it:** a rare flaky "refused" test.
- **Where it is enforced:** the single seam `common::refused_addr()` (127.0.0.1:1, below the Linux and Windows ephemeral ranges); it asserts a `ConnectionRefused` probe at each use, so a listener on port 1 fails loudly. That is the only automatic check, and it covers the environment, not the rule. The rule itself is enforced by review (T1); there is no grep guard. `local_download_refused` keeps its own test binary (refused-then-serve).
- **Don't:** bind port 0 and release it to get a "closed" port.

### Every deadline on a refused path leaves the Windows refusal time

- **Defect that produced it:** F-005. On windows-latest a refused loopback connect returns after about 2.17 s; Linux returns at once. A whole-request deadline below that gives `Timeout` (download: `DownloadInterrupted`), not `CannotReach` (`SourceUnreachable`); only the refusal or the connect timeout gives the latter.
- **What breaks if you violate it:** green on the Linux gate, red only in the Windows job, which blocks every `deploys: true` task behind it.
- **Where it is enforced:**
  - Constants: every refused case takes its `Timeouts` from `common::refused_timeouts()` (connect = `REFUSAL_BUDGET` 5 s; whole-request and no-data = 2 × budget); the leak test's refused scenario uses it, not the 300 ms `ms`.
  - Automatic: the `refused_addr()` probe asserts refusal within `REFUSAL_BUDGET / 2`, so a slower platform fails at the probe; `refused_timeouts_leave_the_refusal_room_on_every_deadline` destructures `Timeouts` exhaustively, so a new deadline field fails to compile until it is set.
  - By review only (T1): that a new refused-path test calls `refused_timeouts()`, and that test-side `took <` bounds stay under the connect budget.
  - `ci.yml` runs the Windows workspace tests with `--no-fail-fast`, so one red binary does not hide the others.
- **Don't:** put a short deadline on a refused address; test a timeout with a refused or unroutable address (use a server that accepts and answers slowly); remove `--no-fail-fast`.

## Open

- The ~2 s user-facing refusal on Windows: OQ-07 (non-blocking).
