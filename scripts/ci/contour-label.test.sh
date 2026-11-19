#!/usr/bin/env bash
# T-231 (docs/tasks/T-231.md): host test of the contour's label start. Labelling an Issue `teamwright` reaches
# the dispatcher as a workflow_dispatch hop 0 through a relay job; the title parser is scripts/ci/contour-label.sh.
#
# THE CONTRACT of scripts/ci/contour-label.sh <TITLE> (exactly one argument):
#   The raw bytes of the Issue title (LC_ALL=C) are matched against ^(T-[0-9]{3,}): ; group 1 is the only value
#   printed and the only part of the title that reaches any command.
#     match     stdout "start <T-NNN>", exit 0
#     no match  stdout "bad <why>" (one line, no title bytes), exit 1
#   Exit 2: not exactly one argument; nothing on stdout.
#
# Pins on the real tree (.github/workflows/teamwright-next.yml and its owned head):
#   - the head holds the trigger `issues:` / `types: [labeled]` and the group
#     teamwright-next-${{ inputs.task_id || format('issue-{0}', github.event.issue.number) }};
#   - the job `start` (the relay) runs only for the label `teamwright` on an open Issue set by the owner (one if: line),
#     takes the title only through an env: value line, calls the parser, dispatches hop 0 on $MAIN with task_id and
#     issue, comments on the Issue for a bad title and ends exit 0;
#   - github.event.issue.title occurs only on env: value lines;
#   - the job `next` runs only on workflow_dispatch;
#   - hop 0 with the verdict `own` ends exit 0 ("already running"), no 3-minute wait and no red;
#   - the CODE_COMPLETE|VERIFIED|DONE branch asks git (existence of task/<ID>, rev-list --count origin/$MAIN..)
#     before gh workflow run ci.yml and gh pr create, and ends exit 0 with a "verify locally" message.
#
# Usage: scripts/ci/contour-label.test.sh   (host bash)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
script="$root/scripts/ci/contour-label.sh"
nx="$root/.github/workflows/teamwright-next.yml"
head_="$root/scripts/ci/contour-heads/teamwright-next.yml"
tmp="$(mktemp -d)" || { echo "contour-label.test: cannot run: mktemp failed" >&2; exit 3; }
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
check() { # check <label> <command...>
  local label="$1"
  shift
  if "$@" >/dev/null 2>&1; then pass "$label"; else fail "$label"; fi
}

# ---- the parser ------------------------------------------------------------------------------------------
# run <args...>: sets $out, $err, $got.
run() {
  if [ ! -f "$script" ]; then
    out="" err="no script $script" got=127
    return
  fi
  (cd "$tmp" && bash "$script" "$@") >"$tmp/out" 2>"$tmp/err"
  got=$?
  out="$(cat "$tmp/out")" err="$(cat "$tmp/err")"
}
start() { # start <label> <want id> <title>
  run "$3"
  if [ "$got" = 0 ] && [ "$out" = "start $2" ]; then pass "$1"; else fail "$1" "want exit 0 'start $2'; got exit $got '$out' ($err)"; fi
}
bad() { # bad <label> <title>
  run "$2"
  if [ "$got" = 1 ] && [[ "$out" == "bad "* ]] && [ "$(grep -c . <<<"$out")" = 1 ] && ! grep -q -E '^start( |$)' <<<"$out"; then
    pass "$1"
  else
    fail "$1" "want exit 1 and one line 'bad <why>'; got exit $got '$out' ($err)"
  fi
}

