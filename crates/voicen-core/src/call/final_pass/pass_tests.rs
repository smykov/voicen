//! T-138: the pass runner (spec 007 FR-023..FR-027, research R9) against fakes, no HTTP.
//!
//! Audio here is *marked*: millisecond `m` of a track is 16 samples `[m % 30000, m / 30000,
//! track flag, 0, ..]`, so a fake engine reads from the request audio which track and which
//! track-relative range it got, and checks that the audio is contiguous (a slice cut wrong, a
//! carry lost or a window dropped fails there). `MarkedSpans` is a primary detector that answers
//! from per-track speech intervals and the markers of the window it is handed.
//!
//! No test calls `Instant::now` (banned outside `crate::clock`); the pass clock is a
//! stepping fake. Error fakes fail exactly one call (F-024).
//!
//! The tests that need HTTP (a wiremock server, the real key store path, real Opus tracks) are
//! in `tests/final_pass_http.rs`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, UNIX_EPOCH};

use super::*;
use crate::call::codec::TrackEncoder;
use crate::call::storage::{write_meta, CallStore, Failure, NewCall, TrackResult};
use crate::call::transcript::render_markdown;
use crate::call::{
    CallState, DecodeEnd, EndpointId, EngineSegment, FailReason, FinalStage, InvalidRule,
    SegmentV1, StorageKind, Track, TranscriptV1, VadFallbackCause,
};
use crate::clock::SystemMonotonic;
use crate::i18n::UiLanguage;
use crate::settings::roots::FsCallRoots;
use crate::settings::{defaults, LangCode};
use crate::test_support::fixtures::speech_3s;
use crate::test_support::spans::ScriptedSpans;
use crate::test_support::TempDir;
use crate::timeouts::Timeouts;
use crate::vad::energy::EnergyDetector;
use crate::vad::VadError;

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

fn some<T>(o: Option<T>, what: &str) -> T {
    match o {
        Some(v) => v,
        None => panic!("missing: {what}"),
    }
}

/// The stepping pass clock: every read is one second later than the one before.
type BeforeHook = Box<dyn FnMut(&Req) + Send>;
type PrepHook = Box<dyn FnOnce(&mut CallMeta, &Path)>;

struct StepClock {
    base: Instant,
    ticks: AtomicU64,
}

impl MonotonicClock for StepClock {
    fn now(&self) -> Instant {
        self.base + Duration::from_millis(1_000 * self.ticks.fetch_add(1, Ordering::SeqCst))
    }
}

fn step_clock() -> Arc<StepClock> {
    Arc::new(StepClock {
        base: SystemMonotonic.now(),
        ticks: AtomicU64::new(0),
    })
}

/// The events sink of a pass.
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

// ---- marked audio -------------------------------------------------------------------

const MARK_UNIT: i64 = 30_000;

