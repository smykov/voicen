# Contract: Core call ports, file formats and log lines

The core API (`crates/voicen-core`) that the shell and the tests build on. Every name here is planned and becomes a contract when its task ships. Signatures are indicative: field and type names are binding, exact generics are not. The module split follows ADR-011 as amended in [plan.md](../plan.md) (OQ-48).

## Modules

| Module | Owns | Linux-tested with |
|---|---|---|
| `call::session` | `CallSession`: toggles, single session, start checks, stop reasons, tray state input | fake track sources, fake store, fake clock |
| `call::capture` | `TrackSink`, `TrackSource` ports; `Timeline` (drift, re-anchor, silence fill); levels; activity / FR-013 rule; `WavTrackSource` | WAV files, scripted drift and gaps |
| `call::codec` | `TrackEncoder` / `TrackDecoder` traits; `OpusOgg` implementation (`opus` + `ogg`) | real libopus in the core image |
| `call::storage` | `CallStore`: roots, folder creation, `meta.json`, track writers, recovery, listing (incl. `damaged`), rename, delete, disk thresholds | temp dirs, fault-injecting writer, `FakeDisk` (the existing `DiskSpace` fake) |
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
// no call-side disk-space port: the call side reuses `local_models::download::DiskSpace`
// (`fn available_bytes(&self, dir: &Path) -> io::Result<u64>`, T-016); the OS code is
// `io::Error::raw_os_error()`, as in `CallStorageFailed { os }` (no `OsCause` type).
```

Dictation's `FrameSink` / `AudioSource` stay unchanged (FR-009).

Built (T-124, `call/capture.rs`, `call/wav_source.rs`; the invariants are in the module docs and the task's analysis): the names above are as built, plus `OpenError` → `RefusedReason` (`From`), `LossCause { Invalidated, NotAvailable, Other }`, `ChannelSink::new(capacity)` (bounded, `try_send`, drop counter; the consumer side is one `TrackReceiver` per track yielding `TrackEvent::{Block, Lost, Dropped}`), and `Timeline::{new(track, call_start), push, lost, dropped(blocks, ms), advance_to}` returning `Step { pcm, reports, levels }` with `Report::{Gap, Dropped}` (`Dropped` carries no position; the session reads `Timeline::written()` first, T-126). `WavTrackSource` (`test-fakes` only) has no thread: the test calls `pump(until)`.

Built (T-125, `call/storage.rs`; invariants in `docs/decisions/call-storage.md`): `CallStore::new(roots: Arc<dyn CallRoots>, clock: Arc<dyn MonotonicClock>)`; `create(&CallSettings, &NewCall) -> Result<ActiveCallFiles, CreateError>` and `create_with(.., open)` (the opener of a track file, for a fault-injecting writer); `list(&CallSettings) -> CallList { entries: Vec<CallEntry>, unavailable: Vec<PathBuf> }`. `NewCall { started: SystemTime, seconds_east, mic, system, lang, app_version }`. `CreateError::{Root(UnwritableReason), Storage(StorageKind)}`; `ActiveCallFiles { mic, system: OpusOgg<W> }` with `dir()`, `meta()` and `write_meta(&CallMeta)`. `CallMeta` (and `Failure`, `Mode`, `TrackMeta`, `DeviceChange`, `Engines`, `Results`, `TrackResult`) is `meta.json` schema 1 with `deny_unknown_fields`; `DeviceChange.cause` is `DeviceCause` and `pass_mode` is `Option<PassMode>` (call/events.rs, serde names = the `call_device cause=` / `final_finished mode=` log literals), so a value outside those sets lists as damaged. `TrackResult` was only `Pending` here; since T-138 it also has `Done` and `Failed` (see "Built (T-138)" below), and a `meta.json` carrying a result outside those three lists as damaged. `ListState` = `Recording | Queued | Transcribing | Ready | Failed(FailReason) | Recovered | Damaged`. Also `pub fn write_meta(dir, &CallMeta)`, `pub(crate) fn write_atomic(dir, name, bytes)` (T-138 and T-141 reuse it) and `pub fn storage_kind(StorageOp) -> StorageKind`. Not built yet: recovery, rename, delete, disk thresholds and their polling (later tasks).

Built (T-126, `call/session.rs`, `call/indicator.rs`; invariants in `docs/decisions/call-session.md`): `CallSession::new(CallDeps)` and `toggle_record(on, CallSource) -> Toggled { Started, AlreadyOn, Refused { part, reason }, Stopped, AlreadyOff }`, `stop(StopReason)`, `is_recording`, `tick` / `next_deadline` (the 10 s reminder, driven by the shell's timer). Ports owned by the session: `CallIndicator` (`set_state`, `set_tick`, `set_call_recording`; inside the lock), `CallShell` (`notice`, `open_calls_tab`; after the lock) and `CallSettingsPort` (`snapshot`, `remember_warned_host`, implemented for `SettingsService`). Start order system, mic, root; every `part=root` refusal opens the Calls tab (#243 D1); a create failure after the probe is `call_storage_failed kind=<kind>` then `call_refused part=root reason=other` (D2), as is a writer thread that cannot be spawned (without the first line). One writer thread drains both tracks through a `Timeline` each into the encoders and publishes `IndicatorTick` (at most 10 per second; `mic_silent` / `system_silent` stay `false` until the FR-013 task). Each `Report::Dropped` gives one `call_track_dropped` line and one `meta.json` `drops[]` entry (#244); `Report::Gap` gives neither. `CallWarning::ProviderNotice { host }` (typed `ProviderHost`, serialized as its text) and `SettingsService::remember_warned_host` (`docs/decisions/settings.md`). Wire fixture `e2e/fixtures/call-indicator-wire.json`.

Built (T-141, `call/storage.rs`, `call/transcript.rs`; wire details in `ipc.md` › Built T-141): `resolve_call`, `CallStore::get(calls, root, id, lang) -> Result<CallDetail, CallError>`, `CallStore::export_bytes(calls, root, id, Format) -> Result<Vec<u8>, CallError>` (copy and export; `transcript::export_markdown` is `render_markdown` with the `meta.json` title and names, `export_json` is `to_json` of the same transcript with them), `export_to(target, bytes)`. `write_atomic` and `export_to` differ only in how the tmp is opened; the tmp+sync+rename tail is the one private `storage::fill_and_rename`.

Built (T-138, `call/final_pass.rs`, `call/storage.rs`; why: `docs/decisions/call-final-pass.md`): the pass runner. The pass has no caller until T-139 (shell wiring) and T-146 (queue, cancel).

- **Entry points.** `run_pass(PassCall { dir, meta }, Arc<Settings>, &dyn CredentialStore, PassDeps { primary: Option<Arc<dyn SpeechSpans>>, events: &dyn PassEvents, clock: Arc<dyn MonotonicClock> }) -> PassOutcome { outcome: FinalOutcome, mode: PassMode }` is the whole pass from `ready-to-transcribe`. `run_pass_with(call, &dyn TimedTranscriber, &TranscribeRequest, EngineRef, PassSources { mic, system: Box<dyn TrackDecoder> }, deps)` is the same pass with the engine and the open tracks given (scheduling tests, no HTTP). `TimedTranscriber: Send + Sync` is the `transcribe_timed` signature above, implemented for `OpenAiCompatibleEngine`. `PassEvents: Send + Sync { log(LogEvent), notice(CallNotice) }` is the only way the pass reports; T-139 wires it to `diag::Log` and the notifier. Stage times come from the injected `MonotonicClock`.
- **Engine setup (`run_pass`).** One `Settings` snapshot and one `creds.read(KeySlot::TranscriptionApi)` per pass. The engine is always the API engine, whatever `settings.engine` is (not `engine_for`): `OpenAiCompatibleEngine::new(check_base_url(api.base_url), api.model, key)`, `TranscribeRequest { language: speech_language, timeouts: Timeouts::from_settings }`, `EngineRef` `api:<api.model>`. A base URL `check_base_url` refuses, or a key that is absent or blank after trim, is `failed: engine_not_set_up`; a credential-store read `Err` is `failed: key`. Either way no request is sent, both results are `failed{reason}`, `failure.track` is `null`, `pass_mode` is `timed`, and only the failed `meta.json` is written (no `transcribing` state). A blank model is not checked. A settings or key change after the start is not picked up.
- **Dispatch.** The calling thread is the dispatcher: it decodes both tracks one window at a time (asks `TIMED_MAX_MS - carry_ms()`), feeds a `ChunkPlanner` per track (window start = sum of the floored window lengths) and keeps at most one ready chunk set per track. At most 3 requests are in flight, drawn from both tracks, the next one being the head chunk with the smaller call time (`start_offset_ms` + chunk start), mic first on a tie. Each request runs on its own scoped thread.
- **Time-less switch.** When a `Segments` request answers `Timeless`, the pass mode becomes `timeless` for good: that chunk and every chunk dispatched after it, answered, in flight or queued, is re-cut by `recut_timeless` and sent `Plain`; an answer with times of a later chunk is dropped and the chunk re-sent (at most 2 extra requests); chunks dispatched before it keep their times. A `Timeless` answer to a `Plain` request fails the track with `provider`.
- **VAD fallback.** Each window's spans are the primary detector's `Ok`. With no primary (`cause=unavailable`) or on its first `Err` (`VadError::Unavailable` gives `unavailable`, any other `error`), the pass logs one `vad_fallback` for the track and uses `EnergyDetector` spans for that window and the rest of the track; an `Err` is never empty spans. If the energy detector itself returns `Err`, the whole window counts as speech.
- **Track failure.** A track's first `Failed(reason)` answer, or a decode `Unreadable` / `Codec` end (`storage`), stops that track: its queue is cleared, its later answers are dropped, the other track runs on. `Ok(None)`, an empty window and `Damaged` end a track normally (the CRC-valid prefix was already returned). When both fail, the reason is the first failure in dispatch order and `failure.track` is `null`; with one failed, `failure.track` is that track.
- **`TrackResult`** (`meta.json` `results.<track>`, snake_case, no longer `Copy`): `"pending"`, `{"done": {"segments": [SegmentV1…]}}` (the call-relative lines `merge` gave that track alone) or `{"failed": {"reason": <FailReason>}}`.
- **Files written**, each through `write_atomic`, in this order: `meta.json` as `transcribing` with `engines.transcription` (before any decoding); then, only when both tracks are done and `validate()` passed, `transcript.md`, `transcript.json`, and `meta.json` as `ready` (`failure` cleared, `pass_mode`, both results `done`). A failed track gives only `meta.json` as `failed{reason, track}` with `pass_mode` and the per-track results; no `transcript.*`. A `validate()` `Err` logs `transcript_invalid` and any write `Err` logs `call_storage_failed kind=transcript` (`transcript.*`) or `kind=meta`; each gives `failed: storage` with a best-effort failed `meta.json` (if that write fails too, it logs `kind=meta` and the pass still ends `storage`). A file is whole or absent. `meta.duration_ms: null` uses the smaller of (track offset + decoded length).
- **Log events and notices.** `final_stage` for `decode`, `vad` and `upload` per track (summed ms, `chunks` = chunks cut for that track); `merge` and `write` run once over both tracks and are logged once per track with the shared ms; exactly one `final_finished` per pass (`mode` = the mode the pass ended in), then exactly one notice: `TranscriptReady { call }` or `PassFailed { reason }`. A request panic is not turned into a reason: it is re-raised on the dispatcher thread.

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

Line times, including the clamps to the chunk and to `duration_ms`: `data-model.md` "Segment".

On read, an unknown key in any schema-1 object (`speakers` entries included) is a parse error. A non-null `engines.diarization` is also a parse error (no R17 rule); a missing key reads as `null`. `engines.transcription` is not parsed on read: its `api:<model>` form comes from `merge`/`EngineRef`.

`validate()` (research R17) checks each line's `speaker` against the value's own `speakers` ids (`merge` copies them from the caller's `meta.json` speakers), not against `meta.json` directly. It runs before every write, and a failure is a programming error logged as `transcript_invalid rule=<…> segment=<n>` (see "Log lines") that leaves the call `failed: storage`. Writes use temp + rename, so a crash never leaves a partial file.

## `transcript.md`

```text
# <title>
<started_at local, "YYYY-MM-DD HH:MM"> · <duration H:MM:SS>

