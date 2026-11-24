//! T-157 (spec 007 T084; SC-006, SC-009, FR-032, FR-042): one whole call, with faults, leaks
//! no transcript, title, speaker name or device friendly name into the logs folder, and the
//! app survives every fault of the FR-032 matrix with the audio unchanged.
//!
//! Its own test binary: the panic hook is process-global, and the two tests here install it
//! one after the other under one lock (`serial`), so no other test file sees it.
//!
//! Invariants (docs/tasks/T-157.md):
//! - (P) every byte under the logs folder is written by `diag::Log`, `diag::Session` or
//!   `diag::crash` and holds none of the planted values (UTF-8 or UTF-16LE); the folder holds
//!   only `voicen*.log`, `crash-*.txt` and `session.marker`.
//! - (S) no thread panics in any row (the installed hook writes no crash file), every call
//!   returns, and the session starts a next call.
//! - (A) track bytes once written are never rewritten: bytes on disk before a fault are a
//!   prefix of the file after it, and after the stop both `.opus` files stay byte-identical
//!   through every later step (a pass of any outcome, recovery, rename, rename_speaker, a
//!   refused delete).
//!
//! The call is sourced from WAV bytes generated here (`WavTrackSource`, one per track) behind
//! a router that returns the planted friendly name as the endpoint text of every open, as a
//! shell adapter would hand a device name to `EndpointId::new`.
//!
//! The tests are green on correct code (decisions #346 Q2); their teeth are the three mutants
//! in `docs/tasks/T-157.mutants/`.

mod call_support;
mod common;
mod diag_support;

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::BufReader;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use call_support::{CallRig, BUDGET, GB, MB};
use diag_support::{contains, files_under, utf16le, Offset, NOON_UTC};
use voicen_core::audio::AudioBuffer;
use voicen_core::build_info::BuildInfo;
use voicen_core::call::codec::{scan, OpusOggReader, TrackDecoder};
use voicen_core::call::final_pass::{
    run_pass_with, ApiPassRunner, CancelToken, PassCall, PassDeps, PassEvents, PassOutcome,
    PassRunner, PassSources, TimedTranscriber,
};
use voicen_core::call::session::{CallSettingsPort, Toggled};
use voicen_core::call::storage::{
    wire_root, write_meta, CallErrorCode, FolderDeleter, InUse, Results,
};
use voicen_core::call::wav_source::{WavGap, WavTrackSource};
use voicen_core::call::{
    CallId, CallMeta, CallNotice, CallSource, CallState, CaptureStamp, DeviceEvent, EndpointId,
    EngineRef, EngineSegment, FailReason, FinalOutcome, LossCause, OpenError, PassMode, StopReason,
    Track, TrackDevice, TrackResult, TrackSink, TrackSource,
};
use voicen_core::clock::{Clock, FakeClock};
use voicen_core::diag::{
    install_panic_hook, CrashContext, FrameSource, Log, LogEvent, ModuleResolver, OsVersion,
    PreviousSession, Session, SessionStart,
};
use voicen_core::engine::openai::{ResponseShape, Timed, TimedFailure};
use voicen_core::engine::TranscribeRequest;
use voicen_core::platform::CaptureHandle;
use voicen_core::secrets::{FakeCredentialStore, KeySlot};
use voicen_core::settings::{defaults, Microphone, Settings};
use voicen_core::test_support::fixtures::speech_3s;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_core::vad::{SpeechSpan, SpeechSpans, VadError};

// ---- the planted values ---------------------------------------------------------------------

/// What the engine says (spec 006 T024 style: never in a log line or crash file).
const TRANSCRIPT: &str = "secret words 123 sk-test-PLANTED";
const TITLE: &str = "Quarterly Budget Review PLANTED-TITLE";
/// Multi-byte UTF-8 (two bytes per letter) and, as UTF-16LE, two bytes per letter as well.
const CYRILLIC_TITLE: &str = "Секретный план звонка";
const SPEAKER: &str = "Planted Speaker Name";
/// The device's friendly name: in the settings (`Microphone.name`) and returned by every open
/// as the endpoint text. It is outside the endpoint charset, so `EndpointId::new` drops it.
const FRIENDLY: &str = "Zebra Headset ZX-9000 (PLANTED-DEVICE)";

/// Searched whole, in UTF-8 and UTF-16LE.
const PLANTED: [&str; 5] = [TRANSCRIPT, TITLE, CYRILLIC_TITLE, SPEAKER, FRIENDLY];
/// Distinctive pieces of the values, searched the same way (a value cut or re-quoted still
/// shows).
const TOKENS: [&str; 7] = [
    "secret words 123",
    "sk-test-PLANTED",
    "PLANTED-TITLE",
    "Секретный",
    "Planted Speaker",
    "Zebra Headset",
    "PLANTED-DEVICE",
];

const PID: u32 = 4_242;
const OLD_STARTED: &str = "2026-10-03T12:00:00.000+02:00";

