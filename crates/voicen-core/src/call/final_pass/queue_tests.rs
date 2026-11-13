//! T-146: the pass queue (spec 007 FR-027, FR-028, FR-037; tasks.md G32 T062) against fakes.
//!
//! The API these tests pin (the developer builds it in `call/final_pass.rs`; the names are the
//! test-writer's, the shapes follow `analysis.seam` and `analysis.approach` of the record):
//!
//! ```text
//! CancelToken                      Clone + Send + Sync; new(), cancel(), is_cancelled()
//! run_pass_with_cancel(call, engine, request, engine_ref, sources, deps, &CancelToken)
//!                                  -> PassOutcome   (run_pass_with = this with a fresh token)
//!   - the tracks it uploads are those whose `call.meta.results.<track>` is not `done`; a
//!     `done` track's decoder is never asked for a window, its kept segments are spliced into
//!     the merge and stay that track's result;
//!   - once the token is set it dispatches no request, writes no `transcript.*`, writes
//!     `meta.json` back as `ready-to-transcribe` with the results the pass started with, logs
//!     `FinalPassFinished { outcome: FinalOutcome::Cancelled, .. }` and sends no notice (Q1).
//! FinalOutcome::Cancelled          new variant in call/events.rs
//! trait PassRunner: Send + Sync    fn run(&self, dir: &Path, cancel: &CancelToken) -> PassOutcome
//!                                  (reads meta.json, settings and key when the pass starts)
//! ApiPassRunner::new(settings: Arc<dyn Fn() -> Arc<Settings> + Send + Sync>,
//!     creds: Arc<dyn CredentialStore>, primary: Option<Arc<dyn SpeechSpans>>,
//!     events: Arc<dyn PassEvents>, clock: Arc<dyn MonotonicClock>)   the production runner
//! PassQueue::new(store: Arc<CallStore>, runner: Arc<dyn PassRunner>)  one worker thread; Drop
//!                                  lets the running pass end and drops what is still queued
//!   enqueue(&self, dir: &Path)                    the stop path / start-up `Recovery.queue`:
//!                                                  no meta change; a call queued or running is
//!                                                  not queued twice
//!   transcribe(&self, dir: &Path) -> Result<(), CallError>   "Transcribe again": refuses a call
//!                                                  this store records (`recordingCall`) and
//!                                                  changes nothing; a call queued or running
//!                                                  is a no-op `Ok`; else the meta transition,
//!                                                  then enqueue. Transition: `ready` -> state
//!                                                  `ready-to-transcribe`, failure None, both
//!                                                  results `pending`, transcript.* removed;
//!                                                  `failed` -> failure None, `done` results
//!                                                  kept, the others `pending`; speakers kept
//!   cancel(&self, dir: &Path)                     a queued call is removed; the running one gets
//!                                                  its token set and cancel returns after the
//!                                                  pass has ended
//!   wait_idle(&self)                              returns when nothing is queued or running
//!   position(&self, dir: &Path) -> QueuePosition  { Absent, Queued, Running }
//! ```
//!
//! Audio here is constant per track (mic `MIC_MARK`, system `SYS_MARK`), the primary detector
//! says "speech everywhere", so the fake engine knows the track from the first sample and every
//! decoded window becomes chunks. No test reads the clock; waits are bounded polls.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::*;
use crate::call::codec::TrackEncoder;
use crate::call::storage::{write_meta, CallErrorCode, CallStore, Failure, NewCall, TrackResult};
use crate::call::{
    CallState, DecodeEnd, EndpointId, EngineSegment, FailReason, SegmentV1, Speaker, Track,
    TranscriptV1,
};
use crate::clock::{Clock, LocalOffset, SystemMonotonic};
use crate::diag::{Log, LogConfig};
use crate::i18n::UiLanguage;
use crate::settings::defaults;
use crate::settings::roots::FsCallRoots;
use crate::test_support::TempDir;
use crate::timeouts::Timeouts;
use crate::vad::{SpeechSpan, VadError};

// ---- small helpers ----------------------------------------------------------------

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => panic!("unexpected error: {e:?}"),
    }
}

