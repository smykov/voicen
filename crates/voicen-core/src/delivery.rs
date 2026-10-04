//! The delivery decision for a released text (spec 001 data-model
//! "DeliveryDecision", FR-10, FR-022, T-001, decision #47 (3)).
//!
//! The clipboard is always written first. Then, first match: auto-paste off ->
//! copied only (no paster call at all); no start window or an elevated one, the
//! modifiers still held after [`MODIFIER_WAIT`], the start window not in front, or
//! Ctrl+V not sent -> copied, paste manually; otherwise pasted.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::Duration;

use crate::i18n::{self, MessageId};
use crate::platform::{Clipboard, ClipboardError, Paster, StartWindow};

/// How long the paste waits for Shift/Ctrl/Alt/Win to be released (data-model
/// "DeliveryDecision": 1 s). The one constant (P-010).
pub const MODIFIER_WAIT: Duration = Duration::from_secs(1);

/// The result of delivering a text that is already in the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryResult {
    /// Ctrl+V sent to the start window.
    Pasted,
    /// Auto-paste is off.
    CopiedOnly,
    /// Auto-paste is on but the paste could not be done safely.
    CopyManual,
}

impl DeliveryResult {
    /// The `Delivered` event's result code: `pasted` / `copied_only` / `copy_manual`.
    pub fn code(self) -> &'static str {
        match self {
            DeliveryResult::Pasted => "pasted",
            DeliveryResult::CopiedOnly => "copied_only",
            DeliveryResult::CopyManual => "copy_manual",
        }
    }

    /// The notice of `JobEnd::Delivered`: none, `notice.copied`,
    /// `notice.copied_paste_manually`.
    pub fn notice(self) -> Option<MessageId> {
        match self {
            DeliveryResult::Pasted => None,
            DeliveryResult::CopiedOnly => Some(i18n::NOTICE_COPIED),
            DeliveryResult::CopyManual => Some(i18n::NOTICE_COPIED_PASTE_MANUALLY),
        }
    }
}

/// Writes `text` to the clipboard, then pastes it into `start_window` if the table
/// allows. `Err` only when the clipboard write failed (no paster call then).
pub fn deliver(
    text: &str,
    auto_paste: bool,
    start_window: Option<&StartWindow>,
    clipboard: &dyn Clipboard,
    paster: &dyn Paster,
) -> Result<DeliveryResult, ClipboardError> {
    clipboard.set_text_excluded_from_history(text)?;
    if !auto_paste {
        return Ok(DeliveryResult::CopiedOnly);
    }
    Ok(paste(start_window, paster))
}

