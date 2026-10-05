//! The overlay's wire payload (spec 001 contracts/ipc.md, event `overlay://state`
//! and the `overlay_ready` reply; T-053).
//!
//! Core decides every overlay state and all of its timing
//! ([`RecordingController`](crate::recording::RecordingController) derives
//! [`OverlayState`], the dictation session publishes it and expires a message at its
//! `until`). This module only turns one state into what the overlay page shows:
//!
//! - `seq` and `lang` are passed through as given (the shell numbers each
//!   `set_overlay` in the controller's order; `lang` is the snapshot's
//!   `ui_language`), so the page keeps the payload with the highest `seq` and calls
//!   `setLanguage(lang)`.
//! - A message is rendered here, by Rust [`i18n::text`](crate::i18n::text) in
//!   `lang`, nested `mic_reason.*` arguments included (decision #64; the UI's `t`
//!   does not resolve nested ids). The page shows `text` as given.
//! - `Recording` carries the elapsed time; `Processing` and `Hidden` carry nothing.
//!   The page renders its own `overlay.recording` / `overlay.processing` texts.
//! - No message id, params, severity, duration or expiry reach the wire: every
//!   expiry is core's (no UI timer).
//!
//! Wire form (camelCase, `kind`-tagged like the other IPC enums):
//! `{"seq":7,"lang":"en","state":{"kind":"recording","elapsedMs":61000}}`,
//! `{"kind":"processing"}`, `{"kind":"message","text":"…"}`, `{"kind":"hidden"}`.
//! Pinned for the Playwright mock by `e2e/fixtures/overlay-wire.json`
//! (`e2e_overlay_wire_fixture_matches_core`).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::Duration;

use serde::ser::SerializeStruct;
use serde::Serialize;

use crate::i18n::{self, UiLanguage};
use crate::recording::OverlayState;

/// What the overlay page shows (the `state` of [`OverlayPayload`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayView {
    /// `{"kind":"recording","elapsedMs":N}`: shown as `m:ss`.
    Recording { elapsed_ms: u64 },
    /// `{"kind":"processing"}`.
    Processing,
    /// `{"kind":"message","text":"…"}`: the full text, rendered by Rust in the
    /// payload's `lang`.
    Message { text: String },
    /// `{"kind":"hidden"}`: the shell destroys the window after this.
    Hidden,
}

/// One `overlay://state` event, or the `overlay_ready` reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayPayload {
    /// The shell's number for this state; higher = newer.
    pub seq: u64,
    /// The language `state`'s text is rendered in, and the page's language.
    pub lang: UiLanguage,
    pub state: OverlayView,
}

/// The payload for `state` in `lang`, numbered `seq`. `elapsed` is the time since
/// the shell received this `Recording` (ignored for every other state). Pure.
pub fn overlay_payload(
    state: &OverlayState,
    lang: UiLanguage,
    seq: u64,
    elapsed: Duration,
) -> OverlayPayload {
    let view = match state {
        OverlayState::Hidden => OverlayView::Hidden,
        OverlayState::Recording => OverlayView::Recording {
            // Saturates instead of wrapping: u64 milliseconds is ~584 million years.
            elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        },
        OverlayState::Processing => OverlayView::Processing,
        // `until` stays in core: the session's timer expires the message.
        OverlayState::Message { id, params, .. } => {
            let args: Vec<(&str, &str)> = params
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect();
            OverlayView::Message {
                text: i18n::text(lang, *id, &args),
            }
        }
    };
    OverlayPayload {
        seq,
        lang,
        state: view,
    }
}

/// contracts/ipc.md `OverlayPayload`: `{ seq, lang, state }` (module docs: the wire
/// form).
impl Serialize for OverlayPayload {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("OverlayPayload", 3)?;
        s.serialize_field("seq", &self.seq)?;
        s.serialize_field("lang", &self.lang)?;
        s.serialize_field("state", &self.state)?;
        s.end()
    }
}

/// contracts/ipc.md `OverlayState`: `{ kind, ... }`, camelCase fields.
impl Serialize for OverlayView {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            OverlayView::Recording { elapsed_ms } => {
                let mut s = serializer.serialize_struct("OverlayState", 2)?;
                s.serialize_field("kind", "recording")?;
                s.serialize_field("elapsedMs", elapsed_ms)?;
                s.end()
            }
            OverlayView::Processing => {
                let mut s = serializer.serialize_struct("OverlayState", 1)?;
                s.serialize_field("kind", "processing")?;
                s.end()
            }
            OverlayView::Message { text } => {
                let mut s = serializer.serialize_struct("OverlayState", 2)?;
                s.serialize_field("kind", "message")?;
                s.serialize_field("text", text)?;
                s.end()
            }
            OverlayView::Hidden => {
                let mut s = serializer.serialize_struct("OverlayState", 1)?;
                s.serialize_field("kind", "hidden")?;
                s.end()
            }
        }
    }
}

// ---- window lifecycle (T-057) -------------------------------------------------------

