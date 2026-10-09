---
description: "Task list for LLM post-processing (003)"
---

# Tasks: LLM Post-Processing

**Input**: Design documents from `specs/003-llm-post-processing/`

**Prerequisites**: [plan.md](plan.md), [spec.md](spec.md), [research.md](research.md), [data-model.md](data-model.md), [contracts/](contracts/), [quickstart.md](quickstart.md)

**Tests**: Required. The constitution (I: P-004 and P-005; III: every Must failure branch has a test) makes red tests mandatory, and every test task comes before its implementation task.

**Organization**: Tasks are grouped by user story. Each task names the requirement ids it covers (`req …`) and the spec FR (`FR-0NN`). These tasks are converted to teamwright tasks with `design_ref: specs/003-llm-post-processing/plan.md`, not run with `/speckit-implement`.

**Verification location** (per task): **L** = Linux host (core with fakes, UI with mocked IPC), **W** = Windows CI runner, **M** = owner's manual check.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: User story from spec.md (US1, US2, US3)

## Path Conventions

`crates/voicen-core/` (area `core`), `src-tauri/` (Windows shell), `src/` and `e2e/` (area `ui`). File names inside modules created by 001/004 follow those features; the paths below name the module.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Module skeleton and test fixtures. This phase needs 001's `openai::Client`, `timeouts::Timeouts`, pipeline and mock server to be merged.

- [ ] T001 Create the module `crates/voicen-core/src/post_process/{mod.rs,chat.rs,settings.rs}` and export it from `crates/voicen-core/src/lib.rs` (empty types only, gate green) — req NFR-11 — L
- [ ] T002 [P] Add a `/chat/completions` route helper to the shared mock OpenAI-compatible test server (from 001, proposed `wiremock`, research R8) in `crates/voicen-core/tests/support/`. It must be able to reply with a fixed body, reply with a status, delay, and record requests. Add a refused-port helper and a non-routable-host helper — req FR-09, FR-24 — L

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: The types and seams every story uses. No user story starts before this phase is done.

- [ ] T003 Write red unit tests in `crates/voicen-core/src/timeouts.rs` asserting `Timeouts::default().post_processing == 15 s` and `.connect == 5 s`, then add the `post_processing: Duration` field to 001's `Timeouts` (one source, P-010) — req FR-24 — FR-004 — L
- [ ] T004 [P] Define `PostProcessOutcome { NotRun, Applied(String), Skipped(SkipReason) }` and `SkipReason { Timeout, Unreachable{host}, InvalidKey, Http{status:u16}, InvalidResponse }` in `crates/voicen-core/src/post_process/mod.rs`, with red tests first:
  - `final_text(raw)` returns the applied text, or raw for `NotRun` and `Skipped`;
  - `log_fields(duration)` renders only `outcome`, `reason` and `duration_ms` (data-model "Log record").
  — req FR-09, FR-20, NFR-04 — FR-014, FR-016 — L
- [ ] T005 [P] Define the `PostProcessor` trait and `PostProcessingSnapshot { base_url, model, prompt, key: Option<Secret> }` in `crates/voicen-core/src/post_process/mod.rs` per `contracts/core-post-process.md`. Reuse 004's `Secret` newtype; it must show a redacted `Debug` (test that `format!("{:?}")` does not contain the key) — req NFR-04, NFR-11 — FR-012, FR-015 — L
- [ ] T006 Write red pipeline tests in `crates/voicen-core/tests/post_process_pipeline.rs`, then replace 001's pass-through stage in `crates/voicen-core/src/pipeline.rs` with `Arc<dyn PostProcessor>` plus a snapshot taken when the job enters the stage. Use a fake `PostProcessor` and a fake delivery. The tests are:
  - **Off.** The fake is never called and raw text is delivered.
  - **Applied.** The applied text is delivered.
  - **Empty raw, no speech, failed transcription.** The fake is not called.
  - **Settings saved mid-request.** The in-flight call keeps its snapshot.
  - **Retry.** A successful retry of a failed transcription (FR-11) runs the stage with the settings current at retry time.
  — req FR-09, FR-11, FR-12, FR-13, NFR-11 — FR-002, FR-008, FR-010, FR-015 — L

**Checkpoint**: The pipeline accepts any `PostProcessor`, and the off path makes no call.

---

## Phase 3: User Story 1 - Dictated text is cleaned up by an LLM before it is pasted (Priority: P1) 🎯 MVP

**Goal**: With post-processing on and a working endpoint, the trimmed reply is delivered instead of the raw transcript.

