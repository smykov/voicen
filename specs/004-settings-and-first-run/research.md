# Research: Settings and First Run

**Feature**: [spec.md](./spec.md) · **Plan**: [plan.md](./plan.md) · **Date**: 2026-10-02

The stack is fixed by `docs/requirements.md` §9 (Tauri 2, Rust 1.99, `windows` crate, `reqwest`, Svelte 5, Playwright with mocked IPC). This file records only how the feature is built within it. Each entry: Decision · Rationale · Alternatives considered.

## R-1 Credential storage (req NFR-04; spec FR-014–FR-016)

- **Decision**: A core trait `CredentialStore` (read / write / delete per `KeySlot`), implemented in `src-tauri` with the `windows` crate (`CredWriteW`, `CredReadW`, `CredDeleteW`, `CRED_TYPE_GENERIC`, `CRED_PERSIST_LOCAL_MACHINE`, i.e. this user on this machine, not roamed). One generic credential per slot, target names `Voicen/transcription-api`, `Voicen/local-server`, `Voicen/post-processing`; user name field `voicen`. The secret is stored as UTF-8 bytes. Deleting an absent entry (`ERROR_NOT_FOUND`) is success. Core tests use an in-memory fake that can be told to fail each operation.
- Keys travel as a `Secret` newtype (redacted `Debug`/`Display`, no `Serialize`, zeroed on drop). The component that holds a key masks it (P-009); the settings model has no key field at all, so the settings file cannot contain one by construction.
- **Rationale**: §9 names the `windows` crate for Credential Manager; three Win32 calls do not justify a wrapper dependency; slot names match 002 and 003 (`KeySlot::TranscriptionApi`, `LocalServer`, `PostProcessing`).
- **Alternatives**: `keyring` crate (extra dependency and its own error mapping; same calls underneath); DPAPI-encrypted file (not Credential Manager — violates NFR-04).

## R-2 Settings file, atomic save and schema version (req FR-13; spec FR-009, FR-010; checklist CHK008)

