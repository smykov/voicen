//! T-139 (spec 007 FR-014, FR-022, FR-030; T048, T049; decisions #298): the call chain through
//! the shell's real wiring, on Windows CI only (decision #5; nothing here runs on the Linux
//! gate, which only compile-checks it): a WAV-driven call is recorded by the real session,
//! stopped, handed to the shell's one `PassQueue` and transcribed against a loopback
//! OpenAI-compatible server to a `transcript.json`, with the notices that reach the one shared
//! notifier.
//!
//! API pinned for the developer (docs/tasks/T-139.md `analysis.seam`; none of it exists yet, so
//! this file fails to compile until it does: that is the red, and the Windows CI red run of
//! `wip/T-139` is the observation):
//!
//! ```text
//! voicen_lib::call::
//!   CallPorts { .., credentials: Arc<dyn CredentialStore>, detector: Option<Arc<dyn SpeechSpans>> }
//!   start_calls(..)      // also manages the one Arc<voicen_core::call::final_pass::PassQueue>
//!   open_transcript<R: Runtime>(app: &AppHandle<R>, root: &Path, id: &CallId)
//!                        // the only `calls://open` emitter: payload {root: wire_root(root), id}
//! voicen_lib::vad::load_shared(resource_dir: &Path)
//!     -> (Result<Box<dyn SpeechDetector>, VadError>, Option<Arc<dyn SpeechSpans>>)
//!                        // one SileroDetector::open, both halves clones of it
//! ```
//!
//! The mock server is a hand-rolled `TcpListener` (as `dictation_e2e.rs`: no new dependency);
//! the key is saved through the settings service into the product `WinCredentialStore` under a
//! throwaway target prefix, under `cred_support::serial()` (decisions #298 Q2). No verdict is a
//! ceiling on a measured time (T-201): every wait is a positive wait or a named hang bound.
//!
//! Risk (docs/tasks/T-139.md): `tone_wav()` may not be speech to Silero, so the main chain runs
//! with no primary detector (the energy fallback finds the tone) and the Silero legs use the
//! real speech clip of `voicen-whisper`.
#![cfg(windows)]

mod call_shell_support;
mod cred_support;
mod timing_support;

use std::fs;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use call_shell_support::{
    eventually, holds_for_a_while, plain_opener, CallRig, WavSource, PUMP_MS,
};
use serde_json::{json, Value};
use tauri::{Listener, Manager};
use voicen_core::call::final_pass::PassQueue;
use voicen_core::call::storage::wire_root;
use voicen_core::call::transcript::TranscriptV1;
use voicen_core::call::{CallId, CallMeta, CallSource, CallState, FailReason};
use voicen_core::i18n::{
    MessageId, NOTICE_CALL_SAVED, NOTICE_PASS_FAILED_ENGINE_NOT_SET_UP, NOTICE_PASS_FAILED_KEY,
    NOTICE_TRANSCRIPT_READY,
};
use voicen_core::secrets::{CredentialStore, KeyEdit, KeyEdits, KeySlot, Secret as KeyText};
use voicen_core::settings::service::{SaveOutcome, SaveRequest};
use voicen_core::settings::EngineKind;
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::vad::silero_model::SILERO_MODEL;
use voicen_core::vad::SpeechSpans;
use voicen_lib::call::{self, CallRequest};
use voicen_lib::credentials::WinCredentialStore;
use voicen_lib::vad;

/// The fake key and model the pass must use: nothing else of the settings carries them.
const KEY: &str = "fake-chain-key-t139";
const MODEL: &str = "whisper-test-t139";
const MODEL_VAR: &str = "VOICEN_SILERO_TEST_MODEL";
const SPEECH_CLIP: &[u8] =
    include_bytes!("../../crates/voicen-whisper/tests/clips/speech-id-phrase.s16le");

/// Makes each run's throwaway Credential Manager prefix its own.
static RUNS: AtomicUsize = AtomicUsize::new(0);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- the mock OpenAI-compatible transcription server -----------------------------------------

/// One request the mock read.
#[derive(Debug, Clone)]
struct Seen {
    /// `POST /v1/audio/transcriptions HTTP/1.1`.
    request_line: String,
    authorization: Option<String>,
    body: Vec<u8>,
}

