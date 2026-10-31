//! T-043 (decisions #59, #60, #136; T-043 analysis invariants 1, 3, 4): the Silero model pin,
//! its verification and its fetch, on the Linux gate. The detector itself (whisper.cpp) is the
//! Windows job's (`crates/voicen-whisper/tests/silero_clips.rs`).
//!
//! Contract these tests pin (from the T-043 analysis, seam "Core"):
//! - `voicen_core::vad::silero_model::SILERO_MODEL: ModelPin`, the one place the pin lives:
//!   `ModelPin { file_name, url, size_bytes, sha256 }` (`&'static str` / `u64`, public fields,
//!   so a test builds its own pin).
//! - `verify(path) -> Result<(), VadError>` checks a file against `SILERO_MODEL`;
//!   `verify_against(path, &pin)` against any pin. Anything but a regular file of exactly the
//!   pinned size and SHA-256 is `Err(VadError::Unavailable)` (invariant 1), never a panic.
//! - `fetch(dir, &Timeouts) -> Result<PathBuf, DownloadFailure>` downloads `SILERO_MODEL` into
//!   `dir`; `fetch_against(dir, &pin, &Timeouts)` the same for any pin. Both go through the one
//!   streaming size + SHA-256 transfer of `local_models::download` (`<file>.part`, then the
//!   rename to `<dir>/<file_name>`; invariant 4): `Ok` is the final path; every failure leaves
//!   neither the final file nor the `.part`.
//!
//! The pin's values are checked only for shape here (no second copy of them, invariant 3);
//! the Windows job's fetch and SHA-256 check is the end-to-end cross-check. The fetch tests use
//! the raw-TCP model server of `tests/common` (the Downloader's own fixture, fake 64 KiB model,
//! 127.0.0.1 only), from plain `#[test]` threads: the blocking client must not run inside a
//! tokio runtime (T-040), so wiremock is not used here.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::download::timeouts;
use common::{dir_entries, leak, model_bytes, Serve, Server, SHA256, SIZE};
use sha2::{Digest, Sha256};
use voicen_core::local_models::download::DownloadFailure;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_core::vad::silero_model::{
    bundled_path, fetch, fetch_against, verify, verify_against, ModelPin, SILERO_MODEL,
};
use voicen_core::vad::VadError;

/// File name of the test pins (served by the mock server under any path).
const TEST_FILE: &str = "ggml-silero-test.bin";
const TEST_PART: &str = "ggml-silero-test.bin.part";

fn is_lower_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A pin for `bytes` whose SHA-256 the test computes (verify_against, Ok case).
fn pin_for(bytes: &[u8]) -> ModelPin {
    ModelPin {
        file_name: TEST_FILE,
        url: "http://127.0.0.1:9/unused",
        size_bytes: bytes.len() as u64,
        sha256: leak(sha256_hex(bytes)),
    }
}

/// The pin of the mock server's fake model (its size and hard-coded SHA-256), served at `url`.
fn served_pin(url: &str) -> ModelPin {
    ModelPin {
        file_name: TEST_FILE,
        url: leak(url.to_string()),
        size_bytes: SIZE,
        sha256: SHA256,
    }
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, bytes).expect("write test file");
    p
}

/// `n` zero bytes: never the Silero model (its SHA-256 is not that of zeros).
fn zeros(n: u64) -> Vec<u8> {
    vec![0u8; usize::try_from(n).expect("pinned size fits usize")]
}

fn assert_unavailable(got: Result<(), VadError>, what: &str) {
    assert_eq!(got, Err(VadError::Unavailable), "{what}");
}

// ---- invariant 3: the pin's shape (values are not copied here) ------------------------

#[test]
fn pin_url_is_the_whisper_vad_repo_at_a_fixed_commit_and_names_the_file() {
    let prefix = "https://huggingface.co/ggml-org/whisper-vad/resolve/";
    let rest = SILERO_MODEL
        .url
        .strip_prefix(prefix)
        .unwrap_or_else(|| panic!("the pin URL does not start with {prefix}"));
    let (commit, file) = rest
        .split_once('/')
        .expect("the pin URL has no /<file> after the commit");
    assert!(
        is_lower_hex(commit, 40),
        "the pin URL's revision is not a 40-hex commit (a branch name would move)"
    );
    assert_eq!(
        file, SILERO_MODEL.file_name,
        "the pin URL's last segment is not the pinned file name (or has a query)"
    );
}

