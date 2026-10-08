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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::audio::AudioBuffer;
use crate::delivery::deliver;
use crate::engine::{engine_for, Engine, TranscribeRequest};
use crate::events::{DictationEvent, OutcomeCode, PipelineObserver, WarningCode};
use crate::failure::FailureReason;
use crate::i18n;
use crate::platform::{Clipboard, ClipboardError, Paster, PendingId, StartWindow, TempAudioStore};
use crate::post_process::{PostProcessInput, PostProcessOutcome, PostProcessor, SkipReason};
use crate::recording::{FinishedRecording, JobEnd, RecordingId};
use crate::secrets::CredentialStore;
use crate::settings::Settings;
use crate::timeouts::Timeouts;
use crate::vad::SpeechGate;

/// Builds the engine for one job from its settings snapshot (default
/// [`engine_for`]; T-017 wraps it in the shell for `BuiltinLocal`).
pub type EngineFactory =
    dyn Fn(&Settings, &dyn CredentialStore) -> Result<Box<dyn Engine>, FailureReason> + Send + Sync;

/// The recording controller's context `C`, taken at press by the dictation
/// session (`Paster::capture_start_window`, `SettingsService::snapshot`; T-051),
/// so the job uses the settings in force when the recording started
/// (Clarification 4).
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

/// The job sequencer. `Send + Sync`, but `release` (and so `run_job`, until
/// T-011 splits it) must run on one thread at a time: `deliver` (clipboard
/// write, modifier wait, Ctrl+V) is not serialized across jobs, so concurrent
/// jobs could paste one transcript into another job's window (research R-10:
/// one delivery thread). `process` may run concurrently. The slot lock in
/// `keep_pending` is an extra safeguard, not permission for concurrent jobs.
/// In the app the one caller is the dictation session's FIFO worker, which owns
/// the pipeline (`crate::dictation`, T-051, decision #48).
pub struct Pipeline {
    deps: PipelineDeps,
    /// Test-only override of every request's durations (`with_timeouts`); `None`
    /// in production, where each job's durations come from its settings snapshot
    /// (FR-24, decision #97).
    timeouts: Option<Timeouts>,
    factory: Box<EngineFactory>,
    /// `JobFinished.seq`, taken at `run_job` entry (T-011 moves it to the stop).
    last_seq: AtomicU64,
    /// The last [`PendingId`] handed out; ids are never reused.
    last_pending_id: AtomicU64,
    /// The one pending recording (data-model "PendingRecording": at most one).
    pending: Mutex<Option<PendingRecording>>,
}

/// The pending slot's entry; its audio is in the [`TempAudioStore`] under `id`.
/// T-007 adds the start window when retry reads it.
#[derive(Debug, Clone)]
struct PendingRecording {
    id: PendingId,
    reason: FailureReason,
}

/// One job's inputs, borrowed from the recording (T-007 adds a second
/// constructor, from the pending slot, into the same `process` -> `release`).
struct Job<'a> {
    recording: RecordingId,
    audio: &'a AudioBuffer,
    start_window: Option<&'a StartWindow>,
    settings: &'a Settings,
    stopped_at: Instant,
    seq: u64,
}

/// What `process` decided, before anything is delivered or kept.
enum Outcome {
    /// A non-blank text to deliver: the post-processed text, or the raw transcript
    /// when post-processing did not run or was skipped (`skipped` then says why).
    Text {
        text: String,
        skipped: Option<SkipReason>,
    },
    NoSpeech,
    Failed(FailureReason),
}

/// The result of `process`, handed to `release` (T-011 queues it in between).
struct JobOutcome {
    outcome: Outcome,
    /// `Engine::kind()` of the engine that was built, if any.
    engine: Option<&'static str>,
    /// When the outcome was known; used only for event durations.
    at: Instant,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Nothing to deliver: empty or whitespace only.
fn is_blank(text: &str) -> bool {
    text.trim().is_empty()
}

impl Pipeline {
    fn build(deps: PipelineDeps, timeouts: Option<Timeouts>) -> Pipeline {
        Pipeline {
            deps,
            timeouts,
            factory: Box::new(engine_for),
            last_seq: AtomicU64::new(0),
            last_pending_id: AtomicU64::new(0),
            pending: Mutex::new(None),
        }
    }

