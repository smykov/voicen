# Tasks: Local Transcription

**Input**: Design documents from `/specs/002-local-transcription/` (plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md)

**Prerequisites**: `001-dictation-via-api` provides the `Engine` trait, the OpenAI-compatible client, the shared timeouts module, `FailureReason`, the pipeline, logging and the credential trait; `004-settings-and-first-run` provides the settings window, settings store, key UI and i18n.

**Tests**: Required. The constitution (P-004, P-005, principle III) needs a failing test for every guarantee, failure branches included. Write the test tasks first and see them fail before implementing.

**Conversion note**: Implementation does not use `/speckit-implement`. Each task becomes a teamwright task (`design_ref: specs/002-local-transcription/tasks.md#Tnnn`). Every task names the requirement ids it covers as `{req …}`. "Spec FR-0nn" refers to this feature's spec, and "req" to `docs/requirements.md`.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependency on an incomplete task)
- **[Story]**: US1 download, US2 built-in engine, US3 local server, US4 delete

---

## Phase 1: Setup

- [ ] T001 Create the crate `crates/voicen-whisper` (Cargo.toml with `whisper-rs` CPU-only, no GPU features; empty `src/lib.rs`), add it to the workspace `members` in `Cargo.toml`, and add `sha2` (and `tokio-util` for cancellation if 001 has not) to `crates/voicen-core/Cargo.toml`; check licences for NFR-12 {req FR-07, NFR-12}
- [ ] T002 [P] Create module skeletons `crates/voicen-core/src/local_models/{mod,catalog,store,download,residency}.rs` and `crates/voicen-core/src/engines/{speech_model,builtin,local_server}.rs`, wired in `crates/voicen-core/src/lib.rs` {req FR-07, FR-08, FR-17}
- [ ] T003 [P] Add the mock HTTP server test helper for downloads (configurable truncate / corrupt / stall / drop / status) next to 001's mock server helper in `crates/voicen-core/tests/support/mock_http.rs`; reuse 001's helper if it already exists (P-011) {req FR-08, FR-17}

---

## Phase 2: Foundational (blocks all stories)

- [ ] T004 Write a red test for the catalog table in `crates/voicen-core/src/local_models/catalog.rs`: "exactly 5 entries; unique ids and file names; exactly one recommended; all URLs share the pinned commit"; ids `tiny`, `base`, `small`, `medium-q5_0`, `large-v3-turbo-q5_0`; no `.en` file names {req FR-08}
- [ ] T005 Fill the catalog in `crates/voicen-core/src/local_models/catalog.rs`: pin one `ggerganov/whisper.cpp` commit, and take each file's size and SHA-256 from its LFS metadata at that commit, cross-checked by downloading and hashing it once (research R-6; never guessed) {req FR-08}
- [ ] T006 Extend 001's shared timeouts module `crates/voicen-core/src/timeouts.rs` with `LOCAL_SERVER_TRANSCRIPTION = 60 s`, `DOWNLOAD_NO_DATA = 30 s` and `MODEL_IDLE_UNLOAD = 600 s`, with a test asserting the production values {req FR-24, NFR-03}
- [ ] T007 [P] Define the `SpeechModelLoader` / `SpeechModel` traits and a scriptable fake (load ok/error, transcribe text/error, records the language, counts loads and drops) in `crates/voicen-core/src/engines/speech_model.rs` per contracts/core-traits.md {req FR-07, NFR-11}
- [ ] T008 [P] Extend 001's `FailureReason` with `download_interrupted`, `checksum_mismatch`, `not_enough_disk_space{needed}`, `source_unreachable{host}`, `disk_error`, `http_status{code}`, `download_busy`, `model_in_use`, `delete_failed` and `no_local_model`, each with a message key, in `crates/voicen-core/src/failure.rs` (001's file) {req FR-07, FR-08, FR-28, FR-11}
- [ ] T009 [P] Add `ModelId` and `LocalModelState` with their state transitions from data-model.md in `crates/voicen-core/src/local_models/mod.rs` {req FR-08}

