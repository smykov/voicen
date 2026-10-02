# Implementation Plan: Transcript History

**Branch**: `005-transcript-history` (spec directory; work happens on `main`) | **Date**: 2026-10-02 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/005-transcript-history/spec.md`

## Summary

Keep the last N delivered final transcripts (text, time, engine) in `%LOCALAPPDATA%\Voicen\history.json` while history is on; show them newest first in an on-demand History window opened from the tray; copy any entry again through the shared clipboard writer; clear with one click; and when history is turned off, delete the stored data and write nothing more (req FR-16, NFR-06). The rules live in a `voicen-core` `History` type over a `HistoryStore` trait with a std-only atomic JSON file store, tested on Linux; `src-tauri` wires it to 001's delivery seam and tray item, 004's settings apply, and three IPC commands plus one change event; the Svelte `/history` route renders it (research R1–R13).

## Technical Context

**Language/Version**: Rust 1.99 (edition 2021) in `crates/voicen-core` and `src-tauri`; TypeScript + Svelte 5 (SvelteKit static SPA) in `src/`

**Primary Dependencies**: `serde` (present), `serde_json` (added to `voicen-core`; MIT/Apache-2.0); Tauri 2 (`WebviewWindow`, events, managed state); `@tauri-apps/api` (`invoke`, `listen`). No new UI dependency; `Intl.DateTimeFormat` for times. Consumed from other features: 001 delivery seam, clipboard writer, tray item; 004 settings (`history.enabled`, `history.size`), apply-to-subscribers, message catalog, open-settings command.

**Storage**: one JSON document `history.json` in the data folder, atomic write via `history.json.tmp` + rename; no encryption (R2, R3)

**Testing**: `cargo test -p voicen-core` in Docker (temp dirs, fault-injecting fake store); vitest for UI logic; Playwright with mocked IPC and a pushed `history-changed` event; `cargo test --workspace` on `windows-latest`; owner's manual check

**Target Platform**: Windows 10/11 x64 (core and UI logic verified on the Linux dev host)

**Project Type**: desktop app (Tauri shell + Rust core library + web UI)

**Performance Goals**: History window shows 100 entries in < 1 s (SC-005); recording an entry adds 0 ms to stop→paste because it runs after delivery (R6)

**Constraints**: text only, only when enabled (NFR-06); no transcript text in logs/errors (FR-20); no network (NFR-05); no resident window (NFR-03); history failures never block delivery or crash (NFR-07); stored data ≤ ~2 MB

**Scale/Scope**: ≤ 100 entries; 3 IPC commands, 1 event, 1 window, 1 core module

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-checked after Phase 1 design — still passes.*

| Principle | How this plan complies |
|---|---|
| I. PRINCIPLES.md | P-004/P-005: every guarantee C1–C12 and every UI state gets a red test, failure branches first (save fail, delete fail, corrupt file, copy fail). P-009: `HistoryError` cannot hold text (R10). P-010: data dir from the one resolver; range/defaults from 004; clipboard writer from 001. P-011: one record call site at 001's delivery seam; one clipboard path. P-013: a test drives 004's real apply path into `History`. P-014: architecture.md gains the history store and event in the same task that adds them. |
| II. Privacy by default | Text only, only when enabled; off ⇒ delete files incl. `.tmp`, no writes; no backup of corrupt data; no network code in the module; logs carry op, error kind and counts only. |
| III. Never lose the user's words | Entry recorded even when paste fails; Copy restores text; history failure never delays or fails delivery (record after delivery, errors swallowed into a notice). FR-16 is a Should; its failure branch (off ⇒ delete, no writes) still appears as US3 and has tests. |
| IV. Core / thin shell | All rules and the file store in `voicen-core` (std only); shell holds state, wires seams, owns the window. |
| V. Measured, not assumed | SC-005 checked in Playwright (indicative) and on the owner's PC; per-requirement verification placement below. |

No violations; Complexity Tracking is empty.

## Verification placement (per requirement)

| Spec req | Linux host — core with fakes | Linux host — UI, mocked IPC | Windows CI runner | Owner's manual check |
|---|---|---|---|---|
| FR-001 (record, text only) | C1, C9, entry fields | — | delivery seam calls `history_on_delivered` | step 2 |
| FR-002 (≤ N) | C1 | 20 of 25 shown | — | — |
| FR-003 (N from 004, trim on apply) | trim tests; apply path test | — | — | — |
| FR-004 (local, persisted, atomic) | C11, reload test | — | NTFS rename, path under `%LOCALAPPDATA%\Voicen` | step 3 |
| FR-005 (window, list, live, single) | — | list, preview, time, live event | single window per label | step 2 |
| FR-006 (copy, exclusion marks) | `text_of` | copy invoke + "Copied" | real clipboard writer with marks | step 2 (Win+V) |
| FR-007 (clear) | C4 | clear → empty | — | step 4 |
| FR-008 (off ⇒ delete, no writes) | C2, C3 | off state | — | step 5 |
| FR-009 (start cleanup, fallback, retry) | C4, C5 | `delete_failed` alert | — | — |
| FR-010 (no block, corrupt ⇒ empty) | C6, C7 | `save_failed` alert | — | — |
| FR-011 (no text in logs) | C8 | — | log of a test session grepped | step 6 |
| FR-012 (off window, no disk read) | C5, `view()` no store call | off state + Open settings | — | step 5 |
| FR-013 (EN/RU) | — | ru run of the e2e; 004 catalog parity gate | — | — |
| FR-014 (no network) | C12 (dependency review) | — | — | — |
| FR-015 (window on demand) | — | — | window destroyed on close | — |
| FR-016 (single location) | store path = given dir | — | resolver → `%LOCALAPPDATA%\Voicen` | step 4 |
| FR-017 (keyboard, names) | — | keyboard-only e2e, role/name selectors | — | — |
| FR-018 (copy fails) | — | `clipboard_unavailable` → alert | clipboard held by another process | — |

## Project Structure

### Documentation (this feature)

```text
specs/005-transcript-history/
├── plan.md              # this file
├── research.md          # Phase 0
├── data-model.md        # Phase 1
├── quickstart.md        # Phase 1
├── contracts/
│   ├── core-history.md  # voicen_core::history API + HistoryStore trait
│   └── ipc.md           # IPC commands, event, shell hooks, UI selectors
├── checklists/
└── tasks.md             # /speckit-tasks
```

### Source Code (repository root)

```text
crates/voicen-core/
├── Cargo.toml                    # + serde_json; dev: tempfile (or std temp dir helper)
└── src/
    ├── lib.rs                    # pub mod history
    └── history/
        ├── mod.rs                # History<S>, apply_settings, record, clear, view
        ├── model.rs              # HistoryEntry, EngineLabel, HistoryView, HistoryNotice, StoredHistory
        ├── error.rs              # HistoryError (no text by construction)
        ├── store.rs              # HistoryStore trait, FileHistoryStore (atomic JSON)
        └── tests.rs              # C1–C11 with a fault-injecting fake store and temp dirs

