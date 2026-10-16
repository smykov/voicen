//! Shared fakes of the T-098 built-in engine tests (`tests/local_residency.rs`,
//! `tests/local_engine.rs`, `tests/local_no_network.rs`, `tests/local_model_delete.rs`):
//! a fake monotonic clock, a scriptable `SpeechModelLoader` / `SpeechModel` pair
//! and the model files the engine resolves through `ModelStore::path_if_downloaded`.
//!
//! No core test reads the clock here (docs/decisions/core-tests.md, T-089): every
//! time the residency and the engine read comes from [`FakeMono`], which starts at
//! an instant the test hands in (`common::timing::now()`) and moves only when a
//! test or a fake advances it. So "10 min idle", "a 450 s transcription" and "a
//! load 10x the limit" are exact and take no wall time (F-005, F-013).
//!
//! Pinned API (T-098 analysis seam, spec 002 contracts/core-traits.md as corrected
//! by the analysis: synchronous, `FailureReason`, a monotonic port):
//! - `voicen_core::clock::MonotonicClock: Send + Sync { fn now(&self) -> Instant; }`
//!   (production impl over `crate::clock::instant()`);
//! - `voicen_core::engine::speech_model::SpeechModelLoader: Send + Sync
//!   { fn load(&self, path: &Path) -> Result<Box<dyn SpeechModel>, FailureReason>; }`;
//! - `voicen_core::engine::speech_model::SpeechModel: Send { fn transcribe(&mut self,
//!   pcm: &[f32], language: Option<&str>, abort: &Abort) -> Result<String, FailureReason>; }`
//!   (dropping the box unloads the model);
//! - `voicen_core::engine::speech_model::Abort` (`Clone + Send + Sync + 'static`, so a
//!   real model can move it into a `'static` abort callback) with
//!   `fn requested(&self) -> bool`: the model polls it while it transcribes and stops
//!   when it turns true (#133: the built-in limit covers transcription only).
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use voicen_core::clock::MonotonicClock;
use voicen_core::engine::speech_model::{Abort, SpeechModel, SpeechModelLoader};
use voicen_core::failure::FailureReason;
use voicen_core::local_models::catalog::{CatalogEntry, ModelId};
use voicen_core::test_support::local_models::{catalog, leak};

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- clock ---------------------------------------------------------------------------

/// A monotonic clock that moves only when [`advance`](Self::advance) is called.
pub struct FakeMono {
    now: Mutex<Instant>,
}

impl FakeMono {
    /// Starts at `start` (a test passes `common::timing::now()`).
    pub fn starting_at(start: Instant) -> Arc<FakeMono> {
        Arc::new(FakeMono {
            now: Mutex::new(start),
        })
    }

    pub fn advance(&self, d: Duration) {
        let mut now = lock(&self.now);
        *now += d;
    }

    pub fn read(&self) -> Instant {
        *lock(&self.now)
    }
}

impl MonotonicClock for FakeMono {
    fn now(&self) -> Instant {
        *lock(&self.now)
    }
}

// ---- speech model --------------------------------------------------------------------

/// What the next load and every transcription of the fake do. Changeable at any
/// time through [`FakeLoader::script`]; a loaded model reads it at each call.
#[derive(Debug, Clone)]
pub struct Script {
    /// Fake time the load takes (the loader advances the clock by it).
    pub load_takes: Duration,
    /// The load returns `Err(LocalEngineFailed)` (after taking `load_takes`).
    pub load_fails: bool,
    /// The transcript of a transcription that runs to its end.
    pub text: String,
    /// Fake time a transcription takes when nothing aborts it.
    pub transcribe_takes: Duration,
    /// The clock step between two polls of the abort hook.
    pub step: Duration,
    /// A transcription that runs to its end returns `Err(LocalEngineFailed)`.
    pub transcribe_fails: bool,
}

impl Default for Script {
    fn default() -> Script {
        Script {
            load_takes: Duration::from_millis(1_234),
            load_fails: false,
            text: "local transcript".to_string(),
            transcribe_takes: Duration::from_secs(2),
            step: Duration::from_secs(1),
            transcribe_fails: false,
        }
    }
}

/// One `SpeechModel::transcribe` call as the fake saw it.
#[derive(Debug, Clone, PartialEq)]
pub struct Transcription {
    pub pcm: Vec<f32>,
    pub language: Option<String>,
    /// Fake time the call advanced the clock by before it returned.
    pub ran_for: Duration,
    /// The abort hook turned true and the model stopped early.
    pub aborted: bool,
}

