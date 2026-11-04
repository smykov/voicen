#!/usr/bin/env bash
# T-199 (decisions #228, #241; docs/tasks/T-199.md): host test of the only CI read of a stage session,
# scripts/tw-wip-run [<commit>].
#
# THE CONTRACT THE DEVELOPER IMPLEMENTS (every line below is pinned by a case):
#
#   Branch. Run in a git working tree on branch tw/<ID>; <ID> comes from the branch, never from an
#   argument or the environment. <commit> (default HEAD) is the pushed commit, resolved with git to a
#   full sha. At most one argument: no option, no refspec, no empty word, no second word, a commit git
#   knows (the same shape as scripts/tw-wip-push [<commit>]).
#
#   gh argv. The program runs no gh command but these two templates, byte-exact (decides_on: literal):
#     LIST  run list --branch wip/<ID> --workflow ci.yml --json databaseId,headSha,status,conclusion,url --limit 20
#     VIEW  run view <databaseId> --json jobs,status,conclusion,headSha,url
#   LIST answers a JSON array, newest first; the run of the commit is the first element whose headSha
#   equals the full sha (a run of another sha is never taken). VIEW is called only for a completed run
#   that is not green, with the databaseId of that run; its "jobs" array holds {name, conclusion}.
#
#   Wait. TW_WIP_RUN_WINDOW seconds (whole seconds) (default 540: under the 10-minute Bash limit) is the time one call
#   waits; TW_WIP_RUN_POLL seconds (default 30; a fraction is allowed) is the time between two LISTs.
#   Within the window the program lists again while there is no run for the sha or the run is not
#   completed; a completed run ends the wait at once.
#
#   Exit codes and output:
#     0  completed, conclusion success. stdout holds the line  CI: <url> @ <full sha>
#     1  completed, conclusion not success (failure, cancelled, timed_out, ...). stdout holds one line
#        FAILED JOB: <name>  for each job whose conclusion is failure, cancelled, timed_out or
#        startup_failure, and no job that did not fail; stdout holds no line that starts with  CI:
#     2  refused (branch, argument, pause, not a commit, not a git tree); gh is never called
#     3  the window ended with the run of the sha not completed (queued, in_progress, ...); the output
#        holds the run's url, and no line that starts with  CI:
#     4  the window ended with no run for the sha; the output holds the full sha
#     5  gh failed (non-zero exit) on either call, or its answer is not the JSON shape above
#   TEAMWRIGHT_PAUSE (the tree's root, the main tree's root, $TEAMWRIGHT_PAUSE_FILE) refuses with 2.
#
# Safety: the script under test is COPIED into a temporary git repository (no remote) and run from
# there; `gh` is a fake first on PATH that records its argv and prints canned JSON; nothing reaches
# the network and the real gh is never run.
#
# Outside the contract: the wording of messages, stderr versus stdout for refusals, how often the
# program lists beyond what each case pins.
#
# Usage: scripts/ci/tw-wip-run.test.sh   (host bash, git)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
script="$root/scripts/tw-wip-run"
for t in git timeout; do
  command -v "$t" >/dev/null 2>&1 || { echo "tw-wip-run.test: cannot run: $t missing" >&2; exit 3; }
done
tmp="$(mktemp -d)" || { echo "tw-wip-run.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_PREFIX TEAMWRIGHT_PAUSE_FILE
unset TW_WIP_RUN_POLL TW_WIP_RUN_WINDOW GH_REPO GH_TOKEN
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=fixture GIT_AUTHOR_EMAIL=fixture@example.com
export GIT_COMMITTER_NAME=fixture GIT_COMMITTER_EMAIL=fixture@example.com
export GIT_TERMINAL_PROMPT=0

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

# ---- The fake gh: records its argv (one line per call), answers from files -----------------------
# $FAKE_GH_DIR/list.<n>.json and view.<id>.<n>.json: the n-th call of that kind prints file n, or the
# last one below n. $FAKE_GH_DIR/fail.list and fail.view: that kind exits 1 (once per call, always).
mkdir -p "$tmp/fake"
cat >"$tmp/fake/gh" <<'FAKE'
#!/usr/bin/env bash
d="${FAKE_GH_DIR:?}"
printf '%s\n' "$*" >>"$d/calls"
case "$1 $2" in
  "run list") pre=list ;;
  "run view") pre="view.$3" ;;
  *) echo "fake gh: unexpected command: $*" >&2; exit 64 ;;
