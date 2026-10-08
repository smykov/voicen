# Post-processing

**Code:** `crates/voicen-core/src/post_process/{mod,chat}.rs`, `crates/voicen-core/src/engine/http.rs`, `Pipeline::process` / `Pipeline::release`, `RecordingController::job_finished` · **Tests that pin it:** `tests/post_process_chat.rs`, `tests/post_process_timeout.rs`, `post_process::tests`, `pipeline::tests::post_processor_*` / `skipped_post_processing_*`, `recording::tests::delivered_skipped_*`, `i18n::tests::post_processing_skip_ids_*`

Spec: `specs/003-llm-post-processing/` (contracts/core-post-process.md, data-model.md). Decisions: #42 (synchronous engines), #91 (scope, `not_configured`, one overlay message), #99 (timeouts are settings). Task: T-020; shell wiring T-074, toast T-075, log fields T-076.

## Invariants

### A post-processed dictation always ends delivered

- **Why:** spec 003 FR-007, FR-016: post-processing improves the text; it never loses the dictation.
- **What breaks if you violate it:** a chat failure costs the user the transcript (or keeps a pending recording for a transcription that succeeded).
- **Where it is enforced:** `PostProcessOutcome::final_text` (applied text, else the raw transcript byte for byte); `Pipeline::release` ends a skip as `JobEnd::DeliveredSkipped`, never `Failed`.
- **Don't:** map a skip to a `FailureReason`, or deliver "" / the reason text on a skip.

### One `Timeouts` per job, for the engine and the post-processor

- **Why:** decision #99: the post-processing deadline is a setting read from the press snapshot.
- **What breaks if you violate it:** a configured `post_processing_s` is ignored (the 15 s default applies), or the test override reaches one stage and not the other.
- **Where it is enforced:** `Pipeline::process` derives `timeouts` once and passes the same value to `TranscribeRequest` and `PostProcessInput`; `ChatPostProcessor` has no default or clamp of its own.
- **Don't:** call `Timeouts::default()` in the post-processing path, or read `Pipeline.timeouts` (the test override, `None` in production) directly.

### One HTTP path for OpenAI-compatible requests

- **Why:** T-020 Context: no second client, key rule or body read.
- **What breaks if you violate it:** the chat path drifts from the transcription one (an unusable key sent anyway, an uncapped body, a different connect limit).
- **Where it is enforced:** `engine::http::{authorization, client, send_capped}`, used by `OpenAiCompatibleEngine::send` and `post_process::chat`; failures through `failure::classify` then `SkipReason::from_failure`.
- **Don't:** use `bearer_auth`, a custom header for the key, `Response::json()`, or a second `Client::builder()`.

### Off means no key read; a bad URL means no key read and a visible skip

- **Why:** spec 003 FR-002 / NFR-05 (no Credential Manager access when off); #91(2) (a broken config is shown, not silent).
- **Where it is enforced:** `ChatPostProcessor::process` order: `enabled` → `check_base_url` → key read → request. `PassThrough` reads nothing.
- **Don't:** read the key in the pipeline or before the URL check.

### A key read error is no key

- **Why:** spec 003 Edge Cases, research R5. Deliberately unlike transcription's `KeyStoreUnavailable` (decision #44): a local LLM server needs no key, and the dictation is delivered anyway.
- **Don't:** "fix" it toward #44.

### Nothing secret in a skip

- **Why:** FR-014, P-009.
- **Where it is enforced:** `SkipReason` holds only `host[:port]` (`engine::openai::host_port` of the configured base URL) or a status; never the error text, URL query, body, prompt, transcript or key.

### One overlay message per skip (decision #91(3))

- `post_process::skip_message`: after a failed paste (`CopyManual`) `notice.copied_paste_manually` wins (the user must act); otherwise the skip message with its params. Tray `Error` marks every skip. #91(3) awaits owner ratification; a change is a change to that one function.

## Known residual

- Redirects use reqwest's default policy (up to 10). A cross-origin redirect drops `Authorization` (pinned by `cross_host_redirect_drops_the_key`), but a 307/308 re-sends the body (prompt and transcript) to the other host, and its reply is used. Same as the transcription engine; changing it means changing the shared `engine::http::client`.
- `chat_completions_url` always appends `chat/completions` (spec 003 FR-001). Whether a full endpoint URL is kept as is, like decision #96 for transcription, is OQ-17.

## Open

- OQ-17 (full endpoint in `post_processing.base_url`).
