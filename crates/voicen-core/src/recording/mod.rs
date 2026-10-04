//! The recording state machine, hold mode (spec 001 data-model "Recording",
//! FR-02, T-042).
//!
//! [`RecordingController`] is the only place in core that decides a recording
//! starts or ends and the only constructor of a [`FinishedRecording`]. It is
//! sans-IO: no threads, no capture, no clock. Every instant comes from the
//! caller's event (the hotkey thread stamps press and release), so worker or lock
//! delay never counts in the 0.3 s hold. It owns the [`IndicatorState`]: the
//! indicator changes only inside controller methods.
//!
//! One press yields at most one [`FinishedRecording`]: `release` hands out a
//! [`StopTicket`] (not `Clone`, built only here) and goes idle at once, and
//! `finish` consumes the ticket. A hold under [`MIN_HOLD`], a capture error or a
//! stale id yields none.
//!
//! The shell calls `settings::gate::dictation_gate` before [`press`]; the engine =
//! none check is not repeated here.
//!
//! [`press`]: RecordingController::press
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

pub mod indicator;

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use crate::audio::AudioBuffer;
use crate::failure::FailureReason;
use crate::i18n::{self, MessageId};

use indicator::{IndicatorInputs, ShownMessage};
pub use indicator::{IndicatorState, JobEnd, OverlayState, TrayState, MESSAGE_DURATION};

/// A hold shorter than this is discarded silently (FR-02); exactly 300 ms is kept.
pub const MIN_HOLD: Duration = Duration::from_millis(300);

/// One recording, monotonic per controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecordingId(u64);

/// Why a recording ended. T-009 and T-006 add the other variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingEnd {
    Released,
    TooShort,
}

/// The capture failure the shell reports (contracts/core-traits.md `CaptureError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    NoDevice,
    AccessDenied,
    DeviceBusy,
    /// OS error text; never copied into a [`FailureReason`].
    Other(String),
}

/// The closed reason set of "microphone unavailable" (contracts/messages.md `mic_reason.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicCause {
    NoDevice,
    AccessDenied,
    Busy,
    Other,
}

impl MicCause {
    /// The cause of a capture error. `Other`'s OS text is dropped here (P-009).
    pub fn of(err: &CaptureError) -> MicCause {
        match err {
            CaptureError::NoDevice => MicCause::NoDevice,
            CaptureError::AccessDenied => MicCause::AccessDenied,
            CaptureError::DeviceBusy => MicCause::Busy,
            CaptureError::Other(_) => MicCause::Other,
        }
    }

    /// The `mic_reason.*` text, the `{reason}` of `failure.microphone_unavailable`.
    pub fn message_id(self) -> MessageId {
        match self {
            MicCause::NoDevice => i18n::MIC_REASON_NO_DEVICE,
            MicCause::AccessDenied => i18n::MIC_REASON_ACCESS_DENIED,
            MicCause::Busy => i18n::MIC_REASON_BUSY,
            MicCause::Other => i18n::MIC_REASON_OTHER,
        }
    }
}

/// Result of [`RecordingController::press`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    Start(RecordingId),
    /// Auto-repeat while recording.
    Ignored,
}

/// Result of [`RecordingController::release`].
#[derive(Debug)]
pub enum Release<C> {
    /// Stop the capture and pass the audio to [`RecordingController::finish`].
    Stop(StopTicket<C>),
    /// Held shorter than [`MIN_HOLD`]: nothing is sent.
    Discarded { id: RecordingId, held: Duration },
    /// No recording on.
    Ignored,
}

/// The right to finish one recording. Not `Clone`; built only by the controller.
#[derive(Debug)]
pub struct StopTicket<C> {
    id: RecordingId,
    ctx: C,
    started_at: Instant,
    stopped_at: Instant,
}

impl<C> StopTicket<C> {
    pub fn id(&self) -> RecordingId {
        self.id
    }
}

/// A recording ready for a job. Built only by [`RecordingController::finish`].
#[derive(Debug)]
pub struct FinishedRecording<C> {
    id: RecordingId,
    audio: AudioBuffer,
    ctx: C,
    started_at: Instant,
    stopped_at: Instant,
    end: RecordingEnd,
}

impl<C> FinishedRecording<C> {
    pub fn id(&self) -> RecordingId {
        self.id
    }
    pub fn audio(&self) -> &AudioBuffer {
        &self.audio
    }
    pub fn ctx(&self) -> &C {
        &self.ctx
    }
    /// The press instant.
    pub fn started_at(&self) -> Instant {
        self.started_at
    }
    /// The release instant (not the instant the capture finished stopping).
    pub fn stopped_at(&self) -> Instant {
        self.stopped_at
    }
    pub fn end(&self) -> RecordingEnd {
        self.end
    }
}