esac
kind="${pre%%.*}"
if [ -e "$d/fail.$kind" ]; then echo "gh: simulated failure" >&2; exit 1; fi
n=$(( $(cat "$d/n.$pre" 2>/dev/null || echo 0) + 1 ))
echo "$n" >"$d/n.$pre"
while [ "$n" -ge 1 ]; do
  if [ -f "$d/$pre.$n.json" ]; then cat "$d/$pre.$n.json"; exit 0; fi
  n=$((n - 1))
done
echo "fake gh: no canned answer for $pre" >&2
exit 65
FAKE
chmod +x "$tmp/fake/gh"

# repo <case> [branch]: a temporary repository on the branch, two commits; sets $repo $ghd $bin $sha
# (HEAD) $sha1 (HEAD~1) $wipid. The script is copied in; a missing script stays missing.
repo() {
  local d="$tmp/$1" br="${2:-tw/T-999}"
  mkdir -p "$d/bin" "$d/gh" "$d/repo/scripts"
  repo="$d/repo"
  ghd="$d/gh"
  bin="$d/bin"
  wipid="${br#tw/}"
  cp -p "$tmp/fake/gh" "$bin/gh"
  git init -q -b main "$repo"
  printf 'one\n' >"$repo/file"
  git -C "$repo" add file
  git -C "$repo" commit -q -m "chore: one"
  git -C "$repo" switch -q -c "$br"
  printf 'two\n' >"$repo/file"
  git -C "$repo" commit -q -am "T-999 two"
  sha="$(git -C "$repo" rev-parse HEAD)"
  sha1="$(git -C "$repo" rev-parse HEAD~1)"
  [ ! -e "$script" ] || cp -p "$script" "$repo/scripts/tw-wip-run"
}

# obj <id> <sha> <status> <conclusion>: one run object. list <obj>...: the array.
obj() {
  printf '{"databaseId":%s,"headSha":"%s","status":"%s","conclusion":"%s","url":"https://example.com/runs/%s"}' \
    "$1" "$2" "$3" "$4" "$1"
}
list() {
  local out="[" sep=""
  for o in "$@"; do out="$out$sep$o"; sep=","; done
  printf '%s]' "$out"
}
put_list() { printf '%s' "$2" >"$ghd/list.$1.json"; } # <n> <json>
put_view() { printf '%s' "$3" >"$ghd/view.$1.$2.json"; } # <id> <n> <json>

# run_it [args...]: runs the copied script in $repo with the fake gh first on PATH, a short window.
# Sets $out (stdout), $err (stderr), $got.
run_it() {
  out="$(cd "$repo" && PATH="$bin:$PATH" FAKE_GH_DIR="$ghd" TW_WIP_RUN_POLL="${POLL:-0.1}" \
    TW_WIP_RUN_WINDOW="${WINDOW:-2}" timeout 30 scripts/tw-wip-run "$@" 2>"$tmp/err" </dev/null)"
  got=$?
  err="$(cat "$tmp/err")"
}

bad=""
need() { [ "$2" = "$3" ] || bad="$bad${bad:+; }$1: got [$2] want [$3]"; } # <what> <got> <want>
verdict() {
  if [ -z "$bad" ]; then pass "$1"; else fail "$1" "$bad"$'\n'"stdout: $out"$'\n'"stderr: $err"; fi
  bad=""
}
has_line() { grep -qxF -- "$1" <<<"$out"; }
all_text() { printf '%s\n%s\n' "$out" "$err"; }
ncalls() { # <prefix>: lines of the calls file starting with it
  local n
  n="$({ grep -c -- "^$1" "$ghd/calls" 2>/dev/null || :; })"
  echo "${n:-0}"
}

# argv_ok: every recorded gh call is one of the two owned templates (byte-exact, decides_on: literal).
argv_ok() {
  if [ ! -f "$ghd/calls" ]; then bad="$bad${bad:+; }gh was never called"; return; fi
  local line stray=""
  local tmpl_list="run list --branch wip/$wipid --workflow ci.yml --json databaseId,headSha,status,conclusion,url --limit 20"
  while IFS= read -r line; do
    [ "$line" = "$tmpl_list" ] && continue
    [[ "$line" =~ ^run\ view\ [0-9]+\ --json\ jobs,status,conclusion,headSha,url$ ]] && continue
    stray="$stray[$line] "
  done <"$ghd/calls"
  [ -z "$stray" ] || bad="$bad${bad:+; }gh argv outside the two templates: $stray"
}
no_ci_line() { # stdout has no line that starts with "CI:"
  if grep -q '^CI:' <<<"$out"; then bad="$bad${bad:+; }a CI: line was printed"; fi
}