**[00:00:03] Me:** text
**[00:00:05] Other party:** text
```

Times are `start_ms` as `HH:MM:SS`, and names come from `meta.json`. `render_markdown` folds every whitespace run in a line's text, the title and the names to one space; `transcript.json` keeps the original text. A speaker id with no name in `meta.json` renders as the id itself. A `started_at` not of the `YYYY-MM-DDTHH:MM` shape is shown whole on the date line, whitespace folded. "Copy as Markdown" and the Markdown export use the same renderer.

## Final-pass engine call

```text
fn transcribe_timed(&self, audio: &AudioBuffer, req: &TranscribeRequest, shape: ResponseShape)
    -> Result<Timed, TimedFailure>;
enum ResponseShape { Segments /* verbose_json + timestamp_granularities[]=segment */, Plain /* json */ }
struct Timed { text: String, language: Option<LangCode>, segments: Option<Vec<EngineSegment>> }
enum TimedFailure { Timeless, Failed(FailReason) }
```

`transcribe_timed` (on `OpenAiCompatibleEngine`, not the `Engine` trait, T-136) decides the row of the table below itself: `Timeless` only for `Segments` on 400/422 or a 2xx whose `segments` is absent/null (or empty while `text` is not blank; empty with blank text is `Ok(Some([]))`); every other failure is `Failed(reason)` through `failure::classify` (`InvalidApiKey` → `key`, `NetworkUnavailable`/`CannotReach` → `network`, `Timeout` → `timeout`, `EngineNotConfigured` → `engine_not_set_up`, the rest → `provider`). A `segments` value that is not an array of objects with finite non-negative numeric `start <= end` and string `text` is `provider`. Language values (top-level onto `Timed.language`, per segment onto `EngineSegment.lang`) are ASCII-lowercased, then a `WHISPER_ISO_639_1` code or one of whisper.cpp's 97 names, else `None`.

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
| `CallTrackDropped` | `call_track_dropped track=<mic\|system> blocks=<n> ms=<n> at_ms=<n>` (T-126, #244: a full track channel dropped `blocks` blocks spanning `ms`, at track position `at_ms`; a stall gap has no line) |
| `CallRecovered` | `call_recovered calls=<n>` |
| `FinalPassStage` | `final_stage track=<…> stage=<decode\|vad\|upload\|merge\|write> ms=<n> chunks=<n>` |
| `FinalPassFinished` | `final_finished outcome=<ready\|failed> [reason=<…>] mode=<timed\|timeless> ms=<n>` (`reason` only on `failed`) |
| `CallSwitchRefused` | `call_switch_refused reason=unknown_value` |
| `VadFallback` | `vad_fallback track=<mic\|system> cause=<unavailable\|error>` (#188: the pass goes on with energy spans) |
| `TranscriptInvalid` | `transcript_invalid rule=<unsorted\|out_of_range\|unknown_speaker\|unknown_track\|bad_source\|bad_lang\|empty_text> segment=<n>` (research R17; `segment` is the index, never the text) |

Levels: WARN for `call_refused`, `call_track_silent`, `call_storage_failed`, `call_track_dropped`, `final_finished outcome=failed`, `call_switch_refused`, `vad_fallback`, `transcript_invalid` and `call_device` with `to=none`; INFO for the rest. The `<EndpointId>` rule (closed charset and length, else `invalid`): `docs/decisions/diagnostics-log.md`.

Never logged: transcript text, titles, speaker names, folder or root paths, hosts, device friendly names, audio (FR-042, SC-009).

## Message ids

All new user-visible text goes through `messages!` with EN and RU entries (FR-046). The ids are listed in [ipc.md](ipc.md) "Message catalog".
