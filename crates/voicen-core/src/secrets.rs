//! API keys: the one key port shared by specs 001–004 (NFR-04; decisions #21).
//!
//! Keys live only in the OS credential store behind [`CredentialStore`]; they never
//! reach `settings.json` or a window. [`Secret`] prints `***` and has no `Serialize`:
//!
//! ```compile_fail,E0277
//! // A Secret cannot be serialized (NFR-04): this must not compile.
//! fn assert_serialize<T: serde::Serialize>(_: &T) {}
//! let secret = voicen_core::secrets::Secret::new("sk-test-not-a-real-key");
//! assert_serialize(&secret);
//! ```
//!
//! The same harness compiles for a type that is `Serialize`, so the doctest above
//! fails only because of the missing `Serialize` impl, not because of the harness:
//!
//! ```
//! fn assert_serialize<T: serde::Serialize>(_: &T) {}
//! fn check(settings: &voicen_core::settings::Settings) {
//!     assert_serialize(settings);
//! }
//! ```

use std::fmt;

use serde::Serialize;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// One credential slot; each maps to one Windows Credential Manager target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum KeySlot {
    /// Key of the OpenAI-compatible transcription API (001).
    TranscriptionApi,
    /// Optional key of the local OpenAI-compatible server (002).
    LocalServer,
    /// Key of the post-processing LLM endpoint (003).
    PostProcessing,
}

impl KeySlot {
    /// Every slot, once.
    pub fn all() -> [KeySlot; 3] {
        [
            KeySlot::TranscriptionApi,
            KeySlot::LocalServer,
            KeySlot::PostProcessing,
        ]
    }

    /// Credential Manager target name (`Voicen/...`).
    pub fn target_name(self) -> &'static str {
        match self {
            KeySlot::TranscriptionApi => "Voicen/transcription-api",
            KeySlot::LocalServer => "Voicen/local-server",
            KeySlot::PostProcessing => "Voicen/post-processing",
        }
    }
}

/// A key value. `Debug` and `Display` print `***`; no `Serialize`; the buffer is
/// zeroed on drop.
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Secret {
        Secret(value.into())
    }

    /// The key itself; only for the credential store and the HTTP client.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl ZeroizeOnDrop for Secret {}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// What a save (or a connection test) does with one slot.
#[derive(Debug, Default)]
pub enum KeyEdit {
    /// Keep whatever the slot holds.
    #[default]
    Untouched,
    /// Store this key in the slot.
    Replace(Secret),
    /// Delete the slot's key.
    Clear,
}

/// One [`KeyEdit`] per slot (the `keys` part of a save request).
#[derive(Debug, Default)]
pub struct KeyEdits {
    pub transcription_api: KeyEdit,
    pub local_server: KeyEdit,
    pub post_processing: KeyEdit,
}

impl KeyEdits {
    pub fn get(&self, slot: KeySlot) -> &KeyEdit {
        match slot {
            KeySlot::TranscriptionApi => &self.transcription_api,
            KeySlot::LocalServer => &self.local_server,
            KeySlot::PostProcessing => &self.post_processing,
        }
    }
}

/// Which slots hold a key: the only key information sent to a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct KeyPresence {
    pub transcription_api: bool,
    pub local_server: bool,
    pub post_processing: bool,
}

impl KeyPresence {
    pub fn get(&self, slot: KeySlot) -> bool {
        match slot {
            KeySlot::TranscriptionApi => self.transcription_api,
            KeySlot::LocalServer => self.local_server,
            KeySlot::PostProcessing => self.post_processing,
        }
    }
}

/// A credential store failure: an OS error code, never key material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredentialError {
    pub os_code: i32,
}

/// The key port. The Windows impl (Credential Manager) lives in `src-tauri` (T-030).
pub trait CredentialStore: Send + Sync {
    fn read(&self, slot: KeySlot) -> Result<Option<Secret>, CredentialError>;
    fn write(&self, slot: KeySlot, secret: &Secret) -> Result<(), CredentialError>;
    /// Deleting an absent key is `Ok`.
    fn delete(&self, slot: KeySlot) -> Result<(), CredentialError>;
}

