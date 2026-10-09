# Post-processing

**Code:** `crates/voicen-core/src/post_process/{mod,chat,settings}.rs`, `crates/voicen-core/src/engine/http.rs`, `src-tauri/src/dictation.rs` `start_dictation`, `Pipeline::process` / `Pipeline::release`, `RecordingController::job_finished` · **Tests that pin it:** `tests/post_process_chat.rs`, `tests/post_process_timeout.rs`, `post_process::tests`, `pipeline::tests::post_processor_*` / `skipped_post_processing_*`, `recording::tests::delivered_skipped_*`, `i18n::tests::post_processing_skip_ids_*`, `post_process::settings::tests::validate_*`, `settings::validate::tests::post_processing_on_is_validated_for_every_engine`, `settings::service::tests::save_refuses_an_unusable_post_processing_url_while_enabled`, `src-tauri/tests/dictation_e2e.rs` `post_processing_*` (Windows CI)

Spec: `specs/003-llm-post-processing/` (contracts/core-post-process.md, data-model.md). Decisions: #42 (synchronous engines), #91 (scope, `not_configured`, one overlay message), #99 (timeouts are settings). Tasks: T-020; save rule and settings tab T-021; shell wiring T-074 (done: follow-up 1 of #91), toast T-075, log fields T-076.

## Invariants

### The app runs the same post-processor as the shell tests

- **Why:** T-006 invariant 5 (`start_dictation` is the one builder of the app's `DictationSession`); #91 follow-up 1 (T-020 was core only; the shell installed `PassThrough`, so an enabled step never ran in the app).
- **What breaks if you violate it:** the installed app silently ignores post-processing, or the shell tests prove a processor the app does not run.
- **Where it is enforced:** `start_dictation` installs `Arc::new(ChatPostProcessor::new())` in `PipelineDeps.post_processor`; it is stateless (settings, key and timeouts come from each job), so `DictationPorts` has no post-processor port (T-074, option B rejected).
- **Don't:** add a post-processor field to `DictationPorts`, or install `PassThrough` in `start_dictation`.

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

### One save rule for post-processing, the engines' URL rule

- **Why:** spec 003 FR-011, 004 FR-004; T-021 (option A).
- **What it says:** while `enabled` is true, a save is refused exactly when the base URL fails the engines' URL rule (`required` / `url.malformed` / `url.credentials`), the model is empty after trim, or the prompt is empty after trim, all errors at once in that order. While off, nothing is refused (the fields are kept as entered). A key is never required.
- **Where it is enforced:** `post_process::settings::validate`, called once from `settings::validate::validate` outside the engine match (so for every engine); the URL mapping is `settings::validate::base_url_rule` (`pub(crate)`), shared with the engine URLs.
- **Don't:** copy the URL rule, put the call inside one engine's arm, add a required or URL check in the UI or the e2e mock (the Playwright failure branch scripts core's own outcome, fixture `refused_post_processing_on_empty`), or require a key. `ChatPostProcessor` still checks the URL at run time for a hand-edited file (`NotConfigured`).

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
