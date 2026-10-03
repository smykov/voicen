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

| Variant | Fields | Level |
|---|---|---|
| `Started` | `build: BuildInfo`, `os: OsVersion`, `pid: u32` | INFO |
| `PreviousSession` | `outcome: Clean \| Crashed{crash_file: FileName} \| EndedByInstaller \| EndedAbnormally{crash_file: Option<FileName>}` | INFO / WARN |
| `Dictation` | `record: DictationRecord` | INFO (delivered) / WARN (failed) |
| `Error` | `area: ErrorArea`, `category: ErrorCategory`, `http_status: Option<u16>`, `host: Option<HostName>`, `os_code: Option<i32>` | ERROR / WARN |
| `Warning` | `kind: WarningKind` (e.g. `VadFallbackToEnergy`, `NotificationFailed`, `HistoryWriteFailed`) , `os_code: Option<i32>` | WARN |
| `LogsRecovered` | — | INFO |
| `CrashFileWritten` | `file: FileName`, `kind: CrashKind` | ERROR |
| `RetentionDeleted` | `logs: u32`, `crash_files: u32` | INFO |
| `Exited` | `reason: TrayExit \| SessionEnd` | INFO |

Bounded string newtypes (constructed only through validating constructors):

| Newtype | Constraint |
|---|---|
| `HostName` | host part of a URL only (no scheme, user-info, path, query); ≤ 253 chars; built from the configured URL, never from error text |
| `ModelName` | ≤ 128 chars, printable, no whitespace control chars; quoted in output |
| `FileName` | a file name inside `AppPaths::root` (no directories) |
| `OsVersion` | `major.minor.build` digits only |

Feature extensions: 001 contributes the `DictationRecord` fields and `ErrorArea`/`ErrorCategory` values it needs; 002 adds `local: Option<Cold|Warm>`, `load_ms` (002 data-model); 003/005 add warning kinds. Adding a variant or field is the only way to log something new — reviewed under P-009.

## DictationRecord (fields supplied by 001's pipeline — spec FR-005)

| Field | Type | Note |
|---|---|---|
| `engine` | `EngineKind` (`api` / `builtin` / `local_server`) | |
| `model` | `Option<ModelName>` | |
| `outcome` | `Delivered{pasted: bool}` / `Failed{category}` / `Discarded{reason}` | categories from 001's failure classification |
| `press_to_frame_ms` | `Option<u32>` | absent if no audio frame |
| `stop_to_text_ms` | `Option<u32>` | absent if no text |
| `text_to_paste_ms` | `Option<u32>` | absent if not pasted |
| `post_processing` | `Option<Applied \| Skipped{category}>` | 003 |

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
| active file | `logs/voicen.log` |
| roll | at ≥ 2 MB or local date change → `voicen-YYYYMMDD-HHMMSS.log` |
| retention | at start and after each roll: delete rolled files older than 7 days, then oldest rolled while total `voicen*.log` > 10 MB; never the active file |
| notify signal | `LogsUnwritable{reason}` at most once per process |

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

A `const` in `voicen-core` (value owned by 004's key-slot naming, e.g. `Voicen/`); used by the app's credential store and by `--purge-credentials`.

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