/// A loopback server over raw TCP: every connection is read (head plus `Content-Length` body),
/// recorded and answered with `status`; a `200` carries a timed transcription of one line.
struct Mock {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Mock {
    fn start(status: u16) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
        let addr = listener.local_addr().expect("local addr");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let record = Arc::clone(&record);
                thread::spawn(move || {
                    if let Ok(request) = answer(stream, status) {
                        lock(&record).push(request);
                    }
                });
            }
        });
        Mock { addr, seen }
    }

    /// What the settings store: `http://127.0.0.1:<port>/v1`.
    fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    fn requests(&self) -> Vec<Seen> {
        lock(&self.seen).clone()
    }
}

fn answer(mut s: TcpStream, status: u16) -> io::Result<Seen> {
    s.set_read_timeout(Some(timing_support::HANG))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        let n = s.read(&mut chunk)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_string();
    let mut length = 0usize;
    let mut authorization = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim();
            if name.eq_ignore_ascii_case("content-length") {
                length = value.parse().unwrap_or(0);
            } else if name.eq_ignore_ascii_case("authorization") {
                authorization = Some(value.to_string());
            }
        }
    }
    let mut body = buf[head_end..].to_vec();
    while body.len() < length {
        let n = s.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let reply = if status == 200 {
        json!({
            "task": "transcribe",
            "language": "english",
            "duration": 1.0,
            "text": "hello",
            "segments": [{"id": 0, "start": 0.0, "end": 1.0, "text": "hello"}]
        })
    } else {
        json!({"error": {"message": "refused by the T-139 mock"}})
    }
    .to_string();
    let phrase = if status == 200 { "OK" } else { "Refused" };
    write!(
        s,
        "HTTP/1.1 {status} {phrase}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    )?;
    s.flush()?;
    Ok(Seen {
        request_line,
        authorization,
        body,
    })
}

// ---- the chain ----------------------------------------------------------------------------

/// Deletes every slot of the throwaway target prefix when the test ends.
struct Cleanup(Arc<WinCredentialStore>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for slot in KeySlot::all() {
            let _ = self.0.delete(slot);
        }
    }
}

/// One call from start to the end of its pass.
struct Chain {
    rig: CallRig,
    mock: Mock,
    /// The root the call was created under (`<data dir>/calls`) and its folder.
    root: PathBuf,
    dir: PathBuf,
    id: CallId,
    // Dropped last: the cleanup runs under the lock.
    _cleanup: Cleanup,
    _serial: MutexGuard<'static, ()>,
}

/// What a chain run varies.
struct Opts {
    /// The mock's answer status.
    status: u16,
    /// Whether the key is saved into the product store.
    key: bool,
    /// The primary detector of the pass worker (`None`: core's energy fallback).
    detector: Option<Arc<dyn SpeechSpans>>,
    /// The source both tracks play.
    source: Arc<WavSource>,
    /// How long the source is pumped, ms.
    pump_ms: u64,
}

impl Opts {
    fn tone(status: u16, key: bool) -> Opts {
        Opts {
            status,
            key,
            detector: None,
            source: WavSource::new(),
            pump_ms: PUMP_MS,
        }
    }

    /// The real speech clip on both tracks, and `detector` as the pass worker's primary.
    fn speech(detector: Option<Arc<dyn SpeechSpans>>) -> Opts {
        let pcm: Vec<i16> = SPEECH_CLIP
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| i16::from_le_bytes(b))
            .collect();
        Opts {
            status: 200,
            key: true,
            detector,
            source: WavSource::with_pcm(&pcm),
            pump_ms: 3_000,
        }
    }
}

/// Saves the engine settings (BuiltinLocal active: the pass must not depend on the dictation
/// engine choice) with the mock as the API, and the key through the service's own store.
fn save_api(rig: &CallRig, mock: &Mock, key: bool) {
    let mut settings = (*rig.service.snapshot()).clone();
    settings.engine = EngineKind::BuiltinLocal;
    settings.api.base_url = mock.base_url();
    settings.api.model = MODEL.to_string();
    let keys = KeyEdits {
        transcription_api: if key {
            KeyEdit::Replace(KeyText::new(KEY))
        } else {
            KeyEdit::Untouched
        },
        ..KeyEdits::default()
    };
    match rig.service.save(SaveRequest { settings, keys }) {
        SaveOutcome::Saved { .. } => {}
        other => panic!("premise: saving the engine settings is refused: {other:?}"),
    }
}