**Independent Test**: The mock endpoint returns `"Привет, как дела?\n"`. One dictation delivers `Привет, как дела?`, and the recorded request carries the model, system = prompt, user = raw, and the bearer key (quickstart §1).

### Tests for User Story 1 (red first)

- [ ] T007 [P] [US1] Write red tests in `crates/voicen-core/tests/post_process_chat.rs`:
  - exactly one `POST {base_url}/chat/completions` per call (base URL with and without a trailing `/`);
  - body `{"model", "messages":[system=prompt, user=raw], "stream": false}` and nothing else (no history, no audio);
  - an `Authorization: Bearer <key>` header when a key is set, and none when there is no key;
  - the reply is trimmed (`"  text\n"` → `"text"`) while inner line breaks are kept.

  — req FR-09, NFR-05 — FR-001, FR-005, FR-012, FR-013 — L
- [ ] T008 [P] [US1] Write a red pipeline test in `crates/voicen-core/tests/post_process_pipeline.rs`: the post-processed text is the final transcript given to delivery and to the history sink (fake), and the raw text is not delivered — req FR-09, FR-16 — FR-016 — L
- [ ] T009 [P] [US1] Write a red test in `crates/voicen-core/tests/post_process_pipeline.rs`: with post-processing off over 5 dictations, the mock chat endpoint records 0 requests (SC-004) — req FR-09, NFR-05 — FR-002 — L

### Implementation for User Story 1

- [ ] T010 [US1] Add `chat_completion(base_url, model, key: Option<&Secret>, messages, total)` to 001's shared client in `crates/voicen-core/src/openai.rs`. It reuses the same connector, the base-URL joining and the 5 s connect timeout, and sends `stream: false` (research R2). It must not add a second HTTP client — req FR-09, NFR-11 — FR-001, FR-015 — L
- [ ] T011 [US1] Implement `ChatPostProcessor::process` in `crates/voicen-core/src/post_process/chat.rs`: build the request from the snapshot, parse `choices[0].message.content`, trim it, and return `Applied` (makes T007–T009 green) — req FR-09 — FR-001, FR-005 — L
- [ ] T012 [US1] Install `ChatPostProcessor` when the shell builds the pipeline in `src-tauri/src/` (the pipeline construction from 001). The Windows CI `cargo test --workspace` and the smoke check must stay green — req FR-09, NFR-11 — FR-015 — W

**Checkpoint**: US1 works end to end on Linux with the mock endpoint. MVP.

---

## Phase 4: User Story 2 - A failed or slow LLM never loses the dictation (Priority: P1)

**Goal**: Every failure delivers the raw transcript and raises one "post-processing skipped — <reason>" notice. The step never takes longer than 15 s.

**Independent Test**: For each failure mode of the mock (refused, non-routable, 401, 403, 404, 429, 500, delay beyond the timeout, `{}`, `{"choices":[]}`, non-JSON, `"   "`), the raw transcript is delivered and exactly one notice with the matching reason is raised (quickstart §1).

### Tests for User Story 2 (red first)

- [ ] T013 [P] [US2] Write red classification tests in `crates/voicen-core/tests/post_process_chat.rs`, one per row of research R4:
  - refused port or non-routable host (with scaled `Timeouts`) → `Unreachable{host}`, with the host taken from the configured base URL;
  - 401 and 403 → `InvalidKey`;
  - 404, 429 and 500 → `Http{status}`;
  - malformed body, no choices, non-string content, or whitespace-only content → `InvalidResponse`;
  - a delayed reply beyond the scaled total → `Timeout`.

  — req FR-09, FR-24 — FR-003, FR-005, FR-006 — L
- [ ] T014 [P] [US2] Write a red real-timeout test in `crates/voicen-core/tests/post_process_timeout.rs`: the mock accepts the connection and never answers, and with default `Timeouts` the step returns `Skipped(Timeout)` within 15 s ± 0.5 s (SC-003) — req FR-24 — FR-003, FR-004 — L
- [ ] T015 [P] [US2] Write red pipeline tests in `crates/voicen-core/tests/post_process_pipeline.rs`. They use a fake `PostProcessor` that returns each `Skipped(reason)`, a fake notifier and a fake pending store. For every reason they check:
  - the raw text is delivered;
  - exactly one `Notice::PostProcessingSkipped(reason)` is raised;
  - the job outcome is delivered;
  - no pending recording is created and no retry is offered.

  — req FR-09, FR-25, NFR-06 — FR-003, FR-006, FR-007 — L
- [ ] T016 [P] [US2] Write a red ordering test in `crates/voicen-core/tests/post_process_pipeline.rs`. A fake post-processor delays dictation A, and dictation B is ready earlier; A is delivered first. Repeat with A skipped: A's raw text is delivered first, then B — req FR-23 — FR-009 — L

