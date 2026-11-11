//! T-138: the pass with the real engine over HTTP (a wiremock server) and real Opus tracks.
//!
//! wiremock serves on its own thread; the pass runs on the test's plain thread (the blocking
//! reqwest client panics inside a tokio context). The runtime here only drives the async calls
//! to the mock server. Fake data only: 127.0.0.1, `key-a`/`key-b`, tone audio.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, UNIX_EPOCH};

use voicen_core::audio::AudioBuffer;
use voicen_core::call::codec::{OpusOggReader, TrackDecoder, TrackEncoder};
use voicen_core::call::final_pass::{
    run_pass, run_pass_with, PassCall, PassDeps, PassEvents, PassOutcome, PassSources,
};
use voicen_core::call::storage::{write_meta, CallStore, Failure, NewCall, TrackResult};
use voicen_core::call::{
    CallMeta, CallNotice, CallState, DecodeEnd, EndpointId, EngineRef, FailReason, FinalOutcome,
    PassMode, StorageKind, Track, TranscriptV1,
};
use voicen_core::clock::SystemMonotonic;
use voicen_core::diag::event::LogEvent;
use voicen_core::engine::openai::OpenAiCompatibleEngine;
use voicen_core::engine::TranscribeRequest;
use voicen_core::i18n::UiLanguage;
use voicen_core::secrets::{
    CredentialError, CredentialOp, CredentialStore, FakeCredentialStore, KeySlot, Secret,
};
use voicen_core::settings::roots::FsCallRoots;
use voicen_core::settings::url::check_base_url;
use voicen_core::settings::{defaults, EngineKind};
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_core::vad::{SpeechSpan, SpeechSpans, VadError};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

#[path = "common/mock_http.rs"]
mod mock_http;

// ---- helpers ------------------------------------------------------------------------

#[derive(Default)]
struct Rec {
    logs: Mutex<Vec<LogEvent>>,
    notices: Mutex<Vec<CallNotice>>,
}

impl PassEvents for Rec {
    fn log(&self, event: LogEvent) {
        self.logs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }

    fn notice(&self, notice: CallNotice) {
        self.notices
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(notice);
    }
}

impl Rec {
    fn logs(&self) -> Vec<LogEvent> {
        self.logs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn notices(&self) -> Vec<CallNotice> {
        self.notices
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Loud 20 ms frames are speech.
struct AmpSpans;

impl SpeechSpans for AmpSpans {
    fn name(&self) -> &'static str {
        "amp"
    }

    fn speech_spans(&self, audio: &AudioBuffer) -> Result<Vec<SpeechSpan>, VadError> {
        let mut out: Vec<SpeechSpan> = Vec::new();
        for (i, frame) in audio.samples().chunks(320).enumerate() {
            if frame.iter().all(|s| s.unsigned_abs() < 2_000) {
                continue;
            }
            let (a, b) = (i as u64 * 20, i as u64 * 20 + 20);
            match out.last_mut() {
                Some(last) if last.end_ms == a => last.end_ms = b,
                _ => out.push(SpeechSpan {
                    start_ms: a,
                    end_ms: b,
                }),
            }
        }
        Ok(out)
    }
}

struct Mocked {
    rt: tokio::runtime::Runtime,
    server: MockServer,
}

impl Mocked {
    fn start(respond: impl Respond + 'static) -> Mocked {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let server = rt.block_on(async {
            let server = mock_http::start().await;
            Mock::given(any())
                .respond_with(respond)
                .mount(&server)
                .await;
            server
        });
        Mocked { rt, server }
    }

    fn uri(&self) -> String {
        self.server.uri()
    }

    fn requests(&self) -> Vec<Request> {
        self.rt
            .block_on(self.server.received_requests())
            .expect("request recording is on")
    }
}

/// `{"segments":[{"start":0,"end":1,"text":"hello"}], ..}`: a timed reply.
struct TimedReply;

impl Respond for TimedReply {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "task": "transcribe",
            "language": "english",
            "duration": 1.0,
            "text": "hello",
            "segments": [{"id": 0, "start": 0.0, "end": 1.0, "text": "hello"}]
        }))
    }
}

struct Call {
    dir: PathBuf,
    meta: CallMeta,
    tmp: TempDir,
}

