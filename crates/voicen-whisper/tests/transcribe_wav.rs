//! T-099 (spec 002 T023, T026; decisions #12, #130, #142): `voicen_whisper::WhisperLoader`
//! on the real `tiny` model. Runs in the Windows CI job only (whisper.cpp builds there,
//! decisions #12), in `cargo test --workspace --exclude voicen`.
//!
//! Contract these tests pin (from the T-099 analysis):
//! - `WhisperLoader::new()` builds the loader; it implements `SpeechModelLoader`.
//! - The model: `VOICEN_WHISPER_TEST_MODELS_DIR` names a `ModelStore` directory that the CI
//!   fetch step filled through `local_models::download::Downloader` before this step (no
//!   network here: the loader reads a path, the test reads the store). A missing variable or
//!   model is red, never a skip. The cached file must carry the catalog's pinned SHA-256
//!   (invariant 5).
//! - Clip: `crates/voicen-core/assets/connection-test-speech.s16le`, about 1 s of 16 kHz
//!   s16le English speech ("speech bubble", CC0, licenses/manual.json).
//! - A load or transcription failure is `Err(FailureReason::LocalEngineFailed)`, never a
//!   panic (invariant 2); an `Abort` that is requested stops `whisper_full` (invariant 2).
//! - Nothing of whisper.cpp / ggml reaches stdout or stderr (invariant 1): checked in a child
//!   process, so its first load is the process's first load.
//! - Direct normal dependencies are exactly voicen-core and whisper-rs with default features
//!   (invariant 1).
//!
//! FR-20 / NFR-04: no assertion message carries a transcript; only lengths and booleans.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use voicen_core::clock::{MonotonicClock, SystemMonotonic};
use voicen_core::engine::speech_model::{Abort, SpeechModel, SpeechModelLoader};
use voicen_core::failure::FailureReason;
use voicen_core::local_models::catalog::{ModelId, MODELS};
use voicen_core::local_models::store::ModelStore;
use voicen_whisper::WhisperLoader;

/// The ModelStore directory the CI fetch step filled (contract above).
const MODELS_DIR_VAR: &str = "VOICEN_WHISPER_TEST_MODELS_DIR";
/// Set by `whisper_and_ggml_print_nothing` for its child run of `quiet_child_run`.
const QUIET_CHILD_VAR: &str = "VOICEN_WHISPER_QUIET_CHILD";

const CLIP_S16LE: &[u8] = include_bytes!("../../voicen-core/assets/connection-test-speech.s16le");
const SAMPLE_RATE: usize = 16_000;

/// The clip as 16 kHz mono f32 in [-1, 1].
fn clip() -> Vec<f32> {
    assert_eq!(CLIP_S16LE.len() % 2, 0, "clip is not whole s16le samples");
    CLIP_S16LE
        .chunks_exact(2)
        .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0)
        .collect()
}

/// `seconds` of audio: the clip, then silence, repeated every 2 s.
fn long_audio(seconds: usize) -> Vec<f32> {
    let clip = clip();
    let period = 2 * SAMPLE_RATE;
    assert!(clip.len() < period, "clip longer than its 2 s period");
    let mut out = vec![0.0f32; seconds * SAMPLE_RATE];
    for start in (0..out.len()).step_by(period) {
        let end = (start + clip.len()).min(out.len());
        out[start..end].copy_from_slice(&clip[..end - start]);
    }
    out
}

/// The cached tiny model from the store, checked against the pinned catalog hash.
fn tiny_path() -> PathBuf {
    let dir = std::env::var_os(MODELS_DIR_VAR).unwrap_or_else(|| {
        panic!("{MODELS_DIR_VAR} is not set: the CI fetch step must fill a ModelStore before this test")
    });
    let store = ModelStore::new(PathBuf::from(dir), MODELS);
    store
        .path_if_downloaded(ModelId::Tiny)
        .unwrap_or_else(|| panic!("the store in {MODELS_DIR_VAR} has no downloaded tiny model"))
}

fn tiny_sha256() -> &'static str {
    MODELS
        .iter()
        .find(|m| m.id == ModelId::Tiny)
        .map(|m| m.sha256)
        .expect("catalog has tiny")
}

fn sha256_hex(path: &Path) -> String {
    let mut f = fs::File::open(path).expect("open model");
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).expect("read model");
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// An abort hook that never fires within a test run.
fn no_abort() -> Abort {
    Abort::new(
        Arc::new(SystemMonotonic),
        SystemMonotonic.now(),
        Duration::from_secs(3600),
    )
}

