# Failures

<!-- This log is filled by this project as incidents happen.
     One entry per incident or defect class worth remembering. This is what the reviewer checks
     every fix against. A second entry of the same class = the rule moves up a tier (see PRINCIPLES.md).
     Describe what happened, not who did it. -->

| ID | Date | What broke | Why (root cause) | Rule it produced | Principle | Tasks |
|---|---|---|---|---|---|---|
| F-001 | 2026-10-03 | Windows CI red since T-003: voicen-core doctest secrets.rs:16 fails to link (LNK4003 on target/debug/build/voicen-*/out/msvcrt.lib, LNK1120 CRT symbols); tauri build, install and smoke never ran | tauri-build's static VC runtime puts a stub msvcrt.lib on the shell's link search path; cargo passes all build scripts' search dirs to every doctest of one invocation, but only the own package's link-args (cargo rust-1.99.0 cargo_test.rs:231, build_runner/mod.rs:301-312) | On the Windows job the shell package is tested in its own cargo invocation; every other crate runs with --exclude voicen (ci.yml comment) | — | T-033 |

## Classes

<!-- Group recurring entries; the recurrence count drives tier upgrades. -->

| Class | Entries | Count | Current tier |
|---|---|---|---|
| ci-toolchain | F-001 | 1 | T1 |
