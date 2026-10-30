# Contract: Core call ports, file formats and log lines

The core API (`crates/voicen-core`) that the shell and the tests build on. Every name here is planned and becomes a contract when its task ships. Signatures are indicative: field and type names are binding, exact generics are not. The module split follows ADR-011 as amended in [plan.md](../plan.md) (OQ-48).

## Modules

| Module | Owns | Linux-tested with |
|---|---|---|
| `call::session` | `CallSession`: toggles, single session, start checks, stop reasons, tray state input | fake track sources, fake store, fake clock |
| `call::capture` | `TrackSink`, `TrackSource` ports; `Timeline` (drift, re-anchor, silence fill); levels; activity / FR-013 rule; `WavTrackSource` | WAV files, scripted drift and gaps |
| `call::codec` | `TrackEncoder` / `TrackDecoder` traits; `OpusOgg` implementation (`opus` + `ogg`) | real libopus in the core image |
| `call::storage` | `CallStore`: roots, folder creation, `meta.json`, track writers, recovery, listing (incl. `damaged`), rename, delete, disk thresholds | temp dirs, fault-injecting writer, fake `DiskSpace` |
| `call::final_pass` | queue, chunker, `SpeechSpans` use, timed/time-less mode, per-track results, cancel | scripted spans, wiremock server |
| `call::transcript` | merge, schema 1 types + `validate`, Markdown render, export, speaker/title rename (pure functions) | fixtures |
| `call::events` | the call value types (`Track`, `CallId`, `EndpointId`, `CallState`, `StopReason`, `FailReason`, the closed log field enums), `CallNotice`, `CallWarning`; the `LogEvent` variants are in `diag::event`, the line bodies in `diag::format` (T-118) | `diag_format`, `call_events` tests |
| `settings` (extended) | `calls` section, `FieldId`s, `SettingsTab::Calls`, `LaunchArgs`, `SecondLaunchAction::StartCallRecord` (built so far: [004 core-traits.md › Built surface (T-120)](../../004-settings-and-first-run/contracts/core-traits.md)) | existing settings tests |
| `tray` (extended) | `TrayState::CallRecording`, precedence function, menu table rows | existing tray tests |
| `vad` (extended) | `SpeechSpans` port; energy-span implementation | scripted buffers |
| `engine` (extended) | `transcribe_timed`, `ResponseShape`, `Timed`, `LangCode` table | wiremock |
| `wer` | WER scoring and normalization (used by the example only) | fixtures |

## Track ports

```text
trait TrackSink: Send + Sync {
    fn block(&self, track: Track, interleaved: &[f32], rate: u32, channels: u16, at: CaptureStamp);
    fn lost(&self, track: Track, cause: LossCause, at: CaptureStamp);
}
trait TrackSource {                                   // shell: CpalTrackSource; tests: WavTrackSource
    fn open(&self, track: Track, device: TrackDevice, sink: Arc<dyn TrackSink>)
        -> Result<(Box<dyn CaptureHandle>, EndpointId), OpenError>;   // OpenError: no_device / access_denied / busy / other
    fn now(&self) -> CaptureStamp;                    // QPC on Windows
}
enum TrackDevice { Selected(DeviceId), DefaultInput, DefaultOutputLoopback }
struct CaptureStamp(u64);                             // ns on the QPC timeline
trait DeviceEvents { fn next(&self) -> Option<DeviceEvent>; }   // fed by the shell's IMMNotificationClient
trait CommunicationsCheck { fn differs(&self) -> Result<bool, OsCause>; }
trait DiskSpace { fn free_bytes(&self, root: &Path) -> Result<u64, OsCause>; }
```

Dictation's `FrameSink` / `AudioSource` stay unchanged (FR-009).

## Codec

```text
trait TrackEncoder { fn push(&mut self, pcm16k: &[f32]) -> Result<(), StorageError>; fn finish(self) -> Result<(), StorageError>; }
trait TrackDecoder { fn next_window(&mut self, max_ms: u32) -> Result<Option<AudioBuffer>, DecodeEnd>; }
```

Error and result types (`call::codec` and `call`, as in the code):

```text
struct StorageError { pub op: StorageOp, pub kind: std::io::ErrorKind }
enum StorageOp { Write, Flush, Sync, Encode }
enum DecodeEnd { Unreadable, Damaged, Codec }
enum ReadEnd { Complete, Truncated, Damaged }
struct TrackScan { pub duration_48k: u64, pub end: ReadEnd }
fn scan<R: Read + Seek>(inner: R) -> Result<TrackScan, DecodeEnd>
```

