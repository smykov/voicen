# The gate: every area's checks, each through scripts/tw-run in the area's toolchain.
.PHONY: check check-core check-ui core-image

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
