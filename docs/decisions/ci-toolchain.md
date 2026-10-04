# CI toolchain: Windows test executables of the shell

**Code:** `src-tauri/build.rs`, `src-tauri/windows-app-manifest.xml`, `src-tauri/Cargo.toml` (`[lib] doctest = false`), `.github/workflows/ci.yml` (windows job, the two `cargo test` steps; no `--doc`), `scripts/ci/shell-test-layout.sh` with its lexer `scripts/ci/shell-test-layout.awk` (Makefile `check-shell-layout`, part of `make check`) · **Tests that pin it:** the windows job's `cargo test -p voicen` run of every `src-tauri/tests/*.rs` exe; `make check-shell-layout`; `make check-shell-layout-fixtures` (`scripts/ci/shell-test-layout.test.sh` on the fixture shell dirs in `scripts/ci/fixtures/shell-test-layout/`)

Tasks: T-033 (F-001), T-030 and T-035 (F-002), T-036 (guard hardening, its doc scanner superseded), T-038 (F-003: doc scan dropped, doctest switches pinned). Classes `ci-toolchain` and `guard-model` in `docs/failures.md`. Decisions: #5 (the shell is built and tested only on the Windows runner), #37 (corrected by #41), #41 (supersedes #39).

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
  - `src-tauri/Cargo.toml` sets `[lib] doctest = false`, so plain `cargo test` builds no lib doctest exe. `cargo test --doc` (and `cargo rustdoc … -- --test`) ignores the key and builds the lib doctests anyway (cargo rust-1.99.0 `src/ops/cargo_compile/unit_generator.rs:425-440`; plain `cargo test` checks `doctested()` at `:397-410`). So both switches are pinned: the key by step 3 of the check below, the invocation by its step 4. Bins and `tests/*.rs` never get doctests, whatever their `doctest` key says (`src/workspace/manifest.rs:1018-1025`).
  - `make check` runs `scripts/ci/shell-test-layout.sh` on the host (bash, find, sort, grep, awk; no toolchain). It fails, naming this file, when the package could produce a test exe that `build.rs` does not configure. Under `src-tauri/src` it reads each file with a small lexer (`scripts/ci/shell-test-layout.awk`) that drops every comment (doc comments included) and string, raw-string and char literals, so text inside them never counts. Doc comments are not checked at all (T-038, decisions #41). It refuses:
    - a test attribute anywhere on a line, with any whitespace or line breaks inside it: `#[test]`, `# [test]`, `#[<path>::test]`, `#[test_case(…)]` (any attribute whose name starts with `test`), also as an attribute argument of `cfg_attr` (`#[cfg_attr(windows, test)]`);
    - a `cfg` or `cfg_attr` predicate naming `test`, on one line or split across lines: `#[cfg(test)]`, `#![cfg(test)]`, `#[cfg(all(windows, test))]`, `#[cfg_attr(test, …)]`;
    - a block comment, string or attribute left open at the end of a file;
    - a source rustc compiles from outside the scanned files:
      - `#[path = …]`, also as a `cfg_attr` argument;
      - the token `include` anywhere in code, outside comments and strings: `include!`, a rename (`use core::include as pull;`) and `r#include!`. An identifier named `include` is refused too (fail-closed; the message says to rename it). Identifiers that only contain it (`included`, `include_count`) and `include_str!` / `include_bytes!` pass;
      - any symlink under `src-tauri/src`. It is refused, not followed: `find -L` would skip a dangling link silently and walk a linked dir outside `src`;
      - a line-start `path` key (bare or quoted) under a `[lib]`, `[[bin]]`, `[[test]]`, `[[bench]]` or `[[example]]` header of `src-tauri/Cargo.toml` (spaces and quotes inside the brackets allowed). `path` in dependency tables (`[dependencies.x]`, `[target.'cfg(windows)'.dev-dependencies.x]`) and inline `{ path = … }` pass;
    - `src-tauri/benches`, `src-tauri/examples`, or `[[bench]]` / `[[example]]` in `src-tauri/Cargo.toml`;
    - in `src-tauri/Cargo.toml` (raw lines, with the same table-header tracking): no line `doctest = false` (spaces and a trailing comment allowed) in the `[lib]` table, or no `[lib]` table; any other line containing `doctest` that is not a full-line `#` comment (`true`, a quoted or dotted key, an inline `lib = { … }`, the key under another table); and `"""` or `'''` anywhere, because a multi-line string could fake a `[lib]` header for the line tracker;
    - `--doc` or `rustdoc` in a `*.yml` / `*.yaml` file of `.github/workflows` (the check's second argument). A missing workflows dir is "cannot run"; an empty one passes.

    Not caught, because a raw scan cannot see them (each is loud on the Windows job, not silent):
    - tests that a proc macro generates from an attribute with another name;
    - a dependency's macro that expands to `include!` (for example an `include_proto!`-style macro). A test it pulls in still reaches the lib test exe;
    - a target path set other than by a line-start `path` key under a target header, for example a root-level dotted `lib.path = …` or an inline `bin = [{ path = … }]`;
    - a workflow that reaches `--doc` through a script or a `.cargo` alias. A doctest exe that fails to link or start fails the step.

    A `find`, `sort`, `grep` or `awk` error is "cannot run" (exit 3), never a pass. `make check-shell-layout-fixtures` pins the shapes above: each fixture shell dir must give its documented exit code (0, 1 or 3). `ok-doc-text` holds the sources of the 25 doc-shape fixtures and the b8b0b81 probes that T-038 dropped, plus code-looking text in every doc form, and must pass.
- **Rule for shell test authors:** put shell tests in `src-tauri/tests/<name>.rs`. They need no per-file setup; the manifest reaches every one of them. Put platform-independent logic and its unit tests in `crates/voicen-core`. Keep every shell source a regular file under `src-tauri/src`, reached without `#[path]`, `include` or a Cargo target `path`. Write examples in shell doc comments as ```` ```text ````: lib doctests never run, so a ```` ```rust ```` block would only look tested. This is a style rule (T1), not checked. If a new exe kind is really needed, extend `build.rs`, this file and the check together in one reviewed change.
- **Don't:**
  - add `#[cfg(test)]` modules or test attributes in `src-tauri/src`;
  - remove `doctest = false` from `[lib]` in `src-tauri/Cargo.toml`, or add another `doctest` key;
  - pass `--doc` or run `rustdoc --test` for `voicen` (in a workflow, a script or an alias);
  - emit the manifest with `cargo:rustc-link-arg` (it reaches the bin too and duplicates its `RT_MANIFEST`) or drop the `-tests` scope;
  - edit `windows-app-manifest.xml`, or upgrade tauri-build, without comparing the copy's elements and values with the new tauri-build's file;
  - upgrade tauri-build, tauri-winres or embed-resource without checking the shell's build-script output on the Windows runner (`target/*/build/voicen-*/output`). Every bins-only line there (`cargo:rustc-link-arg-bins=…`, `cargo:rustc-link-arg-bin=voicen=…`, and any output that moved from all targets to bins) must be mirrored to `cargo:rustc-link-arg-tests` in `build.rs`, or the integration tests silently miss it again. Today the only such output is the Common-Controls manifest;
  - add `/WX` to the test link args (upstream does, for its own workspace; here it would turn any unrelated test-link warning into an error, T-030).

### A doc block never becomes a Windows test exe

- **Defect that produced it:** F-003. The guard's doc-block scan (T-035, T-036) re-implemented rustc's doc desugaring, rustdoc's unindent and CommonMark in awk, and each review round found shapes it passed.
- **What breaks if you violate it:** a doctest exe links the shell with the partial build-script configuration (the F-001/F-002 mechanism). It turns red only on the Windows job, after review.
- **Where it is enforced:** the doctest switches are pinned: `[lib] doctest = false` (step 3 of `scripts/ci/shell-test-layout.sh`) and no `--doc` / `rustdoc` in `.github/workflows` (step 4), with their fixtures. Doc text itself is never read.
- **Don't:** bring back a check that decides on how rustdoc would read doc text (see Rejected approaches).

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
| Track CommonMark list context in the guard so 4-column list continuations pass | each mis-modelled clause is a silent pass (content column W = marker width + 1..4 spaces; W+4 is code, below W closes the item; thematic breaks, ordered markers other than 1 and empty items cannot interrupt a paragraph; quotes nest the same way), against a two-line note telling the author to indent by the marker width | T-036 |
| Model paragraph state in the guard, so a 4-column line right after paragraph text passes as a lazy continuation | T-036 review 1 #1: the lazy pass was granted after lines that leave no paragraph open (`> Note` / `>`, an empty item, `---`, a setext underline, `***`), and rustdoc 1.99 ran each of them as a doctest. Third round in a row with a missed shape; replaced by the coarse rule (any 4+ column line, any marker followed by 4+ columns) | T-036 r1, decisions #39 |
| A per-block minimum for rustdoc's unindent | rustdoc unindents all doc fragments of an item together: outer docs on `mod foo;` and `//!` docs in `foo.rs`, and `#[doc]` strings mixed with `///` (one less). A per-block minimum removed more than rustdoc did, and a 4-column line passed (probe, rustdoc 1.99) | T-036 r1 |
| Model rustdoc's Markdown in awk: fences, indented blocks, list and quote containers, then rustc's and rustdoc's doc preparation under a coarse 4-column rule | every unmodelled clause of three foreign parsers is a silent pass; four consecutive steps found new shapes (T-035 cdfa9ac, T-036 b3f3cac, the #39 rule, b8b0b81). The hazard has two switches that raw text can pin instead | T-038, F-003, decisions #39, #41 |
| (a′) A raw-text fail-closed doc rule (no unindent or Markdown model) | still needs fence open/close state (an HTML block or an unclosed ```` ```text ```` turns a later bare fence into an opening one), or a ban on every fence, tab, 4+ space run, `/** */` and `#[doc]`; no need once the switches are pinned, since no doc text reaches an exe | T-038 |
| rustdoc's own parser in the core image (`rustdoc --test` on the shell) | must expand the shell crate with tauri, which needs tauri's Linux deps (webkit2gtk) the gate never builds (decisions #5); JSON doctest extraction is unstable; it would protect only doc style | T-038 option (c) |
| `embed_resource::compile_for_tests` | would need a direct build-dependency (owner consent, decisions #9); kept only as the fallback if `/MANIFESTINPUT` fails to link | T-035 fallback |

## Open

- Detection latency: shell code reaches `CODE_COMPLETE` without ever being linked on Windows (decisions #5). An optional owner decision is drafted in `docs/tasks/T-035.md` › Investigation ("exit (c)"). It is not needed for the invariants above.
