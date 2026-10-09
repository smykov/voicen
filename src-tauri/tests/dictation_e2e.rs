//! T-006 Acceptance line 1: "Windows CI: integration test pastes a transcript (mock
//! engine, injected WAV source) into a test window and the clipboard holds it, excluded
//! from clipboard history". Through the real wiring: `build_app` (as `run()`), then
//! `voicen_lib::dictation::start_dictation` with the injected ports (the real-time WAV
//! source `RealtimeSource` over `fixtures::speech_3s`, a fixed-text test engine of kind
//! `api`, the real `WinClipboard` and `WinPaster`, `FakeIndicator`), the real hotkey
//! thread and a synthetic Ctrl+Alt+Space hold. Windows CI only (decision #5).
//!
//! Runner capabilities (docs/decisions/windows-ci-runner.md): `sendinput`, `hotkey`,
//! `async_keys`, `clipboard` (`ok` in runs A and B) and `foreground_again` (re-measured by
//! the probe in every job); asserted loudly by `win32_support`. The hold is timed from the observed press (the indicator's
//! Recording), and every wait is at least 3 s (run B: ~1 s to `WM_HOTKEY`).
//!
//! Red-test table row 18. The data dir and the log are a `TempDir`; the key is fake; the
//! transcript carries a canary that must never reach the log (FR-020, NFR-04).
//!
//! T-009: Esc during a hold through the same wiring discards the recording.
//!
//! T-074 (decision #91 follow-up 1): the same hold with post-processing enabled through a
//! settings save, so the post-processor `start_dictation` itself installs runs. The chat
//! endpoint is a raw-TCP loopback mock ([`ChatMock`]) answering every request with a
//! fixed reply; the failure branch points the base URL at `127.0.0.1:1`
//! ([`refused_loopback`], the refused-address rule of decision #53, probed at each call),
//! with connect 5 s and post-processing 10 s so the refusal (~2.2 s on windows-latest,
//! decision #56), not a timer, ends the request. Fake key only; the transcript, the
//! reply, the prompt and the key must never reach the log.
#![cfg(windows)]

mod win32_support;

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use voicen_core::platform::FakeIndicator;
use voicen_core::post_process::settings::PostProcessingSettings;
use voicen_core::recording::TrayState;
use voicen_core::secrets::{KeyEdit, KeyEdits, Secret};
use voicen_core::settings::service::{SaveOutcome, SaveRequest};
use voicen_core::settings::EngineKind;
use voicen_core::test_support::fixtures;
use voicen_core::test_support::realtime::RealtimeSource;
use voicen_lib::dictation::{start_dictation, DictationPorts};
use voicen_lib::win::clipboard::WinClipboard;
use voicen_lib::win::paste::WinPaster;
use win32_support::{
    assert_hotkey_free, dictation_lines, engine_factory, eventually, has_pair, lock,
    read_clipboard, serial, shown, AppRig, ClipboardState, Keys, Shape, Shown, TestWindow,
    TextEngine, BUDGET, HOLD, HOTKEY_KEYS, SETTLE, VK_CONTROL, VK_ESCAPE, VK_MENU, VK_SPACE, WAIT,
};

/// The test engine's transcript: non-ASCII, and a canary the log must never contain.
const TRANSCRIPT: &str = "Voicen e2e TRANSCRIPT-CANARY-5d1 проверка";
const CANARY: &str = "TRANSCRIPT-CANARY-5d1";

