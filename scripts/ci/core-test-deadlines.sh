#!/usr/bin/env bash
# T-165 (P-020, decides_on: token; class os-answer-deadline-race, successor sweep of T-080, T-110,
# T-111; model: core-test-os-answer.sh): no voicen-core test outside tests/common builds a product
# deadline or a timed server plan itself. Every Timeouts value handed to product code and every
# Serve plan with a hold or a gap comes from a named constructor in tests/common whose name states
# its kind (docs/decisions/core-tests.md, in-time rule, held at T3 by this tripwire).
#
# Over every *.rs under the tests dir, recursively, except the top-level <dir>/common/ tree, a line
# is refused when it holds one of these raw tokens, case-sensitive (Rust identifiers are; rustfmt,
# pinned by cargo fmt --check in check-core, keeps each on one line), comments included (no //
# skip: the guard reads no Rust syntax, F-003):
#   Timeouts {        blanks allowed between, so `-> Timeouts {` and `for Timeouts {` refuse too;
#   Timeouts::        a path call (Timeouts::default(), Timeouts::from_settings);
#   Timeouts as       an alias import;
#   = Timeouts        followed by a non-identifier character or the line end (a type alias);
#   Default::default  an inferred Timeouts;
#   Stall, Trickle, SilentHeaders   as words at identifier boundaries (a Serve plan with a hold or
#                     a gap, with or without the Serve:: path or an alias).
# Not tokens, by decision #258 (OQ-82 (a)): TimeoutSettings { .. } and s.timeouts.<role>_s = ..
# (the settings route, a named T1 residual in core-tests.md). Not a token either: a Duration
# handed to common::download::fixture / fixture_with: they take none, so rustc refuses the call
# (pinned by fn-pointer checks in tests/common/deadlines_tests.rs).
# Not caught (T1 review, core-tests.md): field mutation of a Timeouts after construction (#112),
# <_>::default() or T::default() inferred into Timeouts, a deadline built at run time from other
# parts, the #[cfg(test)] units under crates/voicen-core/src, examples/ and src-tauri/tests.
#
# Usage: scripts/ci/core-test-deadlines.sh [tests-dir]   (default crates/voicen-core/tests)
# Exit 0: "ok: <n> file(s) outside common". Exit 1: one "<file>:<line>: deadline ..." per
# offending line. Exit 3: cannot run (dir missing, no *.rs, unreadable file); never a pass.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
dir="${1:-crates/voicen-core/tests}"
dir="${dir%/}"
cannot_run() { echo "core-test-deadlines: cannot run: $1" >&2; exit 3; }
[ -d "$dir" ] && [ -r "$dir" ] || cannot_run "tests dir $dir not found or not readable"

mapfile -d '' all < <(find "$dir" -type f -name '*.rs' -print0 | sort -z)
[ "${#all[@]}" -gt 0 ] || cannot_run "no *.rs under $dir"
files=()
for f in "${all[@]}"; do
  case "$f" in
    "$dir"/common/*) continue ;;
  esac
  [ -r "$f" ] || cannot_run "$f is not readable"
  files+=("$f")
done

if [ "${#files[@]}" -eq 0 ]; then
  echo "core-test-deadlines: ok: 0 file(s) outside common"
  exit 0
fi

report="$(LC_ALL=C awk '
{
  line = $0
  # Pad so the word boundaries also hold at the line start and end.
  padded = " " line " "
  if (line ~ /Timeouts[ \t]*\{/)
    print FILENAME ":" FNR ": deadline: a Timeouts literal or local constructor (Timeouts {); take a named constructor from tests/common (common::os_answer::in_time, common::deadlines)"
  else if (index(line, "Timeouts::") > 0)
    print FILENAME ":" FNR ": deadline: a Timeouts path call (Timeouts::); take a named constructor from tests/common"
  else if (index(line, "Timeouts as") > 0)
    print FILENAME ":" FNR ": deadline: a Timeouts alias import (Timeouts as); take a named constructor from tests/common"
  else if (padded ~ /= Timeouts[^A-Za-z0-9_]/)
    print FILENAME ":" FNR ": deadline: a Timeouts type alias (= Timeouts); take a named constructor from tests/common"
  else if (index(line, "Default::default") > 0)
    print FILENAME ":" FNR ": deadline: an inferred value (Default::default); take a named constructor from tests/common"
  else if (padded ~ /[^A-Za-z0-9_](Stall|Trickle|SilentHeaders)[^A-Za-z0-9_]/)
    print FILENAME ":" FNR ": deadline: a timed Serve plan (Stall, Trickle, SilentHeaders); take a named plan from tests/common"
}
' "${files[@]}")" || cannot_run "awk failed"

if [ -n "$report" ]; then
  printf '%s\n' "$report"
  n="$(printf '%s\n' "$report" | wc -l)"
  echo "core-test-deadlines: FAIL: $n line(s) build a deadline or a timed server plan outside tests/common (T-165, docs/decisions/core-tests.md)" >&2
  exit 1
fi
echo "core-test-deadlines: ok: ${#files[@]} file(s) outside common"
