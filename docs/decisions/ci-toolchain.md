# CI toolchain: Windows test executables of the shell

**Code:** `src-tauri/build.rs`, `src-tauri/windows-app-manifest.xml`, `.github/workflows/ci.yml` (windows job, the two `cargo test` steps), `scripts/ci/shell-test-layout.sh` (Makefile `check-shell-layout`, part of `make check`) · **Tests that pin it:** the windows job's `cargo test -p voicen` run of every `src-tauri/tests/*.rs` exe; `make check-shell-layout`

Tasks: T-033 (F-001), T-030 and T-035 (F-002). Class `ci-toolchain` in `docs/failures.md`. Decisions: #5 (the shell is built and tested only on the Windows runner).

## Why this area exists

`src-tauri/build.rs` calls tauri-build, which emits the Windows link configuration that a tauri-linked exe needs to link and start. tauri-build scopes each part of it for the app bin, through different cargo scopes:

| Output | Cargo scope | Reaches |
|---|---|---|
| stub `msvcrt.lib` search dir (static VC runtime) | `rustc-link-search` | every target of the cargo invocation, including other packages' doctests |
| `/NODEFAULTLIB` CRT args | `rustc-link-arg` | every target of the `voicen` package |
| Common-Controls v6 manifest (embed-resource) | `rustc-link-arg-bins` | bin targets only |

cargo's link-arg scopes are a closed set (cargo rust-1.99.0 `src/compiler/custom_build.rs:266-277`): all, bins, tests (integration tests), benches, examples, cdylib. No selector reaches a lib unit-test exe, a bin unit-test exe or a lib doctest without also reaching the bin. Every other exe kind cargo links for the package gets only part of the configuration. On Windows that part either fails to link (F-001) or links and does not start (F-002). The shell is first linked in the windows job, after review (decisions #5), so each mismatch costs a red windows job that blocks every `deploys: true` task behind it.

## Invariants

### Every Windows test exe of `voicen` gets the release exe's link configuration; only `src-tauri/tests/*.rs` contain shell tests

