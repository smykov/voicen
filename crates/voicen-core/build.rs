use std::process::Command;

#[path = "src/build_commit.rs"]
mod build_commit;

// Embeds the commit the binary is built from: VOICEN_COMMIT (set by CI) or `git rev-parse`,
// chosen by `build_commit::pick_commit`.
fn main() {
    println!("cargo:rerun-if-env-changed=VOICEN_COMMIT");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs");
    let env = std::env::var("VOICEN_COMMIT").ok();
    let git = Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
    let commit = build_commit::pick_commit(env.as_deref(), git.as_deref());
    println!("cargo:rustc-env=VOICEN_COMMIT={commit}");
}
