# Installer and uninstaller

**Code:** `src-tauri/windows/installer.nsi` (fork of the tauri-cli NSIS template), `src-tauri/windows/installer-hooks.nsh`, `src-tauri/tauri.conf.json` › `bundle.resources`, `bundle.windows.nsis`, `package.json` / `pnpm-lock.yaml` › `@tauri-apps/cli` · **Tests that pin it:** `make check-nsis-fork` (`scripts/ci/nsis-fork.sh`, `nsis-fork.test.sh`); the `windows` job of `.github/workflows/ci.yml`: size and notices before the first launch (W1), the same-version `/P` reinstall (W2), `uninstall.exe /S /KEEPDATA` (W3), `uninstall.exe /S` (W4)

Tasks: T-025 (this seam), T-061 (`voicen.exe --purge-credentials`), T-062 (WebView2 data, open). Decisions: #10, #76, #98. Spec: `specs/006-diagnostics-and-release` (US4, `contracts/installer-ci.md`, research R9/R10). Open question: OQ-19.

## Invariants

### User data is removed at exactly one decision point, the PREUNINSTALL hook

User data is `%LOCALAPPDATA%\Voicen` and every Credential Manager entry whose target starts with `voicen_core::secrets::CREDENTIAL_TARGET_PREFIX` (`Voicen/`). `NSIS_HOOK_PREUNINSTALL` decides once (`$VoicenPurgeData`):

| Uninstall | Data | Question |
|---|---|---|
| started by an installer (`/UPDATE /P`, fork edit 1) | kept | none |
| `/KEEPDATA` (with `/S`, `/P` or interactive) | kept | none |
| `/S`, or `/P` started by the user (OQ-19 (a)) | removed | none (default Yes) |
| interactive | removed on Yes | Yes/No, default Yes, EN/RU (`LangString` 1033/1049) |

On Yes the hook runs `"$INSTDIR\voicen.exe" --purge-credentials` while the exe still exists (the stock section deletes it next), and `NSIS_HOOK_POSTUNINSTALL` runs `RMDir /r "$LOCALAPPDATA\Voicen"` (never when `$LOCALAPPDATA` is empty) and removes the installer bookkeeping key `HKCU\${MANUPRODUCTKEY}`. Nothing else in the uninstaller deletes anything outside the install folder and `%LOCALAPPDATA%\Voicen`. The Run value `Voicen` (T-014/T-034) and the shortcuts stay with the stock template: removed on every uninstall without `/UPDATE`, kept with it.

- **Defect that produced it:** found in the T-025 analysis (no F-entry; nothing shipped). In the stock tauri-cli 2.12.1 template an installer passes `/UPDATE` to the old uninstaller only when it got `/UPDATE` itself (the updater plugin, which Voicen does not use). So every manual upgrade ran the old uninstaller as a real uninstall: it deleted the Start-with-Windows Run value, deleted and recreated the shortcuts and showed the confirm page with a "Delete the application data" checkbox; a data question keyed on `$UpdateMode` would have purged settings, models and keys on its default Yes. Under #98 every CI build has a new version, so every Telegram build installed over another is such an upgrade.
- **What breaks if you violate it:** an upgrade deletes the user's settings, models and saved keys, or turns off Start with Windows (FR-024); or a second deletion path removes something outside the folder (FR-023).
- **Where it is enforced:** fork edit 1 (`PageLeaveReinstall` always appends ` /UPDATE /P` to the NSIS uninstall string, the only place an installer starts the uninstaller; the WiX branch is untouched), fork edit 2 (stock checkbox and its removal block deleted), the hooks file. CI: W2 (a same-version `/P` reinstall runs the old uninstaller and must keep data, credential and Run value within 120 s), W3, W4. The `/UPDATE /P` mark is added by the **new** installer, so it also reaches the stock uninstallers of builds installed before T-025.
- **Don't:** key a deletion on anything but the hook's decision; add data removal to the fork; let the hook ask under `/P` or `/S` (a passive question blocks: `/SD` applies only to silent); delete `$INSTDIR` recursively (the interactive directory page may point it elsewhere; the data folder is the literal `%LOCALAPPDATA%\Voicen`, `paths::data_dir`).

### The fork follows exactly one tauri-cli version