/// A call folder with `meta.json` `ready-to-transcribe` and two Opus tracks; `fill` pushes the
/// audio of each track (`push` chunks of 16 kHz samples) before the encoders finish.
fn make_call(duration_ms: u64, fill: impl FnOnce(&mut dyn FnMut(Track, &[f32]))) -> Call {
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
    let mut files = store
        .create(&defaults(None).calls, &new)
        .expect("create the call");
    let dir = files.dir().to_path_buf();
    let mut meta = files.meta().clone();
    fill(&mut |track, pcm| {
        let enc = if track == Track::Mic {
            &mut files.mic
        } else {
            &mut files.system
        };
        enc.push(pcm).expect("push");
    });
    files.mic.finish().expect("finish mic");
    files.system.finish().expect("finish system");
    meta.state = CallState::ReadyToTranscribe;
    meta.duration_ms = Some(duration_ms);
    write_meta(&dir, &meta).expect("write meta");
    Call { dir, meta, tmp }
}

fn meta_of(dir: &Path) -> CallMeta {
    serde_json::from_slice(&fs::read(dir.join("meta.json")).expect("meta.json"))
        .expect("meta.json parses")
}

fn request() -> Timeouts {
    Timeouts::from_settings(&defaults(None).timeouts)
}

fn deps<'a>(rec: &'a Rec, primary: Option<Arc<dyn SpeechSpans>>) -> PassDeps<'a> {
    PassDeps {
        primary,
        events: rec,
        clock: Arc::new(SystemMonotonic),
    }
}

// ---- engine not set up ---------------------------------------------------------------------

/// A pass with `engine` and `key` over a call with empty tracks and a server that must not be
/// reached; returns what the tests look at.
fn setup_failure(
    engine: EngineKind,
    base_url: Option<&str>,
    key: Option<&str>,
    read_error: bool,
) -> (
    PassOutcome,
    CallMeta,
    Vec<CallNotice>,
    usize,
    FakeCredentialStore,
) {
    let server = Mocked::start(TimedReply);
    let call = make_call(1_000, |_| {});
    let mut settings = defaults(None);
    settings.engine = engine;
    settings.api.base_url = base_url.map_or_else(|| server.uri(), str::to_string);
    settings.api.model = "model-a".to_string();
    let mut creds = FakeCredentialStore::new();
    if let Some(key) = key {
        creds = creds.with_key(KeySlot::TranscriptionApi, key);
    }
    if read_error {
        creds.fail(
            CredentialOp::Read,
            KeySlot::TranscriptionApi,
            CredentialError { os_code: 5 },
        );
    }
    let rec = Rec::default();
    let out = run_pass(
        PassCall {
            dir: call.dir.clone(),
            meta: call.meta.clone(),
        },
        Arc::new(settings),
        &creds,
        deps(&rec, Some(Arc::new(AmpSpans))),
    );
    let meta = meta_of(&call.dir);
    let sent = server.requests().len();
    drop(call.tmp);
    (out, meta, rec.notices(), sent, creds)
}

fn assert_failed_before_any_request(
    got: (
        PassOutcome,
        CallMeta,
        Vec<CallNotice>,
        usize,
        FakeCredentialStore,
    ),
    reason: FailReason,
) {
    let (out, meta, notices, sent, _) = got;
    assert_eq!(out.outcome, FinalOutcome::Failed { reason });
    assert_eq!(sent, 0, "no request before the engine is set up");
    assert_eq!(meta.state, CallState::Failed);
    assert_eq!(
        meta.failure,
        Some(Failure {
            reason,
            track: None
        })
    );
    assert_eq!(meta.results.mic, TrackResult::Failed { reason });
    assert_eq!(meta.results.system, TrackResult::Failed { reason });
    assert_eq!(notices, vec![CallNotice::PassFailed { reason }]);
}

#[test]
fn a_builtin_engine_with_no_api_key_fails_engine_not_set_up_before_any_request() {
    let got = setup_failure(EngineKind::BuiltinLocal, None, None, false);
    let reads: Vec<_> = got
        .4
        .calls()
        .into_iter()
        .filter(|c| c.op == CredentialOp::Read && c.slot == KeySlot::TranscriptionApi)
        .collect();
    assert_eq!(reads.len(), 1, "the key is read once");
    assert_failed_before_any_request(got, FailReason::EngineNotSetUp);
}

