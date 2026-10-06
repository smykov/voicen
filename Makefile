# The gate: every area's checks, each through scripts/tw-run in the area's toolchain.
.PHONY: check check-shell-layout check-shell-layout-fixtures check-helper-windows-fixtures check-core \
	check-ci-credentials \
	check-shell-windows \
	check-ui core-image \
	licenses licenses-check licenses-unit licenses-fixture licenses-rust licenses-bundle \
	licenses-npm licenses-generate licenses-stale

# check-shell-windows after check-core: the shell depends on voicen-core, so a core error is
# reported first by the faster native check-core, before the slower cross-target build.
check: check-shell-layout check-shell-layout-fixtures check-helper-windows-fixtures check-ci-credentials check-core \
	check-shell-windows check-ui \
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

# T-061: CI steps plant and observe Credential Manager entries only through
# scripts/ci/credentials.ps1 (no cmdkey in the workflows or scripts/ci/*.ps1); the self-test
# runs the tripwire scripts/ci/ci-credentials.sh on its cases and on the real repo. Host bash.
check-ci-credentials:
	scripts/ci/ci-credentials.test.sh

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

check-ui:
	scripts/tw-run ui -- pnpm lint
	scripts/tw-run ui -- pnpm test
	scripts/tw-run ui -- pnpm e2e

# Toolchain image of the `core` area (CI builds the same one).
core-image:
	docker build -t voicen-rust:1.99 -f docker/rust.Dockerfile docker

# --- Licenses (T-027, decisions #9 #24) -------------------------------------------------
# One accepted list: about.toml `accepted`. Rust: cargo-about 0.9.2 (Windows target only).
# npm: the packages of the client bundle (target/licenses/npm-bundled.json, written by the
# Vite build) plus licenses/manual.json, checked by scripts/licenses/check.mjs.
# Needs crates.io access; without it the check says it cannot run (exit 3), not a license
# failure (decision #24 D3).
LICENSES_DIR := target/licenses

licenses-check: licenses-unit licenses-fixture licenses-rust licenses-npm licenses-stale

# Unit tests of the npm/manual checker.
licenses-unit:
	scripts/tw-run ui -- node --test scripts/licenses/

# Guard: cargo-about with the real about.toml must reject licenses/fixtures/gpl.
licenses-fixture:
	scripts/licenses/fixture-check.sh

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
