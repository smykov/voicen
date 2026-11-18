//! T-130 (spec 007 T033, NFR-03): the call indicator window shares the dictation
//! overlay's lifecycle reducer (`voicen_core::overlay::OverlayLifecycle`, T-057). The
//! reducer is generalised from `&OverlayState` to a shown/hidden input. Red until the
//! developer adds (the existing `on_state`, `on_wake`, `on_destroyed`, `on_build_failed`,
//! `generation`, `phase` stay as they are; `lifecycle_tests` in overlay.rs stay green
//! unchanged):
//!
//! ```text
//! OverlayLifecycle::on_visible(&mut self, seq: u64, visible: bool) -> WindowAction
//!     // on_state(seq, s) == on_visible(seq, !matches!(s, OverlayState::Hidden))
//! OverlayLifecycle::on_wake_visible(&mut self, destroyed: &[u64],
//!                                   newest: Option<(u64, bool)>) -> WindowAction
//!     // on_wake(d, Some((seq, s))) == on_wake_visible(d, Some((seq, !hidden(s))))
//! ```
//!
//! For the indicator: `visible` is the newest `IndicatorState.recording`. The window is
//! built at the first recording state, never at app start, destroyed when recording ends
//! (NFR-03, not hidden), rebuilt at the next call. Both overlay and indicator call the
//! SAME reducer (the differential test below pins that the two inputs agree).

use voicen_core::overlay::{OverlayLifecycle, WindowAction, WindowPhase};
use voicen_core::recording::OverlayState;

/// A reducer with a live window showing `seq`.
fn live(seq: u64) -> OverlayLifecycle {
    let mut r = OverlayLifecycle::new();
    assert_eq!(
        r.on_visible(seq, true),
        WindowAction::Build(seq),
        "premise: the first visible state builds"
    );
    r
}

#[test]
fn nothing_is_built_before_the_first_visible_state() {
    // S1 / invariant: no window at app start; a hidden (idle) state with no window is
    // not a Destroy and not a Build. Bite: Build on any first input, Destroy on hidden.
    let mut r = OverlayLifecycle::new();
    assert_eq!(r.phase(), WindowPhase::Absent);
    assert_eq!(r.on_visible(1, false), WindowAction::Nothing);
    assert_eq!(r.phase(), WindowPhase::Absent);
    assert_eq!(r.on_wake_visible(&[], None), WindowAction::Nothing);
    assert_eq!(r.phase(), WindowPhase::Absent);
}

#[test]
fn the_first_visible_state_builds_for_its_seq() {
    // Bite: a Build carrying another seq, a phase left Absent.
    let mut r = OverlayLifecycle::new();
    assert_eq!(r.on_visible(12, true), WindowAction::Build(12));
    assert_eq!(r.phase(), WindowPhase::Live);
    assert_eq!(r.generation(), 1);
}

#[test]
fn a_hidden_state_destroys_the_live_window_and_the_next_call_builds_a_new_one() {
    // NFR-03 (destroy at stop, not hide) and the rebuild at the next Record: start ->
    // stop -> Destroyed -> start builds again with a higher generation. Bite: Hidden
    // emitted instead of destroying, a rebuild at Destroyed with hidden still wanted,
    // a constant generation.
    let mut r = live(1);
    let g1 = r.generation();
    assert_eq!(r.on_visible(2, false), WindowAction::Destroy);
    assert_eq!(r.phase(), WindowPhase::Destroying);
    assert_eq!(
        r.on_destroyed(),
        WindowAction::Nothing,
        "no rebuild while hidden"
    );
    assert_eq!(r.phase(), WindowPhase::Absent);
    assert_eq!(r.on_visible(3, true), WindowAction::Build(3));
    assert!(r.generation() > g1, "a new window, a new generation");
    assert_eq!(r.phase(), WindowPhase::Live);
}

#[test]
fn a_visible_state_while_the_old_window_is_destroying_builds_only_after_destroyed() {
    // The race of the overlay (stop then an immediate new Record): tauri frees the
    // label at Destroyed, a Build before it fails. One Build, for the newest seq.
    // Bite: Build while Destroying, a Destroyed that forgets the wanted state, a
    // second Build for the same seq.
    let mut r = live(1);
    assert_eq!(r.on_visible(2, false), WindowAction::Destroy);
    assert_eq!(
        r.on_visible(3, true),
        WindowAction::Nothing,
        "label still taken"
    );
    assert_eq!(r.phase(), WindowPhase::Destroying);
    assert_eq!(r.on_destroyed(), WindowAction::Build(3));
    assert_eq!(
        r.on_visible(3, true),
        WindowAction::Nothing,
        "no second build"
    );
}

#[test]
fn newer_visible_states_on_a_live_window_emit_and_stale_ones_change_nothing() {
    // A recording state change (a warning raised, the reminder expiring) is an Emit
    // with the newest seq, never a rebuild; a stale or repeated seq is ignored, a stale
    // hidden destroys nothing. Bite: Build/Destroy per change, no seq check.
    let mut r = live(1);
    assert_eq!(r.on_visible(2, true), WindowAction::Emit(2));
    assert_eq!(r.on_visible(2, true), WindowAction::Nothing, "repeated seq");
    assert_eq!(r.on_visible(1, true), WindowAction::Nothing, "older seq");
    assert_eq!(
        r.on_visible(1, false),
        WindowAction::Nothing,
        "a stale hidden"
    );
    assert_eq!(r.phase(), WindowPhase::Live);
}

