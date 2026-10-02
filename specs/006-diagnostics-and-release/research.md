# Research: Diagnostics and Release

Phase 0 of [plan.md](plan.md). The stack is fixed by `docs/requirements.md` §9 (Tauri 2, Rust 1.99, `windows` crate, NSIS per-user installer via the Tauri bundler, Svelte 5, Playwright with mocked IPC, GitHub Actions `windows-latest`); this file decides only HOW within it. Items marked **confirm on Windows** depend on Tauri/NSIS behaviour that cannot be observed on the Linux host; the first task touching them (T002, T026, T027, T029, T032 in tasks.md) proves or corrects them on the Windows runner before code builds on them. Context7 was unavailable in this session, so Tauri bundler details below come from the Tauri 2 documentation as known at writing time and are flagged accordingly.

## R1 — Log writer: own small writer, not `tracing`/`log` backends

- **Decision**: A small log writer in `voicen-core` (`diag::log`): typed `LogEvent` enum → one formatted `key=value` line, appended under a mutex to `logs/voicen.log`, with size/date rolling and retention done by the same writer. No `log`/`tracing` subscriber is installed in the app, so third-party library log output is dropped (never reaches the file).
- **Rationale**: FR-007/P-009 require an allowlist of typed fields and that library output never reaches the file. `tracing-appender` rotates only by time (no size cap, no total cap, no age-based deletion), so FR-008 would need custom code anyway; a subscriber would also accept arbitrary formatted strings (`info!("{text}")`), which is exactly the leak class P-009 names. With an enum of typed events there is no field that can carry text — the invariant holds by construction (P-002).
- **Alternatives**: `tracing` + `tracing-appender` + a custom layer (more deps, the allowlist becomes a filter, not a type); `flexi_logger` (size + age rotation, but string-based records).

## R2 — Line format and time

- **Decision**: `2026-10-02T14:03:11.123+03:00 INFO  dictation engine=api model=whisper-1 outcome=delivered press_to_frame_ms=42 stop_to_text_ms=812 text_to_paste_ms=35`. Fields are `key=value`, values without spaces are bare, values that are free-but-allowlisted identifiers (model names, host names, file names) are quoted and escaped; absent timings are omitted. UTF-8, `\n` line ends (Notepad on Windows 10 1809+ reads them). The start line keeps the text the CI smoke greps: `voicen <version> (<commit>) started os=<version> pid=<pid>`.
- **Time source**: a `WallClock` trait returning local time with its UTC offset (`chrono::DateTime<FixedOffset>`); the shell implements it with `chrono::Local`, tests use a fake. `chrono` (MIT/Apache-2.0) is added to `voicen-core`. If feature 001's `platform::Clock` already provides wall time with offset, the two are merged in the first task that lands second (no second clock — P-011).
- **Alternatives**: UTC only (harder for the owner to read against "I dictated at 14:05"); `time` crate (local offset is refused in multi-threaded Unix processes, which breaks core tests on Linux).

## R3 — Rolling and retention

- **Decision**: Active file `voicen.log`; it is renamed to `voicen-YYYYMMDD-HHMMSS.log` (time of the roll) when it reaches 2 MB or the local date changes. At start and after each roll, retention deletes (1) rolled files whose last-write time is older than 7 days, then (2) the oldest rolled files while the total of `voicen*.log` exceeds 10 MB. The active file is never deleted by retention. Crash files (`crash-*.txt`) and the session marker are not matched by these globs (FR-008).
- **Why 2 MB**: with a 10 MB total the active file plus at least four rolled files fit, so a burst rolls and drops the oldest instead of overflowing; at the expected volume (~200 bytes per dictation, a few hundred dictations a day) a day fits in one file.
- **Clock jumps**: age is computed from file times against the current clock; a jump can make one pass delete or keep files one pass early or late; the active file is protected, so a jump never empties the log.

## R4 — Log directory not writable

