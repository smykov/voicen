//! T-046: the connection test (spec 004 US4, FR-017, FR-018; research R-9; T-013
//! analysis and Investigation = the design, decision #51; T-046 analysis option A
//! and Investigation choices (i)-(iii) as accepted by the orchestrator).
//!
//! Every test goes through the one public entry, `SettingsService::test_connection`
//! (form timeouts converted by core), or its test-only twin
//! `test_connection_with_timeouts` (durations injected, like
//! `Pipeline::with_timeouts`), over the real service loaded from a fake settings
//! file and a fake credential store. The service's snapshot always points
//! elsewhere than the form (engine `none`, other URLs, other model), so a tester
//! that reads the snapshot instead of the request never reaches the mock.
//!
//! wiremock serves on its own thread and runtime; the blocking call runs on a plain
//! std thread (the blocking reqwest client panics inside a tokio runtime context,
//! decision #42). Refused cases use `refused_addr()` / `refused_timeouts()` (F-004,
//! F-005; docs/decisions/core-tests.md); timeout cases use a server that accepts
//! and answers late. The "connection not accepted in time" row (connect timeout ->
//! CannotReach, Investigation (ii)) has no deterministic local fixture and is
//! pinned by `failure::tests::classify_table` through `classify`.
//! Fake data only: 127.0.0.1, `.invalid` (RFC 6761), `example.com`, `sk-test-…`.

mod common;
/// The checks of `common::refused_addr()`, in each binary that calls it (T-047).
#[path = "common/refused_addr_tests.rs"]
mod refused_addr_tests;

use std::io;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, UNIX_EPOCH};

use common::{refused_addr, refused_timeouts};
use serde_json::json;
use voicen_core::autostart::FakeAutostart;
use voicen_core::clock::FakeClock;
use voicen_core::connection_test::{
    request_timeouts, ConnectionTestRequest, ConnectionTestResult, TestEngine,
};
use voicen_core::hotkey_registrar::FakeHotkeyRegistrar;
use voicen_core::models::FakeDownloadedModels;
use voicen_core::secrets::{
    CredentialCall, CredentialError, CredentialOp, FakeCredentialStore, KeyEdit, KeySlot, Secret,
};
use voicen_core::settings::file::{FakeSettingsFile, FileCall};
use voicen_core::settings::service::{SettingsDeps, SettingsService};
use voicen_core::settings::{
    defaults, EngineKind, ErrorCode, FieldError, FieldId, Settings, TimeoutSettings,
};
use voicen_core::timeouts::{default_settings, Timeouts};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The key stored in the transcription-API slot.
const STORED_API_KEY: &str = "sk-test-STORED-API";
/// The key stored in the local-server slot.
const STORED_LOCAL_KEY: &str = "sk-test-STORED-LOCAL";
/// A key typed in the form.
const TYPED_KEY: &str = "sk-test-TYPED";
/// A secret in a URL query string (decision #27(2)): never in a result.
const QUERY_SECRET: &str = "SECRETQ";
const OS: Option<&str> = Some("en-US");
/// The model the form names; the snapshot names another one.
const FORM_MODEL: &str = "whisper-form";
const SNAPSHOT_MODEL: &str = "SNAPSHOT-MODEL";

// ---- helpers ------------------------------------------------------------------

/// The settings in force (the service's snapshot): engine `none`, every URL and
/// model different from any form below, default timeouts. A tester that builds
/// its request from these never reaches a mock server.
fn stored_settings() -> Settings {
    let mut s = defaults(OS);
    s.engine = EngineKind::None;
    s.api.base_url = "https://api.example.com/v1".to_string();
    s.api.model = SNAPSHOT_MODEL.to_string();
    s.local_server.base_url = "http://192.0.2.10:8000/v1".to_string();
    s.local_server.model = SNAPSHOT_MODEL.to_string();
    s
}

/// The keys stored in both engine slots (the post-processing slot is empty).
fn stored_keys() -> FakeCredentialStore {
    FakeCredentialStore::new()
        .with_key(KeySlot::TranscriptionApi, STORED_API_KEY)
        .with_key(KeySlot::LocalServer, STORED_LOCAL_KEY)
}

/// The real service over fakes, and the fakes to observe it with.
struct World {
    service: Arc<SettingsService>,
    file: Arc<FakeSettingsFile>,
    creds: Arc<FakeCredentialStore>,
}

fn deps(file: Arc<FakeSettingsFile>, creds: Arc<FakeCredentialStore>) -> SettingsDeps {
    SettingsDeps {
        file,
        credentials: creds,
        autostart: Arc::new(FakeAutostart::new()),
        hotkeys: Arc::new(FakeHotkeyRegistrar::new()),
        local_models: Arc::new(FakeDownloadedModels::new(&["base"])),
        clock: Arc::new(FakeClock::at(
            UNIX_EPOCH + Duration::from_secs(1_709_251_199),
        )),
    }
}

/// A `Loaded` service whose file holds `stored`.
fn world_with(stored: &Settings, creds: FakeCredentialStore) -> World {
    let bytes = serde_json::to_vec(stored).expect("serialize settings");
    let file = Arc::new(FakeSettingsFile::with_bytes(&bytes));
    let creds = Arc::new(creds);
    let (service, _) = SettingsService::load_or_init(deps(file.clone(), creds.clone()), OS);
    World {
        service: Arc::new(service),
        file,
        creds,
    }
}

fn world(creds: FakeCredentialStore) -> World {
    world_with(&stored_settings(), creds)
}

/// An `Unavailable` service (decision #19): the file read fails with an I/O error.
fn unavailable_world(creds: FakeCredentialStore) -> World {
    let file = Arc::new(FakeSettingsFile::new());
    file.fail_read(io::ErrorKind::PermissionDenied);
    let creds = Arc::new(creds);
    let (service, _) = SettingsService::load_or_init(deps(file.clone(), creds.clone()), OS);
    World {
        service: Arc::new(service),
        file,
        creds,
    }
}

