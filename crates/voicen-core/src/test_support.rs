//! Test helpers shared by this crate's tests and other crates' tests (feature
//! `test-fakes`; never in a release build).

pub mod fixtures;
pub mod local_models;
pub mod realtime;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A fresh, empty directory under the system temp dir, removed (with its content)
/// on drop. Each instance gets its own directory, so tests can run in parallel.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Panics if no directory can be created (a test environment problem).
    pub fn new() -> TempDir {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        loop {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = base.join(format!("voicen-test-{}-{n}-{nanos}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return TempDir { path },
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("cannot create a temp dir {}: {e}", path.display()),
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Default for TempDir {
    fn default() -> TempDir {
        TempDir::new()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
