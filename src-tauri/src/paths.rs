//! The one data-directory resolver (T-030 J2, P-010; architecture.md: "shell (one
//! function)"). Logs and settings (later history, models) derive from it.
//!
//! T-030 RED SKELETON (test-writer): signatures settled by the T-030 Investigation
//! (item 5); the bodies are stubs the developer replaces.

use std::path::PathBuf;

/// `%LOCALAPPDATA%\Voicen`; `temp_dir()\Voicen` when LOCALAPPDATA is not set.
pub fn data_dir() -> PathBuf {
    // RED STUB
    todo!("T-030: %LOCALAPPDATA%\\Voicen, falling back to temp_dir()\\Voicen")
}

/// `data_dir()\logs`.
pub fn log_dir() -> PathBuf {
    // RED STUB
    todo!("T-030: data_dir().join(\"logs\")")
}
