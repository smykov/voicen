fn main() {
    tauri_build::build();
    embed_manifest_for_tests();
}

/// Gives every `tests/*.rs` integration-test exe the Common-Controls v6 manifest that
/// tauri-build embeds only into bin targets (F-002, T-035; docs/decisions/ci-toolchain.md).
/// Without it a test exe that creates a tauri app binds comctl32 v5.82 and exits
/// `0xc0000139` `STATUS_ENTRYPOINT_NOT_FOUND` (muda's `TaskDialogIndirect`) before any test runs.
/// `rustc-link-arg-tests` reaches integration tests only, so the release bin keeps
/// tauri-build's manifest unchanged. `windows-app-manifest.xml` is a copy of tauri-build
/// 2.7.1's `src/windows-app-manifest.xml` (same elements and values; comments and whitespace
/// differ); compare it with tauri-build's file on every tauri-build upgrade.
fn embed_manifest_for_tests() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" || target_env != "msvc" {
        return;
    }
    // Cargo sets it for every build-script run; without it the linker input is unknown.
    let Some(dir) = std::env::var_os("CARGO_MANIFEST_DIR") else {
        panic!("CARGO_MANIFEST_DIR is not set; cannot locate windows-app-manifest.xml");
    };
    let manifest = std::path::Path::new(&dir).join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed=windows-app-manifest.xml");
    println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-tests=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