start "title 'T-231: x' starts T-231" T-231 "T-231: x"
start "four digits: T-2310 is T-2310, not T-231" T-2310 "T-2310: x"
start "no space after the colon" T-231 "T-231:x"
start "command substitution in the title: only the id is printed" T-231 'T-231: $(touch pwn)'
start "backticks and quotes in the title: only the id is printed" T-231 'T-231: `touch pwn` "; touch pwn; "'
start "multi-line title: the id of the first line" T-231 $'T-231: a\nb'
bad "two digits is not a task id" "T-23: x"
bad "lower-case t" "t-231: x"
bad "no colon" "T-231 x"
bad "space before the colon" "T-231 : x"
bad "prefix before the id" "Fix T-231: x"
bad "empty title" ""
bad "id only" "T-231"
bad "non-ASCII digits (bytes, LC_ALL=C)" $'T-٢٣١: x'
bad "leading space" " T-231: x"
bad "injection attempt in a bad title" 'x"; touch pwn; "'
run 'x"; touch pwn; "'
[ ! -e "$tmp/pwn" ] && pass "a title is never executed (no file created)" || fail "a title is never executed (no file created)"
run 'T-231: $(touch pwn)'
[ ! -e "$tmp/pwn" ] && pass "a matching title is never executed (no file created)" || fail "a matching title is never executed (no file created)"
run "T-999: secret-canary-title"
if [ "$got" = 0 ] && ! grep -q 'canary' <<<"$out$err"; then pass "no title bytes beyond the id on stdout or stderr"; else fail "no title bytes beyond the id on stdout or stderr" "$out $err"; fi
run "bad-canary-title"
if [ "$got" = 1 ] && ! grep -q 'canary' <<<"$out"; then pass "a bad title is not echoed on stdout"; else fail "a bad title is not echoed on stdout" "$out"; fi
# Wrong argument count: exit 2, nothing on stdout.
for args in "" "a b"; do
  # shellcheck disable=SC2086
  if [ -f "$script" ]; then
    # shellcheck disable=SC2086
    (cd "$tmp" && set -f && bash "$script" $args) >"$tmp/o2" 2>/dev/null
    g=$?
  else g=127; fi
  if [ "$g" = 2 ] && [ ! -s "$tmp/o2" ]; then pass "argument count '${args:-none}': exit 2, no stdout"; else fail "argument count '${args:-none}': exit 2, no stdout" "exit $g"; fi
done

# ---- pins on the real tree -----------------------------------------------------------------------------
if [ ! -f "$nx" ] || [ ! -f "$head_" ]; then
  echo "contour-label.test: cannot run: $nx or $head_ missing" >&2
  exit 3
fi
# job <file> <name>: the lines of one job (from "  <name>:" to the next two-space key), empty when absent.
job() { awk -v n="  $2:" '$0 == n {on=1; print; next} on && /^  [A-Za-z_-]+:/ {exit} on {print}' "$1"; }
start_job="$(job "$nx" start)"
next_job="$(job "$nx" next)"

for f in "$nx" "$head_"; do
  b="${f##*/contour-heads/}"
  [ "$f" = "$nx" ] && b="the workflow"
  check "$b: trigger 'issues:' with 'types: [labeled]' (and nothing else under issues)" \
    bash -c 'awk "/^on:/{on=1;next} on&&/^[a-z]/{exit} on" "$1" | awk "/^  issues:\$/{i=1;next} i&&/^  [a-z_]+:/{exit} i" | grep -x -F "    types: [labeled]" && [ "$(awk "/^on:/{on=1;next} on&&/^[a-z]/{exit} on" "$1" | awk "/^  issues:\$/{i=1;next} i&&/^  [a-z_]+:/{exit} i" | grep -c .)" = 1 ]' _ "$f"
  check "$b: workflow_dispatch stays a trigger" grep -x -F '  workflow_dispatch:' "$f"
  check "$b: group is per task, an issue-number fallback for a label run" \
    grep -x -F "  group: teamwright-next-\${{ inputs.task_id || format('issue-{0}', github.event.issue.number) }}" "$f"
  check "$b: exactly one group line, cancel-in-progress false" \
    bash -c '[ "$(grep -c -E "^  group:" "$1")" = 1 ] && grep -x -F "  cancel-in-progress: false" "$1"' _ "$f"
  check "$b: run-name names the issue for a label run" bash -c 'grep -E "^run-name:" "$1" | grep -F "github.event.issue.number"' _ "$f"
done
check "the real header is the owned head (R14 pair)" bash -c 'diff <(sed -n "1,/^jobs:/p" "$1") "$2"' _ "$nx" "$head_"

# The relay job.
sj() { [ "$1" != -- ] || shift; grep -q -E -e "$1" <<<"$start_job"; }
[ -n "$start_job" ] && pass "job 'start' (the relay) exists" || fail "job 'start' (the relay) exists"
ifline="$(grep -E '^    if:' <<<"$start_job")"
for lit in "github.event_name == 'issues'" "github.event.label.name == 'teamwright'" "github.event.issue.state == 'open'" \
  "github.actor == github.repository_owner"; do
  if [ "$(grep -c . <<<"$ifline")" = 1 ] && grep -q -F "$lit" <<<"$ifline"; then pass "start: if: holds $lit"; else fail "start: if: holds $lit" "$ifline"; fi
