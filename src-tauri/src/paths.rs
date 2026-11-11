//! The one data-directory resolver (T-030 J2, P-010; architecture.md: "shell (one
//! function)"). Logs, settings and the built-in models (later history) derive from it.

use std::path::{Path, PathBuf};

use tauri::utils::config::{AppDirectoriesOverride, AppDirectoryOverrides};

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

/// `data_dir()\models`: the built-in models (T-044, spec 002; the one models-dir
/// resolver, P-010).
pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

/// `data_dir()\webview`: the WebView2 data of every window (T-062, FR-28 v4), so the
/// uninstaller's removal of the data folder takes it too.
pub fn webview_dir() -> PathBuf {
    data_dir().join("webview")
}

/// Routes tauri's app local data directory of `context` to `root\webview` (T-062).
/// tauri uses that directory as the WebView2 data folder of every window that sets none
/// of its own, so every window built from the app shares one environment there.
/// `run()` calls it once with `data_dir()` on the release context, before the app is
/// built (`webview_dir()`); the other app directories keep tauri's defaults.
pub fn route_app_local_data<R: tauri::Runtime>(context: &mut tauri::Context<R>, root: &Path) {
    context.config_mut().app.app_directories_override =
        Some(AppDirectoriesOverride::Directories(AppDirectoryOverrides {
            local_data: Some(root.join("webview")),
            ..Default::default()
        }));
}
