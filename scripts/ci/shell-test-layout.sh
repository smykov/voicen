#!/usr/bin/env bash
# T-035 guard (class ci-toolchain, F-001/F-002; docs/decisions/ci-toolchain.md).
# src-tauri/build.rs gives the Windows link configuration of the release exe to the bin and,
# through rustc-link-arg-tests, to every src-tauri/tests/*.rs integration test. No cargo
# selector reaches the other test-exe kinds of the voicen package without also reaching the
# bin, so those kinds must contain no tests. This check fails when any of them could:
#   1. a test attribute under src-tauri/src: #[test], #[<path>::test], #[test_case(..)] (any
#      attribute whose name starts with `test`), or a cfg/cfg_attr
#      predicate naming `test` (#[cfg(test)], #![cfg(test)], #[cfg(all(test, ...))]):
#      lib and bin unit-test exes;
#   2. a doc-comment (/// or //!) code fence under src-tauri/src other than ```text, or a
#      #[doc = include_str!(...)]: lib doctests;
#   3. src-tauri/benches, src-tauri/examples, or [[bench]] / [[example]] in
#      src-tauri/Cargo.toml: bench and example exes.
# Plain grep and awk on the host; no toolchain.
#
# Usage: scripts/ci/shell-test-layout.sh [shell-dir]   (default: src-tauri)
# Exit 0: layout ok. Exit 1: a violation (listed). Exit 3: cannot run (shell dir missing,
# grep or awk error); never reported as a pass.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
shell="${1:-src-tauri}"
if [ ! -d "$shell/src" ] || [ ! -f "$shell/Cargo.toml" ]; then
  echo "shell-test-layout: cannot run: $shell/src or $shell/Cargo.toml not found" >&2
  exit 3
fi

# Each scan fails closed: a grep or awk error is "cannot run" (exit 3), never a pass.
cannot_run() { echo "shell-test-layout: cannot run: $1" >&2; exit 3; }
found=""
# add <note> <hits...>: one listed line per non-empty hit.
add() {
  local note="$1" hit
  while IFS= read -r hit; do
    [ -n "$hit" ] && found="$found  $hit  ($note)"$'\n'
  done <<<"$2"
}

# 1. Test attributes and cfg(test) predicates. `test` must be a predicate token, so
#    cfg(feature = "test") does not match. grep: 0 = hits, 1 = none, 2 = error.
attr_test='^[[:space:]]*#!?\[[[:space:]]*([A-Za-z_][A-Za-z0-9_]*::)*test[A-Za-z0-9_]*[[:space:]]*[](]'
cfg_test='^[[:space:]]*#!?\[[[:space:]]*cfg(_attr)?[[:space:]]*\((.*[(,[:space:]])?test[[:space:]]*[),]'
hits="$(grep -rnE --include='*.rs' -e "$attr_test" -e "$cfg_test" "$shell/src")"
[ $? -le 1 ] || cannot_run "grep failed on $shell/src"
add "test attribute: a unit test in the lib or bin test exe" "$hits"

# 2. Doc-comment code fences other than ```text, and docs pulled in from files.
mapfile -d '' files < <(find "$shell/src" -name '*.rs' -type f -print0 | sort -z)
if [ "${#files[@]}" -gt 0 ]; then
  hits="$(awk '
    FNR == 1 { infence = 0 }
    {
      if ($0 ~ /^[[:space:]]*\/\/(\/|!)/ && $0 !~ /^[[:space:]]*\/\/\/\//) {
        doc = $0
        sub(/^[[:space:]]*\/\/(\/|!)[[:space:]]?/, "", doc)
        if (doc ~ /^[[:space:]]*(```+|~~~+)/) {
          if (infence) { infence = 0; next }
          infence = 1
          info = doc
          sub(/^[[:space:]]*(```+|~~~+)[[:space:]]*/, "", info)
          sub(/[[:space:]]+$/, "", info)
          if (info != "text")
            printf "%s:%d:%s  (doc-comment code fence: a doctest; only ```text is allowed)\n", FILENAME, FNR, $0
        }
        next
      }
      infence = 0
      if ($0 ~ /#!?\[[[:space:]]*doc[[:space:]]*=[[:space:]]*include_str!/)
        printf "%s:%d:%s  (docs from a file can hold doctests)\n", FILENAME, FNR, $0
    }' "${files[@]}")" || cannot_run "awk failed on $shell/src"
  while IFS= read -r hit; do
    [ -n "$hit" ] && found="$found  $hit"$'\n'
  done <<<"$hits"
fi

# 3. Bench and example targets.
for d in benches examples; do
  [ -e "$shell/$d" ] && add "bench or example exes are not configured by build.rs" "$shell/$d/"
done
hits="$(grep -nE '^[[:space:]]*\[\[[[:space:]]*(bench|example)[[:space:]]*\]\]' "$shell/Cargo.toml")"
[ $? -le 1 ] || cannot_run "grep failed on $shell/Cargo.toml"
add "a bench or example target: its exe is not configured by build.rs" "$(sed "s|^|$shell/Cargo.toml:|" <<<"$hits" | grep -v ':$')"

if [ -n "$found" ]; then
  {
    echo "shell-test-layout: FAIL: shell tests belong only in $shell/tests/*.rs (or, for platform-independent logic, in crates/voicen-core)."
    echo "Only integration tests get the release exe's Windows link configuration from $shell/build.rs;"
    echo "any other test exe of the voicen package links or starts only partly on Windows CI (F-001, F-002)."
    echo "Why and what to do instead: docs/decisions/ci-toolchain.md"
    printf '%s' "$found"
  } >&2
  exit 1
fi
echo "shell-test-layout: ok: no tests in $shell/src (no test attributes, no doctests), no benches or examples"
