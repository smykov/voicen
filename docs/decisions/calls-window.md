# Calls window (shell)

**Code:** `src-tauri/src/call.rs` (`open_calls`, `open_window`, `spawn_opener`, `WindowState`, the five `calls_*` commands, `ShellCallShell::changed`), `src-tauri/src/win/save_dialog.rs` (`WinSaveDialog`, `shown`), `src-tauri/src/lib.rs` (`release_clipboard`, `release_call_ports`), `src-tauri/capabilities/calls.json`; core `CallStore::folder` and `CallShell::changed` (`crates/voicen-core/src/call/`) (T-142) · **Tests that pin it:** `src-tauri/tests/call_ipc.rs`, `src-tauri/tests/save_dialog.rs`, the calls cases of `src-tauri/tests/app_acl.rs` (all Windows CI only), core `storage::tests::folder_*` and `tests/call_session.rs` (`changed` cases). Contract: `specs/007-call-mode-record/contracts/ipc.md`. Page: `docs/decisions/calls-ui.md`. Core side: `docs/decisions/call-storage.md`. Grants: `docs/decisions/ipc-acl.md`. Opener-thread pattern: `docs/decisions/windows-shell.md`.

The Calls window is the shell half of FR-033, FR-034 and FR-036. The invariants are numbered as in T-142's analysis (I1, the ACL, is in `ipc-acl.md`; I3, the guard, is in `call-storage.md`).

## Invariants

### I2 — One `calls` window, built only on its own opener thread; every entry posts and never waits

- **What breaks if you violate it:** a build from a command or the main thread can deadlock on the main thread (the `settings_window` precedent, T-052), and two entries racing the label check make a second window or a `WebviewLabelAlreadyExists` error.
- **Where it is enforced:** `call::open_calls(app, Option<CallRef>)` counts the request (`calls_opened`, a test read), posts it to the "calls-window" thread (started by `start_calls`, managed as `CallsOpener`) and returns. `open_window` runs there, one request at a time: an existing window is un-minimized, shown and focused; a missing one is built at `/calls`. A request on an app with no call part is a no-op. Tests: `call_ipc.rs::the_tray_calls_item_opens_one_calls_window_and_a_second_click_no_second`.
- **A failed window build is not logged** (no `WarningKind` exists for it); the window stays absent and the next open builds again.
- **Don't:** call `WebviewWindowBuilder` from a command or the tray handler; wait on the opener from a caller.

### I2 — A targeted open reaches the page as exactly one `calls://open`, also on a window just built