#[test]
fn pin_file_name_is_a_ggml_silero_model() {
    let name = SILERO_MODEL.file_name;
    assert!(
        name.starts_with("ggml-silero-v") && name.ends_with(".bin"),
        "file name {name:?} is not ggml-silero-v<version>.bin"
    );
    assert!(
        !name.contains(['/', '\\']),
        "file name {name:?} holds a path separator"
    );
}

#[test]
fn pin_sha256_is_64_lowercase_hex_and_size_is_set() {
    assert!(
        is_lower_hex(SILERO_MODEL.sha256, 64),
        "the pinned SHA-256 is not 64 lowercase hex digits"
    );
    assert!(SILERO_MODEL.size_bytes > 0, "the pinned size is 0");
}

/// licenses/manual.json's Silero entry derives from the pin (invariant 3; T-043 review 1
/// finding 4): its `url` is `SILERO_MODEL.url` (the notice's Source line), its `version` is
/// the one in `SILERO_MODEL.file_name`, and its `note` holds no copy of the pin's commit,
/// SHA-256 or size that a pin bump could leave stale.
#[test]
fn manual_json_silero_entry_derives_from_the_pin() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../licenses/manual.json");
    let text = fs::read_to_string(&path).expect("read licenses/manual.json");
    let entries: serde_json::Value = serde_json::from_str(&text).expect("manual.json is JSON");
    let entries = entries.as_array().expect("manual.json is an array");
    let silero: Vec<_> = entries
        .iter()
        .filter(|e| {
            e["name"]
                .as_str()
                .is_some_and(|n| n.starts_with("Silero VAD model"))
        })
        .collect();
    assert_eq!(
        silero.len(),
        1,
        "manual.json has no single Silero VAD entry"
    );
    let entry = silero[0];
    assert_eq!(
        entry["url"].as_str(),
        Some(SILERO_MODEL.url),
        "manual.json's Silero url is not SILERO_MODEL.url"
    );
    let version = entry["version"]
        .as_str()
        .expect("Silero entry has a version");
    assert_eq!(
        SILERO_MODEL.file_name,
        format!("ggml-silero-{version}.bin"),
        "manual.json's Silero version is not the pinned file's"
    );
    let note = entry["note"].as_str().expect("Silero entry has a note");
    let longest_hex_run = note
        .split(|c: char| !c.is_ascii_hexdigit())
        .map(str::len)
        .max()
        .unwrap_or(0);
    assert!(
        longest_hex_run < 40,
        "manual.json's Silero note copies a commit or SHA-256 ({longest_hex_run} hex digits)"
    );
    assert!(
        !note.contains(&SILERO_MODEL.size_bytes.to_string()),
        "manual.json's Silero note copies the pinned size"
    );
}

/// The production fetch's signature (used by `examples/fetch_model.rs ... silero`); its
/// network run is the Windows job's fetch step, not this test.
#[test]
fn production_fetch_takes_a_directory_and_the_timeouts() {
    let f: fn(&Path, &Timeouts) -> Result<PathBuf, DownloadFailure> = fetch;
    let _ = f;
}

// ---- invariant 1: verify against the production pin -----------------------------------

#[test]
fn verify_missing_path_is_unavailable() {
    let tmp = TempDir::new();
    assert_unavailable(
        verify(&tmp.path().join(SILERO_MODEL.file_name)),
        "missing model file",
    );
}

#[test]
fn verify_directory_is_unavailable() {
    let tmp = TempDir::new();
    let dir = tmp.path().join(SILERO_MODEL.file_name);
    fs::create_dir(&dir).expect("create dir");
    assert_unavailable(verify(&dir), "a directory under the model's name");
}

#[test]
fn verify_one_byte_short_is_unavailable() {
    let tmp = TempDir::new();
    let p = write(
        tmp.path(),
        SILERO_MODEL.file_name,
        &zeros(SILERO_MODEL.size_bytes - 1),
    );
    assert_unavailable(verify(&p), "file of pinned size - 1");
}

#[test]
fn verify_one_byte_long_is_unavailable() {
    let tmp = TempDir::new();
    let p = write(
        tmp.path(),
        SILERO_MODEL.file_name,
        &zeros(SILERO_MODEL.size_bytes + 1),
    );
    assert_unavailable(verify(&p), "file of pinned size + 1");
}

#[test]
fn verify_pinned_size_with_other_bytes_is_unavailable() {
    let tmp = TempDir::new();
    let p = write(
        tmp.path(),
        SILERO_MODEL.file_name,
        &zeros(SILERO_MODEL.size_bytes),
    );
    assert_unavailable(verify(&p), "file of exactly the pinned size, other bytes");
}

