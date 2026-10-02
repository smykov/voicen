# Implementation Plan: Settings and First Run

**Branch**: `004-settings-and-first-run` | **Date**: 2026-10-02 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/004-settings-and-first-run/spec.md`

## Summary

The settings model and everything that changes it live in `voicen-core`: one `Settings` type with one defaults function, a validator that returns all field errors at once, a `SettingsService` that loads (first run / loaded / reset of an unreadable file), saves as one all-or-nothing transaction (validate → two-phase hotkey → autostart (step added by US5) → keys → atomic file write → commit, with undo in reverse on failure), and publishes snapshots (an `Arc<Settings>` plus a std `mpsc` channel per subscriber) to subscribers so every setting applies without restart. Keys go only through a `CredentialStore` trait (Windows Credential Manager in the shell) and travel as a redacted `Secret`; the settings type has no key field. A single English/Russian message catalog (two JSON files) serves both Rust (tray, notifications) and the UI, with gate tests for parity. Test connection reuses 001's transcription client with a bundled 1 s clip. The shell adds the Windows implementations (Credential Manager, `HKCU\…\Run` autostart, OS display language), the IPC commands and events, the single on-demand settings window and the startup flow. The UI adds the six-tab settings route with draft/dirty handling, field highlighting, key presence, Test connection, the http warning and live language switching, verified with Playwright against a mocked IPC.

## Technical Context

**Language/Version**: Rust 1.99 (edition 2021) for `voicen-core` and `src-tauri`; TypeScript + Svelte 5 (SvelteKit static SPA) for the UI.

**Primary Dependencies**: core — `serde`/`serde_json`, `url` and `zeroize` (direct dependencies, consented by decisions #17), std `mpsc` for `subscribe` (no tokio in core, decisions #22); shell — `windows` crate (`Win32_Security_Credentials`, `Win32_System_Registry`, `Win32_Globalization`), Tauri 2 window/event APIs; UI — `@tauri-apps/api` (core invoke, event, window). 001's OpenAI-compatible client and timeouts module. No new runtime i18n library (research R-7).

**Storage**: `%LOCALAPPDATA%\Voicen\settings.json` (data dir resolved once by the shell, P-010), atomic write via temp file + rename; unreadable files moved to `settings.json.bad-<UTC>`; a file that cannot be moved aside or read for an I/O reason is never written or moved (spec FR-010, decisions #19). Keys only in Windows Credential Manager, slots `Voicen/transcription-api`, `Voicen/local-server`, `Voicen/post-processing`. Autostart in `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Voicen`.

**Testing**: `cargo test -p voicen-core` in `voicen-rust:1.99` (fakes for credential store, settings file, autostart, hotkey registrar, downloaded models, clock; mock HTTP server for Test connection; log capture for leak checks); `cargo test --workspace` on the Windows CI runner (Credential Manager, registry, OS language, install smoke); vitest + Playwright with mocked IPC for the UI.

**Target Platform**: Windows 10/11 x64; core platform-independent.

**Project Type**: Desktop app (Tauri 2) — core library + Windows shell + web UI.

**Performance Goals**: settings window interactive within 1 s of the request on the reference machine (not a requirement; a sanity target). Save completes in < 1 s except when the OS stalls. Test connection bounded by FR-24 (≤ 35 s API, ≤ 65 s local server — spec SC-006). First-run setup ≤ 3 min end to end (req NFR-10).

**Constraints**: keys never in the settings file, logs, crash files, UI state or messages (req NFR-04); settings log lines carry field ids only (spec FR-021); the settings window is created on demand and destroyed on close (req NFR-03); MIT-compatible licences (req NFR-12); per-user only, no elevation.

**Scale/Scope**: ~20 settings on 6 tabs; 3 key slots; 2 UI languages; one window.

**Unknowns resolved in research.md**: credential storage (R-1), file format, atomicity and schema versioning (R-2, closes checklist CHK008), save transaction and undo order (R-3, closes CHK006), live apply (R-4), autostart mechanism (R-5), hotkey key set (R-6, closes CHK015), catalog and OS language (R-7), URL rule and loopback (R-8), Test connection request (R-9), window and first-run flow (R-10), log allowlist (R-11), UI verification (R-12). No NEEDS CLARIFICATION remains.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | How this plan complies | Status |
|---|---|---|
| I. PRINCIPLES.md | P-004/P-005: every validation rule, every save-step failure (hotkey, autostart, key, file), unreadable file, Test connection error kind and the engine-none gate has a red test task; refused saves are asserted to change nothing in every fake. P-009: `Secret` masks itself; `Settings` has no key field; log lines are an allowlist of field ids; a leak test scans file and captured logs for a known key. P-010: one `defaults()`, one URL rule, one catalog, one hotkey representation, data dir from the shell's single resolver. P-011: one publish point for changes, one transcription client for Test connection and dictation, one IPC mock for all e2e specs. P-013: a test loads the saved file through the real loader and checks the value reached the snapshot consumers see. | PASS |
| II. Privacy by default | Keys only in Credential Manager (no fallback, spec FR-016); Test connection sends only a bundled clip to the endpoint the user typed; the settings window makes no other network call (spec FR-018). | PASS |
| III. Never lose the user's words | A refused save never changes the running configuration; an in-progress dictation keeps the settings it started with; engine none never opens the microphone and tells the user what to do. | PASS |
| IV. Core / thin shell | Model, validation, transaction, load/reset, catalog, URL rule, language resolution, Test connection logic and the engine-none gate are in `voicen-core` with fakes. Credential Manager, registry, OS language, windows and IPC are in `src-tauri` behind core traits. | PASS |
| V. Measured, not assumed | Verification location stated per requirement below; NFR-10 by a timed owner run-through; Windows-only behaviour on the Windows CI runner. | PASS |

**Post-design re-check (after Phase 1)**: PASS. No new crate; no violations.

### Where each requirement is verified

| Requirement | Linux host — core with fakes | Linux host — UI with mocked IPC | Windows CI runner | Owner's manual check |
|---|---|---|---|---|
| FR-13 settings, validation, live apply, persistence | every rule of spec FR-004; all-or-nothing with each step failing; atomic write + leftover `.tmp`; reset of unreadable / newer-schema file; snapshot published and read back through the real loader | every field rendered and saved; refusal highlights per field; key presence; discard dialog; focus event switches tab | settings file in `%LOCALAPPDATA%\Voicen`; hotkey re-registration on save (with 001's registrar) | each setting takes effect without restart and survives restart |
| FR-14 Test connection | result mapping vs mock server (OK + latency, 401, 403, unreachable, timeout, 500, bad body); untouched key read from fake store; nothing written | button disabled while running; every result message | — | real OpenAI / Groq with good and wrong keys |
| FR-15 English/Russian | catalog parity (ids, placeholders); `MESSAGE_IDS` present; `resolve_ui_language` table | Russian and English render; live switch on `settings://changed`; `t()` ids exist (vitest) | OS display language read returns a tag | Russian Windows first start: settings, tray, overlay, notifications |
| FR-19 start with Windows | save applies/undoes via fake `Autostart`; failure refuses; reconcile | toggle saved; failure highlight | Run value written/removed; path rewritten by reconcile | reboot with on/off |
| FR-21 first run, engine none | `defaults()` values; `FirstRun` persisted; `dictation_gate` blocks engine none; `blocked_actions` = notify + open Engine tab; `startup_action` (first run/reset → settings, loaded → tray only, with and without autostart) | first-run window on Engine tab with defaults; API pre-fill | silent install + first launch logs `outcome=first_run` | fresh install in Windows Sandbox |
| FR-29 http warning | `is_insecure_remote` table (localhost any case, 127/8, ::1, LAN, https); only URLs in use | warning shown / not shown | — | — |
| NFR-04 keys | known key absent from settings file bytes and captured logs after save, test and a failed request; `Secret` `Debug` = `***`; no fallback on store failure | `SettingsView` in the mock has no key; key field never shows a value | Credential Manager write/read/delete round trip | search of `%LOCALAPPDATA%\Voicen` for a test key |
| NFR-10 ≤ 3 min first run | — | — | — | timed run-through in Windows Sandbox |