/// Blocks a load until the test releases it: `entered` gets one message per load
/// that reached the gate; each `release` send lets one load go on.
pub struct Gate {
    pub entered: Receiver<PathBuf>,
    pub release: Sender<()>,
}

struct GateInside {
    entered: Sender<PathBuf>,
    release: Receiver<()>,
}

/// A `SpeechModelLoader` whose loads and models follow [`Script`] on a
/// [`FakeMono`], and which counts loads, live models and drops.
pub struct FakeLoader {
    clock: Arc<FakeMono>,
    script: Arc<Mutex<Script>>,
    loads: Mutex<Vec<PathBuf>>,
    live: Arc<AtomicUsize>,
    dropped: Arc<AtomicUsize>,
    calls: Arc<Mutex<Vec<Transcription>>>,
    gate: Mutex<Option<GateInside>>,
    transcription_gate: Arc<Mutex<Option<GateInside>>>,
}

impl FakeLoader {
    pub fn new(clock: &Arc<FakeMono>) -> Arc<FakeLoader> {
        FakeLoader::with(clock, Script::default())
    }

    pub fn with(clock: &Arc<FakeMono>, script: Script) -> Arc<FakeLoader> {
        Arc::new(FakeLoader {
            clock: Arc::clone(clock),
            script: Arc::new(Mutex::new(script)),
            loads: Mutex::new(Vec::new()),
            live: Arc::new(AtomicUsize::new(0)),
            dropped: Arc::new(AtomicUsize::new(0)),
            calls: Arc::new(Mutex::new(Vec::new())),
            gate: Mutex::new(None),
            transcription_gate: Arc::new(Mutex::new(None)),
        })
    }

    /// From now on every load waits at a [`Gate`] the test holds.
    pub fn gate(&self) -> Gate {
        let (entered_tx, entered_rx) = channel();
        let (release_tx, release_rx) = channel();
        *lock(&self.gate) = Some(GateInside {
            entered: entered_tx,
            release: release_rx,
        });
        Gate {
            entered: entered_rx,
            release: release_tx,
        }
    }

    /// From now on every transcription waits at a [`Gate`] the test holds, before
    /// it polls the abort hook or advances the clock (it holds the model while it
    /// waits, as a real run does). `entered` gets the pcm length of each
    /// transcription that reached the gate.
    pub fn gate_transcriptions(&self) -> Gate {
        let (entered_tx, entered_rx) = channel();
        let (release_tx, release_rx) = channel();
        *lock(&self.transcription_gate) = Some(GateInside {
            entered: entered_tx,
            release: release_rx,
        });
        Gate {
            entered: entered_rx,
            release: release_tx,
        }
    }

    /// Changes the script (the next load, and every later transcription).
    pub fn script(&self, edit: impl FnOnce(&mut Script)) {
        edit(&mut lock(&self.script));
    }

    /// Every path a load was asked for, in order (failed loads included).
    pub fn loads(&self) -> Vec<PathBuf> {
        lock(&self.loads).clone()
    }

    /// Loaded models not yet dropped.
    pub fn live(&self) -> usize {
        self.live.load(Ordering::SeqCst)
    }

    /// Models dropped (= unloaded) so far.
    pub fn dropped(&self) -> usize {
        self.dropped.load(Ordering::SeqCst)
    }

    pub fn transcriptions(&self) -> Vec<Transcription> {
        lock(&self.calls).clone()
    }

    pub fn last_transcription(&self) -> Transcription {
        lock(&self.calls)
            .last()
            .cloned()
            .unwrap_or_else(|| panic!("no transcription reached the model"))
    }
}

impl SpeechModelLoader for FakeLoader {
    fn load(&self, path: &Path) -> Result<Box<dyn SpeechModel>, FailureReason> {
        lock(&self.loads).push(path.to_path_buf());
        {
            let gate = lock(&self.gate);
            if let Some(gate) = gate.as_ref() {
                let _ = gate.entered.send(path.to_path_buf());
                // A dropped release sender lets every load through.
                let _ = gate.release.recv();
            }
        }
        let script = lock(&self.script).clone();
        self.clock.advance(script.load_takes);
        if script.load_fails {
            return Err(FailureReason::LocalEngineFailed);
        }
        self.live.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeModel {
            clock: Arc::clone(&self.clock),
            script: Arc::clone(&self.script),
            live: Arc::clone(&self.live),
            dropped: Arc::clone(&self.dropped),
            calls: Arc::clone(&self.calls),
            gate: Arc::clone(&self.transcription_gate),
        }))
    }
}

