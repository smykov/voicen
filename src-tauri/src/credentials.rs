//! The Windows implementation of the key port over Credential Manager (T-030; spec
//! 004 T016; contracts/core-traits.md#credentialstore; research R-1). The only code
//! that calls `CredWriteW` / `CredReadW` / `CredDeleteW`.
//!
//! T-030 RED SKELETON (test-writer): signatures settled by the T-030 Investigation
//! (item 1); the bodies are stubs the developer replaces. Settled behaviour: generic
//! credential, `CRED_PERSIST_LOCAL_MACHINE`, user name `voicen`, UTF-8 blob; absent =
//! `Ok(None)` / `Ok(())`; invalid UTF-8 = `CredentialError { os_code: 13 }`; no
//! logging, no fallback storage.

use voicen_core::secrets::{CredentialError, CredentialStore, KeySlot, Secret};

/// Credential Manager key store. Target = prefix + `KeySlot::target_name()`.
pub struct WinCredentialStore {
    prefix: String,
}

impl WinCredentialStore {
    /// The release store: empty prefix, so targets are exactly `Voicen/...`.
    pub fn new() -> WinCredentialStore {
        WinCredentialStore {
            prefix: String::new(),
        }
    }

    /// A store whose targets start with `prefix` (tests: never the user's real
    /// `Voicen/...` entries). Same three calls as the release store.
    pub fn with_target_prefix(prefix: impl Into<String>) -> WinCredentialStore {
        WinCredentialStore {
            prefix: prefix.into(),
        }
    }

    /// The Credential Manager target name of `slot` for this store.
    pub fn target(&self, slot: KeySlot) -> String {
        // RED STUB
        let _ = (&self.prefix, slot);
        String::new()
    }
}

impl Default for WinCredentialStore {
    fn default() -> WinCredentialStore {
        WinCredentialStore::new()
    }
}

/// RED STUB value: not an OS code.
const STUB: CredentialError = CredentialError { os_code: -1 };

impl CredentialStore for WinCredentialStore {
    fn read(&self, slot: KeySlot) -> Result<Option<Secret>, CredentialError> {
        // RED STUB
        let _ = slot;
        Err(STUB)
    }

    fn write(&self, slot: KeySlot, secret: &Secret) -> Result<(), CredentialError> {
        // RED STUB
        let _ = (slot, secret);
        Err(STUB)
    }

    fn delete(&self, slot: KeySlot) -> Result<(), CredentialError> {
        // RED STUB
        let _ = slot;
        Err(STUB)
    }
}
