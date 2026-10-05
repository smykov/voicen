//! T-006: the capture adapter's error map (`voicen_lib::win::capture::capture_error`;
//! design 2, invariant 4, P-009). Windows CI only (decision #5): cpal is a Windows-only
//! dependency of the shell.
//!
//! The default-device facts (frames with rate and channels, none after `stop`, NFR-02;
//! red-test table row 17) are not asserted here: the runner's capture endpoints were never
//! probed (docs/decisions/windows-ci-runner.md, "Open"), so they are the owner's manual
//! check (T-006 Notes, orchestrator decision on Refresh 2). This file needs no runner
//! capability.
//!
//! Red-test table row 16.
#![cfg(windows)]

use cpal::{Error, ErrorKind};
use voicen_core::recording::CaptureError;
use voicen_lib::win::capture::capture_error;

/// OS-like text planted as cpal's message: it must never reach a `CaptureError`.
const CANARY: &str = "PLANTED-OS-TEXT 0x80070005 Zugriff verweigert";

/// Every cpal 0.18.2 `ErrorKind` (error.rs:7-91), by a match that lists each one, so a
/// kind added by a cpal update shows up here.
fn all_kinds() -> Vec<ErrorKind> {
    let all = vec![
        ErrorKind::DeviceBusy,
        ErrorKind::DeviceChanged,
        ErrorKind::DeviceNotAvailable,
        ErrorKind::HostUnavailable,
        ErrorKind::InvalidInput,
        ErrorKind::PermissionDenied,
        ErrorKind::RealtimeDenied,
        ErrorKind::ResourceExhausted,
        ErrorKind::StreamInvalidated,
        ErrorKind::UnsupportedConfig,
        ErrorKind::UnsupportedOperation,
        ErrorKind::Xrun,
        ErrorKind::BackendError,
        ErrorKind::Other,
    ];
    for kind in &all {
        match kind {
            ErrorKind::DeviceBusy
            | ErrorKind::DeviceChanged
            | ErrorKind::DeviceNotAvailable
            | ErrorKind::HostUnavailable
            | ErrorKind::InvalidInput
            | ErrorKind::PermissionDenied
            | ErrorKind::RealtimeDenied
            | ErrorKind::ResourceExhausted
            | ErrorKind::StreamInvalidated
            | ErrorKind::UnsupportedConfig
            | ErrorKind::UnsupportedOperation
            | ErrorKind::Xrun
            | ErrorKind::BackendError
            | ErrorKind::Other => {}
            _ => panic!("a cpal ErrorKind this test does not list: {kind:?}"),
        }
    }
    all
}

#[test]
fn cpal_errors_map_to_capture_causes_and_never_carry_cpal_text() {
    // T-006 row 16 (invariant 4, P-009; spec 001 FR-009 reasons): DeviceNotAvailable ->
    // NoDevice, PermissionDenied -> AccessDenied, DeviceBusy -> DeviceBusy, every other
    // kind -> Other with a fixed literal: the same with and without cpal's message, never
    // containing that message (OS text) or cpal's own Display text, and not empty. The
    // message never changes the cause. Bite: `Other(err.to_string())` (the canary leaks),
    // DeviceBusy or PermissionDenied falling to Other, a cause read from the message.
    let mut wrong = Vec::new();
    for kind in all_kinds() {
        let plain = Error::new(kind);
        let planted = Error::with_message(kind, CANARY);
        let (a, b) = (capture_error(&plain), capture_error(&planted));
        let want = match kind {
            ErrorKind::DeviceNotAvailable => Some(CaptureError::NoDevice),
            ErrorKind::PermissionDenied => Some(CaptureError::AccessDenied),
            ErrorKind::DeviceBusy => Some(CaptureError::DeviceBusy),
            _ => None,
        };
        match want {
            Some(want) => {
                if a != want || b != want {
                    wrong.push(format!("{kind:?}: {a:?} / {b:?}, expected {want:?}"));
                }
            }
            None => match (&a, &b) {
                (CaptureError::Other(x), CaptureError::Other(y)) => {
                    if x != y {
                        wrong.push(format!(
                            "{kind:?}: the message changed the literal: {x:?} / {y:?}"
                        ));
                    }
                    if y.contains("PLANTED") || y.contains("0x80070005") {
                        wrong.push(format!("{kind:?}: cpal's message copied: {y:?}"));
                    }
                    if x.is_empty() || *x == plain.to_string() || *x == kind.to_string() {
                        wrong.push(format!("{kind:?}: not a fixed literal of ours: {x:?}"));
                    }
                }
                _ => wrong.push(format!(
                    "{kind:?}: {a:?} / {b:?}, expected Other(<literal>)"
                )),
            },
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}
