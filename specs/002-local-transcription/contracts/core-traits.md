# Contract: Core interfaces (voicen-core)

Rust signatures are the contract's shape; exact generic/async spelling follows 001's `Engine` trait once it exists. Types prefixed `001::` are owned by `001-dictation-via-api` and only consumed here. `CredentialStore`, `KeySlot`, `Secret`, `Settings` and `DownloadedModels` are owned by `004-settings-and-first-run` (decisions #21).

## SpeechModel — implemented by `crates/voicen-whisper` (real) and core tests (fake)

```rust
pub trait SpeechModelLoader: Send + Sync {
    /// Blocking; called on a blocking thread. Must not touch the network.
    fn load(&self, path: &Path) -> Result<Box<dyn SpeechModel>, EngineError>;
}

pub trait SpeechModel: Send {
    /// Blocking. `pcm`: 16 kHz mono f32 in [-1, 1]. `language`: None = auto-detect, Some("ru") = fixed.
    fn transcribe(&mut self, pcm: &[f32], language: Option<&str>) -> Result<String, EngineError>;
}
// Dropping the Box frees the model's memory (unload).
```

Guarantees: no panics on bad input (errors are `EngineError::Load(reason)` / `EngineError::Transcribe(reason)`); no network; no logging of text.

## ModelStore

```rust
pub struct ModelStore { /* models dir, catalog */ }
// implements 004's `DownloadedModels { is_downloaded(id), list() }` (trait defined by 004; decisions #21)
impl ModelStore {
    pub fn new(models_dir: PathBuf, catalog: &'static [CatalogEntry]) -> Self; // dir from the shell's single data-dir resolver;
                                                        // catalog = `catalog::MODELS` in production, a test entry in tests (T-016)
    pub fn cleanup_at_start(&self) -> io::Result<()>;   // deletes every *.part (all tried, first error returned); missing dir = Ok
    pub fn states(&self) -> Vec<(ModelId, LocalModelState)>; // disk-derived: Downloaded iff final file with catalog size
    pub fn path_if_downloaded(&self, id: ModelId) -> Option<PathBuf>;
}
```

The store removes nothing but `.part` files: a model is deleted only by `LocalModels::delete` (T-019, below), which can also clear the coordinator's transient `Failed`.

## Downloader

```rust
pub trait DiskSpace: Send + Sync { fn available_bytes(&self, dir: &Path) -> io::Result<u64>; }

pub enum DownloadEvent { Progress { id: ModelId, received: u64, total: u64 },
                         Finished { id: ModelId },
                         Failed { id: ModelId, reason: DownloadFailure },
                         Cancelled { id: ModelId } }

// Not 001's `FailureReason` (decision #49): its `retryable` is transcription semantics.
// Codes (contracts/ipc.md) via `code()`, en/ru text via `message_id()` + `message_params()`
// (`download.*` ids); never carries the URL, a reqwest error or OS text.
pub enum DownloadFailure { DownloadInterrupted,              // download_interrupted (cut body, or any body shorter than the catalog size; drop; no data for download_no_data)
                           ChecksumMismatch,                 // checksum_mismatch (also a body longer than the catalog size)
                           NotEnoughDiskSpace { needed: u64 }, // not_enough_disk_space{needed}
                           SourceUnreachable { host: String }, // source_unreachable{host}: host[:port] only
                           DiskError,                        // disk_error
                           HttpStatus { code: u16 } }        // http_status{code}

pub struct Downloader { /* store, disk probe, timeouts, one active slot */ }
impl Downloader {
    pub fn new(store: Arc<ModelStore>, disk: Arc<dyn DiskSpace>, timeouts: Timeouts) -> Self;
    pub fn start(&self, id: ModelId, events: impl Fn(DownloadEvent) + Send + 'static)
        -> Result<(), DownloadError>;   // DownloadError::{Busy, AlreadyDownloaded, NotEnoughDiskSpace{needed},
                                        //   NotInCatalog, CannotStart}; a refusal sends no request, emits no event
                                        //   and leaves no active slot. NotInCatalog: `id` is not in the store's
                                        //   catalog (never with the production catalog, which lists every ModelId);
                                        //   CannotStart: the OS refused the worker thread. Wire codes: T-044 (ipc.md)
    pub fn cancel(&self, id: ModelId) -> bool;
}
pub struct ProgressThrottle;            // should_emit(now: Instant): first chunk at once, then ≥ 250 ms apart
```

Guarantees: synchronous — the download runs on its own std thread with blocking reqwest, no tokio (decisions #22, #42); the no-data timeout is `ClientBuilder::timeout(download_no_data)` (each body read and the header wait) plus `connect_timeout(connect)`, never a total timeout; progress ≥ 1/s while data arrives, ≤ 4/s; final file appears only after the byte count equals the catalog size and the SHA-256 matches (rename of `<file>.part`); every other end closes `.part` and tries to delete it before its end event (best effort: a failed removal is ignored and the end event is still emitted; the leftover `.part` is never read as a model, the next `start` truncates it and `ModelStore::cleanup_at_start` removes it at the next app start; logging the failed removal waits for T-008), and the active slot is cleared before the end event; a cancel takes effect when the current read returns (≤ `download_no_data`) and ends `Cancelled`, not `Failed`; disk check = catalog size + 1 %, a probe error lets the download proceed; Retry is `start` again.

## LocalModels (coordinator, T-044)

```rust
// voicen_core::local_models::service. The one place the shell talks to; platform-free, proven by the Linux gate.
impl LocalModels {
    pub fn open(models_dir, disk: Arc<dyn DiskSpace>, timeouts, catalog) -> (LocalModels, io::Result<()>); // cleanup_at_start runs before it returns; its result is the 2nd element
    pub fn list(&self) -> Vec<LocalModelView>;          // catalog order; disk state merged with the in-memory Downloading/Failed state
    pub fn download(&self, id: &str, emit: impl Fn(LocalModelEvent) + Send + 'static) -> Result<(), ReasonView>;
    pub fn cancel(&self, id: &str) -> bool;             // unknown id or nothing running: false
    pub fn delete(&self, id: &str, residency: &dyn ModelRelease, settings: &SettingsService /* 004 */)
        -> Result<DeleteOutcome, ReasonView>;           // T-019; DeleteError::{ModelInUse, NotDownloaded, DeleteFailed}
    pub fn store(&self) -> Arc<ModelStore>;             // the one store, also SettingsDeps.local_models
}
pub struct DeleteOutcome { pub engine_reset: bool, pub reset_failed: bool }   // wire { engineReset, resetFailed }

// The port from the delete to whatever keeps a model loaded. Production value `NoResidency`
// (never loaded, never in use) until T-017's ModelResidency implements it.
pub trait ModelRelease {
    fn release_for_delete(&self, id: ModelId) -> Result<ReleaseGuard, InUse>; // unload if loaded; no load of `id` until the guard drops
}
impl ReleaseGuard { pub fn new(held: impl Send + 'static) -> ReleaseGuard; } // drops `held` with the guard

// 004's SettingsService, the narrow reset the delete uses (T-019):
impl SettingsService {
    pub fn forget_model(&self, id: &str) -> Result<ForgetOutcome /* { engine_reset } */, ForgetFailed>;
}
```

`delete` order (T-019): `not_downloaded` unless the final file has the catalog size and no download of `id` runs (the residency is not asked) → `release_for_delete` (`InUse` → `model_in_use`, nothing changed) → remove the final file (`NotFound` counts as removed; any other error → `delete_failed`, settings untouched, the model stays downloaded) → drop the transient state of `id` → `forget_model(id)`; the guard drops after the removal. The transient map stays locked throughout. `forget_model` runs under the settings `save_lock`: while `Unavailable` it calls nothing; if `builtin_local.model_id` is `id` it sets it to `None` and, if the engine is `builtin_local`, the engine to `None` (`engine_reset`), writes the file, swaps and publishes; it reads no key and revalidates nothing (a reset through `save` would be refused by a credential read failure or an invalid hand-edited field). A write failure keeps the snapshot and is reported as `reset_failed`, the removal is not undone (OQ-26 (a)). No event is emitted; the reset reaches the window as `settings://changed`.

Wire types (`LocalModelView`, `ReasonView`, `LocalModelEvent`), codes, message ids and the error-to-code mapping: `contracts/ipc.md` (not repeated here). Invariants: `docs/decisions/model-download.md`.

## ModelResidency

```rust
pub enum Warmth { Cold { load_ms: u32 }, Warm }

impl ModelResidency {
    pub fn new(loader: Arc<dyn SpeechModelLoader>, clock: Arc<dyn Clock>, idle: Duration) -> Self;
    pub fn prewarm(&self, id: ModelId, path: PathBuf);             // recording started with builtin; idempotent
    pub async fn acquire(&self, id: ModelId, path: PathBuf)
        -> Result<(ModelGuard, Warmth), EngineError>;              // waits for an in-flight load
    pub fn recording_started(&self) -> ActivityGuard;              // holds the countdown
    pub fn on_engine_or_model_changed(&self, selected: Option<ModelId>); // unload if different
    // implements `ModelRelease::release_for_delete` (T-019) instead of an `unload_if`
    pub fn tick(&self);                                            // called by the shell's timer; unloads when due
    pub fn loaded(&self) -> Option<ModelId>;
}
```

## BuiltinEngine — implements `001::Engine`

```rust
impl 001::Engine for BuiltinEngine {
    async fn transcribe(&self, audio: 001::Audio16kMono, lang: 001::Language)
        -> Result<001::Transcript, 001::EngineFailure>;
}
```

- Model not selected / not downloaded → `EngineFailure::NoLocalModel` → FR-11 path (paste nothing, notify "no local model" with an "open settings" action, audio kept as the pending recording); no load or transcription is attempted.
- Load/transcribe error → `EngineFailure::Engine(reason)` → FR-11 path.
- Reports `Warmth` into the dictation log record.

## Local server — configuration of 001's `OpenAiCompatibleEngine`

Built only by 001's factory `engine_for`, `EngineKind::LocalServer` arm (T-018; 001 [contracts/core-traits.md](../../001-dictation-via-api/contracts/core-traits.md), decision #44); there is no separate `local_server_endpoint` function:

```rust
// engine_for, LocalServer arm:
check_base_url(&settings.local_server.base_url)          // else EngineNotConfigured, no key read
creds.read(KeySlot::LocalServer)                         // once; Err -> KeyStoreUnavailable
OpenAiCompatibleEngine::local_server(base_url, model /* trimmed; None if empty */, key)
```

What 001's client provides for it: optional model (`None` omits the form field), optional key (none or empty omits `Authorization`), the per-endpoint request deadline (`Timeouts::local_server`, default 60 s, and `Timeouts::connect`, default 5 s; both from the job's `timeouts` settings, decision #99) chosen by the endpoint role, `kind() == "local_server"`, and the "cannot reach <host:port>" reason.

## Timeouts (shared module, owned by 001, extended here)

Implemented as fields of 001's one `voicen_core::timeouts::Timeouts` struct (`Timeouts::default()` is `from_settings` of the default `timeouts` settings), not as consts: `connect`, `local_server`, `download_no_data` (T-016, decision #49). `connect` and `local_server` are settings since T-073 (decision #99); `download_no_data` stays fixed at 30 s and `Downloader`/`LocalModels` keep `Timeouts::default()`. The consts below are the defaults.

```rust
pub const CONNECT: Duration = 5 s;                       // req FR-24 — `Timeouts::connect`
pub const LOCAL_SERVER_TRANSCRIPTION: Duration = 60 s;   // req FR-24 — `Timeouts::local_server`
pub const DOWNLOAD_NO_DATA: Duration = 30 s;             // spec Clarification 5 — `Timeouts::download_no_data` (per read)
pub const MODEL_IDLE_UNLOAD: Duration = 600 s;           // req NFR-03
```
