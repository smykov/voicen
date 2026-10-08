# Contract: core interfaces (`crates/voicen-core`)

Rust signatures are indicative; names and semantics are the contract. Types are defined in [data-model.md](../data-model.md). All traits are `Send + Sync`; fakes for every trait are public in `voicen-core` behind the cargo feature `test-fakes` (decisions #23 N4) and are used by the Linux gate; the crate enables the feature for its own tests through a self dev-dependency, and other crates' tests enable it explicitly. It is never on in a release build.

## Owned by 004

004 defines in `voicen_core` every trait and data type shared with 001–003 (decisions #21): `secrets` (`KeySlot`, `Secret`, `KeyEdit`, `KeyPresence`, `CredentialStore`), `settings` (`Settings`, `FieldId`, `defaults()`, `SettingsService`), `HotkeyRegistrar` and `DownloadedModels` (both with fakes) and the data type `post_process::settings::{PostProcessingSettings, STARTER_PROMPT, defaults()}`. 001, 002 and 003 implement or read them; none declares its own key or settings port.

Split (decisions #23): **T-003** built the types and pure rules — modules `secrets`, `settings` (`mod`, `url`, `validate`, `gate`, `hotkey`), `post_process::settings`, `hotkey_registrar`, `models` (all at `voicen_core::…`). **T-032** built `SettingsFile` (`settings/file.rs`), `SettingsService` with `SettingsDeps`, `SaveRequest`/`SaveOutcome`/`FormError`/`SettingsView`, `subscribe` and the core part of live apply (`settings/service.rs`), and the wall clock `clock::Clock` (crate root, `clock.rs`, shared with 005, P-011). **T-014** built `Autostart`, the autostart save step and `reconcile_autostart` (surface below). **T-015** built `is_insecure_remote` and the save warnings (surface below). **T-055** built the hotkey save step (below) and the shell's registrar (`src-tauri` `win::hotkey::HotkeyRegistrarHandle`). **T-046** built the connection test (`connection_test`, `SettingsService::test_connection`; surface below). Not yet in code: the `log` dep.

Built surface (T-003): `secrets::{KeySlot (all(), target_name()), Secret (new, expose), KeyEdit, KeyEdits (get(slot)), KeyPresence (get(slot)), CredentialStore, CredentialError { os_code }}`; `settings::{Settings, defaults, WHISPER_ISO_639_1, FieldId (as_str), ErrorCode (as_str), FieldError, LoadOutcome, EngineKind, Mode}`; `settings::url::{check_base_url, NormalizedUrl, UrlError}`; `settings::validate::{validate, KeyEditsWithPresence}`; `settings::gate::{dictation_gate, blocked_actions, startup_action, startup_action_after_hotkey (T-055), Blocked, ShellAction, StartupAction, SettingsTab}`; `settings::hotkey::{Hotkey, HotkeyKey, HotkeyError, parse_hotkey}`; `hotkey_registrar::{HotkeyRegistrar, Prepared, Unavailable}`; `models::DownloadedModels`.

Built surface (T-032): `settings::file::{SettingsFile, FsSettingsFile (new(dir)), SETTINGS_FILE}`; `settings::service::{SettingsService (load_or_init, snapshot, subscribe, view, save), SettingsDeps { file, credentials, hotkeys, local_models, clock }, SaveRequest { settings, keys: KeyEdits }, SaveOutcome { Saved { view, warnings }, Refused { errors, form_error: Option<FormError> } }, FormError (message_id()), SettingsView, Warning { field, code: WarningCode::EndpointInsecure }}`; `settings::url::normalize_base_url`; `clock::{Clock, SystemClock, utc_compact}`; fakes `settings::file::{FakeSettingsFile, FileCall}`, `clock::FakeClock` and `test_support::TempDir` behind `test-fakes`.

Built surface (T-015, decision #52): `settings::url::is_insecure_remote`; `settings::service::save_warnings(&Settings) -> Vec<Warning>` (the save's commit step fills `SaveOutcome::Saved.warnings` from it); `WarningCode::message_id()` (`settings.warning.endpoint_insecure`); `Warning` serializes as `{field, code, message}` (custom `Serialize`, the `FormError` pattern).

Built surface (T-030): the IPC wire form as serde impls ([ipc.md › Wire form](ipc.md)) — `Deserialize` for `secrets::{Secret, KeyEdit, KeyEdits}` and `settings::service::SaveRequest` (every error a fixed text that never quotes the input); `Serialize` for `SaveOutcome` (externally tagged), `FormError` (`{kind, message, not_restored?}`), `Warning`, `WarningCode (as_str)`, `FieldError`, `FieldId` and `ErrorCode` (as their `as_str()`), `i18n::MessageId` (its id string). `Secret` still has no `Serialize`. Shell (`src-tauri`, Windows only): `credentials::WinCredentialStore` (the `CredentialStore` impl below), `paths::{data_dir, log_dir}`, `locale::{os_language, first_language_tag}`, `settings_ipc::{load_settings, spawn_change_bridge, settings_get, settings_save, settings_speech_languages, SETTINGS_CHANGED}`, `build_app(builder, context, service)` (the one wiring `run()` and the tests share).

Built surface (T-014): `autostart::{Autostart, AutostartError { os_code }, ReconcileAction (as_str())}` with fake `autostart::{FakeAutostart, AutostartCall}` behind `test-fakes`; `SettingsDeps.autostart`; the autostart step of `save` and its undo; `SettingsService::reconcile_autostart`. Shell (Windows only): `autostart::{WinAutostart (new, with_value_name), RUN_VALUE_NAME, RUN_SUBKEY, AUTOSTART_ARG, launched_by_autostart}`; `settings_ipc::load_settings(data_dir, credentials, autostart, os_language)` calls `reconcile_autostart` right after `load_or_init`.

### `CredentialStore` (req NFR-04; spec FR-014–FR-016)

```rust
pub trait CredentialStore: Send + Sync {
    fn read(&self, slot: KeySlot) -> Result<Option<Secret>, CredentialError>;
    fn write(&self, slot: KeySlot, secret: &Secret) -> Result<(), CredentialError>;
    fn delete(&self, slot: KeySlot) -> Result<(), CredentialError>; // absent = Ok
}
```
- Windows impl in `src-tauri` (Credential Manager, research R-1) — the only one (tasks T016; decisions #21). Built by T-030 as `credentials::WinCredentialStore`: generic credential, `CRED_PERSIST_LOCAL_MACHINE`, user name `voicen`, the key's UTF-8 bytes as the blob; absent target → `read` `Ok(None)`, `delete` `Ok(())`; a blob that is not UTF-8 → `CredentialError { os_code: 13 }` (`ERROR_INVALID_DATA`); other failures carry the Win32 code. Target = prefix + `target_name()`: `new()` (release) has an empty prefix, `with_target_prefix(p)` exists for tests that must not touch the user's entries; both run the same three calls. Used by 001 (`KeySlot::TranscriptionApi`), 002 (`KeySlot::LocalServer`) and 003 (`KeySlot::PostProcessing`) to read keys; only `SettingsService` writes or deletes.
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
- Real impl `FsSettingsFile::new(dir)` over a directory with `std::fs` (platform-independent, lives in core); the shell passes the data directory (T-030). `SETTINGS_FILE = "settings.json"` is the only place the name exists.
- `read`: first removes a leftover `settings.json.tmp` (best effort; a `.tmp` is never parsed); `NotFound` (no file or no directory) → `Ok(None)`; any other error → `Err` (a directory at `settings.json` included).
- `write_atomic`: `create_dir_all(dir)`, write and `sync_all` `settings.json.tmp`, rename it over `settings.json`; on failure the `.tmp` is removed (best effort) and `settings.json` keeps its old bytes. No directory fsync (R-2 does not ask for one).
- `move_aside(suffix)`: tries `settings.json.bad-<suffix>`, then `-1` … `-9`. Each name is reserved with `OpenOptions::create_new` (atomic, fails with `AlreadyExists`; `fs::rename` alone would replace an earlier backup), then `settings.json` is renamed over the empty reservation. A failed rename removes the reservation and returns `Err` with `settings.json` in place; all ten names taken → `Err(AlreadyExists)`. The service passes `utc_compact(clock.now())` (`yyyyMMdd-HHmmss`, UTC).
- Fake `FakeSettingsFile` (feature `test-fakes`): bytes in memory, call log (`FileCall::{Read, WriteAtomic, MoveAside(suffix)}`), injectable `read`/`write_atomic`/`move_aside` failures of any `io::ErrorKind`, the backups made.

### `Clock` (P-011)

```rust
pub trait Clock: Send + Sync { fn now(&self) -> SystemTime; }   // voicen_core::clock
pub fn utc_compact(t: SystemTime) -> String;                     // yyyyMMdd-HHmmss in UTC; before the epoch → the epoch; std only
```
- `SystemClock` is the real one; `FakeClock::at(t)` / `set(t)` behind `test-fakes`. The one wall-clock port of the core: the backup suffix and history (005) read the time through it. The connection test does not: its latency is a duration, measured with `std::time::Instant` (T-046).

### `Autostart` (req FR-19; spec FR-019)

```rust
pub trait Autostart: Send + Sync {
    fn is_enabled(&self) -> Result<bool, AutostartError>;
    fn set(&self, enabled: bool) -> Result<(), AutostartError>; // idempotent
}
pub struct AutostartError { pub os_code: i32 }                    // never the exe path (it holds the Windows user name)
pub enum ReconcileAction { None, Written, Removed, Failed }       // as_str(): none | written | removed | failed (R-11 log)
```
- `is_enabled`: the entry exists, whatever its data. `set(true)` always writes the current command (idempotent, fixes a stale path); `set(false)` removes it, an absent entry is `Ok`.
- Windows impl in `src-tauri` (T-014): `autostart::WinAutostart`, the only code that writes or removes the `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value. Value name `RUN_VALUE_NAME = "Voicen"` (the one constant; 006's uninstaller hook deletes the same literal), data `REG_SZ` `"<current exe>" --autostart` (UTF-16 with the terminating NUL counted). Calls: `RegSetKeyValueW`, `RegDeleteKeyValueW` (`ERROR_FILE_NOT_FOUND` → `Ok`), `RegGetValueW` without a data buffer for `is_enabled`. Any failure → `AutostartError` with the Win32 code. `with_value_name(n)` exists for tests that must not touch the user's value. `launched_by_autostart(args)` = `--autostart` (`AUTOSTART_ARG`) is one of the arguments, exactly; `settings_window::on_ready` (T-037) passes it to `startup_action`.
- Fake `FakeAutostart` (feature `test-fakes`): in-memory state (`new()` off, `enabled()` on), call log `AutostartCall::{IsEnabled, Set(bool)}` (failed calls included), `fail_set(value, err)` per value and `fail_is_enabled(err)`; a failed `set` leaves the state unchanged.
- Only `SettingsService` calls it: the save step and `reconcile_autostart`, both under the save lock.

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
- `SettingsDeps { file, credentials, autostart, hotkeys: Arc<dyn HotkeyRegistrar>, local_models: Arc<dyn DownloadedModels>, clock, log }`. Built by T-032; `autostart` added by T-014; `log` not yet (T-008); `hotkeys` is called by the hotkey save step since T-055.
- `load_or_init`: `read` `Err` → `Unavailable`; `None` → `FirstRun` (defaults written); parses → `Loaded` (not validated, not rewritten); does not parse → `move_aside(utc_compact(clock.now()))`: `Ok(name)` → `Reset { backup_file_name: name }` (defaults written), `Err` → `Unavailable`. A failed defaults write keeps `FirstRun`/`Reset`; the next save writes the whole file (decisions #23 N5). No branch calls the credential store.
- `save` order: `Unavailable` → `Refused { [], SettingsUnavailable }` with no dependency called; normalize; read the three key slots (a read error → `key.store_failed` on that slot); `validate` on the raw key edits; hotkey step (T-055: only when the hotkey text differs from the snapshot in force, `prepare(parsed hotkey, draft mode)`; `Unavailable` → `Refused { [recording.hotkey: hotkey.unavailable], None }` with nothing else touched; every later refusal calls `abort` after its undo); autostart step, only when `start_with_windows` differs from the snapshot in force (`set(new)`; a failure → `Refused { [general.start_with_windows: autostart.failed], None }` with no key or file touched); keys in `KeySlot::all()` order (an empty-after-trim `Replace` is `Untouched`, #30; a non-empty one is stored trimmed, #33(a)); `write_atomic`; commit (`HotkeyRegistrar::commit` of a prepared hotkey first, then swap the `Arc`, send it to every live subscriber, prune dropped ones). A failure undoes the completed steps in reverse — keys in reverse slot order, then autostart (`set(old)`) — and continues after an undo error; a failed undo gives `FormError::PartiallyRestored { not_restored }` naming exactly the fields that differ, in step order (`general.start_with_windows`, then the key fields in slot order). One lock serializes saves.
- `reconcile_autostart` (R-5), under the save lock, never touching the file or the credential store: `Unavailable` → `None` with no `Autostart` call (#19 by analogy); snapshot on → `set(true)` → `Written`; snapshot off → `is_enabled()`: absent → `None`, present → `set(false)` → `Removed`; any error → `Failed`. `FirstRun` / `Reset` reconcile from the defaults (off), so a leftover entry is removed. The shell calls it once, inside `load_settings` (J4). Known limit (R-5): a value disabled in Task Manager (`StartupApproved\Run`) is not overridden.
- `view`: `first_run` / `reset_notice` reflect the load outcome for the life of the service; `keys` comes from three credential reads (a read error counts as absent), except while `Unavailable`, when every key is reported absent and the store is not called (decisions #33(b)). After `Saved`, the view's presence comes from the reads and the applied edits, with no extra call.
- `subscribe`: unbounded std channel; no initial value (call `subscribe` then `snapshot`); exactly one message per `Saved`, after the file is written; none on `Refused`. `SettingsService: Send + Sync`.
- Invariant: after `save` returns `Refused`, `snapshot()`, the file, the registered hotkey, the autostart entry and every key slot equal their values before the call (double-failure exception in research R-3).
- Autostart: the autostart step and `reconcile_autostart` were added by T-014 (US5, tasks T056–T058).
- Invariant: `Saved` is returned only after the new hotkey is registered, the old released, when `start_with_windows` changes `Autostart::set(new)` has succeeded (an unchanged value is not re-applied by save; `reconcile_autostart` at start brings the Run value in line, a failure there is logged by T-008), keys are stored and the file is written.

### Pure functions

```rust
pub fn defaults(os_language: Option<&str>) -> Settings;                 // the one source of defaults
pub fn validate(s: &Settings, keys: &KeyEditsWithPresence<'_>, models: &dyn DownloadedModels) -> Vec<FieldError>;   // settings::validate; KeyEditsWithPresence { edits: &KeyEdits, presence: KeyPresence }
pub fn check_base_url(raw: &str) -> Result<NormalizedUrl, UrlError>;   // UrlError: Empty (required), Malformed (url.malformed), Credentials (url.credentials, userinfo; decision #27(2))
pub fn normalize_base_url(raw: &str) -> &str;                           // settings::url; trim + strip one trailing `/`; the storage form of every base URL, valid or not; check_base_url uses it
pub fn is_insecure_remote(url: &NormalizedUrl) -> bool;                // settings::url; re-parses with the `url` crate: true exactly when the scheme is http and the host is not loopback (Domain "localhost" after the crate's lower-casing, Ipv4 in 127.0.0.0/8, Ipv6 == ::1); https never; decision #52
pub fn save_warnings(s: &Settings) -> Vec<Warning>;                     // settings::service; one endpoint.insecure per base URL in use (selected engine's api/local_server URL; post_processing.base_url while enabled) that passes check_base_url and is_insecure_remote; a URL failing check_base_url gets none; never blocks a save
pub const WHISPER_ISO_639_1: [&str; 97];                                  // settings::; the one language list (Whisper LANGUAGES minus jw, haw, yue); validate() gives language.unsupported for anything else
pub fn resolve_ui_language(os_tag: Option<&str>) -> UiLanguage;   // lives in i18n.rs (teamwright T-005); defaults() calls it; the only place an OS tag becomes a language (the UI never derives one)
pub fn dictation_gate(s: &Settings) -> Result<(), Blocked>;            // Blocked::NoEngine
pub fn blocked_actions(b: Blocked) -> Vec<ShellAction>;                 // [Notify(choose_engine), OpenSettings(Engine)]
pub fn startup_action(o: &LoadOutcome, launched_by_autostart: bool) -> StartupAction; // OpenSettings(Engine) | TrayOnly
pub fn parse_hotkey(s: &str) -> Result<Hotkey, HotkeyError>;           // settings::hotkey; canonical text only, round-trips; HotkeyError → hotkey.* code
```

### Connection test (req FR-14; spec FR-017, FR-018; T-046)

```rust
impl SettingsService {
    pub fn test_connection(&self, req: ConnectionTestRequest) -> ConnectionTestResult; // blocking (#42): the shell calls it on spawn_blocking
}
pub fn request_timeouts(req: &ConnectionTestRequest) -> Timeouts;   // connection_test::; Timeouts::from_settings(&req.timeouts)
impl LogEvent { pub fn settings_test_connection(r: &ConnectionTestResult) -> LogEvent; } // R-11 line, no host
```
- Types in `voicen_core::connection_test`: `ConnectionTestRequest`, `TestEngine`, `ConnectionTestResult` (data-model.md). Synchronous like the rest of the core (#42); no separate tester type: the service owns the snapshot, the credential store and the `Unavailable` rule.
- One request at most, through the dictation path: `engine_for` over the snapshot overlaid by the form (engine, base URL, model, timeouts; then the save's `normalize`) -> `OpenAiCompatibleEngine::transcribe` with the bundled clip, no language, no VAD, the form's timeouts (`request_timeouts`).
- Before any key read: the save's `validate`, kept to the fields the test uses (selected engine's URL, model, key; `timeouts.connect` and the engine's request limit) -> `Invalid`, nothing sent. Key: the save's key rule (typed key trimmed; blank `Replace` or `Untouched` -> one read of the selected slot, none while `Unavailable`; `Clear` -> none); a read error -> `KeyStoreUnavailable`, nothing sent; no key where one is required -> `Invalid` with `key.required`.
- Writes nothing: no save, no file, no snapshot change, no credential write or delete (the engine sees the key through a read-only view).
- Mapping from 001's `FailureReason`: `InvalidApiKey` -> `InvalidKey`; `NetworkUnavailable` (DNS) and `CannotReach` -> `CannotReach { host }` (the base URL's `host[:port]`); `Timeout` -> `Timeout`; `ServerError` -> `Http`; `UnexpectedResponse` -> `UnexpectedResponse`; a 2xx transcription (even empty) -> `Ok { latency_ms }` (`Instant`).

### `i18n`

Delivered by the catalog task (teamwright T-005), not by the settings core task; 004 consumes it (`defaults()` calls `resolve_ui_language`). Decisions #21.

```rust
pub struct MessageId(&'static str);   // private field; constants only come from the messages! macro
pub fn text(lang: UiLanguage, id: MessageId, args: &[(&str, &str)]) -> String;  // embedded catalog
pub const MESSAGE_IDS: &[MessageId];  // generated by messages! from the same declaration as the constants
pub(crate) struct Catalog;            // crate-private: Catalog::from_json(en, ru), text(lang, id: &str, args); test-only (#[cfg(test)]): parity_problems(), missing_ids(&[MessageId])
```
- The embedded catalog (`include_str!` of `i18n/{en,ru}.json`) is reached only through `text(lang, MessageId, args)`. The accessor `embedded()` is private to the module and `Catalog` is `pub(crate)` (T-031), so no other crate can call the `&str` `Catalog::text` or build a catalog of its own. The checkers `parity_problems` and `missing_ids` exist only in test builds; the conformance and parity tests live in the module. `catalog_parity_holds_for_the_real_catalogs` pins that the real files parse. If parsing failed anyway, the catalog would be empty and every message would render as its id, without a panic; that fallback itself is not tested.
- `text(lang, MessageId, args)` replaces the earlier `id: &str` signature (T-005, P-014): a Rust id exists only as a `MessageId` constant declared with `messages!`, so every id is in `MESSAGE_IDS` by construction, and a test checks each entry exists in both catalogs.
- `MessageId` serializes as its id string (`"settings.write_failed"`; T-030, `FormError.message` over IPC).
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
| timeouts module | 001 (FR-24) | `TimeoutRole::bounds` (defaults and ranges, decision #99), `Timeouts::from_settings`; the tester converts the form's `timeouts` and uses `connect` and the API / local-server limit |
| `DownloadedModels { is_downloaded(id), list() }` | defined by 004; implemented by 002 (`ModelStore`) | validation of `builtin_local.model_id` |
| `PostProcessingSettings`, `STARTER_PROMPT`, `defaults()` | data type created by 004 (foundational); 003 adds `validate()` and wiring | embedded in `Settings` |
| history apply on change | 005 | subscribes (std `mpsc` receiver) to `SettingsService::subscribe()` |
| log writer | 006 | lines of research R-11 |
