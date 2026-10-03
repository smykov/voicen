//! The one data-directory resolver (T-030 J2, P-010; architecture.md: "shell (one
//! function)"). Logs and settings (later history, models) derive from it.

use std::path::PathBuf;

/// `%LOCALAPPDATA%\Voicen`; `temp_dir()\Voicen` when LOCALAPPDATA is not set.
pub fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Voicen")
}

/// `data_dir()\logs` (FR-20).
pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}
