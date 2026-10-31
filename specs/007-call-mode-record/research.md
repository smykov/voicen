# Research: Call Mode 1a — Record and Read

Phase 0 of [plan.md](plan.md). The stack is fixed by `docs/requirements.md` v7 §8 and §9 (release-2 rows, decisions #178, #182): the existing three-area layout, cpal 0.18 loopback, IMMNotificationClient from `windows` 0.62, Ogg Opus during recording, a final pass of ≤ 120 s chunks cut at a VAD pause and decoded in memory, `--call record` handed to the running instance (#184), and Playwright for the call UI. This file decides only HOW within that stack.

Crate facts were read from the published sources of `opus` 0.4.0 / `opusic-sys` 0.7.5, `ogg` 0.9.2, `cpal` 0.18.2 and `windows` 0.62.2. API facts come from the providers' published API definitions. Items marked **confirm on Windows** depend on WASAPI, Tauri or NSIS behaviour that the Linux host cannot observe. The first task that touches each one proves or corrects it on the Windows runner (P-016 `wip/` run) before any code builds on it. Items marked **confirm at spike** belong to the OQ-45 indicator spike (ADR-007).

## R1 — System track: cpal loopback on the default output device

- **Decision**: The `system` track is a cpal input stream built on `host.default_output_device()`. cpal adds `AUDCLNT_STREAMFLAGS_LOOPBACK` when an input stream is built on a render device. The stream opens in the device's mix format (cpal sets no AUTOCONVERTPCM for input streams; typically 48 kHz f32 stereo). The core downmixes to mono and resamples to 16 kHz with the existing `rubato` path. The stream is built from the *default* handle, not from a resolved id, so cpal's own default-device monitor applies: a change of the default console render device reaches the error callback as `StreamInvalidated` (reopen on the new default) or `DeviceNotAvailable` (no output device left). The shell reuses `win/capture.rs`'s bounded open (`open_bounded`, 3 s budget) for both tracks.
- **Rationale**: This is the documented cpal loopback path, and it needs no new crate (FR-010, req FR-39). Building on the default handle makes default-output following (FR-012) a cpal signal rather than our own bookkeeping.
- **Confirm on Windows**: event-driven loopback may deliver no buffers while nothing plays. Silence can therefore arrive as *missing callbacks*, not zero samples. R3's timeline fills silence from capture timestamps, so both cases give the same track.
- **Alternatives**: raw WASAPI `IAudioClient` in the shell (more unsafe code, the same result); the `wasapi` crate (a new dependency, a second audio stack next to cpal).

## R2 — One clock: QPC capture timestamps

- **Decision**: Call tracks take their time from cpal's `InputCallbackInfo::timestamp().capture`. On WASAPI this is the `qpc_position` of `IAudioCaptureClient::GetBuffer`, the capture instant of the buffer's first frame. Mic and loopback streams report it on the same QPC clock, so the two tracks are directly comparable. The shell converts it to a core value `CaptureStamp` (nanoseconds on the QPC timeline) and passes it with each block through a new call-track sink port (contracts/core-call.md "Track ports"). Dictation's `FrameSink` (with `Instant` taken at callback entry) is not changed. The call start is the QPC time at which both tracks are open (`Stream::now()`).
- **Rationale**: FR-011 needs one clock across two devices. The callback-entry `Instant` carries the callback's scheduling jitter (several ms), while QPC carries the device position. Keeping dictation's sink unchanged leaves the NFR-02 path untouched (FR-009).
- **Confirm on Windows**: cpal's changelog says timestamps stay monotonic across a stream rebuild. The first capture task asserts on the runner (with the WAV source and with whatever real endpoint the runner reports) that stamps are monotonic and on the `Stream::now()` timeline.
- **Alternatives**: `Instant` at callback entry (jitter); sample counting only (no answer to device changes or drift).

## R3 — Drift compensation and silence fill: a per-track timeline in core

- **Decision**: `call::capture::Timeline` places every block at `(stamp − call_start) × 16 kHz` on the track's own 16 kHz sample axis:
  - **Small offsets**: the difference between expected and written position (from clock drift and resampler latency) is absorbed by a slow control loop. It adjusts the rubato resampler's relative ratio within ±500 ppm, with a time constant of tens of seconds.
  - **Large offsets**: any offset above 40 ms (a gap after a device change or missing loopback callbacks) re-anchors at once. Ahead → insert zeros; behind → drop the overlapping samples. Each re-anchor that is a gap of 200 ms or more is recorded as a gap in `meta.json` and the log (FR-012).
  - **Lost or silent track**: it keeps advancing with zeros, driven by the other track's stamps or by the session's own clock tick.
  - **Testing**: fake sources run at +50 ppm and −50 ppm against the fake clock, and the test asserts misalignment ≤ 20 ms at minute 60 (FR-011 allows 100 ms).
- **Rationale**: ADR-002 (adaptive resampling, silence on device change) applied to stamps from R2. Everything except the stamp source is pure core and Linux-testable with `crate::clock` (core-tests.md: no wall-clock ceilings).
- **Alternatives**: post-hoc alignment at stop (fails on a crash, FR-017); dropping and duplicating samples only (audible clicks on the mic track).

## R4 — Device changes: cpal error kinds plus one IMMNotificationClient

- **Decision**:
  - **System track**: cpal's `StreamInvalidated` on the default-output stream means "reopen on the new default". The reopen must finish within 2 s (FR-012); otherwise the track continues as silence and retries every 2 s.
  - **Mic track**: the mic stream is built on the selected device's id, which has no cpal monitor. The shell adds one `IMMNotificationClient` (`#[windows::core::implement]`; features `Win32_Media_Audio`, `Win32_System_Com`, `Win32_Foundation`; `implement` is not a feature in 0.62). It forwards `OnDeviceStateChanged` / `OnDeviceRemoved` for the mic's endpoint id, and `OnDefaultDeviceChanged(eCapture, eConsole)` when the selected microphone is "default".
  - **Callback discipline**: the callbacks only post a typed event to the call session's channel. Nothing blocks there, and nothing (un)registers from inside a callback.
  - **Who decides**: the core decides what to do (reopen on the Windows default input with a notice, or continue as silence).
  - **Lifetime**: registration lives on the call-session capture thread, from Record on to Record off.
  - **Communications check**: at start, `GetDefaultAudioEndpoint(eRender, eCommunications)` is compared by `GetId()` with the console default. `E_NOTFOUND` counts as "no communications device", not a difference.
- **Rationale**: This covers FR-010, FR-012 and r3#16 with one notifier and no polling. cpal 0.18 no longer reports `DeviceChanged` on WASAPI, so dictation's `device_loss_callback` (which ignores `DeviceChanged`, `Xrun` and `RealtimeDenied`) is not reused for call tracks. Call tracks get their own mapping that treats `StreamInvalidated` and `DeviceNotAvailable` as losses.
- **Confirm on Windows**: the order and duplication of the per-role `OnDefaultDeviceChanged` callbacks during a switch; whether losing a specific capture device surfaces as `DeviceNotAvailable` or `StreamInvalidated`. No real device change can be made on the device-less runner (windows-ci-runner.md). The decision logic is proved in core with a fake event stream, and the shell adapter is limited to registration plus a COM smoke on the runner (register, unregister, no callback).
- **Alternatives**: polling `default_output_device().id()` every 500 ms (gap up to the poll interval, a busy loop); a second notifier per track (two registrations doing the same job).

## R5 — Opus encoding: `opus` 0.4 on bundled libopus, CRT pinned

- **Decision**:
  - **Crate and settings**: `opus` 0.4.0 (MIT/Apache-2.0) over `opusic-sys` 0.7.5 (BSD-3-Clause). Its default `bundled` feature builds the vendored libopus 1.6.1 with CMake and links it statically. Encoder: 16 kHz, mono, `Application::Voip`, 20 ms frames (320 samples), VBR at 24 kbit/s, complexity 9, DTX off, FEC off.
  - **Size budget**: 24 kbit/s is about 10.8 MB per track-hour; with Ogg overhead under 2 %, both tracks come to about 22 MB/h against CA-9's 40 MB/h.
  - **Where it lives**: the codec lives in `voicen-core` behind `TrackEncoder` / `TrackDecoder` traits (contracts/core-call.md), so session tests use a fake codec and the codec tests run on real libopus on the Linux host (§9 verification row).
  - **C runtime (confirm on Windows; first acceptance item of the codec task)**: opus's `CMakeLists.txt` picks `/MD` unless `OPUS_STATIC_RUNTIME=ON`, and opusic-sys never sets that option. The root `.cargo/config.toml` forces MSVC values of `CMAKE_*` for whisper.cpp; cmake-rs does not forward those to opusic-sys. Planned fix: a CMake toolchain file, given through the `CMAKE_TOOLCHAIN_FILE` environment variable in `.cargo/config.toml`, that sets `OPUS_STATIC_RUNTIME ON` and CMP0091 NEW. Gates:
    - `scripts/ci/crt-imports.sh` gains `voicen.exe` (already listed) and the call codec test exes;
    - a new `opus-cmake-cache` check reads opus's `CMakeCache.txt` (the T-099 pattern);
    - both are guard changes, so the task carries `checker: true` (P-020).
  - **Linux image**: the core image already has `cmake` (added by T-115; the Dockerfile comment and `docs/decisions/ci-toolchain.md` say why). The mingw cross build of `make check-shell-windows` also builds libopus with mingw gcc, so the same `cmake` serves it.
- **Rationale**: This is the confirmed stack (req §8: `opus` on libopus BSD-3). Voip at 16 kHz/24 kbit/s is libopus's speech sweet spot and leaves room under NFR-18.
- **Alternatives**:
  - `ropus` 0.12 (pure Rust, BSD-3-Clause, no C toolchain, bit-exact claim): fallback if the CRT fix fails, as an owner decision (it is young, created 2026-04);
  - `audiopus` 0.2 (unmaintained since 2021, Opus 1.3);
  - a prebuilt `/MT` `opus.lib` through `OPUS_LIB_DIR` (a binary in the repo or the cache).

## R6 — Ogg container: `ogg` 0.9, a page every second, flush and sync every 5 s

- **Decision**:
  - **Writer**: `ogg` 0.9.2 (BSD-3-Clause) `PacketWriter` with one serial per file.
  - **Headers (RFC 7845)**:
    - page 1 (BOS) holds only `OpusHead`: version 1, 1 channel, pre-skip = encoder lookahead × 3 (48 kHz units), input rate 16000, gain 0, mapping family 0;
    - the next page holds `OpusTags` (vendor `voicen <version>`, no user comments), ended with `EndPage`.
  - **Audio pages**: audio starts on a fresh page, with the granule counted in 48 kHz units (960 per 20 ms packet). Every 50th packet (1 s) is written with `EndPage`, followed by `inner_mut().flush()`. `File::sync_data` runs every 5 s.
  - **Stop**: the last packet is written with `EndStream` (the EOS flag), the final granule trims the tail, then the file is synced and closed.
  - **Failures**: any `io::Error` from write, flush or sync is a storage failure (FR-016).
- **Rationale**:
  - FR-015 asks for a flush at least every 10 s; FR-017 and SC-005 lose at most the last unflushed second plus the unsynced window (≤ 5 s ≪ 10 s).
  - The `ogg` writer never flushes its inner writer, and a page closes by itself only after 255 lacing segments (about 5 s). The forced `EndPage` makes the interval explicit and testable.
  - Page overhead at 1 s pages is about 80 B/s, under 1 % of 24 kbit/s.
- **Alternatives**: one page per packet (about 30 % overhead at this bitrate); page closure by size only (no time bound).

## R7 — Reading a crashed or truncated recording; recovery does not rewrite

- **Decision**:
  - **Reader**: the core `OpusOggReader` walks pages with `ogg::PacketReader`.
    - It treats `Ok(None)` and `Err(ReadError(UnexpectedEof))` after at least one audio page as "end of recoverable data".
    - It treats `HashMismatch` on the last page as the same.
    - `HashMismatch` before the last page is a damaged file. Its readable prefix is still used.
    - A file with no valid `OpusHead` page is unreadable.
  - **Duration**: last good granule − pre-skip.
  - **Recovery** (FR-017) only reads and updates `meta.json`. It sets state `recovered`, end and duration from the shorter readable track, and a `recovered: true` mark. It never truncates, appends or rewrites an `.opus` file: the recorded bytes stay as they were (NFR-15, FR-032), and the final pass and the reader both stop at the last good page.
- **Rationale**: SC-005 (N min − 10 s) and SC-006 ("audio unchanged") hold without a repair step that could itself fail. External players also tolerate a missing EOS.
- **Alternatives**: rewriting a clean EOS page at recovery (a write to user audio during start; a failure mode of its own).

## R8 — Segment-level VAD: a `SpeechSpans` port next to `SpeechDetector`

- **Decision**:
  - **Port**: core adds `vad::SpeechSpans` with one method returning the speech spans `[start_ms, end_ms)` of an `AudioBuffer`, or a `VadError`.
  - **Silero**: `voicen-whisper`'s `SileroDetector` already computes the segments inside `contains_speech` and discards them, so it implements the port by returning them. One model load is shared with dictation through T-102's loader.
  - **Fakes**: core tests use a scripted span fake.
  - **Energy fallback (decided, #188)**: `EnergyDetector` gets an energy-span implementation, used only if Silero cannot load or returns a `VadError`. The pass goes on with energy spans and logs `vad_fallback track=<mic|system> cause=<unavailable|error>` (contracts/core-call.md "Log lines"). Accepted trade-off: music or noise may reach the API in that case, so SC-008 holds only with Silero; Silero ships in the installer (T-102), so the fallback is rare.
  - **Process**: the pass runs Silero per decoded window (≤ 120 s plus carry), never over a whole track at once (FR-018).
- **Rationale**: FR-023 and SC-008 need span boundaries for the cut, not a yes/no. Extending the existing seam keeps one VAD stack (P-011).
- **Alternatives**: a separate VAD crate (a second model and stack); cutting at energy minima (music without vocals would be sent, against SC-008).

## R9 — The chunker (pure core)

- **Decision**: `call::final_pass::chunk` takes a stream of decoded 16 kHz windows plus their spans and yields `Chunk { track, start_ms, end_ms, mode }` in two modes:
  - **Timed mode**:
    - Merge spans whose gap is under 300 ms.
    - In the 120 s window from the current position, cut at the end of the last gap ≥ 300 ms before 120 s; if there is none, cut hard at 120 s and carry the rest (FR-023).
    - Crop each chunk to [first span start − 200 ms, last span end + 200 ms].
    - A window with no span yields no chunk: nothing is sent and no line is made (SC-008).
  - **Time-less mode** (FR-024): every merged span is a chunk, split hard at 30 s.
  - **Upload format**: each chunk is encoded as a 16 kHz mono 16-bit WAV (≤ 3.84 MB) by the existing multipart path.
- **Rationale**: FR-023, FR-024, FR-018 (one window in memory). Cropping keeps leading and trailing music out of the request. The 300 ms pause and 200 ms pad are constants, tuned against the WER sets (FR-047), not settings.
- **Tests (Linux, §9 row)**: ≤ 120 s at pauses; a pause-less 5-min tone with scripted speech gives hard 120 s cuts; a silent window is never sent; time-less ≤ 30 s phrases.

## R10 — Segment-returning transcription: one more call on the existing OpenAI client

- **Decision**:
  - **New method**: the OpenAI-compatible client in `engine` gains `transcribe_timed(&AudioBuffer, &TranscribeRequest, ResponseShape)`, returning `Timed { text, language: Option<LangCode>, segments: Option<Vec<{start_s, end_s, text}>> }`. `ResponseShape` is `Segments` (`response_format=verbose_json`, `timestamp_granularities[]=segment`) or `Plain` (`json`).
  - **Existing parts reused**: it goes through `engine::http`, `multipart_body`, `send_capped` and `failure::classify`, under the FR-24 connect and API timeouts per chunk (FR-029).
    - The response cap stays 1 MiB; a 120 s `verbose_json` reply is tens of KB.
    - Parsing is serde on our own typed structs; `Response::json()` and `bearer_auth` are banned (engine-http.md).
  - **Fallback**: an HTTP 400 or 422 to a `Segments` request, or a 2xx without `segments`, makes the pass's engine time-less. The same chunk is sent once more with `Plain` and re-cut into phrases (FR-024). A 400 or 422 on `Plain` is `provider_error`. 401 and 403 map to the key reason, as today; they never trigger the fallback.
  - **Language**:
    - `verbose_json.language` is a full name ("english"; Groq's spelling and case are undocumented). A core table maps whisper's language names (case-insensitive) and ISO 639-1 codes to `LangCode`.
    - Anything else gives `null`. The chunk's language is stored on each of its segments (FR-025).
  - **Mock server**: the wiremock tests cover `verbose_json` OK, a 400 then `json`, a 2xx without segments, a late answer (timeout) and 401.
- **Rationale**:
  - FR-022, FR-024, FR-025, FR-029. `whisper-1` (OpenAI) and `whisper-large-v3-turbo` (Groq) return segments; OpenAI's `gpt-4o(-mini)-transcribe` accept only `json` and get the time-less path. One client keeps the timeout and ban invariants.
  - The exact error body for a rejected format is not specified by the providers, so the decision keys on the status, not on the message.
- **Alternatives**: a second HTTP client for calls (forbidden by the `engine::http` ban); parsing the error message for "response_format" (provider-specific text).

## R11 — Concurrency within a pass

- **Decision (decided, #187)**:
  - **Bound**: one pass at a time (FR-028). Within it, at most 3 chunk requests are in flight, drawn from both tracks in start order, while the decoder produces chunks one at a time, so at most 3 encoded chunks (≈ 11.5 MB) are held in memory.
  - **Ordering**: results are kept by chunk index; a track is `done` when all its chunks succeed.
  - **Failure**: the first failure of a track cancels that track's remaining chunks and lets the other track finish (FR-027).
  - **Rate limits**: 429 is `provider_error` (no hidden retry; "Transcribe again" exists).
- **Rationale**: SC-003 (1 h call ≤ 5 min after stop) does not fit a strictly sequential pass. Suppose about 60 chunks of 3.8 MB at 20 Mbit/s (≈ 1.5 s each to upload) plus 5–10 s server time each: sequential that is 6–11 min, and 3 in flight is 2–4 min. FR-018's "one chunk at a time" applies to decoding, not to requests (#187): the decoder still holds one chunk at a time, and the in-flight uploads are a bounded few.
- **Alternatives**: (a) strictly sequential, which risks SC-003 on slow providers; (b) unbounded parallel, which hits provider rate limits and memory.

## R12 — Call indicator window (OQ-45 spike, ADR-007)

- **Decision**:
  - **Window**: a second Tauri window with label `call-indicator` on its own opener thread and capability (settings-ui.md):
    - created hidden at first Record, never at app start (overlay.md §5);
    - undecorated, `always_on_top`, `skip_taskbar`, transparent off, focusable;
    - its route is `/call-indicator`.
  - **Showing and focus**: it is shown with `SW_SHOWNOACTIVATE`, so the call app keeps the focus at start. Clicking the stop control may activate it; after a click the shell returns the foreground to the window that had it (ADR-007 fallback).
  - **Moving**: through `data-tauri-drag-region`. The position is saved in `calls.indicator_position` and clamped to a visible monitor at show.
  - **Text**: shown text comes from the core message catalog (the overlay rule, i18n.md); levels and timer arrive as typed events at most 10 Hz.
  - **Smoke**: it is a product window, never in `scripts/ci/helper-windows.txt`. The install smoke's window census passes because the indicator is not shown at start and a refused start never shows it (FR-043).
- **Confirm at spike** (the first indicator task, Windows CI plus an owner check):
  - `SW_SHOWNOACTIVATE` and an interactive click without a focus steal;
  - drag on an undecorated window;
  - staying above a full-screen app (the owner's PC only; `verify_exception`);
  - which framework helper windows a second webview adds, if any (the census).
  - If Tauri cannot keep focus, the fallback is a native Win32 layered window with the same IPC events, decided at the spike (OQ-45 stays open until then).
- **Alternatives**: reusing the dictation overlay (non-interactive by design, overlay.md §3); a native window first (more shell code before the spike says it is needed).

## R13 — `--call record`: a core-decided launch action

- **Decision**:
  - **Parsing**: core `settings::gate` gains `LaunchArgs::parse(argv)` (known: `--autostart`, `--call record`; unknown `--call <x>` → `CallSwitch::Unknown`, logged `call_switch_refused reason=unknown_value`, no action). `second_launch_action` changes signature from today's `second_launch_action(launched_by_autostart: bool)` (`crates/voicen-core/src/settings/gate.rs`) to `second_launch_action(&LaunchArgs)`, and returns `StartCallRecord` when the switch is present (contracts/core-call.md "Launch arguments" names the callers).
  - **Second instance**: `tray::on_second_instance` posts that single request to the main event loop (windows-shell.md: the callback writes nothing and posts one request). The request enters the same `CallSession::toggle_record(On, Source::Switch)` as the tray and the hotkey. The refusals of FR-002, the indicator, the tray state and the reminder therefore follow from the one path.
  - **First launch with the switch**: the switch is read in primary startup after `build()` and the log start line, then applied once the tray exists. A launch without it never starts call mode (FR-001).
  - **Log**: `call_requested source=switch|tray|hotkey`.
- **Rationale**: #184, FR-003, FR-043. The install smoke runs the shipped path unchanged.
- **Alternatives**: none open (#184 decided (a)).

## R14 — Exit, installer close, shutdown, logoff and sleep during a recording

- **Decision**:
  - **Tray Exit while recording**: a native `MessageBoxW` (Yes/No; text from the catalog) on a worker thread. Yes runs `CallSession::stop(StopReason::Exit)` and then exit; No keeps recording (FR-007).
  - **`WM_ENDSESSION` and installer close**: `WM_ENDSESSION` arrives as `RunEvent::Exit`; installer close is the stock "app is running" close, recognised by `logs/installer-ended` (installer.md). Both run `CallSession::finalize(StopReason::SystemEnd)`, without a question. Finalizing means writing the EOS page, syncing both files and writing `meta.json` = `ready-to-transcribe` within a 2 s budget. Drop does not run on exit (windows-shell.md), so this is an explicit call from the exit handler, before the log's exit line.
  - **Restart of interrupted work**: a `ready-to-transcribe` call left this way is queued at the next start (FR-007). A `transcribing` call is reset to `ready-to-transcribe` and queued (FR-028).
  - **Sleep and lock**: `PBT_APMSUSPEND` (already seen on the hotkey thread) posts `stop(StopReason::Sleep)`; `PBT_APMRESUMEAUTOMATIC` posts the "stopped on sleep" notice. A WTS lock or unlock does nothing to a call (FR-008).
  - **Forced end**: a kill that bypasses all of this leaves `recording`, which recovery handles (R7).
- **Confirm on Windows**: the time available between `WM_ENDSESSION` and process end (expected ≥ 5 s); the stock NSIS close message reaching `RunEvent::Exit`.
- **Alternatives**: a webview confirm dialog (a hidden window at exit and an async round-trip); finalizing in `Drop` (never runs).

## R15 — Uninstaller: the calls question and `/PURGECALLS`

- **Decision**:
  - **Decision point**: the existing PREUNINSTALL decision point (installer.md: one decision point) computes `$VoicenPurgeCalls`, after `$VoicenPurgeData`:
    - update mode → keep;
    - `/PURGECALLS` → remove;
    - `/S` or `/P` → keep;
    - else, if `$LOCALAPPDATA\Voicen\calls` exists, a MessageBox "remove recorded calls in <folder>?" with default No (`MB_DEFBUTTON2`, LangStrings 1033/1049);
    - no folder → no question.
  - **POSTUNINSTALL**: `RMDir /r "$LOCALAPPDATA\Voicen"` is replaced. When calls are kept, every top-level entry except `calls` is removed and `$LOCALAPPDATA\Voicen` is left in place. When calls are purged, `calls\` alone is removed with `RMDir /r` (and the rest per `$VoicenPurgeData`).
  - **Other roots**: no storage root other than the default is ever touched; the uninstaller does not read the settings file.
- **Confirm on Windows**: the forked NSIS template of T-025 never runs `RMDir /r "$INSTDIR"`, since `$INSTDIR` is `%LOCALAPPDATA%\Voicen`, the parent of `calls\`. The first installer task greps the fork and proves it on the runner.
- **CI**: the windows job's `/S` uninstall step ("uninstall.exe /S removes the data folder, every Voicen/ credential, the Run value and the shortcut, and keeps other credentials (T-025)") first plants `calls\2026-01-01_000000\meta.json` and after uninstall asserts it exists. A new step runs `/S /PURGECALLS` and asserts it is gone. Both are first run on `wip/` (windows-ci-runner.md).
- **Rationale**: FR-041, decisions #170, #180 #1, r4#12; SC-010.

## R16 — Toasts and notices

- **Decision**: Call notices (FR-006, FR-008, FR-013, FR-016, FR-018, FR-030) go through T-007's notification mechanism (a prerequisite, plan.md). The core emits typed `CallNotice` values with catalog keys; the shell maps them to toasts. Warnings also go to the indicator's warning list until dismissed. "Open" on "Transcript ready" opens the Calls window on that call.
- **Rationale**: one notifier (P-011). Until T-007 lands, the call tasks that need a toast stay blocked, and the indicator list is testable on its own (Playwright).

## R17 — `transcript.json` schema 1 validation without a new crate

- **Decision**: The serde types of §7 schema 1 (`#[serde(deny_unknown_fields)]` and `schema: 1`) and a hand-written `validate()` check:
  - segments sorted by `start_ms`;
  - `0 ≤ start_ms ≤ end_ms ≤ duration_ms`;
  - `speaker` ∈ meta speakers;
  - `track` ∈ {mic, system};
  - `source` = `final`;
  - `lang` either a `LangCode` or null;
  - non-empty `text`.

  Each check has a closed rule name for the `transcript_invalid rule=` log line: `unsorted`, `out_of_range`, `unknown_speaker`, `unknown_track`, `bad_source`, `bad_lang`, `empty_text`.

  Writing is temp file + rename in the call folder, so no partial file is left; a write failure leaves `failed: storage` (FR-018, FR-026). `transcript.md` is rendered from the validated value.
- **Rationale**: the schema is small and fixed; a JSON-Schema crate would be a dependency for one file. Validation runs on our own types before the write (FR-026).

## R18 — Clipboard copy and exports

- **Decision**: "Copy as Markdown" uses the dictation clipboard writer's exclusion formats (FR-10: `ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory = 0`, `CanUploadToCloudClipboard = 0`) through the same shell function, with no second clipboard path (P-011). Exports open a native save dialog in Rust and write temp + rename in the target folder; failure shows the reason, and no partial file remains (FR-036).

## R19 — Storage roots, folder names, disk space

- **Decision**:
  - **Roots**: settings `calls.storage_root: Option<PathBuf>` (`None` = `AppPaths::data_dir()/calls`) and `calls.earlier_roots: Vec<PathBuf>`.
  - **Writable check**: create and delete `.voicen-write-probe` in the root, at save (FR-039) and at each Record start (FR-002).
  - **Folder name**: the local start time `YYYY-MM-DD_HHMMSS`; `-1`, `-2`, … on a clash.
  - **Free space**: a `DiskSpace` port (shell: `GetDiskFreeSpaceExW`) is polled every 10 s while recording. Below 1 GB → warning once; below 200 MB → stop with saving (FR-018).
  - **Time limit**: 4 h stops with saving through the session's clock.
  - **Listing**: the Calls window lists the current root and every earlier root. An unavailable root shows one line naming it (spec Assumptions).
  - **Temp audio**: the dictation `TempAudioStore` gets an explicit exclusion test: it never deletes under any root (FR-020).

## R20 — Log events

- **Decision**: New closed `LogEvent` variants, integers and enums only plus one validated identifier type (diagnostics-log.md; each needs `format.rs`, the exhaustive `LogObserver` and `diag_format` tests). The line bodies are those of contracts/core-call.md "Log lines", which is authoritative for the fields. The variants:
  - `CallRequested { source: tray|hotkey|switch }`; the smoke greps `call_requested source=switch`;
  - `CallStarted { tracks, mic_endpoint, system_endpoint }`;
  - `CallRefused { part: mic|system|root, reason }`; the smoke greps `call_refused part=system reason=no_device`;
  - `CallStopped { reason, duration_ms }`;
  - `CallDeviceChanged { track, cause, gap_ms, endpoint }`;
  - `CallTrackSilent { track }`;
  - `CallRecovered { calls }`;
  - `CallStorageFailed { kind }`;
  - `FinalPassStage { track, stage: decode|vad|upload|merge|write, ms, chunks }`;
  - `FinalPassFinished { outcome, reason }`;
  - `CallSwitchRefused { reason }`;
  - `VadFallback { track, cause: unavailable|error }` (#188);
  - `TranscriptInvalid { rule, segment }`: the R17 rule that `validate()` refused and the segment index, never the segment's text.

  Endpoint ids are logged as `EndpointId`, a newtype accepting only the WASAPI endpoint-id charset (`{`, `}`, `.`, hex digits, `-`) and at most 128 characters. Anything else is logged as `endpoint=invalid`, so no friendly name or free text can reach the file (FR-042, SC-009, P-009). The log never names the call folder or title.
- **Rationale**: FR-042 with the existing allowlist mechanism. SC-003's measurement reads the `FinalPassStage` durations.

## R21 — Live track activity, levels and "can't hear" warnings

- **Decision**: For each track the core computes RMS over 100 ms blocks (levels for the indicator, ≤ 10 Hz) and a per-second speech flag from the existing `EnergyDetector`. FR-013's two warnings fire after > 60 consecutive seconds of silence on one track while the other track has speech in that time. Each fires once per silent stretch and is re-armed after speech returns. Silero is not run live in 1a.
- **Rationale**: FR-013 needs a cheap, continuous signal. Energy is enough for "is anything coming at all". Silero's precision matters for the final pass, not here.

## R22 — WAV track sources (FR-014)

- **Decision**: Core `WavTrackSource` implements the call-track source port from a 16-bit PCM or f32 RIFF WAV file (an in-house reader of about 100 lines, no new crate). It feeds blocks with synthetic `CaptureStamp`s at real-time or accelerated pace on the fake clock, and an optional drift factor and scripted gaps. The shell's integration tests (`src-tauri/tests/call_*.rs`) wire the same port to WAV files on the device-less runner, so capture → storage → final (mock server) → merge runs in Windows CI.

## R23 — WER harness (FR-047, outside CI)

- **Decision**:
  - **Location**: a `voicen-core` example `wer` that the owner runs on their PC with their key. Neither the gate nor CI runs the real measurement (the real key and the real sets); a test may run the built example against a loopback server with fake data (#221).
  - **Scoring**: core's `wer` module computes word error rate (normalization: lowercase, punctuation stripped, numbers as written, `ё`→`е` for Russian; word-level Levenshtein) and is unit-tested on the Linux host.
  - **English set**: a pinned subset of LibriSpeech test-clean (CC BY 4.0, #185).
    - The archive is `test-clean.tar.gz`, 346 663 984 bytes. OpenSLR publishes MD5 `32fa31d27d2e1cad72775fee3f4849a9` only, so the first fetch also pins our own SHA-256.
    - The subset is a committed manifest of utterance ids totalling ≥ 30 min. They are concatenated in manifest order into one 16 kHz mono WAV per speaker group, so the chunker and VAD are exercised.
  - **FLAC (decided, #186)**: the archive is FLAC. The owner converts the subset once to 16 kHz mono WAV with an external tool; the command is documented in the example's usage. The example reads WAV only and adds no dependency (no `claxon`).
  - **Russian set**: the owner's 2 × 10-min labelled calls with word-for-word transcripts, kept outside the repository (customer data rule).
- **Rationale**: FR-047, SC-004, CA-7. Nothing of the sets or the key enters the repository or CI.

## R24 — Dictation during a call (FR-009)

- **Decision**: Dictation keeps its own cpal input stream on the same device (WASAPI shared mode allows two). Its session, Esc handling and NFR-02 figure are unchanged. The call session owns only its two track streams. The tray state precedence (FR-006) is a pure core function over both sessions' states, tested over the full product of states.
- **Confirm on Windows**: two shared-mode streams on one capture endpoint open and run together (the owner's PC; the runner has no capture endpoint, windows-ci-runner.md).
