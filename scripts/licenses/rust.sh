#!/usr/bin/env bash
# T-027: the Rust half of the license check. cargo-about (pinned in docker/rust.Dockerfile)
# checks every crate of the workspace's Windows-target normal dependency graph against
# about.toml (`accepted`, `targets`, `ignore-*-dependencies`) and renders about.hbs into
# target/licenses/rust.txt, the Rust part of THIRD-PARTY-NOTICES.txt.
#
# Two steps, so a missing network is never reported as a license failure (decision #24 D3):
#   1. `cargo fetch --locked` downloads the crate sources (crates.io; cached by tw-run in
#      .teamwright/cache/core). Failure = cannot run.
#   2. `cargo about generate --frozen` reads only local files (no crates.io, no git hosts),
#      so its result depends on Cargo.lock alone. `--fail` makes a crate without a license
#      an error; an unaccepted license is an error anyway.
#
# Exit 0: all crates accepted, rust.txt written. Exit 1: license failure; cargo-about's
# output names each crate (its Cargo.toml path and license). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
out=target/licenses
mkdir -p "$out"
log="$out/rust.log"

if ! scripts/tw-run core -- cargo about --version >"$log" 2>&1; then
  echo "licenses-check: cannot run: cargo-about is not available in the core image (run make core-image); output:" >&2
  cat "$log" >&2
  exit 3
fi

if ! scripts/tw-run core -- cargo fetch --locked >"$log" 2>&1; then
  echo "licenses-check: cannot run: cargo fetch failed, so the crate sources are not available (no network to crates.io, or Cargo.lock is out of date); this is not a license result. Output:" >&2
  cat "$log" >&2
  exit 3
fi

scripts/tw-run core -- cargo about generate --frozen --fail --workspace \
  -c about.toml -o "$out/rust.txt" about.hbs >"$log" 2>&1
status=$?
if [ "$status" -ne 0 ]; then
  echo "licenses-check: FAIL: Rust crates outside the accepted list of about.toml or without a license (cargo-about exit $status); an exception is an owner decision (docs/decisions.md), never an about.toml edit alone:" >&2
  cat "$log" >&2
  exit 1
fi
echo "licenses-check: Rust crates ok (Windows target, cargo-about)"
