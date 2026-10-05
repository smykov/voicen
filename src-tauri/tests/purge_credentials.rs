//! T-061: `voicen.exe --purge-credentials` against the real Windows Credential Manager
//! (spec 006 FR-021/FR-022; contracts/installer-ci.md). Windows CI only (decision #5).
//!
//! The tests call the lib entry `voicen_lib::win::purge::from_args` under a unique
//! store prefix (`voicen-test-<pid>-<n>-<nanos>/`), never the bin: the bin has no
//! prefix override and would purge the developer's real `Voicen/*` keys. A guard
//! deletes every planted target with raw `CredDeleteW` whether the test passes or not.
//! Blobs are obviously fake keys.
#![cfg(windows)]

use std::sync::atomic::{AtomicU64, Ordering};

use voicen_core::secrets::{
    purge_credentials, CredentialEntry, CredentialError, CredentialNamespace,
};
use voicen_lib::win::purge::{from_args, WinCredentialNamespace, PURGE_CREDENTIALS_ARG};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE, CRED_TYPE_DOMAIN_PASSWORD, CRED_TYPE_GENERIC,
};

/// The namespace the app writes (pinned again in core by
/// `every_slot_target_starts_with_prefix`).
const NS: &str = "Voicen/";

/// Obviously fake key planted as every blob.
const FAKE_KEY: &str = "sk-test-PURGE-0000-not-a-real-key";

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

/// Plants `target` of type `kind` with a fake key, with raw `CredWriteW`.
fn raw_write(target: &str, kind: CRED_TYPE) {
    let mut name = wide(target);
    let mut user = wide("voicen");
    let blob = FAKE_KEY.as_bytes();
    let cred = CREDENTIALW {
        Type: kind,
        TargetName: PWSTR(name.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_ptr() as *mut u8,
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: PWSTR(user.as_mut_ptr()),
        ..Default::default()
    };
    // SAFETY: every pointer in `cred` is valid for the duration of the call.
    unsafe { CredWriteW(&cred, 0) }
        .unwrap_or_else(|e| panic!("raw CredWriteW {target} type {}: {e}", kind.0));
}

/// Whether `target` of type `kind` exists, with raw `CredReadW`.
fn raw_exists(target: &str, kind: CRED_TYPE) -> bool {
    let name = wide(target);
    let mut p: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `name` is NUL-terminated and outlives the call; on success `p` is owned
    // by the system until CredFree.
    unsafe {
        if CredReadW(PCWSTR(name.as_ptr()), kind, None, &mut p).is_err() {
            return false;
        }
        CredFree(p as *const core::ffi::c_void);
    }
    true
}

fn raw_delete(target: &str, kind: CRED_TYPE) {
    let name = wide(target);
    // SAFETY: `name` is NUL-terminated and outlives the call.
    let _ = unsafe { CredDeleteW(PCWSTR(name.as_ptr()), kind, None) };
}

/// Plants targets and deletes all of them on drop.
#[derive(Default)]
struct Planted(Vec<(String, CRED_TYPE)>);

impl Planted {
    fn add(&mut self, target: impl Into<String>, kind: CRED_TYPE) -> String {
        let target = target.into();
        // Registered before the write: a half-planted test still cleans up.
        self.0.push((target.clone(), kind));
        raw_write(&target, kind);
        assert!(raw_exists(&target, kind), "premise: {target} planted");
        target
    }
}

impl Drop for Planted {
    fn drop(&mut self) {
        for (target, kind) in &self.0 {
            raw_delete(target, *kind);
        }
    }
}

fn purge_args() -> [&'static str; 2] {
    ["voicen.exe", PURGE_CREDENTIALS_ARG]
}

#[test]
fn purge_removes_exactly_the_prefixed_entries_and_exits_0() {
    // Acceptance: with entries under the prefix and others outside it,
    // --purge-credentials removes exactly the prefixed ones and exits 0.
    // Bite: no enumeration (the non-slot entry stays), the store prefix ignored or
    // the filter without the trailing `/` (VoicenOther or the second test namespace
    // deleted), an exit code other than 0.
    assert_eq!(PURGE_CREDENTIALS_ARG, "--purge-credentials");
    let p = unique_prefix();
    let other = unique_prefix();
    let mut planted = Planted::default();
    let gone = [
        planted.add(format!("{p}{NS}transcription-api"), CRED_TYPE_GENERIC),
        planted.add(
            format!("{p}{NS}slot-of-an-older-version"),
            CRED_TYPE_GENERIC,
        ),
    ];
    let kept = [
        planted.add(format!("{p}VoicenOther/keep"), CRED_TYPE_GENERIC),
        planted.add(format!("{other}{NS}transcription-api"), CRED_TYPE_GENERIC),
    ];

    assert_eq!(from_args(purge_args(), &p), Some(0));
    for target in &gone {
        assert!(
            !raw_exists(target, CRED_TYPE_GENERIC),
            "{target} still present"
        );
    }
    for target in &kept {
        assert!(
            raw_exists(target, CRED_TYPE_GENERIC),
            "{target} outside the prefix deleted"
        );
    }

    // Nothing left: a second purge is still a success.
    assert_eq!(from_args(purge_args(), &p), Some(0));
    for target in &kept {
        assert!(
            raw_exists(target, CRED_TYPE_GENERIC),
            "{target} deleted by the second run"
        );
    }
}

#[test]
fn purge_with_no_entries_exits_0() {
    // Failure branch: no entries at all (CredEnumerateW fails with ERROR_NOT_FOUND).
    // Bite: ERROR_NOT_FOUND of the enumeration mapped to 2.
    let p = unique_prefix();
    let mut planted = Planted::default();
    let outsider = planted.add(format!("{p}VoicenOther/keep"), CRED_TYPE_GENERIC);
    assert_eq!(from_args(purge_args(), &p), Some(0));
    assert!(raw_exists(&outsider, CRED_TYPE_GENERIC), "outsider deleted");
}

#[test]
fn other_args_are_not_a_purge_and_touch_nothing() {
    // Bite: main() would exit on a normal start (no app), or a start with
    // --autostart / an unrelated argument would delete the user's keys.
    let p = unique_prefix();
    let mut planted = Planted::default();
    let key = planted.add(format!("{p}{NS}transcription-api"), CRED_TYPE_GENERIC);
    let starts: [&[&str]; 4] = [
        &["voicen.exe"],
        &["voicen.exe", "--autostart"],
        &["voicen.exe", "--purge-credentials-later"],
        &["voicen.exe", "purge-credentials"],
    ];
    for args in starts {
        assert_eq!(from_args(args.iter().copied(), &p), None, "{args:?}");
        assert!(
            raw_exists(&key, CRED_TYPE_GENERIC),
            "{args:?} deleted a key"
        );
    }
    // OsString arguments, as main() passes std::env::args_os().
    let os_args = vec![
        std::ffi::OsString::from("voicen.exe"),
        std::ffi::OsString::from(PURGE_CREDENTIALS_ARG),
    ];
    assert_eq!(from_args(os_args, &p), Some(0));
    assert!(
        !raw_exists(&key, CRED_TYPE_GENERIC),
        "purge via OsString args did nothing"
    );
}

#[test]
fn adapter_lists_exactly_the_prefixed_entries_with_their_types() {
    // Bite: the filter without `*` (exact match only), ignoring the prefix, wrong
    // TargetName decoding, or the type not copied from the entry.
    let p = unique_prefix();
    let mut planted = Planted::default();
    let a = planted.add(format!("{p}{NS}a"), CRED_TYPE_GENERIC);
    let b = planted.add(format!("{p}{NS}b"), CRED_TYPE_GENERIC);
    planted.add(format!("{p}VoicenOther/keep"), CRED_TYPE_GENERIC);

    let ns = WinCredentialNamespace::new();
    let mut listed = ns
        .list(&format!("{p}{NS}"))
        .expect("list of a populated prefix");
    listed.sort_by(|x, y| x.target.cmp(&y.target));
    let expected: Vec<CredentialEntry> = [a, b]
        .into_iter()
        .map(|target| CredentialEntry {
            target,
            kind: CRED_TYPE_GENERIC.0,
        })
        .collect();
    assert_eq!(listed, expected);

    // Nothing under a fresh prefix: empty, or ERROR_NOT_FOUND for the policy to map.
    match ns.list(&format!("{}{NS}", unique_prefix())) {
        Ok(entries) => assert!(entries.is_empty(), "{entries:?}"),
        Err(err) => assert_eq!(err, CredentialError { os_code: 1168 }),
    }
}

#[test]
fn purge_deletes_each_entry_with_its_own_type() {
    // A `cmdkey /add:` entry is CRED_TYPE_DOMAIN_PASSWORD. Bite: CredDeleteW always
    // with CRED_TYPE_GENERIC (that entry stays, or 2 is returned).
    let p = unique_prefix();
    let mut planted = Planted::default();
    let generic = planted.add(format!("{p}{NS}generic"), CRED_TYPE_GENERIC);
    let domain = planted.add(format!("{p}{NS}domain"), CRED_TYPE_DOMAIN_PASSWORD);
    assert_eq!(from_args(purge_args(), &p), Some(0));
    assert!(
        !raw_exists(&generic, CRED_TYPE_GENERIC),
        "{generic} still present"
    );
    assert!(
        !raw_exists(&domain, CRED_TYPE_DOMAIN_PASSWORD),
        "{domain} still present"
    );
}

/// The real adapter whose delete of one target fails (a real `CredDeleteW` cannot be
/// made to fail on demand).
struct FailOne<'a> {
    inner: &'a WinCredentialNamespace,
    failing: String,
}

