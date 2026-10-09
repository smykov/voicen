# Contract: core post-processing interfaces (`voicen-core`)

**Feature**: `003-llm-post-processing` · Data shapes: [data-model.md](../data-model.md)

Signatures are normative in shape, not in exact spelling. Types marked *(001)* or *(004)* are defined by those features; this feature adds only what is listed under "Added".

## Added: the stage trait (`voicen_core::post_process`)

```rust
#[async_trait] // or native async fn in trait, whichever 001's Engine trait uses
pub trait PostProcessor: Send + Sync {
    /// Called by the pipeline only for a non-empty raw transcript after successful transcription.
    /// Must return within `timeouts.post_processing` of being called; never panics on endpoint data.
    async fn process(&self, raw: &str, snapshot: &PostProcessingSnapshot) -> PostProcessOutcome;
}

pub struct ChatPostProcessor { /* openai::Client (001), Timeouts (001) */ }
impl PostProcessor for ChatPostProcessor { /* … */ }
```

**As implemented (T-020, decisions #42, #91, #99).** Synchronous, like 001's `Engine` (decision #42): `fn process(&self, raw: &str, input: &PostProcessInput<'_>) -> PostProcessOutcome` with `PostProcessInput { settings: &PostProcessingSettings, credentials: &dyn CredentialStore, timeouts: &Timeouts }`. The snapshot is the job's press snapshot (`settings.post_processing`, FR-010); the `enabled`, base-URL and key steps live in `ChatPostProcessor` (so `PassThrough` reads nothing): `enabled` false → `NotRun`, no key read; base URL failing `check_base_url` → `Skipped(NotConfigured)` (#91(2)), no key read; then one key read (`Err` = no key). `timeouts` is the one per-job value `Pipeline::process` also gives the engine (#99). The request goes through the crate-private `engine::http` helpers shared with the transcription engine, not a `chat_completion` method on the engine.

Guarantees:
1. Exactly one HTTP request per call, to `{snapshot.base_url}/chat/completions`, with the request body from data-model "ChatRequest" (FR-001, FR-013).
2. `Authorization` header present iff `snapshot.key.is_some()` (FR-012).
3. It returns `Applied(trimmed)` only when the trimmed content is non-empty. Every other result is `Skipped(reason)` per the R4 mapping (FR-003, FR-005, FR-006).
4. Wall-clock time from call to return is ≤ `timeouts.post_processing` + scheduling slack, with a test tolerance of 0.5 s (FR-004, SC-003).
5. The returned value and any error path contain no key, prompt, raw text or response body (FR-014).

## Added: pipeline wiring (in 001's pipeline)

```rust
// 001 FR-021 stage; this feature replaces the pass-through with:
fn post_process_stage(enabled: bool) -> Option<PostProcessingSnapshot>; // snapshot taken here (FR-010)
// pipeline: outcome = match snapshot { None => NotRun, Some(s) => post_processor.process(&raw, &s).await };
//           final_text = outcome.final_text(&raw);       // FR-016
//           if let Skipped(r) = outcome { notices.raise(Notice::PostProcessingSkipped(r)) } // FR-006
//           log::info!(target: "pipeline", "{}", outcome.log_fields(duration));   // FR-014
//           deliver(final_text)  // unchanged 001 delivery; job outcome = delivered (FR-007)
```

The pipeline constructor takes `Arc<dyn PostProcessor>`. Tests inject a fake. The shell injects `ChatPostProcessor` in `start_dictation` (NFR-11; T-074), not through a port, so the app and the shell tests run the same processor.

**As implemented (T-020).** No `Notice` type: a skip ends the job as `JobEnd::DeliveredSkipped { reason: SkipReason, delivery: DeliveryResult }`; `RecordingController::job_finished` sets tray `Error` and shows one overlay message from `post_process::skip_message` (decision #91(3): `copied_paste_manually` wins over the skip, the skip wins over `copied`). The toast is T-075, the log fields T-076.

## Added: helpers

```rust
pub const STARTER_PROMPT: &str = "…";                          // data-model, single source
pub fn defaults() -> PostProcessingSettings;                     // created by 004 (T068); used by 004's defaults()
pub fn validate(s: &PostProcessingSettings) -> Vec<FieldError>;  // used by 004's validator
impl PostProcessOutcome {
    pub fn final_text<'a>(&'a self, raw: &'a str) -> &'a str;
    pub fn log_fields(&self, duration: Duration) -> LogFields;   // allowlisted fields only
}
```

## Extended (owned elsewhere)

| Owner | Item | Extension by 003 |
|---|---|---|
| 001 | `timeouts::Timeouts` | field `post_processing: Duration`, default 15 s (connect 5 s already there); both are settings (`timeouts.post_processing`, `timeouts.connect`, decision #99), derived per job from the snapshot |
| 001 | `openai::Client` | method `chat_completion(&self, base_url, model, key: Option<&Secret>, messages, total: Duration) -> Result<String, Failure>` using the same connector, base-URL joining and connect timeout |
| 001 | `Notice` | variant `PostProcessingSkipped(SkipReason)` |
| 004 | `Settings` | field `post_processing: PostProcessingSettings` |
| 004 | `CredentialStore` / `KeySlot` | slot `PostProcessing` |
