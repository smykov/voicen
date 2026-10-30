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
//! T-109 additions:
//! - I1: one file-level lock ([`serial`]) so at most one whisper.cpp computation runs at a
//!   time in this process (ggml's barrier is a pure spin; parallel models oversubscribe the
//!   runner). Held first by every test that loads a model, and by the parent of a child
//!   process for the child's whole run.
//! - I2: the engine's thread count comes from one policy function; the properties below hold
//!   for any measured best count.
//! - I3: the abort guarantee is asserted in units of whisper.cpp work (segments), never in
//!   time (F-013, F-021). Durations and thread timings are printed, never compared.
//!
//! New test-only surface the engine must provide (all `#[doc(hidden)]`, decision #200 D2):
//! `WhisperLoader::with_threads(NonZero<usize>) -> WhisperLoader`,
//! `WhisperLoader::load_whisper(&self, &Path) -> Result<WhisperModel, FailureReason>`,
//! `WhisperModel::threads(&self) -> usize`, `WhisperModel::last_segment_count(&self) -> usize`,
//! `voicen_whisper::thread_count_for(parallelism: usize) -> usize`.
//!
//! FR-20 / NFR-04: no assertion message carries a transcript; only lengths and booleans.

use std::fs;
use std::io::Read;
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use voicen_core::clock::{MonotonicClock, SystemMonotonic};
use voicen_core::engine::speech_model::{Abort, SpeechModel, SpeechModelLoader};
use voicen_core::failure::FailureReason;
use voicen_core::local_models::catalog::{ModelId, MODELS};
use voicen_core::local_models::store::ModelStore;
use voicen_whisper::{thread_count_for, WhisperLoader, WhisperModel};

/// The ModelStore directory the CI fetch step filled (contract above).
const MODELS_DIR_VAR: &str = "VOICEN_WHISPER_TEST_MODELS_DIR";
/// Set by `whisper_and_ggml_print_nothing` for its child run of `quiet_child_run`.
const QUIET_CHILD_VAR: &str = "VOICEN_WHISPER_QUIET_CHILD";
/// Set by `whisper_timing_is_printed` for its child run of `whisper_timing_child_run`.
const TIMING_CHILD_VAR: &str = "VOICEN_WHISPER_TIMING_CHILD";

/// I1 (T-109): the one lock of this file. A test that loads a model takes it first and keeps
/// it to its end; the parent of a child process keeps it across the child's whole run.
static MODEL_LOCK: Mutex<()> = Mutex::new(());

/// Takes [`MODEL_LOCK`]. A test that panicked while holding it poisons the mutex; the guard
/// is recovered, so one red test does not turn every later one red.
fn serial() -> MutexGuard<'static, ()> {
    MODEL_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

const CLIP_S16LE: &[u8] = include_bytes!("../../voicen-core/assets/connection-test-speech.s16le");
const SAMPLE_RATE: usize = 16_000;

/// The clip as 16 kHz mono f32 in [-1, 1].
fn clip() -> Vec<f32> {
    assert_eq!(CLIP_S16LE.len() % 2, 0, "clip is not whole s16le samples");
    CLIP_S16LE
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&b| f32::from(i16::from_le_bytes(b)) / 32768.0)
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
    let _serial = serial();
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
    let _serial = serial();
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
    let _serial = serial();
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

/// A concrete model with an explicit ggml thread count (test-only loader).
fn load_tiny_with_threads(threads: usize) -> WhisperModel {
    let n = NonZero::new(threads).expect("thread count is not zero");
    match WhisperLoader::with_threads(n).load_whisper(&tiny_path()) {
        Ok(m) => m,
        Err(e) => panic!("loading the cached tiny model ({threads} threads) failed: {e:?}"),
    }
}

/// I3 (T-109, Acceptance 5, replaces the wall-clock ratio of T-099): an `Abort` that is
/// already requested leaves whisper.cpp's segment count of that run at 0, while the same
/// audio unaborted on the same model leaves more than 0. `whisper_full` clears its result
/// at the start of a run and asks the encoder-begin callback before the first window, so
/// with both stop callbacks removed the aborted run computes on and leaves segments > 0
/// (not the baseline's count: run 40175631024 left 12 against 24).
///
/// Not covered by design: the mel spectrogram of the whole input is computed before any
/// callback runs, so it cannot be aborted and is not what this asserts. Durations are
/// printed, never compared (F-013, F-021).
#[test]
fn pre_requested_abort_returns_promptly() {
    let _serial = serial();
    let mut model = load_tiny_with_threads(1);
    let audio = long_audio(60);

    let t0 = Instant::now();
    let full = model.transcribe(&audio, Some("en"), &no_abort());
    let baseline = t0.elapsed();
    assert!(
        full.is_ok(),
        "the unaborted 60 s baseline failed: {:?}",
        full.err()
    );
    let baseline_segments = model.last_segment_count();
    assert!(
        baseline_segments > 0,
        "the unaborted 60 s run left no segments: the abort assertion below would prove nothing"
    );

    // Limit zero: the first `requested()` is true and latches.
    let abort = Abort::new(
        Arc::new(SystemMonotonic),
        SystemMonotonic.now(),
        Duration::ZERO,
    );
    let t1 = Instant::now();
    let aborted = model.transcribe(&audio, Some("en"), &abort);
    let aborted_for = t1.elapsed();
    assert_eq!(
        aborted.err(),
        Some(FailureReason::LocalEngineFailed),
        "an aborted transcription must be a LocalEngineFailed (decision #133)"
    );
    assert_eq!(
        model.last_segment_count(),
        0,
        "the aborted run left segments (baseline {baseline_segments}): whisper_full was not stopped"
    );
    eprintln!("abort-test: baseline {baseline:?}, aborted {aborted_for:?} (printed, not compared)");
}

