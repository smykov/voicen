//! T-040: the OpenAI-compatible transcription client against a mock server
//! (contracts/openai-transcription.md "Test oracle"; T-040 Investigation (5)).
//!
//! wiremock serves on its own thread and runtime; the engine is called on a plain
//! std thread, because the blocking reqwest client panics inside a tokio runtime
//! context (T-001 Investigation (e)). Fake data only: 127.0.0.1, `.invalid`
//! (RFC 6761), `sk-test-SECRET`.

mod common;
/// The checks of `common::refused_addr()`, in each binary that calls it (T-047).
#[path = "common/refused_addr_tests.rs"]
mod refused_addr_tests;

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::time::{Duration, Instant};

use common::refused_addr;
use serde_json::json;
use voicen_core::audio::{wav, AudioBuffer};
use voicen_core::engine::openai::OpenAiCompatibleEngine;
use voicen_core::engine::{engine_for, Engine, TranscribeRequest};
use voicen_core::failure::FailureReason;
use voicen_core::i18n::{text, UiLanguage};
use voicen_core::secrets::{FakeCredentialStore, KeySlot, Secret};
use voicen_core::settings::url::check_base_url;
use voicen_core::settings::{defaults, EngineKind};
use voicen_core::timeouts::Timeouts;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-test-SECRET";
const QUERY_SECRET: &str = "SECRETQ";
const TRANSCRIPT: &str = "TRANSCRIPT-MARKER";
const MIB: usize = 1024 * 1024;

// ---- helpers ------------------------------------------------------------------

/// 100 ms of a 16 kHz sawtooth.
fn audio() -> AudioBuffer {
    AudioBuffer::from_16k_mono((0..1600).map(|i| ((i % 64) as i16 - 32) * 512).collect())
}

/// Test durations: set explicitly, never the production defaults.
fn timeouts() -> Timeouts {
    Timeouts {
        connect: Duration::from_secs(2),
        api_transcription: Duration::from_secs(5),
        ..Timeouts::default()
    }
}

fn req(language: Option<&str>) -> TranscribeRequest {
    TranscribeRequest {
        language: language.map(str::to_string),
        timeouts: timeouts(),
    }
}

fn req_with_total(total: Duration) -> TranscribeRequest {
    let mut r = req(None);
    r.timeouts.api_transcription = total;
    r
}

fn api_engine(base: &str, key: Option<&str>) -> Box<OpenAiCompatibleEngine> {
    let base =
        check_base_url(base).unwrap_or_else(|e| panic!("test base URL {base:?} refused: {e:?}"));
    Box::new(OpenAiCompatibleEngine::new(
        base,
        "whisper-1",
        key.map(Secret::new),
    ))
}

/// `transcribe` on a plain std thread (no runtime context there).
fn transcribe<E: Engine + ?Sized + 'static>(
    engine: Box<E>,
    req: TranscribeRequest,
) -> Result<String, FailureReason> {
    std::thread::spawn(move || engine.transcribe(&audio(), &req))
        .join()
        .expect("transcribe must not panic")
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

async fn only_request(server: &MockServer) -> wiremock::Request {
    let mut requests = server
        .received_requests()
        .await
        .expect("request recording is on");
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

#[derive(Debug)]
struct Part {
    name: String,
    filename: Option<String>,
    content_type: Option<String>,
    body: Vec<u8>,
}

fn disposition_param(disposition: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    disposition
        .split(';')
        .map(str::trim)
        .find_map(|p| p.strip_prefix(prefix.as_str()))
        .map(|v| v.trim_matches('"').to_string())
}

/// The parts of a `multipart/form-data` request body (RFC 7578), in order.
fn multipart(r: &wiremock::Request) -> Vec<Part> {
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
        let head =
            std::str::from_utf8(&body[head_start..head_end]).expect("part headers are UTF-8");
        let content_end = find(body, &next_delim, head_end + 4).expect("next boundary");
        let mut part = Part {
            name: String::new(),
            filename: None,
            content_type: None,
            body: body[head_end + 4..content_end].to_vec(),
        };
        for line in head.split("\r\n") {
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            match k.trim().to_ascii_lowercase().as_str() {
                "content-disposition" => {
                    part.name = disposition_param(v, "name").unwrap_or_default();
                    part.filename = disposition_param(v, "filename");
                }
                "content-type" => part.content_type = Some(v.trim().to_string()),
                _ => {}
            }
        }
        parts.push(part);
        pos = content_end + 2;
    }
    parts
}

fn part<'a>(parts: &'a [Part], name: &str) -> Option<&'a Part> {
    parts.iter().find(|p| p.name == name)
}

