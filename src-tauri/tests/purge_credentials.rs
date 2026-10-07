//! T-061: `voicen.exe --purge-credentials` against the real Windows Credential Manager
//! (spec 006 FR-021/FR-022; contracts/installer-ci.md). Windows CI only (decision #5).
//!
//! The tests call the lib entry `voicen_lib::win::purge::from_args` under a unique
//! store prefix (`voicen-test-<pid>-<n>-<nanos>/`), never the bin: the bin has no
//! prefix override and would purge the developer's real `Voicen/*` keys. A guard
//! deletes every planted target with raw `CredDeleteW` whether the test passes or not.
//! Blobs are obviously fake keys. Every test takes `serial()` first (T-069): one test at
//! a time in Credential Manager, and a failed read-back names its `CredReadW` error.
#![cfg(windows)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use voicen_core::secrets::{
    purge_credentials, CredentialEntry, CredentialError, CredentialNamespace,
};
use voicen_lib::win::purge::{from_args, WinCredentialNamespace, PURGE_CREDENTIALS_ARG};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredEnumerateW, CredFree, CredReadW, CredWriteW, CREDENTIALW,
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE, CRED_TYPE_GENERIC,
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

/// The fake key as a `CRED_TYPE_GENERIC` blob (opaque bytes).
fn fake_blob() -> Vec<u8> {
    FAKE_KEY.as_bytes().to_vec()
}

/// Writes `target` of type `kind` with the fake key, with raw `CredWriteW`.
fn try_write(target: &str, kind: CRED_TYPE) -> windows::core::Result<()> {
    let mut name = wide(target);
    let mut user = wide("voicen");
    let blob = fake_blob();
    let cred = CREDENTIALW {
        Type: kind,
        TargetName: PWSTR(name.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_ptr() as *mut u8,
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: PWSTR(user.as_mut_ptr()),
        ..Default::default()
    };
    // SAFETY: every pointer in `cred` is valid for the duration of the call; CredWriteW
    // only reads the blob.
    unsafe { CredWriteW(&cred, 0) }
}

/// Raw `CredReadW` of `target` of type `kind`: `Ok` when the entry is readable,
/// otherwise the `CredReadW` error exactly as returned (never collapsed to a bool).
fn read_back(target: &str, kind: CRED_TYPE) -> windows::core::Result<()> {
    let name = wide(target);
    let mut p: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `name` is NUL-terminated and outlives the call; on success `p` is owned
    // by the system until CredFree, which is called exactly once and `p` not used after.
    unsafe {
        CredReadW(PCWSTR(name.as_ptr()), kind, None, &mut p)?;
        CredFree(p as *const core::ffi::c_void);
    }
    Ok(())
}

/// The Win32 code inside `err` when it is a `FACILITY_WIN32` HRESULT, else `None`.
fn win32_code(err: &windows::core::Error) -> Option<u32> {
    let hr = err.code().0 as u32;
    (hr & 0xFFFF_0000 == 0x8007_0000).then_some(hr & 0xFFFF)
}

/// Report-only second look at a failed read-back: does `CredEnumerateW` with the exact
/// target as filter list it? `yes`, `no`, or `err <hresult> (<win32>)`. Never retries
/// the read and never changes the outcome; blobs are not read.
fn enumerate_look(target: &str) -> String {
    let filter = wide(target);
    let mut count: u32 = 0;
    let mut creds: *mut *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `filter` is NUL-terminated and outlives the call; on success `creds` is
    // an array of `count` pointers owned by the system until the one CredFree below.
    unsafe {
        if let Err(e) = CredEnumerateW(PCWSTR(filter.as_ptr()), None, &mut count, &mut creds) {
            return format!("err {}", hresult_and_win32(&e));
        }
        if creds.is_null() {
            return "no".to_string();
        }
        let mut listed = false;
        for i in 0..count as usize {
            let cred = *creds.add(i);
            if cred.is_null() || (*cred).TargetName.is_null() {
                continue;
            }
            if (*cred).TargetName.to_string().is_ok_and(|t| t == target) {
                listed = true;
            }
        }
        CredFree(creds as *const core::ffi::c_void);
        if listed { "yes" } else { "no" }.to_string()
    }
}

/// `HRESULT 0x........ (Win32 N)`, or `(not a Win32 code)`.
fn hresult_and_win32(err: &windows::core::Error) -> String {
    let hr = err.code().0 as u32;
    match win32_code(err) {
        Some(code) => format!("HRESULT {hr:#010x} (Win32 {code})"),
        None => format!("HRESULT {hr:#010x} (not a Win32 code)"),
    }
}

/// The message `Planted::add` panics with when the read-back of a plant fails: names
/// `CredReadW`, its HRESULT and Win32 code, the target and type, and what a
/// report-only `CredEnumerateW(<target>)` sees.
fn premise_failure(target: &str, kind: CRED_TYPE, err: &windows::core::Error) -> String {
    format!(
        "premise: {target} type {} planted (CredWriteW Ok), CredReadW failed: {} {}; \
         CredEnumerateW({target}) lists it: {}",
        kind.0,
        hresult_and_win32(err),
        err.message(),
        enumerate_look(target),
    )
}

/// Whether a read-back says "gone": `CredReadW` failed with `ERROR_NOT_FOUND`. Any
/// other error is not proof of absence.
fn is_not_found(r: &windows::core::Result<()>) -> bool {
    matches!(r, Err(e) if win32_code(e) == Some(ERROR_NOT_FOUND_WIN32))
}

/// One lock per test binary: at most one test touches Credential Manager at a time
/// (T-069). Taken as the first statement of every test, so `Planted`'s cleanup runs
/// under it too; a test that panicked holding it does not poison the rest.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
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
        if let Err(e) = try_write(&target, kind) {
            panic!("setup: raw CredWriteW {target} type {}: {e}", kind.0);
        }
        if let Err(e) = read_back(&target, kind) {
            panic!("{}", premise_failure(&target, kind, &e));
        }
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
    let _serial = serial();
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
        let r = read_back(target, CRED_TYPE_GENERIC);
        assert!(is_not_found(&r), "{target} still present: read-back {r:?}");
    }
    for target in &kept {
        let r = read_back(target, CRED_TYPE_GENERIC);
        assert!(
            r.is_ok(),
            "{target} outside the prefix deleted: read-back {r:?}"
        );
    }

    // Nothing left: a second purge is still a success.
    assert_eq!(from_args(purge_args(), &p), Some(0));
    for target in &kept {
        let r = read_back(target, CRED_TYPE_GENERIC);
        assert!(
            r.is_ok(),
            "{target} deleted by the second run: read-back {r:?}"
        );
    }
}

