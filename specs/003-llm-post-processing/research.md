# Research: LLM Post-Processing

**Feature**: `003-llm-post-processing` · **Date**: 2026-10-02 · **Plan**: [plan.md](plan.md)

The stack is fixed by `docs/requirements.md` §9 (Tauri 2, Rust 1.99, `reqwest`, Svelte 5, Playwright with mocked IPC). This file covers only how post-processing is built within that stack. Items marked *(from 001)* are owned by `001-dictation-via-api`; this feature uses them and does not redefine them.

## R1. Where the step lives and how the pipeline calls it

- **Decision**: A `PostProcessor` trait in `voicen-core` (`crates/voicen-core/src/post_process/`). The pipeline built in 001 (its FR-021) already has one post-processing stage between transcription and delivery, which passes text through unchanged. This feature supplies `ChatPostProcessor`, the real implementation, and the shell installs it when it builds the pipeline. The stage is called only with a non-empty raw transcript after a successful transcription.
- **Rationale**: Constitution IV and NFR-11 require that post-processing does not change recording, engine or delivery code. A single stage keeps P-011 (one seam). The trait lets the pipeline and ordering tests (FR-23) use a fake on Linux.
- **Alternatives considered**: (a) Post-processing as a decorator around each `Engine`. Rejected: it would run inside the engine timeouts, and every engine would have to change. (b) Post-processing in the shell after `voicen-core` returns. Rejected: the fallback and ordering logic would then be untestable on Linux, against constitution IV.

## R2. HTTP client and request shape

- **Decision**: Use the shared OpenAI-compatible `reqwest` client *(from 001: `openai::Client`)* and add one method, `chat_completion(base_url, model, key, messages)`, next to the transcription method. Request: `POST {base_url}/chat/completions` with JSON `{"model", "messages": [{"role":"system","content":prompt},{"role":"user","content":raw}], "stream": false}`. Send `Authorization: Bearer <key>` only when a key is set. Leave `temperature` and `max_tokens` unset, so the provider defaults apply. Accept `choices[0].message.content` as a string.
- **Rationale**: FR-015 and FR-06/FR-17 call for one shared client. `stream: false` makes the 15 s budget cover a single complete reply (Edge Cases). The system/user split is the most portable instruction format across OpenAI, Groq, Ollama, LM Studio and the llama.cpp server. Not setting sampling parameters avoids provider-specific rejections. For example, some reasoning models reject a `temperature` value.
- **Alternatives considered**: (a) Use the legacy `/completions` endpoint with a single concatenated prompt. Rejected: it is not in the FR-09 wording, and several local servers have dropped it. (b) Use the `async-openai` crate. Rejected: it would be a second client beside the shared one (P-011), and its types are tied to OpenAI's own schema rather than "compatible" servers.
- **Base-URL joining**: trailing slashes are normalised by the shared client *(from 001)*, so `…/v1` and `…/v1/` give the same URL.

## R3. Timeouts: 5 s connect, 15 s total

- **Decision**: The values come from `voicen_core::timeouts::Timeouts` *(the single source, P-010; created in 001)*. This feature adds a `post_processing: Duration` field, defaulting to 15 s, next to `connect` (5 s). The client's connector uses `connect`. The whole step (send, wait for headers, read the body, parse) is wrapped in `tokio::time::timeout(timeouts.post_processing, …)`, so the step never runs longer than 15 s from its start, whatever the server does.
- **Rationale**: FR-24 and the spec's FR-004. An outer `tokio::time::timeout` also bounds a slow body stream, which reqwest's per-request timeout handles only when it is set on every request. One outer bound is easier to test.
- **Testing**: The behaviour tests use `Timeouts` with scaled-down values (connect 200 ms, total 500 ms) against a local mock server that delays. One unit test pins the defaults to exactly 5 s and 15 s. One test uses the real 15 s against a server that never answers and asserts the step returns `Skipped(Timeout)` within 15 s ± 0.5 s (SC-003). That test costs about 15 s of gate time, which is accepted.
- **Alternatives considered**: (a) Run under `tokio::time::pause`. Rejected: real sockets and paused time do not mix. (b) Hard-code 15 s inside the post-processor. Rejected by P-010.

## R4. Mapping failures to reason categories

- **Decision**: One function, `classify(&Failure) -> SkipReason`, with an exhaustive mapping:

| Condition | Reason |
|---|---|
| outer 15 s timeout elapsed | `Timeout` |
| DNS failure, connection refused, connect timeout (reqwest `is_connect()`, including `is_connect() && is_timeout()`), TLS or proxy error | `Unreachable { host }` |
| HTTP 401, 403 | `InvalidKey` |
| any other non-2xx status | `Http { status }` |
| 2xx with a non-JSON body, no `choices[0].message.content` string, or content empty after trimming | `InvalidResponse` |

  `host` is taken from the configured base URL, never from the error text.
- **Rationale**: Spec FR-003, FR-005 and FR-006, and US2-1 to US2-5. The categories match 001's failure-reason codes (cannot reach host, invalid API key, timeout, HTTP code, unexpected response), so the notification and message catalog can share keys. Taking the host from the configuration keeps error text out of user-visible messages (FR-014, P-009).
- **Alternatives considered**: Show reqwest's error `Display`. Rejected: it can include the URL and, through some proxies, credentials, and it is not localisable (FR-15).

## R5. Secrets and logging

