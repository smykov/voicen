use std::io::Write;
use std::path::PathBuf;

use voicen_core::BuildInfo;

#[tauri::command]
fn get_build_info() -> BuildInfo {
    voicen_core::build_info()
}

/// `%LOCALAPPDATA%\Voicen\logs` (FR-20); the system temp dir when LOCALAPPDATA is not set.
fn log_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Voicen")
        .join("logs")
}

/// Writes the start line with version and commit (FR-18); a failure never stops the app.
fn log_start() {
    let dir = log_dir();
    let line = format!("voicen {} started\n", voicen_core::build_info());
    let written = std::fs::create_dir_all(&dir).and_then(|_| {
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
    log_start();
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![get_build_info])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
