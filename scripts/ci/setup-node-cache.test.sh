#!/usr/bin/env bash
# T-068 guard of the guard: scripts/ci/setup-node-cache.sh must give its documented exit code
# on every committed fixture workflows dir under scripts/ci/fixtures/setup-node-cache/<case>/,
# on two cases built here (no dir, a dir without workflow files) and on the real
# .github/workflows. Rule and setup-node v5 semantics: the header of setup-node-cache.sh.
#   ok-*  exit 0, "ok: <n> actions/setup-node step(s)" (n pins that the steps were read, not
#         skipped);
#   v-*   exit 1, the listing names <file>:<line>: job <job> of the offending step;
#   c-*   exit 3, "cannot run".
# Every fixture dir must appear in the table, so a case cannot be dropped silently.
# Usage: scripts/ci/setup-node-cache.test.sh   (host bash and awk)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
guard=scripts/ci/setup-node-cache.sh
fx=scripts/ci/fixtures/setup-node-cache
[ -d "$fx" ] || { echo "setup-node-cache.test: cannot run: $fx not found" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "setup-node-cache.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

failed=0
passed=0
declare -A seen=()

# run <label> <expected exit> <needle> [guard args...]; every needle must be in the output.
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
# with "/" is taken relative to the case dir (file:line: job ...).
check() {
  local n="$3"
  seen["$1"]=1
  [ "${n#/}" = "$n" ] || n="$fx/$1$n"
  run "$1" "$2" "$n" "$fx/$1"
}

# Allowed.
check ok-fixed                          0 "ok: 2 actions/setup-node step(s)"   # T-068 fix: gate package-manager-cache: false; windows pnpm/action-setup + cache: pnpm
check ok-pnpm-before-cache-pnpm         0 "ok: 1 actions/setup-node step(s)"   # today's windows job alone
check ok-pnpm-before-auto-cache         0 "ok: 1 actions/setup-node step(s)"   # pnpm/action-setup (with a name: and with:) before, no cache: the auto cache finds pnpm
check ok-cache-npm                      0 "ok: 1 actions/setup-node step(s)"   # npm ships with node
check ok-quoted-false-other-key-order   0 "ok: 1 actions/setup-node step(s)"   # name:/if:/with: before a quoted uses:, "false" quoted with a trailing comment
check ok-steps-same-indent-cache-empty  0 "ok: 1 actions/setup-node step(s)"   # steps items at the steps: indent, cache: '' = unset, FALSE; pnpm/action-setup only inside a run script
check ok-no-setup-node                  0 "ok: 0 actions/setup-node step(s)"

# Violations: exit 1, the listing names file:line and job of the setup-node step.
check v-gate-today                      1 "/ci.yml:15: job gate:"               # the shape of run 37498788064 (the windows job beside it passes)
check v-pnpm-after-setup-node           1 "/ci.yml:12: job build:"              # pnpm/action-setup only after setup-node
check v-pnpm-in-other-job               1 "/ci.yml:19: job second:"             # pnpm set up in another job; cache: pnpm wins over package-manager-cache: false
check v-pmc-commented-out               1 "/ci.yml:12: job gate:"
check v-pmc-at-step-level               1 "/ci.yml:12: job gate:"               # beside with:, not under it: not an input
check v-pmc-on-next-step                1 "/ci.yml:12: job gate:"               # under the next step's with:
check v-pmc-nested-deeper               1 "/ci.yml:12: job gate:"               # text inside a block scalar of another input
check v-pmc-true-or-expression          1 "/ci.yml:12: job gate:"               # true
check v-pmc-true-or-expression          1 "/ci.yml:18: job other:"              # \${{ false }} is not a written false
check v-cache-yarn                      1 "/ci.yml:12: job gate:"
check v-flow-with                       1 "/ci.yml:12: job gate:"               # flow-style with: is not read: fail-closed
check v-merge-key                       1 "/ci.yml:12: job gate:"               # merge key: inputs not written out
check v-flow-step                       1 "/ci.yml:12: a non-comment line names actions/setup-node"
check v-second-file                     1 "/release.yaml:12: job release:"      # every workflow file, .yaml too

# Cannot run.
check c-no-jobs                         3 "cannot run"
check c-empty-file                      3 "cannot run"
run c-no-dir 3 "cannot run" "$tmp/absent"
mkdir -p "$tmp/no-files" && printf 'x\n' >"$tmp/no-files/README.md"
run c-no-workflow-files 3 "cannot run" "$tmp/no-files"

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
  echo "setup-node-cache.test: FAIL: $failed case(s) differ, $passed as expected (T-068)" >&2
  exit 1
fi
echo "setup-node-cache.test: ok: $passed cases as expected"