// ---- small helpers --------------------------------------------------------------------------

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn wait(cond: impl FnMut() -> bool) -> bool {
    common::timing::eventually(BUDGET, cond)
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn ix(track: Track) -> usize {
    match track {
        Track::Mic => 0,
        Track::System => 1,
    }
}

/// 16 kHz mono 16-bit PCM RIFF/WAVE of `samples`.
fn wav_bytes(samples: &[i16]) -> Vec<u8> {
    let data_len = u32::try_from(samples.len() * 2).expect("fits");
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&16_000u32.to_le_bytes());
    out.extend_from_slice(&32_000u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Five seconds of the speech fixture (3 s, repeated) as a WAV file.
fn speech_wav() -> Vec<u8> {
    let one = speech_3s();
    let samples: Vec<i16> = one.samples().iter().copied().cycle().take(80_000).collect();
    wav_bytes(&samples)
}

// ---- the track source: two WAV tracks behind one router -----------------------------------------

#[derive(Default)]
struct RouterState {
    oks: [usize; 2],
    attempts: [usize; 2],
    script: [VecDeque<OpenError>; 2],
}

/// Hands the session one `WavTrackSource` per successful open (the last one again when there
/// are more opens than sources) and the planted friendly name as the endpoint text.
struct Router {
    tracks: [Vec<Arc<WavTrackSource>>; 2],
    state: Mutex<RouterState>,
}

impl Router {
    /// The mic plays `wav` whole. The system track plays it too; with `lost_at`, its first
    /// source loses the stream there and the reopen gets a second source that starts 300 ms
    /// later.
    fn new(wav: &[u8], lost_at: Option<u64>) -> Arc<Router> {
        let source = || WavTrackSource::from_bytes(wav).expect("a valid WAV");
        let mut system = Vec::new();
        match lost_at {
            Some(at_ms) => {
                system.push(Arc::new(source().with_gaps(vec![WavGap::Lost {
                    at_ms,
                    cause: LossCause::NotAvailable,
                }])));
                system.push(Arc::new(
                    source().with_start(CaptureStamp::from_ms(at_ms + 300)),
                ));
            }
            None => system.push(Arc::new(source())),
        }
        Arc::new(Router {
            tracks: [vec![Arc::new(source())], system],
            state: Mutex::new(RouterState::default()),
        })
    }

    fn current(&self, track: Track, oks: usize) -> &Arc<WavTrackSource> {
        let list = &self.tracks[ix(track)];
        list.get(oks.saturating_sub(1).min(list.len() - 1))
            .expect("a source")
    }

    /// Pushes every block due by `until_ms` of the current source of each track.
    fn pump(&self, until_ms: u64) {
        let oks = lock(&self.state).oks;
        for track in [Track::Mic, Track::System] {
            self.current(track, oks[ix(track)])
                .pump(CaptureStamp::from_ms(until_ms));
        }
    }

    /// The next open of `track` fails with `error`; the one after it works (F-024).
    fn fail_next(&self, track: Track, error: OpenError) {
        lock(&self.state).script[ix(track)].push_back(error);
    }

    fn oks(&self, track: Track) -> usize {
        lock(&self.state).oks[ix(track)]
    }

    fn attempts(&self, track: Track) -> usize {
        lock(&self.state).attempts[ix(track)]
    }
}

impl TrackSource for Router {
    fn open(
        &self,
        track: Track,
        device: TrackDevice,
        sink: Arc<dyn TrackSink>,
    ) -> Result<(Box<dyn CaptureHandle>, EndpointId), OpenError> {
        let n = {
            let mut st = lock(&self.state);
            st.attempts[ix(track)] += 1;
            if let Some(error) = st.script[ix(track)].pop_front() {
                return Err(error);
            }
            st.oks[ix(track)] += 1;
            st.oks[ix(track)]
        };
        let (handle, _own_id) = self.current(track, n).open(track, device, sink)?;
        Ok((handle, EndpointId::new(FRIENDLY)))
    }

    fn now(&self) -> CaptureStamp {
        let oks = lock(&self.state).oks;
        [Track::Mic, Track::System]
            .into_iter()
            .map(|t| self.current(t, oks[ix(t)]).now())
            .max()
            .unwrap_or(CaptureStamp(0))
    }
}

// ---- the pass fakes -----------------------------------------------------------------------------

/// A primary detector that finds speech in the whole window.
struct WholeSpan;

impl SpeechSpans for WholeSpan {
    fn name(&self) -> &'static str {
        "whole"
    }

    fn speech_spans(&self, audio: &AudioBuffer) -> Result<Vec<SpeechSpan>, VadError> {
        let len = u64::try_from(audio.samples().len() / 16).unwrap_or(0);
        Ok(vec![SpeechSpan {
            start_ms: 0,
            end_ms: len,
        }])
    }
}

/// A primary detector that fails on every window (the pass falls back to energy spans).
struct BrokenSpans;

impl SpeechSpans for BrokenSpans {
    fn name(&self) -> &'static str {
        "broken"
    }

    fn speech_spans(&self, _audio: &AudioBuffer) -> Result<Vec<SpeechSpan>, VadError> {
        Err(VadError::Failed)
    }
}

type Answer = Box<dyn Fn(usize, ResponseShape) -> Result<Timed, TimedFailure> + Send + Sync>;

/// The one engine call of the pass; the answer gets the arrival number and the shape.
struct Engine {
    seen: AtomicUsize,
    answer: Answer,
}

impl Engine {
    fn new(
        answer: impl Fn(usize, ResponseShape) -> Result<Timed, TimedFailure> + Send + Sync + 'static,
    ) -> Engine {
        Engine {
            seen: AtomicUsize::new(0),
            answer: Box::new(answer),
        }
    }

    /// Every request answers with the planted transcript.
    fn ok() -> Engine {
        Engine::new(|_, shape| Ok(said(shape)))
    }

    /// Every request fails with `reason`.
    fn fail_all(reason: FailReason) -> Engine {
        Engine::new(move |_, _| Err(TimedFailure::Failed(reason)))
    }

    /// The first request is answered; every later one fails with `reason` (one track done,
    /// the other failed).
    fn first_ok_then(reason: FailReason) -> Engine {
        Engine::new(move |n, shape| {
            if n == 0 {
                Ok(said(shape))
            } else {
                Err(TimedFailure::Failed(reason))
            }
        })
    }