impl CredentialNamespace for FailOne<'_> {
    fn list(&self, prefix: &str) -> Result<Vec<CredentialEntry>, CredentialError> {
        self.inner.list(prefix)
    }

    fn remove(&self, entry: &CredentialEntry) -> Result<(), CredentialError> {
        if entry.target == self.failing {
            return Err(CredentialError { os_code: 5 });
        }
        self.inner.remove(entry)
    }
}

#[test]
fn failed_delete_exits_2_keeps_that_entry_and_removes_the_rest() {
    // Failure branch: a delete that fails -> exit 2, the failed entry and the
    // outsider stay, every other prefixed entry is still removed.
    // Bite: stopping at the first failure, or swallowing it (exit 0).
    let p = unique_prefix();
    let mut planted = Planted::default();
    let a = planted.add(format!("{p}{NS}a"), CRED_TYPE_GENERIC);
    let b = planted.add(format!("{p}{NS}b"), CRED_TYPE_GENERIC);
    let c = planted.add(format!("{p}{NS}c"), CRED_TYPE_GENERIC);
    let outsider = planted.add(format!("{p}VoicenOther/keep"), CRED_TYPE_GENERIC);
    let ns = WinCredentialNamespace::new();
    let failing = FailOne {
        inner: &ns,
        failing: b.clone(),
    };
    assert_eq!(purge_credentials(&failing, &p), 2);
    assert!(
        raw_exists(&b, CRED_TYPE_GENERIC),
        "the failed entry is gone"
    );
    assert!(raw_exists(&outsider, CRED_TYPE_GENERIC), "outsider deleted");
    for target in [&a, &c] {
        assert!(
            !raw_exists(target, CRED_TYPE_GENERIC),
            "{target} not removed after the failure"
        );
    }
}