/// `ms` milliseconds of marked audio of the track with `flag` (0 mic, 1 system).
fn marked(flag: i16, ms: u64) -> Vec<i16> {
    let mut out = Vec::with_capacity(usize::try_from(ms).unwrap_or(0) * 16);
    for m in 0..i64::try_from(ms).unwrap_or(0) {
        let lo = (m % MARK_UNIT) as i16;
        let hi = (m / MARK_UNIT) as i16;
        out.extend_from_slice(&[lo, hi, flag, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }
    out
}

/// The track, the first track ms and the length in ms of marked `audio`; panics when the
/// audio is not whole milliseconds, not contiguous or mixes tracks.
fn marks(audio: &AudioBuffer) -> (Track, u64, u64) {
    let samples = audio.samples();
    assert!(
        !samples.is_empty() && samples.len().is_multiple_of(16),
        "audio is not whole milliseconds"
    );
    let (mut start, mut flag, mut count) = (0i64, 0i64, 0usize);
    for (i, ms) in samples.as_chunks::<16>().0.iter().enumerate() {
        let v = |k: usize| i64::from(ms.get(k).copied().unwrap_or(-1));
        let m = v(1) * MARK_UNIT + v(0);
        if i == 0 {
            start = m;
            flag = v(2);
        }
        assert_eq!(m, start + i as i64, "audio is not contiguous at ms {i}");
        assert_eq!(v(2), flag, "audio mixes tracks at ms {i}");
        count = i + 1;
    }
    let track = if flag == 1 { Track::System } else { Track::Mic };
    (track, start as u64, count as u64)
}

/// A primary detector answering from per-track speech intervals (track ms).
struct MarkedSpans {
    mic: Vec<(u64, u64)>,
    system: Vec<(u64, u64)>,
}

impl SpeechSpans for MarkedSpans {
    fn name(&self) -> &'static str {
        "marked"
    }

    fn speech_spans(&self, audio: &AudioBuffer) -> Result<Vec<SpeechSpan>, VadError> {
        let (track, start, len) = marks(audio);
        let own = if track == Track::System {
            &self.system
        } else {
            &self.mic
        };
        Ok(own
            .iter()
            .filter_map(|&(a, b)| {
                let (a, b) = (a.max(start), b.min(start + len));
                (a < b).then(|| SpeechSpan {
                    start_ms: a - start,
                    end_ms: b - start,
                })
            })
            .collect())
    }
}

/// Speech at [1 s, 100 s), [130 s, 230 s) and [260 s, 340 s) of a 400 s track: with 120 s
/// windows the timed chunks are about [0.8, 100.2), [129.8, 230.2) and [259.8, 340.2) s.
const SPEECH: [(u64, u64); 3] = [(1_000, 100_000), (130_000, 230_000), (260_000, 340_000)];

fn marked_primary(spans: &[(u64, u64)]) -> Option<Arc<dyn SpeechSpans>> {
    Some(Arc::new(MarkedSpans {
        mic: spans.to_vec(),
        system: spans.to_vec(),
    }))
}

// ---- the decoder fake ----------------------------------------------------------------

/// One track as the decoder hands it out.
struct Src {
    samples: Vec<i16>,
    /// A window is never longer than this (a ragged decoder), whole ms.
    cap_ms: Option<u32>,
    /// What the decoder says after the data (`Ok(None)` when `None`).
    end: Option<DecodeEnd>,
}

impl Src {
    fn marked(flag: i16, ms: u64) -> Src {
        Src {
            samples: marked(flag, ms),
            cap_ms: None,
            end: None,
        }
    }

    /// `secs` seconds of the speech fixture, repeated.
    fn speech(secs: usize) -> Src {
        let one = speech_3s();
        Src {
            samples: one
                .samples()
                .iter()
                .copied()
                .cycle()
                .take(secs * 16_000)
                .collect(),
            cap_ms: None,
            end: None,
        }
    }

    fn cap(mut self, ms: u32) -> Src {
        self.cap_ms = Some(ms);
        self
    }

    fn ending(mut self, end: DecodeEnd) -> Src {
        self.end = Some(end);
        self
    }
}

struct GenDecoder {
    src: Src,
    pos: usize,
    asks: Arc<Mutex<Vec<u32>>>,
}

impl TrackDecoder for GenDecoder {
    fn next_window(&mut self, max_ms: u32) -> Result<Option<AudioBuffer>, DecodeEnd> {
        lock(&self.asks).push(max_ms);
        if self.pos >= self.src.samples.len() {
            return match self.src.end {
                Some(e) => Err(e),
                None => Ok(None),
            };
        }
        let want = max_ms.min(self.src.cap_ms.unwrap_or(u32::MAX)) as usize * 16;
        let to = self.pos.saturating_add(want).min(self.src.samples.len());
        let out = self
            .src
            .samples
            .get(self.pos..to)
            .unwrap_or_default()
            .to_vec();
        self.pos = to;
        Ok(Some(AudioBuffer::from_16k_mono(out)))
    }
}

// ---- the engine fake -----------------------------------------------------------------

#[derive(Debug, Clone)]
struct Req {
    /// Arrival number.
    n: usize,
    shape: ResponseShape,
    track: Track,
    /// Track ms of the first sample (0 when the audio is not marked).
    start: u64,
    len: u64,
    language: Option<String>,
}

fn label(r: &Req) -> String {
    format!("{}@{}", r.track.as_str(), r.start)
}

type Answer = Box<dyn Fn(&Req) -> Result<Timed, TimedFailure> + Send + Sync>;

/// Releases requests in groups: the last of a group of `size` arrivals frees the others, so a
/// group is released only when `size` requests are in flight at once. A group that never fills
/// is released by the timeout.
struct Gate {
    size: usize,
    released: Mutex<usize>,
    cv: Condvar,
}

impl Gate {
    fn pass(&self, n: usize) {
        let group = n / self.size;
        let mut released = lock(&self.released);
        if (n + 1).is_multiple_of(self.size) {
            *released = group + 1;
            self.cv.notify_all();
            return;
        }
        let _ = self
            .cv
            .wait_timeout_while(released, Duration::from_millis(1_500), |r| *r <= group)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

struct FakeEngine {
    answer: Answer,
    marked: bool,
    log: Mutex<Vec<Req>>,
    inflight: AtomicUsize,
    peak: AtomicUsize,
    gate: Option<Gate>,
    before: Mutex<Option<BeforeHook>>,
    /// The call folder to watch: names other than the pass's own files are recorded.
    watch: Option<PathBuf>,
    strays: Mutex<Vec<String>>,
}

impl FakeEngine {
    fn new(
        marked: bool,
        answer: impl Fn(&Req) -> Result<Timed, TimedFailure> + Send + Sync + 'static,
    ) -> FakeEngine {
        FakeEngine {
            answer: Box::new(answer),
            marked,
            log: Mutex::new(Vec::new()),
            inflight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            gate: None,
            before: Mutex::new(None),
            watch: None,
            strays: Mutex::new(Vec::new()),
        }
    }

    fn gated(mut self, size: usize) -> FakeEngine {
        self.gate = Some(Gate {
            size,
            released: Mutex::new(0),
            cv: Condvar::new(),
        });
        self
    }

    fn on_request(self, f: impl FnMut(&Req) + Send + 'static) -> FakeEngine {
        *lock(&self.before) = Some(Box::new(f));
        self
    }

    fn log(&self) -> Vec<Req> {
        lock(&self.log).clone()
    }
}

impl TimedTranscriber for FakeEngine {
    fn transcribe_timed(
        &self,
        audio: &AudioBuffer,
        req: &TranscribeRequest,
        shape: ResponseShape,
    ) -> Result<Timed, TimedFailure> {
        let (track, start, len) = if self.marked {
            marks(audio)
        } else {
            (Track::Mic, 0, (audio.samples().len() / 16) as u64)
        };
        let r = {
            let mut log = lock(&self.log);
            let r = Req {
                n: log.len(),
                shape,
                track,
                start,
                len,
                language: req.language.clone(),
            };
            log.push(r.clone());
            r
        };
        let now = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        if let Some(dir) = &self.watch {
            for entry in ok(fs::read_dir(dir)) {
                let name = ok(entry).file_name().to_string_lossy().into_owned();
                if !["meta.json", "meta.json.tmp", "mic.opus", "system.opus"]
                    .contains(&name.as_str())
                {
                    lock(&self.strays).push(name);
                }
            }
        }
        if let Some(gate) = &self.gate {
            gate.pass(r.n);
        }
        if let Some(f) = lock(&self.before).as_mut() {
            f(&r);
        }
        let out = (self.answer)(&r);
        self.inflight.fetch_sub(1, Ordering::SeqCst);
        out
    }
}

fn timed(segments: Vec<EngineSegment>) -> Timed {
    Timed {
        text: String::new(),
        language: None,
        segments: Some(segments),
    }
}

fn seg(from_s: f64, to_s: f64, text: &str, lang: Option<&str>) -> EngineSegment {
    EngineSegment {
        start_s: from_s,
        end_s: to_s,
        text: text.to_string(),
        lang: lang.and_then(LangCode::parse),
    }
}

/// The good server: one segment `[1 s, 2 s)` labelled with the chunk for a `Segments` request,
/// a `de` plain text `p<start>` for a `Plain` one.
fn answer_ok(r: &Req) -> Result<Timed, TimedFailure> {
    Ok(match r.shape {
        ResponseShape::Segments => timed(vec![seg(1.0, 2.0, &label(r), None)]),
        ResponseShape::Plain => Timed {
            text: format!("p{}", r.start),
            language: LangCode::parse("de"),
            segments: None,
        },
    })
}

// ---- the run harness -----------------------------------------------------------------

struct Opts {
    /// The system track's `start_offset_ms`.
    offset: u64,
    /// `meta.duration_ms`; the longer track end when `None`.
    duration: Option<u64>,
    primary: Option<Arc<dyn SpeechSpans>>,
    language: Option<String>,
    /// Edits the meta before it is written, and prepares the folder.
    prep: Option<PrepHook>,
}

impl Opts {
    fn new(primary: Option<Arc<dyn SpeechSpans>>) -> Opts {
        Opts {
            offset: 0,
            duration: None,
            primary,
            language: None,
            prep: None,
        }
    }

    fn offset(mut self, ms: u64) -> Opts {
        self.offset = ms;
        self
    }

    fn language(mut self, code: &str) -> Opts {
        self.language = Some(code.to_string());
        self
    }

    fn prep(mut self, f: impl FnOnce(&mut CallMeta, &Path) + 'static) -> Opts {
        self.prep = Some(Box::new(f));
        self
    }
}

struct Out {
    out: PassOutcome,
    rec: Arc<Rec>,
    engine: Arc<FakeEngine>,
    dir: PathBuf,
    asks: [Arc<Mutex<Vec<u32>>>; 2],
    _tmp: TempDir,
}

impl Out {
    fn log(&self) -> Vec<Req> {
        self.engine.log()
    }

    fn logs(&self) -> Vec<LogEvent> {
        lock(&self.rec.logs).clone()
    }

    fn notices(&self) -> Vec<CallNotice> {
        lock(&self.rec.notices).clone()
    }

    fn meta(&self) -> CallMeta {
        ok(serde_json::from_slice(&ok(fs::read(
            self.dir.join("meta.json"),
        ))))
    }

    fn transcript(&self) -> Option<TranscriptV1> {
        let text = fs::read_to_string(self.dir.join("transcript.json")).ok()?;
        Some(ok(TranscriptV1::from_json(&text)))
    }

    fn names(&self) -> BTreeSet<String> {
        ok(fs::read_dir(&self.dir))
            .map(|e| ok(e).file_name().to_string_lossy().into_owned())
            .collect()
    }

    fn finished(&self) -> Vec<(FinalOutcome, PassMode, u64)> {
        self.logs()
            .into_iter()
            .filter_map(|e| match e {
                LogEvent::FinalPassFinished { outcome, mode, ms } => Some((outcome, mode, ms)),
                _ => None,
            })
            .collect()
    }

    fn fallbacks(&self) -> Vec<(Track, VadFallbackCause)> {
        self.logs()
            .into_iter()
            .filter_map(|e| match e {
                LogEvent::VadFallback { track, cause } => Some((track, cause)),
                _ => None,
            })
            .collect()
    }

    fn stage_chunks(&self, track: Track, stage: FinalStage) -> Vec<u32> {
        self.logs()
            .into_iter()
            .filter_map(|e| match e {
                LogEvent::FinalPassStage {
                    track: t,
                    stage: s,
                    chunks,
                    ..
                } if t == track && s == stage => Some(chunks),
                _ => None,
            })
            .collect()
    }
}

/// Creates a call (real folder, `meta.json` `ready-to-transcribe`), runs the pass on the two
/// sources with the fake engine and returns everything the tests look at.
fn run_with(mic: Src, system: Src, engine: FakeEngine, opts: Opts) -> Out {
    let tmp = TempDir::new();
    let store = CallStore::new(
        Arc::new(FsCallRoots::new(tmp.path().join("calls"))),
        Arc::new(SystemMonotonic),
    );
    let new = NewCall {
        started: UNIX_EPOCH + Duration::from_secs(1_800_000_000),
        seconds_east: 0,
        mic: EndpointId::new("mic-1"),
        system: EndpointId::new("sys-1"),
        lang: UiLanguage::En,
        app_version: "0.0.0".to_string(),
    };
    let files = ok(store.create(&defaults(None).calls, &new));
    let dir = files.dir().to_path_buf();
    let mut meta = files.meta().clone();
    ok(files.mic.finish());
    ok(files.system.finish());

    let mic_ms = (mic.samples.len() / 16) as u64;
    let system_ms = (system.samples.len() / 16) as u64;
    meta.state = CallState::ReadyToTranscribe;
    meta.duration_ms = Some(opts.duration.unwrap_or(mic_ms.max(opts.offset + system_ms)));
    if let Some(track) = meta.tracks.get_mut(1) {
        track.start_offset_ms = opts.offset;
    }
    if let Some(prep) = opts.prep {
        prep(&mut meta, &dir);
    }
    ok(write_meta(&dir, &meta));

    let mut engine = engine;
    if engine.watch.is_none() {
        engine.watch = Some(dir.clone());
    }
    let engine = Arc::new(engine);
    let rec = Arc::new(Rec::default());
    let asks = [
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    ];
    let sources = PassSources {
        mic: Box::new(GenDecoder {
            src: mic,
            pos: 0,
            asks: asks[0].clone(),
        }),
        system: Box::new(GenDecoder {
            src: system,
            pos: 0,
            asks: asks[1].clone(),
        }),
    };
    let request = TranscribeRequest {
        language: opts.language,
        timeouts: Timeouts::from_settings(&defaults(None).timeouts),
    };
    let deps = PassDeps {
        primary: opts.primary,
        events: &*rec,
        clock: step_clock(),
    };
    let out = run_pass_with(
        PassCall {
            dir: dir.clone(),
            meta,
        },
        &*engine,
        &request,
        EngineRef::new("api", "m1"),
        sources,
        deps,
    );
    Out {
        out,
        rec,
        engine,
        dir,
        asks,
        _tmp: tmp,
    }
}

/// `(start_ms, end_ms, track, speaker, text, lang)` of every line.
type Line = (i64, i64, String, String, String, Option<String>);

fn lines(t: &TranscriptV1) -> Vec<Line> {
    t.segments
        .iter()
        .map(|s| {
            (
                s.start_ms,
                s.end_ms,
                s.track.clone(),
                s.speaker.clone(),
                s.text.clone(),
                s.lang.clone(),
            )
        })
        .collect()
}

fn line(start: u64, end: u64, track: Track, text: &str, lang: Option<&str>) -> Line {
    let speaker = if track == Track::System { "s1" } else { "me" };
    (
        start as i64,
        end as i64,
        track.as_str().to_string(),
        speaker.to_string(),
        text.to_string(),
        lang.map(str::to_string),
    )
}

fn sorted(mut v: Vec<Line>) -> Vec<Line> {
    v.sort_by_key(|l| (l.0, l.2 == "system"));
    v
}

/// Every speech millisecond (sampled each 100 ms) is in exactly one request.
fn assert_covered_once(reqs: &[&Req], speech: &[(u64, u64)]) {
    for &(a, b) in speech {
        let mut t = a;
        while t < b {
            let n = reqs
                .iter()
                .filter(|r| r.start <= t && t < r.start + r.len)
                .count();
            assert_eq!(n, 1, "speech ms {t} is in {n} requests");
            t += 100;
        }
    }
}

/// The all-good outcome of the 400 s pair: ready, timed, whole files, one request per chunk.
fn assert_ready_timed(out: &Out, offset: u64) {
    assert_eq!(
        out.out,
        PassOutcome {
            outcome: FinalOutcome::Ready,
            mode: PassMode::Timed
        }
    );
    let meta = out.meta();
    assert_eq!(meta.state, CallState::Ready);
    assert_eq!(meta.failure, None);
    assert_eq!(meta.pass_mode, Some(PassMode::Timed));
    assert_eq!(
        meta.engines.transcription,
        Some(EngineRef::new("api", "m1"))
    );
    assert_eq!(
        out.names(),
        [
            "meta.json",
            "mic.opus",
            "system.opus",
            "transcript.json",
            "transcript.md"
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<BTreeSet<_>>(),
        "the call folder holds exactly the call's files"
    );
    let t = some(out.transcript(), "transcript.json");
    ok(t.validate());
    assert_eq!(
        ok(fs::read_to_string(out.dir.join("transcript.md"))),
        render_markdown(&t, &meta.title, &meta.speakers)
    );
    for (name, res) in [("mic", &meta.results.mic), ("system", &meta.results.system)] {
        let want: Vec<SegmentV1> = t
            .segments
            .iter()
            .filter(|s| s.track == name)
            .cloned()
            .collect();
        assert_eq!(res, &TrackResult::Done { segments: want }, "{name} result");
    }
    // One line per request: `[1 s, 2 s)` into the chunk, on the call timeline.
    let log = out.log();
    let want = sorted(
        log.iter()
            .map(|r| {
                let at = r.start + 1_000 + if r.track == Track::System { offset } else { 0 };
                line(at, at + 1_000, r.track, &label(r), None)
            })
            .collect(),
    );
    assert_eq!(lines(&t), want);

    let logs = out.logs();
    assert_eq!(out.finished().len(), 1, "one final_finished");
    let (outcome, mode, ms) = some(out.finished().first().copied(), "final_finished");
    assert_eq!((outcome, mode), (FinalOutcome::Ready, PassMode::Timed));
    assert!(
        ms >= 1_000,
        "final_finished.ms {ms} is not from the pass clock"
    );
    for track in [Track::Mic, Track::System] {
        for stage in [
            FinalStage::Decode,
            FinalStage::Vad,
            FinalStage::Upload,
            FinalStage::Merge,
            FinalStage::Write,
        ] {
            assert_eq!(
                out.stage_chunks(track, stage).len(),
                1,
                "one final_stage {stage:?} for {track:?}"
            );
        }
        assert_eq!(out.stage_chunks(track, FinalStage::Upload), vec![3]);
    }
    assert!(
        out.fallbacks().is_empty(),
        "a working primary logs no fallback"
    );
    assert!(!logs.iter().any(|e| matches!(
        e,
        LogEvent::TranscriptInvalid { .. } | LogEvent::CallStorageFailed { .. }
    )));
    assert_eq!(
        out.notices(),
        vec![CallNotice::TranscriptReady { call: meta.id }]
    );
    assert!(
        lock(&out.engine.strays).is_empty(),
        "stray files in the call folder during the pass: {:?}",
        lock(&out.engine.strays)
    );
}

// ---- characterization (green today, must stay green) ------------------------------------

#[test]
fn characterization_energy_detector_finds_the_speech_fixture() {
    // The VAD fallback tests rely on this: the energy rule finds speech in the fixture.
    let spans = ok(EnergyDetector::new().speech_spans(&speech_3s()));
    assert!(!spans.is_empty(), "energy spans of the speech fixture");
}

// ---- scheduling ---------------------------------------------------------------------------

/// Two 400 s tracks, three timed chunks each, six requests held in groups of three.
fn scheduled(offset: u64) -> Out {
    let engine = FakeEngine::new(true, answer_ok).gated(3);
    let out = run_with(
        Src::marked(0, 400_000),
        Src::marked(1, 400_000),
        engine,
        Opts::new(marked_primary(&SPEECH)).offset(offset),
    );
    assert_ready_timed(&out, offset);
    assert_eq!(out.engine.peak.load(Ordering::SeqCst), 3, "peak in flight");
    let log = out.log();
    assert_eq!(log.len(), 6, "one request per chunk");
    assert!(log.iter().all(|r| r.shape == ResponseShape::Segments));
    let mut order: Vec<&Req> = log.iter().collect();
    order.sort_by_key(|r| {
        let system = r.track == Track::System;
        (r.start + if system { offset } else { 0 }, system)
    });
    for (g, want) in order.chunks(3).enumerate() {
        let want: BTreeSet<(bool, u64)> = want
            .iter()
            .map(|r| (r.track == Track::System, r.start))
            .collect();
        let got: BTreeSet<(bool, u64)> = log
            .iter()
            .skip(g * 3)
            .take(3)
            .map(|r| (r.track == Track::System, r.start))
            .collect();
        assert_eq!(
            got,
            want,
            "requests {}..{} in call-time order",
            g * 3,
            g * 3 + 3
        );
    }
    out
}

#[test]
fn three_requests_in_flight_from_both_tracks_in_call_time_order_with_mic_first_on_a_tie() {
    // Both tracks start together: the first three are mic 0, system 0 and mic 1 (mic first on
    // a tie), the next three system 1, mic 2, system 2.
    scheduled(0);
}

#[test]
fn dispatch_order_uses_the_system_start_offset() {
    // The system track starts 300 s into the call: all three mic chunks go first.
    scheduled(300_000);
}

// ---- decoding: one window at a time --------------------------------------------------------

#[test]
fn decoding_asks_one_window_at_a_time_and_keeps_audio_whole() {
    let out = run_with(
        Src::marked(0, 400_000),
        Src::marked(1, 0),
        FakeEngine::new(true, answer_ok),
        Opts::new(marked_primary(&SPEECH)),
    );
    assert_eq!(out.out.outcome, FinalOutcome::Ready);
    let asks = lock(&out.asks[0]).clone();
    assert!(
        asks.len() >= 4,
        "a 400 s track is read in windows: {asks:?}"
    );
    assert!(
        asks.iter().all(|&a| (1..=120_000).contains(&a)),
        "windows over TIMED_MAX_MS: {asks:?}"
    );
    assert!(
        lock(&out.engine.strays).is_empty(),
        "stray files in the call folder: {:?}",
        lock(&out.engine.strays)
    );
}

#[test]
fn a_ragged_decoder_still_gives_whole_chunks() {
    // Windows shorter than asked (7.777 s): the chunk audio still equals the source (the fake
    // engine checks contiguity) and every speech ms goes up once.
    let speech = [(1_000u64, 200_000u64)];
    let out = run_with(
        Src::marked(0, 250_000).cap(7_777),
        Src::marked(1, 0),
        FakeEngine::new(true, answer_ok),
        Opts::new(marked_primary(&speech)),
    );
    assert_eq!(out.out.outcome, FinalOutcome::Ready);
    let log = out.log();
    assert!(log.len() >= 2, "a 200 s speech is more than one chunk");
    assert_covered_once(&log.iter().collect::<Vec<_>>(), &speech);
    assert!(lock(&out.asks[0]).iter().all(|&a| a <= 120_000));
}

// ---- time-less fallback --------------------------------------------------------------------

#[test]
fn a_400_on_chunk_k_makes_chunk_k_and_every_later_chunk_timeless() {
    // Chunk 1 (the one starting in [100 s, 200 s)) answers "no times"; chunk 2 would answer
    // fine (a flaky server): it is still sent as plain pieces. Chunk 0 keeps its times.
    let engine = FakeEngine::new(true, |r| {
        let k = r.shape == ResponseShape::Segments && (100_000..200_000).contains(&r.start);
        if k {
            Err(TimedFailure::Timeless)
        } else {
            answer_ok(r)
        }
    });
    let out = run_with(
        Src::marked(0, 400_000),
        Src::marked(1, 0),
        engine,
        Opts::new(marked_primary(&SPEECH)),
    );
    assert_eq!(
        out.out,
        PassOutcome {
            outcome: FinalOutcome::Ready,
            mode: PassMode::Timeless
        }
    );
    assert_eq!(out.meta().pass_mode, Some(PassMode::Timeless));
    assert_eq!(out.meta().state, CallState::Ready);
    assert_eq!(out.finished().len(), 1);
    assert_eq!(
        some(out.finished().first().copied(), "final_finished").1,
        PassMode::Timeless
    );

    let log = out.log();
    let plain: Vec<&Req> = log
        .iter()
        .filter(|r| r.shape == ResponseShape::Plain)
        .collect();
    assert!(!plain.is_empty(), "later chunks go as plain requests");
    assert!(plain.iter().all(|r| r.len <= TIMELESS_MAX_MS), "{plain:?}");
    assert!(
        plain.iter().all(|r| r.start >= 100_000),
        "chunk 0 is not re-sent: {plain:?}"
    );
    assert_covered_once(&plain, &SPEECH[1..]);

    // Lines: chunk 0's own segment, and one line spanning each plain piece.
    let t = some(out.transcript(), "transcript.json");
    ok(t.validate());
    let mut want = vec![line(1_800, 2_800, Track::Mic, "mic@800", None)];
    want.extend(plain.iter().map(|r| {
        line(
            r.start,
            r.start + r.len,
            Track::Mic,
            &format!("p{}", r.start),
            Some("de"),
        )
    }));
    assert_eq!(lines(&t), sorted(want));
    assert_eq!(
        out.meta().results.system,
        TrackResult::Done { segments: vec![] },
        "a track without audio is done with no lines"
    );
}

// ---- language -----------------------------------------------------------------------------

#[test]
fn language_is_the_segments_the_requests_or_null() {
    // mic: reply language ru, segment 1 en, segment 2 none -> en, ru. system: no reply
    // language and none on the segment -> null. The request's own setting (de) is only sent.
    let engine = FakeEngine::new(true, |r| {
        Ok(if r.track == Track::Mic {
            Timed {
                text: String::new(),
                language: LangCode::parse("ru"),
                segments: Some(vec![
                    seg(1.0, 2.0, "a", Some("en")),
                    seg(3.0, 4.0, "b", None),
                ]),
            }
        } else {
            timed(vec![seg(2.5, 3.0, "c", None)])
        })
    });
    let speech = [(1_000u64, 5_000u64)];
    let out = run_with(
        Src::marked(0, 10_000),
        Src::marked(1, 10_000),
        engine,
        Opts::new(marked_primary(&speech)).language("de"),
    );
    assert_eq!(out.out.outcome, FinalOutcome::Ready);
    assert!(out
        .log()
        .iter()
        .all(|r| r.language.as_deref() == Some("de")));
    let t = some(out.transcript(), "transcript.json");
    assert_eq!(
        lines(&t),
        vec![
            line(1_800, 2_800, Track::Mic, "a", Some("en")),
            line(3_300, 3_800, Track::System, "c", None),
            line(3_800, 4_800, Track::Mic, "b", Some("ru")),
        ]
    );
}

// ---- the speech detector ----------------------------------------------------------------------

#[test]
fn no_primary_detector_uses_energy_spans_and_logs_the_fallback_per_track() {
    let out = run_with(
        Src::speech(21),
        Src::speech(21),
        FakeEngine::new(false, |_| Ok(timed(vec![seg(0.0, 1.0, "x", None)]))),
        Opts::new(None),
    );
    assert_eq!(out.out.outcome, FinalOutcome::Ready);
    let mut got = out.fallbacks();
    got.sort_by_key(|(t, _)| *t == Track::System);
    assert_eq!(
        got,
        vec![
            (Track::Mic, VadFallbackCause::Unavailable),
            (Track::System, VadFallbackCause::Unavailable)
        ],
        "one vad_fallback per track"
    );
    let log = out.log();
    assert!(log.len() >= 2, "the energy spans of both tracks are sent");
    assert!(log.iter().map(|r| r.len).sum::<u64>() >= 10_000);
}

/// 132 s of speech (two windows) on the mic track, nothing on the system track; the primary
/// answers `first` for the first window and `Ok(vec![])` after that.
fn primary_error_run(first: VadError) -> (Out, Arc<ScriptedSpans>) {
    let primary = Arc::new(ScriptedSpans::new(vec![Err(first)], Ok(Vec::new())));
    let out = run_with(
        Src::speech(132),
        Src::marked(1, 0),
        FakeEngine::new(false, |_| Ok(timed(vec![seg(0.0, 1.0, "x", None)]))),
        Opts::new(Some(primary.clone())),
    );
    (out, primary)
}

#[test]
fn a_primary_error_gives_energy_spans_for_the_window_and_the_rest_of_the_track() {
    let (out, primary) = primary_error_run(VadError::Failed);
    assert_eq!(out.out.outcome, FinalOutcome::Ready);
    assert_eq!(
        out.fallbacks(),
        vec![(Track::Mic, VadFallbackCause::Error)],
        "one vad_fallback for the track"
    );
    assert_eq!(primary.calls(), 1, "the fallback is sticky for the track");
    let sent: u64 = out.log().iter().map(|r| r.len).sum();
    assert!(
        sent >= 60_000,
        "the error is not an empty span list: {sent} ms sent"
    );
}

#[test]
fn an_unavailable_primary_on_a_window_logs_cause_unavailable() {
    let (out, _) = primary_error_run(VadError::Unavailable);
    assert_eq!(out.out.outcome, FinalOutcome::Ready);
    assert_eq!(
        out.fallbacks(),
        vec![(Track::Mic, VadFallbackCause::Unavailable)]
    );
    assert!(!out.log().is_empty());
}

// ---- a track fails --------------------------------------------------------------------------

#[test]
fn a_tracks_first_failure_cancels_its_remaining_chunks_and_the_other_track_finishes() {
    // Mic chunk 1 fails with `key`. The system requests wait for that failure (and then a
    // moment), so the three slots stay taken until the pass has seen it: mic chunk 2 must not
    // be dispatched afterwards; the system track still gets all its chunks.
    let failed = Arc::new(AtomicBool::new(false));
    let flag = failed.clone();
    let engine = FakeEngine::new(true, move |r| {
        if r.track == Track::Mic && (100_000..200_000).contains(&r.start) {
            flag.store(true, Ordering::SeqCst);
            return Err(TimedFailure::Failed(FailReason::Key));
        }
        if r.track == Track::System {
            for _ in 0..300 {
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        answer_ok(r)
    });
    let out = run_with(
        Src::marked(0, 400_000),
        Src::marked(1, 400_000),
        engine,
        Opts::new(marked_primary(&SPEECH)),
    );
    assert!(
        failed.load(Ordering::SeqCst),
        "the scenario reached the failure"
    );
    assert_eq!(
        out.out.outcome,
        FinalOutcome::Failed {
            reason: FailReason::Key
        }
    );
    let log = out.log();
    let mic: Vec<&Req> = log.iter().filter(|r| r.track == Track::Mic).collect();
    assert_eq!(mic.len(), 2, "mic chunks 0 and 1 only: {mic:?}");
    assert!(mic.iter().all(|r| r.start < 200_000), "{mic:?}");
    let system: Vec<&Req> = log.iter().filter(|r| r.track == Track::System).collect();
    assert_eq!(system.len(), 3, "the other track finishes");

    let meta = out.meta();
    assert_eq!(meta.state, CallState::Failed);
    assert_eq!(
        meta.failure,
        Some(Failure {
            reason: FailReason::Key,
            track: Some(Track::Mic)
        })
    );
    assert_eq!(
        meta.results.mic,
        TrackResult::Failed {
            reason: FailReason::Key
        }
    );
    let want: Vec<SegmentV1> = system
        .iter()
        .map(|r| SegmentV1 {
            start_ms: (r.start + 1_000) as i64,
            end_ms: (r.start + 2_000) as i64,
            track: "system".to_string(),
            speaker: "s1".to_string(),
            lang: None,
            text: label(r),
            source: Some("final".to_string()),
        })
        .collect();
    let TrackResult::Done { segments } = &meta.results.system else {
        panic!("system result is {:?}", meta.results.system);
    };
    let mut got = segments.clone();
    got.sort_by_key(|s| s.start_ms);
    let mut want = want;
    want.sort_by_key(|s| s.start_ms);
    assert_eq!(got, want, "the good track keeps its lines in meta.json");
    assert!(
        !out.names().iter().any(|n| n.starts_with("transcript")),
        "no transcript: {:?}",
        out.names()
    );
    assert_eq!(
        out.notices(),
        vec![CallNotice::PassFailed {
            reason: FailReason::Key
        }]
    );
    let fin = out.finished();
    assert_eq!(fin.len(), 1);
    assert_eq!(
        fin.first().map(|f| (f.0, f.1)),
        Some((
            FinalOutcome::Failed {
                reason: FailReason::Key
            },
            PassMode::Timed
        ))
    );
}

// ---- decode ends (D5) -----------------------------------------------------------------------

#[test]
fn a_damaged_tail_ends_the_track_with_the_prefix_it_returned() {
    let speech = [(1_000u64, 5_000u64)];
    let out = run_with(
        Src::marked(0, 10_000).ending(DecodeEnd::Damaged),
        Src::marked(1, 10_000),
        FakeEngine::new(true, answer_ok),
        Opts::new(marked_primary(&speech)),
    );
    assert_eq!(out.out.outcome, FinalOutcome::Ready);
    let t = some(out.transcript(), "transcript.json");
    assert_eq!(t.segments.len(), 2);
}

fn decode_failure(end: DecodeEnd) {
    let speech = [(1_000u64, 5_000u64)];
    let out = run_with(
        Src::marked(0, 10_000),
        Src::marked(1, 10_000).ending(end),
        FakeEngine::new(true, answer_ok),
        Opts::new(marked_primary(&speech)),
    );
    assert_eq!(
        out.out.outcome,
        FinalOutcome::Failed {
            reason: FailReason::Storage
        }
    );
    let meta = out.meta();
    assert_eq!(meta.state, CallState::Failed);
    assert_eq!(
        meta.failure,
        Some(Failure {
            reason: FailReason::Storage,
            track: Some(Track::System)
        })
    );
    assert_eq!(
        meta.results.system,
        TrackResult::Failed {
            reason: FailReason::Storage
        }
    );
    assert!(!out.names().iter().any(|n| n.starts_with("transcript")));
    assert_eq!(
        out.notices(),
        vec![CallNotice::PassFailed {
            reason: FailReason::Storage
        }]
    );
}

#[test]
fn a_codec_error_fails_the_track_with_storage() {
    decode_failure(DecodeEnd::Codec);
}

#[test]
fn an_unreadable_track_fails_with_storage() {
    decode_failure(DecodeEnd::Unreadable);
}

// ---- writes -------------------------------------------------------------------------------------

/// Two short tracks and a good server; `prep` plants the obstacle.
fn short_pair(prep: impl FnOnce(&mut CallMeta, &Path) + 'static) -> Out {
    let speech = [(1_000u64, 5_000u64)];
    run_with(
        Src::marked(0, 10_000),
        Src::marked(1, 10_000),
        FakeEngine::new(true, answer_ok),
        Opts::new(marked_primary(&speech)).prep(prep),
    )
}

fn storage_failed(out: &Out, kind: StorageKind) {
    assert_eq!(
        out.out.outcome,
        FinalOutcome::Failed {
            reason: FailReason::Storage
        }
    );
    assert!(
        out.logs().iter().any(|e| matches!(
            e,
            LogEvent::CallStorageFailed { kind: k, .. } if *k == kind
        )),
        "call_storage_failed {kind:?} in {:?}",
        out.logs()
    );
    assert_eq!(
        out.notices(),
        vec![CallNotice::PassFailed {
            reason: FailReason::Storage
        }]
    );
    let finished = out.finished();
    assert_eq!(finished.len(), 1);
    assert_eq!(
        finished.first().map(|f| f.0),
        Some(FinalOutcome::Failed {
            reason: FailReason::Storage
        })
    );
    // No partial file: nothing that is a file is left under a temp name.
    for name in out.names() {
        if name.ends_with(".tmp") {
            assert!(
                out.dir.join(&name).is_dir(),
                "a partial file {name} was left behind"
            );
        }
    }
}

fn make_dir(name: &'static str) -> impl FnOnce(&mut CallMeta, &Path) {
    move |_, dir| ok(fs::create_dir(dir.join(name)))
}

#[test]
fn a_failed_markdown_write_leaves_failed_storage_and_no_transcript() {
    let out = short_pair(make_dir("transcript.md.tmp"));
    storage_failed(&out, StorageKind::Transcript);
    assert!(!out.names().contains("transcript.md"));
    assert!(!out.names().contains("transcript.json"));
    let meta = out.meta();
    assert_eq!(meta.state, CallState::Failed);
    assert_eq!(
        meta.failure,
        Some(Failure {
            reason: FailReason::Storage,
            track: None
        })
    );
}

#[test]
fn a_failed_json_write_leaves_failed_storage_and_no_transcript_json() {
    let out = short_pair(make_dir("transcript.json.tmp"));
    storage_failed(&out, StorageKind::Transcript);
    assert!(!out.names().contains("transcript.json"));
    if let Ok(md) = fs::read_to_string(out.dir.join("transcript.md")) {
        assert!(md.starts_with("# "), "a whole markdown file or none");
    }
    let meta = out.meta();
    assert_eq!(meta.state, CallState::Failed);
    assert_eq!(meta.failure.map(|f| f.reason), Some(FailReason::Storage));
}

#[test]
fn a_failed_meta_write_is_reported_not_swallowed() {
    // The first request plants a directory where the final `meta.json.tmp` goes: the
    // `transcribing` meta was written before, the final one cannot be.
    let speech = [(1_000u64, 5_000u64)];
    let dir = Arc::new(Mutex::new(None::<PathBuf>));
    let planted = dir.clone();
    let engine = FakeEngine::new(true, answer_ok).on_request(move |_| {
        if let Some(d) = lock(&planted).take() {
            let _ = fs::create_dir(d.join("meta.json.tmp"));
        }
    });
    let out = run_with(
        Src::marked(0, 10_000),
        Src::marked(1, 10_000),
        engine,
        Opts::new(marked_primary(&speech)).prep({
            let dir = dir.clone();
            move |_, d| *lock(&dir) = Some(d.to_path_buf())
        }),
    );
    storage_failed(&out, StorageKind::Meta);
    assert_eq!(
        out.meta().state,
        CallState::Transcribing,
        "the meta that could not be replaced is untouched"
    );
}

#[test]
fn a_transcript_that_fails_validation_is_logged_and_not_written() {
    // `s1` missing from the speakers: the system line (the second by time) breaks the rule.
    let speech = [(1_000u64, 5_000u64)];
    let out = run_with(
        Src::marked(0, 10_000),
        Src::marked(1, 10_000),
        FakeEngine::new(true, answer_ok),
        Opts::new(marked_primary(&speech))
            .offset(5_000)
            .prep(|meta, _| meta.speakers.retain(|s| s.id != "s1")),
    );
    assert_eq!(
        out.out.outcome,
        FinalOutcome::Failed {
            reason: FailReason::Storage
        }
    );
    assert!(
        out.logs().iter().any(|e| matches!(
            e,
            LogEvent::TranscriptInvalid {
                rule: InvalidRule::UnknownSpeaker,
                segment: 1
            }
        )),
        "transcript_invalid in {:?}",
        out.logs()
    );
    assert!(!out.names().iter().any(|n| n.starts_with("transcript")));
    let meta = out.meta();
    assert_eq!(meta.state, CallState::Failed);
    assert_eq!(meta.failure.map(|f| f.reason), Some(FailReason::Storage));
    assert_eq!(
        out.notices(),
        vec![CallNotice::PassFailed {
            reason: FailReason::Storage
        }]
    );
}

// ---- the TrackResult shape (storage) ------------------------------------------------------------

#[test]
fn track_results_have_the_meta_json_shape() {
    let seg = SegmentV1 {
        start_ms: 1,
        end_ms: 2,
        track: "mic".to_string(),
        speaker: "me".to_string(),
        lang: None,
        text: "t".to_string(),
        source: Some("final".to_string()),
    };
    let cases = [
        (TrackResult::Pending, serde_json::json!("pending")),
        (
            TrackResult::Failed {
                reason: FailReason::EngineNotSetUp,
            },
            serde_json::json!({"failed": {"reason": "engine_not_set_up"}}),
        ),
        (
            TrackResult::Done {
                segments: vec![seg.clone()],
            },
            serde_json::json!({"done": {"segments": [serde_json::to_value(&seg).unwrap_or_default()]}}),
        ),
    ];
    for (value, json) in cases {
        assert_eq!(ok(serde_json::to_value(&value)), json);
        let back: TrackResult = ok(serde_json::from_value(json));
        assert_eq!(back, value);
    }
}

// ---- failure-branch bite additions (T-138) ------------------------------------------------

/// Waits (bounded) until `flag` is set, then a moment for the answer to be handed in.
fn wait_for(flag: &AtomicBool) {
    for _ in 0..300 {
        if flag.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(150));
}

#[test]
fn a_chunk_answered_with_times_before_chunk_k_said_no_times_is_sent_again_time_less() {
    // Chunk 2 (start >= 200 s) is answered with times first; only then does chunk 1 (k)
    // answer "no times". "Every later chunk" includes the one already answered: it is
    // re-sent as plain pieces and no line of it comes from the timed answer.
    let later_done = Arc::new(AtomicBool::new(false));
    let flag = later_done.clone();
    let engine = FakeEngine::new(true, move |r| {
        if r.shape == ResponseShape::Segments && r.start >= 200_000 {
            flag.store(true, Ordering::SeqCst);
        }
        if r.shape == ResponseShape::Segments && (100_000..200_000).contains(&r.start) {
            wait_for(&flag);
            return Err(TimedFailure::Timeless);
        }
        answer_ok(r)
    });
    let out = run_with(
        Src::marked(0, 400_000),
        Src::marked(1, 0),
        engine,
        Opts::new(marked_primary(&SPEECH)),
    );
    assert!(
        later_done.load(Ordering::SeqCst),
        "scenario reached chunk 2"
    );
    assert_eq!(out.out.mode, PassMode::Timeless);
    let log = out.log();
    let plain: Vec<&Req> = log
        .iter()
        .filter(|r| r.shape == ResponseShape::Plain)
        .collect();
    assert_covered_once(&plain, &SPEECH[1..]);
    let t = some(out.transcript(), "transcript.json");
    ok(t.validate());
    let mut want = vec![line(1_800, 2_800, Track::Mic, "mic@800", None)];
    want.extend(plain.iter().map(|r| {
        line(
            r.start,
            r.start + r.len,
            Track::Mic,
            &format!("p{}", r.start),
            Some("de"),
        )
    }));
    assert_eq!(lines(&t), sorted(want));
}

#[test]
fn chunks_cut_after_the_flip_are_sent_time_less_even_when_the_server_would_answer_times() {
    // A 26-minute mic track: 12 timed chunks of about 100 s. Chunk 1 answers "no times";
    // chunks 0 and 2 (in flight) wait for that answer, so nothing after them is cut before
    // the flip. Every later timed request would be answered fine (a flaky server): a chunk
    // the planner cuts after the flip must still go out as plain pieces, never as timed.
    let speech: Vec<(u64, u64)> = (0..12u64)
        .map(|i| (i * 130_000 + 1_000, i * 130_000 + 100_000))
        .collect();
    let flipped = Arc::new(AtomicBool::new(false));
    let flag = flipped.clone();
    let engine = FakeEngine::new(true, move |r| {
        if r.shape == ResponseShape::Segments && (100_000..200_000).contains(&r.start) {
            flag.store(true, Ordering::SeqCst);
            return Err(TimedFailure::Timeless);
        }
        if r.shape == ResponseShape::Segments && r.start < 300_000 {
            wait_for(&flag);
        }
        answer_ok(r)
    });
    let out = run_with(
        Src::marked(0, 1_560_000),
        Src::marked(1, 0),
        engine,
        Opts::new(marked_primary(&speech)),
    );
    assert!(flipped.load(Ordering::SeqCst), "scenario reached chunk 1");
    assert_eq!(
        out.out,
        PassOutcome {
            outcome: FinalOutcome::Ready,
            mode: PassMode::Timeless
        }
    );
    assert_eq!(out.meta().pass_mode, Some(PassMode::Timeless));

    let log = out.log();
    assert!(
        log.iter()
            .filter(|r| r.shape == ResponseShape::Segments)
            .all(|r| r.start < 300_000),
        "a timed request for a chunk cut after the flip: {log:?}"
    );
    let plain: Vec<&Req> = log
        .iter()
        .filter(|r| r.shape == ResponseShape::Plain)
        .collect();
    assert!(plain.iter().all(|r| r.len <= TIMELESS_MAX_MS), "{plain:?}");
    assert_covered_once(&plain, speech.get(1..).unwrap_or_default());

    let t = some(out.transcript(), "transcript.json");
    ok(t.validate());
    let mut want = vec![line(1_800, 2_800, Track::Mic, "mic@800", None)];
    want.extend(plain.iter().map(|r| {
        line(
            r.start,
            r.start + r.len,
            Track::Mic,
            &format!("p{}", r.start),
            Some("de"),
        )
    }));
    assert_eq!(lines(&t), sorted(want));
}

#[test]
fn a_plain_request_that_is_refused_for_times_fails_the_track_with_provider() {
    // Every request says "no times", plain ones too: the server is at fault, the track fails.
    let engine = FakeEngine::new(true, |_| Err(TimedFailure::Timeless));
    let speech = [(1_000u64, 5_000u64)];
    let out = run_with(
        Src::marked(0, 10_000),
        Src::marked(1, 0),
        engine,
        Opts::new(marked_primary(&speech)),
    );
    assert_eq!(
        out.out.outcome,
        FinalOutcome::Failed {
            reason: FailReason::Provider
        }
    );
    assert_eq!(
        out.meta().results.mic,
        TrackResult::Failed {
            reason: FailReason::Provider
        }
    );
    assert!(!out.names().iter().any(|n| n.starts_with("transcript")));
}

#[test]
fn when_both_tracks_fail_the_reason_is_the_first_failure_in_dispatch_order() {
    // Mic's chunk is dispatched first (tie, mic first) but fails last, after the system
    // chunk failed with another reason: the call's reason is mic's, with no single track.
    let system_failed = Arc::new(AtomicBool::new(false));
    let flag = system_failed.clone();
    let engine = FakeEngine::new(true, move |r| {
        if r.track == Track::System {
            flag.store(true, Ordering::SeqCst);
            return Err(TimedFailure::Failed(FailReason::Provider));
        }
        wait_for(&flag);
        Err(TimedFailure::Failed(FailReason::Key))
    });
    let speech = [(1_000u64, 5_000u64)];
    let out = run_with(
        Src::marked(0, 10_000),
        Src::marked(1, 10_000),
        engine,
        Opts::new(marked_primary(&speech)),
    );
    assert!(system_failed.load(Ordering::SeqCst));
    assert_eq!(
        out.out.outcome,
        FinalOutcome::Failed {
            reason: FailReason::Key
        }
    );
    assert_eq!(
        out.meta().failure,
        Some(Failure {
            reason: FailReason::Key,
            track: None
        })
    );
    assert_eq!(
        out.meta().results.system,
        TrackResult::Failed {
            reason: FailReason::Provider
        }
    );
}

#[test]
fn a_failed_meta_write_after_a_track_failure_is_reported_and_ends_with_storage() {
    // The chunk fails (provider); the `failed` meta then cannot be written: the pass ends
    // with `storage`, logged, never with the provider reason as if the meta were saved.
    let dir = Arc::new(Mutex::new(None::<PathBuf>));
    let planted = dir.clone();
    let engine = FakeEngine::new(true, |_| Err(TimedFailure::Failed(FailReason::Provider)))
        .on_request(move |_| {
            if let Some(d) = lock(&planted).take() {
                let _ = fs::create_dir(d.join("meta.json.tmp"));
            }
        });
    let speech = [(1_000u64, 5_000u64)];
    let out = run_with(
        Src::marked(0, 10_000),
        Src::marked(1, 0),
        engine,
        Opts::new(marked_primary(&speech)).prep({
            let dir = dir.clone();
            move |_, d| *lock(&dir) = Some(d.to_path_buf())
        }),
    );
    storage_failed(&out, StorageKind::Meta);
}

#[test]
fn a_pauseless_track_is_read_in_windows_that_leave_room_for_the_carry() {
    // Speech without a pause: every hard cut leaves a carry, so the next window is asked
    // for less than TIMED_MAX_MS; what the planner holds never exceeds one window.
    let speech = [(0u64, 400_000u64)];
    let out = run_with(
        Src::marked(0, 400_000),
        Src::marked(1, 0),
        FakeEngine::new(true, answer_ok),
        Opts::new(marked_primary(&speech)),
    );
    assert_eq!(out.out.outcome, FinalOutcome::Ready);
    let asks = lock(&out.asks[0]).clone();
    let log = out.log();
    assert!(
        log.iter().all(|r| r.len <= 120_000),
        "a chunk is at most 120 s: {log:?}"
    );
    assert_covered_once(&log.iter().collect::<Vec<_>>(), &speech);
    assert!(
        asks.iter().any(|&a| a < 120_000),
        "a window asked while the planner carries audio is shorter than 120 s: {asks:?}"
    );
}