/// Where the shell's one overlay window is in its life (T-057 invariant 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowPhase {
    /// No window, and none is being destroyed: a build may start.
    Absent,
    /// A window exists (it was built and not asked to go).
    Live,
    /// `Destroy` was issued; the window's own `Destroyed` event has not come yet, so
    /// tauri still holds its label (a build now fails with `WindowLabelAlreadyExists`).
    Destroying,
}

/// What the overlay thread does next (T-057 invariant 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowAction {
    /// Build the window and show the state numbered `seq` in it.
    Build(u64),
    /// Emit the state numbered `seq` to the live window.
    Emit(u64),
    /// Destroy the live window.
    Destroy,
    /// Nothing to do now.
    Nothing,
}

/// The platform-free overlay window lifecycle (T-057): fed the newest published
/// state (with the shell's `seq`) and the window's own `Destroyed`, it answers the
/// one window operation to run. It builds only when no window exists and none is
/// being destroyed, destroys on `Hidden`, emits only newer states to a live window,
/// and never emits to a window being destroyed. A failed build goes back to no
/// window and is retried only on a newer state. Pure.
///
/// The rule is one comparison of what is wanted (the newest seq seen and whether
/// its state is `Hidden`) with the phase: a state whose seq is not newer than the
/// newest seen changes nothing; while `Destroying` only the wanted state is
/// recorded, and `Destroyed` decides again from it.
#[derive(Debug, Clone)]
pub struct OverlayLifecycle {
    phase: WindowPhase,
    /// The newest `(seq, is_hidden)` seen; `None` before the first state.
    wanted: Option<(u64, bool)>,
}

impl Default for OverlayLifecycle {
    fn default() -> Self {
        OverlayLifecycle::new()
    }
}

impl OverlayLifecycle {
    /// No window, nothing wanted yet.
    pub fn new() -> OverlayLifecycle {
        OverlayLifecycle {
            phase: WindowPhase::Absent,
            wanted: None,
        }
    }

    /// The window's phase after the last answer.
    pub fn phase(&self) -> WindowPhase {
        self.phase
    }

    /// The newest published `state`, numbered `seq` (higher = newer; a `seq` not
    /// newer than one already seen is stale and changes nothing).
    pub fn on_state(&mut self, seq: u64, state: &OverlayState) -> WindowAction {
        if self.wanted.is_some_and(|(seen, _)| seq <= seen) {
            return WindowAction::Nothing;
        }
        let hidden = matches!(state, OverlayState::Hidden);
        self.wanted = Some((seq, hidden));
        match (self.phase, hidden) {
            (WindowPhase::Absent, true) => WindowAction::Nothing,
            (WindowPhase::Absent, false) => {
                self.phase = WindowPhase::Live;
                WindowAction::Build(seq)
            }
            (WindowPhase::Live, true) => {
                self.phase = WindowPhase::Destroying;
                WindowAction::Destroy
            }
            (WindowPhase::Live, false) => WindowAction::Emit(seq),
            // The label is still taken: the newest wanted state waits for Destroyed.
            (WindowPhase::Destroying, _) => WindowAction::Nothing,
        }
    }

    /// The window's own `Destroyed` event (tauri has freed the label). After a
    /// `Destroy` it builds the window again when the newest wanted state is shown.
    /// A `Destroyed` the reducer did not ask for (the window went by itself, e.g.
    /// at exit) leaves no window and is rebuilt only on a newer state, like a
    /// failed build; one with no window is ignored.
    pub fn on_destroyed(&mut self) -> WindowAction {
        match self.phase {
            WindowPhase::Absent => WindowAction::Nothing,
            WindowPhase::Live => {
                self.phase = WindowPhase::Absent;
                WindowAction::Nothing
            }
            WindowPhase::Destroying => match self.wanted {
                Some((seq, false)) => {
                    self.phase = WindowPhase::Live;
                    WindowAction::Build(seq)
                }
                _ => {
                    self.phase = WindowPhase::Absent;
                    WindowAction::Nothing
                }
            },
        }
    }

    /// The last `Build` returned an error: no window exists. Nothing is retried
    /// until a newer state comes (never a build loop).
    pub fn on_build_failed(&mut self) -> WindowAction {
        if self.phase == WindowPhase::Live {
            self.phase = WindowPhase::Absent;
        }
        WindowAction::Nothing
    }
}

#[cfg(test)]
mod lifecycle_tests {
    //! T-057 Acceptance 2 (core half; analysis red-test table row 1): the overlay
    //! window lifecycle reducer. State change and `Destroyed` in; `Build`, `Emit`,
    //! `Destroy` out. The window "world" in the property test is a model of tauri's
    //! (a label is free only after `Destroyed`), never the code under test.

    use std::time::{Duration, Instant};

    use super::*;
    use crate::i18n;

    fn message() -> OverlayState {
        OverlayState::Message {
            id: i18n::NOTICE_NO_SPEECH,
            params: Vec::new(),
            until: Instant::now() + Duration::from_secs(3),
        }
    }

