# Contract: core interfaces (`crates/voicen-core`)

Rust signatures are indicative; names and semantics are the contract. Types are defined in [data-model.md](../data-model.md). All traits are `Send + Sync`; fakes for every trait are public in `voicen-core` behind the cargo feature `test-fakes` (decisions #23 N4) and are used by the Linux gate; the crate enables the feature for its own tests through a self dev-dependency, and other crates' tests enable it explicitly. It is never on in a release build.

## Owned by 004

004 defines in `voicen_core` every trait and data type shared with 001–003 (decisions #21): `secrets` (`KeySlot`, `Secret`, `KeyEdit`, `KeyPresence`, `CredentialStore`), `settings` (`Settings`, `FieldId`, `defaults()`, `SettingsService`), `HotkeyRegistrar` and `DownloadedModels` (both with fakes) and the data type `post_process::settings::{PostProcessingSettings, STARTER_PROMPT, defaults()}`. 001, 002 and 003 implement or read them; none declares its own key or settings port.

Split (decisions #23): **T-003** built the types and pure rules — modules `secrets`, `settings` (`mod`, `url`, `validate`, `gate`, `hotkey`), `post_process::settings`, `hotkey_registrar`, `models` (all at `voicen_core::…`). **T-032** builds `SettingsFile` (`settings/file.rs`) and `SettingsService` with `SettingsDeps`, `Clock`, `SaveRequest`/`SaveOutcome`/`SettingsView`, `subscribe` and live apply (`settings/service.rs`). The sections below on those items are not yet in code.

Built surface (T-003): `secrets::{KeySlot (all(), target_name()), Secret (new, expose), KeyEdit, KeyEdits (get(slot)), KeyPresence (get(slot)), CredentialStore, CredentialError { os_code }}`; `settings::{Settings, defaults, FieldId (as_str), ErrorCode (as_str), FieldError, LoadOutcome, EngineKind, Mode}`; `settings::url::{check_base_url, NormalizedUrl, UrlError}`; `settings::validate::{validate, KeyEditsWithPresence}`; `settings::gate::{dictation_gate, blocked_actions, startup_action, Blocked, ShellAction, StartupAction, SettingsTab}`; `settings::hotkey::{Hotkey, HotkeyKey, HotkeyError, parse_hotkey}`; `hotkey_registrar::{HotkeyRegistrar, Prepared, Unavailable}`; `models::DownloadedModels`.

### `CredentialStore` (req NFR-04; spec FR-014–FR-016)

```rust
pub trait CredentialStore: Send + Sync {
    fn read(&self, slot: KeySlot) -> Result<Option<Secret>, CredentialError>;
    fn write(&self, slot: KeySlot, secret: &Secret) -> Result<(), CredentialError>;
    fn delete(&self, slot: KeySlot) -> Result<(), CredentialError>; // absent = Ok
}
```
- Windows impl in `src-tauri` (Credential Manager, research R-1) — the only one (tasks T016; decisions #21). Used by 001 (`KeySlot::TranscriptionApi`), 002 (`KeySlot::LocalServer`) and 003 (`KeySlot::PostProcessing`) to read keys; only `SettingsService` writes or deletes.
- `KeySlot` targets: `TranscriptionApi` → `Voicen/transcription-api`, `LocalServer` → `Voicen/local-server`, `PostProcessing` → `Voicen/post-processing` (`KeySlot::target_name()`, the only place the strings exist; the Windows impl uses it).
- `Secret`: `Debug` and `Display` print `***`; no `Serialize` (compile-fail doctest in `secrets.rs`); the buffer is zeroed on drop (`zeroize`); `expose()` is only for the credential store and the HTTP client.
- Fake: `secrets::FakeCredentialStore` (feature `test-fakes`) records calls per slot and injects failures per op and slot.
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
pub fn validate(s: &Settings, keys: &KeyEditsWithPresence<'_>, models: &dyn DownloadedModels) -> Vec<FieldError>;   // settings::validate; KeyEditsWithPresence { edits: &KeyEdits, presence: KeyPresence }
pub fn check_base_url(raw: &str) -> Result<NormalizedUrl, UrlError>;
pub fn is_insecure_remote(url: &NormalizedUrl) -> bool;
pub fn resolve_ui_language(os_tag: Option<&str>) -> UiLanguage;   // lives in i18n.rs (teamwright T-005); defaults() calls it; the only place an OS tag becomes a language (the UI never derives one)
pub fn dictation_gate(s: &Settings) -> Result<(), Blocked>;            // Blocked::NoEngine
pub fn blocked_actions(b: Blocked) -> Vec<ShellAction>;                 // [Notify(choose_engine), OpenSettings(Engine)]
pub fn startup_action(o: &LoadOutcome, launched_by_autostart: bool) -> StartupAction; // OpenSettings(Engine) | TrayOnly
pub fn parse_hotkey(s: &str) -> Result<Hotkey, HotkeyError>;           // settings::hotkey; canonical text only, round-trips; HotkeyError → hotkey.* code
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
pub struct MessageId(&'static str);   // private field; constants only come from the messages! macro
pub fn text(lang: UiLanguage, id: MessageId, args: &[(&str, &str)]) -> String;  // embedded catalog
pub const MESSAGE_IDS: &[MessageId];  // generated by messages! from the same declaration as the constants
pub struct Catalog;                   // public for tests and fixtures: Catalog::from_json(en, ru), text(lang, id: &str, args), parity_problems(), missing_ids(&[MessageId])
```
- The embedded catalog (`include_str!` of `i18n/{en,ru}.json`) is reached only through `text(lang, MessageId, args)`; the accessor `embedded()` is private to the module, so the `&str` `Catalog::text` is not reachable on it from outside. `catalog_parity_holds_for_the_real_catalogs` pins that the real files parse. If parsing failed anyway, the catalog would be empty and every message would render as its id, without a panic; that fallback itself is not tested.
- `text(lang, MessageId, args)` replaces the earlier `id: &str` signature (T-005, P-014): a Rust id exists only as a `MessageId` constant declared with `messages!`, so every id is in `MESSAGE_IDS` by construction, and a test checks each entry exists in both catalogs.
- `MessageId` has no `Serialize` yet; the first consumer that sends an id over IPC adds it.
- Lookup order: text of `lang` if non-empty, else the `en` text, else the id itself.
- Placeholder grammar: `{name}` with `name` matching `[a-z][a-z0-9_]*`. Rendering is one left-to-right pass: a known placeholder is replaced by its argument value inserted literally (never re-expanded), a placeholder with no argument stays verbatim, extra arguments are ignored. There is no escaping: any `{` or `}` that is not part of a valid placeholder fails the parity check.
- The UI renders with the same rule (`$lib/i18n`); `i18n/conformance.json` is the shared fixture both test suites run.

### `HotkeyRegistrar` and `DownloadedModels` (defined by 004, implemented elsewhere)

```rust
pub trait HotkeyRegistrar: Send + Sync { prepare(Hotkey, Mode) -> Result<Prepared, Unavailable>; commit(Prepared); abort(Prepared); }
pub trait DownloadedModels: Send + Sync { fn is_downloaded(&self, id: &str) -> bool; fn list(&self) -> Vec<String>; }
```
- Model ids are the stable strings stored in `builtin_local.model_id`; `list()` returns `Vec<String>` (decisions #23 N2), 002 maps its own enum. Fakes: `FakeHotkeyRegistrar` (call log, `fail_prepare`, `active()`) and `FakeDownloadedModels::new(&[ids])`, both behind `test-fakes`.
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
