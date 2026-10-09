# Implementation Plan: LLM Post-Processing

**Branch**: `003-llm-post-processing` | **Date**: 2026-10-02 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/003-llm-post-processing/spec.md`

**Note**: Implementation goes through teamwright tasks (`docs/tasks/T-NNN.md`, `design_ref` → this plan), never `/speckit-implement`.

## Summary

Optional LLM post-processing (req FR-09, FR-24; constraints NFR-04, NFR-05, NFR-11). When enabled, `voicen-core` sends the user's prompt (system role) and the raw transcript (user role) to `<base URL>/chat/completions` through the shared OpenAI-compatible `reqwest` client from 001, and delivers the trimmed reply. Any failure delivers the raw transcript instead and raises a "post-processing skipped — <reason>" notice (toast, 3 s overlay, tray error state). Failures include no connection or a 5 s connect timeout, 401/403, any other HTTP status, an empty or invalid reply, and no reply within 15 s of the step's start. The step is one `PostProcessor` trait object plugged into the pass-through stage that 001 provides. Settings and the key slot belong to 004; this feature supplies the defaults (off, plus a starter prompt), the validation rules, the catalog strings and the privacy note.

## Technical Context

**Language/Version**: Rust 1.99, edition 2021 (`voicen-core`, `src-tauri`); TypeScript + Svelte 5 (UI)

**Primary Dependencies**: `reqwest` (shared client from 001), `tokio` (`time::timeout`), `serde`/`serde_json`; test-only: the mock OpenAI-compatible server chosen in 001 (proposed `wiremock`, see research R8). There are no new runtime crates beyond 001's, and all are MIT-compatible (NFR-12).

**Storage**: Settings file owned by 004 (`post_processing` section, no key). Windows Credential Manager slot `PostProcessing` through 004's `CredentialStore` (`KeySlot::PostProcessing`).

**Testing**: `cargo test -p voicen-core` in Docker (fakes and mock server); vitest and Playwright with mocked IPC for the UI parts; Windows CI `cargo test --workspace` for the shell wiring.

**Target Platform**: Windows 10/11 x64; core logic is tested on the Linux host.

**Project Type**: Desktop app (Tauri 2): core library, Windows shell and web UI.

**Performance Goals**: The step finishes within 15 s total (SC-003, tolerance ≤ 0.5 s). With post-processing off, it adds no measurable latency (no network, no allocation of a request). NFR-01 is measured without post-processing.

**Constraints**: Connect timeout 5 s and total timeout 15 s, both from the single `Timeouts` source (P-010). No key, prompt, transcript or response body in logs, errors or notices (NFR-04, FR-20, P-009). With post-processing off, no network call (NFR-05).

**Scale/Scope**: One request per dictation; transcripts up to a 10-minute recording. Several dictations may be in flight at once, delivered in order (FR-23).

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | How this plan complies | Status |
|---|---|---|
| I. PRINCIPLES.md | P-004/P-005: one red test per guarantee, including every skip reason and the real 15 s timeout. P-009: allowlisted log fields, `Secret` newtype, host taken from configuration rather than error text. P-010: timeouts, starter prompt and defaults each defined once. P-011: one shared HTTP client and one pipeline stage. P-013: a test through the real settings loader proves the runtime reads `post_processing`. | PASS |
| II. Privacy by default | Only prompt, raw transcript and model are sent, and only to the configured endpoint. Off means no call. The key is in Credential Manager only. The privacy note is in the settings section. | PASS |
| III. Never lose the user's words | Every failure delivers the raw transcript and notifies. An empty reply is never pasted. All Must failure branches (FR-09, FR-24) appear in the spec as US2-1 to US2-7 and have tests. | PASS |
| IV. Platform-independent core, thin shell | All logic is in `voicen-core` and testable on Linux. The shell only installs `ChatPostProcessor` and shows the notice through 001's notifier. Post-processing shares the OpenAI-compatible client (FR-06/FR-17). | PASS |
| V. Measured, not assumed | The timeout is measured in a test. Durations go to the log (FR-20). The verification location is stated per requirement (research R9). | PASS |

Post-design re-check (after Phase 1): PASS, with no violations, so Complexity Tracking is empty.

## Verification per requirement

| Req | Linux host | Windows CI | Owner manual |
|---|---|---|---|
| FR-09 (success and failure branch) | core: `ChatPostProcessor` vs mock server; pipeline with fakes | shell wiring builds; notifier test from 001 | real endpoint and unreachable endpoint, per release |
| FR-24 (post-processing 15 s, connect 5 s) | core: scaled timeouts, real 15 s test, defaults test | — | — |
| NFR-04 | core: fake `CredentialStore`, log capture, settings-file scan | Credential Manager slot round trip (004) | log file inspected once per release |
| NFR-05 | core: zero requests when off; UI: privacy note (Playwright) | — | — |
| NFR-11 | core: pipeline built with a fake `PostProcessor`; diff touches no recording, engine or delivery code | — | architecture review |

Details: [research.md](research.md) R9.

## Dependencies

- **001-dictation-via-api** (must be in place first):
  - the pipeline with its pass-through post-processing stage (001 FR-021);
  - `openai::Client`;
  - `timeouts::Timeouts`;
  - `Notice` with the toast, overlay and tray channel (FR-25);
  - the ordering of in-flight dictations (FR-23);
  - the mock OpenAI-compatible server for tests.
- **004-settings-and-first-run**:
  - the `Settings` model with defaults, validation and persistence;
  - `CredentialStore`, `KeySlot` and `Secret` (defined by 004; decisions #21);
  - the data type `PostProcessingSettings`, `STARTER_PROMPT` and `defaults()` (created by 004's foundational phase; 003 adds `validate()` and wiring);
  - the message catalog (en/ru);
  - the Post-processing tab with its fields.

  Core tasks of 003 can start before 004's UI exists. Only the UI tasks (privacy note, validation display) wait for the tab.

## Project Structure

### Documentation (this feature)

```text
specs/003-llm-post-processing/
├── plan.md              # this file
├── research.md          # Phase 0
├── data-model.md        # Phase 1
├── quickstart.md        # Phase 1
├── contracts/
│   ├── core-post-process.md   # PostProcessor trait, helpers, extensions of 001/004 types
│   └── ipc.md                 # settings DTO fragment, notice payload, catalog keys
├── checklists/
│   ├── requirements.md
│   └── requirements-quality.md
└── tasks.md             # /speckit-tasks output
```

### Source Code (repository root)

```text
crates/voicen-core/src/
├── post_process/
│   ├── mod.rs           # PostProcessor trait, PostProcessOutcome, SkipReason, final_text, log_fields (As built T-076: no log_fields; see data-model.md "As built (T-076)")
│   ├── chat.rs          # ChatPostProcessor: request build, reply parse, classify (R4)
│   └── settings.rs      # validate() and wiring; the data type, STARTER_PROMPT and defaults() are created by 004 (decisions #21)
├── openai.rs            # (001) + chat_completion()
├── timeouts.rs          # (001) + post_processing: 15 s
├── pipeline.rs          # (001) stage uses Arc<dyn PostProcessor>; notice + log line (As built T-076: the outcome rides on the dictation line as pp=, pp_reason=, pp_ms=, not a separate line)
└── notice.rs            # (001) + PostProcessingSkipped
crates/voicen-core/tests/
└── post_process_*.rs    # mock-server tests, pipeline tests, log-capture test

src-tauri/src/           # install ChatPostProcessor when building the pipeline; map notice to toast (001's mapper)

i18n/{en,ru}.json        # (the one catalog at the repo root, teamwright T-005, decisions #13) + notice.post_processing_skipped.*, settings.post_processing.privacy_note
src/routes/settings/…    # (004 Post-processing tab) + privacy note
e2e/post-processing.spec.ts  # tab defaults, validation, privacy note, skipped notice text (mocked IPC)
```

**Structure Decision**: Use the existing three-part layout: `voicen-core` (area `core`), `src-tauri` (Windows shell) and `src/` plus `e2e/` (area `ui`). Exact file names inside 001's and 004's modules follow what those features create; the paths above name the module, not a binding filename.

## Complexity Tracking

No violations.