    fn shown_states() -> Vec<OverlayState> {
        vec![OverlayState::Recording, OverlayState::Processing, message()]
    }

    /// A reducer with a live window showing `seq` (built from Recording).
    fn live(seq: u64) -> OverlayLifecycle {
        let mut r = OverlayLifecycle::new();
        assert_eq!(
            r.on_state(seq, &OverlayState::Recording),
            WindowAction::Build(seq),
            "premise: the first shown state builds"
        );
        r
    }

    #[test]
    fn a_shown_state_with_no_window_builds_it() {
        // Every non-Hidden kind builds the window when none exists, for its own seq.
        // Bite: Build only for Recording, or a Build that carries another seq.
        for (i, state) in shown_states().iter().enumerate() {
            let seq = 10 + i as u64;
            let mut r = OverlayLifecycle::new();
            assert_eq!(
                r.on_state(seq, state),
                WindowAction::Build(seq),
                "{state:?}"
            );
            assert_eq!(r.phase(), WindowPhase::Live, "{state:?}");
        }
    }

    #[test]
    fn hidden_with_no_window_does_nothing() {
        // NFR-03: no window exists while Hidden; Hidden with none is not a Destroy.
        // Bite: Destroy or Build on Hidden regardless of the phase.
        let mut r = OverlayLifecycle::new();
        assert_eq!(r.on_state(1, &OverlayState::Hidden), WindowAction::Nothing);
        assert_eq!(r.phase(), WindowPhase::Absent);
    }

    #[test]
    fn hidden_on_a_live_window_destroys_it() {
        // Invariant 3: Hidden destroys the window (no hide, no Emit of Hidden).
        // Bite: Hidden emitted to the live window instead of destroying it.
        let mut r = live(1);
        assert_eq!(r.on_state(2, &OverlayState::Hidden), WindowAction::Destroy);
        assert_eq!(r.phase(), WindowPhase::Destroying);
    }

    #[test]
    fn hidden_then_recording_while_destroying_builds_only_after_destroyed() {
        // Acceptance 2 (failure branch): a sub-0.3 s tap then a new press gives
        // Recording -> Hidden -> Recording. The rebuild waits for the old window's
        // Destroyed (tauri frees the label only then, manager/window.rs:70-71), and
        // then builds exactly once, for the newest seq. Bite: a Build while
        // Destroying (the "label already exists" error), or a Destroyed that forgets
        // the state wanted meanwhile.
        let mut r = live(1);
        assert_eq!(r.on_state(2, &OverlayState::Hidden), WindowAction::Destroy);
        assert_eq!(
            r.on_state(3, &OverlayState::Recording),
            WindowAction::Nothing,
            "a build before Destroyed"
        );
        assert_eq!(r.phase(), WindowPhase::Destroying);
        assert_eq!(r.on_destroyed(), WindowAction::Build(3));
        assert_eq!(r.phase(), WindowPhase::Live);
        // Exactly one build: the same seq again is nothing.
        assert_eq!(
            r.on_state(3, &OverlayState::Recording),
            WindowAction::Nothing,
            "a second build or emit of the seq the build already showed"
        );
    }

    #[test]
    fn a_state_change_while_destroying_emits_nothing() {
        // Invariant 2: nothing is sent to a window being destroyed; the newest state
        // waits for Destroyed. Bite: Emit while Destroying.
        let mut r = live(1);
        assert_eq!(r.on_state(2, &OverlayState::Hidden), WindowAction::Destroy);
        for (i, state) in shown_states().iter().enumerate() {
            assert_eq!(
                r.on_state(3 + i as u64, state),
                WindowAction::Nothing,
                "{state:?} while Destroying"
            );
        }
        assert_eq!(r.phase(), WindowPhase::Destroying);
        let last = 3 + shown_states().len() as u64 - 1;
        assert_eq!(r.on_destroyed(), WindowAction::Build(last));
    }

    #[test]
    fn recording_hidden_recording_coalesced_on_a_live_window_emits_and_never_destroys() {
        // The mailbox keeps only the newest state: Recording(2), Hidden(3),
        // Recording(4) published before the thread woke reach the reducer as
        // Recording(4). The live window stays and gets seq 4. Bite: a Destroy (or
        // rebuild) for a Hidden the reducer never saw, or an Emit of an older seq.
        let mut r = live(1);
        assert_eq!(
            r.on_state(4, &OverlayState::Recording),
            WindowAction::Emit(4)
        );
        assert_eq!(r.phase(), WindowPhase::Live);
    }

    #[test]
    fn each_newer_shown_state_on_a_live_window_is_emitted_with_its_seq() {
        // Acceptance 1: Recording -> Processing -> Message reach the one live
        // window as emits (no rebuild, no destroy). Bite: a Build or Destroy per
        // state change, or a constant seq.
        let mut r = live(1);
        assert_eq!(
            r.on_state(2, &OverlayState::Processing),
            WindowAction::Emit(2)
        );
        assert_eq!(r.on_state(3, &message()), WindowAction::Emit(3));
        assert_eq!(
            r.on_state(7, &OverlayState::Recording),
            WindowAction::Emit(7)
        );
        assert_eq!(r.phase(), WindowPhase::Live);
    }

