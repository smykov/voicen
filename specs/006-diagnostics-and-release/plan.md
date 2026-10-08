# Implementation Plan: Diagnostics and Release

**Branch**: `006-diagnostics-and-release` | **Date**: 2026-10-02 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/006-diagnostics-and-release/spec.md`

## Summary

Make every build identifiable, every problem diagnosable and every crash countable, and ship the app: version and commit in About and in the first log line (req FR-18); a rotating local log with typed, allowlisted events and no text, audio or keys (req FR-20, NFR-04); crash files for panics and native faults plus a session marker that turns any abnormal end into a crash record (req FR-20, NFR-07); "Open logs folder"; the per-user NSIS installer ≤ 100 MB and an uninstaller that asks about user data (req NFR-09, FR-28); the Windows "deployed" job and `v*` release publishing (requirements §9); a license check and bundled third-party notices (req NFR-12); and, first of all, the public GitHub repository those jobs need (decisions #4).

Technical approach (see [research.md](research.md)):

- **Core (`crates/voicen-core`)**: `paths` (the one data-dir resolver), `diag::log` (own writer: typed `LogEvent` → one `key=value` line; roll at 2 MB or date change; 7 days / 10 MB retention; degraded mode with a once-per-session signal), `diag::session` (marker and previous-session classification), `diag::crash` (panic hook writing allowlisted crash files; crash retention 30 days / 20 files), `LogObserver` implementing 001's `PipelineObserver`. All tested on Linux with a temp dir and a fake clock.
- **Shell (`src-tauri`)**: wiring at start (after 001's single-instance check), native-fault filter, `--purge-credentials`, the opener-backed commands, a window helper that puts web view data under `%LOCALAPPDATA%\Voicen\webview`, the `crash_probe` test binary.
- **UI (`src/`)**: an About dialog opened from 004's General tab (req FR-18 "About dialog").
- **Packaging/CI**: NSIS hooks (`src-tauri/windows/installer-hooks.nsh`), bundle resources, `cargo-about` + `pnpm licenses` check and notices, version single-source check, extended Windows job, new `release` job.

## Technical Context

> Superseded in part (T-008): core gets no `chrono`, `WallClock`, `diag/clock.rs`, `HostName` or `ModelName`. Time comes from the existing `clock::Clock` plus a `clock::LocalOffset` port that the shell fills with the `windows` crate. No host or model name is logged (decision #64). The log's shipped API is in contracts/core-diag.md, and the session marker and crash files reuse the same time ports.

**Language/Version**: Rust 1.99 (edition 2021) for `voicen-core` and `src-tauri`; TypeScript + Svelte 5 for About; NSIS script for the installer hooks; PowerShell and bash in CI.

**Primary Dependencies**:
- Core: `chrono` (local time with offset; MIT/Apache-2.0). Nothing else new; `std::backtrace` for frames.
- Shell: `tauri-plugin-opener` (open folder/file/URL from Rust), `windows` (already planned by 001: `SetUnhandledExceptionFilter`, `GetModuleHandleExW`, `CredEnumerateW`/`CredDeleteW`).
- Tooling (install needs owner consent): `cargo-about` in the `voicen-rust:1.99` image; `pnpm licenses` (built in); `gh` in CI (preinstalled on GitHub runners).

**Storage**: files under `%LOCALAPPDATA%\Voicen\logs`: `voicen.log` + rolled `voicen-*.log`, `crash-*.txt`, `session.marker`, `installer-ended`. No database.

**Testing**: `cargo test -p voicen-core` (Docker) for all diag logic; vitest + Playwright (mocked IPC) for About; `cargo test --workspace` on `windows-latest` for the native-fault filter, `crash_probe`, credentials purge, path resolution; the Windows job's install/uninstall smoke; the owner's manual checks (quickstart.md).

**Target Platform**: Windows 10 22H2 / 11 x64; dev host Linux.

**Project Type**: desktop app (Tauri 2) plus CI/release pipeline.

**Performance Goals**: a log write is a buffered append under a mutex (< 1 ms typical); retention runs at start and on roll only; start-line written before any window is created.

**Constraints**: never log or crash-dump text, audio, keys (req FR-20, NFR-04); no network for diagnostics (req NFR-05); installed size ≤ 100 MB (req NFR-09); `panic = "abort"` stays (001 research R-15) — the hook runs before abort; MIT-compatible licenses (req NFR-12); never push from a DEV session.

**Scale/Scope**: ≤ 10 MB of logs, ≤ 20 crash files; one release per tag.

No NEEDS CLARIFICATION remains; Tauri/NSIS behaviours that cannot be observed on Linux are marked "confirm on Windows" in research.md and are the first acceptance item of the tasks that depend on them.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-checked after Phase 1 design.*

| Principle | How this plan complies | Status |
|---|---|---|
| I. PRINCIPLES.md | P-004/P-005: each guarantee (allowlist, start line, retention, once-per-session notice, marker classification, crash content, uninstall yes/no/upgrade, version/tag check, license check) has a red test in tasks.md, failure branches included. P-006: the Windows job checks the running installed app's log for the commit and the crash record. P-009: typed `LogEvent` (no free-text field), library logs dropped, crash files without payload or dumps, host-only URLs. P-010: one `AppPaths`, one `BuildInfo`, one version (Cargo), one credential prefix, one accepted-license list. P-011: one log seam (`PipelineObserver` → `Log`), one open-logs function for tray and About, one window helper. P-013: the start line proves the runtime read BuildInfo and paths. | PASS |
| II. Privacy by default | Logs and crash files stay local; nothing uploaded (FR-010). No text/audio/keys (FR-007, FR-014). Uninstall "yes" removes all data and keys; web view data moved inside the data folder (FR-034). | PASS |
| III. Never lose words, never paste garbage | Diagnostics never block delivery: the writer never fails a caller and degrades silently with one notice (FR-009). | PASS |
| IV. Core + thin shell | Formatting, rotation, retention, session classification, crash rendering and retention are core and Linux-tested; the shell only adds the Win32 fault filter, credential enumeration, opener and NSIS hooks. | PASS |
| V. Measured, not assumed | The log carries the timings NFR-01/NFR-02 are measured from; the "deployed" check reads the installed app's log; the table below states where each requirement is verified. | PASS |

Post-design re-check (after data-model.md and contracts/): still PASS. No complexity-tracking entry.

## Where each requirement is verified

| Spec FR (req) | Linux host — core with fakes | Linux host — UI, mocked IPC | Windows CI runner | Owner's manual check |
|---|---|---|---|---|
| FR-001 (FR-18) | BuildInfo version/commit/`unknown` | — | smoke: start line with the built commit | — |
| FR-002, FR-033 (FR-18, NFR-12) | — | About shows `Voicen v (c)`; build-info error; open errors; buttons call commands | notices resource present in install dir | About in the installed app; notices open in Notepad |
| FR-003, FR-004 (FR-20, FR-18) | start line first, format, timestamp with offset, no interleaving | — | smoke grep `(<commit>) started` | — |
| FR-005, FR-006 (FR-20, NFR-01/02) | `LogObserver` maps a record to one line; absent timings omitted | — | — (timing values are 001's) | NFR-01 benchmark from the log |
| FR-007 (FR-20, NFR-04, NFR-06) | redaction test with planted text/key (no host or model name is logged: T-008 Q1, decision #64) | — | smoke log grepped for the planted CI test credential value: 0 | — |
| FR-008 (FR-20) | roll at 2 MB / date; 7 days; 10 MB; crash files untouched | — | — | — |
| FR-009 (FR-20 failure) | degraded mode, once-per-session signal, silent recovery | — | logs dir made read-only (ACL) → app runs, one notice | — |
| FR-010 (NFR-05) | diag module has no network dependency (review + `cargo tree` check) | — | — | — |
| FR-011, FR-014, FR-016 (FR-20, NFR-07, NFR-04) | panic hook in a spawned thread: file, allowlisted keys, no payload; re-entry | — | `crash_probe panic` | — |
| FR-012 (FR-20, NFR-07) | `render_crash_file` for native fault | — | `crash_probe fault` | — |
| FR-013 (FR-20) | four classifications; second instance never calls `begin` (ordering test in shell) | — | kill → relaunch → abnormal-end record | logoff/shutdown → no record |
| FR-015 (FR-20) | 30 days / 20 files; survives log rotation | — | — | — |
| FR-017 (FR-01, FR-20) | — | button → `open_logs_folder`; error alert | command creates the dir and resolves the path | Explorer opens from tray and About |
| FR-018, FR-019, FR-020 (NFR-09, FR-12) | — | — | silent per-user install, size ≤ 100 MB, shortcut, resources | clean Win 11 Sandbox, no UAC (SC-009) |
| FR-021–FR-025 (FR-28, FR-15, FR-19) | — | — | `/KEEPDATA`, `/S`, `/P` reinstall, credential check through `scripts/ci/credentials.ps1`, autostart value removed | interactive question EN/RU |
| FR-026 (decisions #4) | — | — | first green run of `windows` on the repository | owner creates repo / pushes |
| FR-027, FR-028 (§9 deployed) | — | — | the job itself | — |
| FR-029, FR-030 (§9 release) | `check-version.sh` tests | — | release job on a tag | first release notes reviewed |
| FR-031, FR-032 (NFR-12) | `make licenses-check` in the gate; a fixture with a GPL package fails it | — | — | notices reviewed per release |
| FR-034 (FR-28) | `AppPaths::webview` | — | after launch + settings window, `%LOCALAPPDATA%\dev.voicen.app` absent | — |
| NFR-07 (crash records) | via FR-011–FR-016 | — | via FR-011–FR-013 | two weeks without a crash file (success criterion 2) |

## Project Structure

### Documentation (this feature)

```text
specs/006-diagnostics-and-release/
├── spec.md
├── plan.md              # this file
├── research.md          # Phase 0
├── data-model.md        # Phase 1
├── quickstart.md        # Phase 1
├── contracts/
│   ├── core-diag.md     # paths, log, session, crash, credential prefix
│   ├── ipc.md           # About commands, shell entry points, catalog keys
│   └── installer-ci.md  # installer/uninstaller switches, CI jobs, scripts
├── checklists/
│   ├── requirements.md
│   └── requirements-quality.md
└── tasks.md
```

### Source Code (repository root)

```text
crates/voicen-core/src/
├── build_info.rs            # exists (FR-18)
├── paths.rs                 # AppPaths (shared with 001/002/004/005)
└── diag/
    ├── mod.rs
    ├── clock.rs             # WallClock (merged with 001's Clock if it has wall time)
    ├── event.rs             # LogEvent, newtypes (HostName, ModelName, FileName, OsVersion)
    ├── format.rs            # format_line
    ├── log.rs               # Log writer: roll, retention, degraded mode
    ├── observer.rs          # LogObserver: PipelineObserver (001) → LogEvent
    ├── session.rs           # SessionMarker, PreviousSession
    ├── crash.rs             # CrashContext, panic hook, render, retention
    └── (CREDENTIAL_TARGET_PREFIX and purge_credentials live in secrets.rs, T-061)

