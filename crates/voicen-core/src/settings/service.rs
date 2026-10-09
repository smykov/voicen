//! The settings service: load at startup, snapshot, view, all-or-nothing save with
//! reverse undo, and live apply through std `mpsc` subscribers (research R-2, R-3,
//! R-4; decisions #19, #22, #23 N5, #30, #33; contracts/core-traits.md#settingsservice).

use std::io;
use std::sync::{mpsc, Arc, Mutex, PoisonError, RwLock};

use serde::{Deserialize, Serialize};

use super::file::SettingsFile;
use super::hotkey::parse_hotkey;
use super::url::{check_base_url, is_insecure_remote, normalize_base_url};
use super::validate::{validate, KeyEditsWithPresence};
use super::{
    defaults, EngineKind, ErrorCode, FieldError, FieldId, LoadOutcome, Settings, SCHEMA_VERSION,
};
use crate::autostart::{Autostart, ReconcileAction};
use crate::clock::{utc_compact, Clock};
use crate::connection_test::{
    self, request_timeouts, ConnectionTestRequest, ConnectionTestResult, TestContext,
};
use crate::hotkey_registrar::{HotkeyRegistrar, Prepared};
use crate::i18n::{
    MessageId, NOTICE_SETTINGS_UNAVAILABLE, SETTINGS_PARTIALLY_RESTORED,
    SETTINGS_WARNING_ENDPOINT_INSECURE, SETTINGS_WRITE_FAILED,
};
use crate::models::DownloadedModels;
use crate::secrets::{CredentialStore, KeyEdit, KeyEdits, KeyPresence, KeySlot, Secret};
use crate::timeouts::Timeouts;

/// Everything the service talks to.
pub struct SettingsDeps {
    pub file: Arc<dyn SettingsFile>,
    pub credentials: Arc<dyn CredentialStore>,
    /// The logon start entry (T-014): the save step and `reconcile_autostart`.
    pub autostart: Arc<dyn Autostart>,
    /// The two-step hotkey registration of a save (T-055, R-3 steps 2 and 6):
    /// `prepare` when the hotkey text changes, `commit` after the file is written
    /// and before the snapshot swap, `abort` on every later refusal.
    pub hotkeys: Arc<dyn HotkeyRegistrar>,
    pub local_models: Arc<dyn DownloadedModels>,
    pub clock: Arc<dyn Clock>,
}

/// A save from the settings window: the whole draft plus one edit per key slot.
///
/// Wire form (UI -> shell, `settings_save { request }`):
/// `{"settings": Settings, "keys": KeyEdits}`. Any deserialize error is one fixed
/// text (`invalid save request: ...`): the request carries keys, and serde's own
/// messages quote the input (T-030 J1).
#[derive(Debug)]
pub struct SaveRequest {
    pub settings: Settings,
    pub keys: KeyEdits,
}

/// What a window receives. Never contains a key.
///
/// `unavailable` is true only when the load was `Unavailable` (decision #19): the
/// window shows `notice.settings_unavailable` when it opens (decision #38).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingsView {
    pub settings: Settings,
    pub keys: KeyPresence,
    pub first_run: bool,
    pub reset_notice: bool,
    pub unavailable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningCode {
    EndpointInsecure,
}

impl WarningCode {
    /// The wire code (`endpoint.insecure`).
    pub fn as_str(self) -> &'static str {
        match self {
            WarningCode::EndpointInsecure => "endpoint.insecure",
        }
    }

    /// The catalog id the UI renders for this code (the one mapping; decision #52).
    pub fn message_id(self) -> MessageId {
        match self {
            WarningCode::EndpointInsecure => SETTINGS_WARNING_ENDPOINT_INSECURE,
        }
    }
}

impl Serialize for WarningCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// A non-blocking remark on a saved field, listed in `SaveOutcome::Saved` by
/// [`save_warnings`] (T-015). It never refuses or changes a save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Warning {
    pub field: FieldId,
    pub code: WarningCode,
}

/// Wire form: `{"field": "<FieldId>", "code": "endpoint.insecure", "message":
/// "<MessageId>"}`. `message` is [`WarningCode::message_id`], the one mapping.
impl Serialize for Warning {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Warning", 3)?;
        state.serialize_field("field", &self.field)?;
        state.serialize_field("code", &self.code)?;
        state.serialize_field("message", &self.code.message_id())?;
        state.end()
    }
}

/// The warnings of a save of `settings` (already normalized): one
/// `endpoint.insecure` per base URL in use that passes [`check_base_url`] and is
/// [`is_insecure_remote`]. In use: the selected engine's base URL (`api` or
/// `local_server`; no other engine has one) and `post_processing.base_url` while
/// `post_processing.enabled`. A URL that fails `check_base_url` gets no warning
/// (refusing it is validation's job). Engine field first, then post-processing.
pub fn save_warnings(settings: &Settings) -> Vec<Warning> {
    let engine_url = match settings.engine {
        EngineKind::Api => Some((FieldId::EngineApiBaseUrl, settings.api.base_url.as_str())),
        EngineKind::LocalServer => Some((
            FieldId::EngineLocalServerBaseUrl,
            settings.local_server.base_url.as_str(),
        )),
        EngineKind::None | EngineKind::BuiltinLocal => None,
    };
    let post_processing_url = settings.post_processing.enabled.then_some((
        FieldId::PostProcessingBaseUrl,
        settings.post_processing.base_url.as_str(),
    ));
    engine_url
        .into_iter()
        .chain(post_processing_url)
        .filter(|(_, raw)| check_base_url(raw).is_ok_and(|url| is_insecure_remote(&url)))
        .map(|(field, _)| Warning {
            field,
            code: WarningCode::EndpointInsecure,
        })
        .collect()
}

/// A refusal that is not tied to one field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormError {
    /// The settings file could not be written; everything was restored.
    WriteFailed,
    /// The service is `Unavailable` (decision #19): every save is refused.
    SettingsUnavailable,
    /// An undo failed: exactly these fields (`general.start_with_windows` and/or key
    /// fields, in step order) differ from before the save.
    PartiallyRestored { not_restored: Vec<FieldId> },
}

impl FormError {
    /// The wire kind (`write_failed` | `settings_unavailable` | `partially_restored`),
    /// the one spelling of the wire form and the log's `form_error=` (T-008).
    pub fn kind(&self) -> &'static str {
        match self {
            FormError::WriteFailed => "write_failed",
            FormError::SettingsUnavailable => "settings_unavailable",
            FormError::PartiallyRestored { .. } => "partially_restored",
        }
    }

    pub fn message_id(&self) -> MessageId {
        match self {
            FormError::WriteFailed => SETTINGS_WRITE_FAILED,
            FormError::SettingsUnavailable => NOTICE_SETTINGS_UNAVAILABLE,
            FormError::PartiallyRestored { .. } => SETTINGS_PARTIALLY_RESTORED,
        }
    }
}

/// Wire form: `{"kind": "write_failed" | "settings_unavailable" |
/// "partially_restored", "message": "<MessageId>"}`, plus `"not_restored": [FieldId]`
/// on `partially_restored` only. `message` is [`FormError::message_id`], the one
/// mapping.
impl Serialize for FormError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let not_restored = match self {
            FormError::WriteFailed | FormError::SettingsUnavailable => None,
            FormError::PartiallyRestored { not_restored } => Some(not_restored),
        };
        let len = if not_restored.is_some() { 3 } else { 2 };
        let mut state = serializer.serialize_struct("FormError", len)?;
        state.serialize_field("kind", self.kind())?;
        state.serialize_field("message", &self.message_id())?;
        if let Some(fields) = not_restored {
            state.serialize_field("not_restored", fields)?;
        }
        state.end()
    }
}

/// Wire form (externally tagged, contracts/ipc.md):
/// `{"Saved": {"view": SettingsView, "warnings": [Warning]}}` or
/// `{"Refused": {"errors": [FieldError], "form_error": FormError | null}}`.
// One value per save, never stored in bulk: the size difference does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

/// The fixed deserialize error of [`SaveRequest`].
const SAVE_REQUEST_WIRE_ERROR: &str =
    "invalid save request: expected {\"settings\": Settings, \"keys\": KeyEdits}";

#[derive(Deserialize)]
struct SaveRequestWire {
    settings: Settings,
    keys: KeyEdits,
}

impl<'de> Deserialize<'de> for SaveRequest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match SaveRequestWire::deserialize(deserializer) {
            Ok(wire) => Ok(SaveRequest {
                settings: wire.settings,
                keys: wire.keys,
            }),
            Err(_) => Err(serde::de::Error::custom(SAVE_REQUEST_WIRE_ERROR)),
        }
    }
}

