#!/usr/bin/env bash
# T-192 (decisions #208, #219; docs/tasks/T-192.md): host test of the only sanctioned CI push.
#
# Four subjects, one guarantee each:
#   A. scripts/tw-wip-push [<commit>] writes refs/heads/wip/<ID> on origin and nothing else, <ID>
#      from branch tw/<ID>, <commit> (default HEAD) descending from the fork point from main;
#      anything else is refused with no ref moved; the pre-push hook runs; TEAMWRIGHT_PAUSE stops it.
#   B. scripts/hooks/wip-push-guard.sh, a PreToolUse hook, refuses the `git -C` and `git -c` push
#      forms on one raw token over the whole command text and reads no shell (T-198,
#      analysis.decides_on; project hook contract, scripts/hooks/_hookio.py: deny = one JSON object
#      with permissionDecision=deny, exit 0; allow = no output, exit 0).
#   C. .claude/settings.json: allow holds the two script rules only, deny holds the push forms,
#      PreToolUse wires the hook.
#   D. scripts/tw-launch keeps the kit's denials and states the exception, against a fake kit; a kit
#      without the sentence or a denial is refused and nothing starts.
#
# Safety (A): the script under test is COPIED into a temporary clone and run from there, so neither
# its own location nor the working directory leads back to this repository. Its origin is a
# temporary bare repository, and GIT_ALLOW_PROTOCOL=file refuses any network transport, so even a
# wrong script with a hard-coded URL cannot reach GitHub. Nothing is pushed from this repository.
#
# Outside the contract: the text of a refusal (any exit other than 0, 126, 127 is a refusal; 126 and
# 127 mean the script did not run) and whether main's advanced tip may be pushed (it does descend
# from the fork point; not pinned). The headless-session evidence (last Acceptance line of T-192)
# takes a real session and a real push: not testable here.
#
# Usage: scripts/ci/tw-wip-push.test.sh   (host bash, git, python3)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
script="$root/scripts/tw-wip-push"
hook="$root/scripts/hooks/wip-push-guard.sh"
settings="$root/.claude/settings.json"
real_prepush="$root/scripts/hooks/pre-push"
for t in git python3 timeout; do
  command -v "$t" >/dev/null 2>&1 || { echo "tw-wip-push.test: cannot run: $t missing" >&2; exit 3; }
done
for f in "$real_prepush" "$root/scripts/hooks/_task.py" "$settings"; do
  [ -f "$f" ] || { echo "tw-wip-push.test: cannot run: $f missing" >&2; exit 3; }
