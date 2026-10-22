# Implementation Plan: Call Mode 1a — Record and Read

**Branch**: `007-call-mode-record` | **Date**: 2026-10-22 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/007-call-mode-record/spec.md`

## Summary

Record a call as two never-mixed tracks and read its transcript afterwards. Requirements: v7 §3 row "1a — Record and read"; the spec's source list; decisions #159–#185.

- Record starts from the tray, the call hotkey or `voicen.exe --call record` (#184), always with a visible interactive indicator.
- While recording, the app writes `mic.opus` and `system.opus` incrementally with a `meta.json`.
- After stop, a separate final pass sends ≤ 120 s chunks, cut at Silero pauses, to the dictation API engine.
- The two tracks are merged into `transcript.json` (schema 1) and `transcript.md`.
- A Calls window lists, opens, renames, searches, copies, exports and deletes calls.
- A crashed recording is recovered, and the uninstaller keeps calls by default.

Technical approach (see [research.md](research.md)):

- **Core (`crates/voicen-core`)**:
  - a new `call` module: `session`, `capture` (QPC timeline, drift, silence fill, WAV source), `codec` (Ogg Opus), `storage` (`CallStore`, `meta.json`, recovery, roots, disk), `final_pass` (queue, chunker, timed/time-less), `transcript` (merge, schema 1, Markdown), `events`;
  - extensions to existing modules: `vad::SpeechSpans`, `engine` `transcribe_timed`, the `calls` settings section and `LaunchArgs`, the tray state and menu table, new `LogEvent` variants, and `wer`.
  - All of it is tested on Linux with fakes, WAV files, real libopus and a wiremock server.
- **Shell (`src-tauri`)**:
  - `CpalTrackSource` (mic, plus loopback on the default output device; QPC stamps);
  - `win/audio_notify.rs` (IMMNotificationClient) and the communications-device check;
  - `DiskSpace`;
  - the call-hotkey slot on the existing hotkey thread;
  - the `--call record` route through the single-instance callback;
  - exit confirmation, `WM_ENDSESSION` and sleep finalization;
  - the `call-indicator` and `calls` windows, IPC commands, toasts through T-007's notifier, and the clipboard copy through the dictation writer.
- **UI (`src/`)**: `/calls` and `/call-indicator` routes, the Calls settings tab, `callsApi.ts` and `indicatorApi.ts`, and Playwright specs with core-pinned wire fixtures.
- **Packaging/CI**:
  - NSIS calls question and `/PURGECALLS`, and a POSTUNINSTALL that keeps `calls\`;
  - the install smoke uses `--call record` and greps `call_refused part=system reason=no_device` (FR-043);
  - uninstall checks for a planted folder;
  - libopus `/MT` checks; licence entries.

## Technical Context

**Language/Version**: Rust 1.99 (edition 2021) for `voicen-core`, `voicen-whisper` and `src-tauri`; TypeScript and Svelte 5 for the UI; NSIS for the uninstaller hooks; PowerShell and bash in CI.

**Primary Dependencies**:
- **Existing, reused**: `cpal` 0.18.2 (loopback, `StreamInstant` capture stamps); `windows` 0.62.2; `rubato` 5; `reqwest` 0.13 through `engine::http`; `serde_json`; whisper-rs (Silero) in `voicen-whisper`; tauri 2.12 and single-instance 2.5.
- **New, approved by the owner (decisions #186, OQ-63 (a)); each is added to its `Cargo.toml` by the task that first uses it, not by this plan**:
  - `opus` 0.4 with `opusic-sys` 0.7 (bundled libopus 1.6.1, built with CMake);
  - `ogg` 0.9;
  - `windows` features `Win32_Media_Audio`, `Win32_System_Com`, plus `Win32_System_Com_StructuredStorage` only if a property store is read (no `implement` feature in 0.62);
  - `cmake` in `docker/rust.Dockerfile`.
- **Not added**: `claxon` (#186). The LibriSpeech FLAC files are converted to WAV once with an external tool, and the `wer` example reads WAV only.

**Storage**:
- call folders `<root>\<YYYY-MM-DD_HHMMSS>\` with `mic.opus`, `system.opus`, `meta.json`, `transcript.json`, `transcript.md` (§7); the default root is `%LOCALAPPDATA%\Voicen\calls`;
- the `calls` section in `settings.json`;
- no temp files during the final pass.

**Testing**:
- Linux host: `cargo test -p voicen-core` (Docker) for all call logic; vitest; `pnpm e2e` (Playwright, mocked IPC).
- Windows CI: `cargo test --workspace` and `src-tauri/tests/call_*.rs` with WAV sources and a local mock server, plus the install and uninstall smoke.
- Owner's PC: the manual checks in quickstart.md (`verify_exception` where no kit adapter exists).

**Target Platform**: Windows 10 22H2 / 11 x64; dev host Linux.

**Project Type**: desktop app (Tauri 2).

**Performance Goals**:
- a 1 h call is transcribed ≤ 5 min after stop (SC-003, NFR-13 API);
- tracks are aligned within ≤ 100 ms at minute 60 (FR-011);
- both tracks take ≤ 40 MB/h (FR-015);
- a lost track resumes within ≤ 2 s (FR-012);
- dictation keeps the NFR-02 start figure during a call (FR-009).

**Constraints**:
- never log transcript text, audio, keys, titles, paths, hosts or device friendly names (FR-042, NFR-16);
- no call audio or text written while Record is off (FR-019);
- no crash or audio change on any failure (FR-032, NFR-15);
- static CRT for every C library in `voicen.exe` (ci-toolchain.md);
- Windows-only code only in `src-tauri` or behind core ports;
- shell tests only in `src-tauri/tests`;
- no push from a DEV session.

**Scale/Scope**: one call at a time, ≤ 4 h each; one pass at a time, with at most 3 chunk requests in flight and one chunk decoded at a time (#187); calls kept until deleted (#170).

No NEEDS CLARIFICATION remains. Behaviours the Linux host cannot observe are marked **confirm on Windows** or **confirm at spike** in research.md. Each is the first acceptance item of the task that depends on it.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-checked after Phase 1 design.*

| Principle | How this plan complies | Status |
|---|---|---|
| I. PRINCIPLES.md | **P-004/P-005**: every guarantee has a red test with its failure branch. The guarantees are toggles and refusals, drift, silence fill, flush interval, truncated read, recovery, disk thresholds, chunk cuts, the `verbose_json` → `json` fallback, merge and schema, renames, delete in use, tray precedence, hotkey equality, uninstall keep/purge and log cleanliness.<br>**P-006**: the smoke runs the shipped `--call record` path on the installed app.<br>**P-009**: typed `LogEvent`, validated `EndpointId`, opaque `root`/`id` on the wire.<br>**P-010/P-011**: one session path for every trigger, one HTTP client, one VAD stack, one notifier, one clipboard writer, one data-dir resolver.<br>**P-016**: a `wip/` run before CODE_COMPLETE.<br>**P-020**: the guard files in contracts/installer-ci.md change only in `checker: true` tasks. | PASS |
| II. Privacy by default | Audio and text stay in the call folder. The only upload is the final pass to the user's own API engine; FR-031 names its host. Nothing is logged that could carry content (FR-042). No secret mode: the indicator is always shown (#165). Calls are unencrypted, and the docs say so (FR-021). | PASS, with a note |
| III. Never lose words | Pages are flushed every 1 s and synced every 5 s. Recovery never rewrites audio. A failed pass keeps audio and per-track results. Exit, shutdown and sleep finalize, and a forced end recovers. The uninstaller keeps calls by default. | PASS |
| IV. Core + thin shell | Session, timeline, codec, storage, chunking, engine mapping, merge, schema, settings, tray state and launch args are core and Linux-tested. The shell holds only cpal/WASAPI, COM notification, Win32 disk and dialogs, the windows and NSIS. | PASS |
| V. Measured, not assumed | SC-003 comes from `final_finished ms=`; FR-011 from a ±50 ppm test; CA-9 from a 60-min encode test; WER from the `wer` example on pinned sets. The table below shows where each requirement is verified. | PASS |

**Note on II**: constitution v1.4.0 limited all audio by NFR-06 and cited requirements v3. Amended to v1.5.0 (#189): dictation audio keeps the NFR-06 limit; calls are kept until deleted (#170, FR-77). The plan follows v7 and the amended principle.

Post-design re-check (after data-model.md and contracts/): still PASS with the same note. No complexity-tracking entry.

## Module and seam breakdown (ADR-011 as amended; OQ-48)

ADR-011's capture / segmentation / recognition / speakers / session / storage / transcript split is kept, with two amendments:
- **segmentation** lives in `final_pass` plus the `vad::SpeechSpans` port: 1a has no live segmentation;
- **recognition** is the existing `engine` client extended with `transcribe_timed`, not a new module.

`speakers` is a fixed `me`/`s1` table in `transcript` until 1c.

| Seam | Where | Port / boundary |
|---|---|---|
| Call session | core `call::session` | `toggle_record(on, Source)`; the one lock |
| Track capture | core `call::capture` ↔ shell `CpalTrackSource` | `TrackSource`, `TrackSink`, `CaptureStamp` |
| Device events | shell `win/audio_notify.rs` → core | `DeviceEvents`, `CommunicationsCheck` |
| Encoding | core `call::codec` | `TrackEncoder` / `TrackDecoder` |
| Storage | core `call::storage` | `CallStore`, `DiskSpace` |
| Final pass | core `call::final_pass` → `engine`, `vad` | `transcribe_timed`, `SpeechSpans` (Silero in `voicen-whisper`) |
| Transcript | core `call::transcript` | pure functions |
| Launch | core `settings::gate` ↔ shell single-instance callback | `LaunchArgs`, `SecondLaunchAction::StartCallRecord`; `second_launch_action` changes signature (contracts/core-call.md "Launch arguments") |
| Tray | core `tray` table and state ↔ shell tray | `TrayState::CallRecording`, menu rows |
| Hotkey | core settings ↔ shell hotkey thread | second registrar slot (prepare/commit/abort) |
| Exit/power | shell exit handler, hotkey thread → core | `stop(StopReason)` / `finalize` |
| Notices | core `call::events` → T-007 notifier, indicator | `CallNotice`, `CallWarning` |
| UI | `calls`, `call-indicator`, settings Calls tab ↔ shell commands | contracts/ipc.md |
| Uninstaller | NSIS hooks | `$VoicenPurgeCalls`, `/PURGECALLS` |

## Where each requirement is verified

| Spec FR (req) | Linux host — core with fakes | Linux host — UI, mocked IPC | Windows CI runner | Owner's manual check |
|---|---|---|---|---|
| FR-001, FR-002 (FR-30, FR-31, FR-37) | session toggles, repeated on, refusals per part, no folder left, mic fallback | refusal notices | smoke: `call_refused part=system reason=no_device`, app alive | real start |
| FR-003, FR-043 (FR-32, #184) | `LaunchArgs`, `second_launch_action` | — | smoke via `--call record`; window census | — |
| FR-004 (FR-33) | hotkey equality both ways; conflict leaves the dictation hotkey and tray alone | Calls tab marks the field | second slot registers | Ctrl+Alt+R in a call |
| FR-005, FR-006 (FR-34, FR-35) | tray precedence, full product; warning list rules | indicator content, reminder 10 s, warning list add/dismiss | indicator style bits; shown without activation | focus, drag, above full screen (spike) |
| FR-007, FR-008 (FR-38, FR-52) | stop reasons; finalize → `ready-to-transcribe`; requeue at start | exit question text | — | sign-out, lid close |
| FR-009 (FR-36, NFR-02) | session independence; Esc only dictation | — | — | dictate during a call; NFR-02 from the log |
| FR-010–FR-014 (FR-39–FR-42, FR-46, NFR-22) | timeline ±50 ppm, gaps, re-anchor, silence fill, FR-013 rule, WAV source | — | WAV chain; notifier register/unregister; loopback refusal | device switches, Bluetooth |
| FR-015–FR-021 (FR-47–FR-51, NFR-06, NFR-16, NFR-18) | Ogg write, 40 MB/h, flush interval, truncated read, recovery, damaged, 4 h, disk thresholds, write failure, FR-019, temp-cleanup exclusion | states incl. recovered, damaged | WAV chain writes and reads | kill at minute N (SC-005) |
| FR-022–FR-029 (FR-60, FR-45, FR-59, FR-63–FR-65, FR-82, FR-83) | chunker; `verbose_json` → `json`; timeouts against a late server; per-track results; queue; restart | "Queued", failed reasons, transcribe again | Silero `SpeechSpans` on the model; WAV chain to a mock server | SC-003 timing |
| FR-030, FR-031 (FR-81, FR-67) | notice values; provider host rule; once per new host | provider line in Calls tab | — | toasts |
| FR-032 (NFR-15) | fault injection: capture loss, storage, engine errors, timeouts | — | — | — |
| FR-033–FR-037 (FR-71–FR-77) | listing, renames, export temp + rename, delete in use / recording / transcribing | list, search, renames, delete, export | copy/clipboard exclusion through the shell writer | real use |
| FR-038–FR-040 (FR-59, FR-80) | settings defaults, unwritable root, earlier roots | Calls tab | — | — |
| FR-041 (FR-28) | — | — | `/S` keeps the planted folder; `/S /PURGECALLS` removes only the default | interactive question EN/RU |
| FR-042 (FR-84, FR-20) | log lines; planted-content privacy test (SC-009) | — | smoke log has no planted value | — |
| FR-044 (NFR-21) | `make licenses` | — | crt-imports, opus cmake cache | consent given (#186) |
| FR-045, FR-046 (FR-15) | catalog parity | RU rendering in specs | — | README review |
| FR-047 (FR-60, CA-7) | `wer` scoring unit tests | — | — | WER run (SC-004) |

## Project Structure

### Documentation (this feature)

```text
specs/007-call-mode-record/
├── spec.md
├── plan.md              # this file
├── research.md          # Phase 0
├── data-model.md        # Phase 1
├── quickstart.md        # Phase 1
├── contracts/
│   ├── core-call.md     # core ports, file formats, engine mapping, launch args, log lines
│   ├── ipc.md           # windows, commands, events, UI components, message catalog
│   └── installer-ci.md  # command line, uninstaller, CI steps, guard files, licences
├── checklists/
└── tasks.md             # created by /speckit-tasks, not by this step
```

### Source Code (repository root)

```text
crates/voicen-core/src/
├── call/
│   ├── mod.rs
│   ├── session.rs        # CallSession, Toggles, StopReason
│   ├── capture.rs        # TrackSource/TrackSink, CaptureStamp, Timeline, levels, activity
│   ├── wav_source.rs     # WavTrackSource (FR-014)
│   ├── codec.rs          # TrackEncoder/Decoder, OpusOgg
│   ├── storage.rs        # CallStore, roots, meta.json, recovery, listing, DiskSpace
│   ├── final_pass.rs     # queue, chunker, modes, results
│   ├── transcript.rs     # schema 1, validate, merge, Markdown, renames
│   └── events.rs         # CallNotice, CallWarning
├── vad/                  # + SpeechSpans, energy spans
├── engine/               # + transcribe_timed, LangCode
├── settings/             # + calls section, FieldIds, SettingsTab::Calls; gate.rs: LaunchArgs, second_launch_action(&LaunchArgs)
├── tray.rs               # + CallRecording, precedence, menu rows
├── diag/                 # + call LogEvent variants, EndpointId
├── i18n.rs               # + the call ids in `messages!` (texts in the root i18n/en.json, i18n/ru.json)
└── wer.rs
crates/voicen-core/examples/wer.rs
crates/voicen-whisper/src/   # SileroDetector implements SpeechSpans

