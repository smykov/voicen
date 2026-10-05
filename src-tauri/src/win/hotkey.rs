//! The hotkey thread (T-006 design 1; research R-1, R-2): one std thread with a
//! hidden top-level window registers the settings' hotkey with `RegisterHotKey`
//! (codes from `voicen_core::win32_data::hotkey_codes`, `MOD_NOREPEAT` always) and
//! reports the result with `DictationSession::hotkey_registration`. On `WM_HOTKEY`
//! it sends the menu-mask key (`win32_data::menu_mask_vk`) down and up, then calls
//! `hotkey_pressed` with the instant stamped on arrival; a 10 ms poll of
//! `GetAsyncKeyState` over the poll groups (`win32_data::released`) stamps the
//! release and calls `hotkey_released`. Nothing else: no repeat count, no hold
//! timing, no mode. Failures are typed warning lines (`hotkey_thread_failed`,
//! `hotkey_register_failed` with the OS code).

use std::io;
use std::sync::Arc;

use voicen_core::diag::Log;
use voicen_core::dictation::DictationSession;

/// The running hotkey thread. Dropping it ends the thread (quit message, join) and
/// unregisters the hotkey, so the combination is free again once `drop` returns.
pub struct HotkeyThread {
    _private: (),
}

impl HotkeyThread {
    /// Starts the hotkey thread for `hotkey` (canonical text, `Settings::hotkey`)
    /// over `session`. A hotkey that does not parse or that `RegisterHotKey` refuses
    /// is reported to the session as not registered (and logged); the thread still
    /// runs. `Err` only when the thread could not be started.
    pub fn start(
        session: Arc<DictationSession>,
        hotkey: &str,
        log: Arc<Log>,
    ) -> io::Result<HotkeyThread> {
        let _ = (session, hotkey, log);
        todo!("T-006: the hotkey thread")
    }
}