- **Decision**: The writer has a `Degraded` state: any failed create/open/write sets it, drops the line, and raises a one-shot `LogsUnwritable { reason }` signal per session (Clarification Q3). The shell maps the signal to 001's notifier with message key `notice.logs_unwritable`. On every later write the writer retries opening the file at most once per 60 s; success clears the degraded state silently. Reasons are categories: `permission_denied`, `disk_full`, `not_a_directory`, `other(<os error code>)`.
- **Rationale**: FR-009 — never block or crash, notify once, recover silently.

## R5 — Panic crash files

- **Decision**: `diag::crash::install_panic_hook(ctx)` in `voicen-core` (std only, testable on Linux): the hook ignores the payload (never formats the message — Clarification Q2), captures `std::backtrace::Backtrace::force_capture()`, and writes `crash-YYYYMMDD-HHMMSS-<pid>.txt` with the FR-014 fields, then tries one log line `crash_file_written`. It writes with plain `std::fs` to a path computed at install time, guards against re-entry with an atomic flag, and never panics (all errors ignored). The release profile keeps `panic = "abort"`: the hook runs before the abort.
- **Backtrace without symbols**: the release profile has `strip = true` and no debug info, so frames resolve to addresses only. The hook writes each frame as `<module>+0x<offset>` (module base from the frame's instruction pointer; on Windows via `GetModuleHandleExW` behind a small `ModuleResolver` trait, on Linux a fake). CI builds with `debug = "line-tables-only"` and `split-debuginfo = "packed"` for the release profile so MSVC emits `voicen.pdb` beside the exe; the PDB is uploaded as a CI artifact and attached to the GitHub Release as `voicen-<version>-symbols.zip`, not shipped in the installer. **confirm on Windows** (T026): installer size and that `strip` does not remove the PDB generation on MSVC.
- **Alternatives**: `backtrace` crate with symbol resolution (needs symbols shipped: +size, exposes nothing extra the PDB route cannot); minidumps via `MiniDumpWriteDump` (rejected: a dump contains process memory, i.e. audio, transcript text and possibly keys — violates FR-014/NFR-04).

## R6 — Native faults

- **Decision**: In `src-tauri` (`win/crash.rs`), `SetUnhandledExceptionFilter` installs a filter that writes a crash file of kind `native_fault` with the exception code, the faulting address as `<module>+0x<offset>`, version, commit and session start — using a buffer and a wide path prepared at install time and `CreateFileW`/`WriteFile` only (no allocation, no Rust formatting machinery inside the filter), then returns `EXCEPTION_CONTINUE_SEARCH` so Windows Error Reporting proceeds as usual. Stack overflows and faults that bypass the filter (e.g. fail-fast `__fastfail`) produce no file; the session marker covers them at the next start (FR-016).
- **Test**: a `crash_probe` binary in `src-tauri` behind the cargo feature `crash-probe` installs both handlers through the same functions the app uses and then panics or dereferences null depending on its argument; a Windows-only integration test runs it with a temporary `LOCALAPPDATA` and asserts the crash file and its allowlisted content. The shipped app has no crash trigger.

## R7 — Session marker and "ended by the installer"

- **Decision**: `logs/session.marker` (key=value: `pid`, `started`, `version`, `commit`) written by the primary instance after the single-instance check and before the main loop; deleted by `Session::end_clean()`, which the shell calls on tray Exit and on `WM_ENDSESSION` with `wParam = TRUE` (001's hidden window receives session messages — 001 plan). At the next start `Session::begin()` classifies the previous session:
  1. marker absent → clean;
  2. a crash file whose `session_started` equals the marker's `started` exists → `previous session crashed` (log line naming the file, no new crash file);
  3. `logs/installer-ended` exists with a time ≥ the marker's `started` → `previous session was ended by the installer`, no crash record;
  4. otherwise → crash file of kind `abnormal_end` + log line `previous session ended abnormally`.
  Then the old marker and the installer note are deleted and the new marker is written. A second instance never reaches `Session::begin()` (FR-01 single instance runs first — ordering pinned by a shell test).
- **Installer note**: the NSIS pre-install and pre-uninstall hooks write `%LOCALAPPDATA%\Voicen\logs\installer-ended` (one line, the time) before Tauri's installer closes a running app; if the app was not running the note is harmless and is deleted at the next start.

## R8 — Data directory and web view data

- **Decision**: One resolver `AppPaths` (`voicen-core::paths`) built from a base directory (`%LOCALAPPDATA%` from the shell, a temp dir in tests): `root = <base>\Voicen`, `logs`, `models`, `tmp`, `webview`, settings and history file paths. If 001/004 introduce the resolver first, this feature consumes it and only adds `logs`/`webview` (P-010). The shell's existing `log_dir()` moves onto it.
- **Web view data**: by default Tauri 2 / WebView2 stores web view data under `%LOCALAPPDATA%\<identifier>` (`dev.voicen.app\EBWebView`), outside `%LOCALAPPDATA%\Voicen` — which breaks FR-28's "all app data lives in `%LOCALAPPDATA%\Voicen`" and FR-034. Every window is created through one shell helper that sets `data_directory(AppPaths::webview())`. **confirm on Windows** (T027): after launch and opening the settings window, `%LOCALAPPDATA%\dev.voicen.app` does not exist.

## R9 — Install directory

- **Decision**: Keep the Tauri NSIS `currentUser` default install location, `%LOCALAPPDATA%\Voicen` (the existing CI smoke already finds `voicen.exe` there) — **confirm on Windows** (T002). Program files and data then share a folder: "no" on uninstall removes only the files the installer wrote (NSIS removes its own file list) and keeps the data; "yes" additionally removes the whole folder. CI measures the installed size right after install, before first launch, so no data is counted.
- **Alternative (owner decision, see plan)**: install to `%LOCALAPPDATA%\Programs\Voicen` (the Windows convention for per-user programs), keeping data and program apart. It needs a custom NSIS template or install-dir override, which Tauri supports but costs maintenance on every Tauri upgrade. Not chosen for release 1; raised as a finding.

## R10 — Uninstaller question, credential removal, upgrade

- **Decision**: Tauri NSIS installer hooks (`bundle.windows.nsis.installerHooks = "windows/installer-hooks.nsh"` with macros `NSIS_HOOK_PREINSTALL`, `NSIS_HOOK_PREUNINSTALL`, `NSIS_HOOK_POSTUNINSTALL`):
  - `PREUNINSTALL`: write the installer note (R7). If the uninstaller runs in update mode (Tauri passes `/UPDATE` to the old uninstaller during an upgrade; the template exposes it as `$UpdateMode`) → skip everything below (FR-024). If the command line contains `/KEEPDATA` → answer "no". Otherwise `MessageBox MB_YESNO|MB_ICONQUESTION "$(VoicenRemoveData)" /SD IDYES` → silent default "yes" (Clarification Q4). On "yes": `ExecWait '"$INSTDIR\voicen.exe" --purge-credentials'` and remember the answer.
  - `POSTUNINSTALL`: on "yes" `RMDir /r "$LOCALAPPDATA\Voicen"`; if the folder still exists, show "Some files in %LOCALAPPDATA%\Voicen could not be removed." (silent: skipped). Always remove the autostart entry `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value written by 004 (value name shared with 004 — see contracts).
  - `--purge-credentials` is handled at the top of `main()` before the Tauri builder and the single-instance check: it enumerates Credential Manager entries with the key-slot target prefix owned by 004 (`CredEnumerateW` with filter `<prefix>*`) and deletes them, prints nothing sensitive, exits 0 or 2 on failure; on 2 the hook shows "Saved keys could not be removed. Remove the entries starting with `<prefix>` in Windows Credential Manager." The prefix constant lives in `voicen-core` (one source for app and uninstaller — FR-022, P-010).
  - Strings via `LangString` for English and Russian; `bundle.windows.nsis.languages = ["English", "Russian"]`, no language selector, so NSIS follows the Windows display language (FR-025).
- **Tauri's own "Delete the application data" checkbox**: the stock uninstaller page has it (unchecked by default; it deletes `%APPDATA%\<identifier>` and `%LOCALAPPDATA%\<identifier>`). With R8 those folders hold nothing, so it is harmless but redundant next to our question. **confirm on Windows** (T029) what the page shows; if the duplicate confuses, the follow-up is a custom NSIS template (owner decision, see plan).
- **Alternatives**: deleting credentials from NSIS through the `System` plugin calling `advapi32::CredDeleteW` per known name (duplicates the key-slot list in NSIS — P-010 violation); keeping Tauri's checkbox as the only question (its default is unchecked and its wording does not mention keys — contradicts FR-28's "default yes" and question text).

## R11 — Start-menu shortcut and toast identity

- **Decision**: The Tauri NSIS installer creates the Start-menu shortcut and sets its AppUserModelID to the bundle identifier `dev.voicen.app`; 001's toast code uses the same identifier (P-010: `tauri.conf.json` `identifier` is the source; the shell reads it from the Tauri config at runtime). **confirm on Windows** (T002): the shortcut exists after a silent install; the owner's manual check confirms a toast appears after a fresh install (001's verification).

## R12 — Version single source and release job

- **Decision**: The version lives in the workspace `Cargo.toml` (`[workspace.package] version`). `tauri.conf.json` drops its `version` field so the bundler takes the Cargo version (Tauri 2 falls back to the package version when `version` is absent — **confirm on Windows** T032 via the installer file name). `package.json` keeps a version for pnpm but `scripts/check-version.sh` fails the gate if it differs. The release job (`release` in `.github/workflows/ci.yml`): `if: startsWith(github.ref, 'refs/tags/v')`, `needs: [gate, windows]`, `permissions: contents: write`; it downloads the Windows job's installer and symbols artifacts, runs `scripts/check-version.sh "${GITHUB_REF_NAME#v}"` (mismatch → fail naming both versions, Clarification Q5), fails if `gh release view "$TAG"` succeeds ("release already exists"), writes `SHA256SUMS.txt`, and runs `gh release create "$TAG" <installer> SHA256SUMS.txt <symbols.zip> --title "Voicen $VERSION" --notes-file <generated notes>`. The notes template (in repo) carries the version, commit, checksum, the SmartScreen note (FR-030) and a link to the third-party notices.
- **Alternatives**: `softprops/action-gh-release` (third-party action, fewer explicit failure checks); the `tauri-action` (builds again inside the release job — the released binary would not be the one the smoke test installed, violating P-006).

## R13 — License check and third-party notices

- **Decision**:
  - **Rust**: `cargo-about` with `about.toml` listing the accepted SPDX licenses and `targets = ["x86_64-pc-windows-msvc"]` (the shipped target, evaluated on Linux). `cargo about generate` fails on a crate whose license is not accepted or cannot be determined (FR-031) and renders the notices from a Handlebars template (FR-032). Installed into the `voicen-rust:1.99` image (`cargo install cargo-about --locked`) — an install needing the owner's consent.
  - **npm**: `pnpm licenses list --prod --json` (built into pnpm, no new dependency) parsed by `scripts/check-npm-licenses.mjs`, which reads the accepted list from `about.toml` (one list — P-010) and appends the npm section to the notices.
  - **Non-package components**: `licenses/manual.toml` (hand-maintained): whisper.cpp (MIT, via `whisper-rs-sys`), Silero VAD model (MIT), the downloadable whisper ggml models (MIT, OpenAI Whisper weights; not bundled), NSIS (zlib/libpng, installer only).
  - **Output**: `THIRD-PARTY-NOTICES.txt` at the repo root, committed; `make licenses` regenerates it; `make check` regenerates into a temp file and fails if it differs from the committed one (FR-032 "out of date"). It is bundled via `bundle.resources` and opened from About.
  - **Accepted list** (proposal, owner to confirm): `MIT`, `Apache-2.0`, `Apache-2.0 WITH LLVM-exception`, `BSD-2-Clause`, `BSD-3-Clause`, `ISC`, `Zlib`, `0BSD`, `BSL-1.0`, `CC0-1.0`, `Unicode-3.0`, `Unicode-DFS-2016`, `MPL-2.0` (file-level copyleft; allowed only unmodified, noted in the notices), `CDLA-Permissive-2.0` (webpki root data). Not accepted: GPL-*, LGPL-*, AGPL-*, SSPL, unknown.
- **Alternatives**: `cargo-deny` for the check plus a separate notice generator (two tools, two lists); `license-checker` npm package (a new dependency where pnpm already has the command).

## R14 — CI "deployed" job extensions

- **Decision**: The Windows job keeps its existing steps (tests, `pnpm tauri build`, silent install, launch, start-line check) and adds, in order:
  1. installed size: sum of files under the install dir right after `/S` install and before launch; fail if > 100 MB (FR-019);
  2. launch, start line with commit (existing), then `Stop-Process -Force` (existing);
  3. relaunch → require `previous session ended abnormally` in the log and one `crash-*.txt` of kind `abnormal_end` (FR-013(c), deployed evidence for the "killed" acceptance); exit through a test-only `--exit` command-line request handled by the single-instance forwarder (clean exit) → relaunch → no new crash file (FR-013 clean path) — **if** 001's single-instance handler forwards arguments; otherwise this check stays in the Windows integration tests;
  4. reinstall the same installer silently → user data (log file) still present (FR-024);
  5. create a test credential with `cmdkey /generic:<prefix>ci-test /user:ci /pass:ci`; silent uninstall with `/KEEPDATA` → data folder and credential still present; reinstall; silent uninstall without switches → `%LOCALAPPDATA%\Voicen` absent, `cmdkey /list` shows no `<prefix>` entry (FR-021, FR-022);
  6. upload installer and `voicen.pdb` (zipped) as artifacts named with the commit.
  The uninstaller path is `%LOCALAPPDATA%\Voicen\uninstall.exe` — **confirm on Windows** (T002).
- **PRs from forks**: the job runs without secrets (none needed); the release job never runs for PRs.

## R15 — GitHub repository (prerequisite)

- **Decision**: `gh repo create <owner>/voicen --public --source=. --remote=origin --description "..."`, default branch `main`, Actions enabled with default `GITHUB_TOKEN` permissions read-only (the release job raises `contents: write` itself). Creating the repository and the first push are done by the owner or with the owner's explicit consent in an owner session (CLAUDE.md: never push from a DEV session). After the first push the Windows job's first green run is the evidence that closes decisions #4.
- **Not in scope**: branch protection rules, issue templates, code signing (OQ-02).

## R16 — "Open logs folder" and About actions

- **Decision**: `tauri-plugin-opener` (official Tauri plugin, MIT/Apache-2.0) called from Rust (`app.opener().open_path(...)`), so no JS-side opener permission is granted; IPC commands `open_logs_folder`, `open_third_party_notices`, `open_project_page` take no arguments (paths and the URL are fixed in Rust — the UI cannot open arbitrary paths). The tray item (001) and the About button call the same Rust function (P-011). The project URL is a constant set when the repository exists (R15).

## Verification placement (constitution V)

| Concern | Linux host — core | Linux host — UI (mocked IPC) | Windows CI runner | Owner's manual check |
|---|---|---|---|---|
| Build info resolver, start line text | unit tests | About shows values / failure | smoke: start line with commit | — |
| Log format, allowlist, redaction | unit + redaction test with planted text/key | — | log of the smoke run grepped for the planted test key (none) | — |
| Rolling, retention, degraded mode | fake clock + temp dir | — | read-only logs dir integration test | — |
| Panic crash file | hook test in a spawned thread | — | `crash_probe panic` | — |
| Native fault crash file | — | — | `crash_probe fault` | — |
| Session marker classification | all four cases | — | kill → relaunch in the smoke | shutdown/logoff while running → no crash record |
| Open logs folder / notices / project page | — | buttons call the commands | logs path resolution and folder creation (shell test) | Explorer opens; Notepad shows notices |
| Installer per-user, size, shortcut | — | — | silent install, size, shortcut exists | clean Windows 11 in Windows Sandbox, no UAC |
| Uninstaller yes/no/upgrade/credentials | — | — | `/KEEPDATA`, default, reinstall, `cmdkey` | interactive question text EN/RU |
| Release publishing | `scripts/check-version.test.sh` in the gate | — | tag on the repository | first real release |
| License check + notices | `make licenses-check` in the gate | — | — | notices reviewed once per release |
