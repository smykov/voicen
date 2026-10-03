//! The settings service: load at startup, snapshot, view, all-or-nothing save with
//! reverse undo, and live apply through std `mpsc` subscribers (research R-2, R-3,
//! R-4; decisions #19, #22, #23 N5, #30, #33; contracts/core-traits.md#settingsservice).

use std::io;
use std::sync::{mpsc, Arc, Mutex, PoisonError, RwLock};

use serde::Serialize;

use super::file::SettingsFile;
use super::url::normalize_base_url;
use super::validate::{validate, KeyEditsWithPresence};
use super::{defaults, ErrorCode, FieldError, FieldId, LoadOutcome, Settings, SCHEMA_VERSION};
use crate::clock::{utc_compact, Clock};
use crate::hotkey_registrar::HotkeyRegistrar;
use crate::i18n::{
    MessageId, NOTICE_SETTINGS_UNAVAILABLE, SETTINGS_PARTIALLY_RESTORED, SETTINGS_WRITE_FAILED,
};
use crate::models::DownloadedModels;
use crate::secrets::{CredentialStore, KeyEdit, KeyEdits, KeyPresence, KeySlot, Secret};

/// Everything the service talks to.
pub struct SettingsDeps {
    pub file: Arc<dyn SettingsFile>,
    pub credentials: Arc<dyn CredentialStore>,
    /// Held but not called until T-010 adds the hotkey step.
    pub hotkeys: Arc<dyn HotkeyRegistrar>,
    pub local_models: Arc<dyn DownloadedModels>,
    pub clock: Arc<dyn Clock>,
}

/// A save from the settings window: the whole draft plus one edit per key slot.
#[derive(Debug)]
pub struct SaveRequest {
    pub settings: Settings,
    pub keys: KeyEdits,
}

/// What a window receives. Never contains a key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingsView {
    pub settings: Settings,
    pub keys: KeyPresence,
    pub first_run: bool,
    pub reset_notice: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningCode {
    EndpointInsecure,
}

/// A non-blocking remark on a saved field (filled by T-015; always empty here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Warning {
    pub field: FieldId,
    pub code: WarningCode,
}

/// A refusal that is not tied to one field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormError {
    /// The settings file could not be written; everything was restored.
    WriteFailed,
    /// The service is `Unavailable` (decision #19): every save is refused.
    SettingsUnavailable,
    /// An undo failed: exactly these key fields differ from before the save.
    PartiallyRestored { not_restored: Vec<FieldId> },
}

impl FormError {
    pub fn message_id(&self) -> MessageId {
        match self {
            FormError::WriteFailed => SETTINGS_WRITE_FAILED,
            FormError::SettingsUnavailable => NOTICE_SETTINGS_UNAVAILABLE,
            FormError::PartiallyRestored { .. } => SETTINGS_PARTIALLY_RESTORED,
        }
    }
}

// One value per save, never stored in bulk: the size difference does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveOutcome {
    Saved {
        view: SettingsView,
        warnings: Vec<Warning>,
    },
    Refused {
        errors: Vec<FieldError>,
        form_error: Option<FormError>,
    },
}

/// The one reader and writer of `settings.json` and the one writer of keys.
///
/// - `save` is all-or-nothing (R-3): nothing is changed before the service is known
///   to be available and the draft is normalized and valid; keys are changed next,
///   the file last, and a failure undoes the completed key steps in reverse.
/// - A snapshot handed out is never changed: a `Saved` swaps in a new `Arc`.
/// - While `Unavailable` (decision #19) no save, `view` or later call touches the
///   file or the credential store.
pub struct SettingsService {
    current: RwLock<Arc<Settings>>,
    deps: SettingsDeps,
    load: LoadState,
    /// Serializes saves for the whole transaction, publish included.
    save_lock: Mutex<()>,
    subscribers: Mutex<Vec<mpsc::Sender<Arc<Settings>>>>,
}

/// What the load found; fixed for the life of the service.
#[derive(Debug, Clone, Copy)]
struct LoadState {
    unavailable: bool,
    first_run: bool,
    reset_notice: bool,
}

/// What the key step does with one slot once a blank `Replace` is set aside.
enum KeyStep<'a> {
    Keep,
    Store(&'a str),
    Delete,
}