/// Which [`CredentialStore`] method a call was.
#[cfg(any(test, feature = "test-fakes"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CredentialOp {
    Read,
    Write,
    Delete,
}

/// One recorded call on [`FakeCredentialStore`] (never the key value).
#[cfg(any(test, feature = "test-fakes"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredentialCall {
    pub op: CredentialOp,
    pub slot: KeySlot,
}

#[cfg(any(test, feature = "test-fakes"))]
#[derive(Default)]
struct FakeCredentialState {
    slots: std::collections::BTreeMap<KeySlot, Secret>,
    calls: Vec<CredentialCall>,
    failures: std::collections::HashMap<(CredentialOp, KeySlot), CredentialError>,
}

/// In-memory [`CredentialStore`] with a call log and injectable failures.
///
/// - Every trait call is recorded in [`calls`](Self::calls), failed ones included.
/// - [`fail`](Self::fail) makes every later call of `op` on `slot` return the error
///   (without changing the stored key) until [`clear_failures`](Self::clear_failures).
/// - [`with_key`](Self::with_key) and [`stored`](Self::stored) set and inspect a slot
///   without being recorded.
#[cfg(any(test, feature = "test-fakes"))]
#[derive(Default)]
pub struct FakeCredentialStore {
    state: std::sync::Mutex<FakeCredentialState>,
}

#[cfg(any(test, feature = "test-fakes"))]
impl FakeCredentialStore {
    pub fn new() -> FakeCredentialStore {
        FakeCredentialStore::default()
    }

    /// Pre-load a slot (not recorded as a call).
    pub fn with_key(mut self, slot: KeySlot, value: &str) -> FakeCredentialStore {
        self.state
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .slots
            .insert(slot, Secret::new(value));
        self
    }

    /// The key currently in a slot (not recorded as a call).
    pub fn stored(&self, slot: KeySlot) -> Option<String> {
        self.lock()
            .slots
            .get(&slot)
            .map(|secret| secret.expose().to_string())
    }

    /// Every trait call so far, in order.
    pub fn calls(&self) -> Vec<CredentialCall> {
        self.lock().calls.clone()
    }

    /// Every later `op` on `slot` fails with `error`.
    pub fn fail(&self, op: CredentialOp, slot: KeySlot, error: CredentialError) {
        self.lock().failures.insert((op, slot), error);
    }

    pub fn clear_failures(&self) {
        self.lock().failures.clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FakeCredentialState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Records the call; returns the guard, or the injected failure.
    fn begin(
        &self,
        op: CredentialOp,
        slot: KeySlot,
    ) -> Result<std::sync::MutexGuard<'_, FakeCredentialState>, CredentialError> {
        let mut state = self.lock();
        state.calls.push(CredentialCall { op, slot });
        match state.failures.get(&(op, slot)) {
            Some(&error) => Err(error),
            None => Ok(state),
        }
    }
}

#[cfg(any(test, feature = "test-fakes"))]
impl CredentialStore for FakeCredentialStore {
    fn read(&self, slot: KeySlot) -> Result<Option<Secret>, CredentialError> {
        let state = self.begin(CredentialOp::Read, slot)?;
        Ok(state
            .slots
            .get(&slot)
            .map(|secret| Secret::new(secret.expose())))
    }

    fn write(&self, slot: KeySlot, secret: &Secret) -> Result<(), CredentialError> {
        let mut state = self.begin(CredentialOp::Write, slot)?;
        state.slots.insert(slot, Secret::new(secret.expose()));
        Ok(())
    }