struct FakeModel {
    clock: Arc<FakeMono>,
    script: Arc<Mutex<Script>>,
    live: Arc<AtomicUsize>,
    dropped: Arc<AtomicUsize>,
    calls: Arc<Mutex<Vec<Transcription>>>,
    gate: Arc<Mutex<Option<GateInside>>>,
}

impl SpeechModel for FakeModel {
    fn transcribe(
        &mut self,
        pcm: &[f32],
        language: Option<&str>,
        abort: &Abort,
    ) -> Result<String, FailureReason> {
        {
            let gate = lock(&self.gate);
            if let Some(gate) = gate.as_ref() {
                let _ = gate.entered.send(PathBuf::from(pcm.len().to_string()));
                // A dropped release sender lets every transcription through.
                let _ = gate.release.recv();
            }
        }
        let script = lock(&self.script).clone();
        let mut ran_for = Duration::ZERO;
        let mut aborted = false;
        loop {
            if abort.requested() {
                aborted = true;
                break;
            }
            if ran_for >= script.transcribe_takes {
                break;
            }
            self.clock.advance(script.step);
            ran_for += script.step;
        }
        lock(&self.calls).push(Transcription {
            pcm: pcm.to_vec(),
            language: language.map(str::to_string),
            ran_for,
            aborted,
        });
        if aborted || script.transcribe_fails {
            return Err(FailureReason::LocalEngineFailed);
        }
        Ok(script.text)
    }
}

impl Drop for FakeModel {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::SeqCst);
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

// ---- model files ---------------------------------------------------------------------

/// Bytes of a fake model file: the ggml magic (`0x67676d6c` little-endian) and
/// padding, so a size or magic check before the load (research R-8) accepts it.
pub fn ggml_bytes() -> Vec<u8> {
    let mut bytes = vec![0x6c, 0x6d, 0x67, 0x67];
    bytes.extend((0..4_092u32).map(|i| (i % 251) as u8));
    bytes
}

/// [`ggml_bytes`] with the magic in the wrong byte order (`b"ggml"`, what a
/// big-endian read of `0x67676d6c` would write) and nothing else changed: the
/// size is right, so only a magic check refuses it (R-8, T-098 review 1 #3).
pub fn wrong_magic_bytes() -> Vec<u8> {
    let mut bytes = ggml_bytes();
    bytes[..4].copy_from_slice(b"ggml");
    bytes
}

/// Overwrites the first four bytes of the file at `path` with the ggml magic,
/// keeping its size: a fixture written for the store's size rule (for example
/// `test_support::local_models::model_bytes`) then also passes a magic check.
pub fn stamp_ggml_magic(path: &Path) {
    let mut bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    bytes[..4].copy_from_slice(&ggml_bytes()[..4]);
    std::fs::write(path, bytes).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

/// `ggml-<id>.bin`.
pub fn file_name(id: ModelId) -> &'static str {
    leak(format!("ggml-{}.bin", id.as_str()))
}

/// A catalog of every `ModelId` whose size is that of [`ggml_bytes`]; nothing is
/// ever downloaded from it (the URL is a documentation host).
pub fn local_catalog() -> &'static [CatalogEntry] {
    let size = ggml_bytes().len() as u64;
    catalog(
        ModelId::ALL
            .into_iter()
            .map(|id| CatalogEntry {
                id,
                file_name: file_name(id),
                url: leak(format!("https://models.example.com/{}", file_name(id))),
                size_bytes: size,
                sha256: "0000000000000000000000000000000000000000000000000000000000000000",
                recommended: false,
            })
            .collect(),
    )
}

/// Writes the fake model of each id in `ids` under `dir` with its final name.
pub fn place_models(dir: &Path, ids: &[ModelId]) {
    std::fs::create_dir_all(dir).expect("create models dir");
    for id in ids {
        std::fs::write(dir.join(file_name(*id)), ggml_bytes())
            .unwrap_or_else(|e| panic!("write {}: {e}", file_name(*id)));
    }
}