// ---- I2: the engine's thread count has one policy -------------------------------------

/// Properties of `max(1, min(B, P - 1))` that hold for every B >= 1, so they do not wait
/// for the measured best count: at least 1, below P when P > 1, monotone in P.
#[test]
fn thread_policy_is_at_least_one_and_below_parallelism() {
    let mut previous = 0;
    for p in 1..=256usize {
        let n = thread_count_for(p);
        assert!(n >= 1, "policy for parallelism {p} is {n}");
        if p > 1 {
            assert!(n < p, "policy for parallelism {p} is {n}: not below P");
        } else {
            assert_eq!(n, 1, "policy for parallelism 1 is {n}");
        }
        assert!(
            n >= previous,
            "policy fell from {previous} to {n} at P = {p}"
        );
        previous = n;
    }
    // Unknown parallelism reaches the policy as 1.
    assert_eq!(thread_count_for(1), 1);
    assert_eq!(thread_count_for(2), 1);
}

/// The loader without an override uses the policy on this machine's parallelism; the
/// override is used as asked.
#[test]
fn loader_uses_the_policy_and_with_threads_overrides_it() {
    let _serial = serial();
    let p = std::thread::available_parallelism().map_or(1, |n| n.get());
    let default = WhisperLoader::new()
        .load_whisper(&tiny_path())
        .expect("load tiny");
    assert_eq!(
        default.threads(),
        thread_count_for(p),
        "default thread count"
    );
    drop(default);
    let two = load_tiny_with_threads(2);
    assert_eq!(two.threads(), 2, "with_threads(2)");
}

// ---- Acceptance 1 / 2: the figure for the Windows log (print only, F-013) --------------

/// Work of the child of `whisper_timing_is_printed`; a no-op unless the parent set
/// `VOICEN_WHISPER_TIMING_CHILD`. Per thread count n in 1..=4 it loads tiny, times 3 runs of
/// the 1 s clip and of 30 s of audio, and prints one `whisper-timing:` line to stderr.
/// Print only: no figure is compared to anything.
#[test]
#[ignore = "child of whisper_timing_is_printed"]
fn whisper_timing_child_run() {
    if std::env::var_os(TIMING_CHILD_VAR).is_none() {
        return;
    }
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    let clip = clip();
    let long = long_audio(30);
    let best = |model: &mut WhisperModel, audio: &[f32]| -> f64 {
        (0..3)
            .map(|_| {
                let t = Instant::now();
                let r = model.transcribe(audio, Some("en"), &no_abort());
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                assert!(r.is_ok(), "timing run failed: {:?}", r.err());
                ms
            })
            .fold(f64::INFINITY, f64::min)
    };
    for n in 1..=4usize {
        let mut model = load_tiny_with_threads(n);
        let short_ms = best(&mut model, &clip);
        let long_ms = best(&mut model, &long);
        eprintln!(
            "whisper-timing: threads {n}, 1 s clip best {short_ms:.1} ms, 30 s best {long_ms:.1} ms, \
             available parallelism {cpus}"
        );
    }
    eprintln!(
        "whisper-timing: policy picks {} threads, available parallelism {cpus}",
        thread_count_for(cpus)
    );
}

/// Runs the timing child with nothing else computing in this process (the lock is held for
/// its whole run) and asserts only that it passed. The child's stderr is the parent's, so
/// the Windows CI log carries the `whisper-timing:` lines past libtest's capture.
#[test]
fn whisper_timing_is_printed() {
    let _serial = serial();
    let exe = std::env::current_exe().expect("current test exe");
    let status = Command::new(exe)
        .args([
            "--exact",
            "whisper_timing_child_run",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(TIMING_CHILD_VAR, "1")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .expect("start the timing child run");
    assert!(status.success(), "the timing child run failed: {status:?}");
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
    // Held across the child's whole run: its models must not compute beside another test's.
    let _serial = serial();
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