/// The recording that is on.
#[derive(Debug)]
struct Live<C> {
    id: RecordingId,
    ctx: C,
    started_at: Instant,
}

/// Hold-mode recording controller. `C` is opaque context taken at press and
/// returned with the recording (T-006 passes the start window).
///
/// Methods take `&mut self` and run no threads; the shell serialises the calls.
#[derive(Debug)]
pub struct RecordingController<C> {
    live: Option<Live<C>>,
    next_id: u64,
    /// Recordings handed to a job (counted at `finish(Ok)`) whose job has not ended.
    queued: BTreeSet<RecordingId>,
    /// Tray `Error`: the last job or capture failed; cleared by a delivery or the
    /// tray menu.
    error: bool,
    message: Option<ShownMessage>,
    indicator: IndicatorState,
}

impl<C> Default for RecordingController<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C> RecordingController<C> {
    pub fn new() -> Self {
        let mut c = RecordingController {
            live: None,
            next_id: 0,
            queued: BTreeSet::new(),
            error: false,
            message: None,
            indicator: IndicatorState::default(),
        };
        c.refresh();
        c
    }

    /// Hotkey pressed at `at` (after `settings::gate::dictation_gate`). From idle
    /// it starts a recording and drops any overlay message (not re-shown); while
    /// recording it is hotkey auto-repeat and ignored.
    pub fn press(&mut self, at: Instant, ctx: C) -> Press {
        if self.live.is_some() {
            return Press::Ignored;
        }
        let id = RecordingId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        self.live = Some(Live {
            id,
            ctx,
            started_at: at,
        });
        self.message = None;
        self.refresh();
        Press::Start(id)
    }

    /// Hotkey released at `at`. The controller is idle once this returns. The
    /// hold is measured between the two event instants, saturating at zero (a
    /// release stamped before its press is a zero hold).
    pub fn release(&mut self, at: Instant) -> Release<C> {
        let Some(live) = self.live.take() else {
            return Release::Ignored;
        };
        self.expire(at);
        self.refresh();
        let held = at.saturating_duration_since(live.started_at);
        if held < MIN_HOLD {
            return Release::Discarded { id: live.id, held };
        }
        Release::Stop(StopTicket {
            id: live.id,
            ctx: live.ctx,
            started_at: live.started_at,
            stopped_at: at,
        })
    }

    /// The capture for `id` could not be opened or failed before release. For the
    /// live id: idle, the "microphone unavailable" message for 3 s and tray
    /// `Error`, and the reason is returned. A stale id returns `None` and changes
    /// nothing.
    pub fn capture_failed(
        &mut self,
        id: RecordingId,
        err: CaptureError,
        at: Instant,
    ) -> Option<FailureReason> {
        if self.live.as_ref().map(|l| l.id) != Some(id) {
            return None;
        }
        self.live = None;
        let reason = mic_unavailable(&err);
        self.fail(&reason, at);
        Some(reason)
    }

    /// The capture for `ticket` stopped with `audio`. `Ok` queues a job for the
    /// recording (overlay `Processing` until [`job_finished`](Self::job_finished));
    /// `Err` queues nothing and shows the failure like
    /// [`capture_failed`](Self::capture_failed).
    pub fn finish(
        &mut self,
        ticket: StopTicket<C>,
        audio: Result<AudioBuffer, CaptureError>,
        at: Instant,
    ) -> Result<FinishedRecording<C>, FailureReason> {
        let StopTicket {
            id,
            ctx,
            started_at,
            stopped_at,
        } = ticket;
        self.expire(at);
        match audio {
            Ok(audio) => {
                self.queued.insert(id);
                self.refresh();
                Ok(FinishedRecording {
                    id,
                    audio,
                    ctx,
                    started_at,
                    stopped_at,
                    end: RecordingEnd::Released,
                })
            }
            Err(err) => {
                let reason = mic_unavailable(&err);
                self.fail(&reason, at);
                Err(reason)
            }
        }
    }

