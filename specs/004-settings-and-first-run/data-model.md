# Data Model: Settings and First Run

**Feature**: [spec.md](./spec.md) · **Research**: [research.md](./research.md)

All types live in `crates/voicen-core` (module `settings`, `secrets`, `i18n`) unless stated. Field names are the serialized names (snake_case JSON). `FieldId`s are dotted strings used by validation errors, the UI highlight and log lines.

## Settings (persisted in `settings.json`; one definition, P-010)

| Field | Type | Default (spec FR-005) | Validation (spec FR-004) | FieldId | Req |
|---|---|---|---|---|---|
| `schema_version` | u32 | `1` | `> 1` (newer than known) → file unreadable (research R-2); `0` loads | — | FR-13 |
| `engine` | `none` \| `api` \| `builtin_local` \| `local_server` | `none` | — | `engine.kind` | FR-13, FR-21 |
| `api.base_url` | string | `https://api.openai.com/v1` | engine = api: non-empty, valid URL (R-8) | `engine.api.base_url` | FR-13 |
| `api.model` | string | `whisper-1` | engine = api: non-empty after trim | `engine.api.model` | FR-13 |
| `local_server.base_url` | string | `http://localhost:8000/v1` | engine = local_server: non-empty, valid URL | `engine.local_server.base_url` | FR-13, FR-17 |
| `local_server.model` | string | `""` | optional (002 FR-017) | `engine.local_server.model` | FR-13 |
| `builtin_local.model_id` | string \| null | `null` | engine = builtin_local: non-null and downloaded (from 002's `ModelStore`) | `engine.builtin_local.model_id` | FR-13, FR-07 |
| `speech_language` | string \| null | `null` (auto-detect) | null or a code from the Whisper language list | `engine.speech_language` | FR-13 |
| `microphone` | `{id, name}` \| null | `null` (Windows default) | — (a missing device is not an error, req FR-27) | `recording.microphone` | FR-13 |
| `hotkey` | string, canonical (`Ctrl+Alt+Space`) | `Ctrl+Alt+Space` | canonical text only (see Hotkey); ≥ 1 modifier, one key from the set of R-6, no `Esc`; registrable | `recording.hotkey` | FR-13, FR-05, FR-22 |
| `mode` | `hold` \| `toggle` | `hold` | — | `recording.mode` | FR-13, FR-02 |
| `auto_paste` | bool | `true` | — | `output.auto_paste` | FR-13, FR-10 |
| `post_processing` | `PostProcessingSettings` (defined in core by 004; fields per 003 data-model) | its `defaults()`: off, `""`, `""`, starter prompt | 003's `validate()` when enabled | `post_processing.*` | FR-13, FR-09 |
| `history.enabled` | bool | `true` | — | `history.enabled` | FR-13, FR-16 |
| `history.size` | u32 | `20` | whole number 1–100 | `history.size` | FR-13, FR-16 |
| `start_with_windows` | bool | `false` | applied in save (R-3); failure refuses | `general.start_with_windows` | FR-19 |
| `ui_language` | `en` \| `ru` | `resolve_ui_language(os_tag)` | — | `general.ui_language` | FR-15, FR-21 |

Rules:
- `Settings` has **no key field**; it derives `Serialize`/`Deserialize`; the container has `#[serde(default)]` built from `defaults()` (not the field type's `Default`), so a missing field takes its value from `defaults()` and older files load (R-2).
- The container-level default applies to each nested struct too (`api`, `local_server`, `builtin_local`, `history`, `post_processing`): a partial nested object takes its missing fields from `defaults()`, never from the field type's `Default` (serde's field-level `#[serde(default)]` would; decisions/settings.md).
- Fields of engines (and post-processing) that are not selected are kept and not validated.
- Normalization before validation and storage: URLs trimmed and one trailing `/` removed; models trimmed.
- `defaults(os_tag)` is the one function that builds the defaults; it calls `post_process::settings::defaults()` (data type created by 004's foundational phase; 003 adds only `validate()`) and `resolve_ui_language` (`i18n.rs`, teamwright T-005).

## KeySlot and Secret (never persisted in the settings file)

- Defined by 004 in core and used by 001–003 (decisions #21). `KeySlot`: `TranscriptionApi` (target `Voicen/transcription-api`), `LocalServer` (`Voicen/local-server`), `PostProcessing` (`Voicen/post-processing`).
- Key FieldIds (decisions #25a), for `key.required` and `key.store_failed`: `engine.api.key`, `engine.local_server.key`, `post_processing.key`. They name inputs, not `Settings` fields.
- `Secret(String)`: no `Serialize`; `Debug`/`Display` print `***`; zeroed on drop.
- `KeyEdit` (per slot, in a save or test request): `Untouched` | `Replace(Secret)` | `Clear`. Validation of `engine = api` requires `Replace` or (`Untouched` and the slot holds a key). `Replace` with an empty key counts as entered (decisions #26).
- `KeyPresence`: `{transcription_api: bool, local_server: bool, post_processing: bool}` — the only key information sent to a window.

## SettingsView (what a window receives)

`{ settings: Settings, keys: KeyPresence, first_run: bool, reset_notice: bool }`. Sent by `settings_get` and the `settings://changed` event. Never contains a key.

## SaveRequest / SaveOutcome

- `SaveRequest { settings: Settings, keys: {transcription_api: KeyEdit, local_server: KeyEdit, post_processing: KeyEdit} }`.
- `SaveOutcome`:
  - `Saved { view: SettingsView, warnings: [Warning] }` — `Warning { field: FieldId, code: "endpoint.insecure" }`.
  - `Refused { errors: [FieldError], form_error: Option<MessageRef> }` — nothing changed (or, after a double failure, `form_error = settings.partially_restored` naming the step).
- `FieldError { field: FieldId, code }` with codes: `required`, `url.malformed`, `key.required`, `model.not_downloaded`, `hotkey.no_modifier`, `hotkey.no_key`, `hotkey.esc_reserved`, `hotkey.invalid` (any other grammar error: unknown token, second key, empty part, repeated or out-of-order modifier; decisions #25c), `hotkey.unavailable`, `history.size_range`, `autostart.failed`, `key.store_failed`, plus 003's post-processing codes. Each code maps to one message id `error.<code>`.

### Save state machine (research R-3)

```text
Validate ──errors──▶ Refused
   │ok
HotkeyPrepared? ──fail──▶ Refused(hotkey.unavailable)
   │
AutostartApplied? (US5 only; skipped before) ──fail──▶ undo hotkey ▶ Refused(autostart.failed)
   │
KeysApplied ──fail──▶ undo keys so far, autostart, hotkey ▶ Refused(key.store_failed)
   │
FileWritten ──fail──▶ undo keys, autostart, hotkey ▶ Refused(form: settings.write_failed)
   │
Committed (old hotkey released, snapshot swapped, subscribers notified) ▶ Saved(warnings)
```

## LoadOutcome (startup)

`Loaded(Settings)` | `FirstRun(Settings)` (no file; defaults written) | `Reset { settings, backup_file_name }` (unreadable file moved aside; defaults written) | `Unavailable(Settings)` (the file cannot be moved aside, or a read fails with an I/O error other than not-found: defaults in memory; the file is never written or moved; `save` is refused with a notice until restart; the credential store is not touched — decisions #19). `startup_action` for `Unavailable` is `OpenSettings(Engine)` as for `Reset`, with a notice. `FirstRun` and `Reset` open the settings window on the Engine tab; `Reset` also notifies `notice.settings_reset`. The decision is the pure function `startup_action(outcome, launched_by_autostart)`: `OpenSettings(Engine)` for `FirstRun`/`Reset`, `TrayOnly` for `Loaded`, regardless of `--autostart` (spec US5-3); the startup hotkey-failure branch is 001's.

## ConnectionTestRequest / ConnectionTestResult

- Request: `{ engine: api | local_server, base_url, model, key: KeyEdit }` — form values, unsaved included.
- Result: `Ok { latency_ms }` | `CannotReach { host }` | `InvalidKey` | `Timeout` | `Http { status }` | `UnexpectedResponse` | `Invalid { errors: [FieldError] }` (the form values cannot form a request, e.g. malformed URL). Never contains a key or the response body.

## Message catalog

- Files `i18n/en.json`, `i18n/ru.json`: `{ "<message id>": "<text with {placeholders}>" }`.
- Invariants (gate, T-005 tests only): flat string map; same id set in both; every text non-empty; same placeholder names per id; no brace outside a placeholder `{[a-z][a-z0-9_]*}`; every Rust `MessageId` (`MESSAGE_IDS`) exists in both; in the UI a literal id in `t()` is checked by `svelte-check` (`MessageId = keyof typeof en`).
- Rendering: one pass, values inserted literally, missing argument stays `{name}`, extra arguments ignored; lookup lang → `en` → the id. Pinned by `i18n/conformance.json`, run by core and UI tests.
- Ids are added to both files together; Rust-originated ids are also declared with `messages!` in `i18n.rs`.
- `UiLanguage`: `en` | `ru`. `resolve_ui_language(Option<&str>)`: primary subtag (before the first `-` or `_`) `ru` in any ASCII case → `ru`, else (including `None` and empty) `en`. The UI gets its language from `settings.ui_language` and starts with `en`.

## Hotkey

- `Hotkey { ctrl, alt, shift, win, key: HotkeyKey }`; canonical string form; `HotkeyKey` closed set of 82 keys (research R-6): `A`–`Z`, `0`–`9`, `F1`–`F24`, `Space`, `Insert`, `Delete`, `Home`, `End`, `PageUp`, `PageDown`, arrows `Up`/`Down`/`Left`/`Right`, `Pause`, numpad `Num0`–`Num9`. No numpad operator keys and no `Esc` (decisions #26).
- Parsing is strict: only the canonical text is accepted (exact case, modifiers in the order `Ctrl`, `Alt`, `Shift`, `Win`, each once, the key last), so `parse_hotkey(s)?.to_string() == s` for every accepted `s`. Anything else is `hotkey.invalid` (decisions #26); `hotkey.no_modifier`, `hotkey.no_key`, `hotkey.esc_reserved` keep their own codes. The grammar is `voicen_core::settings::hotkey` (decisions #23 N1).
- `HotkeyRegistrar` (trait defined by 004 with a fake; implemented by 001): `prepare(Hotkey, Mode) -> Result<Prepared, Unavailable>`, `commit(Prepared)`, `abort(Prepared)`.

## Autostart

- `Autostart` (trait, save step and reconcile added by US5, tasks T056–T058): `is_enabled() -> Result<bool>`, `set(bool) -> Result<(), AutostartError>`. Windows value `HKCU\…\Run\Voicen = "<exe>" --autostart`.
