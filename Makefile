# The gate: every area's checks, each through scripts/tw-run in the area's toolchain.
.PHONY: check check-shell-layout check-shell-layout-fixtures check-helper-windows-fixtures check-core \
	check-ci-credentials check-setup-node-cache check-ci-wip check-telegram-send check-telegram-failure-branch check-telegram-guard \
	check-version-stamp check-nsis-fork check-e2e-entry-fixtures \
	check-shell-windows \
	check-ui core-image ui-image \
	licenses licenses-check licenses-unit licenses-fixture licenses-rust licenses-bundle \
	licenses-npm licenses-generate licenses-stale licenses-stale-guard

# check-shell-windows after check-core: the shell depends on voicen-core, so a core error is
# reported first by the faster native check-core, before the slower cross-target build.
check: check-shell-layout check-shell-layout-fixtures check-helper-windows-fixtures check-ci-credentials check-setup-node-cache check-ci-wip check-telegram-send check-telegram-failure-branch check-telegram-guard check-version-stamp check-nsis-fork check-core \
	check-shell-windows check-e2e-entry-fixtures check-ui \
	licenses-check

# Shell tests only in src-tauri/tests/*.rs: no test attributes in src-tauri/src, no benches or
# examples, [lib] doctest = false kept and no --doc / rustdoc in .github/workflows (T-035, T-038,
# F-002; docs/decisions/ci-toolchain.md). Host bash/grep and an awk lexer
# (scripts/ci/shell-test-layout.awk).
check-shell-layout:
	scripts/ci/shell-test-layout.sh

# Guard: the layout check must give its documented exit code on every fixture shell dir in
# scripts/ci/fixtures/shell-test-layout/ (0 allowed, 1 violation, 3 cannot run). Host bash.
check-shell-layout-fixtures:
	scripts/ci/shell-test-layout.test.sh

# T-065 (rca smoke-window-predicate): the helper-window tripwire scripts/ci/helper-windows.sh
# must hold its contract on every fixture in scripts/ci/fixtures/helper-windows/ and on the
# real Windows dependency graph with scripts/ci/helper-windows.txt, and the predicate's copies
# must read that one manifest. Host bash; the real-graph case reaches the core image.
check-helper-windows-fixtures:
	scripts/ci/helper-windows.test.sh

# T-061, T-066: CI reaches Credential Manager only through scripts/ci/credentials.ps1: no other
# non-comment line of a workflow or a scripts/ci file names a Credential Manager entry point
# (a Win32 Cred* call, cmdkey, vaultcmd, keymgr, PasswordVault, the CredentialManager module);
# the self-test runs the tripwire scripts/ci/ci-credentials.sh on its cases and on the real
# repo. Host bash.
check-ci-credentials:
	scripts/ci/ci-credentials.test.sh

# T-068: no actions/setup-node step in .github/workflows needs pnpm on the host unless an
# earlier step of its job runs pnpm/action-setup (setup-node v5 caches the package.json
# packageManager by default; run 37498788064). The self-test runs the tripwire
# scripts/ci/setup-node-cache.sh on scripts/ci/fixtures/setup-node-cache/ and the real repo.
# Host bash and awk.
check-setup-node-cache:
	scripts/ci/setup-node-cache.test.sh

# T-066 (rca, P-016, decisions #90): .github/workflows/ci.yml runs on push to main and wip/**
# (the pre-review run of every non-docs task), and no workflow step saves a cache on a ref
# other than main (fail-closed classification of every uses:). The self-test runs the tripwire
# scripts/ci/ci-wip.sh on scripts/ci/fixtures/ci-wip/ and the real repo. Host bash and awk.
check-ci-wip:
	scripts/ci/ci-wip.test.sh

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

# T-070 (validation 1, M6): every ci.yml step that reads secrets.TELEGRAM_BOT_TOKEN or
# secrets.TELEGRAM_CHAT_ID has an if: with github.event_name == 'push' and (main || v* tag) as
# top-level && operands, no status function and no continue-on-error. The self-test runs the
# tripwire scripts/ci/telegram-guard.sh on scripts/ci/fixtures/telegram-guard/ and the real
# ci.yml (also with M6 applied). Host bash and awk.
check-telegram-guard:
	scripts/ci/telegram-guard.test.sh

# T-077 (decisions #98): scripts/ci/version-stamp.sh, the windows job's one version step, writes
# <repo MAJOR.MINOR>.<run number> (a vX.Y.Z tag: X.Y.Z) into Cargo.toml, tauri.conf.json,
# package.json and the two workspace members of Cargo.lock, no other line, and exports
# VOICEN_VERSION to GITHUB_ENV; a malformed tag, repo version, disagreeing files or a bad run
# number exit 1 naming the value and write nothing. Fixture trees and a copy of the real files,
# offline. Host bash, diff, awk.
check-version-stamp:
	scripts/ci/version-stamp.test.sh

# T-025 (decisions #76): the NSIS template is a minimal-diff fork of the one in the pinned
# tauri-cli, so scripts/ci/nsis-fork.sh refuses a tree where src-tauri/windows/installer.nsi's
# '; upstream: tauri-cli X.Y.Z' header, the exact @tauri-apps/cli in package.json and the
# lockfile's root @tauri-apps/cli version differ, or tauri.conf.json does not use the fork. The
# self-test runs it on fixture trees built in a temp dir and on the real repo. Host bash, awk,
# python3.
check-nsis-fork:
	scripts/ci/nsis-fork.test.sh

# T-064: the ui e2e entry point (package.json scripts.e2e) runs Playwright only in the ui image
# at the lockfile's @playwright/test version, with --network none and the caller's CI; a
# missing or mismatched image stops it before any test, naming the image. Fixture cases with a
# fake docker and fake Playwright CLIs. Host bash, python3, node.
check-e2e-entry-fixtures:
	scripts/ci/e2e-entry.test.sh

check-core:
	scripts/tw-run core -- 'cargo fmt --check -p voicen-core && cargo clippy -p voicen-core --all-targets -- -D warnings'
	scripts/tw-run core -- cargo test -p voicen-core

# Windows-target type check of the shell on Linux (T-056, decisions #63;
# docs/decisions/ci-toolchain.md). First the lib and bin with the release feature set in the
# dev profile (no dev-dependency features, tauri without custom-protocol: not what
# `pnpm tauri build` compiles, so its production context and cfg(not(dev)) /
# cfg(not(debug_assertions)) code stay the windows job's; positive cfg(dev) /
# cfg(debug_assertions) code is checked here), then the test crates (every
# src-tauri/tests/*.rs and the lib/bin unit-test crates) with the dev-dependency features: --tests alone would check the lib with
# test-fakes on and pass a release-only error. cargo check never links and builds no doctest;
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

# The committed notices must equal a fresh generation.
licenses-stale: licenses-generate
	@test -f THIRD-PARTY-NOTICES.txt || { echo "licenses-check: THIRD-PARTY-NOTICES.txt is not committed; run make licenses and commit it" >&2; exit 1; }
	@diff -u THIRD-PARTY-NOTICES.txt $(LICENSES_DIR)/THIRD-PARTY-NOTICES.txt || { echo "licenses-check: THIRD-PARTY-NOTICES.txt is stale; run make licenses and commit it" >&2; exit 1; }

# Regenerate the committed notices.
licenses: licenses-generate
	cp $(LICENSES_DIR)/THIRD-PARTY-NOTICES.txt THIRD-PARTY-NOTICES.txt