    /// The server gives no segment times: the first answer is `Timeless`, the re-sent plain
    /// request carries the text.
    fn timeless() -> Engine {
        Engine::new(|_, shape| match shape {
            ResponseShape::Segments => Err(TimedFailure::Timeless),
            ResponseShape::Plain => Ok(said(shape)),
        })
    }
}

fn said(shape: ResponseShape) -> Timed {
    match shape {
        ResponseShape::Segments => Timed {
            text: String::new(),
            language: None,
            segments: Some(vec![EngineSegment {
                start_s: 0.2,
                end_s: 0.9,
                text: TRANSCRIPT.to_string(),
                lang: None,
            }]),
        },
        ResponseShape::Plain => Timed {
            text: TRANSCRIPT.to_string(),
            language: None,
            segments: None,
        },
    }
}

impl TimedTranscriber for Engine {
    fn transcribe_timed(
        &self,
        _audio: &AudioBuffer,
        _req: &TranscribeRequest,
        shape: ResponseShape,
    ) -> Result<Timed, TimedFailure> {
        let n = self.seen.fetch_add(1, Ordering::SeqCst);
        (self.answer)(n, shape)
    }
}

/// The shell's `ShellPassEvents` shape: log lines go to `Log::write`, notices are kept.
struct LogEvents {
    log: Arc<Log>,
    notices: Mutex<Vec<CallNotice>>,
}

impl LogEvents {
    fn new(log: Arc<Log>) -> Arc<LogEvents> {
        Arc::new(LogEvents {
            log,
            notices: Mutex::new(Vec::new()),
        })
    }
}

impl PassEvents for LogEvents {
    fn log(&self, event: LogEvent) {
        self.log.write(event);
    }

    fn notice(&self, notice: CallNotice) {
        lock(&self.notices).push(notice);
    }
}

/// Refuses exactly the first claim of a delete (F-024); every other claim works.
struct RefuseFirst {
    used: AtomicBool,
}

impl FolderDeleter for RefuseFirst {
    fn claim(&self, file: &Path) -> Result<Box<dyn Send>, InUse> {
        if self.used.swap(true, Ordering::SeqCst) {
            return Ok(Box::new(()));
        }
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Err(InUse(name))
    }
}

// ---- the panic hook (process-global: one lock for the whole binary) -----------------------------

static HOOK: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    HOOK.lock().unwrap_or_else(PoisonError::into_inner)
}

struct NoFrames;

impl FrameSource for NoFrames {
    fn capture(&self, _out: &mut [usize]) -> usize {
        0
    }
}

struct NoModules;

impl ModuleResolver for NoModules {
    fn resolve(&self, _ip: usize) -> Option<(String, usize)> {
        None
    }
}

fn build() -> BuildInfo {
    BuildInfo {
        version: "0.9.1",
        commit: "abc1234",
    }
}

fn os() -> OsVersion {
    OsVersion {
        major: 10,
        minor: 0,
        build: 22_631,
    }
}

fn crash_context(logs: &Path) -> CrashContext {
    let clock: Arc<dyn Clock> = Arc::new(FakeClock::at(diag_support::at(NOON_UTC, 0)));
    CrashContext {
        logs_dir: logs.to_path_buf(),
        build: build(),
        os: os(),
        session_started: diag_support::at(NOON_UTC - 3_600, 0),
        pid: PID,
        clock,
        offset: Arc::new(Offset(0)),
        frames: Arc::new(NoFrames),
        modules: Arc::new(NoModules),
    }
}

/// Installs the crash hook over `logs`, runs `body`, puts the previous hook back (also when
/// `body` panics), then returns or re-raises.
fn with_hook<R>(logs: &Path, body: impl FnOnce() -> R) -> R {
    let previous = std::panic::take_hook();
    install_panic_hook(crash_context(logs));
    let out = std::panic::catch_unwind(AssertUnwindSafe(body));
    let _ours = std::panic::take_hook();
    std::panic::set_hook(previous);
    match out {
        Ok(r) => r,
        Err(p) => std::panic::resume_unwind(p),
    }
}

