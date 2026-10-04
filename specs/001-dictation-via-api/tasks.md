---
description: "Task list for feature 001 — dictation via an OpenAI-compatible API"
---

# Tasks: Dictation via an OpenAI-compatible API

**Input**: Design documents from `specs/001-dictation-via-api/` (plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md)

**Prerequisites**: plan.md, spec.md

**Tests**: Required. The constitution (I: P-004/P-005; III: every Must failure branch has a test) makes red tests mandatory. In the teamwright flow the test writer writes them before the developer, so every implementation task below has a red-test task before it.

**Organization**: Tasks are grouped by user story (spec.md US1–US6). Each task names the requirement ids it covers as `[req …]` (`docs/requirements.md` v3) and, where useful, the spec FR. When tasks are converted to teamwright tasks, `design_ref` points at this file and the task id.

**Where it runs**: **[core]** = `cargo test -p voicen-core` on the Linux host (fakes, wiremock); **[ui]** = vitest/Playwright with mocked IPC; **[win-ci]** = Windows CI runner (`cargo test --workspace`); **[owner]** = owner's manual check on Windows (`verify_exception`).

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (US1–US6)

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Dependencies, fixtures, toolchain — owner consent first (plan "Owner decisions needed").

- [ ] T001 Obtain the owner's consent for the dependency list in research.md R-17, and record it in `docs/decisions.md` (Was → Decided) [req NFR-12]
- [ ] T002 Add the core dependencies to `crates/voicen-core/Cargo.toml`: `reqwest` (default-features off; `blocking`, `multipart`, `rustls`, `system-proxy`; T-040, decision #44), `serde_json`, `thiserror`, `rubato`; optional `whisper-rs` behind feature `silero`; feature `test-audio`; dev-deps `wiremock`, `tokio` (rt, macros) [req NFR-12, FR-06, FR-12]
- [ ] T003 [P] Add the shell dependencies to `src-tauri/Cargo.toml`: `cpal`, `windows` (features: Win32 UI input/keyboard/WindowsAndMessaging, DataExchange, Memory, Security, RemoteDesktop, Power, Credentials, UI_Notifications, Data_Xml_Dom), `tauri-plugin-single-instance`; enable `voicen-core` features `silero` and (dev) `test-audio` [req NFR-12]
- [ ] T004 [P] Commit the speech-gate fixtures (3 s speech, 3 s silence, 1 s cough, keyboard noise; 16 kHz mono WAV, MIT-compatible licence noted in `crates/voicen-core/tests/fixtures/LICENSES.md`) in `crates/voicen-core/tests/fixtures/` [req FR-12, NFR-12]
- [ ] T005 [P] If the owner approves (research R-6), add `cmake` and `clang` to `docker/rust.Dockerfile` and enable `--features silero` in `Makefile` `check-core`. Otherwise the Silero fixture tests run only on [win-ci] [req FR-12]

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Single-source values, platform traits and fakes that every story uses.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [ ] T006 [P] Red test + implement `Timeouts` (connect 5 s, api_transcription 30 s, local_server 60 s, post_processing 15 s; one constructor; test-overridable) in `crates/voicen-core/src/timeouts.rs` [core] [req FR-24]
- [ ] T007 [P] Add every key and text of contracts/messages.md to the one catalog `i18n/en.json`, `i18n/ru.json` (created by teamwright T-005); `MessageKey` maps to `voicen_core::i18n::MessageId` (declared with `messages!`) and `MessageParams` to its `args`. No own catalog files and no own completeness test: parity is checked only by T-005's tests [core] [req FR-15 (spec FR-034)]
- [ ] T008 [P] Red test + implement `DictationEvent` (only the fields of data-model.md "DictationEvent") and the `PipelineObserver` trait, plus a recording fake, in `crates/voicen-core/src/events.rs` (teamwright T-001: `DictationEvent` is `Copy`) [core] [req FR-20, NFR-04 (spec FR-033)]
- [ ] T009 [P] Red test + implement `AudioBuffer` (16 kHz mono i16), mix-down + resample with `rubato` (48 kHz stereo → 16 kHz mono length ±1 sample per 10 ms), and WAV encoding (RIFF PCM 16-bit, 16 000 Hz, mono; 10 min ≈ 19.2 MB) in `crates/voicen-core/src/audio/{mod,resample,wav}.rs` [core] [req FR-06, decisions #1]
- [ ] T010 Define the platform traits of contracts/core-traits.md (`AudioSource`, `Clipboard`, `Paster`, `Notifier`, `Indicator`, `TempAudioStore`; no `SettingsSource` (decision #47 (1)); no `Clock` port, instants come from the caller (teamwright T-042); `CredentialStore` is 004's trait in `voicen_core::secrets`, not redefined here — decisions #21) in `crates/voicen-core/src/platform.rs`, with public fakes next to the traits (feature `test-fakes`, decision #23 N4). Teamwright T-001 built `Clipboard`, `Paster`, `TempAudioStore`, `StartWindow`, `PendingId` and their fakes; `AudioSource` comes with T-006, `Notifier`/`Indicator` with T-006/T-007 [core] [req NFR-11]
- [ ] T011 ~~replaced~~ — decision #47 (1): no `DictationSettings` projection and no `SettingsSource`; a job carries the `Arc<Settings>` snapshot taken at press in `pipeline::PressContext` (teamwright T-001). The P-013 test (a saved value reaches the job) is `saved_settings_reach_the_job` in `crates/voicen-core/tests/api_pipeline.rs`; the defaults are 004's `settings::defaults` [core] [req FR-21 defaults, FR-13 (read only)]
- [ ] T012 [P] Red test + implement the `IndicatorState` machine: TrayState priority "HotkeyError > Recording > Error > Idle"; OverlayState `Recording` / `Processing` (≥ 1 job unreleased and no recording) / `Message` 3 s / `Hidden` — in `crates/voicen-core/src/recording/indicator.rs`, owned by `RecordingController` (teamwright T-042) [core] [req FR-04, FR-25 (spec FR-008, FR-028)]

**Checkpoint**: Foundation ready — user story implementation can begin.

---

## Phase 3: User Story 1 — Dictate into the field I am typing in (Priority: P1) 🎯 MVP

**Goal**: Hold the hotkey, speak, release → the text is pasted into the start window and is in the clipboard (excluded from history).

**Independent Test**: The core pipeline with a fake audio source, wiremock and fake sinks delivers the mock transcript to the recorded start window. [win-ci]: a synthetic hotkey into a test window with the injected WAV. [owner]: quickstart §3 step 1.

### Tests for User Story 1 (write first, must fail)

- [ ] T013 [P] [US1] Red tests: request shape per contracts/openai-transcription.md — path `<base>/audio/transcriptions` with and without a trailing `/`; multipart `file` (audio/wav), `model`, `response_format=json`; no `language` part for auto; no `Authorization` header without a key; 200 `{"text":"hello"}` → `Ok("hello")`; 200 empty text → `Ok("")`. In `crates/voicen-core/tests/openai_client.rs` [core] [req FR-06]
- [ ] T014 [P] [US1] Red tests for hold mode in `RecordingController`: a 4 s hold → one job with ~4 s audio; a hold < 0.3 s → discarded, no job, no notification; auto-repeat presses ignored; releasing any key of the combination ends it; engine = none → no recording, `notice.choose_engine` and open settings: decided by `settings::gate::dictation_gate` before `press`, wired by teamwright T-006. In `crates/voicen-core/src/recording/mod.rs` (unit tests, teamwright T-042) [core] [req FR-02, FR-21 failure branch]
- [ ] T015 [P] [US1] Red tests for the speech gate: silence, cough and keyboard fixtures → `NoSpeech` (no engine call, no clipboard write, `notice.no_speech`); the speech fixture → engine called; primary detector fails to load → energy detector used and one `Warning{vad_fallback}`. In `crates/voicen-core/src/vad/mod.rs` tests, plus `crates/voicen-core/tests/vad_fixtures.rs` (feature `silero`) [core, win-ci] [req FR-12]
- [ ] T016 [P] [US1] Red tests for the `DeliveryDecision` table of data-model.md: the clipboard is always written first; auto-paste off → `CopiedOnly` + `notice.copied`; modifiers still held after 1 s → copy-only + `notice.copied_paste_manually`; start window not in front or closed → same; elevated → same; otherwise `Pasted`. In `crates/voicen-core/src/delivery.rs` (teamwright T-001: the copy-manual rows are `CopyManual`, no window / elevated are checked before the wait, and a `send_ctrl_v` error is `CopyManual`, decision #47 (3)) [core] [req FR-10]
- [ ] T017 [US1] Red end-to-end core test: fake source (speech fixture) + wiremock 200 → the clipboard fake gets the text, the paster fake gets Ctrl+V for the recorded start window, the indicator goes Recording → Processing → Hidden, and the observer gets `RecordingStarted`/`JobFinished`/`Delivered`. In `crates/voicen-core/tests/pipeline.rs` [core] (teamwright T-001, core part: `crates/voicen-core/tests/api_pipeline.rs` drives `Pipeline::run_job` with recordings from the real `RecordingController`; the fake source, `RecordingStarted` and the shell wiring are T-006) [req FR-02, FR-04, FR-06, FR-10, FR-12]

### Implementation for User Story 1

- [ ] T018 [P] [US1] Implement the `Engine` trait, `TranscribeRequest` and `OpenAiCompatibleEngine` (the key is 004's `Secret`, `Debug`/`Display` print `***`; no `SecretString`, decisions #21) (blocking reqwest, connect timeout + whole-request timeout from `Timeouts`, body cap 1 MiB) in `crates/voicen-core/src/engine/{mod,openai}.rs`; make T013 green [core] [req FR-06, FR-24, NFR-04, NFR-11]
- [ ] T019 [P] [US1] Implement `RecordingController` hold mode ("min hold 0.3 s (hold mode only)") in `crates/voicen-core/src/recording/mod.rs` (teamwright T-042); make T014 green [core] [req FR-02]
- [ ] T020 [P] [US1] Implement the `SpeechDetector` trait, `EnergyDetector` (research R-5 constants, pinned by fixtures), `SpeechGate` with fallback, and `SileroDetector` (feature `silero`, bundled ggml model path from the shell) in `crates/voicen-core/src/vad/{mod,energy,silero}.rs`; make T015 green [core, win-ci] [req FR-12]
- [ ] T021 [P] [US1] Implement the `PostProcessor` trait and `PassThrough` in `crates/voicen-core/src/post_process/mod.rs` (teamwright T-001), called between engine and delivery [core] [req FR-09 (integration point; spec FR-021)]
- [ ] T022 [P] [US1] Implement `DeliveryDecision` in `crates/voicen-core/src/delivery.rs`; make T016 green [core] [req FR-10]
- [ ] T023 [US1] Implement the `Pipeline` orchestrator for a single job: hotkey → recording → gate → engine (via core `engine_for` per job; contracts/core-traits.md) → post-process → delivery → indicator/observer, in `crates/voicen-core/src/pipeline.rs`; make T017 green (teamwright T-001: `Pipeline::run_job(FinishedRecording<PressContext>) -> JobReport`; hotkey and recording stay in the controller and T-006) [core] [req FR-02, FR-04, FR-06, FR-10, FR-12, NFR-11]
- [ ] T024 [P] [US1] Implement `CpalSource` (stream opened on start, dropped on stop; push frames to the core) and `WavFileSource` (feature `test-audio`, `VOICEN_TEST_AUDIO`) in `src-tauri/src/win/capture.rs`. Windows test in `src-tauri/tests/capture.rs`: no capture stream exists before the first press or after the handle is dropped (the microphone is open only while recording) [win-ci] [req FR-02, NFR-02]
- [ ] T025 [P] [US1] Implement the clipboard write with `CF_UNICODETEXT` + `ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory`=0, `CanUploadToCloudClipboard`=0 and an open retry of 10 × 20 ms in `src-tauri/src/win/clipboard.rs`; Windows test reads the formats back in `src-tauri/tests/clipboard.rs` [win-ci] [req FR-10]
- [ ] T026 [P] [US1] Implement the paster (root-owner start window, integrity-level comparison, modifier wait ≤ 1 s, one-batch SendInput Ctrl+V) in `src-tauri/src/win/paste.rs`; Windows test pastes into a test-window edit control and refuses a window that is not in front, in `src-tauri/tests/paste.rs` [win-ci] [req FR-10, NFR-08]
- [ ] T027 [US1] Implement the hotkey thread: hidden top-level window, `RegisterHotKey` + `MOD_NOREPEAT`, hold-release polling 10 ms, menu-mask key `0xE8` on press, start-window capture at press, in `src-tauri/src/win/hotkey.rs`; Windows test: a synthetic Ctrl+Alt+Space into a test window produces no `WM_CHAR` and no menu activation, in `src-tauri/tests/hotkey.rs` [win-ci] [req FR-02, FR-10 (spec FR-014)]
- [ ] T028 ~~removed~~ — see decisions #21 (one Credential Manager implementation: 004 T016, `src-tauri/src/credentials.rs`)
- [ ] T029 [US1] Wire the app in `src-tauri/src/lib.rs` and `src-tauri/src/win/tray.rs`: no window at start; tray with idle/recording icons; menu Settings, Open logs folder, Exit (History hidden until 005, Retry hidden without pending); `tauri-plugin-single-instance` opens settings; remove the default main window from `src-tauri/tauri.conf.json`. Red Windows tests first in `src-tauri/tests/startup.rs`: after launch the foreground window is unchanged and no Voicen window is visible; a second launch exits without a second process and signals the first (settings-open request recorded) [win-ci, owner] [req FR-01]
- [ ] T030 [P] [US1] Red UI tests: the overlay renders recording (m:ss), processing, message (3 s, en/ru text from the catalog) and hidden from `overlay://state` payloads and from the `overlay_ready` reply (contracts/ipc.md), in `src/lib/overlay/state.test.ts` and `e2e/overlay.spec.ts` [ui] [req FR-04, FR-25]
- [ ] T031 [US1] Implement the overlay route and store in `src/routes/overlay/+page.svelte` and `src/lib/overlay/state.ts`; make T030 green [ui] [req FR-04]
- [ ] T032 [US1] Implement the overlay window (created on demand, `WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW|WS_EX_TRANSPARENT`, `SW_SHOWNOACTIVATE`, click-through, bottom-centre of the start window's monitor, destroyed on hidden) and the `overlay_ready` command in `src-tauri/src/win/overlay.rs`; Windows test: the foreground window is unchanged after show, in `src-tauri/tests/overlay.rs` [win-ci, owner] [req FR-04, NFR-03]
- [ ] T033 [US1] Windows end-to-end test: installed-shell code with the injected WAV + a local mock endpoint + the test window → the text in the edit control and the clipboard, in `src-tauri/tests/dictation_e2e.rs` [win-ci] [req FR-02, FR-06, FR-10, FR-12]

**Checkpoint**: US1 works on [win-ci] and passes quickstart §3 step 1 [owner].

---

## Phase 4: User Story 2 — Failures are reported and the dictation can be retried (Priority: P1)

**Goal**: Every failure pastes nothing, reports through the toast, the overlay (3 s) and the tray error, and keeps the audio for Retry.

**Independent Test**: The core pipeline with wiremock 401 then 200: failure reported, pending kept; `retry()` delivers and clears it. [owner]: quickstart §3 steps 3–4.

### Tests for User Story 2 (write first, must fail)

- [ ] T034 [P] [US2] Red tests for every row of the contracts/openai-transcription.md response table:
  - 401/403 → `InvalidApiKey`
  - DNS failure → `NetworkUnavailable`
  - closed localhost port → `CannotReach{host}`
  - delay > timeout (ms-scale `Timeouts`) → `Timeout`
  - 400/413/429/500/503 → `ServerError{status}`
  - non-JSON, missing/non-string `text`, > 1 MiB, invalid UTF-8, reset mid-body → `UnexpectedResponse` (and no panic)
  - pure mapping tests over `TransportError` for connect timeout and unreachable

  In `crates/voicen-core/tests/openai_client.rs` and `crates/voicen-core/src/failure.rs` [core] [req FR-06, FR-11, FR-24, NFR-07]
- [ ] T035 [P] [US2] Red tests for `PendingRecording`:
  - "At most one exists"; a newer failure replaces it (old file deleted); a later success does not remove it
  - retry uses the settings and key current at the Retry click
  - retry success → delivered to the original start window (pasted only if in front, otherwise `notice.copied_paste_manually`), then deleted
  - retry failure → reason re-reported, audio kept
  - retry → empty text → deleted + `notice.no_speech`
  - stale or absent id → no-op
  - clipboard write failure → `ClipboardUnavailable` + pending

  Teamwright T-001 covers the non-retry rows (one slot, replacement deletes the old audio, Text/NoSpeech leave it, clipboard failure) in `crates/voicen-core/tests/api_pipeline.rs`; the retry rows are T-007.
  In `crates/voicen-core/src/pending.rs` and `crates/voicen-core/tests/pipeline.rs` [core] [req FR-11, NFR-06, FR-10]
- [ ] T036 [P] [US2] Red tests: each failure → `Notifier` call with the message key and a retry id for retryable reasons, overlay `Message` for 3 s, tray `Error`; `Error` cleared by the next successful delivery or `on_tray_menu_opened`; a toast failure still shows overlay + tray. In `crates/voicen-core/tests/pipeline.rs` [core] [req FR-25, FR-11]
- [ ] T037 [P] [US2] Red tests for `TempAudioStore` on the file system: the pending file is under `<data>/tmp/audio/`; `delete_all` at start removes stale files; shutdown removes everything; in-flight audio never touches disk. In `crates/voicen-core/src/pending.rs` (fs impl with a temp dir) [core] [req NFR-06 (spec FR-032)]

### Implementation for User Story 2

- [ ] T038 [US2] Implement `FailureReason` and the classification (`TransportError` → reason; status → reason), with module-level `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]` in `engine/`, `failure.rs` and `vad/`, in `crates/voicen-core/src/failure.rs`; make T034 green [core] [req FR-06, FR-11, FR-24, NFR-07]
- [ ] T039 [US2] Implement `PendingRecording` and the fs `TempAudioStore` (start cleanup, exit cleanup) in `crates/voicen-core/src/pending.rs`; make T035 and T037 green [core] [req FR-11, NFR-06]
- [ ] T040 [US2] Extend `Pipeline` with failure reporting (notifier, indicator, pending) and `retry(Option<PendingId>)`, in `crates/voicen-core/src/pipeline.rs`; make T036 green [core] [req FR-11, FR-25]
- [ ] T041 [P] [US2] Implement WinRT toasts (AUMID `dev.voicen.app`, a Retry action `retry:<id>`, an `Activated` handler → `Pipeline::retry`; failure to show is ignored) in `src-tauri/src/win/toast.rs` [win-ci (toast XML built and the handler routes a synthetic activation), owner] [req FR-11, FR-25]
- [ ] T042 [US2] Tray error icon and tooltip, "Retry last failed dictation" item visible only with a pending recording, menu-open → `on_tray_menu_opened`, in `src-tauri/src/win/tray.rs` [win-ci, owner] [req FR-11, FR-25 (spec FR-001, FR-028)]
- [ ] T043 [US2] Windows test: after a forced failure and app exit, `%LOCALAPPDATA%\Voicen\tmp\audio` is empty; a stale file placed before start is deleted, in `src-tauri/tests/temp_audio.rs` [win-ci] [req NFR-06]

**Checkpoint**: US1 + US2 = the release-1 core value; SC-003 and SC-005 are measurable.

---

## Phase 5: User Story 3 — Toggle mode and cancel (Priority: P2)

**Goal**: Toggle mode, the 10-minute cap in both modes, and Esc to discard.

**Independent Test**: Core state-machine tests with a fake clock; [win-ci] Esc registration test; [owner] quickstart §3 step 2.

- [ ] T044 [P] [US3] Red tests in `crates/voicen-core/src/recording/mod.rs`:
  - toggle press → press = one job
  - "max 10 min (both modes)" → job + `notice.max_length`
  - Esc → `Cancelled`, nothing sent, indicator off
  - Esc during a hold, then release → no-op
  - Esc claim failure → recording continues + `Warning{esc_unavailable}`

  [core] [req FR-03, FR-22]
- [ ] T045 [US3] Implement toggle mode, the max-length timer and Esc handling in `crates/voicen-core/src/recording/mod.rs`; make T044 green [core] [req FR-03, FR-22]
- [ ] T046 [US3] Register Esc (`RegisterHotKey(VK_ESCAPE)`) only while recording, in `src-tauri/src/win/hotkey.rs`; Windows test: Esc while recording discards; Esc while idle reaches the test window, in `src-tauri/tests/hotkey.rs` [win-ci, owner] [req FR-22]

---

## Phase 6: User Story 4 — The hotkey is always working or the user is told (Priority: P2)

**Goal**: Registration conflicts are reported. The hotkey survives sleep/resume, lock/unlock and an Explorer restart.

**Independent Test**: Core registration state-machine tests; [win-ci] a helper process holds the combo; simulated power/session/TaskbarCreated messages; [owner] quickstart §3 step 6.

- [ ] T047 [P] [US4] Red tests for `HotkeyRegistration`:
  - start failure → tray `HotkeyError` + `notice.hotkey_failed_startup` + open settings on the hotkey field
  - change to a taken combo → `failure.hotkey_unavailable`, the old one stays `Active`
  - re-register failure after resume → `Failed`
  - `HotkeyError` cleared only by a successful registration, not by opening the menu

  In `crates/voicen-core/src/hotkey.rs` [core] [req FR-05, FR-25, FR-26]
- [ ] T048 [US4] Implement `HotkeyRegistration` and its `Pipeline` wiring (`on_hotkey_registration`, open-settings request) in `crates/voicen-core/src/hotkey.rs` and `pipeline.rs`; make T047 green [core] [req FR-05, FR-26]
- [ ] T049 [US4] Implement the `set_hotkey` IPC command (register the new binding before unregistering the old; contracts/ipc.md) in `src-tauri/src/lib.rs` + `src-tauri/src/win/hotkey.rs`, and implement 004's `HotkeyRegistrar` (`prepare`/`commit`/`abort`, defined by 004 T067) there so a refused save keeps the old hotkey (decisions #21) [win-ci] [req FR-05 (spec FR-012)]
- [ ] T050 [US4] Handle `WM_POWERBROADCAST` (suspend → `on_suspend`; resume → re-register), `WM_WTSSESSION_CHANGE` (unlock → re-register) and `TaskbarCreated` (re-add the tray icon) in `src-tauri/src/win/hotkey.rs` and `tray.rs`. Windows tests: a helper process pre-registers Ctrl+Alt+Space → start-up error state; simulated resume and TaskbarCreated messages → re-registration and the tray present. In `src-tauri/tests/hotkey_lifecycle.rs` [win-ci, owner (SC-006)] [req FR-05, FR-26, FR-25]

---

## Phase 7: User Story 5 — Dictate again while the previous one is still processing (Priority: P3)

**Goal**: Overlapping dictations are delivered in recording order, each to its own window.

**Independent Test**: Core ordering tests with slow/fast wiremock responses (SC-007).

- [ ] T051 [P] [US5] Red tests for `DeliveryQueue` and the concurrent pipeline:
  - A slow, B fast → delivered A then B, each to its own start window
  - A fails → B still delivered; A becomes pending
  - a long A still in the speech gate is not overtaken by a short B (seq at stop)
  - A no-speech → leaves the order without blocking
  - a retry clicked while B is in flight gets the next seq ("A retry gets the next seq at the Retry click")
  - pastes never interleave

  In `crates/voicen-core/src/queue.rs` and `crates/voicen-core/tests/pipeline.rs` [core] [req FR-23]
- [ ] T052 [US5] Implement `DeliveryQueue` ("an outcome with seq n is released only after every seq < n is released") and the pipeline concurrency (a worker thread per job, one delivery thread) in `crates/voicen-core/src/queue.rs` and `pipeline.rs`; make T051 green [core] [req FR-23]

---

## Phase 8: User Story 6 — Microphone changes (Priority: P3)

**Goal**: A missing selected microphone falls back to the default with one notice; device loss mid-recording processes what was captured; no device or denied access is reported.

**Independent Test**: Core `MicrophoneChoice` and device-loss tests with the fake source; [win-ci] the injected source signals loss; [owner] quickstart §3 step 5.

- [ ] T053 [P] [US6] Red tests:
  - selected missing → default used + `notice.mic_fallback{device}` once; same state again → no notice; a different fallback device → notice; selected returns → used, no notice
  - device lost at 5 s → a job with 5 s audio
  - no device / access denied / busy → no recording state, `failure.microphone_unavailable{reason}`, no pending

  In `crates/voicen-core/src/microphone.rs` and `crates/voicen-core/tests/pipeline.rs` [core] [req FR-27, FR-04]
- [ ] T054 [US6] Implement `MicrophoneChoice` and the pipeline integration in `crates/voicen-core/src/microphone.rs` and `pipeline.rs`; make T053 green [core] [req FR-27, FR-04]
- [ ] T055 [US6] Map cpal errors (`DeviceNotAvailable` → device lost; access denied → `AccessDenied`; none → `NoDevice`) and device IDs (research R-3) in `src-tauri/src/win/capture.rs`; Windows test with `WavFileSource` signalling loss, in `src-tauri/tests/capture.rs` [win-ci, owner] [req FR-27, FR-04]

---

## Phase 9: Polish & Cross-Cutting Concerns

- [ ] T056 [P] Redaction test across every path: the key `sk-test-SECRET` and the transcript `TRANSCRIPT-MARKER` placed in the settings, the credential fake and wiremock responses (including error bodies) never appear in any `DictationEvent`, `FailureReason` Display/Debug, or notifier params except the delivered text. In `crates/voicen-core/tests/redaction.rs` [core] [req NFR-04, FR-20]
- [ ] T057 [P] Timing events: hotkey → first frame, stop → text, text → paste emitted per dictation; Windows test p95 hotkey → first frame ≤ 200 ms over 20 runs with the injected source, in `src-tauri/tests/timing.rs` [core, win-ci] [req NFR-02, NFR-01, FR-20]
- [ ] T058 [P] Update `docs/architecture.md` (seams: `Engine`, `SpeechDetector`, `PostProcessor`, platform traits, `overlay://state`) and add `docs/decisions/core.md` invariants (ordering seq at stop; at most one pending; engine code never panics under `panic = "abort"`), in the same commits as the code [req NFR-11] (P-014)
- [ ] T059 Owner manual check per quickstart.md §3 on Windows 11 (NFR-08 app list, sleep/lock/Explorer 3/3, Focus Assist, USB unplug, OpenAI + Groq benchmark p90 ≤ 3 s), recorded as the `verify_exception` evidence of the shell tasks [owner] [req NFR-01, NFR-08, FR-26, FR-25, FR-27]
- [ ] T060 Run the quickstart.md §1–§2 validation (`make check` green; Windows CI green) [core, ui, win-ci]

---

## Dependencies & Execution Order

### Phase Dependencies

- Setup (T001–T005): T001 blocks T002/T003. T004 and T005 are independent.
- Foundational (T006–T012): after T002. Blocks all stories. The dependency direction is 001 → 004's traits (`CredentialStore`, `Settings`/`SettingsService`, `HotkeyRegistrar`, 004 T004–T009, T067): 001 reads keys and settings through them and implements `HotkeyRegistrar`; 004 does not wait for 001 (decisions #21).
- US1 (Phase 3): after Foundational. The MVP.
- US2 (Phase 4): after US1's `Pipeline` (T023) and engine (T018).
- US3 and US4: after US1 (`recording/`, `hotkey.rs` shell thread T027); independent of each other and of US2.
- US5: after US2 (failure and pending outcomes take part in the order).
- US6: after US1 (capture T024).
- Polish: after the stories it measures; T058 travels with each code task.

### User Story Dependencies

- US1 → US2 → US5; US1 → US3; US1 → US4; US1 → US6.

### Within Each User Story

- Red tests before implementation (teamwright: test writer, then developer).
- Core before shell; the shell's Windows tests run only on [win-ci].

### Parallel Opportunities

- T003, T004, T005 in parallel after T001.
- T006–T009, T011, T012 in parallel (separate files); T010 next to them.
- US1 tests T013–T016 in parallel; implementation T018–T022 in parallel; shell adapters T024–T026 in parallel.
- After US1: US3, US4 and US6 in parallel with US2.

## Parallel Example: User Story 1

```text
T013 openai_client request-shape tests   | T014 recording hold tests
T015 speech gate tests                   | T016 delivery decision tests
then
T018 engine | T019 recording | T020 vad | T021 postprocess | T022 delivery
T024 capture | T025 clipboard | T026 paste   (shell, Windows CI)
```

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Phases 1–2.
2. Phase 3 (US1). Validate on [win-ci] and quickstart §3 step 1.
3. Then US2 immediately: US1 + US2 together satisfy constitution III for release 1.

### Incremental Delivery

US1 → US2 (P1 complete) → US3 + US4 (P2) → US5 + US6 (P3) → Polish (SC-001/SC-004/SC-006 by the owner).

## Requirement coverage

| req | Tasks |
|---|---|
| FR-01 | T029 |
| FR-02 | T014, T017, T019, T023, T024, T027, T033 |
| FR-03 | T044, T045 |
| FR-04 | T012, T017, T030–T032, T053–T055 |
| FR-05 | T047–T050 |
| FR-06 | T009, T013, T018, T034, T038 |
| FR-10 | T016, T022, T025–T027, T033, T035 |
| FR-11 | T034–T036, T038–T042 |
| FR-12 | T004, T005, T015, T020 |
| FR-22 | T044–T046 |
| FR-23 | T051, T052 |
| FR-24 (connect, API) | T006, T018, T034, T038 |
| FR-25 | T012, T030, T036, T040–T042, T047, T050 |
| FR-26 | T047, T048, T050, T059 |
| FR-27 | T053–T055, T059 |
| NFR-01 (API) | T057, T059 |
| NFR-02 | T024, T057 |
| NFR-06 | T035, T037, T039, T043 |
| NFR-07 (engine) | T034, T038 |
| NFR-08 | T026, T059 |
| Integration: FR-09 | T021 |
| Integration: FR-20 | T008, T056, T057 |
| Supporting: FR-15, FR-21, NFR-04, NFR-11, NFR-12 | T007; T011, T014; T018, 004 T016, T056; T010, T023; T001–T004 |

## Notes

- [P] tasks touch different files and have no unfinished dependencies.
- A task that touches tray, hotkey, toasts or paste carries a `verify_exception` (requirements §9 "UI verification") with the owner's manual check plus its [win-ci] test as evidence.
- Do not use `/speckit-implement`; convert each task to `docs/tasks/T-NNN.md` with `design_ref: specs/001-dictation-via-api/tasks.md#TNNN`.
