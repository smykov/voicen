# Quickstart: validating Transcript History (005)

How to prove the feature works, per place it can run. Contracts: [contracts/core-history.md](contracts/core-history.md), [contracts/ipc.md](contracts/ipc.md); data: [data-model.md](data-model.md).

## Prerequisites

`pnpm install && make core-image` (toolchain image `voicen-rust:1.99`). Features 001 (delivery seam, clipboard writer, tray item) and 004 (history settings, apply, catalog, open-settings) exist or are stubbed by their own tasks.

## 1. Core on the Linux host

```sh
scripts/tw-run core -- cargo test -p voicen-core history
```

Expected: the guarantees C1–C11 of the core contract pass. They include:

- 25 records with N = 20 → 20 entries, newest first; stored doc equals the view (US1-AS1, SC-002).
- Clear and `apply_settings(false, _)` → no `history.json*` in the temp dir; grep of the dir for every recorded sentinel text → 0 matches (SC-003).
- While off, further records → the dir stays empty (US3-AS2).
- Read-only dir / fault-injecting fake store → `record` returns normally, notice `save_failed`; delete failure → fallback save, then `delete_failed` and a retry at the next op (US2-AS3, SC-004).
- Corrupt `history.json` at open → empty history, `Corrupt` error, no panic.
- Error `Display`/`Debug` strings contain none of the sentinel texts (SC-006).

## 2. UI on the Linux host (mocked IPC)

```sh
pnpm test -- src/lib/history
pnpm e2e -- e2e/history.spec.ts
```

The e2e mock defines `window.__TAURI_INTERNALS__.invoke` for `history_get`, `history_copy`, `history_clear`, 004's open-settings command and `plugin:event|listen`; it keeps the registered callback (via `transformCallback`) to push `history-changed` payloads.

Expected:
- 20 entries listed newest first with preview, local time and "API · whisper-1" (US1-AS1); 100 entries render in < 1 s (SC-005, indicative).
- Copy → `history_copy` called with the id, "Copied" for 2 s; `clipboard_unavailable` → "Could not copy — try again" (US1-AS2, FR-018).
- A pushed `history-changed` with a new entry appears at the top; one with `enabled: false` switches to "History is off" (US1-AS3, edge case).
- Clear → empty state; `notice: "delete_failed"` → the alert (US2).
- Off state → "Open settings" invokes 004's command with the History tab; no `history_*` reads beyond `history_get` (FR-012).
- All of it with UI language `ru` shows Russian texts; keyboard-only Tab/Enter reaches every action (FR-013, FR-017).

## 3. Windows CI runner

`cargo test --workspace` on `windows-latest` covers: the `FileHistoryStore` rename-over-existing on NTFS, `history.json` resolved under `%LOCALAPPDATA%\Voicen`, `history_copy` writing through the real clipboard writer with the exclusion formats present, and `show_history_window` creating one window (a second call focuses it). The install smoke from 006 still passes.

## 4. Owner's manual check on Windows (verify_exception, requirements §9)

1. Fresh install → Settings › History shows on, 20 (004).
2. Dictate 3 times → tray › History lists 3 entries; Copy one → paste in Notepad; Win+V does not list it.
3. Exit, restart → the 3 entries are still there.
4. Clear → list empty; `%LOCALAPPDATA%\Voicen\history.json` is gone.
5. Dictate twice, turn history off and save → file gone; dictate again → no file appears; tray › History shows "History is off".
6. Open the log file → no dictated text appears in it.
