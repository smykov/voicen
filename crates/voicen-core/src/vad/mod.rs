//! Speech gate (FR-12, spec 001 R-5, contracts/core-traits.md "SpeechDetector",
//! T-041).
//!
//! [`SpeechGate::decide`] is the one place a speech decision over a recording is
//! made. It is infallible: the primary detector's answer, or the
//! [`EnergyDetector`]'s when the primary is unavailable or has failed once (latched
//! for the gate's lifetime). The gate emits nothing; the caller turns
//! [`GateDecision`] into events.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

pub mod energy;

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::audio::AudioBuffer;

pub use energy::EnergyDetector;

/// Decides whether a recording contains speech. Synchronous (decisions #42).
/// Each detector owns its thresholds; the gate needs no common scale.
pub trait SpeechDetector: Send + Sync {
    /// Short stable name for the `SpeechGate` event: "silero" | "energy".
    fn name(&self) -> &'static str;
    /// Never panics on audio data.
    fn contains_speech(&self, audio: &AudioBuffer) -> Result<bool, VadError>;
}

/// Why a detector cannot decide. `Display` texts are static: no path, no audio.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadError {
    /// The detector could not be loaded (for example the model file is missing).
    Unavailable,
    /// The detector failed on this recording.
    Failed,
}

impl fmt::Display for VadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            VadError::Unavailable => "speech detector unavailable",
            VadError::Failed => "speech detector failed",
        })
    }
}

/// One speech decision over one recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateDecision {
    pub speech: bool,
    /// `name()` of the detector that actually decided.
    pub detector: &'static str,
    /// True on exactly one decision per gate: the first one made by the fallback
    /// because the primary was unavailable or failed. The caller turns it into the
    /// one `Warning{vad_fallback}` event; the gate itself emits nothing.
    pub fallback_warning: bool,
}

/// The primary detector with the energy detector as fallback. Shared by the job
/// workers (`&self`, `Send + Sync`).
pub struct SpeechGate {
    primary: Option<Box<dyn SpeechDetector>>,
    fallback: EnergyDetector,
    primary_failed: AtomicBool,
    warned: AtomicBool,
}

impl SpeechGate {
    pub fn new(
        primary: Result<Box<dyn SpeechDetector>, VadError>,
        fallback: EnergyDetector,
    ) -> SpeechGate {
        SpeechGate {
            primary_failed: AtomicBool::new(primary.is_err()),
            primary: primary.ok(),
            fallback,
            warned: AtomicBool::new(false),
        }
    }

