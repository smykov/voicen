# Feature Specification: Transcript History

**Feature Branch**: `005-transcript-history` (spec directory; work happens on `main` per project rules)

**Created**: 2026-10-02

**Status**: Approved (clarifications confirmed by the owner 2026-10-02)

**Input**: User description: "Transcript history: when enabled (default on, N = 20) the last N final transcripts (text, time, engine; no audio) are kept locally, unencrypted, in %LOCALAPPDATA%\Voicen; the user can open the list from the tray, copy any entry again, and clear it; turning history off deletes stored entries and stops writing."

**Source requirements** (`docs/requirements.md` v3): FR-16 (in scope, Should). Constraints honoured: NFR-06 (history stores text only, and only when enabled), NFR-05 (no network), FR-20 (transcript text never in logs), FR-15 (EN/RU UI text), FR-10 (clipboard exclusion marks, r1#28), FR-01 (tray gives access to history), NFR-03 (windows created on demand), NFR-07 (a failure never crashes the app). Owned elsewhere: history settings fields and their defaults (FR-13, FR-21 → feature 004), uninstall removal of history (FR-28 → feature 006), the tray menu itself and the delivery pipeline (FR-01, FR-10, FR-23 → feature 001).

## Clarifications

### Session 2026-10-02

- Q: What range may the user choose for the history size N? → A: Whole numbers 1–100, default 20, as defined by feature 004 (confirmed by the owner 2026-10-02)
- Q: When the user lowers N below the number of stored entries, when are the oldest entries removed? → A: Immediately when the setting is applied; the removed entries are deleted from disk too (confirmed by the owner 2026-10-02)
- Q: Does "Clear" ask for confirmation before deleting all entries? → A: No confirmation; one click empties the list and deletes the stored data, as the FR-16 acceptance example states (confirmed by the owner 2026-10-02)
- Q: What does the "engine" field of an entry name? → A: The engine kind (API, built-in local, local server) plus the model used (e.g. "API · whisper-1", "Local · small") (confirmed by the owner 2026-10-02)
- Q: What happens when the user opens History from the tray while history is turned off? → A: The History window opens and says history is off, with a button that opens the settings at the history fields; no entries are shown and nothing is read from disk (confirmed by the owner 2026-10-02)

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Recover a recent transcript (Priority: P1)

The user dictated something a few minutes ago — the paste went to the wrong place, the clipboard has since been overwritten, or they want the same text again. They open "History" from the tray, find the entry (newest first, with its time and engine), and click "Copy". The text is in the clipboard and they paste it wherever they want.

**Why this priority**: This is the reason the history exists: never lose the user's words (constitution III). Without it, a lost paste means dictating again.

**Independent Test**: With history on and a fake pipeline producing final texts, open the History window (mocked IPC), see the entries newest first, click Copy on one, and observe the copy command called with that entry's id; on Windows, the clipboard holds exactly that text.

**Acceptance Scenarios**:

1. **Given** history is on with N = 20 and 25 dictations were delivered, **When** the user opens History from the tray, **Then** exactly the 20 latest entries are shown, newest first, each with its text, local time and engine (req FR-16 acceptance).
2. **Given** an entry in the list, **When** the user clicks its "Copy" action, **Then** the clipboard holds that entry's full text, marked as excluded from Windows clipboard history and cloud sync, and the window confirms "Copied" for 2 s (req FR-16, FR-10/r1#28).
3. **Given** the History window is open, **When** a new dictation is delivered, **Then** the new entry appears at the top without reopening the window, and if the list already had N entries the oldest disappears.
4. **Given** the app was exited and restarted, **When** the user opens History, **Then** the entries from before the restart are still listed (stored locally).
5. **Given** history is on and there are no entries yet, **When** the user opens History, **Then** the window shows an empty state ("No transcripts yet").

---

### User Story 2 - Clear the history (Priority: P2)

The user dictated something private and wants it gone from the machine. In the History window they click "Clear"; the list empties and the stored data is deleted from disk.

**Why this priority**: History is stored unencrypted; the user must be able to remove it at any time. It is the privacy counterweight to Story 1.

**Independent Test**: With entries stored in a temporary data directory, call clear through the core API and check the list is empty and the history file no longer contains any of the texts (or is gone); in the UI with mocked IPC, click Clear and see the empty state.

**Acceptance Scenarios**:

1. **Given** history has entries, **When** the user clicks "Clear", **Then** the list is empty and the stored data is deleted from disk (req FR-16 acceptance).
2. **Given** history was cleared, **When** the user dictates again, **Then** history starts again from that one entry.
3. **Given** the stored data cannot be deleted (file locked or not writable), **When** the user clicks "Clear", **Then** the list is still emptied in the app and the stored data is replaced by an empty history instead; if that also fails, the window shows "History could not be deleted from disk" and the deletion is retried at every later change to history.

---

### User Story 3 - Turn history off and on (Priority: P2)

A user who does not want any transcript text kept on disk turns history off in settings (the setting itself belongs to feature 004). From that moment stored entries are deleted and no transcript is ever written to disk. Turning it back on starts an empty history.

**Why this priority**: It is the failure branch of FR-16 and the NFR-06 rule "history stores text only, and only when enabled" — a privacy guarantee that must hold even though the feature itself is a Should.

**Independent Test**: In core with a temporary data directory: store entries, apply a config with history off, check the history file is gone, deliver more final texts through the recording seam, check nothing was created in the data directory and the list is empty.

**Acceptance Scenarios**:

1. **Given** history is on with stored entries, **When** the user turns history off and saves settings, **Then** the stored entries are deleted from disk at once, without an app restart (req FR-16 failure branch, FR-13).
2. **Given** history is off, **When** dictations are delivered, **Then** nothing is written to disk for history and the History window shows "History is off" (req FR-16 failure branch, NFR-06).
3. **Given** history is off and a leftover history file exists at start (e.g. a deletion failed before a crash), **When** the app starts, **Then** that file is deleted before anything else touches history.
4. **Given** history was turned off and is turned back on, **When** the user opens History, **Then** the list is empty (old entries are not restored) and new dictations are recorded again.
5. **Given** history is on with 20 entries, **When** the user lowers N to 5 and saves, **Then** only the 5 latest entries remain, in the window and on disk.
6. **Given** a fresh install, **Then** history is on with N = 20 (req FR-21 defaults; values supplied by feature 004).

---

### Edge Cases

- **Post-processing skipped** (FR-09 failure): the entry holds the text that was actually delivered (the raw transcript), so history always matches what the user received.
- **Paste failed / "copied — paste manually"** (FR-10 failure): the entry is still recorded — the final text was ready and delivered to the clipboard.
- **Transcription failed** (FR-11), **no speech detected** (FR-12), **hold < 0.3 s** (FR-02), **Esc cancel** (FR-22): no entry is created — there is no final text. A later successful retry of the pending recording creates exactly one entry.
- **Results out of order** (FR-23): entries are added in delivery order, which FR-23 guarantees equals recording order; each entry's time is the moment its final text was ready.
- **Empty final text** (engine returned only whitespace): no entry is created.
- **Very long text** (up to a 10-minute recording): stored and copied in full; the list shows a shortened preview (first lines) and the full text is available by expanding the entry.
- **History storage not writable** (disk full, permissions): the dictation is still delivered normally (history never blocks or fails delivery, NFR-07); the entry is kept in memory for the session, a warning without transcript text is logged, and the History window shows "History could not be saved to disk".
- **"Could not be saved / deleted" notice**: it stays visible in the History window until the next successful write or deletion, then disappears.
- **History turned off while the History window is open**: the window switches to the "History is off" state at once.
- **N lowered and history turned off in the same save**: turning off wins — everything is deleted.
- **Copy fails** (clipboard held by another app): "Could not copy — try again"; nothing else changes (FR-018).
- **Stored history unreadable or corrupt** at start: the app starts normally with an empty history, logs a warning without transcript text, and the corrupt data is overwritten on the next write (or deleted if history is off or cleared); it is not kept as a backup, because a backup would keep transcript text the user can no longer see or clear.
- **App crashes or is killed** right after a dictation: an entry recorded before the crash is not lost and the stored data is never left half-written (the previous complete state or the new complete state is on disk).
- **Second instance** (FR-01): only the running instance reads or writes history.
- **Clock changed backwards**: entries keep their insertion order (newest first by insertion, not by timestamp).
- **UI language** (FR-15): all History window text, including empty, off and error states, exists in English and Russian; times are shown in the user's local time and UI-language format.
- **Uninstall** (FR-28, feature 006): history lives inside `%LOCALAPPDATA%\Voicen`, so the uninstaller's "remove settings, history, …" choice removes it; this feature adds no data outside that folder.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001** (req FR-16, NFR-06): When history is enabled, the system MUST record one history entry for every non-empty final text that reaches delivery (req FR-10) — whether or not the paste or the clipboard write then succeeds — containing only the text, the time it became ready, and the engine (kind and model). It MUST NOT store audio, API keys, the target window, or any other data.
- **FR-002** (req FR-16): The system MUST keep at most N entries (the latest N); when a new entry would exceed N, the oldest entry MUST be removed from memory and from disk.
- **FR-003** (req FR-16, FR-13): N is the history size setting owned by feature 004 (a whole number from 1 to 100, default 20, validated and defined in one place by 004); this feature relies on that range and does not define its own. Lowering N MUST remove the excess oldest entries immediately when the setting is applied, from memory and disk.
- **FR-004** (req FR-16, NFR-06): Entries MUST be stored locally in the app data folder `%LOCALAPPDATA%\Voicen`, unencrypted, so that they survive app restarts; the stored data MUST always be a complete state (never partially written).
- **FR-005** (req FR-16, FR-01): The user MUST be able to open a History window from the tray; it lists the entries newest first with text (a preview of at most the first 3 lines or 200 characters, whichever is shorter, expandable to the full text), time (time only for today's entries, date and time otherwise, in local time and the UI language's format) and engine, and updates live when an entry is recorded, trimmed or cleared, or history is turned off or on. Opening History while its window is already open MUST bring that window to front instead of opening a second one.
- **FR-006** (req FR-16, FR-10/r1#28): The user MUST be able to copy any entry's full text to the clipboard again; the copy MUST use the same clipboard path as dictation delivery, marked as excluded from Windows clipboard history and cloud sync. Copy never pastes into another window.
- **FR-007** (req FR-16): The user MUST be able to clear the history with one click, without confirmation; afterwards the list is empty and the stored data is deleted from disk.
- **FR-008** (req FR-16 failure branch, NFR-06): When history is turned off, the system MUST delete all stored entries (memory and disk) at once, without a restart, and while off MUST NOT write any history data to disk; recording a final text while off is a no-op.
- **FR-009** (req FR-16 failure branch, NFR-06): At every start while history is off, the system MUST delete any leftover history data on disk (including an interrupted partial write). If a deletion (clear, off, trim) fails, the system MUST fall back to replacing the stored data with the state it should have (empty, or the trimmed list); if that fails too, it MUST show the failure in the History window and retry at every later history change. Data the disk refused to delete or replace before the app exited may be loaded again at the next start while history is on; the shown failure tells the user it is still on disk.
- **FR-010** (req NFR-07, NFR-06): A history read or write failure MUST NOT block, delay or fail the delivery of a dictation and MUST NOT crash the app; a corrupt or unreadable history at start MUST result in an empty history and a warning.
- **FR-011** (req FR-20, NFR-04): Logs, crash files and error messages about history MUST NOT contain transcript text; they may contain counts, sizes and error kinds only.
- **FR-012** (req FR-16): When history is off, opening History from the tray MUST show a "History is off" state with a button that opens settings at the history fields, and MUST NOT read history data from disk.
- **FR-013** (req FR-15): All user-visible text of the History window (title, actions, empty, off, copied and error states) MUST be available in English and Russian through the single message catalog of feature 004, following the UI language setting.
- **FR-014** (req NFR-05): History MUST NOT cause any network access.
- **FR-015** (req NFR-03): The History window MUST be created when opened and destroyed when closed; history keeps no window resident while idle.
- **FR-016** (req FR-28, P-010): All history data MUST live in a single location inside the app data folder resolved by the one data-directory resolver, so that the uninstaller of feature 006 removes it with the folder.
- **FR-017** (req FR-16): Every History window action (Copy per entry, expand, Clear, Open settings) MUST be reachable and operable with the keyboard alone and carry an accessible name.
- **FR-018** (req FR-16, FR-10): If copying an entry fails (clipboard unavailable), the window MUST show "Could not copy — try again" and the entry and the history stay unchanged.

### Key Entities

- **HistoryEntry** (req §7 glossary): one delivered final transcript — id (unique, increasing), text (the delivered final text, full), time (UTC instant the final text was ready; shown in local time), engine (kind: API / built-in local / local server; model name). No audio.
- **History**: the ordered list of at most N entries, newest first, plus the stored copy on disk; state follows the history settings.
- **History settings** (owned by feature 004, consumed here): enabled (default on), size N (1–100, default 20).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: From the tray, the user can get any of the last N transcripts back into the clipboard in at most 3 clicks (open tray menu, History, Copy).
- **SC-002**: After 25 dictations with N = 20, exactly 20 entries are listed and stored — never more than N at any time.
- **SC-003**: With a writable app data folder, after "Clear" or turning history off, a search of the app data folder for the text of any previous entry finds nothing (0 matches), and while history is off it stays 0 after any number of dictations.
- **SC-004**: With the history storage made unwritable, 100% of dictations are still delivered, and the app does not crash.
- **SC-005**: The History window opens and shows a full list of 100 entries in under 1 second on the reference machine.
- **SC-006**: Logs contain 0 occurrences of any history entry's text after a test session that records, copies and clears entries.

## Assumptions

- The tray menu item "History" (FR-01) and the delivery pipeline's "final text ready" point are provided by feature 001; this feature hooks into that one point (P-011) rather than adding a parallel path.
- The settings fields "history on/off" and "history size", their defaults (on, 20) and their persistence are provided by feature 004 (FR-13, FR-21); applying saved settings notifies history without a restart.
- The clipboard writer with the exclusion marks (FR-10, r1#28) is provided by feature 001 and reused for "Copy".
- The uninstaller (FR-28) is feature 006; this feature only guarantees all history data sits inside `%LOCALAPPDATA%\Voicen`.
- Deleting single entries, search, export and editing entries are out of scope (not in FR-16).
- The stored data relies on the default per-user permissions of `%LOCALAPPDATA%` (readable by the user's account and administrators); no extra access control, as FR-16 accepts unencrypted storage.
- Data volume: at most 100 entries, each at most the text of a 10-minute recording (roughly 15 000 characters), so the stored data stays under about 2 MB.
- SC-005's "reference machine" is the one named in OQ-03 (open).
- "Deleted" means the history file is removed through the file system; secure wiping of disk sectors is out of scope (the requirement accepts unencrypted storage).
- Verification placement: core history logic on the Linux host with a temporary directory and fakes; the History window with Playwright and mocked IPC on the Linux host and in CI; the clipboard copy, tray entry and data-folder location on the Windows CI runner and by the owner's manual check (requirements §9).
