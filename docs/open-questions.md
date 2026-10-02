# Open questions

<!-- Anything unresolved that affects what gets built. Ask before code: a question before costs a message,
     after costs a rewrite. When answered, move the answer to decisions.md and set Status to `answered`.
     Status: open · answered → decisions.md #N · dropped (why). The router treats a question as resolved when
     its Status starts with answered, dropped, closed, resolved or done; a task with `blocked_on: OQ-NN` waits until then.
     New rows go at the end of the table. -->

| ID | Question | Blocks | Options | Asked | Status |
|---|---|---|---|---|---|
| OQ-01 | Target date for release 1? | plan, section 2 | a date / "when ready" | 2026-10-02 | open |
| OQ-02 | Sign the Windows installer in release 1 (SmartScreen warning otherwise)? | NFR-09 | no signing / free OSS signing (SignPath) / paid certificate | 2026-10-02 | open |
| OQ-03 | Reference machine for local-engine timing (CPU, RAM)? | NFR-01, NFR-03 | the owner's Windows PC / a CI runner | 2026-10-02 | open |
| OQ-04 | Maximum recording length? | FR-03 | 5 min / 10 min / configurable | 2026-10-02 | answered → decisions.md #1 |
| OQ-05 | Concurrency model across specs: synchronous core with blocking reqwest (001, decisions #22 for `subscribe`) vs the async signatures in 002 contracts:67,81 (`async fn acquire` / `transcribe`), 003 contracts:10-14 (`#[async_trait] async fn process`; plan.md:17 and T017 use `tokio::time::timeout`) and 004 contracts:77 (`async fn test`). Proposal (agent): (a). Owner decides. | T-001, T-013, T-017, T-018, T-020 (not T-003) | (a) synchronous engines + blocking reqwest everywhere, timeouts via reqwest and a worker thread; (b) async engines with a tokio runtime in core (needs consent beyond decisions #9/#22) | 2026-10-02 | open |