    /// Infallible: the primary's answer, or the fallback's when the primary is
    /// unavailable or has failed once (latched for the gate's lifetime).
    ///
    /// A primary error is never treated as speech: that recording is decided by the
    /// fallback, and the primary is not called again. Exactly one decision per gate
    /// carries `fallback_warning`, even when several workers decide at once.
    pub fn decide(&self, audio: &AudioBuffer) -> GateDecision {
        if !self.primary_failed.load(Ordering::Acquire) {
            match &self.primary {
                Some(primary) => match primary.contains_speech(audio) {
                    Ok(speech) => {
                        return GateDecision {
                            speech,
                            detector: primary.name(),
                            fallback_warning: false,
                        }
                    }
                    Err(_) => self.primary_failed.store(true, Ordering::Release),
                },
                // Not reachable through `new` (no primary means it was an Err), but
                // the latch keeps the two fields consistent anyway.
                None => self.primary_failed.store(true, Ordering::Release),
            }
        }
        GateDecision {
            speech: self.fallback.detect(audio),
            detector: SpeechDetector::name(&self.fallback),
            fallback_warning: !self.warned.swap(true, Ordering::AcqRel),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fixtures;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, Mutex};

    const PRIMARY: &str = "fake-primary";

    /// A primary detector that answers from a script, then with `then` forever, and
    /// counts its calls.
    struct FakeDetector {
        script: Mutex<VecDeque<Result<bool, VadError>>>,
        then: Result<bool, VadError>,
        calls: Arc<AtomicUsize>,
    }

    fn fake(
        script: Vec<Result<bool, VadError>>,
        then: Result<bool, VadError>,
    ) -> (Box<dyn SpeechDetector>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let det = FakeDetector {
            script: Mutex::new(script.into()),
            then,
            calls: Arc::clone(&calls),
        };
        (Box::new(det), calls)
    }

    impl SpeechDetector for FakeDetector {
        fn name(&self) -> &'static str {
            PRIMARY
        }
        fn contains_speech(&self, _audio: &AudioBuffer) -> Result<bool, VadError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut script = self.script.lock().unwrap_or_else(|e| e.into_inner());
            script.pop_front().unwrap_or(self.then)
        }
    }

    fn decision(speech: bool, detector: &'static str, fallback_warning: bool) -> GateDecision {
        GateDecision {
            speech,
            detector,
            fallback_warning,
        }
    }

    fn calls(c: &Arc<AtomicUsize>) -> usize {
        c.load(Ordering::SeqCst)
    }

    #[test]
    fn primary_ok_true_is_trusted_even_on_silence() {
        // The gate uses the given detector: its Ok(true) is the decision even where
        // the energy rule says no, and its name is reported. Bite: the gate asking
        // the energy detector instead of (or as well as, AND) the primary.
        let (primary, n) = fake(vec![], Ok(true));
        let gate = SpeechGate::new(Ok(primary), EnergyDetector::new());
        assert_eq!(
            gate.decide(&fixtures::silence_3s()),
            decision(true, PRIMARY, false)
        );
        assert_eq!(calls(&n), 1);
    }

    #[test]
    fn primary_ok_false_is_trusted_even_on_speech() {
        // Bite: OR-ing the primary with the energy rule (the energy rule says speech
        // here), or a warning on a healthy primary.
        let (primary, n) = fake(vec![], Ok(false));
        let gate = SpeechGate::new(Ok(primary), EnergyDetector::new());
        assert_eq!(
            gate.decide(&fixtures::speech_3s()),
            decision(false, PRIMARY, false)
        );
        assert_eq!(
            gate.decide(&fixtures::speech_3s()),
            decision(false, PRIMARY, false)
        );
        assert_eq!(calls(&n), 2);
    }

    #[test]
    fn unavailable_primary_uses_energy_and_warns_on_the_first_decision_only() {
        // Primary Err at construction (before T-043 the shell always passes
        // Err(Unavailable)): every decision is the energy detector's, and exactly
        // the first carries fallback_warning. Bite: no fallback, a warning on every
        // decision, or none at all.
        let gate = SpeechGate::new(Err(VadError::Unavailable), EnergyDetector::new());
        assert_eq!(
            gate.decide(&fixtures::speech_3s()),
            decision(true, "energy", true)
        );
        assert_eq!(
            gate.decide(&fixtures::silence_3s()),
            decision(false, "energy", false)
        );
        assert_eq!(
            gate.decide(&fixtures::keyboard_3s()),
            decision(false, "energy", false)
        );
        assert_eq!(
            gate.decide(&fixtures::speech_3s()),
            decision(true, "energy", false)
        );
    }

    #[test]
    fn runtime_error_falls_back_for_that_recording_and_latches() {
        // The primary fails on the first recording; afterwards it would say "speech"
        // to anything. The failing recording is decided by the energy detector (with
        // the one warning) and the primary is never called again (R-5: "for that and
        // later recordings"). Bite: retrying the primary (silence would then pass as
        // speech under "fake-primary"), treating Err as speech, or re-warning.
        let (primary, n) = fake(vec![Err(VadError::Failed)], Ok(true));
        let gate = SpeechGate::new(Ok(primary), EnergyDetector::new());
        assert_eq!(
            gate.decide(&fixtures::silence_3s()),
            decision(false, "energy", true)
        );
        assert_eq!(calls(&n), 1);
        assert_eq!(
            gate.decide(&fixtures::silence_3s()),
            decision(false, "energy", false)
        );
        assert_eq!(
            gate.decide(&fixtures::speech_3s()),
            decision(true, "energy", false)
        );
        assert_eq!(calls(&n), 1, "primary not called after its error");
    }

    #[test]
    fn error_after_successes_switches_to_energy_with_one_warning() {
        // Bite: a warning flag set on construction instead of on the first fallback
        // decision, or the latch set by an Ok answer.
        let (primary, n) = fake(vec![Ok(false), Ok(false), Err(VadError::Failed)], Ok(true));
        let gate = SpeechGate::new(Ok(primary), EnergyDetector::new());
        let speech = fixtures::speech_3s();
        assert_eq!(gate.decide(&speech), decision(false, PRIMARY, false));
        assert_eq!(gate.decide(&speech), decision(false, PRIMARY, false));
        assert_eq!(gate.decide(&speech), decision(true, "energy", true));
        assert_eq!(
            gate.decide(&fixtures::silence_3s()),
            decision(false, "energy", false)
        );
        assert_eq!(calls(&n), 3);
    }

    #[test]
    fn a_failing_primary_never_lets_non_speech_through() {
        // Acceptance failure branch: whatever the error and whenever it happens, a
        // recording without speech is not let through as speech by accident; the
        // energy detector decides. Bite: mapping Err to speech=true ("fail open"),
        // or reporting the failed primary as the decider.
        let non_speech = [
            ("silence", fixtures::silence_3s()),
            ("zeros", fixtures::digital_zero_3s()),
            ("cough", fixtures::cough_1s()),
            ("keyboard", fixtures::keyboard_3s()),
        ];
        for err in [VadError::Unavailable, VadError::Failed] {
            for (label, audio) in &non_speech {
                let at_construction = SpeechGate::new(Err(err), EnergyDetector::new());
                let d = at_construction.decide(audio);
                assert!(!d.speech, "{label}, {err:?} at construction");
                assert_eq!(d.detector, "energy", "{label}, {err:?} at construction");

                let (primary, _) = fake(vec![], Err(err));
                let at_run_time = SpeechGate::new(Ok(primary), EnergyDetector::new());
                let d = at_run_time.decide(audio);
                assert!(!d.speech, "{label}, {err:?} at run time");
                assert_eq!(d.detector, "energy", "{label}, {err:?} at run time");
            }
        }
    }

    /// `threads` workers start together on one shared gate and each makes
    /// `per_thread` decisions on `audio`.
    fn decide_concurrently(
        gate: &SpeechGate,
        audio: &AudioBuffer,
        threads: usize,
        per_thread: usize,
    ) -> Vec<GateDecision> {
        let start = Barrier::new(threads);
        std::thread::scope(|s| {
            let handles: Vec<_> = (0..threads)
                .map(|_| {
                    s.spawn(|| {
                        start.wait();
                        (0..per_thread)
                            .map(|_| gate.decide(audio))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| match h.join() {
                    Ok(d) => d,
                    Err(_) => panic!("a decide() call panicked"),
                })
                .collect()
        })
    }

    #[test]
    fn one_warning_across_threads_when_primary_unavailable() {
        // R-10 runs jobs on several workers through one gate: exactly one decision
        // carries the warning however the calls interleave. Bite: a load-then-store
        // flag instead of one atomic swap (two workers both see "not warned").
        let gate = SpeechGate::new(Err(VadError::Unavailable), EnergyDetector::new());
        let got = decide_concurrently(&gate, &fixtures::silence_3s(), 8, 4);
        assert_eq!(got.len(), 32);
        assert_eq!(got.iter().filter(|d| d.fallback_warning).count(), 1);
        assert!(got.iter().all(|d| d.detector == "energy" && !d.speech));
    }

    #[test]
    fn one_warning_across_threads_when_primary_fails_at_run_time() {
        // Several workers may call the failing primary before the latch is seen;
        // still exactly one warning, and every decision is the energy detector's.
        let (primary, _) = fake(vec![], Err(VadError::Failed));
        let gate = SpeechGate::new(Ok(primary), EnergyDetector::new());
        let got = decide_concurrently(&gate, &fixtures::silence_3s(), 8, 4);
        assert_eq!(got.len(), 32);
        assert_eq!(got.iter().filter(|d| d.fallback_warning).count(), 1);
        assert!(got.iter().all(|d| d.detector == "energy" && !d.speech));
    }

    #[test]
    fn gate_and_detectors_are_send_and_sync() {
        // Compile-time guard: the gate is shared by the job workers.
        fn shared<T: Send + Sync>() {}
        shared::<SpeechGate>();
        shared::<EnergyDetector>();
        shared::<GateDecision>();
    }

    #[test]
    fn vad_error_display_is_a_static_non_empty_text_per_variant() {
        // Static texts only (no path, no audio): non-empty and distinguishable.
        let unavailable = VadError::Unavailable.to_string();
        let failed = VadError::Failed.to_string();
        assert!(!unavailable.trim().is_empty());
        assert!(!failed.trim().is_empty());
        assert_ne!(unavailable, failed);
    }
}
