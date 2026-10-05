# Tasks: Diagnostics and Release

**Input**: Design documents from `/specs/006-diagnostics-and-release/` — plan.md, spec.md, research.md, data-model.md, contracts/core-diag.md, contracts/ipc.md, contracts/installer-ci.md, quickstart.md

**Tests**: REQUIRED. The constitution (P-004, P-005, principle III) needs a red test per guarantee, failure branches included, written before the implementation it guards.

**Conversion**: these tasks become teamwright tasks (`docs/tasks/T-NNN.md`, `design_ref: specs/006-diagnostics-and-release/tasks.md#<ID>`); `/speckit-implement` is not used. Each task names its spec requirements as `spec FR-0NN` and the source requirements as `req FR-NN` / `req NFR-NN` / `req §9`. Area: `core` = `crates/voicen-core` (Linux host), `shell` = `src-tauri` (Windows CI only), `ui` = `src/`, `e2e/`, `ci` = `.github/`, `scripts/`, `Makefile`, `docker/`, installer config. **Owner** marks a step only the owner may do (repository creation, push, tag, installs needing consent).

> Superseded in part (T-008, decision #64): T003's `chrono`, T006/T007's `WallClock`, `HostName` and `ModelName`, T016's `retain_now` and `RetentionDeleted`, T015's "total of `voicen*.log` > 10 MiB" retention, and T019/T020's "one `LogEvent` per callback" over a `DictationRecord` do not apply. The shipped shapes are in contracts/core-diag.md "Clock", "Log" and "Pipeline observer": `clock::Clock` + `clock::LocalOffset`; retention inside `open` and each roll, against `total_bytes − roll_bytes`; one line per recording, joined by `RecordingId`. T-054 takes the notice and the tray item of T021.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: can run in parallel (different files, no dependency on an incomplete task)
- **[Story]**: US1 build info (P1), US2 local log (P1), US3 crash records (P1), US4 installer/uninstaller (P1), US5 deployed + release (P1), US6 licenses (P2)

---

## Phase 1: Setup (prerequisites)

- [ ] T001 [area ci] **Owner**: create the public GitHub repository (`gh repo create <owner>/voicen --public --source=. --remote=origin`), default branch `main`, Actions enabled, default `GITHUB_TOKEN` permissions read-only; push `main` (owner session — never from a DEV session); record the outcome as a new row in `docs/decisions.md` (closes decisions #4) and update `docs/plan.md` stage 0. Evidence: the `gate` and `windows` jobs of `.github/workflows/ci.yml` run for the pushed commit (spec FR-026; req §9, decisions #4).
- [ ] T002 [area ci] Make the existing `windows` job green on the repository and confirm the Windows facts the plan depends on, writing the observed values into `specs/006-diagnostics-and-release/research.md` (R9, R11, R14): install dir of the `currentUser` NSIS install, uninstaller path, Start-menu shortcut exists, installer file name. Evidence: a green run whose log shows `voicen <version> (<commit>) started` (spec FR-027; req §9 "deployed", FR-18). Depends on T001.
- [ ] T003 [area core] Add `chrono` (MIT/Apache-2.0) to `crates/voicen-core/Cargo.toml` and `tempfile` as a dev-dependency (owner consent per constitution); create empty modules `crates/voicen-core/src/paths.rs`, `crates/voicen-core/src/diag/mod.rs` and declare them in `crates/voicen-core/src/lib.rs`; `make check` stays green (plan "Source Code").

---

## Phase 2: Foundational (blocks all stories)

- [ ] T004 [P] [area core] Red tests in `crates/voicen-core/src/paths.rs` for `AppPaths::new(base)`: `root = base/Voicen`, `logs = root/logs`, `webview = root/webview`; paths are built only from the given base (spec FR-004, FR-034; req FR-20, FR-28; P-010). If 001/004 already created `AppPaths`, add only the `logs`/`webview` tests.
- [ ] T005 [area core] Implement `AppPaths` until T004 is green (contracts/core-diag.md "Paths") (spec FR-004, FR-034; req FR-20, FR-28).
- [ ] T006 [P] [area core] Red tests in `crates/voicen-core/src/diag/format.rs` / `event.rs`: `format_line(Started)` contains `voicen 0.1.0 (abc1234) started` and the OS version and pid; the timestamp is local time with UTC offset and milliseconds (fake `WallClock`); a `Dictation` record with an absent timing omits that key (never `=0`); `HostName::from_url("https://user:sk-PLANTED@api.example.com/v1?key=sk-PLANTED")` is `api.example.com`; `ModelName` with spaces/quotes/newlines is quoted and escaped so a line never contains `\n` or `\r`; constructing a `ModelName` longer than 128 chars fails (spec FR-003, FR-004, FR-005, FR-007; req FR-18, FR-20, NFR-04).
- [ ] T007 [area core] Implement `diag/clock.rs` (`WallClock`; merge with 001's `platform::Clock` if it already yields wall time), `diag/event.rs` (`LogEvent` and newtypes per data-model.md — no variant accepts free text) and `diag/format.rs` until T006 is green (spec FR-003..FR-007; req FR-20, NFR-04).
- [ ] T008 [P] [area core] Red tests in `crates/voicen-core/src/diag/log.rs` (temp dir): `Log::open` creates `logs/` if missing; the first line written after open is the `Started` line; 8 threads × 500 writes give exactly 4000 complete lines that each parse; `write` never panics when the file is deleted mid-session (it is re-created) (spec FR-003, FR-004; req FR-20).
- [ ] T009 [area core] Implement the `Log` writer core path (`open`, `write`, mutex, buffered append, re-create on missing file) in `crates/voicen-core/src/diag/log.rs` until T008 is green (spec FR-003, FR-004; req FR-20).

**Checkpoint**: typed log events written as whole lines on the Linux host.

---

## Phase 3: User Story 1 — Know which build is running (P1) 🎯 MVP

**Goal**: About shows `Voicen <version> (<commit>)`; the first log line of every session carries the same.

**Independent test**: core start line test; Playwright About with mocked IPC; Windows smoke greps `(<commit>) started`.

- [ ] T010 [P] [US1] [area core] Red test in `crates/voicen-core/src/build_info.rs`: when the build has no commit the value is `unknown`, never empty (extract the selection logic of `crates/voicen-core/build.rs` into a pure function testable from `build.rs` and the crate, e.g. `fn pick_commit(env: Option<&str>, git: Option<&str>) -> String` in a shared file included by both) (spec FR-001; req FR-18).
- [ ] T011 [US1] [area core] Implement the commit selection function until T010 is green; `build_info()` unchanged for callers (spec FR-001; req FR-18).
- [ ] T012 [P] [US1] [area ui] Red tests: `src/lib/about/about.test.ts` (labels from the catalog keys `about.*` in EN and RU; the build-info line uses `formatBuildInfo`) and `e2e/about.spec.ts` with mocked IPC (`get_build_info`, `open_logs_folder`, `open_third_party_notices`, `open_project_page`): shows `Voicen 0.1.0 (abc1234)`; `get_build_info` rejects → `role=alert` "Cannot read build info"; each button invokes its command with no args; `open_logs_folder` rejecting `{code:"cannot_open"}` → alert "Cannot open the logs folder: …"; `open_third_party_notices` rejecting → "Cannot open the license list: …"; the "About Voicen" button opens a modal dialog (role `dialog`, labelled), Esc and Close close it and return focus to the button; all controls reachable by Tab and operable by Enter with accessible names (spec FR-002, FR-017, FR-033; req FR-18, FR-15, NFR-12).
- [ ] T013 [US1] [area ui] Implement `src/lib/about/About.svelte` and `src/lib/about/about.ts` until T012 is green; add the `about.*`, `error.open_logs_folder`, `error.open_notices` keys (EN + RU) to 004's message catalog; add the "About Voicen" button and dialog to 004's General tab — until 004's settings window exists, host the button and dialog on `src/routes/+page.svelte` (replacing the skeleton's build-info paragraph; update `e2e/build-info.spec.ts` accordingly) (spec FR-002; req FR-18, FR-15; Clarification Q1).
- [ ] T014 [US1] [area shell] Replace `log_start()` in `src-tauri/src/lib.rs` with `diag_start(app)` per contracts/ipc.md: build `AppPaths` from `%LOCALAPPDATA%` (temp dir fallback), `Log::open`, write `Started` first (with the OS version); add `tauri-plugin-opener` (owner consent) and `src-tauri/src/diag.rs` commands `open_logs_folder` (create dir, open), `open_third_party_notices` (bundled resource path), `open_project_page` (repository URL constant from T001), each returning `{code:"cannot_open", reason}` on failure; register them. Windows CI tests in `src-tauri/tests/`: the resolved logs path is under `%LOCALAPPDATA%\Voicen\logs`; `open_logs_folder` creates a missing folder. The existing smoke grep `(<commit>) started` must stay green (spec FR-001, FR-003, FR-004, FR-017, FR-033; req FR-18, FR-20, FR-01).

**Checkpoint**: US1 verifiable — Windows smoke shows the commit; About renders under Playwright.

---

## Phase 4: User Story 2 — Diagnose a problem from the local log (P1)

**Goal**: one line per dictation with engine and timings; errors with categories; never text, audio or keys; ≤ 7 days and ≤ 10 MB; works on without a writable folder.

**Independent test**: core tests with a temp dir and fake clock; a planted-secret redaction test; Windows read-only folder test.

- [ ] T015 [P] [US2] [area core] Red tests in `crates/voicen-core/src/diag/log.rs` (fake clock, small `LogConfig` for speed plus one test with the real 2 MiB/10 MiB values): the active file rolls at `roll_bytes` and at local date change into `voicen-YYYYMMDD-HHMMSS.log`; retention at open and after each roll deletes rolled files older than 7 days, then the oldest while the total of `voicen*.log` exceeds 10 MiB; the active file is never deleted; `crash-*.txt`, `session.marker` and `installer-ended` planted in the folder survive any number of rolls, including a 20-day-old crash file; a clock moved back 1 year or forward 1 year never deletes the active file and deletes at most what the rules name (spec FR-008, FR-015; req FR-20).
- [ ] T016 [US2] [area core] Implement rolling and `retain_now` in `crates/voicen-core/src/diag/log.rs` until T015 is green; emit `RetentionDeleted` with counts (spec FR-008; req FR-20).
- [ ] T017 [P] [US2] [area core] Red tests for the degraded mode in `crates/voicen-core/src/diag/log.rs`: logs path is a file → `open` succeeds, `on_unwritable(NotADirectory)` called once, writes dropped without panic; read-only dir (skip when running as root, use a path-is-file case instead) → `PermissionDenied` once; 100 further failing writes → still exactly one callback; after the dir becomes writable and `reopen_every` elapses on the fake clock, the next write succeeds and `LogsRecovered` is written, with no second callback (spec FR-009; req FR-20 failure branch; Clarification Q3).
- [ ] T018 [US2] [area core] Implement the `Healthy`/`Degraded` state machine with reason mapping from `io::ErrorKind`/raw OS error until T017 is green (spec FR-009; req FR-20).
- [ ] T019 [P] [US2] [area core] Red tests in `crates/voicen-core/src/diag/observer.rs`: each `PipelineObserver` callback from 001 maps to exactly one `LogEvent` line (delivered, failed with category + HTTP status + host, discarded with reason; 002's `cold`/`warm` + `load_ms`; 003's post-processing outcome); a redaction test feeds the sentinel transcript `"secret words 123"` and key `"sk-test-PLANTED"` into every input of the pipeline types reachable by the observer (record fields, error values, configured URL with the key in user-info and query) and asserts 0 occurrences in all files of the temp logs folder; `cargo tree -p voicen-core -e normal` shows no `log`/`tracing` subscriber crate is initialised by `diag` (spec FR-005, FR-006, FR-007, FR-010; req FR-20, NFR-01, NFR-02, NFR-04, NFR-05, NFR-06; P-009).
- [ ] T020 [US2] [area core] Implement `LogObserver` (001's `PipelineObserver` → `Log`) in `crates/voicen-core/src/diag/observer.rs` until T019 is green. Depends on 001's `events.rs` (`PipelineObserver`, `DictationRecord` values) (spec FR-005, FR-006, FR-007; req FR-20, NFR-04; P-011).
- [ ] T021 [US2] [area shell] Wire in `src-tauri/src/lib.rs`: `LogObserver` registered with 001's pipeline; `on_unwritable` → 001's notifier with `notice.logs_unwritable` (EN + RU in 004's catalog); 001's tray item "Open logs folder" calls the same function as the `open_logs_folder` command. Windows CI test: with `%LOCALAPPDATA%\Voicen\logs` denied by ACL, the app starts, the notifier fake/real receives exactly one notice, and a dictation through the injected WAV source is still delivered (spec FR-009, FR-017; req FR-20 failure branch, FR-01).

**Checkpoint**: US2 complete — log lines, redaction, retention and degraded mode proven on the Linux host.

---

## Phase 5: User Story 3 — Find out that the app crashed (P1)

**Goal**: every panic, native fault or abnormal end leaves exactly one crash record; clean exits and OS shutdowns leave none; crash files keep 30 days / 20 files.

**Independent test**: core session/crash tests on the Linux host; `crash_probe` on the Windows runner; kill → relaunch in the Windows job (T030).

- [ ] T022 [P] [US3] [area core] Red tests in `crates/voicen-core/src/diag/session.rs` (temp dir, fake clock): no marker → `Clean`, marker written with pid/started/version/commit; marker + crash file with matching `session_started` → `Crashed{file}`, no new crash file; marker + `installer-ended` newer than `started` → `EndedByInstaller`, no crash file; marker + `installer-ended` older than `started` → `EndedAbnormally` with a new `abnormal_end` crash file; marker only → `EndedAbnormally` with one crash file; in all cases the old marker and note are gone and the new marker exists; `end_clean` deletes the marker and writes `Exited`; a corrupt marker file is treated as `EndedAbnormally` without panicking (spec FR-013; req FR-20, r2#4, NFR-07).
- [ ] T023 [US3] [area core] Implement `Session::begin` / `end_clean` and the `PreviousSession` log line in `crates/voicen-core/src/diag/session.rs` until T022 is green (spec FR-013; req FR-20).
- [ ] T024 [P] [US3] [area core] Red tests in `crates/voicen-core/src/diag/crash.rs`: `render_crash_file` emits only the allowlisted keys of data-model.md in order, per kind; with the hook installed, a spawned thread panicking with payload `"secret words 123 sk-test-PLANTED"` produces one `crash-*.txt` containing `kind: panic`, `location:` with this test file's path, `backtrace:` frames as `<module>+0x<hex>` (fake `ModuleResolver`), and 0 occurrences of the payload; a panic inside the hook's own file write (unwritable dir) does not recurse or panic; `retain_crash_files` with 25 files of mixed ages deletes those older than 30 days then keeps the 20 newest (spec FR-011, FR-014, FR-015, FR-016; req FR-20, NFR-04, NFR-07, r2#1; Clarification Q2). Tests that install the global hook run serialised.
- [ ] T025 [US3] [area core] Implement `CrashContext`, `install_panic_hook`, `render_crash_file`, `retain_crash_files` in `crates/voicen-core/src/diag/crash.rs` until T024 is green; the hook never formats the payload (spec FR-011, FR-014, FR-015, FR-016; req FR-20, NFR-07).
- [ ] T026 [US3] [area shell] In `src-tauri/src/win/crash.rs`: `ModuleResolver` via `GetModuleHandleExW`; `SetUnhandledExceptionFilter` writing a `native_fault` crash file from a pre-built buffer and wide path (no allocation in the filter), returning `EXCEPTION_CONTINUE_SEARCH`; `diag_start` installs the panic hook and the filter, runs `retain_crash_files` and `Session::begin` — only after 001's single-instance check; `diag_exit` (`Session::end_clean`) called from 001's tray Exit and `WM_ENDSESSION` (wParam TRUE) handlers. Add `src-tauri/src/bin/crash_probe.rs` behind cargo feature `crash-probe` and `src-tauri/tests/crash_probe.rs`: `crash_probe panic` → one `kind: panic` file without the payload; `crash_probe fault` → one `kind: native_fault` file with `exception_code: 0xC0000005`; a second instance launched while the first runs leaves the first's marker untouched. Release profile: `debug = "line-tables-only"`, `split-debuginfo = "packed"` so a `voicen.pdb` is produced and not bundled — confirm on the runner (research R5) (spec FR-011, FR-012, FR-013, FR-014, FR-016; req FR-20, NFR-07, FR-01).

**Checkpoint**: US3 complete on the Linux host and the Windows runner; NFR-07's evidence source exists.

---

## Phase 6: User Story 4 — Install and uninstall without admin rights (P1)

**Goal**: per-user installer ≤ 100 MB with shortcut and resources; uninstaller asks (default yes), honours `/S` and `/KEEPDATA`, skips on upgrade, removes keys and autostart; no app data outside `%LOCALAPPDATA%\Voicen`.

**Independent test**: Windows job install/uninstall steps (T030); owner's interactive uninstall check.

- [ ] T027 [P] [US4] [area shell] Add `src-tauri/src/window.rs` `webview_window(app, label, route)` that sets `data_directory(AppPaths::webview())`; use it for every window created in `src-tauri` (and require it in 001/004/005 window code — note in `docs/architecture.md` Seams). Windows CI test: after launching the app and opening a window, `%LOCALAPPDATA%\dev.voicen.app` does not exist and `%LOCALAPPDATA%\Voicen\webview` does — confirm on the runner (research R8) (spec FR-034; req FR-28).
- [ ] T028 [P] [US4] [area shell] Add `CREDENTIAL_TARGET_PREFIX` in `crates/voicen-core/src/secrets.rs` (`voicen_core::secrets`, not `diag`; value agreed with 004's key slots; 004's credential store must use it) and `--purge-credentials` at the top of `src-tauri/src/main.rs` (before `run()`, no Tauri, no single instance, no log) policy `voicen_core::secrets::purge_credentials`, Win32 adapter in `src-tauri/src/win/purge.rs` (`from_args`) with `CredEnumerateW(prefix*)` + `CredDeleteW`; exit 0 / 2. Windows CI test: two generic credentials with the prefix and one without → after the call the two are gone, the other stays, exit 0; nothing printed contains a credential blob (spec FR-021, FR-022; req FR-28, NFR-04; P-010).
- [ ] T029 [US4] [area ci] Installer configuration: in `src-tauri/tauri.conf.json` set `bundle.resources` (`THIRD-PARTY-NOTICES.txt`, the Silero VAD model path from 001), `bundle.windows.nsis.languages = ["English","Russian"]`, `displayLanguageSelector: false`, `installerHooks: "windows/installer-hooks.nsh"`; write `src-tauri/windows/installer-hooks.nsh` per contracts/installer-ci.md: `NSIS_HOOK_PREINSTALL` writes `logs\installer-ended`; `NSIS_HOOK_PREUNINSTALL` writes the note, skips on update mode, `/KEEPDATA` → no, else `MessageBox MB_YESNO … /SD IDYES` with `LangString` EN/RU, on yes runs `voicen.exe --purge-credentials` and shows the manual-removal message on exit 2; `NSIS_HOOK_POSTUNINSTALL` on yes `RMDir /r "$LOCALAPPDATA\Voicen"` and reports leftovers; always deletes 004's autostart Run value (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Voicen`, the literal equal to `src-tauri` `autostart::RUN_VALUE_NAME`; T-014). Confirm on the runner what the stock uninstaller page shows next to the question and record it in research.md R10 (spec FR-018, FR-020, FR-021, FR-022, FR-023, FR-024, FR-025; req FR-28, FR-15, FR-19, FR-12, NFR-09). Depends on T028.
- [ ] T030 [US4] [area ci] Extend the `windows` job in `.github/workflows/ci.yml` per contracts/installer-ci.md and research R14: installed size ≤ 100 MB before first launch (prints the size); the Start-menu shortcut exists and the install dir contains `THIRD-PARTY-NOTICES.txt` and the Silero VAD model but no `ggml-*.bin` whisper model; after the existing start-line check and `Stop-Process -Force`, relaunch and require `previous session ended abnormally` plus one `crash-*.txt` with `kind: abnormal_end`; grep the log folder for the CI test credential's value (0 hits); reinstall silently → log file still present; create `cmdkey /generic:<prefix>ci-test`; `uninstall.exe /S /KEEPDATA` → data folder and credential present; reinstall; `uninstall.exe /S` → `%LOCALAPPDATA%\Voicen` absent and `cmdkey /list` shows no `<prefix>` entry; autostart Run value absent; upload `voicen-symbols-<commit>` (zipped PDB) beside the installer artifact (spec FR-013, FR-018, FR-019, FR-020, FR-021, FR-022, FR-023, FR-024, FR-027, FR-028; req NFR-09, FR-28, FR-20, FR-12, NFR-12, §9). Depends on T002, T026, T028, T029.

**Checkpoint**: US4 complete — every push proves install, crash bookkeeping and both uninstall answers on Windows.

---

## Phase 7: User Story 5 — Every commit deployed, every tag released (P1)

**Goal**: one version source; a `v*` tag publishes the smoke-tested installer with checksum and notes; mismatches and re-runs publish nothing.

**Independent test**: `check-version` tests in the gate; a tag pushed by the owner.

- [ ] T031 [P] [US5] [area ci] Red tests `scripts/check-version.test.sh` (run by `make check`): equal versions → exit 0; `package.json` differs → non-zero naming both values; expected `0.2.0` vs `0.1.0` → non-zero naming both; `tauri.conf.json` without `version` is accepted (spec FR-029; req §9; Clarification Q5; P-010).
- [ ] T032 [US5] [area ci] Implement `scripts/check-version.sh` until T031 is green; remove `version` from `src-tauri/tauri.conf.json` (bundler takes the Cargo version); add `version-check` to `make check`; confirm on the runner that the installer file name carries the Cargo version (research R12) (spec FR-001, FR-029; req FR-18, §9).
- [ ] T033 [US5] [area ci] Add the `release` job to `.github/workflows/ci.yml` (tags `v*` only, `needs: [gate, windows]`, `permissions: contents: write`): download the commit's installer and symbols artifacts (never rebuild); `scripts/check-version.sh "${GITHUB_REF_NAME#v}"`; fail with "release already exists" if `gh release view` succeeds; write `SHA256SUMS.txt`; render `.github/release-notes.md` (version, commit, checksum, SmartScreen note, notices link); `gh release create`. **Owner** pushes the first tag; evidence: the release page lists the installer, `SHA256SUMS.txt` and the symbols zip, and the installer's checksum matches the Windows job's artifact (spec FR-029, FR-030; req §9, §2 success 3, OQ-02). Depends on T030, T032.

**Checkpoint**: US5 complete after the owner's first tag.

---

## Phase 8: User Story 6 — Ship only what we are allowed to ship (P2)

**Goal**: unapproved or unknown licenses fail CI; an up-to-date notices file ships and opens from About.

**Independent test**: license check in the gate with a failing fixture; notices present in the install dir.

- [ ] T034 [US6] [area ci] **Owner consent**: add `cargo install cargo-about --locked` to `docker/rust.Dockerfile` (rebuild with `make core-image`); add `about.toml` (accepted list = decisions #9 + #24, the one list in `about.toml`; `targets = ["x86_64-pc-windows-msvc"]`), `about.hbs` (plain-text notices) and `licenses/manual.json` (whisper.cpp MIT, Silero VAD MIT, whisper ggml models MIT — downloaded, NSIS zlib — installer only; NSIS compresses with `zlib`, decision #29); installed as `cargo install cargo-about --locked --version 0.9.2 --features cli` (decision #24; done in T-027) (spec FR-031, FR-032; req NFR-12).
- [ ] T035 [P] [US6] [area ci] Red tests `scripts/licenses/*.test.mjs` (`node --test`, run by `make check`) over module lists of the client bundle (not `pnpm licenses`; see `docs/decisions/licenses.md`): all-MIT passes; a `GPL-3.0` package fails naming it; an `UNKNOWN`/missing license fails naming it; the accepted list is read from `about.toml` (spec FR-031; req NFR-12).
- [ ] T036 [US6] [area ci] Implement `scripts/licenses/` (`check.mjs`, `notices.mjs`) until T035 is green; add `make licenses` (cargo-about for Rust + bundled npm packages + `licenses/manual.json` → `THIRD-PARTY-NOTICES.txt`) and `make licenses-check` (fails on unaccepted licenses or a diff with the committed file) to `make check`; generate and commit `THIRD-PARTY-NOTICES.txt`. Evidence: adding a GPL test crate in a throwaway branch makes `make licenses-check` fail (spec FR-031, FR-032, FR-033; req NFR-12). Depends on T034; T029 bundles the file.

**Checkpoint**: US6 complete.

---

## Phase 9: Polish & cross-cutting

- [ ] T037 [area ci] Docs in the same commits as the code they describe, plus a final sweep: `docs/architecture.md` (diag components, `AppPaths`, window helper seam, release environment), `CLAUDE.md` map (logs folder, `--purge-credentials`, `make licenses`), `docs/decisions.md` rows for the owner decisions of plan.md, `docs/decisions/core.md` invariants (typed log events, crash files without payload) (P-014).
- [ ] T038 [area ci] **Owner** manual checks from quickstart.md "Owner's manual checks" 1–4 on the first release (clean install in Windows Sandbox with a first dictation, About, crash bookkeeping on shutdown/kill, interactive uninstall in EN and RU); recorded as the verify evidence for the tasks carrying a `verify_exception` (T026 shutdown path, T029 interactive question) (spec SC-009, FR-013, FR-021, FR-025; req §2 success 3, NFR-09, FR-28). Step 5 (two-week run, success criterion 2, req NFR-07) follows release.

---

## Dependencies & execution order

- **Phase 1**: T001 (owner) → T002. T003 is independent of T001/T002.
- **Phase 2** (after T003): T004 → T005; T006 → T007; T008 → T009 (needs T007). Blocks all stories.
- **US1**: T010 → T011; T012 → T013; T014 needs T009, T005 (and T001 for the project URL constant, may be filled later).
- **US2**: T015 → T016; T017 → T018; T019 → T020 (needs 001's `PipelineObserver`); T021 needs T014, T018, T020 and 001's tray/notifier.
- **US3**: T022 → T023; T024 → T025; T026 needs T014, T023, T025 and 001's single-instance and tray Exit.
- **US4**: T027 and T028 in parallel after T005; T029 needs T028 (and T036 for the notices file — until then a placeholder file); T030 needs T002, T026, T028, T029.
- **US5**: T031 → T032 → T033 (needs T030).
- **US6**: T034 → T036 (T035 parallel with T034).
- **Polish**: T037 alongside; T038 after T033.
- **Cross-feature**: 001 (observer, tray, single instance, notifier, Silero file), 002 (record fields), 004 (General tab, catalog, key-slot prefix, autostart value name), 005 (data under `AppPaths`).

## Parallel examples

- After T003: T004, T006, T008 (tests in different files) in parallel.
- US1: T010 and T012 in parallel; US2: T015, T017, T019 in parallel; US3: T022 and T024 in parallel; US4: T027 and T028 in parallel; US6: T034 and T035 in parallel.

## Implementation strategy

1. **MVP** = Phase 1 + Phase 2 + US1: a repository with a green Windows job proving the installed commit, and About — this unblocks P-006 verification for every other feature.
2. Then US2 and US3 (needed by 001's timings and by success criterion 2), US4 and US5 (success criterion 3), US6 before the first release tag.
3. The first `v*` tag is pushed only after US4, US5 and US6 are done and T038's clean-install check passes on a release candidate artifact from the Windows job.