/// Form timeouts: connect, API and local-server as given, post-processing and
/// built-in at their defaults.
fn form_timeouts(connect_s: u32, api_transcription_s: u32, local_server_s: u32) -> TimeoutSettings {
    TimeoutSettings {
        connect_s,
        api_transcription_s,
        local_server_s,
        ..default_settings()
    }
}

fn api_req(base_url: &str, key: KeyEdit) -> ConnectionTestRequest {
    ConnectionTestRequest {
        engine: TestEngine::Api,
        base_url: base_url.to_string(),
        model: FORM_MODEL.to_string(),
        key,
        timeouts: default_settings(),
    }
}

fn local_req(base_url: &str, model: &str, key: KeyEdit) -> ConnectionTestRequest {
    ConnectionTestRequest {
        engine: TestEngine::LocalServer,
        base_url: base_url.to_string(),
        model: model.to_string(),
        key,
        timeouts: default_settings(),
    }
}

fn replace(key: &str) -> KeyEdit {
    KeyEdit::Replace(Secret::new(key))
}

/// `test_connection` on a plain std thread (no runtime context there).
fn run(w: &World, req: ConnectionTestRequest) -> ConnectionTestResult {
    let service = w.service.clone();
    std::thread::spawn(move || service.test_connection(req))
        .join()
        .expect("test_connection must not panic")
}

/// `test_connection_with_timeouts` on a plain std thread.
fn run_with(w: &World, req: ConnectionTestRequest, timeouts: Timeouts) -> ConnectionTestResult {
    let service = w.service.clone();
    std::thread::spawn(move || service.test_connection_with_timeouts(req, timeouts))
        .join()
        .expect("test_connection_with_timeouts must not panic")
}

/// Short test durations for a mock that answers at once (never the defaults).
fn quick() -> Timeouts {
    Timeouts {
        connect: Duration::from_secs(2),
        api_transcription: Duration::from_secs(5),
        local_server: Duration::from_secs(5),
        ..Timeouts::default()
    }
}

async fn server_with(response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

fn ok_text(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "text": text }))
}

fn base(server: &MockServer) -> String {
    format!("{}/v1", server.uri())
}

async fn received(server: &MockServer) -> Vec<wiremock::Request> {
    server
        .received_requests()
        .await
        .expect("request recording is on")
}

async fn only_request(server: &MockServer) -> wiremock::Request {
    let mut requests = received(server).await;
    assert_eq!(requests.len(), 1, "exactly one request (no retries)");
    requests.remove(0)
}

fn header<'a>(r: &'a wiremock::Request, name: &str) -> Option<&'a str> {
    r.headers.get(name).and_then(|v| v.to_str().ok())
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// The `(name, body)` parts of a `multipart/form-data` request body (RFC 7578).
fn multipart(r: &wiremock::Request) -> Vec<(String, Vec<u8>)> {
    let content_type = header(r, "content-type").expect("Content-Type header");
    assert!(
        content_type.starts_with("multipart/form-data"),
        "Content-Type {content_type}"
    );
    let boundary = content_type
        .split(';')
        .map(str::trim)
        .find_map(|p| p.strip_prefix("boundary="))
        .expect("multipart boundary")
        .trim_matches('"');
    let delim = format!("--{boundary}").into_bytes();
    let next_delim = [b"\r\n".as_slice(), delim.as_slice()].concat();
    let body = &r.body;
    let mut parts = Vec::new();
    let mut pos = find(body, &delim, 0).expect("first boundary");
    loop {
        let after = pos + delim.len();
        if body.get(after..after + 2) == Some(b"--".as_slice()) {
            break;
        }
        let head_start = after + 2;
        let head_end = find(body, b"\r\n\r\n", head_start).expect("end of part headers");
        let head = String::from_utf8_lossy(&body[head_start..head_end]).into_owned();
        let content_end = find(body, &next_delim, head_end + 4).expect("next boundary");
        let name = head
            .split("\r\n")
            .filter_map(|line| line.split_once(':'))
            .filter(|(k, _)| k.trim().eq_ignore_ascii_case("content-disposition"))
            .flat_map(|(_, v)| v.split(';').map(str::trim).collect::<Vec<_>>())
            .find_map(|p| {
                p.strip_prefix("name=")
                    .map(|n| n.trim_matches('"').to_string())
            })
            .unwrap_or_default();
        parts.push((name, body[head_end + 4..content_end].to_vec()));
        pos = content_end + 2;
    }
    parts
}

fn part_names(parts: &[(String, Vec<u8>)]) -> Vec<String> {
    let mut names: Vec<String> = parts.iter().map(|(n, _)| n.clone()).collect();
    names.sort_unstable();
    names
}

fn part<'a>(parts: &'a [(String, Vec<u8>)], name: &str) -> Option<&'a [u8]> {
    parts
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, b)| b.as_slice())
}

fn part_text(parts: &[(String, Vec<u8>)], name: &str) -> Option<String> {
    part(parts, name).map(|b| String::from_utf8_lossy(b).into_owned())
}

fn read_call(slot: KeySlot) -> CredentialCall {
    CredentialCall {
        op: CredentialOp::Read,
        slot,
    }
}

/// The `Ok` latency, or a panic naming the result.
#[track_caller]
fn latency_of(result: &ConnectionTestResult) -> u128 {
    match result {
        ConnectionTestResult::Ok { latency_ms } => *latency_ms as u128,
        other => panic!("expected Ok, got {other:?}"),
    }
}

