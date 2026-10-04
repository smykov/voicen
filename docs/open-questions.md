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
| OQ-05 | Concurrency model across specs: synchronous core with blocking reqwest (001, decisions #22 for `subscribe`) vs the async signatures in 002 contracts:67,81 (`async fn acquire` / `transcribe`), 003 contracts:10-14 (`#[async_trait] async fn process`; plan.md:17 and T017 use `tokio::time::timeout`) and 004 contracts:77 (`async fn test`). Proposal (agent): (a). Owner decides. | T-001, T-013, T-017, T-018, T-020 (not T-003) | (a) synchronous engines + blocking reqwest everywhere, timeouts via reqwest and a worker thread; (b) async engines with a tokio runtime in core (needs consent beyond decisions #9/#22) | 2026-10-02 | answered 2026-10-04: (a) — decision #42 |
| OQ-06 | Speech clip for "Test connection" (research R-9: a bundled ~1 s 16 kHz mono speech WAV posted to `/audio/transcriptions`; an agent cannot record a voice; some servers reject or hallucinate on silence). Same need as T-043 Q3 (real voice clips for the FR-12 check). Options: (a) the owner records a short phrase, committed under MIT (R-9 default); (b) a synthetic non-silent signal (untested against OpenAI/Groq); (c) a CC0 speech snippet with its licence in THIRD-PARTY-NOTICES. Proposal (agent): (a). | T-046, T-043 | (a) / (b) / (c) | 2026-10-04 | answered → decisions.md #59 |
| OQ-07 | On Windows a refused server (e.g. a local server that is not running) is reported only after ~2 s (TCP SYN retries; T-048), inside FR-24's 5 s connect limit. Making it immediate needs per-socket `TCP_MAXRT` in product code (reqwest/hyper do not expose it). Accept the ~2 s, or ask for a faster "cannot reach" on Windows? Non-blocking. Proposal (agent): accept. | T-048, FR-24 | accept / faster (product code) | 2026-10-04 | open |
| OQ-08 | Model sizes in the UI (T-045): binary units as Windows Explorer shows them (1 MB = 1 048 576 bytes, "141 MB" for base, so "needed" matches the free space Windows reports) or decimal SI as Hugging Face shows ("148 MB")? Only `formatSize` and the e2e literals depend on it. Proposal (agent): binary, Explorer-style. Non-blocking: T-045 proceeds on the proposal. | T-045, spec 002 FR-007 | binary / decimal | 2026-10-04 | open |
| OQ-09 | Closing the settings window while a model downloads (T-045): the download keeps running in the shell and reopening the window shows its progress, or closing cancels it? The spec is silent. Proposal (agent): keep running. Non-blocking: T-045 proceeds on the proposal. | T-045, spec 002 | keep running / cancel on close | 2026-10-04 | open |
