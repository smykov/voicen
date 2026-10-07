# Data Model: Diagnostics and Release

Phase 1 of [plan.md](plan.md). All types live in `voicen-core` unless marked *(shell)*. Nothing here holds transcript text, prompts, audio or keys (spec FR-007, FR-014).

## BuildInfo (exists — `voicen_core::build_info`)

| Field | Type | Source | Rule |
|---|---|---|---|
| `version` | `&'static str` | workspace `Cargo.toml` version | single version source (research R12) |
| `commit` | `&'static str` | `VOICEN_COMMIT` (CI) → `git rev-parse --short=7 HEAD` → `"unknown"` | never empty (spec FR-001) |

Display: `<version> (<commit>)`. Consumers: About, start line, crash files, session marker, CI smoke.

## AppPaths (`voicen_core::paths`) — one resolver (P-010)

| Path | Value | Used by |
|---|---|---|
| `root` | `<base>\Voicen` (`<base>` = `%LOCALAPPDATA%`, or the temp dir when unset) | uninstaller "yes" target |
| `logs` | `root\logs` | log files, crash files, session marker, installer note |
| `webview` | `root\webview` | web view data directory of every window (spec FR-034) |
| `models`, `tmp`, settings file, history file | owned by 002 / 001 / 004 / 005 | — |

## LogEvent (typed allowlist — spec FR-003..FR-007)

An enum; each variant has only typed fields. No variant has a `String` that can be set from transcript, prompt, request/response bodies, headers, URLs or library error text.

T-008 shipped the variants marked "T-008". Their fields hold only integers, closed enums of the crate and `BuildInfo`; there is no `String`, `&str`, path, `io::Error` or `FailureReason` (`crates/voicen-core/src/diag/event.rs`, contracts/core-diag.md "Log"). The rows marked "later" are the plan for later tasks. Each of them arrives as a new variant under the same rules.