#[test]
fn nothing_is_emitted_to_a_window_being_destroyed() {
    // Bite: Emit while Destroying.
    let mut r = live(1);
    assert_eq!(r.on_visible(2, false), WindowAction::Destroy);
    assert_eq!(r.on_visible(3, true), WindowAction::Nothing);
    assert_eq!(r.on_visible(4, true), WindowAction::Nothing);
    assert_eq!(
        r.on_destroyed(),
        WindowAction::Build(4),
        "the newest wanted seq"
    );
}

#[test]
fn a_failed_build_is_retried_only_on_a_newer_visible_state() {
    // Bite: phase left Live after a failed build (no rebuild ever), or a retry of the
    // same seq (a build loop).
    let mut r = live(1);
    assert_eq!(r.on_build_failed(), WindowAction::Nothing);
    assert_eq!(r.phase(), WindowPhase::Absent);
    assert_eq!(r.on_visible(1, true), WindowAction::Nothing, "same seq");
    assert_eq!(r.on_visible(2, true), WindowAction::Build(2));
}

#[test]
fn a_window_gone_by_itself_is_rebuilt_only_for_a_newer_visible_state_in_one_wake() {
    // The current window's Destroyed and a newer visible state arrive in one wake: the
    // wake builds the newer state (Destroyed applied before the state); a Destroyed of an
    // older generation is ignored. Bite: state applied before Destroyed (an Emit to a
    // gone window), a Destroyed acted on whatever its generation.
    let mut r = live(1);
    let g = r.generation();
    assert_eq!(
        r.on_wake_visible(&[g], Some((2, true))),
        WindowAction::Build(2)
    );
    assert!(r.generation() > g);
    assert_eq!(
        r.on_wake_visible(&[g], None),
        WindowAction::Nothing,
        "a stale Destroyed"
    );
    assert_eq!(r.phase(), WindowPhase::Live);

    let mut r = live(1);
    let g = r.generation();
    assert_eq!(
        r.on_wake_visible(&[g], Some((2, false))),
        WindowAction::Nothing
    );
    assert_eq!(r.phase(), WindowPhase::Absent);
}

#[test]
fn a_window_gone_by_itself_is_not_rebuilt_without_a_newer_visible_state() {
    // The user closes the indicator (Alt+F4) during a call: its Destroyed alone leaves no
    // window and no build loop; the next newer recording state (a warning, the reminder's end)
    // builds again. Bite: a Destroyed that rebuilds at once (build loop on a window the
    // user closed), one that leaves the phase Live (the next state is an Emit to nothing).
    let mut r = live(1);
    let g = r.generation();
    assert_eq!(r.on_wake_visible(&[g], None), WindowAction::Nothing);
    assert_eq!(r.phase(), WindowPhase::Absent);
    assert_eq!(
        r.on_visible(1, true),
        WindowAction::Nothing,
        "the seq the gone window showed"
    );
    assert_eq!(r.on_visible(2, true), WindowAction::Build(2));
    assert!(r.generation() > g);
}

/// xorshift64*: a fixed seed, so a failure replays.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) % n
    }
}

#[test]
fn the_overlay_and_the_indicator_inputs_drive_one_reducer() {
    // One reducer, two inputs: for 1000 random wake sequences (Destroyed batches with
    // generations, stale and repeated seqs, shown/hidden), a reducer fed
    // `on_wake(.., &OverlayState)` and one fed `on_wake_visible(.., bool)` return the same
    // action, phase and generation at every step. Bite: a second copy of the rule in
    // `on_wake_visible` that differs in any arm.
    let mut rng = Rng(0x7130_0000_0000_0001);
    for run in 0..1000u32 {
        let mut a = OverlayLifecycle::new();
        let mut b = OverlayLifecycle::new();
        let mut seq = 0u64;
        let mut trace = Vec::new();
        for _ in 0..(1 + rng.below(30)) {
            let g = a.generation();
            let destroyed: Vec<u64> = (0..rng.below(3))
                .map(|_| {
                    if rng.below(2) == 0 {
                        g
                    } else {
                        g.saturating_sub(1)
                    }
                })
                .collect();
            let state = if rng.below(3) == 0 {
                None
            } else {
                // Sometimes a stale or repeated seq.
                seq = if rng.below(4) == 0 {
                    seq.saturating_sub(1)
                } else {
                    seq + 1
                };
                Some((seq, rng.below(2) == 0))
            };
            let held = state.map(|(_, visible)| {
                if visible {
                    OverlayState::Recording
                } else {
                    OverlayState::Hidden
                }
            });
            let newest_overlay = state.zip(held.as_ref()).map(|((s, _), st)| (s, st));
            let act_a = a.on_wake(&destroyed, newest_overlay).action;
            let act_b = b.on_wake_visible(&destroyed, state);
            trace.push(format!("{destroyed:?} {state:?} -> {act_a:?} / {act_b:?}"));
            assert_eq!(act_a, act_b, "run {run}: {trace:#?}");
            assert_eq!(a.phase(), b.phase(), "run {run}: {trace:#?}");
            assert_eq!(a.generation(), b.generation(), "run {run}: {trace:#?}");
        }
    }
}