#[test]
fn a_hotkey_hold_through_the_real_wiring_pastes_the_transcript_into_the_window() {
    // T-006 row 18 (Acceptance line 1; invariant 5): a synthetic Ctrl+Alt+Space hold of
    // 1.5 s from the observed press, with engine `api` (a test engine) and the WAV source
    // injected through `start_dictation`'s ports, ends with
    // - the transcript in the foreground test window's text (Ctrl+V into the start window);
    // - the clipboard holding it with CanIncludeInClipboardHistory = 0,
    //   CanUploadToCloudClipboard = 0 and ExcludeClipboardContentFromMonitorProcessing;
    // - exactly one `dictation … engine=api outcome=delivered result=pasted` line, and no
    //   log line containing the transcript;
    // - the overlay going Recording, Processing, Hidden (nothing else).
    // Bite: no wiring or no hotkey thread (nothing happens), a second observer (two
    // dictation lines), the injected ports not used (no paste), a transcript logged.
    let _serial = serial();
    assert_hotkey_free();
    let window = TestWindow::open(Shape::TopLevel);
    window.front();
    let rig = AppRig::new(EngineKind::Api);
    let engine = TextEngine::new(TRANSCRIPT);
    let indicator = Arc::new(FakeIndicator::new());
    let _dictation = start_dictation(
        rig.app.handle(),
        DictationPorts {
            audio: Arc::new(RealtimeSource::from_buffer(&fixtures::speech_3s())),
            clipboard: Arc::new(WinClipboard::new()),
            paster: Arc::new(WinPaster::new()),
            engine_factory: Some(engine_factory(&engine)),
            indicator: indicator.clone(),
            credentials: Arc::clone(&rig.creds),
            hotkeys: Arc::clone(&rig.hotkeys),
        },
    )
    .expect("start_dictation");

    let mut keys = Keys::press(&HOTKEY_KEYS);
    assert!(
        eventually(WAIT, || shown(&indicator.overlays())
            .contains(&Shown::Recording)),
        "no Recording within {WAIT:?} of the injected Ctrl+Alt+Space: {:?}",
        indicator.overlays()
    );
    thread::sleep(HOLD);
    keys.release(&[VK_SPACE, VK_CONTROL, VK_MENU]);

    assert!(
        eventually(BUDGET, || window.text() == TRANSCRIPT),
        "the window text {BUDGET:?} after the release: {:?} (engine calls {})",
        window.text(),
        engine.calls()
    );
    assert_eq!(
        read_clipboard(),
        ClipboardState {
            text: Some(TRANSCRIPT.to_string()),
            exclude_from_monitor: true,
            can_include_in_history: Some(0),
            can_upload_to_cloud: Some(0),
        }
    );
    assert!(
        eventually(WAIT, || !dictation_lines(&rig.log_lines()).is_empty()),
        "no dictation line in the log"
    );
    let lines = rig.log_lines();
    let dictation = dictation_lines(&lines);
    assert_eq!(dictation.len(), 1, "dictation lines: {dictation:?}");
    for (key, value) in [
        ("engine", "api"),
        ("outcome", "delivered"),
        ("result", "pasted"),
    ] {
        assert!(
            has_pair(&dictation[0], key, value),
            "{key}={value}: {}",
            dictation[0]
        );
    }
    let leaked: Vec<&String> = lines.iter().filter(|l| l.contains(CANARY)).collect();
    assert!(
        leaked.is_empty(),
        "the transcript reached the log: {leaked:?}"
    );
    assert!(
        eventually(WAIT, || shown(&indicator.overlays())
            == vec![Shown::Recording, Shown::Processing, Shown::Hidden]),
        "overlays: {:?}",
        shown(&indicator.overlays())
    );
    assert_eq!(engine.calls(), 1, "engine calls");
}

#[test]
fn esc_through_the_real_wiring_discards_the_recording() {
    // T-009 Acceptance "Esc cancel" through `start_dictation` (FR-22, FR-20): the wiring
    // attaches the Esc claim to the real hotkey thread; Esc during a Ctrl+Alt+Space hold
    // discards the recording: nothing reaches the window or the engine, the overlay goes
    // Recording, Hidden, and the log has exactly one `dictation … outcome=cancelled`
    // line. Bite: start_dictation not wiring a CancelKeyHandle into the session and the
    // thread (Esc does nothing; the hold is delivered at its release), a cancelled
    // recording without its dictation line.
    let _serial = serial();
    assert_hotkey_free();
    let window = TestWindow::open(Shape::TopLevel);
    window.front();
    let rig = AppRig::new(EngineKind::Api);
    let engine = TextEngine::new(TRANSCRIPT);
    let indicator = Arc::new(FakeIndicator::new());
    let _dictation = start_dictation(
        rig.app.handle(),
        DictationPorts {
            audio: Arc::new(RealtimeSource::from_buffer(&fixtures::speech_3s())),
            clipboard: Arc::new(WinClipboard::new()),
            paster: Arc::new(WinPaster::new()),
            engine_factory: Some(engine_factory(&engine)),
            indicator: indicator.clone(),
            credentials: Arc::clone(&rig.creds),
            hotkeys: Arc::clone(&rig.hotkeys),
        },
    )
    .expect("start_dictation");

    let mut keys = Keys::press(&HOTKEY_KEYS);
    assert!(
        eventually(WAIT, || shown(&indicator.overlays())
            .contains(&Shown::Recording)),
        "no Recording within {WAIT:?} of the injected Ctrl+Alt+Space: {:?}",
        indicator.overlays()
    );
    thread::sleep(HOLD);
    let mut esc = Keys::press(&[VK_ESCAPE]);
    assert!(
        eventually(WAIT, || shown(&indicator.overlays())
            == vec![Shown::Recording, Shown::Hidden]),
        "overlays after Esc: {:?}",
        shown(&indicator.overlays())
    );
    esc.release_all();
    keys.release(&[VK_SPACE, VK_CONTROL, VK_MENU]);

    assert!(
        eventually(WAIT, || !dictation_lines(&rig.log_lines()).is_empty()),
        "no dictation line for the cancelled recording"
    );
    thread::sleep(SETTLE);
    let dictation = dictation_lines(&rig.log_lines());
    assert_eq!(dictation.len(), 1, "dictation lines: {dictation:?}");
    assert!(
        has_pair(&dictation[0], "outcome", "cancelled"),
        "{}",
        dictation[0]
    );
    assert_eq!(engine.calls(), 0, "the cancelled audio reached the engine");
    assert_eq!(window.text(), "", "something was pasted");
    assert_eq!(
        shown(&indicator.overlays()),
        vec![Shown::Recording, Shown::Hidden]
    );
}

