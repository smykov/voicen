#!/usr/bin/env bash
# T-080 (rca, class os-answer-deadline-race; F-005, F-013): no voicen-core test outside
# tests/common lets a clock it does not control decide its verdict. Each earlier fix closed one
# kind of uncontrolled clock (the Windows refusal time, the resolver retry schedule, host load)
# with its own budget, and the next kind stayed open; this tripwire holds the kind-independent
# rule at T3 (docs/decisions/core-tests.md, invariants I1 and I2).
#
# Over every *.rs under the tests dir, recursively, except the top-level <dir>/common/ tree:
#   I1  no code line names an OS-answer target: a host under `.invalid` (RFC 6761), the refused
#       loopback 127.0.0.1:1 (as text or as SocketAddr::from(([127, 0, 0, 1], 1))) or the
#       blackhole 192.0.2.1. Such a target comes only from tests/common::os_answer, together
#       with deadlines sized by its probe. 192.0.2.10, 127.0.0.1:0/:10/:18080 are other values.
#   I2  no code line reads the wall clock itself: Instant::now, .elapsed( or an alias of Instant
#       (`Instant as`). Timing goes through tests/common::timing (at_least, within_spec, measure).
# A line whose first non-blank characters are // (also /// and //!) is a comment and is not
# read; `//` inside a string on a code line (http://...) is code. The rule decides on raw text
# (F-003): it does not model what a helper does. Not caught: a target built from parts at run
# time, arithmetic on two Instants handed out by common, SystemTime (not a measurement).
#
# Usage: scripts/ci/core-test-clocks.sh [tests-dir]   (default crates/voicen-core/tests)
# Exit 0: "ok: <n> file(s) outside common". Exit 1: violations, one "<file>:<line>: os-answer
# ..." or "<file>:<line>: clock ..." per offending line. Exit 3: cannot run (dir missing, no
# *.rs, unreadable file); never reported as a pass.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
dir="${1:-crates/voicen-core/tests}"
dir="${dir%/}"
cannot_run() { echo "core-test-clocks: cannot run: $1" >&2; exit 3; }
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
  echo "core-test-clocks: ok: 0 file(s) outside common"
  exit 0
fi

report="$(awk '
/^[ \t]*\/\// { next }
{
  line = $0
  if (line ~ /[A-Za-z0-9-]\.invalid([^A-Za-z0-9-]|$)/)
    print FILENAME ":" FNR ": os-answer: a host under .invalid (RFC 6761); take common::os_answer::OsAnswer::unresolvable()"
  else if (line ~ /127\.0\.0\.1:1([^0-9]|$)/ || line ~ /\[ *127 *, *0 *, *0 *, *1 *\] *, *1 *\)/)
    print FILENAME ":" FNR ": os-answer: the refused loopback 127.0.0.1:1; take common::os_answer::OsAnswer::refused()"
  else if (line ~ /192\.0\.2\.1([^0-9]|$)/)
    print FILENAME ":" FNR ": os-answer: the blackhole 192.0.2.1; take it from tests/common with its deadlines"
  if (line ~ /Instant::now/ || line ~ /\.elapsed\(/ || line ~ /Instant[ \t]+as[ \t]/)
    print FILENAME ":" FNR ": clock: a wall-clock reading outside tests/common; use common::timing (at_least / within_spec / measure / now)"
}
' "${files[@]}")" || cannot_run "awk failed"

if [ -n "$report" ]; then
  printf '%s\n' "$report"
  n="$(printf '%s\n' "$report" | wc -l)"
  echo "core-test-clocks: FAIL: $n line(s) let an uncontrolled clock decide a verdict (T-080 I1/I2, docs/decisions/core-tests.md)" >&2
  exit 1
fi
echo "core-test-clocks: ok: ${#files[@]} file(s) outside common"
