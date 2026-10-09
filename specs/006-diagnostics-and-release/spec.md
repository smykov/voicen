# Feature Specification: Diagnostics and Release

**Feature Branch**: `006-diagnostics-and-release` (spec directory; work happens on `main` per project rules)

**Created**: 2026-10-02

**Status**: Approved (clarifications confirmed by the owner 2026-10-02)

**Input**: User description: "Diagnostics and release: version and commit in About and the start log line; a rotating local log in %LOCALAPPDATA%\Voicen\logs (7 days, 10 MB) with timings and errors but never transcript text, audio or keys; crash files for panics and native faults plus a session marker, kept 30 days / 20 files; 'Open logs folder' in the tray; the per-user NSIS installer (≤ 100 MB without models) with an uninstaller that asks whether to remove settings, history, models, logs and saved keys; a v* tag publishes the installer to GitHub Releases; license compliance of all bundled components."

**Source requirements** (`docs/requirements.md` v3): FR-18 (Must), FR-20 (Must), FR-28 — uninstaller part only (Should; model deletion belongs to 002), NFR-07 — crash records (Must; "a failure in any engine never crashes the app" is verified by 001/002/003), NFR-09 (Must), NFR-12 (Must); the "deployed" pipeline of requirements §9 (Windows CI build, silent install, launch, start-line check) and the `v*` release publishing; the GitHub repository creation (decisions #4) as a prerequisite. Constraints honoured: NFR-04 (no keys in logs or crash files), NFR-05 (no telemetry: logs and crash files never leave the machine), NFR-06 (no audio in logs), FR-15 (EN/RU text), FR-01 (tray gives access to "Open logs folder"), §2 success criteria 2 and 3. Owned elsewhere: the tray menu itself, notifications, single-instance handling and the per-dictation timings (FR-01, FR-25, FR-033 of 001 → feature 001); the settings window and its tabs, key slots in Credential Manager, the autostart entry and the message catalog (FR-13, NFR-04, FR-19, FR-15 → feature 004); the bundled Silero VAD model file (FR-12 → feature 001, packaged by this feature's installer).

## Clarifications

### Session 2026-10-02

- Q: Where does the user find the About information (version and commit), given the tray menu of FR-01 has no About item? → A: An "About Voicen" button on the settings window's General tab (feature 004's window) opens the About dialog, showing `Voicen <version> (<commit>)`, the license, the project link, the third-party licenses and an "Open logs folder" button (confirmed by the owner 2026-10-02)
- Q: What may a crash file contain about the failure itself, given a panic message can carry arbitrary runtime text? → A: Only allowlisted facts — kind (panic / native fault / abnormal end), time, version, commit, OS version, thread name, source location (file:line) for a panic, exception code and module offset for a native fault, and a backtrace as module + offset per frame; never the panic message text, memory contents or a memory dump (confirmed by the owner 2026-10-02)
- Q: How often is the "log folder not writable" notification shown? → A: At most once per app session (each start re-checks; a session that later recovers writes again without a new notification) (confirmed by the owner 2026-10-02)
- Q: What does a silent uninstall (no user present to answer the question) do with user data? → A: It applies the question's default ("yes": remove the data folder and saved keys); a `/KEEPDATA` command-line switch answers "no" silently (confirmed by the owner 2026-10-02)
- Q: What happens when a `v*` tag does not match the app version recorded in the build? → A: The release job fails before publishing anything and names both versions; nothing is uploaded to GitHub Releases (confirmed by the owner 2026-10-02). *Superseded by decisions #102 and #103 (T-026): a tag build takes its version from the tag (#98), so the check is: the tag's MAJOR.MINOR equals the repository's (#102) and the tag patch is >= the tag run's `github.run_number` (#103).*

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Know which build is running (Priority: P1)

The owner (or a user filing a bug) needs to know exactly which build runs. They open About and see `Voicen 0.1.0 (abc1234)`; the first line the app writes to its log in every session carries the same version and commit. The Windows CI job uses that log line to prove the commit it just built is the one that was installed and launched.

