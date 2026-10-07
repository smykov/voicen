//! T-030: `WinCredentialStore` against the real Windows Credential Manager (spec 004
//! T016; contracts/core-traits.md#credentialstore; research R-1). Windows CI only
//! (decision #5).
//!
//! Every test uses its own throwaway target prefix, so the user's real `Voicen/...`
//! entries are never touched, and a guard deletes the prefixed targets with raw
//! `CredDeleteW` (not through the store under test) whether the test passes or not.
//! Keys are obviously fake.
#![cfg(windows)]

mod cred_support;

use std::sync::atomic::{AtomicU64, Ordering};

use cred_support::{is_not_found, serial};
use voicen_core::secrets::{CredentialError, CredentialStore, KeySlot, Secret};
use voicen_lib::credentials::WinCredentialStore;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};

/// `voicen-test-<pid>-<n>-<nanos>/`: unique per call, never a real `Voicen/` target.
fn unique_prefix() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!(
        "voicen-test-{}-{}-{nanos}/",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// What Credential Manager holds under one target, read with raw `CredReadW`.
#[derive(Debug, PartialEq, Eq)]
struct RawCredential {
    blob: Vec<u8>,
    user_name: String,
    persist: u32,
}

/// Raw `CredReadW` of `target`: the credential, or the `CredReadW` error as returned
/// (T-069: never collapsed to "absent").
fn raw_read(target: &str) -> windows::core::Result<RawCredential> {
    let name = wide(target);
    let mut p: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `name` is NUL-terminated and outlives the call; on success `p` points to
    // a CREDENTIALW owned by the system until CredFree.
    unsafe {
        CredReadW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None, &mut p)?;
        let cred = &*p;
        let blob = if cred.CredentialBlob.is_null() || cred.CredentialBlobSize == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(cred.CredentialBlob, cred.CredentialBlobSize as usize)
                .to_vec()
        };
        let user_name = if cred.UserName.is_null() {
            String::new()
        } else {
            cred.UserName.to_string().unwrap_or_default()
        };
        let persist = cred.Persist.0;
        CredFree(p as *const core::ffi::c_void);
        Ok(RawCredential {
            blob,
            user_name,
            persist,
        })
    }
}

/// Writes `blob` under `target` with raw `CredWriteW` (to plant bytes the store
/// itself would never write).
fn raw_write(target: &str, blob: &[u8]) {
    let mut name = wide(target);
    let mut user = wide("voicen");
    let cred = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(name.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_ptr() as *mut u8,
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: PWSTR(user.as_mut_ptr()),
        ..Default::default()
    };
    // SAFETY: every pointer in `cred` is valid for the duration of the call.
    unsafe { CredWriteW(&cred, 0) }.expect("raw CredWriteW");
}

fn raw_delete(target: &str) {
    let name = wide(target);
    // SAFETY: `name` is NUL-terminated and outlives the call.
    let _ = unsafe { CredDeleteW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None) };
}

/// Deletes `<prefix><slot target>` for every slot on drop.
struct Cleanup(String);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for slot in KeySlot::all() {
            raw_delete(&format!("{}{}", self.0, slot.target_name()));
        }
    }
}

/// A store under a fresh prefix, plus the guard that removes its targets.
fn throwaway_store() -> (WinCredentialStore, String, Cleanup) {
    let prefix = unique_prefix();
    (
        WinCredentialStore::with_target_prefix(prefix.clone()),
        prefix.clone(),
        Cleanup(prefix),
    )
}

fn fake_key(slot: KeySlot, n: u32) -> String {
    // Non-ASCII on purpose: the blob is UTF-8 (R-1), not UTF-16.
    format!("sk-test-{slot:?}-{n}-ключ-not-a-real-key")
}

#[track_caller]
fn read_key(store: &WinCredentialStore, slot: KeySlot) -> Option<String> {
    store
        .read(slot)
        .unwrap_or_else(|e| panic!("read {slot:?}: {e:?}"))
        .map(|secret| secret.expose().to_string())
}