src-tauri/
├── src/call.rs              # wiring: session, windows, commands (not in build_app)
├── src/win/track_capture.rs # CpalTrackSource (mic, loopback)
├── src/win/audio_notify.rs  # IMMNotificationClient, communications check
├── src/win/disk.rs          # DiskSpace
├── src/win/hotkey.rs        # + call-hotkey slot, suspend → stop
├── src/tray.rs              # + --call record route, Call menu
├── tests/call_*.rs
├── capabilities/{calls,call-indicator}.json
└── windows/installer-hooks.nsh   # + calls question, /PURGECALLS, POSTUNINSTALL

src/routes/calls/, src/routes/call-indicator/, src/lib/calls/, src/lib/callIndicator/
src/lib/settings/ (Calls tab)
e2e/calls.spec.ts, e2e/call-indicator.spec.ts, e2e/settings-calls.spec.ts, e2e/fixtures/{calls,call-indicator}-wire.json
.github/workflows/ci.yml, scripts/ci/opus-cmake-cache.sh, scripts/ci/crt-imports.sh
.cargo/config.toml, docker/rust.Dockerfile
i18n/en.json, i18n/ru.json (the one EN/RU catalog at the repository root, decision #13; i18n/conformance.json is the shared render-rule fixture and gains no ids)
README.md (consent section EN/RU, FR-045), docs/architecture.md, docs/decisions/<area>.md
```

**Structure Decision**: the existing three-area layout with no new area. Call mode is a `call` module in the core next to the dictation pipeline, with thin shell adapters behind core ports and two new web routes. Opus and Ogg live in the core so that §9's Linux verification of write, truncation and recovery runs on real files.

## Dependencies on other features and tasks

- **T-102** (Silero in the installer, shell wiring): the final pass needs the bundled model (FR-023, #175). It must land before the final-pass shell task.
- **T-007** (failure feedback and notifier): toasts for FR-006, FR-030 and the stop notices. Indicator-only parts can go first.
- **001**: tray, hotkey thread, capture (`win/capture.rs`), clipboard writer, single-instance callback.
- **004**: settings window, tabs, catalog, registrar two-step.
- **006**: log, `AppPaths`, installer hooks and the forked NSIS template (T-025), smoke and uninstall steps, licence pipeline.
- **API engine and key slots** (001/004): the 1a final engine (FR-022).

## Owner decisions needed

1. **New dependencies** (OQ-63 (a), FR-044), each asked for when its increment starts:
   - `opus` 0.4 plus `opusic-sys` 0.7 (bundled libopus, CMake);
   - `ogg` 0.9;
   - the `windows` features `Win32_Media_Audio` and `Win32_System_Com` (plus `Win32_System_Com_StructuredStorage` if needed);
   - `cmake` in `docker/rust.Dockerfile`;
   - optional `claxon` for the WER example (or the owner converts the FLAC files once with an external tool).

   **Decided (#186)**: all approved except `claxon`; the FLAC files are converted once with an external tool.
2. **Chunk concurrency** (research R11): at most 3 requests in flight, with decoding still one chunk at a time, against FR-018's wording; or strictly sequential, which risks SC-003. **Decided (#187)**: at most 3 in flight.
3. **Silero unavailable during a pass** (research R8): use energy spans with a logged `vad_fallback`, which may send music (against SC-008); or fail the track with a new reason that FR-027's list lacks. **Decided (#188)**: energy spans with `vad_fallback`.
4. **Fallback codec** if libopus cannot be built `/MT` (research R5): pure-Rust `ropus` (young), or a prebuilt `/MT` library. Open: OQ-66.
5. **Constitution amendment** for principle II and the v3 references (Constitution Check note). **Decided (#189)**: amended, constitution v1.5.0.
6. **OQ-45** stays open until the indicator spike. **OQ-48** is answered here as amended ADR-011, for owner confirmation. **OQ-44** needs nothing in 1a (FR-61 is 1d).

## Risks

- The libopus static CRT is unproven: opus's CMake defaults to `/MD` (research R5).
- The indicator mechanism (focus, full-screen apps) is open until the spike (OQ-45).
- The runner has no capture endpoint (windows-ci-runner.md). Real capture, device switches and the drift on real hardware are proven only on the owner's PC.
- The QPC semantics of `StreamInstant` and loopback silence-as-no-callbacks need confirmation on Windows (research R1, R2).
- OpenAI's `gpt-4o(-mini)-transcribe` refuse `verbose_json`. Those models always take the time-less path, with coarser line times.
- POSTUNINSTALL's current `RMDir /r` would delete calls, and `$INSTDIR` is the data folder's parent. The installer task must land before any user records a call with an installed build (research R15).
- Antivirus may flag loopback capture, and the 1a scope is heavy (requirements §10). OQ-64 (a) keeps it as written.

## Complexity Tracking

No constitution violations. Principle II was amended to v1.5.0 (#189), see the note.
