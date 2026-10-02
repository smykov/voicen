use std::process::Command;

// Embeds the commit the binary is built from: VOICEN_COMMIT (set by CI) or `git rev-parse`.
fn main() {
    println!("cargo:rerun-if-env-changed=VOICEN_COMMIT");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs");
    let commit = std::env::var("VOICEN_COMMIT")
        .ok()
        .filter(|c| !c.trim().is_empty())
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "--short=7", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=VOICEN_COMMIT={commit}");
}
