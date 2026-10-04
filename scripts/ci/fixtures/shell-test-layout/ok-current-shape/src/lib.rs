//! The shape of src-tauri/src today: platform cfgs, tauri commands, a mobile entry point.
use std::path::Path;

#[cfg(windows)]
pub mod autostart;
pub mod locale;

#[tauri::command]
fn get_build_info() -> String {
    String::new()
}

#[tauri::command(async)]
async fn save() -> Result<(), String> {
    Ok(())
}

/// The command registration; reached only through [`run`].
fn commands(path: &Path) -> bool {
    path.exists()
}

#[cfg(windows)]
fn platform() -> &'static str {
    "windows"
}

#[cfg(not(windows))]
fn platform() -> &'static str {
    "other"
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = (platform(), commands(Path::new(".")));
}
