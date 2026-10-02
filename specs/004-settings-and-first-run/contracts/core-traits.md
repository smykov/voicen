# Contract: core interfaces (`crates/voicen-core`)

Rust signatures are indicative; names and semantics are the contract. Types are defined in [data-model.md](../data-model.md). All traits are `Send + Sync`; fakes for every trait live in the core's test support and are used by the Linux gate.

## Owned by 004

### `CredentialStore` (req NFR-04; spec FR-014–FR-016)

```rust
pub trait CredentialStore: Send + Sync {
    fn read(&self, slot: KeySlot) -> Result<Option<Secret>, CredentialError>;
    fn write(&self, slot: KeySlot, secret: &Secret) -> Result<(), CredentialError>;
    fn delete(&self, slot: KeySlot) -> Result<(), CredentialError>; // absent = Ok
}
```
- Windows impl in `src-tauri` (Credential Manager, research R-1). Used by 001, 002 and 003 to read keys; only `SettingsService` writes or deletes.
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
    pub fn load_or_init(deps: SettingsDeps, os_language: Option<&str>) -> (Self, LoadOutcome);
    pub fn snapshot(&self) -> Arc<Settings>;
    pub fn subscribe(&self) -> watch::Receiver<Arc<Settings>>;
    pub fn view(&self) -> SettingsView;                         // includes KeyPresence
    pub fn save(&self, req: SaveRequest) -> SaveOutcome;        // serialized, all-or-nothing (research R-3)
    pub fn reconcile_autostart(&self) -> ReconcileAction;       // at start (research R-5)
}
```
- `SettingsDeps { file, credentials, autostart, hotkeys: Arc<dyn HotkeyRegistrar>, local_models: Arc<dyn DownloadedModels>, clock, log }`.
- Invariant: after `save` returns `Refused`, `snapshot()`, the file, the registered hotkey, the autostart entry and every key slot equal their values before the call (double-failure exception in research R-3).
- Invariant: `Saved` is returned only after the new hotkey is registered, the old released, the autostart entry matches, keys are stored and the file is written.

### Pure functions

```rust
pub fn defaults(os_language: Option<&str>) -> Settings;                 // the one source of defaults
pub fn validate(s: &Settings, keys: &KeyEditsWithPresence, models: &dyn DownloadedModels) -> Vec<FieldError>;
pub fn check_base_url(raw: &str) -> Result<NormalizedUrl, UrlError>;
pub fn is_insecure_remote(url: &NormalizedUrl) -> bool;
pub fn resolve_ui_language(os_tag: Option<&str>) -> UiLanguage;
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

```rust
pub fn text(lang: UiLanguage, id: &str, args: &[(&str, &str)]) -> String; // missing id → the id itself, and a test fails
pub const MESSAGE_IDS: &[&str];  // ids used by Rust code, checked against both catalogs
```

## Consumed from other features

| Interface | Owner | What 004 needs |
|---|---|---|
| `HotkeyRegistrar { prepare, commit, abort }` | 001 (hotkey) | two-phase replace so a refused save keeps the old hotkey (req FR-05). If 001 defines a different shape, the 004 task adapts to it but keeps the two-phase semantics. |
| `TranscriptionClient` + failure reasons | 001 | one request with a given endpoint, audio and timeout; reasons as in data-model `ConnectionTestResult` |
| timeouts module | 001 (FR-24) | connect 5 s; API 30 s; local server 60 s |
| `DownloadedModels { is_downloaded(id), list() }` | 002 (`ModelStore`) | validation of `builtin_local.model_id` |
| `PostProcessingSettings`, its defaults and `validate` | 003 | embedded in `Settings` |
| history apply on change | 005 | subscribes to `SettingsService::subscribe()` |
| log writer | 006 | lines of research R-11 |
