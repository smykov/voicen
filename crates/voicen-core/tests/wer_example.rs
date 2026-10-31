//! T-140 (validation r1): the owner-run `wer` example never prints transcript text,
//! reference text or the API key (analysis invariant; NFR-04, FR-20).
//!
//! The example is run as a child process in `pairs` mode against a loopback wiremock
//! server and a temp directory holding one fake WAV and one fake reference. Nothing
//! leaves the host; every value is an obvious fake marker. The child's stdout and
//! stderr are the "output" the invariant speaks of. The gate still never runs a real
//! measurement (#185): the server and the audio are fakes.
//!
//! These tests are green on a correct example; they guard it by mutation (the catalog
//! `docs/tasks/T-140.mutants/` entries `25..`): printing the transcript, the reference
//! or the key turns them red.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::json;
use voicen_core::audio::{wav, AudioBuffer};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-test-wer-keymarkzzz";
/// Reference words: absent from every server answer, so a hit in the output is the
/// reference being printed.
const REFERENCE: &str = "Refmarkone refmarktwo";
/// What the fake server transcribes; two substitutions against `REFERENCE`.
const TRANSCRIPT: &str = "transmarkone transmarktwo";
const ERROR_BODY: &str = "serverbodymarkq";

/// 200 ms of a 16 kHz sawtooth, as a PCM16 WAV.
fn fake_wav() -> Vec<u8> {
    wav::encode(&AudioBuffer::from_16k_mono(
        (0..3200).map(|i| ((i % 64) as i16 - 32) * 512).collect(),
    ))
}

/// A temp directory removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let path =
            std::env::temp_dir().join(format!("voicen-wer-example-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp dir");
        Dir(path)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The built example: `cargo build --example wer` (a no-op when fresh; `cargo test`
/// of a single target does not build examples), then the binary next to `deps/`.
fn example() -> PathBuf {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args([
            "build",
            "-j2",
            "--quiet",
            "-p",
            "voicen-core",
            "--example",
            "wer",
        ])
        .status()
        .expect("cargo starts");
    assert!(status.success(), "cargo build --example wer failed");
    let exe = std::env::current_exe().expect("test exe path");
    let profile_dir = exe
        .parent()
        .and_then(Path::parent)
        .expect("target/<profile>/deps/<test>");
    let name = if cfg!(windows) { "wer.exe" } else { "wer" };
    let path = profile_dir.join("examples").join(name);
    assert!(path.is_file(), "example binary missing: {}", path.display());
    path
}

/// Runs `wer pairs <dir> [ru]` against `base_url` with the fake key; stdout and stderr
/// joined, lowercased.
fn run_pairs(dir: &Dir, base_url: &str, with_reference: bool) -> (Output, String) {
    std::fs::write(dir.0.join("item1.wav"), fake_wav()).expect("write wav");
    if with_reference {
        std::fs::write(dir.0.join("item1.txt"), REFERENCE).expect("write reference");
    }
    let out = Command::new(example())
        .arg("pairs")
        .arg(&dir.0)
        .arg("ru")
        .env("VOICEN_WER_KEY", KEY)
        .env("VOICEN_WER_BASE_URL", base_url)
        .env("VOICEN_WER_MODEL", "fake-model")
        .output()
        .expect("example starts");
    let all = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
    .to_lowercase();
    (out, all)
}

/// The invariant: no secret-bearing token in `output` (already lowercased).
fn assert_no_secrets(output: &str, also: &[&str]) {
    let mut markers = vec![
        KEY,
        "keymarkzzz",
        "refmarkone",
        "refmarktwo",
        "transmarkone",
        "transmarktwo",
    ];
    markers.extend_from_slice(also);
    for marker in markers {
        assert!(
            !output.contains(&marker.to_lowercase()),
            "the example printed {marker:?}:\n{output}"
        );
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

#[tokio::test]
async fn wer_example_output_has_no_transcript_reference_or_key_on_success() {
    let server =
        server_with(ResponseTemplate::new(200).set_body_json(json!({ "text": TRANSCRIPT }))).await;
    let dir = Dir::new("ok");
    let (out, all) = run_pairs(&dir, &server.uri(), true);
    // The run really scored the fake answer (else the checks below prove nothing).
    assert!(out.status.success(), "example failed:\n{all}");
    assert!(
        all.contains("item1 s=2 d=0 i=0 n=2 wer=100.00%"),
        "no score line for the fake item:\n{all}"
    );
    assert!(
        all.contains("wer: total over 1 scored items (0 failed)"),
        "{all}"
    );
    let seen = server.received_requests().await.expect("recording is on");
    assert_eq!(seen.len(), 1, "one request for one item");
    assert_no_secrets(&all, &[]);
}

#[tokio::test]
async fn wer_example_output_has_no_server_text_reference_or_key_when_the_server_fails() {
    let server = server_with(ResponseTemplate::new(500).set_body_string(ERROR_BODY)).await;
    let dir = Dir::new("failed");
    let (out, all) = run_pairs(&dir, &server.uri(), true);
    assert_eq!(out.status.code(), Some(1), "a failed item exits 1:\n{all}");
    assert!(all.contains("item1 failed"), "no failure line:\n{all}");
    assert_no_secrets(&all, &[ERROR_BODY]);
}

#[tokio::test]
async fn wer_example_output_has_no_transcript_or_key_when_the_reference_is_missing() {
    let server =
        server_with(ResponseTemplate::new(200).set_body_json(json!({ "text": TRANSCRIPT }))).await;
    let dir = Dir::new("noref");
    let (out, all) = run_pairs(&dir, &server.uri(), false);
    assert!(
        !out.status.success(),
        "nothing to score is a refusal:\n{all}"
    );
    assert!(all.contains("item1 skipped"), "no skip line:\n{all}");
    let seen = server.received_requests().await.expect("recording is on");
    assert!(seen.is_empty(), "a skipped item sends no request");
    assert_no_secrets(&all, &[]);
}

#[tokio::test]
async fn wer_example_output_has_no_key_when_the_base_url_is_refused() {
    let dir = Dir::new("refused");
    // Not an http(s) URL: the example refuses before any request.
    let (out, all) = run_pairs(&dir, "ftp://example.com/v1", true);
    assert_eq!(out.status.code(), Some(2), "a refused URL exits 2:\n{all}");
    assert!(
        all.contains("voicen_wer_base_url"),
        "no refusal line:\n{all}"
    );
    assert_no_secrets(&all, &[]);
}