**Why this priority**: Every other diagnosis and every verification of a deployed build (P-006) starts from "which commit is this?". It is the backbone of the "deployed" check.

**Independent Test**: In core, the build information reports the package version and an embedded commit and formats as `<version> (<commit>)`; in the UI with mocked IPC, About shows the reported values; on the Windows runner, the installed app's log contains `(<commit>) started` for the commit under test.

**Acceptance Scenarios**:

1. **Given** a build from commit abc1234 with version 0.1.0, **When** the user opens About, **Then** it shows `Voicen 0.1.0 (abc1234)` (req FR-18).
2. **Given** the same build, **When** the app starts, **Then** the first log line of that session contains `0.1.0 (abc1234) started` (req FR-18, FR-20).
3. **Given** a build made outside a git checkout and without a commit supplied by CI, **When** the app starts, **Then** About and the start line show `unknown` as the commit, never an empty value.
4. **Given** About cannot obtain the build information (IPC failure), **When** it is shown, **Then** it displays "Cannot read build info" instead of a blank or a stale value.

---

### User Story 2 - Diagnose a problem from the local log (Priority: P1)

Something went wrong — a dictation was slow, failed, or the hotkey stopped working. The user clicks "Open logs folder" in the tray; Explorer opens `%LOCALAPPDATA%\Voicen\logs`. The log shows, for each dictation, the engine and the timings (hotkey press → first audio frame, stop → text, text → paste) and any error with its reason — and never the words that were dictated, the audio or an API key. The log never grows past 10 MB and nothing older than 7 days is kept.

**Why this priority**: It is how the owner measures NFR-01/NFR-02 and how any failure is diagnosed; diagnosis is a Must scope row.

**Independent Test**: In core with a temporary directory and a fake clock: write dictation records and errors, check the line format, check that a planted transcript text and key never appear in any file, advance the clock and the volume past the limits and check retention; in the UI/IPC layer, check the "Open logs folder" command opens the folder path; on Windows, the tray action opens Explorer.

**Acceptance Scenarios**:

1. **Given** one successful dictation via the API engine, **When** the user opens the log, **Then** there is one line with the engine and the three timings (press → first frame, stop → text, text → paste) in milliseconds, and the dictated text does not appear anywhere in the log folder (req FR-20 acceptance).
2. **Given** a dictation that fails (e.g. invalid API key), **When** the log is read, **Then** there is a line with the engine, the failure category and the HTTP status, and neither the key nor any request body appears (req FR-20, NFR-04).
3. **Given** log files older than 7 days, **When** the app starts or the log rotates, **Then** those files are deleted (req FR-20).
4. **Given** the log files together reach 10 MB, **When** more lines are written, **Then** the oldest log files are deleted first so the total stays ≤ 10 MB, and the newest lines are kept (req FR-20).
5. **Given** the user clicks "Open logs folder" in the tray (or the About button), **When** the folder exists, **Then** Explorer opens it; **When** it does not exist yet, **Then** it is created first and opened (req FR-01, FR-20).
6. **Given** the log directory is not writable (permissions, disk full, path is a file), **When** the app starts or a write fails, **Then** the app keeps working (hotkey, dictation and delivery unaffected) and shows one notification "logs cannot be written: <reason>" for that session (req FR-20 failure branch; Clarification Q3).
7. **Given** "Open logs folder" is clicked and the folder can neither be created nor opened, **Then** the user is notified "cannot open the logs folder: <reason>" and nothing else changes.

---

### User Story 3 - Find out that the app crashed (Priority: P1)

The owner uses Voicen daily for two weeks (success criterion 2). If the app ever crashes — a Rust panic, a native fault in a library, or the process is killed or dies without a clean exit — a crash file appears in the logs folder, and the next start writes a log line saying the previous session ended abnormally. If no crash file appeared in two weeks, the criterion is met.

**Why this priority**: NFR-07 and success criterion 2 are measured by the presence of crash files; without them "no crash" is unprovable.

**Independent Test**: In core with a temporary directory: start a session (marker written), "crash" by not ending it, start again → a crash record "previous session ended abnormally" is written and logged; end a session cleanly → no record; plant crash files of various ages/counts → retention keeps ≤ 20 and none older than 30 days, and log rotation never deletes them. On the Windows runner: a test build forced to panic writes a crash file and the next start logs "previous session crashed"; killing the process makes the next start write a crash record.