- `StorageOp::Encode` is libopus refusing to set up the encoder or to encode a frame; it is not an I/O error and its `kind` is `Other`.
- `DecodeEnd::Unreadable`: no valid `OpusHead` on the first page. `Damaged`: a bad page is followed by a valid one; the readable prefix before it was returned first. `Codec`: libopus refused to set up the decoder or to decode a packet of a CRC-valid page (not an I/O error).
- `next_window(max_ms)` treats `0` as 1 ms. After a damaged end the reader keeps returning `Err(DecodeEnd::Damaged)`.
- `OpusOgg::create(inner: impl SyncWrite, serial, clock)` is the encoder; `OpusOggReader::open(inner: impl Read + Seek)` is the decoder. `scan` reads a track without decoding: `duration_48k` = last good granule - pre-skip, `end` = how the readable data ended (`Complete` EOS page read, `Truncated` data ends early, `Damaged` as above).

`push` runs on the session's writer thread, never in the audio callback. The audio callback only hands blocks to a bounded channel; a full channel drops blocks and the timeline fills that time with silence and logs it.

## `mic.opus` / `system.opus`

| Property | Value |
|---|---|
| Container | Ogg, one logical stream (one serial) per file |
| Page 1 | BOS, `OpusHead` only: version 1, channels 1, pre-skip = lookahead × 3, input rate 16000, gain 0, mapping 0; granule 0 |
| Page 2 | `OpusTags`: vendor `voicen <version>`, 0 comments; granule 0 |
| Audio | Opus Voip, 16 kHz encoder, 20 ms packets, VBR 24 kbit/s; granule in 48 kHz units (+960 per packet) |
| Paging | `EndPage` every 50 packets (1 s), `flush` after each page, `sync_data` every 5 s |
| End | Last page with EOS; final granule trims the tail |
| Truncated tail | Read up to the last page with a valid CRC; never rewritten (research R7) |

## `meta.json` (schema 1 of the call metadata)

A JSON object with the fields of data-model.md "Call", in that order, snake_case keys, written with temp + rename. Example states: `"state": "recording"`, `"state": "failed", "failure": {"reason": "timeout", "track": "system"}`. `engines` is `{"transcription": "api:<model>", "diarization": null}` once a pass has started (`EngineRef`, the same string as in `transcript.json`; data-model.md "Call").

## `transcript.json` (requirements §7 schema 1)

Exactly the §7 keys. In 1a:
- `engines.transcription` is `"api:<model>"`, the `EngineRef` string `"<kind>:<model>"` also stored in `meta.json` (one representation in both files; §7 gives `engines.transcription` as a single value);
- `engines.diarization` is `null`;
- every segment has `source: "final"`;
- no `hidden` or `confidence`.

`validate()` (research R17) runs before every write, and a failure is a programming error logged as `transcript_invalid rule=<…> segment=<n>` (see "Log lines") that leaves the call `failed: storage`. Writes use temp + rename, so a crash never leaves a partial file.

## `transcript.md`

```text
# <title>
<started_at local, "YYYY-MM-DD HH:MM"> · <duration H:MM:SS>

**[00:00:03] Me:** text
**[00:00:05] Other party:** text
```

Times are `start_ms` as `HH:MM:SS`, and names come from `meta.json`. `render_markdown` folds every whitespace run in a line's text, the title and the names to one space; `transcript.json` keeps the original text. A speaker id with no name in `meta.json` renders as the id itself. "Copy as Markdown" and the Markdown export use the same renderer.

## Final-pass engine call

```text
fn transcribe_timed(&self, audio: &AudioBuffer, req: &TranscribeRequest, shape: ResponseShape)
    -> Result<Timed, FailureReason>;
enum ResponseShape { Segments /* verbose_json + timestamp_granularities[]=segment */, Plain /* json */ }
struct Timed { text: String, language: Option<LangCode>, segments: Option<Vec<EngineSegment>> }
```

Mapping to the pass:

| Outcome | Pass effect |
|---|---|
| `Segments` OK with segments | lines at chunk start + segment times |
| `Segments` → 400/422, or 2xx without `segments` | engine time-less for the rest of the pass; same chunk sent as `Plain` and re-cut into ≤ 30 s phrases |
| 401 / 403 | track `failed: key` |
| connect error / DNS / TLS | `failed: network` |
| timeout (FR-24 values per chunk) | `failed: timeout` |
| other 4xx/5xx, 429, malformed body | `failed: provider` |
| no API key / no base URL | pass `failed: engine_not_set_up` before any request |

## Launch arguments

