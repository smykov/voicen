# Research: Transcript History (005)

The stack is fixed by `docs/requirements.md` §9 (Tauri 2, Rust 1.99, Svelte 5, Playwright with mocked IPC). This document records only **how** history is built inside it. Each item: Decision / Rationale / Alternatives considered.

## R1. Where the history logic lives

- **Decision**: All history state and rules (record, trim, clear, turn off/on, start-up cleanup, deletion fallback, failure notice) live in `voicen-core` as a `History` type over a `HistoryStore` trait. The file implementation of the store (`FileHistoryStore`) also lives in `voicen-core`: it uses only `std::fs`, so it runs and is tested on Linux against a temporary directory. `src-tauri` only owns the instance, wires it to the delivery seam, the settings apply, the clipboard, the tray item, the window and the change event.
- **Rationale**: Constitution IV puts history in core and requires Linux testability; decisions #5 means anything in `src-tauri` is proven only on the Windows runner, so the shell part must stay thin. `std::fs` has no Windows-only code.
- **Alternatives**: store in `src-tauri` (logic only testable on Windows CI — rejected); SQLite (`rusqlite`) — a native dependency for at most 100 rows, and "delete" of a row leaves the text in free pages/WAL until vacuum, which weakens SC-003 — rejected.

## R2. Storage format and file