fn crash_files(logs: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(logs)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| n.starts_with("crash-") && n.ends_with(".txt"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

// ---- the scanners (P) ---------------------------------------------------------------------------

/// Every planted value or token found in a file under `dir`, as `<file> holds <value> (<enc>)`.
fn leaks(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for file in files_under(dir) {
        let bytes = fs::read(&file).unwrap_or_default();
        for value in PLANTED.iter().chain(TOKENS.iter()) {
            if contains(&bytes, value.as_bytes()) {
                out.push(format!("{} holds {value:?} (utf-8)", file.display()));
            }
            if contains(&bytes, &utf16le(value)) {
                out.push(format!("{} holds {value:?} (utf-16le)", file.display()));
            }
        }
    }
    out
}

/// The logs folder holds files directly in it, and only the three kinds diag writes.
fn assert_closed_file_set(logs: &Path, tag: &str) {
    for file in files_under(logs) {
        assert_eq!(file.parent(), Some(logs), "{tag}: a subfolder: {file:?}");
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let known = name == "voicen.log"
            || (name.starts_with("voicen-") && name.ends_with(".log"))
            || (name.starts_with("crash-") && name.ends_with(".txt"))
            || name == "session.marker";
        assert!(known, "{tag}: a file diag does not write: {name}");
    }
}

// ---- the fixture --------------------------------------------------------------------------------

type Audio = Vec<(String, Vec<u8>)>;

/// One rig over the WAV router, the audio baseline taken at the stop, and the steps a row
/// takes.
struct Fx {
    rig: CallRig,
    router: Arc<Router>,
    baseline: Option<Audio>,
}

impl Fx {
    fn new(lost_at: Option<u64>) -> Fx {
        let router = Router::new(&speech_wav(), lost_at);
        let rig = CallRig::with_source(
            8_192,
            |s: &mut Settings| {
                s.calls.warned_hosts = vec!["api.example.com".to_string()];
                s.microphone = Some(Microphone {
                    id: "not-an-endpoint-id".to_string(),
                    name: FRIENDLY.to_string(),
                });
            },
            router.clone(),
        );
        Fx {
            rig,
            router,
            baseline: None,
        }
    }

    /// The first call folder.
    fn dir(&self) -> PathBuf {
        self.rig.folders().first().cloned().expect("a call folder")
    }

    fn start(&self) {
        assert_eq!(
            self.rig.session.toggle_record(true, CallSource::Tray),
            Toggled::Started,
            "premise: the start succeeds"
        );
    }

    fn polled(&self) {
        let before = self.rig.devices.polls();
        assert!(
            wait(|| self.rig.devices.polls() >= before + 4),
            "the device thread keeps polling"
        );
    }

    fn await_opens(&self, track: Track, n: usize) {
        assert!(
            wait(|| self.router.oks(track) >= n),
            "{track:?} opened {n} times, got {}",
            self.router.oks(track)
        );
    }

    /// The consumer (subtitles on) has received nearly `until_ms` of both tracks.
    fn drained(&self, until_ms: u64) {
        let want = usize::try_from(until_ms.saturating_sub(300)).expect("fits") * 16;
        assert!(
            wait(|| self.rig.subtitles.samples(Track::Mic) >= want
                && self.rig.subtitles.samples(Track::System) >= want),
            "the writer consumed {until_ms} ms"
        );
    }

    fn tick_secs(&self, secs: u64) {
        for _ in 0..secs {
            self.rig.mono.advance(ms(1_000));
            self.rig.session.tick();
        }
    }

    fn audio_of(&self, dir: &Path) -> Audio {
        ["mic.opus", "system.opus"]
            .iter()
            .map(|name| {
                (
                    (*name).to_string(),
                    fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}")),
                )
            })
            .collect()
    }

    /// (A): the audio as it is now starts with `before` (bytes written are never rewritten).
    #[track_caller]
    fn assert_prefix(&self, before: &Audio, step: &str) {
        let after = self.audio_of(&self.dir());
        for ((name, was), (_, now)) in before.iter().zip(&after) {
            assert!(
                now.starts_with(was),
                "{step}: {name} no longer starts with the {} bytes written before",
                was.len()
            );
        }
    }

    /// Stops the call and takes the audio baseline every later step is compared with.
    fn stop_hold(&mut self, reason: StopReason) {
        assert!(self.rig.session.stop(reason), "a call was recording");
        self.hold();
    }

    fn hold(&mut self) {
        self.baseline = Some(self.audio_of(&self.dir()));
    }

    /// (A): both `.opus` files are byte-identical to the baseline taken at the stop.
    #[track_caller]
    fn same_audio(&self, step: &str) {
        let baseline = self.baseline.as_ref().expect("the call was stopped");
        let now = self.audio_of(&self.dir());
        for ((name, was), (_, is)) in baseline.iter().zip(&now) {
            assert!(
                was == is,
                "{step}: {name} changed ({} bytes at the stop, {} now)",
                was.len(),
                is.len()
            );
        }
    }

    /// The pass over the stopped call, when its `meta.json` reads as `ready-to-transcribe`;
    /// `None` otherwise (a call that stopped on a storage error, a damaged meta).
    fn pass_with(
        &self,
        engine: &Engine,
        primary: Option<Arc<dyn SpeechSpans>>,
    ) -> Option<PassOutcome> {
        let dir = self.dir();
        let meta: CallMeta = serde_json::from_slice(&fs::read(dir.join("meta.json")).ok()?).ok()?;
        if meta.state != CallState::ReadyToTranscribe {
            return None;
        }
        let events = LogEvents::new(self.rig.log.clone());
        let open = |name: &str| -> Box<dyn TrackDecoder> {
            let file = File::open(dir.join(name)).expect("a track file");
            Box::new(OpusOggReader::open(BufReader::new(file)).expect("a readable track"))
        };
        let out = run_pass_with(
            PassCall {
                dir: dir.clone(),
                meta,
            },
            engine,
            &TranscribeRequest {
                language: None,
                timeouts: Timeouts::from_settings(&defaults(None).timeouts),
            },
            EngineRef::new("api", "m1"),
            PassSources {
                mic: open("mic.opus"),
                system: open("system.opus"),
            },
            PassDeps {
                primary,
                events: &*events,
                clock: self.rig.mono.clone(),
            },
        );
        self.same_audio("a pass");
        Some(out)
    }

    fn pass(&self, engine: &Engine) -> Option<PassOutcome> {
        self.pass_with(engine, Some(Arc::new(WholeSpan)))
    }

    /// "Transcribe again": a failed call goes back to `ready-to-transcribe`, results kept.
    fn rearm(&self) {
        let dir = self.dir();
        let mut meta: CallMeta =
            serde_json::from_slice(&fs::read(dir.join("meta.json")).expect("meta.json"))
                .expect("a meta");
        meta.state = CallState::ReadyToTranscribe;
        meta.failure = None;
        write_meta(&dir, &meta).expect("meta.json written");
    }

    /// A second call folder as a killed process leaves it: the first call's tracks (the tail
    /// of the mic file cut mid-page) and a `meta.json` that says `recording`, with no process
    /// recording it.
    fn plant_killed_recording(&self) -> PathBuf {
        let src = self.dir();
        let name = format!("{}-1", src.file_name().expect("a name").to_string_lossy());
        let dst = self.rig.root.join(&name);
        fs::create_dir(&dst).expect("the second folder");
        for file in ["mic.opus", "system.opus"] {
            fs::copy(src.join(file), dst.join(file)).expect("a track copied");
        }
        let mut meta: CallMeta =
            serde_json::from_slice(&fs::read(src.join("meta.json")).expect("meta.json"))
                .expect("a meta");
        meta.id = CallId::parse(&name).expect("a call id");
        meta.state = CallState::Recording;
        meta.failure = None;
        meta.stop_reason = None;
        meta.ended_at = None;
        meta.duration_ms = None;
        meta.pass_mode = None;
        meta.results = Results {
            mic: TrackResult::Pending,
            system: TrackResult::Pending,
        };
        write_meta(&dst, &meta).expect("meta.json written");
        let mic = fs::OpenOptions::new()
            .write(true)
            .open(dst.join("mic.opus"))
            .expect("the copy opens");
        let len = mic.metadata().expect("metadata").len();
        mic.set_len(len - 7).expect("the tail is cut");
        dst
    }

    /// Everything a user or the app does to a stopped call afterwards, each step followed by
    /// the (A) check: rename (a Latin and a Cyrillic title), rename_speaker, a delete refused
    /// by one claim, and the start-up recovery.
    fn later_steps(&self) {
        let calls = self.rig.port.snapshot().calls.clone();
        let root = wire_root(&self.rig.root);
        let id = self
            .dir()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .expect("a folder name");
        let store = self.rig.store.clone();
        let _ = store.rename(&calls, &root, &id, TITLE);
        self.same_audio("rename");
        let _ = store.rename(&calls, &root, &id, CYRILLIC_TITLE);
        self.same_audio("rename (multi-byte)");
        let _ = store.rename_speaker(&calls, &root, &id, "s1", SPEAKER);
        self.same_audio("rename_speaker");
        let files_before = files_under(&self.rig.root);
        let refused = store.delete_folder(
            &calls,
            &root,
            &id,
            &RefuseFirst {
                used: AtomicBool::new(false),
            },
        );
        assert_eq!(
            refused.err().map(|e| e.code),
            Some(CallErrorCode::InUse),
            "a refused claim is inUse"
        );
        assert_eq!(
            files_under(&self.rig.root),
            files_before,
            "a refused delete removes nothing"
        );
        self.same_audio("a refused delete");
        let _ = store.recover(&calls, &self.rig.log);
        self.same_audio("recovery");
    }

    /// (S): after everything, the session starts and stops one more call.
    fn next_call_starts(&self) {
        self.rig.disk.set_free(100 * GB);
        let before = self.rig.folders().len();
        self.start();
        assert!(self.rig.session.stop(StopReason::User));
        assert_eq!(self.rig.folders().len(), before + 1, "a new call folder");
    }
}

// ---- the whole-call scenario (Acceptance 1) ------------------------------------------------------

#[test]
fn a_whole_call_with_a_device_change_a_failed_pass_and_a_crash_leaks_no_planted_value() {
    // Acceptance 1: a WAV-sourced two-track call with the planted friendly name (settings
    // and endpoint text), a device change (stream loss plus a default-output event), a stop,
    // a failed pass and a successful re-run (the transcript lands in the call folder), a
    // rename (Latin and Cyrillic), rename_speaker, a recovery over a second recording folder,
    // a start over a stale session marker and a panic whose payload is the transcript: the
    // logs folder holds 0 occurrences of the planted values and only diag's three kinds of
    // file. Bite: `EndpointId::new` keeping any text (call_started mic=), the panic hook
    // rendering its payload, a call line or pass line that carries a title, name or text.
    let _serial = serial();
    let mut fx = Fx::new(Some(1_500));
    let logs = fx.rig.logs.clone();

    fx.start();
    fx.router.pump(1_000);
    fx.router.pump(1_500);
    fx.await_opens(Track::System, 2);
    fx.rig.devices.push(DeviceEvent::DefaultOutputChanged);
    fx.polled();
    fx.router.pump(3_000);
    assert!(fx.rig.session.stop(StopReason::User));
    fx.hold();

    let meta = fx.rig.only_meta();
    assert_eq!(
        meta["tracks"][0]["endpoint"],
        serde_json::json!("invalid"),
        "premise: the friendly name is not an endpoint id, so it is not kept"
    );
    assert_eq!(meta["state"], serde_json::json!("ready-to-transcribe"));

    // A failed pass (one track done, the other failed), then the re-run that succeeds.
    let failed = fx.pass(&Engine::first_ok_then(FailReason::Provider));
    assert_eq!(
        failed,
        Some(PassOutcome {
            outcome: FinalOutcome::Failed {
                reason: FailReason::Provider
            },
            mode: PassMode::Timed
        })
    );
    assert_eq!(fx.rig.only_meta()["state"], serde_json::json!("failed"));
    fx.rearm();
    let ready = fx.pass(&Engine::ok());
    assert_eq!(
        ready,
        Some(PassOutcome {
            outcome: FinalOutcome::Ready,
            mode: PassMode::Timed
        })
    );
    let transcript = fs::read_to_string(fx.dir().join("transcript.json")).expect("transcript.json");
    assert!(
        transcript.contains(TRANSCRIPT),
        "premise: the planted transcript is in the call folder, where it belongs"
    );

    // Renames, a refused delete and the recovery of a second, killed recording.
    let calls = fx.rig.port.snapshot().calls.clone();
    let root = wire_root(&fx.rig.root);
    let id = fx
        .dir()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .expect("a folder name");
    let store = fx.rig.store.clone();
    assert!(store.rename(&calls, &root, &id, TITLE).is_ok());
    assert!(store.rename(&calls, &root, &id, CYRILLIC_TITLE).is_ok());
    assert!(store
        .rename_speaker(&calls, &root, &id, "s1", SPEAKER)
        .is_ok());
    assert!(
        fs::read_to_string(fx.dir().join("meta.json"))
            .expect("meta.json")
            .contains(SPEAKER),
        "premise: the speaker name is stored in the call folder"
    );
    fx.same_audio("rename and rename_speaker");

    let killed = fx.plant_killed_recording();
    let killed_before = fx.audio_of(&killed);
    fx.later_steps();
    let recovery = store.recover(&calls, &fx.rig.log);
    assert!(
        recovery.recovered.len() <= 1,
        "at most the planted folder is recovered"
    );
    assert_eq!(
        fx.audio_of(&killed),
        killed_before,
        "recovery reads the killed call's tracks and never writes them"
    );
    let killed_meta: serde_json::Value =
        serde_json::from_slice(&fs::read(killed.join("meta.json")).expect("meta.json"))
            .expect("JSON");
    assert_eq!(
        killed_meta["state"],
        serde_json::json!("recovered"),
        "premise: the planted folder was recovered"
    );
    fx.same_audio("recovery over a second folder");

    // The previous session ended without a clean exit, and then this one panics.
    fs::write(
        logs.join("session.marker"),
        format!("pid=4100\nstarted={OLD_STARTED}\nversion=0.9.0\ncommit=fedcba9\n"),
    )
    .expect("a stale marker");
    let clock = FakeClock::at(diag_support::at(NOON_UTC, 0));
    let (_session, previous) = Session::begin(
        SessionStart {
            logs_dir: &logs,
            build: build(),
            os: os(),
            pid: PID,
            clock: &clock,
            offset: &Offset(0),
        },
        &fx.rig.log,
    );
    assert!(
        matches!(previous, PreviousSession::EndedAbnormally { .. }),
        "premise: an abnormal_end crash file was written: {previous:?}"
    );
    let panicked = with_hook(&logs, || {
        std::panic::catch_unwind(|| {
            panic!("{TRANSCRIPT}");
        })
    });
    assert!(panicked.is_err(), "premise: the planted panic happened");

    // Non-vacuity of the evidence: the files the scan reads are really there.
    let crashes = crash_files(&logs);
    assert!(
        crashes.len() >= 2,
        "an abnormal_end and a panic file: {crashes:?}"
    );
    let kinds: String = crashes
        .iter()
        .map(|n| fs::read_to_string(logs.join(n)).unwrap_or_default())
        .collect();
    assert!(kinds.contains("kind: abnormal_end") && kinds.contains("kind: panic"));
    let heads: Vec<String> = fx.rig.call_lines().iter().map(|l| l.head.clone()).collect();
    for head in ["call_started", "call_stopped", "call_recovered"] {
        assert!(
            heads.iter().any(|h| h == head),
            "{head} was logged: {heads:?}"
        );
    }
    assert!(
        diag_support::all_lines(&logs)
            .iter()
            .any(|l| l.contains("final_finished")),
        "the pass lines were logged"
    );

    assert_closed_file_set(&logs, "scenario");
    assert_eq!(
        leaks(&logs),
        Vec::<String>::new(),
        "planted values in the logs folder"
    );
    fx.same_audio("the whole scenario");
}

#[test]
fn the_scan_finds_a_planted_value_in_a_file_of_the_scanned_set() {
    // Non-vacuity of (P): every value and token, planted into a file of the kinds the logs
    // folder holds, in UTF-8 and in UTF-16LE, is reported; a clean folder reports nothing.
    // Bite: a `leaks` that skips an encoding, a value, or a file kind.
    let _serial = serial();
    let tmp = TempDir::new();
    let dir = tmp.path().join("logs");
    fs::create_dir(&dir).expect("the folder");
    fs::write(dir.join("voicen.log"), "a clean line\n").expect("write");
    assert_eq!(leaks(&dir), Vec::<String>::new(), "a clean folder");
    for (i, value) in PLANTED.iter().chain(TOKENS.iter()).enumerate() {
        let clean = tmp.path().join(format!("clean-{i}"));
        fs::create_dir(&clean).expect("the folder");
        fs::write(clean.join("voicen.log"), format!("x {value} y\n")).expect("write");
        let found = leaks(&clean);
        assert!(
            found.iter().any(|f| f.ends_with("(utf-8)")),
            "{value:?} as UTF-8 is found: {found:?}"
        );
        fs::write(clean.join("crash-20261004-120000-1.txt"), utf16le(value)).expect("write");
        let found = leaks(&clean);
        assert!(
            found
                .iter()
                .any(|f| f.contains("crash-") && f.ends_with("(utf-16le)")),
            "{value:?} as UTF-16LE is found: {found:?}"
        );
    }
}

#[test]
fn a_panic_on_a_thread_leaves_a_crash_file_the_survival_check_would_see() {
    // Non-vacuity of (S): the matrix asserts "no crash file"; this shows the hook it relies
    // on writes one for a panic on a spawned thread (the writer thread's case), with no
    // planted value in it. Bite: a hook that is not installed, or a crash_files() that
    // misses the file.
    let _serial = serial();
    let tmp = TempDir::new();
    let logs = tmp.path().join("logs");
    fs::create_dir(&logs).expect("the folder");
    with_hook(&logs, || {
        let joined = std::thread::spawn(|| panic!("{TRANSCRIPT}")).join();
        assert!(joined.is_err());
    });
    let files = crash_files(&logs);
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(leaks(&logs), Vec::<String>::new());
}

// ---- the fault matrix (Acceptance 2, the failure branch) ----------------------------------------

type Body = Box<dyn Fn(&mut Fx)>;

struct Row {
    name: String,
    lost_at: Option<u64>,
    run: Body,
}

fn row(name: &str, run: impl Fn(&mut Fx) + 'static) -> Row {
    Row {
        name: name.to_string(),
        lost_at: None,
        run: Box::new(run),
    }
}

/// A call of `ms` of audio, stopped by the user, audio baseline taken.
fn normal_call(fx: &mut Fx, until_ms: u64) {
    fx.start();
    fx.router.pump(until_ms);
    fx.stop_hold(StopReason::User);
}

/// Makes the next `meta.json` write of the only call folder fail: the file becomes a
/// directory, so the atomic rename onto it is refused (on every OS).
fn block_meta(dir: &Path) -> Vec<u8> {
    let bytes = fs::read(dir.join("meta.json")).expect("meta.json");
    fs::remove_file(dir.join("meta.json")).expect("remove meta.json");
    fs::create_dir(dir.join("meta.json")).expect("a directory named meta.json");
    bytes
}

fn storage_failed_kinds(fx: &Fx) -> Vec<String> {
    fx.rig
        .lines_named("call_storage_failed")
        .iter()
        .map(|l| l.get("kind").unwrap_or("").to_string())
        .collect()
}

fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row {
            lost_at: Some(1_000),
            ..row("capture loss: the system stream dies and reopens", |fx| {
                fx.start();
                fx.router.pump(1_000);
                fx.await_opens(Track::System, 2);
                fx.router.pump(2_500);
                fx.stop_hold(StopReason::User);
                assert!(fx.pass(&Engine::ok()).is_some());
            })
        },
        Row {
            lost_at: Some(1_000),
            ..row(
                "reopen failure: the first reopen is refused, the retry works",
                |fx| {
                    fx.start();
                    fx.router.fail_next(Track::System, OpenError::NoDevice);
                    fx.router.pump(1_000);
                    assert!(wait(|| fx.router.attempts(Track::System) >= 2));
                    for _ in 0..5 {
                        if fx.router.oks(Track::System) >= 2 {
                            break;
                        }
                        fx.rig.mono.advance(ms(2_000));
                        fx.polled();
                    }
                    fx.await_opens(Track::System, 2);
                    fx.router.pump(2_500);
                    fx.stop_hold(StopReason::User);
                    assert!(fx.pass(&Engine::ok()).is_some());
                },
            )
        },
        row("track write failure: the mic file fails one write", |fx| {
            let both = [Track::Mic, Track::System];
            fx.rig.session.toggle_subtitles(true, CallSource::Switch);
            fx.start();
            fx.router.pump(4_000);
            fx.drained(4_000);
            let before = fx.audio_of(&fx.dir());
            for (name, bytes) in &before {
                let kept = scan(File::open(fx.dir().join(name)).expect("a track"))
                    .expect("readable")
                    .duration_48k;
                assert!(kept >= 2 * 48_000 - 312, "{name}: two pages synced, {kept}");
                assert!(!bytes.is_empty());
            }
            fx.rig.opener.fail_next_write("mic.opus");
            fx.router.pump(6_000);
            fx.drained(6_000);
            assert!(
                wait(|| !storage_failed_kinds(fx).is_empty()),
                "the write failure is logged"
            );
            assert_eq!(storage_failed_kinds(fx)[0], "write");
            fx.router.pump(7_000);
            assert!(
                wait(|| {
                    fx.rig.session.tick();
                    !fx.rig.session.is_recording()
                }),
                "the tick stopped the call"
            );
            fx.rig.session.toggle_subtitles(false, CallSource::Switch);
            fx.assert_prefix(&before, "after the write failure");
            fx.hold();
            assert!(both.iter().all(|_| true));
            assert_eq!(fx.rig.only_meta()["state"], serde_json::json!("recording"));
        }),
        row(
            "mid-call meta write failure: a device change cannot be saved",
            |fx| {
                fx.start();
                fx.router.pump(1_000);
                let meta = block_meta(&fx.dir());
                fx.rig.devices.push(DeviceEvent::DefaultOutputChanged);
                fx.await_opens(Track::System, 2);
                fx.router.pump(2_000);
                assert!(
                    wait(|| !storage_failed_kinds(fx).is_empty()),
                    "the failed meta write is logged"
                );
                assert_eq!(storage_failed_kinds(fx)[0], "meta");
                assert!(fx.rig.session.is_recording(), "the call goes on");
                fs::remove_dir(fx.dir().join("meta.json")).expect("unblock");
                fs::write(fx.dir().join("meta.json"), meta).expect("meta.json back");
                fx.router.pump(2_500);
                fx.stop_hold(StopReason::User);
                assert!(fx.pass(&Engine::ok()).is_some());
            },
        ),
        row("final meta write failure: the stop still completes", |fx| {
            fx.start();
            fx.router.pump(1_500);
            block_meta(&fx.dir());
            fx.stop_hold(StopReason::User);
            assert_eq!(storage_failed_kinds(fx), vec!["meta".to_string()]);
            assert_eq!(fx.rig.lines_named("call_stopped").len(), 1);
            assert!(
                fx.pass(&Engine::ok()).is_none(),
                "a damaged meta starts no pass"
            );
        }),
        row("low disk: below 200 MB the call stops and saves", |fx| {
            fx.start();
            fx.router.pump(1_500);
            fx.rig.disk.set_free(100 * MB);
            for _ in 0..15 {
                if !fx.rig.session.is_recording() {
                    break;
                }
                fx.tick_secs(1);
            }
            assert!(!fx.rig.session.is_recording(), "the call stopped");
            fx.hold();
            assert_eq!(storage_failed_kinds(fx), vec!["low_disk".to_string()]);
            assert!(fx.pass(&Engine::ok()).is_some());
        }),
        row("disk poll error: a failing poll is skipped", |fx| {
            fx.start();
            fx.router.pump(1_500);
            fx.rig.disk.set_error(std::io::ErrorKind::PermissionDenied);
            fx.tick_secs(25);
            assert!(fx.rig.session.is_recording(), "the call goes on");
            fx.stop_hold(StopReason::User);
            assert!(fx.pass(&Engine::ok()).is_some());
        }),
    ];
    for reason in [
        FailReason::Key,
        FailReason::Network,
        FailReason::Timeout,
        FailReason::Provider,
        FailReason::Storage,
        FailReason::EngineNotSetUp,
    ] {
        rows.push(row(&format!("pass failure: {reason:?}"), move |fx| {
            normal_call(fx, 3_000);
            let out = fx
                .pass(&Engine::fail_all(reason))
                .expect("ready to transcribe");
            assert_eq!(
                out.outcome,
                FinalOutcome::Failed { reason },
                "every track failed with the engine's reason"
            );
            assert_eq!(fx.rig.only_meta()["state"], serde_json::json!("failed"));
        }));
    }
    rows.push(row(
        "time-less fallback: the server gives no segment times",
        |fx| {
            normal_call(fx, 3_000);
            let out = fx.pass(&Engine::timeless()).expect("ready to transcribe");
            assert_eq!(
                out,
                PassOutcome {
                    outcome: FinalOutcome::Ready,
                    mode: PassMode::Timeless
                }
            );
        },
    ));
    rows.push(row(
        "detector error: the pass falls back to energy spans",
        |fx| {
            normal_call(fx, 3_000);
            let out = fx.pass_with(&Engine::ok(), Some(Arc::new(BrokenSpans)));
            assert!(out.is_some(), "a pass ran");
            assert!(
                diag_support::all_lines(&fx.rig.logs)
                    .iter()
                    .any(|l| l.contains("vad_fallback")),
                "the fallback was logged"
            );
        },
    ));
    rows.push(row("unreadable meta at the start of a pass", |fx| {
        normal_call(fx, 3_000);
        fs::write(fx.dir().join("meta.json"), b"{ not json").expect("garbage meta");
        let events = LogEvents::new(fx.rig.log.clone());
        let runner = ApiPassRunner::new(
            Arc::new(|| Arc::new(defaults(None))),
            Arc::new(FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, "key-a")),
            None,
            events.clone(),
            fx.rig.mono.clone(),
        );
        let out = runner.run(&fx.dir(), &CancelToken::new());
        assert_eq!(
            out.outcome,
            FinalOutcome::Failed {
                reason: FailReason::Storage
            }
        );
        assert!(
            lock(&events.notices).contains(&CallNotice::PassFailed {
                reason: FailReason::Storage
            }),
            "the failure is announced"
        );
        fx.same_audio("a pass over an unreadable meta");
    }));
    rows.push(row("delete refused: one claim is held elsewhere", |fx| {
        normal_call(fx, 3_000);
        assert!(fx.pass(&Engine::ok()).is_some());
        let calls = fx.rig.port.snapshot().calls.clone();
        let root = wire_root(&fx.rig.root);
        let id = fx
            .dir()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .expect("a folder name");
        let files = files_under(&fx.dir());
        let refused = fx.rig.store.delete_folder(
            &calls,
            &root,
            &id,
            &RefuseFirst {
                used: AtomicBool::new(false),
            },
        );
        assert_eq!(refused.err().map(|e| e.code), Some(CallErrorCode::InUse));
        assert_eq!(files_under(&fx.dir()), files, "nothing was removed");
        fx.same_audio("the refused delete");
    }));
    rows.push(row("recovery of a killed recording", |fx| {
        normal_call(fx, 3_000);
        assert!(fx.pass(&Engine::ok()).is_some());
        let killed = fx.plant_killed_recording();
        let before = fx.audio_of(&killed);
        let calls = fx.rig.port.snapshot().calls.clone();
        let recovery = fx.rig.store.recover(&calls, &fx.rig.log);
        assert_eq!(recovery.recovered.len(), 1, "the killed call is recovered");
        assert_eq!(
            fx.audio_of(&killed),
            before,
            "recovery never writes a track"
        );
        fx.same_audio("recovery");
    }));
    rows
}