// ---- T-074: chat post-processing through the real wiring ----------------------------

/// The post-processing key: fake, and a canary the log must never contain.
const PP_KEY: &str = "sk-test-T074-PP-FAKE-KEY";
const PP_MODEL: &str = "gpt-test-mini";
/// The system prompt: a canary the log must never contain.
const PP_PROMPT: &str = "PROMPT-CANARY-T074 fix punctuation";
/// The mock's reply: non-ASCII, and a canary the log must never contain.
const REPLY: &str = "Voicen e2e REPLY-CANARY-74c, проверено.";
const REPLY_CANARY: &str = "REPLY-CANARY-74c";
/// The refusal budget of decision #56 (`voicen-core` tests/common `REFUSAL_BUDGET`).
const REFUSAL_BUDGET: Duration = Duration::from_secs(5);

/// One request the mock chat server read.
#[derive(Debug, Clone)]
struct ChatRequest {
    /// `POST /v1/chat/completions HTTP/1.1`.
    request_line: String,
    authorization: Option<String>,
    body: Vec<u8>,
}

/// A loopback OpenAI-compatible chat endpoint over raw TCP: every connection is read
/// (head plus `Content-Length` body), recorded and answered `200` with
/// `{"choices":[{"message":{"role":"assistant","content":REPLY}}]}`.
struct ChatMock {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<ChatRequest>>>,
}

impl ChatMock {
    fn start() -> ChatMock {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
        let addr = listener.local_addr().expect("local addr");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let seen = Arc::clone(&seen);
                thread::spawn(move || {
                    if let Ok(request) = answer_chat(stream) {
                        lock(&seen).push(request);
                    }
                });
            }
        });
        ChatMock { addr, requests }
    }

    /// The base URL the settings store (`http://127.0.0.1:<port>/v1`).
    fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    fn requests(&self) -> Vec<ChatRequest> {
        lock(&self.requests).clone()
    }
}

/// Reads one request and answers it with the fixed reply.
fn answer_chat(mut s: TcpStream) -> io::Result<ChatRequest> {
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
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
    let reply = serde_json::json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": REPLY },
            "finish_reason": "stop",
        }],
    })
    .to_string();
    write!(
        s,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    )?;
    s.flush()?;
    Ok(ChatRequest {
        request_line,
        authorization,
        body,
    })
}

/// `127.0.0.1:1`, the one loopback address a test may expect to be refused (decision
/// #53: below every ephemeral range, never bound and released). Probed at each call:
/// a plain connect must be refused within `REFUSAL_BUDGET / 2` (decision #56), else
/// this panics (a runner problem, not a product failure).
fn refused_loopback() -> SocketAddr {
    let addr = SocketAddr::from(([127, 0, 0, 1], 1));
    let started = Instant::now();
    let probe = TcpStream::connect_timeout(&addr, REFUSAL_BUDGET);
    let took = started.elapsed();
    match probe {
        Ok(_) => panic!("premise: something listens on {addr} (decision #53)"),
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
            assert!(
                took < REFUSAL_BUDGET / 2,
                "premise: the connect to {addr} was refused only after {took:?} (decision #56)"
            );
            addr
        }
        Err(e) => panic!(
            "premise: the connect to {addr} was not refused within {REFUSAL_BUDGET:?} \
             ({:?} after {took:?})",
            e.kind()
        ),
    }
}

