# The gate: every area's checks, each through scripts/tw-run in the area's toolchain.
.PHONY: check check-core check-ui core-image \
	licenses licenses-check licenses-unit licenses-fixture licenses-rust licenses-npm \
	licenses-generate licenses-stale

check: check-core check-ui

check-core:
	scripts/tw-run core -- 'cargo fmt --check -p voicen-core && cargo clippy -p voicen-core --all-targets -- -D warnings'
	scripts/tw-run core -- cargo test -p voicen-core

check-ui:
	scripts/tw-run ui -- pnpm lint
	scripts/tw-run ui -- pnpm test
	scripts/tw-run ui -- pnpm e2e

# Toolchain image of the `core` area (CI builds the same one).
core-image:
	docker build -t voicen-rust:1.99 -f docker/rust.Dockerfile docker

# --- Licenses (T-027, decisions #9 #24) -------------------------------------------------
# One accepted list: about.toml `accepted`. Rust: cargo-about (Windows target only).
# npm: the packages of the client bundle (target/licenses/npm-bundled.json, written by the
# Vite build) plus licenses/manual.json, checked by scripts/licenses/check.mjs.
# Needs crates.io access; without it the check must say it cannot run (decision #24 D3).
LICENSES_DIR := target/licenses

# wired into `check` by the T-027 implementation (red until then)
licenses-check: licenses-unit licenses-fixture licenses-stale

# Unit tests of the npm/manual checker.
licenses-unit:
	scripts/tw-run ui -- node --test scripts/licenses/

# Guard: cargo-about with the real about.toml must reject licenses/fixtures/gpl.
licenses-fixture:
	scripts/licenses/fixture-check.sh

# Rust crates of the workspace, Windows target, against about.toml.
licenses-rust:
	mkdir -p $(LICENSES_DIR)
	scripts/tw-run core -- cargo about generate --fail -c about.toml -o $(LICENSES_DIR)/rust.txt about.hbs

# npm packages of the client bundle and the hand-kept list, against about.toml.
licenses-npm:
	scripts/tw-run ui -- pnpm build
	scripts/tw-run ui -- node scripts/licenses/check.mjs --about about.toml \
		--bundle $(LICENSES_DIR)/npm-bundled.json --manual licenses/manual.json

# Notices generated into target/licenses (both targets below use it).
licenses-generate: licenses-rust licenses-npm
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
