# Contract: installer, uninstaller, CI jobs and release

Interfaces seen from outside the app: command-line switches, files, CI job names and their pass/fail conditions. The `windows` job smoke step asserts `voicen.exe`, `uninstall.exe` and `Voicen.lnk` at the paths below (prints `found <name> at <path>` or fails with `<name> not found at <path>`). Items marked **confirm on Windows** are Tauri/NSIS behaviour proven by the first Windows task (research.md).

## Installer (`Voicen_<version>_x64-setup.exe`, Tauri NSIS bundler)

| Aspect | Contract | Spec |
|---|---|---|
| Install mode | `bundle.windows.nsis.installMode = "currentUser"` (exists); no UAC prompt | FR-018 |
| Install dir | `%LOCALAPPDATA%\Voicen` (Tauri default) — observed in run 37044674209 (commit 0e73b52), `voicen.exe` there; recorded by `docs/tasks/T-029.md` | R9 |
| Shortcut | Start-menu shortcut with AppUserModelID = `identifier` (`dev.voicen.app`); `Voicen.lnk` under `%APPDATA%\Microsoft\Windows\Start Menu\Programs` — path observed in run 37044674209 (commit 0e73b52), recorded by `docs/tasks/T-029.md` (AppUserModelID not asserted by CI, unconfirmed) | FR-018 |
| Bundled resources | `THIRD-PARTY-NOTICES.txt` (`bundle.resources` map `{"../THIRD-PARTY-NOTICES.txt": "THIRD-PARTY-NOTICES.txt"}`, installed next to `voicen.exe`; T-025), Silero VAD model (path from 001, T-043) | FR-020 |
| Not bundled | whisper models, PDB | FR-020, R5 |
| Languages | `languages = ["English", "Russian"]`, `displayLanguageSelector = false` | FR-025 |
| Silent | `/S` | FR-027 |
| Size | sum of installed files ≤ 100 MB (measured before first launch) | FR-019 |
| Template | `bundle.windows.nsis.template = "windows/installer.nsi"`: a minimal-diff fork of the tauri-cli 2.12.1 template (header `; upstream: tauri-cli 2.12.1 …`), `@tauri-apps/cli` pinned exactly; edits: `PageLeaveReinstall` always passes `/UPDATE /P` to the old NSIS uninstaller, the stock "Delete the application data" checkbox removed; `make check-nsis-fork` fails when the fork's version, `package.json` and the lockfile differ (decisions #76, T-025, `docs/decisions/installer.md`) | FR-023, FR-024 |
| Hooks | `bundle.windows.nsis.installerHooks = "windows/installer-hooks.nsh"`: `NSIS_HOOK_PREUNINSTALL` (the data decision, `voicen.exe --purge-credentials`) and `NSIS_HOOK_POSTUNINSTALL` (`RMDir /r "$LOCALAPPDATA\Voicen"`, bookkeeping key) (T-025); `NSIS_HOOK_POSTINSTALL`: record the installer language (`HKCU\${MANUPRODUCTKEY}` "Installer Language"), also on silent installs, so a passive uninstall shows no language dialog (T-025); `NSIS_HOOK_PREINSTALL`: write `logs\installer-ended` (T-024, not yet) | FR-013(b), FR-021, FR-022 |

## Uninstaller (`%LOCALAPPDATA%\Voicen\uninstall.exe` — observed in run 37044674209 (commit 0e73b52), recorded by `docs/tasks/T-029.md`)

| Invocation | Data folder | Credentials | Autostart | Spec |
|---|---|---|---|---|
| interactive, answer Yes (default button) | removed | removed | removed | FR-021 |
| interactive, answer No | kept | kept | removed | FR-021 |
| `/S` | removed | removed | removed | FR-021 (Q4) |
| `/S /KEEPDATA` | kept | kept | removed | FR-021 (Q4) |
| `/P` started by the user | removed (as `/S`; `/P /KEEPDATA` keeps) | removed | removed | FR-021, OQ-19 (a) |
| update mode: the old uninstaller run by an installer, always with `/UPDATE /P` (forked template; upgrade, downgrade, same-version reinstall) | kept, no question | kept | kept | FR-024 |

Question text (`LangString VoicenRemoveData`): EN "Remove settings, history, models, logs and saved keys?" / RU "Удалить настройки, историю, модели, журналы и сохранённые ключи?".
Failure texts: EN "Saved keys could not be removed. Remove the entries starting with <prefix> in Windows Credential Manager." / "Some files in %LOCALAPPDATA%\Voicen could not be removed."; RU equivalents.

`voicen.exe --purge-credentials`: exit 0 = all entries with the prefix removed (or none existed); 2 = at least one failed. No window, no log, no marker.