**Checkpoint**: catalog pinned, traits and timeouts in place.

---

## Phase 3: User Story 1 — Download a local model (P1) 🎯 MVP part 1

**Goal**: the user downloads any of the five models with progress. The model becomes selectable only after its SHA-256 matches. Every failure deletes the partial file and offers Retry.

**Independent test**: the downloader against the mock HTTP server with a fake model whose hash is pinned in a test catalog (quickstart §1). The UI with mocked IPC (quickstart §2).

### Tests (write first, must fail)

- [ ] T010 [P] [US1] Red tests for ModelStore in `crates/voicen-core/tests/local_store.rs`: at start, `*.part` files are deleted; a final file of catalog size is `Downloaded`; a wrong-size file is `NotDownloaded`; the store is listed without network access {req FR-08, spec FR-008}
- [ ] T011 [P] [US1] Red tests for the Downloader in `crates/voicen-core/tests/local_download.rs`. Happy path: progress at least 1/s and at most 4/s, and the final file appears only after a hash match. Failure branches: truncated body, altered byte (checksum mismatch), stall beyond the injected no-data timeout, connection drop, HTTP 404/500, connection refused, connect timeout. Each must leave no `.part` or final file and return `Failed{reason}`. Retry after a failure succeeds {req FR-08, FR-24, spec FR-002–FR-004}
- [ ] T012 [P] [US1] Red tests for cancel, one-at-a-time and disk space in `crates/voicen-core/tests/local_download.rs`. Cancel deletes `.part` and gives `NotDownloaded` with no retry. A second start gives `Busy`. Free space below size + 1 % gives `NotEnoughDiskSpace{needed}` and makes no request {req FR-08, spec FR-005–FR-007}
- [ ] T013 [P] [US1] Red Playwright test `e2e/local-models.spec.ts` (mocked IPC): five rows with sizes and the recommended badge on `small`; progress bar driven by `local-model://progress`; a `failed` state shows the reason and Retry; Cancel; other Download buttons disabled during a download {req FR-08, FR-15}

### Implementation