    /// The production pipeline: each job's durations are
    /// [`Timeouts::from_settings`] of its settings snapshot; [`engine_for`].
    pub fn new(deps: PipelineDeps) -> Pipeline {
        Pipeline::build(deps, None)
    }

    /// Test durations (milliseconds) for every job, instead of the snapshot's.
    #[cfg(any(test, feature = "test-fakes"))]
    pub fn with_timeouts(deps: PipelineDeps, timeouts: Timeouts) -> Pipeline {
        Pipeline::build(deps, Some(timeouts))
    }

    /// Replaces the engine factory (T-017's `BuiltinLocal` wrapper; test fakes).
    pub fn with_engine_factory(self, factory: Box<EngineFactory>) -> Pipeline {
        Pipeline { factory, ..self }
    }

    /// Runs one job to its end. Blocking: call it on a plain std thread, never
    /// inside a tokio runtime.
    ///
    /// Speech gate, then (speech only) the engine from the factory with the
    /// recording's settings snapshot, then the post-processor (with the same
    /// per-job `Timeouts` as the engine), then delivery: the
    /// clipboard is written only for a non-blank text, and Ctrl+V only per
    /// [`deliver`]'s table. A retryable failure keeps the audio as the one pending
    /// recording; a skipped post-processing delivers the raw transcript and ends as
    /// `JobEnd::DeliveredSkipped`. Events, in order: `Warning{vad_fallback}` (once
    /// per gate), `SpeechGate`, `JobFinished` (after the clipboard write),
    /// `Delivered`.
    pub fn run_job(&self, rec: FinishedRecording<PressContext>) -> JobReport {
        let ctx = rec.ctx();
        let job = Job {
            recording: rec.id(),
            audio: rec.audio(),
            start_window: ctx.start_window.as_ref(),
            settings: &ctx.settings,
            stopped_at: rec.stopped_at(),
            seq: self
                .last_seq
                .fetch_add(1, Ordering::Relaxed)
                .wrapping_add(1),
        };
        let done = self.process(&job);
        self.release(&job, done)
    }

    /// The pending recording and its last reason (tray `retry_available`, read
    /// by the dictation session's worker after each job, T-051; T-007: the toast's
    /// retry id). It takes the slot lock, which `keep_pending` holds across
    /// `put_pending`: call it on the job thread, not on an input thread.
    pub fn pending(&self) -> Option<(PendingId, FailureReason)> {
        lock(&self.pending)
            .as_ref()
            .map(|p| (p.id, p.reason.clone()))
    }

    fn emit(&self, e: DictationEvent) {
        self.deps.observer.event(&e);
    }

    /// Gate, engine, post-processor. No side effects besides the `Warning` and
    /// `SpeechGate` events: no clipboard, paste or pending slot.
    fn process(&self, job: &Job<'_>) -> JobOutcome {
        let decision = self.deps.gate.decide(job.audio);
        if decision.fallback_warning {
            self.emit(DictationEvent::Warning {
                code: WarningCode::VadFallback,
            });
        }
        self.emit(DictationEvent::SpeechGate {
            recording: job.recording,
            detector: decision.detector,
            speech: decision.speech,
        });
        if !decision.speech {
            return JobOutcome {
                outcome: Outcome::NoSpeech,
                engine: None,
                at: Instant::now(),
            };
        }

        let engine = match (self.factory)(job.settings, &*self.deps.credentials) {
            Ok(engine) => engine,
            Err(reason) => {
                return JobOutcome {
                    outcome: Outcome::Failed(reason),
                    engine: None,
                    at: Instant::now(),
                }
            }
        };
        // Derived once per job from its snapshot (or the test override) and handed
        // to both the engine and the post-processor: a save applies to the next
        // job, never to a running one (P-013, Clarification 4; decision #99).
        let timeouts = self
            .timeouts
            .unwrap_or_else(|| Timeouts::from_settings(&job.settings.timeouts));
        let request = TranscribeRequest {
            language: job.settings.speech_language.clone(),
            timeouts,
        };
        let outcome = match engine.transcribe(job.audio, &request) {
            Err(reason) => Outcome::Failed(reason),
            Ok(text) if is_blank(&text) => Outcome::NoSpeech,
            Ok(raw) => {
                let input = PostProcessInput {
                    settings: &job.settings.post_processing,
                    credentials: &*self.deps.credentials,
                    timeouts: &request.timeouts,
                };
                let processed = self.deps.post_processor.process(&raw, &input);
                let text = processed.final_text(&raw);
                if is_blank(text) {
                    Outcome::NoSpeech
                } else {
                    Outcome::Text {
                        text: text.to_string(),
                        skipped: match processed {
                            PostProcessOutcome::Skipped(reason) => Some(reason),
                            PostProcessOutcome::NotRun | PostProcessOutcome::Applied(_) => None,
                        },
                    }
                }
            }
        };
        JobOutcome {
            outcome,
            engine: Some(engine.kind()),
            at: Instant::now(),
        }
    }