done
if grep -q -F '||' <<<"$ifline"; then fail "start: if: has no '||' (every condition is required)" "$ifline"; else pass "start: if: has no '||' (every condition is required)"; fi
sj '^      TITLE: \$\{\{ github\.event\.issue\.title \}\}$' && pass "start: the title is an env: value (TITLE)" || fail "start: the title is an env: value (TITLE)"
sj 'scripts/ci/contour-label\.sh "\$TITLE"' && pass "start: the parser gets the title as \"\$TITLE\"" || fail "start: the parser gets the title as \"\$TITLE\""
sj 'gh workflow run teamwright-next\.yml' && pass "start: dispatches teamwright-next.yml" || fail "start: dispatches teamwright-next.yml"
sj -- '--ref "\$MAIN"' && pass "start: dispatches on \$MAIN" || fail "start: dispatches on \$MAIN"
for fl in 'task_id=' 'issue=' 'hop=0'; do
  sj -- "-f $fl" && pass "start: passes -f $fl" || fail "start: passes -f $fl"
done
sj 'gh issue comment' && pass "start: a bad title gets an Issue comment" || fail "start: a bad title gets an Issue comment"
sj 'exit 0' && pass "start: a bad title ends exit 0 (no red run)" || fail "start: a bad title ends exit 0 (no red run)"
if grep -q -E '(^|[^a-z])claude( |$)' <<<"$start_job"; then fail "start: runs no agent"; else pass "start: runs no agent"; fi
# The title never leaves env: every occurrence in the file is an env value line.
occ="$(grep -n -F 'github.event.issue.title' "$nx")"
if [ -n "$occ" ] && ! grep -v -E '^[0-9]+:      TITLE: \$\{\{ github\.event\.issue\.title \}\}$' <<<"$occ" | grep -q .; then
  pass "github.event.issue.title occurs only on env: value lines"
else
  fail "github.event.issue.title occurs only on env: value lines" "$occ"
fi
check "the title is expanded nowhere in a run: script (no \${{ github.event.issue }} use in the file's run blocks)" \
  bash -c '! grep -E "github\.event\.issue\.(body|title)" "$1" | grep -v -E "^      TITLE: "' _ "$nx"

# The dispatcher job.
nj() { grep -q -E "$1" <<<"$next_job"; }
nif="$(grep -E '^    if:' <<<"$next_job")"
if grep -q -F "github.event_name == 'workflow_dispatch'" <<<"$nif"; then pass "next: if: requires workflow_dispatch"; else fail "next: if: requires workflow_dispatch" "$nif"; fi
if grep -q -F "github.actor == github.repository_owner" <<<"$nif" && grep -q -F "github-actions[bot]" <<<"$nif"; then
  pass "next: the actor condition is kept"
else
  fail "next: the actor condition is kept" "$nif"
fi
# hop 0 + own.
own_lines="$(grep -n -F 'already running' "$nx")"
if [ "$(grep -c . <<<"$own_lines")" -ge 1 ] && grep -q -E 'own\)' "$nx"; then
  ln="$(head -n 1 <<<"$own_lines" | cut -d: -f1)"
  if sed -n "${ln}p" "$nx" | grep -q 'exit 0' && sed -n "$((ln - 4)),${ln}p" "$nx" | grep -q -E 'HOP.*(= *0|-eq 0)|"\$HOP" = 0'; then
    pass "hop 0 with the verdict own: 'already running', exit 0"
  else
    fail "hop 0 with the verdict own: 'already running', exit 0" "$(sed -n "$((ln - 4)),$((ln + 3))p" "$nx")"
  fi
else
  fail "hop 0 with the verdict own: 'already running', exit 0 (text 'already running' absent)"
fi
# CODE_COMPLETE branch: the git questions come before ci.yml and the pull request.
cc="$(awk '/^            CODE_COMPLETE \| VERIFIED \| DONE\)/ {on=1} on {print NR ": " $0} on && /exit 0 ;;/ {exit}' "$nx")"
l_ahead="$(grep -E 'rev-list --count' <<<"$cc" | head -n 1 | cut -d: -f1)"
l_ci="$(grep -E 'gh workflow run ci\.yml' <<<"$cc" | head -n 1 | cut -d: -f1)"
l_pr="$(grep -E 'gh pr create' <<<"$cc" | head -n 1 | cut -d: -f1)"
l_ls="$(grep -E 'ls-remote|FETCH_HEAD|task/\$TASK_ID' <<<"$cc" | head -n 1 | cut -d: -f1)"
if [ -n "$l_ahead" ] && [ -n "$l_ci" ] && [ -n "$l_pr" ] && [ "$l_ahead" -lt "$l_ci" ] && [ "$l_ahead" -lt "$l_pr" ]; then
  pass "CODE_COMPLETE branch: rev-list --count comes before ci.yml and gh pr create"