- [ ] T014 [US1] Implement ModelStore (`new`, `cleanup_at_start`, `states`, `path_if_downloaded`) in `crates/voicen-core/src/local_models/store.rs`. The models dir is passed in from the shell's single data-dir resolver (P-010) {req FR-08, FR-28}
- [ ] T015 [US1] Implement the Downloader in `crates/voicen-core/src/local_models/download.rs` (research R-2, R-3): stream to `.part` while hashing incrementally; check size and hash; rename atomically; delete `.part` on every other path; use timeouts `CONNECT` and `DOWNLOAD_NO_DATA` from the shared module; cancellation token; single active slot; `DiskSpace` trait check {req FR-08, FR-24}
- [ ] T016 [US1] Implement the shell side in `src-tauri/src/local_models.rs`: IPC commands `local_models_list`, `local_model_download`, `local_model_cancel_download`; events `local-model://progress` and `local-model://state` (contracts/ipc.md); `DiskSpace` via `GetDiskFreeSpaceExW`; `cleanup_at_start` on app start. Register them in `src-tauri/src/lib.rs` (Windows CI) {req FR-08}
- [ ] T017 [US1] Implement `src/lib/local-models/localModels.ts` (typed invoke/listen wrapper) with a unit test `src/lib/local-models/localModels.test.ts`, and `src/lib/local-models/LocalModelList.svelte` (hosted by 004's engine tab) making T013 pass; reasons shown as message keys {req FR-08, FR-15}
- [ ] T018 [US1] In `.github/workflows/ci.yml`, cache `ggml-tiny.bin` (key = pinned commit + hash). On a cache miss, fetch it with the app's own downloader through a test-only example `crates/voicen-core/examples/fetch_model.rs`, which checks the pinned `tiny` hash against Hugging Face (research R-10) {req FR-08}

**Checkpoint**: models can be downloaded, verified, cancelled and retried. Delivers spec SC-004.

---

## Phase 4: User Story 2 — Dictate offline with the built-in engine (P1) 🎯 MVP part 2

**Goal**: with a downloaded, selected model and the network off, dictation is transcribed on the machine and delivered. The model stays warm and unloads after 10 minutes idle.

**Independent test**: the pipeline in the core with the fake SpeechModel, a fake clock and a request-recording network fake (quickstart §1). Real whisper.cpp on the Windows CI runner (quickstart §3).

### Tests (write first, must fail)

- [ ] T019 [P] [US2] Red tests for ModelResidency (fake clock) in `crates/voicen-core/tests/local_residency.rs`. Covers:
  - Not loaded at app start.
  - `prewarm` on recording start loads once; a transcription during the load waits for the same load.
  - The second dictation is `Warm`; the first is `Cold{load_ms}`.
  - Unloads at exactly 10 min after the latest activity end, and not while a recording or transcription is active.
  - Unloads at once on engine change, on another model selected, and before deleting this model.
  - A load error leaves the residency empty {req NFR-03, NFR-01, spec FR-013–FR-015}
- [ ] T020 [P] [US2] Red tests for BuiltinEngine in `crates/voicen-core/tests/local_engine.rs`. Covers:
  - Fake transcription text reaches the result.
  - The language is passed through: `None` for auto, `Some("ru")` for a fixed language.
  - No model selected, or the selected model not downloaded → `NoLocalModel`: no load attempted, audio becomes pending.
  - Load error and transcribe error → engine failure through the FR-11 path, app continues.
  - Transcriptions run one at a time in order {req FR-07, FR-11, FR-23, NFR-07, spec FR-009, FR-011, FR-012, FR-016}
- [ ] T021 [P] [US2] Red test in `crates/voicen-core/tests/local_no_network.rs`: the pipeline with the built-in engine and post-processing off, over N dictations, makes zero requests on a request-recording network fake. Also a check that `crates/voicen-whisper/Cargo.toml` depends on no HTTP crate {req NFR-05, FR-07, spec FR-010, SC-003}
- [ ] T022 [P] [US2] Red test in `crates/voicen-core/tests/local_engine.rs`: the dictation log record carries `local=cold|warm` and `load_ms`, and contains no transcript text {req NFR-01, FR-20, spec FR-015, FR-024}
- [ ] T023 [P] [US2] Integration test `crates/voicen-whisper/tests/transcribe_wav.rs` (Windows CI, cached `tiny`): the bundled 16 kHz WAV fixture under `crates/voicen-whisper/tests/fixtures/` (MIT/CC0) is transcribed to non-empty text containing the expected word, with a fixed language and with auto-detect. A truncated model file returns a load error and does not abort {req FR-07, NFR-07}

### Implementation

- [ ] T024 [US2] Implement ModelResidency in `crates/voicen-core/src/local_models/residency.rs` (research R-4): injected clock; idle `MODEL_IDLE_UNLOAD`; activity guards; shared in-flight load; `tick`; unload causes logged {req NFR-03, NFR-01}
- [ ] T025 [US2] Implement BuiltinEngine in `crates/voicen-core/src/engines/builtin.rs`: 001's `Engine` trait; i16 → f32 conversion; language mapping; residency acquire guard; `Warmth` into the log record; size and ggml-magic check before load (research R-8) {req FR-07, FR-11, NFR-07, NFR-11}
- [ ] T026 [US2] Implement `SpeechModelLoader` / `SpeechModel` with whisper-rs in `crates/voicen-whisper/src/lib.rs`: CPU threads `min(available_parallelism, 8)`, no `unwrap` in the call path, every error mapped to `EngineError`. Confirm the current whisper-rs API (research R-1) {req FR-07, NFR-07}
- [ ] T027 [US2] Wire the shell in `src-tauri/src/lib.rs`: build BuiltinEngine with the `voicen-whisper` loader; call `prewarm` on recording start when engine = built-in; call `on_engine_or_model_changed` on settings save; schedule `residency.tick()` at the residency's next idle deadline (re-armed on every activity change; a 5 s periodic tick is an acceptable fallback); give the "no local model" notification an "open settings on the Engine tab" action (Windows CI and owner manual check) {req FR-07, NFR-03, FR-25}

**Checkpoint**: offline dictation with a downloaded model works (stage 2 done criterion, spec SC-005). US1 and US2 together are the MVP.

---

## Phase 5: User Story 3 — Use a local OpenAI-compatible server (P2)

**Goal**: dictation through a user-run server at a given URL, with an optional model name and key, a 60 s timeout, and "cannot reach host:port" on failure.

**Independent test**: the core against the mock OpenAI-compatible server (quickstart §1).

### Tests (write first, must fail)

- [ ] T028 [P] [US3] Red tests in `crates/voicen-core/tests/local_server.rs` against the mock server. Covers:
  - The request goes to `<URL>/audio/transcriptions`.
  - Without a key there is no `Authorization` header; with a key, the bearer token is read from the `local-server` credential slot.
  - The model form field is sent only when set.
  - Server stopped → "cannot reach 127.0.0.1:<port>", audio pending.
  - Connection not accepted in time → "cannot reach" (injected connect timeout).
  - No response within the injected request timeout → "timeout".
  - 401/403 → "invalid API key".
  - Production timeouts asserted: 5 s and 60 s from the shared module {req FR-17, FR-24, FR-06, FR-11, NFR-04}
- [ ] T029 [P] [US3] Red test in `crates/voicen-core/tests/local_server.rs` through the real settings loader: a saved local-server URL, model and key reference reach the endpoint used at runtime without a restart (P-013) {req FR-17, FR-13}

### Implementation

- [ ] T030 [US3] Implement `local_server_endpoint` in `crates/voicen-core/src/engines/local_server.rs`: 001's client configured from `LocalServerConfig` with model `Option<String>` ("empty → None → not sent"), key `Option<SecretRef>` in the target `Voicen/local-server`, and timeouts connect 5 s / request 60 s. No new HTTP code (P-011). If 001's client lacks optional model, optional key, a per-endpoint timeout or the port in "cannot reach", extend 001's client in its own module, not here {req FR-17, FR-24, NFR-04}
- [ ] T031 [US3] Register the local-server engine in the engine factory in `src-tauri/src/lib.rs` (the factory comes from 001); the `local-server` key slot uses Credential Manager (Windows CI test) {req FR-17, NFR-04}

**Checkpoint**: local-server dictation works and fails correctly (spec SC-006).

---

## Phase 6: User Story 4 — Delete a downloaded model (P3)

**Goal**: free disk space; deleting the selected model resets the engine to "none".

**Independent test**: the core with a temporary models dir (quickstart §1); the UI with mocked IPC.

### Tests (write first, must fail)

- [ ] T032 [P] [US4] Red tests in `crates/voicen-core/tests/local_store.rs`. Covers:
  - Delete → file gone, state `NotDownloaded`.
  - The selected model → unloaded if loaded, engine `None` persisted.
  - Deleting while a transcription holds it → `ModelInUse`, nothing changes.
  - Removal I/O error → `delete_failed`, still `Downloaded` and usable.
  - A retry of a pending built-in recording after deletion → `NoLocalModel`, audio still pending {req FR-28, FR-07, FR-11, spec FR-021–FR-023}
- [ ] T033 [P] [US4] Red Playwright test in `e2e/local-models.spec.ts`: Delete asks for confirmation; `engineReset: true` shows the engine as "none"; a `model_in_use` error shows its message {req FR-28, FR-15}

### Implementation

- [ ] T034 [US4] Implement `ModelStore::delete` in `crates/voicen-core/src/local_models/store.rs`, in this order: refuse if in use → unload → remove → reset engine and persist {req FR-28}
- [ ] T035 [US4] Add the IPC command `local_model_delete` in `src-tauri/src/local_models.rs` returning `{ engineReset }`, plus a Windows CI test that deletes the file after a load/unload, and Delete with confirmation in `src/lib/local-models/LocalModelList.svelte` {req FR-28}

**Checkpoint**: all four stories work independently.

---

## Phase 7: Polish & cross-cutting

- [ ] T036 [P] Add logging of download, delete, load and unload events with the model id only, and the local-server URL as host:port only, in `crates/voicen-core/src/local_models/*.rs`, with a log-redaction test in `crates/voicen-core/tests/local_logging.rs`: no text, audio, key or URL userinfo {req FR-20, NFR-04, NFR-05, spec FR-024}
- [ ] T037 [P] Add en/ru texts for every message key this feature adds in 004's catalog files `i18n/en.json`, `i18n/ru.json` (coordinate with 004), with a test that both locales have every key {req FR-15, spec FR-025}
- [ ] T038 [P] Update `docs/architecture.md` (the `voicen-whisper` crate, the model residency, the models dir, the local-server key slot) and `docs/decisions/core.md` (the in-process whisper and native-abort risk, research R-8) in the same commit as the code they describe {req NFR-11, NFR-07}
- [ ] T039 [P] Add whisper.cpp (MIT), whisper-rs, the ggml model licence (MIT, from `ggerganov/whisper.cpp`) and `sha2` to the third-party licence list owned by 006 (its file, e.g. the NFR-12 licence list in the repository root, as 006 defines it) {req NFR-12}
- [ ] T040 Run `specs/002-local-transcription/quickstart.md` §1–§3 and record evidence; prepare the owner's manual checklist from quickstart §4: offline dictation, NFR-01 warm benchmark with `small`, RAM drop after 10 min, speaches, delete {req FR-07, FR-08, FR-17, FR-28, NFR-01, NFR-03}

---

## Dependencies & execution order

- **Setup (T001–T003)** → **Foundational (T004–T009)** → stories.
- **US1 (T010–T018)** depends on the foundational tasks only.
- **US2 (T019–T027)** depends on the foundational tasks and on `ModelStore::path_if_downloaded` (T014). It can be tested with a hand-placed model file, so US1's downloader is not required.
- **US3 (T028–T031)** depends on T006 and 001's client only. It is independent of US1 and US2 and can run in parallel with them.
- **US4 (T032–T035)** depends on T014 (store) and T024 (residency).
- **Polish (T036–T040)** comes after the stories it touches. T040 comes last.
- Within each story, the tests come before the implementation, and the core comes before the shell and the UI.
- External: 001 (engine trait, client, timeouts, FailureReason, pipeline, logging, credentials) is needed before T006–T008, T020, T025, T028 and T030. 004 (settings window host, settings store, key UI, i18n) is needed before T017, T027, T029, T031 and T037.

## Parallel examples

- **US1**: T010, T011, T012 and T013 together (different files); then T014 → T015 → T016 and T017 in parallel.
- **US2**: T019, T020, T021, T022 and T023 together; then T024 → T025, with T026 in parallel (different crate), then T027.
- **US3** alongside US1 and US2: T028 and T029 together, then T030 → T031.

## Implementation strategy

1. **MVP** = Setup + Foundational + US1 + US2. This meets stage 2's "offline dictation with a downloaded `small` model". Validate with quickstart §1–§3 and owner steps 1–4.
2. **Increment 2**: US3, the local server (completes the stage 2 done criterion "local server works").
3. **Increment 3**: US4, deletion (Should).
4. Polish runs alongside, each item in the same task as the code it documents.

## Requirement coverage

| Requirement | Tasks |
|---|---|
| FR-07 | T001, T002, T007, T008, T020, T021, T023, T025, T026, T027, T032, T040 |
| FR-08 | T004, T005, T009, T010–T018, T040 |
| FR-17 | T003, T028–T031, T040 |
| FR-24 (local server 60 s, connect 5 s) | T006, T011, T015, T028, T030 |
| FR-28 (deletion part) | T008, T014, T032–T035, T040 |
| NFR-01 (local, warm) | T019, T022, T024, T040 |
| NFR-03 | T006, T019, T024, T027, T040 |
| NFR-05 | T021, T036 |