#[test]
fn a_blank_api_key_is_no_key() {
    let got = setup_failure(EngineKind::BuiltinLocal, None, Some("   "), false);
    assert_failed_before_any_request(got, FailReason::EngineNotSetUp);
}

#[test]
fn a_refused_base_url_fails_engine_not_set_up() {
    let got = setup_failure(EngineKind::Api, Some("not a url"), Some("key-a"), false);
    assert_failed_before_any_request(got, FailReason::EngineNotSetUp);
}

#[test]
fn an_unreadable_key_store_fails_with_key() {
    let got = setup_failure(EngineKind::Api, None, Some("key-a"), true);
    assert_failed_before_any_request(got, FailReason::Key);
}

// ---- 30 minutes of real Opus ---------------------------------------------------------------

/// 5 s of a 440 Hz tone, 1 s of silence, repeated for `secs` seconds, as one second blocks.
fn tone_block(second: usize) -> Vec<f32> {
    let loud = second % 6 < 5;
    (0..16_000)
        .map(|i| {
            if loud {
                let t = (second * 16_000 + i) as f32 / 16_000.0;
                0.4 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
            } else {
                0.0
            }
        })
        .collect()
}

struct Recording<D> {
    inner: D,
    asks: Arc<Mutex<Vec<u32>>>,
}

impl<D: TrackDecoder> TrackDecoder for Recording<D> {
    fn next_window(&mut self, max_ms: u32) -> Result<Option<AudioBuffer>, DecodeEnd> {
        self.asks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(max_ms);
        self.inner.next_window(max_ms)
    }
}

fn open(dir: &Path, name: &str, asks: Arc<Mutex<Vec<u32>>>) -> Box<dyn TrackDecoder> {
    Box::new(Recording {
        inner: OpusOggReader::open(File::open(dir.join(name)).expect("open track"))
            .expect("valid Opus track"),
        asks,
    })
}

#[test]
fn a_thirty_minute_pair_is_decoded_in_windows_and_uploaded_in_bounded_chunks() {
    const SECS: usize = 1_800;
    let server = Mocked::start(TimedReply);
    let call = make_call(SECS as u64 * 1_000, |push| {
        for s in 0..SECS {
            let block = tone_block(s);
            push(Track::Mic, &block);
            push(Track::System, &block);
        }
    });
    let asks = [
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    ];
    let engine = OpenAiCompatibleEngine::new(
        check_base_url(&server.uri()).expect("url"),
        Some("model-a".to_string()),
        Some(Secret::new("key-a")),
    );
    let rec = Rec::default();
    let out = run_pass_with(
        PassCall {
            dir: call.dir.clone(),
            meta: call.meta.clone(),
        },
        &engine,
        &TranscribeRequest {
            language: None,
            timeouts: request(),
        },
        EngineRef::new("api", "model-a"),
        PassSources {
            mic: open(&call.dir, "mic.opus", asks[0].clone()),
            system: open(&call.dir, "system.opus", asks[1].clone()),
        },
        deps(&rec, Some(Arc::new(AmpSpans))),
    );
    assert_eq!(
        out,
        PassOutcome {
            outcome: FinalOutcome::Ready,
            mode: PassMode::Timed
        }
    );
    for (name, ask) in ["mic", "system"].iter().zip(&asks) {
        let ask = ask.lock().unwrap_or_else(PoisonError::into_inner);
        assert!(
            ask.len() >= 15,
            "{name}: 30 min is read in windows: {}",
            ask.len()
        );
        assert!(
            ask.iter().all(|&a| (1..=120_000).contains(&a)),
            "{name}: a window over 120 s: {ask:?}"
        );
    }
    let bodies = server.requests();
    assert!(bodies.len() >= 30, "at least 15 chunks a track");
    // 16 kHz 16-bit mono WAV of 120 s plus the multipart envelope.
    let bound = 44 + 32 * 120_000 + 4_096;
    assert!(
        bodies.iter().all(|r| r.body.len() <= bound),
        "an upload over 120 s of audio"
    );
    let names: std::collections::BTreeSet<String> = fs::read_dir(&call.dir)
        .expect("list")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names.into_iter().collect::<Vec<_>>(),
        [
            "meta.json",
            "mic.opus",
            "system.opus",
            "transcript.json",
            "transcript.md"
        ]
    );
    let t = TranscriptV1::from_json(
        &fs::read_to_string(call.dir.join("transcript.json")).expect("transcript.json"),
    )
    .expect("parses");
    t.validate().expect("valid");
    assert!(t.segments.len() >= 30);
    assert_eq!(rec.notices().len(), 1);
}

