# Data Model: Call Mode 1a — Record and Read

Phase 1 of [plan.md](plan.md). This file adds field types, rules and state transitions to the entities of the spec's Key Entities and requirements §7 (release 2). The stored formats are in [contracts/core-call.md](contracts/core-call.md), and the wire shapes are in [contracts/ipc.md](contracts/ipc.md). Durations are integer milliseconds; times on disk are ISO-8601 local time with offset (`clock::Clock` + `clock::LocalOffset`, the one clock of 006).

## CallSession (core, in memory; one per process)

| Field | Type | Rule |
|---|---|---|
| `record` | `bool` | Off at every start (FR-001). Turning it on starts a new `Call`; turning it off ends that call. |
| `subtitles` | `bool` | Always off in 1a. The model has it for FR-019 and the 1b UI. A fake subtitle consumer proves that Subtitles-only writes nothing. |
| `active` | derived: `record \|\| subtitles` | — |
| `call` | `Option<ActiveCall>` | `Some` exactly while `record`. |
| `last_combination` | `Toggles` | Persisted in `CallSettings`. The hotkey starts with it; in 1a it is always `{record}`. |

Every toggle source (tray, hotkey, `--call record`) calls `toggle_record(on, Source)` under the one session lock. The indicator and tray updates happen inside the lock (the dictation-session invariant, dictation-session.md). A repeated "on" while `record` is already on applies to the current call: no second capture, no second folder.

```text
            toggle_record(on)                      stop(reason) / failure
  Idle ───────────────────────▶ Starting ──ok──▶ Recording ─────────────────▶ Stopping ──▶ Idle
   ▲                               │                 │ (sleep, 4 h, < 200 MB, write failure,
   │        refused (track, root)  │                 │  exit Yes, WM_ENDSESSION, installer close)
   └───────────────────────────────┘                 │
                                                     └── device events → Timeline (stays Recording)
```

`Starting` opens both tracks and checks the root before it creates any folder. The system track is opened first, then the mic, then the root is checked; the first failure refuses the start, so on a host with no output device the refusal is `call_refused part=system reason=no_device` whatever the capture endpoints (FR-043). A refusal leaves nothing on disk, closes any track already opened and logs `call_refused` (FR-002).

## Call (`meta.json`, the source of state and names)