fn part_text(parts: &[Part], name: &str) -> Option<String> {
    part(parts, name).map(|p| String::from_utf8_lossy(&p.body).into_owned())
}

/// A 2xx body `{"text":"aaa…"}` of exactly `total` bytes.
fn text_body_of_len(total: usize) -> (Vec<u8>, String) {
    let prefix = b"{\"text\":\"";
    let suffix = b"\"}";
    let text = "a".repeat(total - prefix.len() - suffix.len());
    let body = [prefix.as_slice(), text.as_bytes(), suffix.as_slice()].concat();
    assert_eq!(body.len(), total);
    (body, text)
}

/// Reads one HTTP request (headers + Content-Length or chunked body).
fn read_request(s: &mut TcpStream) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        let Some(head_end) = find(&buf, b"\r\n\r\n", 0) else {
            continue;
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
        let length = head
            .lines()
            .find_map(|l| l.strip_prefix("content-length:"))
            .and_then(|v| v.trim().parse::<usize>().ok());
        match length {
            Some(len) if buf.len() >= head_end + 4 + len => return,
            None if buf.ends_with(b"0\r\n\r\n") => return,
            _ => {}
        }
    }
}

/// One-shot raw HTTP server: reads the request, sends `200` with
/// `Content-Length: 100` and only part of the body (containing the transcript
/// marker), then holds the connection for `hold` and closes it (FIN). Returns the
/// base URL (with the secret query). The thread is not joined: with an engine that
/// never connects it just waits in `accept`.
fn partial_body_server(hold: Duration) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
    let addr = listener.local_addr().expect("local addr");
    std::thread::spawn(move || {
        let Ok((mut s, _)) = listener.accept() else {
            return;
        };
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        read_request(&mut s);
        let head =
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n";
        let _ = s.write_all(format!("{head}{{\"text\":\"{TRANSCRIPT}").as_bytes());
        let _ = s.flush();
        std::thread::sleep(hold);
        let _ = s.shutdown(Shutdown::Both);
    });
    format!("http://{addr}/v1?api-version={QUERY_SECRET}")
}

// ---- Acceptance 1: request shape --------------------------------------------

#[tokio::test]
async fn request_is_multipart_with_file_model_language_and_bearer() {
    // Contract "Request". Bite: wrong path, no Bearer, file not the WAV of the
    // buffer (raw PCM, another name or type), a missing model/language/
    // response_format part, a JSON body instead of multipart, a second request.
    let server = server_with(ok_text("hello world")).await;
    let got = transcribe(
        api_engine(&format!("{}/v1", server.uri()), Some(KEY)),
        req(Some("de")),
    );
    assert_eq!(got, Ok("hello world".to_string()));

    let r = only_request(&server).await;
    assert_eq!(r.method.as_str(), "POST");
    assert_eq!(r.url.path(), "/v1/audio/transcriptions");
    assert_eq!(r.url.query(), None);
    assert_eq!(header(&r, "authorization"), Some("Bearer sk-test-SECRET"));

    let parts = multipart(&r);
    let mut names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["file", "language", "model", "response_format"]);

    let file = part(&parts, "file").expect("file part");
    assert_eq!(file.filename.as_deref(), Some("audio.wav"));
    assert_eq!(file.content_type.as_deref(), Some("audio/wav"));
    assert!(
        file.body == wav::encode(&audio()),
        "file part is not wav::encode(&buffer)"
    );
    assert_eq!(part_text(&parts, "model").as_deref(), Some("whisper-1"));
    assert_eq!(part_text(&parts, "language").as_deref(), Some("de"));
    assert_eq!(
        part_text(&parts, "response_format").as_deref(),
        Some("json")
    );
}

