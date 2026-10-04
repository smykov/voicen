//! The dictation job: a finished recording through the speech gate, the engine and
//! post-processing to the delivery decision (spec 001 US1, T-001, decisions #43,
//! #47).
//!
//! [`Pipeline::run_job`] is the only path a finished recording takes. It is
//! blocking and runs on the caller's plain std thread, never inside a tokio
//! runtime (the blocking reqwest client panics there, T-040). The job is reported
//! only through [`DictationEvent`](crate::events::DictationEvent)s on the one
//! [`PipelineObserver`] and through the returned [`JobReport`].
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use crate::engine::{engine_for, Engine};
use crate::events::PipelineObserver;
use crate::failure::FailureReason;
use crate::platform::{Clipboard, Paster, PendingId, StartWindow, TempAudioStore};
use crate::post_process::PostProcessor;
use crate::recording::{FinishedRecording, JobEnd};
use crate::secrets::CredentialStore;
use crate::settings::Settings;
use crate::timeouts::Timeouts;
use crate::vad::SpeechGate;

/// Builds the engine for one job from its settings snapshot (default
/// [`engine_for`]; T-017 wraps it in the shell for `BuiltinLocal`).
pub type EngineFactory =
    dyn Fn(&Settings, &dyn CredentialStore) -> Result<Box<dyn Engine>, FailureReason> + Send + Sync;

/// The recording controller's context `C`, taken at press (T-006:
/// `Paster::capture_start_window`, `SettingsService::snapshot`), so the job uses
/// the settings in force when the recording started (Clarification 4).
#[derive(Debug, Clone)]
pub struct PressContext {
    pub start_window: Option<StartWindow>,
    pub settings: Arc<Settings>,
}

/// Everything the pipeline talks to.
pub struct PipelineDeps {
    pub gate: SpeechGate,
    /// The same instance as the `SettingsService`'s.
    pub credentials: Arc<dyn CredentialStore>,
    pub clipboard: Arc<dyn Clipboard>,
    pub paster: Arc<dyn Paster>,
    pub temp_audio: Arc<dyn TempAudioStore>,
    pub observer: Arc<dyn PipelineObserver>,
    pub post_processor: Arc<dyn PostProcessor>,
}

/// How a job ended, for `RecordingController::job_finished`, and the pending
/// recording it left (only after a retryable failure whose audio was stored).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobReport {
    pub end: JobEnd,
    pub pending: Option<PendingId>,
}

/// The job sequencer. `Send + Sync`, shared by the job threads.
pub struct Pipeline {
    // Skeleton (T-001 red tests): the fields are read by the implementation.
    #[allow(dead_code)]
    deps: PipelineDeps,
    #[allow(dead_code)]
    timeouts: Timeouts,
    #[allow(dead_code)]
    factory: Box<EngineFactory>,
}

impl Pipeline {
    /// The production pipeline: [`Timeouts::default()`] and [`engine_for`].
    pub fn new(deps: PipelineDeps) -> Pipeline {
        Pipeline {
            deps,
            timeouts: Timeouts::default(),
            factory: Box::new(engine_for),
        }
    }

    /// Test durations (milliseconds) instead of the FR-24 defaults.
    #[cfg(any(test, feature = "test-fakes"))]
    pub fn with_timeouts(deps: PipelineDeps, timeouts: Timeouts) -> Pipeline {
        Pipeline {
            deps,
            timeouts,
            factory: Box::new(engine_for),
        }
    }

    /// Replaces the engine factory (T-017's `BuiltinLocal` wrapper; test fakes).
    pub fn with_engine_factory(self, factory: Box<EngineFactory>) -> Pipeline {
        Pipeline { factory, ..self }
    }

    /// Runs one job to its end. Blocking: call it on a plain std thread, never
    /// inside a tokio runtime.
    pub fn run_job(&self, rec: FinishedRecording<PressContext>) -> JobReport {
        // Skeleton (T-001 red tests): not implemented yet.
        let _ = rec;
        JobReport {
            end: JobEnd::Failed(FailureReason::UnexpectedResponse),
            pending: None,
        }
    }