impl SettingsService {
    /// Reads the file once and decides the [`LoadOutcome`]; never calls the
    /// credential store. FirstRun and Reset write the defaults; if that write fails
    /// the outcome stays and the next save writes the file (decision #23 N5).
    pub fn load_or_init(deps: SettingsDeps, os_language: Option<&str>) -> (Self, LoadOutcome) {
        let (outcome, write_defaults) = match deps.file.read() {
            Err(_) => (LoadOutcome::Unavailable(defaults(os_language)), false),
            Ok(None) => (LoadOutcome::FirstRun(defaults(os_language)), true),
            Ok(Some(bytes)) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(settings) => (LoadOutcome::Loaded(settings), false),
                Err(_) => match deps.file.move_aside(&utc_compact(deps.clock.now())) {
                    Ok(backup_file_name) => (
                        LoadOutcome::Reset {
                            settings: defaults(os_language),
                            backup_file_name,
                        },
                        true,
                    ),
                    Err(_) => (LoadOutcome::Unavailable(defaults(os_language)), false),
                },
            },
        };
        let (settings, load) = match &outcome {
            LoadOutcome::Loaded(s) => (s, LoadState::new(false, false, false)),
            LoadOutcome::FirstRun(s) => (s, LoadState::new(false, true, false)),
            LoadOutcome::Reset { settings, .. } => (settings, LoadState::new(false, false, true)),
            LoadOutcome::Unavailable(s) => (s, LoadState::new(true, false, false)),
        };
        if write_defaults {
            // N5: a failure keeps the outcome; every save writes the whole file.
            let _ = encode(settings).and_then(|bytes| deps.file.write_atomic(&bytes));
        }
        let service = SettingsService {
            current: RwLock::new(Arc::new(settings.clone())),
            deps,
            load,
            save_lock: Mutex::new(()),
            subscribers: Mutex::new(Vec::new()),
        };
        (service, outcome)
    }

    /// The settings in force now. The value behind the `Arc` never changes.
    pub fn snapshot(&self) -> Arc<Settings> {
        self.current
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// A channel that receives the new snapshot after each `Saved`, and nothing
    /// else (no initial value: call `subscribe` then `snapshot`). A dropped
    /// receiver is pruned at the next publish.
    pub fn subscribe(&self) -> mpsc::Receiver<Arc<Settings>> {
        let (tx, rx) = mpsc::channel();
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(tx);
        rx
    }

    /// What a window shows. Key presence is read from the credential store (a read
    /// error counts as absent); while `Unavailable` every key is reported absent
    /// and the store is not called (decision #33(b)).
    pub fn view(&self) -> SettingsView {
        let keys = if self.load.unavailable {
            KeyPresence::default()
        } else {
            self.read_presence()
        };
        self.view_with((*self.snapshot()).clone(), keys)
    }

    /// The save transaction (R-3; data-model save state machine).
    pub fn save(&self, req: SaveRequest) -> SaveOutcome {
        // (0) Decision #19: no dependency is called while Unavailable.
        if self.load.unavailable {
            return refused(Vec::new(), Some(FormError::SettingsUnavailable));
        }
        let _guard = self
            .save_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner);

        // (1) Normalize before validate and store.
        let settings = normalize(req.settings);

        // (2) The old keys: kept for the undo, and the presence `validate` needs.
        let mut old: Vec<(KeySlot, Option<Secret>)> = Vec::with_capacity(3);
        for slot in KeySlot::all() {
            match self.deps.credentials.read(slot) {
                Ok(value) => old.push((slot, value)),
                Err(_) => return refused(vec![key_store_failed(slot)], None),
            }
        }
        let presence =
            presence_of(|slot| old.iter().any(|(s, value)| *s == slot && value.is_some()));

        // (3) Validate on the raw edits: a blank API `Replace` is still
        // `key.required` (#27(1)).
        let errors = validate(
            &settings,
            &KeyEditsWithPresence {
                edits: &req.keys,
                presence,
            },
            self.deps.local_models.as_ref(),
        );
        if !errors.is_empty() {
            return refused(errors, None);
        }

        // (4) Hotkey step: none until T-010 (prepare / abort / commit go here).

        // (5) Keys, in slot order; each completed step is kept for the undo.
        let mut done: Vec<(KeySlot, Option<Secret>)> = Vec::new();
        let mut presence_after = presence;
        for (slot, old_value) in old {
            let result = match key_step(&req.keys, slot) {
                KeyStep::Keep => continue,
                KeyStep::Store(key) => self
                    .deps
                    .credentials
                    .write(slot, &Secret::new(key))
                    .map(|()| true),
                KeyStep::Delete => self.deps.credentials.delete(slot).map(|()| false),
            };
            match result {
                Ok(present) => {
                    set_presence(&mut presence_after, slot, present);
                    done.push((slot, old_value));
                }
                Err(_) => {
                    let form_error = self.undo(done).map(partially_restored);
                    return refused(vec![key_store_failed(slot)], form_error);
                }
            }
        }

        // (6) The file: tmp + sync + rename, or the old file stays (I3).
        let written = encode(&settings).and_then(|bytes| self.deps.file.write_atomic(&bytes));
        if written.is_err() {
            let form_error = self
                .undo(done)
                .map_or(FormError::WriteFailed, partially_restored);
            return refused(Vec::new(), Some(form_error));
        }

        // (7) Commit: swap the snapshot, then publish to every live subscriber.
        let snapshot = Arc::new(settings);
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = snapshot.clone();
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|tx| tx.send(snapshot.clone()).is_ok());
        SaveOutcome::Saved {
            view: self.view_with((*snapshot).clone(), presence_after),
            warnings: Vec::new(),
        }
    }

    fn view_with(&self, settings: Settings, keys: KeyPresence) -> SettingsView {
        SettingsView {
            settings,
            keys,
            first_run: self.load.first_run,
            reset_notice: self.load.reset_notice,
        }
    }

    fn read_presence(&self) -> KeyPresence {
        presence_of(|slot| matches!(self.deps.credentials.read(slot), Ok(Some(_))))
    }

    /// Restores the completed key steps in reverse order and goes on after a
    /// failure. `None` = all restored; otherwise the key fields still changed, in
    /// slot order.
    fn undo(&self, done: Vec<(KeySlot, Option<Secret>)>) -> Option<Vec<FieldId>> {
        let mut not_restored = Vec::new();
        for (slot, old_value) in done.into_iter().rev() {
            let restored = match &old_value {
                Some(key) => self.deps.credentials.write(slot, key),
                None => self.deps.credentials.delete(slot),
            };
            if restored.is_err() {
                not_restored.push(key_field(slot));
            }
        }
        if not_restored.is_empty() {
            None
        } else {
            not_restored.reverse();
            Some(not_restored)
        }
    }
}

impl LoadState {
    fn new(unavailable: bool, first_run: bool, reset_notice: bool) -> LoadState {
        LoadState {
            unavailable,
            first_run,
            reset_notice,
        }
    }
}

/// Spec 004 data-model "normalize before store": every base URL through
/// [`normalize_base_url`] and every model name trimmed, selected engine or not
/// and valid or not; `schema_version` set to the current one. The hotkey,
/// `speech_language`, the prompt and the microphone are kept as entered.
fn normalize(mut s: Settings) -> Settings {
    s.schema_version = SCHEMA_VERSION;
    for url in [
        &mut s.api.base_url,
        &mut s.local_server.base_url,
        &mut s.post_processing.base_url,
    ] {
        let normalized = normalize_base_url(url).to_string();
        *url = normalized;
    }
    for model in [
        &mut s.api.model,
        &mut s.local_server.model,
        &mut s.post_processing.model,
    ] {
        let trimmed = model.trim().to_string();
        *model = trimmed;
    }
    s
}

/// The effective edit of a slot: a `Replace` that is empty after `trim()` keeps
/// the stored key (decision #30); any other `Replace` is stored trimmed (#33(a)).
fn key_step(edits: &KeyEdits, slot: KeySlot) -> KeyStep<'_> {
    match edits.get(slot) {
        KeyEdit::Untouched => KeyStep::Keep,
        KeyEdit::Clear => KeyStep::Delete,
        KeyEdit::Replace(key) => match key.expose().trim() {
            "" => KeyStep::Keep,
            trimmed => KeyStep::Store(trimmed),
        },
    }
}

/// The input field of a key slot (`engine.api.key`, ...).
fn key_field(slot: KeySlot) -> FieldId {
    match slot {
        KeySlot::TranscriptionApi => FieldId::EngineApiKey,
        KeySlot::LocalServer => FieldId::EngineLocalServerKey,
        KeySlot::PostProcessing => FieldId::PostProcessingKey,
    }
}

fn key_store_failed(slot: KeySlot) -> FieldError {
    FieldError {
        field: key_field(slot),
        code: ErrorCode::KeyStoreFailed,
    }
}

fn partially_restored(not_restored: Vec<FieldId>) -> FormError {
    FormError::PartiallyRestored { not_restored }
}

fn presence_of(mut has_key: impl FnMut(KeySlot) -> bool) -> KeyPresence {
    KeyPresence {
        transcription_api: has_key(KeySlot::TranscriptionApi),
        local_server: has_key(KeySlot::LocalServer),
        post_processing: has_key(KeySlot::PostProcessing),
    }
}

fn set_presence(presence: &mut KeyPresence, slot: KeySlot, present: bool) {
    match slot {
        KeySlot::TranscriptionApi => presence.transcription_api = present,
        KeySlot::LocalServer => presence.local_server = present,
        KeySlot::PostProcessing => presence.post_processing = present,
    }
}

