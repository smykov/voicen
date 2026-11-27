#!/usr/bin/env bash
# T-027 guard: cargo-about, run with the REAL about.toml on the fixture workspace
# licenses/fixtures/gpl/ (a GPL-3.0-only crate and a crate without a license), must
# FAIL and reject both crates as errors. If it passes, the Rust license check is toothless.
#
# Required, all three (cargo-about 0.9.2 output forms) for the GPL fixture:
#   1. a non-zero exit;
#   2. "error: failed to satisfy license requirements" whose diagnostic points at
#      gpl-fixture/Cargo.toml and its "GPL-3.0-only" (the GPL crate is named only by path);
#   3. a line starting "error: unable to synthesize license expression for 'nolicense-fixture"
#      -- without --fail cargo-about prints the same text as a [WARN] only, which must
#      NOT satisfy the guard (a mere mention of the crate name is not enough).
# And (T-181) cargo-about with about.toml plus a later top-level `accepted = ["MIT"]`
# (target/licenses/about-dup.toml) must exit non-zero saying "duplicate key".
#
# Exit 0: all hold. Exit 1: any is missing (guard broken).
# Exit 3: cannot run (cargo-about missing from the core image, docker missing, no network);
#         never reported as a license result.
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
# Plain text for matching: strip ANSI colour codes.
plain="$out/fixture-gpl.plain.log"
sed 's/\x1b\[[0-9;]*m//g' "$log" >"$plain"
missing=""
# 2. the GPL crate as a license-requirement failure: the error line followed (within its
#    diagnostic) by the fixture's manifest path and its license expression.
if ! awk '
  /^error: failed to satisfy license requirements/ { open = 1; path = 0; expr = 0; n = 0; next }
  open { n++
         if (index($0, "gpl-fixture/Cargo.toml")) path = 1
         if (index($0, "GPL-3.0-only")) expr = 1
         if (path && expr) { found = 1; open = 0 }
         if (n >= 8 || /^error:/) open = 0 }
  END { exit found ? 0 : 1 }' "$plain"; then
  missing="$missing gpl-fixture(license-requirement error)"
fi
# 3. the unlicensed crate as an error, not a warning.
if ! grep -Eq "^error: unable to synthesize license expression for 'nolicense-fixture[ ']" "$plain"; then
  missing="$missing nolicense-fixture(synthesize error)"
fi
if [ -n "$missing" ]; then
  echo "licenses-check: FAIL: cargo-about exited $status on licenses/fixtures/gpl but did not report as errors:$missing" >&2
  cat "$plain" >&2
  exit 1
fi
# 4. (T-181, I-5) cargo-about refuses a later top-level `accepted` (duplicate key), which
#    accepted.mjs relies on: the real about.toml, byte for byte, plus `accepted = ["MIT"]`,
#    on a temp copy (about.toml itself is never edited). The config error comes before any
#    download; a network failure is still "cannot run".
dup="$out/about-dup.toml"
duplog="$out/fixture-dup.log"
{ cat about.toml && printf '%s\n' 'accepted = ["MIT"]'; } >"$dup" \
  || { echo "licenses-check: cannot run: could not write $dup" >&2; exit 3; }
scripts/tw-run core -- cargo about generate --fail \
  --manifest-path licenses/fixtures/gpl/Cargo.toml -c "$dup" \
  -o "$out/fixture-dup.txt" about.hbs >"$duplog" 2>&1
dupstatus=$?
if [ "$dupstatus" -eq 0 ]; then
  echo "licenses-check: FAIL: cargo-about accepted $dup (about.toml plus a later top-level accepted); the duplicate key refusal accepted.mjs relies on is gone (output in $duplog)" >&2
  exit 1
fi
if grep -Eqi 'failed to download|could not resolve host|network|spurious' "$duplog"; then
  echo "licenses-check: cannot run: no network for cargo-about (output in $duplog)" >&2
  exit 3
fi
dupplain="$out/fixture-dup.plain.log"
sed 's/\x1b\[[0-9;]*m//g' "$duplog" >"$dupplain"
if ! grep -qi 'duplicate key' "$dupplain"; then
  echo "licenses-check: FAIL: cargo-about exited $dupstatus on $dup but did not say duplicate key (output follows):" >&2
  cat "$dupplain" >&2
  exit 1
fi
echo "licenses-check: fixture guard ok: cargo-about rejects gpl-fixture (GPL-3.0-only) and nolicense-fixture (no license) as errors, and a later top-level accepted (duplicate key)"
