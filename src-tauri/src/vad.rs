//! The one place that resolves and loads the bundled speech detector (T-102 invariant 1).
//! The file comes from the installer's resources, never the data or the models directory.
//! In a release build `panic = "abort"` makes `SileroDetector::load`'s `catch_unwind` inert,
//! so a bad model or an unsupported CPU rests on its checks before any whisper.cpp call.

use std::path::Path;

use voicen_core::vad::silero_model::bundled_path;
use voicen_core::vad::{SpeechDetector, VadError};
use voicen_whisper::SileroDetector;

/// Loads the detector from `<resource_dir>/vad/`. A model that is missing, corrupt or
/// unverifiable, or a CPU without AVX2, is `Err(VadError::Unavailable)`; the gate then falls
/// back to the energy detector. The ggml thread count stays `SileroDetector`'s own.
pub fn load_bundled(resource_dir: &Path) -> Result<Box<dyn SpeechDetector>, VadError> {
    SileroDetector::load(&bundled_path(resource_dir))
}