#[test]
fn purge_with_no_entries_exits_0() {
    let _serial = serial();
    // Failure branch: no entries at all (CredEnumerateW fails with ERROR_NOT_FOUND).
    // Bite: ERROR_NOT_FOUND of the enumeration mapped to 2.
    let p = unique_prefix();
    let mut planted = Planted::default();
    let outsider = planted.add(format!("{p}VoicenOther/keep"), CRED_TYPE_GENERIC);
    assert_eq!(from_args(purge_args(), &p), Some(0));
    let r = read_back(&outsider, CRED_TYPE_GENERIC);
    assert!(r.is_ok(), "outsider deleted: read-back {r:?}");
}

#[test]
fn other_args_are_not_a_purge_and_touch_nothing() {
    let _serial = serial();
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
        let r = read_back(&key, CRED_TYPE_GENERIC);
        assert!(r.is_ok(), "{args:?} deleted a key: read-back {r:?}");
    }
    // OsString arguments, as main() passes std::env::args_os().
    let os_args = vec![
        std::ffi::OsString::from("voicen.exe"),
        std::ffi::OsString::from(PURGE_CREDENTIALS_ARG),
    ];
    assert_eq!(from_args(os_args, &p), Some(0));
    let r = read_back(&key, CRED_TYPE_GENERIC);
    assert!(
        is_not_found(&r),
        "purge via OsString args did nothing: read-back {r:?}"
    );
}

#[test]
fn adapter_lists_exactly_the_prefixed_entries_with_their_types() {
    let _serial = serial();
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
    let _serial = serial();
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
    let r = read_back(&b, CRED_TYPE_GENERIC);
    assert!(r.is_ok(), "the failed entry is gone: read-back {r:?}");
    let r = read_back(&outsider, CRED_TYPE_GENERIC);
    assert!(r.is_ok(), "outsider deleted: read-back {r:?}");
    for target in [&a, &c] {
        let r = read_back(target, CRED_TYPE_GENERIC);
        assert!(
            is_not_found(&r),
            "{target} not removed after the failure: read-back {r:?}"
        );
    }
}

