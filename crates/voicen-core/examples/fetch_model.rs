//! Fetches one catalog model into a directory through the one model downloader
//! (`local_models::download::Downloader`: catalog size and pinned SHA-256, `.part` then
//! rename). Used by the Windows CI job to provide the tiny model for
//! `crates/voicen-whisper/tests/transcribe_wav.rs` (T-099); not shipped.
//!
//! Usage: `cargo run -p voicen-core --example fetch_model -- <dir> <model id>`
//! Exit 0: the model is in `<dir>` (already there, or downloaded and verified).
//! Exit 1: the download was refused or failed (the reason's wire code is printed).
//! Exit 2: wrong usage, or `<dir>` cannot be created.

use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{mpsc, Arc};

use voicen_core::local_models::catalog::{ModelId, MODELS};
use voicen_core::local_models::download::{DiskSpace, DownloadEvent, Downloader};
use voicen_core::local_models::store::ModelStore;
use voicen_core::timeouts::Timeouts;

/// No free-space probe: the downloader then proceeds (R-9); a full disk ends the
/// download as a disk error instead.
struct NoProbe;

impl DiskSpace for NoProbe {
    fn available_bytes(&self, _dir: &Path) -> io::Result<u64> {
        Err(io::Error::other("no free-space probe in fetch_model"))
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir, id] = args.as_slice() else {
        eprintln!("usage: fetch_model <dir> <model id>");
        return ExitCode::from(2);
    };
    let Some(id) = ModelId::parse(id) else {
        eprintln!("fetch_model: {id} is not a catalog model id");
        return ExitCode::from(2);
    };
    let dir = PathBuf::from(dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("fetch_model: cannot create {}: {e}", dir.display());
        return ExitCode::from(2);
    }
    let store = Arc::new(ModelStore::new(dir, MODELS));
    if let Some(path) = store.path_if_downloaded(id) {
        println!("fetch_model: {} is already downloaded", path.display());
        return ExitCode::SUCCESS;
    }

    let downloader = Downloader::new(Arc::clone(&store), Arc::new(NoProbe), Timeouts::default());
    let (tx, rx) = mpsc::channel();
    let started = downloader.start(id, move |event| {
        if !matches!(event, DownloadEvent::Progress { .. }) {
            // The receiver is gone only when main has already ended.
            let _ = tx.send(event);
        }
    });
    if let Err(e) = started {
        eprintln!("fetch_model: the download did not start: {e:?}");
        return ExitCode::from(1);
    }
    match rx.recv() {
        Ok(DownloadEvent::Finished { .. }) => match store.path_if_downloaded(id) {
            Some(path) => {
                println!("fetch_model: downloaded {}", path.display());
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("fetch_model: finished, but the store does not see the model");
                ExitCode::from(1)
            }
        },
        Ok(DownloadEvent::Failed { reason, .. }) => {
            eprintln!("fetch_model: the download failed: {}", reason.code());
            ExitCode::from(1)
        }
        Ok(_) => {
            eprintln!("fetch_model: the download was cancelled");
            ExitCode::from(1)
        }
        Err(_) => {
            eprintln!("fetch_model: the download thread ended without a result");
            ExitCode::from(1)
        }
    }
}