- **Defect it prevents:** an event emitted right after the window is built can arrive before the page's `listen`, and is lost.
- **The mechanism (a developer decision of T-142, no contract change):** the call part keeps `WindowState { listening, pending }` under one mutex. A fresh window starts with `listening: false` and `pending` = the open's target. `calls_list` from a webview labelled `calls` sets `listening` and emits the pending target once (`emit_to("calls", "calls://open", ..)`). The page sends its first `calls_list` only after all its listeners exist (`src/routes/calls/+page.svelte`, `docs/decisions/calls-ui.md` D8), so the event cannot be early. On a window that already listed, an open emits at once; a later open before the first list replaces the pending target; a bare open (tray) emits nothing. There is one emit site: the private `call::emit_open` (`emit_to("calls", ..)`), fed by this held or immediate target and by T-139's `call::open_transcript(app, root, id)`, which emits at once and does not open or show the window (wiring: `docs/architecture.md`, decisions #298).
- **Where it is enforced:** `call_ipc.rs::an_open_with_a_target_on_a_fresh_window_emits_calls_open_once_after_the_first_list`, `..._on_a_listening_window_emits_at_once_and_a_bare_open_none`.
- **Rejected:** a URL query (needs a UI change and percent-encoding of root text); a "ready" command (a contract change). If the page ever sends `calls_list` before registering its listeners, this mechanism breaks silently; `calls-ui.md` D8 and this record move together.
- **Known limit (T-142 review 1 #5):** `listening` is reset only when a new window is built. A reload of the same window's page (no new build) leaves it `true`, so an open that arrives after the reload but before the reloaded page registered its listeners is emitted at once and can be lost. The app offers no reload entry of its own (a webview reload key is the only route); the reloaded page re-lists on load, so only an open in that window of time is at risk. Not fixed in T-142.
- **Don't:** emit `calls://open` straight after the build; reset `listening` anywhere but on a new window.

### I4 — Copy goes through the one clipboard writer of the process

- **What breaks if you violate it:** a second writer in the call part misses the FR-10 exclusion formats (P-011, research R18), and the transcript lands in clipboard history.
- **Where it is enforced:** `run()` builds one `WinClipboard` (`release_clipboard`) and passes the same `Arc<dyn Clipboard>` to `release_ports` (dictation) and `release_call_ports` (`CallPorts.clipboard`). `calls_copy_markdown` writes exactly `CallStore::export_bytes(Md)` with `set_text_excluded_from_history`, once, with no retry; a failed write is `ioFailed`. Tests: `call_ipc.rs::copy_writes_the_markdown_once_through_the_one_clipboard_port`, `copy_through_the_real_clipboard_carries_the_text_and_the_three_exclusion_formats`.
- **Don't:** build a second `WinClipboard` or write the clipboard from the call part by another route.

### I5 — Export takes its target only from the save dialog; a cancel writes nothing

- **Order:** `calls_export` calls `export_bytes` first, so the guard (and the "no transcript" refusal) runs before any dialog; only then `SaveDialog::pick`. `None` (cancel) gives `{saved: false}` and neither `export_to` nor any file or tmp; a pick writes only through `export_to`; a dialog `Err` is `ioFailed`, never taken for a cancel.
- **Off the async runtime:** `calls_export` is an `async` command; the guard (`export_bytes`), `SaveDialog::pick` and the `export_to` write run in one `tauri::async_runtime::spawn_blocking` task, never on a runtime worker (the modal dialog blocks until the user answers, and `pick` joins its dialog thread; the `settings_test_connection` pattern, #42). A blocking task that cannot finish (panic, cancel) is `ioFailed{other}`. T-142 review 1 #2.
- **The dialog:** `win::save_dialog::WinSaveDialog::pick` shows `IFileSaveDialog` on a short-lived "calls-save-dialog" thread per pick, in a COM STA, never on the main thread or a tauri worker. It has no owner window (the port's contract carries none; `Show(None)`). UX effect: the dialog is not modal to the Calls window, it may open behind other windows, and Export can be clicked again while one is open; each export then runs on its own blocking thread and its own dialog. T-142 review 1 #3. `shown` is the one mapping: `HRESULT_FROM_WIN32(ERROR_CANCELLED)` is no pick; any other failure is `Err` carrying the OS code only (#45).
- **Where it is enforced:** `call_ipc.rs::a_cancelled_save_dialog_returns_saved_false_and_writes_nothing`, `a_failing_save_dialog_is_io_failed_not_a_cancel_and_writes_nothing`, `a_picked_target_gets_exactly_the_export_bytes_of_the_format`, `a_foreign_root_or_a_bad_id_is_not_found_and_no_port_is_touched`; `save_dialog.rs::show_maps_a_pick_a_cancel_and_a_failure`. The real dialog is not driven headless: review only.
- **Don't:** show the dialog before the guard; map any `Err` to a cancel; write to a default target on a cancel; call `pick` (or make `calls_export` a sync `#[tauri::command(async)]` body) on a runtime worker.

### I6 — `calls://changed` goes to the `calls` window only, after the session lock

- **Where it is enforced:** core `CallShell::changed(root, id)` is called after the session lock at Started and at the one stop-notice path (`notice_stopped`), so worker and tick-thread stops both pass through it, and never on a refused or repeated toggle. `ShellCallShell::changed` emits `calls://changed {root: wire_root, id}` with `emit_to("calls", ..)`; nothing hears it while no window exists. Tests: core `call_session` (`changed` cases, the lock-held flag), `call_ipc.rs::a_start_and_a_stop_emit_one_changed_each_to_the_calls_window_only` (subscribed first, matched on payload: F-018).
- **Don't:** use `app.emit` (every webview hears it); call `changed` under the session lock (a shell that takes it deadlocks); emit from the worker only (tick stops would be silent).

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| `tauri-plugin-dialog` for the save dialog | one more dependency (`make licenses`), a plugin init and a permission surface the page does not need; the `windows` crate is already linked | T-142 option B |
| `calls://changed` from the shell worker only | `Toggled` carries no call id and tick stops never reach the worker | T-142 option C |
| `calls://open` carried in the URL query, or a "page ready" command | a UI change with percent-encoding of root text; a contract change | T-142 analysis |

## Open

- The toast "Transcript ready → Open" (T-213) calls `open_calls(app, Some(CallRef))` through the toast activator (`win/toast.rs` `app_route`), with the root the notice carried; the argument holds only the call id, so a click after a restart only starts the app.