// ---- the settings of a running pass -----------------------------------------------------------

/// Writes key B into the shared store on the first request: a key saved during the pass.
struct ChangeKeyOnFirst {
    creds: Arc<FakeCredentialStore>,
    first: Mutex<bool>,
}

impl Respond for ChangeKeyOnFirst {
    fn respond(&self, r: &Request) -> ResponseTemplate {
        let mut first = self.first.lock().unwrap_or_else(PoisonError::into_inner);
        if !*first {
            *first = true;
            let _ = self
                .creds
                .write(KeySlot::TranscriptionApi, &Secret::new("key-b"));
        }
        TimedReply.respond(r)
    }
}

#[test]
fn a_key_saved_during_the_pass_is_not_picked_up() {
    const SECS: usize = 130;
    let creds = Arc::new(FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, "key-a"));
    let server = Mocked::start(ChangeKeyOnFirst {
        creds: creds.clone(),
        first: Mutex::new(false),
    });
    let call = make_call(SECS as u64 * 1_000, |push| {
        for s in 0..SECS {
            let block = tone_block(s);
            push(Track::Mic, &block);
            push(Track::System, &block);
        }
    });
    let mut settings = defaults(None);
    settings.engine = EngineKind::Api;
    settings.api.base_url = server.uri();
    settings.api.model = "model-a".to_string();
    let rec = Rec::default();
    let out = run_pass(
        PassCall {
            dir: call.dir.clone(),
            meta: call.meta.clone(),
        },
        Arc::new(settings),
        &*creds,
        deps(&rec, Some(Arc::new(AmpSpans))),
    );
    assert_eq!(out.outcome, FinalOutcome::Ready);
    let sent = server.requests();
    assert!(sent.len() >= 3, "several requests: {}", sent.len());
    for r in &sent {
        let auth = r.headers.get("authorization").and_then(|v| v.to_str().ok());
        assert_eq!(auth, Some("Bearer key-a"), "every request carries key A");
        assert!(
            r.body.windows(7).any(|w| w == b"model-a"),
            "every request carries model A"
        );
    }
    assert_eq!(
        creds.stored(KeySlot::TranscriptionApi).as_deref(),
        Some("key-b"),
        "the scenario did change the stored key"
    );
    let reads = creds
        .calls()
        .into_iter()
        .filter(|c| c.op == CredentialOp::Read)
        .count();
    assert_eq!(reads, 1, "the key is read once, before the first request");
    assert_eq!(
        meta_of(&call.dir).engines.transcription,
        Some(EngineRef::new("api", "model-a"))
    );
}

// ---- the real open_track: files, failures, storage (validation 3: 50, 52, 53, 77) -------------

/// One second of a 440 Hz tone when `loud`, else silence.
fn pulse_block(second: usize, loud: bool) -> Vec<f32> {
    (0..16_000)
        .map(|i| {
            if loud {
                let t = (second * 16_000 + i) as f32 / 16_000.0;
                0.4 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
            } else {
                0.0
            }
        })
        .collect()
}

/// `run_pass` over the call's own files with the mock server as the API.
fn real_pass(call: &Call, server: &Mocked) -> (PassOutcome, Rec) {
    let creds = FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, "key-a");
    let mut settings = defaults(None);
    settings.engine = EngineKind::Api;
    settings.api.base_url = server.uri();
    settings.api.model = "model-a".to_string();
    let rec = Rec::default();
    let out = run_pass(
        PassCall {
            dir: call.dir.clone(),
            meta: call.meta.clone(),
        },
        Arc::new(settings),
        &creds,
        deps(&rec, Some(Arc::new(AmpSpans))),
    );
    (out, rec)
}

/// A 10 s call: speech on the mic at seconds 1-2, on the system track at seconds 5-8.
fn distinguishable_call() -> Call {
    make_call(10_000, |push| {
        for s in 0..10 {
            push(Track::Mic, &pulse_block(s, (1..3).contains(&s)));
            push(Track::System, &pulse_block(s, (5..9).contains(&s)));
        }
    })
}

