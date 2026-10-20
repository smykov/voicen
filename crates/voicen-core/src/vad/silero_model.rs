//! The Silero VAD model file: its one pin, its verification and its fetch (T-043;
//! decisions #60, #136).
//!
//! [`SILERO_MODEL`] is the only place the pin lives (T-043 invariant 3): the CI fetch
//! (`examples/fetch_model.rs <dir> silero`), the runtime [`verify`] and the
//! `licenses/manual.json` entry derive from it. whisper.cpp's VAD loader cannot fail
//! cleanly on bad bytes (it throws C++ exceptions across `extern "C"`), so a detector hands
//! it only a file [`verify`] accepted (invariant 1). [`fetch`] goes through the model
//! downloader's transfer (`local_models::download`), never a second one (invariant 4).

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use sha2::{Digest, Sha256};

use super::VadError;
use crate::local_models::download::{download_pinned, hex_eq, DownloadFailure, End, Pinned};
use crate::local_models::store::ModelStore;
use crate::timeouts::Timeouts;

/// A model file pinned by its URL, exact size and SHA-256.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelPin {
    /// The file's name on disk and the last segment of `url`.
    pub file_name: &'static str,
    /// Download URL at a fixed upstream commit (never a branch).
    pub url: &'static str,
    /// Exact size in bytes.
    pub size_bytes: u64,
    /// SHA-256, 64 lowercase hex digits; never computed from upstream at runtime.
    pub sha256: &'static str,
}

/// The Silero VAD model v6.2.0 in whisper.cpp's ggml format (`ggml-org/whisper-vad`, MIT),
/// at the commit in `url`. A `static`, not a `const`: one
/// value at one address, and checks of it are not folded at compile time.
pub static SILERO_MODEL: ModelPin = ModelPin {
    file_name: "ggml-silero-v6.2.0.bin",
    url: "https://huggingface.co/ggml-org/whisper-vad/resolve/9ffd54a1e1ee413ddf265af9913beaf518d1639b/ggml-silero-v6.2.0.bin",
    size_bytes: 885_098,
    sha256: "2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987",
};

/// Checks `path` against [`SILERO_MODEL`]; see [`verify_against`].
pub fn verify(path: &Path) -> Result<(), VadError> {
    verify_against(path, &SILERO_MODEL)
}

/// `Ok` only when `path` is a regular file of exactly `pin.size_bytes` bytes whose
/// SHA-256 is `pin.sha256` (the size is checked first, from the metadata). Anything else,
/// an I/O error included, is `Err(VadError::Unavailable)`; never panics.
pub fn verify_against(path: &Path, pin: &ModelPin) -> Result<(), VadError> {
    let meta = std::fs::metadata(path).map_err(|_| VadError::Unavailable)?;
    if !meta.is_file() || meta.len() != pin.size_bytes {
        return Err(VadError::Unavailable);
    }
    let digest = sha256_of(path).map_err(|_| VadError::Unavailable)?;
    if hex_eq(&digest, pin.sha256) {
        Ok(())
    } else {
        Err(VadError::Unavailable)
    }
}

/// Streamed SHA-256 of the file at `path`.
fn sha256_of(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(buf.get(..n).unwrap_or_default()),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(hasher.finalize().to_vec())
}

/// Downloads [`SILERO_MODEL`] into `dir`; see [`fetch_against`].
pub fn fetch(dir: &Path, timeouts: &Timeouts) -> Result<PathBuf, DownloadFailure> {
    fetch_against(dir, &SILERO_MODEL, timeouts)
}

/// Downloads `pin` into `dir` (created when missing) through the model downloader's
/// transfer: `<dir>/<file_name>.part`, renamed to `<dir>/<file_name>` only when the size
/// and SHA-256 equal the pin's. `Ok` is the final path; every failure attempts to remove
/// the `.part` and leaves no final file. Blocking; not for a tokio runtime thread.
pub fn fetch_against(
    dir: &Path,
    pin: &ModelPin,
    timeouts: &Timeouts,
) -> Result<PathBuf, DownloadFailure> {
    let pinned = Pinned {
        file_name: pin.file_name,
        url: pin.url,
        size_bytes: pin.size_bytes,
        sha256: pin.sha256,
    };
    // Nothing cancels a fetch.
    let never = AtomicBool::new(false);
    match download_pinned(pinned, dir, timeouts, &never, &|_| {}) {
        End::Finished => Ok(ModelStore::final_path_in(dir, pin.file_name)),
        End::Failed(reason) => Err(reason),
        // Not reachable (`never` stays false); reported as an interrupted download.
        End::Cancelled => Err(DownloadFailure::DownloadInterrupted),
    }
}
