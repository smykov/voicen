//! T-142: the cancel mapping of the Calls window's save dialog (`win::save_dialog::shown`,
//! I5). The real `IFileSaveDialog` cannot be driven headless; what `Show` answers is mapped
//! by this one pure fn: a pick goes on, the user's cancel
//! (`HRESULT_FROM_WIN32(ERROR_CANCELLED)`) is "cancelled" (then `calls_export` answers
//! `{saved: false}` and writes nothing), any other failure is an error with the OS code.
//! Windows CI only (decision #5).
#![cfg(windows)]

use voicen_lib::win::save_dialog::shown;
use windows::core::Error;
use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_CANCELLED, E_FAIL};

#[test]
fn show_maps_a_pick_a_cancel_and_a_failure() {
    // Bite: a cancel mapped to an error (the export would answer ioFailed) or to a pick
    // (GetResult on a cancelled dialog); every failure mapped to a cancel (a broken dialog
    // would look like the user's choice).
    assert!(shown(Ok(())).expect("a pick"), "a pick goes on");
    assert!(
        !shown(Err(Error::from_hresult(ERROR_CANCELLED.to_hresult()))).expect("a cancel"),
        "the user's cancel is no pick"
    );
    let failed = shown(Err(Error::from_hresult(E_FAIL))).expect_err("E_FAIL is a failure");
    assert_eq!(failed.raw_os_error(), Some(E_FAIL.0));
    let denied = shown(Err(Error::from_hresult(ERROR_ACCESS_DENIED.to_hresult())))
        .expect_err("a Win32 failure");
    assert_eq!(denied.raw_os_error(), Some(ERROR_ACCESS_DENIED.0 as i32));
}
