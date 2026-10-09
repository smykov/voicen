#!/usr/bin/env bash
# T-080 (rca, class os-answer-deadline-race) guard of the guard: the tripwire
# scripts/ci/core-test-clocks.sh must give its documented exit code on every committed fixture
# tests dir under scripts/ci/fixtures/core-test-clocks/<case>/, on two cases built here (no
# dir, a dir without *.rs) and on the real crates/voicen-core/tests.
#
# The rule the tripwire enforces (docs/decisions/core-tests.md, T-080 invariants I1 and I2),
# over every *.rs under the tests dir, recursively, except the top-level <dir>/common/ tree:
#   I1  no non-comment line names an OS-answer target: a host under `.invalid` (RFC 6761),
#       the refused loopback 127.0.0.1:1 (as text or as SocketAddr::from(([127, 0, 0, 1], 1))),
#       the blackhole 192.0.2.1 (192.0.2.10, 127.0.0.1:0 and 127.0.0.1:18080 are other values);
#       such a target comes only from tests/common together with its deadlines;
#   I2  no non-comment line reads the wall clock itself: Instant::now, .elapsed(, or an alias
#       of Instant (`Instant as`); timing goes through the tests/common timing helpers.
# A line whose first non-blank characters are // (also /// and //!) is a comment; `//` inside
# a string on a code line (http://...) is not.
#   ok-*  exit 0, "ok: <n> file(s) outside common" (n pins that the files were read);
#   v-*   exit 1, the listing names <file>:<line>: os-answer ... (I1) or <file>:<line>: clock
#         ... (I2), one line per offending line;
#   c-*   exit 3, "cannot run".
# Every fixture dir must appear in the table, so a case cannot be dropped silently.
# Usage: scripts/ci/core-test-clocks.test.sh   (host bash)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
guard=scripts/ci/core-test-clocks.sh
fx=scripts/ci/fixtures/core-test-clocks
[ -d "$fx" ] || { echo "core-test-clocks.test: cannot run: $fx not found" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "core-test-clocks.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

failed=0
passed=0
declare -A seen=()

# run <label> <expected exit> <needle> [guard args...]; the needle must be in the output.
run() {
  local label="$1" want="$2" needle="$3" out got
  shift 3
  if [ ! -x "$guard" ]; then
    echo "FAIL $label: $guard not found or not executable (the tripwire does not exist)" >&2
    failed=$((failed + 1))
    return
  fi
  out="$("$guard" "$@" 2>&1)"
  got=$?
  if [ "$got" -ne "$want" ]; then
    echo "FAIL $label: exit $got, want $want" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
  elif ! grep -qF -- "$needle" <<<"$out"; then
    echo "FAIL $label: exit $got as wanted, but the output does not contain: $needle" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
  else
    echo "ok   $label: exit $got"
    passed=$((passed + 1))
  fi
}

# check <case> <expected exit> <needle>: the case's dir as the tests dir. A needle starting
# with "/" is taken relative to the case dir (file:line: kind).
check() {
  local n="$3"
  seen["$1"]=1
  [ "${n#/}" = "$n" ] || n="$fx/$1$n"
  run "$1" "$2" "$n" "$fx/$1"
}

# Allowed.
check ok-targets-and-clock-in-common  0 "ok: 1 file(s) outside common"   # the literals and the clock live in common/; the caller takes the case
check ok-other-addresses              0 "ok: 1 file(s) outside common"   # 192.0.2.10 (settings_log), 127.0.0.1:0 bind, 127.0.0.1:18080, 198.51.100.7
check ok-comments                     0 "ok: 1 file(s) outside common"   # //, ///, //! lines naming targets and Instant::now
check ok-config-comparison            0 "ok: 1 file(s) outside common"   # Duration comparisons of configured values, no clock read
check ok-timing-helpers               0 "ok: 2 file(s) outside common"   # measure / fastest / at_least / within_spec("SC-003"); a nested dir is read
check ok-unanswered-case              0 "ok: 1 file(s) outside common"   # the blackhole only as common::os_answer::Unanswered (review 1 #2); 198.51.100.9 never contacted

# I1: an OS-answer target outside common.
check v-invalid-own-connect           1 "/openai_client.rs:13: os-answer"   # T-078's shape: voicen-test.invalid with its own 2 s connect; `//` of http:// is not a comment
check v-invalid-own-connect           1 "/openai_client.rs:21: os-answer"   # the same with a query
check v-other-invalid-name            1 "/engine.rs:6: os-answer"           # any .invalid name, not only voicen-test
check v-refused-url                   1 "/api_pipeline.rs:6: os-answer"     # http://127.0.0.1:1/v1
check v-refused-socketaddr            1 "/local_server.rs:7: os-answer"     # SocketAddr::from(([127, 0, 0, 1], 1))
check v-refused-socketaddr            1 "/local_server.rs:11: os-answer"    # "127.0.0.1:1".parse()
check v-blackhole                     1 "/api_pipeline.rs:9: os-answer"     # http://192.0.2.1/v1
check v-blackhole                     1 "/dictation_session.rs:5: os-answer" # http://192.0.2.1:9/v1
check v-common-prefix-file            1 "/common_extra.rs:4: os-answer"     # only the common/ dir is exempt, not a file named common*

# I2: a wall-clock reading outside common.
check v-took-ceiling                  1 "/post_process_timeout.rs:8: clock" # let started = Instant::now();
check v-took-ceiling                  1 "/post_process_timeout.rs:10: clock" # started.elapsed(); then took < 1.5 s
# The same ceiling on measure()'s result (`let (got, took) = measure(..); assert!(took < ..)`)
# names no clock, so it is not this tripwire's: it does not compile, because measure returns
# an opaque common::timing::Took (pinned by timing_tests::a_measured_time_cannot_be_compared_
# or_read_back_so_no_ceiling_compiles, in make check through cargo test; review 1 #1).
check v-elapsed-on-helper-instant     1 "/local_download.rs:9: clock"       # .elapsed() on an Instant handed out by common
check v-instant-alias                 1 "/openai_client.rs:3: clock"        # use std::time::Instant as Clock;
check v-nested-dir                    1 "/diag_support/mod.rs:6: clock"     # a subdir other than common/ is read
check v-nested-common-dir             1 "/diag_support/common/mod.rs:5: clock" # only the top-level common/ is exempt

# Cannot run.
run c-no-dir 3 "cannot run" "$tmp/absent"
mkdir -p "$tmp/no-rs" && printf 'x\n' >"$tmp/no-rs/README.md"
run c-no-rs-files 3 "cannot run" "$tmp/no-rs"

# The real tree with the guard's default (crates/voicen-core/tests): red until every OS-answer
# target and clock reading outside tests/common is routed through it.
run "real tree (crates/voicen-core/tests)" 0 "ok:"

# Every committed fixture dir is in the table.
for d in "$fx"/*/; do
  c="$(basename "$d")"
  if [ -z "${seen[$c]:-}" ]; then
    echo "FAIL $c: fixture dir has no expected exit code in $0" >&2
    failed=$((failed + 1))
  fi
done

if [ "$failed" -gt 0 ]; then
  echo "core-test-clocks.test: FAIL: $failed case(s) differ, $passed as expected (T-080)" >&2
  exit 1
fi
echo "core-test-clocks.test: ok: $passed cases as expected"