#[tokio::test]
async fn auto_language_omits_language_part() {
    // language = auto -> no `language` part at all (not an empty one, not "auto").
    let server = server_with(ok_text("hello")).await;
    let got = transcribe(
        api_engine(&format!("{}/v1", server.uri()), Some(KEY)),
        req(None),
    );
    assert_eq!(got, Ok("hello".to_string()));
    let parts = multipart(&only_request(&server).await);
    assert!(
        part(&parts, "language").is_none(),
        "language part sent for auto: {:?}",
        part_text(&parts, "language")
    );
    assert!(part(&parts, "file").is_some() && part(&parts, "model").is_some());
}

#[tokio::test]
async fn no_key_sends_no_authorization_header() {
    // NFR-04 / contract: no key stored -> no Authorization header (not
    // "Bearer " with an empty key).
    let server = server_with(ok_text("hello")).await;
    let got = transcribe(api_engine(&format!("{}/v1", server.uri()), None), req(None));
    assert_eq!(got, Ok("hello".to_string()));
    let r = only_request(&server).await;
    assert_eq!(header(&r, "authorization"), None);
}

#[tokio::test]
async fn base_url_with_query_keeps_query_and_gets_path() {
    // Decision #27(2): `?api-version=…` stays, the path is appended before it.
    // Bite: string concatenation (path inside the query) or `Url::join`.
    let server = server_with(ok_text("hello")).await;
    let base = format!("{}/v1?api-version=2024-06-01", server.uri());
    let got = transcribe(api_engine(&base, Some(KEY)), req(None));
    assert_eq!(got, Ok("hello".to_string()));
    let r = only_request(&server).await;
    assert_eq!(r.url.path(), "/v1/audio/transcriptions");
    assert_eq!(r.url.query(), Some("api-version=2024-06-01"));
}

#[tokio::test]
async fn trailing_slash_base_url_gives_same_path() {
    // `…/v1`, `…/v1/` and `…/v1//` (stored as `…/v1/`) all post to
    // `/v1/audio/transcriptions`; a root base posts to `/audio/transcriptions`.
    // Bite: no pop_if_empty (`/v1//audio/…`).
    for (suffix, path) in [
        ("/v1", "/v1/audio/transcriptions"),
        ("/v1/", "/v1/audio/transcriptions"),
        ("/v1//", "/v1/audio/transcriptions"),
        ("", "/audio/transcriptions"),
        ("/", "/audio/transcriptions"),
    ] {
        let server = server_with(ok_text("hello")).await;
        let got = transcribe(
            api_engine(&format!("{}{suffix}", server.uri()), Some(KEY)),
            req(None),
        );
        assert_eq!(got, Ok("hello".to_string()), "{suffix:?}");
        assert_eq!(only_request(&server).await.url.path(), path, "{suffix:?}");
    }
}

#[tokio::test]
async fn text_is_trimmed_and_blank_text_is_ok_empty() {
    // Contract "Response": leading/trailing whitespace trimmed, inner kept; empty
    // or whitespace-only text is Ok("") (the pipeline's NoSpeech), not an error;
    // extra response fields (verbose servers) are ignored.
    // Bite: no trim, blank text as UnexpectedResponse, deny_unknown_fields.
    let cases: Vec<(serde_json::Value, &str)> = vec![
        (json!({ "text": "  hello  world \n" }), "hello  world"),
        (json!({ "text": "hello" }), "hello"),
        (json!({ "text": "   \n\t" }), ""),
        (json!({ "text": "" }), ""),
        (
            json!({ "text": " hi ", "language": "en", "duration": 1.25, "x_groq": { "id": "req_x" } }),
            "hi",
        ),
    ];
    for (body, expected) in cases {
        let server = server_with(ResponseTemplate::new(200).set_body_json(&body)).await;
        let got = transcribe(
            api_engine(&format!("{}/v1", server.uri()), Some(KEY)),
            req(None),
        );
        assert_eq!(got, Ok(expected.to_string()), "{body}");
    }
}