/// Saves post-processing on, with `base_url`, the test model and prompt, connect 5 s
/// and post-processing 10 s, and stores [`PP_KEY`] through the save (the settings
/// service's own credential store, the one `start_dictation` gets).
fn enable_post_processing(rig: &AppRig, base_url: &str) {
    let mut s = (*rig.service.snapshot()).clone();
    s.post_processing = PostProcessingSettings {
        enabled: true,
        base_url: base_url.to_string(),
        model: PP_MODEL.to_string(),
        prompt: PP_PROMPT.to_string(),
    };
    s.timeouts.connect_s = 5;
    s.timeouts.post_processing_s = 10;
    match rig.service.save(SaveRequest {
        settings: s,
        keys: KeyEdits {
            post_processing: KeyEdit::Replace(Secret::new(PP_KEY)),
            ..KeyEdits::default()
        },
    }) {
        SaveOutcome::Saved { .. } => {}
        other => panic!("premise: saving post-processing on is refused: {other:?}"),
    }
    let saved = rig.service.snapshot();
    assert!(
        saved.post_processing.enabled && saved.post_processing.base_url == base_url,
        "premise: the saved post-processing settings: {:?}",
        saved.post_processing
    );
}

/// Starts the dictation of `rig` through `start_dictation` (the ports of the T-006
/// test) and makes one synthetic Ctrl+Alt+Space hold of [`HOLD`] from the observed
/// Recording. Returns the handle (dropping it frees the hotkey).
fn dictate_once(
    rig: &AppRig,
    engine: &Arc<TextEngine>,
    indicator: &Arc<FakeIndicator>,
) -> voicen_lib::dictation::DictationHandle {
    let dictation = start_dictation(
        rig.app.handle(),
        DictationPorts {
            audio: Arc::new(RealtimeSource::from_buffer(&fixtures::speech_3s())),
            clipboard: Arc::new(WinClipboard::new()),
            paster: Arc::new(WinPaster::new()),
            engine_factory: Some(engine_factory(engine)),
            indicator: indicator.clone(),
            credentials: Arc::clone(&rig.creds),
            hotkeys: Arc::clone(&rig.hotkeys),
        },
    )
    .expect("start_dictation");
    let mut keys = Keys::press(&HOTKEY_KEYS);
    assert!(
        eventually(WAIT, || shown(&indicator.overlays())
            .contains(&Shown::Recording)),
        "no Recording within {WAIT:?} of the injected Ctrl+Alt+Space: {:?}",
        indicator.overlays()
    );
    thread::sleep(HOLD);
    keys.release(&[VK_SPACE, VK_CONTROL, VK_MENU]);
    dictation
}

/// The log carries none of the canaries (FR-020, NFR-04, spec 003 FR-014).
fn assert_no_canary(lines: &[String]) {
    for canary in [CANARY, REPLY_CANARY, PP_KEY, "PROMPT-CANARY-T074"] {
        let leaked: Vec<&String> = lines.iter().filter(|l| l.contains(canary)).collect();
        assert!(leaked.is_empty(), "{canary:?} reached the log: {leaked:?}");
    }
}

/// The one `dictation` line of the log, `outcome=delivered result=pasted`.
fn assert_one_pasted_dictation_line(rig: &AppRig) {
    assert!(
        eventually(WAIT, || !dictation_lines(&rig.log_lines()).is_empty()),
        "no dictation line in the log"
    );
    let dictation = dictation_lines(&rig.log_lines());
    assert_eq!(dictation.len(), 1, "dictation lines: {dictation:?}");
    for (key, value) in [("outcome", "delivered"), ("result", "pasted")] {
        assert!(
            has_pair(&dictation[0], key, value),
            "{key}={value}: {}",
            dictation[0]
        );
    }
}