/// Records a call through the real session, stops it like the user, waits for the managed
/// queue to go idle and returns what the tests look at.
#[track_caller]
fn run_chain(opts: Opts) -> Chain {
    let serial = cred_support::serial();
    let prefix = format!(
        "voicen-test-{}-chain-{}/",
        std::process::id(),
        RUNS.fetch_add(1, Ordering::SeqCst)
    );
    let store = Arc::new(WinCredentialStore::with_target_prefix(prefix));
    let cleanup = Cleanup(Arc::clone(&store));
    let as_port: Arc<dyn CredentialStore> = store;
    let mock = Mock::start(opts.status);
    let rig = CallRig::bare_with(as_port);
    save_api(&rig, &mock, opts.key);
    rig.start_with(
        opts.source.clone(),
        FakeDisk::with_available(100_000_000_000),
        plain_opener(),
        opts.detector,
    );

    call::request(rig.handle(), CallRequest::Record(CallSource::Tray));
    assert!(eventually(|| rig.recording()), "the call never started");
    assert!(
        eventually(|| opts.source.opens() == 2),
        "both tracks were not opened"
    );
    opts.source.pump_for(opts.pump_ms);
    rig.stop_call();

    let queue = rig
        .handle()
        .try_state::<Arc<PassQueue>>()
        .map(|q| Arc::clone(&q))
        .expect("start_calls manages the one PassQueue");
    assert!(
        timing_support::within_hang(move || queue.wait_idle()).is_some(),
        "HANG: the pass queue did not go idle"
    );

    let root = rig.dir.path().join("calls");
    let mut folders: Vec<PathBuf> = fs::read_dir(&root)
        .expect("the calls root")
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(folders.len(), 1, "premise: one call folder: {folders:?}");
    let dir = folders.remove(0);
    let id = dir
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(CallId::parse)
        .expect("the folder is named by the call id");
    Chain {
        rig,
        mock,
        root,
        dir,
        id,
        _cleanup: cleanup,
        _serial: serial,
    }
}

impl Chain {
    fn offered(&self) -> Vec<MessageId> {
        self.rig.notifier.calls().iter().map(|c| c.id).collect()
    }

    fn meta(&self) -> CallMeta {
        serde_json::from_slice(&fs::read(self.dir.join("meta.json")).expect("meta.json"))
            .expect("meta.json parses")
    }

    fn no_transcript(&self) {
        for name in ["transcript.json", "transcript.md"] {
            assert!(
                !self.dir.join(name).exists(),
                "{name} must not exist after a failed pass"
            );
        }
    }

    /// The `(speaker name, track)` of every line of a valid `transcript.json`.
    #[track_caller]
    fn lines(&self) -> Vec<(String, String)> {
        let text = fs::read_to_string(self.dir.join("transcript.json")).expect("transcript.json");
        let t = TranscriptV1::from_json(&text).expect("transcript.json parses");
        t.validate().expect("transcript.json validates");
        t.segments
            .iter()
            .map(|s| {
                let name = t
                    .speakers
                    .iter()
                    .find(|p| p.id == s.speaker)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| panic!("no speaker {:?}", s.speaker));
                (name, s.track.clone())
            })
            .collect()
    }

    /// The log lines that carry every one of `words`, once the writer has flushed `want`.
    #[track_caller]
    fn log_count(&self, words: &[&str], want: usize) -> usize {
        eventually(|| self.rig.count(words) == want);
        self.rig.count(words)
    }
}

/// `Me` on the mic track and `Other party` on the system track, as the contract names them.
fn assert_both_parties(lines: &[(String, String)]) {
    assert!(
        lines.contains(&("Me".to_string(), "mic".to_string())),
        "no `Me` line on the mic track: {lines:?}"
    );
    assert!(
        lines.contains(&("Other party".to_string(), "system".to_string())),
        "no `Other party` line on the system track: {lines:?}"
    );
}

// ---- the tests ------------------------------------------------------------------------------

