//! T-140 (validation r1), T-163: the owner-run `wer` example never prints transcript
//! text, reference text or the API key (analysis invariant; NFR-04, FR-20).
//!
//! The example is run as a child process against a loopback wiremock server and a temp
//! directory holding fake WAVs and fake references. Every item (a `pairs` file or a
//! LibriSpeech `<spk>-<ch>` group) goes through one final pass over a temp call folder,
//! so the mock sees the pass's chunk requests (and, on a 400, its time-less resend), not
//! one request per file. In `pairs` mode the tests go through every path of the shared
//! `run()`; in `librispeech` mode through its own lines up to the 30-minute refusal and,
//! with one silent WAV of exactly 30 min, past it; in both modes also through the branch
//! where a readable reference sits next to an unreadable WAV (validation r2, H1 and H2).
//! Nothing leaves the host; every value is an obvious fake marker. The child's stdout
//! and stderr are the "output" the invariant speaks of. The gate and CI run the example
//! only like this and never the real measurement (decision #221; research R23, T051 as
//! amended).
//!
//! These tests are green on a correct example; they guard it by mutation (the catalogs
//! `docs/tasks/T-140.mutants/` entries `25..`, `114..`, `201..` and
//! `docs/tasks/T-163.mutants/`): printing the transcript, the reference or the key turns
//! them red, and so does a wrong figure, a dropped key, an empty reference mishandled,
//! the key on a path that sends no request, a pass that fails scored as an empty
//! hypothesis, a leaked call folder, a wrong group order or a missing key accepted.

#[path = "common/mock_http.rs"]
mod mock_http;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

use serde_json::json;
use voicen_core::audio::{wav, AudioBuffer};
use voicen_core::test_support::fixtures;
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

/// `seconds` of synthetic syllabic speech (the energy detector counts it as speech;
/// the pass cuts and sends only speech), as 16 kHz samples. T-163: the final pass sends
/// nothing for audio without speech, so a fixture item must hold some.
fn speech_samples(seconds: u32) -> Vec<i16> {
    let mut out = Vec::new();
    for seed in 0..seconds.div_ceil(20) {
        let piece = fixtures::speech(
            u64::from(seed) + 1,
            f64::from((seconds - seed * 20).min(20)),
            0.0,
            -12.0,
        );
        out.extend_from_slice(piece.samples());
    }
    out
}

/// 3 s of speech as a PCM16 WAV: one chunk, so one request.
fn fake_wav() -> Vec<u8> {
    wav::encode(&AudioBuffer::from_16k_mono(speech_samples(3)))
}

/// `seconds` of speech as a PCM16 WAV; past 120 s the pass must cut it into several
/// requests (`TIMED_MAX_MS`).
fn long_wav(seconds: u32) -> Vec<u8> {
    wav::encode(&AudioBuffer::from_16k_mono(speech_samples(seconds)))
}

