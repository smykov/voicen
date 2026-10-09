# HTTP clients and host-name lookup (`engine::http`)

**Code:** `crates/voicen-core/src/engine/http.rs` (`HttpClient`, `client`, `client_with_read_timeout`, `DeadlineResolver`, `LookupRecord`), `crates/voicen-core/src/engine/lookup.rs` (`Lookup`, `SystemLookup`; the `test-fakes` `HeldLookup`, `FixedLookup`, `install`), `engine::openai::send_error` · **Tests that pin it:** `engine::http::tests` (the lookup record), `openai_client` (`unanswered_lookup_is_network_unavailable_at_the_connect_deadline`, `lookup_answered_after_the_connect_deadline_is_network_unavailable_not_a_late_success`, `unanswered_lookup_cut_by_a_total_deadline_below_connect_is_network_unavailable`, `lookup_error_at_once_is_network_unavailable`, `lookup_answered_at_once_with_a_refused_address_is_cannot_reach`), `post_process_chat`, `post_process_timeout`, `local_download`, `connection_test`, `api_pipeline` (the `unanswered_lookup_*` twins), `failure::classify_table`; `make check-http-client-builder`

Tasks: T-079 (found by T-078). Decisions: #106, #113. Contract: `specs/001-dictation-via-api/contracts/openai-transcription.md` › Classification. Related: F-013.

## Invariants

### Every reqwest client of the crate is built by `engine::http`, with the deadline resolver

- **Defect that produced it:** T-079 (F-013 part 1). With reqwest's default `GaiResolver` (reqwest 0.13.5, hyper-util 0.1.21) the lookup runs on the runtime's blocking pool. A lookup with no answer was cut by the connect timer without the dns flag (so `CannotReach` instead of `NetworkUnavailable`), and the client's drop then waited for it in the runtime's blocking-pool shutdown until the OS resolver gave up (~20 s on Linux, against FR-24's 5 s connect limit). `Downloader::transfer` had its own `Client::builder()` with the same behaviour.
- **What breaks if you violate it:** a client built elsewhere gets the default resolver back: a dictation, post-processing call, model download or Test connection whose DNS server does not answer waits for the OS resolver, and dictation reports `CannotReach` instead of `NetworkUnavailable`.
- **Where it is enforced:** `engine::http::client` (engine, Test connection, post-processing) and `engine::http::client_with_read_timeout` (the download's per-read timeout) are the only constructors; both install `DeadlineResolver`. `make check-http-client-builder` (`scripts/ci/http-client-builder.sh`, T3) refuses `Client::builder`, `ClientBuilder::new` and `Client::new(` in any other `*.rs` under `crates/voicen-core/src`. Not caught (review only): a `use ... as` alias of the reqwest types, a client built by a macro.
- **Don't:** build a client "just for this one call" with `reqwest::blocking::Client::builder()`, or turn on reqwest's `hickory-dns` feature as a shortcut (option C of the T-079 analysis: a new dependency tree, other Windows resolver semantics, and it still needs the record).

### A lookup never runs on the runtime's blocking pool and is never awaited

- **Defect that produced it:** T-079: the lookup on the blocking pool was joined at the client's drop.
- **What breaks if you violate it:** the call returns only when the OS resolver gives up, long after its connect deadline.
- **Where it is enforced:** `DeadlineResolver` runs each lookup (`Lookup::lookup`, the OS resolver through std `ToSocketAddrs` in production) on its own named, detached thread and wakes the future through a std-only one-shot slot. Nothing keeps the thread's handle. The thread ends when the OS answers (bounded by user retries: one thread per unanswered lookup). Tests: every `unanswered_lookup_*` twin decides by ordering ("the call returned while the lookup was still held", `tests/common/held.rs`).
- **Don't:** join the lookup threads at the resolver's drop, or move the lookup to `tokio::task::spawn_blocking`.

### A send that fails while a lookup of its client is unanswered is a DNS failure

- **Defect that produced it:** T-079: a `DnsError` came only from the resolver's own error, so the dns fact depended on which timer fired first.
- **What breaks if you violate it:** dictation gives `CannotReach` (connect timer) or `Timeout` (a whole-request deadline below connect, #113) for a DNS server that does not answer.
- **Where it is enforced:** `LookupRecord` counts the lookups started and not yet answered, and marks one whose future was dropped before its answer (the drop guard of `DeadlineResolver`'s pending future). Both are needed: the blocking client applies the whole-request deadline on the caller's thread, so `send_error` can run while the runtime thread still holds the lookup future. `HttpClient::send` (and `HttpClient::send_capped`) pass `record.unanswered()` to `engine::openai::send_error`, which sets `dns = e.is_dns() || unanswered`. Classification stays in `failure::classify` (dns → `NetworkUnavailable`) and `DownloadFailure::from_transport` (dns → `SourceUnreachable`); Test connection maps `NetworkUnavailable` to `CannotReach{host}` in `connection_test::from_failure` (#51). An answered lookup, or the resolver's own error, leaves the record false, so a refused or blackholed address behind a name stays `CannotReach`.
- **Don't:** give the resolver its own deadline at `connect − ε` instead of the record (option B: a timer race, the F-005 / F-013 clock class moved into the product); set the record on every drop; read the record only for errors with the connect flag (the owner chose `NetworkUnavailable` for a lookup cut by the whole-request deadline too, #113).

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| Resolver deadline at `connect − margin`, no record (option B) | the DNS error and reqwest's `TimedOut` race across a 1 ms tick or a starved runtime thread; a margin only makes the race rarer | T-079 analysis, F-013 |
| reqwest `hickory-dns` with a resolver timeout (option C) | new dependency tree and license review; does not take the Windows OS resolver configuration the way GetAddrInfoW does; still needs the record | T-079 analysis |

## Open

- none