#[test]
fn a_stopped_wav_call_is_transcribed_through_the_shells_pass_worker_with_the_dictation_api_settings(
) {
    // Acceptance 1, 2, 5 (toasts): the chain runs to a valid transcript with both parties, the
    // pass used the saved model, the bearer of the key saved through the product store and the
    // saved base URL, and the notifier got call_saved, then transcript_ready, once each.
    // Bite: no pass worker or no enqueue on a stop (no transcript, no ready notice), the
    // model or base URL read from anywhere but `settings.api`, the key not from the
    // TranscriptionApi slot, the pass notice before the saved one, a second enqueue (a second
    // ready notice).
    let chain = run_chain(Opts::tone(200, true));

    let meta = chain.meta();
    assert_eq!(meta.state, CallState::Ready, "{meta:?}");
    assert_both_parties(&chain.lines());
    assert!(chain.dir.join("transcript.md").is_file());

    let requests = chain.mock.requests();
    assert!(!requests.is_empty(), "the pass sent no request");
    let model_field = format!("name=\"model\"\r\n\r\n{MODEL}\r\n");
    for r in &requests {
        assert!(
            r.request_line.starts_with("POST /v1/"),
            "not sent to settings.api.base_url: {}",
            r.request_line
        );
        assert_eq!(
            r.authorization.as_deref(),
            Some(format!("Bearer {KEY}").as_str()),
            "the key of the TranscriptionApi slot"
        );
        assert!(
            String::from_utf8_lossy(&r.body).contains(&model_field),
            "settings.api.model is not in the request"
        );
    }

    assert_eq!(
        chain.offered(),
        vec![NOTICE_CALL_SAVED, NOTICE_TRANSCRIPT_READY],
        "call saved first, then exactly one transcript ready"
    );
    assert!(
        holds_for_a_while(|| chain.offered().len() == 2),
        "a later notice: {:?}",
        chain.rig.notifier.calls()
    );
}

#[test]
fn a_refused_key_gives_pass_failed_key_meta_failed_and_no_transcript() {
    // Acceptance 4 (failure branch): the server answers 401 -> `notice.pass_failed.key` after
    // `notice.call_saved`, meta `failed` with reason key, no transcript.json or .md. Bite: the
    // pass error swallowed (no notice), a ready notice on failure, a transcript written
    // anyway, meta left ready-to-transcribe.
    let chain = run_chain(Opts::tone(401, true));

    assert!(
        !chain.mock.requests().is_empty(),
        "premise: the pass reached the server"
    );
    assert_eq!(
        chain.offered(),
        vec![NOTICE_CALL_SAVED, NOTICE_PASS_FAILED_KEY]
    );
    let meta = chain.meta();
    assert_eq!(meta.state, CallState::Failed, "{meta:?}");
    assert_eq!(meta.failure.map(|f| f.reason), Some(FailReason::Key));
    chain.no_transcript();
}

#[test]
fn no_saved_key_gives_pass_failed_engine_not_set_up_and_no_request() {
    // Acceptance 4 (failure branch): the TranscriptionApi slot is empty -> the pass fails
    // `engine_not_set_up` before any request. Bite: a request sent without a key, the notice
    // missing, a ready notice.
    let chain = run_chain(Opts::tone(200, false));

    assert_eq!(
        chain.offered(),
        vec![NOTICE_CALL_SAVED, NOTICE_PASS_FAILED_ENGINE_NOT_SET_UP]
    );
    assert!(
        holds_for_a_while(|| chain.mock.requests().is_empty()),
        "a request went out without a key: {:?}",
        chain.mock.requests()
    );
    let meta = chain.meta();
    assert_eq!(meta.state, CallState::Failed, "{meta:?}");
    assert_eq!(
        meta.failure.map(|f| f.reason),
        Some(FailReason::EngineNotSetUp)
    );
    chain.no_transcript();
}

/// The CI-fetched model laid out as the installer does: `<dir>/vad/<file name>`.
fn resource_dir_with_model() -> TempDir {
    let from = PathBuf::from(
        std::env::var_os(MODEL_VAR)
            .unwrap_or_else(|| panic!("{MODEL_VAR} is not set: the CI fetch step must run first")),
    )
    .join(SILERO_MODEL.file_name);
    assert!(
        from.is_file(),
        "{MODEL_VAR} has no {}",
        SILERO_MODEL.file_name
    );
    let dir = TempDir::new();
    fs::create_dir_all(dir.path().join("vad")).expect("vad dir");
    fs::copy(&from, dir.path().join("vad").join(SILERO_MODEL.file_name)).expect("copy the model");
    dir
}

