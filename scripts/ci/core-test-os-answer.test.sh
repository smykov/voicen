#!/usr/bin/env bash
# T-089 (P-020, decides_on: token; successor of the I1 half of scripts/ci/core-test-clocks.sh,
# T-080 rca, class os-answer-deadline-race) guard of the guard: the tripwire
# scripts/ci/core-test-os-answer.sh must give its documented exit code on every committed fixture
# tests dir under scripts/ci/fixtures/core-test-os-answer/<case>/, on two cases built here (no
# dir, a dir without *.rs) and on the real crates/voicen-core/tests.
#
# The rule (docs/decisions/core-tests.md, invariant I1), over every *.rs under the tests dir,
# recursively, except the top-level <dir>/common/ tree: no line names an OS-answer target, as a
# raw token, in any case, comments included (no // skip; the guard reads no Rust syntax):
#   .invalid             not followed by a letter, digit or _ (RFC 6761 host; failure.invalid_x,
#                        .invalidate( are other words);
#   127.0.0.1:1          not followed by a digit (the refused loopback; :10, :18080, :0 are other
#                        values);
#   192.0.2.1            not followed by a digit (the blackhole; 192.0.2.10 is another value).
# Such a target comes only from tests/common::os_answer together with its deadlines. The
# wall-clock rule I2 left this tripwire: rustc decides it (clippy disallowed-methods,
# make check-ban-canary), so a clock read is not this guard's (ok-clock-reads-are-not-this-rule).
# Not caught (decisions #112, T-089 §7): the array form SocketAddr::from(([127, 0, 0, 1], 1)),
# Ipv4Addr::LOCALHOST, 127.1, :01, an address built at run time from other parts.
#   ok-*  exit 0, "ok: <n> file(s) outside common" (n pins that the files were read);
#   v-*   exit 1, the listing names <file>:<line>: os-answer ..., one line per offending line;
#   c-*   exit 3, "cannot run".
# Every fixture dir must appear in the table, so a case cannot be dropped silently.
# Usage: scripts/ci/core-test-os-answer.test.sh   (host bash)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
guard=scripts/ci/core-test-os-answer.sh
fx=scripts/ci/fixtures/core-test-os-answer
[ -d "$fx" ] || { echo "core-test-os-answer.test: cannot run: $fx not found" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "core-test-os-answer.test: cannot run: mktemp failed" >&2; exit 3; }
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
check ok-targets-in-common             0 "ok: 1 file(s) outside common"   # the literals live in common/; the caller takes the case
check ok-other-addresses               0 "ok: 1 file(s) outside common"   # 192.0.2.10, 127.0.0.1:0 bind, 127.0.0.1:18080, :10, 198.51.100.7
check ok-unanswered-case               0 "ok: 1 file(s) outside common"   # the blackhole only as common::os_answer::Unanswered
check ok-invalid-inside-a-word         0 "ok: 1 file(s) outside common"   # failure.invalid_api_key, .invalidValue, .invalidate(), .invalid2
check ok-clock-reads-are-not-this-rule 0 "ok: 1 file(s) outside common"   # Instant::now / .elapsed(): rustc's ban, not this tripwire's

# Code lines naming a target (T-080 I1 cases, kept).
check v-invalid-own-connect            1 "/openai_client.rs:13: os-answer"   # T-078's shape: voicen-test.invalid with its own 2 s connect
check v-invalid-own-connect            1 "/openai_client.rs:21: os-answer"   # the same with a query
check v-other-invalid-name             1 "/engine.rs:6: os-answer"           # any .invalid name, not only voicen-test
check v-refused-url                    1 "/api_pipeline.rs:6: os-answer"     # http://127.0.0.1:1/v1
check v-refused-text                   1 "/local_server.rs:11: os-answer"    # "127.0.0.1:1".parse()
check v-blackhole                      1 "/api_pipeline.rs:9: os-answer"     # http://192.0.2.1/v1
check v-blackhole                      1 "/dictation_session.rs:5: os-answer" # http://192.0.2.1:9/v1
check v-common-prefix-file             1 "/common_extra.rs:4: os-answer"     # only the common/ dir is exempt, not a file named common*
check v-nested-dir                     1 "/diag_support/mod.rs:3: os-answer" # a subdir other than common/ is read
check v-nested-common-dir              1 "/diag_support/common/mod.rs:3: os-answer" # only the top-level common/ is exempt

# T-089: the token in any case, comments included, with no syntax around it (each exit 0 on the
# old core-test-clocks.sh: the // skip, the case-sensitive match, the name before the dot).
check v-invalid-in-line-comment        1 "/connection_test.rs:4: os-answer"  # // the host is voicen-test.invalid, ...
check v-invalid-in-doc-comment         1 "/openai_client.rs:1: os-answer"    # //! Fake data only: example.com, voicen-test.invalid.
check v-invalid-upper-case             1 "/openai_client.rs:3: os-answer"    # http://VOICEN-TEST.INVALID/v1
check v-invalid-built-by-format        1 "/engine.rs:4: os-answer"           # format!("http://{}.invalid/v1", name)
check v-refused-at-line-end-in-comment 1 "/local_server.rs:4: os-answer"     # // ... 127.0.0.1:1 at the end of the line
check v-blackhole-in-comment           1 "/api_pipeline.rs:4: os-answer"     # an indented // 192.0.2.1 comment

# Cannot run.
run c-no-dir 3 "cannot run" "$tmp/absent"
mkdir -p "$tmp/no-rs" && printf 'x\n' >"$tmp/no-rs/README.md"
run c-no-rs-files 3 "cannot run" "$tmp/no-rs"

# The real tree with the guard's default (crates/voicen-core/tests): red until the comment lines
# naming a target outside tests/common are reworded (T-089 §7).
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
  echo "core-test-os-answer.test: FAIL: $failed case(s) differ, $passed as expected (T-089)" >&2
  exit 1
fi
echo "core-test-os-answer.test: ok: $passed cases as expected"