#[test]
fn verify_empty_file_is_unavailable() {
    let tmp = TempDir::new();
    let p = write(tmp.path(), SILERO_MODEL.file_name, b"");
    assert_unavailable(verify(&p), "empty file");
}

// ---- invariant 1: verify_against a test pin -------------------------------------------

#[test]
fn verify_against_matching_size_and_hash_is_ok() {
    let tmp = TempDir::new();
    let bytes = model_bytes();
    let p = write(tmp.path(), TEST_FILE, &bytes);
    assert_eq!(
        verify_against(&p, &pin_for(&bytes)),
        Ok(()),
        "a file with the pin's size and SHA-256 is refused"
    );
}

#[test]
fn verify_against_one_flipped_byte_is_unavailable() {
    let tmp = TempDir::new();
    let bytes = model_bytes();
    let pin = pin_for(&bytes);
    let mut altered = bytes;
    let at = altered.len() / 3;
    altered[at] ^= 0x01;
    let p = write(tmp.path(), TEST_FILE, &altered);
    assert_unavailable(
        verify_against(&p, &pin),
        "same size, one byte flipped (only the hash can tell)",
    );
}

#[test]
fn verify_against_other_pinned_hash_is_unavailable() {
    let tmp = TempDir::new();
    let bytes = model_bytes();
    let p = write(tmp.path(), TEST_FILE, &bytes);
    let mut pin = pin_for(&bytes);
    // The pin's last hex digit changed: the file is the same, the pin is not its hash.
    let mut other = pin.sha256.to_string();
    let last = if other.ends_with('0') { "1" } else { "0" };
    other.replace_range(63.., last);
    pin.sha256 = leak(other);
    assert_unavailable(
        verify_against(&p, &pin),
        "pin hash differs in its last digit",
    );
}

#[test]
fn verify_against_right_hash_but_other_pinned_size_is_unavailable() {
    let tmp = TempDir::new();
    let bytes = model_bytes();
    let p = write(tmp.path(), TEST_FILE, &bytes);
    let mut pin = pin_for(&bytes);
    pin.size_bytes += 1;
    assert_unavailable(
        verify_against(&p, &pin),
        "the file's hash is pinned, its size is not",
    );
}

// ---- invariant 4: fetch through the one transfer ---------------------------------------

#[test]
fn fetch_exact_bytes_renames_into_the_final_file() {
    let server = Server::start(vec![Serve::Full]);
    let tmp = TempDir::new();
    let pin = served_pin(&server.url(TEST_FILE));
    let got = fetch_against(tmp.path(), &pin, &timeouts(Duration::from_secs(5)));
    assert_eq!(
        got,
        Ok(tmp.path().join(TEST_FILE)),
        "fetch of the exact pinned bytes"
    );
    assert_eq!(
        dir_entries(tmp.path()),
        vec![TEST_FILE.to_string()],
        "after a fetch only the final file is in the directory (no .part)"
    );
    let on_disk = fs::read(tmp.path().join(TEST_FILE)).expect("read fetched file");
    assert!(
        on_disk == model_bytes(),
        "the fetched file is not the served bytes ({} bytes)",
        on_disk.len()
    );
    assert_eq!(
        verify_against(&tmp.path().join(TEST_FILE), &pin),
        Ok(()),
        "the fetched file does not verify against its own pin"
    );
    assert_eq!(server.accepts(), 1, "fetch made more than one request");
}

#[test]
fn fetch_creates_a_missing_directory() {
    let server = Server::start(vec![Serve::Full]);
    let tmp = TempDir::new();
    let dir = tmp.path().join("voicen-silero");
    let pin = served_pin(&server.url(TEST_FILE));
    let got = fetch_against(&dir, &pin, &timeouts(Duration::from_secs(5)));
    assert_eq!(got, Ok(dir.join(TEST_FILE)), "fetch into a new directory");
}

#[test]
fn fetch_short_body_leaves_no_final_file() {
    let server = Server::start(vec![Serve::Short]);
    let tmp = TempDir::new();
    let pin = served_pin(&server.url(TEST_FILE));
    let got = fetch_against(tmp.path(), &pin, &timeouts(Duration::from_secs(5)));
    assert_eq!(
        got,
        Err(DownloadFailure::DownloadInterrupted),
        "a body shorter than the pinned size"
    );
    assert_eq!(
        dir_entries(tmp.path()),
        Vec::<String>::new(),
        "a short body left a file behind (final or .part)"
    );
}

