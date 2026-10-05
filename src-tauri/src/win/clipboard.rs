//! The clipboard (T-006 design 3; research R-11): `WinClipboard` writes the text as
//! `CF_UNICODETEXT` together with the registered formats
//! `ExcludeClipboardContentFromMonitorProcessing` (present),
//! `CanIncludeInClipboardHistory` (DWORD 0) and `CanUploadToCloudClipboard`
//! (DWORD 0), so Windows keeps it out of the clipboard history and the cloud
//! clipboard (FR-022). `OpenClipboard` is retried briefly; any failure is
//! `ClipboardError` and the clipboard is closed on every path.

use voicen_core::platform::{Clipboard, ClipboardError};

/// The system clipboard. Holds no state across calls.
pub struct WinClipboard {
    _private: (),
}

impl WinClipboard {
    pub fn new() -> WinClipboard {
        todo!("T-006: WinClipboard::new")
    }
}

impl Default for WinClipboard {
    fn default() -> WinClipboard {
        WinClipboard::new()
    }
}

impl Clipboard for WinClipboard {
    fn set_text_excluded_from_history(&self, text: &str) -> Result<(), ClipboardError> {
        let _ = text;
        todo!("T-006: WinClipboard::set_text_excluded_from_history")
    }
}