fn refused(errors: Vec<FieldError>, form_error: Option<FormError>) -> SaveOutcome {
    SaveOutcome::Refused { errors, form_error }
}

/// The file form of the settings (`Settings` has no key field, I1).
fn encode(settings: &Settings) -> io::Result<Vec<u8>> {
    serde_json::to_vec_pretty(settings).map_err(io::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FakeClock;
    use crate::hotkey_registrar::FakeHotkeyRegistrar;
    use crate::i18n::MESSAGE_IDS;
    use crate::models::FakeDownloadedModels;
    use crate::secrets::{
        CredentialCall, CredentialError, CredentialOp, FakeCredentialStore, KeyEdit, KeySlot,
        Secret,
    };
    use crate::settings::file::{FakeSettingsFile, FileCall, FsSettingsFile, SETTINGS_FILE};
    use crate::settings::fixtures::sample;
    use crate::settings::{EngineKind, ErrorCode, Microphone, SCHEMA_VERSION};
    use crate::test_support::TempDir;
    use std::fs;
    use std::io;
    use std::path::Path;
    use std::sync::mpsc::TryRecvError;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const OS: Option<&str> = Some("ru-RU");
    /// FakeClock time of every test: 2024-02-29 23:59:59 UTC.
    const T0_SECS: u64 = 1_709_251_199;
    const SUFFIX: &str = "20240229-235959";
    const BACKUP: &str = "settings.json.bad-20240229-235959";
    /// A fake OS error code (no meaning).
    const STORE_ERR: CredentialError = CredentialError { os_code: 1312 };

    const API: KeySlot = KeySlot::TranscriptionApi;
    const LOCAL: KeySlot = KeySlot::LocalServer;
    const PP: KeySlot = KeySlot::PostProcessing;

    fn t0() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(T0_SECS)
    }

    /// Fakes for every dependency, kept so a test can inspect them.
    struct World {
        file: Arc<FakeSettingsFile>,
        creds: Arc<FakeCredentialStore>,
        hotkeys: Arc<FakeHotkeyRegistrar>,
    }

    impl World {
        fn new(file: FakeSettingsFile, creds: FakeCredentialStore) -> World {
            World {
                file: Arc::new(file),
                creds: Arc::new(creds),
                hotkeys: Arc::new(FakeHotkeyRegistrar::new()),
            }
        }

        fn deps(&self) -> SettingsDeps {
            SettingsDeps {
                file: self.file.clone(),
                credentials: self.creds.clone(),
                hotkeys: self.hotkeys.clone(),
                local_models: Arc::new(FakeDownloadedModels::new(&["base"])),
                clock: Arc::new(FakeClock::at(t0())),
            }
        }

        fn load(&self) -> (SettingsService, LoadOutcome) {
            SettingsService::load_or_init(self.deps(), OS)
        }
    }

    /// The real file over `dir`, with fakes for the rest.
    fn fs_deps(dir: &TempDir, creds: &Arc<FakeCredentialStore>) -> SettingsDeps {
        SettingsDeps {
            file: Arc::new(FsSettingsFile::new(dir.path().to_path_buf())),
            credentials: creds.clone(),
            hotkeys: Arc::new(FakeHotkeyRegistrar::new()),
            local_models: Arc::new(FakeDownloadedModels::new(&["base"])),
            clock: Arc::new(FakeClock::at(t0())),
        }
    }

    fn json(s: &Settings) -> Vec<u8> {
        serde_json::to_vec_pretty(s).expect("settings serialize")
    }

    fn parse(bytes: &[u8]) -> Settings {
        serde_json::from_slice(bytes).unwrap_or_else(|e| {
            panic!(
                "not a settings file ({e}): {}",
                String::from_utf8_lossy(bytes)
            )
        })
    }

    fn replace(key: &str) -> KeyEdit {
        KeyEdit::Replace(Secret::new(key))
    }

    fn req(settings: Settings, keys: KeyEdits) -> SaveRequest {
        SaveRequest { settings, keys }
    }

    fn all_keys(slot_edit: impl Fn() -> KeyEdit) -> KeyEdits {
        KeyEdits {
            transcription_api: slot_edit(),
            local_server: slot_edit(),
            post_processing: slot_edit(),
        }
    }

    fn call(op: CredentialOp, slot: KeySlot) -> CredentialCall {
        CredentialCall { op, slot }
    }

    /// The credential calls that change a slot (every call but `read`).
    fn changes(creds: &FakeCredentialStore) -> Vec<CredentialCall> {
        creds
            .calls()
            .into_iter()
            .filter(|c| c.op != CredentialOp::Read)
            .collect()
    }

    fn stored(creds: &FakeCredentialStore) -> [Option<String>; 3] {
        [creds.stored(API), creds.stored(LOCAL), creds.stored(PP)]
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("list dir")
            .map(|e| {
                e.expect("dir entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    #[track_caller]
    fn expect_saved(outcome: SaveOutcome) -> SettingsView {
        match outcome {
            SaveOutcome::Saved { view, warnings } => {
                assert!(
                    warnings.is_empty(),
                    "no warnings before T-015: {warnings:?}"
                );
                view
            }
            other => panic!("expected Saved, got {other:?}"),
        }
    }

    #[track_caller]
    fn expect_refused(outcome: SaveOutcome) -> (Vec<FieldError>, Option<FormError>) {
        match outcome {
            SaveOutcome::Refused { errors, form_error } => (errors, form_error),
            other => panic!("expected Refused, got {other:?}"),
        }
    }

    fn field_error(field: FieldId, code: ErrorCode) -> FieldError {
        FieldError { field, code }
    }

    // ---- load_or_init -------------------------------------------------------

    #[test]
    fn first_run_writes_defaults() {
        // Bite: the defaults write omitted, defaults(None) instead of the OS
        // language, a credential call at load, or first_run not reported.
        let dir = TempDir::new();
        let creds = Arc::new(FakeCredentialStore::new());
        let (service, outcome) = SettingsService::load_or_init(fs_deps(&dir, &creds), OS);

        assert_eq!(outcome, LoadOutcome::FirstRun(defaults(OS)));
        let bytes = fs::read(dir.path().join(SETTINGS_FILE)).expect("defaults written");
        assert_eq!(parse(&bytes), defaults(OS));
        assert_eq!(entries(dir.path()), vec![SETTINGS_FILE.to_string()]);
        assert_eq!(*service.snapshot(), defaults(OS));
        assert!(creds.calls().is_empty(), "load called the credential store");
        let view = service.view();
        assert!(view.first_run);
        assert!(!view.reset_notice);
    }

    #[test]
    fn valid_file_loads_without_validation_or_rewrite() {
        // Bite: validating at load (an invalid hand-edited value would reset the
        // file), normalizing at load, or rewriting a readable file.
        let mut on_disk = sample(EngineKind::Api);
        on_disk.history.size = 0;
        on_disk.hotkey = "Esc".into();
        on_disk.api.base_url = "  https://api.example.com/v1/ ".into();
        let bytes = json(&on_disk);
        let world = World::new(
            FakeSettingsFile::with_bytes(&bytes),
            FakeCredentialStore::new(),
        );

        let (service, outcome) = world.load();
        assert_eq!(outcome, LoadOutcome::Loaded(on_disk.clone()));
        assert_eq!(*service.snapshot(), on_disk);
        assert_eq!(world.file.calls(), vec![FileCall::Read]);
        assert_eq!(world.file.bytes(), Some(bytes));
        assert!(world.creds.calls().is_empty());
        let view = service.view();
        assert!(!view.first_run);
        assert!(!view.reset_notice);
    }

    #[test]
    fn view_reports_key_presence_from_the_store() {
        // Bite: presence hard-coded, read from the wrong slot, or a key value
        // reaching the view.
        let creds = FakeCredentialStore::new()
            .with_key(API, "sk-test-CANARY-api")
            .with_key(PP, "sk-test-CANARY-pp");
        let world = World::new(
            FakeSettingsFile::with_bytes(&json(&sample(EngineKind::None))),
            creds,
        );
        let (service, _) = world.load();
        let view = service.view();
        assert_eq!(
            view.keys,
            KeyPresence {
                transcription_api: true,
                local_server: false,
                post_processing: true,
            }
        );
        assert!(changes(&world.creds).is_empty(), "view changed a slot");
        let text = serde_json::to_string(&view).expect("view serializes");
        assert!(!text.contains("CANARY"), "key in view: {text}");
    }

    #[test]
    fn unreadable_file_reset() {
        // Bite: the bad file deleted or overwritten instead of moved, a wrong backup
        // name (suffix not from the clock), defaults not written, the reset notice
        // missing, or a credential call at load.
        let cases: [(&str, &[u8]); 4] = [
            ("invalid JSON", b"{\"engine\": "),
            ("wrong type", br#"{"history": {"size": "twenty"}}"#),
            ("schema_version 2", br#"{"schema_version": 2}"#),
            ("partial microphone", br#"{"microphone": {"id": "x"}}"#),
        ];
        for (what, bad) in cases {
            let dir = TempDir::new();
            fs::write(dir.path().join(SETTINGS_FILE), bad).expect("seed");
            let creds = Arc::new(FakeCredentialStore::new());
            let (service, outcome) = SettingsService::load_or_init(fs_deps(&dir, &creds), OS);

            assert_eq!(
                outcome,
                LoadOutcome::Reset {
                    settings: defaults(OS),
                    backup_file_name: BACKUP.to_string(),
                },
                "{what}"
            );
            assert_eq!(
                fs::read(dir.path().join(BACKUP)).expect(what),
                bad,
                "{what}"
            );
            let written = fs::read(dir.path().join(SETTINGS_FILE)).expect(what);
            assert_eq!(parse(&written), defaults(OS), "{what}");
            assert_eq!(
                entries(dir.path()),
                vec![SETTINGS_FILE.to_string(), BACKUP.to_string()],
                "{what}"
            );
            assert!(creds.calls().is_empty(), "{what}: credential call at load");
            assert_eq!(*service.snapshot(), defaults(OS), "{what}");
            let view = service.view();
            assert!(view.reset_notice, "{what}");
            assert!(!view.first_run, "{what}");
        }
    }

    #[test]
    fn move_aside_never_overwrites_existing_backup() {
        // Bite: a plain rename into settings.json.bad-<UTC> (destroys the earlier
        // backup of the same second).
        let dir = TempDir::new();
        fs::write(dir.path().join(BACKUP), b"MARKER older backup").expect("seed backup");
        fs::write(dir.path().join(SETTINGS_FILE), b"{not json").expect("seed");
        let creds = Arc::new(FakeCredentialStore::new());
        let (_service, outcome) = SettingsService::load_or_init(fs_deps(&dir, &creds), OS);

        let second = format!("{BACKUP}-1");
        assert_eq!(
            outcome,
            LoadOutcome::Reset {
                settings: defaults(OS),
                backup_file_name: second.clone(),
            }
        );
        assert_eq!(
            fs::read(dir.path().join(BACKUP)).expect("older backup"),
            b"MARKER older backup"
        );
        assert_eq!(
            fs::read(dir.path().join(&second)).expect("new backup"),
            b"{not json"
        );
        let written = fs::read(dir.path().join(SETTINGS_FILE)).expect("defaults");
        assert_eq!(parse(&written), defaults(OS));
    }

    #[test]
    fn move_aside_failure_is_unavailable() {
        // Bite: defaults written over a file that could not be moved aside (its
        // bytes lost), a save accepted, or a credential call.
        let bad: &[u8] = b"{not json";
        let world = World::new(
            FakeSettingsFile::with_bytes(bad),
            FakeCredentialStore::new().with_key(API, "sk-test-old-api"),
        );
        world.file.fail_move_aside(io::ErrorKind::PermissionDenied);

        let (service, outcome) = world.load();
        assert_eq!(outcome, LoadOutcome::Unavailable(defaults(OS)));
        let after_load = vec![FileCall::Read, FileCall::MoveAside(SUFFIX.to_string())];
        assert_eq!(world.file.calls(), after_load);
        assert_eq!(world.file.bytes().as_deref(), Some(bad));
        assert!(world.file.backups().is_empty());
        assert_eq!(*service.snapshot(), defaults(OS));

        let (errors, form) =
            expect_refused(service.save(req(sample(EngineKind::None), KeyEdits::default())));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(form, Some(FormError::SettingsUnavailable));
        assert_eq!(world.file.calls(), after_load, "file touched after load");
        assert!(world.creds.calls().is_empty());
    }

    #[test]
    fn read_io_error_is_unavailable() {
        // Bite: an I/O error other than NotFound treated as "no file" (FirstRun
        // would overwrite it) or as unreadable content (Reset would move it).
        // A real directory at settings.json.
        let dir = TempDir::new();
        fs::create_dir(dir.path().join(SETTINGS_FILE)).expect("dir at settings.json");
        let creds = Arc::new(FakeCredentialStore::new());
        let (service, outcome) = SettingsService::load_or_init(fs_deps(&dir, &creds), OS);
        assert_eq!(outcome, LoadOutcome::Unavailable(defaults(OS)));
        assert_eq!(entries(dir.path()), vec![SETTINGS_FILE.to_string()]);
        assert!(dir.path().join(SETTINGS_FILE).is_dir());
        assert!(creds.calls().is_empty());
        let (_, form) =
            expect_refused(service.save(req(sample(EngineKind::None), KeyEdits::default())));
        assert_eq!(form, Some(FormError::SettingsUnavailable));
        assert_eq!(entries(dir.path()), vec![SETTINGS_FILE.to_string()]);

        // Injected errors of other kinds.
        for kind in [
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::InvalidData,
            io::ErrorKind::Other,
        ] {
            let world = World::new(
                FakeSettingsFile::with_bytes(&json(&sample(EngineKind::None))),
                FakeCredentialStore::new(),
            );
            world.file.fail_read(kind);
            let (_service, outcome) = world.load();
            assert_eq!(outcome, LoadOutcome::Unavailable(defaults(OS)), "{kind:?}");
            assert_eq!(world.file.calls(), vec![FileCall::Read], "{kind:?}");
            assert!(world.creds.calls().is_empty(), "{kind:?}");
        }
    }

    #[test]
    fn defaults_write_failure_keeps_outcome() {
        // Bite (#23 N5): a failed defaults write turned into Unavailable (or a
        // panic), or the service refusing the later save that retries the write.
        // FirstRun.
        let world = World::new(FakeSettingsFile::new(), FakeCredentialStore::new());
        world.file.fail_write(io::ErrorKind::Other);
        let (service, outcome) = world.load();
        assert_eq!(outcome, LoadOutcome::FirstRun(defaults(OS)));
        assert_eq!(
            world.file.calls(),
            vec![FileCall::Read, FileCall::WriteAtomic]
        );
        assert_eq!(world.file.bytes(), None);
        world.file.clear_failures();
        expect_saved(service.save(req(sample(EngineKind::None), KeyEdits::default())));
        assert_eq!(
            parse(&world.file.bytes().expect("written by the save")),
            sample(EngineKind::None)
        );

        // Reset.
        let bad: &[u8] = b"{not json";
        let world = World::new(
            FakeSettingsFile::with_bytes(bad),
            FakeCredentialStore::new(),
        );
        world.file.fail_write(io::ErrorKind::Other);
        let (service, outcome) = world.load();
        assert_eq!(
            outcome,
            LoadOutcome::Reset {
                settings: defaults(OS),
                backup_file_name: BACKUP.to_string(),
            }
        );
        assert_eq!(
            world.file.backups(),
            vec![(BACKUP.to_string(), bad.to_vec())]
        );
        assert_eq!(world.file.bytes(), None);
        assert!(service.view().reset_notice);
        world.file.clear_failures();
        expect_saved(service.save(req(sample(EngineKind::None), KeyEdits::default())));
        assert_eq!(
            parse(&world.file.bytes().expect("written by the save")),
            sample(EngineKind::None)
        );
    }

    // ---- Unavailable ----------------------------------------------------------

    #[test]
    fn save_refused_while_unavailable() {
        // Bite (#19): any dependency called by a save while Unavailable, a save
        // validated (and refused with field errors) instead of refused outright, or
        // the refusal reported without the notice.
        let world = World::new(
            FakeSettingsFile::with_bytes(&json(&sample(EngineKind::None))),
            FakeCredentialStore::new()
                .with_key(API, "sk-test-old-api")
                .with_key(LOCAL, "sk-test-old-local")
                .with_key(PP, "sk-test-old-pp"),
        );
        world.file.fail_read(io::ErrorKind::PermissionDenied);
        let (service, outcome) = world.load();
        assert!(
            matches!(outcome, LoadOutcome::Unavailable(_)),
            "{outcome:?}"
        );
        let rx = service.subscribe();
        let before = service.snapshot();

        let mut invalid = sample(EngineKind::Api);
        invalid.history.size = 0;
        let requests = [
            req(sample(EngineKind::Api), all_keys(|| replace("sk-test-new"))),
            req(sample(EngineKind::None), all_keys(|| KeyEdit::Clear)),
            req(invalid, KeyEdits::default()),
        ];
        for request in requests {
            assert_eq!(
                service.save(request),
                SaveOutcome::Refused {
                    errors: vec![],
                    form_error: Some(FormError::SettingsUnavailable),
                }
            );
        }
        assert_eq!(world.file.calls(), vec![FileCall::Read]);
        assert!(world.creds.calls().is_empty(), "{:?}", world.creds.calls());
        assert!(world.hotkeys.calls().is_empty());
        assert!(Arc::ptr_eq(&before, &service.snapshot()));
        assert_eq!(*service.snapshot(), defaults(OS));
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
        assert_eq!(
            stored(&world.creds),
            [
                Some("sk-test-old-api".to_string()),
                Some("sk-test-old-local".to_string()),
                Some("sk-test-old-pp".to_string()),
            ]
        );
    }

    #[test]
    fn unavailable_view_reports_no_keys_without_credential_calls() {
        // Bite (#33(b)): view() reading presence from the store while Unavailable.
        let world = World::new(
            FakeSettingsFile::new(),
            FakeCredentialStore::new()
                .with_key(API, "sk-test-old-api")
                .with_key(LOCAL, "sk-test-old-local")
                .with_key(PP, "sk-test-old-pp"),
        );
        world.file.fail_read(io::ErrorKind::PermissionDenied);
        let (service, outcome) = world.load();
        assert_eq!(outcome, LoadOutcome::Unavailable(defaults(OS)));

        let view = service.view();
        assert_eq!(
            view.keys,
            KeyPresence::default(),
            "every key reported absent"
        );
        assert_eq!(view.settings, defaults(OS));
        assert!(world.creds.calls().is_empty(), "{:?}", world.creds.calls());
    }

    // ---- save: keys -------------------------------------------------------------

    #[test]
    fn save_never_writes_key_bytes() {
        // Property (I1): no key byte reaches settings.json, any file in the data
        // directory, the Saved view or view(). Bite: a key field added to Settings
        // or SettingsView, or a key serialized into the file.
        let marks = [
            (API, "sk-test-CANARY-api-0000"),
            (LOCAL, "sk-test-CANARY-local-0000"),
            (PP, "sk-test-CANARY-pp-0000"),
        ];
        let dir = TempDir::new();
        let creds = Arc::new(FakeCredentialStore::new());
        let (service, _) = SettingsService::load_or_init(fs_deps(&dir, &creds), OS);

        let keys = KeyEdits {
            transcription_api: replace(marks[0].1),
            local_server: replace(marks[1].1),
            post_processing: replace(marks[2].1),
        };
        let view = expect_saved(service.save(req(sample(EngineKind::Api), keys)));

        // The keys did go to the store (otherwise the property is vacuous).
        for (slot, mark) in marks {
            assert_eq!(creds.stored(slot).as_deref(), Some(mark), "{slot:?}");
        }
        assert_eq!(
            view.keys,
            KeyPresence {
                transcription_api: true,
                local_server: true,
                post_processing: true,
            }
        );
        let mut texts = vec![
            serde_json::to_string(&view).expect("view serializes"),
            serde_json::to_string(&service.view()).expect("view serializes"),
        ];
        for name in entries(dir.path()) {
            let bytes = fs::read(dir.path().join(&name)).expect("data dir file");
            texts.push(String::from_utf8_lossy(&bytes).into_owned());
        }
        assert!(texts.len() >= 3, "settings.json missing: {texts:?}");
        for text in texts {
            assert!(!text.contains("CANARY"), "key bytes leaked: {text}");
        }
    }

    #[test]
    fn non_empty_replace_is_stored_trimmed() {
        // Bite (#33(a)): the key stored as entered (a pasted newline would give a
        // 401), or trimmed with an ASCII-only trim, or inner spaces removed.
        let world = World::new(FakeSettingsFile::new(), FakeCredentialStore::new());
        let (service, _) = world.load();
        let keys = KeyEdits {
            transcription_api: replace("  sk-test-trim api\n"),
            local_server: replace("\tsk-test-trim-local "),
            post_processing: replace("\u{3000}sk-test-trim-pp\r\n"),
        };
        expect_saved(service.save(req(sample(EngineKind::Api), keys)));
        assert_eq!(
            stored(&world.creds),
            [
                Some("sk-test-trim api".to_string()),
                Some("sk-test-trim-local".to_string()),
                Some("sk-test-trim-pp".to_string()),
            ]
        );
    }

    #[test]
    fn blank_replace_is_untouched_in_every_slot() {
        // Bite (#30): a blank Replace written to a slot, or mapped to Clear; or
        // validate fed the mapped edit (the API blank must stay key.required).
        let old = [
            Some("sk-test-old-api".to_string()),
            Some("sk-test-old-local".to_string()),
            Some("sk-test-old-pp".to_string()),
        ];
        let world = World::new(
            FakeSettingsFile::with_bytes(&json(&sample(EngineKind::None))),
            FakeCredentialStore::new()
                .with_key(API, "sk-test-old-api")
                .with_key(LOCAL, "sk-test-old-local")
                .with_key(PP, "sk-test-old-pp"),
        );
        let (service, _) = world.load();

        for blank in ["", "   ", "\n\t \u{3000}"] {
            let mut draft = sample(EngineKind::None);
            draft.history.size = 42;
            let view = expect_saved(service.save(req(draft.clone(), all_keys(|| replace(blank)))));
            assert_eq!(
                view.keys,
                KeyPresence {
                    transcription_api: true,
                    local_server: true,
                    post_processing: true,
                },
                "{blank:?}"
            );
            assert_eq!(*service.snapshot(), draft, "{blank:?}");
        }
        assert!(
            changes(&world.creds).is_empty(),
            "{:?}",
            changes(&world.creds)
        );
        assert_eq!(stored(&world.creds), old);

        // Engine api: a blank API Replace is still key.required, and nothing is written.
        let keys = KeyEdits {
            transcription_api: replace("  "),
            ..KeyEdits::default()
        };
        let (errors, form) = expect_refused(service.save(req(sample(EngineKind::Api), keys)));
        assert_eq!(
            errors,
            vec![field_error(FieldId::EngineApiKey, ErrorCode::KeyRequired)]
        );
        assert_eq!(form, None);
        assert!(
            changes(&world.creds).is_empty(),
            "{:?}",
            changes(&world.creds)
        );
        assert_eq!(stored(&world.creds), old);
    }

    #[test]
    fn clear_deletes_and_saved_view_reports_absent() {
        // AC4 "only Clear deletes" on the success path, and the Saved view's
        // presence taken from the applied edits (core-traits.md). Bite: the Delete
        // step reported as present (`.map(|()| true)` at the KeyStep::Delete arm),
        // the delete skipped, a sibling slot touched, or view() reading stale
        // presence.
        let old = [
            (API, "sk-test-old-api"),
            (LOCAL, "sk-test-old-local"),
            (PP, "sk-test-old-pp"),
        ];
        for slot in KeySlot::all() {
            let world = World::new(
                FakeSettingsFile::with_bytes(&json(&sample(EngineKind::None))),
                FakeCredentialStore::new()
                    .with_key(old[0].0, old[0].1)
                    .with_key(old[1].0, old[1].1)
                    .with_key(old[2].0, old[2].1),
            );
            let (service, _) = world.load();

            let mut keys = KeyEdits::default();
            match slot {
                KeySlot::TranscriptionApi => keys.transcription_api = KeyEdit::Clear,
                KeySlot::LocalServer => keys.local_server = KeyEdit::Clear,
                KeySlot::PostProcessing => keys.post_processing = KeyEdit::Clear,
            }
            let mut draft = sample(EngineKind::None);
            draft.history.size = 42;
            let saved = expect_saved(service.save(req(draft.clone(), keys)));

            let expected_stored: Vec<Option<String>> = old
                .iter()
                .map(|(s, key)| (*s != slot).then(|| key.to_string()))
                .collect();
            assert_eq!(stored(&world.creds).to_vec(), expected_stored, "{slot:?}");
            assert_eq!(
                changes(&world.creds),
                vec![call(CredentialOp::Delete, slot)],
                "{slot:?}"
            );

            let expected_presence = KeyPresence {
                transcription_api: slot != API,
                local_server: slot != LOCAL,
                post_processing: slot != PP,
            };
            assert_eq!(saved.keys, expected_presence, "{slot:?}: Saved view");
            assert_eq!(saved.settings, draft, "{slot:?}");
            assert_eq!(
                service.view().keys,
                expected_presence,
                "{slot:?}: later view()"
            );
            assert_eq!(*service.snapshot(), draft, "{slot:?}");
            assert_eq!(
                parse(&world.file.bytes().expect("file written")),
                draft,
                "{slot:?}"
            );
        }
    }

    #[test]
    fn save_stores_normalized_values() {
        // Bite: normalize skipped, applied only to the selected engine or only to
        // valid URLs, stripping more than one `/`, schema_version 0 written back as
        // 0, or text fields outside the rule (prompt, microphone) trimmed too.
        let world = World::new(FakeSettingsFile::new(), FakeCredentialStore::new());
        let (service, _) = world.load();

        let mut draft = sample(EngineKind::Api);
        draft.schema_version = 0;
        draft.api.base_url = "  https://api.example.com/v1/ \n".into();
        draft.api.model = "\t whisper-test  ".into();
        // Not the selected engine.
        draft.local_server.base_url = " http://192.0.2.10:8000/v1// ".into();
        draft.local_server.model = "  local-test ".into();
        // Post-processing off, and its URL is not even valid: still normalized.
        draft.post_processing.enabled = false;
        draft.post_processing.base_url = "  llm.example.com/v1/  ".into();
        draft.post_processing.model = " llm-test\n".into();
        // Outside the rule: kept as entered.
        draft.post_processing.prompt = "  Fix the text.  \n".into();
        draft.microphone = Some(Microphone {
            id: " {fake-device-id} ".into(),
            name: "  Test Microphone (fake) ".into(),
        });

        let mut expected = draft.clone();
        expected.schema_version = SCHEMA_VERSION;
        expected.api.base_url = "https://api.example.com/v1".into();
        expected.api.model = "whisper-test".into();
        expected.local_server.base_url = "http://192.0.2.10:8000/v1/".into();
        expected.local_server.model = "local-test".into();
        expected.post_processing.base_url = "llm.example.com/v1".into();
        expected.post_processing.model = "llm-test".into();

        let keys = KeyEdits {
            transcription_api: replace("sk-test-api"),
            ..KeyEdits::default()
        };
        let view = expect_saved(service.save(req(draft, keys)));
        assert_eq!(view.settings, expected);
        assert_eq!(*service.snapshot(), expected);
        assert_eq!(parse(&world.file.bytes().expect("file written")), expected);
    }

    // ---- save: failure branches and undo ----------------------------------------

    #[test]
    fn refused_save_changes_nothing() {
        // Bite (I2): a side effect before validation (a key written, the file
        // written, the snapshot swapped or a message sent on a validation refusal).
        let on_disk = sample(EngineKind::None);
        let bytes = json(&on_disk);
        let world = World::new(
            FakeSettingsFile::with_bytes(&bytes),
            FakeCredentialStore::new()
                .with_key(API, "sk-test-old-api")
                .with_key(LOCAL, "sk-test-old-local"),
        );
        let (service, _) = world.load();
        let rx = service.subscribe();
        let before = service.snapshot();

        let mut draft = sample(EngineKind::Api);
        draft.api.base_url = "api.example.com".into();
        draft.history.size = 0;
        let keys = KeyEdits {
            transcription_api: replace("sk-test-new-api"),
            local_server: KeyEdit::Clear,
            post_processing: replace("sk-test-new-pp"),
        };
        let (errors, form) = expect_refused(service.save(req(draft, keys)));
        assert_eq!(
            errors,
            vec![
                field_error(FieldId::EngineApiBaseUrl, ErrorCode::UrlMalformed),
                field_error(FieldId::HistorySize, ErrorCode::HistorySizeRange),
            ]
        );
        assert_eq!(form, None);

        assert_eq!(world.file.bytes(), Some(bytes));
        assert_eq!(world.file.calls(), vec![FileCall::Read]);
        assert!(Arc::ptr_eq(&before, &service.snapshot()));
        assert_eq!(*service.snapshot(), on_disk);
        assert!(
            changes(&world.creds).is_empty(),
            "{:?}",
            changes(&world.creds)
        );
        assert_eq!(
            stored(&world.creds),
            [
                Some("sk-test-old-api".to_string()),
                Some("sk-test-old-local".to_string()),
                None,
            ]
        );
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn file_write_failure_restores_keys() {
        // Bite (I2): keys left changed after the file write fails, undo in forward
        // order, a Clear undone by a delete, or the snapshot swapped / published.
        let on_disk = sample(EngineKind::None);
        let bytes = json(&on_disk);
        let world = World::new(
            FakeSettingsFile::with_bytes(&bytes),
            FakeCredentialStore::new()
                .with_key(API, "sk-test-old-api")
                .with_key(LOCAL, "sk-test-old-local"),
        );
        let (service, _) = world.load();
        let rx = service.subscribe();
        let before = service.snapshot();
        world.file.fail_write(io::ErrorKind::Other);

        let mut draft = sample(EngineKind::None);
        draft.history.size = 50;
        let keys = KeyEdits {
            transcription_api: replace("sk-test-new-api"),
            local_server: KeyEdit::Clear,
            post_processing: KeyEdit::Untouched,
        };
        let (errors, form) = expect_refused(service.save(req(draft, keys)));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(form, Some(FormError::WriteFailed));

        assert_eq!(
            world.file.calls(),
            vec![FileCall::Read, FileCall::WriteAtomic]
        );
        assert_eq!(world.file.bytes(), Some(bytes));
        assert!(Arc::ptr_eq(&before, &service.snapshot()));
        assert_eq!(
            stored(&world.creds),
            [
                Some("sk-test-old-api".to_string()),
                Some("sk-test-old-local".to_string()),
                None,
            ]
        );
        assert_eq!(
            changes(&world.creds),
            vec![
                call(CredentialOp::Write, API),
                call(CredentialOp::Delete, LOCAL),
                // Undo, in reverse.
                call(CredentialOp::Write, LOCAL),
                call(CredentialOp::Write, API),
            ]
        );
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn key_store_failure_refuses_with_key_store_failed() {
        // Bite: the earlier slot's change kept, the error put on the wrong key
        // field, the file written after a key failure, or a later slot still tried.
        let on_disk = sample(EngineKind::None);
        let bytes = json(&on_disk);
        let world = World::new(
            FakeSettingsFile::with_bytes(&bytes),
            FakeCredentialStore::new().with_key(API, "sk-test-old-api"),
        );
        world.creds.fail(CredentialOp::Write, LOCAL, STORE_ERR);
        let (service, _) = world.load();
        let rx = service.subscribe();
        let before = service.snapshot();

        let keys = KeyEdits {
            transcription_api: replace("sk-test-new-api"),
            local_server: replace("sk-test-new-local"),
            post_processing: replace("sk-test-new-pp"),
        };
        let (errors, form) = expect_refused(service.save(req(sample(EngineKind::None), keys)));
        assert_eq!(
            errors,
            vec![field_error(
                FieldId::EngineLocalServerKey,
                ErrorCode::KeyStoreFailed
            )]
        );
        assert_eq!(form, None);

        assert_eq!(world.file.calls(), vec![FileCall::Read], "file touched");
        assert_eq!(world.file.bytes(), Some(bytes));
        assert!(Arc::ptr_eq(&before, &service.snapshot()));
        assert_eq!(
            stored(&world.creds),
            [Some("sk-test-old-api".to_string()), None, None]
        );
        assert_eq!(
            changes(&world.creds),
            vec![
                call(CredentialOp::Write, API),
                call(CredentialOp::Write, LOCAL),
                call(CredentialOp::Write, API),
            ]
        );
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn key_read_failure_refuses_before_any_write() {
        // Bite: a slot whose old value cannot be read is changed anyway (it could
        // then not be restored), or the error lands on another field.
        let bytes = json(&sample(EngineKind::None));
        let world = World::new(
            FakeSettingsFile::with_bytes(&bytes),
            FakeCredentialStore::new().with_key(API, "sk-test-old-api"),
        );
        world.creds.fail(CredentialOp::Read, PP, STORE_ERR);
        let (service, _) = world.load();

        let keys = all_keys(|| replace("sk-test-new"));
        let (errors, form) = expect_refused(service.save(req(sample(EngineKind::None), keys)));
        assert_eq!(
            errors,
            vec![field_error(
                FieldId::PostProcessingKey,
                ErrorCode::KeyStoreFailed
            )]
        );
        assert_eq!(form, None);
        assert!(
            changes(&world.creds).is_empty(),
            "{:?}",
            changes(&world.creds)
        );
        assert_eq!(
            stored(&world.creds),
            [Some("sk-test-old-api".to_string()), None, None]
        );
        assert_eq!(world.file.calls(), vec![FileCall::Read]);
    }

    #[test]
    fn undo_failure_reports_partially_restored() {
        // Bite: a failed undo hidden (reported as plain key.store_failed /
        // WriteFailed), the wrong slot named, or the undo stopping at the first
        // undo error.
        // A key failure whose undo fails.
        let bytes = json(&sample(EngineKind::None));
        let world = World::new(
            FakeSettingsFile::with_bytes(&bytes),
            FakeCredentialStore::new(),
        );
        world.creds.fail(CredentialOp::Write, LOCAL, STORE_ERR);
        world.creds.fail(CredentialOp::Delete, API, STORE_ERR);
        let (service, _) = world.load();
        let before = service.snapshot();

        let keys = KeyEdits {
            transcription_api: replace("sk-test-new-api"),
            local_server: replace("sk-test-new-local"),
            post_processing: KeyEdit::Untouched,
        };
        let (errors, form) = expect_refused(service.save(req(sample(EngineKind::None), keys)));
        assert_eq!(
            errors,
            vec![field_error(
                FieldId::EngineLocalServerKey,
                ErrorCode::KeyStoreFailed
            )]
        );
        assert_eq!(
            form,
            Some(FormError::PartiallyRestored {
                not_restored: vec![FieldId::EngineApiKey],
            })
        );
        assert_eq!(world.file.calls(), vec![FileCall::Read]);
        assert_eq!(world.file.bytes(), Some(bytes));
        assert!(Arc::ptr_eq(&before, &service.snapshot()));
        // The one documented leftover (R-3).
        assert_eq!(
            stored(&world.creds),
            [Some("sk-test-new-api".to_string()), None, None]
        );

        // A file write failure whose first undo (in reverse order) fails: the undo
        // goes on, and PartiallyRestored replaces WriteFailed.
        let bytes = json(&sample(EngineKind::None));
        let world = World::new(
            FakeSettingsFile::with_bytes(&bytes),
            FakeCredentialStore::new().with_key(API, "sk-test-old-api"),
        );
        world.creds.fail(CredentialOp::Delete, PP, STORE_ERR);
        let (service, _) = world.load();
        let before = service.snapshot();
        world.file.fail_write(io::ErrorKind::Other);

        let keys = KeyEdits {
            transcription_api: replace("sk-test-new-api"),
            local_server: KeyEdit::Untouched,
            post_processing: replace("sk-test-new-pp"),
        };
        let (errors, form) = expect_refused(service.save(req(sample(EngineKind::None), keys)));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            form,
            Some(FormError::PartiallyRestored {
                not_restored: vec![FieldId::PostProcessingKey],
            })
        );
        assert_eq!(world.file.bytes(), Some(bytes));
        assert!(Arc::ptr_eq(&before, &service.snapshot()));
        assert_eq!(
            stored(&world.creds),
            [
                Some("sk-test-old-api".to_string()),
                None,
                Some("sk-test-new-pp".to_string()),
            ]
        );
        assert_eq!(
            changes(&world.creds),
            vec![
                call(CredentialOp::Write, API),
                call(CredentialOp::Write, PP),
                call(CredentialOp::Delete, PP),
                call(CredentialOp::Write, API),
            ]
        );
    }

    #[test]
    fn partially_restored_lists_fields_in_slot_order() {
        // The doc of `undo` and FormError::PartiallyRestored: the key fields still
        // changed, in slot order. The undo runs in reverse, so two failing undos
        // come out reversed. Bite: `not_restored.reverse()` removed, or the undo
        // stopping at its first error (the second field would be missing).
        // A file write failure; the undos of API and PP fail, LOCAL's succeeds.
        let bytes = json(&sample(EngineKind::None));
        let world = World::new(
            FakeSettingsFile::with_bytes(&bytes),
            FakeCredentialStore::new(),
        );
        world.creds.fail(CredentialOp::Delete, API, STORE_ERR);
        world.creds.fail(CredentialOp::Delete, PP, STORE_ERR);
        let (service, _) = world.load();
        world.file.fail_write(io::ErrorKind::Other);

        let keys = KeyEdits {
            transcription_api: replace("sk-test-new-api"),
            local_server: replace("sk-test-new-local"),
            post_processing: replace("sk-test-new-pp"),
        };
        let (errors, form) = expect_refused(service.save(req(sample(EngineKind::None), keys)));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            form,
            Some(FormError::PartiallyRestored {
                not_restored: vec![FieldId::EngineApiKey, FieldId::PostProcessingKey],
            })
        );
        assert_eq!(
            changes(&world.creds),
            vec![
                call(CredentialOp::Write, API),
                call(CredentialOp::Write, LOCAL),
                call(CredentialOp::Write, PP),
                // Undo, in reverse; every step tried.
                call(CredentialOp::Delete, PP),
                call(CredentialOp::Delete, LOCAL),
                call(CredentialOp::Delete, API),
            ]
        );
        assert_eq!(
            stored(&world.creds),
            [
                Some("sk-test-new-api".to_string()),
                None,
                Some("sk-test-new-pp".to_string()),
            ]
        );
        assert_eq!(world.file.bytes(), Some(bytes));

        // A key failure on PP; the undos of API and LOCAL both fail.
        let world = World::new(
            FakeSettingsFile::with_bytes(&json(&sample(EngineKind::None))),
            FakeCredentialStore::new(),
        );
        world.creds.fail(CredentialOp::Write, PP, STORE_ERR);
        world.creds.fail(CredentialOp::Delete, API, STORE_ERR);
        world.creds.fail(CredentialOp::Delete, LOCAL, STORE_ERR);
        let (service, _) = world.load();
        let keys = all_keys(|| replace("sk-test-new"));
        let (errors, form) = expect_refused(service.save(req(sample(EngineKind::None), keys)));
        assert_eq!(
            errors,
            vec![field_error(
                FieldId::PostProcessingKey,
                ErrorCode::KeyStoreFailed
            )]
        );
        assert_eq!(
            form,
            Some(FormError::PartiallyRestored {
                not_restored: vec![FieldId::EngineApiKey, FieldId::EngineLocalServerKey],
            })
        );
    }

    #[test]
    fn form_error_message_ids() {
        // Bite (#23 N3): a form error mapped to another message, or an id not
        // declared with messages! (so not checked against both catalogs).
        let table = [
            (FormError::WriteFailed, "settings.write_failed"),
            (
                FormError::SettingsUnavailable,
                "notice.settings_unavailable",
            ),
            (
                FormError::PartiallyRestored {
                    not_restored: vec![FieldId::EngineApiKey],
                },
                "settings.partially_restored",
            ),
        ];
        for (error, id) in table {
            let message = error.message_id();
            assert_eq!(
                format!("{message:?}"),
                format!("MessageId({id:?})"),
                "{error:?}"
            );
            assert!(MESSAGE_IDS.contains(&message), "{id} not in MESSAGE_IDS");
        }
    }
}
