//! T-043 (FR-12; decisions #12, #59, #60, #136; T-043 analysis invariants 1, 2, 5):
//! `voicen_whisper::SileroDetector` on the pinned Silero model and on real clips. Runs in the
//! Windows CI job only (whisper.cpp builds there, decisions #12), in
//! `cargo test --workspace --exclude voicen`.
//!
//! Contract these tests pin (from the T-043 analysis, seam "Detector"):
//! - `SileroDetector::load(path: &Path) -> Result<Box<dyn SpeechDetector>, VadError>` is the
//!   only constructor. It calls `voicen_core::vad::silero_model::verify(path)` before any
//!   whisper.cpp call: a missing, short, long, altered or hostile file is
//!   `Err(VadError::Unavailable)` and never reaches the native loader (invariant 1; whisper.cpp's
//!   VAD loader throws C++ exceptions across `extern "C"`, which would abort this process).
//! - `name()` is `"silero"`. `contains_speech` never panics; empty audio is `Ok(false)`
//!   (invariant 2). One detector serves concurrent callers (`&self`, `Send + Sync`).
//! - The model: `VOICEN_SILERO_TEST_MODEL` names the directory the CI fetch step filled with
//!   `cargo run -p voicen-core --example fetch_model -- <dir> silero` (through
//!   `vad::silero_model::fetch`); the file in it is `SILERO_MODEL.file_name`, so ci.yml copies
//!   no value of the pin (invariant 3). A missing variable or file is red, never a skip
//!   (invariant 5).
//! - Clips (`tests/clips/*.s16le`, 16 kHz mono s16le, every one CC0-1.0 from Wikimedia Commons,
//!   each in licenses/manual.json, decision #59; public-domain-only sources are not used, as the
//!   accepted licence list of about.toml has no public-domain id): speech `speech-id-phrase`
//!   (Indonesian phrase, 2.7 s), `speech-ru-lingualibre` (Lingua Libre, Russian word, 1.3 s) and
//!   core's `connection-test-speech` (Lingua Libre, English, 1 s); non-speech `cough-woman`
//!   (Freesound 151212, 1.6 s), `cough-smoker` (4 s), `keyboard-model-m` (3 s),
//!   `keyboard-soft-laptop` (3 s), `room-tone-ac-unit` (air-conditioner fan, 3 s); digital
//!   silence is generated. The rule's constants are fixed for the whole set (never per clip).
//! - Fallback (T-041 gate): `SpeechGate::new(SileroDetector::load(bad), EnergyDetector::new())`
//!   decides with `"energy"` and carries `fallback_warning` on its first decision only; with the
//!   good model it decides with `"silero"` and never warns.
//! - Nothing of whisper.cpp / ggml reaches stdout or stderr (checked in a child process, as
//!   `transcribe_wav.rs::whisper_and_ggml_print_nothing`).
//!
//! FR-20 / NFR-04: no assertion message carries audio; only clip names, lengths and booleans.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Barrier};

use voicen_core::audio::AudioBuffer;
use voicen_core::vad::silero_model::{verify, SILERO_MODEL};
use voicen_core::vad::{EnergyDetector, SpeechDetector, SpeechGate, VadError};
use voicen_whisper::SileroDetector;

/// The directory the CI fetch step filled (contract above).
const MODEL_VAR: &str = "VOICEN_SILERO_TEST_MODEL";
/// Set by `silero_prints_nothing` for its child run of `quiet_child_run`.
const QUIET_CHILD_VAR: &str = "VOICEN_SILERO_QUIET_CHILD";
/// Set by `hostile_header_is_unavailable_and_the_process_survives` for its child run.
const HOSTILE_CHILD_VAR: &str = "VOICEN_SILERO_HOSTILE_CHILD";

const SAMPLE_RATE: usize = 16_000;

