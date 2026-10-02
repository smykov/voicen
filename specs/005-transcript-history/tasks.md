# Tasks: Transcript History

**Input**: Design documents from `/specs/005-transcript-history/` — plan.md, spec.md, research.md, data-model.md, contracts/core-history.md, contracts/ipc.md, quickstart.md

**Tests**: REQUIRED. The constitution (P-004, P-005, principle III) needs a red test per guarantee, failure branches included, written before the implementation it guards.

**Conversion**: these tasks become teamwright tasks (`docs/tasks/T-NNN.md`, `design_ref: specs/005-transcript-history/tasks.md#<ID>`); `/speckit-implement` is not used. Each task names its spec requirements as `spec FR-0NN` and the source requirements as `req FR-NN`. Area: `core` = `crates/voicen-core`, `shell` = `src-tauri` (Windows CI only), `ui` = `src/`, `e2e/`.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: can run in parallel (different files, no dependency on an incomplete task)
- **[Story]**: US1 (recover a transcript, P1), US2 (clear, P2), US3 (turn off/on, P2)

---

## Phase 1: Setup

- [ ] T001 [area core] Add `serde_json` (and a temp-dir dev helper: `tempfile` dev-dependency, MIT/Apache-2.0, NFR-12) to `crates/voicen-core/Cargo.toml`, create the empty module `crates/voicen-core/src/history/mod.rs` and `pub mod history;` in `crates/voicen-core/src/lib.rs`; `make check` stays green (plan "Source Code"; req FR-16).

---

## Phase 2: Foundational (blocks all stories)

- [ ] T002 [area core] Red tests for the model and the error type in `crates/voicen-core/src/history/tests.rs`: `HistoryEntry` serde round-trip with fields `id: u64`, `text`, `ready_at_ms: i64`, `engine: { kind: "api" | "local" | "server", model }`; `HistoryView`/`HistoryNotice` serialize as `"save_failed" | "delete_failed"`; `HistoryError` holds only `op` (`Load`/`Save`/`Delete`) and `Io(ErrorKind)`/`Corrupt`, and its `Display`/`Debug` never contain a sentinel entry text, also when built from a serde_json error on a document that contains the sentinel (contract C8; spec FR-001, FR-011; req FR-16, FR-20, NFR-06).
- [ ] T003 [area core] Implement `model.rs` and `error.rs` in `crates/voicen-core/src/history/` per contracts/core-history.md "Types" until T002 is green; nothing but text, time and engine is stored (spec FR-001, FR-011; req FR-16, NFR-06, FR-20).
- [ ] T004 [area core] Red tests for `FileHistoryStore` against a temp dir in `crates/voicen-core/src/history/tests.rs`: `load` with no file → `Ok(None)`; `save` then `load` round-trips; invalid JSON, unknown `schema`, or an entry with empty text → `Err(Corrupt)` without panic; an interrupted save (a `history.json.tmp` present, rename not done) leaves `load` returning the previous document (C11); `delete` removes `history.json` and `history.json.tmp` and is `Ok` when nothing exists; a read-only dir makes `save` return `Err(Io(_))` with the previous document intact (spec FR-004, FR-009, FR-010, FR-016; req FR-16, NFR-06, NFR-07).
- [ ] T005 [area core] Implement `HistoryStore` and `FileHistoryStore` in `crates/voicen-core/src/history/store.rs`: document `{ "schema": 1, "next_id", "entries" }`, write `history.json.tmp` + `sync_all` + `rename`, `delete` removes both files; the directory is a constructor argument (no path resolution in core, P-010) until T004 is green (spec FR-004, FR-016; req FR-16).
- [ ] T006 [area core] A fault-injecting `FakeHistoryStore` test helper in `crates/voicen-core/src/history/tests.rs` that records every call (`load`/`save`/`delete`) and can be set to fail each one with an `io::ErrorKind` — used by T007–T018 (contract "Behavioural guarantees").

**Checkpoint**: storage seam proven on the Linux host.

---

## Phase 3: User Story 1 — Recover a recent transcript (P1) 🎯 MVP

**Goal**: delivered final texts are kept (≤ N, newest first, persisted) and any of them can be copied again from the tray's History window.

**Independent test**: with history on, 25 records with N = 20 give 20 entries newest first in core and on disk; in the History window (mocked IPC) the list renders and Copy calls `history_copy` with the entry's id.

### Tests (red first)