**Acceptance Scenarios**:

1. **Given** a test build with a forced panic, **When** the panic happens, **Then** a crash file is written to the logs folder before the process ends, and the next start logs "previous session crashed" naming that crash file (req FR-20 acceptance).
2. **Given** the process is killed (Task Manager, `Stop-Process -Force`, power loss), **When** the app starts next time, **Then** it writes a crash record "previous session ended abnormally" and logs it (req FR-20 acceptance, r2#4).
3. **Given** a native fault (access violation in a native library), **When** it happens, **Then** a crash file with the exception code is written before the process ends (req FR-20).
4. **Given** the user exits from the tray (or Windows ends the session at logoff/shutdown and the app exits), **When** the app starts next time, **Then** no crash record is written (req FR-20, r2#4).
5. **Given** a crash file 20 days old, **When** logs rotate, **Then** the crash file is still present (req FR-20 acceptance, r2#1).
6. **Given** crash files older than 30 days, or more than 20 crash files, **When** the app starts, **Then** files older than 30 days are deleted and of the rest only the 20 newest are kept (req FR-20, r2#1).
7. **Given** any crash file, **When** it is inspected, **Then** it contains no transcript text, audio, API key, panic message text or memory dump (req FR-20, NFR-04; Clarification Q2).

---

### User Story 4 - Install and uninstall without admin rights (Priority: P1)

A user downloads the installer from GitHub Releases and runs it. It installs for the current user without asking for administrator rights, creates the Start-menu shortcut that notifications need, and the app starts. Later the user uninstalls it from Windows Settings; the uninstaller asks "Remove settings, history, models, logs and saved keys?" (default yes). With yes, `%LOCALAPPDATA%\Voicen` and Voicen's saved keys in Windows Credential Manager are gone; with no, they stay for a later reinstall.

**Why this priority**: Success criterion 3 (public release installs and works on a clean Windows 11) and NFR-09; the uninstaller is how a privacy-minded user removes every trace (FR-28).

**Independent Test**: On the Windows runner: silent install of the commit's installer without elevation, check the install location is per-user, the installed size excluding models is ≤ 100 MB, the app launches and logs the commit; silent uninstall (default yes) removes the data folder and the test credentials; silent uninstall with `/KEEPDATA` keeps them. On the owner's PC: install on a clean Windows 11 in Windows Sandbox and complete one dictation.

**Acceptance Scenarios**:

1. **Given** a standard (non-admin) Windows 11 user, **When** the installer runs, **Then** it completes without a UAC prompt and installs for the current user only (req NFR-09).
2. **Given** a completed install, **When** the installed files (excluding downloaded models) are measured, **Then** they total ≤ 100 MB (req NFR-09).
3. **Given** a completed install, **Then** a Start-menu shortcut exists that carries the app's identity, so toast notifications can be shown (requirements §8, feature 001 dependency).
4. **Given** the app is installed with settings, history, a downloaded model, logs and saved keys, **When** the user uninstalls and answers "yes" (the default), **Then** `%LOCALAPPDATA%\Voicen` is removed and every Voicen entry in Windows Credential Manager is removed (req FR-28 acceptance).
5. **Given** the same state, **When** the user uninstalls and answers "no", **Then** the program is removed but `%LOCALAPPDATA%\Voicen` data (settings, history, models, logs) and the saved keys stay (req FR-28 acceptance).
6. **Given** the app is running, **When** the uninstaller (or an installer upgrading it) starts, **Then** the running app is closed first, and the next start does not count this as a crash (no crash record for a session ended by the installer).
7. **Given** a newer installer is run over an existing installation (upgrade), **Then** no question about user data is asked and all user data and keys are kept.
8. **Given** a silent uninstall without switches, **Then** it behaves as answering "yes"; with `/KEEPDATA`, as "no" (Clarification Q4).

---

### User Story 5 - Every commit is "deployed" on Windows, every tag is released (Priority: P1)

The owner pushes to `main`. GitHub Actions runs the gate on Linux, then on `windows-latest` builds the NSIS installer for that commit, installs it silently per-user, launches the app and checks that the start log line carries the commit — that is what "deployed" means for every task. When the owner pushes a tag `v0.2.0`, the same pipeline runs and then publishes that installer to a GitHub Release named for the tag.

**Why this priority**: Without it no task can reach `VERIFIED` (P-006), and success criterion 3 requires a published installer.

**Independent Test**: Push a commit to the repository → the Windows job is green and its log shows `(<commit>) started`; push a test tag on a fork or the repository → a release with the installer and its checksum appears; a tag whose version does not match the app version fails without publishing.

**Acceptance Scenarios**:

1. **Given** the public GitHub repository exists with Actions enabled (decisions #4), **When** a commit is pushed to `main`, **Then** the gate job and the Windows job run, and the Windows job builds, silently installs, launches the app and finds `(<commit>) started` in the start log line (requirements §9 "deployed", FR-18).
2. **Given** the installed app does not write the expected line within 30 s, **When** the Windows job runs, **Then** the job fails and prints the log it found (or says none exists).
3. **Given** a tag `vX.Y.Z` matching the app version, **When** it is pushed, **Then** after the gate and the Windows job pass, a GitHub Release `vX.Y.Z` is created with the installer and its SHA-256 checksum attached (requirements §9, §2 success 3).
4. **Given** a tag whose version differs from the app version, **When** it is pushed, **Then** the release job fails naming both versions and publishes nothing (Clarification Q5).
5. **Given** the gate or the Windows job fails for a tagged commit, **Then** nothing is published.
6. **Given** a release for the tag already exists, **When** the job runs again, **Then** it does not overwrite the published installer and fails with "release already exists".

---

### User Story 6 - Ship only what we are allowed to ship (Priority: P2)

Voicen is MIT-licensed open source. Every component that ends up in the installer — Rust crates, npm packages bundled into the UI, whisper.cpp, the Silero VAD model — has a license compatible with distributing it under MIT, and the list of components with their licenses ships with the app and is reachable from About. CI fails when a dependency with an unapproved or unknown license is added.

**Why this priority**: NFR-12 is a Must, but it guards a release rather than a user flow, so it follows the flows above.

**Independent Test**: Run the license check locally/in CI on the current tree → passes; add a dependency with a disallowed license (or an unknown one) in a throwaway branch → the check fails naming it; build → the third-party notices file is present in the installed app and the About dialog links to it.

**Acceptance Scenarios**:

1. **Given** the current dependency tree, **When** CI runs the license check, **Then** it passes and every bundled component has an approved license (req NFR-12).
2. **Given** a new dependency whose license is not on the approved list or cannot be determined, **When** CI runs, **Then** the check fails and names the component and its license (req NFR-12).
3. **Given** an installed app, **When** the user opens About → "Third-party licenses", **Then** a list of every bundled component with its license text or identifier is shown, including whisper.cpp (MIT) and the Silero VAD model (MIT), and the downloadable whisper models' license (req NFR-12).

---

### Edge Cases

- **Second instance launched** (FR-01): it hands over to the running instance and exits without writing a session marker, rotating logs or deleting crash files; only the running instance owns the log and marker.
- **Panic while handling a panic, or a crash while writing the crash file**: the crash path never panics itself; if the crash file cannot be written, the session marker still produces a crash record at the next start.
- **Stack overflow or a fault the crash handler cannot run for**: no crash file at fault time; the next start writes the "previous session ended abnormally" record from the marker, so the crash is still counted.
- **Panic followed by a next start**: one crash, one record — the next start logs "previous session crashed" referencing the existing crash file and does not write a second crash file for the same session.
- **Windows logoff/shutdown/restart while the app runs**: the app exits cleanly and removes the marker, so routine shutdowns are not counted as crashes (success criterion 2 stays meaningful).
- **Sleep, hibernate, lock** (FR-26): the session continues; no marker change.
- **Installer or uninstaller closes the running app**: the next start logs "previous session was ended by the installer" and writes no crash record.
- **Log directory missing**: created at start; if it was deleted while running, it is re-created on the next write.
- **Log directory not writable**: the app works without logging; one notification per session (Clarification Q3); crash files cannot be written either, which the same notification covers.
- **Disk full mid-write**: the line is dropped, never a partial file that breaks the next read; the same once-per-session notification.
- **A single burst larger than 10 MB within a day**: the active file is rolled when it reaches its size cap, and the oldest files are deleted, so the total never exceeds 10 MB.
- **System clock moved backwards or forwards**: retention uses file times; a clock jump can at worst delete or keep files one rotation early/late, never all files at once; the current session's file is never deleted.
- **Error texts from libraries** (HTTP client, OS errors) that might echo a URL, header or body: only an error category, HTTP status, host and OS error code reach the log, never the raw error text of a request.
- **Base URL containing a key or credentials** (`https://user:key@host/...` or `?key=...`): only the host is logged.
- **Commit unknown** (local build outside git): `unknown` is shown and logged (User Story 1, scenario 3).
- **Uninstall while Credential Manager is unavailable**: the uninstaller still removes the folder (if "yes"), then tells the user that saved keys could not be removed and names the entries' prefix so they can be removed by hand.
- **Uninstall "yes" while a file in the data folder is locked**: everything else is removed; the uninstaller reports the folder could not be fully removed.
- **User picked a custom install folder**: uninstall still removes only Voicen's program files plus, on "yes", `%LOCALAPPDATA%\Voicen`; it never deletes anything outside these two locations.
- **Web view data** (cache, local storage of the settings/history/overlay windows): kept inside `%LOCALAPPDATA%\Voicen`, never in a separate per-app folder, so "yes" on uninstall removes it too (FR-034).
- **Start with Windows was on** (FR-19): the uninstaller always removes the autostart entry (it would point at a removed program), regardless of the answer.
- **WebView2 missing on the target machine**: the installer obtains it the standard way for Tauri apps; on Windows 11 it is preinstalled.
- **SmartScreen warning** for the unsigned installer (OQ-02): accepted in release 1; the release notes say how to proceed.
- **Release asset name**: contains the version (e.g. `Voicen_0.2.0_x64-setup.exe`) so users can tell releases apart.
- **UI language** (FR-15): About, the "Open logs folder" item, the log-folder notifications and the uninstaller question exist in English and Russian; the uninstaller follows the Windows display language, English otherwise.

## Requirements *(mandatory)*

### Functional Requirements

**Build information**

- **FR-001** (req FR-18, P-010): The system MUST resolve the version and the commit of the running build from one source, used by About, the start log line, crash files and the CI smoke check. The commit MUST be the CI-supplied commit when present, else the checkout's commit, else `unknown` — never empty.
- **FR-002** (req FR-18; Clarification Q1): The About dialog (opened by an "About Voicen" button on the General tab of the settings window) MUST show `Voicen <version> (<commit>)`, the license (MIT), the project link, a "Third-party licenses" entry and an "Open logs folder" button, all reachable and operable with the keyboard; if the build information cannot be read it MUST show "Cannot read build info".

**Log**

- **FR-003** (req FR-20, FR-18): At every start of the primary instance, the first log line of the session MUST contain the version and commit in the form `voicen <version> (<commit>) started`, plus the OS version and the start time.
- **FR-004** (req FR-20): The system MUST write the log to `%LOCALAPPDATA%\Voicen\logs`, resolved from the one app-data-folder resolver (P-010); each line MUST carry a timestamp with UTC offset and a level, and lines written concurrently from several threads MUST never interleave or be cut (one complete line per event, UTF-8).
- **FR-005** (req FR-20, NFR-01, NFR-02): For every dictation the log MUST hold one line with the engine (kind and model), the outcome (delivered / failed with category / discarded with reason), and the timings hotkey press → first audio frame, stop → text, text → paste in milliseconds (a timing that did not happen is shown as absent, not as 0). The values are supplied by the dictation pipeline (feature 001); this feature defines the record and writes it.
- **FR-006** (req FR-20): Errors and warnings (engine failures, notification failures, VAD fallback, hotkey registration failures, file errors) MUST be logged with a category, and where applicable an HTTP status, host and OS error code.
- **FR-007** (req FR-20, NFR-04, NFR-06, P-009): Log lines MUST be built only from an allowlist of typed fields (version, commit, OS version, engine kind, model name, host, timings, counts, sizes, categories, status codes, error codes, file names inside the app data folder); there MUST be no field that can carry transcript text, prompts, audio, API keys, request or response bodies, headers, full URLs or raw library error text. Output of third-party libraries MUST NOT reach the log file except through this allowlist.
- **FR-008** (req FR-20): Log files older than 7 days MUST be deleted at start and at each rotation; the total size of log files MUST stay ≤ 10 MB by deleting the oldest log files first; the file currently written is rotated when it reaches 2 MB or when the local date changes, so that no single file can push the total past 10 MB. Crash files MUST be excluded from this rotation and from the 10 MB total.
- **FR-009** (req FR-20 failure branch; Clarification Q3): If the log directory cannot be created or written (which also disables the session marker and crash files for that session), the app MUST keep working with all features, MUST show the notification "logs cannot be written: <reason>" at most once per session, and MUST resume writing without a further notification if the directory becomes writable again.
- **FR-010** (req FR-20, NFR-05): Logs and crash files MUST stay on the machine; the system MUST NOT upload or transmit them.

**Crash records**

- **FR-011** (req FR-20, NFR-07): On an unhandled panic in any thread, the system MUST write a crash file to the logs folder before the process ends.
- **FR-012** (req FR-20, NFR-07): On a native fault reported to the process's unhandled-exception filter, the system MUST write a crash file with the exception code before the process ends.
- **FR-013** (req FR-20, r2#4): At start, the primary instance MUST create a session marker; at a clean exit (tray Exit, Windows logoff/shutdown) it MUST remove it. If the marker exists at the next start, the system MUST: (a) if a crash file was written for that session, log "previous session crashed" naming it, without a second crash file; (b) if the installer ended that session, log "previous session was ended by the installer" without a crash record; (c) otherwise write a crash record — a crash file of kind "abnormal end" — "previous session ended abnormally" and log it.
- **FR-014** (req FR-20, NFR-04; Clarification Q2): A crash file MUST contain only: kind (panic / native fault / abnormal end), time, version, commit, OS version, session start time, thread name, and for a panic the source location and a backtrace, for a native fault the exception code and the faulting module with offset. Backtrace frames are recorded as module + offset (shipped binaries carry no symbols); the matching symbol files are kept by CI with each build so a crash can be symbolised later. It MUST NOT contain the panic message text, transcript text, audio, keys, memory contents or a memory dump.
- **FR-015** (req FR-20, r2#1): Crash files MUST be kept 30 days and at most 20 files: at start, files older than 30 days are deleted and, of the rest, only the 20 newest are kept.
- **FR-016** (req FR-20): The crash path MUST NOT itself panic, block indefinitely, or depend on the log being writable; a failure to write a crash file is covered by the session marker at the next start.

**Open logs folder**

- **FR-017** (req FR-01, FR-20): The "Open logs folder" action (tray menu item owned by 001, and the About button) MUST open `%LOCALAPPDATA%\Voicen\logs` in Explorer, creating it first if missing; on failure it MUST notify "cannot open the logs folder: <reason>".

**Installer and uninstaller**

- **FR-018** (req NFR-09): The installer MUST be a single `.exe` that installs per-user without administrator rights or a UAC prompt, and MUST create a Start-menu shortcut that carries the app's identity for toast notifications (requirements §8).
- **FR-019** (req NFR-09): The installed size — the files the installer wrote, excluding downloaded local models and other user data — MUST be ≤ 100 MB; CI MUST measure it right after the silent install (before first launch) and fail above the limit.
- **FR-020** (req FR-12, NFR-12): The installer MUST bundle the Silero VAD model and the third-party notices file; it MUST NOT bundle whisper transcription models.
- **FR-021** (req FR-28; Clarification Q4): On uninstall (not on upgrade), the uninstaller MUST ask "Remove settings, history, models, logs and saved keys?" with "yes" as the default. "Yes" MUST remove `%LOCALAPPDATA%\Voicen` and every Voicen entry in Windows Credential Manager; "no" MUST keep both. A silent uninstall MUST apply "yes"; the `/KEEPDATA` switch MUST apply "no".
- **FR-022** (req FR-28, NFR-04, P-010): The set of Credential Manager entries removed MUST be identified by the same key-slot naming the app uses (feature 004), not by a separate list kept in the uninstaller; if removal fails, the uninstaller MUST tell the user which entries to remove by hand and continue.
- **FR-023** (req FR-28, FR-19): The uninstaller MUST always remove program files, shortcuts and the autostart entry, and MUST close a running app first; it MUST NOT delete anything outside the install folder and `%LOCALAPPDATA%\Voicen`.
- **FR-024** (req FR-28, NFR-06): An upgrade (installing a newer version over an existing one) MUST NOT ask about or remove user data or keys.
- **FR-025** (req FR-15): The uninstaller's question and messages MUST be in English and Russian, following the Windows display language, English otherwise.

**Pipeline and release**

- **FR-026** (decisions #4): A public GitHub repository for the project MUST exist with `main` as the default branch and GitHub Actions enabled, before the Windows job and releases can run (prerequisite; owner consent required for its creation).
- **FR-027** (requirements §9 "deployed", FR-18): For every push to `main` and every pull request, CI MUST run the gate on Linux and then, on `windows-latest`, run the full workspace tests, build the NSIS installer for that commit, install it silently per-user, launch the app and require the start log line with that commit within 30 s; on failure it MUST print the log (or its absence) and fail. The installer MUST be kept as a build artifact named with the commit.
- **FR-028** (requirements §9 "deployed", FR-20, FR-28): The Windows job MUST also check, on the installed app: the installed size limit (FR-019), that the logs folder exists, and a silent uninstall with `/KEEPDATA` (data kept) followed by a silent uninstall with the default (data folder gone).
- **FR-029** (requirements §9 release, §2 success 3; Clarification Q5): On a pushed tag `v<version>`, after the gate and the Windows job pass for that commit, CI MUST create a GitHub Release named for the tag with the installer and a SHA-256 checksum file attached. If the tag's version differs from the app version, or the release already exists, it MUST fail and publish nothing. *(Decisions #102, #103, T-026: "differs from the app version" means the tag's MAJOR.MINOR differs from the repository's, or the tag patch is below the tag run's `github.run_number`; the job names both values.)*
- **FR-030** (OQ-02): Release 1 installers are unsigned; the release notes MUST state that Windows SmartScreen may warn and how to proceed.

**License compliance**

- **FR-031** (req NFR-12): CI MUST check the licenses of all Rust crates and npm packages that end up in the shipped app against one approved-license list kept in the repository (permissive licenses compatible with distribution under MIT; copyleft licenses such as GPL, LGPL and AGPL are not approved); an unapproved or undeterminable license MUST fail the check and name the component.
- **FR-032** (req NFR-12): The project MUST keep a third-party notices file listing every bundled component (crates, npm packages bundled into the UI, whisper.cpp, Silero VAD model) and the license of the downloadable whisper models, generated from the dependency tree plus a short hand-maintained list for non-package components; CI MUST fail if it is out of date with the dependency tree.
- **FR-033** (req NFR-12; Clarification Q1): The third-party notices MUST be installed with the app; About's "Third-party licenses" entry MUST open the installed notices file with the system's default viewer, and report "cannot open the license list: <reason>" if that fails.

**App data location**

- **FR-034** (req FR-28): The app MUST keep all its data — including the embedded web view's data (cache, local storage) — inside `%LOCALAPPDATA%\Voicen`, so that an uninstall answered "yes" leaves no Voicen data elsewhere in the user profile (Credential Manager entries are removed per FR-022).

### Key Entities

- **BuildInfo**: version and commit of the running build; one resolver, many consumers (About, start line, crash files, CI smoke).
- **Log line**: timestamp with UTC offset, level, event name, allowlisted typed fields only.
- **Dictation record**: engine (kind, model), outcome (delivered / failed + category / discarded + reason), timings (press → first frame, stop → text, text → paste; each optional), local-engine `cold`/`warm` mark (002); never text or audio.
- **Log file set**: the active file plus rotated files in the logs folder; retention 7 days and ≤ 10 MB total.
- **Session marker**: a small file in the logs folder written at start by the primary instance, holding the session start time, version, commit and process id; removed on clean exit; an "ended by installer" note can be added by the installer.
- **Crash file**: one file per crash in the logs folder with the allowlisted content of FR-014; retention 30 days and ≤ 20 files, outside log rotation.
- **Uninstall choice**: yes (default) / no; from the dialog or from the command line (silent default yes, `/KEEPDATA` no); skipped on upgrade.
- **Release**: tag `v<version>`, the installer, its SHA-256 checksum, release notes.
- **Third-party notice**: component name, version, license identifier and text, source (crate, npm, native, model).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: For 100% of CI runs on `main`, the installed app's first log line of the session carries the commit that was built (the Windows job checks it).
- **SC-002**: After a test session that dictates, fails a request with a known test key and crashes once, the log folder (logs and crash files) contains 0 occurrences of the dictated text, the test key or any audio bytes.
- **SC-003**: The log folder's log files never exceed 10 MB in total and never include a file older than 7 days after a start or rotation; crash files survive any number of rotations up to 30 days.
- **SC-004**: Every abnormal end of a session — panic, native fault, kill — produces exactly one crash record by the end of the next start; a clean exit or OS shutdown produces none.
- **SC-005**: The installer installs without an administrator prompt and the installed app (without models) is ≤ 100 MB.
- **SC-006**: After an uninstall answered "yes", `%LOCALAPPDATA%\Voicen` does not exist and Credential Manager lists 0 Voicen entries; after "no", both are unchanged.
- **SC-007**: A `v*` tag matching the app version leads to a published GitHub Release with the installer and checksum without manual steps; a mismatched tag publishes nothing.
- **SC-008**: The license check fails for 100% of added dependencies with a disallowed or unknown license, and the third-party notices list every bundled component.
- **SC-009**: On a clean Windows 11 (Windows Sandbox), the installer from GitHub Releases installs and the user completes a first dictation (success criterion 3; owner's manual check).

## Assumptions

- The tray menu, its "Open logs folder" item, notifications (toast + overlay + tray state), single-instance handling and the measurement of the dictation timings belong to feature 001; this feature provides the actions, the log facility and the record they write into (P-011: one log seam).
- The settings window and its General tab, the key slots in Credential Manager and their names, the autostart entry and the message catalog belong to feature 004; this feature adds the "About Voicen" button and its dialog to that tab and reuses the key-slot naming for the uninstaller.
- The Silero VAD model file is chosen and loaded by feature 001; this feature only packages it.
- All app data lives in `%LOCALAPPDATA%\Voicen` (requirements §7); the program itself may be installed in the same folder (the Tauri per-user default) — "no" on uninstall keeps the data but removes the program files.
- No log level setting and no verbose/debug mode in release 1; the log holds information, warnings and errors only.
- Log and crash-file times are the local time with UTC offset, so the owner can read them directly.
- The release is unsigned (OQ-02 open); code signing, auto-update and an in-app "check for updates" are out of scope (requirements §3).
- No user notification about a previous crash in release 1; the owner checks the crash files (success criterion 2). The log line at the next start is the in-app trace.
- The repository is created under the owner's GitHub account with `gh`; creating it and pushing to it need the owner's explicit consent (requirements §6 install policy, CLAUDE.md "never push from a DEV session").
- Verification placement: log format, redaction, retention, marker and crash-file logic and the build-info resolver in core on the Linux host with a temporary directory and a fake clock; About and the "Open logs folder" button with Playwright and mocked IPC on the Linux host and in CI; the panic/native-fault handlers, Explorer opening, installer, uninstaller, Credential Manager removal and installed size on the Windows CI runner; clean-install, SmartScreen and two-week crash-free use by the owner's manual check.
