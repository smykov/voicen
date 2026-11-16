//! The one place that resolves and loads the bundled speech detector (T-102 invariant 1).
//! The file comes from the installer's resources, never the data or the models directory.
//! In a release build `panic = "abort"` makes `SileroDetector::open`'s `catch_unwind` inert,
//! so a bad model or an unsupported CPU rests on its checks before any whisper.cpp call.
//! T-139: the one load feeds both the dictation gate and the call pass worker.

use std::path::Path;
use std::sync::Arc;

use voicen_core::vad::silero_model::bundled_path;
use voicen_core::vad::{SpeechDetector, SpeechSpans, VadError};
use voicen_whisper::SileroDetector;

/// The two halves of the one detector load: the dictation gate's and the call pass's.
pub type SharedDetector = (
    Result<Box<dyn SpeechDetector>, VadError>,
    Option<Arc<dyn SpeechSpans>>,
);

/// Loads the detector from `<resource_dir>/vad/` once (one `SileroDetector::open`) and
/// returns two clones of it over the one native context: the dictation gate's
/// (`DictationPorts::detector`) and the call pass worker's (`CallPorts::detector`). A model
/// that is missing, corrupt or unverifiable, or a CPU without AVX2, is
/// `(Err(VadError::Unavailable), None)`: both sides then fall back to the energy detector.
/// The ggml thread count stays `SileroDetector`'s own.
pub fn load_shared(resource_dir: &Path) -> SharedDetector {
    load_shared_with(resource_dir, SileroDetector::open)
}

/// [`load_shared`] over `open` (the product passes `SileroDetector::open`): `open` is called
/// exactly once, with `<resource_dir>/vad/<model file>`, and both halves are clones of what it
/// returned; an `Err` gives `(Err(e), None)`.
pub fn load_shared_with<D, F>(resource_dir: &Path, open: F) -> SharedDetector
where
    D: SpeechDetector + SpeechSpans + Clone + 'static,
    F: FnOnce(&Path) -> Result<D, VadError>,
{
    match open(&bundled_path(resource_dir)) {
        Ok(detector) => {
            let spans: Arc<dyn SpeechSpans> = Arc::new(detector.clone());
            (Ok(Box::new(detector)), Some(spans))
        }
        Err(e) => (Err(e), None),
    }
}