| Variant | Fields | Level | Status |
|---|---|---|---|
| `Started` | `build: BuildInfo`, `pid: u32` | INFO | T-008, without `os`. `os: OsVersion` comes later, with the session work |
| `Dictation` | `DictationLine` (below) | INFO (delivered, no_speech, too_short) / WARN (failed, capture_failed) | T-008, T-051 |
| `DictationBlocked` | `reason: settings::gate::Blocked` (`no_engine`), written `dictation outcome=blocked reason=<reason>`; no `rec=` (a blocked press is not a recording) | WARN | T-006 (from `DictationEvent::PressBlocked`) |
| `Warning` | `kind: WarningKind`, `os_code: Option<i32>`. Kinds in T-008: `vad_fallback`, `esc_unavailable`, `toast_failed` (the pipeline's `WarningCode`s), `models_cleanup_failed`, `settings_window_failed`, `change_bridge_failed`. T-052: `settings_opener_failed` (the settings window's opener thread could not start), `tray_failed` (the tray icon could not be built, or a state could not be applied to it), `tray_follower_failed` (the tray's language follower thread could not start). Later features add kinds (e.g. notification or history write failures) | WARN | T-008 |
| `SettingsLoad` | `LoadKind`: `loaded` / `first_run` / `reset` / `unavailable` (from `LoadOutcome`, without the settings or the backup name) | INFO / WARN (reset, unavailable) | T-008 (spec 004 R-11) |
| `SettingsSave` | `SaveLine`: `ok` with warnings, or `refused` / `failed` with field ids + codes, the form error and `not_restored` field ids; no values (from `SaveOutcome`) | INFO / WARN (failed) | T-008 (spec 004 R-11; no `changed=` list) |
| `AutostartReconcile` | `ReconcileAction`: `none` / `written` / `removed` / `failed` | INFO / WARN (failed) | T-008 (spec 004 R-11) |
| `LogsRecovered` | — | INFO | T-008 |
| `PreviousSession` | `outcome: Clean \| Crashed{crash_file: FileName} \| EndedByInstaller \| EndedAbnormally{crash_file: Option<FileName>}` | INFO / WARN | later (session marker) |
| `Error` | `area: ErrorArea`, `category: ErrorCategory`, `http_status: Option<u16>`, `os_code: Option<i32>` (no `host`: see below) | ERROR / WARN | later |
| `CrashFileWritten` | `file: FileName`, `kind: CrashKind` | ERROR | later (crash files) |
| `RetentionDeleted` | `logs: u32`, `crash_files: u32` | INFO | later, if wanted (T-008 retention writes no line) |
| `Exited` | `reason: TrayExit \| SessionEnd` | INFO | later (session marker) |

Bounded string newtypes (constructed only through validating constructors). None exists after T-008:

| Newtype | Constraint | Status |
|---|---|---|
| `HostName` | host part of a URL only (no scheme, user-info, path, query); ≤ 253 chars; built from the configured URL, never from error text | not built. T-008 logs no host (Q1 default, decision #64). Adding one is a new decision |
| `ModelName` | ≤ 128 chars, printable, no whitespace control chars; quoted in output | not built. T-008 logs no model name (Q1 default, decision #64). Adding one is a new decision |
| `FileName` | a file name inside the data folder (no directories) | later (crash files, session) |
| `OsVersion` | `major.minor.build` digits only | later (`Started`, crash files) |

Feature extensions: 001 contributes outcome and failure values through its events (`DictationEvent`, `FailureReason::code()` via `FailureTag`); 002 adds `local: Option<Cold|Warm>`, `load_ms` (002 data-model); 003/005 add warning kinds. Adding a variant or field is the only way to log something new — reviewed under P-009.

## DictationLine (was "DictationRecord"; assembled by `LogObserver` from 001's events — spec FR-005)

T-008 shipped `DictationLine`, which `LogObserver` aggregates per `RecordingId` from the pipeline's `DictationEvent`s (contracts/core-diag.md "Pipeline observer"). There is no `DictationRecord` value from the pipeline and no `dictation_finished` callback. A field whose event did not come is `None`, and its key is left out of the line (never `0`).

| Field | Type | Source event | Status |
|---|---|---|---|
| `recording` | `u64` (`RecordingId::get()`) | every event of the recording | T-008 |
| `engine` | `Option<EngineTag>`: `api` / `builtin` / `local_server` / `other` (`EngineTag::from_kind`) | `JobFinished.engine` (absent when no engine was built) | T-008 |
| `outcome` | `Delivered(DeliveryResult)` (`pasted` / `copied_only` / `copy_manual`), `Failed{failure: FailureTag, http_status: Option<u16>}`, `NoSpeech`, `TooShort`, `CaptureFailed{cause: MicCause}` (`no_device` / `access_denied` / `busy` / `other`, written `outcome=capture_failed mic=<cause>`) | `Delivered`, `JobFinished`, `RecordingEnded{TooShort}`, `CaptureFailed` | T-008; `CaptureFailed` T-051 |
| `detector` | `Option<DetectorTag>`: `silero` / `energy` / `other` | `SpeechGate.detector` | T-008 |
| `press_to_frame_ms` | `Option<u64>` | `RecordingStarted` (emitted by the dictation session at the release, T-051) | T-008 |
| `duration_ms` | `Option<u64>` | `RecordingEnded` | T-008 (Q2: FR-20's three timings plus the duration) |
| `stop_to_text_ms` | `Option<u64>` | `JobFinished` | T-008 |
| `text_to_paste_ms` | `Option<u64>` | `Delivered` | T-008 |
| `model` | — | — | not logged (T-008 Q1, decision #64; no `ModelName`) |
| `post_processing` | `Option<Applied \| Skipped{category}>` | 003 | later |
| `local`, `load_ms` | see 002 data-model | 002 | later |

## LogWriter state

```text
            write ok
   ┌──────────────────────┐
   ▼                      │
 Healthy ──write/open fails──▶ Degraded{since, notified=true(once per session)}
   ▲                                │
   └──── reopen succeeds (≤ every 60 s) ─┘   (emits LogsRecovered, no notification)
```

| Attribute | Rule |
|---|---|
| active file | `logs/voicen.log`, never larger than 2 MiB (`roll_bytes`) |
| roll | before a line would make the active file larger than 2 MiB, or at the local date change → `voicen-YYYYMMDD-HHMMSS.log` (local time of the roll; `-1`, `-2`, … when the name is taken) |
| retention | at start and after each roll: delete rolled `voicen-*.log` files older than 7 days (by modification time), then the oldest rolled files while the rolled files total more than 10 MiB − 2 MiB = 8 MiB (`total_bytes − roll_bytes`); never the active file or any other file. Rolled ≤ 8 MiB plus active ≤ 2 MiB keeps the whole set ≤ 10 MiB at every moment (T-008: the earlier wording, "delete while total `voicen*.log` > 10 MB" after a roll, let the rolled files keep 10 MB and the active file grow to 2 MB on top, 12 MB on disk) |
| notify signal | `LogsUnwritable{reason}` at most once per `Log`, the one log of the process |

## SessionMarker (`logs/session.marker`)

| Field | Type |
|---|---|
| `pid` | `u32` |
| `started` | local time with offset (identity of the session) |
| `version`, `commit` | from BuildInfo |

Lifecycle: written by `Session::begin()` (primary instance only, after the single-instance check) → deleted by `Session::end_clean()` (tray Exit, `WM_ENDSESSION`). Found at start → classification (research R7).

## InstallerNote (`logs/installer-ended`)

One line: local time with offset. Written by the NSIS pre-install / pre-uninstall hooks; consumed and deleted at the next start.

## CrashFile (`logs/crash-YYYYMMDD-HHMMSS-<pid>.txt`) — spec FR-014

Plain text, `key: value` lines, fixed order:

| Key | Value |
|---|---|
| `kind` | `panic` / `native_fault` / `abnormal_end` |
| `time` | local time with offset |
| `version`, `commit` | BuildInfo |
| `os` | OsVersion |
| `session_started` | marker `started` (links the file to the session) |
| `thread` | thread name or `unnamed` (panic only) |
| `location` | `file:line:column` (panic only) |
| `exception_code` | `0xC0000005` etc. (native fault only) |
| `fault_address` | `<module>+0x<offset>` (native fault only) |
| `backtrace` | one `<module>+0x<offset>` per line (panic only) |
| `previous_session_pid` | (abnormal end only) |

Never: panic message, transcript, audio, keys, memory contents.

Retention: at start, delete files older than 30 days, then keep the 20 newest (by time in the name, falling back to file time).

## UninstallChoice *(installer)*

`Yes` (default; silent default) / `No` (dialog or `/KEEPDATA`) / `Skipped` (update mode). `Yes` → `voicen.exe --purge-credentials` then `RMDir /r %LOCALAPPDATA%\Voicen`.

## CredentialTargetPrefix

The `voicen_core::secrets::CREDENTIAL_TARGET_PREFIX` const (value `"Voicen/"`; owned by 004's key-slot naming); used by the app's credential store and by `--purge-credentials`.

## Release *(CI)*

| Field | Rule |
|---|---|
| tag | `v<version>`; `<version>` must equal BuildInfo.version |
| assets | `Voicen_<version>_x64-setup.exe`, `SHA256SUMS.txt`, `voicen-<version>-symbols.zip` |
| notes | version, commit, checksum, SmartScreen note, link to notices |
| uniqueness | an existing release for the tag → fail, no overwrite |

## ThirdPartyNotice

| Field | Source |
|---|---|
| name, version | Cargo metadata / pnpm / `licenses/manual.json` |
| license (SPDX) | same |
| license text | cargo-about / package files / manual |
| origin | `crate` / `npm` / `native` / `model` |