#[test]
fn load_shared_gives_both_halves_for_the_bundled_model_and_neither_without_it() {
    // Acceptance 6: the one load is one product helper whose two halves come from one
    // `SileroDetector::open`. With the model: the dictation half is `Ok` and the pass half
    // `Some`. Without it: `Err` and `None` (the energy fallback on both sides). Bite: one half
    // loaded separately or dropped, `Some` over a missing model, `Err` with `Some`.
    let dir = resource_dir_with_model();
    let (detector, spans) = vad::load_shared(dir.path());
    assert!(detector.is_ok(), "the dictation half: {:?}", detector.err());
    assert!(spans.is_some(), "the pass half");

    let empty = TempDir::new();
    let (detector, spans) = vad::load_shared(empty.path());
    assert!(detector.is_err(), "no model: the dictation half is Err");
    assert!(spans.is_none(), "no model: the pass half is None");
}

#[test]
fn a_call_with_the_shared_silero_half_writes_a_transcript_and_no_vad_fallback() {
    // Acceptance 5, 6: the pass worker's primary is the shared detector's `Some` half; the
    // speech clip is found by Silero, so the transcript has both parties and the log has no
    // `vad_fallback` line. Bite: the worker ignoring `CallPorts::detector` (a fallback line per
    // track), the half unusable.
    let dir = resource_dir_with_model();
    let (_, spans) = vad::load_shared(dir.path());
    assert!(spans.is_some(), "premise: the model loads");
    let chain = run_chain(Opts::speech(spans));

    assert_eq!(chain.meta().state, CallState::Ready);
    assert_both_parties(&chain.lines());
    assert_eq!(chain.log_count(&["vad_fallback"], 0), 0);
    assert!(
        holds_for_a_while(|| chain.rig.count(&["vad_fallback"]) == 0),
        "a vad_fallback line: {:?}",
        chain.rig.lines(&["vad_fallback"])
    );
}

#[test]
fn a_call_with_no_detector_falls_back_to_energy_and_logs_vad_fallback_unavailable_per_track() {
    // Acceptance 5 (energy fallback): `CallPorts::detector = None` -> the pass still writes a
    // transcript, and logs `vad_fallback cause=unavailable` once per track. Bite: a worker
    // that fails without a detector (no transcript), a silent fallback (no line), one line
    // for both tracks.
    let chain = run_chain(Opts::speech(None));

    assert_eq!(chain.meta().state, CallState::Ready);
    assert_both_parties(&chain.lines());
    assert_eq!(
        chain.log_count(&["vad_fallback", "cause=unavailable", "track=mic"], 1),
        1
    );
    assert_eq!(
        chain.log_count(&["vad_fallback", "cause=unavailable", "track=system"], 1),
        1
    );
}

#[test]
fn open_transcript_emits_exactly_one_calls_open_with_the_calls_root_and_id() {
    // Acceptance 3: `call::open_transcript(app, root, id)` is the one emitter of
    // `calls://open`, payload `{root: wire_root(root), id}` with the chain call's own root and
    // id (that this root is the notice's is core's `final_pass_http` test). Nothing is emitted
    // by the chain itself. Bite: no emit, two emits, the root read from the current settings,
    // the root as a different string, an id from anywhere else.
    let chain = run_chain(Opts::tone(200, true));
    let (tx, events) = mpsc::channel::<String>();
    chain.rig.handle().listen("calls://open", move |event| {
        let _ = tx.send(event.payload().to_string());
    });
    assert!(
        holds_for_a_while(|| events.try_recv().is_err()),
        "premise: a stop and a pass emit no calls://open"
    );

    call::open_transcript(chain.rig.handle(), &chain.root, &chain.id);

    let mut got = Vec::new();
    assert!(
        eventually(|| {
            while let Ok(payload) = events.try_recv() {
                got.push(payload);
            }
            !got.is_empty()
        }),
        "no calls://open"
    );
    assert!(
        holds_for_a_while(|| {
            while let Ok(payload) = events.try_recv() {
                got.push(payload);
            }
            got.len() == 1
        }),
        "more than one calls://open: {got:?}"
    );
    let payload: Value = serde_json::from_str(&got[0]).expect("a JSON payload");
    assert_eq!(
        payload,
        json!({"root": wire_root(&chain.root), "id": chain.id.to_string()})
    );
}
