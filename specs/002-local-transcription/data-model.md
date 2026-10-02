# Data Model: Local Transcription

Entities from spec §Key Entities, with fields, validation and state transitions. Rust names are indicative; the contracts in `contracts/` are authoritative for interfaces.

## LocalModelSpec (catalog entry, constant)

| Field | Type | Rule |
|---|---|---|
| `id` | enum `ModelId` { Tiny, Base, Small, MediumQ5_0, LargeV3TurboQ5_0 } | stable string form: `tiny`, `base`, `small`, `medium-q5_0`, `large-v3-turbo-q5_0` (used in settings, IPC, logs) |
| `display_name` | message key | localised by 004 |
| `file_name` | `&'static str` | `ggml-<name>.bin`, never `.en` variants |
| `url` | `&'static str` | Hugging Face `ggerganov/whisper.cpp` at one pinned commit (research R-6) |
| `size_bytes` | `u64` | exact; used for progress total, disk check, load-time size check |
| `sha256` | `[u8; 32]` / 64-hex | pinned; never computed at runtime from upstream |
| `recommended` | `bool` | true only for `small` |

Validation (unit test): exactly 5 entries; unique ids and file names; exactly one recommended; all URLs share the pinned commit.

## LocalModelState (per model, runtime)

```text
NotDownloaded ──download──▶ Downloading{received,total}
Downloading ──hash ok, renamed──▶ Downloaded{path}
Downloading ──error / no data 30 s / connect 5 s / bad size / bad hash──▶ Failed{reason}   (.part deleted)
Downloading ──cancel──▶ NotDownloaded                                                   (.part deleted)
Failed ──retry (= download)──▶ Downloading
Downloaded ──delete ok──▶ NotDownloaded
Downloaded ──delete refused (in use) / remove error──▶ Downloaded   (error reported)
(start) ──final file exists with catalog size──▶ Downloaded; else NotDownloaded; *.part deleted
```

- `Failed` is what spec FR-004 calls "shown as not downloaded with the reason": the UI renders it as not downloaded, with the reason and a Retry action; it is never selectable.
- Persisted only as files on disk. `Downloading` and `Failed` live in memory and are lost on restart (a restart shows `NotDownloaded`; spec FR-008).
- `Failed.reason`: `FailureReason` code + message key (001's type, extended with `download_interrupted`, `checksum_mismatch`, `not_enough_disk_space{needed}`, `source_unreachable{host}`, `disk_error`, `http_status{code}`).
- A file with the final name but the wrong size is treated as not downloaded (spec edge case) and is not deleted automatically (the user can re-download, which overwrites it via rename).

## Download (runtime, at most one)

| Field | Type | Rule |
|---|---|---|
| `model` | `ModelId` | |
| `received` | `u64` | monotonic |
| `total` | `u64` | catalog size |
| `cancel` | cancellation token | |
| `last_data_at` | instant | no-data timeout 30 s (`timeouts::DOWNLOAD_NO_DATA`) |

## LoadedModel / ModelResidency (runtime, at most one loaded)

| Field | Type | Rule |
|---|---|---|
| `model` | `ModelId` | must equal the selected model; otherwise unloaded at once |
| `handle` | `Box<dyn SpeechModel>` | dropping it frees the memory |
| `loaded_at`, `load_ms` | instant, u32 | logged |
| `active` | count of activity guards | > 0 holds the countdown |
| `idle_deadline` | `Option<Instant>` | `last activity end + 10 min` (`IDLE_UNLOAD = 600 s`) |

```text
Empty ──prewarm(selected) on recording start──▶ Loading(model)
Loading ──ok──▶ Loaded(model)            (deadline = now + 10 min if no activity)
Loading ──error──▶ Empty                 (waiting transcription fails, spec FR-012)
Loaded ──acquire──▶ Loaded(active+1)     (deadline cleared)
Loaded(active→0) ──▶ Loaded              (deadline = now + 10 min)
Loaded ──tick, now ≥ deadline, active=0──▶ Empty   (cause idle)
Loaded|Loading ──engine≠builtin / other model selected / delete of this model──▶ Empty (cause …)
```

Rule: never `prewarm` at app start; deletion of the loaded model while `active > 0` is refused (`ModelInUse`).

## LocalServerConfig (part of Settings, owned by 004)

| Field | Type | Rule |
|---|---|---|
| `base_url` | URL string | required when engine = local server; valid http/https URL (004 validates, req FR-13) |
| `model` | `Option<String>` | trimmed; empty → `None` → not sent |
| `key` | none in settings | the key is read through 004's `CredentialStore` with `KeySlot::LocalServer` (target `Voicen/local-server`) as `Option<Secret>`; the value is never in settings |

Timeouts are not settings: connect 5 s, transcription 60 s from the shared timeouts module.

## EngineChoice (part of Settings, owned by 004; consumed here)

`None | Api | Builtin{model: Option<ModelId>} | LocalServer`. Deleting the selected built-in model sets it to `None` and persists the settings (spec FR-022).

## DictationLogRecord extension (001's record)

Adds `local: Option<Cold|Warm>` and `load_ms: Option<u32>` for the built-in engine. No text, audio or keys.