    /// The job of recording `id` ended. An id with no queued job (unknown, or
    /// already ended) changes nothing.
    pub fn job_finished(&mut self, id: RecordingId, end: JobEnd, at: Instant) {
        if !self.queued.remove(&id) {
            return;
        }
        self.expire(at);
        match end {
            JobEnd::Delivered { notice } => {
                self.error = false;
                if let Some(notice) = notice {
                    self.show(notice, Vec::new(), at);
                }
            }
            JobEnd::Notice(notice) => self.show(notice, Vec::new(), at),
            JobEnd::Failed(reason) => {
                self.error = true;
                self.show(reason.message_id(), reason.message_params(), at);
            }
        }
        self.refresh();
    }

    /// The tray menu was opened: clears tray `Error`.
    pub fn tray_menu_opened(&mut self, at: Instant) {
        self.expire(at);
        self.error = false;
        self.refresh();
    }

    /// Timer callback: expires the overlay message (at exactly its `until`).
    pub fn tick(&mut self, at: Instant) {
        self.expire(at);
        self.refresh();
    }

    /// When the shell's timer should call [`tick`](Self::tick) next.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.message.as_ref().map(|m| m.until)
    }

    pub fn indicator(&self) -> &IndicatorState {
        &self.indicator
    }

    /// Show `reason` for 3 s and set tray `Error`.
    fn fail(&mut self, reason: &FailureReason, at: Instant) {
        self.expire(at);
        self.error = true;
        self.show(reason.message_id(), reason.message_params(), at);
        self.refresh();
    }

    fn show(&mut self, id: MessageId, params: Vec<(&'static str, String)>, at: Instant) {
        self.message = Some(ShownMessage {
            id,
            params,
            until: at + MESSAGE_DURATION,
        });
    }

    fn expire(&mut self, at: Instant) {
        if self.message.as_ref().is_some_and(|m| at >= m.until) {
            self.message = None;
        }
    }

    /// Re-derive the indicator from the state; every mutating method ends here.
    fn refresh(&mut self) {
        self.indicator = IndicatorState::derive(&IndicatorInputs {
            // No input sets it yet: T-006 adds the hotkey-registration input.
            hotkey_error: false,
            recording: self.live.is_some(),
            error: self.error,
            message: self.message.clone(),
            queued_jobs: self.queued.len(),
        });
    }
}

