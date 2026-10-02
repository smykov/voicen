# Contract: Tauri IPC (UI ↔ shell)

The UI calls only these commands and listens only to these events for this feature. The Playwright mock (`e2e/support/tauriMock.ts`) implements exactly this contract. Payloads are JSON with the types of [data-model.md](../data-model.md). No payload sent **to** a window ever contains a key.

## Commands

| Command | Args | Returns | Notes |
|---|---|---|---|
| `settings_get` | — | `SettingsView` | current saved settings + key presence + first-run/reset flags |
| `settings_save` | `{ request: SaveRequest }` | `SaveOutcome` | all-or-nothing; `KeyEdit.Replace` carries the typed key once, UI → shell only |
| `settings_test_connection` | `{ request: ConnectionTestRequest }` | `ConnectionTestResult` | never changes settings or keys; at most one in flight per window (UI disables the button) |
| `settings_list_microphones` | — | `[{ id, name, is_default }]` | provided by 001's capture enumeration; the saved microphone is shown "(not connected)" when absent |
| `local_models_list` (002) | — | 002's model list | used by the Engine tab for the built-in model selection |

Errors: a command that cannot run at all rejects with `{ code: "ipc.unavailable" }`; the UI shows `error.ipc_unavailable` and keeps the draft.

## Events (shell → windows)

| Event | Payload | When |
|---|---|---|
| `settings://changed` | `SettingsView` | after every successful save and after a reset; all open windows re-render, and switch language if `ui_language` changed |
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