| Field | Type | Rule |
|---|---|---|
| `schema` | `1` | Version of `meta.json`. An unknown version is listed as `damaged`. |
| `id` | `CallId` = folder name `YYYY-MM-DD_HHMMSS[-n]` | Unique within a root. |
| `title` | `String` | Default "Call DD.MM HH:MM" (local start time, in the UI language at creation), rendered from catalog id `calls.title_default` = `Call {day}.{month} {hour}:{minute}` (two-digit values). Empty or whitespace-only is refused (FR-035). |
| `started_at` | ISO-8601 with offset | The QPC instant at which both tracks were open, mapped to wall time once. |
| `ended_at`, `duration_ms` | `Option<…>` | Set at stop or recovery. `duration_ms` = the shorter readable track. |
| `state` | `CallState` | See the transitions below. |
| `failure` | `Option<{reason: FailReason, track: Option<Track>}>` | Present only in `failed`. |
| `stop_reason` | `Option<StopReason>` | `user` / `hotkey` / `exit` / `system_end` / `sleep` / `max_length` / `low_disk` / `storage_error`. 1a has no stop switch (#184), so `switch` is a start `Source` only, never a `StopReason`. |
| `mode` | `{record: true, subtitles: bool}` | §7 "mode flags". |
| `tracks` | `[TrackMeta; 2]` | `mic`, `system`. |
| `device_changes` | `Vec<DeviceChange>` | Appended while recording (FR-012). |
| `engines` | `{transcription: Option<EngineRef>, diarization: null}` | Set when a pass starts. `EngineRef` is a typed core value serialized as one string `"<kind>:<model>"`; in 1a always `"api:<model>"` (e.g. `"api:whisper-1"`). The same representation is used in `meta.json` and in `transcript.json` `engines.transcription` (requirements §7 lists `engines.transcription` as one value, not an object). The base URL host is not stored (the provider notice has its own setting). |
| `app_version` | `String` | `BuildInfo::version`. |
| `speakers` | `Vec<Speaker>` | In 1a always `me` and `s1`. |
| `results` | `{mic: TrackResult, system: TrackResult}` | Per-track result of the last pass (FR-027). |
| `pass_mode` | `Option<"timed" \| "timeless">` | The engine mode the last pass ended in (FR-024). |

**Rules**: `meta.json` is written with temp + rename on every state change and every device change, never in place. A missing or unparsable `meta.json`, or a missing track file, lists the folder as `damaged` (FR-033), with "Open folder" and "Delete" only.

### CallState transitions

```text
 recording ──stop / finalize──▶ ready-to-transcribe ──queue picks──▶ transcribing ──both done──▶ ready
     │                                  ▲                                │
     │ (no owning session at start)     │ exit/crash during a pass       ├──any track failed──▶ failed(reason)
     ▼                                  │ (reset at next start)          │
 recovered ──"Transcribe"───────────────┘◀───────────────────────────────┘
                                        ▲
 ready / failed ──"Transcribe again"────┘   (failed: only the failed track is redone)
 recording ──write failure──▶ recording (stays; recovered at next start, FR-016/FR-017)
```

| Shown in the Calls window | When |
|---|---|
| Recording | `recording` owned by the running session |
| Queued | `ready-to-transcribe`, or waiting in the pass queue (FR-028) |
| Transcribing | `transcribing` |
| Ready | `ready` |
| Failed: <reason> | `failed` (`key`, `network`, `timeout`, `provider`, `storage`, `engine_not_set_up`) |
| Recovered after a crash | `recovered` (offered, never started by itself) |
| Damaged | unreadable folder (not a stored state) |

`recording` with no owning session at start → `recovered` (FR-017). This also covers a write failure (FR-016), a kill or a power loss. `transcribing` at start → `ready-to-transcribe`, re-queued (FR-028). `ready-to-transcribe` at start → queued (FR-007).

## Track and TrackMeta

| Field | Type | Rule |
|---|---|---|
| `track` | `mic` \| `system` | Never mixed (FR-010). |
| `file` | `mic.opus` / `system.opus` | Ogg Opus, mono, 16 kHz (contracts/core-call.md). |
| `rate` | `16000` | — |
| `start_offset_ms` | `u64` | The track's first sample relative to `started_at` (0 unless a track opened later). |
| `endpoint` | `EndpointId` at start | Never a friendly name (FR-042). |
| `speaker` | `me` (mic) / `s1` (system) | ADR-003. |

## DeviceChange

| Field | Type | Rule |
|---|---|---|
| `at_ms` | `u64` | Relative to `started_at`. |
| `track` | `Track` | — |
| `cause` | `default_output_changed` / `mic_lost` / `mic_default_changed` / `track_lost` / `reopened` / `no_device` | — |
| `to` | `Option<EndpointId>` | `None` while the track runs as silence. |
| `gap_ms` | `u64` | Silence inserted by the timeline (≤ 2 s on a successful reopen, FR-012). |

## Timeline (core, per track, in memory)

| Field | Type | Rule |
|---|---|---|
| `written` | `u64` samples at 16 kHz | Monotonic; equals the track position. |
| `expected` | from `(stamp − call_start) × 16 kHz` | R2/R3. |
| `ratio_adj` | `f64` | Clamped to ±500 ppm. |
| `reanchor_threshold` | 40 ms | Above it, zeros are inserted or overlap is dropped at once. |

Invariant: after any sequence of blocks and gaps, `|written_mic − written_system|` measured at the same stamp is ≤ 20 ms (tested, FR-011 allows 100 ms).

## Chunk (final pass, in memory only)

| Field | Type | Rule |
|---|---|---|
| `track` | `Track` | — |
| `index` | `u32` | Order within the track. |
| `start_ms`, `end_ms` | `u64` | Track-relative. `end − start` ≤ 120 000 ms in timed mode and ≤ 30 000 ms in time-less mode (FR-023, FR-024). |
| `mode` | `timed` \| `timeless` | — |
| `audio` | 16 kHz mono PCM16 WAV bytes | Built just before the upload and dropped after it. Never written to disk (FR-018). |

## Segment (line)

As in §7 schema 1: `start_ms`, `end_ms` (relative to `started_at` = track `start_offset_ms` + chunk `start_ms` + engine segment time, clamped to the chunk), `track`, `speaker`, `lang` (`LangCode` or `null`, FR-025), `text` (non-empty after trim; an empty engine segment is dropped), `source: "final"`. `hidden` and `confidence` are absent in 1a. In time-less mode a line spans its chunk's start and end (FR-024).

## Speaker

| Field | Type | Rule |
|---|---|---|
| `id` | `me` \| `s1` | 1a: exactly these two. |
| `name` | `String` | Default "Me"/"Other party" (EN) or "Я"/"Собеседник" (RU) at call creation. Renaming refuses empty or whitespace-only names; duplicates are allowed (FR-035). The name is stored in `meta.json` and re-rendered into `transcript.*`. |

## FinalPass and the queue (core)

| Field | Type | Rule |
|---|---|---|
| `queue` | FIFO of `(root, CallId, TrackSet)` | One pass at a time (FR-028). New calls are appended at stop. At start, `ready-to-transcribe` calls are enqueued oldest first. |
| `in_flight` | ≤ 3 chunk requests | Decided (#187): at most 3 requests in flight; the decoder and the chunker still hold one chunk at a time (FR-018 applies to decoding). |
| `engine_mode` | `timed` → `timeless` (one way, per pass) | FR-024. |
| `cancel` | token | Deleting a transcribing call cancels its pass first (FR-037). |

`TrackResult` = `pending` \| `done{segments}` \| `failed{reason}`. Done segments of a track are kept in `meta.json` under `results.<track>` (written with temp + rename like every `meta.json` change; no extra file beyond the §7 layout), so "Transcribe again" on a failed call redoes only the failed track (FR-027). On a ready call, "Transcribe again" clears both.

## StorageRoot

| Field | Type | Rule |
|---|---|---|
| `current` | `PathBuf` | `calls.storage_root` or `AppPaths::data_dir()/calls`. |
| `earlier` | `Vec<PathBuf>` | Appended when the root changes; no duplicates. |
| `available` | derived at listing | An unavailable root shows one line naming it. |
| `default` | `bool` | Only the default root may be deleted by the uninstaller (FR-041). |

## CallSettings (the `calls` section of `settings.json`)

| Field | Type | Default | Rule |
|---|---|---|---|
| `hotkey` | `Hotkey` | Ctrl+Alt+R | Refused if equal to the dictation hotkey, from either side (FR-004). Registered through the two-step prepare/commit/abort in a second slot. A conflict marks the field (`FieldId::CallHotkey`) and never sets the tray hotkey-error state. |
| `last_combination` | `Toggles` | `{record}` | — |
| `reminder` | `bool` | `true` | FR-005. |
| `primary_language` | `LangCode` | the UI language | Stored only (FR-038). |
| `storage_root` | `Option<PathBuf>` | `None` | Writable check at save (FR-039). |
| `earlier_roots` | `Vec<PathBuf>` | `[]` | — |
| `warned_hosts` | `Vec<String>` (hosts) | `[]` | Hosts whose provider notice was shown once (FR-031). A host is never logged. |
| `indicator_position` | `Option<{x, y}>` | `None` | Clamped to a visible monitor at show. |

The settings `SCHEMA_VERSION` stays 1. The section is added with container-level `#[serde(default)]` from `settings::defaults`, so an older file loads with defaults (settings.md). New `FieldId`s: `CallHotkey`, `CallReminder`, `PrimaryLanguage`, `StorageRoot`. New `SettingsTab::Calls`.

## CallWarning and CallNotice

| Kind | Trigger | Where |
|---|---|---|
| `communications_device_differs` | At start, eRender/eCommunications ≠ eConsole (FR-010) | indicator + toast |
| `track_lost` (track) | Lost and not reopened within 2 s (FR-012) | indicator + toast |
| `mic_changed` | Mic fell back to the default input (FR-012, FR-002) | indicator + toast |
| `cant_hear_other_party` / `cant_hear_you` | > 60 s rule (FR-013) | indicator + toast |
| `low_disk` | < 1 GB (FR-018) | indicator + toast |
| `call_saved_transcribing`, `transcript_ready{call}`, `pass_failed{reason}` | FR-030 | toast |
| `stopped{reason}` | sleep, 4 h, < 200 MB, storage error (FR-008, FR-016, FR-018) | toast |
| `provider_notice{host}` | First session on a new cloud host (FR-031) | indicator (`warning.provider_notice`, wire kind `providerNotice`) |

Warnings stay in the indicator's list until dismissed (FR-006); a dismissed kind re-arms on its next trigger.

## Tray state

`TrayState` gains `CallRecording`. The shown state is a pure function with precedence dictation `HotkeyError` > dictation `Recording` > `CallRecording` > `Error` > `Idle` (FR-006), tested over every combination (core `tray::shown(dictation, call_recording)`; no call-side input yields `HotkeyError`, FR-004). `CallRecording` shows the red-dot Recording icon and its own tooltip (`tray.tooltip.call_recording`). The tray menu table, top to bottom (decision #197 D2, D3): submenu "Call" { "Record" (check, checked iff `call_recording`), "Stop" (enabled iff `call_recording`) }, "Calls", "Settings", "Open logs folder", "Exit" last; core `tray::menu(retry_available, call_recording)`.

## Log events

Listed in research R20, with fields in contracts/core-call.md "Log lines". Integers, enums and `EndpointId` only.