- **Decision**:
  - **Key storage and reading.** The key is read for each dictation through the credential-store trait *(from 004: `CredentialStore`, `KeySlot::PostProcessing`)* and held as `Secret`, a newtype whose `Debug` and `Display` print `***`. A read error or missing entry is treated as no key (spec Edge Cases).
  - **Error values.** `Failure` and `SkipReason` hold no body, no key, no prompt and no transcript.
  - **Log line.** There is exactly one, built from an allowlist of fields: `post_process outcome=applied|skipped reason=<code> duration_ms=<n>`.
  - **Redirects.** The key goes only to the configured endpoint (FR-012). The shared client keeps reqwest's default redirect policy, which drops `Authorization` on a cross-host redirect. A test pins this behaviour (T023).
  - **Library logging.** reqwest and hyper logging stays at its default `off` (no `log`/`tracing` subscriber enables their targets above `warn`). 006 owns the logger; this feature asserts the property with a capture test.
- **Rationale**: P-009 says to name every path. The paths are our own log line, error values that reach notifications, library request logging, and panic messages. Panic messages are covered by not formatting secrets into `expect` text and by the `Secret` newtype.
- **Alternatives considered**: Use the `secrecy` crate. That is acceptable, MIT/Apache (NFR-12), and can replace the newtype if 001 or 004 already adopted it. Whichever type 004 uses for key slots is used here, so there is one secret type (P-010).

## R6. Settings snapshot and live apply

- **Decision**: The post-processing settings are `PostProcessingSettings { enabled, base_url, model, prompt }` inside the settings model owned by 004. The pipeline takes an immutable snapshot of settings and key when a dictation enters the stage. A save during an in-flight request does not affect it (spec FR-010). The defaults and the starter-prompt text are constants in `voicen_core::post_process::defaults` that 004's single defaults function calls (P-010). Validation rules for the section (FR-011) are a function in this module that 004's validator calls.
- **Rationale**: 004 owns the model, persistence and validation display. 003 owns the post-processing defaults and rules, as 004's spec says ("its endpoint/model/prompt defaults are 003's").
- **Alternatives considered**: Read settings lazily during the request. Rejected because it breaks the rule that an in-flight request keeps the values it started with.

## R7. Notification and i18n

- **Decision**: The step returns `PostProcessOutcome`. The pipeline maps `Skipped(reason)` to a `Notice::PostProcessingSkipped(reason)` *(notice channel from 001: toast, overlay 3 s, tray error state per FR-25)*, then delivers the raw text as usual. Message keys `notice.post_processing_skipped.{timeout,unreachable,invalid_key,http,invalid_response}`, with parameters `host` and `status`, go into 004's English and Russian catalog.
- **Rationale**: FR-25 and spec FR-006 and Clarification Q4. The texts are localised per FR-15.
- **Alternatives considered**: Format the text in core. Rejected because the UI language lives in 004's catalog.

## R8. Mock chat endpoint for Linux tests

- **Decision**: Use the same mock OpenAI-compatible server that 001's engine tests use. The proposal is `wiremock` (MIT/Apache, async, can delay or refuse and records requests), extended with a `/chat/completions` route. For "connection refused", bind a port, drop the listener, and use that port. For "no TCP connection within 5 s", use a non-routable address (`10.255.255.1`) with the scaled connect timeout.
- **Rationale**: The project's verification decision (requirements §9) is "engine clients against a mock OpenAI-compatible server" on the Linux host. Recorded requests let a test assert the exact body, the auth header and that no request was made (SC-004).
- **Alternatives considered**: (a) A hand-written tokio TCP server. That is acceptable if 001 chose one; this feature follows 001's choice. (b) Real providers in CI. Rejected because of keys and flakiness.

## R9. Where each requirement is verified

| Requirement | Linux host (core with fakes / UI with mocked IPC) | Windows CI runner | Owner's manual check |
|---|---|---|---|
| FR-09 success (rewrite delivered, request shape, trim) | core: `ChatPostProcessor` against the mock server; pipeline with a fake engine and fake delivery | build, install and smoke only | one real dictation with post-processing on (OpenAI or Groq chat, and a local Ollama) per release |
| FR-09 failure branch (every reason → raw delivered + notice) | core: one test per reason against the mock server; pipeline test that raw text reaches delivery and a `PostProcessingSkipped` notice is raised | toast, tray error state: Windows integration test on the notifier from 001 | the toast and overlay text seen once per release |
| FR-24 (5 s connect, 15 s total) | core: scaled timeouts and one real 15 s test; defaults test | — | — |
| FR-23 interaction (ordering with a slow post-processor) | core: pipeline ordering test with a fake post-processor that delays | — | — |
| NFR-04 (key only in the credential store, never logged) | core: fake `CredentialStore`; log-capture test over success and every failure mode; settings-file scan | Credential Manager round trip for slot `PostProcessing` (owned by 004's Windows test) | — |
| NFR-05 (off → no network call) | core: mock server receives zero requests with post-processing off; UI: privacy note shown in the section (Playwright, mocked IPC) | — | — |
| NFR-11 (one optional step, no change to recording, engine or delivery) | core: the pipeline is built with a fake `PostProcessor`; review checks the diff touches no recording, engine or delivery file | — | architecture review |
| Settings defaults, validation, live apply (FR-13 part) | core: defaults and validation functions; UI: Playwright on the Post-processing tab (004's page) with mocked IPC | — | — |
