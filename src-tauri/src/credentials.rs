//! The Windows implementation of the key port over Credential Manager (T-030; spec
//! 004 T016; contracts/core-traits.md#credentialstore; research R-1). The only code
//! that calls `CredWriteW` / `CredReadW` / `CredDeleteW`.
//!
//! - Generic credential, `CRED_PERSIST_LOCAL_MACHINE`, user name `voicen`, the key as
//!   its UTF-8 bytes in the blob.
//! - Absent target: `read` is `Ok(None)`, `delete` is `Ok(())`.
//! - A blob that is not UTF-8: `CredentialError { os_code: 13 }` (`ERROR_INVALID_DATA`).
//! - Any other failure: `CredentialError` with the Win32 code. Nothing is logged and no
//!   other storage is ever used (NFR-04).

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    ERROR_INVALID_DATA, ERROR_INVALID_PARAMETER, ERROR_NOT_FOUND, WIN32_ERROR,
};
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};

use voicen_core::secrets::{CredentialError, CredentialStore, KeySlot, Secret};

/// The user name stored with every credential (research R-1).
const USER_NAME: &str = "voicen";

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
        format!("{}{}", self.prefix, slot.target_name())
    }

    /// The target as a NUL-terminated UTF-16 string.
    fn wide_target(&self, slot: KeySlot) -> Vec<u16> {
        wide(&self.target(slot))
    }
}

impl Default for WinCredentialStore {
    fn default() -> WinCredentialStore {
        WinCredentialStore::new()
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// The Win32 code of a failed call (the HRESULT when it is not a Win32 one).
fn os_code(err: &windows::core::Error) -> i32 {
    WIN32_ERROR::from_error(err)
        .map(|code| code.0 as i32)
        .unwrap_or(err.code().0)
}

fn is_not_found(err: &windows::core::Error) -> bool {
    WIN32_ERROR::from_error(err).map(|code| code.0) == Some(ERROR_NOT_FOUND.0)
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

impl CredentialStore for WinCredentialStore {
    fn read(&self, slot: KeySlot) -> Result<Option<Secret>, CredentialError> {
        let target = self.wide_target(slot);
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `target` is NUL-terminated and outlives the call; on success `cred`
        // points to a CREDENTIALW owned by the system until `CredFree`.
        let read =
            unsafe { CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None, &mut cred) };
        match read {
            Ok(()) => {}
            Err(err) if is_not_found(&err) => return Ok(None),
            Err(err) => {
                return Err(CredentialError {
                    os_code: os_code(&err),
                })
            }
        }
        if cred.is_null() {
            return Err(credential_error(ERROR_INVALID_DATA));
        }
        // SAFETY: `cred` is the non-null buffer returned by a successful `CredReadW`;
        // its blob is `CredentialBlobSize` bytes long. The buffer is freed exactly
        // once, after the blob was copied and zeroed, and not used afterwards.
        let bytes = unsafe {
            let c = &*cred;
            let bytes = if c.CredentialBlob.is_null() || c.CredentialBlobSize == 0 {
                Vec::new()
            } else {
                let blob =
                    std::slice::from_raw_parts_mut(c.CredentialBlob, c.CredentialBlobSize as usize);
                let copy = blob.to_vec();
                wipe(blob);
                copy
            };
            CredFree(cred as *const core::ffi::c_void);
            bytes
        };
        match String::from_utf8(bytes) {
            Ok(key) => Ok(Some(Secret::new(key))),
            Err(err) => {
                let mut bytes = err.into_bytes();
                wipe(&mut bytes);
                Err(credential_error(ERROR_INVALID_DATA))
            }
        }
    }

    fn write(&self, slot: KeySlot, secret: &Secret) -> Result<(), CredentialError> {
        let mut target = self.wide_target(slot);
        let mut user = wide(USER_NAME);
        let blob = secret.expose().as_bytes();
        let blob_size =
            u32::try_from(blob.len()).map_err(|_| credential_error(ERROR_INVALID_PARAMETER))?;
        let cred = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target.as_mut_ptr()),
            CredentialBlobSize: blob_size,
            // CredWriteW only reads the blob.
            CredentialBlob: blob.as_ptr() as *mut u8,
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(user.as_mut_ptr()),
            ..Default::default()
        };
        // SAFETY: every pointer in `cred` is valid for the duration of the call.
        unsafe { CredWriteW(&cred, 0) }.map_err(|err| CredentialError {
            os_code: os_code(&err),
        })
    }

    fn delete(&self, slot: KeySlot) -> Result<(), CredentialError> {
        let target = self.wide_target(slot);
        // SAFETY: `target` is NUL-terminated and outlives the call.
        match unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) } {
            Ok(()) => Ok(()),
            Err(err) if is_not_found(&err) => Ok(()),
            Err(err) => Err(CredentialError {
                os_code: os_code(&err),
            }),
        }
    }
}