- **Decision**: One JSON file `<data dir>/settings.json` (data dir `%LOCALAPPDATA%\Voicen`, resolved once by the shell, P-010). Save writes `settings.json.tmp`, flushes and syncs it, then renames it over `settings.json` (`std::fs::rename` → `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` on Windows). A leftover `.tmp` at load is deleted and never read.
- The file carries `schema_version: 1`. Missing fields take their default (container-level `#[serde(default)]` built from `defaults()`, so a missing field takes the `defaults()` value; decisions #21/H4), unknown fields are ignored: an older file loads (upgrade path) without a migration step in release 1. A file whose `schema_version` is greater than the app knows (written by a newer version — a downgrade), or that is not valid JSON, or whose known fields have the wrong type, is **unreadable**: it is renamed to `settings.json.bad-<UTC yyyyMMdd-HHmmss>` and the defaults are used as on a first run (spec FR-010). Future schema changes add a migration function per version step.
- File access goes through a small core trait `SettingsFile` (read bytes / write atomically / move aside) so tests can inject write and rename failures (the Linux gate runs as root in Docker, where permission tricks do not fail).
- **Rationale**: JSON is readable for support and already used by `serde_json`; write-then-rename gives "never a partial file" (spec FR-009); the version rule makes downgrade safe (backup kept) without promising forward compatibility.
- **Alternatives**: TOML (no benefit here; one more crate); in-place write with a lock (partial file on crash); the Tauri `store` plugin (writes in place, no version rule, values in the webview's reach).

## R-3 Save as one transaction with compensation (req FR-13, FR-05, FR-19, NFR-04; spec FR-003, FR-004, FR-008; checklist CHK006)

- **Decision**: `SettingsService::save` runs under one mutex (saves are serialized) in this order; any failure undoes the completed steps in reverse and returns a refusal:
  1. Normalize (trim, strip a trailing `/` from URLs) and validate the whole draft; all field errors are returned together. Nothing has changed yet.
  2. Hotkey changed → `HotkeyRegistrar::prepare(new)` registers the new combination while the old one stays registered. Failure → field error `hotkey.unavailable`.
  3. Start with Windows changed → `Autostart::set(new)`. Failure → undo 2; field error on `general.start_with_windows`.
  4. Key edits (replace / clear) per slot: the old value is read first (held as `Secret`), then written or deleted. Failure → restore the slots already changed, undo 3 and 2; field error on that key field.
  5. Write the settings file atomically. Failure → undo 4, 3, 2; form-level error "cannot save settings: <reason>".
  6. Commit: `HotkeyRegistrar::commit` (releases the old hotkey), swap the in-memory snapshot, publish it to subscribers, log the outcome, return success with warnings (R-8).
  If an undo step itself fails (a double failure), the log records which step could not be undone (by name), the refusal says so, and the next start reconciles what it can: the autostart entry is rewritten from the settings file (R-5). A key written in step 4 whose undo failed stays in the credential store; this is the only state that can survive a double failure and is documented.
- **Rationale**: the settings file is written last, so it is always the source of truth; steps that touch the OS come before it and are reversible; the hotkey is two-phase because Windows lets both combinations be registered at once (req FR-05 "old one stays active").
- **Alternatives**: apply each setting independently (violates the all-or-nothing clarification Q1); write the file first and apply afterwards (a failed hotkey would leave a file that disagrees with the running app).

## R-4 Applying settings without restart (req FR-13; spec FR-008, FR-013)

- **Decision**: The service holds the current settings as `Arc<Settings>` behind a std `mpsc` channel per subscriber (no tokio in core, decisions #22). Consumers read a snapshot when they start work: 001's pipeline at recording start (so a dictation keeps the settings it started with), 005 on change, the shell's tray builder and notifier on change. The shell forwards every change to open webviews as the Tauri event `settings://changed` with the same `SettingsView` that `settings_get` returns. Hotkey and autostart are applied inside the save (R-3), so they are active before success is reported.
- **Rationale**: one publish point (P-011); a snapshot at work start gives the "in-progress dictation unaffected" edge case for free; core stays synchronous (decisions #22).
- **Alternatives**: consumers re-read the file (races, P-013 risk); per-setting callbacks (N parallel paths).

## R-5 Start with Windows (req FR-19; spec FR-019)

- **Decision**: Core trait `Autostart { is_enabled, set(bool) }`; Windows impl writes the per-user value `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Voicen` = `"<current exe path>" --autostart` with `RegSetKeyValueW`, and removes it with `RegDeleteKeyValueW` (absent = success). The app parses `--autostart` and then never opens a window except in the cases listed in spec US5-3. At every start the shell reconciles the entry with the saved setting (rewrites the path if the app moved or was updated; removes a stale entry). The installer/uninstaller part is 006's.
- If the user disabled the entry in Task Manager (`StartupApproved\Run`), Windows will not start the app; the app does not override the user's choice there. The settings window still shows the saved setting. Recorded as a known limitation in quickstart.
- **Rationale**: §9 names the `windows` crate for autostart; a Run value is per user, needs no elevation, and is what Task Manager's Startup tab lists.
- **Alternatives**: `tauri-plugin-autostart` (wraps `auto-launch`, same registry value, adds a plugin and JS permissions); Task Scheduler logon task (heavier, elevation prompts in some policies); Startup folder shortcut (needs COM `IShellLink`).

## R-6 Hotkey model and capture (req FR-02, FR-05, FR-22 via FR-13; spec FR-004; checklist CHK015)

- **Decision**: `Hotkey { ctrl, alt, shift, win: bool, key: HotkeyKey }`, serialized canonically as `"Ctrl+Alt+Space"` (order Ctrl, Alt, Shift, Win). `HotkeyKey` is a closed set: `A`–`Z`, `0`–`9`, `F1`–`F24`, `Space`, `Insert`, `Delete`, `Home`, `End`, `PageUp`, `PageDown`, the four arrows, `Pause`, numpad `0`–`9`. Validation: at least one modifier, exactly one key from the set; `Esc` is never in the set (reserved for cancel, req FR-22). Combinations reserved by Windows (e.g. `Win+L`) pass validation but fail registration and are refused as "hotkey unavailable" (req FR-05). The UI capture field maps `KeyboardEvent.code` to `HotkeyKey` and shows the canonical text; the shell maps `HotkeyKey` to a virtual-key code for `RegisterHotKey` (001's registrar).
- **Rationale**: a closed set makes validation total and testable; the canonical text is the single representation in the file, the UI and logs (P-010).
- **Alternatives**: free-form accelerator strings (Tauri `global-shortcut` syntax) — open-ended, harder to validate and to map identically in UI and shell.

## R-7 Message catalog and UI language (req FR-15, FR-21; spec FR-011–FR-013)

- **Decision**: One catalog as two flat JSON files at the repository root, `i18n/en.json` and `i18n/ru.json`: message id → text with named placeholders `{name}`. Rust embeds them with `include_str!` in `voicen-core::i18n` (`text(lang, id, args)`), used by the shell for the tray menu and notifications; the UI imports the same files through a Vite alias `$i18n` and renders them with a small `t(id, args)` function bound to a Svelte store of the current language. Message ids are dotted and grouped by owner (`settings.*`, `tray.*`, `notice.*`, `error.*`, `post_processing.*` …); features 001–006 add their ids to the same files.
- Gate checks (spec FR-012): a core test that both files have the same id set and, per id, the same placeholder set; a core test that every `MessageId` constant used by Rust exists; unknown literal ids passed to `t(...)` in `src/**` are caught by `svelte-check`, because `t` takes `MessageId = keyof typeof en` (this replaced the planned vitest scan of literal ids).
- OS language: the shell reads the first entry of `GetUserPreferredUILanguages(MUI_LANGUAGE_NAME)` (Windows display language, e.g. `ru-RU`) at first run only; the pure core function `resolve_ui_language(Option<&str>)` returns `ru` when the primary subtag is `ru` (any case, any region) and `en` otherwise, including `None` (spec FR-011).
- Speech-language names are produced by `Intl.DisplayNames([uiLang], {type: "language"})` from the codes (spec FR-012 exemption).
- **Rationale**: one source for both Rust and UI (P-010); flat JSON needs no runtime library; no message in release 1 needs plural forms (numbers appear as "N ms" or in fields), so ICU/Fluent is not needed.
- **Alternatives**: Fluent (`fluent-rs` + `@fluent/bundle`) — plural support not needed yet, two runtimes; UI fetching the catalog over IPC — every e2e mock would have to serve it and the UI would show nothing before the call; separate catalogs for UI and Rust — two sources that drift.

## R-8 URL validation and the http warning (req FR-13, FR-29; spec FR-004, FR-020)

- **Decision**: Core function `check_base_url(&str) -> Result<NormalizedUrl, UrlError>` using the `url` crate (already a dependency of `reqwest`): trim whitespace, strip one trailing `/`, require scheme `http` or `https` and a non-empty host. `is_insecure_remote(&NormalizedUrl)` is true for `http` when the host is not loopback: `localhost` (the parser lowercases it), any IPv4 in `127.0.0.0/8`, or IPv6 `::1`. The save returns a warning `endpoint.insecure` per URL in use (spec FR-020) together with success.
- **Rationale**: one rule for all three URLs (closes 003 CHK010); parsing with the same crate the HTTP client uses avoids two notions of "valid URL".
- **Alternatives**: regex validation (accepts what the client rejects and vice versa).

## R-9 Test connection (req FR-14, FR-24; spec FR-017, FR-018)

- **Decision**: Core `ConnectionTester` builds an endpoint from the form values (engine API or local server, base URL, model — omitted when empty for the local server — and the key: typed key, or the stored key when untouched, or none when cleared) and calls 001's OpenAI-compatible `TranscriptionClient` directly with a bundled 1 s 16 kHz mono WAV clip (embedded with `include_bytes!`, ≈ 32 KB) and no language, bypassing the VAD gate. Latency is measured with the injected clock from request start to response. Result mapping uses 001's failure reasons: unreachable / connect timeout → `CannotReach{host}`; 401/403 → `InvalidKey`; total timeout (30 s API, 60 s local server, from the shared timeouts module) → `Timeout`; other status → `Http{status}`; unparsable body → `UnexpectedResponse`; any 2xx with a valid transcription body (even empty text) → `Ok{latency_ms}`. The tester never touches the settings service or the credential store's write path.
- The clip is recorded for the project (a short spoken phrase) and committed under the project's MIT licence; it is a test asset, not user audio.
- **Rationale**: same client as dictation (req FR-06/FR-17, P-011), so the test exercises the real request; the clip is small enough to embed.
- **Alternatives**: `GET /models` (not supported by all OpenAI-compatible servers, and does not prove transcription works); generated silence (some servers reject or hallucinate on silence — and req FR-12 notes silence handling).

## R-10 Settings window and first-run flow (req FR-13, FR-21; spec FR-002, FR-005–FR-007)

- **Decision**: The settings UI is a SvelteKit route `src/routes/settings/+page.svelte` (static SPA) with six tabs. The shell opens it on demand as the single webview window labelled `settings` (`WebviewWindowBuilder`, URL `settings?tab=<tab>&field=<field>`); a request while it exists focuses it and emits `settings://focus {tab, field}`. Closing destroys it (req NFR-03). The form keeps a draft separate from the last `SettingsView`; dirty = draft differs. The close button is intercepted with `getCurrentWindow().onCloseRequested` and asks "discard unsaved changes?" in an in-page dialog.
- Startup in the shell: `SettingsService::load_or_init()` returns `Loaded`, `FirstRun` (defaults written immediately) or `Reset{backup}`; for `FirstRun` and `Reset` the settings window opens on the Engine tab (plus the reset notification). `--autostart` does not change this (spec US5-3).
- Engine-none gate: core `dictation_gate(&Settings) -> Result<(), Blocked>` returns `Blocked::NoEngine` for engine none; 001's hotkey handler calls it before opening the microphone; the shell then shows `notice.choose_engine` and opens settings on the Engine tab.
- **Rationale**: one window, one route, on-demand creation (NFR-03); the gate is a pure function the pipeline and tests share.
- **Alternatives**: settings as a permanent hidden window (RAM, NFR-03); first-run wizard separate from settings (requirements say settings open on the Engine tab).

## R-11 Logging without values (req FR-20, NFR-04; spec FR-021)

- **Decision**: Settings log lines carry an allowlist of fields only: `settings load outcome=<loaded|first_run|reset>`, `settings save outcome=<ok|refused|failed> changed=<field ids> errors=<field ids:codes> warnings=<codes>`, `settings test_connection result=<ok|cannot_reach|invalid_key|timeout|http_<n>|unexpected> latency_ms=<n>`, `autostart reconcile action=<none|written|removed|failed>`. No values, no URLs, no hosts. Log writing itself is 006's.
- **Rationale**: P-009 — the line format is the allowlist; field ids are the same strings as in `FieldError`.
- **Alternatives**: structured logging of the settings struct with redaction (one forgotten field leaks the prompt or a URL with credentials).

## R-12 UI verification with mocked IPC (constitution V)

- **Decision**: Playwright specs `e2e/settings-*.spec.ts` install an IPC mock in `window.__TAURI_INTERNALS__` (as `e2e/build-info.spec.ts` does) that implements the commands of [contracts/ipc.md](./contracts/ipc.md) over an in-memory state, records calls, and can emit `settings://changed` / `settings://focus` and the window close request through the event plugin's `listen` callbacks (`transformCallback`). A shared helper `e2e/support/tauriMock.ts` is the one mock (P-011) other features' specs reuse.
- **Rationale**: the UI is tested on the Linux host and in CI without Rust; the mock implements exactly the IPC contract, so contract drift shows up as a failing spec.
- **Alternatives**: tauri-driver on the Windows runner (no kit adapter, decision in requirements §9).
