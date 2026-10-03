# Contract: Tauri IPC (UI ↔ shell)

The UI calls only these commands and listens only to these events for this feature. The Playwright mock (`e2e/support/tauriMock.ts`) implements exactly this contract. Payloads are JSON with the types of [data-model.md](../data-model.md). No payload sent **to** a window ever contains a key.

## Commands

| Command | Args | Returns | Notes |
|---|---|---|---|
| `settings_get` | — | `SettingsView` | current saved settings + key presence + first-run/reset flags |
| `settings_save` | `{ request: SaveRequest }` | `SaveOutcome` | all-or-nothing; `KeyEdit.Replace` carries the typed key once, UI → shell only |
| `settings_speech_languages` | — | `string[]` | core's `WHISPER_ISO_639_1` in core order (97 codes, #30); the speech-language picker's list, never re-spelled in the UI (T-030) |
| `settings_test_connection` | `{ request: ConnectionTestRequest }` | `ConnectionTestResult` | never changes settings or keys; at most one in flight per window (UI disables the button) |
| `settings_list_microphones` | — | `[{ id, name, is_default }]` | provided by 001's capture enumeration; the saved microphone is shown "(not connected)" when absent |
| `local_models_list` (002) | — | 002's model list | used by the Engine tab for the built-in model selection |

Errors: a command that cannot run at all rejects with `{ code: "ipc.unavailable" }`; the UI shows `error.ipc_unavailable` and keeps the draft.

Built (T-030): `settings_get`, `settings_save` (runs off the main thread), `settings_speech_languages`, and the `settings://changed` event. Malformed `settings_save` args reject with Tauri's `invalid args` text followed by a fixed message (`invalid save request: ...`, `invalid key edits: ...`, `invalid key edit: ...`, `invalid key: ...`) that never quotes the input, so a mistyped key never reaches a window (T-030 J1).

## Wire form (serde impls in `voicen_core`, pinned by core tests on the Linux gate)

| Type | Direction | JSON |
|---|---|---|
| `SaveRequest` | UI → shell | `{ "settings": Settings, "keys": KeyEdits }` |
| `KeyEdits` | UI → shell | `{ "transcription_api": KeyEdit, "local_server": KeyEdit, "post_processing": KeyEdit }`; all three required, unknown fields ignored |
| `KeyEdit` | UI → shell | `"Untouched"` \| `"Clear"` \| `{ "Replace": "<key>" }` (the key unchanged; the service trims it) |
| `SettingsView` | shell → UI | `{ "settings": Settings, "keys": { "transcription_api": bool, "local_server": bool, "post_processing": bool }, "first_run": bool, "reset_notice": bool }` |
| `SaveOutcome` | shell → UI | `{ "Saved": { "view": SettingsView, "warnings": [Warning] } }` \| `{ "Refused": { "errors": [FieldError], "form_error": FormError \| null } }` |
| `FieldError` | shell → UI | `{ "field": FieldId, "code": ErrorCode }`; the UI shows `error.<code>` |
| `Warning` | shell → UI | `{ "field": FieldId, "code": "endpoint.insecure" }` |
| `FormError` | shell → UI | `{ "kind": "write_failed" \| "settings_unavailable" \| "partially_restored", "message": MessageId, "not_restored"?: [FieldId] }`; `not_restored` only on `partially_restored`; `message` is `FormError::message_id()` (`settings.write_failed`, `notice.settings_unavailable`, `settings.partially_restored`) |
| `FieldId` / `ErrorCode` / `MessageId` | shell → UI | the dotted string (`"engine.api.base_url"`, `"url.malformed"`, `"settings.write_failed"`): `as_str()` / the catalog id, the one spelling |

## Events (shell → windows)

| Event | Payload | When |
|---|---|---|
| `settings://changed` | `SettingsView` (`SettingsService::view()`) | after every `Saved`, from any caller, exactly once; never after a `Refused`. Emitted only by the shell's subscribe bridge (`settings_ipc::spawn_change_bridge`, T-030), not by `settings_save`. All open windows re-render, and switch language if `ui_language` changed. After a startup reset no event is sent (no window exists yet); the window reads `reset_notice` through `settings_get` |
| `settings://focus` | `{ tab: "engine"\|"recording"\|"output"\|"post_processing"\|"history"\|"general", field?: FieldId }` | the settings window is requested while already open (spec FR-002) |

## Window

- Label `settings`, URL `settings?tab=<tab>[&field=<FieldId>]`, single instance, created on demand and destroyed on close.
- Close request: the UI calls `onCloseRequested`; with unsaved changes it prevents the close and shows the discard dialog; otherwise the window closes.

## Example (`settings_save` refused at validation: all field errors at once; OS steps are not attempted)

```json
{ "Refused": { "errors": [ { "field": "engine.api.base_url", "code": "url.malformed" },
                            { "field": "history.size", "code": "history.size_range" } ],
               "form_error": null } }
```
