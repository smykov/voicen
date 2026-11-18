#!/usr/bin/env bash
# T-165 (P-020, decides_on: token; class os-answer-deadline-race, successor sweep of T-080, T-110,
# T-111; model: core-test-os-answer.test.sh) guard of the guard: the tripwire
# scripts/ci/core-test-deadlines.sh must give its documented exit code on every committed fixture
# tests dir under scripts/ci/fixtures/core-test-deadlines/<case>/, on two cases built here (no
# dir, a dir without *.rs) and on the real crates/voicen-core/tests.
#
# The rule (docs/decisions/core-tests.md, in-time rule), over every *.rs under the tests dir,
# recursively, except the top-level <dir>/common/ tree: no line builds a product deadline or a
# timed server plan, as a raw token, case-sensitive (Rust identifiers are), comments included (no
# // skip; the guard reads no Rust syntax). A line is refused when it holds one of:
#   Timeouts {      blanks allowed between, so `-> Timeouts {` and `for Timeouts {` refuse too;
#   Timeouts::      a path call (`Timeouts::default()`, `Timeouts::from_settings`);
#   Timeouts as     an alias import;
#   Timeouts>::     a qualified-path call (`<Timeouts>::default()`,
#                   `<voicen_core::timeouts::Timeouts>::from_settings(..)`);
#   = Timeouts      with optional blanks after `=` and optional `ident::` segments before
#                   `Timeouts`, then a non-identifier character or the line end (a type alias,
#                   `type D = voicen_core::timeouts::Timeouts;`; `= TimeoutsView` is another word);
#   Default::default   an inferred Timeouts;
#   Stall, Trickle, SilentHeaders   as words at identifier boundaries (any Serve plan with a
#                   hold or gap, with or without the `Serve::` path, after `use Serve::*`, under
#                   an alias; `Stalled`, `Trickled` are other words).
# Not tokens (decision #258, OQ-82 (a)): `TimeoutSettings { .. }` and `s.timeouts.<role>_s = ..`,
# the settings route, a named T1 residual (exit 0 expected). Not a token either: a Duration handed
# to common::download::fixture / fixture_with / timeouts: those functions lose the parameter and
# rustc refuses the call (pinned by fn-pointer checks in tests/common/deadlines_tests.rs), so the
# fixture ok-fixture-duration-is-rustcs expects exit 0 here.
# Contract (decides_on): exit 0 ok, 1 listed lines, 3 cannot run (as core-test-os-answer.sh).
#   ok-*  exit 0, "ok: <n> file(s) outside common" (n pins that the files were read);
#   v-*   exit 1, the listing names <file>:<line>: deadline ..., one line per offending line;
#   c-*   exit 3, "cannot run".
# Every fixture dir must appear in the table, so a case cannot be dropped silently.
# Usage: scripts/ci/core-test-deadlines.test.sh   (host bash)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
guard=scripts/ci/core-test-deadlines.sh
fx=scripts/ci/fixtures/core-test-deadlines
[ -d "$fx" ] || { echo "core-test-deadlines.test: cannot run: $fx not found" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "core-test-deadlines.test: cannot run: mktemp failed" >&2; exit 3; }
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
check ok-in-common                    0 "ok: 1 file(s) outside common"   # every constructor lives in common/; the caller takes the named one
check ok-type-positions               0 "ok: 1 file(s) outside common"   # Timeouts as a type only: t: Timeouts, Option<Timeouts>, Vec<Timeouts>
check ok-lowercase-timeouts           0 "ok: 1 file(s) outside common"   # a field, a variable and the module path
check ok-other-serve-plans            0 "ok: 1 file(s) outside common"   # Serve::Full / Status / Redirect have no hold or gap
check ok-settings-route               0 "ok: 1 file(s) outside common"   # decision #258: TimeoutSettings { .. } and s.timeouts.x_s = .. are not tokens
check ok-stalled-trickled-words       0 "ok: 1 file(s) outside common"   # Stalled, Trickled, StallCount, BigTrickle: identifier boundary
check ok-alias-prefix-word            0 "ok: 1 file(s) outside common"   # = TimeoutsView, = Timeouts2: another word
check ok-fixture-duration-is-rustcs   0 "ok: 1 file(s) outside common"   # fixture(&server, Duration::from_secs(2)): rustc refuses it once the signature changes, not this tripwire

# One refused case per token.
check v-timeouts-literal              1 "/api_pipeline.rs:4: deadline"           # Timeouts {
check v-timeouts-return-type          1 "/local_server.rs:3: deadline"           # -> Timeouts {
check v-timeouts-impl-for             1 "/diag_pipeline.rs:3: deadline"          # impl X for Timeouts {
check v-timeouts-blanks               1 "/openai_client.rs:2: deadline"          # Timeouts    {  (blanks between)
check v-timeouts-path                 1 "/post_process_timeout.rs:2: deadline"   # Timeouts::default()
check v-timeouts-path                 1 "/post_process_timeout.rs:5: deadline"   # Timeouts::from_settings
check v-timeouts-as                   1 "/connection_test.rs:1: deadline"        # {Timeouts as Deadlines}
check v-timeouts-type-alias           1 "/final_pass_http.rs:3: deadline"        # type Deadlines = Timeouts;
check v-timeouts-qualified-path       1 "/post_process_chat.rs:4: deadline"      # handoff(<Timeouts>::default()): only Timeouts>:: matches
check v-timeouts-path-alias           1 "/connection_test.rs:3: deadline"        # type D = voicen_core::timeouts::Timeouts;: only the widened alias matches
check v-default-default               1 "/local_engine.rs:2: deadline"           # let t: Timeouts = Default::default();
check v-serve-stall                   1 "/local_download.rs:2: deadline"         # Serve::Stall { hold }
check v-serve-trickle                 1 "/local_models_service.rs:2: deadline"   # Serve::Trickle { gap }
check v-serve-silent-headers          1 "/local_model_delete.rs:2: deadline"     # Serve::SilentHeaders { hold }
check v-silent-headers-bare           1 "/local_no_network.rs:4: deadline"       # SilentHeaders { .. } without Serve::
check v-silent-headers-bare           1 "/local_no_network.rs:1: deadline"       # use Serve::SilentHeaders;
check v-use-serve-glob-then-stall     1 "/silero_model.rs:5: deadline"           # use Serve::*; then Stall { .. }
check v-trickle-alias                 1 "/local_download.rs:1: deadline"         # {Trickle as Slow}
check v-comment-hit                   1 "/post_process_chat.rs:1: deadline"      # // ... Timeouts::default() ...: comments included
check v-nested-dir                    1 "/diag_support/mod.rs:2: deadline"       # a subdir other than common/ is read
check v-common-prefix-file            1 "/common_extra.rs:2: deadline"           # only the common/ dir is exempt, not a file named common*
check v-nested-common-dir             1 "/diag_support/common/mod.rs:3: deadline" # only the top-level common/ is exempt

# Cannot run.
run c-no-dir 3 "cannot run" "$tmp/absent"
mkdir -p "$tmp/no-rs" && printf 'x\n' >"$tmp/no-rs/README.md"
run c-no-rs-files 3 "cannot run" "$tmp/no-rs"

# The real tree with the guard's default (crates/voicen-core/tests): red until the deadline
# constructors move into tests/common (T-165, 71 token lines in 13 files at the red commit).
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
  echo "core-test-deadlines.test: FAIL: $failed case(s) differ, $passed as expected (T-165)" >&2
  exit 1
fi
echo "core-test-deadlines.test: ok: $passed cases as expected"
