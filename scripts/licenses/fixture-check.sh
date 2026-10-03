#!/usr/bin/env bash
# T-027 guard: cargo-about, run with the REAL about.toml on the fixture workspace
# licenses/fixtures/gpl/ (a GPL-3.0-only crate and a crate without a license), must
# FAIL and name both crates. If it passes, the Rust license check is toothless.
#
# Exit 0: the check failed as required and named both crates.
# Exit 1: the check passed, or failed without naming both crates (guard broken).
# Exit 3: cannot run (cargo-about missing from the core image, docker missing, no network);
#         never reported as a license result.
#
# The cargo-about flags below (`generate`, `--manifest-path`, `-c`, `--fail`, `-o`) must be
# checked against the version pinned in docker/rust.Dockerfile (developer, T-027).
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
out=target/licenses
mkdir -p "$out"
log="$out/fixture-gpl.log"

if ! scripts/tw-run core -- cargo about --version >"$log" 2>&1; then
  echo "licenses-check: cannot run: cargo-about is not available in the core image (output in $log):" >&2
  cat "$log" >&2
  exit 3
fi

scripts/tw-run core -- cargo about generate --fail \
  --manifest-path licenses/fixtures/gpl/Cargo.toml -c about.toml \
  -o "$out/fixture-gpl.txt" about.hbs >"$log" 2>&1
status=$?

if [ "$status" -eq 0 ]; then
  echo "licenses-check: FAIL: cargo-about accepted licenses/fixtures/gpl (GPL-3.0-only and unlicensed crates); the Rust check is toothless" >&2
  exit 1
fi
if grep -Eqi 'failed to download|could not resolve host|network|spurious' "$log"; then
  echo "licenses-check: cannot run: no network for cargo-about (output in $log)" >&2
  exit 3
fi
missing=""
for crate in gpl-fixture nolicense-fixture; do
  grep -q "$crate" "$log" || missing="$missing $crate"
done
if [ -n "$missing" ]; then
  echo "licenses-check: FAIL: cargo-about failed on licenses/fixtures/gpl (exit $status) without naming:$missing" >&2
  cat "$log" >&2
  exit 1
fi
echo "licenses-check: fixture guard ok: cargo-about rejects gpl-fixture and nolicense-fixture"
