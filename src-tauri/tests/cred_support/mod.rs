//! T-069: the Credential Manager test support shared by `credentials.rs` and
//! `purge_credentials.rs` (each declares `mod cred_support;`), the `win32_support/`
//! pattern. One copy of the lock and the "gone" rule, so the lock tests below protect the
//! `serial()` every Credential Manager test in either binary takes first.
//!
//! How the lock tests cover both binaries: an integration test is its own crate, so each
//! binary that declares `mod cred_support;` compiles this file, with its own `SERIAL`
//! static, and libtest runs the `#[test]` fns below in that binary too. So the exclusion
//! and poison tests run once in `credentials` and once in `purge_credentials`, each time
//! against the very lock that binary's tests share. A binary with its own lock copy
//! instead of this module would escape them (validation 2, M8): keep it the only one.
//!
//! Contract the developer adds above the tests (T-069 validation 2):
//!
//!   pub const ERROR_NOT_FOUND_WIN32: u32 = 1168;
//!       ERROR_NOT_FOUND, the Win32 code CredReadW returns for a target that does not
//!       exist;
//!   pub fn is_not_found<T>(r: &windows::core::Result<T>) -> bool
//!       a read says "gone": the error is HRESULT_FROM_WIN32(ERROR_NOT_FOUND); any other
//!       error is not proof of absence;
//!   pub fn serial() -> std::sync::MutexGuard<'static, ()>
//!       one lock per test binary (a static Mutex<()>, poison recovered): at most one test
//!       touches Credential Manager at a time; taken as the first statement of every test,
//!       so its cleanup runs under it too.
#![allow(dead_code)]

use std::sync::{Mutex, MutexGuard, PoisonError};

/// ERROR_NOT_FOUND, the Win32 code CredReadW returns for a target that does not exist.
pub const ERROR_NOT_FOUND_WIN32: u32 = 1168;

/// Whether a raw read says "gone": `CredReadW` failed with `ERROR_NOT_FOUND`. Any
/// other error is not proof of absence.
pub fn is_not_found<T>(r: &windows::core::Result<T>) -> bool {
    let not_found = windows::core::HRESULT::from_win32(ERROR_NOT_FOUND_WIN32);
    matches!(r, Err(e) if e.code() == not_found)
}

/// One lock per test binary: at most one test touches Credential Manager at a time.
/// Taken as the first statement of every test, so its cleanup runs under it too; a
/// test that panicked holding it does not poison the rest.
static SERIAL: Mutex<()> = Mutex::new(());

pub fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
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