const SPEECH: &[(&str, &[u8])] = &[
    (
        "speech-id-phrase",
        include_bytes!("clips/speech-id-phrase.s16le"),
    ),
    (
        "speech-ru-lingualibre",
        include_bytes!("clips/speech-ru-lingualibre.s16le"),
    ),
    (
        "connection-test-speech",
        include_bytes!("../../voicen-core/assets/connection-test-speech.s16le"),
    ),
];

const COUGH_WOMAN: &[u8] = include_bytes!("clips/cough-woman.s16le");
const COUGH_SMOKER: &[u8] = include_bytes!("clips/cough-smoker.s16le");
const KEYBOARD_MODEL_M: &[u8] = include_bytes!("clips/keyboard-model-m.s16le");
const KEYBOARD_SOFT_LAPTOP: &[u8] = include_bytes!("clips/keyboard-soft-laptop.s16le");
const ROOM_TONE: &[u8] = include_bytes!("clips/room-tone-ac-unit.s16le");

fn samples(name: &str, raw: &[u8]) -> Vec<i16> {
    assert_eq!(raw.len() % 2, 0, "clip {name} is not whole s16le samples");
    assert!(
        raw.len() >= SAMPLE_RATE,
        "clip {name} is shorter than 0.5 s"
    );
    raw.as_chunks::<2>()
        .0
        .iter()
        .map(|&b| i16::from_le_bytes(b))
        .collect()
}

fn buffer(name: &str, raw: &[u8]) -> AudioBuffer {
    AudioBuffer::from_16k_mono(samples(name, raw))
}

fn speech_clip(i: usize) -> (&'static str, AudioBuffer) {
    let (name, raw) = SPEECH[i];
    (name, buffer(name, raw))
}

/// The fetched model: `<VOICEN_SILERO_TEST_MODEL>/<SILERO_MODEL.file_name>`, which must exist.
fn model_path() -> PathBuf {
    let dir = std::env::var_os(MODEL_VAR).unwrap_or_else(|| {
        panic!("{MODEL_VAR} is not set: the CI fetch step must fetch the Silero model before this test")
    });
    let path = PathBuf::from(dir).join(SILERO_MODEL.file_name);
    assert!(
        path.is_file(),
        "{MODEL_VAR} has no {} (the fetch step did not run or failed)",
        SILERO_MODEL.file_name
    );
    path
}

fn load_good() -> Box<dyn SpeechDetector> {
    match SileroDetector::load(&model_path()) {
        Ok(d) => d,
        Err(e) => panic!("loading the fetched Silero model failed: {e:?}"),
    }
}

fn decide(d: &dyn SpeechDetector, name: &str, audio: &AudioBuffer) -> bool {
    match d.contains_speech(audio) {
        Ok(v) => v,
        Err(e) => panic!("{name}: contains_speech failed: {e:?}"),
    }
}

fn assert_speech(name: &str, audio: &AudioBuffer) {
    let d = load_good();
    assert!(
        decide(d.as_ref(), name, audio),
        "{name}: speech clip not detected as speech"
    );
}

fn assert_not_speech(name: &str, audio: &AudioBuffer) {
    let d = load_good();
    assert!(
        !decide(d.as_ref(), name, audio),
        "{name}: non-speech clip detected as speech"
    );
}

