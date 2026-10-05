//! The microphone (T-006 design 2): `CpalSource`, an `AudioSource` over the Windows
//! default input device (cpal 0.18, WASAPI). The open runs on an opener thread and
//! `start` waits for it at most a fixed budget, because cpal's device activation
//! has no bound of its own; a stream that opens after `start` gave up is dropped at
//! once (NFR-02). cpal's errors map to `CaptureError` kinds with fixed literals
//! ([`capture_error`]): cpal's message is never copied (P-009).

use std::sync::Arc;

use voicen_core::platform::{AudioSource, CaptureHandle, FrameSink};
use voicen_core::recording::CaptureError;

/// The Windows default input device (T-012 adds the device choice).
pub struct CpalSource {
    _private: (),
}

impl CpalSource {
    pub fn new() -> CpalSource {
        todo!("T-006: CpalSource::new")
    }
}

impl Default for CpalSource {
    fn default() -> CpalSource {
        CpalSource::new()
    }
}

impl AudioSource for CpalSource {
    fn start(&self, sink: Arc<dyn FrameSink>) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        let _ = sink;
        todo!("T-006: CpalSource::start")
    }
}

/// The capture error of a cpal error: `DeviceNotAvailable` -> `NoDevice`,
/// `PermissionDenied` -> `AccessDenied`, `DeviceBusy` -> `DeviceBusy`, anything
/// else -> `Other` with a fixed literal (never cpal's message: OS text).
pub fn capture_error(err: &cpal::Error) -> CaptureError {
    let _ = err;
    todo!("T-006: the cpal error map")
}