    /// Clipboard, paste, pending slot, `JobFinished` / `Delivered`.
    fn release(&self, job: &Job<'_>, done: JobOutcome) -> JobReport {
        let stop_to_text_ms = millis(done.at.saturating_duration_since(job.stopped_at));
        let finished =
            |outcome: OutcomeCode, failure: Option<&FailureReason>| DictationEvent::JobFinished {
                seq: job.seq,
                // The log joins the job to its recording by this id (T-008).
                recording: job.recording,
                engine: done.engine,
                stop_to_text_ms,
                outcome,
                failure: failure.map(FailureReason::code),
                http_status: match failure {
                    Some(FailureReason::ServerError { status }) => Some(*status),
                    _ => None,
                },
            };
        let reason = match done.outcome {
            Outcome::NoSpeech => {
                self.emit(finished(OutcomeCode::NoSpeech, None));
                return JobReport {
                    end: JobEnd::Notice(i18n::NOTICE_NO_SPEECH),
                    pending: None,
                };
            }
            Outcome::Failed(reason) => reason,
            Outcome::Text { text, skipped } => {
                match deliver(
                    &text,
                    job.settings.auto_paste,
                    job.start_window,
                    &*self.deps.clipboard,
                    &*self.deps.paster,
                ) {
                    Ok(result) => {
                        self.emit(finished(OutcomeCode::Text, None));
                        self.emit(DictationEvent::Delivered {
                            seq: job.seq,
                            text_to_paste_ms: millis(done.at.elapsed()),
                            result,
                        });
                        // A skip is still a delivery (spec 003 FR-007): no pending
                        // recording, `JobFinished` outcome text; the controller
                        // picks the one message by decision #91(3).
                        let end = match skipped {
                            Some(reason) => JobEnd::DeliveredSkipped {
                                reason,
                                delivery: result,
                            },
                            None => JobEnd::Delivered {
                                notice: result.notice(),
                            },
                        };
                        return JobReport { end, pending: None };
                    }
                    Err(ClipboardError) => FailureReason::ClipboardUnavailable,
                }
            }
        };

        self.emit(finished(OutcomeCode::Failed, Some(&reason)));
        let pending = if reason.retryable() {
            self.keep_pending(job.audio, &reason)
        } else {
            None
        };
        JobReport {
            end: JobEnd::Failed(reason),
            pending,
        }
    }

