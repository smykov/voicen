# Contract: IPC commands and events (src-tauri ↔ UI)

The UI never calls Windows APIs; Playwright e2e mocks exactly these commands and events via `window.__TAURI_INTERNALS__`. Names follow `snake_case` commands and `local-model://…` events. Engine selection, the selected model, the local-server URL/model and the speech language are saved through 004's settings commands; the local-server key through 004's key commands with slot `local-server`. Those are not redefined here.

## Types (TypeScript view)

```ts
type ModelId = 'tiny' | 'base' | 'small' | 'medium-q5_0' | 'large-v3-turbo-q5_0';

type ModelState =
  | { kind: 'not_downloaded' }
  | { kind: 'downloading'; received: number; total: number }
  | { kind: 'downloaded' }
  | { kind: 'failed'; reason: FailureReason };          // retry offered

interface LocalModelView {
  id: ModelId;
  nameKey: string;          // i18n key
  sizeBytes: number;
  recommended: boolean;     // true only for 'small'
  state: ModelState;
  loaded: boolean;          // currently in memory
}

interface FailureReason { code: string; messageKey: string; params?: Record<string, string | number> }
// codes added by this feature: download_interrupted, checksum_mismatch, not_enough_disk_space{needed},
// source_unreachable{host}, disk_error, http_status{code}, model_in_use, delete_failed, no_local_model
```

## Commands

| Command | Args | Returns | Errors (`FailureReason.code`) |
|---|---|---|---|
| `local_models_list` | — | `LocalModelView[]` (always five, catalog order) | — |
| `local_model_download` | `{ id }` | `void` (progress via events) | `download_busy`, `already_downloaded`, `not_enough_disk_space` |
| `local_model_cancel_download` | `{ id }` | `boolean` (true if a download was cancelled) | — |
| `local_model_delete` | `{ id }` | `{ engineReset: boolean }` (true when the engine became "none") | `model_in_use`, `delete_failed`, `not_downloaded` |

Retry = `local_model_download` again.

## Events (shell → UI)

| Event | Payload | When |
|---|---|---|
| `local-model://progress` | `{ id, received, total }` | ≥ 1/s and ≤ 4/s while data arrives |
| `local-model://state` | `{ id, state: ModelState }` | every state transition (finished, failed with reason, cancelled, deleted) |

## Notification action (shell-internal, listed for completeness)

"no local model" notification action → opens the settings window on the Engine tab (004's `open_settings{tab:'engine'}`).
