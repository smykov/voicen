# Contract: Command line, installer and CI

## Command line (`voicen.exe`)

| Switch | Instance | Effect | Spec |
|---|---|---|---|
| `--call record` | running instance | the single-instance callback posts one `StartCallRecord` request; it enters the tray's Record path (refusals, indicator, tray state, reminder) | FR-003, FR-043, #184 |
| `--call record` | no instance running | the process becomes primary, starts normally (log start line, tray), then applies the switch once | spec Assumptions |
| `--call <other>` | either | ignored, `call_switch_refused reason=unknown_value` logged by the primary | — |
| `--autostart`, `--purge-credentials` | — | unchanged | 001, 006 |

There is no stop switch in 1a.

## Uninstaller

| Invocation | Data question (existing) | Calls question (new) | Default calls folder |
|---|---|---|---|
| interactive | asked as today | "Remove recorded calls in <folder>?", default **No**, asked only when `%LOCALAPPDATA%\Voicen\calls` exists | kept unless Yes |
| `/S`, `/P` | as today (`/S` removes data) | never asked; keep | **kept** |
| `/S /PURGECALLS`, `/P /PURGECALLS` | as today | never asked; remove | removed |
| `/S /KEEPDATA` | keep | keep | kept |
| `/S /KEEPDATA /PURGECALLS` | keep | never asked; remove (decisions #197) | removed, the rest of the data folder stays |
| bare `/PURGECALLS` | as `/P`: data removed, nothing asked (the fork's `/P` also matches it) | never asked; remove | removed. **Unconfirmed on the runner**: read in the NSIS 3.11 source, no CI step runs it (OQ-75, `docs/decisions/installer.md`) |
| update mode | keep | keep | kept |

- **Decision point**: `$VoicenPurgeCalls` is computed in PREUNINSTALL next to `$VoicenPurgeData`, the one decision point (installer.md). Question strings are LangStrings 1033/1049.
- **POSTUNINSTALL**: it never runs `RMDir /r`, anywhere. Every removal goes through the hooks' walker `un.VoicenRemoveEntry`, which removes a junction or symlink as a link only and never enters it (decisions #197; `docs/decisions/installer.md`). While calls are kept it removes the other entries one by one and leaves `calls\`.
- **Other roots**: no root other than the default is ever deleted (FR-041, SC-010).

## Windows CI (`.github/workflows/ci.yml`, windows job)

| Step | Assertion | Spec |
|---|---|---|
| `cargo test --workspace --exclude voicen` | core call tests (Linux ones also pass on Windows) + `voicen-whisper` `SpeechSpans` on the fetched Silero model | FR-023, R8 |
| `cargo test -p voicen` (`src-tauri/tests/call_*.rs`) | WAV-sourced chain capture → storage → final (local mock server) → merge; IMMNotificationClient register/unregister; loopback open on a device-less runner gives `no_device`; indicator window style bits (topmost, no taskbar, shown without activation) | FR-014, FR-010, FR-005 |
| `crt-imports.sh` | `voicen.exe` and the call test exes import no `MSVCP140` / `VCRUNTIME140` (the list gains the call exes with T-127 / T-139; the libopus `/MT` proof is `opus-cmake-cache.sh`, decisions #197) | R5 |
| `opus-cmake-cache.sh` (new) | opus `CMakeCache.txt`: `OPUS_STATIC_RUNTIME:BOOL=ON`, `CMAKE_BUILD_TYPE:STRING=Release`, no line naming `CMAKE_MSVC_RUNTIME_LIBRARY`; `dumpbin -directives` of each `opus.lib`: `/DEFAULTLIB:LIBCMT`, never `/DEFAULTLIB:MSVCRT` (decisions #197) | R5 |
| install smoke (existing step "Install silently, launch, check the start, settings-load and reconcile log lines, the first-run settings.json, the Run value and the settings window", extended) | after the start line: `voicen.exe --call record` → log has `call_requested source=switch` and `call_refused part=system reason=no_device` (the "no output device" refusal of FR-043; the session opens the system track before the mic, so this holds whatever capture endpoints the runner has), the process is still running, no call folder exists, the window census is unchanged | FR-043, FR-002 |
| W4 uninstall `/S` (existing step "uninstall.exe /S removes the data except calls\, every Voicen/ credential, the Run value and the shortcut, and keeps calls\ and other credentials (T-025, T-156)", extended) | a planted `calls\2026-01-01_000000\meta.json` exists after uninstall; the former assertion that the whole data folder is gone is now "only `calls\` is left" | FR-041 |
| W5 uninstall `/S /KEEPDATA /PURGECALLS` (new, after W4; step "uninstall.exe /S /KEEPDATA /PURGECALLS removes calls\ and keeps the rest of the data folder (T-156)") | `calls\` is gone; `settings.json` and `ci-keep.txt` remain; then a silent reinstall | FR-041, decisions #197 |
| W5b (new, added after the red run): a `/P` reinstall, a user-started `uninstall.exe /P`, and `uninstall.exe /S /KEEPDATA` | each keeps a planted `calls\2026-01-01_000000\meta.json` (`/P` reinstall and `/S /KEEPDATA` keep the data too; a user `/P` removes it as W4 does); then a silent reinstall. No red run of its own | FR-041 |
| W6 uninstall `/S /PURGECALLS` (new, after W5b; its step ends the sequence) | with a nested and a top-level junction planted, the data folder is gone and every sentinel file behind each junction, `$RUNNER_TEMP\calls-elsewhere` included, still exists | FR-041, SC-010, decisions #197 |

New CI tests run red first on `wip/<ID>` (P-016, windows-ci-runner.md). The runner has no capture-endpoint probe fact, so no step asserts real capture.

## Guard and owned files touched

**Guard trigger** (decisions #154, #155; `scripts/hooks/pre-commit`, tested by `make check-guard-trigger`): a staged path under `scripts/ci/` or `scripts/licenses/`, the path `Makefile`, or a path ending in `.mk`. Only such a change needs a task with `checker: true` and a non-empty `analysis.decides_on` (P-020). The hook reads paths only, so the other files below are owned files checked by their own `make check` targets, not guard changes.

| File | Guard change? |
|---|---|
| `scripts/ci/opus-cmake-cache.sh`, its test and fixtures, `Makefile` `check-opus-cmake-cache` | yes |
| `scripts/ci/crt-imports.sh` (and its test) | yes |
| `scripts/ci/shell-targets.txt`, `scripts/ci/helper-windows.txt` | yes, if changed (planned unchanged) |
| `scripts/ci/nsis-fork.sh` (if extended by G42) | yes |
| `.cargo/config.toml`, `docker/rust.Dockerfile`, `about.toml`, `licenses/manual.json`, `THIRD-PARTY-NOTICES.txt`, `.github/workflows/ci.yml` | no (owned files with their own checks, e.g. `licenses-check`, `check-whisper-cmake-cache`, and the `ci.yml` byte comparisons of `make check`) |

Planned changes:

| File | Change |
|---|---|
| `.cargo/config.toml` | `CMAKE_TOOLCHAIN_FILE_x86_64_pc_windows_msvc` -> `cmake/msvc-static-crt.cmake` (opus static runtime) — confirm it does not change whisper.cpp's cache (`whisper-cmake-cache.sh` stays green) |
| `scripts/ci/opus-cmake-cache.sh` + Makefile `check-opus-cmake-cache` + host fixtures | new checker |
| `scripts/ci/crt-imports.sh` exe list | call test exes (T-127 / T-139, not T-117) |
| `docker/rust.Dockerfile` | `cmake` (core image; also serves the mingw build of `check-shell-windows`) — approved (#186) |
| `scripts/ci/shell-targets.txt` | unchanged (new shell tests are `tests/*.rs`, not targets of lib/bin) — confirm with `make check-shell-layout` |
| `scripts/ci/helper-windows.txt` | unchanged; the indicator and Calls windows are product windows (overlay.md §5). If the spike shows a new framework helper window, it is added there with its evidence |
| `about.toml` / `licenses/manual.json` / `THIRD-PARTY-NOTICES.txt` | `opus`, `opusic-sys` (BSD-3-Clause, with the bundled libopus notice), `ogg` (BSD-3-Clause); `make licenses` |

## Licences of new components

| Component | Licence | Bundled C | Notice |
|---|---|---|---|
| `opus` 0.4 | MIT OR Apache-2.0 | — | cargo-about |
| `opusic-sys` 0.7 | BSD-3-Clause | libopus 1.6.1 (BSD-3-Clause) | cargo-about + libopus copyright in `licenses/manual.json` if not in the crate metadata |
| `ogg` 0.9 | BSD-3-Clause | — | cargo-about |
| LibriSpeech test-clean subset | CC BY 4.0 | — | not shipped; attribution in the WER report |
