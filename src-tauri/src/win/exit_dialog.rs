//! The Win32 [`ExitConfirm`] of the tray's Exit while a call records (T-145; spec 007
//! FR-008, research R14, decision D5): a system `MessageBoxW` with Yes / No (default No,
//! button texts localized by Windows), the question icon and `MB_SETFOREGROUND`, the given
//! text (`exit.recording.question` in the UI language) and the product name as caption.
//! It has no owner window: the tray has none to offer. The call part calls it on its own
//! question thread (`call-exit-question`), never on the main thread or the call worker,
//! and at most one at a time; the box pumps its own modal loop there.

use windows::core::w;
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONQUESTION, MB_SETFOREGROUND, MB_YESNO,
};

use super::wide;
use crate::call::ExitConfirm;

/// The system Yes / No box as an [`ExitConfirm`].
pub struct WinExitConfirm;

impl ExitConfirm for WinExitConfirm {
    /// `true` only for Yes; No, a closed box or a box that could not be shown (`0`) is
    /// `false`, so the call goes on.
    fn ask(&self, text: &str) -> bool {
        let text = wide(text);
        // SAFETY: `text` is a NUL-terminated UTF-16 buffer alive for the whole call; the
        // caption is a static string; no owner window.
        let answer = unsafe {
            MessageBoxW(
                None,
                windows::core::PCWSTR(text.as_ptr()),
                w!("Voicen"),
                MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2 | MB_SETFOREGROUND,
            )
        };
        answer == IDYES
    }
}