#[tokio::test]
async fn engine_for_api_settings_sends_stored_key_and_model() {
    // The factory's engine uses the stored key, the settings' model and base URL.
    // Bite: engine_for dropping the key it read, or using the default model.
    let server = server_with(ok_text("hi")).await;
    let mut settings = defaults(None);
    settings.engine = EngineKind::Api;
    settings.api.base_url = format!("{}/v1", server.uri());
    settings.api.model = "whisper-large-v3-turbo".to_string();
    let creds = FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, KEY);
    let engine = match engine_for(&settings, &creds) {
        Ok(engine) => engine,
        Err(e) => panic!("API engine expected, got {e:?}"),
    };
    assert_eq!(transcribe(engine, req(Some("ru"))), Ok("hi".to_string()));
    let r = only_request(&server).await;
    assert_eq!(r.url.path(), "/v1/audio/transcriptions");
    assert_eq!(header(&r, "authorization"), Some("Bearer sk-test-SECRET"));
    let parts = multipart(&r);
    assert_eq!(
        part_text(&parts, "model").as_deref(),
        Some("whisper-large-v3-turbo")
    );
    assert_eq!(part_text(&parts, "language").as_deref(), Some("ru"));
}

// ---- Acceptance 3: failure branch -------------------------------------------

#[tokio::test]
async fn status_401_and_403_are_invalid_api_key() {
    // Bite: 403 as ServerError, or the error body parsed for `text`.
    for status in [401u16, 403] {
        let server = server_with(
            ResponseTemplate::new(status)
                .set_body_json(json!({ "text": TRANSCRIPT, "error": KEY })),
        )
        .await;
        let got = transcribe(
            api_engine(&format!("{}/v1", server.uri()), Some(KEY)),
            req(None),
        );
        assert_eq!(got, Err(FailureReason::InvalidApiKey), "{status}");
        only_request(&server).await;
    }
}

#[tokio::test]
async fn other_status_is_server_error() {
    // Any other non-2xx -> ServerError{status}, one request, no retry (spec edge
    // case). Bite: a retry on 429/5xx, a status mapped to UnexpectedResponse, a
    // body with `text` accepted on an error status.
    for status in [400u16, 404, 413, 429, 500, 503] {
        let server =
            server_with(ResponseTemplate::new(status).set_body_json(json!({ "text": TRANSCRIPT })))
                .await;
        let got = transcribe(
            api_engine(&format!("{}/v1", server.uri()), Some(KEY)),
            req(None),
        );
        assert_eq!(got, Err(FailureReason::ServerError { status }), "{status}");
        only_request(&server).await;
    }
}

#[tokio::test]
async fn delay_past_ms_timeout_is_timeout() {
    // FR-24: the whole-request duration comes from the request's Timeouts.
    // A 5 s delay against a 300 ms limit is Timeout, well before 5 s.
    // Bite: the client's 30 s default or Timeouts::default() used instead of
    // req.timeouts (the call then succeeds after 5 s), or a timeout mapped to
    // CannotReach.
    let server = server_with(ok_text("late").set_delay(Duration::from_secs(5))).await;
    let started = Instant::now();
    let got = transcribe(
        api_engine(&format!("{}/v1", server.uri()), Some(KEY)),
        req_with_total(Duration::from_millis(300)),
    );
    let elapsed = started.elapsed();
    assert_eq!(got, Err(FailureReason::Timeout));
    assert!(elapsed < Duration::from_secs(3), "took {elapsed:?}");
}

