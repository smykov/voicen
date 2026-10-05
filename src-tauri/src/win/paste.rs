//! Paste into the start window (T-006 design 4; research R-12): `WinPaster`.
//! `capture_start_window` (under the session lock) reads the foreground window's
//! root owner, its process id and its integrity level against ours
//! (`win32_data::target_elevated`), and never sends a message to that window;
//! `wait_modifiers_released` polls `win32_data::modifier_wait_keys` for at most the
//! given time; `is_in_front` compares the foreground root owner and its process id;
//! `send_ctrl_v` sends Ctrl+V in one `SendInput` batch. No mutex is held across
//! its methods.

use std::time::Duration;

use voicen_core::platform::{PasteError, Paster, StartWindow};

/// Ctrl+V into the start window. Its own integrity level is read once.
pub struct WinPaster {
    _private: (),
}

impl WinPaster {
    pub fn new() -> WinPaster {
        todo!("T-006: WinPaster::new")
    }
}

impl Default for WinPaster {
    fn default() -> WinPaster {
        WinPaster::new()
    }
}

impl Paster for WinPaster {
    fn capture_start_window(&self) -> Option<StartWindow> {
        todo!("T-006: WinPaster::capture_start_window")
    }

    fn wait_modifiers_released(&self, max_wait: Duration) -> bool {
        let _ = max_wait;
        todo!("T-006: WinPaster::wait_modifiers_released")
    }

    fn is_in_front(&self, w: &StartWindow) -> bool {
        let _ = w;
        todo!("T-006: WinPaster::is_in_front")
    }

    fn send_ctrl_v(&self) -> Result<(), PasteError> {
        todo!("T-006: WinPaster::send_ctrl_v")
    }
}
