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
// source_unreachable{host}, disk_error, http_status{code}, model_in_use, delete_failed, no_local_model,
// and for a refused download: download_busy, already_downloaded, not_in_catalog, download_cannot_start (T-044)
```

## Commands

| Command | Args | Returns | Errors (`FailureReason.code`) |
|---|---|---|---|
| `local_models_list` | — | `LocalModelView[]` (always five, catalog order) | — |
| `local_model_download` | `{ id }` | `void` (progress via events) | `download_busy`, `already_downloaded`, `not_enough_disk_space{needed}`, `not_in_catalog`, `download_cannot_start` |
| `local_model_cancel_download` | `{ id }` | `boolean` (true if a download was cancelled; `false` for an id that is not running or not in the catalog) | — |
| `local_model_delete` | `{ id }` | `{ engineReset: boolean }` (true when the engine became "none") | `model_in_use`, `delete_failed`, `not_downloaded` |

Retry = `local_model_download` again.

A refused `local_model_download` rejects with a `FailureReason`, changes no listed state and emits no event. The one mapping from core's `DownloadError` (contracts/core-traits.md "Downloader"; `voicen_core::local_models::service::ReasonView`, T-044):

| Core value | `code` | `messageKey` | `params` |
|---|---|---|---|
| `Busy` (any download running) | `download_busy` | `download.busy` | — |
| `AlreadyDownloaded` | `already_downloaded` | `download.already_downloaded` | — |
| `NotEnoughDiskSpace { needed }` (size + 1 %) | `not_enough_disk_space` | `download.not_enough_disk_space` | `needed` |
| `NotInCatalog`, or an `id` that is not a `ModelId` string | `not_in_catalog` | `download.not_in_catalog` | — |
| `CannotStart` (the OS refused the download thread) | `download_cannot_start` | `download.cannot_start` | — |

A `failed` state's `reason` uses the `DownloadFailure` codes above with message keys `download.<…>` (`download_interrupted` → `download.interrupted`, the others `download.<code>`).

## Wire form (T-044)

Serialized by serde impls in `voicen-core` (`local_models::service`); `e2e/fixtures/local-models-wire.json` holds the core-checked values for the Playwright mock.

- `LocalModelView` fields: `id`, `nameKey` (= `local_model.name.<id>`, en/ru in `i18n/`), `sizeBytes`, `recommended`, `state`, `loaded` (always `false` until T-017's residency).
- `ModelState`: `{ "kind": "not_downloaded" | "downloading" | "downloaded" | "failed" }`, plus `received` and `total` for `downloading`, `reason` for `failed`.
- `FailureReason`: `code`, `messageKey`, and `params` only when there are any; param values are strings (`{ "needed": "66256" }`, `{ "code": "503" }`, `{ "host": "huggingface.co" }`).
- `Downloading` and `Failed` are in memory only: a restart lists them as `not_downloaded`. `failed` stays until a retry.

## Events (shell → UI)

| Event | Payload | When |
|---|---|---|
| `local-model://progress` | `{ id, received, total }` | ≥ 1/s and ≤ 4/s while data arrives |
| `local-model://state` | `{ id, state: ModelState }` | every state transition (finished, failed with reason, cancelled, deleted) |

Both are emitted from the download thread to the settings window only (`emit_to(EventTarget::webview_window("settings"))`), after the listed state is updated, so a `local_models_list` after an event agrees with it. A cancel ends in `state: { kind: 'not_downloaded' }` (no reason).

## Notification action (shell-internal, listed for completeness)

"no local model" notification action → opens the settings window on the Engine tab (004's `open_settings{tab:'engine'}`).
