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

use serde::{Deserialize, Serialize};
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

/// Every Credential Manager target the app writes starts with this (FR-022): the
/// namespace `voicen.exe --purge-credentials` empties (T-061).
///
/// T-061 red-test skeleton: value and use in [`KeySlot::target_name`] are the
/// developer's.
pub const CREDENTIAL_TARGET_PREFIX: &str = "";

/// One Credential Manager entry found by a purge: its target name and credential
/// type (`CRED_TYPE`), never its blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialEntry {
    pub target: String,
    pub kind: u32,
}

/// The part of the credential store a purge needs (T-061): enumerate by prefix and
/// remove one entry. The Windows impl (`CredEnumerateW` / `CredDeleteW`) lives in
/// `src-tauri` (`win::purge`).
pub trait CredentialNamespace {
    /// Every entry whose target starts with `prefix`.
    fn list(&self, prefix: &str) -> Result<Vec<CredentialEntry>, CredentialError>;
    /// Deletes `entry` (its own target and type).
    fn remove(&self, entry: &CredentialEntry) -> Result<(), CredentialError>;
}

/// Removes every entry whose target starts with `store_prefix` +
/// [`CREDENTIAL_TARGET_PREFIX`]; the process exit code: 0 = all removed or none
/// existed, 2 = at least one failed (contracts/installer-ci.md).
///
/// T-061 red-test skeleton: the body is the developer's.
pub fn purge_credentials(namespace: &dyn CredentialNamespace, store_prefix: &str) -> i32 {
    let _ = (namespace, store_prefix);
    todo!("T-061: purge_credentials")
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

// ---- IPC wire form, UI -> shell (contracts/ipc.md; T-030 J1) ----
//
// Tauri puts the serde error of a command argument into the IPC rejection the window
// receives, and serde's own messages quote the input (`unknown variant `sk-...``,
// `invalid type: string "sk-..."`; serde_json raises the latter itself, whatever the
// visitor). So every key-bearing type below deserializes through a private wire
// helper and replaces any error with a fixed text: no error ever contains input.

/// The fixed deserialize error of [`Secret`].
const SECRET_WIRE_ERROR: &str = "invalid key: expected a string";
/// The fixed deserialize error of [`KeyEdit`].
const KEY_EDIT_WIRE_ERROR: &str =
    r#"invalid key edit: expected "Untouched", "Clear" or {"Replace": <string>}"#;
/// The fixed deserialize error of [`KeyEdits`].
const KEY_EDITS_WIRE_ERROR: &str = "invalid key edits: expected transcription_api, \
     local_server and post_processing, each a key edit";

/// A key arrives as a JSON string, unchanged (trimming is the service's rule, #30).
/// There is still no `Serialize`: a key only ever travels UI -> shell.
impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)
            .map(Secret)
            .map_err(|_| serde::de::Error::custom(SECRET_WIRE_ERROR))
    }
}

/// `"Untouched"`, `"Clear"` or `{"Replace": "<key>"}` (externally tagged).
#[derive(Deserialize)]
enum KeyEditWire {
    Untouched,
    Replace(Secret),
    Clear,
}

impl<'de> Deserialize<'de> for KeyEdit {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match KeyEditWire::deserialize(deserializer) {
            Ok(KeyEditWire::Untouched) => Ok(KeyEdit::Untouched),
            Ok(KeyEditWire::Replace(secret)) => Ok(KeyEdit::Replace(secret)),
            Ok(KeyEditWire::Clear) => Ok(KeyEdit::Clear),
            Err(_) => Err(serde::de::Error::custom(KEY_EDIT_WIRE_ERROR)),
        }
    }
}

/// All three slots are required; unknown fields are ignored.
#[derive(Deserialize)]
struct KeyEditsWire {
    transcription_api: KeyEdit,
    local_server: KeyEdit,
    post_processing: KeyEdit,
}

impl<'de> Deserialize<'de> for KeyEdits {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match KeyEditsWire::deserialize(deserializer) {
            Ok(wire) => Ok(KeyEdits {
                transcription_api: wire.transcription_api,
                local_server: wire.local_server,
                post_processing: wire.post_processing,
            }),
            Err(_) => Err(serde::de::Error::custom(KEY_EDITS_WIRE_ERROR)),
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