    /// The pending recording and its last reason (T-006/T-007: tray
    /// `retry_available`, the toast's retry id).
    pub fn pending(&self) -> Option<(PendingId, FailureReason)> {
        // Skeleton (T-001 red tests): not implemented yet.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::AudioBuffer;
    use crate::engine::TranscribeRequest;
    use crate::events::{DictationEvent, OutcomeCode, RecordingObserver};
    use crate::i18n;
    use crate::platform::{FakeClipboard, FakePaster, FakeTempAudioStore, StoreCall, WindowRef};
    use crate::post_process::{PassThrough, PostProcessed};
    use crate::recording::{MicCause, Press, RecordingController, Release};
    use crate::secrets::{CredentialCall, CredentialOp, FakeCredentialStore, KeySlot};
    use crate::settings::{defaults, EngineKind};
    use crate::test_support::fixtures;
    use crate::vad::EnergyDetector;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, PoisonError};
    use std::time::{Duration, Instant};

    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What the fake factory and engine saw.
    #[derive(Default)]
    struct Seen {
        factory_calls: AtomicUsize,
        settings: Mutex<Vec<Settings>>,
        requests: Mutex<Vec<TranscribeRequest>>,
    }

    struct FakeEngine {
        reply: Result<String, FailureReason>,
        seen: Arc<Seen>,
    }

