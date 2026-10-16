//! The built-in engine's model port (spec 002 contracts/core-traits.md
//! "SpeechModelLoader / SpeechModel", T-098). The core holds no whisper code
//! (decision #12): `crates/voicen-whisper` implements these traits (T-099), the
//! core tests use fakes.
//!
//! Both traits are synchronous (decision #42) and speak [`FailureReason`]: a load
//! or transcription error is `LocalEngineFailed`; the residency and the engine pass
//! `NoLocalModel` on and turn every other error into `LocalEngineFailed`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::clock::MonotonicClock;
use crate::failure::FailureReason;

/// Loads a model file into memory.
pub trait SpeechModelLoader: Send + Sync {
    /// Loads the model at `path` (a path from `ModelStore::path_if_downloaded`).
    fn load(&self, path: &Path) -> Result<Box<dyn SpeechModel>, FailureReason>;
}

/// A loaded model. Dropping the box unloads it.
pub trait SpeechModel: Send {
    /// Transcribes 16 kHz mono f32 samples in `[-1, 1]`. `language` is an ISO
    /// 639-1 code, `None` = auto-detect. The model polls `abort` while it runs and
    /// stops as soon as [`Abort::requested`] is true (decision #133: the built-in
    /// limit covers the transcription only); what it returns then is ignored, the
    /// engine reports `Timeout`.
    fn transcribe(
        &mut self,
        pcm: &[f32],
        language: Option<&str>,
        abort: &Abort,
    ) -> Result<String, FailureReason>;
}

/// The abort hook of one transcription: true once `limit` has passed since `start`
/// on the monotonic port. `Clone + Send + Sync + 'static`, so a model can move it into a
/// native abort callback.
#[derive(Clone)]
pub struct Abort {
    clock: Arc<dyn MonotonicClock>,
    start: Instant,
    limit: Duration,
    fired: Arc<AtomicBool>,
}

impl Abort {
    /// A hook that turns true when `limit` has passed on `clock` since `start`.
    pub fn new(clock: Arc<dyn MonotonicClock>, start: Instant, limit: Duration) -> Abort {
        Abort {
            clock,
            start,
            limit,
            fired: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Whether the model must stop now: `now - start >= limit` (the deadline
    /// itself counts as passed). Once true it stays true, for every clone.
    pub fn requested(&self) -> bool {
        if self.fired.load(Ordering::SeqCst) {
            return true;
        }
        if self.clock.now().saturating_duration_since(self.start) >= self.limit {
            self.fired.store(true, Ordering::SeqCst);
            return true;
        }
        false
    }

    /// Whether [`requested`](Self::requested) ever answered true: the model was
    /// asked to stop.
    pub(crate) fn fired(&self) -> bool {
        self.fired.load(Ordering::SeqCst)
    }
}

impl std::fmt::Debug for Abort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Abort")
            .field("start", &self.start)
            .field("limit", &self.limit)
            .field("fired", &self.fired())
            .finish()
    }
}