/// The errors of an `Invalid` result, sorted by field id then code, or a panic.
#[track_caller]
fn invalid_errors(result: &ConnectionTestResult) -> Vec<(&'static str, &'static str)> {
    match result {
        ConnectionTestResult::Invalid { errors } => {
            let mut e: Vec<(&'static str, &'static str)> = errors
                .iter()
                .map(|e| (e.field.as_str(), e.code.as_str()))
                .collect();
            e.sort_unstable();
            e
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}

/// No key, no URL query and no response body text in the result's `Debug` or its
/// wire form (FR-017: never contains a key or the response body; NFR-04).
#[track_caller]
fn assert_clean(result: &ConnectionTestResult, also: &[&str]) {
    let debug = format!("{result:?}");
    let wire = serde_json::to_string(result).expect("ConnectionTestResult serializes");
    for text in [&debug, &wire] {
        for needle in [
            STORED_API_KEY,
            STORED_LOCAL_KEY,
            TYPED_KEY,
            QUERY_SECRET,
            "sk-test",
        ]
        .iter()
        .chain(also)
        {
            assert!(!text.contains(needle), "{needle:?} in result {text}");
        }
    }
}

/// What a test must leave as it found it: the file (no call at all after the
/// load), the snapshot (same `Arc`, same value), the subscribers (nothing
/// published) and the credential store (reads only, every key still stored).
struct Untouched {
    file_calls: Vec<FileCall>,
    snapshot: Arc<Settings>,
    changes: mpsc::Receiver<Arc<Settings>>,
    stored: [Option<String>; 3],
}

fn before(w: &World) -> Untouched {
    Untouched {
        file_calls: w.file.calls(),
        snapshot: w.service.snapshot(),
        changes: w.service.subscribe(),
        stored: stored_now(&w.creds),
    }
}

fn stored_now(creds: &FakeCredentialStore) -> [Option<String>; 3] {
    KeySlot::all().map(|slot| creds.stored(slot))
}

impl Untouched {
    #[track_caller]
    fn check(self, w: &World) {
        assert_eq!(
            w.file.calls(),
            self.file_calls,
            "the settings file was used"
        );
        let now = w.service.snapshot();
        assert!(
            Arc::ptr_eq(&now, &self.snapshot),
            "the snapshot was replaced"
        );
        assert_eq!(*now, *self.snapshot, "the snapshot changed");
        assert!(
            self.changes.try_recv().is_err(),
            "a snapshot was published to subscribers"
        );
        for call in w.creds.calls() {
            assert_eq!(call.op, CredentialOp::Read, "credential store: {call:?}");
        }
        assert_eq!(stored_now(&w.creds), self.stored, "a stored key changed");
    }
}

/// Parsed header of the canonical 44-byte PCM WAV the client sends:
/// (channels, sample rate, bits per sample, samples).
fn wav_info(wav: &[u8]) -> (u16, u32, u16, Vec<i16>) {
    assert!(
        wav.len() >= 44,
        "WAV shorter than its header: {} bytes",
        wav.len()
    );
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    let u16_at = |i: usize| u16::from_le_bytes([wav[i], wav[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([wav[i], wav[i + 1], wav[i + 2], wav[i + 3]]);
    let samples = wav[44..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes(*c))
        .collect();
    (u16_at(22), u32_at(24), u16_at(34), samples)
}

// ---- the form's timeouts (FR-017, R-9, T-073 analysis; Investigation) -----------

#[test]
fn request_timeouts_are_the_forms_values() {
    // FR-017 / R-9: the connect and request limits are the form's `timeouts`, as
    // entered, through the one conversion `Timeouts::from_settings`. Distinct
    // in-range values per role, none equal to a default or to another role. Bite:
    // Timeouts::default() (5/30/60), the snapshot's values, two roles swapped,
    // milliseconds instead of seconds, or a second conversion that skips a field.
    let form = TimeoutSettings {
        connect_s: 7,
        api_transcription_s: 45,
        local_server_s: 90,
        post_processing_s: 20,
        builtin_local_s: 150,
    };
    for engine in [TestEngine::Api, TestEngine::LocalServer] {
        let req = ConnectionTestRequest {
            engine,
            base_url: "http://localhost:8000/v1".to_string(),
            model: String::new(),
            key: KeyEdit::Untouched,
            timeouts: form,
        };
        let t = request_timeouts(&req);
        assert_eq!(t.connect, Duration::from_secs(7), "connect");
        assert_eq!(t.api_transcription, Duration::from_secs(45), "api");
        assert_eq!(t.local_server, Duration::from_secs(90), "local server");
        assert_eq!(t, Timeouts::from_settings(&form), "one conversion site");
    }
}

#[test]
fn request_timeouts_clamp_like_from_settings() {
    // R-9: "clamped by Timeouts::from_settings" (the guard behind choice (i)). A 0
    // must not reach a request (every request would fail at once) and u32::MAX must
    // not wait forever. Bite: the raw seconds used without the clamp.
    let form = TimeoutSettings {
        connect_s: 0,
        api_transcription_s: u32::MAX,
        local_server_s: 0,
        ..default_settings()
    };
    let req = ConnectionTestRequest {
        engine: TestEngine::Api,
        base_url: "http://localhost:8000/v1".to_string(),
        model: FORM_MODEL.to_string(),
        key: KeyEdit::Untouched,
        timeouts: form,
    };
    let t = request_timeouts(&req);
    assert_eq!(t, Timeouts::from_settings(&form));
    assert_eq!(t.connect, Duration::from_secs(1));
    assert_eq!(t.api_transcription, Duration::from_secs(600));
    assert_eq!(t.local_server, Duration::from_secs(5));
}

#[tokio::test]
async fn local_server_test_uses_the_forms_local_server_limit() {
    // US4-5 / FR-017 / SC-006: the local-server test's request limit is the form's
    // `local_server_s` (5 s), not the API limit (600 s here), the snapshot's (60 s)
    // or the default (60 s). A server that answers after 6 s is a Timeout, and not
    // before ~5 s (a hard-coded short limit is not the form's either). Through the
    // public entry, which takes no durations. Bite: Timeout::default() or the
    // snapshot's timeouts (Ok after 6 s), the API deadline (Ok after 6 s), a fixed
    // test deadline (Timeout far too early).
    let server = server_with(ok_text("late").set_delay(Duration::from_secs(6))).await;
    let w = world(stored_keys());
    let mut req = local_req(&base(&server), "", KeyEdit::Untouched);
    req.timeouts = form_timeouts(5, 600, 5);
    let started = Instant::now();
    let got = run(&w, req);
    let elapsed = started.elapsed();
    assert_eq!(got, ConnectionTestResult::Timeout);
    // No upper bound (T-046 review 1 #2): every longer wrong limit (the snapshot's,
    // the default, the other role's) lets the 6 s answer through as Ok, so the
    // Timeout assertion catches it; a ceiling caught nothing more and tripped
    // under host load (T-078 class).
    assert!(
        elapsed >= Duration::from_millis(4500),
        "Timeout after {elapsed:?}, expected the form's 5 s"
    );
}

#[tokio::test]
async fn api_test_uses_the_forms_api_limit() {
    // FR-017: the API test's request limit is the form's `api_transcription_s`
    // (5 s), not the local-server limit (1800 s here), the snapshot's (30 s) or the
    // default (30 s). Bite: as above, for the API role.
    let server = server_with(ok_text("late").set_delay(Duration::from_secs(6))).await;
    let w = world(stored_keys());
    let mut req = api_req(&base(&server), KeyEdit::Untouched);
    req.timeouts = form_timeouts(5, 5, 1800);
    let started = Instant::now();
    let got = run(&w, req);
    let elapsed = started.elapsed();
    assert_eq!(got, ConnectionTestResult::Timeout);
    // No upper bound (T-046 review 1 #2): every longer wrong limit (the snapshot's,
    // the default, the other role's) lets the 6 s answer through as Ok, so the
    // Timeout assertion catches it; a ceiling caught nothing more and tripped
    // under host load (T-078 class).
    assert!(
        elapsed >= Duration::from_millis(4500),
        "Timeout after {elapsed:?}, expected the form's 5 s"
    );
}

#[tokio::test]
async fn saved_limit_does_not_override_the_forms_limit() {
    // Spec edge case "the result refers to the values at the click": unsaved form
    // timeouts win over the saved ones. The saved API and local-server limits are
    // 5 s; the form says 30 s; a 6 s answer is Ok. Bite: the snapshot's timeouts
    // (Timeout at 5 s), or the shorter of the two.
    let server = server_with(ok_text("slow but fine").set_delay(Duration::from_secs(6))).await;
    let mut stored = stored_settings();
    stored.timeouts = form_timeouts(5, 5, 5);
    let w = world_with(&stored, stored_keys());
    let mut req = api_req(&base(&server), KeyEdit::Untouched);
    req.timeouts = form_timeouts(5, 30, 60);
    let got = run(&w, req);
    assert!(latency_of(&got) >= 6000, "{got:?}");
}

#[tokio::test]
async fn injected_short_timeout_is_timeout() {
    // T052: no answer within an injected short total timeout -> Timeout, well
    // before the server's 5 s delay. Bite: the injected durations ignored
    // (test_connection_with_timeouts calling the form conversion), or a timeout
    // mapped to CannotReach / UnexpectedResponse.
    let server = server_with(ok_text("late").set_delay(Duration::from_secs(5))).await;
    let w = world(stored_keys());
    let t = Timeouts {
        api_transcription: Duration::from_millis(300),
        ..quick()
    };
    let started = Instant::now();
    let got = run_with(&w, api_req(&base(&server), KeyEdit::Untouched), t);
    assert_eq!(got, ConnectionTestResult::Timeout);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

// ---- out-of-bounds form timeouts (choice (i)) ------------------------------------

#[tokio::test]
async fn out_of_range_limits_the_test_uses_are_invalid_and_send_nothing() {
    // Choice (i): a form limit outside the #99 bounds that the test would use (the
    // connect limit and the selected engine's request limit) is `Invalid` with the
    // save's `timeout.range` on that field, no request, no credential call. Bite:
    // silent clamping (a request is sent), the error on another field, or the
    // check left to the save only.
    let server = server_with(ok_text("hello")).await;
    let w = world(stored_keys());
    let guard = before(&w);

    let mut req = api_req(&base(&server), replace(TYPED_KEY));
    req.timeouts = form_timeouts(0, 2, 60);
    let got = run(&w, req);
    assert_eq!(
        invalid_errors(&got),
        vec![
            ("timeouts.api_transcription", "timeout.range"),
            ("timeouts.connect", "timeout.range"),
        ]
    );
    assert_clean(&got, &[]);

    let mut req = local_req(&base(&server), "", replace(TYPED_KEY));
    req.timeouts = form_timeouts(5, 30, 4);
    let got = run(&w, req);
    assert_eq!(
        invalid_errors(&got),
        vec![("timeouts.local_server", "timeout.range")]
    );

    assert!(received(&server).await.is_empty(), "a request was sent");
    assert_eq!(w.creds.calls(), vec![], "no credential call");
    guard.check(&w);
}

#[tokio::test]
async fn out_of_range_limits_the_test_does_not_use_are_ignored() {
    // Choice (i) keeps only the fields the test uses: the other engine's limit,
    // post-processing and built-in are not part of this request. Bite: the whole
    // `validate` error list returned (Invalid on a field the user cannot fix from
    // this test), or no filter by engine.
    let server = server_with(ok_text("hello")).await;
    let w = world(stored_keys());

    let mut req = api_req(&base(&server), replace(TYPED_KEY));
    req.timeouts = TimeoutSettings {
        local_server_s: 0,
        post_processing_s: 0,
        builtin_local_s: 0,
        ..form_timeouts(5, 30, 60)
    };
    let got = run(&w, req);
    latency_of(&got);

    let mut req = local_req(&base(&server), "", replace(TYPED_KEY));
    req.timeouts = TimeoutSettings {
        api_transcription_s: 0,
        post_processing_s: 0,
        builtin_local_s: 0,
        ..form_timeouts(5, 30, 60)
    };
    let got = run(&w, req);
    latency_of(&got);
    assert_eq!(received(&server).await.len(), 2);
}

#[tokio::test]
async fn invalid_saved_fields_outside_the_test_do_not_refuse_it() {
    // The snapshot may hold values a save would refuse (a hand-edited file is not
    // validated on load): a bad history size, an unsupported speech language. The
    // test validates only the fields it uses. Bite: `validate` over the overlaid
    // settings with every error kept.
    let server = server_with(ok_text("hello")).await;
    let mut stored = stored_settings();
    stored.history.size = 0;
    stored.speech_language = Some("xx".to_string());
    let w = world_with(&stored, stored_keys());
    let got = run(&w, api_req(&base(&server), KeyEdit::Untouched));
    latency_of(&got);
}

// ---- 200 -> Ok, the request shape -------------------------------------------------

#[tokio::test]
async fn ok_reports_latency_at_least_the_server_delay_and_saves_nothing() {
    // US4-1: a valid answer -> Ok with the latency in ms, measured around the
    // request (Instant, #47(2)): at least the server's 200 ms delay and at most the
    // call's own duration. Nothing saved: no file call, same snapshot, nothing
    // published, the store sees only one read of the API slot (Untouched). Bite: a
    // constant or missing latency, a save() inside the test, a key write.
    let server = server_with(ok_text("hello").set_delay(Duration::from_millis(200))).await;
    let w = world(stored_keys());
    let guard = before(&w);
    let started = Instant::now();
    let got = run(&w, api_req(&base(&server), KeyEdit::Untouched));
    let elapsed = started.elapsed().as_millis();
    let latency = latency_of(&got);
    assert!(latency >= 200, "latency {latency} ms < the 200 ms delay");
    assert!(
        latency <= elapsed,
        "latency {latency} ms > the call's {elapsed} ms"
    );
    assert_clean(&got, &[]);
    only_request(&server).await;
    assert_eq!(w.creds.calls(), vec![read_call(KeySlot::TranscriptionApi)]);
    guard.check(&w);
}

#[tokio::test]
async fn ok_with_empty_text_is_ok() {
    // R-9: any 2xx with a valid transcription body, even an empty text (silence
    // transcribed as nothing), is Ok. Bite: an empty transcript mapped to a
    // failure (the dictation path's "no speech" rule copied in).
    let server = server_with(ok_text("")).await;
    let w = world(stored_keys());
    let got = run(&w, api_req(&base(&server), KeyEdit::Untouched));
    latency_of(&got);
}

#[tokio::test]
async fn api_request_is_the_dictation_request_built_from_the_form() {
    // R-9: POST {form base}/audio/transcriptions through the dictation client:
    // the form's URL with its query kept (#27(2), so the one URL joiner is used),
    // the form's model trimmed (normalize), `response_format=json`, no language
    // (the test is language-free even though the snapshot sets one), the stored
    // API key as Bearer (Untouched), and the bundled ~1 s 16 kHz mono clip, not
    // silent. Bite: the snapshot's URL / model, the query dropped, a GET /models
    // probe, `language` sent, an empty or silent clip, a second client.
    let server = server_with(ok_text("hello")).await;
    let mut stored = stored_settings();
    stored.speech_language = Some("de".to_string());
    let w = world_with(&stored, stored_keys());
    let mut req = api_req(
        &format!("{}?api-version={QUERY_SECRET}", base(&server)),
        KeyEdit::Untouched,
    );
    req.model = format!("  {FORM_MODEL}  ");
    let got = run(&w, req);
    latency_of(&got);
    assert_clean(&got, &[]);

    let r = only_request(&server).await;
    assert_eq!(r.method.as_str(), "POST");
    assert_eq!(r.url.path(), "/v1/audio/transcriptions");
    assert_eq!(
        r.url.query(),
        Some(format!("api-version={QUERY_SECRET}").as_str())
    );
    let expected_auth = format!("Bearer {STORED_API_KEY}");
    assert_eq!(header(&r, "authorization"), Some(expected_auth.as_str()));
    let parts = multipart(&r);
    assert_eq!(part_names(&parts), ["file", "model", "response_format"]);
    assert_eq!(part_text(&parts, "model").as_deref(), Some(FORM_MODEL));
    assert_eq!(
        part_text(&parts, "response_format").as_deref(),
        Some("json")
    );

    let (channels, rate, bits, samples) = wav_info(part(&parts, "file").expect("file part"));
    assert_eq!((channels, rate, bits), (1, 16_000, 16), "16 kHz mono PCM16");
    assert!(
        (8_000..=24_000).contains(&samples.len()),
        "clip of {} samples, expected about 1 s (16 000)",
        samples.len()
    );
    let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    assert!(peak >= 1_000, "clip is silent (peak {peak})");
}

#[tokio::test]
async fn local_server_without_key_or_model_sends_neither() {
    // US4-5 / T052: the local server with an empty model and no stored key: no
    // Authorization header and no `model` part (T-018's engine_for arm). The API
    // slot's key never reaches the local server. Bite: the API slot read, `model`
    // sent empty, the API arm used for the local engine.
    let server = server_with(ok_text("hello")).await;
    let w = world(FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, STORED_API_KEY));
    let got = run(&w, local_req(&base(&server), "   ", KeyEdit::Untouched));
    latency_of(&got);
    let r = only_request(&server).await;
    assert_eq!(r.url.path(), "/v1/audio/transcriptions");
    assert_eq!(header(&r, "authorization"), None);
    assert_eq!(part_names(&multipart(&r)), ["file", "response_format"]);
    assert_eq!(w.creds.calls(), vec![read_call(KeySlot::LocalServer)]);
}

#[tokio::test]
async fn local_server_sends_the_forms_model_and_stored_local_key() {
    // Bite: the snapshot's local model, the API slot's key, the model untrimmed.
    let server = server_with(ok_text("hello")).await;
    let w = world(stored_keys());
    let got = run(
        &w,
        local_req(&base(&server), "  local-form-model ", KeyEdit::Untouched),
    );
    latency_of(&got);
    let r = only_request(&server).await;
    let expected = format!("Bearer {STORED_LOCAL_KEY}");
    assert_eq!(header(&r, "authorization"), Some(expected.as_str()));
    assert_eq!(
        part_text(&multipart(&r), "model").as_deref(),
        Some("local-form-model")
    );
    assert_eq!(w.creds.calls(), vec![read_call(KeySlot::LocalServer)]);
}

// ---- the key rule (#30, #33(a), the save's key_step) ------------------------------

#[tokio::test]
async fn typed_key_is_sent_trimmed_without_reading_the_store() {
    // #33(a): a typed key is used trimmed; the stored key is neither read in its
    // place nor overwritten. Bite: the untrimmed key sent, the stored key sent
    // instead, the typed key written to the store.
    let server = server_with(ok_text("hello")).await;
    for (engine, slot) in [
        (TestEngine::Api, KeySlot::TranscriptionApi),
        (TestEngine::LocalServer, KeySlot::LocalServer),
    ] {
        server.reset().await;
        Mock::given(any())
            .respond_with(ok_text("hello"))
            .mount(&server)
            .await;
        let w = world(stored_keys());
        let guard = before(&w);
        let mut req = api_req(&base(&server), replace(&format!("  {TYPED_KEY}\t ")));
        req.engine = engine;
        let got = run(&w, req);
        latency_of(&got);
        assert_clean(&got, &[]);
        let r = only_request(&server).await;
        let expected = format!("Bearer {TYPED_KEY}");
        assert_eq!(
            header(&r, "authorization"),
            Some(expected.as_str()),
            "{slot:?}"
        );
        for call in w.creds.calls() {
            assert_eq!(
                call,
                read_call(slot),
                "{slot:?}: only the selected slot may be read"
            );
        }
        assert!(
            w.creds.calls().len() <= 1,
            "{slot:?}: {:?}",
            w.creds.calls()
        );
        guard.check(&w);
    }
}

#[tokio::test]
async fn blank_typed_key_uses_the_stored_key() {
    // #30: a Replace that is empty after trim() is Untouched: the stored key, one
    // read. Bite: a blank Bearer sent, or no key at all.
    let server = server_with(ok_text("hello")).await;
    let w = world(stored_keys());
    let got = run(&w, api_req(&base(&server), replace("   ")));
    latency_of(&got);
    let r = only_request(&server).await;
    let expected = format!("Bearer {STORED_API_KEY}");
    assert_eq!(header(&r, "authorization"), Some(expected.as_str()));
    assert_eq!(w.creds.calls(), vec![read_call(KeySlot::TranscriptionApi)]);
}

#[tokio::test]
async fn cleared_local_key_sends_none_and_deletes_nothing() {
    // `Clear` in a test means "no key", not "delete": the local server gets no
    // Authorization and the stored key stays. Bite: the stored key sent (Clear read
    // as Untouched), or a delete on the store.
    let server = server_with(ok_text("hello")).await;
    let w = world(stored_keys());
    let guard = before(&w);
    let got = run(&w, local_req(&base(&server), "", KeyEdit::Clear));
    latency_of(&got);
    let r = only_request(&server).await;
    assert_eq!(header(&r, "authorization"), None);
    guard.check(&w);
    assert_eq!(
        w.creds.stored(KeySlot::LocalServer).as_deref(),
        Some(STORED_LOCAL_KEY)
    );
}

#[tokio::test]
async fn api_without_key_is_invalid_and_sends_nothing() {
    // T-013 Q5: the API engine with no key (Clear, or Untouched with nothing
    // stored) is the save's `key.required` on engine.api.key, with no request,
    // rather than an unauthenticated request and "invalid API key". Bite: the
    // request sent without a key, the check skipped for Clear.
    let server = server_with(ok_text("hello")).await;

    let w = world(stored_keys());
    let got = run(&w, api_req(&base(&server), KeyEdit::Clear));
    assert_eq!(
        invalid_errors(&got),
        vec![("engine.api.key", "key.required")]
    );
    assert_eq!(w.creds.calls(), vec![], "Clear needs no credential call");

    let w = world(FakeCredentialStore::new());
    let got = run(&w, api_req(&base(&server), KeyEdit::Untouched));
    assert_eq!(
        invalid_errors(&got),
        vec![("engine.api.key", "key.required")]
    );
    assert_eq!(w.creds.calls(), vec![read_call(KeySlot::TranscriptionApi)]);

    let w = world(FakeCredentialStore::new());
    let got = run(&w, api_req(&base(&server), replace("  ")));
    assert_eq!(
        invalid_errors(&got),
        vec![("engine.api.key", "key.required")]
    );

    assert!(received(&server).await.is_empty(), "a request was sent");
}

#[tokio::test]
async fn api_with_blank_model_is_invalid_and_sends_nothing() {
    // The save's rule: the API model is required (`required` on engine.api.model).
    // Bite: a request with an empty model part.
    let server = server_with(ok_text("hello")).await;
    let w = world(stored_keys());
    let mut req = api_req(&base(&server), replace(TYPED_KEY));
    req.model = "   ".to_string();
    let got = run(&w, req);
    assert_eq!(invalid_errors(&got), vec![("engine.api.model", "required")]);
    assert!(received(&server).await.is_empty(), "a request was sent");
}

#[tokio::test]
async fn key_store_read_error_is_key_store_unavailable_and_sends_nothing() {
    // #51 / T-013 Q3: a credential read error during an Untouched test is
    // KeyStoreUnavailable, read once, no request. Bite: the error read as "no key"
    // (an unauthenticated request or key.required), a retry.
    let server = server_with(ok_text("hello")).await;
    let creds = stored_keys();
    creds.fail(
        CredentialOp::Read,
        KeySlot::LocalServer,
        CredentialError { os_code: 1312 },
    );
    let w = world(creds);
    let got = run(&w, local_req(&base(&server), "", KeyEdit::Untouched));
    assert_eq!(got, ConnectionTestResult::KeyStoreUnavailable);
    assert_eq!(w.creds.calls(), vec![read_call(KeySlot::LocalServer)]);
    assert!(received(&server).await.is_empty(), "a request was sent");
}

// ---- while Unavailable (#19; choice (iii)) -----------------------------------------

#[tokio::test]
async fn unavailable_settings_allow_a_test_without_credential_calls() {
    // Choice (iii): a test while the service is Unavailable is allowed (it saves
    // nothing); Untouched means "no key" with no credential call (#19, #33(b)).
    // The local server is reached without Authorization; a typed key is still
    // sent; the API engine with Untouched is key.required. Bite: the test refused
    // with settings_unavailable, the store read while Unavailable, the file read
    // or written.
    let server = server_with(ok_text("hello")).await;
    let w = unavailable_world(stored_keys());
    let guard = before(&w);

    let got = run(&w, local_req(&base(&server), "", KeyEdit::Untouched));
    latency_of(&got);
    let got = run(&w, api_req(&base(&server), replace(TYPED_KEY)));
    latency_of(&got);
    let got = run(&w, api_req(&base(&server), KeyEdit::Untouched));
    assert_eq!(
        invalid_errors(&got),
        vec![("engine.api.key", "key.required")]
    );

    let requests = received(&server).await;
    assert_eq!(requests.len(), 2, "two requests (local, typed API key)");
    assert_eq!(header(&requests[0], "authorization"), None);
    let expected = format!("Bearer {TYPED_KEY}");
    assert_eq!(
        header(&requests[1], "authorization"),
        Some(expected.as_str())
    );
    assert_eq!(
        w.creds.calls(),
        vec![],
        "no credential call while Unavailable"
    );
    guard.check(&w);
}

// ---- failure mapping (FR-017) --------------------------------------------------------

#[tokio::test]
async fn status_401_and_403_are_invalid_key() {
    // Bite: 401/403 mapped to Http, the error body (which echoes the key here)
    // reaching the result.
    for status in [401u16, 403] {
        let body = format!("bad key {TYPED_KEY} TRANSCRIPT-MARKER");
        let server = server_with(ResponseTemplate::new(status).set_body_string(body)).await;
        let w = world(stored_keys());
        let got = run(&w, api_req(&base(&server), replace(TYPED_KEY)));
        assert_eq!(got, ConnectionTestResult::InvalidKey, "{status}");
        assert_clean(&got, &["TRANSCRIPT-MARKER", "bad key"]);
    }
}

#[tokio::test]
async fn other_status_is_http_with_the_status() {
    // Bite: a fixed status, 500 mapped to UnexpectedResponse, the body kept.
    for status in [500u16, 404, 429] {
        let server = server_with(
            ResponseTemplate::new(status).set_body_string("BODY-MARKER internal error"),
        )
        .await;
        let w = world(stored_keys());
        let got = run(&w, api_req(&base(&server), replace(TYPED_KEY)));
        assert_eq!(got, ConnectionTestResult::Http { status }, "{status}");
        assert_clean(&got, &["BODY-MARKER"]);
    }
}

#[tokio::test]
async fn non_json_body_is_unexpected_response() {
    // Bite: a 2xx with a non-JSON body reported as Ok (the test must prove the
    // server transcribes), or the body text in the result.
    let server =
        server_with(ResponseTemplate::new(200).set_body_string("<html>BODY-MARKER welcome</html>"))
            .await;
    let w = world(stored_keys());
    let got = run(&w, api_req(&base(&server), replace(TYPED_KEY)));
    assert_eq!(got, ConnectionTestResult::UnexpectedResponse);
    assert_clean(&got, &["BODY-MARKER"]);
}

#[test]
fn refused_port_is_cannot_reach_host_port_without_query() {
    // FR-017: connection refused -> CannotReach with the base URL's host:port,
    // never the query (#27(2)) or the path. Bite: NetworkUnavailable passed
    // through, the whole URL as host. refused_timeouts(): the refusal, not a
    // timer, ends the connect (T-048).
    let addr = refused_addr();
    let w = world(stored_keys());
    let got = run_with(
        &w,
        api_req(
            &format!("http://{addr}/v1?api-version={QUERY_SECRET}"),
            replace(TYPED_KEY),
        ),
        refused_timeouts(),
    );
    assert_eq!(
        got,
        ConnectionTestResult::CannotReach {
            host: addr.to_string()
        }
    );
    assert_clean(&got, &["/v1", "http"]);
}

#[test]
fn refused_port_through_the_public_entry_is_cannot_reach() {
    // The same through `test_connection` with the form's default limits (connect
    // 5 s = REFUSAL_BUDGET), the path the shell takes. Bite: the public entry
    // building another client or swallowing the failure.
    let addr = refused_addr();
    let w = world(stored_keys());
    let got = run(
        &w,
        local_req(&format!("http://{addr}/v1"), "", KeyEdit::Untouched),
    );
    assert_eq!(
        got,
        ConnectionTestResult::CannotReach {
            host: addr.to_string()
        }
    );
}

#[test]
fn unresolvable_host_is_cannot_reach_host() {
    // FR-017 / T-013 H3: a DNS failure is "cannot reach <host>" in a test, though
    // the dictation path keeps NetworkUnavailable (classify unchanged, #51). The
    // explicit port is part of `host`. Bite: NetworkUnavailable passed through
    // (no such result kind: the mapping must convert it), the port dropped.
    let w = world(stored_keys());
    let got = run_with(
        &w,
        api_req("http://voicen-test.invalid/v1", replace(TYPED_KEY)),
        quick(),
    );
    assert_eq!(
        got,
        ConnectionTestResult::CannotReach {
            host: "voicen-test.invalid".to_string()
        }
    );
    let got = run_with(
        &w,
        local_req(
            &format!("http://voicen-test.invalid:8443/v1?k={QUERY_SECRET}"),
            "",
            KeyEdit::Untouched,
        ),
        quick(),
    );
    assert_eq!(
        got,
        ConnectionTestResult::CannotReach {
            host: "voicen-test.invalid:8443".to_string()
        }
    );
    assert_clean(&got, &[]);
}

// ---- malformed URL -> Invalid, nothing sent (FR-017; the save's URL rule) -------------

#[test]
fn malformed_base_url_is_invalid_with_no_credential_call() {
    // A form URL that cannot form a request is the save's URL error on the
    // selected engine's field, before any key read (no key read for a URL that
    // could carry userinfo). Bite: EngineNotConfigured passed through, the error
    // on the other engine's field, a key read first, a URL rule copied with other
    // codes.
    let cases: Vec<(TestEngine, &str, &str, &str)> = vec![
        (
            TestEngine::Api,
            "not a url",
            "engine.api.base_url",
            "url.malformed",
        ),
        (
            TestEngine::Api,
            "ftp://api.example.com/v1",
            "engine.api.base_url",
            "url.malformed",
        ),
        (
            TestEngine::Api,
            "https://user:pass@api.example.com/v1",
            "engine.api.base_url",
            "url.credentials",
        ),
        (
            TestEngine::LocalServer,
            "   ",
            "engine.local_server.base_url",
            "required",
        ),
        (
            TestEngine::LocalServer,
            "http://user@localhost:8000/v1",
            "engine.local_server.base_url",
            "url.credentials",
        ),
    ];
    for (engine, url, field, code) in cases {
        let w = world(stored_keys());
        let guard = before(&w);
        let mut req = api_req(url, KeyEdit::Untouched);
        req.engine = engine;
        let got = run(&w, req);
        let errors = invalid_errors(&got);
        assert!(errors.contains(&(field, code)), "{url:?}: {errors:?}");
        assert!(
            errors.iter().all(|(f, _)| f.starts_with("engine.")),
            "{url:?}: {errors:?}"
        );
        assert_eq!(w.creds.calls(), vec![], "{url:?}: no credential call");
        assert_clean(&got, &["user:pass", "pass@"]);
        guard.check(&w);
    }
}

#[test]
fn malformed_url_with_typed_key_is_exactly_the_url_error() {
    // With a typed key and a model, the only error is the URL one. Bite: a key or
    // model error invented for a valid key.
    let w = world(stored_keys());
    let got = run(&w, api_req("not a url", replace(TYPED_KEY)));
    assert_eq!(
        got,
        ConnectionTestResult::Invalid {
            errors: vec![FieldError {
                field: FieldId::EngineApiBaseUrl,
                code: ErrorCode::UrlMalformed,
            }]
        }
    );
}

// ---- the wire request (contracts/ipc.md; T-030 J1) -------------------------------------

#[test]
fn request_deserializes_from_the_wire_form() {
    // `{engine: "api" | "local_server", base_url, model, key: KeyEdit, timeouts:
    // TimeoutSettings}`. Bite: another spelling of the engine, a field renamed,
    // the form's timeouts dropped.
    let req: ConnectionTestRequest = serde_json::from_value(json!({
        "engine": "local_server",
        "base_url": "http://localhost:8000/v1",
        "model": "m",
        "key": { "Replace": TYPED_KEY },
        "timeouts": {
            "connect_s": 7, "api_transcription_s": 45, "local_server_s": 90,
            "post_processing_s": 20, "builtin_local_s": 150
        },
    }))
    .expect("valid request");
    assert!(matches!(req.engine, TestEngine::LocalServer));
    assert_eq!(req.base_url, "http://localhost:8000/v1");
    assert_eq!(req.model, "m");
    assert!(matches!(&req.key, KeyEdit::Replace(k) if k.expose() == TYPED_KEY));
    assert_eq!(req.timeouts.connect_s, 7);
    assert_eq!(req.timeouts.local_server_s, 90);
    assert!(!format!("{req:?}").contains(TYPED_KEY), "key in Debug");

    let req: ConnectionTestRequest = serde_json::from_value(json!({
        "engine": "api", "base_url": "https://api.example.com/v1", "model": "m",
        "key": "Untouched", "timeouts": default_settings(),
    }))
    .expect("valid request");
    assert!(matches!(req.engine, TestEngine::Api));
    assert!(matches!(req.key, KeyEdit::Untouched));
}

#[test]
fn bad_request_is_one_fixed_text_without_input() {
    // The request carries a key, and serde's own messages quote the input
    // (T-030 J1): every deserialize error is one fixed text. Engines that cannot
    // be tested ("none", "builtin_local") are refused at deserialize (closed
    // TestEngine). A request without the form's timeouts is refused (the UI must
    // send them; a default would hide the drift FR-017 forbids). Bite: serde's
    // message passed through (the key or engine text quoted), EngineKind reused
    // (none accepted), `timeouts` defaulted.
    let canary = "sk-test-CANARY-wire";
    let t = serde_json::to_value(default_settings()).expect("timeouts serialize");
    let bad = [
        json!({ "engine": "none", "base_url": "u", "model": "m", "key": "Untouched", "timeouts": t }),
        json!({ "engine": "builtin_local", "base_url": "u", "model": "m", "key": "Untouched", "timeouts": t }),
        json!({ "engine": canary, "base_url": "u", "model": "m", "key": "Untouched", "timeouts": t }),
        json!({ "engine": "api", "base_url": "u", "model": "m", "key": { "Replace": canary } }),
        json!({ "engine": "api", "base_url": 5, "model": "m", "key": { "Replace": canary }, "timeouts": t }),
        json!({ "engine": "api", "base_url": "u", "model": "m", "key": { "Replace": 42 }, "timeouts": t }),
        json!({ "engine": "api", "base_url": "u", "model": canary, "key": "Untouched", "timeouts": { "connect_s": canary } }),
        json!([canary]),
    ];
    let mut messages = Vec::new();
    for value in bad {
        let err = match serde_json::from_value::<ConnectionTestRequest>(value.clone()) {
            Ok(req) => panic!("accepted {value}: {req:?}"),
            Err(e) => e.to_string(),
        };
        assert!(!err.contains("CANARY") && !err.contains("sk-test"), "{err}");
        messages.push(err);
    }
    messages.dedup();
    assert_eq!(messages.len(), 1, "one fixed text, got {messages:?}");
}
