# Contract: core interfaces (`crates/voicen-core`)

Rust signatures are indicative; names and semantics are the contract. Types are defined in [data-model.md](../data-model.md). All traits are `Send + Sync`; fakes for every trait live in the core's test support and are used by the Linux gate.

## Owned by 004

004 defines in `voicen_core` every trait and data type shared with 001–003 (decisions #21): `secrets` (`KeySlot`, `Secret`, `KeyEdit`, `KeyPresence`, `CredentialStore`), `settings` (`Settings`, `FieldId`, `defaults()`, `SettingsService`), `HotkeyRegistrar` and `DownloadedModels` (both with fakes) and the data type `post_process::settings::{PostProcessingSettings, STARTER_PROMPT, defaults()}`. 001, 002 and 003 implement or read them; none declares its own key or settings port.

### `CredentialStore` (req NFR-04; spec FR-014–FR-016)

```rust
pub trait CredentialStore: Send + Sync {
    fn read(&self, slot: KeySlot) -> Result<Option<Secret>, CredentialError>;
    fn write(&self, slot: KeySlot, secret: &Secret) -> Result<(), CredentialError>;
    fn delete(&self, slot: KeySlot) -> Result<(), CredentialError>; // absent = Ok
}
```
- Windows impl in `src-tauri` (Credential Manager, research R-1) — the only one (tasks T016; decisions #21). Used by 001 (`KeySlot::TranscriptionApi`), 002 (`KeySlot::LocalServer`) and 003 (`KeySlot::PostProcessing`) to read keys; only `SettingsService` writes or deletes.
- `CredentialError` carries an OS error code and no key material. No implementation may fall back to another storage.

### `SettingsFile` (spec FR-009, FR-010)

```rust
pub trait SettingsFile: Send + Sync {
    fn read(&self) -> io::Result<Option<Vec<u8>>>;          // None = no file
    fn write_atomic(&self, bytes: &[u8]) -> io::Result<()>;   // tmp + sync + rename
    fn move_aside(&self, suffix: &str) -> io::Result<String>; // returns the backup file name
}
```
- Real impl over a directory with `std::fs` (platform-independent, lives in core); fake with injectable failures.

### `Autostart` (req FR-19; spec FR-019)

```rust
pub trait Autostart: Send + Sync {
    fn is_enabled(&self) -> Result<bool, AutostartError>;
    fn set(&self, enabled: bool) -> Result<(), AutostartError>; // idempotent
}
```

### `SettingsService` (req FR-13, FR-21)

```rust
impl SettingsService {
    pub fn load_or_init(deps: SettingsDeps, os_language: Option<&str>) -> (Self, LoadOutcome); // see the unreadable-file rule below
    pub fn snapshot(&self) -> Arc<Settings>;
    pub fn subscribe(&self) -> mpsc::Receiver<Arc<Settings>>;  // std; one channel per subscriber, no tokio (decisions #22)
    pub fn view(&self) -> SettingsView;                         // includes KeyPresence
    pub fn save(&self, req: SaveRequest) -> SaveOutcome;        // serialized, all-or-nothing (research R-3)
    pub fn reconcile_autostart(&self) -> ReconcileAction;       // at start (research R-5)
}
```
- `load_or_init` and the file: if the file cannot be moved aside (`move_aside` fails), or a read fails with an I/O error other than not-found, the outcome is `LoadOutcome::Unavailable` — defaults in memory, the file is never written or moved, `save` returns `Refused` with a notice until restart, and the credential store is not touched (spec FR-010, decisions #19).
- `SettingsDeps { file, credentials, autostart, hotkeys: Arc<dyn HotkeyRegistrar>, local_models: Arc<dyn DownloadedModels>, clock, log }`.
- Invariant: after `save` returns `Refused`, `snapshot()`, the file, the registered hotkey, the autostart entry and every key slot equal their values before the call (double-failure exception in research R-3).
- Autostart: the autostart step and `reconcile_autostart` are added by US5 (tasks T056–T058); before that the save has no autostart step.
- Invariant: `Saved` is returned only after the new hotkey is registered, the old released, the autostart entry matches (once US5 lands), keys are stored and the file is written.

### Pure functions

```rust
pub fn defaults(os_language: Option<&str>) -> Settings;                 // the one source of defaults
pub fn validate(s: &Settings, keys: &KeyEditsWithPresence, models: &dyn DownloadedModels) -> Vec<FieldError>;
pub fn check_base_url(raw: &str) -> Result<NormalizedUrl, UrlError>;
pub fn is_insecure_remote(url: &NormalizedUrl) -> bool;
pub fn resolve_ui_language(os_tag: Option<&str>) -> UiLanguage;   // lives in i18n.rs (teamwright T-005); defaults() calls it
pub fn dictation_gate(s: &Settings) -> Result<(), Blocked>;            // Blocked::NoEngine
pub fn blocked_actions(b: Blocked) -> Vec<ShellAction>;                 // [Notify(choose_engine), OpenSettings(Engine)]
pub fn startup_action(o: &LoadOutcome, launched_by_autostart: bool) -> StartupAction; // OpenSettings(Engine) | TrayOnly
pub fn parse_hotkey(s: &str) -> Result<Hotkey, HotkeyError>;           // canonical form round-trips
```

### `ConnectionTester` (req FR-14; spec FR-017, FR-018)

```rust
impl ConnectionTester {
    pub async fn test(&self, req: ConnectionTestRequest) -> ConnectionTestResult;
}
```
- Uses 001's `TranscriptionClient` and the shared timeouts module; reads the stored key only for `KeyEdit::Untouched`; never writes anything; no VAD.

### `i18n`

Delivered by the catalog task (teamwright T-005), not by the settings core task; 004 consumes it (`defaults()` calls `resolve_ui_language`). Decisions #21.

```rust
pub fn text(lang: UiLanguage, id: &str, args: &[(&str, &str)]) -> String; // missing id → the id itself, and a test fails
pub const MESSAGE_IDS: &[&str];  // ids used by Rust code, checked against both catalogs
```

### `HotkeyRegistrar` and `DownloadedModels` (defined by 004, implemented elsewhere)

```rust
pub trait HotkeyRegistrar: Send + Sync { prepare(Hotkey, Mode) -> Result<Prepared, Unavailable>; commit(Prepared); abort(Prepared); }
pub trait DownloadedModels: Send + Sync { fn is_downloaded(&self, id: &str) -> bool; fn list(&self) -> Vec<ModelId>; }
```
- 004 depends only on these traits and their fakes. 001 implements `HotkeyRegistrar`; 002's `ModelStore` implements `DownloadedModels`. Neither implementation is needed to build or test 004's core.

## Consumed from other features

| Interface | Owner | What 004 needs |
|---|---|---|
| `HotkeyRegistrar { prepare, commit, abort }` | defined by 004; implemented by 001 (hotkey) | two-phase replace so a refused save keeps the old hotkey (req FR-05). 001 implements this trait; it does not define another shape. |
| `TranscriptionClient` + failure reasons | 001 | one request with a given endpoint, audio and timeout; reasons as in data-model `ConnectionTestResult` |
| timeouts module | 001 (FR-24) | connect 5 s; API 30 s; local server 60 s |
| `DownloadedModels { is_downloaded(id), list() }` | defined by 004; implemented by 002 (`ModelStore`) | validation of `builtin_local.model_id` |
| `PostProcessingSettings`, `STARTER_PROMPT`, `defaults()` | data type created by 004 (foundational); 003 adds `validate()` and wiring | embedded in `Settings` |
| history apply on change | 005 | subscribes (std `mpsc` receiver) to `SettingsService::subscribe()` |
| log writer | 006 | lines of research R-11 |