done
tmp="$(mktemp -d)" || { echo "tw-wip-push.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

# No ambient git state: this test may run under a git hook or with a user's global config.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_PREFIX TEAMWRIGHT_ALLOW_PUSH TEAMWRIGHT_PAUSE_FILE
unset TEAMWRIGHT_WIP_REFS_RE TEAMWRIGHT_PUSH_STATUSES
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_ALLOW_PROTOCOL=file
export GIT_AUTHOR_NAME=fixture GIT_AUTHOR_EMAIL=fixture@example.com
export GIT_COMMITTER_NAME=fixture GIT_COMMITTER_EMAIL=fixture@example.com
export GIT_TERMINAL_PROMPT=0 PYTHONDONTWRITEBYTECODE=1

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

# ==== A. scripts/tw-wip-push against a temporary bare remote ===================================
g() { git -C "$clone" "$@"; }
refs() { git -C "$1" for-each-ref --format='%(refname) %(objectname)' | LC_ALL=C sort; }

# fixture <case>: remote history old -> fork (main), clone on tw/T-900 with c1 -> c2 on top of fork.
# Commits that are objects only: sib (child of fork, not an ancestor of HEAD), oldsib (child of old:
# forks from main before the fork point), orphan (no parent). A wrapper pre-push hook records its
# run and its stdin, then runs a copy of the real pre-push. Sets $bare $clone $hooks $old $fork $c1
# $c2 $sib $oldsib $orphan.
fixture() {
  local d="$tmp/$1"
  mkdir -p "$d"
  bare="$d/origin.git"
  clone="$d/clone"
  hooks="$d/hooks"
  git init -q --bare -b main "$bare"
  git clone -q "$bare" "$clone" 2>/dev/null
  g symbolic-ref HEAD refs/heads/main
  mkdir -p "$clone/docs/tasks" "$clone/scripts"
  printf '%s\n' '---' 'id: T-900' 'status: IN_PROGRESS' '---' >"$clone/docs/tasks/T-900.md"
  printf 'TEAMWRIGHT_PAUSE\n' >"$clone/.gitignore"
  printf 'one\n' >"$clone/file"
  g add -A
  g commit -q -m "chore: old"
  old="$(g rev-parse HEAD)"
  printf 'two\n' >"$clone/file"
  g commit -q -am "chore: fork"
  fork="$(g rev-parse HEAD)"
  g push -q origin main
  g switch -q -c tw/T-900
  printf 'three\n' >"$clone/file"
  g commit -q -am "T-900 first"
  c1="$(g rev-parse HEAD)"
  printf 'four\n' >"$clone/file"
  g commit -q -am "T-900 second"
  c2="$(g rev-parse HEAD)"
  sib="$(g commit-tree -p "$fork" -m "T-900 sibling" "$fork^{tree}")"
  oldsib="$(g commit-tree -p "$old" -m "foreign, older fork" "$old^{tree}")"
  orphan="$(g commit-tree -m "foreign, unrelated" "$fork^{tree}")"
  mkdir -p "$hooks"
  cp "$real_prepush" "$hooks/pre-push.real"
  cp "$root/scripts/hooks/_task.py" "$hooks/_task.py"
  cat >"$hooks/pre-push" <<'HOOK'
#!/usr/bin/env bash
here="$(cd "$(dirname "$0")" && pwd)"
cat >"$here/stdin"
printf 'ran\n' >>"$here/ran"
[ ! -e "$here/refuse" ] || exit 1
bash "$here/pre-push.real" "$@" <"$here/stdin"
HOOK
  chmod +x "$hooks/pre-push"
  g config core.hooksPath "$hooks"
  # Copied, not linked: see Safety. A missing script stays missing and every case fails on it.
  [ ! -e "$script" ] || cp -p "$script" "$clone/scripts/tw-wip-push"
}

# push_run <dir> <args...>: runs the copied script from <dir>. Sets $out, $got.
push_run() {
  local dir="$1"
  shift
  out="$(cd "$dir" && timeout 60 scripts/tw-wip-push "$@" 2>&1 </dev/null)"
  got=$?
}
ran() {
  [ -e "$1/scripts/tw-wip-push" ] && [ "$got" -ne 126 ] && [ "$got" -ne 127 ]
}

# want_wip <case> <sha>: exit 0; on the remote exactly refs/heads/wip/T-900 changed, to <sha>.
want_wip() {
  local c="$1" sha="$2" before="$3" want now
  now="$(refs "$bare")"
  want="$(printf '%s\n' "$before" | grep -v '^refs/heads/wip/T-900 ' ; printf 'refs/heads/wip/T-900 %s\n' "$sha")"
  want="$(printf '%s\n' "$want" | LC_ALL=C sort)"
  if ! ran "$clone" || [ "$got" -ne 0 ]; then
    fail "$c: exit $got (want 0)" "$out"
  elif [ "$now" != "$want" ]; then
    fail "$c: remote refs differ" "$(printf 'want:\n%s\ngot:\n%s' "$want" "$now")"
  else
    pass "$c"
  fi
}

# want_refused <case> <before refs>: the script ran, exit != 0, the remote is unchanged.
want_refused() {
  local c="$1" before="$2" now
  now="$(refs "$bare")"
  if ! ran "$clone"; then
    fail "$c: the script did not run (exit $got)" "$out"
  elif [ "$got" -eq 0 ]; then
    fail "$c: exit 0 (want refused)" "$out"
  elif [ "$now" != "$before" ]; then
    fail "$c: a ref moved on refusal" "$(printf 'before:\n%s\nafter:\n%s' "$before" "$now")"
  else
    pass "$c"
  fi
}

# ---- Pushes: the destination and the default commit -------------------------------------------
fixture a-default-head
before="$(refs "$bare")"
push_run "$clone"
want_wip a-default-head "$c2" "$before"

# The hook saw one line, for the wip ref, of the commit. Not skipped by --no-verify or by a
# core.hooksPath override.
if [ -e "$hooks/ran" ] && [ "$(wc -l <"$hooks/stdin")" -eq 1 ] \
  && [ "$(cut -d' ' -f2,3 "$hooks/stdin")" = "$c2 refs/heads/wip/T-900" ]; then
  pass a-hook-ran
else
  fail a-hook-ran "$(cat "$hooks/stdin" 2>/dev/null || echo 'pre-push did not run')"
fi

fixture a-hook-refusal
touch "$hooks/refuse"
before="$(refs "$bare")"
push_run "$clone"
want_refused a-hook-refusal-stops-push "$before"

# A task whose status the DRAIN rule would refuse still passes: the wip exemption of pre-push, by
# the real hook, is what the fixture's IN_PROGRESS record exercises in a-default-head.

# ---- An explicit commit descending from the fork point ----------------------------------------
fixture a-explicit-ancestor
before="$(refs "$bare")"
push_run "$clone" "$c1"
want_wip a-explicit-ancestor "$c1" "$before"

fixture a-explicit-rev-short
before="$(refs "$bare")"
push_run "$clone" "${c1:0:12}"
want_wip a-explicit-short-sha "$c1" "$before"
fixture a-explicit-rev-expr
before="$(refs "$bare")"
push_run "$clone" HEAD~1
want_wip a-explicit-rev-expression "$c1" "$before"

# T-102's shape: a child of the fork point that is not an ancestor of HEAD.
fixture a-explicit-sibling
before="$(refs "$bare")"
push_run "$clone" "$sib"
want_wip a-explicit-sibling-of-head "$sib" "$before"

# T-156's shape: wip already at HEAD moves back to an ancestor (not a fast-forward).
fixture a-move-back
push_run "$clone"
before="$(refs "$bare")"
push_run "$clone" "$c1"
want_wip a-moves-wip-back-to-ancestor "$c1" "$before"
# ... and forward again.
before="$(refs "$bare")"
push_run "$clone"
want_wip a-moves-wip-forward-again "$c2" "$before"

# main advanced after the fork: the fork point stays, HEAD still descends from it.
fixture a-main-advanced
g fetch -q origin
next="$(g commit-tree -p "$fork" -m "chore: main moved" "$fork^{tree}")"
g push -q origin "$next:refs/heads/main"
g update-ref refs/heads/main "$next"
before="$(refs "$bare")"
push_run "$clone"
want_wip a-main-advanced-head-accepted "$c2" "$before"

# A linked worktree is where the stage sessions run (.git is a file there).
fixture a-linked-worktree
g branch -q -f keep "$fork"
wt="$tmp/a-linked-worktree/wt"
g switch -q main
g worktree add -q "$wt" tw/T-900
mkdir -p "$wt/scripts"
[ ! -e "$script" ] || cp -p "$script" "$wt/scripts/tw-wip-push"
before="$(refs "$bare")"
push_run "$wt"
want_wip a-linked-worktree "$c2" "$before"

# An annotated tag names the commit it points at: the ref written is a branch, so it holds the commit.
fixture a-annotated-tag
g tag -a -m tagged t1 "$c1"
before="$(refs "$bare")"
push_run "$clone" t1
want_wip a-explicit-annotated-tag "$c1" "$before"

# A worktree's clone may have no local main: the fork point then comes from origin/main.
fixture a-no-local-main
g branch -q -D main
before="$(refs "$bare")"
push_run "$clone" "$c1"
want_wip a-no-local-main-uses-origin-main "$c1" "$before"

# ---- Refused: the branch ----------------------------------------------------------------------
refuse_on_branch() {
  local c="$1" branch="$2"
  fixture "$c"
  g switch -q -c "$branch" "$c2" || { fail "$c: fixture: cannot create $branch"; return; }
  before="$(refs "$bare")"
  push_run "$clone"
  want_refused "$c" "$before"
}
fixture r-branch-main
g switch -q main
before="$(refs "$bare")"
push_run "$clone"
want_refused r-branch-main "$before"
refuse_on_branch r-branch-feature "feature/T-900"
refuse_on_branch r-branch-prefix-like "xtw/T-900"
refuse_on_branch r-branch-wip "wip/T-900"
refuse_on_branch r-branch-not-an-id "tw/not-an-id"
refuse_on_branch r-branch-nested "tw/T-901/extra" # T-900 exists as a ref: git forbids tw/T-900/x
refuse_on_branch r-branch-embedded "x/refs/heads/tw/T-902"
fixture r-detached
g switch -q --detach "$c2"
before="$(refs "$bare")"
push_run "$clone"
want_refused r-detached-head "$before"
# The commit is valid, the branch is not: the argument does not rescue it.
fixture r-branch-main-with-arg
g switch -q main
before="$(refs "$bare")"
push_run "$clone" "$c2"
want_refused r-branch-main-explicit-commit "$before"

# ---- Refused: arguments. Each would write or widen something if passed on to git -------------
refuse_args() {
  local c="$1"
  shift
  fixture "$c"
  before="$(refs "$bare")"
  # pre-push's DRAIN rule would refuse a write to main or a tag by itself; it is opened here so the
  # refusal proven is the script's.
  TEAMWRIGHT_PUSH_STATUSES=IN_PROGRESS push_run "$clone" "$@"
  want_refused "$c" "$before"
}
refuse_args r-arg-extra-commit HEAD HEAD~1
refuse_args r-arg-extra-word HEAD extra
refuse_args r-arg-extra-refspec HEAD HEAD:refs/heads/main
refuse_args r-arg-extra-tag-refspec HEAD HEAD:refs/tags/v1
refuse_args r-arg-extra-dry-run HEAD --dry-run
refuse_args r-arg-double-dash -- HEAD
refuse_args r-arg-empty ""
refuse_args r-arg-refspec-main HEAD:refs/heads/main
refuse_args r-arg-refspec-main-short HEAD:main
refuse_args r-arg-refspec-force +HEAD:refs/heads/main
refuse_args r-arg-refspec-tag HEAD:refs/tags/v1
refuse_args r-arg-refspec-other-wip HEAD:refs/heads/wip/T-901
refuse_args r-arg-force --force
refuse_args r-arg-all --all
refuse_args r-arg-mirror --mirror
refuse_args r-arg-tags --tags
refuse_args r-arg-delete --delete
refuse_args r-arg-no-verify --no-verify
refuse_args r-arg-receive-pack --receive-pack=true
refuse_args r-arg-remote origin
refuse_args r-arg-missing-rev no-such-revision

# ---- Refused: a foreign commit ----------------------------------------------------------------
fixture r-foreign-orphan
before="$(refs "$bare")"
push_run "$clone" "$orphan"
want_refused r-foreign-orphan "$before"
fixture r-foreign-older-fork
before="$(refs "$bare")"
push_run "$clone" "$oldsib"
want_refused r-foreign-older-fork "$before"
fixture r-foreign-below-fork
before="$(refs "$bare")"
push_run "$clone" "$old"
want_refused r-foreign-ancestor-of-fork-point "$before"
fixture r-foreign-tree
before="$(refs "$bare")"
push_run "$clone" "$(g rev-parse "HEAD^{tree}")"
want_refused r-foreign-tree-object "$before"

# Local main is ahead of origin/main and the branch forked from it: the fork point is the local one,
# so origin/main's tip (below it) is foreign.
fixture r-foreign-local-main-ahead
g switch -q main
printf 'local\n' >"$clone/file"
g commit -q -am "chore: main, not pushed yet"
g switch -q -C tw/T-900
printf 'five\n' >"$clone/file"
g commit -q -am "T-900 on the new fork"
before="$(refs "$bare")"
push_run "$clone" "$fork"
want_refused r-foreign-below-local-fork-point "$before"

# No main anywhere: there is no fork point to descend from.
fixture r-no-main
g branch -q -D main
g update-ref -d refs/remotes/origin/main
before="$(refs "$bare")"
push_run "$clone"
want_refused r-no-main-at-all "$before"

# A tw/ branch with no history shared with main: no fork point, so even its own HEAD is foreign.
fixture r-no-fork
g checkout -q --orphan tw/T-901
g commit -q -m "T-901 unrelated"
before="$(refs "$bare")"
push_run "$clone"
want_refused r-no-fork-point "$before"

# ---- TEAMWRIGHT_PAUSE -------------------------------------------------------------------------
fixture p-pause
touch "$clone/TEAMWRIGHT_PAUSE"
before="$(refs "$bare")"
push_run "$clone"
want_refused p-pause-at-root "$before"

fixture p-pause-file
touch "$tmp/p-pause-file/pause"
before="$(refs "$bare")"
TEAMWRIGHT_PAUSE_FILE="$tmp/p-pause-file/pause" push_run "$clone"
want_refused p-pause-file-env "$before"

# scripts/hooks/pre-push exits 0 on this variable before it looks at the pause.
fixture p-pause-override
touch "$clone/TEAMWRIGHT_PAUSE"
before="$(refs "$bare")"
TEAMWRIGHT_ALLOW_PUSH=1 push_run "$clone"
want_refused p-pause-not-overridden-by-allow-push "$before"

# The owner pauses in the main tree. pre-push looks only at the top level of the tree it runs in, so
# a linked worktree does not see the file: the script has to.
fixture p-pause-main-tree
wt="$tmp/p-pause-main-tree/wt"
g switch -q main
g worktree add -q "$wt" tw/T-900
mkdir -p "$wt/scripts"
[ ! -e "$script" ] || cp -p "$script" "$wt/scripts/tw-wip-push"
touch "$clone/TEAMWRIGHT_PAUSE"
before="$(refs "$bare")"
push_run "$wt"
want_refused p-pause-in-main-tree-stops-linked-worktree "$before"

# The script's own check, not pre-push's: with no pre-push hook at all, each place still stops it.
fixture p-nohook-root
g config --unset core.hooksPath
touch "$clone/TEAMWRIGHT_PAUSE"
before="$(refs "$bare")"
push_run "$clone"
want_refused p-pause-at-root-without-hook "$before"

fixture p-nohook-file
g config --unset core.hooksPath
touch "$tmp/p-nohook-file/pause"
before="$(refs "$bare")"
TEAMWRIGHT_PAUSE_FILE="$tmp/p-nohook-file/pause" push_run "$clone"
want_refused p-pause-file-env-without-hook "$before"

fixture p-nohook-own-linked
g config --unset core.hooksPath
wt="$tmp/p-nohook-own-linked/wt"
g switch -q main
g worktree add -q "$wt" tw/T-900
mkdir -p "$wt/scripts"
[ ! -e "$script" ] || cp -p "$script" "$wt/scripts/tw-wip-push"
touch "$wt/TEAMWRIGHT_PAUSE"
before="$(refs "$bare")"
push_run "$wt"
want_refused p-pause-in-own-linked-tree-without-hook "$before"

# ---- No other ref, whatever the clone's config says -------------------------------------------
# Remote fixed: another remote configured as the default gets nothing, origin gets the wip ref.
fixture o-other-remote
other="$tmp/o-other-remote/other.git"
git init -q --bare -b main "$other"
g remote add other "$other"
g config remote.pushDefault other
g config branch.tw/T-900.remote other
g config branch.tw/T-900.pushRemote other
before="$(refs "$bare")"
push_run "$clone"
want_wip o-origin-is-fixed "$c2" "$before"
if [ -z "$(refs "$other")" ]; then
  pass o-other-remote-untouched
else
  fail o-other-remote-untouched "$(refs "$other")"
fi

# push.followTags would also write a reachable annotated tag.
fixture o-follow-tags
g tag -a -m release v1 "$c1"
g config push.followTags true
before="$(refs "$bare")"
push_run "$clone"
want_wip o-no-tag-with-follow-tags "$c2" "$before"

# A configured push refspec and push.default do not widen it.
fixture o-config-refspec
g config --add remote.origin.push 'refs/heads/*:refs/heads/*'
g config push.default matching
g branch -q side "$c1"
before="$(refs "$bare")"
push_run "$clone"
want_wip o-no-ref-from-configured-refspec "$c2" "$before"

# The ID comes from the branch, never from the commit's message or a branch at the commit.
fixture o-id-from-branch
msg="$(g commit-tree -p "$fork" -m "T-901 belongs elsewhere" "$fork^{tree}")"
g branch -q tw/T-901 "$msg"
before="$(refs "$bare")"
push_run "$clone" "$msg"
want_wip o-id-is-the-current-branch "$msg" "$before"

# With push.recurseSubmodules=on-demand git would also push the submodule's commit to its own remote.
fixture o-submodule
subbare="$tmp/o-submodule/sub.git"
git init -q --bare -b main "$subbare"
subwork="$tmp/o-submodule/subwork"
git clone -q "$subbare" "$subwork" 2>/dev/null
git -C "$subwork" symbolic-ref HEAD refs/heads/main
git -C "$subwork" commit -q --allow-empty -m "sub one"
git -C "$subwork" push -q origin main
g -c protocol.file.allow=always submodule --quiet add "$subbare" sub
git -C "$clone/sub" commit -q --allow-empty -m "sub, not pushed"
g add -A
g commit -q -m "T-900 moves the submodule"
c3="$(g rev-parse HEAD)"
g config push.recurseSubmodules on-demand
subbefore="$(refs "$subbare")"
before="$(refs "$bare")"
push_run "$clone"
want_wip o-no-submodule-push-exit "$c3" "$before"
if [ "$(refs "$subbare")" = "$subbefore" ]; then
  pass o-submodule-remote-untouched
else
  fail o-submodule-remote-untouched "$(refs "$subbare")"
fi

# ---- No repository, config or hook override through the environment ---------------------------
# Another repository named by GIT_DIR: the script still acts on the tree it runs in.
fixture o-env-git-dir
otherbare="$tmp/o-env-git-dir/other.git"
git init -q --bare -b main "$otherbare"
git clone -q "$otherbare" "$tmp/o-env-git-dir/otherclone" 2>/dev/null
git -C "$tmp/o-env-git-dir/otherclone" symbolic-ref HEAD refs/heads/tw/T-901
git -C "$tmp/o-env-git-dir/otherclone" commit -q --allow-empty -m "other"
before="$(refs "$bare")"
GIT_DIR="$tmp/o-env-git-dir/otherclone/.git" push_run "$clone"
want_wip o-env-git-dir-ignored "$c2" "$before"
if [ -z "$(refs "$otherbare")" ]; then
  pass o-env-git-dir-other-remote-untouched
else
  fail o-env-git-dir-other-remote-untouched "$(refs "$otherbare")"
fi

# A config override of core.hooksPath must not drop the pre-push hook, whose refusal then stops it.
fixture o-env-config-parameters
touch "$hooks/refuse"
before="$(refs "$bare")"
GIT_CONFIG_PARAMETERS="'core.hooksPath=/dev/null'" push_run "$clone"
want_refused o-env-config-parameters-keep-the-hook "$before"

fixture o-env-config-count
touch "$hooks/refuse"
before="$(refs "$bare")"
GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.hooksPath GIT_CONFIG_VALUE_0=/dev/null push_run "$clone"
want_refused o-env-config-count-keeps-the-hook "$before"

# ==== B. scripts/hooks/wip-push-guard.sh, fed Claude Code hook JSON ============================
hookdir="$tmp/hook-cwd"
mkdir -p "$hookdir"

# hook_decision <event> <tool> <command>: prints deny | silent | other:<detail>.
hook_decision() {
  local input o rc
  input="$(EV="$1" TOOL="$2" CMD="$3" CWD="$hookdir" python3 -I -c '
import json, os
print(json.dumps({"session_id": "fixture", "cwd": os.environ["CWD"], "hook_event_name": os.environ["EV"],
                  "tool_name": os.environ["TOOL"], "tool_input": {"command": os.environ["CMD"]}}))')"
  hook_feed "$input"
}
hook_feed() {
  local o rc
  o="$(cd "$hookdir" && CLAUDE_PROJECT_DIR="$hookdir" timeout 30 bash "$hook" <<<"$1" 2>/dev/null)"
  rc=$?
  if [ -z "$o" ]; then
    [ "$rc" -eq 0 ] && echo silent || echo "other:exit $rc, no output"
    return
  fi
  [ "$rc" -eq 0 ] || { echo "other:exit $rc"; return; }
  OUT="$o" python3 -I -c '
import json, os
try:
    d = json.loads(os.environ["OUT"])["hookSpecificOutput"]["permissionDecision"]
except Exception:
    d = None
print("deny" if d == "deny" else "other:" + os.environ["OUT"][:200])'
}

hook_case() {
  local want="$1" cmd="$2" got_d
  if [ ! -f "$hook" ]; then
    fail "hook $want: $cmd" "scripts/hooks/wip-push-guard.sh missing"
    return
  fi
  got_d="$(hook_decision PreToolUse Bash "$cmd")"
  if [ "$got_d" = "$want" ]; then
    pass "hook $want: $cmd"
  else
    fail "hook $want: $cmd" "got: $got_d"
  fi
}

# Refused: a git global option before `push`, in every shape a session writes.
while IFS= read -r c; do
  [ -z "$c" ] || hook_case deny "$c"
done <<'CMDS'
git -C . push origin main
git -C . push -f origin HEAD:wip/x
git -C /other/repo push -f origin HEAD:wip/x
git -c x=y push origin main
git -c core.hooksPath=/dev/null push -f origin HEAD:wip/x
git -c core.sshCommand=/bin/true push origin HEAD:refs/tags/v1
git -C . -c x=y push origin main
git -c x=y -C . push origin main
git -c a=b -c c=d push
git  -C  .  push  origin  main
git -C "some dir" push origin main
git -C . --no-pager push origin main
/usr/bin/git -C . push origin main
true && git -C . push origin main
true; git -c x=y push origin main
true | git -C . push origin main
false || git -c x=y push origin main
(git -C . push origin main)
FOO=1 git -C . push origin main
env FOO=1 git -c x=y push origin main
sh -c 'git -C . push origin main'
bash -c "git -c x=y push origin main"
git --git-dir .git push origin main
git --work-tree . push origin main
git --git-dir=.git push origin main
command git -C . push origin main
timeout 5 git -c x=y push origin main
sudo git -C . push origin main
eval git -C . push origin main
! git -C . push origin main
if git -C . push origin main; then true; fi
git -C . push origin main >/dev/null 2>&1
>out git -C . push origin main
echo "$(git -C . push origin main)"
echo $(git -c x=y push origin main)
echo "`git -C . push origin main`"
echo `git -C . push origin main`
echo `sh -c 'git -C . push origin main'`
echo "it's `git -C . push origin main`"
echo 'plain' `git -C . push origin main`
echo "$(sh -c 'git -C . push origin main')"
git -C . push origin 'unbalanced
CMDS
hook_case deny "$(printf 'true\ngit -C . push origin main')"
hook_case deny "$(printf 'bash <<EOF\ngit -C . push origin main\nEOF')"
hook_case deny "$(printf 'true\ngit -c x=y push origin main')"

# T-198, the Acceptance shapes and what the model never read: refused because the raw text holds the
# token, whatever runs it.
while IFS= read -r c; do
  [ -z "$c" ] || hook_case deny "$c"
done <<'CMDS'
function f { git -C . push origin main; }; f
bash <<< 'git -C . push origin main'
echo 'git -C . push origin main' | sh
sh <(echo 'git -C . push origin main')
echo "$(true $(x); git -C . push o)"
coproc git -C . push o
setsid git -C . push o
flock /tmp/x git -C . push o
env -S 'git -C . push o'
echo 'git -C . push origin main' | bash -s
git -c alias.p=push p
never-named-wrapper git -C . push o
find . -exec git -C . push \;
CMDS
# A wrapper no hook text can name: the name changes every run, so a clause cannot list it.
hook_case deny "wrapper_$$_$RANDOM git -C . push o"

# T-198, the 11 commands the model let through: the token is in the text, so they are refused too. A
# session splits the call or uses another tool (T-198 Investigation, 0.17% of measured calls).
while IFS= read -r c; do
  [ -z "$c" ] || hook_case deny "$c"
done <<'CMDS'
git -C . log --grep=push
git -C . stash push
git -C docs status && echo push
git -C . commit -m "do not push"
echo git -C . push origin main
echo 'git -C . push origin main'
echo 'a `git -C . push origin main` b'
echo '$(git -C . push origin main)'
echo \`git -C . push origin main\`
git -C . status > push
CMDS
hook_case deny "$(printf 'cat <<EOF\ngit -C . push origin main\nEOF')"

# T-198, the edges of the token (analysis.decides_on), each one part of the regex.
while IFS= read -r c; do
  [ -z "$c" ] || hook_case deny "$c"
done <<'CMDS'
git -C . status; git push origin HEAD:wip/T-1
git -c push.default=simple status
git -C . 'push' o
git status; git -C . push o
CMDS
hook_case deny "$(printf 'git\t-C . push o')"
hook_case deny "$(printf 'git -C .\tpush o')"
hook_case deny "$(printf 'git \\\n  -C . push o')"
hook_case deny "$(printf 'git -C . \\\n  push o')"
hook_case deny "$(printf 'git -C .\ngit -C . push o')"

# T-198: a refusal blocks in every gate mode (hard=True); gate-first mode only warns on a soft deny.
TEAMWRIGHT_GATE_MODE=warn hook_case deny "setsid git -C . push o"

# T-198: no reader of shell is left in the hook.
if [ ! -f "$hook" ]; then
  fail "hook holds no shlex" "scripts/hooks/wip-push-guard.sh missing"
elif grep -q shlex "$hook"; then
  fail "hook holds no shlex" "$(grep -n shlex "$hook")"
else
  pass "hook holds no shlex"
fi

# Not refused: the script, git with no option before the subcommand, and git with the same options
# whose text does not hold the token.
while IFS= read -r c; do
  [ -z "$c" ] || hook_case silent "$c"
done <<'CMDS'
scripts/tw-wip-push
scripts/tw-wip-push 0123456789abcdef
git -C . log; scripts/tw-wip-push
git -C . status
git -c color.ui=never log --oneline
git -C . remote get-url --push origin
git -C . diff -- scripts/hooks/pre-push
git -C . log -- docs/push.md
git -C . log --grep=pushed
git -C . unpush
git -C . push-notes
git push origin main
git push origin HEAD:wip/T-1
git push origin --delete wip/T-109
git push -f origin main
git stash push
git log -p push
git-foo -x push
legit -C . push origin
git status
echo git
CMDS
# The token stays on one logical line: a real newline ends it.
hook_case silent "$(printf 'git -C .\npush origin main')"
hook_case silent "$(printf 'git\n-C . push origin main')"

# T-198, the stated limits (docs/process/gates.md): the text does not hold the token. Refusing one of
# these takes a reader of quotes or variables, which is the model this rule replaced.
while IFS= read -r c; do
  [ -z "$c" ] || hook_case silent "$c"
done <<'CMDS'
git "-C" . push origin main
git -C . pu""sh origin main
X=push; git -C . $X
G=git; $G -C . push origin main
git -C . send-pack origin main
CMDS

# Only a PreToolUse of Bash is its business; a broken input follows the fail-open contract.
hook_other() {
  local c="$1" want="$2" got_d="$3"
  if [ ! -f "$hook" ]; then
    fail "hook $c" "scripts/hooks/wip-push-guard.sh missing"
  elif [ "$got_d" = "$want" ]; then
    pass "hook $c"
  else
    fail "hook $c" "got: $got_d"
  fi
}
hook_other "other tool not refused" silent "$(hook_decision PreToolUse Write 'git -C . push origin main')"
hook_other "other event not refused" silent "$(hook_decision PostToolUse Bash 'git -C . push origin main')"
hook_other "broken input fails open" silent "$(hook_feed 'not json')"

# ==== C. .claude/settings.json =================================================================
settings_out="$(SETTINGS="$settings" python3 -I - <<'PY'
import json, os, re

s = json.load(open(os.environ["SETTINGS"], encoding="utf-8"))
perms = s.get("permissions") or {}
allow, deny = perms.get("allow") or [], perms.get("deny") or []

def matches(rule, command):
    """A Bash(...) rule's prefix-and-wildcard reading: `*` spans anything, spaces included
    (T-192 Investigation; the docs' bash rule limits), `:*` is the old prefix form."""
    if rule == "Bash":
        return True
    m = re.fullmatch(r"Bash\((.*)\)", rule, re.S)
    if not m:
        return False
    body = m.group(1)
    body = body[:-2] + " *" if body.endswith(":*") else body
    rx = ".*".join(re.escape(p) for p in body.split("*"))
    if body.endswith(" *"):  # `x *` also matches a bare `x`
        rx = rx[: -len(re.escape(" ")) - 2] + r"(?:\s.*)?"
    return re.fullmatch(rx, command, re.S) is not None

def check(name, ok, detail=""):
    print(("ok " if ok else "FAIL ") + name + ("" if ok else " :: " + detail))

want_allow = {"Bash(scripts/tw-wip-push)", "Bash(scripts/tw-wip-push *)"}
check("allow holds exactly the two script rules", len(allow) == 2 and set(allow) == want_allow, repr(allow))
check("the script runs under allow, bare", any(matches(r, "scripts/tw-wip-push") for r in allow), repr(allow))
check("the script runs under allow, with a commit", any(matches(r, "scripts/tw-wip-push 0123abc") for r in allow), repr(allow))

named = [r for r in allow if re.search(r"git\b.*\bpush|\bmain\b|\btags?\b|refs/", r)]
check("no allow rule names a git push, main or a tag", not named, repr(named))
for cmd in ("git push origin main", "git push origin HEAD:main", "git push origin refs/tags/v1",
            "git push --tags", "git -C . push origin main", "git -c x=y push origin main"):
    hit = [r for r in allow if matches(r, cmd)]
    check("no allow rule matches: " + cmd, not hit, repr(hit))

for cmd in ("git -C . push origin main", "git -C /other/repo push -f origin HEAD:wip/x",
            "git -C . -c x=y push origin main", "git -c x=y push origin main",
            "git -c core.hooksPath=/dev/null push -f origin HEAD:wip/x", "git -c x=y -C . push"):
    check("deny matches: " + cmd, any(matches(r, cmd) for r in deny), repr(deny))
for cmd in ("scripts/tw-wip-push", "scripts/tw-wip-push 0123abc", "git -C . status", "git -c x=y log"):
    hit = [r for r in deny if matches(r, cmd)]
    check("deny does not match: " + cmd, not hit, repr(hit))
# Every other push stays denied: the rules the task found must not go to make room for the script.
for rule in ("Bash(git push --force *)", "Bash(git push --force-with-lease *)", "Bash(git push -f *)",
             "Bash(git push * --force*)", "Bash(git push * -f)", "Bash(git push * -f *)"):
    check("deny keeps: " + rule, rule in deny, repr(deny))

want_cmd = 'bash "$CLAUDE_PROJECT_DIR/scripts/hooks/wip-push-guard.sh"'
wired = False
for entry in (s.get("hooks") or {}).get("PreToolUse") or []:
    matcher = entry.get("matcher") or ""
    if matcher == "*" or "Bash" in matcher.split("|"):
        wired = wired or any(h.get("command") == want_cmd and h.get("type") == "command"
                             for h in entry.get("hooks") or [])
check("PreToolUse wires the hook on Bash", wired, "no Bash entry runs " + want_cmd)
PY
)"
rc=$?
if [ "$rc" -ne 0 ] || [ -z "$settings_out" ]; then
  fail "settings: the check did not run (python exit $rc)" "$settings_out"
fi
while IFS= read -r line; do
  case "$line" in
    "ok "*) pass "settings: ${line#ok }" ;;
    "FAIL "*) l="${line#FAIL }"; fail "settings: ${l%% :: *}" "${l#* :: }" ;;
  esac
done <<<"$settings_out"

# ==== D. scripts/tw-launch, dry: a fake kit records what it is started with =====================
# No claude session starts: TEAMWRIGHT_KIT_DIR points at a fake kit, whose launcher writes its
# arguments and $TEAMWRIGHT_CLAUDE_ARGS to a file and exits. Nothing but the fake kit is executed.
launch="$root/scripts/tw-launch"
GOOD_DENY='Bash(git push:*) Bash(git merge:*) Bash(gh:*)'
GOOD_PROMPT='AUTONOMOUS RUN. Stop on a question. Do not push, do not merge. The end.'

# fake_kit <dir> <disallowedTools value> <prompt>
fake_kit() {
  mkdir -p "$1/scripts"
  DENY="$2" PROMPT="$3" python3 -I -c '
import os, sys
sys.stdout.write("""import json, os, sys
DEFAULT_ARGS = ["--permission-mode", "acceptEdits", "--allowedTools", "Bash Read",
                "--disallowedTools", %r, "--append-system-prompt", %r]
if __name__ == "__main__":
    with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "started"), "w") as f:
        json.dump({"argv": sys.argv[1:], "args": os.environ.get("TEAMWRIGHT_CLAUDE_ARGS")}, f)
""" % (os.environ["DENY"], os.environ["PROMPT"]))' >"$1/scripts/tw-parallel.py"
}

# launch_run <kit dir> <args...>: sets $lout, $lgot.
launch_run() {
  local kit="$1"
  shift
  lout="$(TEAMWRIGHT_KIT_DIR="$kit" timeout 30 "$launch" "$@" 2>"$tmp/launch.err" </dev/null)"
  lgot=$?
}

# Refused: exit 2 (not a crash), no value printed, the kit's launcher not started.
want_launch_refused() {
  local c="$1" kit="$2"
  shift 2
  rm -f "$kit/scripts/started"
  launch_run "$kit" "$@"
  if [ ! -x "$launch" ]; then
    fail "$c: scripts/tw-launch missing or not executable"
  elif [ "$lgot" -ne 2 ]; then
    fail "$c: exit $lgot (want 2)" "$lout$(cat "$tmp/launch.err")"
  elif [ -n "$lout" ]; then
    fail "$c: printed a value" "$lout"
  elif [ -e "$kit/scripts/started" ]; then
    fail "$c: the kit's launcher started" "$(cat "$kit/scripts/started")"
  else
    pass "$c"
  fi
}

# print_args_check <case> <value>: the value keeps every denial and every other argument, and the
# only prompt change is the sentence: the kit's gone, the exception there.
print_args_check() {
  local r
  r="$(VALUE="$2" python3 -I -c '
import os, shlex
a = shlex.split(os.environ["VALUE"])
opt = {k: v for k, v in zip(a[::2], a[1::2])}
bad = []
deny = opt.get("--disallowedTools", "")
for d in ("Bash(git push:*)", "Bash(git merge:*)", "Bash(gh:*)"):
    if d not in deny:
        bad.append("denial lost: " + d)
p = opt.get("--append-system-prompt", "")
if "Do not push, do not merge." in p:
    bad.append("the kit sentence is still there")
if "scripts/tw-wip-push" not in p or "no other push" not in p or "Do not merge." not in p:
    bad.append("the exception sentence is missing: " + p)
if len(a) % 2 or opt.get("--permission-mode") != "acceptEdits" or "Bash" not in opt.get("--allowedTools", "").split():
    bad.append("other arguments changed: " + repr(a))
print("; ".join(bad))' 2>&1)"
  if [ -z "$r" ]; then pass "$1"; else fail "$1" "$r"; fi
}

if [ ! -x "$launch" ]; then
  fail "launch: scripts/tw-launch missing or not executable"
fi

kit="$tmp/kit-good"
fake_kit "$kit" "$GOOD_DENY" "$GOOD_PROMPT"
launch_run "$kit" --print-args
if [ "$lgot" -ne 0 ]; then
  fail "launch print-args: exit $lgot" "$(cat "$tmp/launch.err")"
else
  print_args_check "launch print-args: denials kept, exception stated" "$lout"
  if [ -e "$kit/scripts/started" ]; then
    fail "launch print-args: starts nothing" "$(cat "$kit/scripts/started")"
  else
    pass "launch print-args: starts nothing"
  fi
  case "$lout" in
    *"AUTONOMOUS RUN. Stop on a question."*"The end."*) pass "launch print-args: the rest of the prompt stays" ;;
    *) fail "launch print-args: the rest of the prompt stays" "$lout" ;;
  esac
fi
printargs="$lout"

# The launch itself: the kit's launcher gets these ids, this project and the same value.
launch_run "$kit" T-900 T-901
started="$(WANT="$printargs" ROOT_DIR="$root" python3 -I -c '
import json, os, sys
d = json.load(open(sys.argv[1]))
want = ["launch", "T-900", "T-901", "--project", os.environ["ROOT_DIR"]]
print("ok" if d["argv"] == want and d["args"] == os.environ["WANT"] else repr(d))' "$kit/scripts/started" 2>&1)"
if [ "$lgot" -eq 0 ] && [ "$started" = ok ]; then
  pass "launch: the kit gets the ids, the project and the changed args"
else
  fail "launch: the kit gets the ids, the project and the changed args" "exit $lgot; $started"
fi

# A kit without the sentence, or with it twice, or short of a denial: nothing starts, either mode.
kit="$tmp/kit-no-sentence"
fake_kit "$kit" "$GOOD_DENY" 'AUTONOMOUS RUN. Do not merge.'
want_launch_refused "launch refused: kit without the sentence, print-args" "$kit" --print-args
want_launch_refused "launch refused: kit without the sentence, launch" "$kit" T-900
kit="$tmp/kit-two-sentences"
fake_kit "$kit" "$GOOD_DENY" "$GOOD_PROMPT"
python3 -I -c '
import sys
p = sys.argv[1]; t = open(p).read()
open(p, "w").write(t.replace("\"--permission-mode\", \"acceptEdits\",", "\"--permission-mode\", \"Do not push, do not merge.\","))' "$kit/scripts/tw-parallel.py"
want_launch_refused "launch refused: the sentence in two arguments" "$kit" T-900
for d in 'Bash(git push:*)' 'Bash(git merge:*)' 'Bash(gh:*)'; do
  kit="$tmp/kit-short-$d"
  fake_kit "$kit" "${GOOD_DENY/"$d"/}" "$GOOD_PROMPT"
  want_launch_refused "launch refused: kit does not deny $d, print-args" "$kit" --print-args
  want_launch_refused "launch refused: kit does not deny $d, launch" "$kit" T-900
done
# A denial only named in the prompt denies nothing.
kit="$tmp/kit-deny-in-prompt"
fake_kit "$kit" 'Bash(git merge:*) Bash(gh:*)' "$GOOD_PROMPT Bash(git push:*)"
want_launch_refused "launch refused: a denial only in the prompt" "$kit" T-900
# No launcher in the kit; no arguments; an option among the ids.
want_launch_refused "launch refused: no kit launcher" "$tmp/kit-empty" T-900
want_launch_refused "launch refused: no arguments" "$tmp/kit-good"
want_launch_refused "launch refused: an option among the ids" "$tmp/kit-good" T-900 --force
want_launch_refused "launch refused: an option with print-args" "$tmp/kit-good" --print-args --force

# Without $TEAMWRIGHT_KIT_DIR the kit is the version in .teamwright/installed.json, under $HOME.
lroot="$tmp/launch-root"
lhome="$tmp/launch-home"
mkdir -p "$lroot/scripts" "$lroot/.teamwright"
if [ -x "$launch" ]; then cp -p "$launch" "$lroot/scripts/tw-launch"; fi
fake_kit "$lhome/.claude/plugins/cache/teamwright/teamwright/9.9.9" "$GOOD_DENY" "$GOOD_PROMPT"
printf '{"@kit": {"version": "9.9.9"}}\n' >"$lroot/.teamwright/installed.json"
lout="$(env -u TEAMWRIGHT_KIT_DIR HOME="$lhome" timeout 30 "$lroot/scripts/tw-launch" --print-args 2>&1 </dev/null)"
lgot=$?
if [ "$lgot" -eq 0 ]; then
  print_args_check "launch installed version: the kit under HOME is read" "$lout"
else
  fail "launch installed version: the kit under HOME is read" "exit $lgot; $lout"
fi
env -u TEAMWRIGHT_KIT_DIR HOME="$lhome" timeout 30 "$lroot/scripts/tw-launch" T-900 >/dev/null 2>&1 </dev/null
if grep -q "\"$lroot\"" "$lhome/.claude/plugins/cache/teamwright/teamwright/9.9.9/scripts/started" 2>/dev/null; then
  pass "launch installed version: the project is the tree the script is in"
else
  fail "launch installed version: the project is the tree the script is in"
fi
for broken in '' '{"@kit": {}}' 'not json'; do
  printf '%s' "$broken" >"$lroot/.teamwright/installed.json"
  lout="$(env -u TEAMWRIGHT_KIT_DIR HOME="$lhome" timeout 30 "$lroot/scripts/tw-launch" --print-args 2>/dev/null </dev/null)"
  lgot=$?
  if [ "$lgot" -eq 2 ] && [ -z "$lout" ]; then
    pass "launch refused: installed.json is [$broken]"
  else
    fail "launch refused: installed.json is [$broken]" "exit $lgot; $lout"
  fi
done

# The kit actually installed here, when there is one (not in CI): a kit update that drops the
# sentence or a denial turns this red, which is the signal to look at tw-launch again.
ver="$(python3 -I -c '
import json
print(json.load(open(".teamwright/installed.json"))["@kit"]["version"])' 2>/dev/null || true)"
real_kit="$HOME/.claude/plugins/cache/teamwright/teamwright/$ver"
if [ -n "$ver" ] && [ -f "$real_kit/scripts/tw-parallel.py" ]; then
  launch_run "$real_kit" --print-args
  if [ "$lgot" -eq 0 ]; then
    print_args_check "launch print-args: the installed kit $ver" "$lout"
  else
    fail "launch print-args: the installed kit $ver" "exit $lgot; $(cat "$tmp/launch.err")"
  fi
else
  echo "skip launch print-args: no installed kit here"
fi

echo "tw-wip-push.test: $passed passed, $failed failed"
[ "$failed" -eq 0 ] || exit 1