## Project Structure

### Documentation (this feature)

```text
specs/004-settings-and-first-run/
├── plan.md              # This file
├── research.md          # Phase 0
├── data-model.md        # Phase 1
├── quickstart.md        # Phase 1
├── contracts/
│   ├── core-traits.md   # CredentialStore, SettingsFile, Autostart, SettingsService, HotkeyRegistrar, DownloadedModels, ConnectionTester, i18n, consumed interfaces
│   └── ipc.md           # Tauri commands, events and the settings window
├── checklists/
└── tasks.md             # Phase 2 (/speckit-tasks)
```

### Source Code (repository root)

```text
i18n/
├── en.json                       # the one message catalog (English)
└── ru.json                       # the one message catalog (Russian)

crates/voicen-core/src/
├── settings/
│   ├── mod.rs                    # Settings, defaults(), FieldId, SettingsView
│   ├── validate.rs               # all rules of spec FR-004
│   ├── url.rs                    # check_base_url, is_insecure_remote
│   ├── hotkey.rs                 # Hotkey, HotkeyKey set, parse/format
│   ├── file.rs                   # SettingsFile trait + std::fs impl (atomic write, move aside)
│   ├── service.rs                # SettingsService: load_or_init, save transaction, subscribe, reconcile
│   └── gate.rs                   # dictation_gate (engine none)
├── secrets.rs                    # KeySlot, Secret, KeyEdit, CredentialStore trait
├── autostart.rs                  # Autostart trait
├── connection_test.rs            # ConnectionTester over 001's client
├── i18n.rs                       # catalog embed, text(), MESSAGE_IDS, resolve_ui_language — delivered by the catalog task (teamwright T-005), consumed here (decisions #21)
├── post_process/settings.rs      # PostProcessingSettings, STARTER_PROMPT, defaults() — data type created here, 003 adds validate() (decisions #21)
├── assets/test-clip.wav          # bundled 1 s clip for Test connection
└── (unit tests in each module; crates/voicen-core/tests/settings_*.rs)

src-tauri/src/
├── credentials.rs                # CredentialStore over Credential Manager
├── autostart.rs                  # Autostart over HKCU\…\Run
├── os_language.rs                # GetUserPreferredUILanguages
├── settings_ipc.rs               # commands + events of contracts/ipc.md
├── settings_window.rs             # single on-demand window, focus requests
└── lib.rs                        # startup: load_or_init, reconcile, first-run window, --autostart, subscribers (tray, notifier)

src/lib/i18n/
├── index.ts                      # t(), language store, imports $i18n catalogs
└── i18n.test.ts                  # every t() literal id exists in both catalogs
src/lib/settings/
├── settingsApi.ts                # IPC wrapper
├── draft.ts                      # draft/dirty, KeyEdit state, error mapping
├── draft.test.ts
├── HotkeyField.svelte            # capture KeyboardEvent.code → canonical hotkey
├── KeyField.svelte               # presence, replace, clear; never shows a stored key
└── tabs/{Engine,Recording,Output,PostProcessing,History,General}.svelte
src/routes/settings/+page.svelte  # the settings window

e2e/support/tauriMock.ts          # the one IPC mock (commands + events)
e2e/settings-*.spec.ts            # first run, save/refusal, keys, language, test connection, warning
```

**Structure Decision**: Keep the existing three areas (core, shell, UI); no new crate. The catalog sits at the repository root because both the Rust core (`include_str!`) and the UI (Vite alias) read it — placing it in either area would make the other reach across. Tabs host components owned by other features (002's `LocalModelList`, 003's post-processing fields) without owning their behaviour.

## Notes for other features

- 004 defines `HotkeyRegistrar` and `DownloadedModels` (with fakes) in core; 001 implements `HotkeyRegistrar`, 002's `ModelStore` implements `DownloadedModels` (decisions #21). 001 calls `dictation_gate` before opening the microphone; its tray "Settings", second-instance and startup hotkey-failure paths call `settings_window::open(tab, field)`.
- The current `tauri.conf.json` opens a main window at start; the tray-only start (req FR-01) removes it (001), and the first-run path opens the `settings` window instead (this feature).
- 005 subscribes to `SettingsService::subscribe()` for history on/off and size changes.

## Complexity Tracking

No constitution violations to justify.