    impl Engine for FakeEngine {
        fn kind(&self) -> &'static str {
            "fake"
        }
        fn transcribe(
            &self,
            _audio: &AudioBuffer,
            req: &TranscribeRequest,
        ) -> Result<String, FailureReason> {
            lock(&self.seen.requests).push(req.clone());
            self.reply.clone()
        }
    }

    /// A factory that reads the transcription key from the store it is given (so
    /// the credential log shows which store that was) and builds a [`FakeEngine`].
    fn fake_factory(reply: Result<String, FailureReason>, seen: &Arc<Seen>) -> Box<EngineFactory> {
        let seen = Arc::clone(seen);
        Box::new(move |settings: &Settings, creds: &dyn CredentialStore| {
            seen.factory_calls.fetch_add(1, Ordering::SeqCst);
            lock(&seen.settings).push(settings.clone());
            let _ = creds.read(KeySlot::TranscriptionApi);
            Ok(Box::new(FakeEngine {
                reply: reply.clone(),
                seen: Arc::clone(&seen),
            }) as Box<dyn Engine>)
        })
    }

    /// Returns a fixed text and records its inputs.
    struct FixedPostProcessor {
        out: String,
        inputs: Mutex<Vec<String>>,
    }

    impl FixedPostProcessor {
        fn new(out: &str) -> Arc<FixedPostProcessor> {
            Arc::new(FixedPostProcessor {
                out: out.to_string(),
                inputs: Mutex::new(Vec::new()),
            })
        }
        fn inputs(&self) -> Vec<String> {
            lock(&self.inputs).clone()
        }
    }

    impl PostProcessor for FixedPostProcessor {
        fn process(&self, text: String, _settings: &Settings) -> PostProcessed {
            lock(&self.inputs).push(text);
            PostProcessed {
                text: self.out.clone(),
                notice: None,
            }
        }
    }

    struct Fakes {
        clipboard: Arc<FakeClipboard>,
        paster: Arc<FakePaster>,
        store: Arc<FakeTempAudioStore>,
        observer: Arc<RecordingObserver>,
        creds: Arc<FakeCredentialStore>,
    }

    fn fakes() -> Fakes {
        Fakes {
            clipboard: Arc::new(FakeClipboard::new()),
            paster: Arc::new(FakePaster::new()),
            store: Arc::new(FakeTempAudioStore::new()),
            observer: Arc::new(RecordingObserver::new()),
            creds: Arc::new(
                FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, "sk-test-SECRET"),
            ),
        }
    }

    fn deps(f: &Fakes, post_processor: Arc<dyn PostProcessor>) -> PipelineDeps {
        PipelineDeps {
            gate: SpeechGate::new(Ok(Box::new(EnergyDetector::new())), EnergyDetector::new()),
            credentials: f.creds.clone(),
            clipboard: f.clipboard.clone(),
            paster: f.paster.clone(),
            temp_audio: f.store.clone(),
            observer: f.observer.clone(),
            post_processor,
        }
    }

    fn settings() -> Settings {
        let mut s = defaults(None);
        s.engine = EngineKind::Api;
        s.api.base_url = "https://api.example.com/v1".to_string();
        s.speech_language = Some("de".to_string());
        s
    }

    fn window() -> StartWindow {
        StartWindow {
            handle: WindowRef(0x51),
            process_id: 7,
            elevated: false,
        }
    }

    /// A recording made through the real controller (press, release 3 s later,
    /// finish).
    fn finished(audio: AudioBuffer, settings: Settings) -> FinishedRecording<PressContext> {
        let mut ctrl = RecordingController::<PressContext>::new();
        let t0 = Instant::now();
        let ctx = PressContext {
            start_window: Some(window()),
            settings: Arc::new(settings),
        };
        assert!(matches!(ctrl.press(t0, ctx), Press::Start(_)));
        let ticket = match ctrl.release(t0 + Duration::from_secs(3)) {
            Release::Stop(t) => t,
            other => panic!("expected a stop ticket, got {other:?}"),
        };
        match ctrl.finish(ticket, Ok(audio), t0 + Duration::from_secs(3)) {
            Ok(rec) => rec,
            Err(e) => panic!("finish failed: {e:?}"),
        }
    }

    fn job_finished(events: &[DictationEvent]) -> Vec<DictationEvent> {
        events
            .iter()
            .copied()
            .filter(|e| matches!(e, DictationEvent::JobFinished { .. }))
            .collect()
    }

    #[test]
    fn request_carries_the_pipeline_timeouts_and_the_snapshot_language() {
        // Invariant (2): the engine comes from the one factory, called with the
        // job's snapshot and the deps' credential store, and every request carries
        // the pipeline's one Timeouts value. Bite: Timeouts::default() inside
        // run_job, language not copied from the snapshot (or hard-coded None),
        // another credential store passed to the factory.
        let f = fakes();
        let seen = Arc::new(Seen::default());
        let t = Timeouts {
            connect: Duration::from_millis(123),
            api_transcription: Duration::from_millis(456),
            ..Timeouts::default()
        };
        let p = Pipeline::with_timeouts(deps(&f, Arc::new(PassThrough)), t)
            .with_engine_factory(fake_factory(Ok("hallo".to_string()), &seen));
        let s = settings();
        let _ = p.run_job(finished(fixtures::speech_3s(), s.clone()));

        assert_eq!(
            lock(&seen.requests).clone(),
            vec![TranscribeRequest {
                language: Some("de".to_string()),
                timeouts: t,
            }]
        );
        assert_eq!(seen.factory_calls.load(Ordering::SeqCst), 1);
        assert_eq!(lock(&seen.settings).clone(), vec![s]);
        assert_eq!(
            f.creds.calls(),
            vec![CredentialCall {
                op: CredentialOp::Read,
                slot: KeySlot::TranscriptionApi
            }]
        );
    }

    #[test]
    fn production_constructor_uses_default_timeouts() {
        // FR-24 rests on defaults_are_fr24 plus this: Pipeline::new hands the
        // engine Timeouts::default(). Bite: Pipeline::new with other durations, or
        // the request built without the pipeline's value.
        let f = fakes();
        let seen = Arc::new(Seen::default());
        let p = Pipeline::new(deps(&f, Arc::new(PassThrough)))
            .with_engine_factory(fake_factory(Ok("hallo".to_string()), &seen));
        let _ = p.run_job(finished(fixtures::speech_3s(), settings()));
        let timeouts: Vec<Timeouts> = lock(&seen.requests).iter().map(|r| r.timeouts).collect();
        assert_eq!(timeouts, vec![Timeouts::default()]);
    }

    #[test]
    fn post_processor_output_is_what_gets_delivered() {
        // Step 4: the post-processor gets the engine's text and its output, not the
        // engine's, reaches the clipboard. Bite: delivering the engine text, or
        // skipping the post-processor.
        let f = fakes();
        let seen = Arc::new(Seen::default());
        let pp = FixedPostProcessor::new("POST TEXT");
        let p = Pipeline::new(deps(&f, pp.clone()))
            .with_engine_factory(fake_factory(Ok("ENGINE TEXT".to_string()), &seen));
        let report = p.run_job(finished(fixtures::speech_3s(), settings()));

        assert_eq!(pp.inputs(), vec!["ENGINE TEXT".to_string()]);
        assert_eq!(f.clipboard.texts(), vec!["POST TEXT".to_string()]);
        assert_eq!(
            report,
            JobReport {
                end: JobEnd::Delivered { notice: None },
                pending: None
            }
        );
        let finished_events = job_finished(&f.observer.events());
        assert!(
            matches!(
                finished_events.as_slice(),
                [DictationEvent::JobFinished {
                    engine: Some("fake"),
                    outcome: OutcomeCode::Text,
                    failure: None,
                    http_status: None,
                    ..
                }]
            ),
            "{finished_events:?}"
        );
    }

    #[test]
    fn post_processor_is_not_called_without_text() {
        // Bite: post-processing a blank text, a failure or a no-speech recording
        // (T-020 would send an LLM request for nothing), or the factory called for
        // a recording without speech.
        let cases: Vec<(&str, AudioBuffer, Result<String, FailureReason>, JobEnd)> = vec![
            (
                "blank engine text",
                fixtures::speech_3s(),
                Ok(String::new()),
                JobEnd::Notice(i18n::NOTICE_NO_SPEECH),
            ),
            (
                "engine failure",
                fixtures::speech_3s(),
                Err(FailureReason::Timeout),
                JobEnd::Failed(FailureReason::Timeout),
            ),
            (
                "silence",
                fixtures::silence_3s(),
                Ok("never asked".to_string()),
                JobEnd::Notice(i18n::NOTICE_NO_SPEECH),
            ),
        ];
        for (label, audio, reply, want) in cases {
            let f = fakes();
            let seen = Arc::new(Seen::default());
            let pp = FixedPostProcessor::new("POST TEXT");
            let silence = label == "silence";
            let p =
                Pipeline::new(deps(&f, pp.clone())).with_engine_factory(fake_factory(reply, &seen));
            let report = p.run_job(finished(audio, settings()));
            assert_eq!(report.end, want, "{label}");
            assert_eq!(pp.inputs(), Vec::<String>::new(), "{label}");
            assert_eq!(f.clipboard.texts(), Vec::<String>::new(), "{label}");
            if silence {
                assert_eq!(seen.factory_calls.load(Ordering::SeqCst), 0, "{label}");
            }
        }
    }

    #[test]
    fn empty_post_processed_text_is_no_speech() {
        // Step 4: empty after post-processing -> NoSpeech; the clipboard is written
        // only for a non-empty text (invariant (3)). Bite: an empty clipboard write
        // and a Delivered for nothing.
        let f = fakes();
        let seen = Arc::new(Seen::default());
        let p = Pipeline::new(deps(&f, FixedPostProcessor::new("")))
            .with_engine_factory(fake_factory(Ok("ENGINE TEXT".to_string()), &seen));
        let report = p.run_job(finished(fixtures::speech_3s(), settings()));
        assert_eq!(
            report,
            JobReport {
                end: JobEnd::Notice(i18n::NOTICE_NO_SPEECH),
                pending: None
            }
        );
        assert_eq!(f.clipboard.texts(), Vec::<String>::new());
        assert_eq!(f.paster.calls(), vec![]);
    }

    #[test]
    fn retryable_failure_is_kept_pending_with_its_reason() {
        // Invariant (4); http_status comes from ServerError only. Bite: no
        // put_pending, pending() not updated, the reason lost, no http_status.
        let f = fakes();
        let seen = Arc::new(Seen::default());
        let p = Pipeline::new(deps(&f, Arc::new(PassThrough))).with_engine_factory(fake_factory(
            Err(FailureReason::ServerError { status: 503 }),
            &seen,
        ));
        let report = p.run_job(finished(fixtures::speech_3s(), settings()));
        assert_eq!(
            report.end,
            JobEnd::Failed(FailureReason::ServerError { status: 503 })
        );
        let Some(id) = report.pending else {
            panic!("a retryable failure leaves a pending recording: {report:?}")
        };
        assert_eq!(
            p.pending(),
            Some((id, FailureReason::ServerError { status: 503 }))
        );
        assert_eq!(f.store.calls(), vec![StoreCall::Put(id)]);
        let finished_events = job_finished(&f.observer.events());
        assert!(
            matches!(
                finished_events.as_slice(),
                [DictationEvent::JobFinished {
                    engine: Some("fake"),
                    outcome: OutcomeCode::Failed,
                    failure: Some("ServerError"),
                    http_status: Some(503),
                    ..
                }]
            ),
            "{finished_events:?}"
        );
    }

    #[test]
    fn non_retryable_failure_is_not_kept_pending() {
        // Only a retryable Failed creates a pending recording (data-model
        // "Retryable"). The engine never returns MicrophoneUnavailable in practice;
        // a fake does here to pin that run_job asks retryable(). Bite: every
        // Failed stored as pending.
        let f = fakes();
        let seen = Arc::new(Seen::default());
        let reason = FailureReason::MicrophoneUnavailable {
            cause: MicCause::Other,
        };
        let p = Pipeline::new(deps(&f, Arc::new(PassThrough)))
            .with_engine_factory(fake_factory(Err(reason.clone()), &seen));
        let report = p.run_job(finished(fixtures::speech_3s(), settings()));
        assert_eq!(
            report,
            JobReport {
                end: JobEnd::Failed(reason),
                pending: None
            }
        );
        assert_eq!(p.pending(), None);
        assert_eq!(f.store.calls(), vec![]);
    }

    #[test]
    fn seq_increases_per_job_and_delivered_carries_it() {
        // JobFinished.seq is a pipeline counter; Delivered repeats its job's seq.
        // Bite: a constant seq, or Delivered with another job's seq.
        let f = fakes();
        let seen = Arc::new(Seen::default());
        let p = Pipeline::new(deps(&f, Arc::new(PassThrough)))
            .with_engine_factory(fake_factory(Ok("hallo".to_string()), &seen));
        let _ = p.run_job(finished(fixtures::speech_3s(), settings()));
        let _ = p.run_job(finished(fixtures::speech_3s(), settings()));
        let mut finished_seqs = Vec::new();
        let mut delivered_seqs = Vec::new();
        for e in f.observer.events() {
            match e {
                DictationEvent::JobFinished { seq, .. } => finished_seqs.push(seq),
                DictationEvent::Delivered { seq, .. } => delivered_seqs.push(seq),
                _ => {}
            }
        }
        assert_eq!(finished_seqs.len(), 2, "{finished_seqs:?}");
        assert!(
            finished_seqs
                .windows(2)
                .all(|w| matches!(w, [a, b] if a < b)),
            "{finished_seqs:?}"
        );
        assert_eq!(delivered_seqs, finished_seqs);
    }

    #[test]
    fn dictation_event_is_copy() {
        // Type-level guard of invariant (5): a String, Vec, Secret or FailureReason
        // field makes this fail to compile.
        fn assert_copy<T: Copy>() {}
        assert_copy::<DictationEvent>();
    }

    #[test]
    fn pipeline_is_send_and_sync() {
        // Compile-time guard: shared by the job threads (T-006, T-011).
        fn shared<T: Send + Sync>() {}
        shared::<Pipeline>();
        shared::<JobReport>();
        shared::<PressContext>();
    }
}