# ==== (2) green: the run of the pushed sha, not another one ======================================
repo g-green
put_list 1 "$(list "$(obj 201 "$sha1" completed success)" "$(obj 202 "$sha" completed success)")"
run_it
need "exit" "$got" 0
has_line "CI: https://example.com/runs/202 @ $sha" || bad="$bad${bad:+; }no line 'CI: <url> @ <sha>' for the run of HEAD"
argv_ok
need "view calls on a green run" "$(ncalls 'run view')" 0
verdict "green: completed success for HEAD prints CI: <url> @ <sha>, exit 0, only the list template"

# The ID in the argv comes from the branch.
repo g-green-other-id tw/T-998
put_list 1 "$(list "$(obj 301 "$sha" completed success)")"
run_it
need "exit" "$got" 0
argv_ok
verdict "argv names wip/<ID> of the current branch (tw/T-998 -> wip/T-998)"

# An explicit commit selects that commit's run, as the full sha.
repo g-arg-commit
put_list 1 "$(list "$(obj 302 "$sha" in_progress "")" "$(obj 301 "$sha1" completed success)")"
run_it HEAD~1
need "exit" "$got" 0
has_line "CI: https://example.com/runs/301 @ $sha1" || bad="$bad${bad:+; }no CI: line for the explicit commit"
argv_ok
verdict "green: an explicit commit argument selects the run of that commit"

# ==== (3) red: exit 1, the failed jobs named, no CI: line ==========================================
repo r-red
put_list 1 "$(list "$(obj 201 "$sha1" completed success)" "$(obj 202 "$sha" completed failure)")"
put_view 202 1 '{"status":"completed","conclusion":"failure","headSha":"'"$sha"'","url":"https://example.com/runs/202","jobs":[{"name":"windows","status":"completed","conclusion":"failure","steps":[{"name":"Build","conclusion":"failure"}]},{"name":"core-linux","status":"completed","conclusion":"failure"},{"name":"lint","status":"completed","conclusion":"success"},{"name":"smoke","status":"completed","conclusion":"skipped"},{"name":"msi","status":"completed","conclusion":"timed_out"},{"name":"setup","status":"completed","conclusion":"startup_failure"}]}'
run_it
need "exit" "$got" 1
has_line "FAILED JOB: windows" || bad="$bad${bad:+; }failed job windows not named"
has_line "FAILED JOB: core-linux" || bad="$bad${bad:+; }failed job core-linux not named"
has_line "FAILED JOB: msi" || bad="$bad${bad:+; }timed_out job msi not named"
has_line "FAILED JOB: setup" || bad="$bad${bad:+; }startup_failure job setup not named"
has_line "FAILED JOB: lint" && bad="$bad${bad:+; }a green job named as failed"
has_line "FAILED JOB: smoke" && bad="$bad${bad:+; }a skipped job named as failed"
no_ci_line
argv_ok
grep -qxF "run view 202 --json jobs,status,conclusion,headSha,url" "$ghd/calls" 2>/dev/null \
  || bad="$bad${bad:+; }no 'run view 202 --json jobs,status,conclusion,headSha,url' (the run of HEAD)"
need "exit differs from green and pending" "$([ "$got" -ne 0 ] && [ "$got" -ne 3 ] && echo ok)" ok
verdict "red: completed failure exits 1 and names each failed job, none other, view of the run of HEAD"

repo r-cancelled
put_list 1 "$(list "$(obj 202 "$sha" completed cancelled)")"
put_view 202 1 '{"status":"completed","conclusion":"cancelled","headSha":"'"$sha"'","url":"https://example.com/runs/202","jobs":[{"name":"windows","conclusion":"cancelled"},{"name":"lint","conclusion":"success"}]}'
run_it
need "exit" "$got" 1
has_line "FAILED JOB: windows" || bad="$bad${bad:+; }cancelled job windows not named"
no_ci_line
argv_ok
verdict "red: a cancelled run is red (exit 1), not green"

