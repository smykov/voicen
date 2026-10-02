---

description: "Task list for 004 Settings and First Run"
---

# Tasks: Settings and First Run

**Input**: Design documents from `/specs/004-settings-and-first-run/`

**Prerequisites**: [plan.md](./plan.md), [spec.md](./spec.md), [research.md](./research.md), [data-model.md](./data-model.md), [contracts/](./contracts/), [quickstart.md](./quickstart.md)

**Tests**: Required. The constitution (P-004, P-005) requires a failing test for every guarantee, failure branches included; each story lists its red tests before implementation.

**Implementation**: not through `/speckit-implement`. Each task becomes a teamwright task (`docs/tasks/T-NNN.md`) with `design_ref` to this file; every task names the requirement ids it covers (`req …`) and the spec FR ids (`spec …`).

**Areas**: `core` = `crates/voicen-core` (Linux gate, Docker); `shell` = `src-tauri` (Windows CI only, decisions #5); `ui` = `src/`, `e2e/`.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: can run in parallel (different files, no dependency on an incomplete task)
- **[Story]**: US1–US6 from spec.md

---

## Phase 1: Setup

- [ ] T001 Add core dependencies `serde_json`, `url` and `zeroize` (both consented, decisions #17; no `tokio`, decisions #22) to `crates/voicen-core/Cargo.toml`; confirm MIT-compatible licences (req NFR-12) — area core
- [ ] T002 [P] Add `windows` crate features `Win32_Security_Credentials`, `Win32_System_Registry`, `Win32_Globalization`, `Win32_Foundation` to `src-tauri/Cargo.toml` (req NFR-04, FR-19, FR-15) — area shell
- [ ] T003 [P] Create the catalog files `i18n/en.json` and `i18n/ru.json` (empty objects) and the Vite/TS alias `$i18n` in `vite.config.ts` / `svelte.config.js` (req FR-15; spec FR-012) — area ui

---

## Phase 2: Foundational (blocks all stories)

- [ ] T004 [P] Red tests for `Secret` (`Debug`/`Display` print `***`, no `Serialize`), `KeySlot` target names `Voicen/transcription-api`, `Voicen/local-server`, `Voicen/post-processing`, and the in-memory `FakeCredentialStore` with injectable read/write/delete failures in `crates/voicen-core/src/secrets.rs` (req NFR-04; spec FR-014) — area core
- [ ] T005 Implement `KeySlot`, `Secret` (zeroed on drop), `KeyEdit { Untouched | Replace(Secret) | Clear }`, `KeyPresence`, `CredentialStore` trait and the fake in `crates/voicen-core/src/secrets.rs` per contracts/core-traits.md (req NFR-04; spec FR-014, FR-016) — area core
- [ ] T006 [P] Red tests for `Settings` serde: round trip; container-level `#[serde(default)]` built from `defaults()` so a file missing fields loads with the values of `defaults()`, not the field type's `Default` (test `missing_fields_take_defaults`); unknown fields ignored; `schema_version` greater than 1 rejected; the serialized struct has no key field (assert on the JSON of a fully populated value) in `crates/voicen-core/src/settings/mod.rs` (req FR-13, NFR-04; spec FR-009, FR-014) — area core
- [ ] T007 Implement `Settings` with every field of data-model.md (types, `FieldId` strings), embedding the `PostProcessingSettings` of T068, in `crates/voicen-core/src/settings/mod.rs` (req FR-13) — area core
- [ ] T008 [P] Red tests for `SettingsFile` std::fs impl: `write_atomic` leaves the old file intact when the write fails midway (fake), leftover `settings.json.tmp` is deleted and never read, `move_aside` produces `settings.json.bad-<UTC yyyyMMdd-HHmmss>`; plus a `FakeSettingsFile` with injectable failures, in `crates/voicen-core/src/settings/file.rs` (req FR-13; spec FR-009, FR-010) — area core
- [ ] T009 Implement `SettingsFile` trait, the std::fs impl (tmp + sync + rename) and the fake in `crates/voicen-core/src/settings/file.rs` (research R-2) — area core
- [ ] T010 [P] (delivered by the catalog task, teamwright T-005, not by the settings core task; decisions #21) Red tests for `i18n`: both catalogs have the same id set and per-id placeholder names; every id in `MESSAGE_IDS` exists in both; `text()` substitutes `{name}` placeholders, in `crates/voicen-core/src/i18n.rs` (req FR-15; spec FR-012) — area core
- [ ] T011 (delivered by teamwright T-005; decisions #21) Implement `i18n` (embed `i18n/*.json` with `include_str!`, `UiLanguage`, `text()`, `MESSAGE_IDS`) in `crates/voicen-core/src/i18n.rs` (req FR-15; spec FR-012) — area core
- [ ] T012 [P] Implement the UI `t(id, args)` and the `uiLanguage` store importing `$i18n/en.json` and `$i18n/ru.json`, with vitest `src/lib/i18n/i18n.test.ts` asserting every literal id passed to `t(...)` in `src/**/*.{ts,svelte}` exists in both catalogs, in `src/lib/i18n/index.ts` (req FR-15; spec FR-012) — area ui
- [ ] T013 [P] Create the one IPC mock `e2e/support/tauriMock.ts`: in-memory implementation of every command of contracts/ipc.md, call recording, and emission of `settings://changed`, `settings://focus` and the window close request through `transformCallback`/`plugin:event|listen` (research R-12) — area ui
- [ ] T014 [P] Red tests for the `SettingsService` transaction skeleton with all fakes: `load_or_init` returns `FirstRun` (defaults written) with no file, `Loaded` with a valid file, `Reset{backup}` with invalid JSON / wrong types / `schema_version` 2 (file moved aside, defaults written, credential store untouched); `move_aside` failing, or `read` failing with an I/O error other than not-found → `Unavailable`: defaults in memory, file never written or moved, `save` refused with a notice, credential store untouched (spec FR-010, decisions #19); `save` writes keys then the file then publishes; a file-write failure undoes key changes and leaves file, snapshot and keys unchanged; a key-store failure refuses with `key.store_failed` on that key field and never writes the key elsewhere, in `crates/voicen-core/tests/settings_service.rs` (req FR-13, FR-21, NFR-04; spec FR-003, FR-009, FR-010, FR-016) — area core
- [ ] T015 Implement `SettingsService` (`load_or_init`, `snapshot`, `subscribe` over an `Arc<Settings>` snapshot plus a std `mpsc` channel per subscriber (decisions #22), `view`, `save` with the step order and reverse undo of research R-3 — the hotkey step is a no-op until T041; there is no autostart step until US5 adds it, T057) in `crates/voicen-core/src/settings/service.rs` (req FR-13; spec FR-003, FR-008, FR-009, FR-010) — area core
- [ ] T016 [P] Implement `CredentialStore` over Credential Manager (`CredWriteW`/`CredReadW`/`CredDeleteW`, generic, `CRED_PERSIST_LOCAL_MACHINE`, absent = Ok) with a Windows integration test (write → read → delete → read none, throwaway target prefix) in `src-tauri/src/credentials.rs` (req NFR-04; spec FR-014, FR-016) — area shell
- [ ] T017 Implement IPC commands `settings_get` and `settings_save` and the `settings://changed` event per contracts/ipc.md in `src-tauri/src/settings_ipc.rs`, wired in `src-tauri/src/lib.rs` (req FR-13) — area shell
- [ ] T018 [P] Implement `settingsApi.ts` (IPC wrapper for contracts/ipc.md) in `src/lib/settings/settingsApi.ts` (req FR-13) — area ui

- [ ] T067 [P] Define the traits `HotkeyRegistrar { prepare, commit, abort }` and `DownloadedModels { is_downloaded, list }` with fakes (`FakeHotkeyRegistrar` with injectable `prepare` failure and call log, `FakeDownloadedModels`) in `crates/voicen-core/src/hotkey_registrar.rs`, `crates/voicen-core/src/models.rs`; 004 depends on these traits, not on 001's or 002's implementations (decisions #21; req FR-05, FR-13) — area core
- [ ] T068 [P] Create the data type `post_process::settings::{PostProcessingSettings, STARTER_PROMPT, defaults()}` in `crates/voicen-core/src/post_process/settings.rs` per 003 data-model (off, `""`, `""`, starter prompt); 003 T025 adds only `validate()` and wiring (decisions #21; req FR-13, FR-09) — area core

**Checkpoint**: settings can be loaded, saved atomically with keys, and published; catalogs and mock exist.

---

## Phase 3: User Story 1 — First run: choose an engine and dictate (P1) 🎯 MVP

**Goal**: a fresh install opens settings on the Engine tab with engine none and the documented defaults; choosing API with a key and saving makes dictation work without restart; engine none blocks recording.

**Independent Test**: spec US1 Independent Test; quickstart §2 first-run scenario and §3 install smoke.

### Tests (red first)

- [ ] T019 [P] [US1] Red tests for `defaults(os_tag)`: engine none; hotkey `Ctrl+Alt+Space`; mode hold; auto-paste on; speech language auto (`null`); history on, N = 20; start with Windows off; microphone `null`; post-processing = `post_process::settings::defaults()` (off, `""`, `""`, starter prompt); API `https://api.openai.com/v1` + `whisper-1`; local server `http://localhost:8000/v1` + `""`; `ui_language` from `resolve_ui_language`, in `crates/voicen-core/src/settings/mod.rs` (req FR-21, FR-13; spec FR-005) — area core
- [ ] T020 [P] [US1] Red tests in `crates/voicen-core/src/settings/gate.rs`: `dictation_gate` — engine none → `Blocked::NoEngine`, every other engine → Ok; `blocked_actions(Blocked::NoEngine)` → `[Notify(notice.choose_engine), OpenSettings(Engine)]` and never `OpenMicrophone`; `startup_action(outcome, launched_by_autostart)` → `OpenSettings(Engine)` for `FirstRun` and `Reset` (with or without autostart), `TrayOnly` for `Loaded` (with or without autostart); `Unavailable` → `OpenSettings(Engine)` (req FR-21 failure branch, FR-01, FR-19; spec FR-006, FR-007, US1-4, US1-5, US5-3) — area core
- [ ] T021 [P] [US1] Red Playwright spec `e2e/settings-first-run.spec.ts`: with `first_run: true` the Engine tab is active and shows every default of T019; selecting API shows the pre-filled base URL and model and an empty key; entering a key and saving sends `KeyEdit.Replace` for `transcription_api` and the mock's stored `SettingsView` has `keys.transcription_api = true` and no key value anywhere in the payload (req FR-21, FR-13, NFR-04; spec US1-1..3) — area ui

### Implementation

- [ ] T022 [US1] Implement `defaults()` as the one source of defaults (calls `post_process::settings::defaults()` from T068 and `resolve_ui_language` from teamwright T-005) in `crates/voicen-core/src/settings/mod.rs` (req FR-21; spec FR-005) — area core
- [ ] T023 [US1] Implement `dictation_gate`, `blocked_actions` and `startup_action` in `crates/voicen-core/src/settings/gate.rs`; 001's hotkey handler calls `dictation_gate` before opening the microphone (req FR-21, FR-01; spec FR-006, FR-007) — area core
- [ ] T024 [US1] Implement the single on-demand settings window (`label settings`, URL `settings?tab=&field=`, focus + `settings://focus` when open, destroyed on close) in `src-tauri/src/settings_window.rs` (req FR-13, FR-21, NFR-03; spec FR-002) — area shell
- [ ] T025 [US1] Implement the startup flow in `src-tauri/src/lib.rs` as a thin executor of `startup_action` and `blocked_actions` (no decisions in the shell): `load_or_init` with the OS language (`None` until T049), open the settings window on the Engine tab for `FirstRun`/`Reset`, show `notice.settings_reset` for `Reset`, tray only for `Loaded` (also with `--autostart`); engine-none hotkey path shows `notice.choose_engine` and opens the Engine tab; log `settings load outcome=…` (req FR-21, FR-01; spec FR-005, FR-006, FR-007, FR-010, FR-021) — area shell
- [ ] T026 [US1] Implement the settings route with six tabs and the Engine tab (engine choice; API and local-server base URL, model, `KeyField`; built-in model selection hosting 002's `LocalModelList`; speech language with `Intl.DisplayNames` names) in `src/routes/settings/+page.svelte`, `src/lib/settings/tabs/Engine.svelte`, `src/lib/settings/KeyField.svelte` (req FR-13, FR-21, NFR-04; spec FR-001, FR-015) — area ui
- [ ] T027 [US1] Add en/ru messages for the first-run window, Engine tab, `notice.choose_engine`, `notice.settings_reset` to `i18n/en.json`, `i18n/ru.json` (req FR-15, FR-21) — area ui
- [ ] T028 [US1] Windows CI smoke: after silent install and first launch the log contains `settings load outcome=first_run` and `settings.json` has `"engine":"none"`, in `.github/workflows/ci.yml` (req FR-21) — area shell

**Checkpoint**: US1 independently demonstrable; owner's NFR-10 run-through possible once 001 delivers dictation (T066).

---

## Phase 4: User Story 2 — Change any setting, validated, applied without restart, persisted (P1)

**Goal**: every setting editable; invalid input refused per field; save all-or-nothing; changes apply immediately and persist.

**Independent Test**: spec US2 Independent Test; quickstart §1 settings tests and §2 save/refusal scenarios.

### Tests (red first)

- [ ] T029 [P] [US2] Red tests for `check_base_url`: trims spaces and one trailing `/`; refuses empty, `api.openai.com`, `htp://x`, `https://`, `ftp://host`; accepts `http://localhost:8000/v1`, `https://api.openai.com/v1` in `crates/voicen-core/src/settings/url.rs` (req FR-13 failure branch; spec FR-004) — area core
- [ ] T030 [P] [US2] Red tests for `Hotkey`: canonical parse/format round trip (`Ctrl+Alt+Space`, order Ctrl, Alt, Shift, Win); refuse no modifier, no key, `Esc`; accept only the closed key set of research R-6, in `crates/voicen-core/src/settings/hotkey.rs` (req FR-13, FR-22, FR-05; spec FR-004) — area core
- [ ] T031 [P] [US2] Red tests for `validate`, one per rule of spec FR-004: API — base URL empty/malformed, model empty, no key (`Untouched` without presence) → `key.required`; local server — base URL empty/malformed (model and key optional); built-in — `model_id` null or not downloaded (fake `DownloadedModels`) → `model.not_downloaded`; post-processing on — 003's rules; history size 0, 101 → `history.size_range`; fields of unselected engines and post-processing off are not validated; all errors returned at once, in `crates/voicen-core/src/settings/validate.rs` (req FR-13; spec FR-004, SC-003) — area core
- [ ] T032 [P] [US2] Red tests for the hotkey step of the transaction with a fake `HotkeyRegistrar`: unavailable → refused with `hotkey.unavailable`, old hotkey still registered, nothing else changed; a later step failing → `abort` called; success → `commit` called before `Saved`; unchanged hotkey → registrar not called, in `crates/voicen-core/tests/settings_service.rs` (req FR-05, FR-13; spec FR-003, FR-008, Clarification Q1) — area core
- [ ] T033 [P] [US2] Red tests for keys in save: `Untouched` keeps the stored key; `Clear` deletes it (engine not requiring it); `Replace` writes it; a refused save leaves every slot unchanged; for each slot (API, local server, post-processing) (req NFR-04; spec FR-015, US2-5) — area core, `crates/voicen-core/tests/settings_service.rs`
- [ ] T034 [P] [US2] Red test for live apply and persistence (P-013): after `Saved`, a subscriber receives the new snapshot; a new `SettingsService` built over the same file returns the saved values for every field; a snapshot taken before the save is unchanged (in-progress dictation keeps its settings) in `crates/voicen-core/tests/settings_apply.rs` (req FR-13; spec FR-008, FR-009, SC-002) — area core
- [ ] T035 [P] [US2] Red test for the log allowlist: a save, a refused save and a reset produce lines with field ids only; a known key, the prompt text and every URL are absent from the captured log, in `crates/voicen-core/tests/settings_log.rs` (req FR-20, NFR-04; spec FR-021, SC-004) — area core
- [ ] T036 [P] [US2] Red Playwright spec `e2e/settings-save.spec.ts`: every field of spec FR-001 is rendered and its edit reaches `settings_save`; a `Refused` outcome highlights each named field with its localized reason and keeps the draft; key field with presence shows "key saved", never a value, and sends `Untouched`/`Clear`/`Replace`; close with unsaved edits shows the discard dialog (discard closes, keep editing stays); a saved microphone missing from `settings_list_microphones` stays selected and is shown "(not connected)"; `settings://focus` switches tab and focuses the field (req FR-13, NFR-04; spec US2-1..7, FR-002) — area ui
- [ ] T037 [P] [US2] Red vitest for the draft model: dirty detection, `KeyEdit` state transitions, mapping `FieldError` → field highlight + message id, in `src/lib/settings/draft.test.ts` (req FR-13) — area ui

### Implementation

- [ ] T038 [US2] Implement `check_base_url` in `crates/voicen-core/src/settings/url.rs` (research R-8; req FR-13) — area core
- [ ] T039 [US2] Implement `Hotkey`, `HotkeyKey` (closed set), parse/format in `crates/voicen-core/src/settings/hotkey.rs` (research R-6; req FR-13, FR-22) — area core
- [ ] T040 [US2] Implement `validate` with field ids and codes of data-model.md in `crates/voicen-core/src/settings/validate.rs` (req FR-13; spec FR-004) — area core
- [ ] T041 [US2] Add the hotkey step (prepare / abort / commit) and the full key-edit step to `SettingsService::save`, and the settings log lines of research R-11, in `crates/voicen-core/src/settings/service.rs` (req FR-05, FR-13, FR-20, NFR-04; spec FR-003, FR-008, FR-021) — area core
- [ ] T042 [US2] Shell wiring (needs the implementations: 001's `HotkeyRegistrar`, 002's `ModelStore` as `DownloadedModels`; `subscribe` itself and the live-apply test T034 are core, T-003): register the 001/002/005 subscribers, the tray rebuild and the notifier on `SettingsService::subscribe()`, and add `settings_list_microphones` (001's enumeration) in `src-tauri/src/lib.rs`, `src-tauri/src/settings_ipc.rs` (req FR-13, FR-05, FR-27; spec FR-008) — area shell
- [ ] T043 [US2] Implement the draft model and the Recording (microphone with "(not connected)", `HotkeyField` capturing `KeyboardEvent.code`, mode), Output, Post-processing (hosting 003's fields), History and General tabs, field highlighting, discard dialog via `onCloseRequested`, and `settings://focus` handling in `src/lib/settings/draft.ts`, `src/lib/settings/HotkeyField.svelte`, `src/lib/settings/tabs/*.svelte`, `src/routes/settings/+page.svelte` (req FR-13, FR-27; spec FR-001, FR-002, FR-004, FR-015) — area ui
- [ ] T044 [US2] Add en/ru messages for all tabs, field labels and every `error.<code>` of data-model.md to `i18n/en.json`, `i18n/ru.json` (req FR-15, FR-13) — area ui
- [ ] T045 [US2] Windows CI integration test: save through `SettingsService` with the real registrar replaces the hotkey (old released, new registered) and a taken combination is refused with the old one still registered, in `src-tauri/tests/settings_hotkey.rs` (req FR-05, FR-13) — area shell

**Checkpoint**: US1 + US2 = the full settings window in English.

---

## Phase 5: User Story 3 — English and Russian interface (P2)

**Goal**: the whole app in English or Russian, defaulting to the OS display language, switchable live.

**Independent Test**: spec US3 Independent Test; quickstart §2 language scenario and §4 Russian first start.

### Tests (red first)

- [ ] T046 [P] [US3] (delivered by teamwright T-005; decisions #21) Red tests for `resolve_ui_language`: `ru`, `ru-RU`, `RU-ua`, `ru-Latn` → `ru`; `en-US`, `en-GB`, `de-DE`, `uk-UA`, `""`, `None` → `en`; a saved `ui_language` is never re-derived on load, in `crates/voicen-core/src/i18n.rs` (req FR-15 failure branch, FR-21; spec FR-011, US3-1..3, US3-5) — area core
- [ ] T047 [P] [US3] Red Playwright spec `e2e/settings-language.spec.ts`: with `ui_language: "ru"` the settings window renders Russian texts; selecting English and saving, then the mock emitting `settings://changed`, re-renders in English without reload; and back to Russian (req FR-15; spec US3-4, FR-013) — area ui

### Implementation

- [ ] T048 [US3] (delivered by teamwright T-005; `defaults()` consumes it; decisions #21) Implement `resolve_ui_language` in `crates/voicen-core/src/i18n.rs` (research R-7; req FR-15) — area core
- [ ] T049 [US3] Implement the OS display-language read (`GetUserPreferredUILanguages`, first entry) passed to `load_or_init` at first run, with a Windows CI test that it returns a non-empty tag, in `src-tauri/src/os_language.rs` (req FR-15, FR-21; spec FR-011) — area shell
- [ ] T050 [US3] Switch the UI language store on `settings://changed` and on load; rebuild the tray menu and use the current language for every later notification in the shell subscriber, in `src/lib/i18n/index.ts`, `src-tauri/src/lib.rs` (req FR-15; spec FR-013) — area ui + shell
- [ ] T051 [US3] Complete the Russian catalog for every id present in English (parity test T010 is the gate) in `i18n/ru.json` (req FR-15; spec FR-012, SC-005) — area ui

---

## Phase 6: User Story 4 — Test connection (P3)

**Goal**: "Test connection" reports OK with latency or the precise reason, without changing anything.

**Independent Test**: spec US4 Independent Test; quickstart §1 connection tests and §2 test-connection scenario.

### Tests (red first)

- [ ] T052 [P] [US4] Red tests for `ConnectionTester` against a mock OpenAI-compatible server: 200 with a transcription body → `Ok{latency_ms}` (fake clock); 401 and 403 → `InvalidKey`; connection refused / unresolvable host → `CannotReach{host}`; no answer within an injected short total timeout → `Timeout`; 500 → `Http{500}`; non-JSON body → `UnexpectedResponse`; the local server without key sends no `Authorization` and omits `model` when empty; `Untouched` uses the stored key, `Clear` sends none; the fake credential store records no write/delete and the settings snapshot is unchanged; timeouts come from the shared module (30 s API, 60 s local server), in `crates/voicen-core/tests/connection_test.rs` (req FR-14, FR-24, NFR-04; spec FR-017, FR-018, SC-006) — area core
- [ ] T053 [P] [US4] Red Playwright spec `e2e/settings-test-connection.spec.ts`: button present for API and local server only; disabled with progress while the mock is pending; each result kind shows its localized message (`OK, {ms} ms`, `cannot reach {host}`, `invalid API key`, `timeout`, `HTTP {status}`, `unexpected response`); a refused-form result highlights fields; closing the window while a test is pending discards the result and saves nothing (req FR-14; spec US4-1..6, Edge Cases) — area ui

### Implementation

- [ ] T054 [US4] Record and add the bundled 1 s 16 kHz mono WAV clip `crates/voicen-core/src/assets/test-clip.wav` and implement `ConnectionTester` over 001's `TranscriptionClient` (no VAD) in `crates/voicen-core/src/connection_test.rs` (research R-9; req FR-14; spec FR-017, FR-018) — area core
- [ ] T055 [US4] Add the IPC command `settings_test_connection` in `src-tauri/src/settings_ipc.rs`, the button and result display in `src/lib/settings/tabs/Engine.svelte`, and the en/ru result messages in `i18n/*.json` (req FR-14, FR-15) — area shell + ui

---

## Phase 7: User Story 5 — Start with Windows (P3)

**Goal**: the option registers/removes a per-user logon start that runs tray-only.

**Independent Test**: spec US5 Independent Test; quickstart §3 autostart and §4 reboot check.

### Tests (red first)

- [ ] T056 [P] [US5] Red tests for the autostart step with a fake `Autostart`: on/off applied only when changed; failure → refused with `autostart.failed` on `general.start_with_windows` and the hotkey step aborted; a later key or file failure restores the previous autostart state; `reconcile_autostart` writes a missing entry for `true` and removes a stale one for `false`, in `crates/voicen-core/tests/settings_service.rs` (req FR-19; spec FR-019, US5-4) — area core

### Implementation

- [ ] T057 [US5] Add the `Autostart` trait in `crates/voicen-core/src/autostart.rs` and the autostart step + `reconcile_autostart` in `crates/voicen-core/src/settings/service.rs` (research R-3, R-5; req FR-19) — area core
- [ ] T058 [US5] Implement `Autostart` over `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Voicen` = `"<exe>" --autostart` with a Windows CI test (set true writes, set false removes, absent = Ok), call reconcile at start, and parse `--autostart` (tray only except the cases of spec US5-3) in `src-tauri/src/autostart.rs`, `src-tauri/src/lib.rs` (req FR-19; spec FR-019, US5-3) — area shell
- [ ] T059 [US5] Add the General-tab toggle messages in `i18n/*.json` and a Playwright case in `e2e/settings-save.spec.ts` for the toggle saved and for an `autostart.failed` refusal highlight (req FR-19, FR-15) — area ui

---

## Phase 8: User Story 6 — Warning for unencrypted remote endpoints (P4)

**Goal**: saving an `http://` URL in use on a non-loopback host shows a warning and still saves.

**Independent Test**: spec US6 Independent Test; quickstart §2 warning scenario.

### Tests (red first)

- [ ] T060 [P] [US6] Red tests for `is_insecure_remote` and the save warnings: `http://example.com/v1`, `http://192.168.1.5:8000/v1` → warning; `http://localhost:8000/v1`, `http://LOCALHOST`, `http://127.0.0.1`, `http://127.1.2.3`, `http://[::1]:8000/v1`, any `https://` → none; a warning only for the selected engine's URL and the post-processing endpoint while it is on; `Saved` returned with the warning, in `crates/voicen-core/src/settings/url.rs`, `crates/voicen-core/tests/settings_service.rs` (req FR-29; spec FR-020, US6-1..4) — area core
- [ ] T061 [P] [US6] Red Playwright spec `e2e/settings-warning.spec.ts`: `Saved` with `endpoint.insecure` shows the localized warning next to the field and the window shows saved state; no warning without it (req FR-29, FR-15) — area ui

### Implementation

- [ ] T062 [US6] Implement `is_insecure_remote` and add warnings to `SaveOutcome::Saved` in `crates/voicen-core/src/settings/url.rs`, `crates/voicen-core/src/settings/service.rs` (research R-8; req FR-29; spec FR-020) — area core
- [ ] T063 [US6] Show save warnings in the UI and add the en/ru warning message in `src/routes/settings/+page.svelte`, `i18n/*.json` (req FR-29, FR-15) — area ui

---

## Phase 9: Polish & cross-cutting

- [ ] T064 [P] Leak test end to end in core (SC-004): with a known test key, save, test connection against the mock server (including a 401), and fail a request; assert the key is absent from the settings file bytes and every captured log line, in `crates/voicen-core/tests/secrets_leak.rs` (req NFR-04; spec FR-014, SC-004) — area core
- [ ] T065 Update `docs/architecture.md` (settings service, catalog location, IPC commands, seams) and add `docs/decisions/core.md` invariants (save all-or-nothing order; no key field in `Settings`) in the same commits as the code (P-014) — docs
- [ ] T066 Owner's manual checks of quickstart §4: NFR-10 timed first run in Windows Sandbox, Russian first start, autostart reboot on/off, live apply of each setting, real-endpoint Test connection, key search in `%LOCALAPPDATA%\Voicen`, hotkey pressed with engine none (no recording, notification, settings on the Engine tab), settings requested while open (one window comes to front) (req NFR-10, FR-13, FR-14, FR-15, FR-19, FR-21, NFR-04; spec SC-001, SC-005, SC-007, FR-002, FR-007) — verify_exception with the manual record in `docs/tasks/<ID>.verify/`

---

## Dependencies & Execution Order

- **Setup (T001–T003)** → **Foundational (T004–T018, T067, T068)** → stories. 004 depends on the traits `HotkeyRegistrar`, `DownloadedModels` and `CredentialStore` it defines (T005, T067), not on 001/002/003 implementations; the dependency direction is 001/002/003 → 004. T010/T011/T046/T048 come from teamwright T-005 (catalog), which T-003 consumes.
- **US1 (T019–T028)** needs Foundational. **US2 (T029–T045)** needs Foundational; T032/T041 need only T067's trait and fake; T042/T045 (shell) need 001's `HotkeyRegistrar` and 002's `ModelStore` implementations; T031 uses T067's fake. T034 (live apply) is core. US1 and US2 can proceed in parallel; the UI tasks T026 and T043 touch the same route file — do T026 first.
- **US3 (T046–T051)** needs T011/T012; independent of US2 except that T051 completes after all messages exist (do it last).
- **US4 (T052–T055)** needs 001's `TranscriptionClient` and timeouts module.
- **US5 (T056–T059)** needs T015; it adds the `Autostart` trait, the autostart save step and `reconcile` (the Foundational and US2 save has no autostart step).
- **US6 (T060–T063)** needs T038.
- **Polish** after all stories; T066 needs 001's dictation for NFR-10.

Cross-feature: 001 (implements `HotkeyRegistrar`; client, timeouts, tray, notifier, microphone list), 002 (`ModelStore` implements `DownloadedModels`; `LocalModelList`), 003 (`post_process::settings::validate`, its fields component), 005 (subscriber), 006 (log writer, uninstall of the Run value and keys).

## Parallel Examples

- Foundational: T004, T006, T008, T010, T012, T013, T014 (red tests and UI scaffolding in different files) together; then T005, T007, T009, T011 in parallel; T016 and T018 alongside.
- US2: T029, T030, T031, T035, T036, T037 together; T038, T039 together, then T040.
- US4 and US6 red tests (T052, T053, T060, T061) can be written in parallel with US2 implementation.

## Implementation Strategy

1. **MVP** = Setup + Foundational + US1: a fresh install opens settings, the user chooses the API engine with a key, and (with 001) dictates; engine none is blocked.
2. Add US2 (full settings and validation) — completes req FR-13 (Must).
3. Add US3 (Russian) — completes req FR-15 (Must; first to cut after the Shoulds per requirements §6 scope note).
4. Add US4 and US5 (Should), then US6 (Could).
5. Each story ends with its red tests green, `make check` green, and its Windows CI or manual evidence.

## Requirement coverage

| Req | Tasks |
|---|---|
| FR-13 | T067, T068, T006–T009, T014, T015, T017, T018, T026, T029–T045 |
| FR-14 | T052–T055, T066 |
| FR-15 | T003, T010–T012, T027, T044, T046–T051, T055, T059, T061, T063, T066 |
| FR-19 | T020, T056–T059, T066 |
| FR-21 | T019–T028, T049, T066 |
| FR-29 | T060–T063 |
| NFR-04 | T004, T005, T014, T016, T021, T033, T035, T036, T052, T064, T066 |
| NFR-10 | T066 |
