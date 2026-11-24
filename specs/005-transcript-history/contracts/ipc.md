# Contract: History IPC (UI ↔ `src-tauri`) and shell hooks

The UI talks to Rust only through these commands and one event (architecture "Seams"). Playwright mocks exactly these. Shapes use camelCase on the wire (`#[serde(rename_all = "camelCase")]`): core's `HistoryView` is converted by the shell's `HistoryWire` / `HistoryEntryWire` (`src-tauri/src/history.rs`); core and the stored file stay snake_case.

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
| `history_copy` | `{ id: number }` | `null` | `{ code: "not_found" }` entry no longer in the list (the UI shows the same "Could not copy — try again" alert; the next `history://changed` already removed the entry); `{ code: "clipboard_unavailable" }` clipboard write failed | FR-006, FR-018 |
| `history_clear` | — | `HistoryView` (after clearing; `notice` may be `delete_failed`) | — | FR-007, FR-009 |

Opening settings at the History tab uses **004's** open-settings request with the History tab; 004 defines no UI command for it, so T-022 added `settings_open { tab }` -> `null` (`settings_window::settings_open`, in `APP_COMMANDS`, granted to the `history` label only): `tab` is a `SettingsTab::as_str` token, an unknown one answers `{ code: "unknown_tab" }` and posts nothing, otherwise it posts `request(OpenTarget::Tab(tab, None))` (the path of `ShellRequests::open_settings`) and returns at once. Copy uses **001's** clipboard writer (exclusion marks), never a paste.

## Event

| Event | Payload | Emitted when | Req |
|---|---|---|---|
| `history://changed` | `HistoryView` | after every `record`, `clear`, `apply_settings` and retried disk operation that changed the view or notice; only to the window labelled `history`, if open | FR-005 (live update) |

The UI replaces its whole state with each payload (no merging).

## Shell hooks (Rust, `src-tauri`, Windows CI only; as built, T-022)

The planned `history_on_delivered` / `history_on_settings_applied` functions do not exist: delivery and the settings follower are core (`TranscriptSink`, `follow_settings`; core-history.md), and the shell only wires them.

| Function | Called by | Does | Req |
|---|---|---|---|
| `history::open_history(app)` | the tray item "History" (`TrayAction::OpenHistory`, id `history`, between Settings and Open logs) | posts to the "history-window" opener thread and returns at once; that thread builds `WebviewWindow` label `history` at `/history`, or unminimizes, shows and focuses the existing one; closing destroys it | FR-005, FR-015 |
| `history::start_history(app, dir, clipboard)` | `run()`, before the dictation session | `History::open(FileHistoryStore::new(dir), enabled, size)` from the settings snapshot (read after the follower's `subscribe()`), a `HistoryRecorder` whose `on_change` is `emit_to("history", "history://changed", HistoryWire)` and whose `on_error` writes `history_failed op=.. kind=..`, the "history-settings" thread running `follow_settings`, the opener thread; manages the part. `dir` is `paths::data_dir()`. An open error is logged the same way | FR-004, FR-009, FR-016, FR-010 |
| `history::transcript_sink(app)` | `dictation::start_dictation_with`, into `PipelineDeps::transcript_sink` | the managed recorder, or a sink that keeps nothing when the app has no history part (test apps) | FR-001 |
| `history_get` / `history_copy` / `history_clear` | the History page | memory read; `text_of` then the process's one `Clipboard::set_text_excluded_from_history` (errors: code only); `clear`. Without a history part: an empty view (`enabled` from the settings), `not_found` | FR-005, FR-006, FR-007 |

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