/// The one reader and writer of `settings.json` and the one writer of keys.
///
/// - `save` is all-or-nothing (R-3): nothing is changed before the service is known
///   to be available and the draft is normalized and valid; the autostart entry is
///   changed next (only when `start_with_windows` changes), then the keys, the file
///   last, and a failure undoes the completed steps in reverse.
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
pub(crate) enum KeyStep<'a> {
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

        // (4) Hotkey (T-055, R-3 step 2): only when the hotkey text differs from the
        // snapshot in force (an unchanged one is never re-registered, T032). The new
        // one is registered next to the old; a failure refuses before anything else
        // is touched, and the old hotkey stays the one in force.
        let prepared = if settings.hotkey != self.snapshot().hotkey {
            let prepared = parse_hotkey(&settings.hotkey)
                .ok()
                .and_then(|hotkey| self.deps.hotkeys.prepare(hotkey, settings.mode).ok());
            match prepared {
                Some(prepared) => Some(prepared),
                None => {
                    return refused(
                        vec![FieldError {
                            field: FieldId::RecordingHotkey,
                            code: ErrorCode::HotkeyUnavailable,
                        }],
                        None,
                    )
                }
            }
        } else {
            None
        };
        // Every refusal from here on releases the prepared registration, last.
        let abort = |prepared: Option<Prepared>| {
            if let Some(prepared) = prepared {
                self.deps.hotkeys.abort(prepared);
            }
        };

        // (5) Autostart (T-014, R-3 step 3): only when the value differs from the
        // snapshot in force; a failure refuses before any key or the file is touched.
        let old_start = self.snapshot().start_with_windows;
        let new_start = settings.start_with_windows;
        let autostart_done = if new_start != old_start {
            if self.deps.autostart.set(new_start).is_err() {
                abort(prepared);
                return refused(
                    vec![FieldError {
                        field: FieldId::GeneralStartWithWindows,
                        code: ErrorCode::AutostartFailed,
                    }],
                    None,
                );
            }
            Some(old_start)
        } else {
            None
        };

        // (6) Keys, in slot order; each completed step is kept for the undo.
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
                    let form_error = self.undo(autostart_done, done).map(partially_restored);
                    abort(prepared);
                    return refused(vec![key_store_failed(slot)], form_error);
                }
            }
        }

        // (7) The file: tmp + sync + rename, or the old file stays (I3).
        let written = encode(&settings).and_then(|bytes| self.deps.file.write_atomic(&bytes));
        if written.is_err() {
            let form_error = self
                .undo(autostart_done, done)
                .map_or(FormError::WriteFailed, partially_restored);
            abort(prepared);
            return refused(Vec::new(), Some(form_error));
        }

        // (8) Commit: the new hotkey becomes the active one and the old one is
        // released (R-3 step 6) before any reader can see the new snapshot; then swap
        // the snapshot and publish to every live subscriber. The warnings are decided
        // here, once, over what was saved (T-015).
        if let Some(prepared) = prepared {
            self.deps.hotkeys.commit(prepared);
        }
        let warnings = save_warnings(&settings);
        let snapshot = Arc::new(settings);
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = snapshot.clone();
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|tx| tx.send(snapshot.clone()).is_ok());
        SaveOutcome::Saved {
            view: self.view_with((*snapshot).clone(), presence_after),
            warnings,
        }
    }

    /// Makes the logon start entry match the snapshot (R-5), under the save lock;
    /// never touches the file or the credential store. `Unavailable` → `None` with
    /// no call; on → `set(true)` → `Written`; off → `is_enabled()`: absent →
    /// `None`, present → `set(false)` → `Removed`; any error → `Failed`.
    pub fn reconcile_autostart(&self) -> ReconcileAction {
        if self.load.unavailable {
            return ReconcileAction::None;
        }
        let _guard = self
            .save_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let autostart = self.deps.autostart.as_ref();
        if self.snapshot().start_with_windows {
            return match autostart.set(true) {
                Ok(()) => ReconcileAction::Written,
                Err(_) => ReconcileAction::Failed,
            };
        }
        match autostart.is_enabled() {
            Ok(false) => ReconcileAction::None,
            Ok(true) => match autostart.set(false) {
                Ok(()) => ReconcileAction::Removed,
                Err(_) => ReconcileAction::Failed,
            },
            Err(_) => ReconcileAction::Failed,
        }
    }

    fn view_with(&self, settings: Settings, keys: KeyPresence) -> SettingsView {
        SettingsView {
            settings,
            keys,
            first_run: self.load.first_run,
            reset_notice: self.load.reset_notice,
            unavailable: self.load.unavailable,
        }
    }

    fn read_presence(&self) -> KeyPresence {
        presence_of(|slot| matches!(self.deps.credentials.read(slot), Ok(Some(_))))
    }

    /// Undoes the completed steps in reverse: the key steps in reverse slot order,
    /// then the autostart step (`autostart_done` = the value before the save), and
    /// goes on after a failure. `None` = all restored; otherwise the fields still
    /// changed, in step order (autostart, then the keys in slot order).
    fn undo(
        &self,
        autostart_done: Option<bool>,
        done: Vec<(KeySlot, Option<Secret>)>,
    ) -> Option<Vec<FieldId>> {
        let mut keys_not_restored = Vec::new();
        for (slot, old_value) in done.into_iter().rev() {
            let restored = match &old_value {
                Some(key) => self.deps.credentials.write(slot, key),
                None => self.deps.credentials.delete(slot),
            };
            if restored.is_err() {
                keys_not_restored.push(key_field(slot));
            }
        }
        let mut not_restored = Vec::new();
        if let Some(old_start) = autostart_done {
            if self.deps.autostart.set(old_start).is_err() {
                not_restored.push(FieldId::GeneralStartWithWindows);
            }
        }
        not_restored.extend(keys_not_restored.into_iter().rev());
        if not_restored.is_empty() {
            None
        } else {
            Some(not_restored)
        }
    }

    /// "Test connection" (FR-017, FR-018; T-046): one transcription request with
    /// the bundled clip, built from the snapshot overlaid by the form, with the
    /// form's timeouts ([`request_timeouts`]). Writes nothing: no save, no file,
    /// no snapshot change, no credential write or delete; at most one read of the
    /// selected key slot, none while `Unavailable` (#19). Blocking: call it off
    /// any async runtime worker (#42). See [`crate::connection_test`].
    pub fn test_connection(&self, req: ConnectionTestRequest) -> ConnectionTestResult {
        let timeouts = request_timeouts(&req);
        self.run_test(req, timeouts)
    }

    /// [`Self::test_connection`] with injected durations (tests only).
    #[cfg(any(test, feature = "test-fakes"))]
    pub fn test_connection_with_timeouts(
        &self,
        req: ConnectionTestRequest,
        timeouts: Timeouts,
    ) -> ConnectionTestResult {
        self.run_test(req, timeouts)
    }

    fn run_test(&self, req: ConnectionTestRequest, timeouts: Timeouts) -> ConnectionTestResult {
        let snapshot = self.snapshot();
        let ctx = TestContext {
            snapshot: &snapshot,
            unavailable: self.load.unavailable,
            credentials: self.deps.credentials.as_ref(),
            models: self.deps.local_models.as_ref(),
        };
        connection_test::run(&ctx, req, timeouts)
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
pub(crate) fn normalize(mut s: Settings) -> Settings {
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
pub(crate) fn key_step(edits: &KeyEdits, slot: KeySlot) -> KeyStep<'_> {
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
    use crate::autostart::{AutostartCall, AutostartError, FakeAutostart};
    use crate::clock::FakeClock;
    use crate::hotkey_registrar::{FakeHotkeyRegistrar, RegistrarCall};
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
        autostart: Arc<FakeAutostart>,
        hotkeys: Arc<FakeHotkeyRegistrar>,
    }

    impl World {
        /// The autostart entry starts absent; see [`World::with_autostart`].
        fn new(file: FakeSettingsFile, creds: FakeCredentialStore) -> World {
            World {
                file: Arc::new(file),
                creds: Arc::new(creds),
                autostart: Arc::new(FakeAutostart::new()),
                hotkeys: Arc::new(FakeHotkeyRegistrar::new()),
            }
        }

        fn with_autostart(mut self, autostart: FakeAutostart) -> World {
            self.autostart = Arc::new(autostart);
            self
        }

        fn deps(&self) -> SettingsDeps {
            SettingsDeps {
                file: self.file.clone(),
                credentials: self.creds.clone(),
                autostart: self.autostart.clone(),
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
            autostart: Arc::new(FakeAutostart::new()),
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

    /// A `Saved` whose warnings are exactly `expected` (order-insensitive; T-015).
    #[track_caller]
    fn expect_saved(outcome: SaveOutcome, expected: &[Warning]) -> SettingsView {
        match outcome {
            SaveOutcome::Saved { view, warnings } => {
                let key = |w: &Warning| (w.field.as_str(), w.code.as_str());
                let mut got = warnings.clone();
                got.sort_by_key(key);
                let mut want = expected.to_vec();
                want.sort_by_key(key);
                assert_eq!(got, want, "Saved warnings");
                view
            }
            other => panic!("expected Saved, got {other:?}"),
        }
    }

    /// The one warning T-015 defines, on `field`.
    fn insecure(field: FieldId) -> Warning {
        Warning {
            field,
            code: WarningCode::EndpointInsecure,
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
        expect_saved(
            service.save(req(sample(EngineKind::None), KeyEdits::default())),
            &[],
        );
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
        expect_saved(
            service.save(req(sample(EngineKind::None), KeyEdits::default())),
            &[],
        );
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
        // T-014: the first request turns autostart on (defaults: off); no call.
        assert!(
            world.autostart.calls().is_empty(),
            "{:?}",
            world.autostart.calls()
        );
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
        let view = expect_saved(service.save(req(sample(EngineKind::Api), keys)), &[]);

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
        expect_saved(service.save(req(sample(EngineKind::Api), keys)), &[]);
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
            let view = expect_saved(
                service.save(req(draft.clone(), all_keys(|| replace(blank)))),
                &[],
            );
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
            let saved = expect_saved(service.save(req(draft.clone(), keys)), &[]);

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
        let view = expect_saved(service.save(req(draft, keys)), &[]);
        assert_eq!(view.settings, expected);
        assert_eq!(*service.snapshot(), expected);
        assert_eq!(parse(&world.file.bytes().expect("file written")), expected);
    }

    // ---- T-015: endpoint.insecure warnings on save -------------------------------

    /// A save over a fresh first-run service with an API key in the store (so an
    /// `api` engine passes validation); returns the outcome and the world.
    fn save_with_api_key(draft: Settings) -> (SaveOutcome, World, SettingsService) {
        let world = World::new(
            FakeSettingsFile::new(),
            FakeCredentialStore::new().with_key(API, "sk-test-fake-api"),
        );
        let (service, _) = world.load();
        let outcome = service.save(req(draft, KeyEdits::default()));
        (outcome, world, service)
    }

    #[test]
    fn save_warns_on_http_remote_api_endpoint_and_still_saves() {
        // FR-29 / spec 004 US6-1 / decision #52: engine api + http://example.com/v1
        // saves (file written, snapshot swapped) and returns exactly one warning
        // {engine.api.base_url, endpoint.insecure, settings.warning.endpoint_insecure}.
        // Bite: `warnings: Vec::new()` in step (8); the warning turned into a refusal;
        // the warning on another field; `message` missing from the wire.
        let mut draft = sample(EngineKind::Api);
        draft.api.base_url = " http://example.com/v1/ ".into();
        draft.post_processing.enabled = false;
        let mut expected = normalize(draft.clone());
        expected.api.base_url = "http://example.com/v1".into();

        let (outcome, world, service) = save_with_api_key(draft);
        let wire_warnings = match &outcome {
            SaveOutcome::Saved { warnings, .. } => {
                serde_json::to_value(warnings).expect("warnings serialize")
            }
            other => panic!("expected Saved, got {other:?}"),
        };
        let view = expect_saved(outcome, &[insecure(FieldId::EngineApiBaseUrl)]);

        assert_eq!(view.settings, expected);
        assert_eq!(*service.snapshot(), expected);
        assert_eq!(parse(&world.file.bytes().expect("file written")), expected);
        assert_eq!(
            wire_warnings,
            serde_json::json!([{
                "field": "engine.api.base_url",
                "code": "endpoint.insecure",
                "message": "settings.warning.endpoint_insecure",
            }])
        );
    }

    #[test]
    fn save_warns_on_http_remote_local_server_endpoint() {
        // US6-1 for the other engine URL: the field is the selected engine's own.
        // Bite: the field hard-coded to engine.api.base_url, or only the api URL
        // checked. sample's local server is http://192.0.2.10:8000/v1.
        let mut draft = sample(EngineKind::LocalServer);
        draft.post_processing.enabled = false;
        let world = World::new(FakeSettingsFile::new(), FakeCredentialStore::new());
        let (service, _) = world.load();
        expect_saved(
            service.save(req(draft, KeyEdits::default())),
            &[insecure(FieldId::EngineLocalServerBaseUrl)],
        );
    }

    #[test]
    fn save_does_not_warn_on_loopback_local_server() {
        // Acceptance failure branch: http on loopback (any letter case, 127.0.0.0/8,
        // ::1) is local, so no warning. Bite: `scheme == "http"` alone as the rule,
        // or a case-sensitive "localhost" text compare.
        for url in [
            "http://LOCALHOST:8000/v1",
            "http://localhost:8000/v1/",
            "http://127.0.0.1",
            "http://127.1.2.3:9000/v1",
            "http://[::1]",
            "http://[::1]:8000/v1",
        ] {
            let mut draft = sample(EngineKind::LocalServer);
            draft.local_server.base_url = url.into();
            draft.post_processing.enabled = false;
            let world = World::new(FakeSettingsFile::new(), FakeCredentialStore::new());
            let (service, _) = world.load();
            let view = expect_saved(service.save(req(draft, KeyEdits::default())), &[]);
            assert_eq!(view.settings.local_server.base_url, normalize_base_url(url));
        }
    }

    #[test]
    fn save_ignores_http_remote_url_of_an_engine_not_in_use() {
        // US6-1: only URLs in use warn. engine api over https, a stale
        // http://192.0.2.10 local-server URL and an http post-processing URL with
        // post-processing off stay quiet. Bite: every stored base URL checked
        // regardless of engine / `post_processing.enabled`.
        let mut draft = sample(EngineKind::Api);
        draft.local_server.base_url = "http://192.0.2.10:8000/v1".into();
        draft.post_processing.enabled = false;
        draft.post_processing.base_url = "http://llm.example.com/v1".into();
        let (outcome, _, _) = save_with_api_key(draft);
        expect_saved(outcome, &[]);

        // engine none: no engine URL is in use at all.
        let mut draft = sample(EngineKind::None);
        draft.api.base_url = "http://example.com/v1".into();
        draft.post_processing.enabled = false;
        let (outcome, _, _) = save_with_api_key(draft);
        expect_saved(outcome, &[]);
    }

    #[test]
    fn save_warns_on_http_remote_post_processing_endpoint_only_while_enabled() {
        // The post-processing URL is in use while `post_processing.enabled`
        // (decision #52), whatever engine is selected. Bite: post_processing.base_url
        // never checked, or checked with post-processing off.
        for engine in [EngineKind::Api, EngineKind::None] {
            let mut on = sample(engine);
            on.post_processing.enabled = true;
            on.post_processing.base_url = "http://llm.example.com/v1".into();
            let (outcome, _, _) = save_with_api_key(on.clone());
            expect_saved(outcome, &[insecure(FieldId::PostProcessingBaseUrl)]);

            let mut off = on;
            off.post_processing.enabled = false;
            let (outcome, _, _) = save_with_api_key(off);
            expect_saved(outcome, &[]);
        }
    }

    #[test]
    fn save_lists_every_insecure_url_in_use() {
        // Two URLs in use, both http remote -> two warnings. Bite: the first hit
        // returned alone.
        let mut draft = sample(EngineKind::Api);
        draft.api.base_url = "http://192.0.2.20/v1".into();
        draft.post_processing.enabled = true;
        draft.post_processing.base_url = "http://198.51.100.7:8080/v1".into();
        let (outcome, _, _) = save_with_api_key(draft);
        expect_saved(
            outcome,
            &[
                insecure(FieldId::EngineApiBaseUrl),
                insecure(FieldId::PostProcessingBaseUrl),
            ],
        );
    }

    #[test]
    fn save_warnings_skips_a_url_that_fails_check_base_url() {
        // Direct test of save_warnings (a save refuses these URLs at validation, step
        // (3), so it never reaches the warning step (8) with them). An in-use URL
        // that fails check_base_url gets no warning and no panic: the classifier
        // runs only on a URL that parsed. Every in-use URL is fed malformed: the
        // post-processing URL (engine api and none), the api URL and the
        // local_server URL. Bite: check_base_url(..).unwrap()/expect() or a warning
        // decided on the raw text.
        let unusable = [
            "",
            "http//llm.example.com",
            "llm.example.com/v1",
            "ftp://llm.example.com",
        ];
        for url in unusable {
            for engine in [EngineKind::Api, EngineKind::None] {
                let mut s = sample(engine);
                s.post_processing.enabled = true;
                s.post_processing.base_url = url.into();
                assert_eq!(save_warnings(&s), vec![], "{engine:?} pp {url:?}");
            }

            let mut s = sample(EngineKind::Api);
            s.api.base_url = url.into();
            s.post_processing.enabled = true;
            s.post_processing.base_url = url.into();
            assert_eq!(save_warnings(&s), vec![], "api+pp {url:?}");

            let mut s = sample(EngineKind::LocalServer);
            s.local_server.base_url = url.into();
            assert_eq!(save_warnings(&s), vec![], "local_server {url:?}");

            // A malformed URL beside an insecure one: only the usable one warns.
            let mut s = sample(EngineKind::Api);
            s.api.base_url = "http://192.0.2.20/v1".into();
            s.post_processing.enabled = true;
            s.post_processing.base_url = url.into();
            assert_eq!(
                save_warnings(&s),
                vec![insecure(FieldId::EngineApiBaseUrl)],
                "insecure api + pp {url:?}"
            );
        }
    }

    #[test]
    fn save_refuses_an_unusable_post_processing_url_while_enabled() {
        // T-021 (replaces save_gives_no_warning_for_an_unchecked_post_processing_url,
        // which pinned the gap: these URLs were Saved while post-processing was on).
        // With post-processing on, an empty or malformed post_processing.base_url is
        // Refused with that field's code from the engine URL rule; nothing is written,
        // the snapshot is kept and there is no warning (a refusal carries none). The
        // refusal comes before step (8), so this test never reaches save_warnings;
        // its no-unwrap guarantee is pinned by
        // save_warnings_skips_a_url_that_fails_check_base_url. Bite: no
        // post-processing rule in validate (Saved), the rule's error on another field
        // or with another code, or a write before validation.
        for (url, code) in [
            ("", ErrorCode::Required),
            ("http//llm.example.com", ErrorCode::UrlMalformed),
            ("llm.example.com/v1", ErrorCode::UrlMalformed),
            ("ftp://llm.example.com", ErrorCode::UrlMalformed),
        ] {
            let world = World::new(
                FakeSettingsFile::new(),
                FakeCredentialStore::new().with_key(API, "sk-test-fake-api"),
            );
            let (service, _) = world.load();
            let written_before = world.file.bytes();
            let before = service.snapshot();
            let mut draft = sample(EngineKind::Api);
            draft.post_processing.enabled = true;
            draft.post_processing.base_url = url.into();
            let (errors, form_error) =
                expect_refused(service.save(req(draft, KeyEdits::default())));
            assert_eq!(
                errors,
                vec![field_error(FieldId::PostProcessingBaseUrl, code)],
                "base_url {url:?}"
            );
            assert_eq!(form_error, None, "base_url {url:?}");
            assert_eq!(world.file.bytes(), written_before, "base_url {url:?}");
            assert!(
                Arc::ptr_eq(&before, &service.snapshot()),
                "base_url {url:?}"
            );
        }
    }

    #[test]
    fn refused_save_carries_no_warning_and_writes_nothing() {
        // The warning never replaces or masks a refusal: an http remote api URL with
        // a missing key is Refused{key.required} (no Saved, no file).
        let mut draft = sample(EngineKind::Api);
        draft.api.base_url = "http://example.com/v1".into();
        let world = World::new(FakeSettingsFile::new(), FakeCredentialStore::new());
        let (service, _) = world.load();
        let written_before = world.file.bytes();
        let (errors, form_error) = expect_refused(service.save(req(draft, KeyEdits::default())));
        assert_eq!(
            errors,
            vec![field_error(FieldId::EngineApiKey, ErrorCode::KeyRequired)]
        );
        assert_eq!(form_error, None);
        assert_eq!(world.file.bytes(), written_before);
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
    fn refused_timeout_saves_nothing() {
        // T-073 failure branch: an out-of-range timeout is refused on save with its
        // field error and nothing is saved: no file write, no key change, no
        // autostart or hotkey call, the snapshot kept, no subscriber message. The
        // timeouts rule refuses whatever engine is selected (here api with a valid
        // setup, so timeouts.local_server is the only error). Then the same draft
        // with an in-range value is Saved and the value is in the file and the
        // snapshot (the field is persisted, not dropped). Bite: no timeouts rule in
        // validate (Saved), the rule only for the selected engine's limit, another
        // field or code, or the timeouts not serialized to the file.
        let on_disk = with_start(sample(EngineKind::None), false);
        let bytes = json(&on_disk);
        let world = World::new(
            FakeSettingsFile::with_bytes(&bytes),
            FakeCredentialStore::new().with_key(LOCAL, "sk-test-old-local"),
        );
        let (service, _) = world.load();
        let rx = service.subscribe();
        let before = service.snapshot();

        let mut draft = with_start(sample(EngineKind::Api), true);
        draft.hotkey = "Ctrl+Shift+F10".into();
        draft.timeouts.local_server_s = 0;
        let keys = || KeyEdits {
            transcription_api: replace("sk-test-new-api"),
            local_server: KeyEdit::Clear,
            post_processing: replace("sk-test-new-pp"),
        };
        let (errors, form) = expect_refused(service.save(req(draft.clone(), keys())));
        assert_eq!(
            errors,
            vec![field_error(
                FieldId::TimeoutsLocalServer,
                ErrorCode::TimeoutRange
            )]
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
            [None, Some("sk-test-old-local".to_string()), None]
        );
        assert!(
            world.autostart.calls().is_empty(),
            "{:?}",
            world.autostart.calls()
        );
        assert!(
            world.hotkeys.calls().is_empty(),
            "{:?}",
            world.hotkeys.calls()
        );
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));

        // The control: in range, the same save goes through and the value is stored.
        draft.timeouts.local_server_s = 1800;
        let view = expect_saved(service.save(req(draft, keys())), &[]);
        assert_eq!(view.settings.timeouts.local_server_s, 1800);
        assert_eq!(service.snapshot().timeouts.local_server_s, 1800);
        let written = world.file.bytes().expect("settings written");
        assert_eq!(parse(&written).timeouts.local_server_s, 1800);
        assert_eq!(parse(&written).timeouts.connect_s, 7);
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

    // ---- T-014: autostart step of save, and reconcile_autostart ----------------

    /// A fake OS error code (no meaning): ERROR_ACCESS_DENIED.
    const RUN_ERR: AutostartError = AutostartError { os_code: 5 };

    fn with_start(mut s: Settings, on: bool) -> Settings {
        s.start_with_windows = on;
        s
    }

    /// One state-changing call on any dependency, in the order the service made it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Step {
        Key(CredentialOp, KeySlot),
        Autostart(bool),
        WriteFile,
        /// T-055: a registrar call (`JournaledHotkeys`).
        Hotkey(RegistrarCall),
        /// T-055: at a commit, the snapshot already held the committed hotkey.
        Swapped,
        /// T-055: a subscriber received a snapshot (noted at a commit, or by the
        /// test after the save).
        Published,
    }

    type Journal = Arc<Mutex<Vec<Step>>>;

    fn note(journal: &Journal, step: Step) {
        journal
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(step);
    }

    fn steps(journal: &Journal) -> Vec<Step> {
        journal
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Delegates to the World's fake and notes every change in one shared journal,
    /// so an order across dependencies (keys vs. autostart vs. file) is visible.
    struct JournaledCreds(Arc<FakeCredentialStore>, Journal);

    impl CredentialStore for JournaledCreds {
        fn read(&self, slot: KeySlot) -> Result<Option<Secret>, CredentialError> {
            self.0.read(slot)
        }
        fn write(&self, slot: KeySlot, secret: &Secret) -> Result<(), CredentialError> {
            note(&self.1, Step::Key(CredentialOp::Write, slot));
            self.0.write(slot, secret)
        }
        fn delete(&self, slot: KeySlot) -> Result<(), CredentialError> {
            note(&self.1, Step::Key(CredentialOp::Delete, slot));
            self.0.delete(slot)
        }
    }

    struct JournaledAutostart(Arc<FakeAutostart>, Journal);

    impl Autostart for JournaledAutostart {
        fn is_enabled(&self) -> Result<bool, AutostartError> {
            self.0.is_enabled()
        }
        fn set(&self, enabled: bool) -> Result<(), AutostartError> {
            note(&self.1, Step::Autostart(enabled));
            self.0.set(enabled)
        }
    }

    struct JournaledFile(Arc<FakeSettingsFile>, Journal);

    impl SettingsFile for JournaledFile {
        fn read(&self) -> io::Result<Option<Vec<u8>>> {
            self.0.read()
        }
        fn write_atomic(&self, bytes: &[u8]) -> io::Result<()> {
            note(&self.1, Step::WriteFile);
            self.0.write_atomic(bytes)
        }
        fn move_aside(&self, suffix: &str) -> io::Result<String> {
            self.0.move_aside(suffix)
        }
    }

    impl World {
        /// `load` with every changing call noted in one journal.
        fn load_journaled(&self) -> (SettingsService, Journal) {
            let journal: Journal = Arc::new(Mutex::new(Vec::new()));
            let mut deps = self.deps();
            deps.file = Arc::new(JournaledFile(self.file.clone(), journal.clone()));
            deps.credentials = Arc::new(JournaledCreds(self.creds.clone(), journal.clone()));
            deps.autostart = Arc::new(JournaledAutostart(self.autostart.clone(), journal.clone()));
            let (service, _) = SettingsService::load_or_init(deps, OS);
            (service, journal)
        }
    }

    #[test]
    fn autostart_failure_refuses_and_changes_nothing() {
        // Acceptance failure branch (Q1): the Run value cannot be written → the save
        // is refused as a whole, old state kept. Bite: the autostart step missing
        // (Saved), placed after the keys or the file (a key or the file changed), the
        // error on another field or code, or the snapshot swapped / published.
        let on_disk = with_start(sample(EngineKind::None), false);
        let bytes = json(&on_disk);
        let world = World::new(
            FakeSettingsFile::with_bytes(&bytes),
            FakeCredentialStore::new().with_key(API, "sk-test-old-api"),
        );
        world.autostart.fail_set(true, RUN_ERR);
        let (service, _) = world.load();
        let rx = service.subscribe();
        let before = service.snapshot();

        let mut draft = with_start(sample(EngineKind::None), true);
        draft.history.size = 42;
        let keys = KeyEdits {
            transcription_api: replace("sk-test-new-api"),
            local_server: replace("sk-test-new-local"),
            post_processing: KeyEdit::Clear,
        };
        assert_eq!(
            service.save(req(draft, keys)),
            SaveOutcome::Refused {
                errors: vec![field_error(
                    FieldId::GeneralStartWithWindows,
                    ErrorCode::AutostartFailed
                )],
                form_error: None,
            }
        );

        assert_eq!(world.autostart.calls(), vec![AutostartCall::Set(true)]);
        assert!(!world.autostart.is_on(), "Run value written anyway");
        assert!(
            changes(&world.creds).is_empty(),
            "{:?}",
            changes(&world.creds)
        );
        assert_eq!(
            stored(&world.creds),
            [Some("sk-test-old-api".to_string()), None, None]
        );
        assert_eq!(world.file.calls(), vec![FileCall::Read], "file touched");
        assert_eq!(world.file.bytes(), Some(bytes));
        assert!(Arc::ptr_eq(&before, &service.snapshot()));
        assert_eq!(*service.snapshot(), on_disk);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));

        // The other direction: removing the value fails.
        let on_disk = with_start(sample(EngineKind::None), true);
        let bytes = json(&on_disk);
        let world = World::new(
            FakeSettingsFile::with_bytes(&bytes),
            FakeCredentialStore::new(),
        )
        .with_autostart(FakeAutostart::enabled());
        world.autostart.fail_set(false, RUN_ERR);
        let (service, _) = world.load();
        let before = service.snapshot();
        let (errors, form) = expect_refused(service.save(req(
            with_start(sample(EngineKind::None), false),
            all_keys(|| replace("sk-test-new")),
        )));
        assert_eq!(
            errors,
            vec![field_error(
                FieldId::GeneralStartWithWindows,
                ErrorCode::AutostartFailed
            )]
        );
        assert_eq!(form, None);
        assert_eq!(world.autostart.calls(), vec![AutostartCall::Set(false)]);
        assert!(world.autostart.is_on());
        assert!(changes(&world.creds).is_empty());
        assert_eq!(world.file.calls(), vec![FileCall::Read]);
        assert_eq!(world.file.bytes(), Some(bytes));
        assert!(Arc::ptr_eq(&before, &service.snapshot()));
    }

    #[test]
    fn autostart_applied_only_when_changed() {
        // T056 "applied only when changed", against the snapshot. Bite: the step
        // skipped (Saved with the Run value unchanged), run on every save (an
        // unrelated save refused while the Run key is unwritable), compared with the
        // wrong side (draft vs. draft), or run before validate.
        let world = World::new(
            FakeSettingsFile::with_bytes(&json(&with_start(sample(EngineKind::None), false))),
            FakeCredentialStore::new(),
        );
        let (service, _) = world.load();

        // off → on: one set(true), and the value is in force when Saved returns.
        let view = expect_saved(
            service.save(req(
                with_start(sample(EngineKind::None), true),
                KeyEdits::default(),
            )),
            &[],
        );
        assert!(view.settings.start_with_windows);
        assert_eq!(world.autostart.calls(), vec![AutostartCall::Set(true)]);
        assert!(world.autostart.is_on());
        assert!(parse(&world.file.bytes().expect("written")).start_with_windows);

        // on → on with another change: no call, even when the Run key is unwritable.
        world.autostart.fail_set(true, RUN_ERR);
        world.autostart.fail_set(false, RUN_ERR);
        world.autostart.fail_is_enabled(RUN_ERR);
        let mut draft = with_start(sample(EngineKind::None), true);
        draft.history.size = 42;
        expect_saved(service.save(req(draft.clone(), KeyEdits::default())), &[]);
        assert_eq!(*service.snapshot(), draft);
        assert_eq!(world.autostart.calls(), vec![AutostartCall::Set(true)]);
        world.autostart.clear_failures();

        // An invalid draft that would turn it off: refused by validate, no call.
        let mut invalid = with_start(sample(EngineKind::None), false);
        invalid.history.size = 0;
        let (errors, _) = expect_refused(service.save(req(invalid, KeyEdits::default())));
        assert_eq!(
            errors,
            vec![field_error(
                FieldId::HistorySize,
                ErrorCode::HistorySizeRange
            )]
        );
        assert_eq!(world.autostart.calls(), vec![AutostartCall::Set(true)]);
        assert!(world.autostart.is_on());

        // on → off: one set(false).
        expect_saved(
            service.save(req(
                with_start(sample(EngineKind::None), false),
                KeyEdits::default(),
            )),
            &[],
        );
        assert_eq!(
            world.autostart.calls(),
            vec![AutostartCall::Set(true), AutostartCall::Set(false)]
        );
        assert!(!world.autostart.is_on());
        assert!(!service.snapshot().start_with_windows);
    }

    #[test]
    fn autostart_step_runs_before_the_keys_and_the_file() {
        // R-3 step 3: after validate and the hotkey placeholder, before the keys,
        // the file last. Bite: the autostart step moved after the keys or after
        // write_atomic (then a Run-key failure would have to undo more).
        let world = World::new(
            FakeSettingsFile::with_bytes(&json(&with_start(sample(EngineKind::None), false))),
            FakeCredentialStore::new(),
        );
        let (service, journal) = world.load_journaled();
        let keys = KeyEdits {
            transcription_api: replace("sk-test-new-api"),
            ..KeyEdits::default()
        };
        expect_saved(
            service.save(req(with_start(sample(EngineKind::None), true), keys)),
            &[],
        );
        assert_eq!(
            steps(&journal),
            vec![
                Step::Autostart(true),
                Step::Key(CredentialOp::Write, API),
                Step::WriteFile,
            ]
        );
    }

    #[test]
    fn key_failure_restores_autostart() {
        // I2 with the autostart step: a later key failure undoes it. Bite: the
        // autostart undo missing (Run value left on after Refused), undone before
        // the keys, or undone with the new value.
        let world = World::new(
            FakeSettingsFile::with_bytes(&json(&with_start(sample(EngineKind::None), false))),
            FakeCredentialStore::new(),
        );
        world.creds.fail(CredentialOp::Write, LOCAL, STORE_ERR);
        let (service, journal) = world.load_journaled();
        let before = service.snapshot();

        let keys = KeyEdits {
            transcription_api: replace("sk-test-new-api"),
            local_server: replace("sk-test-new-local"),
            post_processing: KeyEdit::Untouched,
        };
        let (errors, form) =
            expect_refused(service.save(req(with_start(sample(EngineKind::None), true), keys)));
        assert_eq!(
            errors,
            vec![field_error(
                FieldId::EngineLocalServerKey,
                ErrorCode::KeyStoreFailed
            )]
        );
        assert_eq!(form, None);
        assert_eq!(
            world.autostart.calls(),
            vec![AutostartCall::Set(true), AutostartCall::Set(false)]
        );
        assert!(!world.autostart.is_on(), "Run value left on");
        assert_eq!(
            steps(&journal),
            vec![
                Step::Autostart(true),
                Step::Key(CredentialOp::Write, API),
                Step::Key(CredentialOp::Write, LOCAL),
                // Undo, in reverse: keys, then autostart.
                Step::Key(CredentialOp::Delete, API),
                Step::Autostart(false),
            ]
        );
        assert_eq!(stored(&world.creds), [None, None, None]);
        assert_eq!(world.file.calls(), vec![FileCall::Read]);
        assert!(Arc::ptr_eq(&before, &service.snapshot()));
    }

    #[test]
    fn file_write_failure_restores_keys_then_autostart() {
        // I2: a file write failure undoes the keys in reverse slot order, then the
        // autostart step. Bite: the autostart undo missing or done first, or
        // WriteFailed replaced although every undo succeeded.
        let on_disk = with_start(sample(EngineKind::None), true);
        let bytes = json(&on_disk);
        let world = World::new(
            FakeSettingsFile::with_bytes(&bytes),
            FakeCredentialStore::new().with_key(API, "sk-test-old-api"),
        )
        .with_autostart(FakeAutostart::enabled());
        let (service, journal) = world.load_journaled();
        let rx = service.subscribe();
        let before = service.snapshot();
        world.file.fail_write(io::ErrorKind::Other);

        let keys = KeyEdits {
            transcription_api: replace("sk-test-new-api"),
            local_server: KeyEdit::Untouched,
            post_processing: replace("sk-test-new-pp"),
        };
        let (errors, form) =
            expect_refused(service.save(req(with_start(sample(EngineKind::None), false), keys)));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(form, Some(FormError::WriteFailed));
        assert_eq!(
            steps(&journal),
            vec![
                Step::Autostart(false),
                Step::Key(CredentialOp::Write, API),
                Step::Key(CredentialOp::Write, PP),
                Step::WriteFile,
                // Undo, in reverse: keys, then autostart.
                Step::Key(CredentialOp::Delete, PP),
                Step::Key(CredentialOp::Write, API),
                Step::Autostart(true),
            ]
        );
        assert!(world.autostart.is_on(), "Run value left removed");
        assert_eq!(
            stored(&world.creds),
            [Some("sk-test-old-api".to_string()), None, None]
        );
        assert_eq!(world.file.bytes(), Some(bytes));
        assert!(Arc::ptr_eq(&before, &service.snapshot()));
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn autostart_undo_failure_reports_partially_restored() {
        // R-3 double failure: a failed autostart undo is named in not_restored, in
        // step order (autostart before the key fields). Bite: the failed undo hidden
        // (plain key.store_failed / WriteFailed), the field missing or after the
        // keys, or the key undo skipped after the autostart undo fails.

        // (a) A key failure; the autostart undo fails.
        let world = World::new(
            FakeSettingsFile::with_bytes(&json(&with_start(sample(EngineKind::None), false))),
            FakeCredentialStore::new(),
        );
        world.creds.fail(CredentialOp::Write, LOCAL, STORE_ERR);
        world.autostart.fail_set(false, RUN_ERR);
        let (service, _) = world.load();
        let keys = KeyEdits {
            local_server: replace("sk-test-new-local"),
            ..KeyEdits::default()
        };
        let (errors, form) =
            expect_refused(service.save(req(with_start(sample(EngineKind::None), true), keys)));
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
                not_restored: vec![FieldId::GeneralStartWithWindows],
            })
        );
        assert_eq!(
            world.autostart.calls(),
            vec![AutostartCall::Set(true), AutostartCall::Set(false)]
        );
        // The one documented leftover (R-3).
        assert!(world.autostart.is_on());

        // (b) A file write failure; only the autostart undo fails: PartiallyRestored
        // replaces WriteFailed.
        let world = World::new(
            FakeSettingsFile::with_bytes(&json(&with_start(sample(EngineKind::None), false))),
            FakeCredentialStore::new(),
        );
        world.autostart.fail_set(false, RUN_ERR);
        let (service, _) = world.load();
        world.file.fail_write(io::ErrorKind::Other);
        let (errors, form) = expect_refused(service.save(req(
            with_start(sample(EngineKind::None), true),
            KeyEdits::default(),
        )));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            form,
            Some(FormError::PartiallyRestored {
                not_restored: vec![FieldId::GeneralStartWithWindows],
            })
        );

        // (c) A file write failure; the API key undo and the autostart undo both
        // fail: every undo is tried, fields in step order.
        let world = World::new(
            FakeSettingsFile::with_bytes(&json(&with_start(sample(EngineKind::None), false))),
            FakeCredentialStore::new(),
        );
        world.creds.fail(CredentialOp::Delete, API, STORE_ERR);
        world.autostart.fail_set(false, RUN_ERR);
        let (service, journal) = world.load_journaled();
        world.file.fail_write(io::ErrorKind::Other);
        let keys = KeyEdits {
            transcription_api: replace("sk-test-new-api"),
            ..KeyEdits::default()
        };
        let (errors, form) =
            expect_refused(service.save(req(with_start(sample(EngineKind::None), true), keys)));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            form,
            Some(FormError::PartiallyRestored {
                not_restored: vec![FieldId::GeneralStartWithWindows, FieldId::EngineApiKey],
            })
        );
        assert_eq!(
            steps(&journal),
            vec![
                Step::Autostart(true),
                Step::Key(CredentialOp::Write, API),
                Step::WriteFile,
                Step::Key(CredentialOp::Delete, API),
                Step::Autostart(false),
            ]
        );
    }

    /// The file and credential calls after a load, for "reconcile touched neither".
    #[track_caller]
    fn assert_reconcile_touched_nothing_else(world: &World, after_load: &[FileCall]) {
        assert_eq!(world.file.calls(), after_load, "reconcile touched the file");
        assert!(
            world.creds.calls().is_empty(),
            "reconcile called the credential store: {:?}",
            world.creds.calls()
        );
    }

    #[test]
    fn reconcile_autostart_on_writes_the_entry() {
        // R-5: snapshot on → set(true) whether or not the entry exists (refreshes a
        // stale path) → Written. Bite: set(true) skipped when is_enabled says
        // present, a wrong action, or the file / store touched.
        for present in [false, true] {
            let fake = if present {
                FakeAutostart::enabled()
            } else {
                FakeAutostart::new()
            };
            let world = World::new(
                FakeSettingsFile::with_bytes(&json(&with_start(sample(EngineKind::None), true))),
                FakeCredentialStore::new(),
            )
            .with_autostart(fake);
            let (service, _) = world.load();

            assert_eq!(
                service.reconcile_autostart(),
                ReconcileAction::Written,
                "present={present}"
            );
            assert!(
                world.autostart.calls().contains(&AutostartCall::Set(true)),
                "present={present}: {:?}",
                world.autostart.calls()
            );
            assert!(
                !world.autostart.calls().contains(&AutostartCall::Set(false)),
                "present={present}: {:?}",
                world.autostart.calls()
            );
            assert!(world.autostart.is_on(), "present={present}");
            assert_reconcile_touched_nothing_else(&world, &[FileCall::Read]);
        }
    }

    #[test]
    fn reconcile_autostart_off_removes_a_present_entry() {
        // R-5: snapshot off + entry present → set(false) → Removed. Bite: the
        // leftover kept, or removed without asking is_enabled first.
        let world = World::new(
            FakeSettingsFile::with_bytes(&json(&with_start(sample(EngineKind::None), false))),
            FakeCredentialStore::new(),
        )
        .with_autostart(FakeAutostart::enabled());
        let (service, _) = world.load();

        assert_eq!(service.reconcile_autostart(), ReconcileAction::Removed);
        assert_eq!(
            world.autostart.calls(),
            vec![AutostartCall::IsEnabled, AutostartCall::Set(false)]
        );
        assert!(!world.autostart.is_on());
        assert_reconcile_touched_nothing_else(&world, &[FileCall::Read]);
    }

    #[test]
    fn reconcile_autostart_off_and_absent_is_none_without_a_write() {
        // Default-off users: one read, no OS write (Investigation: "is_enabled read
        // only"). Bite: an unconditional set(false), or a wrong action.
        let world = World::new(
            FakeSettingsFile::with_bytes(&json(&with_start(sample(EngineKind::None), false))),
            FakeCredentialStore::new(),
        );
        let (service, _) = world.load();

        assert_eq!(service.reconcile_autostart(), ReconcileAction::None);
        assert_eq!(world.autostart.calls(), vec![AutostartCall::IsEnabled]);
        assert!(!world.autostart.is_on());
        assert_reconcile_touched_nothing_else(&world, &[FileCall::Read]);
    }

    #[test]
    fn reconcile_autostart_while_unavailable_makes_no_call() {
        // #19 by analogy: the in-memory defaults say off; acting on them would
        // delete the entry of a user whose real file merely could not be read.
        // Bite: reconcile run from the defaults (set(false) on a present entry) or
        // even an is_enabled read.
        let world = World::new(
            FakeSettingsFile::with_bytes(&json(&with_start(sample(EngineKind::None), true))),
            FakeCredentialStore::new(),
        )
        .with_autostart(FakeAutostart::enabled());
        world.file.fail_read(io::ErrorKind::PermissionDenied);
        let (service, outcome) = world.load();
        assert_eq!(outcome, LoadOutcome::Unavailable(defaults(OS)));

        assert_eq!(service.reconcile_autostart(), ReconcileAction::None);
        assert!(
            world.autostart.calls().is_empty(),
            "{:?}",
            world.autostart.calls()
        );
        assert!(world.autostart.is_on(), "entry removed while Unavailable");
        assert_reconcile_touched_nothing_else(&world, &[FileCall::Read]);
    }

    #[test]
    fn reconcile_autostart_errors_are_failed() {
        // R-11: any Autostart error → Failed, nothing else done. Bite: an error
        // swallowed into Written / Removed / None, or set(false) after a failed
        // is_enabled.
        // on + set(true) fails.
        let world = World::new(
            FakeSettingsFile::with_bytes(&json(&with_start(sample(EngineKind::None), true))),
            FakeCredentialStore::new(),
        );
        world.autostart.fail_set(true, RUN_ERR);
        let (service, _) = world.load();
        assert_eq!(service.reconcile_autostart(), ReconcileAction::Failed);
        assert!(world.autostart.calls().contains(&AutostartCall::Set(true)));
        assert!(!world.autostart.is_on());
        assert_reconcile_touched_nothing_else(&world, &[FileCall::Read]);

        // off + present + set(false) fails.
        let world = World::new(
            FakeSettingsFile::with_bytes(&json(&with_start(sample(EngineKind::None), false))),
            FakeCredentialStore::new(),
        )
        .with_autostart(FakeAutostart::enabled());
        world.autostart.fail_set(false, RUN_ERR);
        let (service, _) = world.load();
        assert_eq!(service.reconcile_autostart(), ReconcileAction::Failed);
        assert_eq!(
            world.autostart.calls(),
            vec![AutostartCall::IsEnabled, AutostartCall::Set(false)]
        );
        assert!(world.autostart.is_on());

        // off + is_enabled fails: no set at all.
        let world = World::new(
            FakeSettingsFile::with_bytes(&json(&with_start(sample(EngineKind::None), false))),
            FakeCredentialStore::new(),
        )
        .with_autostart(FakeAutostart::enabled());
        world.autostart.fail_is_enabled(RUN_ERR);
        let (service, _) = world.load();
        assert_eq!(service.reconcile_autostart(), ReconcileAction::Failed);
        assert_eq!(world.autostart.calls(), vec![AutostartCall::IsEnabled]);
        assert!(world.autostart.is_on());
        assert_reconcile_touched_nothing_else(&world, &[FileCall::Read]);
    }

    #[test]
    fn reconcile_autostart_first_run_and_reset_remove_a_leftover() {
        // R-3/R-5: FirstRun and Reset reconcile from the defaults (off), matching
        // the settings window. Bite: reconcile skipped for FirstRun / Reset like
        // for Unavailable, or run before the defaults are in the snapshot.
        // FirstRun.
        let world = World::new(FakeSettingsFile::new(), FakeCredentialStore::new())
            .with_autostart(FakeAutostart::enabled());
        let (service, outcome) = world.load();
        assert_eq!(outcome, LoadOutcome::FirstRun(defaults(OS)));
        assert_eq!(service.reconcile_autostart(), ReconcileAction::Removed);
        assert!(!world.autostart.is_on());
        assert_eq!(
            world.autostart.calls(),
            vec![AutostartCall::IsEnabled, AutostartCall::Set(false)]
        );
        assert_reconcile_touched_nothing_else(&world, &[FileCall::Read, FileCall::WriteAtomic]);

        // Reset.
        let world = World::new(
            FakeSettingsFile::with_bytes(b"{not json"),
            FakeCredentialStore::new(),
        )
        .with_autostart(FakeAutostart::enabled());
        let (service, outcome) = world.load();
        assert!(matches!(outcome, LoadOutcome::Reset { .. }), "{outcome:?}");
        assert_eq!(service.reconcile_autostart(), ReconcileAction::Removed);
        assert!(!world.autostart.is_on());
        assert_reconcile_touched_nothing_else(
            &world,
            &[
                FileCall::Read,
                FileCall::MoveAside(SUFFIX.to_string()),
                FileCall::WriteAtomic,
            ],
        );
    }

    #[test]
    fn reconcile_autostart_follows_the_saved_snapshot() {
        // Reconcile reads the snapshot in force, not the load-time file. Bite: the
        // value cached at load used instead of snapshot().
        let world = World::new(
            FakeSettingsFile::with_bytes(&json(&with_start(sample(EngineKind::None), true))),
            FakeCredentialStore::new(),
        )
        .with_autostart(FakeAutostart::enabled());
        let (service, _) = world.load();
        expect_saved(
            service.save(req(
                with_start(sample(EngineKind::None), false),
                KeyEdits::default(),
            )),
            &[],
        );
        // The entry comes back behind the service's back (e.g. a stale value).
        world.autostart.set(true).expect("fake set");
        let before = world.autostart.calls().len();

        assert_eq!(service.reconcile_autostart(), ReconcileAction::Removed);
        assert_eq!(
            world.autostart.calls()[before..],
            [AutostartCall::IsEnabled, AutostartCall::Set(false)]
        );
        assert!(!world.autostart.is_on());
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

    // ---- T-055: hotkey step of save (spec 004 R-3 steps 2/6, T032) ----------------
    //
    // The registrar is the World's `FakeHotkeyRegistrar` behind `JournaledHotkeys`,
    // which notes every call in the shared journal next to the autostart, key and
    // file steps. At `commit` it also notes, before the commit itself, whether the
    // snapshot was already swapped (`Swapped`) and whether a subscriber already got
    // the new snapshot (`Published`), so "commit after the file, before the swap and
    // the publish" is one comparison of the journal.

    /// The hotkey every T-055 draft saves: not the `sample` one (Ctrl+Shift+F9).
    const NEW_HOTKEY: &str = "Ctrl+Alt+F9";

    fn hotkey_of(text: &str) -> crate::settings::hotkey::Hotkey {
        crate::settings::hotkey::parse_hotkey(text).expect("a canonical test hotkey")
    }

    /// What `JournaledHotkeys` looks at when `commit` is called.
    #[derive(Default)]
    struct CommitProbe {
        service: Mutex<Option<std::sync::Weak<SettingsService>>>,
        rx: Mutex<Option<mpsc::Receiver<Arc<Settings>>>>,
    }

    impl CommitProbe {
        /// Watches `service` (its snapshot and one subscription taken now).
        fn watch(&self, service: &Arc<SettingsService>) {
            *self.rx.lock().unwrap_or_else(PoisonError::into_inner) = Some(service.subscribe());
            *self.service.lock().unwrap_or_else(PoisonError::into_inner) =
                Some(Arc::downgrade(service));
        }

        /// Notes `Swapped` when the snapshot already holds `hotkey`, and `Published`
        /// when the subscription already received a snapshot.
        fn note_state(&self, journal: &Journal, hotkey: &str) {
            let service = self
                .service
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
                .and_then(std::sync::Weak::upgrade);
            if let Some(service) = service {
                if service.snapshot().hotkey == hotkey {
                    note(journal, Step::Swapped);
                }
            }
            let got = self
                .rx
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
                .map(|rx| rx.try_recv().is_ok());
            if got == Some(true) {
                note(journal, Step::Published);
            }
        }
    }

    struct JournaledHotkeys(Arc<FakeHotkeyRegistrar>, Journal, Arc<CommitProbe>);

    impl HotkeyRegistrar for JournaledHotkeys {
        fn prepare(
            &self,
            hotkey: crate::settings::hotkey::Hotkey,
            mode: crate::settings::Mode,
        ) -> Result<crate::hotkey_registrar::Prepared, crate::hotkey_registrar::Unavailable>
        {
            note(&self.1, Step::Hotkey(RegistrarCall::Prepare(hotkey, mode)));
            self.0.prepare(hotkey, mode)
        }
        fn commit(&self, prepared: crate::hotkey_registrar::Prepared) {
            self.2.note_state(&self.1, &prepared.hotkey.to_string());
            note(
                &self.1,
                Step::Hotkey(RegistrarCall::Commit(prepared.hotkey)),
            );
            self.0.commit(prepared);
        }
        fn abort(&self, prepared: crate::hotkey_registrar::Prepared) {
            note(&self.1, Step::Hotkey(RegistrarCall::Abort(prepared.hotkey)));
            self.0.abort(prepared);
        }
    }

    impl World {
        /// `load_journaled` with the registrar journaled too; the service is watched
        /// by the returned probe (one subscription of its own).
        fn load_journaled_hotkeys(&self) -> (Arc<SettingsService>, Journal, Arc<CommitProbe>) {
            let journal: Journal = Arc::new(Mutex::new(Vec::new()));
            let probe = Arc::new(CommitProbe::default());
            let mut deps = self.deps();
            deps.file = Arc::new(JournaledFile(self.file.clone(), journal.clone()));
            deps.credentials = Arc::new(JournaledCreds(self.creds.clone(), journal.clone()));
            deps.autostart = Arc::new(JournaledAutostart(self.autostart.clone(), journal.clone()));
            deps.hotkeys = Arc::new(JournaledHotkeys(
                self.hotkeys.clone(),
                journal.clone(),
                probe.clone(),
            ));
            let (service, _) = SettingsService::load_or_init(deps, OS);
            let service = Arc::new(service);
            probe.watch(&service);
            (service, journal, probe)
        }
    }

    /// The registrar calls of a journal, in order.
    fn hotkey_steps(journal: &Journal) -> Vec<RegistrarCall> {
        steps(journal)
            .into_iter()
            .filter_map(|s| match s {
                Step::Hotkey(call) => Some(call),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn taken_hotkey_refuses_with_hotkey_unavailable_and_changes_nothing() {
        // Acceptance failure branch (FR-05; R-3 step 2): `prepare` fails (the hotkey
        // is taken by another process) → Refused with the field error
        // recording.hotkey / hotkey.unavailable and no form error; the registrar saw
        // exactly that one prepare (no commit, nothing active); no autostart call, no
        // key change, no file write; the snapshot kept, no subscriber message. The
        // draft also changes autostart, two keys and a plain field, so a hotkey step
        // placed after any of them shows here. Bite: no hotkey step (Saved, today),
        // prepare after the autostart / keys / file, the error on another field or
        // code, a commit on the failed prepare, the snapshot swapped.
        let on_disk = with_start(sample(EngineKind::None), false);
        let bytes = json(&on_disk);
        let world = World::new(
            FakeSettingsFile::with_bytes(&bytes),
            FakeCredentialStore::new().with_key(API, "sk-test-old-api"),
        );
        world.hotkeys.fail_prepare(true);
        let (service, _) = world.load();
        let rx = service.subscribe();
        let before = service.snapshot();

        let mut draft = with_start(sample(EngineKind::None), true);
        draft.hotkey = NEW_HOTKEY.into();
        draft.history.size = 42;
        let keys = KeyEdits {
            transcription_api: replace("sk-test-new-api"),
            local_server: replace("sk-test-new-local"),
            post_processing: KeyEdit::Untouched,
        };
        assert_eq!(
            service.save(req(draft.clone(), keys)),
            SaveOutcome::Refused {
                errors: vec![field_error(
                    FieldId::RecordingHotkey,
                    ErrorCode::HotkeyUnavailable
                )],
                form_error: None,
            }
        );
        assert_eq!(
            world.hotkeys.calls(),
            vec![RegistrarCall::Prepare(hotkey_of(NEW_HOTKEY), draft.mode)]
        );
        assert_eq!(
            world.hotkeys.active(),
            None,
            "a refused hotkey became active"
        );
        assert!(
            world.autostart.calls().is_empty(),
            "{:?}",
            world.autostart.calls()
        );
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
        assert_eq!(world.file.bytes(), Some(bytes));
        assert!(Arc::ptr_eq(&before, &service.snapshot()));
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    /// Which later step of `every_later_refusal_aborts_the_prepared_hotkey` refuses.
    #[derive(Debug, Clone, Copy)]
    enum LaterRefusal {
        Autostart,
        KeyWrite,
        FileWrite,
    }

    #[test]
    fn every_later_refusal_aborts_the_prepared_hotkey() {
        // R-3 steps 3-5 ("undo ... 2"): after a successful prepare, a refusal by the
        // autostart step, a key write or the file write aborts the prepared hotkey,
        // last (the undo runs in reverse: keys, autostart, then the hotkey); never a
        // commit; the old hotkey stays the one in force (nothing active in the fake,
        // the snapshot kept). Bite: an abort missing in any one branch (the spare
        // registration leaks and the combo stays taken), a commit on a refusal, the
        // abort before the undo of the later steps.
        let new = hotkey_of(NEW_HOTKEY);
        for case in [
            LaterRefusal::Autostart,
            LaterRefusal::KeyWrite,
            LaterRefusal::FileWrite,
        ] {
            let world = World::new(
                FakeSettingsFile::with_bytes(&json(&with_start(sample(EngineKind::None), false))),
                FakeCredentialStore::new(),
            );
            match case {
                LaterRefusal::Autostart => world.autostart.fail_set(true, RUN_ERR),
                LaterRefusal::KeyWrite => world.creds.fail(CredentialOp::Write, LOCAL, STORE_ERR),
                LaterRefusal::FileWrite => world.file.fail_write(io::ErrorKind::PermissionDenied),
            }
            let (service, journal, _probe) = world.load_journaled_hotkeys();
            let before = service.snapshot();

            let mut draft = with_start(sample(EngineKind::None), true);
            draft.hotkey = NEW_HOTKEY.into();
            let keys = KeyEdits {
                transcription_api: replace("sk-test-new-api"),
                local_server: replace("sk-test-new-local"),
                post_processing: KeyEdit::Untouched,
            };
            let outcome = service.save(req(draft.clone(), keys));
            assert!(
                matches!(outcome, SaveOutcome::Refused { .. }),
                "{case:?}: {outcome:?}"
            );
            let prepare = Step::Hotkey(RegistrarCall::Prepare(new, draft.mode));
            let abort = Step::Hotkey(RegistrarCall::Abort(new));
            let expected = match case {
                LaterRefusal::Autostart => vec![prepare, Step::Autostart(true), abort],
                LaterRefusal::KeyWrite => vec![
                    prepare,
                    Step::Autostart(true),
                    Step::Key(CredentialOp::Write, API),
                    Step::Key(CredentialOp::Write, LOCAL),
                    Step::Key(CredentialOp::Delete, API),
                    Step::Autostart(false),
                    abort,
                ],
                LaterRefusal::FileWrite => vec![
                    prepare,
                    Step::Autostart(true),
                    Step::Key(CredentialOp::Write, API),
                    Step::Key(CredentialOp::Write, LOCAL),
                    Step::WriteFile,
                    Step::Key(CredentialOp::Delete, LOCAL),
                    Step::Key(CredentialOp::Delete, API),
                    Step::Autostart(false),
                    abort,
                ],
            };
            assert_eq!(steps(&journal), expected, "{case:?}");
            assert_eq!(world.hotkeys.active(), None, "{case:?}");
            assert!(Arc::ptr_eq(&before, &service.snapshot()), "{case:?}");
        }
    }

    #[test]
    fn a_saved_hotkey_is_committed_after_the_file_and_before_the_swap_and_publish() {
        // R-3 steps 2 and 6 (FR-05 "works at once, no restart"): prepare(new hotkey,
        // the draft's mode) right after validate and before the autostart step;
        // commit after write_atomic, before the snapshot swap and before any
        // subscriber receives the new snapshot; Saved; the new hotkey is the active
        // one. Bite: no hotkey step (no Prepare / Commit, today), prepare with the
        // old mode or after the autostart step, commit before the file write (a later
        // write failure would leave the new combo active), commit after the swap or
        // the publish (a consumer sees a hotkey that is not registered yet), commit
        // skipped (the old combo never released).
        let world = World::new(
            FakeSettingsFile::with_bytes(&json(&with_start(sample(EngineKind::None), false))),
            FakeCredentialStore::new(),
        );
        let (service, journal, probe) = world.load_journaled_hotkeys();

        let mut draft = with_start(sample(EngineKind::None), true);
        draft.hotkey = NEW_HOTKEY.into();
        draft.mode = crate::settings::Mode::Hold;
        let keys = KeyEdits {
            transcription_api: replace("sk-test-new-api"),
            ..KeyEdits::default()
        };
        let view = expect_saved(service.save(req(draft, keys)), &[]);
        assert_eq!(view.settings.hotkey, NEW_HOTKEY);
        // A publish after the commit shows up here, after it.
        probe.note_state(&journal, "never a hotkey");

        let new = hotkey_of(NEW_HOTKEY);
        assert_eq!(
            steps(&journal),
            vec![
                Step::Hotkey(RegistrarCall::Prepare(new, crate::settings::Mode::Hold)),
                Step::Autostart(true),
                Step::Key(CredentialOp::Write, API),
                Step::WriteFile,
                Step::Hotkey(RegistrarCall::Commit(new)),
                Step::Published,
            ]
        );
        assert_eq!(
            world.hotkeys.active(),
            Some((new, crate::settings::Mode::Hold))
        );
        assert_eq!(service.snapshot().hotkey, NEW_HOTKEY);
    }

    #[test]
    fn an_unchanged_hotkey_never_calls_the_registrar() {
        // Spec 004 T032 "unchanged hotkey → registrar not called"; T-055 Q2 (an
        // unchanged hotkey is not retried by a save, also after a startup failure):
        // a save that keeps the hotkey text and changes the mode, autostart, a key and
        // a plain field is Saved with no registrar call, even while every prepare
        // would fail (so a prepare made anyway refuses the save). Bite: prepare on
        // every save (Refused here; on Windows the own combo fails with 1409), the
        // comparison against the draft instead of the snapshot, a mode change taken
        // as a hotkey change.
        let world = World::new(
            FakeSettingsFile::with_bytes(&json(&with_start(sample(EngineKind::None), false))),
            FakeCredentialStore::new(),
        );
        world.hotkeys.fail_prepare(true);
        let (service, journal, _probe) = world.load_journaled_hotkeys();
        let unchanged = service.snapshot().hotkey.clone();

        let mut draft = with_start(sample(EngineKind::None), true);
        assert_eq!(draft.hotkey, unchanged, "premise: the same hotkey text");
        draft.mode = crate::settings::Mode::Hold;
        draft.history.size = 42;
        let keys = KeyEdits {
            transcription_api: replace("sk-test-new-api"),
            ..KeyEdits::default()
        };
        expect_saved(service.save(req(draft, keys)), &[]);
        assert!(
            world.hotkeys.calls().is_empty(),
            "{:?}",
            world.hotkeys.calls()
        );
        assert!(hotkey_steps(&journal).is_empty());
        assert_eq!(service.snapshot().mode, crate::settings::Mode::Hold);
    }

    // ---- T-030: IPC wire form, shell -> UI (contracts/ipc.md, data-model.md) ----

    const IPC_MD: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../specs/004-settings-and-first-run/contracts/ipc.md"
    ));

    /// JSON text without the whitespace outside string literals.
    fn minify(json: &str) -> String {
        let mut out = String::with_capacity(json.len());
        let (mut in_string, mut escaped) = (false, false);
        for c in json.chars() {
            if in_string {
                out.push(c);
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    in_string = false;
                }
            } else if c == '"' {
                in_string = true;
                out.push(c);
            } else if !c.is_whitespace() {
                out.push(c);
            }
        }
        out
    }

    /// The first ```json block under the ipc.md heading of the refused example,
    /// minified.
    fn ipc_refused_example() -> String {
        let (_, after) = IPC_MD
            .split_once("## Example (`settings_save` refused")
            .expect("ipc.md lost its refused example heading");
        let (_, block) = after
            .split_once("```json")
            .expect("ipc.md refused example has no json block");
        let (block, _) = block
            .split_once("```")
            .expect("ipc.md refused example block is not closed");
        minify(block)
    }

    fn wire(outcome: &SaveOutcome) -> serde_json::Value {
        serde_json::to_value(outcome).expect("SaveOutcome serializes")
    }

    fn wire_str(id: &MessageId) -> String {
        serde_json::to_value(id)
            .expect("MessageId serializes")
            .as_str()
            .expect("MessageId is a string")
            .to_string()
    }

    #[test]
    fn save_outcome_wire_form() {
        // Bite: SaveOutcome not externally tagged, a field renamed or reordered,
        // FieldId/ErrorCode derived (`"EngineApiBaseUrl"`) instead of as_str(),
        // FormError without `message` or with `not_restored` on every kind, a
        // Warning code other than `endpoint.insecure`.

        // 1. The ipc.md example, byte for byte.
        let refused = SaveOutcome::Refused {
            errors: vec![
                field_error(FieldId::EngineApiBaseUrl, ErrorCode::UrlMalformed),
                field_error(FieldId::HistorySize, ErrorCode::HistorySizeRange),
            ],
            form_error: None,
        };
        let example = ipc_refused_example();
        assert!(example.starts_with(r#"{"Refused":"#), "example: {example}");
        assert_eq!(
            serde_json::to_string(&refused).expect("SaveOutcome serializes"),
            example
        );

        // 2. Saved, with a warning; the view is SettingsView's own form.
        let view = SettingsView {
            settings: sample(EngineKind::Api),
            keys: KeyPresence {
                transcription_api: true,
                local_server: false,
                post_processing: true,
            },
            first_run: false,
            reset_notice: true,
            unavailable: false,
        };
        let saved = SaveOutcome::Saved {
            view: view.clone(),
            warnings: vec![Warning {
                field: FieldId::EngineApiBaseUrl,
                code: WarningCode::EndpointInsecure,
            }],
        };
        // T-015 / decision #52: a Warning carries its catalog id (`message`, the
        // FormError precedent), so the UI renders t(message) with no cast or rule.
        assert_eq!(
            wire(&saved),
            serde_json::json!({ "Saved": {
                "view": serde_json::to_value(&view).expect("view serializes"),
                "warnings": [ {
                    "field": "engine.api.base_url",
                    "code": "endpoint.insecure",
                    "message": "settings.warning.endpoint_insecure",
                } ],
            } })
        );
        // The message is a declared MessageId (so the catalog completeness test
        // covers it), not a string built from the code.
        assert!(
            MESSAGE_IDS
                .iter()
                .any(|id| wire_str(id) == "settings.warning.endpoint_insecure"),
            "settings.warning.endpoint_insecure is not declared in messages!"
        );
        let no_warnings = SaveOutcome::Saved {
            view: view.clone(),
            warnings: vec![],
        };
        assert_eq!(
            wire(&no_warnings)["Saved"]["warnings"],
            serde_json::json!([])
        );

        // 3. Each FormError: kind + its message id; not_restored only on
        //    partially_restored.
        let table = [
            (
                FormError::WriteFailed,
                serde_json::json!({ "kind": "write_failed", "message": "settings.write_failed" }),
            ),
            (
                FormError::SettingsUnavailable,
                serde_json::json!({
                    "kind": "settings_unavailable",
                    "message": "notice.settings_unavailable"
                }),
            ),
            (
                FormError::PartiallyRestored {
                    not_restored: vec![FieldId::EngineApiKey, FieldId::PostProcessingKey],
                },
                serde_json::json!({
                    "kind": "partially_restored",
                    "message": "settings.partially_restored",
                    "not_restored": ["engine.api.key", "post_processing.key"]
                }),
            ),
        ];
        for (form_error, expected) in table {
            let errors = vec![field_error(
                FieldId::EngineApiKey,
                ErrorCode::KeyStoreFailed,
            )];
            let outcome = SaveOutcome::Refused {
                errors,
                form_error: Some(form_error.clone()),
            };
            assert_eq!(
                wire(&outcome),
                serde_json::json!({ "Refused": {
                    "errors": [ { "field": "engine.api.key", "code": "key.store_failed" } ],
                    "form_error": expected,
                } }),
                "{form_error:?}"
            );
        }

        // 4. Every FieldId and every ErrorCode goes on the wire as its as_str().
        let fields = [
            FieldId::EngineKind,
            FieldId::EngineApiBaseUrl,
            FieldId::EngineApiModel,
            FieldId::EngineApiKey,
            FieldId::EngineLocalServerBaseUrl,
            FieldId::EngineLocalServerModel,
            FieldId::EngineLocalServerKey,
            FieldId::EngineBuiltinLocalModelId,
            FieldId::EngineSpeechLanguage,
            FieldId::RecordingMicrophone,
            FieldId::RecordingHotkey,
            FieldId::RecordingMode,
            FieldId::OutputAutoPaste,
            FieldId::PostProcessingBaseUrl,
            FieldId::PostProcessingModel,
            FieldId::PostProcessingPrompt,
            FieldId::PostProcessingKey,
            FieldId::HistoryEnabled,
            FieldId::HistorySize,
            FieldId::GeneralStartWithWindows,
            FieldId::GeneralUiLanguage,
            FieldId::TimeoutsConnect,
            FieldId::TimeoutsApiTranscription,
            FieldId::TimeoutsLocalServer,
            FieldId::TimeoutsPostProcessing,
            FieldId::TimeoutsBuiltinLocal,
        ];
        let codes = [
            ErrorCode::Required,
            ErrorCode::UrlMalformed,
            ErrorCode::KeyRequired,
            ErrorCode::ModelNotDownloaded,
            ErrorCode::HotkeyNoModifier,
            ErrorCode::HotkeyNoKey,
            ErrorCode::HotkeyEscReserved,
            ErrorCode::HotkeyInvalid,
            ErrorCode::HotkeyUnavailable,
            ErrorCode::HistorySizeRange,
            ErrorCode::AutostartFailed,
            ErrorCode::KeyStoreFailed,
            ErrorCode::UrlCredentials,
            ErrorCode::LanguageUnsupported,
            ErrorCode::TimeoutRange,
        ];
        let errors: Vec<FieldError> = fields
            .iter()
            .map(|&f| field_error(f, ErrorCode::Required))
            .chain(codes.iter().map(|&c| field_error(FieldId::EngineKind, c)))
            .collect();
        let expected: Vec<serde_json::Value> = errors
            .iter()
            .map(|e| serde_json::json!({ "field": e.field.as_str(), "code": e.code.as_str() }))
            .collect();
        assert_eq!(
            wire(&SaveOutcome::Refused {
                errors,
                form_error: None
            }),
            serde_json::json!({ "Refused": { "errors": expected, "form_error": null } })
        );
    }

    // ---- T-004: SettingsView.unavailable and the e2e wire fixture ------------------

    /// The view of a fresh load over `file`, as the JSON a window receives.
    fn view_wire(file: FakeSettingsFile, os: Option<&str>) -> (serde_json::Value, LoadOutcome) {
        let world = World::new(file, FakeCredentialStore::new());
        let (service, outcome) = SettingsService::load_or_init(world.deps(), os);
        (
            serde_json::to_value(service.view()).expect("SettingsView serializes"),
            outcome,
        )
    }

    #[test]
    fn settings_view_wire_form_carries_unavailable() {
        // Decision #38 (Q4): the window shows notice.settings_unavailable when it
        // opens, so the view says whether the service is Unavailable; the wire field
        // is `unavailable: bool`, beside first_run / reset_notice (contracts/ipc.md
        // SettingsView). Bite: the field missing, renamed, always false, or true on
        // another outcome.
        let unreadable = FakeSettingsFile::new();
        unreadable.fail_read(io::ErrorKind::PermissionDenied);
        let cases = [
            ("unavailable", unreadable, true),
            ("first_run", FakeSettingsFile::new(), false),
            (
                "loaded",
                FakeSettingsFile::with_bytes(&json(&sample(EngineKind::Api))),
                false,
            ),
            ("reset", FakeSettingsFile::with_bytes(b"{not json"), false),
        ];
        for (name, file, unavailable) in cases {
            let (view, outcome) = view_wire(file, OS);
            assert_eq!(
                view["unavailable"],
                serde_json::Value::Bool(unavailable),
                "{name}: {outcome:?} -> {view}"
            );
            let mut keys: Vec<&str> = view
                .as_object()
                .expect("view is an object")
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(
                keys,
                [
                    "first_run",
                    "keys",
                    "reset_notice",
                    "settings",
                    "unavailable"
                ],
                "{name}"
            );
        }
    }

    /// The e2e fixture: the first-run view and the speech-language list, so the
    /// Playwright mock serves core's real values (T-004 Investigation 6, option A3).
    const E2E_WIRE_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../e2e/fixtures/settings-wire.json"
    ));

    #[test]
    fn e2e_settings_wire_fixture_matches_core() {
        // P-010: the e2e mock holds no hand copy of the defaults or the language list.
        // `first_run_view` is SettingsService::view() after a first run with no OS
        // language (ui_language en) and no keys; `speech_languages` is
        // WHISPER_ISO_639_1 in core order (what settings_speech_languages returns).
        // Bite: a default changed in defaults(), a field added to or renamed in
        // SettingsView/Settings, or a code added, removed or reordered in the list,
        // without regenerating e2e/fixtures/settings-wire.json.
        let fixture: serde_json::Value =
            serde_json::from_str(E2E_WIRE_FIXTURE).expect("settings-wire.json is valid JSON");
        let (view, outcome) = view_wire(FakeSettingsFile::new(), None);
        assert!(matches!(outcome, LoadOutcome::FirstRun(_)), "{outcome:?}");
        assert_eq!(
            fixture["first_run_view"],
            view,
            "e2e/fixtures/settings-wire.json first_run_view differs from core; core says:\n{}",
            serde_json::to_string_pretty(&view).expect("view serializes")
        );
        assert_eq!(
            fixture["speech_languages"],
            serde_json::json!(crate::settings::WHISPER_ISO_639_1.as_slice()),
            "e2e/fixtures/settings-wire.json speech_languages differs from WHISPER_ISO_639_1"
        );
    }

    #[test]
    fn e2e_settings_wire_fixture_saved_insecure_outcome_matches_core() {
        // T-015: the Playwright warning scenario scripts core's real Saved-with-warning
        // outcome (P-010: no hand-written SaveOutcome in e2e). `saved_insecure_api`
        // is the wire of a save, over a first run with no OS language and no keys, of
        // defaults(None) with engine api, api.base_url `http://example.com/v1` and a
        // new API key. Bite: Saved without the warning, a Warning field renamed or
        // `message` missing, or the view shape changed, without regenerating
        // e2e/fixtures/settings-wire.json.
        let fixture: serde_json::Value =
            serde_json::from_str(E2E_WIRE_FIXTURE).expect("settings-wire.json is valid JSON");
        let world = World::new(FakeSettingsFile::new(), FakeCredentialStore::new());
        let (service, outcome) = SettingsService::load_or_init(world.deps(), None);
        assert!(matches!(outcome, LoadOutcome::FirstRun(_)), "{outcome:?}");
        let mut draft = defaults(None);
        draft.engine = EngineKind::Api;
        draft.api.base_url = "http://example.com/v1".into();
        let keys = KeyEdits {
            transcription_api: replace("sk-test-fake-e2e"),
            ..KeyEdits::default()
        };
        let saved = wire(&service.save(req(draft, keys)));
        assert_eq!(
            fixture["saved_insecure_api"],
            saved,
            "e2e/fixtures/settings-wire.json saved_insecure_api differs from core; core says:\n{}",
            serde_json::to_string_pretty(&saved).expect("outcome serializes")
        );
    }

    #[test]
    fn e2e_settings_wire_fixture_refused_post_processing_matches_core() {
        // T-021 / P-010: the Playwright failure branch of the Post-processing tab
        // scripts core's real refusal, not a hand-written one.
        // `refused_post_processing_on_empty` is the wire of a save, over a first run
        // with no OS language and no keys, of defaults(None) (engine none) with
        // post_processing enabled and base_url, model and prompt "" and no key edits:
        // Refused with post_processing.base_url, .model and .prompt `required`, in
        // that order, and no form error. Bite: no post-processing rule (Saved), a
        // field or code renamed, a key required, or the order changed, without
        // regenerating e2e/fixtures/settings-wire.json.
        let fixture: serde_json::Value =
            serde_json::from_str(E2E_WIRE_FIXTURE).expect("settings-wire.json is valid JSON");
        let world = World::new(FakeSettingsFile::new(), FakeCredentialStore::new());
        let (service, outcome) = SettingsService::load_or_init(world.deps(), None);
        assert!(matches!(outcome, LoadOutcome::FirstRun(_)), "{outcome:?}");
        let mut draft = defaults(None);
        assert_eq!(draft.engine, EngineKind::None);
        draft.post_processing.enabled = true;
        draft.post_processing.base_url = String::new();
        draft.post_processing.model = String::new();
        draft.post_processing.prompt = String::new();
        let outcome = service.save(req(draft, KeyEdits::default()));
        let refused = wire(&outcome);
        let (errors, form_error) = expect_refused(outcome);
        assert_eq!(
            errors,
            vec![
                field_error(FieldId::PostProcessingBaseUrl, ErrorCode::Required),
                field_error(FieldId::PostProcessingModel, ErrorCode::Required),
                field_error(FieldId::PostProcessingPrompt, ErrorCode::Required),
            ]
        );
        assert_eq!(form_error, None);
        assert_eq!(
            fixture["refused_post_processing_on_empty"],
            refused,
            "e2e/fixtures/settings-wire.json refused_post_processing_on_empty differs from core; core says:\n{}",
            serde_json::to_string_pretty(&refused).expect("outcome serializes")
        );
    }

    #[test]
    fn e2e_settings_wire_fixture_test_connection_results_match_core() {
        // T-046 / P-010: the Playwright mock of `settings_test_connection` scripts
        // core's real ConnectionTestResult wire, one value per kind, so the UI's
        // mapping to messages is checked against what core sends (no hand-written
        // result in e2e). `test_connection_results` holds, by name: ok (latency 123
        // ms), cannot_reach (host 127.0.0.1:1), invalid_key, timeout, http (500),
        // unexpected_response, invalid ([engine.api.base_url url.malformed]) and
        // key_store_unavailable. Every kind has its own wire value. Bite: a kind
        // renamed, merged with another, or its fields renamed, without regenerating
        // e2e/fixtures/settings-wire.json (and its `_format` note).
        use crate::connection_test::ConnectionTestResult as R;
        let fixture: serde_json::Value =
            serde_json::from_str(E2E_WIRE_FIXTURE).expect("settings-wire.json is valid JSON");
        let results = [
            ("ok", R::Ok { latency_ms: 123 }),
            (
                "cannot_reach",
                R::CannotReach {
                    host: "127.0.0.1:1".to_string(),
                },
            ),
            ("invalid_key", R::InvalidKey),
            ("timeout", R::Timeout),
            ("http", R::Http { status: 500 }),
            ("unexpected_response", R::UnexpectedResponse),
            (
                "invalid",
                R::Invalid {
                    errors: vec![FieldError {
                        field: FieldId::EngineApiBaseUrl,
                        code: ErrorCode::UrlMalformed,
                    }],
                },
            ),
            ("key_store_unavailable", R::KeyStoreUnavailable),
        ];
        let core: serde_json::Map<String, serde_json::Value> = results
            .iter()
            .map(|(name, r)| {
                let v = serde_json::to_value(r).expect("ConnectionTestResult serializes");
                (name.to_string(), v)
            })
            .collect();
        let mut distinct: Vec<String> = core.values().map(|v| v.to_string()).collect();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            results.len(),
            "two kinds share a wire value: {core:?}"
        );
        let core = serde_json::Value::Object(core);
        assert_eq!(
            fixture["test_connection_results"],
            core,
            "e2e/fixtures/settings-wire.json test_connection_results differs from core; core says:\n{}",
            serde_json::to_string_pretty(&core).expect("results serialize")
        );
    }
}