else
  fail "CODE_COMPLETE branch: rev-list --count comes before ci.yml and gh pr create" "ahead=$l_ahead ci=$l_ci pr=$l_pr"
fi
grep -q -F 'origin/$MAIN..' <<<"$cc" && pass "CODE_COMPLETE branch: counts commits origin/\$MAIN..task" || fail "CODE_COMPLETE branch: counts commits origin/\$MAIN..task"
grep -q -F 'verify locally' <<<"$cc" && pass "CODE_COMPLETE branch: says 'verify locally'" || fail "CODE_COMPLETE branch: says 'verify locally'"
grep -q -F 'nothing to merge' <<<"$cc" && pass "CODE_COMPLETE branch: says 'nothing to merge'" || fail "CODE_COMPLETE branch: says 'nothing to merge'"
if [ "$(grep -c -E '^[0-9]+:[[:space:]]+exit 0' <<<"$cc")" -ge 2 ] || [ "$(grep -c 'exit 0' <<<"$cc")" -ge 2 ]; then
  pass "CODE_COMPLETE branch: the nothing-to-merge end and the normal end both exit 0"
else
  fail "CODE_COMPLETE branch: the nothing-to-merge end and the normal end both exit 0"
fi
[ -n "$l_ls" ] && [ "$l_ls" -lt "${l_ci:-0}" ] && pass "CODE_COMPLETE branch: the existence of task/<ID> is decided before ci.yml" || fail "CODE_COMPLETE branch: the existence of task/<ID> is decided before ci.yml"

# ---- behaviour: the Next stage step run against fake git, gh and sleep ---------------------------------------
# The step's script is cut out of the real file and run in a scratch dir; the fakes log every gh call. The cases
# are the decisions of the step, not text: nothing to merge never opens a pull request or runs ci.yml.
extract() { awk '/^      - name: Next stage/ {on=1} on && /^        run: \|/ {r=1; next} r {print}' "$nx" | sed 's/^          //'; }
extract >"$tmp/step.sh"
if [ ! -s "$tmp/step.sh" ]; then
  fail "behaviour: the Next stage script can be extracted"
else
  # drive <label> <status> <busy> <hop> <prev_stage> <branch: yes|no> <ahead>; sets $bout, $bcalls, $bsum, $bgot.
  drive() {
    local d="$tmp/drive"
    rm -rf "$d"
    mkdir -p "$d/bin" "$d/scripts/ci"
    cp "$root/scripts/ci/contour-busy.sh" "$d/scripts/ci/"
    : >"$d/calls"
    : >"$d/summary"
    cat >"$d/bin/gh" <<'FAKE'
#!/usr/bin/env bash
echo "gh $*" >>"$FAKE_DIR/calls"
if [ "$1 $2" = "run list" ]; then cat "$FAKE_DIR/runlist"; fi
exit 0
FAKE
    cat >"$d/bin/git" <<'FAKE'
#!/usr/bin/env bash
echo "git $*" >>"$FAKE_DIR/calls"
case "$1" in
  ls-remote) [ "$FAKE_BRANCH" = yes ] && exit 0 || exit 2 ;;
  fetch) exit 0 ;;
  show) printf 'status: %s\nstage: implement\n' "$FAKE_STATUS" ;;
  rev-list) [ "$FAKE_AHEAD" = FAIL ] && exit 1; echo "$FAKE_AHEAD" ;;