### Implementation for User Story 2

- [ ] T017 [US2] Implement `classify` and the outer `tokio::time::timeout(timeouts.post_processing, …)` around the whole request in `crates/voicen-core/src/post_process/chat.rs` (research R3, R4). The host comes from the configured base URL, never from the error text (makes T013 and T014 green) — req FR-09, FR-24, NFR-04 — FR-003, FR-004, FR-006 — L
- [ ] T018 [US2] Add `Notice::PostProcessingSkipped(SkipReason)` to 001's notice set in `crates/voicen-core/src/notice.rs`. In the pipeline, raise it on `Skipped` and still deliver the raw text with outcome delivered (makes T015 and T016 green) — req FR-09, FR-25, NFR-06 — FR-006, FR-007, FR-009 — L
- [ ] T019 [P] [US2] Add the five `notice.post_processing_skipped.*` keys (en and ru texts from `contracts/ipc.md`) to 004's message catalog `i18n/en.json`, `i18n/ru.json`. Add vitest tests that each key resolves in both languages with `{host}` and `{status}` substituted — req FR-09, FR-15 — FR-006 — L
- [ ] T020 [US2] Write a Playwright test in `e2e/post-processing.spec.ts`: a mocked notice event `{kind:"post_processing_skipped", reason:"http", params:{status:500}}` shows "Post-processing skipped — HTTP 500" in the overlay for 3 s, and the Russian text when the UI language is `ru`. Make it pass in the overlay component (001) — req FR-09, FR-25, FR-15 — FR-006 — L
- [ ] T021 [US2] Check that the toast and the tray error state for `PostProcessingSkipped` go through 001's existing notifier mapping in `src-tauri/src/` (Windows integration test from 001 extended with the new variant). Add a `verify_exception` for the manual toast and tray check (quickstart §5 step 2) — req FR-25 — FR-006 — W, M

**Checkpoint**: Every failure mode delivers raw text plus one notice, and the 15 s bound is measured.

---

## Phase 5: User Story 3 - Post-processing settings and key are kept safely (Priority: P2)

**Goal**: Defaults, validation, live apply, the separate key slot, no secrets or text in logs, and the privacy note.

**Independent Test**: With a fake credential store and a temp settings file, save a configuration. The key is only in slot `PostProcessing`; the settings file and the log contain no key, prompt or text; the next dictation uses the new values (quickstart §1–2).

### Tests for User Story 3 (red first)

- [ ] T022 [P] [US3] Write red tests in `crates/voicen-core/src/post_process/settings.rs`.
  - **Defaults.** `defaults()` = `enabled: false`, `base_url: ""`, `model: ""`, `prompt: STARTER_PROMPT`.
  - **Validation while `enabled`.** Each of these is refused with its field id (`post_processing.base_url`, `.model`, `.prompt`):
    - "non-empty, parses as an absolute `http`/`https` URL with a host";
    - model "non-empty after trimming";
    - prompt "non-empty after trimming".
  - **Validation while disabled.** Every field may be empty.

  — req FR-13, FR-21 — FR-011 — L
