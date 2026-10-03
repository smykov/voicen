use std::io::Write;
use std::path::{Path, PathBuf};

use tauri::{Builder, Runtime};
use voicen_core::BuildInfo;

// T-030 RED SKELETON (test-writer): the modules below carry the signatures settled by
// the T-030 Investigation, with stub bodies, so src-tauri/tests compile. The developer
// replaces every stub (marked "RED STUB").
#[cfg(windows)]
pub mod credentials;
pub mod locale;
pub mod paths;
pub mod settings_ipc;

#[tauri::command]
fn get_build_info() -> BuildInfo {
    voicen_core::build_info()
}

/// The one command registration, shared by `run()` and the tests (T-030 J4).
pub fn commands<R: Runtime>(builder: Builder<R>) -> Builder<R> {
    builder.invoke_handler(tauri::generate_handler![
        get_build_info,
        settings_ipc::settings_get,
        settings_ipc::settings_save,
        settings_ipc::settings_speech_languages
    ])
}

/// `%LOCALAPPDATA%\Voicen\logs` (FR-20); the system temp dir when LOCALAPPDATA is not set.
// T-030: moves onto `paths::data_dir()` (Investigation item 5).
fn log_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Voicen")
        .join("logs")
}

/// Writes the start line with version and commit into `dir/voicen.log` (FR-18); a
/// failure never stops the app.
pub fn log_start(dir: &Path) {
    let line = format!("voicen {} started\n", voicen_core::build_info());
    let written = std::fs::create_dir_all(dir).and_then(|_| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("voicen.log"))
            .and_then(|mut f| f.write_all(line.as_bytes()))
    });
    if let Err(err) = written {
        eprintln!("cannot write log in {}: {err}", dir.display());
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    log_start(&log_dir());
    // T-030 RED SKELETON: the service (settings_ipc::load_settings over
    // paths::data_dir() and WinCredentialStore::new()) is not managed yet, and the
    // change bridge is not started in `.setup()`.
    commands(tauri::Builder::default())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
