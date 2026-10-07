#!/usr/bin/env bash
# T-066 guard of the guard: scripts/ci/ci-wip.sh must give its documented exit code on every
# committed fixture workflows dir under scripts/ci/fixtures/ci-wip/<case>/, on a case built
# here (no dir) and on the real .github/workflows. Rule: the header of ci-wip.sh.
#   ok-*  exit 0, "ok: ... <n> step uses:" (n pins that the steps were read, not skipped);
#   v-*   exit 1, the listing names <file>:<line> of the offending trigger or step;
#   c-*   exit 3, "cannot run".
# Every fixture dir must appear in the table, so a case cannot be dropped silently.
# Usage: scripts/ci/ci-wip.test.sh   (host bash and awk)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
guard=scripts/ci/ci-wip.sh
fx=scripts/ci/fixtures/ci-wip
[ -d "$fx" ] || { echo "ci-wip.test: cannot run: $fx not found" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "ci-wip.test: cannot run: mktemp failed" >&2; exit 3; }
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

# check <case> <expected exit> <needle>: the case's dir as the workflows dir. A needle starting
# with "/" is taken relative to the case dir (file:line: ...).
check() {
  local n="$3"
  seen["$1"]=1
  [ "${n#/}" = "$n" ] || n="$fx/$1$n"
  run "$1" "$2" "$n" "$fx/$1"
}

# Allowed.
check ok-fixed                         0 "ok: ci.yml push triggers on main and wip/**; 9 step uses:"   # the T-066 shape: restore + main-only save, save-if, setup-node without cache
check ok-block-branches-quoted-on      0 "3 step uses:"     # "on": quoted, block branches list with a comment, if: without \${{ }}, blanks dropped in save-if, pnpm cache: false
check ok-second-file-no-trigger        0 "2 step uses: in 2 workflow file(s)"   # the trigger rule is for ci.yml only

# Violations: exit 1, the listing names file:line.
check v-main-only-today                1 "/ci.yml:2: push: branches does not name wip/**"   # the shape before T-066
check v-wip-one-star                   1 "/ci.yml:2: push: branches does not name wip/**"
check v-no-main                        1 "/ci.yml:2: push: branches does not name main"
check v-paths-ignore                   1 "/ci.yml:4: push: has paths-ignore:"
check v-on-inline                      1 "/ci.yml:1: on: is written inline"
check v-wip-only-under-pull-request    1 "/ci.yml:4: push: branches does not name wip/**"
check v-actions-cache                  1 "/ci.yml:9: job gate: actions/cache saves in its post step"   # the Gate cargo-registry step before T-066
check v-rust-cache-default             1 "/ci.yml:9: job windows: Swatinem/rust-cache saves on every ref"   # the windows step before T-066
check v-rust-cache-save-if-true        1 "(it has save-if: true)"
check v-rust-cache-save-if-step-level  1 "/ci.yml:8: job windows: Swatinem/rust-cache saves on every ref"   # beside with:, not an input
check v-save-no-if                     1 "/ci.yml:8: job gate: actions/cache/save runs on refs other than main"
check v-save-if-or                     1 "/ci.yml:8: job gate: actions/cache/save runs on refs other than main"
check v-save-if-not-main               1 "/ci.yml:8: job gate: actions/cache/save runs on refs other than main"
check v-setup-node-cache-pnpm          1 "/ci.yml:9: job windows: actions/setup-node caches"   # the windows setup-node before T-066
check v-setup-node-auto-cache          1 "/ci.yml:9: job windows: actions/setup-node caches"   # packageManager auto cache
check v-pnpm-action-cache              1 "/ci.yml:8: job windows: pnpm/action-setup with cache: true"
check v-unclassified                   1 "/ci.yml:9: job a: example/some-cache-action@v1 is not classified"
check v-job-uses                       1 "/ci.yml:6: job a: a job-level uses:"
check v-flow-step                      1 "/ci.yml:8: a non-comment uses: line"
check v-flow-with                      1 "/ci.yml:8: job a: Swatinem/rust-cache@v2 has a flow-style with:"
check v-second-file                    1 "/release.yaml:9: job release: Swatinem/rust-cache"   # the cache rule holds in every workflow file

# Cannot run.
check c-no-ci-file                     3 "cannot run"
check c-no-jobs                        3 "cannot run"
run c-no-dir 3 "cannot run" "$tmp/absent"

# The real repo with the guard's defaults.
run "real repo (.github/workflows)" 0 "ok:"

# Every committed fixture dir is in the table.
for d in "$fx"/*/; do
  c="$(basename "$d")"
  if [ -z "${seen[$c]:-}" ]; then
    echo "FAIL $c: fixture dir has no expected exit code in $0" >&2
    failed=$((failed + 1))
  fi
done

if [ "$failed" -gt 0 ]; then
  echo "ci-wip.test: FAIL: $failed case(s) differ, $passed as expected (T-066)" >&2
  exit 1
fi
echo "ci-wip.test: ok: $passed cases as expected"