fn mic_unavailable(err: &CaptureError) -> FailureReason {
    FailureReason::MicrophoneUnavailable {
        cause: MicCause::of(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{self, MessageId};

    /// Opaque context standing in for T-001's `StartWindow`.
    type Ctx = &'static str;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn audio() -> AudioBuffer {
        AudioBuffer::from_16k_mono(vec![1, -2, 3, -4, 5])
    }

    fn idle() -> IndicatorState {
        IndicatorState {
            tray: TrayState::Idle,
            overlay: OverlayState::Hidden,
        }
    }

    /// Any notice id (the controller treats it as opaque).
    const NOTICE: MessageId = i18n::NOTICE_SETTINGS_RESET;

    fn start(c: &mut RecordingController<Ctx>, at: Instant, ctx: Ctx) -> RecordingId {
        match c.press(at, ctx) {
            Press::Start(id) => id,
            Press::Ignored => panic!("press from idle was ignored"),
        }
    }

    fn stop(c: &mut RecordingController<Ctx>, at: Instant) -> StopTicket<Ctx> {
        match c.release(at) {
            Release::Stop(t) => t,
            other => panic!("expected Stop, got {other:?}"),
        }
    }

    /// press at t0, release at t0 + held, finish with `audio()`.
    fn record(
        c: &mut RecordingController<Ctx>,
        t0: Instant,
        held: Duration,
    ) -> FinishedRecording<Ctx> {
        start(c, t0, "window");
        let ticket = stop(c, t0 + held);
        match c.finish(ticket, Ok(audio()), t0 + held + ms(5)) {
            Ok(f) => f,
            Err(e) => panic!("finish failed: {e:?}"),
        }
    }

    fn message(id: MessageId, params: Vec<(&'static str, String)>, at: Instant) -> OverlayState {
        OverlayState::Message {
            id,
            params,
            until: at + Duration::from_secs(3),
        }
    }

    // ---- AC1: hold length ------------------------------------------------------

    #[test]
    fn hold_of_at_least_300ms_gives_one_finished_recording() {
        // FR-02: 300 (boundary, kept), 301 and 4000 ms each yield exactly one
        // FinishedRecording carrying the captured audio, the press context, the
        // event instants and end = Released; the ticket is consumed, so a second
        // release is Ignored. Bite: `held > MIN_HOLD` (300 discarded), a release
        // that does not go idle (second release stops again), stopped_at taken
        // from finish's instant, ctx not returned.
        for held in [300u64, 301, 4000] {
            let t0 = Instant::now();
            let mut c = RecordingController::<Ctx>::new();
            let id = start(&mut c, t0, "start-window");
            let ticket = stop(&mut c, t0 + ms(held));
            assert_eq!(ticket.id(), id, "held {held} ms: ticket id");
            let f = match c.finish(ticket, Ok(audio()), t0 + ms(held + 40)) {
                Ok(f) => f,
                Err(e) => panic!("held {held} ms: finish failed: {e:?}"),
            };
            assert_eq!(f.id(), id, "held {held} ms: id");
            assert_eq!(f.audio(), &audio(), "held {held} ms: audio");
            assert_eq!(*f.ctx(), "start-window", "held {held} ms: ctx");
            assert_eq!(f.started_at(), t0, "held {held} ms: started_at");
            assert_eq!(f.stopped_at(), t0 + ms(held), "held {held} ms: stopped_at");
            assert_eq!(f.end(), RecordingEnd::Released, "held {held} ms: end");
            assert!(
                matches!(c.release(t0 + ms(held + 50)), Release::Ignored),
                "held {held} ms: a second release must be Ignored"
            );
        }
    }

    #[test]
    fn hold_under_300ms_is_discarded_and_indicator_returns_to_idle() {
        // FR-02: 0, 1 and 299 ms -> Discarded with the hold length, no ticket, tray
        // Idle, overlay Hidden, no message and no timer. Bite: `held >= MIN_HOLD`
        // replaced by `>` on the wrong side, a Stop for short holds, the overlay
        // left on Recording/Processing, a message for the silent discard.
        for held in [0u64, 1, 299] {
            let t0 = Instant::now();
            let mut c = RecordingController::<Ctx>::new();
            let id = start(&mut c, t0, "w");
            match c.release(t0 + ms(held)) {
                Release::Discarded { id: got, held: h } => {
                    assert_eq!(got, id, "held {held} ms: id");
                    assert_eq!(h, ms(held), "held {held} ms: held");
                }
                other => panic!("held {held} ms: expected Discarded, got {other:?}"),
            }
            assert_eq!(c.indicator(), &idle(), "held {held} ms: indicator");
            assert_eq!(c.next_deadline(), None, "held {held} ms: deadline");
            assert!(
                matches!(c.release(t0 + ms(held + 10)), Release::Ignored),
                "held {held} ms: a second release must be Ignored"
            );
        }
    }

    #[test]
    fn recording_ids_are_distinct_and_increasing() {
        // data-model: RecordingId is monotonic. Bite: a constant id (a stale
        // capture_failed or job_finished would then hit the live recording).
        let t0 = Instant::now();
        let mut c = RecordingController::<Ctx>::new();
        let a = record(&mut c, t0, ms(500)).id();
        let b = record(&mut c, t0 + ms(1000), ms(500)).id();
        let d = record(&mut c, t0 + ms(2000), ms(500)).id();
        assert!(a < b && b < d, "ids not increasing: {a:?} {b:?} {d:?}");
    }

    // ---- AC2: indicator follows one job ----------------------------------------

    #[test]
    fn indicator_recording_then_processing_then_hidden() {
        // One job: press -> Recording/Recording; release -> tray Idle; finish ->
        // overlay Processing; Delivered (pasted) -> Hidden. Bite: no Recording on
        // press, no job counted at finish, the job not removed on Delivered.
        let t0 = Instant::now();
        let mut c = RecordingController::<Ctx>::new();
        let id = start(&mut c, t0, "w");
        assert_eq!(
            c.indicator(),
            &IndicatorState {
                tray: TrayState::Recording,
                overlay: OverlayState::Recording,
            },
            "after press"
        );
        let ticket = stop(&mut c, t0 + ms(1000));
        assert_eq!(c.indicator().tray, TrayState::Idle, "tray after release");
        assert!(c.finish(ticket, Ok(audio()), t0 + ms(1010)).is_ok());
        assert_eq!(
            c.indicator(),
            &IndicatorState {
                tray: TrayState::Idle,
                overlay: OverlayState::Processing,
            },
            "after finish"
        );
        c.job_finished(id, JobEnd::Delivered { notice: None }, t0 + ms(2000));
        assert_eq!(c.indicator(), &idle(), "after delivery");
        assert_eq!(c.next_deadline(), None);
    }

    #[test]
    fn failure_shows_message_for_3s_then_hidden_tray_error_until_delivery() {
        // Failed(Timeout) -> overlay Message until +3 s, tray Error, timer at +3 s;
        // tick before +3 s changes nothing; tick at +3 s -> Hidden (exactly 3 s
        // expires); tray Error stays until a later job is Delivered. Bite: no
        // expiry in tick, expiry at `>` instead of `>=`, Error cleared by the
        // tick, Delivered not clearing Error.
        let t0 = Instant::now();
        let mut c = RecordingController::<Ctx>::new();
        let a = record(&mut c, t0, ms(1000)).id();
        let tf = t0 + ms(5000);
        c.job_finished(a, JobEnd::Failed(FailureReason::Timeout), tf);
        assert_eq!(
            c.indicator(),
            &IndicatorState {
                tray: TrayState::Error,
                overlay: message(i18n::FAILURE_TIMEOUT, vec![], tf),
            },
            "after failure"
        );
        assert_eq!(c.next_deadline(), Some(tf + Duration::from_secs(3)));
        c.tick(tf + ms(2999));
        assert_eq!(
            c.indicator().overlay,
            message(i18n::FAILURE_TIMEOUT, vec![], tf),
            "tick before expiry"
        );
        c.tick(tf + ms(3000));
        assert_eq!(
            c.indicator(),
            &IndicatorState {
                tray: TrayState::Error,
                overlay: OverlayState::Hidden,
            },
            "after expiry"
        );
        assert_eq!(c.next_deadline(), None, "no timer after expiry");

        let b = record(&mut c, t0 + ms(10_000), ms(1000)).id();
        assert_eq!(
            c.indicator().tray,
            TrayState::Error,
            "Error survives a new job"
        );
        c.job_finished(b, JobEnd::Delivered { notice: None }, t0 + ms(12_000));
        assert_eq!(c.indicator(), &idle(), "Delivered clears Error");
    }

    #[test]
    fn tray_menu_opened_clears_error_but_not_the_message() {
        // data-model: Error is cleared by opening the tray menu. Bite: a no-op
        // tray_menu_opened, or one that also drops the overlay message.
        let t0 = Instant::now();
        let mut c = RecordingController::<Ctx>::new();
        let a = record(&mut c, t0, ms(1000)).id();
        let tf = t0 + ms(2000);
        let reason = FailureReason::CannotReach {
            host: "api.example.com".to_string(),
        };
        c.job_finished(a, JobEnd::Failed(reason.clone()), tf);
        assert_eq!(c.indicator().tray, TrayState::Error);
        c.tray_menu_opened(tf + ms(500));
        assert_eq!(
            c.indicator(),
            &IndicatorState {
                tray: TrayState::Idle,
                overlay: message(reason.message_id(), reason.message_params(), tf),
            }
        );
    }

    #[test]
    fn notice_shows_message_without_tray_error() {
        // Notice (no speech, paste manually) -> Message for 3 s, tray unchanged.
        // Bite: a notice treated as a failure (tray Error), or no message.
        let t0 = Instant::now();
        let mut c = RecordingController::<Ctx>::new();
        let a = record(&mut c, t0, ms(1000)).id();
        let tn = t0 + ms(2000);
        c.job_finished(a, JobEnd::Notice(NOTICE), tn);
        assert_eq!(
            c.indicator(),
            &IndicatorState {
                tray: TrayState::Idle,
                overlay: message(NOTICE, vec![], tn),
            }
        );
        assert_eq!(c.next_deadline(), Some(tn + Duration::from_secs(3)));
    }

    #[test]
    fn delivered_with_notice_shows_it_and_clears_error() {
        // CopiedOnly: Delivered{notice: Some} -> its message, and tray Error from
        // an earlier failure cleared. Bite: the notice ignored on Delivered, or
        // Error kept.
        let t0 = Instant::now();
        let mut c = RecordingController::<Ctx>::new();
        let a = record(&mut c, t0, ms(1000)).id();
        c.job_finished(a, JobEnd::Failed(FailureReason::Timeout), t0 + ms(2000));
        let b = record(&mut c, t0 + ms(6000), ms(1000)).id();
        let td = t0 + ms(8000);
        c.job_finished(
            b,
            JobEnd::Delivered {
                notice: Some(NOTICE),
            },
            td,
        );
        assert_eq!(
            c.indicator(),
            &IndicatorState {
                tray: TrayState::Idle,
                overlay: message(NOTICE, vec![], td),
            }
        );
    }

    #[test]
    fn new_recording_preempts_message_and_it_is_not_reshown() {
        // data-model: a new recording pre-empts the message display, and the
        // message is not re-shown. Tray: Recording over Error, Error after.
        // Bite: the message kept and shown again after a short (discarded) hold
        // that ends within its 3 s.
        let t0 = Instant::now();
        let mut c = RecordingController::<Ctx>::new();
        let a = record(&mut c, t0, ms(1000)).id();
        let tf = t0 + ms(2000);
        c.job_finished(a, JobEnd::Failed(FailureReason::Timeout), tf);
        start(&mut c, tf + ms(1000), "w");
        assert_eq!(
            c.indicator(),
            &IndicatorState {
                tray: TrayState::Recording,
                overlay: OverlayState::Recording,
            },
            "during the new recording"
        );
        assert!(matches!(
            c.release(tf + ms(1100)),
            Release::Discarded { .. }
        ));
        assert_eq!(
            c.indicator(),
            &IndicatorState {
                tray: TrayState::Error,
                overlay: OverlayState::Hidden,
            },
            "after the discard, still inside the message's 3 s"
        );
        assert_eq!(c.next_deadline(), None);
    }

    #[test]
    fn message_wins_over_processing_and_processing_returns_after_expiry() {
        // Two queued jobs: the first fails -> Message (over Processing); after the
        // 3 s the second is still queued -> Processing. Bite: a job counter reset
        // on any job end, Processing over Message.
        let t0 = Instant::now();
        let mut c = RecordingController::<Ctx>::new();
        let a = record(&mut c, t0, ms(1000)).id();
        let b = record(&mut c, t0 + ms(1500), ms(1000)).id();
        let tf = t0 + ms(3000);
        c.job_finished(a, JobEnd::Failed(FailureReason::Timeout), tf);
        assert_eq!(
            c.indicator().overlay,
            message(i18n::FAILURE_TIMEOUT, vec![], tf)
        );
        c.tick(tf + ms(3000));
        assert_eq!(c.indicator().overlay, OverlayState::Processing);
        c.job_finished(b, JobEnd::Delivered { notice: None }, tf + ms(4000));
        assert_eq!(
            c.indicator(),
            &IndicatorState {
                tray: TrayState::Idle,
                overlay: OverlayState::Hidden,
            }
        );
    }

    #[test]
    fn press_after_release_starts_while_previous_job_is_finishing() {
        // FR-029: the controller is idle as soon as release returns, so a new
        // press starts before the previous capture is finished; Recording wins
        // over Processing; jobs are keyed by id. Bite: release staying busy until
        // finish, a single job flag instead of a per-id set.
        let t0 = Instant::now();
        let mut c = RecordingController::<Ctx>::new();
        let a = start(&mut c, t0, "first");
        let ta = stop(&mut c, t0 + ms(1000));
        let b = start(&mut c, t0 + ms(1100), "second");
        assert_ne!(a, b);
        let fa = match c.finish(ta, Ok(audio()), t0 + ms(1200)) {
            Ok(f) => f,
            Err(e) => panic!("finish a: {e:?}"),
        };
        assert_eq!(*fa.ctx(), "first");
        assert_eq!(
            c.indicator(),
            &IndicatorState {
                tray: TrayState::Recording,
                overlay: OverlayState::Recording,
            },
            "b recording while a is queued"
        );
        let tb = stop(&mut c, t0 + ms(2000));
        let fb = match c.finish(tb, Ok(audio()), t0 + ms(2100)) {
            Ok(f) => f,
            Err(e) => panic!("finish b: {e:?}"),
        };
        assert_eq!(fb.id(), b);
        assert_eq!(*fb.ctx(), "second");
        c.job_finished(a, JobEnd::Delivered { notice: None }, t0 + ms(3000));
        assert_eq!(
            c.indicator().overlay,
            OverlayState::Processing,
            "b still queued"
        );
        c.job_finished(b, JobEnd::Delivered { notice: None }, t0 + ms(4000));
        assert_eq!(c.indicator(), &idle());
    }

    #[test]
    fn job_finished_for_unknown_or_repeated_id_changes_nothing() {
        // An unknown id changes nothing (not even tray Error); a repeated id cannot
        // underflow the queue and hide a later job. Bite: a decrementing counter
        // (the repeat hides job c's Processing), Delivered for an unknown id
        // clearing Error.
        let t0 = Instant::now();
        let mut c = RecordingController::<Ctx>::new();
        let a = record(&mut c, t0, ms(1000)).id();
        c.job_finished(a, JobEnd::Failed(FailureReason::Timeout), t0 + ms(2000));
        c.tick(t0 + ms(5000));
        let before = c.indicator().clone();
        assert_eq!(before.tray, TrayState::Error);
        c.job_finished(a, JobEnd::Delivered { notice: None }, t0 + ms(5100));
        assert_eq!(c.indicator(), &before, "repeated id");

        let b = record(&mut c, t0 + ms(6000), ms(1000)).id();
        c.job_finished(b, JobEnd::Delivered { notice: None }, t0 + ms(8000));
        c.job_finished(b, JobEnd::Delivered { notice: None }, t0 + ms(8100));
        let d = record(&mut c, t0 + ms(9000), ms(1000)).id();
        c.job_finished(b, JobEnd::Notice(NOTICE), t0 + ms(10_100));
        assert_eq!(
            c.indicator(),
            &IndicatorState {
                tray: TrayState::Idle,
                overlay: OverlayState::Processing,
            },
            "job d still queued after repeats for b"
        );
        c.job_finished(d, JobEnd::Delivered { notice: None }, t0 + ms(11_000));
        assert_eq!(c.indicator(), &idle());
    }

    // ---- AC3: failure branch ----------------------------------------------------

    #[test]
    fn release_without_press_is_ignored_and_leaves_controller_usable() {
        // A stray release (key released after a gate refusal, or twice) is Ignored,
        // never panics, shows nothing, and the next press/release still works.
        // Bite: a release that stamps a phantom recording, or that panics.
        let t0 = Instant::now();
        let mut c = RecordingController::<Ctx>::new();
        assert!(matches!(c.release(t0), Release::Ignored));
        assert_eq!(c.indicator(), &idle());
        let f = record(&mut c, t0 + ms(100), ms(800));
        assert_eq!(f.started_at(), t0 + ms(100));
    }

    #[test]
    fn second_press_while_recording_is_ignored_as_auto_repeat() {
        // data-model.md:53: a press while recording is hotkey auto-repeat. Still
        // one recording, with the first press's context and instant; one ticket
        // after release. Bite: a second press restarting the recording (new id,
        // later started_at, the hold measured from the repeat -> discarded) or
        // ending it.
        let t0 = Instant::now();
        let mut c = RecordingController::<Ctx>::new();
        let id = start(&mut c, t0, "first");
        for k in 1..=5u64 {
            assert_eq!(
                c.press(t0 + ms(30 * k), "repeat"),
                Press::Ignored,
                "repeat {k}"
            );
        }
        assert_eq!(c.indicator().overlay, OverlayState::Recording);
        // 310 ms after the first press, 160 ms after the last repeat.
        let ticket = stop(&mut c, t0 + ms(310));
        assert_eq!(ticket.id(), id);
        let f = match c.finish(ticket, Ok(audio()), t0 + ms(320)) {
            Ok(f) => f,
            Err(e) => panic!("finish: {e:?}"),
        };
        assert_eq!(f.id(), id);
        assert_eq!(*f.ctx(), "first");
        assert_eq!(f.started_at(), t0);
        assert!(matches!(c.release(t0 + ms(400)), Release::Ignored));
    }

    #[test]
    fn capture_failed_on_live_id_gives_microphone_unavailable_and_idle() {
        // data-model "Idle --press [device fails]--> Idle + failure": for the live
        // id the controller goes idle with MicrophoneUnavailable{cause}; no job is
        // queued, the later key release is Ignored, and the next press starts a new
        // recording. Bite: capture_failed returning None for the live id, the
        // controller left Recording, a job counted.
        let t0 = Instant::now();
        let mut c = RecordingController::<Ctx>::new();
        let id = start(&mut c, t0, "w");
        let got = c.capture_failed(id, CaptureError::AccessDenied, t0 + ms(20));
        assert_eq!(
            got,
            Some(FailureReason::MicrophoneUnavailable {
                cause: MicCause::AccessDenied,
            })
        );
        assert_ne!(c.indicator().tray, TrayState::Recording, "tray");
        assert!(
            !matches!(
                c.indicator().overlay,
                OverlayState::Recording | OverlayState::Processing
            ),
            "overlay {:?}: no recording, no job",
            c.indicator().overlay
        );
        assert!(matches!(c.release(t0 + ms(800)), Release::Ignored));
        let next = start(&mut c, t0 + ms(1000), "w");
        assert_ne!(next, id);
    }

    #[test]
    fn capture_error_maps_to_closed_mic_cause_without_os_text() {
        // core-traits CaptureError -> messages.md mic_reason set; the OS text of
        // Other never reaches the reason (P-009). Bite: DeviceBusy -> Other, a
        // reason carrying the OS string.
        let os_text = "0x88890004 example-os-text-canary";
        let rows = [
            (CaptureError::NoDevice, MicCause::NoDevice),
            (CaptureError::AccessDenied, MicCause::AccessDenied),
            (CaptureError::DeviceBusy, MicCause::Busy),
            (CaptureError::Other(os_text.to_string()), MicCause::Other),
        ];
        for (err, cause) in rows {
            let t0 = Instant::now();
            let mut c = RecordingController::<Ctx>::new();
            let id = start(&mut c, t0, "w");
            let got = c.capture_failed(id, err.clone(), t0 + ms(10));
            assert_eq!(
                got,
                Some(FailureReason::MicrophoneUnavailable { cause }),
                "{err:?}"
            );
            if let Some(r) = got {
                assert!(
                    !format!("{r:?} {r}").contains("canary"),
                    "{err:?}: OS text leaked into {r:?}"
                );
            }
        }
    }

    #[test]
    fn capture_failed_with_stale_id_returns_none_and_changes_nothing() {
        // A late capture error for an earlier recording must not end the live one.
        // Bite: capture_failed ignoring the id.
        let t0 = Instant::now();
        let mut c = RecordingController::<Ctx>::new();
        let old = record(&mut c, t0, ms(500)).id();
        c.job_finished(old, JobEnd::Delivered { notice: None }, t0 + ms(600));
        let live = start(&mut c, t0 + ms(1000), "live");
        let before = c.indicator().clone();
        assert_eq!(
            c.capture_failed(old, CaptureError::DeviceBusy, t0 + ms(1100)),
            None
        );
        assert_eq!(c.indicator(), &before);
        let ticket = stop(&mut c, t0 + ms(2000));
        assert_eq!(ticket.id(), live, "the live recording still stops normally");
    }

    #[test]
    fn capture_failed_after_release_is_stale() {
        // Once release returned, the id is no longer live (the ticket owns it).
        // Bite: a capture_failed that still matches the stopped id and reports a
        // second outcome for it.
        let t0 = Instant::now();
        let mut c = RecordingController::<Ctx>::new();
        let id = start(&mut c, t0, "w");
        let ticket = stop(&mut c, t0 + ms(1000));
        assert_eq!(
            c.capture_failed(id, CaptureError::NoDevice, t0 + ms(1001)),
            None
        );
        assert!(c.finish(ticket, Ok(audio()), t0 + ms(1010)).is_ok());
    }

    #[test]
    fn finish_with_capture_error_gives_err_and_no_job() {
        // The stream failed while stopping: Err(MicrophoneUnavailable{cause}), no
        // FinishedRecording, nothing queued; the controller is usable. Bite: an
        // empty FinishedRecording built from an Err, a job counted before the
        // audio is known.
        let t0 = Instant::now();
        let mut c = RecordingController::<Ctx>::new();
        start(&mut c, t0, "w");
        let ticket = stop(&mut c, t0 + ms(1000));
        match c.finish(ticket, Err(CaptureError::DeviceBusy), t0 + ms(1010)) {
            Err(r) => assert_eq!(
                r,
                FailureReason::MicrophoneUnavailable {
                    cause: MicCause::Busy,
                }
            ),
            Ok(f) => panic!("expected Err, got {f:?}"),
        }
        assert_ne!(c.indicator().overlay, OverlayState::Processing, "no job");
        assert_ne!(c.indicator().tray, TrayState::Recording);
        let f = record(&mut c, t0 + ms(2000), ms(500));
        assert_eq!(f.end(), RecordingEnd::Released);
    }

    #[test]
    fn release_stamped_before_press_is_a_zero_hold_and_discarded() {
        // Instants come from two threads; a release stamped earlier than its press
        // counts as a 0 s hold (saturating), is discarded, never panics. Bite:
        // `at - started_at` / `duration_since` panicking or wrapping into a long
        // hold that is kept.
        let t0 = Instant::now() + Duration::from_secs(10);
        let mut c = RecordingController::<Ctx>::new();
        let id = start(&mut c, t0, "w");
        match c.release(t0 - ms(50)) {
            Release::Discarded { id: got, held } => {
                assert_eq!(got, id);
                assert_eq!(held, Duration::ZERO);
            }
            other => panic!("expected Discarded, got {other:?}"),
        }
        assert_eq!(c.indicator(), &idle());
    }
}
