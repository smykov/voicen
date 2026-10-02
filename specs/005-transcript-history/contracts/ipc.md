# Contract: History IPC (UI ↔ `src-tauri`) and shell hooks

The UI talks to Rust only through these commands and one event (architecture "Seams"). Playwright mocks exactly these. Shapes use camelCase on the wire (`#[serde(rename_all = "camelCase")]`).

## Wire types

```ts
type EngineKind = "api" | "local" | "server";
interface HistoryEntry { id: number; text: string; readyAtMs: number; engine: { kind: EngineKind; model: string } }
type HistoryNotice = "save_failed" | "delete_failed";
interface HistoryView { enabled: boolean; entries: HistoryEntry[]; notice: HistoryNotice | null }
```

`id` is a u64 in Rust; values stay far below 2^53, so a JS number is exact.

## Commands

| Command | Args | Returns | Errors | Req |
|---|---|---|---|---|
| `history_get` | — | `HistoryView` | — (never fails; reads memory only, never the disk) | FR-005, FR-012 |
| `history_copy` | `{ id: number }` | `null` | `{ code: "not_found" }` entry no longer in the list (the UI shows the same "Could not copy — try again" alert; the next `history-changed` already removed the entry); `{ code: "clipboard_unavailable" }` clipboard write failed | FR-006, FR-018 |
| `history_clear` | — | `HistoryView` (after clearing; `notice` may be `delete_failed`) | — | FR-007, FR-009 |

Opening settings at the History tab uses **004's** open-settings command with the History tab (not defined here). Copy uses **001's** clipboard writer (exclusion marks), never a paste.

## Event

| Event | Payload | Emitted when | Req |
|---|---|---|---|
| `history-changed` | `HistoryView` | after every `record`, `clear`, `apply_settings` and retried disk operation that changed the view or notice; only to the window labelled `history`, if open | FR-005 (live update) |

The UI replaces its whole state with each payload (no merging).

## Shell hooks (Rust, `src-tauri`, Windows CI only)

| Function | Called by | Does | Req |
|---|---|---|---|
| `show_history_window(app)` | 001's tray item "History" | create `WebviewWindow` label `history`, route `/history`; if it exists: unminimize, show, focus | FR-005, FR-015 |
| `history_on_delivered(app, text, ready_at_ms, engine)` | 001's delivery step, once per job with outcome delivered/copied-only, after the clipboard/paste attempt | `History::record`, log error kind if any, emit `history-changed` | FR-001, FR-010 |
| `history_on_settings_applied(app, enabled, size)` | 004's settings apply (subscriber) | `History::apply_settings`, emit | FR-003, FR-008 |
| startup | app setup | `History::open(FileHistoryStore::new(data_dir()), …)` with the data dir from the single resolver | FR-004, FR-009, FR-016 |

## UI contract (Playwright selectors)

| Element | Accessible name / role | Notes |
|---|---|---|
| list | `role=list`, name "History" (catalog) | entries newest first |
| entry | `role=listitem` | preview, time, engine label "API · whisper-1" |
| expand | `role=button`, `aria-expanded` | only when the text is longer than the preview |
| copy | `role=button`, name "Copy" | then `role=status` "Copied" for 2 s; on error `role=alert` "Could not copy — try again" |
| clear | `role=button`, name "Clear" | no confirmation |
| empty state | text "No transcripts yet" | enabled, 0 entries |
| off state | text "History is off" + `role=button` "Open settings" | `enabled=false` |
| notice | `role=alert` "History could not be saved to disk" / "History could not be deleted from disk" | while `notice` set |
