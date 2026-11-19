#!/usr/bin/env bash
# T-216 (decisions #292, #293, #295, #296; docs/tasks/T-216.md): host test of the contour dispatcher's busy
# count, scripts/ci/contour-busy.sh <TASK_ID> <LIMIT>, and of the pins that make it the only decision.
#
# THE CONTRACT (every line below is pinned by a case):
#
#   Input. stdin is the JSON of `gh run list --workflow teamwright-agent.yml --json status,displayTitle`:
#   an array of objects. A run is unfinished when its status is not "completed" (queued, waiting,
#   pending, requested, in_progress all hold or will hold a runner). Its task is the id in the title
#   ^teamwright agent (T-[0-9]{3,}) (the run-name of teamwright-agent.yml), compared as a whole word,
#   so T-2160 is not T-216. A title that does not parse counts as one more other task (fail-closed).
#   Output: one line on stdout, exit 0:
#     own        an unfinished run of <TASK_ID> exists (own wins over any count)
#     full <n>   no own run; n distinct other tasks (plus unparsable runs) unfinished, n >= LIMIT
#     free <n>   no own run; n < LIMIT
#   Exit 2: bad arguments (not exactly two, <TASK_ID> not ^T-[0-9]{3,}$, <LIMIT> not a positive
#   integer); exit 3: stdin not a JSON array of objects (empty input included). Neither prints a
#   verdict word on stdout.
#
#   Pins on the real tree: teamwright-next.yml pipes `gh run list` into
#   scripts/ci/contour-busy.sh "$TASK_ID" "$MAX_PARALLEL" and stops on `full` with an Issue comment
#   naming the limit; MAX_PARALLEL is assigned exactly once in .github/workflows, as a positive
#   integer in teamwright-next.yml (the value itself is not pinned: raising it is one line); the old
#   inline "one task at a time" check is gone; both owned heads hold their per-task concurrency group;
#   the agent's run-name is the title the script parses.
#
# Usage: scripts/ci/contour-busy.test.sh   (host bash, jq)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
script="$root/scripts/ci/contour-busy.sh"
wf="$root/.github/workflows"
chd="$root/scripts/ci/contour-heads"
command -v jq >/dev/null 2>&1 || { echo "contour-busy.test: cannot run: jq missing" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "contour-busy.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

failed=0
passed=0
fail() {
  echo "FAIL $1" >&2
  [ -z "${2:-}" ] || sed 's/^/       | /' <<<"$2" >&2
  failed=$((failed + 1))
}
pass() {
  echo "ok   $1"
  passed=$((passed + 1))
}

# run <stdin> <args...>: runs the script; sets $out (stdout), $err (stderr), $got (exit code).
run() {
  local in="$1"
  shift
  if [ ! -f "$script" ]; then
    out="" err="no script $script" got=127
    return
  fi
  printf '%s' "$in" | bash "$script" "$@" >"$tmp/out" 2>"$tmp/err"
  got=$?
  out="$(cat "$tmp/out")" err="$(cat "$tmp/err")"
}
# want <label> <exit> <stdout> <stdin> <args...>
want() {
  local label="$1" ex="$2" so="$3" in="$4"
  shift 4
  run "$in" "$@"
  if [ "$got" = "$ex" ] && [ "$out" = "$so" ]; then
    pass "$label"
  else
    fail "$label" "want exit $ex stdout '$so'; got exit $got stdout '$out' stderr '$err'"
  fi
}
# refused <label> <exit> <stdin> <args...>: that exit, and no verdict word on stdout.
refused() {
  local label="$1" ex="$2" in="$3"
  shift 3
  run "$in" "$@"
  if [ "$got" = "$ex" ] && ! grep -q -E '^(own|full|free)' <<<"$out"; then
    pass "$label"
  else
    fail "$label" "want exit $ex and no verdict; got exit $got stdout '$out' stderr '$err'"
  fi
}

# r <task> <status> [stage]: one run object of teamwright-agent.yml (its run-name).
r() { printf '{"status":"%s","displayTitle":"teamwright agent %s %s (hop 1)"}' "$2" "$1" "${3:-implement}"; }
# raw <status> <title>: a run with a title of any shape.
raw() { printf '{"status":"%s","displayTitle":"%s"}' "$1" "$2"; }
arr() { local IFS=,; printf '[%s]' "$*"; }

# ---- The count -------------------------------------------------------------------------------------
want "empty list, limit 2: free 0" 0 "free 0" "[]" T-216 2
want "one other task unfinished, limit 2: free 1" 0 "free 1" "$(arr "$(r T-301 in_progress)")" T-216 2
want "two distinct other tasks unfinished, limit 2: full 2" 0 "full 2" \
  "$(arr "$(r T-301 in_progress)" "$(r T-302 queued)")" T-216 2
want "three runs of one other task, limit 2: free 1 (tasks are counted, not runs)" 0 "free 1" \
  "$(arr "$(r T-301 in_progress analyze)" "$(r T-301 queued review)" "$(r T-301 pending red-tests)")" T-216 2
four="$(arr "$(r T-301 in_progress)" "$(r T-302 queued)" "$(r T-303 waiting)" "$(r T-304 requested)")"
want "four other tasks, limit 5: free 4" 0 "free 4" "$four" T-216 5
want "the same four other tasks, limit 2: full 4 (the limit is the only knob)" 0 "full 4" "$four" T-216 2
want "the same four other tasks, limit 4: full 4 (n equal to the limit is full)" 0 "full 4" "$four" T-216 4
want "completed runs are ignored" 0 "free 0" \
  "$(arr "$(r T-301 completed)" "$(r T-302 completed)" "$(r T-303 completed)")" T-216 2
want "completed and unfinished runs: only the unfinished task counts" 0 "free 1" \
  "$(arr "$(r T-301 completed)" "$(r T-302 completed)" "$(r T-303 in_progress)")" T-216 2
for st in queued waiting pending requested in_progress; do
  want "status $st counts as unfinished" 0 "full 1" "$(arr "$(r T-301 "$st")")" T-216 1
done
want "own run unfinished, no other: own" 0 "own" "$(arr "$(r T-216 in_progress)")" T-216 2
want "own wins over a full count" 0 "own" \
  "$(arr "$(r T-301 in_progress)" "$(r T-216 queued)" "$(r T-302 in_progress)")" T-216 2
want "own completed, others unfinished: counted, not own" 0 "full 2" \
  "$(arr "$(r T-216 completed)" "$(r T-301 in_progress)" "$(r T-302 in_progress)")" T-216 2
want "T-2160 running is another task for T-216" 0 "free 1" "$(arr "$(r T-2160 in_progress)")" T-216 2
want "T-216 running is another task for T-2160" 0 "free 1" "$(arr "$(r T-216 in_progress)")" T-2160 2
want "T-21 running is another task for T-216" 0 "free 1" "$(arr "$(r T-21 in_progress)")" T-216 2
want "an unparsable title counts as another task" 0 "full 2" \
  "$(arr "$(raw in_progress "something else")" "$(r T-301 in_progress)")" T-216 2
want "two unparsable titles count as two other tasks" 0 "full 2" \
  "$(arr "$(raw in_progress "x")" "$(raw queued "teamwright agent")")" T-216 2
want "a title naming the task after the prefix shape is unparsable, not own" 0 "free 1" \
  "$(arr "$(raw in_progress "agent T-216 implement")")" T-216 2
want "a missing title counts as another task" 0 "free 1" '[{"status":"in_progress"}]' T-216 2
want "a non-string title counts as another task" 0 "free 1" '[{"status":"queued","displayTitle":7}]' T-216 2
want "an unparsable completed run is ignored" 0 "free 0" "$(arr "$(raw completed "x")")" T-216 2

# ---- Bad input: exit 3 -----------------------------------------------------------------------------
refused "not JSON: exit 3" 3 "not json" T-216 2
refused "empty input: exit 3" 3 "" T-216 2
refused "a JSON object, not an array: exit 3" 3 '{"status":"queued"}' T-216 2
refused "an array holding a non-object: exit 3" 3 '["teamwright agent T-301 x"]' T-216 2
refused "truncated JSON: exit 3" 3 '[{"status":"queued"' T-216 2
refused "a run without a status: exit 3" 3 '[{"displayTitle":"teamwright agent T-301 x (hop 1)"}]' T-216 2

# ---- Bad arguments: exit 2 -------------------------------------------------------------------------
for lim in 0 -1 abc "" 2x " 2" 02 1.5; do
  refused "bad limit '$lim': exit 2" 2 "[]" T-216 "$lim"
done
refused "no limit: exit 2" 2 "[]" T-216
refused "no arguments: exit 2" 2 "[]"
refused "a third argument: exit 2" 2 "[]" T-216 2 x
for id in T-21 216 t-216 "T-216 " "" T-216x; do
  refused "bad task id '$id': exit 2" 2 "[]" "$id" 2
done

# ---- Pins on the real tree -------------------------------------------------------------------------
nx="$wf/teamwright-next.yml"
ag="$wf/teamwright-agent.yml"
if grep -q -F 'contour-busy.sh "$TASK_ID" "$MAX_PARALLEL"' "$nx" 2>/dev/null; then
  pass "teamwright-next.yml calls scripts/ci/contour-busy.sh \"\$TASK_ID\" \"\$MAX_PARALLEL\""
else
  fail "teamwright-next.yml calls scripts/ci/contour-busy.sh \"\$TASK_ID\" \"\$MAX_PARALLEL\""
fi
if grep -q -E '\| scripts/ci/contour-busy\.sh "\$TASK_ID" "\$MAX_PARALLEL"\)"' "$nx" 2>/dev/null &&
  grep -q -F 'gh run list -R "$REPO" --workflow teamwright-agent.yml --limit 50 --json status,displayTitle | scripts/ci/contour-busy.sh' "$nx"; then
  pass "teamwright-next.yml feeds gh run list of teamwright-agent.yml into the script"
else
  fail "teamwright-next.yml feeds gh run list of teamwright-agent.yml into the script"
fi
if grep -q -E '^ +"full "\*\) say "[^"]*limit[^"]*\$MAX_PARALLEL[^"]*"; exit 0 ;;$' "$nx" 2>/dev/null; then
  pass "teamwright-next.yml: full stops with a comment naming the limit, exit 0"
else
  fail "teamwright-next.yml: full stops with a comment naming the limit, exit 0"
fi
if [ -f "$nx" ] && ! grep -q -i -E 'one task at a time|\*other\*' "$nx"; then
  pass "teamwright-next.yml: the inline one-task-at-a-time check is gone"
else
  fail "teamwright-next.yml: the inline one-task-at-a-time check is gone"
fi
# Any assignment of MAX_PARALLEL (a YAML key, a shell assignment, a GITHUB_ENV line) in any workflow file.
defs="$(grep -r -n -E '(^|[^A-Za-z0-9_$])MAX_PARALLEL[[:space:]]*[:=]' "$wf" 2>/dev/null)"
if [ "$(grep -c . <<<"$defs")" = 1 ] && grep -q -E "^$nx:[0-9]+:      MAX_PARALLEL: \"[1-9][0-9]*\"\$" <<<"$defs"; then
  pass "MAX_PARALLEL is set exactly once in .github/workflows, a positive integer in the job env of teamwright-next.yml"
else
  fail "MAX_PARALLEL is set exactly once in .github/workflows, a positive integer in the job env of teamwright-next.yml" "$defs"
fi
# T-231: the dispatcher's group falls back to the Issue number for a label run (the relay job dispatches hop 0, so
# every hop of a task is in teamwright-next-<ID>); the agent's group is unchanged.
for p in "teamwright-agent.yml:teamwright-agent:\${{ inputs.task_id }}" \
  "teamwright-next.yml:teamwright-next:\${{ inputs.task_id || format('issue-{0}', github.event.issue.number) }}"; do
  cn="${p%%:*}"
  rest="${p#*:}" g="${rest%%:*}" expr="${rest#*:}"
  h="$chd/$cn"
  if [ -f "$h" ] && [ "$(grep -c -x -F "  group: $g-$expr" "$h")" = 1 ] &&
    [ "$(grep -c -E '^  group:' "$h")" = 1 ] && grep -q -x -F '  cancel-in-progress: false' "$h"; then
    pass "owned head $cn holds the per-task group $g-$expr, cancel-in-progress false"
  else
    fail "owned head $cn holds the per-task group $g-$expr, cancel-in-progress false"
  fi
done
if grep -q -x -F 'run-name: "teamwright agent ${{ inputs.task_id }} ${{ inputs.stage }} (hop ${{ inputs.hop }})"' "$ag" 2>/dev/null; then
  pass "teamwright-agent.yml's run-name is the title the script parses"
else
  fail "teamwright-agent.yml's run-name is the title the script parses"
fi

echo "contour-busy.test: $passed passed, $failed failed"
[ "$failed" -eq 0 ] || exit 1
