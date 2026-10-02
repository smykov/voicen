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
