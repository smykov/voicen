#!/usr/bin/env bash
# T-035 guard (class ci-toolchain, F-001/F-002; docs/decisions/ci-toolchain.md).
# src-tauri/build.rs gives the Windows link configuration of the release exe to the bin and,
# through rustc-link-arg-tests, to every src-tauri/tests/*.rs integration test. No cargo
# selector reaches the other test-exe kinds of the voicen package without also reaching the
# bin, so those kinds must contain no tests. Lib doctests are off (`[lib] doctest = false` in
# src-tauri/Cargo.toml); a doc code block would then be skipped silently instead of run, so
# it is refused too. This check fails when any of the following is present:
#   1. under src-tauri/src, read by a lexer (scripts/ci/shell-test-layout.awk) that drops
#      plain comments and string/char literals, so text inside them never counts:
#      - a test attribute anywhere on a line, with any whitespace or line breaks inside it:
#        #[test], # [test], #[<path>::test], #[test_case(..)] (any attribute whose name
#        starts with `test`), also as an attribute argument of cfg_attr;
#      - a cfg or cfg_attr predicate naming `test`, on one line or split across lines:
#        #[cfg(test)], #![cfg(test)], #[cfg(all(windows, test))], #[cfg_attr(test, ..)];
#      - a doc code block in any doc form (/// and //! lines, /** */ and /*! */ blocks,
#        #[doc = "..."] / #![doc = "..."] strings): a ``` or ~~~ fence (also after a list or
#        quote marker) other than ```text, or an indented (4+ columns) block, i.e. an
#        indented doc line after a blank doc line, a heading, a fence or the start of the
#        docs (an indented line right after paragraph text is a lazy continuation and passes);
#      - #[doc = <anything but a string literal>], e.g. include_str!(..), concat!(..);
#      - a block comment, string or attribute left open at the end of a file;
#   2. src-tauri/benches, src-tauri/examples, or [[bench]] / [[example]] in
#      src-tauri/Cargo.toml: bench and example exes.
# Not caught (outside what a source scan can see): tests a proc macro generates from an
# attribute with another name, and doc text built at compile time other than via #[doc = ..].
# Host bash, find, sort, grep and awk; no toolchain.
#
# Usage: scripts/ci/shell-test-layout.sh [shell-dir]   (default: src-tauri)
# Exit 0: layout ok. Exit 1: a violation (listed). Exit 3: cannot run (shell dir missing,
# find, sort, grep or awk error); never reported as a pass.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
shell="${1:-src-tauri}"
scan=scripts/ci/shell-test-layout.awk
if [ ! -d "$shell/src" ] || [ ! -f "$shell/Cargo.toml" ]; then
  echo "shell-test-layout: cannot run: $shell/src or $shell/Cargo.toml not found" >&2
  exit 3
fi

# Each scan fails closed: a tool error is "cannot run" (exit 3), never a pass.
cannot_run() { echo "shell-test-layout: cannot run: $1" >&2; exit 3; }
[ -f "$scan" ] || cannot_run "$scan not found"
found=""
# add <note> <hits...>: one listed line per non-empty hit, prefixed with <prefix>.
add() {
  local note="$1" prefix="${3:-}" hit
  while IFS= read -r hit; do
    [ -n "$hit" ] && found="$found  $prefix$hit  ($note)"$'\n'
  done <<<"$2"
}

# 1. Test attributes, cfg(test) predicates and doc code blocks under src (one awk lexer).
# A path with a newline splits into names that do not exist, so awk fails: still exit 3.
list="$(find "$shell/src" -name '*.rs' -type f)" || cannot_run "find failed on $shell/src"
list="$(LC_ALL=C sort <<<"$list")" || cannot_run "sort failed on the file list of $shell/src"
files=()
[ -n "$list" ] && mapfile -t files <<<"$list"
if [ "${#files[@]}" -gt 0 ]; then
  hits="$(LC_ALL=C awk -f "$scan" "${files[@]}")" || cannot_run "awk failed on $shell/src"
  while IFS= read -r hit; do
    [ -n "$hit" ] && found="$found  $hit"$'\n'
  done <<<"$hits"
fi

# 2. Bench and example targets. grep: 0 = hits, 1 = none, 2 = error.
for d in benches examples; do
  [ -e "$shell/$d" ] && add "bench or example exes are not configured by build.rs" "$shell/$d/"
done
hits="$(grep -nE '^[[:space:]]*\[\[[[:space:]]*(bench|example)[[:space:]]*\]\]' "$shell/Cargo.toml")"
[ $? -le 1 ] || cannot_run "grep failed on $shell/Cargo.toml"
add "a bench or example target: its exe is not configured by build.rs" "$hits" "$shell/Cargo.toml:"

if [ -n "$found" ]; then
  {
    echo "shell-test-layout: FAIL: shell tests belong only in $shell/tests/*.rs (or, for platform-independent logic, in crates/voicen-core)."
    echo "Only integration tests get the release exe's Windows link configuration from $shell/build.rs;"
    echo "any other test exe of the voicen package links or starts only partly on Windows CI (F-001, F-002)."
    echo "Lib doctests are off ([lib] doctest = false), so a doc code block would be skipped, not run: write examples as \`\`\`text."
    echo "Why and what to do instead: docs/decisions/ci-toolchain.md"
    printf '%s' "$found"
  } >&2
  exit 1
fi
echo "shell-test-layout: ok: no tests in $shell/src (no test attributes, no doc code blocks), no benches or examples"
