//! `voicen.exe --purge-credentials` (T-061; spec 006 FR-021/FR-022;
//! contracts/installer-ci.md): removes every Credential Manager entry whose target
//! starts with the store prefix + `voicen_core::secrets::CREDENTIAL_TARGET_PREFIX`,
//! before tauri, the single-instance plugin, the log or any window exist.
//!
//! Nothing here logs, prints or keeps key material: the enumeration's blobs are
//! zeroed before the buffer is freed, and only target names and types leave it.

use std::ffi::OsStr;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_INVALID_DATA, WIN32_ERROR};
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredEnumerateW, CredFree, CREDENTIALW, CRED_TYPE,
};

use voicen_core::secrets::{
    purge_credentials, CredentialEntry, CredentialError, CredentialNamespace,
};

use super::os_code;

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
    /// `CredEnumerateW("<prefix>*")`. No match is `Err(ERROR_NOT_FOUND)` (1168),
    /// which the policy reads as "nothing to remove".
    fn list(&self, prefix: &str) -> Result<Vec<CredentialEntry>, CredentialError> {
        let filter = wide(&format!("{prefix}*"));
        let mut count: u32 = 0;
        let mut creds: *mut *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `filter` is NUL-terminated and outlives the call; on success `creds`
        // points to `count` CREDENTIALW pointers owned by the system until `CredFree`.
        unsafe { CredEnumerateW(PCWSTR(filter.as_ptr()), None, &mut count, &mut creds) }.map_err(
            |err| CredentialError {
                os_code: os_code(&err),
            },
        )?;
        if creds.is_null() {
            return Ok(Vec::new());
        }
        let mut entries = Vec::with_capacity(count as usize);
        let mut bad_name = false;
        // SAFETY: `creds` is the non-null array of `count` non-null pointers returned
        // by a successful `CredEnumerateW`. Each blob is `CredentialBlobSize` bytes;
        // it is zeroed in place, never copied. The array is freed exactly once, after
        // the last read, and not used afterwards.
        unsafe {
            for i in 0..count as usize {
                let cred = *creds.add(i);
                if cred.is_null() {
                    continue;
                }
                let c = &*cred;
                if !c.CredentialBlob.is_null() && c.CredentialBlobSize > 0 {
                    let blob = std::slice::from_raw_parts_mut(
                        c.CredentialBlob,
                        c.CredentialBlobSize as usize,
                    );
                    wipe(blob);
                }
                if c.TargetName.is_null() {
                    bad_name = true;
                    continue;
                }
                match c.TargetName.to_string() {
                    Ok(target) => entries.push(CredentialEntry {
                        target,
                        kind: c.Type.0,
                    }),
                    Err(_) => bad_name = true,
                }
            }
            CredFree(creds as *const core::ffi::c_void);
        }
        if bad_name {
            // A target that is not valid UTF-16 cannot be named for CredDeleteW;
            // report it rather than claim the namespace is empty.
            return Err(credential_error(ERROR_INVALID_DATA));
        }
        Ok(entries)
    }

    /// `CredDeleteW(target, entry's own type)`; the error is the Win32 code.
    fn remove(&self, entry: &CredentialEntry) -> Result<(), CredentialError> {
        let target = wide(&entry.target);
        // SAFETY: `target` is NUL-terminated and outlives the call.
        unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE(entry.kind), None) }.map_err(
            |err| CredentialError {
                os_code: os_code(&err),
            },
        )
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn credential_error(code: WIN32_ERROR) -> CredentialError {
    CredentialError {
        os_code: code.0 as i32,
    }
}

/// Overwrites `bytes` with zeros in a way the compiler keeps (key material).
fn wipe(bytes: &mut [u8]) {
    for byte in bytes.iter_mut() {
        // SAFETY: `byte` is a valid, aligned, exclusive reference.
        unsafe { std::ptr::write_volatile(byte, 0) };
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
    let requested = args
        .into_iter()
        .any(|arg| arg.as_ref() == OsStr::new(PURGE_CREDENTIALS_ARG));
    if !requested {
        return None;
    }
    Some(purge_credentials(
        &WinCredentialNamespace::new(),
        store_prefix,
    ))
}