// T-069 tests of the three helpers defined above:
//
//   fn serial() -> std::sync::MutexGuard<'static, ()>
//       one process-wide lock (a static Mutex<()>, poison recovered), taken first by
//       every test in this file;
//   fn read_back(target: &str, kind: CRED_TYPE) -> windows::core::Result<()>
//       raw CredReadW of `target`, replacing the bool `raw_exists`: Ok when the entry
//       is readable, otherwise the CredReadW error as returned;
//   fn premise_failure(target: &str, kind: CRED_TYPE, err: &windows::core::Error) -> String
//       the message `Planted::add` panics with when the read-back of a plant fails.

/// ERROR_NOT_FOUND, the Win32 code CredReadW returns for a target that does not exist.
const ERROR_NOT_FOUND_WIN32: u32 = 1168;

#[test]
fn read_back_of_a_never_written_target_names_its_win32_error() {
    // Acceptance (failure branch): a read-back that fails says why, with the CredReadW
    // error code in the premise message. A fresh target is never written, so the
    // read-back must fail with ERROR_NOT_FOUND.
    // Bite: read_back returning Ok on a CredReadW error (or collapsing to a bool), the
    // error replaced by another code, a message without the call name, without the
    // Win32 code (windows::core::Error's own Display shows only the HRESULT) or
    // without the HRESULT, or without the target.
    let _serial = serial();
    let target = format!("{}{NS}never-written", unique_prefix());

    let err = read_back(&target, CRED_TYPE_GENERIC)
        .expect_err("read-back of a target that was never written");
    assert_eq!(
        err.code(),
        windows::core::HRESULT::from_win32(ERROR_NOT_FOUND_WIN32),
        "CredReadW error of a never-written target: {err:?}"
    );
    assert_eq!(err.code().0 as u32, 0x8007_0490);

    let message = premise_failure(&target, CRED_TYPE_GENERIC, &err);
    for needle in ["CredReadW", "1168", "0x80070490", target.as_str()] {
        assert!(
            message.contains(needle),
            "premise message lacks {needle:?}: {message}"
        );
    }
    // T-069 validation 1 (M2): the codes must sit in the CredReadW part itself. For a
    // never-written target the report-only CredEnumerateW look also prints
    // `err HRESULT 0x80070490 (Win32 1168)`, so "1168 anywhere" survives a message
    // that drops the CredReadW code.
    // Bite: premise_failure without hresult_and_win32(err) after "CredReadW failed:".
    let creadw_part = "CredReadW failed: HRESULT 0x80070490 (Win32 1168)";
    assert!(
        message.contains(creadw_part),
        "premise message lacks {creadw_part:?}: {message}"
    );
}

// T-069 validation 1 (M4): the seam that lets a test make a plant's read-back fail.
//
//   impl Planted {
//       fn add_with(
//           &mut self,
//           target: impl Into<String>,
//           kind: CRED_TYPE,
//           read_back: impl Fn(&str, CRED_TYPE) -> windows::core::Result<()>,
//       ) -> String
//   }
//
// Same as `add` (register, raw CredWriteW, then the read-back; on a read-back error
// panic with `premise_failure(target, kind, &err)`, no retry), with the read-back
// injected. `add(target, kind)` is `add_with(target, kind, read_back)`.

/// ERROR_ACCESS_DENIED: the code the fake read-back fails with (not 1168, so it can
/// only come from the injected CredReadW error, never from the CredEnumerateW look).
const ERROR_ACCESS_DENIED_WIN32: u32 = 5;