    /// Stores the audio under a new id, makes it the one pending recording (or
    /// none, if storing failed), and deletes the audio it replaced either way.
    ///
    /// The id, the store write and the swap happen under the slot lock as a
    /// safeguard; it is not a concurrency guarantee (`release` runs on one thread
    /// at a time, see [`Pipeline`]). The replaced audio is deleted after the lock
    /// is released: its id has left the slot and is never handed out again.
    fn keep_pending(&self, audio: &AudioBuffer, reason: &FailureReason) -> Option<PendingId> {
        let (stored, older) = {
            let mut slot = lock(&self.pending);
            let id = PendingId(
                self.last_pending_id
                    .fetch_add(1, Ordering::Relaxed)
                    .wrapping_add(1),
            );
            let stored = self
                .deps
                .temp_audio
                .put_pending(id, audio)
                .is_ok()
                .then_some(id);
            let entry = stored.map(|id| PendingRecording {
                id,
                reason: reason.clone(),
            });
            (stored, std::mem::replace(&mut *slot, entry))
        };
        if let Some(older) = older {
            // Errors are ignored: the start/exit delete_all (T-007) removes leftovers.
            let _ = self.deps.temp_audio.delete_pending(older.id);
        }
        stored
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::AudioBuffer;
    use crate::delivery::DeliveryResult;
    use crate::engine::TranscribeRequest;
    use crate::events::{DictationEvent, OutcomeCode, RecordingObserver};
    use crate::i18n;
    use crate::platform::{
        FakeClipboard, FakePaster, FakeTempAudioStore, PasterCall, StoreCall, WindowRef,
    };
    use crate::post_process::{PassThrough, PostProcessInput, PostProcessOutcome, SkipReason};
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

    /// Returns a fixed outcome and records what it was given: the raw text, the
    /// job's post-processing settings and `Timeouts`; it reads the post-processing
    /// key from the store it is given, so the credential log shows which store
    /// that was (T-020).
    struct FixedPostProcessor {
        out: PostProcessOutcome,
        inputs: Mutex<Vec<String>>,
        settings: Mutex<Vec<crate::post_process::settings::PostProcessingSettings>>,
        timeouts: Mutex<Vec<Timeouts>>,
    }

    impl FixedPostProcessor {
        /// `Applied(out)` (the pre-T-020 fake returned this text).
        fn new(out: &str) -> Arc<FixedPostProcessor> {
            FixedPostProcessor::returning(PostProcessOutcome::Applied(out.to_string()))
        }
        fn returning(out: PostProcessOutcome) -> Arc<FixedPostProcessor> {
            Arc::new(FixedPostProcessor {
                out,
                inputs: Mutex::new(Vec::new()),
                settings: Mutex::new(Vec::new()),
                timeouts: Mutex::new(Vec::new()),
            })
        }
        fn inputs(&self) -> Vec<String> {
            lock(&self.inputs).clone()
        }
        fn timeouts(&self) -> Vec<Timeouts> {
            lock(&self.timeouts).clone()
        }
    }

    impl PostProcessor for FixedPostProcessor {
        fn process(&self, raw: &str, input: &PostProcessInput<'_>) -> PostProcessOutcome {
            lock(&self.inputs).push(raw.to_string());
            lock(&self.settings).push(input.settings.clone());
            lock(&self.timeouts).push(*input.timeouts);
            let _ = input.credentials.read(KeySlot::PostProcessing);
            self.out.clone()
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
        // FR-24 rests on defaults_are_fr24 plus this: Pipeline::new derives each
        // job's Timeouts from that job's settings snapshot, and the default
        // snapshot (`settings()`) converts to Timeouts::default(). The per-job case
        // with other values is request_carries_the_job_settings_timeouts. Bite:
        // Pipeline::new with other durations, or the request built without the
        // snapshot's value.
        let f = fakes();
        let seen = Arc::new(Seen::default());
        let p = Pipeline::new(deps(&f, Arc::new(PassThrough)))
            .with_engine_factory(fake_factory(Ok("hallo".to_string()), &seen));
        let _ = p.run_job(finished(fixtures::speech_3s(), settings()));
        let timeouts: Vec<Timeouts> = lock(&seen.requests).iter().map(|r| r.timeouts).collect();
        assert_eq!(timeouts, vec![Timeouts::default()]);
    }

    /// `settings()` with the given timeouts (whole seconds).
    fn settings_with_timeouts(t: crate::settings::TimeoutSettings) -> Settings {
        let mut s = settings();
        s.timeouts = t;
        s
    }

    #[test]
    fn request_carries_the_job_settings_timeouts() {
        // T-073 (P-013): every duration of a job comes from that job's settings
        // snapshot, through the production constructor, with no restart: two jobs on
        // one pipeline with different snapshots get different request timeouts, and
        // a hand-edited out-of-range value reaches the engine clamped. Bite: the
        // pipeline's one Timeouts (Timeouts::default()) copied into every request, a
        // value taken once at construction or from the first job, a role read from
        // another setting, or no clamp on the job path.
        use crate::settings::TimeoutSettings;
        let f = fakes();
        let seen = Arc::new(Seen::default());
        let p = Pipeline::new(deps(&f, Arc::new(PassThrough)))
            .with_engine_factory(fake_factory(Ok("hallo".to_string()), &seen));

        let first = TimeoutSettings {
            connect_s: 7,
            api_transcription_s: 45,
            local_server_s: 90,
            post_processing_s: 20,
            builtin_local_s: 150,
        };
        let second = TimeoutSettings {
            connect_s: 2,
            api_transcription_s: 9,
            local_server_s: 11,
            post_processing_s: 6,
            builtin_local_s: 33,
        };
        // A hand-edited file: load does not validate, the conversion clamps.
        let edited = TimeoutSettings {
            connect_s: 0,
            api_transcription_s: u32::MAX,
            ..second
        };
        for t in [first, second, edited] {
            let _ = p.run_job(finished(fixtures::speech_3s(), settings_with_timeouts(t)));
        }

        let secs = Duration::from_secs;
        let want = |connect, api, local, post, builtin| Timeouts {
            connect: secs(connect),
            api_transcription: secs(api),
            local_server: secs(local),
            post_processing: secs(post),
            builtin: secs(builtin),
            download_no_data: secs(30),
        };
        let got: Vec<Timeouts> = lock(&seen.requests).iter().map(|r| r.timeouts).collect();
        assert_eq!(
            got,
            vec![
                want(7, 45, 90, 20, 150),
                want(2, 9, 11, 6, 33),
                want(1, 600, 11, 6, 33),
            ]
        );
        // Every request still carries its snapshot's language.
        assert!(lock(&seen.requests)
            .iter()
            .all(|r| r.language.as_deref() == Some("de")));
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

    // ---- T-020: the post-processing stage -------------------------------------

    #[test]
    fn post_processor_gets_the_job_timeouts_of_its_request() {
        // #99 / T-020 invariant: Pipeline::process derives one Timeouts per job
        // (the snapshot's, or the with_timeouts override) and hands the same value
        // to the engine and to the post-processor. Two jobs with different
        // snapshots on one Pipeline::new: each post-processor input equals that
        // job's request timeouts and Timeouts::from_settings(snapshot); under
        // with_timeouts(t) both are t. Bite: Timeouts::default() in the
        // post-processing path, the override field read directly (None in
        // production), a value fixed at construction or taken from the first job,
        // two derivations of which only one sees the override.
        use crate::settings::TimeoutSettings;
        let first = TimeoutSettings {
            connect_s: 7,
            api_transcription_s: 45,
            local_server_s: 90,
            post_processing_s: 20,
            builtin_local_s: 150,
        };
        let second = TimeoutSettings {
            connect_s: 2,
            api_transcription_s: 9,
            local_server_s: 11,
            post_processing_s: 6,
            builtin_local_s: 33,
        };

        let f = fakes();
        let seen = Arc::new(Seen::default());
        let pp = FixedPostProcessor::new("POST TEXT");
        let p = Pipeline::new(deps(&f, pp.clone()))
            .with_engine_factory(fake_factory(Ok("ENGINE TEXT".to_string()), &seen));
        for t in [first, second] {
            let _ = p.run_job(finished(fixtures::speech_3s(), settings_with_timeouts(t)));
        }
        let requested: Vec<Timeouts> = lock(&seen.requests).iter().map(|r| r.timeouts).collect();
        let want = vec![
            Timeouts::from_settings(&first),
            Timeouts::from_settings(&second),
        ];
        assert_ne!(want.first(), want.last(), "the two snapshots must differ");
        assert_eq!(pp.timeouts(), want, "post-processor");
        assert_eq!(requested, want, "engine request");

        let f = fakes();
        let seen = Arc::new(Seen::default());
        let pp = FixedPostProcessor::new("POST TEXT");
        let t = Timeouts {
            connect: Duration::from_millis(123),
            post_processing: Duration::from_millis(789),
            ..Timeouts::default()
        };
        let p = Pipeline::with_timeouts(deps(&f, pp.clone()), t)
            .with_engine_factory(fake_factory(Ok("ENGINE TEXT".to_string()), &seen));
        let _ = p.run_job(finished(
            fixtures::speech_3s(),
            settings_with_timeouts(first),
        ));
        let requested: Vec<Timeouts> = lock(&seen.requests).iter().map(|r| r.timeouts).collect();
        assert_eq!(pp.timeouts(), vec![t], "post-processor under with_timeouts");
        assert_eq!(requested, vec![t], "engine request under with_timeouts");
    }

    #[test]
    fn post_processor_gets_the_snapshot_settings_and_the_deps_credentials() {
        // spec 003 FR-010: the stage reads the post-processing settings of the
        // job's press snapshot and the deps' one credential store. Bite: defaults
        // or a live settings value instead of the snapshot, another store.
        let f = fakes();
        let seen = Arc::new(Seen::default());
        let pp = FixedPostProcessor::new("POST TEXT");
        let p = Pipeline::new(deps(&f, pp.clone()))
            .with_engine_factory(fake_factory(Ok("ENGINE TEXT".to_string()), &seen));
        let mut s = settings();
        s.post_processing.enabled = true;
        s.post_processing.base_url = "https://llm.example.com/v1".to_string();
        s.post_processing.model = "gpt-test-mini".to_string();
        s.post_processing.prompt = "PROMPT-MARKER".to_string();
        let _ = p.run_job(finished(fixtures::speech_3s(), s.clone()));
        assert_eq!(lock(&pp.settings).clone(), vec![s.post_processing]);
        assert_eq!(pp.inputs(), vec!["ENGINE TEXT".to_string()]);
        assert_eq!(
            f.creds.calls(),
            vec![
                CredentialCall {
                    op: CredentialOp::Read,
                    slot: KeySlot::TranscriptionApi
                },
                CredentialCall {
                    op: CredentialOp::Read,
                    slot: KeySlot::PostProcessing
                },
            ]
        );
    }

    #[test]
    fn skipped_post_processing_delivers_the_raw_transcript() {
        // Acceptance 2 / spec 003 FR-007, FR-016: a skip delivers the engine's text
        // byte for byte, ends as DeliveredSkipped with its reason and the delivery
        // result, keeps no pending recording, and logs JobFinished outcome text
        // with no failure. auto_paste off gives CopiedOnly in the same end. Bite:
        // the skip delivered as Delivered (no tray error, reason lost), as Failed
        // (pending kept, nothing pasted), the reason replaced, nothing delivered.
        let raw = "  ENGINE  TEXT\twith spacing ";
        for (auto_paste, delivery, pasted) in [
            (true, DeliveryResult::Pasted, true),
            (false, DeliveryResult::CopiedOnly, false),
        ] {
            let f = fakes();
            let seen = Arc::new(Seen::default());
            let pp = FixedPostProcessor::returning(PostProcessOutcome::Skipped(SkipReason::Http {
                status: 500,
            }));
            let p = Pipeline::new(deps(&f, pp.clone()))
                .with_engine_factory(fake_factory(Ok(raw.to_string()), &seen));
            let mut s = settings();
            s.auto_paste = auto_paste;
            let report = p.run_job(finished(fixtures::speech_3s(), s));

            assert_eq!(
                report,
                JobReport {
                    end: JobEnd::DeliveredSkipped {
                        reason: SkipReason::Http { status: 500 },
                        delivery,
                    },
                    pending: None,
                },
                "auto_paste {auto_paste}"
            );
            assert_eq!(pp.inputs(), vec![raw.to_string()]);
            assert_eq!(
                f.clipboard.texts(),
                vec![raw.to_string()],
                "raw byte for byte"
            );
            assert_eq!(
                f.paster
                    .calls()
                    .iter()
                    .any(|c| matches!(c, PasterCall::SendCtrlV)),
                pasted,
                "auto_paste {auto_paste}: {:?}",
                f.paster.calls()
            );
            assert_eq!(p.pending(), None);
            assert_eq!(f.store.calls(), vec![]);
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
    }

    #[test]
    fn not_run_delivers_the_raw_transcript_as_delivered() {
        // PostProcessOutcome::final_text: NotRun (post-processing off) delivers the
        // engine's text, a plain Delivered with the delivery notice. Bite: NotRun
        // treated as a skip (tray error on every dictation), or as no speech.
        let f = fakes();
        let seen = Arc::new(Seen::default());
        let pp = FixedPostProcessor::returning(PostProcessOutcome::NotRun);
        let p = Pipeline::new(deps(&f, pp.clone()))
            .with_engine_factory(fake_factory(Ok("ENGINE TEXT".to_string()), &seen));
        let mut s = settings();
        s.auto_paste = false;
        let report = p.run_job(finished(fixtures::speech_3s(), s));
        assert_eq!(
            report,
            JobReport {
                end: JobEnd::Delivered {
                    notice: Some(i18n::NOTICE_COPIED)
                },
                pending: None
            }
        );
        assert_eq!(f.clipboard.texts(), vec!["ENGINE TEXT".to_string()]);
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
