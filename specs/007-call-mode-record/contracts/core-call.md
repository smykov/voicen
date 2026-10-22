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
| `call::events` | `CallNotice`, `CallWarning`, the `LogEvent` variants | `diag_format` tests |
| `settings` (extended) | `calls` section, `FieldId`s, `SettingsTab::Calls`, `LaunchArgs`, `SecondLaunchAction::StartCallRecord` | existing settings tests |
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

A JSON object with the fields of data-model.md "Call", in that order, snake_case keys, written with temp + rename. Example states: `"state": "recording"`, `"state": "failed", "failure": {"reason": "timeout", "track": "system"}`.

## `transcript.json` (requirements §7 schema 1)

Exactly the §7 keys. In 1a:
- `engines.transcription` is `"api:<model>"`;
- `engines.diarization` is `null`;
- every segment has `source: "final"`;
- no `hidden` or `confidence`.

`validate()` (research R17) runs before every write, and a failure is a programming error logged as `transcript_invalid` that leaves the call `failed: storage`. Writes use temp + rename, so a crash never leaves a partial file.

## `transcript.md`

```text
# <title>
<started_at local, "YYYY-MM-DD HH:MM"> · <duration H:MM:SS>

**[00:00:03] Me:** text
**[00:00:05] Other party:** text
```

Times are `start_ms` as `HH:MM:SS`, and names come from `meta.json`. "Copy as Markdown" and the Markdown export use the same renderer.

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

`LaunchArgs::parse(argv)` recognises `--autostart` and `--call record`. `second_launch_action(&LaunchArgs, settings_open: bool)` returns:
- `StartCallRecord` when the switch is present;
- otherwise the existing `FrontSettings` / `TrayOnly`.

`--call <other>` → `CallSwitchRefused { reason: unknown_value }`.

## Log lines

One line per event, `key=value` after the 006 prefix. The smoke and tests grep the starred lines.

| Event | Line body |
|---|---|
| `CallRequested` | `call_requested source=<tray\|hotkey\|switch>` |
| `CallRefused` ★ | `call_refused part=<mic\|system\|root> reason=<no_device\|access_denied\|busy\|unwritable\|missing\|other>` |
| `CallStarted` | `call_started record=1 subtitles=0 mic=<EndpointId> system=<EndpointId> comms_differs=<0\|1>` |
| `CallDeviceChanged` | `call_device track=<mic\|system> cause=<…> gap_ms=<n> to=<EndpointId\|none>` |
| `CallTrackSilent` | `call_track_silent track=<…> rule=<lost\|cant_hear>` |
| `CallStopped` | `call_stopped reason=<StopReason> duration_ms=<n> mic_bytes=<n> system_bytes=<n>` |
| `CallStorageFailed` | `call_storage_failed kind=<write\|flush\|sync\|meta\|transcript\|low_disk> os=<code>` |
| `CallRecovered` | `call_recovered calls=<n>` |
| `FinalPassStage` | `final_stage track=<…> stage=<decode\|vad\|upload\|merge\|write> ms=<n> chunks=<n>` |
| `FinalPassFinished` | `final_finished outcome=<ready\|failed> reason=<…> mode=<timed\|timeless> ms=<n>` |
| `CallSwitchRefused` | `call_switch_refused reason=unknown_value` |

Never logged: transcript text, titles, speaker names, folder or root paths, hosts, device friendly names, audio (FR-042, SC-009).

## Message ids

All new user-visible text goes through `messages!` with EN and RU entries (FR-046). The ids are listed in [ipc.md](ipc.md) "Message catalog".