#[test]
fn each_slot_round_trips_and_delete_makes_it_absent() {
    let _serial = serial();
    // Bite: a slot written to another slot's target, a UTF-16 blob, a user name or
    // persistence other than R-1's, a second write not replacing the first, delete
    // leaving the credential, or deleting an absent key being an error.
    let (store, prefix, _cleanup) = throwaway_store();

    for slot in KeySlot::all() {
        store
            .write(slot, &Secret::new(fake_key(slot, 1)))
            .unwrap_or_else(|e| panic!("write {slot:?}: {e:?}"));
    }
    for slot in KeySlot::all() {
        let key = fake_key(slot, 1);
        assert_eq!(read_key(&store, slot), Some(key.clone()), "{slot:?}");
        let target = format!("{prefix}{}", slot.target_name());
        match raw_read(&target) {
            Ok(raw) => assert_eq!(
                raw,
                RawCredential {
                    blob: key.into_bytes(),
                    user_name: "voicen".to_string(),
                    persist: CRED_PERSIST_LOCAL_MACHINE.0,
                },
                "Credential Manager content of {target}"
            ),
            Err(e) => panic!("raw CredReadW of {target}: {e:?}"),
        }
    }

    // A second write replaces the key.
    let api = KeySlot::TranscriptionApi;
    store
        .write(api, &Secret::new(fake_key(api, 2)))
        .expect("second write");
    assert_eq!(read_key(&store, api), Some(fake_key(api, 2)));

    for slot in KeySlot::all() {
        store
            .delete(slot)
            .unwrap_or_else(|e| panic!("delete {slot:?}: {e:?}"));
        assert_eq!(read_key(&store, slot), None, "{slot:?} still present");
        let r = raw_read(&format!("{prefix}{}", slot.target_name()));
        assert!(
            is_not_found(&r),
            "{slot:?} still in Credential Manager: read-back {r:?}"
        );
        assert_eq!(store.delete(slot), Ok(()), "deleting absent {slot:?}");
    }
}

#[test]
fn missing_target_reads_absent() {
    let _serial = serial();
    // Bite: ERROR_NOT_FOUND (1168) returned as an error instead of absent, on read
    // or on delete.
    let (store, _prefix, _cleanup) = throwaway_store();
    for slot in KeySlot::all() {
        assert!(
            matches!(store.read(slot), Ok(None)),
            "missing {slot:?} must read as Ok(None)"
        );
        assert_eq!(store.delete(slot), Ok(()), "deleting missing {slot:?}");
    }
}

#[test]
fn release_store_targets_are_slot_target_names() {
    let _serial = serial();
    // Bite: the release store adding any prefix or suffix (keys saved by one build
    // unreadable by another), or the test prefix not being a plain prefix.
    let release = WinCredentialStore::new();
    let default = WinCredentialStore::default();
    let prefix = unique_prefix();
    let prefixed = WinCredentialStore::with_target_prefix(prefix.clone());
    for slot in KeySlot::all() {
        assert_eq!(release.target(slot), slot.target_name(), "{slot:?}");
        assert_eq!(default.target(slot), slot.target_name(), "{slot:?}");
        assert_eq!(
            prefixed.target(slot),
            format!("{prefix}{}", slot.target_name()),
            "{slot:?}"
        );
    }
    // That `target()` is the name the calls really use is pinned by the raw reads in
    // each_slot_round_trips_and_delete_makes_it_absent.
}

#[test]
fn non_utf8_blob_reads_as_invalid_data_error() {
    let _serial = serial();
    // Bite: invalid UTF-8 decoded lossily (a different key used silently) or read
    // as absent; the error must be ERROR_INVALID_DATA (13) and carry no bytes.
    let (store, prefix, _cleanup) = throwaway_store();
    let slot = KeySlot::PostProcessing;
    raw_write(
        &format!("{prefix}{}", slot.target_name()),
        &[0x73, 0x6b, 0xff, 0xfe],
    );
    match store.read(slot) {
        Err(err) => assert_eq!(err, CredentialError { os_code: 13 }),
        Ok(value) => panic!("non-UTF-8 blob read as {value:?}"),
    }
}

#[test]
fn oversized_key_is_refused_and_previous_key_kept() {
    let _serial = serial();
    // A real (not injected) Credential Manager refusal: a blob over
    // CRED_MAX_CREDENTIAL_BLOB_SIZE (2560 bytes). Bite: the store reporting Ok,
    // truncating, or touching the stored key on a failed write.
    let (store, _prefix, _cleanup) = throwaway_store();
    let slot = KeySlot::LocalServer;
    store
        .write(slot, &Secret::new(fake_key(slot, 1)))
        .expect("first write");

    let oversized = format!("sk-test-{}", "x".repeat(4096));
    let err = store
        .write(slot, &Secret::new(oversized))
        .expect_err("a 4 KiB blob must be refused");
    assert_ne!(err.os_code, 0, "a refusal carries the OS code");
    assert_eq!(read_key(&store, slot), Some(fake_key(slot, 1)));
}