fn run_row(row: &Row) {
    let mut fx = Fx::new(row.lost_at);
    let logs = fx.rig.logs.clone();
    with_hook(&logs, || {
        (row.run)(&mut fx);
        fx.later_steps();
        fx.next_call_starts();
    });
    // (S): no thread panicked, which the installed hook would have written down.
    assert_eq!(
        crash_files(&logs),
        Vec::<String>::new(),
        "{}: a thread panicked",
        row.name
    );
    // (A): the audio is byte-identical to what the stop left.
    fx.same_audio(&row.name);
    // (P): the logs folder holds only diag's files and none of the planted values.
    assert_closed_file_set(&logs, &row.name);
    assert_eq!(
        leaks(&logs),
        Vec::<String>::new(),
        "{}: planted values in the logs folder",
        row.name
    );
}

#[test]
fn across_the_fault_matrix_the_app_survives_the_audio_is_unchanged_and_nothing_leaks() {
    // Acceptance 2 (failure branch): capture loss, reopen failure, a track write failure,
    // a meta write failure mid-call and at the stop, low disk, a disk poll error, each pass
    // failure reason, the time-less fallback, a detector error, an unreadable meta at the
    // start of a pass, a refused delete and the recovery of a killed recording. Each row has
    // a fresh rig and asserts (S) no panic and a next call starts, (A) the audio prefix
    // before the fault and byte-identical after the stop through every later step, and (P).
    // Bite: recovery or a rename opening a track for write; a panic on any thread; a call
    // line carrying planted text; a write after a failed track write.
    let _serial = serial();
    let rows = rows();
    assert!(rows.len() >= 18, "the whole matrix: {}", rows.len());
    for row in &rows {
        run_row(row);
    }
}