/// A temp directory removed on drop (best effort).
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let p =
            std::env::temp_dir().join(format!("voicen-silero-t043-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).expect("create temp dir");
        TempDir(p)
    }

    fn file(&self, bytes: &[u8]) -> PathBuf {
        let p = self.0.join(SILERO_MODEL.file_name);
        fs::write(&p, bytes).expect("write test model");
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn good_bytes() -> Vec<u8> {
    let bytes = fs::read(model_path()).expect("read the fetched model");
    assert!(
        bytes.starts_with(b"lmgg"),
        "the fetched Silero model does not start with the ggml magic"
    );
    bytes
}

/// The good model with its first header field after the magic, `str_len` (int32 LE, the
/// length of the model-type string, whisper.cpp `whisper_vad_init_with_params`), set to
/// `i32::MAX`: same size as the pin, other hash. whisper.cpp sizes a `std::vector` from it
/// (`str_len + 1` overflows), which throws through `extern "C"` and aborts the process.
fn hostile_header_bytes() -> Vec<u8> {
    let mut bytes = good_bytes();
    bytes[4..8].copy_from_slice(&i32::MAX.to_le_bytes());
    bytes
}

fn assert_unavailable(path: &Path, what: &str) {
    match SileroDetector::load(path) {
        Ok(_) => panic!("{what}: load returned a detector"),
        Err(e) => assert_eq!(e, VadError::Unavailable, "{what}: wrong error"),
    }
}

// ---- invariant 5: the model under test ------------------------------------------------

#[test]
fn fetched_model_is_the_pinned_file() {
    assert_eq!(
        verify(&model_path()),
        Ok(()),
        "the fetched model does not match SILERO_MODEL's size and SHA-256"
    );
}

#[test]
fn detector_is_named_silero() {
    assert_eq!(load_good().name(), "silero");
}

// ---- Acceptance 1: real speech is speech -----------------------------------------------

#[test]
fn speech_id_phrase_is_speech() {
    let (name, audio) = speech_clip(0);
    assert_speech(name, &audio);
}

#[test]
fn speech_ru_lingualibre_is_speech() {
    let (name, audio) = speech_clip(1);
    assert_speech(name, &audio);
}

#[test]
fn connection_test_speech_is_speech() {
    let (name, audio) = speech_clip(2);
    assert_speech(name, &audio);
}

// ---- Acceptance 1: real non-speech is not speech ---------------------------------------

#[test]
fn digital_silence_is_not_speech() {
    let audio = AudioBuffer::from_16k_mono(vec![0i16; 2 * SAMPLE_RATE]);
    assert_not_speech("digital silence (2 s)", &audio);
}

#[test]
fn room_tone_is_not_speech() {
    assert_not_speech("room-tone-ac-unit", &buffer("room-tone-ac-unit", ROOM_TONE));
}

#[test]
fn cough_woman_is_not_speech() {
    assert_not_speech("cough-woman", &buffer("cough-woman", COUGH_WOMAN));
}

#[test]
fn cough_smoker_is_not_speech() {
    assert_not_speech("cough-smoker", &buffer("cough-smoker", COUGH_SMOKER));
}

#[test]
fn keyboard_model_m_is_not_speech() {
    assert_not_speech(
        "keyboard-model-m",
        &buffer("keyboard-model-m", KEYBOARD_MODEL_M),
    );
}

#[test]
fn keyboard_soft_laptop_is_not_speech() {
    assert_not_speech(
        "keyboard-soft-laptop",
        &buffer("keyboard-soft-laptop", KEYBOARD_SOFT_LAPTOP),
    );
}

// ---- invariant 2: empty audio ------------------------------------------------------------

#[test]
fn empty_buffer_is_not_speech() {
    let d = load_good();
    assert_eq!(
        d.contains_speech(&AudioBuffer::from_16k_mono(Vec::new())),
        Ok(false),
        "an empty recording"
    );
}

// ---- Acceptance 3 / invariant 1: bad model files are Unavailable, before whisper.cpp ----

#[test]
fn missing_model_is_unavailable() {
    let dir = TempDir::new("missing");
    assert_unavailable(&dir.0.join(SILERO_MODEL.file_name), "missing model file");
}

#[test]
fn directory_instead_of_model_is_unavailable() {
    let dir = TempDir::new("dir");
    let p = dir.0.join(SILERO_MODEL.file_name);
    fs::create_dir(&p).expect("create dir");
    assert_unavailable(&p, "a directory under the model's name");
}

#[test]
fn truncated_model_is_unavailable() {
    let dir = TempDir::new("truncated");
    let good = good_bytes();
    let p = dir.file(&good[..good.len() / 2]);
    assert_unavailable(
        &p,
        "the first half of the model (valid magic, tensors cut off)",
    );
}

#[test]
fn one_byte_flipped_model_is_unavailable() {
    let dir = TempDir::new("flipped");
    let mut bytes = good_bytes();
    let at = bytes.len() / 2;
    bytes[at] ^= 0x01;
    let p = dir.file(&bytes);
    assert_unavailable(&p, "the model with one byte flipped (pinned size)");
}

#[test]
fn path_with_nul_is_unavailable() {
    // whisper-rs panics on a NUL in the path (CString::new(..).expect); load must refuse it.
    assert_unavailable(
        Path::new("ggml-silero\0.bin"),
        "a path holding a NUL character",
    );
}

/// Work of the child process of `hostile_header_is_unavailable_and_the_process_survives`; a
/// no-op unless the parent set `VOICEN_SILERO_HOSTILE_CHILD`.
#[test]
#[ignore = "child of hostile_header_is_unavailable_and_the_process_survives"]
fn hostile_header_child_run() {
    if std::env::var_os(HOSTILE_CHILD_VAR).is_none() {
        return;
    }
    let dir = TempDir::new("hostile");
    let p = dir.file(&hostile_header_bytes());
    assert_unavailable(&p, "pinned size, str_len = i32::MAX");
}

/// The hostile file in a child process: if `load` handed it to whisper.cpp, the child aborts
/// (a C++ exception through `extern "C"`) and only the child's exit status shows it.
#[test]
fn hostile_header_is_unavailable_and_the_process_survives() {
    let hostile = hostile_header_bytes();
    assert_eq!(
        hostile.len() as u64,
        SILERO_MODEL.size_bytes,
        "the hostile file must have the pinned size (only the hash may refuse it)"
    );
    let exe = std::env::current_exe().expect("current test exe");
    let out = Command::new(exe)
        .args([
            "--exact",
            "hostile_header_child_run",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(HOSTILE_CHILD_VAR, "1")
        .env(MODEL_VAR, model_path().parent().expect("model dir"))
        .output()
        .expect("start the child test run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "the child run with the hostile model did not succeed (aborted or failed): {:?}",
        out.status
    );
    assert!(
        stdout.contains("1 passed"),
        "the child run did not run hostile_header_child_run"
    );
}

// ---- Acceptance 3: the gate falls back to the energy detector -------------------------

#[test]
fn gate_with_a_missing_model_falls_back_to_energy_and_warns_once() {
    let dir = TempDir::new("gate-missing");
    let gate = SpeechGate::new(
        SileroDetector::load(&dir.0.join(SILERO_MODEL.file_name)),
        EnergyDetector::new(),
    );
    let (_, audio) = speech_clip(0);
    let first = gate.decide(&audio);
    let second = gate.decide(&audio);
    assert_eq!(first.detector, "energy", "first decision's detector");
    assert!(
        first.fallback_warning,
        "the first fallback decision carries no warning"
    );
    assert_eq!(second.detector, "energy", "second decision's detector");
    assert!(!second.fallback_warning, "the second decision warns again");
}

#[test]
fn gate_with_a_corrupt_model_falls_back_to_energy_and_warns_once() {
    let dir = TempDir::new("gate-corrupt");
    let mut bytes = good_bytes();
    let at = bytes.len() / 3;
    bytes[at] ^= 0x80;
    let gate = SpeechGate::new(
        SileroDetector::load(&dir.file(&bytes)),
        EnergyDetector::new(),
    );
    let (_, audio) = speech_clip(0);
    let first = gate.decide(&audio);
    let second = gate.decide(&audio);
    assert_eq!(first.detector, "energy", "first decision's detector");
    assert!(
        first.fallback_warning,
        "the first fallback decision carries no warning"
    );
    assert!(!second.fallback_warning, "the second decision warns again");
}

#[test]
fn gate_with_the_good_model_decides_with_silero_without_warning() {
    let gate = SpeechGate::new(SileroDetector::load(&model_path()), EnergyDetector::new());
    let (name, speech) = speech_clip(0);
    let cough = buffer("cough-woman", COUGH_WOMAN);
    for (what, audio, want) in [(name, &speech, true), ("cough-woman", &cough, false)] {
        let d = gate.decide(audio);
        assert_eq!(d.detector, "silero", "{what}: decided by");
        assert!(!d.fallback_warning, "{what}: warned with a good model");
        assert_eq!(d.speech, want, "{what}: speech decision");
    }
}

// ---- invariant 2: one detector, concurrent callers -------------------------------------

#[test]
fn two_threads_decide_at_once() {
    let detector: Arc<Box<dyn SpeechDetector>> = Arc::new(load_good());
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = [true, false]
        .into_iter()
        .map(|speech| {
            let detector = Arc::clone(&detector);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let (name, audio) = if speech {
                    speech_clip(0)
                } else {
                    (
                        "keyboard-model-m",
                        buffer("keyboard-model-m", KEYBOARD_MODEL_M),
                    )
                };
                let mut got = Vec::new();
                barrier.wait();
                for _ in 0..3 {
                    got.push(detector.contains_speech(&audio));
                }
                (name, speech, got)
            })
        })
        .collect();
    for h in handles {
        let (name, want, got) = h.join().expect("a deciding thread panicked");
        for r in got {
            assert_eq!(r, Ok(want), "{name}: concurrent decision");
        }
    }
}

// ---- latency figure for the CI log (T-043 review 1 finding 1) --------------------------

/// Prints the elapsed time of three decisions on the 3 s `keyboard-model-m` clip, and their
/// best, to the process's stderr past libtest's capture, so the Windows CI log carries the
/// figure (`silero-timing:` lines). Print only: no wall-clock ceiling (F-013,
/// docs/decisions/core-tests.md). The other tests of this binary run at the same time, so
/// the best of three is the figure closest to a decision with nothing else running.
#[test]
fn three_second_decision_time_is_printed() {
    use std::io::Write;
    use std::time::Instant;
    let d = load_good();
    let audio = buffer("keyboard-model-m", KEYBOARD_MODEL_M);
    let mut ms = Vec::new();
    for _ in 0..3 {
        let start = Instant::now();
        let got = d.contains_speech(&audio);
        ms.push(start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(got, Ok(false), "keyboard-model-m: timed decision");
    }
    let best = ms.iter().copied().fold(f64::INFINITY, f64::min);
    let audio_s = audio.samples().len() as f64 / SAMPLE_RATE as f64;
    let cpus = std::thread::available_parallelism().map_or(0, |n| n.get());
    let line = format!(
        "silero-timing: {audio_s:.2} s clip, decisions {ms:.1?} ms, best {best:.1} ms, \
         available parallelism {cpus}\n"
    );
    let _ = std::io::stderr().write_all(line.as_bytes());
}

// ---- nothing of whisper.cpp / ggml on stdout or stderr ---------------------------------

/// Work of the child process of `silero_prints_nothing`; a no-op unless the parent set
/// `VOICEN_SILERO_QUIET_CHILD`.
#[test]
#[ignore = "child of silero_prints_nothing"]
fn quiet_child_run() {
    if std::env::var_os(QUIET_CHILD_VAR).is_none() {
        return;
    }
    let d = load_good();
    let (_, speech) = speech_clip(0);
    let _ = d.contains_speech(&speech);
    let _ = d.contains_speech(&buffer("cough-woman", COUGH_WOMAN));
    let _ = d.contains_speech(&AudioBuffer::from_16k_mono(Vec::new()));
    let dir = TempDir::new("quiet");
    let _ = SileroDetector::load(&dir.0.join("absent.bin"));
    let good = good_bytes();
    let _ = SileroDetector::load(&dir.file(&good[..good.len() / 2]));
}

#[test]
fn silero_prints_nothing() {
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
        .env(MODEL_VAR, model_path().parent().expect("model dir"))
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
        for token in ["whisper", "ggml", "vad", "silero"] {
            let n = text.matches(token).count();
            assert_eq!(n, 0, "the child's {name} holds '{token}' {n} time(s)");
        }
    }
}
