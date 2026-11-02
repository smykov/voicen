//! T-140 (validation r1): the owner-run `wer` example never prints transcript text,
//! reference text or the API key (analysis invariant; NFR-04, FR-20).
//!
//! The example is run as a child process against a loopback wiremock server and a temp
//! directory holding one fake WAV and one fake reference: in `pairs` mode through every
//! path of the shared `run()`, in `librispeech` mode through its own lines up to the
//! 30-minute refusal and, with one silent WAV of exactly 30 min, past it; in both modes
//! also through the branch where a readable reference sits next to an unreadable WAV
//! (validation r2, H1 and H2). Nothing leaves the host; every value is an obvious fake
//! marker. The child's stdout and stderr are the "output" the invariant speaks of. The
//! gate and CI run the example only like this and never the real measurement (decision
//! #221; research R23, T051 as amended).
//!
//! These tests are green on a correct example; they guard it by mutation (the catalog
//! `docs/tasks/T-140.mutants/` entries `25..`): printing the transcript, the reference
//! or the key turns them red, and so does a wrong figure or a dropped key (validation
//! r3, entries `114..`).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

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
/// Against `SCORED_ANSWER`: the shared words differ only in case and punctuation, and
/// one reference word has no counterpart, so argument order and normalization both
/// change the figure.
const SCORED_REFERENCE: &str = "Hello, World! extra";
const SCORED_ANSWER: &str = "HELLO, world!";
/// The counts of `SCORED_ANSWER` against `SCORED_REFERENCE`.
const SCORED_COUNTS: &str = "s=0 d=1 i=0 n=3 wer=33.33%";
/// The words of the scored fixtures; none of them may reach the output.
const SCORED_WORDS: [&str; 3] = ["hello", "world", "extra"];

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
/// Built once per test binary, so parallel tests share one cargo run.
fn example() -> &'static Path {
    static EXAMPLE: OnceLock<PathBuf> = OnceLock::new();
    EXAMPLE.get_or_init(|| {
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
    })
}

/// Runs `wer pairs <dir> [ru]` against `base_url` with the fake key; stdout and stderr
/// joined, lowercased.
fn run_pairs(dir: &Dir, base_url: &str, with_reference: bool) -> (Output, String) {
    std::fs::write(dir.0.join("item1.wav"), fake_wav()).expect("write wav");
    if with_reference {
        std::fs::write(dir.0.join("item1.txt"), REFERENCE).expect("write reference");
    }
    run_example(&["pairs", path_arg(dir), "ru"], base_url)
}

fn path_arg(dir: &Dir) -> &str {
    dir.0.to_str().expect("temp dir path is UTF-8")
}

