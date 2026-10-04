# Failures

<!-- This log is filled by this project as incidents happen.
     One entry per incident or defect class worth remembering. This is what the reviewer checks
     every fix against. A second entry of the same class = the rule moves up a tier (see PRINCIPLES.md).
     Describe what happened, not who did it. -->

| ID | Date | What broke | Why (root cause) | Rule it produced | Principle | Tasks |
|---|---|---|---|---|---|---|
| F-001 | 2026-10-03 | Windows CI red since T-003: voicen-core doctest secrets.rs:16 fails to link (LNK4003 on target/debug/build/voicen-*/out/msvcrt.lib, LNK1120 CRT symbols); tauri build, install and smoke never ran | tauri-build's static VC runtime puts a stub msvcrt.lib on the shell's link search path; cargo passes all build scripts' search dirs to every doctest of one invocation, but only the own package's link-args (cargo rust-1.99.0 cargo_test.rs:231, build_runner/mod.rs:301-312) | On the Windows job the shell package is tested in its own cargo invocation; every other crate runs with --exclude voicen (ci.yml comment) | — | T-033 |
| F-002 | 2026-10-03 | Windows CI red at f2e239b (T-030): src-tauri/tests/settings_ipc.rs exe exits 0xc0000139 STATUS_ENTRYPOINT_NOT_FOUND before any test; tauri build, install and smoke skipped | tauri-build scopes its link outputs for the app bin: the Common-Controls v6 manifest reaches bin targets only (embed-resource rustc-link-arg-bins; cargo custom_build.rs:266-277), so integration-test exes bind comctl32 v5.82 and muda's TaskDialogIndirect import (tauri default feature common-controls-v6) has no entry point. Same class mechanism as F-001: each test-exe kind gets a different subset of the shell's build-script outputs | src-tauri/build.rs gives every integration test the same manifest (rustc-link-arg-tests /MANIFEST:EMBED /MANIFESTINPUT, windows-msvc only); shell tests with tests live only in src-tauri/tests, checked in make check (docs/decisions/ci-toolchain.md) | — | T-030, T-035 |
| F-003 | 2026-10-04 | shell-test-layout guard passed doc shapes rustdoc reads as code blocks, in T-035 r1, T-035 r2/T-036 analysis, T-036 r1 and b8b0b81 (no CI failure; caught in review) | a host-side awk re-implementation of rustc doc desugaring, rustdoc unindent and CommonMark decided pass by default; each unmodelled clause was a silent pass | a guard decides on a switch or on raw text the tool cannot reinterpret, never on a model of another tool's parser; the doc scan was dropped and the doctest switches pinned (docs/decisions/ci-toolchain.md) | — | T-035, T-036, T-038 |
| F-004 | 2026-10-04 | A voicen-core test expecting a refused connection flaked once in 37 runs (T-016 review round 1 #1); the same pattern sat in three test files: tests/local_download.rs, tests/openai_client.rs, tests/api_pipeline.rs | The refused-port helpers bound 127.0.0.1:0, read the port and released it; the kernel can hand a released ephemeral port to a sibling test's server in the same binary, so the address was no longer refused (measured ~1/7058 per bind, T-047). A "refused" address taken from a released port has no guarantee | A refused address never comes from a released port: tests use `common::refused_addr()` (127.0.0.1:1, below the Linux and Windows ephemeral ranges), which asserts a `ConnectionRefused` probe at each use so a listener on port 1 fails loudly. `local_download_refused` keeps its own test binary (refused-then-serve; 027aa47). Decisions #53 | — | T-016 (reviews/1.md #1), T-047 |

## Classes

<!-- Group recurring entries; the recurrence count drives tier upgrades. -->

| Class | Entries | Count | Current tier |
|---|---|---|---|
| ci-toolchain | F-001, F-002 | 2 | T3 (make check: shell test layout; Windows job) |
| guard-model | F-003 | 1 | T3 (fixtures: make check-shell-layout-fixtures) |
| test-port-race | F-004 | 1 | T2 (`common::refused_addr()` probes at each use); no grep guard (decisions #53) |