    fn delete(&self, slot: KeySlot) -> Result<(), CredentialError> {
        let mut state = self.begin(CredentialOp::Delete, slot)?;
        state.slots.remove(&slot);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Obviously fake key; must never show up in any formatted output.
    const CANARY: &str = "sk-test-CANARY-0000-not-a-real-key";

    #[test]
    fn key_slot_target_names() {
        // Bite: target_name of any slot changed, or two slots sharing a target.
        let expected = [
            (KeySlot::TranscriptionApi, "Voicen/transcription-api"),
            (KeySlot::LocalServer, "Voicen/local-server"),
            (KeySlot::PostProcessing, "Voicen/post-processing"),
        ];
        for (slot, target) in expected {
            assert_eq!(slot.target_name(), target, "{slot:?}");
        }
        assert_eq!(
            KeySlot::all(),
            [
                KeySlot::TranscriptionApi,
                KeySlot::LocalServer,
                KeySlot::PostProcessing
            ]
        );
    }

    #[test]
    fn secret_debug_and_display_are_masked() {
        // Bite: a derived Debug, or Display writing self.0.
        let secret = Secret::new(CANARY);
        assert_eq!(format!("{secret:?}"), "***");
        assert_eq!(format!("{secret}"), "***");
        assert_eq!(format!("{secret:#?}"), "***");
        // The value is still there for the store and the HTTP client.
        assert_eq!(secret.expose(), CANARY);
    }

    #[test]
    fn key_edits_debug_never_shows_a_key() {
        // Property: no Debug of a structure holding a Secret contains the key.
        let edits = KeyEdits {
            transcription_api: KeyEdit::Replace(Secret::new(CANARY)),
            local_server: KeyEdit::Replace(Secret::new(CANARY)),
            post_processing: KeyEdit::Replace(Secret::new(CANARY)),
        };
        let debug = format!("{edits:?} {edits:#?}");
        assert!(!debug.contains(CANARY), "key leaked in Debug: {debug}");
        assert!(!debug.contains("CANARY"), "part of a key leaked: {debug}");
    }

    #[test]
    fn key_edits_map_each_slot_to_its_own_edit() {
        // Bite: get() returning the edit of another slot.
        let edits = KeyEdits {
            transcription_api: KeyEdit::Replace(Secret::new("sk-test-api")),
            local_server: KeyEdit::Clear,
            post_processing: KeyEdit::Replace(Secret::new("sk-test-pp")),
        };
        match edits.get(KeySlot::TranscriptionApi) {
            KeyEdit::Replace(s) => assert_eq!(s.expose(), "sk-test-api"),
            other => panic!("transcription_api: {other:?}"),
        }
        assert!(matches!(edits.get(KeySlot::LocalServer), KeyEdit::Clear));
        match edits.get(KeySlot::PostProcessing) {
            KeyEdit::Replace(s) => assert_eq!(s.expose(), "sk-test-pp"),
            other => panic!("post_processing: {other:?}"),
        }
        // Default: every slot untouched.
        let none = KeyEdits::default();
        for slot in KeySlot::all() {
            assert!(matches!(none.get(slot), KeyEdit::Untouched), "{slot:?}");
        }
    }

    #[test]
    fn key_presence_is_per_slot_and_serializes_only_booleans() {
        // Bite: get() reading the wrong field; extra fields in the window payload.
        let presence = KeyPresence {
            transcription_api: true,
            local_server: false,
            post_processing: true,
        };
        assert!(presence.get(KeySlot::TranscriptionApi));
        assert!(!presence.get(KeySlot::LocalServer));
        assert!(presence.get(KeySlot::PostProcessing));
        let json = serde_json::to_value(presence).expect("KeyPresence serializes");
        assert_eq!(
            json,
            serde_json::json!({
                "transcription_api": true,
                "local_server": false,
                "post_processing": true
            })
        );
    }

    #[test]
    fn credential_error_debug_has_no_key_material() {
        // The error type can only carry an OS code: its Debug is the code alone.
        let err = CredentialError { os_code: 1168 };
        assert_eq!(format!("{err:?}"), "CredentialError { os_code: 1168 }");
    }
}