/// Runs the example with `args` against `base_url` with the fake key; stdout and stderr
/// joined, lowercased.
fn run_example(args: &[&str], base_url: &str) -> (Output, String) {
    let out = Command::new(example())
        .args(args)
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
async fn wer_example_librispeech_refusal_has_no_reference_or_key() {
    let server =
        server_with(ResponseTemplate::new(200).set_body_json(json!({ "text": TRANSCRIPT }))).await;
    let dir = Dir::new("librispeech");
    // One real manifest id: `read_transcripts` and the item loop run before the refusal.
    let chapter = dir.0.join("1089").join("134686");
    std::fs::create_dir_all(&chapter).expect("chapter dir");
    std::fs::write(
        chapter.join("1089-134686.trans.txt"),
        format!("1089-134686-0000 {REFERENCE}\n"),
    )
    .expect("write trans.txt");
    std::fs::write(chapter.join("1089-134686-0000.wav"), fake_wav()).expect("write wav");
    let (out, all) = run_example(&["librispeech", path_arg(&dir)], &server.uri());
    assert_eq!(
        out.status.code(),
        Some(1),
        "too little audio exits 1:\n{all}"
    );
    // The run really loaded the fake item (else the checks below prove nothing).
    assert!(all.contains("wer: 1 of "), "no found count of 1:\n{all}");
    assert!(all.contains("wer: refused:"), "no refusal line:\n{all}");
    let seen = server.received_requests().await.expect("recording is on");
    assert!(seen.is_empty(), "a refused set sends no request");
    assert_no_secrets(&all, &[]);
}

/// Validation r2, H1: the reference is read, the WAV next to it is not, so the item is
/// skipped with its reference at hand.
#[tokio::test]
async fn wer_example_pairs_output_has_no_reference_when_the_wav_is_unreadable() {
    let server =
        server_with(ResponseTemplate::new(200).set_body_json(json!({ "text": TRANSCRIPT }))).await;
    let dir = Dir::new("pairs-bad-wav");
    std::fs::write(dir.0.join("item1.wav"), b"not a wav").expect("write wav");
    std::fs::write(dir.0.join("item1.txt"), REFERENCE).expect("write reference");
    let (out, all) = run_example(&["pairs", path_arg(&dir), "ru"], &server.uri());
    assert_eq!(
        out.status.code(),
        Some(1),
        "nothing to score exits 1:\n{all}"
    );
    // The run really skipped this item (else the checks below prove nothing).
    assert!(all.contains("item1 skipped"), "no skip line:\n{all}");
    assert!(all.contains("wer: 0 pairs found (1 skipped)"), "{all}");
    let seen = server.received_requests().await.expect("recording is on");
    assert!(seen.is_empty(), "a skipped item sends no request");
    assert_no_secrets(&all, &[]);
}

/// Validation r2, H2: the manifest id has its reference in `trans.txt`, its WAV is
/// unreadable, so it counts as missing with its reference at hand.
#[tokio::test]
async fn wer_example_librispeech_output_has_no_reference_when_the_wav_is_unreadable() {
    let server =
        server_with(ResponseTemplate::new(200).set_body_json(json!({ "text": TRANSCRIPT }))).await;
    let dir = Dir::new("librispeech-bad-wav");
    let chapter = dir.0.join("1089").join("134686");
    std::fs::create_dir_all(&chapter).expect("chapter dir");
    std::fs::write(
        chapter.join("1089-134686.trans.txt"),
        format!("1089-134686-0000 {REFERENCE}\n"),
    )
    .expect("write trans.txt");
    std::fs::write(chapter.join("1089-134686-0000.wav"), b"not a wav").expect("write wav");
    let (out, all) = run_example(&["librispeech", path_arg(&dir)], &server.uri());
    assert_eq!(
        out.status.code(),
        Some(1),
        "too little audio exits 1:\n{all}"
    );
    // The item was not loaded (else this is the other librispeech test).
    assert!(all.contains("wer: 0 of "), "no found count of 0:\n{all}");
    assert!(all.contains("wer: refused:"), "no refusal line:\n{all}");
    let seen = server.received_requests().await.expect("recording is on");
    assert!(seen.is_empty(), "a refused set sends no request");
    assert_no_secrets(&all, &[]);
}

/// `line` is one whole line of `all` (already lowercased): a figure inside a longer line
/// would not tell the item line from the total.
fn assert_has_line(all: &str, line: &str) {
    assert!(
        all.lines().any(|l| l == line),
        "no line {line:?} in:\n{all}"
    );
}

/// Validation r3 (116, 117, 118, 122, 114): the reported figure is
/// `score(reference, hypothesis)` over `normalize` on both sides, the total adds it, and
/// the key from the environment reaches the request.
#[tokio::test]
async fn wer_example_pairs_scores_normalized_reference_against_answer_and_sends_the_key() {
    let server =
        server_with(ResponseTemplate::new(200).set_body_json(json!({ "text": SCORED_ANSWER })))
            .await;
    let dir = Dir::new("scored");
    std::fs::write(dir.0.join("item1.wav"), fake_wav()).expect("write wav");
    std::fs::write(dir.0.join("item1.txt"), SCORED_REFERENCE).expect("write reference");
    let (out, all) = run_example(&["pairs", path_arg(&dir), "ru"], &server.uri());
    assert!(out.status.success(), "example failed:\n{all}");
    assert_has_line(&all, &format!("item1 {SCORED_COUNTS}"));
    assert_has_line(
        &all,
        &format!("wer: total over 1 scored items (0 failed): {SCORED_COUNTS}"),
    );
    let seen = server.received_requests().await.expect("recording is on");
    assert_eq!(seen.len(), 1, "one request for one item");
    let authorization = seen
        .first()
        .and_then(|request| request.headers.get("authorization"))
        .and_then(|value| value.to_str().ok());
    assert_eq!(authorization, Some(format!("Bearer {KEY}").as_str()));
    assert_no_secrets(&all, &SCORED_WORDS);
}

/// Validation r3 (114, 115): the set figure is the sum of edits over the sum of reference
/// words. 1/3 and 3/5 give 4/8 = 50.00%; the last item alone is 60.00%, the mean of the
/// rates 46.67%.
#[tokio::test]
async fn wer_example_pairs_total_is_sum_of_edits_over_sum_of_reference_words() {
    let server =
        server_with(ResponseTemplate::new(200).set_body_json(json!({ "text": SCORED_ANSWER })))
            .await;
    let dir = Dir::new("scored-set");
    for (name, reference) in [
        ("item1", SCORED_REFERENCE),
        ("item2", "Hello, World! extra extra extra"),
    ] {
        std::fs::write(dir.0.join(format!("{name}.wav")), fake_wav()).expect("write wav");
        std::fs::write(dir.0.join(format!("{name}.txt")), reference).expect("write reference");
    }
    let (out, all) = run_example(&["pairs", path_arg(&dir), "ru"], &server.uri());
    assert!(out.status.success(), "example failed:\n{all}");
    assert_has_line(&all, &format!("item1 {SCORED_COUNTS}"));
    assert_has_line(&all, "item2 s=0 d=3 i=0 n=5 wer=60.00%");
    assert_has_line(
        &all,
        "wer: total over 2 scored items (0 failed): s=0 d=4 i=0 n=8 wer=50.00%",
    );
    assert_no_secrets(&all, &SCORED_WORDS);
}

/// Validation r3 (119): `librispeech` normalizes its own reference. The path lies behind
/// the refusal, so the one fake item holds exactly 30 min: silence, written as a header
/// and a length, so the file costs no disk where the filesystem has holes. The limit is
/// the example's own and stays as it is.
#[tokio::test]
async fn wer_example_librispeech_past_the_refusal_scores_the_normalized_reference() {
    const THIRTY_MINUTES_BYTES: u32 = 30 * 60 * 16_000 * 2;
    let server =
        server_with(ResponseTemplate::new(200).set_body_json(json!({ "text": SCORED_ANSWER })))
            .await;
    let dir = Dir::new("librispeech-30-min");
    let chapter = dir.0.join("1089").join("134686");
    std::fs::create_dir_all(&chapter).expect("chapter dir");
    std::fs::write(
        chapter.join("1089-134686.trans.txt"),
        format!("1089-134686-0000 {SCORED_REFERENCE}\n"),
    )
    .expect("write trans.txt");
    // The header of an empty WAV, with its two sizes set to the 30 minutes.
    let mut header = wav::encode(&AudioBuffer::from_16k_mono(Vec::new()));
    header.splice(4..8, (THIRTY_MINUTES_BYTES + 36).to_le_bytes());
    header.splice(40..44, THIRTY_MINUTES_BYTES.to_le_bytes());
    let path = chapter.join("1089-134686-0000.wav");
    std::fs::write(&path, &header).expect("write wav header");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .and_then(|file| file.set_len(44 + u64::from(THIRTY_MINUTES_BYTES)))
        .expect("extend wav");
    let (out, all) = run_example(&["librispeech", path_arg(&dir)], &server.uri());
    assert!(out.status.success(), "example failed:\n{all}");
    assert!(all.contains("wer: 1 of "), "no found count of 1:\n{all}");
    assert!(!all.contains("wer: refused:"), "refused:\n{all}");
    assert_has_line(&all, &format!("1089-134686-0000 {SCORED_COUNTS}"));
    assert_has_line(
        &all,
        &format!("wer: total over 1 scored items (0 failed): {SCORED_COUNTS}"),
    );
    assert_no_secrets(&all, &SCORED_WORDS);
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
