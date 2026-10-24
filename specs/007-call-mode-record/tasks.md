# Tasks: Call Mode 1a — Record and Read

**Input**: Design documents from `/specs/007-call-mode-record/`: plan.md (owner decisions #186–#189 marked Decided), spec.md, research.md, data-model.md, contracts/core-call.md, contracts/ipc.md, contracts/installer-ci.md, quickstart.md; `docs/decisions.md` #159–#189.

**Tests**: REQUIRED. The constitution (P-004, P-005, principle III) and the plan's Constitution Check need a red test per guarantee, failure branches included, written before the code it guards. In every group the red-test task comes first.

**Conversion**: each **group** (`G01`…`G46`) becomes one teamwright task (`docs/tasks/T-NNN.md`, `design_ref: specs/007-call-mode-record/tasks.md#gNN`), with one seam and one area. The orchestrator derives the teamwright tasks; `/speckit-implement` is not used. Tasks name spec requirements as `spec FR-0NN` / `SC-0NN`, source requirements as `req FR-NN` / `req NFR-NN`, and research items as `R<n>`.

**Areas**:
- `core`: `crates/voicen-core`, plus `crates/voicen-whisper` for G21 (Linux host in Docker through `scripts/tw-run core`; voicen-whisper is built and tested only in Windows CI);
- `shell`: `src-tauri`, Windows CI only, tests only in `src-tauri/tests/call_*.rs`;
- `ui`: `src/`, `e2e/` (vitest; `pnpm e2e` with mocked IPC through `e2e/support/tauriMock.ts`, specs import `test`/`expect` from `e2e/support/boot`);
- `installer-ci`: `.github/workflows/ci.yml`, `scripts/ci/`, `Makefile`, `docker/`, `.cargo/config.toml`, `src-tauri/windows/*.nsh`;
- `docs`: docs-only groups (no CI evidence needed, P-016).

**Markers**:
- **checker** = the group stages a guard path: a path under `scripts/ci/` or `scripts/licenses/`, the path `Makefile`, or a path ending in `.mk` (the `scripts/hooks/pre-commit` trigger, `make check-guard-trigger`; contracts/installer-ci.md "Guard and owned files"). The teamwright task needs `checker: true` and `analysis.decides_on` (P-020, decisions #154/#155). `.cargo/config.toml`, `docker/rust.Dockerfile`, `about.toml` and `licenses/manual.json` are not guard paths.
- **Needs T-102** = waits for the teamwright task T-102 (Silero ships in the installer).
- **Needs T-007** = waits for the teamwright task T-007 (toasts and notifier). Indicator-only and log-only parts of these groups can go first; the toast path stays open until T-007 is DONE.
- **First acceptance** = an item that must be proven, on the `wip/<ID>` run or by the owner, before any code builds on it (research "confirm on Windows" / "confirm at spike").
- **Owner** = a step only the owner can do.
- **Licences** = the group changes dependencies, so `make licenses` runs and `THIRD-PARTY-NOTICES.txt` is committed in the same commit (FR-044, req NFR-21).

## Format: `[ID] [P?] [Story?] [area] Description`

- **[P]**: can run in parallel (different files, no dependency on an incomplete task)
- **[Story]**: US1 record (P1), US2 final transcript (P1), US3 read/copy/export (P1), US4 never lose (P1), US5 failed pass (P2), US6 device changes (P2), US7 dictate during a call (P2), US8 manage calls (P2), US9 Calls settings tab (P2), US10 uninstall keeps calls (P2)

---

## Phase 1: Setup (toolchain and codec dependencies)

### G01 — Core image gains CMake (area: installer-ci) <a id="g01"></a>

- [ ] T001 [area installer-ci] Add `cmake` to `docker/rust.Dockerfile` (approved, decisions #186) and rebuild with `make core-image`. Evidence: `scripts/tw-run core -- cmake --version` works; `make check` is green, including the mingw build of `check-shell-windows` (contracts/installer-ci.md "Guard and owned files"; R5 "Linux image").

### G02 — Ogg Opus codec (area: core) <a id="g02"></a> — Licences

- [ ] T002 [area core] Add `opus` 0.4 (over `opusic-sys` 0.7, bundled libopus 1.6.1) and `ogg` 0.9 to `crates/voicen-core/Cargo.toml` (approved, #186). Run `make licenses`: `about.toml` accepts BSD-3-Clause if it does not already, `licenses/manual.json` gets the libopus copyright if the crate metadata lacks it, and `THIRD-PARTY-NOTICES.txt` is committed. Create `crates/voicen-core/src/call/mod.rs` with an empty `codec` module and add `pub mod call;` in `crates/voicen-core/src/lib.rs`. Depends on T001 (spec FR-044; req NFR-21).
- [ ] T003 [area core] Red tests in `crates/voicen-core/src/call/codec.rs`, on real libopus in the core image. Each format value is quoted from contracts/core-call.md "`mic.opus` / `system.opus`".
  - Page 1 is BOS with only `OpusHead`: "version 1, channels 1, pre-skip = lookahead × 3, input rate 16000, gain 0, mapping 0; granule 0". Page 2 is `OpusTags`, vendor `voicen <version>`, 0 comments.
  - Granules advance by "+960 per packet". A page ends every 50 packets, with an inner-writer flush after each page (counting fake writer) and `sync_data` every 5 s (fake clock).
  - The last page carries EOS and its final granule trims the tail.
  - 60 min of encoded speech-like signal on both tracks takes ≤ 40 MB (SC-007, CA-9).
  - The reader treats `UnexpectedEof` or a CRC mismatch on the last page as the end of recoverable data. A mismatch before the last page gives a damaged file whose readable prefix is still returned. No valid `OpusHead` → unreadable. Duration = last good granule − pre-skip.
  - An `io::Error` from write, flush or sync maps to `StorageError` (FR-016).
  - `TrackDecoder::next_window(max_ms)` returns at most `max_ms` of 16 kHz audio.
  - (spec FR-015, FR-016, FR-017, SC-005; R5, R6, R7)
- [ ] T004 [area core] Implement `TrackEncoder` / `TrackDecoder` and `OpusOgg` in `crates/voicen-core/src/call/codec.rs` until T003 is green. Encoder: "Opus Voip, 16 kHz encoder, 20 ms packets, VBR 24 kbit/s", complexity 9, DTX and FEC off (R5).

### G03 — libopus static CRT, proven in Windows CI (area: installer-ci) <a id="g03"></a> — checker

- [ ] T005 [area installer-ci] Red checker tests `scripts/ci/opus-cmake-cache.test.sh` with fixture caches under `scripts/ci/fixtures/opus-cmake-cache/`. A cache with `OPUS_STATIC_RUNTIME:BOOL=ON` and `CMAKE_BUILD_TYPE:STRING=Release` passes. A missing cache, `OFF`, any line naming `CMAKE_MSVC_RUNTIME_LIBRARY`, or a cache missing either owned line fails, with the reason named; `dumpbin -directives` of each `opus.lib` needs `/DEFAULTLIB:LIBCMT` and never `/DEFAULTLIB:MSVCRT` (decisions #197). The `crt-imports.sh` exe list is not extended here: T-127 / T-139 add their call test exes (contracts/installer-ci.md; R5; the T-099 pattern of `whisper-cmake-cache`).
- [ ] T006 [area installer-ci] Implement `scripts/ci/opus-cmake-cache.sh`, the `check-opus-cmake-cache` target in `Makefile` (wired into `check`), until T005 is green (no `crt-imports.sh` change: decisions #197).
- [ ] T007 [area installer-ci] Add a CMake toolchain file that sets `OPUS_STATIC_RUNTIME ON` and CMP0091 NEW. Point `CMAKE_TOOLCHAIN_FILE_x86_64_pc_windows_msvc` at it from `.cargo/config.toml`, drop restored opusic-sys outputs before the first cargo command, and add the `opus-cmake-cache.sh` step after the build in the windows job of `.github/workflows/ci.yml`; no action line changes. **First acceptance**: on the `wip/<ID>` run, `crt-imports.sh` (voicen.exe) and `opus-cmake-cache.sh` are green, and `whisper-cmake-cache.sh` stays green (whisper.cpp's cache is unchanged). **Failure branch**: if `/MT` cannot be reached, stop and set the teamwright task BLOCKED on OQ-66 (pure-Rust `ropus` or a prebuilt `/MT` lib, owner decides). Do not ship an `/MD` build. Depends on T004, T006 (R5; plan Risks).

**Checkpoint**: real Ogg Opus files are written and read on Linux, and the static CRT is proven on Windows.

---

## Phase 2: Foundational (blocks the user stories)

### G04 — Call types, log events, `EndpointId` (area: core) <a id="g04"></a>

- [ ] T008 [P] [area core] Red tests in `crates/voicen-core/src/diag/event.rs` / `format.rs` and `crates/voicen-core/src/call/events.rs`.
  - Every new `LogEvent` formats exactly as the line bodies in contracts/core-call.md "Log lines": `call_requested`, `call_refused`, `call_started`, `call_device`, `call_track_silent`, `call_stopped`, `call_storage_failed`, `call_recovered`, `final_stage`, `final_finished`, `call_switch_refused`, `vad_fallback track=<mic|system> cause=<unavailable|error>` (#188) and `transcript_invalid rule=<…> segment=<n>` (R17 rule names; never the segment text).
  - `EndpointId` accepts only "`{`, `}`, `.`, hex digits, `-`" and "at most 128 characters"; anything else formats as `endpoint=invalid`.
  - The exhaustive matches in diag/format.rs (`level`, `write_message`) and the `diag_format` tests cover every variant (`LogObserver` gets no call arm; decision #195).
  - `CallNotice` / `CallWarning` hold catalog ids and enums only, never free text.
  - `Track`, `CallId` (grammar `YYYY-MM-DD_HHMMSS[-n]`), `StopReason`, `FailReason` and `CallState` serialize with the names of data-model.md (snake_case; `CallState` keeps data-model's hyphenated `ready-to-transcribe`; decision #195).
  - (spec FR-042, SC-009; R20; P-009)
- [ ] T009 [area core] Implement the types in `crates/voicen-core/src/call/{mod,events}.rs`, the `LogEvent` variants, `format.rs` and `EndpointId` until T008 is green.

### G05 — Message catalog, EN and RU (area: core) <a id="g05"></a>

- [ ] T010 [P] [area core] Red tests in `crates/voicen-core/src/i18n.rs` (the `messages!` declaration and the existing catalog checks over the root `i18n/en.json` and `i18n/ru.json`): every id in contracts/ipc.md "Message catalog" exists in EN and RU, including `warning.provider_notice` with `{host}`, and the RU speaker labels are "Я" / "Собеседник". `calls.title_default` has the four distinct placeholders `{day}`, `{month}`, `{hour}`, `{minute}` in EN and RU, and renders a call started on 5 March at 09:07 as "Call 05.03 09:07". `notice.call_refused.{part}.{reason}` covers every part × reason of `CallRefused` (spec FR-046; req FR-15).
- [ ] T011 [area core] Add the ids to `messages!` in `crates/voicen-core/src/i18n.rs` and the texts to `i18n/en.json` and `i18n/ru.json` until T010 is green, and update `src/lib/i18n` id tests if they mirror the catalog. `i18n/conformance.json` (the render-rule fixture) is unchanged.

### G06 — `calls` settings section and provider-host rule (area: core) <a id="g06"></a>

- [ ] T012 [P] [area core] Red tests in `crates/voicen-core/src/settings/` (`validate.rs`, `mod.rs`, `service.rs`).
  - **Defaults** from data-model.md "CallSettings": `hotkey` Ctrl+Alt+R, `last_combination` `{record}`, `reminder` `true`, `primary_language` = the UI language, `storage_root` `None`, `earlier_roots` `[]`, `warned_hosts` `[]`, `indicator_position` `None`. An older `settings.json` without the section loads with these defaults and `SCHEMA_VERSION` stays 1.
  - **Hotkey**: the call hotkey is "Refused if equal to the dictation hotkey, from either side" (`FieldId::CallHotkey`).
  - **Storage root**: an unwritable root (`.voicen-write-probe` create and delete fails) is refused as `StorageRoot unwritable{reason}`. A new root appends the old one to `earlier_roots`, "Appended when the root changes; no duplicates".
  - **Primary language**: an unknown value is refused (`PrimaryLanguage`).
  - **Provider host**: the rule says whether a base URL host is loopback (`localhost`, `127.0.0.0/8`, `::1` → none; `api.openai.com`, `192.168.1.5` → host). A host is "never logged".
  - **Wire**: `SettingsTab::Calls` exists, and `e2e/fixtures/settings-wire.json` is extended and pinned by the core wire test.
  - (spec FR-004, FR-031, FR-038, FR-039, FR-040; req FR-33, FR-59, FR-67, FR-80)
- [ ] T013 [area core] Implement the section, the `FieldId`s, the validation, the host rule and the fixture until T012 is green.

### G07 — Launch arguments `--call record` (area: core) <a id="g07"></a>

- [ ] T014 [P] [area core] Red tests in `crates/voicen-core/src/settings/gate.rs`: `LaunchArgs::parse` recognises `--autostart` and `--call record`; `--call <other>` gives `CallSwitch::Unknown` (logged `call_switch_refused reason=unknown_value`, no action); `second_launch_action` changes signature from `(launched_by_autostart: bool)` to `(&LaunchArgs)` (contracts/core-call.md "Launch arguments"): `--call record` gives `StartCallRecord` with or without `--autostart`, an unknown value gives `TrayOnly`, and otherwise the existing rows hold (`second_launch_action_table` rewritten over `LaunchArgs` with the same rows) (spec FR-003, FR-043; R13; #184).
- [ ] T015 [area core] Implement `LaunchArgs` and the new `second_launch_action(&LaunchArgs)` until T014 is green. The existing caller `src-tauri/src/tray.rs` `on_second_instance` moves to `second_launch_action(&LaunchArgs::parse(&argv))` in the same commit, keeping the `StartCallRecord` arm a no-op until G14, and `src-tauri/src/autostart.rs` `launched_by_autostart` delegates to `LaunchArgs::parse` (one parser); the shell part is proven on the `wip/<ID>` run (`src-tauri/tests/single_instance.rs`, `autostart.rs`).

### G08 — Tray state and menu rows (area: core) <a id="g08"></a>

- [ ] T016 [P] [area core] Red tests in `crates/voicen-core/src/tray.rs`. The shown state follows "dictation `HotkeyError` > dictation `Recording` > `CallRecording` > `Error` > `Idle`", tested over the full product of dictation and call states. A call-hotkey conflict never yields `HotkeyError`. The menu table gains "Call" › "Record" (check item, checked while recording) and "Stop", plus "Calls" (spec FR-003, FR-004, FR-006; req FR-32, FR-34).
- [ ] T017 [area core] Implement `TrayState::CallRecording`, the precedence function and the menu rows until T016 is green.

### G09 — `transcript.json` schema 1, merge, Markdown (area: core) <a id="g09"></a>

- [ ] T018 [P] [area core] Red tests in `crates/voicen-core/src/call/transcript.rs`, with fixtures.
  - **Serde**: the types of §7 schema 1 use `deny_unknown_fields` and `schema: 1`. `engines.transcription` is `"api:<model>"` (the `EngineRef` string, the same one `meta.json` stores), `engines.diarization` is `null`, and there is no `hidden` or `confidence`.
  - **`validate()`** refuses each broken rule of R17: segments unsorted by `start_ms`; outside `0 ≤ start_ms ≤ end_ms ≤ duration_ms`; a speaker not in the meta speakers; a track not in {mic, system}; `source` ≠ `final`; a bad `lang`; empty `text`.
  - **Merge**: the lines of both tracks are ordered by start time and overlaps are kept (US2 #5). The segment time is "track `start_offset_ms` + chunk `start_ms` + engine segment time, clamped to the chunk". An empty engine segment "is dropped".
  - **Markdown** renders exactly as contracts/core-call.md "`transcript.md`" (`**[HH:MM:SS] <name>:** text`, names from `meta.json`), and the same renderer serves copy and export.
  - (spec FR-025, FR-026; req FR-63, FR-64, §7)
- [ ] T019 [area core] Implement the pure functions in `crates/voicen-core/src/call/transcript.rs` until T018 is green.

**Checkpoint**: the shared types, settings, launch, tray and transcript seams are green on Linux. The user-story groups can start.

---

## Phase 3: User Story 1 — Record a call as two tracks (P1) MVP

**Goal**: one session from the tray, the hotkey or `--call record` records `mic.opus` and `system.opus` with `meta.json`, always with the indicator; refusals leave nothing behind.

**Independent test**: Linux, the session with WAV fakes (start → `recording` folder → stop → readable files, `ready-to-transcribe`). Windows CI: the WAV chain; the installed app refuses with `no_device` and keeps running. Owner: a real call.

### G10 — Track ports, timeline, WAV source (area: core) <a id="g10"></a>

- [ ] T020 [P] [US1] [area core] Red tests in `crates/voicen-core/src/call/capture.rs` and `call/wav_source.rs`, with the fake clock (`crate::clock`, no wall-clock ceilings).
  - **Placement**: blocks are placed at `(stamp − call_start) × 16 kHz`, with downmix and resampling.
  - **Drift**: fake sources at +50 ppm and −50 ppm stay within ≤ 20 ms of each other at minute 60 (FR-011 allows 100 ms); `ratio_adj` is "Clamped to ±500 ppm".
  - **Re-anchor**: an offset above 40 ms re-anchors at once (ahead → zeros, behind → overlap dropped), and a gap of 200 ms or more is reported as a gap.
  - **Lost track**: it keeps advancing with zeros from the other track's stamps or the session tick.
  - **Callback channel**: a full bounded channel drops blocks, the time is filled with silence and the drop is reported.
  - **WAV source**: `WavTrackSource` reads 16-bit PCM and f32 RIFF, feeds blocks with synthetic `CaptureStamp`s at an accelerated pace, and supports a drift factor and scripted gaps.
  - **Levels**: RMS over 100 ms blocks, at most 10 Hz.
  - (spec FR-011, FR-012 (silence fill), FR-014, SC-007; R2, R3, R21, R22)
- [ ] T021 [US1] [area core] Implement `TrackSink`, `TrackSource`, `CaptureStamp`, `TrackDevice`, `Timeline`, the levels and `WavTrackSource` until T020 is green (contracts/core-call.md "Track ports"). Dictation's `FrameSink` / `AudioSource` stay unchanged (FR-009).

### G11 — `CallStore`: folders, `meta.json`, track writers, listing (area: core) <a id="g11"></a>

- [ ] T022 [P] [US1] [area core] Red tests in `crates/voicen-core/src/call/storage.rs`, with temp dirs, a fault-injecting writer and a fake `DiskSpace`.
  - **Folders**: a call folder `<root>\YYYY-MM-DD_HHMMSS` is created from the local start time, with `-1`, `-2` on a clash and never an overwrite.
  - **`meta.json`**: written from the start with `state: recording`, using the field order of data-model.md "Call". It is written "with temp + rename on every state change and every device change, never in place".
  - **Defaults**: the default title is "Call DD.MM HH:MM"; the speakers are `me` / `s1` with "Me" / "Other party" (EN) or "Я" / "Собеседник" (RU).
  - **Listing** covers the current root and every earlier root, newest first. An unavailable root gives one `unavailable` line. A missing or unparsable `meta.json`, an unknown schema or a missing track file lists the folder as `damaged`, never hidden. `ready-to-transcribe` is shown as Queued.
  - **Write probe**: the `.voicen-write-probe` check runs at each start.
  - (spec FR-015, FR-033 (listing), FR-039; req FR-47, FR-48, FR-71, §7)
- [ ] T023 [US1] [area core] Implement `CallStore` (roots, create, `meta.json`, track writers over `TrackEncoder`, listing) and the `DiskSpace` port until T022 is green.

### G12 — `CallSession` (area: core) <a id="g12"></a>

- [ ] T024 [US1] [area core] Red tests in `crates/voicen-core/src/call/session.rs`, with fake sources, a fake store and a fake clock.
  - **Off at start**: call mode is off at construction.
  - **One session**: tray, hotkey and switch all enter `toggle_record(on, Source)` under one lock, and a repeated on gives one session, one capture per track and one folder. Off then on gives a second folder.
  - **Refusals**: either track failing to open (`no_device`, `access_denied`, `busy`, `other`) or a missing or unwritable root refuses the start with `call_refused part=… reason=…`. Neither track is kept open and no folder is left. The system track opens first, then the mic, then the root check: with no output device and no capture device the refusal is `part=system reason=no_device` (FR-043). An unwritable root also asks to open the Calls tab.
  - **Mic fallback**: a missing selected mic with a default input present starts on the default input with `mic_changed`.
  - **FR-019**: Subtitles-only, with a fake subtitle consumer, writes nothing to disk.
  - **Stop**: `stop(StopReason)` closes both files readable, sets `ready-to-transcribe`, emits `call_saved_transcribing` and logs `call_stopped` with byte counts.
  - **Indicator state**: `IndicatorState` / `IndicatorTick` change inside the lock. The reminder and headphones hint are visible for 10 s at each start, and not at all when `reminder` is off.
  - **Provider notice**: `provider_notice{host}` is emitted once for the first session on a new non-loopback host (`warned_hosts`).
  - **Independence**: Esc and dictation never change the call (session independence).
  - **Wire**: `e2e/fixtures/call-indicator-wire.json` is pinned by a core test.
  - Depends on T009, T021, T023.
  - (spec FR-001, FR-002, FR-005, FR-009, FR-019, FR-030 (stop notice), FR-031; req FR-30, FR-31, FR-35, FR-37, FR-51)
- [ ] T025 [US1] [area core] Implement `CallSession` and the indicator wire types until T024 is green.

### G13 — `CpalTrackSource` and `DiskSpace` on Windows (area: shell) <a id="g13"></a>

- [ ] T026 [P] [US1] [area shell] Red tests in `src-tauri/tests/call_capture.rs`. On the device-less runner, opening `DefaultOutputLoopback` gives `OpenError::no_device` within the `open_bounded` 3 s budget. The WAV source wired through the shell's sink reaches the core session. `GetDiskFreeSpaceExW` returns a value for `%TEMP%`. **First acceptance** (R1, R2): stamps are monotonic and on the `Stream::now()` timeline for whatever endpoint the runner reports; record the observed facts in research.md R1/R2 (spec FR-002, FR-010, FR-014; R1, R2).
- [ ] T027 [US1] [area shell] Implement `src-tauri/src/win/track_capture.rs` (`CpalTrackSource`: a mic on the selected id or the default input, loopback on `default_output_device()`, QPC `CaptureStamp`, a call-track mapping in which `StreamInvalidated` / `DeviceNotAvailable` count as losses) and `src-tauri/src/win/disk.rs` until T026 is green. Dictation's `win/capture.rs` sink is unchanged.

### G14 — Session wiring, tray "Call" menu, `--call record` route (area: shell) <a id="g14"></a> — Needs T-007 (refusal and stop toasts); must not reach `main` before G42

- [ ] T028 [US1] [area shell] Red tests in `src-tauri/tests/call_launch.rs` and `call_tray.rs`.
  - A second instance launched with `--call record` posts exactly one `StartCallRecord` request, which enters `toggle_record(On, Source::Switch)` and logs `call_requested source=switch`. On the runner it is then refused with `call_refused part=system reason=no_device`, and the app keeps running.
  - A first launch with the switch applies it once after the tray exists; a launch without the switch never starts call mode.
  - The tray "Call" › "Record" / "Stop" and "Calls" items invoke the same path.
  - The call wiring lives in `src-tauri/src/call.rs`, outside `build_app`.
  - (spec FR-001, FR-002, FR-003, FR-043; R13)
- [ ] T029 [US1] [area shell] Implement `src-tauri/src/call.rs` (session ownership, refusal notices: log and indicator now, toasts through T-007's notifier) and the route in `src-tauri/src/tray.rs` until T028 is green. Uses T015, T017, T025, T027.

### G15 — Call-hotkey slot (area: shell) <a id="g15"></a> — Needs T-007 (conflict notice)

- [ ] T030 [P] [US1] [area shell] Red tests in `src-tauri/tests/call_hotkey.rs`.
  - A second registrar slot registers Ctrl+Alt+R with the two-step prepare / commit / abort; the first press starts with `last_combination` (Record only) and the next press stops.
  - A taken call hotkey at startup gives a notice only: no settings window, no tray hotkey-error state, the dictation hotkey still works, and the field is marked `callHotkey`.
  - Saving a new call hotkey re-registers it without a restart.
  - Dictation and call presses are independent.
  - (spec FR-004, FR-040; US1 #3, US9 #2, #3; req FR-33)
- [ ] T031 [US1] [area shell] Implement the slot in `src-tauri/src/win/hotkey.rs` and its wiring to `call.rs` until T030 is green.

### G16 — Call indicator window, OQ-45 spike (area: shell) <a id="g16"></a>

- [ ] T032 [US1] [area shell] **Spike, first acceptance (OQ-45, R12)**, as red tests in `src-tauri/tests/call_indicator.rs`.
  - The `call-indicator` window (capability `src-tauri/capabilities/call-indicator.json`, its own opener thread) is created hidden at the first Record, never at app start.
  - It is undecorated, topmost, has no taskbar entry and is focusable.
  - It is shown with `SW_SHOWNOACTIVATE`: the foreground window is unchanged after show.
  - After a stop-control click, the shell returns the foreground.
  - Window census: a second webview adds no unlisted helper window. If it does, record the evidence and decide here whether to add it to `scripts/ci/helper-windows.txt`; that addition makes the group **checker**.
  - **Owner**: the indicator stays above a full-screen app, and dragging works (`verify_exception`). If Tauri cannot keep the focus, stop and record the native-window fallback for the owner. OQ-45 closes only with this result.
- [ ] T033 [US1] [area shell] Implement the window and the commands and events of contracts/ipc.md (`call_indicator_stop`, `call_indicator_dismiss`, `call_indicator_moved`, `call_indicator_state`, `indicator://tick` ≤ 10 Hz, `indicator://state`). The position is saved in `calls.indicator_position` and clamped to a visible monitor. Until T032 and these are green (spec FR-005, FR-006; req FR-34).

### G17 — Indicator route (area: ui) <a id="g17"></a>

- [ ] T034 [P] [US1] [area ui] Red tests: `src/lib/callIndicator/indicatorApi.test.ts` and `e2e/call-indicator.spec.ts` against `e2e/fixtures/call-indicator-wire.json`.
  - It shows "● Recording" with a running timer and two level bars.
  - The reminder and the headphones hint show for 10 s, and are absent when `reminderVisible` is false.
  - Stop invokes `call_indicator_stop`.
  - Warnings are listed until dismissed (`call_indicator_dismiss`), and a dismissed kind re-arms on its next state.
  - There is a drag region; every control is a button with an accessible name; RU renders.
  - (spec FR-005, FR-006, FR-046; US6 #7)
- [ ] T035 [US1] [area ui] Implement `src/routes/call-indicator/+page.svelte` and `src/lib/callIndicator/indicatorApi.ts` until T034 is green.

### G18 — Install smoke through `--call record` (area: installer-ci) <a id="g18"></a> — checker only if `scripts/ci/` changes

- [ ] T036 [US1] [area installer-ci] Extend the install smoke in `.github/workflows/ci.yml` (step "Install silently, launch, check the start, settings-load and reconcile log lines, the first-run settings.json, the Run value and the settings window"). After the start line, run `voicen.exe --call record` and assert all of the following:
  - the log has `call_requested source=switch` and `call_refused part=system reason=no_device` (spec FR-043 names only the no-output-device refusal);
  - the process is still running;
  - no call folder exists under `%LOCALAPPDATA%\Voicen\calls`;
  - the window census is unchanged;
  - the log has none of the planted values.

  Run it red first on `wip/<ID>`, before G14 is merged. If the smoke predicate under `scripts/ci/` (`visible-windows.ps1`, `helper-windows.sh`) changes, mark the group checker. Depends on T029 for green (spec FR-043, FR-002, FR-042; contracts/installer-ci.md; P-006).

### G19 — Consent and storage docs (area: docs) <a id="g19"></a>

- [ ] T037 [P] [US1] [area docs] Add a recording-consent section to `README.md` in English and Russian: the user is responsible for the other party's consent, with a sample opening phrase. Also state that calls are stored unencrypted and are readable by the Windows user, and that the `--call record` trade-off (#184) exists (spec FR-021, FR-045; #165, #184).

**Checkpoint**: US1 is verifiable. The Linux session tests, the Windows WAV chain and the smoke refusal are green; the owner records a real call (quickstart owner check 1, without the transcript).

---

## Phase 4: User Story 2 — Get the final transcript after the call (P1)

**Goal**: after stop, ≤ 120 s chunks cut at Silero pauses go to the dictation API engine; the merged `transcript.json` and `transcript.md` are written and "Transcript ready — Open" appears.

**Independent test**: Linux against a wiremock server, with a 30-min WAV pair. Owner: SC-003 and SC-004.

### G20 — `SpeechSpans` port and energy spans (area: core) <a id="g20"></a>

- [ ] T038 [P] [US2] [area core] Red tests in `crates/voicen-core/src/vad/mod.rs` / `energy.rs`: the scripted span fake; energy spans on tone or silence buffers return `[start_ms, end_ms)` spans; silence returns none; a `VadError` is surfaced (spec FR-023; R8; #188).
- [ ] T039 [US2] [area core] Implement `vad::SpeechSpans` and the energy-span implementation until T038 is green.

### G21 — Silero implements `SpeechSpans` (area: core, `crates/voicen-whisper`) <a id="g21"></a> — Needs T-102

- [ ] T040 [US2] [area core] Red tests in `crates/voicen-whisper/src/silero.rs`, run in Windows CI with `VOICEN_SILERO_TEST_MODEL`: spans of a speech WAV fixture lie within ±200 ms of the labelled spans; 2 min of silence and of music without vocals give no span; the model loads once through T-102's loader, shared with dictation (spec FR-023, SC-008; R8; #175).
- [ ] T041 [US2] [area core] Return the segments `SileroDetector` already computes as `SpeechSpans` until T040 is green; `make check-whisper` stays green.

### G22 — `transcribe_timed` and `LangCode` (area: core) <a id="g22"></a>

- [ ] T042 [P] [US2] [area core] Red tests in `crates/voicen-core/src/engine/openai.rs` against wiremock.
  - A `Segments` request sends `response_format=verbose_json` and `timestamp_granularities[]=segment`, and an OK reply parses its segments.
  - A 400 or 422 to `Segments`, or a 2xx without `segments`, is reported as "time-less"; a 400 on `Plain` is `provider`.
  - 401 and 403 map to `key` and never fall back.
  - A connect error maps to `network`.
  - A late answer under the FR-24 timeout values maps to `timeout`.
  - 429, other 4xx/5xx and a malformed body map to `provider`.
  - The response cap stays 1 MiB.
  - Language: "english", "English" and ISO `en` map to `en`; an unknown value maps to `null`.
  - The request carries the dictation speech language.
  - The `engine::http` bans hold (no `Response::json()`, no `bearer_auth`).
  - (spec FR-022, FR-024, FR-025, FR-029; R10)
- [ ] T043 [US2] [area core] Implement `transcribe_timed`, `ResponseShape`, `Timed` and the language table until T042 is green.

### G23 — The chunker (area: core) <a id="g23"></a>

- [ ] T044 [P] [US2] [area core] Red tests in `crates/voicen-core/src/call/final_pass.rs` (chunk part), with scripted spans.
  - **Timed mode**: spans with gaps under 300 ms merge; a cut falls at the end of the last ≥ 300 ms gap before 120 s; a pause-less 5 min is cut hard at 120 s with the rest carried; chunks are cropped to first span − 200 ms and last span + 200 ms; a window with no span yields no chunk.
  - **Limits**: chunk `end − start` "≤ 120 000 ms in timed mode and ≤ 30 000 ms in time-less mode".
  - **Time-less mode**: every merged span is a chunk, split hard at 30 s.
  - **WAV**: the encoding is 16 kHz mono PCM16, ≤ 3.84 MB.
  - **1 h track**: no upload exceeds 25 MB, and the chunks stay in call order.
  - (spec FR-023, FR-024; US2 #2, #3, #7; R9)
- [ ] T045 [US2] [area core] Implement the chunker until T044 is green.

### G24 — Final-pass run (area: core) <a id="g24"></a>

- [ ] T046 [US2] [area core] Red tests in `crates/voicen-core/src/call/final_pass.rs`, against wiremock with a 30-min WAV pair encoded by the real codec.
  - **Decoding**: one window at a time (≤ 120 s plus carry); no temp file is created in the call folder or the temp dir.
  - **In flight**: at most 3 requests at once (#187), drawn from both tracks in start order.
  - **Fallback to time-less**: a 400 on chunk k makes chunk k and every later chunk time-less, and lines span their chunks; `pass_mode: timeless` is recorded.
  - **Language**: per-segment, per-request and missing language give `en`, the request value and `null` respectively.
  - **Silero unavailable**: energy spans are used and `vad_fallback` is logged (#188).
  - **Engine not set up**: a built-in dictation engine with no API key gives `failed: engine_not_set_up` before any request.
  - **Track failure**: the first failure of a track cancels that track's remaining chunks and the other track finishes; per-track results are kept in `meta.json` `results`.
  - **Write**: `transcript.json` is validated, then written with temp + rename, and so is `transcript.md`. A failed write leaves `failed: storage` and no partial file.
  - **Logs and notices**: `final_stage` lines per stage and track, and `final_finished outcome= reason= mode= ms=`. Notices are `transcript_ready{call}` or `pass_failed{reason}`.
  - **Engine settings**: a change to the API engine settings during a pass is not picked up by the running pass.
  - (spec FR-018, FR-022…FR-030, FR-032, SC-008; US2 #1, #4, #6, #8, #11; R8, R9, R10, R11)
- [ ] T047 [US2] [area core] Implement the pass runner (decode → spans → chunks → requests → merge → write) until T046 is green. Depends on T004, T019, T039, T043, T045.

### G25 — Final pass in the shell, WAV chain to a mock server (area: shell) <a id="g25"></a> — Needs T-102, Needs T-007

- [ ] T048 [US2] [area shell] Red tests in `src-tauri/tests/call_chain.rs`. A WAV-sourced call goes through the real session, then storage, then the final pass against a local mock OpenAI-compatible server, then a valid `transcript.json` with "Me" and "Other party" lines. The pass uses the dictation API engine's base URL, model and key slot from Credential Manager, planted only through `scripts/ci/credentials.ps1`. "Transcript ready" → Open emits `calls://open`; a failure gives the `pass_failed` notice (spec FR-014, FR-022, FR-030; req NFR-22).
- [ ] T049 [US2] [area shell] Wire the pass worker in `src-tauri/src/call.rs` (the bundled Silero from T-102, the energy fallback, toasts through T-007's notifier) until T048 is green.

### G26 — WER scoring and harness (area: core) <a id="g26"></a>

- [ ] T050 [P] [US2] [area core] Red tests in `crates/voicen-core/src/wer.rs`: normalization (lowercase, punctuation stripped, numbers as written, `ё` → `е`), word-level Levenshtein WER on fixtures, and empty-reference handling (spec FR-047; R23).
- [ ] T051 [US2] [area core] Implement `wer.rs` and `crates/voicen-core/examples/wer.rs` until T050 is green. The example reads WAV only, with no `claxon` (#186: the owner converts the FLAC files once with an external tool, and the command is documented in the example's usage). It takes a committed LibriSpeech test-clean manifest of utterance ids (≥ 30 min, archive MD5 `32fa31d27d2e1cad72775fee3f4849a9`, our own SHA-256 pinned at the first fetch) and the key from that one command's environment. Neither the gate nor CI runs it (#185).

**Checkpoint**: US2 is verifiable. The Linux pass is green against wiremock; the Windows chain gives a transcript.

---

## Phase 5: User Story 3 — Read, search, copy and export a call (P1)

**Goal**: the Calls window lists, opens, searches, copies (clipboard-excluded) and exports calls, and opens their folders.

**Independent test**: Playwright with mocked IPC; Linux, the export leaves nothing partial; Windows CI, the clipboard exclusion.

### G27 — Calls wire and export (area: core) <a id="g27"></a>

- [ ] T052 [P] [US3] [area core] Red tests in `crates/voicen-core/src/call/storage.rs` / `transcript.rs`.
  - **Mapping**: listing maps to the `CallList` / `CallSummary` / `CallDetail` of contracts/ipc.md, camelCase, `state` ∈ the seven wire values, and `failReason` camelCase (`engineNotSetUp`).
  - **Fixture**: `e2e/fixtures/calls-wire.json` is pinned.
  - **Path guard**: a `root` that is not current or remembered, or an `id` outside the call-id grammar, gives `notFound` with no filesystem access (P-009).
  - **Export**: Markdown or JSON is written with temp + rename into a target dir; a read-only target leaves no file and returns `ioFailed{reason}`.
  - (spec FR-033, FR-034, FR-036; US3 #6)
- [ ] T053 [US3] [area core] Implement until T052 is green.

### G28 — Calls window and read commands (area: shell) <a id="g28"></a> — Needs T-007 (Open from a toast)

- [ ] T054 [US3] [area shell] Red tests in `src-tauri/tests/call_ipc.rs`.
  - The `calls` window (capability `src-tauri/capabilities/calls.json`, its own opener thread) opens from the tray "Calls" and from `calls://open`.
  - `calls_list`, `calls_get`, `calls_copy_markdown`, `calls_export` (native save dialog; `saved: false` on cancel), `calls_open_folder` and `calls://changed` follow contracts/ipc.md.
  - The copied transcript carries the FR-10 exclusion formats through the dictation clipboard writer, with no second path (P-011).
  - (spec FR-033, FR-034, FR-036; R18)
- [ ] T055 [US3] [area shell] Implement the window and commands in `src-tauri/src/call.rs` until T054 is green.

### G29 — Calls route: list, detail, search, copy, export (area: ui) <a id="g29"></a>

- [ ] T056 [P] [US3] [area ui] Red tests: `src/lib/calls/callsApi.test.ts` and `e2e/calls.spec.ts` against `e2e/fixtures/calls-wire.json`.
  - 3 calls show newest first, with title, date and time, duration and state.
  - Every state badge appears, including Queued, failed with each reason, "Recovered after a crash" and damaged (Open folder and Delete only).
  - An empty list shows "No calls yet" with the hotkey; unavailable-root lines show.
  - The detail shows time, speaker colour and label, and text.
  - A search for "contract" highlights matches, and an unmatched search shows "No matches".
  - Copy invokes its command, and an export failure shows the reason.
  - RU renders.
  - (spec FR-033, FR-034, FR-036, FR-046)
- [ ] T057 [US3] [area ui] Implement `src/routes/calls/+page.svelte` and `src/lib/calls/` until T056 is green.

**Checkpoint**: the 1a done-when (SC-001) can be observed: the owner records and reads a call.

---

## Phase 6: User Story 4 — Never lose a recording (P1)

### G30 — Recovery, limits, write failures, temp-cleanup exclusion (area: core) <a id="g30"></a>

- [ ] T058 [P] [US4] [area core] Red tests in `crates/voicen-core/src/call/storage.rs` / `session.rs`.
  - **Crash recovery**: at start, a `recording` folder with no owning session becomes `recovered` (offered, never started by itself), with end and duration from the shorter readable track, and `.opus` bytes unchanged (hash compare). A crash at minute 20 gives ≥ 19 min 50 s of audio (SC-005).
  - **Interrupted pass**: `transcribing` becomes `ready-to-transcribe` and is queued; `ready-to-transcribe` is queued oldest first.
  - **Write failure** mid-recording: the recording stops, the flushed audio is kept, `meta.json` stays `recording`, `call_storage_failed` is logged and `stopped{storage}` is noticed.
  - **Disk**: free space is polled every 10 s; below 1 GB a `low_disk` warning fires once, and below 200 MB the recording stops with saving.
  - **4 h**: the recording stops with `max_length`.
  - **Temp cleanup**: dictation `TempAudioStore` cleanup never deletes under any root (FR-020).
  - **Faults**: injected faults never panic and never change the audio (SC-006).
  - (spec FR-016, FR-017, FR-018, FR-020, FR-032; US4 #1, #5–#8, #10)
- [ ] T059 [US4] [area core] Implement until T058 is green.

### G31 — Exit, shutdown, installer close, sleep, forced kill (area: shell) <a id="g31"></a> — Needs T-007 (wake notice)

- [ ] T060 [US4] [area shell] Red tests in `src-tauri/tests/call_lifecycle.rs`.
  - Tray Exit while recording asks Yes / No (`MessageBoxW` on a worker thread, catalog text): Yes stops and exits, No keeps recording.
  - `RunEvent::Exit` (`WM_ENDSESSION`, installer close recognised by `logs/installer-ended`) finalizes without a question within 2 s, before the log's exit line, and leaves `meta.json` at `ready-to-transcribe`.
  - `PBT_APMSUSPEND` posts `stop(Sleep)`; resume posts the notice; a WTS lock changes nothing.
  - A forced kill during a WAV-backed recording gives `recovered` at the next start.
  - **First acceptance** (R14): the time available after `WM_ENDSESSION`, and the stock NSIS close reaching `RunEvent::Exit`; record both in research.md R14.
  - (spec FR-007, FR-008, FR-017)
- [ ] T061 [US4] [area shell] Implement in `src-tauri/src/call.rs`, the exit handler and the `src-tauri/src/win/hotkey.rs` power branch until T060 is green. **Owner**: sign-out and lid-close checks (quickstart 5, 6).

---

## Phase 7: User Story 5 — Recover from a failed final pass (P2)

### G32 — Pass queue lifecycle and "Transcribe again" (area: core) <a id="g32"></a>

- [ ] T062 [P] [US5] [area core] Red tests in `crates/voicen-core/src/call/final_pass.rs` (queue).
  - Passes run one at a time, FIFO; B recorded and stopped during A's pass is Queued and starts after A.
  - Recording is allowed during a pass.
  - "Transcribe again" on a `failed` call redoes only the failed track; on a `ready` call it clears and redoes both; speaker names are kept.
  - A 401 gives `failed: key`; after the key is fixed, a retry succeeds.
  - Deleting a transcribing call cancels the pass first (cancel token).
  - Exit or crash during a pass restarts it at the next start.
  - The `transcribe` request on a `recording` call is `recordingCall`.
  - (spec FR-027, FR-028, FR-037 (cancel); US5 #1–#7)
- [ ] T063 [US5] [area core] Implement until T062 is green.

### G33 — `calls_transcribe` and restart at start (area: shell) <a id="g33"></a>

- [ ] T064 [US5] [area shell] Red tests in `src-tauri/tests/call_ipc.rs`: `calls_transcribe` returns `queued`; a pass interrupted by an exit restarts after relaunch against the mock server (spec FR-017, FR-027, FR-028).
- [ ] T065 [US5] [area shell] Implement until T064 is green.

### G34 — Transcribe / Transcribe again in the UI (area: ui) <a id="g34"></a>

- [ ] T066 [P] [US5] [area ui] Red tests in `e2e/calls.spec.ts`: "Transcribe" on recovered calls, "Transcribe again" on failed and ready calls, the row becomes Queued, and a `recordingCall` error is shown (spec FR-027, FR-028).
- [ ] T067 [US5] [area ui] Implement until T066 is green.

---

## Phase 8: User Story 6 — Keep recording through device changes (P2)

### G35 — Device events, activity warnings, warning list (area: core) <a id="g35"></a>

- [ ] T068 [P] [US6] [area core] Red tests in `crates/voicen-core/src/call/session.rs` / `capture.rs`, with a fake `DeviceEvents` stream and the fake clock.
  - **System track**: a default-output change reopens on the new default; a ≤ 2 s gap is filled with silence and appended to `device_changes` and `call_device … gap_ms=`.
  - **Mic track**: mic loss reopens on the default input with `mic_changed`; the call is not stopped.
  - **Track lost**: not reopened within 2 s, or no device left, means silence from that point, a `track_lost` warning, `call_track_silent rule=lost`, and a retry every 2 s.
  - **Communications device**: `CommunicationsCheck::differs` at start gives `communications_device_differs`, and `E_NOTFOUND` counts as no difference.
  - **FR-013**: > 60 s of system silence while the mic has speech gives `cant_hear_other_party`, and the mirror case gives `cant_hear_you`; each fires once per stretch and re-arms after speech.
  - **Warning list**: warnings stay until dismissed, and a dismissed kind re-arms on its next trigger.
  - (spec FR-006, FR-010, FR-012, FR-013; US6 #1–#7; R4, R21)
- [ ] T069 [US6] [area core] Implement until T068 is green.

### G36 — IMMNotificationClient and communications check (area: shell) <a id="g36"></a>

- [ ] T070 [US6] [area shell] Add the `windows` 0.62 features `Win32_Media_Audio` and `Win32_System_Com` (#186; `Win32_System_Com_StructuredStorage` only if a property store is read) to `src-tauri/Cargo.toml`, with `make licenses` if the notices change. Red tests in `src-tauri/tests/call_notify.rs`: register and unregister on the call-session capture thread with no callback on the runner, and the communications check returns a value or "none" without error (spec FR-010, FR-012; R4).
- [ ] T071 [US6] [area shell] Implement `src-tauri/src/win/audio_notify.rs` (callbacks only post typed events; nothing blocks or (un)registers inside a callback) until T070 is green. **Owner**: the per-role callback order, device switches and Bluetooth (quickstart 3; `verify_exception`).

---

## Phase 9: User Story 7 — Dictate during a call (P2)

### G37 — Dictation alongside a call (area: shell) <a id="g37"></a>

- [ ] T072 [US7] [area shell] Red tests in `src-tauri/tests/call_dictation.rs`, with WAV sources.
  - A dictation during a call opens its own stream, pastes, and leaves the call track continuous.
  - The tray shows dictation recording, then call recording again.
  - Esc without a dictation reaches the focused app and the call continues; Esc during a dictation discards only the dictation.
  - With call mode off and no dictation, no capture stream is open.
  - (spec FR-009; US7 #1, #3–#5; req FR-22, NFR-02)
- [ ] T073 [US7] [area shell] Fix whatever T072 shows until it is green. **Owner**: two shared-mode streams on one endpoint, and the NFR-02 start figure from the log during a call (R24).

---

## Phase 10: User Story 8 — Rename, delete, damaged folders (P2)

### G38 — Rename and delete in core (area: core) <a id="g38"></a>

- [ ] T074 [P] [US8] [area core] Red tests in `crates/voicen-core/src/call/storage.rs` / `transcript.rs`.
  - **Renames**: a rename of `s1` to "Colleague" re-renders `transcript.md` and `transcript.json`. "Empty or whitespace-only is refused" for titles and speaker names; duplicates are allowed; the title reaches `transcript.json` `title`.
  - **Delete**: a ready call's folder is removed whole. Deleting the recording call is `recordingCall`. A transcribing call is cancelled first. A locked file (fake fs) gives `inUse{file: "mic.opus"}` and the folder is unchanged.
  - **Damaged**: a damaged folder can be deleted.
  - (spec FR-035, FR-037; US8 #1–#6)
- [ ] T075 [US8] [area core] Implement until T074 is green.

### G39 — Rename and delete commands (area: shell) <a id="g39"></a>

- [ ] T076 [US8] [area shell] Red tests in `src-tauri/tests/call_ipc.rs`: `calls_rename`, `calls_rename_speaker` and `calls_delete`. A real Windows share lock on `mic.opus` gives `inUse` and nothing is deleted (spec FR-035, FR-037).
- [ ] T077 [US8] [area shell] Implement until T076 is green.

### G40 — Rename, delete and damaged flows (area: ui) <a id="g40"></a>

- [ ] T078 [P] [US8] [area ui] Red tests in `e2e/calls.spec.ts`: inline title and speaker rename; the `emptyName` refusal keeps the old name; delete asks for confirmation; Delete is unavailable on the recording call; the in-use error text appears; the damaged row offers Open folder and Delete (spec FR-033, FR-035, FR-037).
- [ ] T079 [US8] [area ui] Implement until T078 is green.

---

## Phase 11: User Story 9 — Calls settings tab (P2)

### G41 — Calls tab (area: ui) <a id="g41"></a>

- [ ] T080 [P] [US9] [area ui] Red tests: `src/lib/settings/` unit tests and `e2e/settings-calls.spec.ts` with the extended settings-wire fixture.
  - The tab offers the call hotkey, the reminder, the primary language and the storage folder.
  - The hotkey-equal refusal shows from either side on `callHotkey`; a conflict mark shows from a save error.
  - An unwritable root shows "This folder can't be written: {reason}" with the field highlighted.
  - The provider line "The other party's audio goes to provider {host}" shows for a non-loopback host and is absent for `http://localhost:8000/v1`.
  - A saved value persists across a reload; RU renders.
  - (spec FR-031, FR-038, FR-039, FR-040, FR-046; US9 #1–#6)
- [ ] T081 [US9] [area ui] Implement `src/lib/settings/tabs/Calls.svelte` and add it to `TABS` until T080 is green. Depends on T013; re-registration on save comes from G15.

---

## Phase 12: User Story 10 — Uninstall never silently deletes calls (P2)

### G42 — Uninstaller calls question, `/PURGECALLS`, CI proof (area: installer-ci) <a id="g42"></a> — checker (`scripts/ci/nsis-fork.sh` if extended)

- [ ] T082 [US10] [area installer-ci] Red CI steps in `.github/workflows/ci.yml`, run on `wip/<ID>`:
  - in the existing step "uninstall.exe /S removes the data folder, every Voicen/ credential, the Run value and the shortcut, and keeps other credentials (T-025)", plant `calls\2026-01-01_000000\meta.json` before the uninstall and assert that it exists after it; the step's "data folder gone" assertion becomes "gone except `calls\`";
  - add a step: reinstall, then `/S /PURGECALLS`; the planted default folder is gone and `$RUNNER_TEMP\calls-elsewhere` still exists;
  - **first acceptance** (R15): prove that the forked template never runs `RMDir /r "$INSTDIR"`, by extending `scripts/ci/nsis-fork.sh` and its test if needed (this is what makes the group checker).

  (spec FR-041, SC-010; contracts/installer-ci.md "Uninstaller")
- [ ] T083 [US10] [area installer-ci] Implement in `src-tauri/windows/installer-hooks.nsh` until T082 is green:
  - `$VoicenPurgeCalls` at the one PREUNINSTALL decision point (update → keep; `/PURGECALLS` → remove; `/S` or `/P` → keep; otherwise a MessageBox with default No, asked only if the folder exists);
  - LangStrings 1033/1049;
  - a POSTUNINSTALL that never runs `RMDir /r` on `%LOCALAPPDATA%\Voicen` while calls are kept;
  - no root other than the default is touched.

  **Owner**: the interactive question in EN and RU (quickstart 9).

---

## Phase 13: Polish and cross-cutting

- [ ] T084 [area core] **G43** <a id="g43"></a> Privacy and fault-injection sweep in `crates/voicen-core/tests/call_privacy.rs` (or the crate's call test module). Run a whole WAV-sourced call with a planted transcript `"secret words 123"`, a planted title, a planted speaker name and a planted device friendly name, a device change and a failed pass. Assert 0 occurrences in the temp log folder and crash files. Also assert 100 % app survival and unchanged audio across the fault matrix (spec SC-006, SC-009, FR-032, FR-042). Depends on G24, G30, G35, G38.
- [ ] T085 [area docs] **G44** <a id="g44"></a> Final docs sweep; each group already updates its own docs in its commit (P-014):
  - `docs/architecture.md` (the call module, ports, windows, data flow);
  - `docs/decisions/<area>.md` invariants: call storage, `meta.json` temp + rename, recovery never rewrites audio, the libopus CRT, the indicator as a product window;
  - `CLAUDE.md` "Where things live" rows for `call/` and `opus-cmake-cache.sh`;
  - OQ-48 recorded for owner confirmation.
- [x] T086 [area docs] **G45** <a id="g45"></a> Done, not a pending task: constitution principle II was amended to v1.5.0 by decisions #189 in commit cdb98e4 (calls are kept until deleted, dictation audio keeps its limit, v3 references updated). No teamwright task is derived from this group.
- [ ] T087 [area docs] **G46** <a id="g46"></a> **Owner** manual checks from quickstart.md 1–10, recorded as the verify evidence of the `verify_exception` items in G16, G31, G36, G37 and G42:
  - SC-001;
  - SC-003 via `final_finished ms=` with `whisper-1` and Groq;
  - SC-004 via the `wer` example (EN ≤ 10 %, RU ≤ 15 %);
  - SC-005;
  - SC-011, the NFR-19 short set;
  - the indicator above a full-screen app (closes OQ-45).

---

## Dependencies and execution order

- **Outside this feature**:
  - **T-102** blocks G21 and G25.
  - **T-007** blocks the toast parts of G14, G15, G25, G28 and G31. Their log and indicator parts may land first, but the teamwright task stays open until T-007 is DONE.
  - OQ-66 blocks G03 only on its failure branch. OQ-45 is closed by G16 / T087.
- **Phase 1**: G01 → G02 → G03 (G03's T005/T006 can start alongside G02).
- **Phase 2** (after G02 for `call/mod.rs`): G04, G05, G06, G07, G08, G09 are independent of each other.
- **US1**: G10 and G11 (after G02, G04) → G12 (also needs G06) → G13 ∥ G15 ∥ G16 → G14 (needs G07, G08, G12, G13) → G18. G17 needs G12's fixture. G19 is independent.
- **Ordering rule (R15, plan Risks)**: G42 reaches `main` before G14, because today's POSTUNINSTALL would delete calls recorded with an installed build. G42 depends on nothing in this feature, so it can run from day one.
- **US2**: G20 → G21 (T-102); G22 and G23 are independent; G24 needs G02, G09, G20, G22, G23 → G25 (needs G14, G21, T-102, T-007). G26 is independent.
- **US3**: G27 (after G11, G09) → G28 (after G14) and G29 (fixture from G27).
- **US4**: G30 (after G11, G12, G24) → G31 (after G14).
- **US5**: G32 (after G24, G30) → G33 → G34 (after G29).
- **US6**: G35 (after G12) → G36 (after G13).
- **US7**: G37 after G14 (and G08).
- **US8**: G38 (after G11, G32 for the cancel) → G39 → G40 (after G29).
- **US9**: G41 after G06; it also needs G15 for live re-registration.
- **US10**: G42 is independent (see the ordering rule).
- **Polish**: G43 after G24, G30, G35, G38. G44 at the end. G45 is done (#189, cdb98e4). G46 after G25, G31, G36, G37, G42.

## Parallel opportunities

- **Start at once**: G01, G42, G19, G26 and the checker half of G03 (T005, T006).
- **After G02**: the red-test tasks of G04, G05, G06, G07, G08 and G09 (T008, T010, T012, T014, T016, T018) are all different files.
- **US1**: T020 ∥ T022; after G12, T026 ∥ T030 ∥ T032 ∥ T034.
- **US2**: T038 ∥ T042 ∥ T044 ∥ T050. G21 can start once T-102 lands, independent of G22–G24.
- **UI**: G17, G29, G34, G40 and G41 depend only on their wire fixtures, so they run alongside the shell groups of the same story.
- **Windows-only groups** (G13–G16, G25, G28, G31, G33, G36, G37, G39, G42): one heavy CI run per agent at a time (host load limits).

## Implementation strategy

1. **MVP (the 1a done-when, SC-001)**: Phase 1, Phase 2, US1, US2 and US3, with G42 merged before G14.
2. **Data safety next**: US4. It is P1, but its tests need the US1 and US2 seams.
3. **Then the P2 stories**: US5, US6, US7, US8, US9.
4. **Close out**: Polish and the owner checks (G46); the constitution amendment (G45) is already done (#189).
