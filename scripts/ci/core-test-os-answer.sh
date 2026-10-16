#!/usr/bin/env bash
# T-089 (P-020, decides_on: token; successor of the I1 half of the retired core-test-clocks.sh,
# T-080 rca, class os-answer-deadline-race; F-005, F-013): no voicen-core test outside
# tests/common names an OS-answer target itself. Such a target comes only from
# tests/common::os_answer, together with deadlines sized by its probe (docs/decisions/core-tests.md,
# invariant I1, held at T3 by this tripwire).
#
# Over every *.rs under the tests dir, recursively, except the top-level <dir>/common/ tree, a line
# is refused when it holds one of these raw tokens, in any case, comments included (no // skip:
# the guard reads no Rust syntax, F-003):
#   .invalid       not followed by a letter, digit or _ (RFC 6761 host; failure.invalid_x and
#                  .invalidate( are other words);
#   127.0.0.1:1    not followed by a digit (the refused loopback; :10, :18080, :0 are other values);
#   192.0.2.1      not followed by a digit (the blackhole; 192.0.2.10 is another value).
# The wall-clock rule I2 is not this guard's: rustc decides it (clippy disallowed-methods in
# crates/voicen-core/clippy.toml, proven by make check-ban-canary).
# Not caught (T1 review, decisions #112, T-089 §7): the array form SocketAddr::from(([127, 0, 0,
# 1], 1)), Ipv4Addr::LOCALHOST, 127.1, :01, an address built at run time from other parts, the
# #[cfg(test)] unit tests under crates/voicen-core/src and the shell tests (src-tauri/tests).
#
# Usage: scripts/ci/core-test-os-answer.sh [tests-dir]   (default crates/voicen-core/tests)
# Exit 0: "ok: <n> file(s) outside common". Exit 1: one "<file>:<line>: os-answer ..." per
# offending line. Exit 3: cannot run (dir missing, no *.rs, unreadable file); never a pass.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
dir="${1:-crates/voicen-core/tests}"
dir="${dir%/}"
cannot_run() { echo "core-test-os-answer: cannot run: $1" >&2; exit 3; }
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
  echo "core-test-os-answer: ok: 0 file(s) outside common"
  exit 0
fi

report="$(LC_ALL=C awk '
{
  line = tolower($0)
  if (line ~ /\.invalid([^a-z0-9_]|$)/)
    print FILENAME ":" FNR ": os-answer: a host under .invalid (RFC 6761); take common::os_answer::OsAnswer::unresolvable()"
  else if (line ~ /127\.0\.0\.1:1([^0-9]|$)/)
    print FILENAME ":" FNR ": os-answer: the refused loopback 127.0.0.1:1; take common::os_answer::OsAnswer::refused()"
  else if (line ~ /192\.0\.2\.1([^0-9]|$)/)
    print FILENAME ":" FNR ": os-answer: the blackhole 192.0.2.1; take common::os_answer::Unanswered::blackhole()"
}
' "${files[@]}")" || cannot_run "awk failed"

if [ -n "$report" ]; then
  printf '%s\n' "$report"
  n="$(printf '%s\n' "$report" | wc -l)"
  echo "core-test-os-answer: FAIL: $n line(s) name an OS-answer target outside tests/common (T-080 I1, docs/decisions/core-tests.md)" >&2
  exit 1
fi
echo "core-test-os-answer: ok: ${#files[@]} file(s) outside common"