Autostart value: the `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value name is the constant owned by 004 (FR-19): `src-tauri` `autostart::RUN_VALUE_NAME` = `Voicen` (T-014) = the template's `${PRODUCTNAME}`. The stock template code deletes that value on every uninstall without `/UPDATE` and keeps it with `/UPDATE` (an installer-started uninstall); the hook has no autostart code. An absent value is not an error.

## CI (`.github/workflows/ci.yml`)

| Job | Runs on | Trigger | Must pass | Spec |
|---|---|---|---|---|
| `gate` | ubuntu-24.04 | push `main`, tags `v*`, PRs | `make check` (incl. `licenses-check`, `version-check`) | FR-031, FR-032 |
| `windows` | windows-latest | same, after `gate` | `cargo test --workspace --exclude voicen`, then `cargo test -p voicen` (the shell in its own invocation, F-001) (incl. `crash_probe` tests); `pnpm tauri build`; size ≤ 100 MB; silent install; launch → `(<commit>) started` and `%LOCALAPPDATA%\Voicen\settings.json` (parses, `engine` = `none`; spec 004 T028's settings half, T-030) and a leftover `HKCU\…\Run\Voicen` value seeded before the launch removed (default off; T-014) and exactly one shown window, titled `Voicen` (the first-run settings window, T-037), within 30 s; then `start_with_windows` set to `true` in `settings.json` and `voicen.exe --autostart` launched → the Run value `Voicen` equals `"%LOCALAPPDATA%\Voicen\voicen.exe" --autostart` within 30 s, set back to `false` and relaunched → the value absent within 30 s (T-014 reconcile at start); a plain relaunch with that loaded `settings.json` → its start line within 30 s, then zero shown windows and the process still running for 10 s (T-037). A shown window is a visible (`IsWindowVisible`), unowned, top-level window of the process that is not a tool window (`WS_EX_TOOLWINDOW`) and not of the class `Tao Thread Event Target`; one predicate for both checks, `scripts/ci/visible-windows.ps1`. `Process.MainWindowHandle` cannot decide it: every tauri process has tao's event-target window, top-level, unowned and with `WS_VISIBLE` set (tao 0.37.1 `event_loop.rs:629-687`); kill → relaunch → `previous session ended abnormally` + one `abnormal_end` crash file (T-024, not yet); after every launching step (T-025): a same-version `setup.exe /P` reinstall (it runs the old uninstaller) finishes within 120 s and keeps a marker file, `settings.json`, a `Voicen/` test credential and the Run value; `uninstall.exe /S /KEEPDATA` keeps data + test credential and removes the Run value and the shortcut, then a silent reinstall; `uninstall.exe /S` removes the folder, every `Voicen/` credential, the Run value, the shortcut and the uninstall key and keeps a credential outside the prefix (the uninstaller re-runs from `%TEMP%`: both uninstall steps poll for 60 s); test credentials are planted and observed only through `scripts/ci/credentials.ps1`; artifacts `voicen-installer-<commit>`, `voicen-symbols-<commit>` | FR-019, FR-021, FR-022, FR-024, FR-027, FR-028 |
| `release` | ubuntu-24.04 | tags `v*` only, after `gate` + `windows` | version check; release does not exist; `gh release create` with installer, `SHA256SUMS.txt`, symbols zip, notes | FR-029, FR-030 |

Note (T-014, T-037): every smoke launch ends with a forced kill (`Stop-Process -Force`): the first launch, the on/off relaunches (T-014) and the loaded launch (T-037). Each leaves an `abnormal_end` crash file; 006's future check for exactly one `abnormal_end` crash file must account for them or run before them.

On failure of the start-line check the job prints the log if it exists (nothing if the log file is absent), then fails. A missing first-run `settings.json` fails with `settings.json not found at <path> after the first launch`, another engine with `first-run settings.json has engine '<value>', expected 'none'`; on success the step prints `found settings.json at <path> with engine = none`. A Run value still present after the first launch fails with `the leftover HKCU Run value Voicen is still present after the first launch (start_with_windows off)`; the on/off step fails with `HKCU Run value Voicen is '<value>', expected '<command>'` or `HKCU Run value Voicen still present ('<value>') with start_with_windows off` (T-014). Anything but exactly one shown window titled `Voicen` after the first launch fails with `not exactly one shown window titled Voicen after the first launch (last seen: <shown windows>)`; on success the step prints `exactly one shown window after the first launch: <shown windows>`. `<shown windows>` is `none`, `process exited` or each window as `class '<class>', title '<title>'`, joined by `; `. The loaded launch fails with `settings.json not found at <path> before the loaded launch`, `start log line with commit <commit> not written by the loaded launch`, `voicen.exe exited during the loaded launch (expected it to keep running with no window)` or `a window is shown on a launch with a loaded settings.json (<shown windows>)`; on success it prints `no shown window for 10 s after the start line of a launch with a loaded settings.json` (T-037). The 10 s check is a bounded negative: a window slower than 10 s would be missed, a window that is not there is never reported.

## Scripts

| Script | Contract |
|---|---|
| `scripts/ci/visible-windows.ps1` (pwsh, Windows; dot-sourced by the install smoke) | `Get-ShownWindows <pid>` returns the shown windows of the process (`Class`, `Title`): visible, unowned, top-level, tool windows included; excluded only when its class is a `visible-helper` of `scripts/ci/helper-windows.txt` and its ex-style has all four helper bits (user32 `EnumWindows`; T-065, docs/decisions/overlay.md §5); `Get-HelperDrift` fails the smoke on an unlisted or stale helper, its drift line naming both causes (a new framework helper: add an entry to the manifest; a product window shown at start: F-003, never list it); `Format-ShownWindows` renders them for the log (T-037) |
| `scripts/check-version.sh [<expected>]` | exits non-zero when the versions in `Cargo.toml` (workspace), `package.json` (and `tauri.conf.json` if it still has one) differ from each other or from `<expected>`; prints both values |
| `scripts/licenses/check.mjs`, `notices.mjs` | read the accepted list from `about.toml`; check the npm packages of the client bundle (written by the Vite plugin) and `licenses/manual.json`; fail naming each component with an unaccepted/unknown license; `notices.mjs` renders `THIRD-PARTY-NOTICES.txt` (decisions #24, #29) |
| `make licenses` / `make licenses-check` | regenerate `THIRD-PARTY-NOTICES.txt` / fail if regeneration differs from the committed file or any license is unaccepted |
