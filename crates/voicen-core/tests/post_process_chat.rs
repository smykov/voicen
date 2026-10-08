//! T-020: `ChatPostProcessor` against a mock OpenAI-compatible chat endpoint
//! (spec 003 contracts/core-post-process.md guarantees 1-5, data-model
//! "ChatRequest" / "ChatReply" / "SkipReason", research R4/R5; decision #91).
//!
//! wiremock serves on its own thread and runtime; the processor is called on a
//! plain std thread, because the blocking reqwest client panics inside a tokio
//! runtime context (T-040). Refused connects use `common::refused_addr()` with
//! `common::refused_timeouts()` (F-004, F-005); the timeout case uses a server that
//! accepts and answers late. Fake data only: 127.0.0.1, `sk-test-...` keys.

mod common;
/// The checks of `common::refused_addr()`, in each binary that calls it (T-047).
#[path = "common/refused_addr_tests.rs"]
mod refused_addr_tests;

use std::time::{Duration, Instant};

use common::{refused_addr, refused_timeouts};
use serde_json::{json, Value};
use voicen_core::post_process::chat::ChatPostProcessor;
use voicen_core::post_process::settings::PostProcessingSettings;
use voicen_core::post_process::{PostProcessInput, PostProcessOutcome, PostProcessor, SkipReason};
use voicen_core::secrets::{
    CredentialCall, CredentialError, CredentialOp, FakeCredentialStore, KeySlot,
};
use voicen_core::timeouts::Timeouts;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-test-pp-SECRET";
const MODEL: &str = "gpt-test-mini";
const PROMPT: &str = "PROMPT-MARKER: fix punctuation";
/// Acceptance line 1.
const RAW: &str = "привет как дела";
const REPLY: &str = "Привет, как дела?";
const QUERY_SECRET: &str = "SECRETQ";
const BODY_MARKER: &str = "BODY-MARKER";
const MIB: usize = 1024 * 1024;

// ---- helpers ------------------------------------------------------------------

fn pp_settings(base: &str) -> PostProcessingSettings {
    PostProcessingSettings {
        enabled: true,
        base_url: base.to_string(),
        model: MODEL.to_string(),
        prompt: PROMPT.to_string(),
    }
}

/// Test durations: set explicitly, never the production defaults.
fn timeouts() -> Timeouts {
    Timeouts {
        connect: Duration::from_secs(2),
        post_processing: Duration::from_secs(5),
        ..Timeouts::default()
    }
}

fn creds_with_key(key: &str) -> FakeCredentialStore {
    FakeCredentialStore::new().with_key(KeySlot::PostProcessing, key)
}

/// `ChatPostProcessor::process` on a plain std thread (no runtime context there).
fn run(
    settings: &PostProcessingSettings,
    creds: &FakeCredentialStore,
    timeouts: Timeouts,
    raw: &str,
) -> PostProcessOutcome {
    std::thread::scope(|s| {
        s.spawn(|| {
            let input = PostProcessInput {
                settings,
                credentials: creds,
                timeouts: &timeouts,
            };
            ChatPostProcessor::new().process(raw, &input)
        })
        .join()
        .expect("process must not panic")
    })
}

async fn server_with(response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

fn chat_reply(content: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": content } }],
    }))
}

async fn requests(server: &MockServer) -> Vec<wiremock::Request> {
    server
        .received_requests()
        .await
        .expect("request recording is on")
}

async fn only_request(server: &MockServer) -> wiremock::Request {
    let mut all = requests(server).await;
    assert_eq!(all.len(), 1, "exactly one request (no retries)");
    all.remove(0)
}

fn header<'a>(r: &'a wiremock::Request, name: &str) -> Option<&'a str> {
    r.headers.get(name).and_then(|v| v.to_str().ok())
}

fn read_pp() -> Vec<CredentialCall> {
    vec![CredentialCall {
        op: CredentialOp::Read,
        slot: KeySlot::PostProcessing,
    }]
}