src-tauri/src/
├── lib.rs                        # register commands, manage Mutex<History>, open at startup
└── history.rs                    # history_get/copy/clear, show_history_window, history_on_delivered,
                                  # history_on_settings_applied, emit history-changed

src/
├── routes/history/+page.svelte   # History window
└── lib/history/
    ├── historyApi.ts             # invoke/listen wrappers, wire types
    ├── format.ts                 # preview (3 lines / 200 chars), time (today vs date), engine label
    ├── format.test.ts
    └── HistoryList.svelte        # list, entry, copy, expand, clear, states

e2e/history.spec.ts               # mocked IPC incl. pushed history-changed events
```

**Structure Decision**: the existing three-part layout (core library, Tauri shell, web UI) from `CLAUDE.md`; history adds one core module, one shell module, one route and one e2e spec.

## Dependencies on other features

- **001-dictation-via-api**: delivery step calls `history_on_delivered` (one call site); clipboard writer with exclusion marks; tray item "History" calls `show_history_window`; single-instance guarantee.
- **004-settings-and-first-run**: `history.enabled` / `history.size` (1–100, default on/20) and their validation; apply-to-subscribers calls `history_on_settings_applied`; message catalog keys `history.*` (EN + RU); open-settings command with the History tab; data-dir resolver shared with settings.
- **006-diagnostics-and-release**: uninstaller removes `%LOCALAPPDATA%\Voicen` (req FR-28); the log writer (history logs only error kinds and counts).

Until 001/004 land, core and UI tasks proceed against the contracts; the shell wiring task depends on them.

## Complexity Tracking

None.
