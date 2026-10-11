#![cfg(not(test))]
// Line 1 (T-092, F-002): under --cfg test (--bins, --all-targets) this crate is empty, so its
// unit-test exe holds no shell code; make check compares this line byte for byte
// (scripts/ci/shell-test-layout.sh, docs/decisions/ci-toolchain.md).
// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `voicen.exe --purge-credentials` (T-061, uninstaller): touches only Credential
    // Manager and exits before tauri, single instance, the log or any window exist.
    #[cfg(windows)]
    if let Some(code) = voicen_lib::win::purge::from_args(std::env::args_os(), "") {
        std::process::exit(code);
    }
    voicen_lib::run()
}