/// A 2xx chat reply of exactly `total` bytes whose content is `a…a`.
fn reply_of_len(total: usize) -> (Vec<u8>, String) {
    let prefix = br#"{"choices":[{"message":{"content":""#;
    let suffix = br#""}}]}"#;
    let text = "a".repeat(total - prefix.len() - suffix.len());
    let body = [prefix.as_slice(), text.as_bytes(), suffix.as_slice()].concat();
    assert_eq!(body.len(), total);
    (body, text)
}

// ---- Acceptance 1: applied, trimmed, request shape ------------------------------

#[tokio::test]
async fn reply_is_applied_trimmed_from_one_chat_completions_request() {
    // Acceptance 1 and contract guarantees 1-3: "привет как дела" + prompt -> the
    // mock's "Привет, как дела?" (sent with surrounding whitespace) comes back
    // trimmed as Applied; exactly one POST to {base}/chat/completions, JSON body
    // with the model, the system prompt and the raw text as the user message (and
    // nothing else but `stream: false`), Bearer key, the key read once from the
    // post-processing slot. Bite: no request at all (PassThrough), the
    // transcription path or a GET, the prompt and the raw text swapped or merged,
    // the key of another slot, an untrimmed reply, a retry.
    let server = server_with(chat_reply(json!(format!("  {REPLY}\n")))).await;
    let creds = creds_with_key(KEY);
    let got = run(
        &pp_settings(&format!("{}/v1", server.uri())),
        &creds,
        timeouts(),
        RAW,
    );
    assert_eq!(got, PostProcessOutcome::Applied(REPLY.to_string()));

    let r = only_request(&server).await;
    assert_eq!(r.method.as_str(), "POST");
    assert_eq!(r.url.path(), "/v1/chat/completions");
    assert_eq!(r.url.query(), None);
    assert!(
        header(&r, "content-type").is_some_and(|v| v.starts_with("application/json")),
        "content-type {:?}",
        header(&r, "content-type")
    );
    let expected_auth = format!("Bearer {KEY}");
    assert_eq!(header(&r, "authorization"), Some(expected_auth.as_str()));
    let body: Value = serde_json::from_slice(&r.body).expect("the request body is JSON");
    assert_eq!(body["model"], json!(MODEL));
    assert_eq!(
        body["messages"],
        json!([
            { "role": "system", "content": PROMPT },
            { "role": "user", "content": RAW },
        ])
    );
    let keys: Vec<&String> = body.as_object().expect("a JSON object").keys().collect();
    assert!(
        keys.iter()
            .all(|k| ["model", "messages", "stream"].contains(&k.as_str())),
        "FR-013: no other data is sent: {keys:?}"
    );
    if let Some(stream) = body.get("stream") {
        assert_eq!(stream, &json!(false));
    }
    assert_eq!(creds.calls(), read_pp());
}

#[tokio::test]
async fn trailing_slash_and_query_of_the_base_url_are_kept() {
    // spec 003 FR-001: {base}/chat/completions joined on the parsed URL; one empty
    // trailing segment dropped, a query (api-version) kept. Bite: string
    // concatenation (`/v1//chat/completions`, the query in the middle of the
    // path), or the query dropped.
    let server = server_with(chat_reply(json!(REPLY))).await;
    for base in [
        format!("{}/v1/?api-version={QUERY_SECRET}", server.uri()),
        format!("{}/v1?api-version={QUERY_SECRET}", server.uri()),
    ] {
        let got = run(&pp_settings(&base), &creds_with_key(KEY), timeouts(), RAW);
        assert_eq!(
            got,
            PostProcessOutcome::Applied(REPLY.to_string()),
            "{base}"
        );
    }
    let all = requests(&server).await;
    assert_eq!(all.len(), 2);
    for r in &all {
        assert_eq!(r.url.path(), "/v1/chat/completions");
        let expected_query = format!("api-version={QUERY_SECRET}");
        assert_eq!(r.url.query(), Some(expected_query.as_str()));
    }
}

// ---- the key: only when set, never from a failed read --------------------------