src-tauri/
├── src/lib.rs               # diag_start / diag_exit wiring; commands
├── src/main.rs              # --purge-credentials before run()
├── src/diag.rs              # commands open_logs_folder / open_third_party_notices / open_project_page
├── src/window.rs            # webview_window helper (data_directory)
├── src/win/crash.rs         # SetUnhandledExceptionFilter, ModuleResolver impl
├── src/win/purge.rs         # CredEnumerateW/CredDeleteW by prefix
├── src/bin/crash_probe.rs   # cfg(feature = "crash-probe")
├── tests/crash_probe.rs     # Windows-only
├── windows/installer-hooks.nsh
└── tauri.conf.json          # resources, nsis hooks/languages, no `version`

src/lib/about/About.svelte (+ about.ts, about.test.ts)
e2e/about.spec.ts
about.toml, about.hbs, licenses/manual.json, THIRD-PARTY-NOTICES.txt
scripts/check-version.sh, scripts/licenses/
.github/workflows/ci.yml, .github/release-notes.md
docker/rust.Dockerfile       # + cargo-about (consent)
Makefile                     # + licenses, licenses-check, version-check in check
```

**Structure Decision**: The existing three-area layout. Diagnostics logic is a `diag` module in the core next to `build_info`; the shell gets thin adapters; release artefacts live at the repo root and in `.github/`. Other features plug in through `PipelineObserver` (001), the window helper and `AppPaths`.

## Dependencies on other features

- **001**: `PipelineObserver` trait and `DictationRecord` values; single-instance check before `diag_start`; tray item "Open logs folder" calls `open_logs_folder`; tray Exit and `WM_ENDSESSION` call `diag_exit`; notifier for `notice.logs_unwritable`; the Silero model file to bundle.
- **002**: `cold`/`warm` and `load_ms` fields in `DictationRecord`; models live under `AppPaths` (removed with the folder).
- **004**: General tab hosts About; message catalog; key-slot naming → `CREDENTIAL_TARGET_PREFIX`; autostart Run value name.
- **005**: history file under `AppPaths` (removed with the folder).

## Owner decisions needed

1. **Installs**: `cargo-about` in `docker/rust.Dockerfile`; crates `chrono`, `tauri-plugin-opener` (research R13, R2, R16).
2. **Accepted-license list** (research R13), in particular MPL-2.0 and the Unicode licenses.
3. **Install directory** shared with the data folder (Tauri default) vs `%LOCALAPPDATA%\Programs\Voicen` (research R9).
4. ~~**Tauri's stock "Delete the application data" checkbox** next to our question — accept for release 1 or move to a custom NSIS template (research R10).~~ Decided (decisions #76): a minimal-diff fork of the tauri-cli 2.12.1 template without the checkbox (T-025).
5. **Creating the public repository and pushing** (FR-026, research R15) — owner session.
6. The five clarifications in spec.md (confirmed by the owner 2026-10-02).

## Complexity Tracking

No constitution violations.