#[test]
fn fetch_cut_off_body_leaves_no_final_file() {
    let server = Server::start(vec![Serve::CutOff]);
    let tmp = TempDir::new();
    let pin = served_pin(&server.url(TEST_FILE));
    let got = fetch_against(tmp.path(), &pin, &timeouts(Duration::from_secs(5)));
    assert_eq!(
        got,
        Err(DownloadFailure::DownloadInterrupted),
        "connection closed after half the body"
    );
    assert_eq!(dir_entries(tmp.path()), Vec::<String>::new(), "left a file");
}

#[test]
fn fetch_wrong_hash_is_checksum_mismatch_and_leaves_no_part() {
    let server = Server::start(vec![Serve::Altered]);
    let tmp = TempDir::new();
    let pin = served_pin(&server.url(TEST_FILE));
    let got = fetch_against(tmp.path(), &pin, &timeouts(Duration::from_secs(5)));
    assert_eq!(
        got,
        Err(DownloadFailure::ChecksumMismatch),
        "pinned size, one byte flipped"
    );
    let left = dir_entries(tmp.path());
    assert!(
        !left.iter().any(|n| n == TEST_PART),
        "the .part is still there after a checksum mismatch: {left:?}"
    );
    assert!(
        !left.iter().any(|n| n == TEST_FILE),
        "a mismatching download was renamed to the final name: {left:?}"
    );
}

#[test]
fn fetch_longer_body_is_checksum_mismatch() {
    let server = Server::start(vec![Serve::Long]);
    let tmp = TempDir::new();
    let pin = served_pin(&server.url(TEST_FILE));
    let got = fetch_against(tmp.path(), &pin, &timeouts(Duration::from_secs(5)));
    assert_eq!(
        got,
        Err(DownloadFailure::ChecksumMismatch),
        "a body longer than the pinned size"
    );
    assert_eq!(dir_entries(tmp.path()), Vec::<String>::new(), "left a file");
}

#[test]
fn fetch_http_error_status_is_reported_and_leaves_nothing() {
    let server = Server::start(vec![Serve::Status(404)]);
    let tmp = TempDir::new();
    let pin = served_pin(&server.url(TEST_FILE));
    let got = fetch_against(tmp.path(), &pin, &timeouts(Duration::from_secs(5)));
    assert_eq!(got, Err(DownloadFailure::HttpStatus { code: 404 }), "404");
    assert_eq!(dir_entries(tmp.path()), Vec::<String>::new(), "left a file");
}

// ---- T-102: where the installer puts the bundled model ---------------------------------

#[test]
fn bundled_path_is_vad_dir_under_the_resource_dir_with_the_pinned_file_name() {
    // T-102 invariant 1: resource_dir/vad/<SILERO_MODEL.file_name>, the file name from the
    // pin (never a second copy of it), nothing under models/ or the data dir.
    let want = Path::new("res").join("vad").join(SILERO_MODEL.file_name);
    assert_eq!(bundled_path(Path::new("res")), want);
}

#[test]
fn bundled_path_keeps_a_non_ascii_resource_dir_whole() {
    // A Cyrillic user name under %LOCALAPPDATA% (T-102 re-check finding): no lossy rewrite.
    let dir = PathBuf::from("Пользователь").join("Voicen");
    let want = dir.join("vad").join(SILERO_MODEL.file_name);
    assert_eq!(bundled_path(&dir), want);
}

/// A read that fails after the size check passed must not count as verified (T-102 invariant
/// 2). The core image runs as root, so a mode 000 file still opens; `/proc/self/mem` is a
/// regular file of length 0 whose first read at offset 0 is an I/O error, so a pin of 0 bytes
/// passes the size check and only `sha256_of` can refuse it.
/// Bite: the `sha256_of(path)` `?` in `verify_against` swallowed into `Ok`.
#[cfg(target_os = "linux")]
#[test]
fn verify_against_read_error_after_the_size_check_is_unavailable() {
    let path = Path::new("/proc/self/mem");
    let meta = std::fs::metadata(path).expect("premise: /proc/self/mem exists");
    assert!(meta.is_file() && meta.len() == 0, "premise: {meta:?}");
    let mut buf = [0u8; 1];
    let read = std::io::Read::read(&mut std::fs::File::open(path).expect("open"), &mut buf);
    assert!(read.is_err(), "premise: the first read fails, got {read:?}");
    assert_unavailable(verify_against(path, &pin_for(&[])), "a read error");
}