- [ ] T023 [P] [US3] Write a red secrets and logging test in `crates/voicen-core/tests/post_process_secrets.rs`:
  - it runs success plus every failure mode of T013 with a log capture and a fake `CredentialStore` (004's);
  - the mock error bodies echo the transcript and the key;
  - the captured log, the `Debug` output of every outcome and notice, and the saved settings file contain none of: key, prompt, raw text, reply, error body (SC-005);
  - each dictation yields exactly one `post_process outcome=… duration_ms=…` line.
  - a mock `302` redirect from the configured host to another host does not carry the `Authorization` header to that host (the key is sent only to the configured endpoint, research R5);
  - when 006's crash-file writer is in place, a panic injected in a fake post-processor while a snapshot is alive writes a crash file that contains none of the above.

  — req NFR-04, FR-20 — FR-012, FR-014 — L
- [ ] T024 [P] [US3] Write a red test through the real settings loader (P-013) in `crates/voicen-core/tests/post_process_settings_live.rs`. It saves a new model and prompt via 004's settings service; the next dictation's recorded request carries them without a restart. A key missing from the fake credential store means no `Authorization` header — req FR-13, NFR-04 — FR-010, FR-012 — L

### Implementation for User Story 3

- [ ] T025 [US3] Implement `validate()` only in `crates/voicen-core/src/post_process/settings.rs`; `PostProcessingSettings`, `STARTER_PROMPT` (exact text from data-model.md) and `defaults()` are created there by 004's foundational phase (004 T068, decisions #21). Wire `validate()` into 004's validator (makes T022 and T024 green) — req FR-13, FR-21 — FR-010, FR-011 — L
- [x] T026 [US3] Read the key from 004's `CredentialStore` with `KeySlot::PostProcessing` when taking the snapshot, treating a missing entry or read error as no key. Emit the one allowlisted log line per post-processed dictation through 006's logger target (makes T023 green). Done as built by T-076: no separate line; the outcome rides on the dictation line as `pp=`, `pp_reason=`, `pp_ms=` (data-model.md "As built (T-076)") — req NFR-04, FR-20 — FR-012, FR-014 — L
- [ ] T027 [P] [US3] Add the `settings.post_processing.privacy_note` text (en/ru, `contracts/ipc.md`) to 004's Post-processing tab in `src/routes/settings/`. Add a Playwright test in `e2e/post-processing.spec.ts` with mocked `get_settings` that checks:
  - the tab shows "off" and the starter prompt;
  - the privacy note is visible;
  - saving with post-processing on and an empty model or prompt is refused with the field highlighted, while the same save with it off succeeds.

  — req NFR-05, FR-13, FR-15 — FR-011, FR-013 — L

**Checkpoint**: All three stories work independently and the gate is green.

---

## Phase 6: Polish & Cross-Cutting Concerns

- [ ] T028 [P] Update `docs/architecture.md` (Seams: the `PostProcessor` trait; cross-cutting values: `Timeouts.post_processing`, `STARTER_PROMPT`) in the same commit as T006 or T017 (P-014) — req NFR-11 — L
- [ ] T029 Run `make check` and quickstart §1–3 on Linux. On the Windows CI runner, check the build, silent install and smoke — req FR-09, FR-24 — L, W
- [ ] T030 Owner's manual check per quickstart §5:
  - a real endpoint pastes the punctuated text;
  - an unreachable endpoint pastes raw text with the notice for 3 s and the tray error state;
  - the log line has no text, prompt or key.

  Record it as the verify record for the US2 tasks that carry a `verify_exception` — req FR-09, FR-25, NFR-04 — M

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)** depends on 001's shared client, `Timeouts`, pipeline and mock server.
- **Foundational (Phase 2)** depends on Setup, and blocks every story.
- **US1 (Phase 3)** depends on Phase 2. It is the MVP.
- **US2 (Phase 4)** depends on Phase 2. T017 builds on T011 (the same file, `chat.rs`), so US2's implementation follows US1's. US2's tests (T013–T016) can be written alongside US1.
- **US3 (Phase 5)** depends on Phase 2 and on 004's `Settings`, `CredentialStore`, the `post_process::settings` data type (004 T068) and the Post-processing tab; the direction is 003 → 004 (decisions #21). T019 and T027 need 004's catalog and tab.
- **Polish (Phase 6)** comes after the stories it documents.

### Within Each User Story

The red tests come first and must fail, then the implementation, then the checkpoint. Tasks touching the same file run in sequence: `chat.rs` (T011 → T017), `pipeline.rs` (T006 → T018), and the `post_process_pipeline.rs` tests (T006 → T008, T009, T015, T016, which may share the file but add separate test functions).

## Parallel Example: User Story 2

```text
T013 classification tests (post_process_chat.rs)
T014 real 15 s timeout test (post_process_timeout.rs)
T015 skipped → raw + notice pipeline tests
T016 ordering test
T019 catalog keys (root i18n/, decision #13)   ← ui area, independent of core
```

## Implementation Strategy

1. **MVP**: Phases 1–3 (US1). The rewrite works with a healthy endpoint, and post-processing off is unchanged.
2. **Must for release 1**: Phase 4 (US2). Release 1 cannot ship FR-09 without its failure branch and FR-24, so US2 directly follows US1.
3. **Phase 5 (US3)**: done once 004's settings and tab exist.
4. **Phase 6**: docs, gate and the owner's check.

## Requirement Coverage

| Req | Tasks |
|---|---|
| FR-09 | T002, T004, T006–T013, T015, T017–T021, T029, T030 |
| FR-24 | T002, T003, T013, T014, T017, T029 |
| NFR-04 | T004, T005, T017, T023, T024, T026, T030 |
| NFR-05 | T007, T009, T027 |
| NFR-11 | T001, T005, T006, T010, T012, T028 |
| related: FR-11, FR-12, FR-13, FR-15, FR-16, FR-20, FR-21, FR-23, FR-25, NFR-06 | T006, T008, T015, T016, T018–T027 |
