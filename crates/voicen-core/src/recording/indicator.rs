//! What the tray icon and the overlay show (spec 001 data-model "IndicatorState",
//! T-042).
//!
//! Owned by [`RecordingController`](super::RecordingController): it changes only
//! inside controller methods, from controller transitions and `job_finished`
//! outcomes. No clock is read here; every instant comes from the caller's event.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::{Duration, Instant};

use crate::delivery::DeliveryResult;
use crate::failure::FailureReason;
use crate::i18n::MessageId;
use crate::post_process::SkipReason;

/// How long a failure or notice message stays on the overlay.
pub const MESSAGE_DURATION: Duration = Duration::from_secs(3);

/// Tray icon. Priority: `HotkeyError > Recording > Error > Idle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Recording,
    Error,
    HotkeyError,
}

/// Overlay. Priority: `Recording > Message > Processing > Hidden`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayState {
    Hidden,
    Recording,
    Processing,
    /// A failure or notice, shown until `until` (= its event instant + 3 s).
    Message {
        id: MessageId,
        params: Vec<(&'static str, String)>,
        until: Instant,
    },
}

/// Both indicators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndicatorState {
    pub tray: TrayState,
    pub overlay: OverlayState,
}

impl Default for IndicatorState {
    fn default() -> Self {
        IndicatorState {
            tray: TrayState::Idle,
            overlay: OverlayState::Hidden,
        }
    }
}

/// How one queued job ended, as the pipeline reports it to the controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobEnd {
    /// Pasted (`notice: None`) or copied only (`notice: Some(..)`); clears tray `Error`.
    Delivered { notice: Option<MessageId> },
    /// Delivered with the raw transcript because post-processing was skipped
    /// (spec 003 FR-007, T-020): sets tray `Error`; the overlay message is
    /// `post_process::skip_message(reason, delivery)` (decision #91(3)).
    DeliveredSkipped {
        reason: SkipReason,
        delivery: DeliveryResult,
    },
    /// No speech, copy manually, ...: a message, tray unchanged.
    Notice(MessageId),
    /// A message and tray `Error`.
    Failed(FailureReason),
}

/// A message on the overlay and its expiry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShownMessage {
    pub id: MessageId,
    pub params: Vec<(&'static str, String)>,
    pub until: Instant,
}

/// The facts the indicator is derived from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndicatorInputs {
    pub hotkey_error: bool,
    pub recording: bool,
    pub error: bool,
    pub message: Option<ShownMessage>,
    pub queued_jobs: usize,
}

impl IndicatorState {
    /// The indicator for `inputs`, by the two priority orders.
    pub(crate) fn derive(inputs: &IndicatorInputs) -> IndicatorState {
        let tray = if inputs.hotkey_error {
            TrayState::HotkeyError
        } else if inputs.recording {
            TrayState::Recording
        } else if inputs.error {
            TrayState::Error
        } else {
            TrayState::Idle
        };
        let overlay = if inputs.recording {
            OverlayState::Recording
        } else if let Some(m) = &inputs.message {
            OverlayState::Message {
                id: m.id,
                params: m.params.clone(),
                until: m.until,
            }
        } else if inputs.queued_jobs > 0 {
            OverlayState::Processing
        } else {
            OverlayState::Hidden
        };
        IndicatorState { tray, overlay }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n;

    fn inputs(hotkey_error: bool, recording: bool, error: bool) -> IndicatorInputs {
        IndicatorInputs {
            hotkey_error,
            recording,
            error,
            message: None,
            queued_jobs: 0,
        }
    }

    #[test]
    fn tray_priority_table() {
        // data-model "IndicatorState": HotkeyError > Recording > Error > Idle, every
        // combination of the three flags. Bite: Error checked before Recording (an
        // old failure hides that the mic is on), HotkeyError not first, or a
        // constant tray.
        let mut wrong = Vec::new();
        for hotkey_error in [false, true] {
            for recording in [false, true] {
                for error in [false, true] {
                    let want = if hotkey_error {
                        TrayState::HotkeyError
                    } else if recording {
                        TrayState::Recording
                    } else if error {
                        TrayState::Error
                    } else {
                        TrayState::Idle
                    };
                    let got = IndicatorState::derive(&inputs(hotkey_error, recording, error)).tray;
                    if got != want {
                        wrong.push(format!(
                            "hotkey_error={hotkey_error} recording={recording} error={error}: \
                             {got:?}, expected {want:?}"
                        ));
                    }
                }
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    #[test]
    fn overlay_priority_table() {
        // Recording > Message > Processing (>= 1 queued job) > Hidden, every
        // combination. Bite: Processing shown while recording (Clarification 5),
        // a message hidden behind Processing, Processing for 0 jobs, a constant
        // overlay.
        let t0 = Instant::now();
        let msg = ShownMessage {
            id: FailureReason::Timeout.message_id(),
            params: FailureReason::Timeout.message_params(),
            until: t0 + MESSAGE_DURATION,
        };
        let mut wrong = Vec::new();
        for recording in [false, true] {
            for message in [None, Some(msg.clone())] {
                for queued_jobs in [0usize, 1, 2] {
                    let want = if recording {
                        OverlayState::Recording
                    } else if let Some(m) = &message {
                        OverlayState::Message {
                            id: m.id,
                            params: m.params.clone(),
                            until: m.until,
                        }
                    } else if queued_jobs > 0 {
                        OverlayState::Processing
                    } else {
                        OverlayState::Hidden
                    };
                    let got = IndicatorState::derive(&IndicatorInputs {
                        hotkey_error: false,
                        recording,
                        error: false,
                        message: message.clone(),
                        queued_jobs,
                    })
                    .overlay;
                    if got != want {
                        wrong.push(format!(
                            "recording={recording} message={} queued={queued_jobs}: \
                             {got:?}, expected {want:?}",
                            message.is_some()
                        ));
                    }
                }
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    #[test]
    fn message_carries_id_params_and_expiry() {
        // The overlay message is the one passed in, params included (CannotReach's
        // `host`), and `until` unchanged. Bite: params dropped, a fixed id, until
        // recomputed.
        let t0 = Instant::now();
        let reason = FailureReason::CannotReach {
            host: "api.example.com:8443".to_string(),
        };
        let shown = ShownMessage {
            id: reason.message_id(),
            params: reason.message_params(),
            until: t0 + MESSAGE_DURATION,
        };
        let got = IndicatorState::derive(&IndicatorInputs {
            hotkey_error: false,
            recording: false,
            error: true,
            message: Some(shown),
            queued_jobs: 1,
        });
        assert_eq!(
            got,
            IndicatorState {
                tray: TrayState::Error,
                overlay: OverlayState::Message {
                    id: i18n::FAILURE_CANNOT_REACH,
                    params: vec![("host", "api.example.com:8443".to_string())],
                    until: t0 + Duration::from_secs(3),
                },
            }
        );
    }
}