fn load_tiny() -> Box<dyn SpeechModel> {
    match WhisperLoader::new().load(&tiny_path()) {
        Ok(m) => m,
        Err(e) => panic!("loading the cached tiny model failed: {e:?}"),
    }
}

/// Whether the transcript names the clip; never prints it.
fn names_the_clip(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("speech") || t.contains("bubble")
}

fn assert_transcribes_clip(language: Option<&str>) {
    let mut model = load_tiny();
    let text = match model.transcribe(&clip(), language, &no_abort()) {
        Ok(t) => t,
        Err(e) => panic!("transcribe (language {language:?}) failed: {e:?}"),
    };
    assert!(
        !text.trim().is_empty(),
        "transcript (language {language:?}) is empty"
    );
    assert!(
        names_the_clip(&text),
        "transcript (language {language:?}, {} chars) contains neither 'speech' nor 'bubble'",
        text.chars().count()
    );
}

/// A temp directory removed on drop (best effort).
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let p =
            std::env::temp_dir().join(format!("voicen-whisper-t099-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).expect("create temp dir");
        TempDir(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The first half of the tiny model: a valid `lmgg` header, tensors cut off.
fn write_truncated_tiny(dir: &Path) -> PathBuf {
    let full = fs::read(tiny_path()).expect("read tiny");
    assert!(
        full.starts_with(b"lmgg"),
        "cached tiny does not start with the ggml magic"
    );
    let cut = dir.join("ggml-tiny.bin");
    fs::write(&cut, &full[..full.len() / 2]).expect("write truncated tiny");
    cut
}

fn assert_load_fails(path: &Path, what: &str) {
    match WhisperLoader::new().load(path) {
        Ok(_) => panic!("{what}: load returned a model"),
        Err(e) => assert_eq!(
            e,
            FailureReason::LocalEngineFailed,
            "{what}: wrong failure reason"
        ),
    }
}

// ---- invariant 5: the model under test ------------------------------------------------

#[test]
fn cached_tiny_model_is_the_pinned_catalog_file() {
    let path = tiny_path();
    assert_eq!(
        sha256_hex(&path),
        tiny_sha256(),
        "the cached tiny model is not the catalog's pinned file"
    );
}

// ---- Acceptance 1: the bundled clip is transcribed without network --------------------

#[test]
fn clip_is_transcribed_with_fixed_language_en() {
    assert_transcribes_clip(Some("en"));
}

#[test]
fn clip_is_transcribed_with_auto_detect() {
    assert_transcribes_clip(None);
}

/// A loader that returns a fixed text would pass the two tests above.
#[test]
fn silence_is_not_transcribed_as_the_clip() {
    let mut model = load_tiny();
    let silence = vec![0.0f32; SAMPLE_RATE];
    let text = match model.transcribe(&silence, Some("en"), &no_abort()) {
        Ok(t) => t,
        Err(e) => panic!("transcribing 1 s of silence failed: {e:?}"),
    };
    assert!(
        !names_the_clip(&text),
        "1 s of silence was transcribed as the clip ({} chars)",
        text.chars().count()
    );
}

/// One loaded model serves more than one transcription (the residency keeps it, #133).
#[test]
fn one_model_transcribes_twice() {
    let mut model = load_tiny();
    for round in 0..2 {
        let text = match model.transcribe(&clip(), Some("en"), &no_abort()) {
            Ok(t) => t,
            Err(e) => panic!("transcription {round} failed: {e:?}"),
        };
        assert!(
            names_the_clip(&text),
            "transcription {round} does not name the clip"
        );
    }
}

// ---- Acceptance 3 / invariant 2: failure branches are errors, not panics -------------

#[test]
fn missing_model_path_is_a_load_error() {
    let dir = TempDir::new("missing");
    assert_load_fails(&dir.0.join("ggml-tiny.bin"), "missing path");
}

#[test]
fn truncated_model_is_a_load_error() {
    let dir = TempDir::new("truncated");
    let cut = write_truncated_tiny(&dir.0);
    assert_load_fails(&cut, "truncated tiny (valid magic, tensors cut off)");
}

/// The monotonic clock, counting the reads: `Abort::requested` reads it until it latches.
struct CountingClock(AtomicUsize);

impl MonotonicClock for CountingClock {
    fn now(&self) -> Instant {
        self.0.fetch_add(1, Ordering::SeqCst);
        SystemMonotonic.now()
    }
}

#[test]
fn pre_requested_abort_returns_promptly() {
    let mut model = load_tiny();
    let audio = long_audio(60);

    let t0 = SystemMonotonic.now();
    let full = model.transcribe(&audio, Some("en"), &no_abort());
    let baseline = SystemMonotonic.now().saturating_duration_since(t0);
    assert!(
        full.is_ok(),
        "the unaborted 60 s baseline failed: {:?}",
        full.err()
    );

    // Limit zero: the first `requested()` the model makes is true and latches. The start is
    // read from the system clock directly, so every read of `clock` is the model's.
    let clock = Arc::new(CountingClock(AtomicUsize::new(0)));
    let abort = Abort::new(clock.clone(), SystemMonotonic.now(), Duration::ZERO);
    let t1 = SystemMonotonic.now();
    let _ignored = model.transcribe(&audio, Some("en"), &abort); // result ignored (#133)
    let aborted = SystemMonotonic.now().saturating_duration_since(t1);

    assert!(
        clock.0.load(Ordering::SeqCst) >= 1,
        "the model never asked the Abort whether to stop"
    );
    assert!(
        aborted * 4 <= baseline,
        "an aborted transcription took {aborted:?}, the unaborted one {baseline:?}: whisper_full was not stopped"
    );
}

// ---- invariant 1: no whisper / ggml text on stdout or stderr --------------------------

/// Work of the child process of `whisper_and_ggml_print_nothing`; a no-op unless the
/// parent set `VOICEN_WHISPER_QUIET_CHILD`.
#[test]
#[ignore = "child of whisper_and_ggml_print_nothing"]
fn quiet_child_run() {
    if std::env::var_os(QUIET_CHILD_VAR).is_none() {
        return;
    }
    let mut model = load_tiny();
    let _ = model.transcribe(&clip(), Some("en"), &no_abort());
    let _ = model.transcribe(&clip(), None, &no_abort());
    let dir = TempDir::new("quiet");
    let _ = WhisperLoader::new().load(&dir.0.join("absent.bin"));
    let cut = write_truncated_tiny(&dir.0);
    let _ = WhisperLoader::new().load(&cut);
}

#[test]
fn whisper_and_ggml_print_nothing() {
    let exe = std::env::current_exe().expect("current test exe");
    let out = Command::new(exe)
        .args([
            "--exact",
            "quiet_child_run",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(QUIET_CHILD_VAR, "1")
        .output()
        .expect("start the child test run");
    let stdout = String::from_utf8_lossy(&out.stdout).to_lowercase();
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(
        out.status.success(),
        "the child run failed: {:?}",
        out.status
    );
    assert!(
        stdout.contains("1 passed"),
        "the child run did not run quiet_child_run"
    );
    for (name, text) in [("stdout", &stdout), ("stderr", &stderr)] {
        for token in ["whisper", "ggml", "speech", "bubble"] {
            let n = text.matches(token).count();
            assert_eq!(n, 0, "the child's {name} holds '{token}' {n} time(s)");
        }
    }
}

// ---- invariant 1: dependencies --------------------------------------------------------

#[test]
fn direct_dependencies_are_voicen_core_and_whisper_rs() {
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let out = Command::new(cargo)
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(&manifest)
        .output()
        .expect("run cargo metadata");
    assert!(
        out.status.success(),
        "cargo metadata failed: {:?}",
        out.status
    );
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).expect("metadata JSON");
    let pkg = meta["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .find(|p| p["name"] == "voicen-whisper")
        .expect("package voicen-whisper");
    let deps = pkg["dependencies"].as_array().expect("dependencies");

    let mut normal: Vec<&str> = deps
        .iter()
        .filter(|d| d["kind"].is_null())
        .map(|d| d["name"].as_str().expect("dependency name"))
        .collect();
    normal.sort_unstable();
    assert_eq!(
        normal,
        ["voicen-core", "whisper-rs"],
        "direct normal dependencies"
    );

    let build: Vec<&str> = deps
        .iter()
        .filter(|d| d["kind"] == "build")
        .map(|d| d["name"].as_str().unwrap_or("?"))
        .collect();
    assert!(build.is_empty(), "build dependencies: {build:?}");

    for d in deps.iter().filter(|d| d["kind"].is_null()) {
        let name = d["name"].as_str().unwrap_or("?");
        assert_eq!(d["optional"], false, "{name} is optional");
        assert!(d["rename"].is_null(), "{name} is renamed");
        let features = d["features"].as_array().expect("features");
        if name == "whisper-rs" {
            assert_eq!(
                d["uses_default_features"], true,
                "whisper-rs default features off"
            );
            assert!(
                features.is_empty(),
                "whisper-rs features {features:?} (no log/tracing backend, no GPU)"
            );
        } else {
            assert!(
                !features.iter().any(|f| f == "test-fakes"),
                "voicen-core normal dependency enables test-fakes"
            );
        }
    }
}