/// A timed (`verbose_json`) answer: one segment with `text`.
fn timed_body(text: &str) -> serde_json::Value {
    json!({
        "task": "transcribe",
        "language": "english",
        "duration": 1.0,
        "text": text,
        "segments": [{"id": 0, "start": 0.0, "end": 1.0, "text": text}]
    })
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
    let server = mock_http::start().await;
    Mock::given(any())
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn wer_example_output_has_no_transcript_reference_or_key_on_success() {
    let server =
        server_with(ResponseTemplate::new(200).set_body_json(timed_body(TRANSCRIPT))).await;
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
    assert!(
        all.lines().any(|l| l == "item1 failed provider"),
        "a 500 is a failed item with the code `provider`:\n{all}"
    );
    assert_no_secrets(&all, &[ERROR_BODY]);
}

#[tokio::test]
async fn wer_example_output_has_no_transcript_or_key_when_the_reference_is_missing() {
    let server =
        server_with(ResponseTemplate::new(200).set_body_json(timed_body(TRANSCRIPT))).await;
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
        server_with(ResponseTemplate::new(200).set_body_json(timed_body(TRANSCRIPT))).await;
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
        server_with(ResponseTemplate::new(200).set_body_json(timed_body(TRANSCRIPT))).await;
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
        server_with(ResponseTemplate::new(200).set_body_json(timed_body(TRANSCRIPT))).await;
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
        server_with(ResponseTemplate::new(200).set_body_json(timed_body(SCORED_ANSWER))).await;
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
        server_with(ResponseTemplate::new(200).set_body_json(timed_body(SCORED_ANSWER))).await;
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
        server_with(ResponseTemplate::new(200).set_body_json(timed_body(SCORED_ANSWER))).await;
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
    // 3 s of speech at the start, silence (a hole) after it: the pass has one chunk to
    // send, and the example reads the 30 minutes.
    let burst: Vec<u8> = speech_samples(3)
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect();
    header.extend_from_slice(&burst);
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
    assert_has_line(&all, &format!("1089-134686 {SCORED_COUNTS}"));
    assert_has_line(
        &all,
        &format!("wer: total over 1 scored items (0 failed): {SCORED_COUNTS}"),
    );
    assert_no_secrets(&all, &SCORED_WORDS);
}

/// Validation r4, group A (201-204; decision #238): an empty reference runs through the
/// example. The reference normalizes to no words, so every answer word is an insertion,
/// no rate exists (`n/a`, never `inf%`, `NaN%` or `0.00%`) and the total still adds the
/// insertions; the run neither panics nor fails.
#[tokio::test]
async fn wer_example_pairs_empty_reference_counts_insertions_without_a_rate() {
    let server =
        server_with(ResponseTemplate::new(200).set_body_json(timed_body(SCORED_ANSWER))).await;
    let dir = Dir::new("empty-reference");
    std::fs::write(dir.0.join("item1.wav"), fake_wav()).expect("write wav");
    std::fs::write(dir.0.join("item1.txt"), " ,. ").expect("write reference");
    let (out, all) = run_example(&["pairs", path_arg(&dir), "ru"], &server.uri());
    assert_eq!(out.status.code(), Some(0), "example failed:\n{all}");
    assert_has_line(&all, "item1 s=0 d=0 i=2 n=0 wer=n/a");
    assert_has_line(
        &all,
        "wer: total over 1 scored items (0 failed): s=0 d=0 i=2 n=0 wer=n/a",
    );
    let seen = server.received_requests().await.expect("recording is on");
    assert_eq!(seen.len(), 1, "one request for one item");
    assert_no_secrets(&all, &SCORED_WORDS);
}

/// Validation r4, group B (207-209; decision #238): the paths that send no request, the
/// usage, a `verify-archive` mismatch and a `pairs` directory that cannot be listed,
/// print no key either, though the key is in their environment.
#[test]
fn wer_example_paths_without_a_request_print_no_key() {
    let dir = Dir::new("no-request");
    let archive = dir.0.join("fake-archive.tar.gz");
    std::fs::write(&archive, b"not the test-clean archive").expect("write fake archive");
    let archive = archive.to_str().expect("temp path is UTF-8");
    let gone = dir.0.join("gone");
    let gone = gone.to_str().expect("temp path is UTF-8");
    // No request is made on these paths; the URL is a closed loopback port all the same.
    let base_url = "http://127.0.0.1:9/v1";
    for (args, code, marker) in [
        (vec!["nonsense"], 2, "usage:"),
        (
            vec!["verify-archive", archive],
            1,
            "differs from the pinned",
        ),
        (vec!["pairs", gone], 2, "wer: cannot list"),
    ] {
        let (out, all) = run_example(&args, base_url);
        assert_eq!(out.status.code(), Some(code), "{args:?}:\n{all}");
        // The path really ran (else the check below proves nothing).
        assert!(all.contains(marker), "{args:?}: no {marker:?} in:\n{all}");
        assert_no_secrets(&all, &[]);
    }
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

// ---- T-163: the example measures the final pass ------------------------------------------
//
// Every item goes through `call::final_pass::run_pass` over a temp call folder. Observable
// from outside: the requests the mock sees (chunks, not one per file), the line of the
// group id, the pass mode and chunk counts on a line of their own (`mode=<timed|timeless>`,
// `chunks=<n>`), the key refusal and the temp folder removed.

/// Runs the example like `run_example`, with the key and the child's temp directory chosen.
fn run_custom(
    args: &[&str],
    base_url: &str,
    key: Option<&str>,
    tmpdir: Option<&Path>,
) -> (Output, String) {
    let mut command = Command::new(example());
    command
        .args(args)
        .env_remove("VOICEN_WER_KEY")
        .env("VOICEN_WER_BASE_URL", base_url)
        .env("VOICEN_WER_MODEL", "fake-model");
    if let Some(key) = key {
        command.env("VOICEN_WER_KEY", key);
    }
    if let Some(tmpdir) = tmpdir {
        command
            .env("TMPDIR", tmpdir)
            .env("TEMP", tmpdir)
            .env("TMP", tmpdir);
    }
    let out = command.output().expect("example starts");
    let all = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
    .to_lowercase();
    (out, all)
}

/// The number after `field` (for example `i=`) on the line of `all` that starts with `prefix`.
fn field_of(all: &str, prefix: &str, field: &str) -> Option<u64> {
    let line = all.lines().find(|l| l.starts_with(prefix))?;
    let rest = line.split(field).nth(1)?;
    rest.split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// Acceptance 1 (pairs): a pair of 150 s of speech is cut by the chunker, so the mock sees
/// several requests, and the hypothesis is the joined answers: the reference is one word,
/// every answer one word, so the extra words are insertions. Bites: a per-file
/// `Engine::transcribe` (one request), a hypothesis of the first chunk only.
#[tokio::test]
async fn wer_example_pairs_long_item_goes_out_as_several_chunk_requests() {
    let server = server_with(ResponseTemplate::new(200).set_body_json(timed_body("hello"))).await;
    let dir = Dir::new("long-pair");
    std::fs::write(dir.0.join("item1.wav"), long_wav(150)).expect("write wav");
    std::fs::write(dir.0.join("item1.txt"), "hello").expect("write reference");
    let (out, all) = run_example(&["pairs", path_arg(&dir), "ru"], &server.uri());
    assert!(out.status.success(), "example failed:\n{all}");
    let seen = server.received_requests().await.expect("recording is on");
    assert!(
        seen.len() >= 2,
        "150 s of speech is more than one chunk, got {} requests:\n{all}",
        seen.len()
    );
    assert_eq!(field_of(&all, "item1 s=", " n="), Some(1), "{all}");
    let inserted = field_of(&all, "item1 s=", " i=").expect("score line");
    assert!(
        (1..seen.len() as u64).contains(&inserted),
        "the joined answers of every chunk are the hypothesis ({} requests):\n{all}",
        seen.len()
    );
    assert!(all.contains("mode=timed"), "no pass mode:\n{all}");
    assert_eq!(
        field_of(&all, "item1 pass", "chunks="),
        Some(seen.len() as u64),
        "the reported chunk count is the number of chunk requests:\n{all}"
    );
    assert_no_secrets(&all, &["hello"]);
}

/// Acceptance 1 (librispeech): the utterances of a `<spk>-<ch>` group are concatenated in
/// manifest order and scored as one item named by the group; the reference is the
/// utterances' references joined (4 words); the group is cut into several requests. A
/// 30-minute file of another group lifts the set over the floor. Bites: per-utterance
/// items (lines `1089-134686-0000`), references of the first utterance only.
#[tokio::test]
async fn wer_example_librispeech_scores_a_speaker_group_as_one_item() {
    const THIRTY_MINUTES_BYTES: u32 = 30 * 60 * 16_000 * 2;
    let server = server_with(ResponseTemplate::new(200).set_body_json(timed_body("hello"))).await;
    let dir = Dir::new("librispeech-group");
    let chapter = dir.0.join("1089").join("134686");
    std::fs::create_dir_all(&chapter).expect("chapter dir");
    let ids = [
        "1089-134686-0000",
        "1089-134686-0001",
        "1089-134686-0002",
        "1089-134686-0003",
    ];
    let trans: String = ids.iter().map(|id| format!("{id} hello\n")).collect();
    std::fs::write(chapter.join("1089-134686.trans.txt"), trans).expect("write trans.txt");
    for id in ids {
        std::fs::write(chapter.join(format!("{id}.wav")), long_wav(40)).expect("write wav");
    }
    // The other group: 3 s of speech, then silence up to 30 min (a hole).
    let other = dir.0.join("1188").join("133604");
    std::fs::create_dir_all(&other).expect("chapter dir");
    std::fs::write(
        other.join("1188-133604.trans.txt"),
        "1188-133604-0000 hello\n",
    )
    .expect("write trans.txt");
    let mut header = wav::encode(&AudioBuffer::from_16k_mono(Vec::new()));
    header.splice(4..8, (THIRTY_MINUTES_BYTES + 36).to_le_bytes());
    header.splice(40..44, THIRTY_MINUTES_BYTES.to_le_bytes());
    header.extend(speech_samples(3).iter().flat_map(|s| s.to_le_bytes()));
    let path = other.join("1188-133604-0000.wav");
    std::fs::write(&path, &header).expect("write wav");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .and_then(|file| file.set_len(44 + u64::from(THIRTY_MINUTES_BYTES)))
        .expect("extend wav");
    let (out, all) = run_example(&["librispeech", path_arg(&dir)], &server.uri());
    assert!(out.status.success(), "example failed:\n{all}");
    assert_eq!(field_of(&all, "1089-134686 s=", " n="), Some(4), "{all}");
    assert!(
        !all.lines().any(|l| l.starts_with("1089-134686-0000")),
        "no per-utterance line:\n{all}"
    );
    assert!(
        all.lines().any(|l| l.starts_with("1188-133604 s=")),
        "{all}"
    );
    let seen = server.received_requests().await.expect("recording is on");
    assert!(
        seen.len() >= 3,
        "160 s of speech is at least two chunks, plus the other group's one: {} requests",
        seen.len()
    );
    let position = |prefix: &str| all.lines().position(|l| l.starts_with(prefix));
    assert!(
        position("1089-134686 s=") < position("1188-133604 s="),
        "groups are scored in the order the manifest first names them:\n{all}"
    );
    assert!(
        all.contains("wer: total over 2 scored items (0 failed)"),
        "{all}"
    );
    assert_no_secrets(&all, &["hello"]);
}

/// Acceptance 1: a provider without times (400 on `verbose_json`) still scores: the pass
/// re-sends time-less, prints no server text and says `mode=timeless`. The first request
/// must be the timed one (the example asks the final pass's way, not the plain way).
#[tokio::test]
async fn wer_example_scores_through_the_timeless_resend_and_prints_no_server_text() {
    struct NoTimes;
    impl wiremock::Respond for NoTimes {
        fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
            if request.body.windows(12).any(|w| w == b"verbose_json") {
                ResponseTemplate::new(400).set_body_string(ERROR_BODY)
            } else {
                ResponseTemplate::new(200).set_body_json(json!({ "text": SCORED_ANSWER }))
            }
        }
    }
    let server = server_with_responder(NoTimes).await;
    let dir = Dir::new("timeless");
    std::fs::write(dir.0.join("item1.wav"), fake_wav()).expect("write wav");
    std::fs::write(dir.0.join("item1.txt"), SCORED_REFERENCE).expect("write reference");
    let (out, all) = run_example(&["pairs", path_arg(&dir), "ru"], &server.uri());
    assert!(out.status.success(), "example failed:\n{all}");
    assert_has_line(&all, &format!("item1 {SCORED_COUNTS}"));
    assert!(all.contains("mode=timeless"), "no pass mode:\n{all}");
    let seen = server.received_requests().await.expect("recording is on");
    assert!(
        seen.iter()
            .any(|r| r.body.windows(12).any(|w| w == b"verbose_json")),
        "no timed request: the example did not go through the final pass"
    );
    assert_no_secrets(&all, &[ERROR_BODY, "hello", "world", "extra"]);
}

async fn server_with_responder(responder: impl wiremock::Respond + 'static) -> MockServer {
    let server = mock_http::start().await;
    Mock::given(any())
        .respond_with(responder)
        .mount(&server)
        .await;
    server
}

/// Failure branch (T-163 invariant; FR-022): the final pass needs the key, so a missing or
/// blank `VOICEN_WER_KEY` is a configuration error (exit 2) before any request, in both
/// modes, and nothing secret is printed. Bites: a keyless run that sends requests.
#[tokio::test]
async fn wer_example_refuses_a_missing_or_blank_key_before_any_request() {
    let server =
        server_with(ResponseTemplate::new(200).set_body_json(timed_body(TRANSCRIPT))).await;
    let dir = Dir::new("no-key");
    std::fs::write(dir.0.join("item1.wav"), fake_wav()).expect("write wav");
    std::fs::write(dir.0.join("item1.txt"), REFERENCE).expect("write reference");
    for key in [None, Some(""), Some("   ")] {
        let (out, all) = run_custom(&["pairs", path_arg(&dir), "ru"], &server.uri(), key, None);
        assert_eq!(out.status.code(), Some(2), "key {key:?}:\n{all}");
        assert!(all.contains("voicen_wer_key"), "no refusal line:\n{all}");
        assert_no_secrets(&all, &[]);
    }
    let seen = server.received_requests().await.expect("recording is on");
    assert!(seen.is_empty(), "a refused run sends no request");
}

/// Failure branch: an item whose pass does not end `Ready` is a failed item with a code,
/// never an empty hypothesis (no score line, not in the total).
#[tokio::test]
async fn wer_example_a_pass_that_fails_is_a_failed_item_not_an_empty_hypothesis() {
    let server = server_with(ResponseTemplate::new(401).set_body_string(ERROR_BODY)).await;
    let dir = Dir::new("failed-pass");
    std::fs::write(dir.0.join("item1.wav"), fake_wav()).expect("write wav");
    std::fs::write(dir.0.join("item1.txt"), REFERENCE).expect("write reference");
    let (out, all) = run_example(&["pairs", path_arg(&dir), "ru"], &server.uri());
    assert_eq!(out.status.code(), Some(1), "a failed item exits 1:\n{all}");
    assert!(
        all.lines().any(|l| l == "item1 failed key"),
        "a 401 is a failed item with the code `key`:\n{all}"
    );
    assert!(
        !all.lines().any(|l| l.starts_with("item1 s=")),
        "a failed pass was scored:\n{all}"
    );
    assert!(
        all.contains("wer: total over 0 scored items (1 failed)"),
        "{all}"
    );
    assert_no_secrets(&all, &[ERROR_BODY]);
}

/// The temp call folder is removed when the item ends, on success and on failure: the
/// child's temp directory is empty afterwards.
#[tokio::test]
async fn wer_example_leaves_no_call_folder_behind_on_success_or_failure() {
    let ok = server_with(ResponseTemplate::new(200).set_body_json(timed_body("hello"))).await;
    let bad = server_with(ResponseTemplate::new(500).set_body_string(ERROR_BODY)).await;
    for (server, name, code) in [(ok, "tmp-ok", 0), (bad, "tmp-bad", 1)] {
        let data = Dir::new(&format!("{name}-data"));
        let tmp = Dir::new(&format!("{name}-tmp"));
        std::fs::write(data.0.join("item1.wav"), fake_wav()).expect("write wav");
        std::fs::write(data.0.join("item1.txt"), "hello").expect("write reference");
        let (out, all) = run_custom(
            &["pairs", path_arg(&data), "ru"],
            &server.uri(),
            Some(KEY),
            Some(&tmp.0),
        );
        assert_eq!(out.status.code(), Some(code), "{name}:\n{all}");
        // The item really went through the pass (else an empty folder proves nothing).
        let seen = server.received_requests().await.expect("recording is on");
        assert!(!seen.is_empty(), "{name}: no request");
        assert!(all.contains("item1 "), "{name}: no item line:\n{all}");
        let left: Vec<_> = std::fs::read_dir(&tmp.0).expect("tmp").collect();
        assert!(left.is_empty(), "{name}: left in the temp dir: {left:?}");
    }
}
