# The gate: every area's checks, each through scripts/tw-run in the area's toolchain.
.PHONY: check check-shell-layout check-shell-layout-fixtures check-helper-windows-fixtures check-core \
	check-ci-credentials \
	check-clippy-lines \
	check-core-test-os-answer check-ci-wip check-telegram-send check-telegram-failure-branch \
	check-version-stamp check-release-check check-release check-release-guard check-nsis-fork check-e2e-entry-fixtures \
	check-whisper-cmake-cache check-crt-imports \
	check-shell-windows check-ban-canary \
	check-ui core-image ui-image \
	licenses licenses-check licenses-unit licenses-fixture licenses-rust licenses-bundle \
	licenses-npm licenses-generate licenses-stale licenses-stale-guard

# check-ban-canary and check-shell-windows after check-core: both build voicen-core, so a core
# error is reported first by the faster native check-core, before the slower runs.
check: check-shell-layout check-shell-layout-fixtures check-helper-windows-fixtures check-ci-credentials \
	check-clippy-lines \
	check-core-test-os-answer check-ci-wip check-telegram-send check-telegram-failure-branch check-version-stamp check-release-check check-release check-release-guard check-nsis-fork check-whisper-cmake-cache check-crt-imports check-core \
	check-ban-canary check-shell-windows check-e2e-entry-fixtures check-ui \
	licenses-check

# Shell tests only in src-tauri/tests/*.rs (T-035, T-038, T-092, F-002; docs/decisions/ci-toolchain.md):
# cargo metadata's voicen targets (no kind-test rows) equal scripts/ci/shell-targets.txt (lib and
# bin test = false, doctest = false; no example, bench or extra target), and line 1 of the lib and
# bin crate roots is the crate-level cfg pin. cargo metadata in the core image, host python3 -I.
check-shell-layout:
	scripts/ci/shell-test-layout.sh

# Guard: the layout check must give its documented exit code on every fixture shell dir in
# scripts/ci/fixtures/shell-test-layout/ (0 allowed, 1 violation, 3 cannot run), and the pins
# must empty the crate under every cargo test selector (mechanism crate). Host bash, core image.
check-shell-layout-fixtures:
	scripts/ci/shell-test-layout.test.sh

# T-065 (rca smoke-window-predicate): the helper-window tripwire scripts/ci/helper-windows.sh
# must hold its contract on every fixture in scripts/ci/fixtures/helper-windows/ and on the
# real Windows dependency graph with scripts/ci/helper-windows.txt, and the predicate's copies
# must read that one manifest. Host bash; the real-graph case reaches the core image.
check-helper-windows-fixtures:
	scripts/ci/helper-windows.test.sh

# T-061, T-066: CI reaches Credential Manager only through scripts/ci/credentials.ps1: no other
# line of a workflow or a scripts/ci file, comments included (T-090), names a Credential Manager entry point
# (a Win32 Cred* call, cmdkey, vaultcmd, keymgr, PasswordVault, the CredentialManager module);
# the self-test runs the tripwire scripts/ci/ci-credentials.sh on its cases and on the real
# repo. Host bash.
check-ci-credentials:
	scripts/ci/ci-credentials.test.sh

# T-089 (P-020; docs/decisions/ci-toolchain.md, docs/tasks/T-089.md): the voicen-core call bans
# (a reqwest client built outside engine::http, decisions #106, #113; Instant::now / elapsed
# outside crate::clock and tests/common, T-080 I2) are rustc's path resolution: the
# disallowed-types / -methods lints, configured next to crates/voicen-core/Cargo.toml and pinned
# at deny by its [lints] table (check-core adds -D warnings).
# check-ban-canary (decides_on: switch): scripts/ci/ban-canary.sh lints the owned probe files
# (features ban-canary-lib, ban-canary-tests) in the core image and compares every diagnostic
# with scripts/ci/ban-canary.expected.txt, so each config entry and the deny level are proven.
check-ban-canary:
	scripts/ci/ban-canary.sh

# The token guard below (decides_on: token + literal): every tracked line holding the lint
# tool's name (any case, comments included) in a *.rs, Cargo.toml, .cargo config or make file
# is an owned line of the guard's list, so no allow, cap, config or edit of check-core's lint
# command can lower the bans unseen; this file's lines holding the name are owned too. The
# self-test runs the guard on trees built in a temp dir, on copies of the real repository and
# on the real repository. Host bash, git.
check-clippy-lines:
	scripts/ci/clippy-lines.test.sh