#[test]
fn a_plant_whose_read_back_fails_panics_with_the_credreadw_error() {
    // Acceptance 2 (failure branch), at the call site: a plant that cannot be read
    // back fails with the CredReadW error code in the message, and is still cleaned
    // up. The write is real; only the read-back is faked (access denied).
    // Bite: Planted::add's panic without premise_failure (the old
    // "premise: <t> planted"), the read-back error ignored (no panic), the read-back
    // called before the write or with another target/type, a retry, or the target
    // registered for cleanup only after a successful read-back.
    let _serial = serial();
    let target = format!("{}{NS}read-back-denied", unique_prefix());
    let denied = || {
        windows::core::Error::from(windows::core::HRESULT::from_win32(
            ERROR_ACCESS_DENIED_WIN32,
        ))
    };
    let calls: std::cell::RefCell<Vec<(String, CRED_TYPE, bool)>> = Default::default();

    let mut planted = Planted::default();
    let outcome: std::thread::Result<String> =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            planted.add_with(target.clone(), CRED_TYPE_GENERIC, |t, k| {
                // Was the real write already done when the read-back ran?
                let written = read_back(t, k).is_ok();
                calls.borrow_mut().push((t.to_string(), k, written));
                Err(denied())
            })
        }));
    let payload: Box<dyn std::any::Any + Send> = match outcome {
        Ok(returned) => panic!("add_with returned {returned:?} after a failed read-back"),
        Err(payload) => payload,
    };
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .expect("panic payload is a string");

    assert_eq!(
        *calls.borrow(),
        vec![(target.clone(), CRED_TYPE_GENERIC, true)],
        "read-back calls (target, type, written before it)"
    );
    for needle in [
        "CredReadW failed: HRESULT 0x80070005 (Win32 5)",
        target.as_str(),
    ] {
        assert!(
            message.contains(needle),
            "plant panic lacks {needle:?}: {message}"
        );
    }
    // The whole message is premise_failure's, built while the plant is still there
    // (the CredEnumerateW look sees it).
    assert_eq!(
        message,
        premise_failure(&target, CRED_TYPE_GENERIC, &denied()),
        "plant panic is not premise_failure"
    );

    drop(planted);
    let r = read_back(&target, CRED_TYPE_GENERIC);
    assert!(
        is_not_found(&r),
        "{target} left behind after the failed plant: read-back {r:?}"
    );
}

#[test]
fn read_back_is_ok_for_a_planted_target_and_err_1168_once_deleted() {
    // read_back replaces raw_exists in every assertion of this file: Ok must mean
    // "present" and Err(ERROR_NOT_FOUND) "gone", or the purge assertions lose their
    // meaning.
    // Bite: read_back always Err (every plant would fail its premise; caught here
    // directly), read_back always Ok (caught after the delete).
    let _serial = serial();
    let target = format!("{}{NS}read-back", unique_prefix());
    let mut planted = Planted::default();
    planted.add(target.clone(), CRED_TYPE_GENERIC);

    if let Err(e) = read_back(&target, CRED_TYPE_GENERIC) {
        panic!("read-back of a planted target: {e:?}");
    }
    let name = wide(&target);
    // SAFETY: `name` is NUL-terminated and outlives the call.
    unsafe { CredDeleteW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None) }
        .unwrap_or_else(|e| panic!("CredDeleteW of {target}: {}", hresult_and_win32(&e)));
    let err = read_back(&target, CRED_TYPE_GENERIC).expect_err("read-back after delete");
    assert_eq!(
        err.code(),
        windows::core::HRESULT::from_win32(ERROR_NOT_FOUND_WIN32),
        "{err:?}"
    );
}

#[test]
fn serial_excludes_a_second_holder_until_the_first_drops() {
    // Invariant: at most one test touches Credential Manager at a time. While this
    // thread holds serial(), a second thread's serial() must not return; once the
    // guard drops, it must.
    // Bite: serial() locking a fresh Mutex per call, or returning without a lock.
    // A slow runner can only make a broken lock look correct (the second thread not
    // yet scheduled), never make a correct lock look broken: no spurious red.
    let first = serial();
    let (tx, rx) = std::sync::mpsc::channel();
    let second = std::thread::spawn(move || {
        let _held = serial();
        tx.send(()).expect("main thread waits");
    });
    assert!(
        rx.recv_timeout(std::time::Duration::from_millis(200))
            .is_err(),
        "a second serial() returned while the first guard was held"
    );
    drop(first);
    rx.recv_timeout(std::time::Duration::from_secs(30))
        .expect("serial() after the first guard dropped");
    second.join().expect("second holder");
}

#[test]
fn serial_survives_a_test_that_panicked_holding_it() {
    // A failing test panics with the guard held (its Planted cleanup still runs under
    // the lock). The next test must still get the lock, or one red test turns every
    // later test in the binary red with PoisonError and hides the real failure.
    // Bite: serial() as SERIAL.lock().unwrap().
    let panicked = std::thread::spawn(|| {
        let _held = serial();
        panic!("T-069: deliberate panic with the serial guard held");
    })
    .join();
    assert!(panicked.is_err(), "the holder thread did not panic");
    let _again = serial();
}
