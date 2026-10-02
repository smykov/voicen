# Contract: installer, uninstaller, CI jobs and release

Interfaces seen from outside the app: command-line switches, files, CI job names and their pass/fail conditions. The `windows` job smoke step asserts `voicen.exe`, `uninstall.exe` and `Voicen.lnk` at the paths below (prints `found <name> at <path>` or fails with `<name> not found at <path>`). Items marked **confirm on Windows** are Tauri/NSIS behaviour proven by the first Windows task (research.md).

## Installer (`Voicen_<version>_x64-setup.exe`, Tauri NSIS bundler)

| Aspect | Contract | Spec |
|---|---|---|
| Install mode | `bundle.windows.nsis.installMode = "currentUser"` (exists); no UAC prompt | FR-018 |
| Install dir | `%LOCALAPPDATA%\Voicen` (Tauri default; `voicen.exe` there asserted by the smoke step since T-002, confirmed by the first green run after the owner's push) | R9 |
| Shortcut | Start-menu shortcut with AppUserModelID = `identifier` (`dev.voicen.app`); `Voicen.lnk` under `%APPDATA%\Microsoft\Windows\Start Menu\Programs` asserted by the smoke step since T-002, confirmed by the first green run (AppUserModelID not asserted by CI) | FR-018 |
| Bundled resources | `THIRD-PARTY-NOTICES.txt`, Silero VAD model (path from 001) | FR-020 |
| Not bundled | whisper models, PDB | FR-020, R5 |
| Languages | `languages = ["English", "Russian"]`, `displayLanguageSelector = false` | FR-025 |
| Silent | `/S` | FR-027 |
| Size | sum of installed files ≤ 100 MB (measured before first launch) | FR-019 |
| Hook | `NSIS_HOOK_PREINSTALL`: write `logs\installer-ended` | FR-013(b) |

## Uninstaller (`%LOCALAPPDATA%\Voicen\uninstall.exe` — **confirm on Windows**)

| Invocation | Data folder | Credentials | Autostart | Spec |
|---|---|---|---|---|
| interactive, answer Yes (default button) | removed | removed | removed | FR-021 |
| interactive, answer No | kept | kept | removed | FR-021 |
| `/S` | removed | removed | removed | FR-021 (Q4) |
| `/S /KEEPDATA` | kept | kept | removed | FR-021 (Q4) |
| update mode (old uninstaller run by a newer installer) | kept, no question | kept | kept | FR-024 |

Question text (`LangString VoicenRemoveData`): EN "Remove settings, history, models, logs and saved keys?" / RU "Удалить настройки, историю, модели, журналы и сохранённые ключи?".
Failure texts: EN "Saved keys could not be removed. Remove the entries starting with <prefix> in Windows Credential Manager." / "Some files in %LOCALAPPDATA%\Voicen could not be removed."; RU equivalents.

`voicen.exe --purge-credentials`: exit 0 = all entries with the prefix removed (or none existed); 2 = at least one failed. No window, no log, no marker.

Autostart value: the `HKCU\…\Run` value name is the constant owned by 004 (FR-19); the hook deletes that value unconditionally (FR-023).

## CI (`.github/workflows/ci.yml`)

| Job | Runs on | Trigger | Must pass | Spec |
|---|---|---|---|---|
| `gate` | ubuntu-24.04 | push `main`, tags `v*`, PRs | `make check` (incl. `licenses-check`, `version-check`) | FR-031, FR-032 |
| `windows` | windows-latest | same, after `gate` | `cargo test --workspace` (incl. `crash_probe` tests); `pnpm tauri build`; size ≤ 100 MB; silent install; launch → `(<commit>) started` within 30 s; kill → relaunch → `previous session ended abnormally` + one `abnormal_end` crash file; reinstall keeps data; `/S /KEEPDATA` keeps data + test credential; `/S` removes folder + credential; artifacts `voicen-installer-<commit>`, `voicen-symbols-<commit>` | FR-019, FR-021, FR-022, FR-024, FR-027, FR-028 |
| `release` | ubuntu-24.04 | tags `v*` only, after `gate` + `windows` | version check; release does not exist; `gh release create` with installer, `SHA256SUMS.txt`, symbols zip, notes | FR-029, FR-030 |

On failure of the start-line check the job prints the log or "no log file" (exists).

## Scripts

| Script | Contract |
|---|---|
| `scripts/check-version.sh [<expected>]` | exits non-zero when the versions in `Cargo.toml` (workspace), `package.json` (and `tauri.conf.json` if it still has one) differ from each other or from `<expected>`; prints both values |
| `scripts/check-npm-licenses.mjs` | reads the accepted list from `about.toml`; fails naming each package with an unaccepted/unknown license; `--notices` prints the npm section |
| `make licenses` / `make licenses-check` | regenerate `THIRD-PARTY-NOTICES.txt` / fail if regeneration differs from the committed file or any license is unaccepted |