# ==== (4) in_progress then success: waits, no early exit ===========================================
repo w-wait
WINDOW=8 # a long window: the run succeeds inside it, so the case ends early
put_list 1 "$(list "$(obj 202 "$sha" in_progress "")")"
put_list 2 "$(list "$(obj 202 "$sha" in_progress "")")"
put_list 3 "$(list "$(obj 202 "$sha" completed success)")"
run_it
need "exit" "$got" 0
has_line "CI: https://example.com/runs/202 @ $sha" || bad="$bad${bad:+; }no CI: line after the wait"
need "list calls" "$(ncalls 'run list')" 3
argv_ok
verdict "wait: in_progress, in_progress, success -> three lists, exit 0 with the CI: line"
unset WINDOW

# The run of the commit appears late (the push has just happened).
repo w-late-run
WINDOW=8 # a long window: the run succeeds inside it, so the case ends early
put_list 1 "[]"
put_list 2 "$(list "$(obj 201 "$sha1" completed success)")"
put_list 3 "$(list "$(obj 202 "$sha" queued "")" "$(obj 201 "$sha1" completed success)")"
put_list 4 "$(list "$(obj 202 "$sha" completed success)" "$(obj 201 "$sha1" completed success)")"
run_it
need "exit" "$got" 0
has_line "CI: https://example.com/runs/202 @ $sha" || bad="$bad${bad:+; }no CI: line for the late run"
argv_ok
verdict "wait: no run, then only another sha's run, then queued, then success -> exit 0"
unset WINDOW

# ==== (5) after the window: pending (3) and no run (4) are two distinct results ==================
repo p-pending
put_list 1 "$(list "$(obj 202 "$sha" in_progress "")")"
run_it
need "exit" "$got" 3
all_text | grep -qF "https://example.com/runs/202" || bad="$bad${bad:+; }the pending output does not name the run"
no_ci_line
[ "$(ncalls 'run list')" -ge 2 ] || bad="$bad${bad:+; }it did not poll again inside the window"
argv_ok
verdict "pending: the run is in_progress at the window's end -> exit 3 naming the run, polled more than once"

repo p-queued
put_list 1 "$(list "$(obj 202 "$sha" queued "")")"
run_it
need "exit" "$got" 3
no_ci_line
argv_ok
verdict "pending: a queued run is pending (exit 3), not green and not a missing run"

repo n-no-run
put_list 1 "[]"
run_it
need "exit" "$got" 4
all_text | grep -qF "$sha" || bad="$bad${bad:+; }the output does not name the sha"
no_ci_line
[ "$(ncalls 'run list')" -ge 2 ] || bad="$bad${bad:+; }it did not poll again inside the window"
argv_ok
verdict "no run: an empty list for the whole window -> exit 4 naming the sha (the paths-ignore case)"

repo n-other-sha
put_list 1 "$(list "$(obj 201 "$sha1" completed success)")"
run_it
need "exit" "$got" 4
no_ci_line
argv_ok
verdict "no run: a green run of another sha is not the run of HEAD -> exit 4, no CI: line"

# ==== gh failed or answered something else: exit 5, never a result ================================
repo e-list-fails
: >"$ghd/fail.list"
run_it
need "exit" "$got" 5
no_ci_line
verdict "gh failure: run list exits non-zero -> exit 5"

repo e-list-garbage
put_list 1 'this is not json'
run_it
need "exit" "$got" 5
no_ci_line
verdict "gh failure: run list prints no JSON -> exit 5"

repo e-list-not-array
put_list 1 '{"databaseId":202}'
run_it
need "exit" "$got" 5
no_ci_line
verdict "gh failure: run list prints an object, not an array -> exit 5"

repo e-view-fails
put_list 1 "$(list "$(obj 202 "$sha" completed failure)")"
: >"$ghd/fail.view"
run_it
need "exit" "$got" 5
no_ci_line
has_line "FAILED JOB: windows" && bad="$bad${bad:+; }jobs named without a view answer"
verdict "gh failure: run view exits non-zero on a red run -> exit 5, not a red result without jobs"

repo e-view-garbage
put_list 1 "$(list "$(obj 202 "$sha" completed failure)")"
put_view 202 1 'not json'
run_it
need "exit" "$got" 5
no_ci_line
verdict "gh failure: run view prints no JSON on a red run -> exit 5"

# A run id that is not a non-negative integer is not the JSON shape: no run view with it.
repo e-list-bad-id
put_list 1 "$(list "$(obj -1 "$sha" completed failure)")"
run_it
need "exit" "$got" 5
no_ci_line
need "view calls with a bad run id" "$(ncalls 'run view')" 0
verdict "gh failure: run list answers a negative databaseId -> exit 5, no run view"