esac
exit 0
FAKE
    printf '#!/usr/bin/env bash\necho "sleep $*" >>"$FAKE_DIR/calls"\n' >"$d/bin/sleep"
    chmod +x "$d/bin/"*
    if [ "$3" = own ]; then printf '[{"status":"in_progress","displayTitle":"teamwright agent T-777 review (hop 1)"}]' >"$d/runlist"; else echo '[]' >"$d/runlist"; fi
    (cd "$d" && PATH="$d/bin:$PATH" FAKE_DIR="$d" FAKE_STATUS="$2" FAKE_BRANCH="$6" FAKE_AHEAD="$7" TASK_ID=T-777 HOP="$4" \
      PREV_STAGE="$5" PREV_STATUS="" ISSUE="" REPO=example/repo MAIN=main RUN_URL=https://example.com/run GH_TOKEN=fake \
      MAX_HOPS=12 MAX_PARALLEL=5 MODEL=opus AUTH_VAR=subscription TEAMWRIGHT_CONTOUR_PAUSE="" GITHUB_STEP_SUMMARY="$d/summary" \
      bash "$tmp/step.sh") >"$d/out" 2>&1
    bgot=$?
    bcalls="$(cat "$d/calls")" bsum="$(cat "$d/summary")" bout="$(cat "$d/out")"
  }
  has() { grep -q -E -e "$1" <<<"$bcalls"; }
  for st in CODE_COMPLETE VERIFIED DONE; do
    drive x "$st" free 0 "" yes 0
    if [ "$bgot" = 0 ] && ! has 'gh pr create' && ! has 'gh workflow run ci\.yml' && grep -q 'verify locally' <<<"$bsum"; then
      pass "behaviour: $st, task/<ID> not ahead of main: exit 0, 'verify locally', no pull request, no ci.yml run"
    else
      fail "behaviour: $st, task/<ID> not ahead of main: exit 0, 'verify locally', no pull request, no ci.yml run" "exit $bgot; $bcalls; $bsum; $bout"
    fi
  done
  drive x CODE_COMPLETE free 0 "" no 2
  if [ "$bgot" = 0 ] && ! has 'gh pr create' && ! has 'gh workflow run ci\.yml' && grep -q 'verify locally' <<<"$bsum"; then
    pass "behaviour: CODE_COMPLETE with no task/<ID> branch: exit 0, 'verify locally', no pull request, no ci.yml run"
  else
    fail "behaviour: CODE_COMPLETE with no task/<ID> branch: exit 0, 'verify locally', no pull request, no ci.yml run" "exit $bgot; $bcalls; $bsum; $bout"
  fi
  drive x CODE_COMPLETE free 0 "" yes 2
  if [ "$bgot" = 0 ] && has 'gh pr create .*--head task/T-777' && has 'gh workflow run ci\.yml'; then
    pass "behaviour: CODE_COMPLETE, task/<ID> two commits ahead: ci.yml run and pull request as today"
  else
    fail "behaviour: CODE_COMPLETE, task/<ID> two commits ahead: ci.yml run and pull request as today" "exit $bgot; $bcalls; $bout"
  fi
  drive x CODE_COMPLETE free 0 "" yes 2
  if has 'rev-list --count origin/main\.\.' ; then pass "behaviour: the ahead count is origin/\$MAIN..<task head>"; else fail "behaviour: the ahead count is origin/\$MAIN..<task head>" "$bcalls"; fi
  drive x NEEDS_REVIEW free 1 implement yes 3
  if [ "$bgot" = 0 ] && has 'gh workflow run teamwright-agent\.yml .*stage=review'; then
    pass "behaviour: NEEDS_REVIEW still starts the review stage (ahead check is only for the end branch)"
  else
    fail "behaviour: NEEDS_REVIEW still starts the review stage (ahead check is only for the end branch)" "exit $bgot; $bcalls; $bout"
  fi
  # git failing, or answering junk, for the count: treated as nothing ahead (the end branch, no pull request).
  for bad_ahead in FAIL "fatal: bad revision 'origin/main..FETCH_HEAD'"; do
    drive x CODE_COMPLETE free 0 "" yes "$bad_ahead"
    if [ "$bgot" = 0 ] && ! has 'gh pr create' && ! has 'gh workflow run ci\.yml' && grep -q 'verify locally' <<<"$bsum"; then
      pass "behaviour: CODE_COMPLETE, rev-list answers '${bad_ahead:0:5}': nothing to merge, exit 0, no pull request, no ci.yml run"
    else
      fail "behaviour: CODE_COMPLETE, rev-list answers '${bad_ahead:0:5}': nothing to merge, exit 0, no pull request, no ci.yml run" "exit $bgot; $bcalls; $bsum; $bout"
    fi
  done
  # hop 0, an own stage already running: this start does nothing, exit 0, no wait, no stage.
  drive x IN_PROGRESS own 0 "" yes 1
  if [ "$bgot" = 0 ] && grep -q 'already running' <<<"$bsum" && ! has 'sleep' && ! has 'gh workflow run teamwright-agent'; then
    pass "behaviour: hop 0 with an own run: exit 0, 'already running', no sleep, no stage started"
  else
    fail "behaviour: hop 0 with an own run: exit 0, 'already running', no sleep, no stage started" "exit $bgot; $bcalls; $bsum; $bout"
  fi
  # a later hop with an own run still waits (the stage that started it may be ending) and ends red when it stays.
  drive x IN_PROGRESS own 3 review yes 1
  if [ "$bgot" = 1 ] && has 'sleep 10'; then
    pass "behaviour: hop 3 with an own run keeps waiting, then stops red (unchanged)"
  else
    fail "behaviour: hop 3 with an own run keeps waiting, then stops red (unchanged)" "exit $bgot; $bsum; $bout"
  fi