fn tone_call() -> Call {
    make_call(6_000, |push| {
        for s in 0..6 {
            push(Track::Mic, &pulse_block(s, true));
            push(Track::System, &pulse_block(s, true));
        }
    })
}

fn no_transcript(dir: &Path) {
    for name in ["transcript.json", "transcript.md"] {
        assert!(!dir.join(name).exists(), "{name} must not exist");
    }
}

#[test]
fn each_track_is_transcribed_from_its_own_file() {
    let server = Mocked::start(TimedReply);
    let call = distinguishable_call();
    let (out, _rec) = real_pass(&call, &server);
    assert_eq!(out.outcome, FinalOutcome::Ready);
    let meta = meta_of(&call.dir);
    // The reply is one segment at the start of its chunk, so a line starts where its track's
    // speech starts (less the 200 ms lead-in): mic about 0.8 s, system about 4.8 s.
    for (name, result, lo, hi) in [
        ("mic", &meta.results.mic, 300, 1_500),
        ("system", &meta.results.system, 4_300, 5_500),
    ] {
        let TrackResult::Done { segments } = result else {
            panic!("{name}: not done: {result:?}");
        };
        assert_eq!(segments.len(), 1, "{name}: one chunk, one line");
        for s in segments {
            assert_eq!(s.track, name);
            assert!(
                (lo..=hi).contains(&s.start_ms),
                "{name}: line at {} ms is not from its own file",
                s.start_ms
            );
        }
    }
    let t = TranscriptV1::from_json(
        &fs::read_to_string(call.dir.join("transcript.json")).expect("transcript.json"),
    )
    .expect("parses");
    assert_eq!(t.segments.len(), 2);
}

fn storage_failure_of(call: &Call, track: Track, server: &Mocked) {
    let (out, rec) = real_pass(call, server);
    assert_eq!(
        out.outcome,
        FinalOutcome::Failed {
            reason: FailReason::Storage
        }
    );
    let meta = meta_of(&call.dir);
    assert_eq!(meta.state, CallState::Failed);
    assert_eq!(
        meta.failure,
        Some(Failure {
            reason: FailReason::Storage,
            track: Some(track)
        })
    );
    let failed = TrackResult::Failed {
        reason: FailReason::Storage,
    };
    let (bad, good) = if track == Track::Mic {
        (&meta.results.mic, &meta.results.system)
    } else {
        (&meta.results.system, &meta.results.mic)
    };
    assert_eq!(bad, &failed);
    assert!(matches!(good, TrackResult::Done { .. }), "{good:?}");
    no_transcript(&call.dir);
    assert_eq!(
        rec.notices(),
        vec![CallNotice::PassFailed {
            reason: FailReason::Storage
        }]
    );
}

#[test]
fn a_missing_track_file_fails_the_track_with_storage() {
    let server = Mocked::start(TimedReply);
    let call = tone_call();
    fs::remove_file(call.dir.join("mic.opus")).expect("remove mic.opus");
    storage_failure_of(&call, Track::Mic, &server);
}

#[test]
fn a_track_with_a_corrupt_header_fails_with_storage() {
    let server = Mocked::start(TimedReply);
    let call = tone_call();
    fs::write(call.dir.join("system.opus"), [0xFFu8; 96]).expect("corrupt system.opus");
    storage_failure_of(&call, Track::System, &server);
}

#[test]
fn a_failed_transcribing_meta_write_fails_storage_before_any_request() {
    let server = Mocked::start(TimedReply);
    let call = tone_call();
    // An obstacle where `meta.json.tmp` goes: the first write of the pass cannot happen.
    fs::create_dir(call.dir.join("meta.json.tmp")).expect("plant the obstacle");
    let (out, rec) = real_pass(&call, &server);
    assert_eq!(
        out.outcome,
        FinalOutcome::Failed {
            reason: FailReason::Storage
        }
    );
    assert_eq!(
        server.requests().len(),
        0,
        "no request after a failed write"
    );
    assert!(
        rec.logs().iter().any(|e| matches!(
            e,
            LogEvent::CallStorageFailed {
                kind: StorageKind::Meta,
                ..
            }
        )),
        "call_storage_failed meta in {:?}",
        rec.logs()
    );
    assert_eq!(
        rec.notices(),
        vec![CallNotice::PassFailed {
            reason: FailReason::Storage
        }]
    );
    no_transcript(&call.dir);
}
