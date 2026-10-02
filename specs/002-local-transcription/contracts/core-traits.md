# Contract: Core interfaces (voicen-core)

Rust signatures are the contract's shape; exact generic/async spelling follows 001's `Engine` trait once it exists. Types prefixed `001::` are owned by `001-dictation-via-api` and only consumed here.

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
impl ModelStore {
    pub fn new(models_dir: PathBuf) -> Self;            // dir from the shell's single data-dir resolver
    pub fn cleanup_at_start(&self) -> io::Result<()>;   // deletes *.part
    pub fn states(&self) -> Vec<(ModelId, LocalModelState)>; // disk-derived: Downloaded iff final file with catalog size
    pub fn path_if_downloaded(&self, id: ModelId) -> Option<PathBuf>;
    pub fn delete(&self, id: ModelId, residency: &ModelResidency, settings: &mut dyn 001::SettingsStore)
        -> Result<(), DeleteError>;                     // DeleteError::{InUse, NotDownloaded, Io(reason)}
}
```

`delete` order: refuse if `residency` is active on `id` → unload if loaded → remove file → if selected, set engine `None` and persist.

## Downloader

```rust
pub trait DiskSpace: Send + Sync { fn available_bytes(&self, dir: &Path) -> io::Result<u64>; }

pub enum DownloadEvent { Progress { id: ModelId, received: u64, total: u64 },
                         Finished { id: ModelId },
                         Failed { id: ModelId, reason: 001::FailureReason },
                         Cancelled { id: ModelId } }

pub struct Downloader { /* http client, timeouts, disk probe, store */ }
impl Downloader {
    pub fn start(&self, id: ModelId, events: impl Fn(DownloadEvent) + Send + 'static)
        -> Result<(), DownloadError>;                   // DownloadError::{Busy, AlreadyDownloaded, NotEnoughDiskSpace{needed}}
    pub fn cancel(&self, id: ModelId) -> bool;
}
```

Guarantees: progress ≥ 1/s while data arrives, ≤ 4/s; final file appears only after size and SHA-256 match (atomic rename); every non-success path deletes `.part`; Retry is `start` again.

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
    pub fn unload_if(&self, id: ModelId) -> Result<(), InUse>;
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

## Local server — configuration of `001::OpenAiCompatClient`

```rust
pub fn local_server_endpoint(cfg: &LocalServerConfig, secrets: &dyn 001::CredentialStore)
    -> 001::Endpoint;   // base_url, model: Option, key: Option, timeouts { connect: 5 s, request: 60 s }
```

Required of 001's client: optional model (omit the form field), optional key (omit `Authorization`), per-endpoint request timeout, "cannot reach <host:port>" reason.

## Timeouts (shared module, owned by 001, extended here)

```rust
pub const CONNECT: Duration = 5 s;                       // req FR-24
pub const LOCAL_SERVER_TRANSCRIPTION: Duration = 60 s;   // req FR-24
pub const DOWNLOAD_NO_DATA: Duration = 30 s;             // spec Clarification 5
pub const MODEL_IDLE_UNLOAD: Duration = 600 s;           // req NFR-03
```