# ==== (6) refusals: exit 2, gh never called =======================================================
# Each fixture holds a green answer, so a program that went on would print a CI: line.
refused() { # <name> then the arguments of the run
  local c="$1"
  shift
  put_list 1 "$(list "$(obj 202 "$sha" completed success)")"
  run_it "$@"
  need "exit" "$got" 2
  [ ! -e "$ghd/calls" ] || bad="$bad${bad:+; }gh was called: $(cat "$ghd/calls")"
  no_ci_line
  verdict "$c"
}

repo f-main
git -C "$repo" switch -q main
refused "refused: on main"

repo f-feature feature/T-999
refused "refused: on feature/T-999"

repo f-wip wip/T-999
refused "refused: on wip/T-999"

repo f-not-an-id tw/not-an-id
refused "refused: on tw/not-an-id"

repo f-prefix xtw/T-999
refused "refused: on xtw/T-999"

repo f-detached
git -C "$repo" switch -q --detach HEAD
refused "refused: detached HEAD"

repo f-two-args
refused "refused: two arguments" HEAD HEAD~1
repo f-opt-watch
refused "refused: option --watch" --watch
repo f-opt-repo
refused "refused: option --repo=other/repo" --repo=other/repo
repo f-opt-branch
refused "refused: option --branch main" --branch main
repo f-opt-json
refused "refused: option --json" --json
repo f-opt-short
refused "refused: option -R" -R
repo f-opt-after-commit
refused "refused: HEAD --watch" HEAD --watch
repo f-double-dash
refused "refused: --" --
repo f-empty-arg
refused "refused: an empty argument" ""
repo f-refspec
refused "refused: a refspec HEAD:refs/heads/main" HEAD:refs/heads/main
repo f-not-a-commit
refused "refused: a revision git does not know" no-such-revision
repo f-word
refused "refused: a run id as the argument (a word, not a commit)" 12345678

repo f-pause-root
touch "$repo/TEAMWRIGHT_PAUSE"
refused "refused: TEAMWRIGHT_PAUSE at the tree's root"

repo f-pause-file
touch "$tmp/f-pause-file/pause"
TEAMWRIGHT_PAUSE_FILE="$tmp/f-pause-file/pause" refused "refused: TEAMWRIGHT_PAUSE_FILE exists"

repo f-pause-main-tree
git -C "$repo" switch -q main
git -C "$repo" worktree add -q "$tmp/f-pause-main-tree/wt" tw/T-999
mkdir -p "$tmp/f-pause-main-tree/wt/scripts"
[ ! -e "$script" ] || cp -p "$script" "$tmp/f-pause-main-tree/wt/scripts/tw-wip-run"
touch "$repo/TEAMWRIGHT_PAUSE"
repo_main="$repo"
repo="$tmp/f-pause-main-tree/wt"
refused "refused: TEAMWRIGHT_PAUSE in the main tree, run from a linked worktree"
repo="$repo_main"

# Not in a git tree at all.
repo f-no-git
mkdir -p "$tmp/f-no-git/plain"
put_list 1 "$(list "$(obj 202 "$sha" completed success)")"
out="$(cd "$tmp/f-no-git/plain" && PATH="$bin:$PATH" FAKE_GH_DIR="$ghd" TW_WIP_RUN_POLL=0.1 TW_WIP_RUN_WINDOW=2 \
  GIT_CEILING_DIRECTORIES="$tmp" timeout 30 "$repo/scripts/tw-wip-run" 2>"$tmp/err" </dev/null)"
got=$?
err="$(cat "$tmp/err")"
need "exit" "$got" 2
[ ! -e "$ghd/calls" ] || bad="$bad${bad:+; }gh was called"
verdict "refused: outside a git working tree"

# A linked worktree is where the stage sessions run: the same green result there.
repo g-linked
git -C "$repo" switch -q main
git -C "$repo" worktree add -q "$tmp/g-linked/wt" tw/T-999
mkdir -p "$tmp/g-linked/wt/scripts"
[ ! -e "$script" ] || cp -p "$script" "$tmp/g-linked/wt/scripts/tw-wip-run"
put_list 1 "$(list "$(obj 202 "$sha" completed success)")"
repo="$tmp/g-linked/wt"
run_it
need "exit" "$got" 0
has_line "CI: https://example.com/runs/202 @ $sha" || bad="$bad${bad:+; }no CI: line from a linked worktree"
verdict "green: from a linked worktree"

echo "tw-wip-run.test: $passed passed, $failed failed"
[ "$failed" -eq 0 ] || exit 1