# check-core-test-os-answer (T-080 rca, class os-answer-deadline-race; T-089, decides_on: token):
# no core test outside crates/voicen-core/tests/common names an OS-answer target (.invalid,
# 127.0.0.1:1, 192.0.2.1), any case, comments included; targets come with their deadlines from
# tests/common (docs/decisions/core-tests.md, I1). The self-test runs the tripwire
# scripts/ci/core-test-os-answer.sh on scripts/ci/fixtures/core-test-os-answer/ and the real
# tests dir. Host bash.
check-core-test-os-answer:
	scripts/ci/core-test-os-answer.test.sh

# T-066 (rca, P-016, decisions #90): .github/workflows/ci.yml runs on push to main and wip/**
# (the pre-review run of every non-docs task), and no workflow step saves a cache on a ref
# other than main. Since T-088 both are release-guard rules (R7 on ci.yml, R11-R13): this name
# stays, as a prerequisite-only alias, because PRINCIPLES.md P-016 names it.
check-ci-wip: check-release-guard

# T-070 (decisions #93): scripts/ci/telegram-send.sh, the one Telegram send path of ci.yml, keeps
# the token and chat id out of curl's argv and its output, sends the caption as a form string,
# and turns every failure into one ::warning:: line with exit 0. Stub curl on PATH, offline.
# Host bash.
check-telegram-send:
	scripts/ci/telegram-send.test.sh

# T-070 (review 2 #1): scripts/ci/telegram-failure-branch.sh, the verdict of ci.yml's fake-token
# failure-branch step (off main and v* tags), is red only on the send script's contract (exit
# non-zero, no ::warning::, the fake token printed) and a warning when Telegram was not reached.
# Canned outputs, offline. Host bash.
check-telegram-failure-branch:
	scripts/ci/telegram-failure-branch.test.sh

# T-077 (decisions #98): scripts/ci/version-stamp.sh, the windows job's one version step, writes
# <repo MAJOR.MINOR>.<run number> (a vX.Y.Z tag: X.Y.Z) into Cargo.toml, tauri.conf.json,
# package.json and the two workspace members of Cargo.lock, no other line, and exports
# VOICEN_VERSION to GITHUB_ENV; a malformed tag, repo version, disagreeing files or a bad run
# number exit 1 naming the value and write nothing. Fixture trees and a copy of the real files,
# offline. Host bash, diff, awk.
check-version-stamp:
	scripts/ci/version-stamp.test.sh

# T-026 (decisions #102, #103): "release job version check". scripts/ci/release-check.sh, the one
# release verdict of ci.yml (release job and the windows job's premise / failure branch), accepts
# a tag vX.Y.Z only when X.Y is the committed MAJOR.MINOR, Z >= the run number, exactly one
# installer carries _X.Y.Z_ and (with GH_TOKEN) no release exists; every refusal exits 1 naming
# both values, and it never calls a gh write subcommand. Stub gh on PATH, offline. Host bash.
check-release-check:
	scripts/ci/release-check.test.sh

# T-085 (rca, decisions #120): scripts/ci/release.sh, the release job's one step, refuses before
# any gh call without GH_TOKEN, runs release-check.sh and, only on its pass, writes
# SHA256SUMS.txt and the notes and calls gh release create --verify-tag exactly once; a refusal
# writes nothing and a failed create is not a pass. Stub gh on PATH, offline. Host bash.
check-release:
	scripts/ci/release.test.sh

# T-026, T-085 (rca, decisions #120, #121, #122, P-020), T-084 (#119), T-087 (#124): raw-text
# rules R1-R10 over every file in .github/workflows: the workflow-level permissions: contents: read block (R1), the release job
# to the end of its file byte-identical to the guard's owned copy (R2), permissions only there
# (R3), no secret but the two Telegram ones and no vars (R4), the word gh only inside the release
# job (R5), no YAML escape that spells a character or joins two lines (R6), the header of the
# file holding the release job, line 1 to jobs:, byte-identical to scripts/ci/release-header.yml
# (R7; decisions #121), no YAML tag: the raw texts !!, !< and %TAG nowhere (R8; decisions
# #122), the one installer upload byte-identical to scripts/ci/release-upload.yml (R9), and the
# raw token secrets.TELEGRAM_ (any case, comments included) only in the one Telegram send step,
# byte-identical to scripts/ci/telegram-send-step.yml (R10; replaces telegram-guard, T-087:
# T-070 validation 1 M6 and probes T1-T4 are its fixtures); and (T-088, replaces ci-wip and
# setup-node-cache) R7 also on ci.yml (its trigger: push to main and wip/**), every line with the
# word uses or an action name (actions/cache, setup-node, rust-cache, cache/save,
# pnpm/action-setup) an owned line of scripts/ci/action-lines.txt or inside a byte-exact owned
# step fragment of scripts/ci/cache-steps/ (whose own text the guard checks: saves from main
# only, no setup-node cache) (R11), no column-0 line after jobs: (R12) and no YAML merge key
# (R13). The self-test runs the tripwire scripts/ci/release-guard.sh on
# scripts/ci/fixtures/release-guard/ and the real workflows dir (also mutated). Host bash and awk.
check-release-guard:
	scripts/ci/release-guard.test.sh