/// The paste half of the table, for a text already in the clipboard with
/// auto-paste on. Static conditions first (nothing waits for a window that cannot
/// be pasted into), then the wait, then the front check right before sending.
fn paste(start_window: Option<&StartWindow>, paster: &dyn Paster) -> DeliveryResult {
    let Some(window) = start_window.filter(|w| !w.elevated) else {
        return DeliveryResult::CopyManual;
    };
    if !paster.wait_modifiers_released(MODIFIER_WAIT) || !paster.is_in_front(window) {
        return DeliveryResult::CopyManual;
    }
    match paster.send_ctrl_v() {
        Ok(()) => DeliveryResult::Pasted,
        Err(_) => DeliveryResult::CopyManual,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{self, text as render, UiLanguage};
    use crate::platform::{
        FakeClipboard, FakePaster, PasteError, PasterCall, StartWindow, WindowRef,
    };
    use std::sync::{Mutex, PoisonError};

    const TEXT: &str = "hello from the fake engine";

    fn window(elevated: bool) -> StartWindow {
        StartWindow {
            handle: WindowRef(0x00ab_cdef),
            process_id: 4242,
            elevated,
        }
    }

    fn wait() -> PasterCall {
        PasterCall::WaitModifiersReleased(MODIFIER_WAIT)
    }

    fn front() -> PasterCall {
        PasterCall::IsInFront(WindowRef(0x00ab_cdef))
    }

    #[test]
    fn modifier_wait_is_one_second() {
        // data-model "modifiers still held after 1 s". Bite: another constant.
        assert_eq!(MODIFIER_WAIT, Duration::from_secs(1));
    }

    #[test]
    fn all_clear_is_pasted_after_wait_and_front_check() {
        // Bite: no clipboard write, the wait / front check skipped, Ctrl+V not sent,
        // or a wait other than MODIFIER_WAIT.
        let clipboard = FakeClipboard::new();
        let paster = FakePaster::new();
        let w = window(false);
        let got = deliver(TEXT, true, Some(&w), &clipboard, &paster);
        assert_eq!(got, Ok(DeliveryResult::Pasted));
        assert_eq!(clipboard.texts(), vec![TEXT.to_string()]);
        assert_eq!(paster.calls(), vec![wait(), front(), PasterCall::SendCtrlV]);
    }

    /// One log for a clipboard and a paster, to see their order.
    #[derive(Default)]
    struct OrderLog {
        log: Mutex<Vec<&'static str>>,
        clipboard_fails: bool,
    }

    impl OrderLog {
        fn push(&self, s: &'static str) {
            self.log
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(s);
        }
        fn log(&self) -> Vec<&'static str> {
            self.log
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl Clipboard for OrderLog {
        fn set_text_excluded_from_history(&self, _text: &str) -> Result<(), ClipboardError> {
            self.push("clipboard");
            if self.clipboard_fails {
                Err(ClipboardError)
            } else {
                Ok(())
            }
        }
    }

    impl Paster for OrderLog {
        fn capture_start_window(&self) -> Option<StartWindow> {
            self.push("capture");
            None
        }
        fn wait_modifiers_released(&self, _max_wait: Duration) -> bool {
            self.push("wait");
            true
        }
        fn is_in_front(&self, _w: &StartWindow) -> bool {
            self.push("front");
            true
        }
        fn send_ctrl_v(&self) -> Result<(), PasteError> {
            self.push("send");
            Ok(())
        }
    }

    #[test]
    fn clipboard_is_written_before_any_paster_call() {
        // Invariant (3): Ctrl+V only after the clipboard write succeeded. Bite:
        // pasting first (the target gets the previous clipboard content), or the
        // front check made before the wait.
        let both = OrderLog::default();
        let w = window(false);
        let got = deliver(TEXT, true, Some(&w), &both, &both);
        assert_eq!(got, Ok(DeliveryResult::Pasted));
        assert_eq!(both.log(), vec!["clipboard", "wait", "front", "send"]);
    }

    #[test]
    fn clipboard_error_is_err_with_no_paster_call() {
        // data-model row 1: the write fails -> Failed(ClipboardUnavailable) in the
        // pipeline; nothing is pasted. Bite: pasting anyway (the old clipboard
        // content would be pasted), or the error swallowed.
        let both = OrderLog {
            clipboard_fails: true,
            ..OrderLog::default()
        };
        let w = window(false);
        let got = deliver(TEXT, true, Some(&w), &both, &both);
        assert_eq!(got, Err(ClipboardError));
        assert_eq!(both.log(), vec!["clipboard"]);

        let clipboard = FakeClipboard::new();
        clipboard.set_fail(true);
        let paster = FakePaster::new();
        let got = deliver(TEXT, false, Some(&w), &clipboard, &paster);
        assert_eq!(got, Err(ClipboardError), "auto_paste off too");
        assert_eq!(clipboard.texts(), vec![TEXT.to_string()]);
        assert_eq!(paster.calls(), vec![]);
    }

    #[test]
    fn auto_paste_off_copies_only_without_any_paster_call() {
        // Bite: waiting for modifiers or checking the window when auto-paste is off,
        // or reporting CopyManual instead of CopiedOnly.
        let clipboard = FakeClipboard::new();
        let paster = FakePaster::new();
        let w = window(false);
        let got = deliver(TEXT, false, Some(&w), &clipboard, &paster);
        assert_eq!(got, Ok(DeliveryResult::CopiedOnly));
        assert_eq!(clipboard.texts(), vec![TEXT.to_string()]);
        assert_eq!(paster.calls(), vec![]);
    }

    #[test]
    fn no_start_window_is_copy_manual_without_waiting() {
        // Static condition, checked first: nothing waits 1 s for nothing. Bite: a
        // wait or a Ctrl+V into whatever window is in front.
        let clipboard = FakeClipboard::new();
        let paster = FakePaster::new();
        let got = deliver(TEXT, true, None, &clipboard, &paster);
        assert_eq!(got, Ok(DeliveryResult::CopyManual));
        assert_eq!(clipboard.texts(), vec![TEXT.to_string()]);
        assert_eq!(paster.calls(), vec![]);
    }

    #[test]
    fn elevated_window_is_copy_manual_without_waiting() {
        // Bite: sending Ctrl+V to an elevated window (UIPI drops it silently), or
        // waiting first.
        let clipboard = FakeClipboard::new();
        let paster = FakePaster::new();
        let w = window(true);
        let got = deliver(TEXT, true, Some(&w), &clipboard, &paster);
        assert_eq!(got, Ok(DeliveryResult::CopyManual));
        assert_eq!(clipboard.texts(), vec![TEXT.to_string()]);
        assert_eq!(paster.calls(), vec![]);
    }

    #[test]
    fn modifiers_held_is_copy_manual_without_front_check_or_send() {
        // Bite: Ctrl+V with Alt/Shift still held (a different shortcut fires), or
        // the result ignored.
        let clipboard = FakeClipboard::new();
        let paster = FakePaster::new().with_modifiers_released(false);
        let w = window(false);
        let got = deliver(TEXT, true, Some(&w), &clipboard, &paster);
        assert_eq!(got, Ok(DeliveryResult::CopyManual));
        assert_eq!(clipboard.texts(), vec![TEXT.to_string()]);
        assert_eq!(paster.calls(), vec![wait()]);
    }

    #[test]
    fn window_not_in_front_is_copy_manual_without_send() {
        // Bite: pasting into another window that came to the front meanwhile.
        let clipboard = FakeClipboard::new();
        let paster = FakePaster::new().with_in_front(false);
        let w = window(false);
        let got = deliver(TEXT, true, Some(&w), &clipboard, &paster);
        assert_eq!(got, Ok(DeliveryResult::CopyManual));
        assert_eq!(clipboard.texts(), vec![TEXT.to_string()]);
        assert_eq!(paster.calls(), vec![wait(), front()]);
    }

    #[test]
    fn send_error_is_copy_manual() {
        // Decision #47 (3): the text is in the clipboard; the user pastes manually.
        // Bite: reporting Pasted after a failed send, or turning it into a failure.
        let clipboard = FakeClipboard::new();
        let paster = FakePaster::new().with_send_error();
        let w = window(false);
        let got = deliver(TEXT, true, Some(&w), &clipboard, &paster);
        assert_eq!(got, Ok(DeliveryResult::CopyManual));
        assert_eq!(clipboard.texts(), vec![TEXT.to_string()]);
        assert_eq!(paster.calls(), vec![wait(), front(), PasterCall::SendCtrlV]);
    }

    #[test]
    fn result_codes_and_notices() {
        // data-model "Delivered" result codes; the JobEnd notice per result
        // (Pasted: none). Bite: a shared notice, a notice on Pasted, wrong codes.
        assert_eq!(DeliveryResult::Pasted.code(), "pasted");
        assert_eq!(DeliveryResult::CopiedOnly.code(), "copied_only");
        assert_eq!(DeliveryResult::CopyManual.code(), "copy_manual");
        assert_eq!(DeliveryResult::Pasted.notice(), None);
        assert_eq!(
            DeliveryResult::CopiedOnly.notice(),
            Some(i18n::NOTICE_COPIED)
        );
        assert_eq!(
            DeliveryResult::CopyManual.notice(),
            Some(i18n::NOTICE_COPIED_PASTE_MANUALLY)
        );
    }

    #[test]
    fn job_notices_render_in_both_languages() {
        // contracts/messages.md texts for the three T-001 notices. Bite: an id
        // missing from a catalog (text() falls back to the id or to English).
        let rows = [
            (
                i18n::NOTICE_NO_SPEECH,
                "No speech detected",
                "Речь не распознана",
            ),
            (
                i18n::NOTICE_COPIED,
                "Copied to clipboard",
                "Скопировано в буфер обмена",
            ),
            (
                i18n::NOTICE_COPIED_PASTE_MANUALLY,
                "Copied — paste manually",
                "Скопировано — вставьте вручную",
            ),
        ];
        let mut wrong = Vec::new();
        for (id, en, ru) in rows {
            for (lang, want) in [(UiLanguage::En, en), (UiLanguage::Ru, ru)] {
                let got = render(lang, id, &[]);
                if got != want {
                    wrong.push(format!("{id:?} {lang:?}: {got:?}, expected {want:?}"));
                }
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }
}