#[tokio::test]
async fn no_key_or_empty_key_sends_no_authorization() {
    // contract guarantee 2: Authorization iff a key is set; an empty stored key is
    // no key (the transcription rule, T-040). Bite: `Bearer ` with an empty key,
    // or the request refused without a key (local servers need none).
    let server = server_with(chat_reply(json!(REPLY))).await;
    for (label, creds) in [
        ("no key", FakeCredentialStore::new()),
        ("empty key", creds_with_key("")),
    ] {
        let got = run(
            &pp_settings(&format!("{}/v1", server.uri())),
            &creds,
            timeouts(),
            RAW,
        );
        assert_eq!(
            got,
            PostProcessOutcome::Applied(REPLY.to_string()),
            "{label}"
        );
        assert_eq!(creds.calls(), read_pp(), "{label}");
    }
    let all = requests(&server).await;
    assert_eq!(all.len(), 2);
    for r in &all {
        assert_eq!(header(r, "authorization"), None);
    }
}

#[tokio::test]
async fn key_read_error_is_no_key_and_still_applies() {
    // spec 003 Edge Cases / research R5: a credential read error is treated as no
    // key (deliberately unlike transcription's KeyStoreUnavailable, decision #44).
    // Bite: the #44 rule copied (a skip, no request), or a retry of the read.
    let server = server_with(chat_reply(json!(REPLY))).await;
    let creds = creds_with_key(KEY);
    creds.fail(
        CredentialOp::Read,
        KeySlot::PostProcessing,
        CredentialError { os_code: 1312 },
    );
    let got = run(
        &pp_settings(&format!("{}/v1", server.uri())),
        &creds,
        timeouts(),
        RAW,
    );
    assert_eq!(got, PostProcessOutcome::Applied(REPLY.to_string()));
    let r = only_request(&server).await;
    assert_eq!(header(&r, "authorization"), None);
    assert_eq!(creds.calls(), read_pp());
}

#[tokio::test]
async fn unusable_key_is_invalid_key_and_sends_nothing() {
    // The shared authorization rule (T-040 review 1 #4): a key that cannot be a
    // header value is InvalidKey and no request leaves. Bite: a second key rule
    // that drops the bad key and sends without it, or a header-build error
    // classified as Unreachable.
    let server = server_with(chat_reply(json!(REPLY))).await;
    for key in ["sk-test-a\nb", "sk-test-a\rb", "sk-test-a\u{7f}b"] {
        let got = run(
            &pp_settings(&format!("{}/v1", server.uri())),
            &creds_with_key(key),
            timeouts(),
            RAW,
        );
        assert_eq!(
            got,
            PostProcessOutcome::Skipped(SkipReason::InvalidKey),
            "{key:?}"
        );
    }
    assert_eq!(
        requests(&server).await.len(),
        0,
        "a request reached the server"
    );
}

// ---- Acceptance 2: failure branches -> one skip reason ---------------------------

#[tokio::test]
async fn status_401_and_403_are_invalid_key() {
    // research R4. Bite: Http{401}, or the key retried without Authorization.
    for status in [401u16, 403] {
        let server = server_with(ResponseTemplate::new(status)).await;
        let got = run(
            &pp_settings(&format!("{}/v1", server.uri())),
            &creds_with_key(KEY),
            timeouts(),
            RAW,
        );
        assert_eq!(
            got,
            PostProcessOutcome::Skipped(SkipReason::InvalidKey),
            "{status}"
        );
        let _ = only_request(&server).await;
    }
}

#[tokio::test]
async fn other_status_is_http_with_that_status() {
    // research R4: any other non-2xx -> Http{status}. Bite: a fixed status, the
    // error body parsed as a reply, 429 retried, 3xx/4xx/5xx collapsed into one.
    for status in [400u16, 404, 429, 500, 503] {
        let server = server_with(ResponseTemplate::new(status).set_body_json(json!({
            "choices": [{ "message": { "content": "error body is not a reply" } }]
        })))
        .await;
        let got = run(
            &pp_settings(&format!("{}/v1", server.uri())),
            &creds_with_key(KEY),
            timeouts(),
            RAW,
        );
        assert_eq!(
            got,
            PostProcessOutcome::Skipped(SkipReason::Http { status }),
            "{status}"
        );
        let _ = only_request(&server).await;
    }
}