- [ ] T007 [P] [US1] [area core] Red tests for `History::open` (enabled) and `record` in `crates/voicen-core/src/history/tests.rs`: 25 records with size 20 → 20 entries, newest first, stored doc equals `view()` (C1); whitespace-only text is never recorded (C9); ids strictly increase and order is insertion order even with decreasing `ready_at_ms` (C10); reopening the same dir restores the entries (US1-AS4); a stored file with more than `size` entries is trimmed on open; a failing `save` keeps the entry in memory, sets notice `SaveFailed`, returns the error, and does not panic (C7, SC-004); a corrupt file at open → empty view + `Corrupt` returned, no backup file created (C6) (spec FR-001, FR-002, FR-004, FR-010; req FR-16, NFR-07).
- [ ] T008 [P] [US1] [area ui] Red unit tests in `src/lib/history/format.test.ts`: preview = first 3 lines or 200 characters, whichever is shorter, and `needsExpand` only when truncated; time = time only for today and date + time otherwise, in local time and the UI language (`en`, `ru`) with a fixed time zone; engine label `"API · whisper-1"`, `"Local · small"`, `"Server · <model>"` with the catalog's kind names (spec FR-005, FR-013; req FR-16, FR-15).
- [ ] T009 [P] [US1] [area ui] Red Playwright tests in `e2e/history.spec.ts` with mocked IPC (`history_get`, `history_copy`, `plugin:event|listen` with a kept callback to push `history-changed`): 20 entries listed newest first with preview, time and engine (US1-AS1); Copy → `history_copy({id})` and `role=status` "Copied" for 2 s (US1-AS2); `clipboard_unavailable` → `role=alert` "Could not copy — try again" and the list unchanged (spec FR-018); a pushed view with a new entry shows it on top and drops the oldest at N (US1-AS3); empty view → "No transcripts yet" (US1-AS5); `notice: "save_failed"` → alert "History could not be saved to disk"; keyboard-only Tab/Enter reaches Copy and expand, all with accessible names (spec FR-017); a 100-entry view renders within 1 s (SC-005, indicative) (spec FR-005, FR-006, FR-010, FR-017, FR-018; req FR-16, FR-10).

### Implementation

