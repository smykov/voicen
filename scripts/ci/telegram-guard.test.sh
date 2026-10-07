#!/usr/bin/env bash
# T-070 (validation 1, finding 1, mutation M6): guard of the guard for scripts/ci/telegram-guard.sh.
# The installer may reach Telegram only from a push to main or a v* tag after every earlier step
# succeeded; that rests on the `if:` of each step that reads the Telegram secrets. Contract of the
# checker, pinned here:
#   Usage: scripts/ci/telegram-guard.sh <workflow-file>
#   For every step that references secrets.TELEGRAM_BOT_TOKEN or secrets.TELEGRAM_CHAT_ID
#   (anywhere in the step: env:, with:, run:), its step `if:` -- blanks removed, `${{ }}`
#   optional -- must
#     - have as top-level `&&` operands both `github.event_name=='push'` and
#       `(github.ref=='refs/heads/main'||startsWith(github.ref,'refs/tags/v'))`
#       (a top-level `||` makes them not top-level `&&` operands);
#     - contain no status function: always(), failure(), cancelled(), !cancelled(), success()||;
#   and the step must have no `continue-on-error:`. Such a step without `if:` is a violation.
#   Steps that do not reference the secrets are not judged (they may use always() etc.).
#   Exit 0: ok, one line with "ok:" and "<n> step(s) read the Telegram secrets" (n pins that the
#           steps were read, not skipped; 0 for a workflow without them).
#   Exit 1: a violation; each listed as "<workflow-file>:<line>: <why>", <line> = the line of the
#           step's "- " item.
#   Exit 2: usage error (no argument, or more than one).
#   Exit 3: cannot run (the file is missing or unreadable); never a pass.
# Cases: every dir scripts/ci/fixtures/telegram-guard/<case>/ci.yml (ok-* exit 0, v-* exit 1),
# the real .github/workflows/ci.yml (exit 0, 1 step), the real ci.yml with mutation M6 and with
# the event_name operand dropped (exit 1, its send step's line), usage and a missing file.
# Every fixture dir must appear in the table, so a case cannot be dropped silently.
# Usage: scripts/ci/telegram-guard.test.sh   (host bash, sed, grep)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
guard=scripts/ci/telegram-guard.sh
fx=scripts/ci/fixtures/telegram-guard
real=.github/workflows/ci.yml
[ -d "$fx" ] || { echo "telegram-guard.test: cannot run: $fx not found" >&2; exit 3; }
[ -f "$real" ] || { echo "telegram-guard.test: cannot run: $real not found" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "telegram-guard.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

failed=0
passed=0
declare -A seen=()

# run <label> <expected exit> <needle> [guard args...]; the needle must be in the output (case-insensitive).
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
  elif ! grep -qiF -- "$needle" <<<"$out"; then
    echo "FAIL $label: exit $got as wanted, but the output does not contain: $needle" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
  elif [ -n "${absent:-}" ] && grep -qF -- "$absent" <<<"$out"; then
    echo "FAIL $label: the output names a step that is not a violation: $absent" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
  else
    echo "ok   $label: exit $got"
    passed=$((passed + 1))
  fi
}

# check <case> <expected exit> <needle>: the case's ci.yml as the workflow. A needle starting
# with ":" is prefixed with the case's file path (file:line: ...).
check() {
  local f="$fx/$1/ci.yml" n="$3"
  seen["$1"]=1
  [ "${n#:}" = "$n" ] || n="$f$n"
  run "$1" "$2" "$n" "$f"
}

ok_n() { echo "$1 step(s) read the Telegram secrets"; }

# Allowed.
check ok-guarded               0 "$(ok_n 1)"   # the shipped shape; unrelated steps with !cancelled(), always(), continue-on-error and the bare name TELEGRAM_BOT_TOKEN are not judged
check ok-no-braces-reordered   0 "$(ok_n 1)"   # no ${{ }}, odd blanks, operands reordered
check ok-no-secret             0 "$(ok_n 0)"   # no step reads the secrets