# T-025 (decisions #76): the NSIS template is a minimal-diff fork of the one in the pinned
# tauri-cli, so scripts/ci/nsis-fork.sh refuses a tree where src-tauri/windows/installer.nsi's
# '; upstream: tauri-cli X.Y.Z' header, the exact @tauri-apps/cli in package.json and the
# version of the installed node_modules/@tauri-apps/cli/package.json differ, or tauri.conf.json
# does not use the fork; pnpm-lock.yaml is not read (T-093, P-020: the specifier equality is
# pnpm --frozen-lockfile's). No installed CLI: cannot run. The self-test runs it on fixture trees
# built in a temp dir and on the real repo. Host bash, python3 (stdlib json).
check-nsis-fork:
	scripts/ci/nsis-fork.test.sh

# T-064: the ui e2e entry point (package.json scripts.e2e) runs Playwright only in the ui image
# at the lockfile's @playwright/test version, with --network none and the caller's CI; a
# missing or mismatched image stops it before any test, naming the image. Fixture cases with a
# fake docker and fake Playwright CLIs. Host bash, python3, node.
check-e2e-entry-fixtures:
	scripts/ci/e2e-entry.test.sh

# T-099 (decisions #142, invariant 3): scripts/ci/whisper-cmake-cache.sh, the windows job's
# verdict on every whisper-rs-sys CMakeCache.txt under target/ (decides_on: token + literal),
# passes only when each holds GGML_NATIVE:BOOL=OFF, GGML_AVX2:BOOL=ON, GGML_AVX512:BOOL=OFF and
# BUILD_SHARED_LIBS:BOOL=OFF exactly once and every line naming one of those options or
# GGML_BACKEND_DL / GGML_CPU_ALL_VARIANTS is an owned line; no cache found is red. Fixture trees,
# offline. Host bash.
check-whisper-cmake-cache:
	scripts/ci/whisper-cmake-cache.test.sh

# T-099 (invariant 4, F-001 class): scripts/ci/crt-imports.sh, the windows job's verdict on the
# C runtime of every exe linking whisper.cpp (decides_on: token), refuses an exe whose dumpbin
# -dependents output holds MSVCP140 or VCRUNTIME140, a missing exe and an empty or failed dump.
# Stub dumpbin, offline. Host bash.
check-crt-imports:
	scripts/ci/crt-imports.test.sh

check-core:
	scripts/tw-run core -- 'cargo fmt --check -p voicen-core && cargo clippy -p voicen-core --all-targets -- -D warnings'
	scripts/tw-run core -- cargo test -p voicen-core

# Windows-target type check of the shell on Linux (T-056, decisions #63;
# docs/decisions/ci-toolchain.md). First the lib and bin with the release feature set in the
# dev profile (no dev-dependency features, tauri without custom-protocol: not what
# `pnpm tauri build` compiles, so its production context and cfg(not(dev)) /
# cfg(not(debug_assertions)) code stay the windows job's; positive cfg(dev) /
# cfg(debug_assertions) code is checked here), then with --tests every src-tauri/tests/*.rs, the
# lib (and voicen-core) checked as their dependency with the dev-dependency features; lib and bin
# have test = false, so there are no unit-test crates (T-092). --tests alone would check the lib
# only with test-fakes on and pass a release-only error, and not check the bin. cargo check never links and builds no doctest;
# linking, starting and running the shell stay the windows job's. Needs the gnu target and
# mingw gcc baked into the core image (docker/rust.Dockerfile; `make core-image`).
check-shell-windows:
	scripts/tw-run core -- 'cargo check -p voicen --target x86_64-pc-windows-gnu && cargo check -p voicen --target x86_64-pc-windows-gnu --tests'