- [ ] T010 [US1] [area core] Implement `History<S>` `open` (enabled branch), `record`, `text_of`, `view` in `crates/voicen-core/src/history/mod.rs` until T007 is green; `view()` never calls the store (spec FR-001, FR-002, FR-004, FR-005, FR-010, FR-012; req FR-16, NFR-06, NFR-07).
- [ ] T011 [P] [US1] [area ui] Implement `src/lib/history/format.ts` (preview, time, engine label) until T008 is green, and `src/lib/history/historyApi.ts` (wire types and `invoke`/`listen` wrappers for `history_get`, `history_copy`, `history_clear`, `history-changed` per contracts/ipc.md) (spec FR-005; req FR-16).
- [ ] T012 [US1] [area ui] Implement `src/routes/history/+page.svelte` and `src/lib/history/HistoryList.svelte`: list, expand toggle (`aria-expanded`), Copy with 2 s "Copied", copy error alert, empty state, notice alert, live replacement of state from `history-changed`; all texts from 004's message catalog keys `history.*` in EN and RU, until T009 is green (spec FR-005, FR-006, FR-013, FR-017, FR-018; req FR-16, FR-15).
- [ ] T013 [US1] [area shell] In `src-tauri/src/history.rs` and `src-tauri/src/lib.rs`: manage `Mutex<History<FileHistoryStore>>` opened at startup with the dir from the single data-directory resolver (`%LOCALAPPDATA%\Voicen`); commands `history_get` and `history_copy` (through 001's clipboard writer with the exclusion marks, never a paste; errors `not_found` / `clipboard_unavailable`); `history_on_delivered` called from 001's delivery step once per job with outcome delivered or copied-only, after the clipboard/paste attempt; emit `history-changed` to the `history` window; `show_history_window` (label `history`, route `/history`, focus if it exists, destroyed on close) wired to 001's tray item "History"; history errors logged as op + error kind + counts only. Tests: the text passed is the delivered final text (the raw transcript when post-processing was skipped); failed transcription, no speech, hold < 0.3 s and Esc cancel create no entry and a successful retry creates exactly one (spec edge cases); with an unwritable data dir every dictation is still delivered with an unchanged outcome (SC-004). If 001 places the delivery outcome in `voicen-core`, these run on the Linux host at that seam, otherwise on Windows CI. Windows CI tests: store path is under `%LOCALAPPDATA%\Voicen`; copy writes the exclusion formats; a second `show_history_window` focuses the existing window; closing the window destroys it (spec FR-015). Depends on 001 (delivery seam, clipboard writer, tray item) (spec FR-001, FR-004, FR-005, FR-006, FR-010, FR-011, FR-015, FR-016, FR-018; req FR-16, FR-01, FR-10, NFR-03, FR-20).

**Checkpoint**: US1 works end to end (MVP); owner's manual check steps 1–3 and 6 of quickstart.md §4.

---

## Phase 4: User Story 2 — Clear the history (P2)

**Goal**: one click empties the list and deletes the stored data; a failed deletion falls back and is reported.

**Independent test**: entries in a temp dir → `clear()` → empty view, no `history.json*`, 0 matches for any prior text in the dir; in the UI, Clear → empty state.

- [ ] T014 [P] [US2] [area core] Red tests for `clear` in `crates/voicen-core/src/history/tests.rs`: after `clear()` the dir has no `history.json*` and a byte search of every file in the dir for each prior sentinel text finds 0 matches (C3, SC-003); a record after clear starts a one-entry history (US2-AS2); with `delete` failing, `clear` saves an empty document instead and the old texts are gone from disk (C4); with `delete` and `save` both failing, the view is empty, notice `DeleteFailed`, and the next `record` first retries the pending deletion and, on success, clears the notice (C4; US2-AS3) (spec FR-007, FR-009; req FR-16, NFR-06).
- [ ] T015 [P] [US2] [area ui] Red Playwright tests in `e2e/history.spec.ts`: Clear has no confirmation, calls `history_clear` and shows "No transcripts yet" (US2-AS1); a returned `notice: "delete_failed"` shows the alert "History could not be deleted from disk" until a later view without notice (spec FR-007, FR-009; req FR-16).
- [ ] T016 [US2] [area core] Implement `History::clear`, the delete → save-empty fallback and the `pending_disk` retry at the start of every later operation in `crates/voicen-core/src/history/mod.rs` until T014 is green (spec FR-007, FR-009; req FR-16, NFR-06).
- [ ] T017 [US2] [area ui+shell] Add the Clear button and the delete-failed alert to `src/lib/history/HistoryList.svelte` until T015 is green; add the `history_clear` command (returns `HistoryView`, emits `history-changed`) in `src-tauri/src/history.rs` (spec FR-007, FR-009, FR-017; req FR-16).

**Checkpoint**: quickstart.md §4 step 4.

---

## Phase 5: User Story 3 — Turn history off and on (P2)

**Goal**: off deletes stored entries at once and writes nothing more; on starts empty; N lowered trims at once.

**Independent test**: entries in a temp dir → `apply_settings(false, 20)` → no files; 5 records while off → dir still empty, `save` never called; `open(enabled=false)` deletes leftovers without loading.

- [ ] T018 [P] [US3] [area core] Red tests in `crates/voicen-core/src/history/tests.rs`: `apply_settings(false, _)` deletes both files at once and empties the view (US3-AS1, C3); while off, records are no-ops and the fake store sees zero `save` calls; the temp dir stays without `history.json*` (C2, US3-AS2, SC-003); `open(store, enabled=false, _)` calls `delete` and never `load`, removing a leftover `history.json` and `history.json.tmp` (C5, US3-AS3, spec FR-012); off → on gives an empty view, and the next record writes a one-entry file (US3-AS4); size 20 → 5 trims memory and disk to the 5 latest (US3-AS5); off and a lower size in one call → everything deleted (edge case); a failed delete while turning off falls back to an empty save, then `DeleteFailed` + retry as in T014 (spec FR-003, FR-008, FR-009, FR-012; req FR-16 failure branch, NFR-06).
- [ ] T019 [P] [US3] [area ui] Red Playwright tests in `e2e/history.spec.ts`: a view with `enabled: false` shows "History is off" with an "Open settings" button that invokes 004's open-settings command with the History tab, and no entries (spec FR-012); a pushed `history-changed` with `enabled: false` switches an open window to that state at once (edge case); the whole window in `ru` shows Russian texts for list, empty, off, copied and error states (spec FR-013; req FR-16, FR-15).
- [ ] T020 [US3] [area core] Implement `History::apply_settings` and the disabled branch of `History::open` in `crates/voicen-core/src/history/mod.rs` until T018 is green (spec FR-003, FR-008, FR-009, FR-012; req FR-16, NFR-06).
- [ ] T021 [US3] [area ui] Implement the off state and the "Open settings" button in `src/lib/history/HistoryList.svelte` until T019 is green (spec FR-012, FR-013, FR-017; req FR-16, FR-15).
- [ ] T022 [US3] [area shell] Subscribe history to 004's settings apply: `history_on_settings_applied(enabled, size)` in `src-tauri/src/history.rs`, emit `history-changed`; startup passes the saved `history.enabled`/`history.size` to `History::open`. A test through 004's real settings loader/apply path shows the saved values reach `History` (P-013); fresh install → on, 20 (US3-AS6, values from 004). Depends on 004 (settings fields, apply, open-settings command) (spec FR-003, FR-008, FR-009; req FR-16, FR-13, FR-21).

**Checkpoint**: quickstart.md §4 step 5.

---

## Phase 6: Polish & cross-cutting

- [ ] T023 [P] [area core] Privacy sweep test in `crates/voicen-core/src/history/tests.rs`: a session that records sentinel texts, triggers every error path (save fail, delete fail, corrupt load) and clears — every returned `HistoryError` formatted with `Display` and `Debug` contains none of the sentinels (C8, SC-006); and the `voicen-core` history module has no HTTP dependency (C12, reviewer check) (spec FR-011, FR-014; req FR-20, NFR-04, NFR-05).
- [ ] T024 [P] [area docs] Update `docs/architecture.md` in the same task as T013: history store `history.json` in the data folder, the `HistoryStore` seam, the `history-changed` event and the `history_*` IPC commands; add the History window to the UI row (P-014) (spec FR-016; req FR-16).
- [ ] T025 [area verify] Owner's manual check on Windows per quickstart.md §4 steps 1–6 as the `verify_exception` record for the tray, real clipboard and Win+V parts (requirements §9 UI verification row) (spec FR-005, FR-006, FR-008, FR-011, FR-016; req FR-16, FR-10, NFR-06).

---

## Dependencies & execution order

- Setup T001 → Foundational T002–T006 (T002→T003, T004→T005, T006 after T003) → stories.
- **US1** (T007–T013) needs Foundational. T013 also needs 001 (delivery seam, clipboard writer, tray item).
- **US2** (T014–T017) needs T010 (`History` exists); independent of US3.
- **US3** (T018–T022) needs T010; T022 needs 004 (settings, apply, open-settings) and T013 (shell state).
- Polish after the stories it covers; T024 lands with T013.
- External: req FR-28 removal of the data folder on uninstall is verified by 006, not here.

### Parallel opportunities

- T002 and T004 (different concerns, same test file — split into `tests/model.rs` / `tests/store.rs` sections if run in parallel).
- Within US1: T007 (core), T008 (ui unit), T009 (e2e) in parallel; then T010 and T011 in parallel.
- US2 and US3 core tests (T014, T018) and e2e tests (T015, T019) in parallel once T010 is done.

## Implementation strategy

1. MVP = Phase 1 + 2 + US1 (T001–T013): history is recorded, persisted, shown and copyable.
2. US2 (Clear) and US3 (off/on, trim) next — US3 carries the req FR-16 failure branch and NFR-06, so it must ship with FR-16 even though FR-16 is a Should.
3. Polish and the owner's manual check close the feature.

## Requirement coverage

| Spec req | Tasks |
|---|---|
| FR-001 | T002, T003, T007, T010, T013 |
| FR-002 | T007, T010 |
| FR-003 | T018, T020, T022 |
| FR-004 | T004, T005, T007, T010, T013 |
| FR-005 | T008, T009, T011, T012, T013 |
| FR-006 | T009, T012, T013 |
| FR-007 | T014–T017 |
| FR-008 | T018, T020, T022 |
| FR-009 | T004, T014, T016, T018, T020 |
| FR-010 | T004, T007, T009, T010, T013 |
| FR-011 | T002, T013, T023 |
| FR-012 | T018, T019, T020, T021 |
| FR-013 | T008, T012, T019, T021 |
| FR-014 | T023 |
| FR-015 | T013 |
| FR-016 | T005, T013, T024 |
| FR-017 | T009, T012, T017, T021 |
| FR-018 | T009, T012, T013 |
