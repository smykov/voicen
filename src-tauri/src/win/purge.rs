//! `voicen.exe --purge-credentials` (T-061; spec 006 FR-021/FR-022;
//! contracts/installer-ci.md): removes every Credential Manager entry whose target
//! starts with the store prefix + `voicen_core::secrets::CREDENTIAL_TARGET_PREFIX`,
//! before tauri, the single-instance plugin, the log or any window exist.
//!
//! T-061 red-test skeleton: the signatures are what `src-tauri/tests/
//! purge_credentials.rs` calls; the bodies are the developer's.

use std::ffi::OsStr;

use voicen_core::secrets::{CredentialEntry, CredentialError, CredentialNamespace};

/// The argument the uninstaller passes (T-025).
pub const PURGE_CREDENTIALS_ARG: &str = "--purge-credentials";

/// Credential Manager as a [`CredentialNamespace`]: `CredEnumerateW` with the filter
/// `<prefix>*`, `CredDeleteW` with each entry's own type. Never reads a blob out.
pub struct WinCredentialNamespace {
    _private: (),
}

impl WinCredentialNamespace {
    pub fn new() -> WinCredentialNamespace {
        WinCredentialNamespace { _private: () }
    }
}

impl Default for WinCredentialNamespace {
    fn default() -> WinCredentialNamespace {
        WinCredentialNamespace::new()
    }
}

impl CredentialNamespace for WinCredentialNamespace {
    fn list(&self, prefix: &str) -> Result<Vec<CredentialEntry>, CredentialError> {
        let _ = prefix;
        todo!("T-061: WinCredentialNamespace::list")
    }

    fn remove(&self, entry: &CredentialEntry) -> Result<(), CredentialError> {
        let _ = entry;
        todo!("T-061: WinCredentialNamespace::remove")
    }
}

/// `Some(exit code)` when `args` ask for the purge (the purge has then run under
/// `store_prefix`; the release `main()` passes `""`), `None` otherwise (nothing
/// touched; `main()` goes on to `run()`).
pub fn from_args<I, S>(args: I, store_prefix: &str) -> Option<i32>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let _ = (args.into_iter(), store_prefix);
    todo!("T-061: from_args")
}