    // ---- T-030: IPC wire form, UI -> shell (contracts/ipc.md, data-model.md) ----

    /// `{"transcription_api":<a>,"local_server":<b>,"post_processing":<c>}`.
    fn key_edits_json(api: &str, local: &str, pp: &str) -> String {
        format!(r#"{{"transcription_api":{api},"local_server":{local},"post_processing":{pp}}}"#)
    }

    fn replace_json(key: &str) -> String {
        serde_json::to_string(&serde_json::json!({ "Replace": key })).expect("json")
    }

    #[track_caller]
    fn expect_replace(edit: &KeyEdit, key: &str) {
        match edit {
            KeyEdit::Replace(secret) => assert_eq!(secret.expose(), key),
            other => panic!("expected Replace, got {other:?}"),
        }
    }

    #[test]
    fn key_edits_deserialize_wire_form() {
        // Bite: a variant spelled or tagged differently ("clear", {"Clear":null}
        // only, adjacently tagged), Replace not carrying its string, slots swapped,
        // or a missing slot defaulting to Untouched instead of being refused.

        // Each variant, in each slot (rotated so a swapped slot shows).
        let key = "sk-test-wire-0000-not-a-real-key";
        let r = replace_json(key);
        let rotations = [
            [r.as_str(), r#""Clear""#, r#""Untouched""#],
            [r#""Untouched""#, r.as_str(), r#""Clear""#],
            [r#""Clear""#, r#""Untouched""#, r.as_str()],
        ];
        for [api, local, pp] in rotations {
            let json = key_edits_json(api, local, pp);
            let edits: KeyEdits = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("valid key edits refused ({e}): {json}"));
            for (slot, wire) in KeySlot::all().into_iter().zip([api, local, pp]) {
                let edit = edits.get(slot);
                match wire {
                    r#""Clear""# => assert!(matches!(edit, KeyEdit::Clear), "{slot:?}: {edit:?}"),
                    r#""Untouched""# => {
                        assert!(matches!(edit, KeyEdit::Untouched), "{slot:?}: {edit:?}")
                    }
                    _ => expect_replace(edit, key),
                }
            }
        }

        // The key crosses the wire unchanged: trimming is the service's rule (#30).
        for raw in ["  sk-test-padded  ", "", "sk-test-ключ-не-настоящий"] {
            let edit: KeyEdit = serde_json::from_str(&replace_json(raw))
                .unwrap_or_else(|e| panic!("Replace {raw:?} refused: {e}"));
            expect_replace(&edit, raw);
        }

        // All three slots are required: a missing one is an error, not Untouched.
        let present = [
            ("transcription_api", r#""Untouched""#),
            ("local_server", r#""Untouched""#),
            ("post_processing", r#""Untouched""#),
        ];
        for missing in present.iter().map(|(name, _)| *name) {
            let fields: Vec<String> = present
                .iter()
                .filter(|(name, _)| *name != missing)
                .map(|(name, value)| format!(r#""{name}":{value}"#))
                .collect();
            let json = format!("{{{}}}", fields.join(","));
            assert!(
                serde_json::from_str::<KeyEdits>(&json).is_err(),
                "a request without {missing} was accepted: {json}"
            );
        }

        // The whole save request (ipc.md: settings_save { request: SaveRequest }).
        let settings = crate::settings::defaults(Some("ru-RU"));
        let json = format!(
            r#"{{"settings":{},"keys":{}}}"#,
            serde_json::to_string(&settings).expect("settings serialize"),
            key_edits_json(r#""Clear""#, &r, r#""Untouched""#)
        );
        let request: crate::settings::service::SaveRequest = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("valid save request refused ({e}): {json}"));
        assert_eq!(request.settings, settings);
        assert!(matches!(request.keys.transcription_api, KeyEdit::Clear));
        expect_replace(&request.keys.local_server, key);
        assert!(matches!(request.keys.post_processing, KeyEdit::Untouched));
    }

    #[test]
    fn key_edit_deserialize_error_never_echoes_input() {
        // Property (J1): no deserialize error of a key-bearing type contains any part
        // of the input key. Tauri puts the serde error text of the `request` argument
        // into the IPC rejection (tauri 2.12.1 error.rs:59), i.e. into the window.
        // Bite: a derived Deserialize for KeyEdit (`unknown variant `sk-...``,
        // `invalid type: string "sk-...", expected unit`), or a hand-written one that
        // formats the input into its error.
        let bad_edits = [
            format!(r#""{CANARY}""#),                     // key typed as a unit variant
            format!(r#"{{"{CANARY}":"x"}}"#),             // key as the variant name
            format!(r#"{{"{CANARY}":null}}"#),            // same, no content
            format!(r#"{{"Clear":"{CANARY}"}}"#),         // unit variant with content
            format!(r#"{{"Untouched":"{CANARY}"}}"#),     // same
            format!(r#"{{"Replace":["{CANARY}"]}}"#),     // wrong inner type: sequence
            format!(r#"{{"Replace":{{"{CANARY}":1}}}}"#), // wrong inner type: map
            format!(r#"{{"Replace":"{CANARY}","x":1}}"#), // two entries
            format!(r#"["Replace","{CANARY}"]"#),         // sequence form
            format!(r#"{{"tag":"Replace","value":"{CANARY}"}}"#), // adjacently tagged form
            format!(r#""{CANARY}"#),                      // truncated string
        ];

        #[track_caller]
        fn assert_no_echo(what: &str, input: &str, err: &str) {
            assert!(
                !err.contains(CANARY) && !err.contains("CANARY"),
                "{what} error echoes the key: {err:?} (input {input})"
            );
        }

        // Positive control: the same deserializer accepts the valid forms, so an
        // impl that refuses everything does not pass this test.
        for valid in [r#""Untouched""#, r#""Clear""#, &replace_json(CANARY)] {
            assert!(
                serde_json::from_str::<KeyEdit>(valid).is_ok(),
                "valid KeyEdit refused: {valid}"
            );
        }

        let settings = serde_json::to_string(&crate::settings::defaults(None)).expect("json");
        for bad in &bad_edits {
            // KeyEdit itself: refused, with the fixed text only.
            let err = serde_json::from_str::<KeyEdit>(bad)
                .expect_err(&format!("malformed KeyEdit accepted: {bad}"))
                .to_string();
            assert_no_echo("KeyEdit", bad, &err);
            assert!(
                err.starts_with("invalid key edit"),
                "KeyEdit error is not the fixed text: {err:?}"
            );

            // Inside KeyEdits and inside the SaveRequest Tauri deserializes, in every
            // slot: still refused, still without the key.
            for slot in 0..3 {
                let mut slots = [r#""Untouched""#; 3];
                slots[slot] = bad.as_str();
                let keys = key_edits_json(slots[0], slots[1], slots[2]);
                let err = serde_json::from_str::<KeyEdits>(&keys)
                    .err()
                    .unwrap_or_else(|| panic!("malformed KeyEdits accepted (slot {slot})"))
                    .to_string();
                assert_no_echo("KeyEdits", &keys, &err);

                let request = format!(r#"{{"settings":{settings},"keys":{keys}}}"#);
                let err = serde_json::from_str::<crate::settings::service::SaveRequest>(&request)
                    .err()
                    .unwrap_or_else(|| panic!("malformed SaveRequest accepted (slot {slot})"))
                    .to_string();
                assert_no_echo("SaveRequest", &request, &err);
            }
        }

        // A key typed where a slot name belongs: ignored or refused, never echoed
        // (`unknown field `sk-...`` if the struct denied unknown fields).
        let extra = format!(
            r#"{{"transcription_api":"Untouched","local_server":"Untouched","post_processing":"Untouched","{CANARY}":"Clear"}}"#
        );
        if let Err(err) = serde_json::from_str::<KeyEdits>(&extra) {
            assert_no_echo("KeyEdits", &extra, &err.to_string());
        }
        let request = format!(r#"{{"settings":{settings},"keys":{extra},"{CANARY}":1}}"#);
        if let Err(err) = serde_json::from_str::<crate::settings::service::SaveRequest>(&request) {
            assert_no_echo("SaveRequest", &request, &err.to_string());
        }
    }

    #[test]
    fn credential_error_debug_has_no_key_material() {
        // The error type can only carry an OS code: its Debug is the code alone.
        let err = CredentialError { os_code: 1168 };
        assert_eq!(format!("{err:?}"), "CredentialError { os_code: 1168 }");
    }

    // ---- T-061: the credential namespace and the purge policy ----
    // (contracts/installer-ci.md: exit 0 = every entry with the prefix removed or none
    // existed, 2 = at least one failed; FR-021, FR-022.)

    #[test]
    fn every_slot_target_starts_with_prefix() {
        // Bite: CREDENTIAL_TARGET_PREFIX missing or renamed (the uninstaller would purge
        // another namespace), or a slot whose target is outside it (its key would
        // survive an uninstall).
        assert_eq!(CREDENTIAL_TARGET_PREFIX, "Voicen/");
        let mut suffixes = std::collections::HashSet::new();
        for slot in KeySlot::all() {
            let target = slot.target_name();
            let suffix = target
                .strip_prefix(CREDENTIAL_TARGET_PREFIX)
                .unwrap_or_else(|| panic!("{slot:?} target {target:?} outside the prefix"));
            assert!(!suffix.is_empty(), "{slot:?} target is the bare prefix");
            assert!(suffixes.insert(suffix), "{slot:?} shares its target");
        }
    }

    /// `ERROR_NOT_FOUND`.
    const NOT_FOUND: i32 = 1168;
    /// `ERROR_ACCESS_DENIED`: a delete or an enumeration that really failed.
    const ACCESS_DENIED: i32 = 5;
    /// `CRED_TYPE_GENERIC`, `CRED_TYPE_DOMAIN_PASSWORD`.
    const GENERIC: u32 = 1;
    const DOMAIN_PASSWORD: u32 = 2;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum NsCall {
        List(String),
        Remove(CredentialEntry),
    }

    /// In-memory Credential Manager: entries (with a blob the purge never sees), a call
    /// log, injectable failures, and `widen`: targets every `list` also returns
    /// whatever the prefix (a filter wider than asked, e.g. case-insensitive).
    #[derive(Default)]
    struct FakeNamespace {
        entries: std::cell::RefCell<Vec<(CredentialEntry, String)>>,
        widen: Vec<String>,
        list_error: Option<CredentialError>,
        remove_errors: std::collections::HashMap<String, CredentialError>,
        calls: std::cell::RefCell<Vec<NsCall>>,
    }

    impl FakeNamespace {
        fn with(entries: &[(&str, u32)]) -> FakeNamespace {
            let fake = FakeNamespace::default();
            for &(target, kind) in entries {
                fake.entries.borrow_mut().push((
                    CredentialEntry {
                        target: target.to_string(),
                        kind,
                    },
                    CANARY.to_string(),
                ));
            }
            fake
        }

        /// Targets still present, in insertion order.
        fn remaining(&self) -> Vec<String> {
            self.entries
                .borrow()
                .iter()
                .map(|(entry, _)| entry.target.clone())
                .collect()
        }

        fn calls(&self) -> Vec<NsCall> {
            self.calls.borrow().clone()
        }

        fn removed_targets(&self) -> Vec<String> {
            self.calls()
                .into_iter()
                .filter_map(|call| match call {
                    NsCall::Remove(entry) => Some(entry.target),
                    NsCall::List(_) => None,
                })
                .collect()
        }

        fn list_prefixes(&self) -> Vec<String> {
            self.calls()
                .into_iter()
                .filter_map(|call| match call {
                    NsCall::List(prefix) => Some(prefix),
                    NsCall::Remove(_) => None,
                })
                .collect()
        }
    }

    impl CredentialNamespace for FakeNamespace {
        fn list(&self, prefix: &str) -> Result<Vec<CredentialEntry>, CredentialError> {
            self.calls
                .borrow_mut()
                .push(NsCall::List(prefix.to_string()));
            if let Some(error) = self.list_error {
                return Err(error);
            }
            Ok(self
                .entries
                .borrow()
                .iter()
                .filter(|(entry, _)| {
                    entry.target.starts_with(prefix) || self.widen.contains(&entry.target)
                })
                .map(|(entry, _)| entry.clone())
                .collect())
        }

        /// Like `CredDeleteW`: the target with that type, else `ERROR_NOT_FOUND`.
        fn remove(&self, entry: &CredentialEntry) -> Result<(), CredentialError> {
            self.calls.borrow_mut().push(NsCall::Remove(entry.clone()));
            if let Some(&error) = self.remove_errors.get(&entry.target) {
                return Err(error);
            }
            let mut entries = self.entries.borrow_mut();
            match entries.iter().position(|(e, _)| e == entry) {
                Some(index) => {
                    entries.remove(index);
                    Ok(())
                }
                None => Err(CredentialError { os_code: NOT_FOUND }),
            }
        }
    }

    #[test]
    fn purge_removes_every_prefixed_entry_and_returns_0() {
        // Bite: a purge per KeySlot instead of the enumeration (the old-version entry
        // stays), deleting with a fixed CRED_TYPE_GENERIC (the domain entry stays),
        // listing with another prefix, or an exit code other than 0.
        let fake = FakeNamespace::with(&[
            ("Voicen/transcription-api", GENERIC),
            ("VoicenOther/keep", GENERIC),
            ("Voicen/local-server", GENERIC),
            ("Voicen/post-processing", GENERIC),
            ("voicen-test-1/Voicen/transcription-api", GENERIC),
            ("Voicen/slot-of-an-older-version", DOMAIN_PASSWORD),
        ]);
        assert_eq!(purge_credentials(&fake, ""), 0);
        assert_eq!(
            fake.remaining(),
            ["VoicenOther/keep", "voicen-test-1/Voicen/transcription-api"]
        );
        assert_eq!(fake.list_prefixes(), ["Voicen/"]);
        // Each entry deleted once, with its own type.
        for call in fake.calls() {
            if let NsCall::Remove(entry) = call {
                let expected = if entry.target == "Voicen/slot-of-an-older-version" {
                    DOMAIN_PASSWORD
                } else {
                    GENERIC
                };
                assert_eq!(entry.kind, expected, "{entry:?}");
            }
        }
        let mut removed = fake.removed_targets();
        removed.sort();
        assert_eq!(
            removed,
            [
                "Voicen/local-server",
                "Voicen/post-processing",
                "Voicen/slot-of-an-older-version",
                "Voicen/transcription-api"
            ]
        );
    }

    #[test]
    fn purge_under_a_store_prefix_touches_only_that_namespace() {
        // The tests' store prefix (WinCredentialStore::with_target_prefix): the purge
        // composes store prefix + CREDENTIAL_TARGET_PREFIX. Bite: the store prefix
        // ignored (the user's real Voicen/ entries deleted by a test) or appended
        // instead of prepended.
        let fake = FakeNamespace::with(&[
            ("voicen-test-1/Voicen/transcription-api", GENERIC),
            ("Voicen/transcription-api", GENERIC),
            ("voicen-test-1/Voicen/post-processing", GENERIC),
            ("voicen-test-2/Voicen/transcription-api", GENERIC),
            ("voicen-test-1/VoicenOther/keep", GENERIC),
        ]);
        assert_eq!(purge_credentials(&fake, "voicen-test-1/"), 0);
        assert_eq!(
            fake.remaining(),
            [
                "Voicen/transcription-api",
                "voicen-test-2/Voicen/transcription-api",
                "voicen-test-1/VoicenOther/keep"
            ]
        );
        assert_eq!(fake.list_prefixes(), ["voicen-test-1/Voicen/"]);
    }

    #[test]
    fn purge_with_nothing_to_remove_returns_0() {
        // Empty namespace, and an enumeration that reports ERROR_NOT_FOUND (what
        // CredEnumerateW does when nothing matches). Bite: "nothing found" mapped to
        // 2, so an uninstall without saved keys reports a failure.
        let empty = FakeNamespace::with(&[("VoicenOther/keep", GENERIC)]);
        assert_eq!(purge_credentials(&empty, ""), 0);
        assert_eq!(empty.removed_targets(), Vec::<String>::new());
        assert_eq!(empty.remaining(), ["VoicenOther/keep"]);
        assert_eq!(empty.list_prefixes(), ["Voicen/"]);

        let not_found = FakeNamespace {
            list_error: Some(CredentialError { os_code: NOT_FOUND }),
            ..FakeNamespace::default()
        };
        assert_eq!(purge_credentials(&not_found, ""), 0);
        assert_eq!(not_found.removed_targets(), Vec::<String>::new());
    }

    #[test]
    fn purge_continues_after_a_failed_delete_and_returns_2() {
        // Failure branch: one delete fails (first, middle, last). Bite: stopping at the
        // first failure (the later entries stay), swallowing the error (exit 0), or
        // retrying / touching the failed entry otherwise.
        let targets = ["Voicen/a", "Voicen/b", "Voicen/c"];
        for failing in targets {
            let mut fake = FakeNamespace::with(&[
                (targets[0], GENERIC),
                (targets[1], GENERIC),
                ("VoicenOther/keep", GENERIC),
                (targets[2], GENERIC),
            ]);
            fake.remove_errors.insert(
                failing.to_string(),
                CredentialError {
                    os_code: ACCESS_DENIED,
                },
            );
            assert_eq!(purge_credentials(&fake, ""), 2, "failing {failing}");
            let mut remaining = fake.remaining();
            remaining.sort();
            let mut expected = [failing, "VoicenOther/keep"];
            expected.sort();
            assert_eq!(
                remaining, expected,
                "failing {failing}: the others must still be removed"
            );
            let mut tried = fake.removed_targets();
            tried.sort();
            assert_eq!(tried, targets, "failing {failing}: every entry tried once");
        }
    }

    #[test]
    fn purge_list_error_returns_2_and_removes_nothing() {
        // Failure branch: the enumeration itself fails (not ERROR_NOT_FOUND). Bite:
        // the error treated as "nothing there" (exit 0 while keys remain).
        let fake = FakeNamespace {
            list_error: Some(CredentialError {
                os_code: ACCESS_DENIED,
            }),
            ..FakeNamespace::with(&[("Voicen/transcription-api", GENERIC)])
        };
        assert_eq!(purge_credentials(&fake, ""), 2);
        assert_eq!(fake.removed_targets(), Vec::<String>::new());
        assert_eq!(fake.remaining(), ["Voicen/transcription-api"]);
    }

    #[test]
    fn purge_never_removes_an_entry_outside_the_prefix_even_if_listed() {
        // Defence in depth: the enumeration filter (wildcards, case) must never widen
        // the scope. Bite: deleting whatever list returns without re-checking
        // starts_with(store prefix + CREDENTIAL_TARGET_PREFIX).
        let fake = FakeNamespace {
            widen: vec![
                "VoicenOther/keep".to_string(),
                "Voicen".to_string(),
                "x/Voicen/keep".to_string(),
                "Voicen/x".to_string(),
            ],
            ..FakeNamespace::with(&[
                ("VoicenOther/keep", GENERIC),
                ("Voicen", GENERIC),
                ("x/Voicen/keep", GENERIC),
                ("t/Voicen/a", GENERIC),
                ("Voicen/x", GENERIC),
            ])
        };
        assert_eq!(purge_credentials(&fake, "t/"), 0);
        assert_eq!(
            fake.remaining(),
            ["VoicenOther/keep", "Voicen", "x/Voicen/keep", "Voicen/x"]
        );
        assert_eq!(fake.removed_targets(), ["t/Voicen/a"]);
    }

    #[test]
    fn purge_counts_a_target_gone_before_its_delete_as_removed() {
        // An entry deleted between the enumeration and its delete (ERROR_NOT_FOUND)
        // is gone, which is what the purge wants. Bite: every delete error mapped to 2.
        let mut fake = FakeNamespace::with(&[("Voicen/a", GENERIC), ("Voicen/b", GENERIC)]);
        fake.remove_errors.insert(
            "Voicen/a".to_string(),
            CredentialError { os_code: NOT_FOUND },
        );
        assert_eq!(purge_credentials(&fake, ""), 0);
        assert!(!fake.remaining().contains(&"Voicen/b".to_string()));
    }
}