fi

# ---- behaviour: the relay step run against a fake gh ------------------------------------------------------------
# The step "Start the chain of the labelled Issue" of job start is cut out and run with the real parser (or a
# stub with no verdict). Its error branches end red; a bad title comments and ends 0.
rextract() { awk '/^      - name: Start the chain of the labelled Issue/ {on=1} on && /^        run: \|/ {r=1; next} r && ($0 == "" || /^          /) {print; next} r {exit}' "$nx" | sed 's/^          //'; }
rextract >"$tmp/relay.sh"
if [ ! -s "$tmp/relay.sh" ]; then
  fail "behaviour: the relay step script can be extracted"
else
  # relay <label> <title> <gh fails: yes|no> <parser: real|stub>; sets $rgot, $rcalls, $rsum.
  relay() {
    local d="$tmp/relay"
    rm -rf "$d"
    mkdir -p "$d/bin" "$d/scripts/ci"
    : >"$d/calls"
    : >"$d/summary"
    if [ "$4" = real ]; then cp "$root/scripts/ci/contour-label.sh" "$d/scripts/ci/"; else printf '#!/usr/bin/env bash\necho weird\n' >"$d/scripts/ci/contour-label.sh"; fi
    chmod +x "$d/scripts/ci/contour-label.sh"
    cat >"$d/bin/gh" <<'FAKE'
#!/usr/bin/env bash
echo "gh $*" >>"$FAKE_DIR/calls"
if [ "$1 $2" = "workflow run" ] && [ "$FAKE_GH_FAIL" = yes ]; then exit 1; fi
exit 0
FAKE
    chmod +x "$d/bin/gh"
    (cd "$d" && PATH="$d/bin:$PATH" FAKE_DIR="$d" FAKE_GH_FAIL="$3" TITLE="$2" ISSUE=16 REPO=example/repo MAIN=main \
      RUN_URL=https://example.com/run GH_TOKEN=fake GITHUB_STEP_SUMMARY="$d/summary" bash "$tmp/relay.sh") >"$d/out" 2>&1
    rgot=$?
    rcalls="$(cat "$d/calls")" rsum="$(cat "$d/summary")"
  }
  relay x "T-231: x" no real
  if [ "$rgot" = 0 ] && grep -q -E 'gh workflow run teamwright-next\.yml .*task_id=T-231 .*hop=0' <<<"$rcalls"; then
    pass "behaviour: relay, good title: hop 0 dispatched, exit 0"
  else
    fail "behaviour: relay, good title: hop 0 dispatched, exit 0" "exit $rgot; $rcalls; $(cat "$tmp/relay/out")"
  fi
  relay x "T-231: x" yes real
  if [ "$rgot" = 1 ]; then pass "behaviour: relay, hop 0 dispatch fails: exit 1"; else fail "behaviour: relay, hop 0 dispatch fails: exit 1" "exit $rgot; $rcalls"; fi
  relay x "T-231: x" no stub
  if [ "$rgot" = 1 ] && ! grep -q 'gh workflow run' <<<"$rcalls"; then pass "behaviour: relay, parser gives no verdict: exit 1, nothing dispatched"; else fail "behaviour: relay, parser gives no verdict: exit 1, nothing dispatched" "exit $rgot; $rcalls"; fi
  relay x "no id here" no real
  if [ "$rgot" = 0 ] && grep -q 'gh issue comment' <<<"$rcalls" && ! grep -q 'gh workflow run' <<<"$rcalls"; then
    pass "behaviour: relay, bad title: comment on the Issue, nothing dispatched, exit 0"
  else
    fail "behaviour: relay, bad title: comment on the Issue, nothing dispatched, exit 0" "exit $rgot; $rcalls"
  fi
fi

check "Makefile: check-contour-label runs this test and is part of check" \
  bash -c 'grep -q -x "check-contour-label:" "$1" && grep -A1 -x "check-contour-label:" "$1" | grep -q "scripts/ci/contour-label.test.sh" && grep -E "^check:" "$1" | grep -q "check-contour-label"' _ "$root/Makefile"

echo "contour-label.test: $passed passed, $failed failed"
[ "$failed" = 0 ]