#[test]
fn post_processing_on_through_the_real_wiring_pastes_the_chat_reply() {
    // T-074 Acceptance line 1: with post-processing enabled and configured (base URL =
    // the loopback chat mock, a key stored through the save), a hotkey hold through
    // `start_dictation` and the real hotkey thread ends with
    // - the mock's reply, not the transcript, in the foreground test window and on the
    //   clipboard;
    // - exactly one request at the mock: `POST /v1/chat/completions`, `Bearer PP_KEY`,
    //   body {model, messages: [system PP_PROMPT, user TRANSCRIPT]};
    // - no skip: overlays Recording, Processing, Hidden and no tray Error;
    // - one `dictation … outcome=delivered result=pasted` line and none of the
    //   transcript, reply, prompt or key in the log.
    // Bite: PassThrough in start_dictation (the transcript is pasted, no request), a
    // processor other than the settings' (no request / another URL), the key not read
    // from the session's credential store (no Authorization), the raw text delivered
    // after an Applied reply.
    let _serial = serial();
    assert_hotkey_free();
    let mock = ChatMock::start();
    let window = TestWindow::open(Shape::TopLevel);
    window.front();
    let rig = AppRig::new(EngineKind::Api);
    enable_post_processing(&rig, &mock.base_url());
    let engine = TextEngine::new(TRANSCRIPT);
    let indicator = Arc::new(FakeIndicator::new());
    let _dictation = dictate_once(&rig, &engine, &indicator);

    assert!(
        eventually(BUDGET, || window.text() == REPLY),
        "the window text {BUDGET:?} after the release: {:?} (expected the chat reply; \
         engine calls {}, chat requests {})",
        window.text(),
        engine.calls(),
        mock.requests().len()
    );
    assert_eq!(
        read_clipboard().text.as_deref(),
        Some(REPLY),
        "clipboard text"
    );
    assert_one_pasted_dictation_line(&rig);

    let requests = mock.requests();
    assert_eq!(requests.len(), 1, "chat requests: {requests:?}");
    let request = &requests[0];
    assert_eq!(request.request_line, "POST /v1/chat/completions HTTP/1.1");
    assert_eq!(
        request.authorization.as_deref(),
        Some(format!("Bearer {PP_KEY}").as_str()),
        "Authorization"
    );
    let body: serde_json::Value =
        serde_json::from_slice(&request.body).expect("the chat request body is JSON");
    assert_eq!(body["model"], PP_MODEL, "model: {body}");
    assert_eq!(
        body["messages"],
        serde_json::json!([
            { "role": "system", "content": PP_PROMPT },
            { "role": "user", "content": TRANSCRIPT },
        ]),
        "messages"
    );

    assert!(
        eventually(WAIT, || shown(&indicator.overlays())
            == vec![Shown::Recording, Shown::Processing, Shown::Hidden]),
        "overlays: {:?}",
        shown(&indicator.overlays())
    );
    let trays = indicator.trays();
    assert!(
        !trays.iter().any(|(state, _)| *state == TrayState::Error),
        "an applied post-processing set tray Error: {trays:?}"
    );
    assert_eq!(engine.calls(), 1, "engine calls");
    assert_no_canary(&rig.log_lines());
}

#[test]
fn post_processing_server_refusing_still_pastes_the_raw_transcript_with_tray_error() {
    // T-074 Acceptance line 2 (failure branch): post-processing enabled with base URL =
    // a loopback port that refuses (`127.0.0.1:1`) ends with
    // - the raw transcript, byte for byte, in the test window and on the clipboard;
    // - the skip reason reaching the session: overlay Recording, Processing, then
    //   `notice.post_processing_skipped.unreachable` with host=127.0.0.1:1, and tray
    //   Error (decision #91(3));
    // - one `dictation … outcome=delivered result=pasted` line (a skip is a delivery)
    //   and none of the transcript, prompt or key in the log.
    // Bite: PassThrough in start_dictation (NotRun: no message, no Error), a skip that
    // fails the dictation (nothing pasted, outcome=failed), the reason lost (another
    // message or no host).
    let _serial = serial();
    assert_hotkey_free();
    let refused = refused_loopback();
    let window = TestWindow::open(Shape::TopLevel);
    window.front();
    let rig = AppRig::new(EngineKind::Api);
    enable_post_processing(&rig, &format!("http://{refused}/v1"));
    let engine = TextEngine::new(TRANSCRIPT);
    let indicator = Arc::new(FakeIndicator::new());
    let _dictation = dictate_once(&rig, &engine, &indicator);

    assert!(
        eventually(BUDGET, || window.text() == TRANSCRIPT),
        "the window text {BUDGET:?} after the release: {:?} (expected the raw transcript; \
         engine calls {})",
        window.text(),
        engine.calls()
    );
    assert_eq!(
        read_clipboard().text.as_deref(),
        Some(TRANSCRIPT),
        "clipboard text"
    );
    let skipped = Shown::Message(
        "notice.post_processing_skipped.unreachable",
        vec![("host", refused.to_string())],
    );
    assert!(
        eventually(WAIT, || shown(&indicator.overlays()).contains(&skipped)),
        "no skip message {skipped:?}; overlays: {:?}",
        shown(&indicator.overlays())
    );
    let overlays = shown(&indicator.overlays());
    assert_eq!(
        overlays.get(..3),
        Some(&[Shown::Recording, Shown::Processing, skipped.clone()][..]),
        "overlays: {overlays:?}"
    );
    assert!(
        eventually(WAIT, || indicator
            .trays()
            .last()
            .is_some_and(|(state, _)| *state == TrayState::Error)),
        "tray after the skip: {:?}",
        indicator.trays()
    );
    assert_one_pasted_dictation_line(&rig);
    assert_eq!(engine.calls(), 1, "engine calls");
    assert_no_canary(&rig.log_lines());
}