- **Defect that produced it:** none yet (decision #76). The fork replaces the bundler's own template; a `pnpm install` that moved `@tauri-apps/cli` to another 2.x would build installers from a template written for a different bundler (other handlebars variables, other plugins).
- **What breaks if you violate it:** a silent mismatch between the fork and the bundler: a broken or subtly wrong installer.
- **Where it is enforced:** `make check-nsis-fork`: the header line `; upstream: tauri-cli X.Y.Z crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi` in the first 20 lines of the fork, the exact specifier in `package.json`, the lockfile's specifier and version, and `bundle.windows.nsis.template` must agree.
- **Don't:** bump `@tauri-apps/cli` without re-forking: take the new upstream `installer.nsi` (tag `tauri-cli-vX.Y.Z`, or the text embedded in the CLI binary), re-apply the two edits listed in the fork's header, update the header version, then run the Windows job.

### Every install records its language, silent installs included

- **Defect that produced it:** CI run 37765499554 (T-025, wip, before review): the same-version `/P` reinstall (W2) hung for 120 s. With two languages loaded (English, Russian), the uninstaller's `un.onInit` (`MUI_UNGETLANGUAGE`) finds no `HKCU\${MANUPRODUCTKEY}` "Installer Language" value and calls `MUI_LANGDLL_DISPLAY`. That macro is skipped only under `/S`, and LangDLL shows its language dialog whenever more than one language is visible (NSIS `Contrib/LangDLL/LangDLL.c`, `visible_langs_num > 1`). MUI writes the value only from the instfiles page's leave function (`MUI_LANGDLL_SAVELANGUAGE` in `Pages/InstallFiles.nsh`), which never runs in a silent install. With the stock single language the dialog never showed, which is why the gap appeared only with `languages`.
- **What breaks if you violate it:** after a silent install, every passive uninstall (the one each reinstall or upgrade starts with `/UPDATE /P`, or a user's `/P`) waits on an "Installer Language" dialog; an interactive uninstall shows an extra dialog.
- **Where it is enforced:** `NSIS_HOOK_POSTINSTALL` writes `$LANGUAGE` to `MUI_LANGDLL_REGISTRY_ROOT`/`_KEY`/`_VALUENAME` (the template's own defines) on every install; CI W2.
- **Don't:** remove that hook while more than one language is listed; rely on the uninstaller's hooks (they run after `un.onInit`).

## Residual risks

- **Language value missing.** If the "Installer Language" value is gone (deleted by hand, or an install from a build without the POSTINSTALL hook, e.g. afd5352), a passive uninstall still waits on the language dialog: `un.onInit` runs before any hook.

- **Downgrade to a pre-T-025 build.** Installing an older (stock-template) build over a T-025 build runs our uninstaller without `/UPDATE`, so the question shows with default Yes. That is the old installer's code and cannot be fixed; the owner can answer No.
- **WebView2 data** goes to `%LOCALAPPDATA%\dev.voicen.app` until T-062 lands, and nothing removes it: the stock checkbox was the only path that could, and it deleted outside the folder (FR-023).
- **`$LOCALAPPDATA` vs `%LOCALAPPDATA%`.** NSIS takes `$LOCALAPPDATA` from the shell folder, the app takes the env var; they are equal on a normal profile (assumption).
- **App running at uninstall.** The purge runs before the stock "app is running" check (PREUNINSTALL is the last point where `voicen.exe` exists). If the user then cancels closing the app, the uninstall aborts with the keys already removed.
- **Language IDs before `MUI_LANGUAGE`.** The hooks file is included before the language macros, so its `LangString`s use 1033/1049; makensis on the Windows runner is the proof that this compiles.

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| Hooks only, keyed on `$UpdateMode` | the stock template sets it only from the updater plugin: a manual upgrade purges on the default Yes | decisions.md #76 |
| Hooks + an environment marker set by the installer (option A) | stock confirm page and checkbox stay visible on upgrade, the checkbox deletes outside the folder, the Run value would need re-creating | decisions.md #76 |
| Install the program to `%LOCALAPPDATA%\Programs\Voicen` | does not address upgrade detection | decisions.md #10, research R9 |
| CI proves upgrade by installing two versions silently | silent installs never run the old uninstaller, so the check cannot fail | decisions.md #76 |

## Open

- OQ-19: a user-run `uninstall.exe /P` — implemented as (a), like `/S`.