    #[test]
    fn a_stale_or_repeated_seq_changes_nothing() {
        // The page keeps the highest seq (contracts/ipc.md); the thread may be woken
        // with no newer state (a Destroyed, a spurious wake). A seq not newer than
        // the last seen is no Emit, and a stale Hidden destroys nothing. Bite: no
        // seq check (re-emit on every wake), or a stale Hidden acted on.
        let mut r = live(1);
        assert_eq!(
            r.on_state(5, &OverlayState::Processing),
            WindowAction::Emit(5)
        );
        assert_eq!(
            r.on_state(5, &OverlayState::Processing),
            WindowAction::Nothing
        );
        assert_eq!(
            r.on_state(4, &OverlayState::Recording),
            WindowAction::Nothing
        );
        assert_eq!(r.on_state(3, &OverlayState::Hidden), WindowAction::Nothing);
        assert_eq!(r.phase(), WindowPhase::Live);
        // And a stale shown state never builds once Hidden destroyed the window.
        assert_eq!(r.on_state(6, &OverlayState::Hidden), WindowAction::Destroy);
        assert_eq!(r.on_destroyed(), WindowAction::Nothing);
        assert_eq!(
            r.on_state(5, &OverlayState::Recording),
            WindowAction::Nothing
        );
        assert_eq!(r.phase(), WindowPhase::Absent);
    }

    #[test]
    fn destroyed_with_hidden_still_wanted_builds_nothing() {
        // NFR-03: after Hidden the window stays gone. Bite: Destroyed always
        // rebuilding.
        let mut r = live(1);
        assert_eq!(r.on_state(2, &OverlayState::Hidden), WindowAction::Destroy);
        assert_eq!(r.on_destroyed(), WindowAction::Nothing);
        assert_eq!(r.phase(), WindowPhase::Absent);
    }

    #[test]
    fn a_failed_build_returns_to_no_window_and_retries_only_on_a_newer_state() {
        // Analysis design: a Build error moves back to no window; retry only on the
        // next state change, never in a loop. Bite: phase left Live after a failed
        // build (no rebuild ever), or a retry on the same seq (a build loop).
        let mut r = live(1);
        assert_eq!(r.on_build_failed(), WindowAction::Nothing);
        assert_eq!(r.phase(), WindowPhase::Absent);
        assert_eq!(
            r.on_state(1, &OverlayState::Recording),
            WindowAction::Nothing,
            "a retry of the failed seq"
        );
        assert_eq!(
            r.on_state(2, &OverlayState::Processing),
            WindowAction::Build(2)
        );
        assert_eq!(r.phase(), WindowPhase::Live);
    }

    /// xorshift64*: a fixed-seed generator (no new crate), so a failure replays.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// tauri's side of the window, as the shell sees it: a label exists from a
    /// Build until that window's Destroyed, which comes some time after Destroy.
    #[derive(Debug, Default)]
    struct World {
        exists: bool,
        destroy_pending: bool,
        /// The seq the window shows (set by Build and Emit).
        showing: Option<u64>,
        builds: u32,
    }

    impl World {
        /// Applies `action`, panicking on any operation tauri would refuse or the
        /// invariant forbids. `newest` is the newest seq the reducer was given.
        fn apply(&mut self, action: WindowAction, newest: u64, trace: &[String]) {
            match action {
                WindowAction::Build(seq) => {
                    assert!(
                        !self.exists,
                        "Build while the label is still taken (no Destroyed since the \
                         last Build): {trace:#?}"
                    );
                    assert!(seq <= newest, "Build of an unseen seq {seq}: {trace:#?}");
                    self.exists = true;
                    self.showing = Some(seq);
                    self.builds += 1;
                }
                WindowAction::Emit(seq) => {
                    assert!(
                        self.exists && !self.destroy_pending,
                        "Emit to no live window: {trace:#?}"
                    );
                    assert!(
                        self.showing.is_some_and(|s| seq > s) && seq <= newest,
                        "Emit of seq {seq} not newer than the shown {:?} (newest {newest}): \
                         {trace:#?}",
                        self.showing
                    );
                    self.showing = Some(seq);
                }
                WindowAction::Destroy => {
                    assert!(
                        self.exists && !self.destroy_pending,
                        "Destroy of no live window: {trace:#?}"
                    );
                    self.destroy_pending = true;
                }
                WindowAction::Nothing => {}
            }
        }

        /// The window's Destroyed, if a Destroy is outstanding.
        fn destroyed(&mut self) -> bool {
            if !self.destroy_pending {
                return false;
            }
            self.exists = false;
            self.destroy_pending = false;
            self.showing = None;
            true
        }
    }