/// Polls `cond` (2 ms apart, about 10 s at most): a wait for a state, never a fixed sleep.
fn wait_for(what: &str, cond: impl Fn() -> bool) {
    for _ in 0..5_000 {
        if cond() {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("timed out waiting for: {what}");
}

const MIC_MARK: i16 = 1_111;
const SYS_MARK: i16 = 2_222;
/// Both tracks of every call are this long.
const CALL_MS: u64 = 3_000;

fn name_of(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn read_meta(dir: &Path) -> CallMeta {
    ok(serde_json::from_slice(&ok(fs::read(dir.join("meta.json")))))
}

fn meta_bytes(dir: &Path) -> Vec<u8> {
    ok(fs::read(dir.join("meta.json")))
}

fn transcript_of(dir: &Path) -> Option<TranscriptV1> {
    let text = fs::read_to_string(dir.join("transcript.json")).ok()?;
    Some(ok(TranscriptV1::from_json(&text)))
}

fn no_transcript(dir: &Path) {
    for name in ["transcript.json", "transcript.md"] {
        assert!(!dir.join(name).exists(), "{name} must not exist");
    }
}

fn kept(track: Track, text: &str) -> SegmentV1 {
    SegmentV1 {
        start_ms: 1_000,
        end_ms: 2_000,
        track: track.as_str().to_string(),
        speaker: if track == Track::System { "s1" } else { "me" }.to_string(),
        lang: None,
        text: text.to_string(),
        source: Some("final".to_string()),
    }
}

fn texts(t: &TranscriptV1) -> Vec<(String, String)> {
    t.segments
        .iter()
        .map(|s| (s.track.clone(), s.text.clone()))
        .collect()
}

// ---- the world: a store and calls in it ---------------------------------------------

struct World {
    _tmp: TempDir,
    store: Arc<CallStore>,
    made: AtomicU64,
}

fn world() -> World {
    let tmp = TempDir::new();
    let store = Arc::new(CallStore::new(
        Arc::new(FsCallRoots::new(tmp.path().join("calls"))),
        Arc::new(SystemMonotonic),
    ));
    World {
        _tmp: tmp,
        store,
        made: AtomicU64::new(0),
    }
}

impl World {
    /// A new call this store records now (an active call): the files are open.
    fn recording(&self) -> crate::call::ActiveCallFiles {
        let n = self.made.fetch_add(1, Ordering::SeqCst);
        let new = NewCall {
            started: UNIX_EPOCH + Duration::from_secs(1_800_000_000 + 60 * n),
            seconds_east: 0,
            mic: EndpointId::new("mic-1"),
            system: EndpointId::new("sys-1"),
            lang: UiLanguage::En,
            app_version: "0.0.0".to_string(),
        };
        ok(self.store.create(&defaults(None).calls, &new))
    }

    /// A stopped call, `ready-to-transcribe`, both results pending, `CALL_MS` long; `edit`
    /// changes the meta before it is written.
    fn call(&self, edit: impl FnOnce(&mut CallMeta)) -> PathBuf {
        let files = self.recording();
        let dir = files.dir().to_path_buf();
        let mut meta = files.meta().clone();
        ok(files.mic.finish());
        ok(files.system.finish());
        self.store.release(&dir);
        meta.state = CallState::ReadyToTranscribe;
        meta.duration_ms = Some(CALL_MS);
        edit(&mut meta);
        ok(write_meta(&dir, &meta));
        dir
    }
}

/// Renamed speakers, to see that a redo keeps them.
fn rename(meta: &mut CallMeta) {
    meta.speakers = vec![
        Speaker {
            id: "me".to_string(),
            name: "Alice".to_string(),
        },
        Speaker {
            id: "s1".to_string(),
            name: "Bob".to_string(),
        },
    ];
}

/// A `failed` call: the mic track failed, the system track is done with a kept line.
fn failed_mic_done_system(meta: &mut CallMeta) {
    rename(meta);
    meta.state = CallState::Failed;
    meta.failure = Some(Failure {
        reason: FailReason::Provider,
        track: Some(Track::Mic),
    });
    meta.results.mic = TrackResult::Failed {
        reason: FailReason::Provider,
    };
    meta.results.system = TrackResult::Done {
        segments: vec![kept(Track::System, "kept-system-line")],
    };
}

/// A `ready` call with both tracks done and its two transcript files on disk.
fn ready_with_files(dir: &Path, meta: &mut CallMeta) {
    rename(meta);
    meta.state = CallState::Ready;
    meta.pass_mode = Some(PassMode::Timed);
    meta.results.mic = TrackResult::Done {
        segments: vec![kept(Track::Mic, "old-mic-line")],
    };
    meta.results.system = TrackResult::Done {
        segments: vec![kept(Track::System, "old-system-line")],
    };
    ok(fs::write(dir.join("transcript.md"), "old"));
    ok(fs::write(dir.join("transcript.json"), "{}"));
}

// ---- a gate and the runner that does nothing ------------------------------------------

struct Gate {
    open: Mutex<bool>,
    cv: Condvar,
}

impl Gate {
    fn new(open: bool) -> Arc<Gate> {
        Arc::new(Gate {
            open: Mutex::new(open),
            cv: Condvar::new(),
        })
    }

    fn open(&self) {
        *lock(&self.open) = true;
        self.cv.notify_all();
    }

    /// Blocks until opened; a gate that is never opened ends the wait after 20 s.
    fn wait(&self) {
        let g = lock(&self.open);
        let _ = self
            .cv
            .wait_timeout_while(g, Duration::from_secs(20), |open| !*open)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

/// A runner that records which call it ran, in which order and how many ran at once, and
/// blocks on the gate. It touches no file.
struct PureRunner {
    gate: Arc<Gate>,
    started: Mutex<Vec<String>>,
    running: AtomicUsize,
    peak: AtomicUsize,
}

impl PureRunner {
    fn new(open: bool) -> Arc<PureRunner> {
        Arc::new(PureRunner {
            gate: Gate::new(open),
            started: Mutex::new(Vec::new()),
            running: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        })
    }

    fn started(&self) -> Vec<String> {
        lock(&self.started).clone()
    }
}

impl PassRunner for PureRunner {
    fn run(&self, dir: &Path, _cancel: &CancelToken) -> PassOutcome {
        lock(&self.started).push(name_of(dir));
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        self.gate.wait();
        self.running.fetch_sub(1, Ordering::SeqCst);
        PassOutcome {
            outcome: FinalOutcome::Ready,
            mode: PassMode::Timed,
        }
    }
}

fn queue_of(w: &World, runner: &Arc<PureRunner>) -> PassQueue {
    PassQueue::new(w.store.clone(), runner.clone())
}

// ---- the runner that runs the real pass over a fake engine ------------------------------

#[derive(Default)]
struct Rec {
    logs: Mutex<Vec<LogEvent>>,
    notices: Mutex<Vec<CallNotice>>,
}

impl PassEvents for Rec {
    fn log(&self, event: LogEvent) {
        lock(&self.logs).push(event);
    }

    fn notice(&self, notice: CallNotice) {
        lock(&self.notices).push(notice);
    }
}

/// Says "speech" for the whole window.
struct EverywhereSpans;

impl SpeechSpans for EverywhereSpans {
    fn name(&self) -> &'static str {
        "everywhere"
    }

    fn speech_spans(&self, audio: &AudioBuffer) -> Result<Vec<SpeechSpan>, VadError> {
        let ms = (audio.samples().len() / 16) as u64;
        Ok(vec![SpeechSpan {
            start_ms: 0,
            end_ms: ms,
        }])
    }
}

struct Dec {
    mark: i16,
    left_ms: u64,
    asks: Arc<AtomicUsize>,
}

impl TrackDecoder for Dec {
    fn next_window(&mut self, max_ms: u32) -> Result<Option<AudioBuffer>, DecodeEnd> {
        self.asks.fetch_add(1, Ordering::SeqCst);
        if self.left_ms == 0 {
            return Ok(None);
        }
        let n = self.left_ms.min(u64::from(max_ms));
        self.left_ms -= n;
        Ok(Some(AudioBuffer::from_16k_mono(vec![
            self.mark;
            n as usize * 16
        ])))
    }
}

/// Answers `<track>-line` for every chunk; blocks every request on `gate` when there is one.
struct Engine {
    gate: Option<Arc<Gate>>,
    log: Mutex<Vec<Track>>,
    entered: AtomicUsize,
}

impl Engine {
    fn new(gate: Option<Arc<Gate>>) -> Arc<Engine> {
        Arc::new(Engine {
            gate,
            log: Mutex::new(Vec::new()),
            entered: AtomicUsize::new(0),
        })
    }

    fn sent(&self) -> Vec<Track> {
        lock(&self.log).clone()
    }
}

impl TimedTranscriber for Engine {
    fn transcribe_timed(
        &self,
        audio: &AudioBuffer,
        _req: &TranscribeRequest,
        shape: ResponseShape,
    ) -> Result<Timed, TimedFailure> {
        let track = if audio.samples().first() == Some(&SYS_MARK) {
            Track::System
        } else {
            Track::Mic
        };
        lock(&self.log).push(track);
        self.entered.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = &self.gate {
            gate.wait();
        }
        let text = format!("{}-line", track.as_str());
        Ok(match shape {
            ResponseShape::Segments => Timed {
                text: String::new(),
                language: None,
                segments: Some(vec![EngineSegment {
                    start_s: 0.5,
                    end_s: 1.5,
                    text,
                    lang: None,
                }]),
            },
            ResponseShape::Plain => Timed {
                text,
                language: None,
                segments: None,
            },
        })
    }
}

struct TestRunner {
    engine: Arc<Engine>,
    rec: Arc<Rec>,
    /// Track length the decoders hand out.
    length_ms: u64,
    tokens: Mutex<Vec<CancelToken>>,
    /// Windows asked of each track's decoder, over all passes: [mic, system].
    asks: [Arc<AtomicUsize>; 2],
    /// Passes that have ended.
    ended: AtomicUsize,
}

impl TestRunner {
    fn new(engine: &Arc<Engine>, length_ms: u64) -> Arc<TestRunner> {
        Arc::new(TestRunner {
            engine: engine.clone(),
            rec: Arc::new(Rec::default()),
            length_ms,
            tokens: Mutex::new(Vec::new()),
            asks: [Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0))],
            ended: AtomicUsize::new(0),
        })
    }

    fn asked(&self, track: Track) -> usize {
        let i = usize::from(track == Track::System);
        self.asks.get(i).map_or(0, |a| a.load(Ordering::SeqCst))
    }

    fn cancelled(&self) -> bool {
        lock(&self.tokens).iter().any(CancelToken::is_cancelled)
    }
}

impl PassRunner for TestRunner {
    fn run(&self, dir: &Path, cancel: &CancelToken) -> PassOutcome {
        lock(&self.tokens).push(cancel.clone());
        let meta = read_meta(dir);
        let [mic_asks, system_asks] = &self.asks;
        let sources = PassSources {
            mic: Box::new(Dec {
                mark: MIC_MARK,
                left_ms: self.length_ms,
                asks: mic_asks.clone(),
            }),
            system: Box::new(Dec {
                mark: SYS_MARK,
                left_ms: self.length_ms,
                asks: system_asks.clone(),
            }),
        };
        let request = TranscribeRequest {
            language: None,
            timeouts: Timeouts::from_settings(&defaults(None).timeouts),
        };
        let deps = PassDeps {
            primary: Some(Arc::new(EverywhereSpans)),
            events: &*self.rec,
            clock: Arc::new(SystemMonotonic),
        };
        let out = run_pass_with_cancel(
            PassCall {
                dir: dir.to_path_buf(),
                meta,
            },
            &*self.engine,
            &request,
            EngineRef::new("api", "m1"),
            sources,
            deps,
            cancel,
        );
        self.ended.fetch_add(1, Ordering::SeqCst);
        out
    }
}

// ---- guarantee: one at a time, FIFO ---------------------------------------------------------

#[test]
fn passes_run_one_at_a_time_in_the_order_they_were_requested() {
    let w = world();
    let (a, b, c) = (w.call(|_| {}), w.call(|_| {}), w.call(|_| {}));
    let runner = PureRunner::new(false);
    let q = queue_of(&w, &runner);
    q.enqueue(&a);
    wait_for("A to start", || runner.started().len() == 1);
    // B and C are stopped while A's pass runs.
    q.enqueue(&b);
    q.enqueue(&c);
    assert_eq!(q.position(&a), QueuePosition::Running);
    assert_eq!(q.position(&b), QueuePosition::Queued);
    assert_eq!(q.position(&c), QueuePosition::Queued);
    assert_eq!(runner.started(), vec![name_of(&a)], "B waits for A's pass");
    runner.gate.open();
    q.wait_idle();
    assert_eq!(
        runner.started(),
        vec![name_of(&a), name_of(&b), name_of(&c)],
        "FIFO"
    );
    assert_eq!(runner.peak.load(Ordering::SeqCst), 1, "never two at once");
    for dir in [&a, &b, &c] {
        assert_eq!(q.position(dir), QueuePosition::Absent);
    }
}

#[test]
fn a_call_queued_or_running_is_not_queued_twice() {
    let w = world();
    let (a, b) = (w.call(|_| {}), w.call(|_| {}));
    let runner = PureRunner::new(false);
    let q = queue_of(&w, &runner);
    q.enqueue(&a);
    wait_for("A to start", || runner.started().len() == 1);
    q.enqueue(&b);
    let before = (meta_bytes(&a), meta_bytes(&b));
    // Again, by enqueue and by "Transcribe again", while running and while queued.
    q.enqueue(&a);
    q.enqueue(&b);
    assert_eq!(q.transcribe(&a), Ok(()));
    assert_eq!(q.transcribe(&b), Ok(()));
    assert_eq!(before, (meta_bytes(&a), meta_bytes(&b)), "a no-op");
    runner.gate.open();
    q.wait_idle();
    assert_eq!(runner.started(), vec![name_of(&a), name_of(&b)]);
}

#[test]
fn cancelling_a_queued_call_removes_it_without_running_it() {
    let w = world();
    let (a, b, c) = (w.call(|_| {}), w.call(|_| {}), w.call(|_| {}));
    let runner = PureRunner::new(false);
    let q = queue_of(&w, &runner);
    q.enqueue(&a);
    wait_for("A to start", || runner.started().len() == 1);
    q.enqueue(&b);
    q.enqueue(&c);
    q.cancel(&b);
    assert_eq!(q.position(&b), QueuePosition::Absent);
    runner.gate.open();
    q.wait_idle();
    assert_eq!(runner.started(), vec![name_of(&a), name_of(&c)]);
}

// ---- guarantee: recording is allowed during a pass; recordingCall ------------------------------

#[test]
fn a_call_can_be_recorded_and_stopped_during_a_pass_and_then_waits_its_turn() {
    let w = world();
    let a = w.call(|_| {});
    let runner = PureRunner::new(false);
    let q = queue_of(&w, &runner);
    q.enqueue(&a);
    wait_for("A to start", || runner.started().len() == 1);

    // A new call starts, records and stops while A's pass is blocked: nothing here waits for it.
    let files = w.recording();
    let dir = files.dir().to_path_buf();
    assert_eq!(read_meta(&dir).state, CallState::Recording);
    let mut meta = files.meta().clone();
    ok(files.mic.finish());
    ok(files.system.finish());
    meta.state = CallState::ReadyToTranscribe;
    meta.duration_ms = Some(CALL_MS);
    ok(write_meta(&dir, &meta));
    w.store.release(&dir);
    q.enqueue(&dir);
    assert_eq!(q.position(&dir), QueuePosition::Queued);
    assert_eq!(runner.started(), vec![name_of(&a)]);

    runner.gate.open();
    q.wait_idle();
    assert_eq!(runner.started(), vec![name_of(&a), name_of(&dir)]);
}

#[test]
fn transcribe_on_a_recording_call_is_refused_recording_call_and_nothing_changes() {
    let w = world();
    let files = w.recording();
    let dir = files.dir().to_path_buf();
    let before = meta_bytes(&dir);
    let runner = PureRunner::new(true);
    let q = queue_of(&w, &runner);
    let err = match q.transcribe(&dir) {
        Err(e) => e,
        Ok(()) => panic!("a recording call must be refused"),
    };
    assert_eq!(err.code, CallErrorCode::RecordingCall);
    q.wait_idle();
    assert_eq!(meta_bytes(&dir), before, "meta.json is untouched");
    assert_eq!(q.position(&dir), QueuePosition::Absent);
    assert!(runner.started().is_empty(), "no pass was started");
    drop(files);
}

// ---- guarantee: the meta transition of "Transcribe again" --------------------------------------

#[test]
fn transcribe_on_a_ready_call_clears_both_results_and_the_transcript_files_and_keeps_speakers() {
    let w = world();
    let dir = w.call(|_| {});
    let mut meta = read_meta(&dir);
    ready_with_files(&dir, &mut meta);
    ok(write_meta(&dir, &meta));
    let speakers = meta.speakers.clone();

    let runner = PureRunner::new(false);
    let q = queue_of(&w, &runner);
    assert_eq!(q.transcribe(&dir), Ok(()));
    wait_for("the pass to start", || runner.started().len() == 1);
    let meta = read_meta(&dir);
    assert_eq!(meta.state, CallState::ReadyToTranscribe);
    assert_eq!(meta.failure, None);
    assert_eq!(meta.results.mic, TrackResult::Pending);
    assert_eq!(meta.results.system, TrackResult::Pending);
    assert_eq!(meta.speakers, speakers, "speaker names are kept");
    no_transcript(&dir);
    runner.gate.open();
    q.wait_idle();
}

#[test]
fn transcribe_on_a_failed_call_resets_only_the_failed_track_and_keeps_the_done_one() {
    let w = world();
    let dir = w.call(failed_mic_done_system);
    let want = read_meta(&dir);
    let runner = PureRunner::new(false);
    let q = queue_of(&w, &runner);
    assert_eq!(q.transcribe(&dir), Ok(()));
    wait_for("the pass to start", || runner.started().len() == 1);
    let meta = read_meta(&dir);
    assert_eq!(meta.state, CallState::ReadyToTranscribe);
    assert_eq!(meta.failure, None, "the failure is cleared");
    assert_eq!(meta.results.mic, TrackResult::Pending);
    assert_eq!(meta.results.system, want.results.system, "done is kept");
    assert_eq!(meta.speakers, want.speakers, "speaker names are kept");
    runner.gate.open();
    q.wait_idle();
}

#[test]
fn transcribe_on_a_released_recording_meta_recovers_it_in_place_and_queues_it() {
    // Q2: a write failure ended this process's recording (the store released the folder) and
    // left `meta.json` as `recording`: listed Recovered, "Transcribe" offered. It is recovered
    // the way `CallStore::recover` does (duration and end from the tracks), then queued.
    let w = world();
    let files = w.recording();
    let dir = files.dir().to_path_buf();
    w.store.release(&dir);
    let before = read_meta(&dir);
    assert_eq!(before.state, CallState::Recording);
    assert_eq!(before.duration_ms, None);
    assert_eq!(before.ended_at, None);

    let runner = PureRunner::new(false);
    let q = queue_of(&w, &runner);
    assert_eq!(q.transcribe(&dir), Ok(()));
    wait_for("the pass to start", || runner.started().len() == 1);
    let meta = read_meta(&dir);
    assert_eq!(meta.state, CallState::ReadyToTranscribe);
    assert_eq!(meta.failure, None);
    assert!(meta.duration_ms.is_some(), "recovered in place: a duration");
    assert!(meta.ended_at.is_some(), "recovered in place: an end");
    assert_eq!(meta.results.mic, TrackResult::Pending);
    assert_eq!(meta.results.system, TrackResult::Pending);
    runner.gate.open();
    q.wait_idle();
    drop(files);
}

#[test]
fn a_meta_write_that_fails_in_the_transcribe_transition_is_io_failed_and_queues_nothing() {
    // Q4. A directory where the atomic write creates its tmp file makes the write fail.
    let w = world();
    let dir = w.call(failed_mic_done_system);
    let before = meta_bytes(&dir);
    ok(fs::create_dir(dir.join("meta.json.tmp")));
    let runner = PureRunner::new(true);
    let q = queue_of(&w, &runner);
    let err = match q.transcribe(&dir) {
        Err(e) => e,
        Ok(()) => panic!("a meta that cannot be written must be refused"),
    };
    assert_eq!(err.code, CallErrorCode::IoFailed);
    q.wait_idle();
    assert_eq!(meta_bytes(&dir), before, "meta.json is untouched");
    assert_eq!(q.position(&dir), QueuePosition::Absent);
    assert!(runner.started().is_empty(), "no pass was started");
}

#[test]
fn a_transcript_file_that_cannot_be_removed_is_io_failed_and_queues_nothing() {
    // Q4, the second write of a `ready` call's transition: the removal of `transcript.md`.
    let w = world();
    let dir = w.call(|_| {});
    let mut meta = read_meta(&dir);
    ready_with_files(&dir, &mut meta);
    ok(write_meta(&dir, &meta));
    ok(fs::remove_file(dir.join("transcript.md")));
    ok(fs::create_dir(dir.join("transcript.md")));
    ok(fs::write(dir.join("transcript.md").join("held"), "x"));
    let runner = PureRunner::new(true);
    let q = queue_of(&w, &runner);
    let err = match q.transcribe(&dir) {
        Err(e) => e,
        Ok(()) => panic!("a transcript that cannot be removed must be refused"),
    };
    assert_eq!(err.code, CallErrorCode::IoFailed);
    q.wait_idle();
    assert_eq!(q.position(&dir), QueuePosition::Absent);
    assert!(runner.started().is_empty(), "no pass was started");
}

#[test]
fn transcribe_on_a_folder_that_is_not_a_call_is_not_found() {
    let w = world();
    let dir = w._tmp.path().join("calls").join("not-a-call");
    ok(fs::create_dir_all(&dir));
    let runner = PureRunner::new(true);
    let q = queue_of(&w, &runner);
    let err = match q.transcribe(&dir) {
        Err(e) => e,
        Ok(()) => panic!("a folder that is not a call must be refused"),
    };
    assert_eq!(err.code, CallErrorCode::NotFound);

    // A call folder (its name is a call id) that is gone.
    let gone = w.call(|_| {});
    ok(fs::remove_dir_all(&gone));
    let err = match q.transcribe(&gone) {
        Err(e) => e,
        Ok(()) => panic!("a folder that is gone must be refused"),
    };
    assert_eq!(err.code, CallErrorCode::NotFound);
    q.wait_idle();
    assert!(runner.started().is_empty());
}

// ---- guarantee: a redo uploads only what is not done ---------------------------------------------

#[test]
fn a_redo_of_one_track_records_that_passs_engine_and_mode() {
    // Q5: the call's `engines.transcription` and `pass_mode` are those of the latest pass,
    // also when it redid one track only.
    let w = world();
    let dir = w.call(|meta| {
        failed_mic_done_system(meta);
        meta.engines.transcription = Some(EngineRef::new("api", "old-model"));
        meta.pass_mode = Some(PassMode::Timeless);
    });
    let engine = Engine::new(None);
    let runner = TestRunner::new(&engine, CALL_MS);
    let q = PassQueue::new(w.store.clone(), runner.clone());
    assert_eq!(q.transcribe(&dir), Ok(()));
    q.wait_idle();
    let meta = read_meta(&dir);
    assert_eq!(meta.state, CallState::Ready);
    assert_eq!(
        meta.engines.transcription,
        Some(EngineRef::new("api", "m1")),
        "this pass's engine"
    );
    assert_eq!(meta.pass_mode, Some(PassMode::Timed), "this pass's mode");
}

#[test]
fn a_redo_of_a_failed_mic_sends_only_mic_chunks_and_keeps_the_system_lines_and_speaker_names() {
    let w = world();
    let dir = w.call(failed_mic_done_system);
    let speakers = read_meta(&dir).speakers;
    let engine = Engine::new(None);
    let runner = TestRunner::new(&engine, CALL_MS);
    let q = PassQueue::new(w.store.clone(), runner.clone());
    assert_eq!(q.transcribe(&dir), Ok(()));
    q.wait_idle();

    assert!(!engine.sent().is_empty(), "the mic track was uploaded");
    assert!(
        engine.sent().iter().all(|t| *t == Track::Mic),
        "only mic chunks: {:?}",
        engine.sent()
    );
    assert_eq!(
        runner.asked(Track::System),
        0,
        "the done track is not decoded"
    );
    let meta = read_meta(&dir);
    assert_eq!(meta.state, CallState::Ready);
    assert_eq!(meta.failure, None);
    assert_eq!(meta.speakers, speakers);
    assert_eq!(
        meta.results.system,
        TrackResult::Done {
            segments: vec![kept(Track::System, "kept-system-line")]
        },
        "the system result is exactly what it was"
    );
    assert!(matches!(meta.results.mic, TrackResult::Done { .. }));
    let t = match transcript_of(&dir) {
        Some(t) => t,
        None => panic!("transcript.json is written"),
    };
    ok(t.validate());
    assert_eq!(
        texts(&t),
        vec![
            ("mic".to_string(), "mic-line".to_string()),
            ("system".to_string(), "kept-system-line".to_string())
        ],
        "the new mic line and the kept system line (mic 0.5 s, system 1.0 s)"
    );
    assert!(
        t.segments
            .iter()
            .any(|s| *s == kept(Track::System, "kept-system-line")),
        "the kept system line is unchanged"
    );
}

#[test]
fn a_redo_of_a_ready_call_clears_it_and_sends_both_tracks() {
    let w = world();
    let dir = w.call(|_| {});
    // Make it a ready call with files; the closure form cannot see the folder.
    let mut meta = read_meta(&dir);
    ready_with_files(&dir, &mut meta);
    ok(write_meta(&dir, &meta));
    let speakers = meta.speakers.clone();

    let engine = Engine::new(None);
    let runner = TestRunner::new(&engine, CALL_MS);
    let q = PassQueue::new(w.store.clone(), runner.clone());
    assert_eq!(q.transcribe(&dir), Ok(()));
    q.wait_idle();

    let sent = engine.sent();
    assert!(
        sent.contains(&Track::Mic) && sent.contains(&Track::System),
        "{sent:?}"
    );
    let meta = read_meta(&dir);
    assert_eq!(meta.state, CallState::Ready);
    assert_eq!(meta.speakers, speakers, "speaker names are kept");
    let t = match transcript_of(&dir) {
        Some(t) => t,
        None => panic!("transcript.json is written"),
    };
    ok(t.validate());
    let got = texts(&t);
    assert!(
        got.contains(&("mic".to_string(), "mic-line".to_string())),
        "{got:?}"
    );
    assert!(
        got.contains(&("system".to_string(), "system-line".to_string())),
        "{got:?}"
    );
    assert!(
        got.iter().all(|(_, text)| !text.starts_with("old-")),
        "nothing of the old transcript is kept: {got:?}"
    );
}

#[test]
fn a_pass_takes_its_track_set_from_meta_json_when_it_starts() {
    // `enqueue` carries no track set: the stop path and the start-up queue only name the folder.
    let w = world();
    let dir = w.call(failed_mic_done_system);
    let mut meta = read_meta(&dir);
    meta.state = CallState::ReadyToTranscribe;
    meta.failure = None;
    meta.results.mic = TrackResult::Pending;
    ok(write_meta(&dir, &meta));
    let engine = Engine::new(None);
    let runner = TestRunner::new(&engine, CALL_MS);
    let q = PassQueue::new(w.store.clone(), runner.clone());
    q.enqueue(&dir);
    q.wait_idle();
    assert!(!engine.sent().is_empty());
    assert!(
        engine.sent().iter().all(|t| *t == Track::Mic),
        "{:?}",
        engine.sent()
    );
    assert_eq!(runner.asked(Track::System), 0);
    assert_eq!(read_meta(&dir).state, CallState::Ready);
}

// ---- guarantee: cancel -------------------------------------------------------------------------

#[test]
fn cancel_during_a_blocked_chunk_sends_nothing_more_writes_no_transcript_and_returns_after_the_pass(
) {
    let w = world();
    let dir = w.call(|meta| meta.duration_ms = Some(600_000));
    let gate = Gate::new(false);
    let engine = Engine::new(Some(gate.clone()));
    // Ten minutes per track: far more chunks than the three that fit in flight.
    let runner = TestRunner::new(&engine, 600_000);
    let q = Arc::new(PassQueue::new(w.store.clone(), runner.clone()));
    q.enqueue(&dir);
    wait_for("three requests in flight", || {
        engine.entered.load(Ordering::SeqCst) == 3
    });

    let returned_after_pass_end = Arc::new(AtomicUsize::new(usize::MAX));
    let canceller = {
        let (q, runner, dir, flag) = (
            q.clone(),
            runner.clone(),
            dir.clone(),
            returned_after_pass_end.clone(),
        );
        thread::spawn(move || {
            q.cancel(&dir);
            flag.store(runner.ended.load(Ordering::SeqCst), Ordering::SeqCst);
        })
    };
    // The token is set while the requests are still blocked; only then they are answered.
    wait_for("the cancel token", || runner.cancelled());
    gate.open();
    ok(canceller.join().map_err(|_| "cancel thread panicked"));

    assert_eq!(
        returned_after_pass_end.load(Ordering::SeqCst),
        1,
        "cancel returned only after the pass ended"
    );
    assert_eq!(engine.sent().len(), 3, "no request after the token was set");
    no_transcript(&dir);
    let meta = read_meta(&dir);
    assert_eq!(
        meta.state,
        CallState::ReadyToTranscribe,
        "back to queued-able"
    );
    assert_eq!(meta.results.mic, TrackResult::Pending);
    assert_eq!(meta.results.system, TrackResult::Pending);
    assert!(lock(&runner.rec.logs).iter().any(|e| matches!(
        e,
        LogEvent::FinalPassFinished {
            outcome: FinalOutcome::Cancelled,
            ..
        }
    )));
    assert!(
        lock(&runner.rec.notices).is_empty(),
        "a cancel sends no notice"
    );
    assert_eq!(q.position(&dir), QueuePosition::Absent);
}

// ---- guarantee: restart after exit or crash ---------------------------------------------------------

struct Wall;

impl Clock for Wall {
    fn now(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }
}

struct Utc;

impl LocalOffset for Utc {
    fn seconds_east(&self, _t: SystemTime) -> i32 {
        0
    }
}

fn open_log(dir: &Path) -> Log {
    Log::open(
        dir.to_path_buf(),
        Arc::new(Wall),
        Arc::new(Utc),
        LogConfig::default(),
        Box::new(|_| {}),
    )
}

#[test]
fn a_redo_cut_off_mid_pass_restarts_at_the_next_start_for_the_same_track_only() {
    let w = world();
    // What a redo of the mic track leaves on disk while its pass runs: `transcribing`, the
    // system result still done, the mic result pending. The process then exits.
    let dir = w.call(|meta| {
        failed_mic_done_system(meta);
        meta.state = CallState::Transcribing;
        meta.failure = None;
        meta.results.mic = TrackResult::Pending;
    });
    let logs = TempDir::new();
    let log = open_log(logs.path());
    let recovery = w.store.recover(&defaults(None).calls, &log);
    assert_eq!(
        recovery.queue.len(),
        1,
        "the interrupted call is queued again"
    );

    let engine = Engine::new(None);
    let runner = TestRunner::new(&engine, CALL_MS);
    let q = PassQueue::new(w.store.clone(), runner.clone());
    for entry in &recovery.queue {
        q.enqueue(&entry.root.join(entry.id.as_str()));
    }
    q.wait_idle();

    assert!(!engine.sent().is_empty());
    assert!(
        engine.sent().iter().all(|t| *t == Track::Mic),
        "{:?}",
        engine.sent()
    );
    assert_eq!(runner.asked(Track::System), 0);
    let meta = read_meta(&dir);
    assert_eq!(meta.state, CallState::Ready);
    assert_eq!(
        meta.results.system,
        TrackResult::Done {
            segments: vec![kept(Track::System, "kept-system-line")]
        }
    );
}

#[test]
fn the_calls_of_the_start_up_queue_run_oldest_first() {
    let w = world();
    let older = w.call(|meta| meta.state = CallState::Transcribing);
    let newer = w.call(|_| {});
    let logs = TempDir::new();
    let log = open_log(logs.path());
    let recovery = w.store.recover(&defaults(None).calls, &log);
    let runner = PureRunner::new(true);
    let q = queue_of(&w, &runner);
    for entry in &recovery.queue {
        q.enqueue(&entry.root.join(entry.id.as_str()));
    }
    q.wait_idle();
    assert_eq!(runner.started(), vec![name_of(&older), name_of(&newer)]);
}