#[tokio::test]
async fn unusable_reply_is_invalid_response() {
    // data-model "ChatReply": only choices[0].message.content as a string is read;
    // absent, null, non-string, empty or whitespace-only -> InvalidResponse, never
    // Applied("") and never a panic. Bite: content defaulted to "", a number or
    // null coerced to text, an untrimmed blank applied, lossy UTF-8, any unwrap.
    let cases: Vec<(&str, ResponseTemplate)> = vec![
        (
            "not JSON",
            ResponseTemplate::new(200).set_body_string(format!("hello {BODY_MARKER}")),
        ),
        ("empty body", ResponseTemplate::new(200)),
        (
            "no choices",
            ResponseTemplate::new(200).set_body_json(json!({ "text": BODY_MARKER })),
        ),
        (
            "choices empty",
            ResponseTemplate::new(200).set_body_json(json!({ "choices": [] })),
        ),
        (
            "choices not an array",
            ResponseTemplate::new(200).set_body_json(json!({ "choices": BODY_MARKER })),
        ),
        (
            "no message",
            ResponseTemplate::new(200)
                .set_body_json(json!({ "choices": [{ "text": BODY_MARKER }] })),
        ),
        (
            "content missing",
            ResponseTemplate::new(200)
                .set_body_json(json!({ "choices": [{ "message": { "role": "assistant" } }] })),
        ),
        ("content null", chat_reply(Value::Null)),
        ("content a number", chat_reply(json!(42))),
        ("content an array", chat_reply(json!([BODY_MARKER]))),
        ("content empty", chat_reply(json!(""))),
        ("content whitespace", chat_reply(json!(" \n\t\u{3000} "))),
        (
            "JSON array",
            ResponseTemplate::new(200).set_body_json(json!([BODY_MARKER])),
        ),
        (
            "invalid UTF-8",
            ResponseTemplate::new(200).set_body_raw(
                b"{\"choices\":[{\"message\":{\"content\":\"\xff\xfe BODY\"}}]}".to_vec(),
                "application/json",
            ),
        ),
    ];
    let mut wrong = Vec::new();
    for (name, template) in cases {
        let server = server_with(template).await;
        let got = run(
            &pp_settings(&format!("{}/v1", server.uri())),
            &creds_with_key(KEY),
            timeouts(),
            RAW,
        );
        if got != PostProcessOutcome::Skipped(SkipReason::InvalidResponse) {
            wrong.push(format!("{name}: {got:?}"));
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

#[tokio::test]
async fn reply_body_is_capped_at_1_mib() {
    // The shared capped read (engine/http.rs, the 1 MiB of T-040): a valid reply
    // of 1 MiB + 1 bytes is InvalidResponse, one of exactly 1 MiB is applied.
    // Bite: an uncapped read_to_end / `json()`, an off-by-one cap, or a second cap
    // of another size.
    let (over, _) = reply_of_len(MIB + 1);
    let server =
        server_with(ResponseTemplate::new(200).set_body_raw(over, "application/json")).await;
    let got = run(
        &pp_settings(&format!("{}/v1", server.uri())),
        &creds_with_key(KEY),
        timeouts(),
        RAW,
    );
    assert_eq!(
        got,
        PostProcessOutcome::Skipped(SkipReason::InvalidResponse),
        "1 MiB + 1"
    );

    let (exact, text) = reply_of_len(MIB);
    let server =
        server_with(ResponseTemplate::new(200).set_body_raw(exact, "application/json")).await;
    let got = run(
        &pp_settings(&format!("{}/v1", server.uri())),
        &creds_with_key(KEY),
        timeouts(),
        RAW,
    );
    assert!(
        got == PostProcessOutcome::Applied(text),
        "exactly 1 MiB must be applied: {}",
        match &got {
            PostProcessOutcome::Applied(t) => format!("Applied(len {})", t.len()),
            other => format!("{other:?}"),
        }
    );
}

#[test]
fn refused_port_is_unreachable_with_host_port() {
    // research R4: connection refused -> Unreachable{host} with the configured
    // base URL's host:port, never the error text. refused_timeouts() so the
    // refusal, not a timer, ends the connect on Windows too (F-005). Bite:
    // Timeout or InvalidResponse for a refusal, the whole URL (query included) as
    // host.
    let addr = refused_addr();
    let got = run(
        &pp_settings(&format!("http://{addr}/v1?api-version={QUERY_SECRET}")),
        &creds_with_key(KEY),
        refused_timeouts(),
        RAW,
    );
    assert_eq!(
        got,
        PostProcessOutcome::Skipped(SkipReason::Unreachable {
            host: addr.to_string()
        })
    );
}

#[tokio::test]
async fn late_reply_past_the_input_deadline_is_timeout() {
    // FR-24 / contract guarantee 4: the whole request is bounded by
    // input.timeouts.post_processing (here 300 ms against a 5 s answer), so the
    // call returns Timeout well before the answer. Bite: Timeouts::default() (15 s,
    // the 5 s answer is applied), the transcription or connect deadline instead of
    // post_processing, a read without the request timeout, Timeout mapped to
    // Unreachable.
    let server = server_with(chat_reply(json!(REPLY)).set_delay(Duration::from_secs(5))).await;
    let short = Timeouts {
        connect: Duration::from_secs(2),
        post_processing: Duration::from_millis(300),
        ..Timeouts::default()
    };
    let started = Instant::now();
    let got = run(
        &pp_settings(&format!("{}/v1", server.uri())),
        &creds_with_key(KEY),
        short,
        RAW,
    );
    let elapsed = started.elapsed();
    assert_eq!(got, PostProcessOutcome::Skipped(SkipReason::Timeout));
    assert!(elapsed < Duration::from_secs(3), "took {elapsed:?}");
}

#[tokio::test]
async fn cross_host_redirect_drops_the_key() {
    // Acceptance 2 / research R5: the key goes only to the configured endpoint.
    // The configured server answers 302 to another server (127.0.0.1, another
    // port: another origin); that server must see no header or URL carrying the
    // key, while the configured one got the Bearer key. The shared client keeps
    // reqwest's default redirect policy (R5), so the redirect is followed. Bite: a
    // redirect policy that re-attaches Authorization, the key sent in a header
    // reqwest does not treat as sensitive (`api-key`), or in the URL.
    let target = server_with(chat_reply(json!(REPLY))).await;
    let origin = server_with(
        ResponseTemplate::new(302)
            .insert_header("location", format!("{}/v1/chat/completions", target.uri())),
    )
    .await;
    let _ = run(
        &pp_settings(&format!("{}/v1", origin.uri())),
        &creds_with_key(KEY),
        timeouts(),
        RAW,
    );

    let first = only_request(&origin).await;
    let expected_auth = format!("Bearer {KEY}");
    assert_eq!(
        header(&first, "authorization"),
        Some(expected_auth.as_str())
    );

    let followed = requests(&target).await;
    assert_eq!(
        followed.len(),
        1,
        "the redirect is followed once (default policy, research R5)"
    );
    for r in &followed {
        assert_eq!(
            header(r, "authorization"),
            None,
            "Authorization on the redirect"
        );
        let leaked: Vec<String> = r
            .headers
            .iter()
            .filter(|(_, v)| v.to_str().is_ok_and(|v| v.contains(KEY)))
            .map(|(k, _)| k.to_string())
            .collect();
        assert!(leaked.is_empty(), "key in headers {leaked:?}");
        assert!(!r.url.as_str().contains(KEY), "key in the URL");
    }
}

// ---- off or not set up: no key read, no request ---------------------------------

#[tokio::test]
async fn disabled_reads_no_key_and_sends_nothing() {
    // spec 003 FR-002 / NFR-05: post-processing off -> NotRun, no credential read
    // (no Credential Manager access) and no request, whatever else is stored.
    // Bite: the key read before the enabled check, a request with enabled false,
    // Skipped(NotConfigured) for a switched-off feature.
    let server = server_with(chat_reply(json!(REPLY))).await;
    let mut s = pp_settings(&format!("{}/v1", server.uri()));
    s.enabled = false;
    let creds = creds_with_key(KEY);
    let got = run(&s, &creds, timeouts(), RAW);
    assert_eq!(got, PostProcessOutcome::NotRun);
    assert_eq!(creds.calls(), vec![]);
    assert_eq!(requests(&server).await.len(), 0);
}

#[tokio::test]
async fn unusable_base_url_is_not_configured_without_key_read() {
    // decision #91(2): enabled with a base URL that fails check_base_url ->
    // Skipped(NotConfigured), no credential read before the URL check (as
    // engine_for), no request. The userinfo case points at the live mock, so a
    // request would show. Bite: InvalidResponse or a silent NotRun for a broken
    // config, the key read first, the userinfo URL sent anyway.
    let server = server_with(chat_reply(json!(REPLY))).await;
    let port = server.address().port();
    let mut wrong = Vec::new();
    for base in [
        String::new(),
        "   ".to_string(),
        "not a url".to_string(),
        format!("ftp://127.0.0.1:{port}/v1"),
        format!("http://user:pw@127.0.0.1:{port}/v1"),
    ] {
        let creds = creds_with_key(KEY);
        let got = run(&pp_settings(&base), &creds, timeouts(), RAW);
        if got != PostProcessOutcome::Skipped(SkipReason::NotConfigured) {
            wrong.push(format!("{base:?}: {got:?}"));
        }
        if !creds.calls().is_empty() {
            wrong.push(format!("{base:?}: credential calls {:?}", creds.calls()));
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    assert_eq!(requests(&server).await.len(), 0);
}

// ---- nothing secret in an outcome -------------------------------------------------

#[tokio::test]
async fn no_skip_carries_key_prompt_raw_text_query_or_body() {
    // contract guarantee 5 / FR-014 / P-009: a skip's Debug, code and message
    // params never hold the key, the prompt, the raw transcript, the URL query or
    // the response body, on every failure path. Bite: a reason built from the
    // reqwest error text (URL with query), from the response body, or carrying
    // the request.
    let refused = refused_addr();
    let status_500 = server_with(
        ResponseTemplate::new(500).set_body_string(format!("{BODY_MARKER} {KEY} {PROMPT}")),
    )
    .await;
    let bad_body =
        server_with(ResponseTemplate::new(200).set_body_string(format!("{BODY_MARKER} {RAW}")))
            .await;
    let unauthorized = server_with(ResponseTemplate::new(401).set_body_string(BODY_MARKER)).await;
    let query = format!("?api-version={QUERY_SECRET}");
    let cases = [
        (format!("http://{refused}/v1{query}"), refused_timeouts()),
        (format!("{}/v1{query}", status_500.uri()), timeouts()),
        (format!("{}/v1{query}", bad_body.uri()), timeouts()),
        (format!("{}/v1{query}", unauthorized.uri()), timeouts()),
    ];
    let mut wrong = Vec::new();
    for (base, t) in cases {
        let got = run(&pp_settings(&base), &creds_with_key(KEY), t, RAW);
        let PostProcessOutcome::Skipped(reason) = &got else {
            wrong.push(format!("{base}: not a skip: {got:?}"));
            continue;
        };
        let mut seen = format!("{got:?} {} ", reason.code());
        for (k, v) in reason.message_params() {
            seen.push_str(&format!("{k}={v} "));
        }
        for secret in [KEY, PROMPT, RAW, QUERY_SECRET, BODY_MARKER] {
            if seen.contains(secret) {
                wrong.push(format!("{base}: {secret:?} in {seen:?}"));
            }
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}