- **Decision**: One JSON file `history.json` directly in the data folder (`%LOCALAPPDATA%\Voicen\history.json`), path given to `FileHistoryStore` by the shell's single data-directory resolver (P-010, architecture "Cross-cutting values"). Document: `{ "schema": 1, "next_id": <u64>, "entries": [ … newest first … ] }`, serialized with `serde_json` (MIT/Apache-2.0, NFR-12). UTF-8, no encryption (FR-16 accepts it).
- **Rationale**: ≤ 100 entries × ~15 000 characters → < ~2 MB; rewriting the whole file per change is cheap and makes "the stored data is exactly the current list" trivially true, so SC-003 (no old text left after Clear/off/trim) holds by construction. `serde` is already a core dependency.
- **Alternatives**: append-only log (old text survives until compaction — breaks SC-003); one file per entry (more files to delete, more partial states); binary format (no benefit at this size, harder to inspect in the owner's manual check).

## R3. Never half-written (FR-004, crash consistency)

- **Decision**: Write to `history.json.tmp` in the same folder, `sync_all`, then `std::fs::rename` over `history.json` (on Windows `rename` maps to `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`, atomic on NTFS for the same volume). At start and on every deletion, a leftover `history.json.tmp` is deleted too (it can hold transcript text).
- **Rationale**: Either the previous or the new complete document is on disk at any instant. Deleting the temp file is required for SC-003 and FR-009 ("including an interrupted partial write").
- **Alternatives**: in-place overwrite (torn file after a crash); `tempfile` crate's persist (extra dependency for one rename).

## R4. Deletion and its failure (FR-007/FR-008/FR-009)

- **Decision**: "Delete" = remove `history.json` and `history.json.tmp`. If removing `history.json` fails, fall back to an atomic replace with the state it should hold (empty document for Clear/off; trimmed list for trim). If both fail, the in-memory state is authoritative for the session, a `delete_failed` notice is set, and every later change (record, clear, apply settings) retries the pending disk operation first. At start, when history is off, both files are deleted.
- **Rationale**: A failed Clear while history is on leaves nothing on disk that a later start could act on (a marker file would fail on the same unwritable disk), so the spec (FR-009) was made explicit: retry at every later change; at start only the "off" case can be enforced. The fallback replace covers the common "file locked by a scanner/indexer for delete but writable via rename" case.
- **Alternatives**: a persistent "pending clear" marker in the settings file (owned by 004, same disk, couples features — rejected); refusing to clear in memory until disk succeeds (violates US2-AS3: list is emptied in the app).

## R5. Time and engine of an entry

- **Decision**: `ready_at` is UTC milliseconds since the Unix epoch (`i64`), supplied by the caller at the delivery seam (the moment the final text was ready), not read by `History` itself — so tests control time without a clock trait. `engine` = `{ kind: "api" | "local" | "server", model: String }` captured from the settings the dictation ran with (004 edge case: a running dictation keeps its settings). Order is insertion order; ids are a persisted, strictly increasing `u64` (`next_id`), so a backwards clock change does not reorder anything.
- **Rationale**: Avoids a `chrono`/`time` dependency in core; the UI formats with `Intl.DateTimeFormat` in local time and the UI language (FR-005).
- **Alternatives**: RFC 3339 string (needs a date crate or hand formatting); sort by time (breaks on clock changes).

## R6. Hook into the pipeline without a parallel path (P-011)

- **Decision**: 001's delivery step is the only caller of `History::record`. It calls it once per dictation job whose outcome is "delivered" or "copied-only" with a non-empty (after trim of whitespace) final text, **after** the clipboard write/paste attempt has completed, on the pipeline worker thread. `record` never returns an error to the caller: failures turn into the notice and a log line (FR-010).
- **Rationale**: Delivery is not delayed by the disk write (NFR-01 timings are measured to paste); history can never fail delivery (NFR-07). One call site keeps FR-001's "every non-empty final text that reaches delivery" checkable at one seam.
- **Alternatives**: record before paste (adds disk latency to stop→paste); a separate event bus subscriber (a second path that could diverge from what was delivered).

## R7. Concurrency and live updates

- **Decision**: The shell holds `Mutex<History>` in Tauri managed state. After every change, the shell emits one Tauri event `history-changed` with the full `HistoryView` to the History window if it exists. The window loads with `history_get` and then replaces its state with each event payload.
- **Rationale**: ≤ 2 MB payload, at most a few changes per minute; sending the full view avoids diff bugs and makes "the window shows exactly the stored state" one assertion. A mutex is enough: operations are short and never nested.
- **Alternatives**: incremental events (entry-added / entry-removed) — more states to test; polling — wasteful and laggy.

## R8. The History window (FR-005, FR-015, NFR-03)

- **Decision**: A separate Tauri `WebviewWindow` with label `history` that loads the SPA route `/history`, created by the shell function `show_history_window()` (called from 001's tray item) and destroyed on close. If a window with that label exists, it is shown, unminimized and focused instead.
- **Rationale**: NFR-03 idle RAM; FR-005 "bring to front". Tauri's label uniqueness gives the single-window guarantee.
- **Alternatives**: hide instead of destroy (keeps a webview resident — violates FR-015).

## R9. Copy (FR-006, FR-018)

- **Decision**: IPC command `history_copy(id)` looks the entry up in core and writes its text through 001's clipboard writer (the same one dictation delivery uses, with the "exclude from clipboard history" and "can upload to cloud clipboard = 0" formats). It never sends Ctrl+V. Errors map to `not_found` (entry gone) or `clipboard_unavailable`.
- **Rationale**: P-011 one clipboard path; the exclusion marks are 001's invariant, reused, not reimplemented.
- **Alternatives**: `navigator.clipboard` in the webview (skips the exclusion marks — breaks req FR-10/r1#28).

## R10. Logging without text (FR-011, FR-20, P-009)

- **Decision**: `HistoryError` carries only the operation (`load`/`save`/`delete`) and `std::io::ErrorKind` (or `corrupt`), plus counts; it has no field that can hold text, and its `Display` cannot include it. Serde errors from a corrupt file are reduced to `corrupt` (serde_json error messages may quote input). The shell logs only `HistoryError` values and counts.
- **Rationale**: Makes "no text in logs" structural; a test asserts that the `Display`/`Debug` of every error produced while handling entries with a sentinel text does not contain the sentinel.
- **Alternatives**: logging `io::Error` / `serde_json::Error` directly (serde_json messages can echo document fragments — rejected).

## R11. Settings dependency (004)

- **Decision**: History consumes `history.enabled` and `history.size` from 004's settings model through 004's "apply to subscribers" mechanism: `History::apply_settings(enabled, size)`. The range 1–100 and the defaults (on, 20) are 004's constants; core history asserts `size >= 1` defensively and clamps nothing else. The "Open settings" button calls 004's open-settings command with the History tab.
- **Rationale**: P-010 one source for range and defaults; P-013 the value must reach the runtime — a test drives the real settings apply path into `History`.
- **Alternatives**: history reading the settings file itself (a second loader — rejected).

## R12. UI text and time format (FR-013, FR-005)

- **Decision**: All strings via 004's message catalog (keys `history.*`); times via `Intl.DateTimeFormat(uiLanguage, { timeStyle: "short" })` for today and `{ dateStyle: "medium", timeStyle: "short" }` otherwise, in the system time zone. Preview: first 3 lines or 200 characters, whichever is shorter, with an expand toggle (`aria-expanded`).
- **Rationale**: one catalog (004 FR-012 makes a missing RU key fail the gate); `Intl` needs no dependency and is testable with a fixed time zone in vitest/Playwright.
- **Alternatives**: a date library (unneeded weight).

## R13. Verification placement

- **Decision**: core rules and file behaviour → Linux host, `cargo test -p voicen-core` with `tempfile`-style temp dirs (std `env::temp_dir` + unique subfolder, or the `tempfile` dev-dependency) and a fault-injecting fake store; UI states → vitest + Playwright with mocked IPC (`window.__TAURI_INTERNALS__`, including a mocked `history-changed` event); shell wiring (tray item, window, real clipboard marks, real `%LOCALAPPDATA%` path, rename on NTFS) → Windows CI `cargo test --workspace` plus the owner's manual check (requirements §9, UI verification row).
- **Rationale**: constitution V; decisions #5.

No NEEDS CLARIFICATION remains.
