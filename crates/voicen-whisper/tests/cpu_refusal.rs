//! T-017 (decision #142, analysis seam (4)): `WhisperLoader::load` refuses a CPU without
//! AVX2 with `LocalEngineFailed` BEFORE any native call (a non-AVX2 CPU would die with an
//! illegal instruction in the first whisper.cpp load). Runs in the Windows CI job only
//! (whisper.cpp builds there, decisions #12); the runner has AVX2, so the predicate is
//! injected.
//!
//! Pinned test-only surface (`#[doc(hidden)]`, as T-109's `with_threads`):
//! `WhisperLoader::with_cpu_check(check: fn() -> bool) -> WhisperLoader`: the loader asks
//! `check` at the start of every load (`load` and `load_whisper`); `WhisperLoader::new()`
//! and `Default` use the real predicate shared with `SileroDetector` (`cpu_supported`).
//!
//! The refusal is observable only against a model that WOULD load: a missing file also
//! ends `LocalEngineFailed` from the native call. So the model is the CI-cached tiny
//! (`VOICEN_WHISPER_TEST_MODELS_DIR`, red when missing) and the control leg loads it with
//! the predicate true.

use std::path::PathBuf;

use voicen_core::engine::speech_model::SpeechModelLoader;
use voicen_core::failure::FailureReason;
use voicen_core::local_models::catalog::{ModelId, MODELS};
use voicen_core::local_models::store::ModelStore;
use voicen_whisper::WhisperLoader;

const MODELS_DIR_VAR: &str = "VOICEN_WHISPER_TEST_MODELS_DIR";

fn tiny_path() -> PathBuf {
    let dir = std::env::var_os(MODELS_DIR_VAR)
        .unwrap_or_else(|| panic!("{MODELS_DIR_VAR} is not set: the CI fetch step must run first"));
    ModelStore::new(PathBuf::from(dir), MODELS)
        .path_if_downloaded(ModelId::Tiny)
        .unwrap_or_else(|| panic!("the store in {MODELS_DIR_VAR} has no downloaded tiny model"))
}

fn no_avx2() -> bool {
    false
}

fn avx2() -> bool {
    true
}

#[test]
fn a_cpu_without_avx2_is_refused_with_local_engine_failed_for_a_model_that_would_load() {
    // Bite: no predicate in `load` (the valid tiny loads, Ok), the predicate asked only
    // in `load_whisper` or only in `load` (the other entry loads), a refusal with another
    // reason, a predicate asked after `WhisperContext::new_with_params` (the native call ran).
    let path = tiny_path();

    let control = WhisperLoader::with_cpu_check(avx2);
    assert!(
        control.load(&path).is_ok(),
        "control: the predicate says AVX2, the cached tiny loads"
    );

    let loader = WhisperLoader::with_cpu_check(no_avx2);
    assert_eq!(
        loader.load(&path).err(),
        Some(FailureReason::LocalEngineFailed),
        "SpeechModelLoader::load"
    );
    assert_eq!(
        loader.load_whisper(&path).err(),
        Some(FailureReason::LocalEngineFailed),
        "load_whisper"
    );
}