# Violations: exit 1, the listing names file:line of the step's "- " item.
check v-m6-always              1 ":9:"   # validation 1 mutation M6: always() && send, guard gone
check v-always-guarded         1 ":9:"   # guard intact, always() added (status function alone)
check v-failure                1 ":9:"
check v-not-cancelled          1 ":9:"
check v-cancelled              1 ":9:"
check v-success-or             1 ":9:"   # (success() || ...) as an operand
check v-no-event-name          1 ":9:"   # main-or-tag without event_name == 'push' (a pull_request from main)
check v-event-name-negated     1 ":9:"   # event_name != 'push': names event_name, not the operand
check v-no-main-or-tag         1 ":9:"   # push on any ref, wip/** included
check v-top-level-or           1 ":9:"   # guard && send || ref == wip/...: both operands present as text, not top-level &&
check v-widened-ref-operand    1 ":9:"   # (main || tag || event_name == 'pull_request'): not the required operand
check v-continue-on-error      1 ":9:"
check v-no-if                  1 ":9:"
absent="$fx/v-second-step/ci.yml:9:"     # the guarded send at line 9 is fine; only Notify is named
check v-second-step            1 ":16:"  # a second step reads only secrets.TELEGRAM_CHAT_ID, if: ref only
absent=""
check v-secret-in-run          1 ":9:"   # the secret inline in run:, no env:, no if:

# The real workflow: ok, and exactly its one send step is read.
run "real $real" 0 "$(ok_n 1)" "$real"

# The real workflow mutated: M6 exactly as validation 1 applied it, and the event_name operand dropped.
line="$(grep -n -- '- name: Send the installer to Telegram' "$real" | head -n 1 | cut -d: -f1)"
if [ -z "$line" ] || ! grep -q "github.event_name == 'push' && (github.ref == 'refs/heads/main' || startsWith(github.ref, 'refs/tags/v')) && steps.telegram-installer.outputs.send == 'true'" "$real"; then
  echo "FAIL real mutations: the send step or its if: is not in $real as this test expects (update the test with the workflow)" >&2
  failed=$((failed + 1))
else
  sed "s/github.event_name == 'push' && (github.ref == 'refs\/heads\/main' || startsWith(github.ref, 'refs\/tags\/v')) && steps.telegram-installer.outputs.send == 'true'/always() \&\& steps.telegram-installer.outputs.send == 'true'/" \
    "$real" > "$tmp/m6.yml"
  if cmp -s "$real" "$tmp/m6.yml"; then
    echo "FAIL real M6: the mutation did not apply" >&2
    failed=$((failed + 1))
  else
    run "real M6 (always() && send)" 1 "$tmp/m6.yml:$line:" "$tmp/m6.yml"
  fi
  sed "s/github.event_name == 'push' && (github.ref == 'refs\/heads\/main'/(github.ref == 'refs\/heads\/main'/" "$real" > "$tmp/no-event.yml"
  run "real without event_name" 1 "$tmp/no-event.yml:$line:" "$tmp/no-event.yml"
fi

# Usage and cannot run.
run "usage: no argument"        2 "usage"
run "usage: two arguments"      2 "usage" "$real" "$real"
run "cannot run: missing file"  3 "cannot run" "$tmp/absent.yml"

# Every committed fixture dir is in the table.
for d in "$fx"/*/; do
  c="$(basename "$d")"
  if [ -z "${seen[$c]:-}" ]; then
    echo "FAIL $c: fixture dir has no expected exit code in $0" >&2
    failed=$((failed + 1))
  fi
done

if [ "$failed" -gt 0 ]; then
  echo "telegram-guard.test: FAIL: $failed case(s) differ, $passed as expected (T-070)" >&2
  exit 1
fi
echo "telegram-guard.test: ok: $passed cases as expected"