`LaunchArgs::parse(argv)` recognises `--autostart` and `--call record`: `LaunchArgs { autostart: bool, call: Option<CallSwitch> }`, `CallSwitch` = `Record` | `Unknown`. It becomes the one parser of `--autostart`: the shell's `autostart::launched_by_autostart` (`src-tauri/src/autostart.rs`, also read at primary startup in `src-tauri/src/lib.rs`) delegates to it, and `AUTOSTART_ARG` stays the constant pinned by `src-tauri/tests/autostart.rs`.

**Signature change of an existing function.** Today `crates/voicen-core/src/settings/gate.rs` has `pub fn second_launch_action(launched_by_autostart: bool) -> SecondLaunchAction` (`FrontSettings` | `TrayOnly`). It becomes `pub fn second_launch_action(args: &LaunchArgs) -> SecondLaunchAction`, and `SecondLaunchAction` gains `StartCallRecord`:
- `call: Some(Record)` → `StartCallRecord` (with or without `--autostart`);
- `call: Some(Unknown)` → `TrayOnly` (no action), and the primary logs `CallSwitchRefused { reason: unknown_value }`;
- otherwise the existing table, unchanged: `autostart` → `TrayOnly`, else `FrontSettings`.

No `settings_open` argument: whether a settings window is open stays the shell's concern inside `FrontSettings`, as today. The existing callers change with it:
- `src-tauri/src/tray.rs` `on_second_instance`, which today calls `second_launch_action(second_launched_by_autostart(&argv))`; it calls `second_launch_action(&LaunchArgs::parse(&argv))`, and `StartCallRecord` posts one request to the main loop (exercised by `src-tauri/tests/single_instance.rs`);
- the core test `second_launch_action_table` in `gate.rs`, rewritten over `LaunchArgs` with its existing rows kept.

## Log lines

One line per event, `key=value` after the 006 prefix. The smoke and tests grep the starred lines; the install smoke greps `call_requested source=switch` and `call_refused part=system reason=no_device` (FR-043; the session opens the system track first, data-model.md "CallSession").

| Event | Line body |
|---|---|
| `CallRequested` ★ | `call_requested source=<tray\|hotkey\|switch>` |
| `CallRefused` ★ | `call_refused part=<mic\|system\|root> reason=<no_device\|access_denied\|busy\|unwritable\|missing\|other>` |
| `CallStarted` | `call_started record=1 subtitles=0 mic=<EndpointId> system=<EndpointId> comms_differs=<0\|1>` |
| `CallDeviceChanged` | `call_device track=<mic\|system> cause=<…> gap_ms=<n> to=<EndpointId\|none>` |
| `CallTrackSilent` | `call_track_silent track=<…> rule=<lost\|cant_hear>` |
| `CallStopped` | `call_stopped reason=<StopReason> duration_ms=<n> mic_bytes=<n> system_bytes=<n>` |
| `CallStorageFailed` | `call_storage_failed kind=<write\|flush\|sync\|meta\|transcript\|low_disk> [os=<code>]` (`os` left out when unknown) |
| `CallRecovered` | `call_recovered calls=<n>` |
| `FinalPassStage` | `final_stage track=<…> stage=<decode\|vad\|upload\|merge\|write> ms=<n> chunks=<n>` |
| `FinalPassFinished` | `final_finished outcome=<ready\|failed> [reason=<…>] mode=<timed\|timeless> ms=<n>` (`reason` only on `failed`) |
| `CallSwitchRefused` | `call_switch_refused reason=unknown_value` |
| `VadFallback` | `vad_fallback track=<mic\|system> cause=<unavailable\|error>` (#188: the pass goes on with energy spans) |
| `TranscriptInvalid` | `transcript_invalid rule=<unsorted\|out_of_range\|unknown_speaker\|unknown_track\|bad_source\|bad_lang\|empty_text> segment=<n>` (research R17; `segment` is the index, never the text) |

Levels: WARN for `call_refused`, `call_track_silent`, `call_storage_failed`, `final_finished outcome=failed`, `call_switch_refused`, `vad_fallback`, `transcript_invalid` and `call_device` with `to=none`; INFO for the rest. The `<EndpointId>` rule (closed charset and length, else `invalid`): `docs/decisions/diagnostics-log.md`.

Never logged: transcript text, titles, speaker names, folder or root paths, hosts, device friendly names, audio (FR-042, SC-009).

## Message ids

All new user-visible text goes through `messages!` with EN and RU entries (FR-046). The ids are listed in [ipc.md](ipc.md) "Message catalog".
