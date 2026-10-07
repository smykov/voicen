# Contract: Diagnostics IPC (UI ↔ `src-tauri`) and shell entry points

The UI talks to Rust only through these commands (architecture "Seams"); Playwright mocks exactly these. Shapes use camelCase on the wire.

## Wire types

```ts
interface BuildInfo { version: string; commit: string }          // exists (src/lib/buildInfo.ts)
type OpenError = { code: "cannot_open"; reason: string }          // reason: OS error category, no paths outside the app folder
```

## Commands

| Command | Args | Returns | Errors | Spec |
|---|---|---|---|---|
| `get_build_info` | — | `BuildInfo` | — (exists) | FR-001, FR-002 |
| `open_logs_folder` | — | `null` | `OpenError` → UI shows "cannot open the logs folder: <reason>" | FR-017 |
| `open_third_party_notices` | — | `null` | `OpenError` → "cannot open the license list: <reason>" | FR-033 |
| `open_project_page` | — | `null` | `OpenError` | FR-002 |

No command takes a path or URL: the targets are fixed in Rust (`AppPaths::logs()`, the bundled notices resource, the repository URL constant). The same Rust function behind `open_logs_folder` serves 001's tray item "Open logs folder" (P-011).

## UI component

`src/lib/about/About.svelte`: a modal dialog (`<dialog>` with `aria-labelledby`, closed by Esc and a Close button, focus returned to the opener) inside the settings window, opened by the "About Voicen" button on 004's General tab (Clarification Q1; req FR-18 "About dialog"). Shows `Voicen <version> (<commit>)` (`formatBuildInfo`, exists), "MIT License", buttons "Project page", "Third-party licenses", "Open logs folder"; `role="alert"` for "Cannot read build info" and open errors. All controls are buttons with accessible names (keyboard operable).

## Message catalog entries (added to 004's catalog, EN + RU)

| Key | English |
|---|---|
| `about.title` | About Voicen |
| `about.close` | Close |
| `about.license` | MIT License |
| `about.project_page` | Project page |
| `about.third_party` | Third-party licenses |
| `about.open_logs` | Open logs folder |
| `about.build_info_error` | Cannot read build info |
| `error.open_logs_folder` | Cannot open the logs folder: {reason} |
| `error.open_notices` | Cannot open the license list: {reason} |
| `notice.logs_unwritable` | Logs cannot be written: {reason} |
| `tray.open_logs` | Open logs folder (item owned by 001; text listed here for completeness) |

## Shell entry points (Rust, `src-tauri`, Windows CI only)

| Function / flag | Called by | Does | Spec |
|---|---|---|---|
| `main()` `--purge-credentials` | uninstaller hook | before Tauri and single-instance: delete all Credential Manager entries with `CREDENTIAL_TARGET_PREFIX`; exit 0, or 2 on any failure; writes nothing to the log | FR-021, FR-022 |
| `diag::start(logs_dir: PathBuf, on_unwritable: OnUnwritable) -> Arc<Log>` (`src-tauri/src/diag.rs`; T-008) | `run()`, with `paths::log_dir()`, as the first startup side effect of `assemble`'s `parts`, which run only after tauri's `build()` has run the single-instance plugin's setup, so only the primary opens the log (T-052, decision #64) | `Log::open(logs_dir, SystemClock, OsLocalOffset, LogConfig::default(), on_unwritable)` (`OsLocalOffset` is the Windows time-zone `LocalOffset`), which never fails and runs retention; then writes `Started` as the first line. `assemble`'s wiring manages the returned `Arc<Log>`, and every later shell line is written to it. Not built yet: the session marker (`Session::begin`), the panic hook, the native-fault filter and crash-file retention (later tasks; they take the same `Clock` + `LocalOffset`, contracts/core-diag.md "Clock") | FR-003; FR-011..FR-016 later |
| `diag_exit(app, reason)` | 001's tray Exit and `WM_ENDSESSION` handler | `Session::end_clean` | FR-013 |
| `open_logs_folder(app)` | tray item (001, built by T-071 as `src-tauri` `logs_folder::request`), IPC | create dir if missing, open via the `FolderOpener` port: `ShellExecuteExW` "open" on the directory (T-071, `docs/decisions/windows-shell.md`; replaces `tauri-plugin-opener` for this action) | FR-017 |
| `webview_window(app, label, route)` | every feature creating a window | `WebviewWindowBuilder` with `data_directory(AppPaths::webview())` | FR-034 |