# The e2e step goes through the e2e entry (package.json scripts.e2e) on the host: it checks the
# ui image is present and re-enters it through scripts/tw-run ui with --network none (T-064).
check-ui:
	scripts/tw-run ui -- pnpm lint
	scripts/tw-run ui -- pnpm test
	scripts/e2e.sh

# Toolchain image of the `core` area (CI builds the same one).
core-image:
	docker build -t voicen-rust:1.99 -f docker/rust.Dockerfile docker

# Toolchain image of the `ui` area: the official Playwright image at the lockfile's
# @playwright/test version plus pnpm (docker/ui.Dockerfile; CI builds the same one, T-064).
ui-image:
	docker build -t voicen-ui:1.63.0 -f docker/ui.Dockerfile docker

# --- Licenses (T-027, decisions #9 #24) -------------------------------------------------
# One accepted list: about.toml `accepted`. Rust: cargo-about 0.9.2 (Windows target only).
# npm: the packages of the client bundle (target/licenses/npm-bundled.json, written by the
# Vite build) plus licenses/manual.json, checked by scripts/licenses/check.mjs.
# Needs crates.io access; without it the check says it cannot run (exit 3), not a license
# failure (decision #24 D3).
LICENSES_DIR := target/licenses
# The committed notices file. `:=`, not `?=`: an environment variable cannot redirect the
# check; only a command-line override can (licenses-stale-guard, on its fixtures).
NOTICES := THIRD-PARTY-NOTICES.txt

licenses-check: licenses-unit licenses-fixture licenses-stale-guard licenses-rust licenses-npm licenses-stale

# Unit tests of the npm/manual checker. The files are named: since Node 22 (the ui image has
# Node 24) `node --test <dir>` runs the directory as a module instead of finding its tests.
licenses-unit:
	scripts/tw-run ui -- 'node --test scripts/licenses/*.test.mjs'

# Guard: cargo-about with the real about.toml must reject licenses/fixtures/gpl.
licenses-fixture:
	scripts/licenses/fixture-check.sh

# T-063 guard: the real licenses-stale recipe, run without the generation on
# licenses/fixtures/stale/<case>/ (LICENSES_DIR and NOTICES overridden), must pass equal notices
# and fail stale and missing ones naming the file. Offline, no docker. Host bash, make, diff.
licenses-stale-guard:
	scripts/licenses/stale-check.sh

# Rust crates of the workspace, Windows target, against about.toml; writes rust.txt.
licenses-rust:
	scripts/licenses/rust.sh

# Client build; its Vite plugin writes $(LICENSES_DIR)/npm-bundled.json. The previous list
# is deleted first, so a build that records nothing cannot pass on a stale list.
licenses-bundle:
	scripts/tw-run ui -- 'rm -f $(LICENSES_DIR)/npm-bundled.json && pnpm build'

# npm packages of the client bundle and the hand-kept list, against about.toml.
# --require svelte: the bundle list must hold the Svelte runtime, or the plugin saw nothing.
licenses-npm: licenses-bundle
	scripts/tw-run ui -- node scripts/licenses/check.mjs --about about.toml \
		--bundle $(LICENSES_DIR)/npm-bundled.json --manual licenses/manual.json --require svelte

# Notices generated into target/licenses (both targets below use it). cargo-about renders
# the Rust part only when every crate is accepted; npm and manual entries are listed
# whatever their license (licenses-npm judges them), so a notice is never dropped.
licenses-generate: licenses-rust licenses-bundle
	scripts/tw-run ui -- node scripts/licenses/notices.mjs --rust $(LICENSES_DIR)/rust.txt \
		--bundle $(LICENSES_DIR)/npm-bundled.json --manual licenses/manual.json \
		--out $(LICENSES_DIR)/THIRD-PARTY-NOTICES.txt

# The committed notices must equal a fresh generation (guarded by licenses-stale-guard).
licenses-stale: licenses-generate
	@test -f $(NOTICES) || { echo "licenses-check: $(NOTICES) is not committed; run make licenses and commit it" >&2; exit 1; }
	@diff -u $(NOTICES) $(LICENSES_DIR)/THIRD-PARTY-NOTICES.txt || { echo "licenses-check: $(NOTICES) is stale; run make licenses and commit it" >&2; exit 1; }

# Regenerate the committed notices.
licenses: licenses-generate
	cp $(LICENSES_DIR)/THIRD-PARTY-NOTICES.txt $(NOTICES)