    fn state_of(kind: u64) -> OverlayState {
        match kind {
            0 => OverlayState::Hidden,
            1 => OverlayState::Recording,
            2 => OverlayState::Processing,
            _ => message(),
        }
    }

    #[test]
    fn any_sequence_never_builds_over_a_taken_label_and_settles_to_the_newest_state() {
        // The invariant, not the known cases: for 2000 random interleavings of new
        // states (Hidden about half the time) and Destroyed events (delivered at a
        // random later point, as the Wry loop does), no Build is issued while the
        // label is taken, no Emit or Destroy reaches a window that is not live, every
        // Emit is newer than what the window shows, and once every Destroyed has
        // arrived the window exists exactly when the newest state is not Hidden and
        // then shows the newest seq. Bite: any of the single-case shortcuts above.
        let mut rng = Rng(0x7057_0057_0000_0001);
        for run in 0..2000u32 {
            let mut r = OverlayLifecycle::new();
            let mut world = World::default();
            let mut trace: Vec<String> = Vec::new();
            let mut seq = 0u64;
            let mut newest_hidden = true;
            let steps = 1 + rng.below(24);
            for _ in 0..steps {
                if world.destroy_pending && rng.below(3) == 0 {
                    world.destroyed();
                    let action = r.on_destroyed();
                    trace.push(format!("Destroyed -> {action:?}"));
                    world.apply(action, seq, &trace);
                } else {
                    seq += 1;
                    let kind = if rng.below(2) == 0 {
                        0
                    } else {
                        1 + rng.below(3)
                    };
                    let state = state_of(kind);
                    newest_hidden = kind == 0;
                    let action = r.on_state(seq, &state);
                    trace.push(format!("{seq} {state:?} -> {action:?}"));
                    world.apply(action, seq, &trace);
                }
            }
            // Settle: the outstanding Destroyed arrives.
            if world.destroyed() {
                let action = r.on_destroyed();
                trace.push(format!("Destroyed (settle) -> {action:?}"));
                world.apply(action, seq, &trace);
            }
            assert!(
                !world.destroy_pending,
                "run {run}: a Destroy issued after the window's Destroyed: {trace:#?}"
            );
            assert_eq!(
                world.exists, !newest_hidden,
                "run {run}: settled with window={} for a newest state hidden={newest_hidden}: \
                 {trace:#?}",
                world.exists
            );
            if world.exists {
                assert_eq!(
                    world.showing,
                    Some(seq),
                    "run {run}: the settled window does not show the newest seq: {trace:#?}"
                );
                assert_eq!(r.phase(), WindowPhase::Live, "run {run}: {trace:#?}");
            } else {
                assert_eq!(r.phase(), WindowPhase::Absent, "run {run}: {trace:#?}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! T-053 Acceptance 2 and 3 (core half; red-test table rows 1 and 2 of the
    //! analysis). Expected texts are spelled here from `i18n/en.json` /
    //! `i18n/ru.json` (contracts/messages.md), not derived from the code under test.
    //! Fake data only (`api.example.com`).

    use std::collections::BTreeSet;
    use std::time::Instant;

    use serde_json::{json, Value};

    use super::*;
    use crate::failure::FailureReason;
    use crate::i18n;
    use crate::recording::{MicCause, MESSAGE_DURATION};

    const LANGS: [UiLanguage; 2] = [UiLanguage::En, UiLanguage::Ru];
    const CAUSES: [MicCause; 4] = [
        MicCause::NoDevice,
        MicCause::AccessDenied,
        MicCause::Busy,
        MicCause::Other,
    ];

    /// The overlay message of `reason`, as the controller builds it (its own id and
    /// params; `until` is irrelevant to the payload).
    fn failure(reason: &FailureReason) -> OverlayState {
        OverlayState::Message {
            id: reason.message_id(),
            params: reason.message_params(),
            until: Instant::now() + MESSAGE_DURATION,
        }
    }

    fn mic(cause: MicCause) -> OverlayState {
        failure(&FailureReason::MicrophoneUnavailable { cause })
    }

    fn notice(id: i18n::MessageId) -> OverlayState {
        OverlayState::Message {
            id,
            params: Vec::new(),
            until: Instant::now() + MESSAGE_DURATION,
        }
    }

    fn cannot_reach() -> OverlayState {
        failure(&FailureReason::CannotReach {
            host: "api.example.com:8443".to_string(),
        })
    }

    /// One state of every kind, messages with no param, a plain param and a nested
    /// id included.
    fn every_kind() -> Vec<OverlayState> {
        let all = vec![
            OverlayState::Hidden,
            OverlayState::Recording,
            OverlayState::Processing,
            notice(i18n::NOTICE_NO_SPEECH),
            cannot_reach(),
            mic(MicCause::AccessDenied),
        ];
        // Exhaustive: a new OverlayState variant must be added above.
        for s in &all {
            match s {
                OverlayState::Hidden
                | OverlayState::Recording
                | OverlayState::Processing
                | OverlayState::Message { .. } => {}
            }
        }
        all
    }

    fn message_text(p: &OverlayPayload) -> &str {
        match &p.state {
            OverlayView::Message { text } => text,
            other => panic!("expected a message, got {other:?}"),
        }
    }

    fn wire(p: &OverlayPayload) -> Value {
        serde_json::to_value(p).unwrap_or_else(|e| panic!("OverlayPayload serializes: {e}"))
    }

    /// Every object key anywhere in `v`.
    fn keys(v: &Value, out: &mut BTreeSet<String>) {
        match v {
            Value::Object(m) => {
                for (k, child) in m {
                    out.insert(k.clone());
                    keys(child, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|child| keys(child, out)),
            _ => {}
        }
    }

    /// Every string value anywhere in `v`.
    fn strings<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
        match v {
            Value::String(s) => out.push(s),
            Value::Object(m) => m.values().for_each(|child| strings(child, out)),
            Value::Array(a) => a.iter().for_each(|child| strings(child, out)),
            _ => {}
        }
    }

    // ---- one payload per OverlayState kind ---------------------------------------

    #[test]
    fn recording_payload_carries_the_elapsed_milliseconds() {
        // Recording -> recording with elapsedMs = `elapsed` (R-13: a webview that
        // loads late shows the true m:ss from the overlay_ready reply). Bite:
        // elapsed ignored (0 or a constant), Recording mapped to another kind.
        for (elapsed, want) in [
            (Duration::ZERO, 0),
            (Duration::from_millis(61_000), 61_000),
            (Duration::from_millis(600_250), 600_250),
        ] {
            assert_eq!(
                overlay_payload(&OverlayState::Recording, UiLanguage::En, 3, elapsed),
                OverlayPayload {
                    seq: 3,
                    lang: UiLanguage::En,
                    state: OverlayView::Recording { elapsed_ms: want },
                },
                "elapsed {elapsed:?}"
            );
        }
    }

    #[test]
    fn processing_and_hidden_payloads_carry_only_their_kind() {
        // Processing -> processing, Hidden -> hidden, whatever `elapsed` is (it
        // belongs to Recording only). Bite: the two swapped, either mapped to
        // recording because elapsed is non-zero, Hidden turned into a message.
        let elapsed = Duration::from_millis(4_200);
        assert_eq!(
            overlay_payload(&OverlayState::Processing, UiLanguage::Ru, 8, elapsed),
            OverlayPayload {
                seq: 8,
                lang: UiLanguage::Ru,
                state: OverlayView::Processing,
            }
        );
        assert_eq!(
            overlay_payload(&OverlayState::Hidden, UiLanguage::Ru, 9, elapsed),
            OverlayPayload {
                seq: 9,
                lang: UiLanguage::Ru,
                state: OverlayView::Hidden,
            }
        );
    }

    #[test]
    fn message_text_is_the_catalog_text_in_the_payload_language() {
        // A Message is rendered by Rust in the payload's lang, with its own params:
        // a notice without params and `failure.cannot_reach {host}`. Bite: the id
        // sent as the text, params not substituted (`{host}` left), the text
        // always rendered in en, the params of another message.
        let rows = [
            (
                notice(i18n::NOTICE_NO_SPEECH),
                UiLanguage::En,
                "No speech detected",
            ),
            (
                notice(i18n::NOTICE_NO_SPEECH),
                UiLanguage::Ru,
                "Речь не распознана",
            ),
            (
                cannot_reach(),
                UiLanguage::En,
                "Cannot reach api.example.com:8443",
            ),
            (
                cannot_reach(),
                UiLanguage::Ru,
                "Не удаётся подключиться к api.example.com:8443",
            ),
        ];
        let mut wrong = Vec::new();
        for (state, lang, want) in rows {
            let got = overlay_payload(&state, lang, 1, Duration::ZERO);
            let expected = OverlayPayload {
                seq: 1,
                lang,
                state: OverlayView::Message {
                    text: want.to_string(),
                },
            };
            if got != expected {
                wrong.push(format!(
                    "{state:?} {lang:?}: {got:?}, expected {expected:?}"
                ));
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    #[test]
    fn microphone_access_denied_renders_the_full_text_in_en_and_ru() {
        // Acceptance 2 (failure branch): `failure.microphone_unavailable` whose
        // `{reason}` param carries the nested id `mic_reason.access_denied` shows
        // the full text in the payload's language, never the raw id (decision #64:
        // the UI cannot resolve nested ids). Bite: Catalog::text without the nested
        // step ("Microphone unavailable: mic_reason.access_denied"), the nested
        // text taken from en for ru, the message id or the template sent instead.
        let state = mic(MicCause::AccessDenied);
        for (lang, want) in [
            (
                UiLanguage::En,
                "Microphone unavailable: access denied in Windows privacy settings",
            ),
            (
                UiLanguage::Ru,
                "Микрофон недоступен: доступ запрещён в настройках конфиденциальности Windows",
            ),
        ] {
            let got = overlay_payload(&state, lang, 12, Duration::ZERO);
            assert_eq!(
                got,
                OverlayPayload {
                    seq: 12,
                    lang,
                    state: OverlayView::Message {
                        text: want.to_string(),
                    },
                },
                "{lang:?}"
            );
        }
    }

    #[test]
    fn every_microphone_cause_renders_its_reason_text_never_the_raw_id() {
        // The same rule for every MicCause in both languages, as a property: the
        // text is the localized prefix followed by the cause's own text, and holds
        // no catalog id and no unfilled placeholder. Bite: any cause left as
        // `mic_reason.*`, `{reason}` left unfilled, the en reason inside ru text.
        let prefix = |lang| match lang {
            UiLanguage::En => "Microphone unavailable: ",
            UiLanguage::Ru => "Микрофон недоступен: ",
        };
        let reason = |cause, lang| match (cause, lang) {
            (MicCause::NoDevice, UiLanguage::En) => "no input device",
            (MicCause::NoDevice, UiLanguage::Ru) => "нет устройства ввода",
            (MicCause::AccessDenied, UiLanguage::En) => "access denied in Windows privacy settings",
            (MicCause::AccessDenied, UiLanguage::Ru) => {
                "доступ запрещён в настройках конфиденциальности Windows"
            }
            (MicCause::Busy, UiLanguage::En) => "the device is busy",
            (MicCause::Busy, UiLanguage::Ru) => "устройство занято",
            (MicCause::Other, UiLanguage::En) => "the device could not be opened",
            (MicCause::Other, UiLanguage::Ru) => "не удалось открыть устройство",
        };
        let mut wrong = Vec::new();
        for cause in CAUSES {
            for lang in LANGS {
                let p = overlay_payload(&mic(cause), lang, 1, Duration::ZERO);
                let text = message_text(&p);
                let want = format!("{}{}", prefix(lang), reason(cause, lang));
                if text != want
                    || text.contains("mic_reason.")
                    || text.contains("failure.")
                    || text.contains('{')
                    || text.contains('}')
                {
                    wrong.push(format!("{cause:?} {lang:?}: {text:?}, expected {want:?}"));
                }
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    #[test]
    fn seq_and_lang_are_passed_through_for_every_kind() {
        // The page orders payloads by seq and takes its language from lang, so
        // both are exactly the caller's, for every kind. Bite: seq renumbered or
        // fixed, seq + 1, lang fixed to en, lang taken from the OS.
        let mut wrong = Vec::new();
        for state in every_kind() {
            for seq in [0, 1, 41, u64::MAX] {
                for lang in LANGS {
                    let p = overlay_payload(&state, lang, seq, Duration::from_millis(1_500));
                    if p.seq != seq || p.lang != lang {
                        wrong.push(format!(
                            "{state:?}: seq {} lang {:?}, expected {seq} {lang:?}",
                            p.seq, p.lang
                        ));
                    }
                }
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    // ---- the wire ------------------------------------------------------------------

    #[test]
    fn payload_wire_is_camel_case_and_kind_tagged() {
        // contracts/ipc.md: {seq, lang, state} with state tagged by `kind`
        // (snake_case values) and camelCase fields, the convention of the other IPC
        // enums. Exact objects: no extra field. Bite: `elapsed_ms`, an externally
        // tagged enum ({"Recording":{..}}), lang as "En", the state flattened into
        // the payload, a `text` on processing.
        let rows = [
            (
                OverlayPayload {
                    seq: 7,
                    lang: UiLanguage::En,
                    state: OverlayView::Recording { elapsed_ms: 61_000 },
                },
                json!({"seq": 7, "lang": "en", "state": {"kind": "recording", "elapsedMs": 61_000}}),
            ),
            (
                OverlayPayload {
                    seq: 8,
                    lang: UiLanguage::Ru,
                    state: OverlayView::Processing,
                },
                json!({"seq": 8, "lang": "ru", "state": {"kind": "processing"}}),
            ),
            (
                OverlayPayload {
                    seq: 9,
                    lang: UiLanguage::En,
                    state: OverlayView::Message {
                        text: "No speech detected".to_string(),
                    },
                },
                json!({"seq": 9, "lang": "en", "state": {"kind": "message", "text": "No speech detected"}}),
            ),
            (
                OverlayPayload {
                    seq: u64::MAX,
                    lang: UiLanguage::Ru,
                    state: OverlayView::Hidden,
                },
                json!({"seq": u64::MAX, "lang": "ru", "state": {"kind": "hidden"}}),
            ),
        ];
        for (payload, want) in rows {
            assert_eq!(wire(&payload), want, "{payload:?}");
        }
    }

    #[test]
    fn wire_carries_no_message_id_params_severity_or_expiry() {
        // Decision #64 / analysis Q-O3: the page gets rendered text only. For every
        // kind (messages with no param, a plain param and a nested id, in both
        // languages) the wire holds only seq, lang, state, kind, elapsedMs and
        // text, and no string on it is a catalog id. Bite: OverlayState's own
        // fields serialized (id, params, until), the old contract's key / params /
        // durationMs / severity / pending added, the nested id passed through as a
        // value.
        let allowed: BTreeSet<String> = ["seq", "lang", "state", "kind", "elapsedMs", "text"]
            .into_iter()
            .map(String::from)
            .collect();
        let mut wrong = Vec::new();
        for state in every_kind() {
            for lang in LANGS {
                let v = wire(&overlay_payload(
                    &state,
                    lang,
                    5,
                    Duration::from_millis(2_000),
                ));
                let mut found = BTreeSet::new();
                keys(&v, &mut found);
                let extra: Vec<&String> = found.difference(&allowed).collect();
                if !extra.is_empty() {
                    wrong.push(format!("{state:?} {lang:?}: extra keys {extra:?} in {v}"));
                }
                let mut values = Vec::new();
                strings(&v, &mut values);
                for s in values {
                    if i18n::MESSAGE_IDS
                        .iter()
                        .any(|id| serde_json::to_value(id).ok() == Some(json!(s)))
                        || s.contains("mic_reason.")
                    {
                        wrong.push(format!("{state:?} {lang:?}: catalog id {s:?} in {v}"));
                    }
                }
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    // ---- the e2e fixture -------------------------------------------------------------

    /// The Playwright mock's overlay data (T-053 UI half: `e2e/overlay.spec.ts`).
    const E2E_WIRE_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../e2e/fixtures/overlay-wire.json"
    ));

    #[test]
    fn e2e_overlay_wire_fixture_matches_core() {
        // P-010: the e2e mock holds no hand copy of the payload shape or of any
        // message text; the nested-id case on the page is text Rust rendered.
        // Every key of the fixture (except `_format`) is serde_json of
        // overlay_payload(state, lang, seq, elapsed) below, and the key set is
        // exactly core's. Bite: a field renamed or added, a kind spelled another
        // way, a catalog text changed (en or ru), the nested step lost, without
        // regenerating e2e/fixtures/overlay-wire.json.
        let fixture: Value = serde_json::from_str(E2E_WIRE_FIXTURE)
            .unwrap_or_else(|e| panic!("overlay-wire.json is valid JSON: {e}"));
        let access_denied = mic(MicCause::AccessDenied);
        let no_speech = notice(i18n::NOTICE_NO_SPEECH);
        let cases: [(&str, &OverlayState, UiLanguage, u64, Duration); 10] = [
            (
                "recording_en",
                &OverlayState::Recording,
                UiLanguage::En,
                1,
                Duration::from_millis(61_000),
            ),
            (
                "processing_en",
                &OverlayState::Processing,
                UiLanguage::En,
                2,
                Duration::ZERO,
            ),
            (
                "message_no_speech_en",
                &no_speech,
                UiLanguage::En,
                3,
                Duration::ZERO,
            ),
            (
                "message_microphone_access_denied_en",
                &access_denied,
                UiLanguage::En,
                4,
                Duration::ZERO,
            ),
            (
                "hidden_en",
                &OverlayState::Hidden,
                UiLanguage::En,
                5,
                Duration::ZERO,
            ),
            (
                "recording_ru",
                &OverlayState::Recording,
                UiLanguage::Ru,
                6,
                Duration::from_millis(5_000),
            ),
            (
                "processing_ru",
                &OverlayState::Processing,
                UiLanguage::Ru,
                7,
                Duration::ZERO,
            ),
            (
                "message_no_speech_ru",
                &no_speech,
                UiLanguage::Ru,
                8,
                Duration::ZERO,
            ),
            (
                "message_microphone_access_denied_ru",
                &access_denied,
                UiLanguage::Ru,
                9,
                Duration::ZERO,
            ),
            (
                "hidden_ru",
                &OverlayState::Hidden,
                UiLanguage::Ru,
                10,
                Duration::ZERO,
            ),
        ];
        let mut core = serde_json::Map::new();
        for (key, state, lang, seq, elapsed) in cases {
            core.insert(
                key.to_string(),
                wire(&overlay_payload(state, lang, seq, elapsed)),
            );
        }
        for (key, value) in &core {
            assert_eq!(
                fixture.get(key.as_str()),
                Some(value),
                "e2e/fixtures/overlay-wire.json {key} differs from core; core says:\n{}",
                serde_json::to_string_pretty(value).unwrap_or_else(|e| e.to_string())
            );
        }
        let fixture_keys: BTreeSet<&str> = fixture
            .as_object()
            .unwrap_or_else(|| panic!("overlay-wire.json is not an object"))
            .keys()
            .map(String::as_str)
            .filter(|k| *k != "_format")
            .collect();
        let core_keys: BTreeSet<&str> = core.keys().map(String::as_str).collect();
        assert_eq!(
            fixture_keys, core_keys,
            "e2e/fixtures/overlay-wire.json keys differ from core's"
        );
    }
}