#[test]
fn refused_loopback_port_is_cannot_reach_host_port() {
    // Connection refused -> CannotReach with the base URL's host:port.
    // Bite: NetworkUnavailable for a refused port, the whole URL as `host`.
    let addr = refused_addr();
    let got = transcribe(
        api_engine(&format!("http://{addr}/v1"), Some(KEY)),
        req(None),
    );
    assert_eq!(
        got,
        Err(FailureReason::CannotReach {
            host: addr.to_string()
        })
    );
}

#[test]
fn invalid_host_is_network_unavailable() {
    // RFC 6761 `.invalid` never resolves; reqwest reports is_dns AND is_connect.
    // Bite: is_connect checked before is_dns (-> CannotReach).
    let got = transcribe(
        api_engine("http://voicen-test.invalid/v1", Some(KEY)),
        req(None),
    );
    assert_eq!(got, Err(FailureReason::NetworkUnavailable));
}

#[tokio::test]
async fn malformed_bodies_are_unexpected_response() {
    // 2xx bodies that are not {"text": "<string>"} -> UnexpectedResponse, never
    // a panic and never Ok. Bite: lossy UTF-8 decoding, `text` defaulting to "",
    // a number/null coerced to a string, any unwrap on the body.
    let cases: Vec<(&str, ResponseTemplate)> = vec![
        (
            "not JSON",
            ResponseTemplate::new(200).set_body_string(format!("hello {TRANSCRIPT}")),
        ),
        (
            "text missing",
            ResponseTemplate::new(200).set_body_json(json!({ "transcript": TRANSCRIPT })),
        ),
        (
            "text a number",
            ResponseTemplate::new(200).set_body_json(json!({ "text": 42 })),
        ),
        (
            "text null",
            ResponseTemplate::new(200).set_body_json(json!({ "text": null })),
        ),
        (
            "JSON array",
            ResponseTemplate::new(200).set_body_json(json!([TRANSCRIPT])),
        ),
        (
            "JSON string",
            ResponseTemplate::new(200).set_body_json(json!(TRANSCRIPT)),
        ),
        (
            "invalid UTF-8",
            ResponseTemplate::new(200).set_body_raw(
                b"{\"text\":\"\xff\xfe TRANSCRIPT-MARKER\"}".to_vec(),
                "application/json",
            ),
        ),
        ("empty body", ResponseTemplate::new(200)),
    ];
    let mut wrong = Vec::new();
    for (name, template) in cases {
        let server = server_with(template).await;
        let got = transcribe(
            api_engine(&format!("{}/v1", server.uri()), Some(KEY)),
            req(None),
        );
        if got != Err(FailureReason::UnexpectedResponse) {
            wrong.push(format!("{name}: {got:?}"));
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

#[tokio::test]
async fn oversized_body_is_unexpected_response() {
    // Body cap 1 MiB (contract: "> 1 MiB" -> UnexpectedResponse): a valid JSON
    // body of 1 MiB + 1 bytes is refused, one of exactly 1 MiB is accepted.
    // Bite: `json()` / read_to_end without a cap (the 1 MiB + 1 body would be Ok),
    // or an off-by-one cap (`take(1 MiB)` refusing nothing / exactly 1 MiB).
    let (over, _) = text_body_of_len(MIB + 1);
    let server =
        server_with(ResponseTemplate::new(200).set_body_raw(over, "application/json")).await;
    let got = transcribe(
        api_engine(&format!("{}/v1", server.uri()), Some(KEY)),
        req(None),
    );
    assert_eq!(got, Err(FailureReason::UnexpectedResponse), "1 MiB + 1");

    let (exact, text) = text_body_of_len(MIB);
    let server =
        server_with(ResponseTemplate::new(200).set_body_raw(exact, "application/json")).await;
    let got = transcribe(
        api_engine(&format!("{}/v1", server.uri()), Some(KEY)),
        req(None),
    );
    assert!(
        got.as_deref() == Ok(text.as_str()),
        "exactly 1 MiB must be Ok(text): {:?}",
        got.map(|t| t.len())
    );
}

#[test]
fn stall_mid_body_is_timeout_and_cut_mid_body_is_unexpected_response() {
    // T-040 probe: a stall mid-body surfaces as io::Error wrapping a reqwest
    // timeout -> Timeout (FR-24: the whole request); a connection closed mid-body
    // is the same wrapper without timeout -> UnexpectedResponse.
    // Bite: the adapter not downcasting io::Error::get_ref() to reqwest::Error
    // (stall -> UnexpectedResponse), or treating every body error as Timeout.
    let started = Instant::now();
    let got = transcribe(
        api_engine(&partial_body_server(Duration::from_secs(4)), Some(KEY)),
        req_with_total(Duration::from_millis(500)),
    );
    let elapsed = started.elapsed();
    assert_eq!(got, Err(FailureReason::Timeout), "stall mid-body");
    assert!(elapsed < Duration::from_secs(3), "stall took {elapsed:?}");

    let got = transcribe(
        api_engine(&partial_body_server(Duration::ZERO), Some(KEY)),
        req(None),
    );
    assert_eq!(
        got,
        Err(FailureReason::UnexpectedResponse),
        "closed mid-body"
    );
}

#[tokio::test]
async fn no_failure_contains_query_key_or_transcript() {
    // P-009 / NFR-04 / FR-20: base URL `?api-version=SECRETQ`, key
    // `sk-test-SECRET`, `TRANSCRIPT-MARKER` in the response bodies. No reason's
    // Display, Debug, code, message params or rendered message contains any of
    // them. Each case first proves it reached the expected reason (else a
    // constant reason would pass vacuously). Bite: a reason built from
    // reqwest::Error's Display (it contains the URL query), from the URL, or from
    // the body.
    let leaky = format!("{{\"error\":\"{TRANSCRIPT} {KEY} api-version={QUERY_SECRET}\"}}");
    let mut outcomes: Vec<(String, Result<String, FailureReason>, FailureReason)> = Vec::new();

    let http_cases: Vec<(&str, ResponseTemplate, Duration, FailureReason)> = vec![
        (
            "401",
            ResponseTemplate::new(401).set_body_string(leaky.clone()),
            Duration::from_secs(5),
            FailureReason::InvalidApiKey,
        ),
        (
            "500",
            ResponseTemplate::new(500).set_body_string(leaky.clone()),
            Duration::from_secs(5),
            FailureReason::ServerError { status: 500 },
        ),
        (
            "200 without text",
            ResponseTemplate::new(200).set_body_string(leaky.clone()),
            Duration::from_secs(5),
            FailureReason::UnexpectedResponse,
        ),
        (
            "200 oversized",
            ResponseTemplate::new(200).set_body_string(format!(
                "{{\"text\":\"{}\"}}",
                TRANSCRIPT.repeat(MIB / TRANSCRIPT.len() + 1)
            )),
            Duration::from_secs(5),
            FailureReason::UnexpectedResponse,
        ),
        (
            "delay",
            ok_text(TRANSCRIPT).set_delay(Duration::from_secs(5)),
            Duration::from_millis(300),
            FailureReason::Timeout,
        ),
    ];
    for (name, template, total, expected) in http_cases {
        let server = server_with(template).await;
        let base = format!("{}/v1?api-version={QUERY_SECRET}", server.uri());
        let got = transcribe(api_engine(&base, Some(KEY)), req_with_total(total));
        outcomes.push((name.to_string(), got, expected));
    }

    let addr = refused_addr();
    let got = transcribe(
        api_engine(
            &format!("http://{addr}/v1?api-version={QUERY_SECRET}"),
            Some(KEY),
        ),
        req(None),
    );
    outcomes.push((
        "refused".to_string(),
        got,
        FailureReason::CannotReach {
            host: addr.to_string(),
        },
    ));

    let got = transcribe(
        api_engine(
            &format!("http://voicen-test.invalid/v1?api-version={QUERY_SECRET}"),
            Some(KEY),
        ),
        req(None),
    );
    outcomes.push((
        "invalid host".to_string(),
        got,
        FailureReason::NetworkUnavailable,
    ));

    let got = transcribe(
        api_engine(&partial_body_server(Duration::ZERO), Some(KEY)),
        req(None),
    );
    outcomes.push((
        "closed mid-body".to_string(),
        got,
        FailureReason::UnexpectedResponse,
    ));

    let needles = [KEY, QUERY_SECRET, TRANSCRIPT, "api-version"];
    let mut wrong = Vec::new();
    for (name, got, expected) in outcomes {
        let reason = match got {
            Err(reason) if reason == expected => reason,
            other => {
                wrong.push(format!("{name}: {other:?}, expected Err({expected:?})"));
                continue;
            }
        };
        let params = reason.message_params();
        let args: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let mut texts = vec![
            format!("{reason}"),
            format!("{reason:?}"),
            reason.code().to_string(),
            text(UiLanguage::En, reason.message_id(), &args),
            text(UiLanguage::Ru, reason.message_id(), &args),
        ];
        texts.extend(params.iter().map(|(_, v)| v.clone()));
        for t in &texts {
            for needle in needles {
                if t.contains(needle) {
                    wrong.push(format!("{name}: {needle:?} in {t:?}"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

// ---- Review round 1 finding 4: an unusable stored key --------------------------

/// Keys with an inner control character that no HTTP header value may carry
/// (fake values). Tab is a legal header byte, so it is not among them.
const UNUSABLE_KEYS: [&str; 5] = [
    "sk-test-a\nb",
    "sk-test-a\rb",
    "sk-test-a\u{0}b",
    "sk-test-a\u{1}b",
    "sk-test-a\u{7f}b",
];

#[tokio::test]
async fn key_with_inner_control_char_is_invalid_api_key_and_sends_nothing() {
    // Review 1 #4: a stored key with an inner control character cannot become an
    // `Authorization` header. The user must be told the key is the problem
    // (InvalidApiKey), not that the server is unreachable (today the header build
    // error is a builder error -> Setup -> CannotReach{host}); and no request may
    // reach the server (no request without the key's header, no retry without it).
    // Bite: the header-build failure classified as Setup/CannotReach; the bad key
    // silently dropped (the request then goes out without Authorization and gets
    // Ok, or reaches the server at all).
    let mut wrong = Vec::new();
    for key in UNUSABLE_KEYS {
        let server = server_with(ok_text("hello")).await;
        let got = transcribe(
            api_engine(&format!("{}/v1", server.uri()), Some(key)),
            req(None),
        );
        if got != Err(FailureReason::InvalidApiKey) {
            wrong.push(format!("{key:?}: {got:?}, expected Err(InvalidApiKey)"));
        }
        let sent = server
            .received_requests()
            .await
            .expect("request recording is on")
            .len();
        if sent != 0 {
            wrong.push(format!("{key:?}: {sent} request(s) reached the server"));
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

#[tokio::test]
async fn engine_for_stored_key_with_control_char_is_invalid_api_key_and_sends_nothing() {
    // The same guarantee on the path the user hits: the key comes from the
    // credential store through `engine_for`. Either the factory refuses with
    // InvalidApiKey, or its engine's `transcribe` does; never CannotReach, never a
    // request. Bite: as above, for a fix placed only where `engine_for` does not
    // reach it.
    let server = server_with(ok_text("hello")).await;
    let mut settings = defaults(None);
    settings.engine = EngineKind::Api;
    settings.api.base_url = format!("{}/v1", server.uri());
    let creds = FakeCredentialStore::new().with_key(KeySlot::TranscriptionApi, "sk-test-a\nb");
    let got = match engine_for(&settings, &creds) {
        Ok(engine) => transcribe(engine, req(None)),
        Err(e) => Err(e),
    };
    assert_eq!(got, Err(FailureReason::InvalidApiKey));
    let sent = server
        .received_requests()
        .await
        .expect("request recording is on")
        .len();
    assert_eq!(sent, 0, "a request reached the server");
}