- **Defect that produced it:** F-002. At f2e239b (T-030) the `src-tauri/tests/settings_ipc.rs` exe exited `0xc0000139 STATUS_ENTRYPOINT_NOT_FOUND` before any test ran (run 37159339288). The manifest reached bin targets only, so the integration-test exe bound comctl32 v5.82, which lacks `TaskDialogIndirect` (imported by muda's About dialog, tauri default feature `common-controls-v6`). `tests/credentials.rs` creates no tauri app and passed.
- **What breaks if you violate it:** the exe fails to link or exits `0xc0000139`, and only in the windows job after review. The Linux gate never links the shell.
- **Where it is enforced:**
  - `src-tauri/build.rs`: after `tauri_build::build()`, on `windows` + `msvc` only, it emits `cargo:rustc-link-arg-tests=/MANIFEST:EMBED` and `/MANIFESTINPUT:<src-tauri>/windows-app-manifest.xml`. `-tests` reaches integration tests only, so the release bin keeps tauri-build's manifest unchanged. Upstream tauri does the same for its own tests (tauri 2.12.1 `build.rs`, `embed_manifest_for_tests`, tauri PR 4383).
  - `src-tauri/windows-app-manifest.xml` is a copy of tauri-build 2.7.1's `src/windows-app-manifest.xml`: the same elements, attributes and values (one `dependentAssembly`, `Microsoft.Windows.Common-Controls` 6.0.0.0, `processorArchitecture="*"`, `publicKeyToken="6595b64144ccf1df"`, `language="*"`). Only whitespace and comments differ. The comments sit inside the root element, never before it:
    - one names the source file and version;
    - one on the `publicKeyToken` line carries `teamwright:allow-secret`, because the commit secret scan reads `…Token="<16 hex>"` as a secret (it is the public key token of the Windows assembly; `docs/process/gates.md` › Secret scan, false-positive exit). That marker is why the copy cannot be byte-identical.
  - `make check` runs `scripts/ci/shell-test-layout.sh` on the host (grep and awk, no toolchain). It fails, naming this file, when the package could produce a test exe that `build.rs` does not configure:
    - a test attribute under `src-tauri/src`: `#[test]`, `#[<path>::test]`, any attribute whose name starts with `test`, or a `cfg`/`cfg_attr` predicate naming `test` (`#[cfg(test)]`, `#![cfg(test)]`, `#[cfg(all(test, …))]`);
    - a `///` or `//!` code fence under `src-tauri/src` other than ```` ```text ````, or `#[doc = include_str!(…)]`;
    - `src-tauri/benches`, `src-tauri/examples`, or `[[bench]]` / `[[example]]` in `src-tauri/Cargo.toml`.

    A grep or awk error is "cannot run" (exit 3), never a pass.
- **Rule for shell test authors:** put shell tests in `src-tauri/tests/<name>.rs`. They need no per-file setup; the manifest reaches every one of them. Put platform-independent logic and its unit tests in `crates/voicen-core`. Write examples in shell doc comments as ```` ```text ````. If a new exe kind is really needed, extend `build.rs`, this file and the check together in one reviewed change.
- **Don't:**
  - add `#[cfg(test)]` modules or doctests in `src-tauri/src`;
  - emit the manifest with `cargo:rustc-link-arg` (it reaches the bin too and duplicates its `RT_MANIFEST`) or drop the `-tests` scope;
  - edit `windows-app-manifest.xml`, or upgrade tauri-build, without comparing the copy's elements and values with the new tauri-build's file;
  - add `/WX` to the test link args (upstream does, for its own workspace; here it would turn any unrelated test-link warning into an error, T-030).

### Other packages' tests never share a cargo invocation with `voicen` on Windows

- **Defect that produced it:** F-001. From T-003 to T-033 a voicen-core doctest failed to link on windows-latest (LNK4003 on `target/debug/build/voicen-*/out/msvcrt.lib`, then LNK1120). cargo gives every build script's search dirs to every doctest of one invocation, but only the doctest's own package's link args (cargo rust-1.99.0 `cargo_test.rs:231`, `build_runner/mod.rs:301-312`).
- **What breaks if you violate it:** other crates' doctests link against the stub `msvcrt.lib` without the matching CRT args and fail to link.
- **Where it is enforced:** `.github/workflows/ci.yml`, windows job: `cargo test --workspace --exclude voicen`, then `cargo test -p voicen` (the comment above the two steps). The reviewer checks it.
- **Don't:** merge the two steps back into `cargo test --workspace`.

### A test exe that cannot start turns the windows job red

- **Defect that produced it:** none; it is what makes the two invariants above observable. In run 37159339288 cargo ended the step with exit code 1 when the settings_ipc exe did not start.
- **Where it is enforced:** the windows test steps have no `continue-on-error`, `|| true` or `--no-run`.
- **Don't:** add any of them to the windows test steps.

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| Turn off tauri's `common-controls-v6` feature | changes the release (muda's About dialog and the "WebView2 missing" dialog become `MessageBoxW` without a hyperlink); a case list: the next v6-only import brings the defect back | T-030 option C, F-002 |
| Manifest for all targets (`cargo:rustc-link-arg`) | next to tauri-build's resource the release link gets a duplicate `RT_MANIFEST` id 1 (`CVT1100`); without that resource (`new_without_app_manifest()`) link.exe generates the release manifest and adds `trustInfo`, so it changes | T-030 option D |
| `build.windows.staticVCRuntime: false` in tauri.conf.json | changes the shipped binary: `voicen.exe` would need the VC++ redistributable on the user's PC | T-033 |
| `cargo test --workspace --all-targets` on Windows (no doctests there) | hides the symptom, keeps the cause; future doctests would run only on Linux | T-033 option B |
| `RUSTDOCFLAGS=-Ctarget-feature=+crt-static` for the Windows test step | hides the shared search path instead of removing it; doctests link a different CRT than the test exes | T-033 option C |
| A CI step that fails unless every built test exe printed a `test result` line | redundant: cargo already fails the step when an exe does not start | T-035 option C |
| `embed_resource::compile_for_tests` | would need a direct build-dependency (owner consent, decisions #9); kept only as the fallback if `/MANIFESTINPUT` fails to link | T-035 fallback |

## Open

- Detection latency: shell code reaches `CODE_COMPLETE` without ever being linked on Windows (decisions #5). An optional owner decision is drafted in `docs/tasks/T-035.md` › Investigation ("exit (c)"). It is not needed for the invariants above.
