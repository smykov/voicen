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
//!
//! STUB (T-003 red tests): every body is `todo!()`; the developer implements them.

use std::fmt;

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
        todo!("T-003: KeySlot::all")
    }

    /// Credential Manager target name (`Voicen/...`).
    pub fn target_name(self) -> &'static str {
        todo!("T-003: KeySlot::target_name")
    }
}

/// A key value. `Debug` and `Display` print `***`; no `Serialize`; zeroed on drop.
pub struct Secret(
    // STUB: read by the implementation of `expose`.
    #[allow(dead_code)] String,
);

impl Secret {
    #[allow(unused_variables)] // STUB: body is todo!()
    pub fn new(value: impl Into<String>) -> Secret {
        todo!("T-003: Secret::new")
    }

    /// The key itself; only for the credential store and the HTTP client.
    pub fn expose(&self) -> &str {
        todo!("T-003: Secret::expose")
    }
}

impl fmt::Debug for Secret {
    #[allow(unused_variables)] // STUB: body is todo!()
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!("T-003: Secret Debug")
    }
}

impl fmt::Display for Secret {
    #[allow(unused_variables)] // STUB: body is todo!()
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!("T-003: Secret Display")
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
    #[allow(unused_variables)] // STUB: body is todo!()
    pub fn get(&self, slot: KeySlot) -> &KeyEdit {
        todo!("T-003: KeyEdits::get")
    }
}

/// Which slots hold a key: the only key information sent to a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyPresence {
    pub transcription_api: bool,
    pub local_server: bool,
    pub post_processing: bool,
}

impl KeyPresence {
    #[allow(unused_variables)] // STUB: body is todo!()
    pub fn get(&self, slot: KeySlot) -> bool {
        todo!("T-003: KeyPresence::get")
    }
}

// STUB: replace with `#[derive(Serialize)]` (field names as in data-model.md).
impl serde::Serialize for KeyPresence {
    #[allow(unused_variables)] // STUB: body is todo!()
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        todo!("T-003: KeyPresence Serialize")
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
    // STUB: the developer chooses the state (e.g. a Mutex over slots, log, failures).
    _state: (),
}

#[cfg(any(test, feature = "test-fakes"))]
#[allow(unused_variables)] // STUB: bodies are todo!()
impl FakeCredentialStore {
    pub fn new() -> FakeCredentialStore {
        todo!("T-003: FakeCredentialStore::new")
    }

    /// Pre-load a slot (not recorded as a call).
    pub fn with_key(self, slot: KeySlot, value: &str) -> FakeCredentialStore {
        todo!("T-003: FakeCredentialStore::with_key")
    }

    /// The key currently in a slot (not recorded as a call).
    pub fn stored(&self, slot: KeySlot) -> Option<String> {
        todo!("T-003: FakeCredentialStore::stored")
    }

    /// Every trait call so far, in order.
    pub fn calls(&self) -> Vec<CredentialCall> {
        todo!("T-003: FakeCredentialStore::calls")
    }

    /// Every later `op` on `slot` fails with `error`.
    pub fn fail(&self, op: CredentialOp, slot: KeySlot, error: CredentialError) {
        todo!("T-003: FakeCredentialStore::fail")
    }

    pub fn clear_failures(&self) {
        todo!("T-003: FakeCredentialStore::clear_failures")
    }
}

#[cfg(any(test, feature = "test-fakes"))]
#[allow(unused_variables)] // STUB: bodies are todo!()
impl CredentialStore for FakeCredentialStore {
    fn read(&self, slot: KeySlot) -> Result<Option<Secret>, CredentialError> {
        todo!("T-003: FakeCredentialStore::read")
    }

    fn write(&self, slot: KeySlot, secret: &Secret) -> Result<(), CredentialError> {
        todo!("T-003: FakeCredentialStore::write")
    }

    fn delete(&self, slot: KeySlot) -> Result<(), CredentialError> {
        todo!("T-003: FakeCredentialStore::delete")
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
