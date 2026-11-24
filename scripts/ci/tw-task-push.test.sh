#!/usr/bin/env bash
# T-265 (decisions #364, #366, #367; docs/tasks/T-265.md): host test of the main-tree orchestrator's
# hand-off push, scripts/tw-task-push <ID>.
#
# Three subjects, one guarantee each:
#   A. scripts/tw-task-push <ID>, run from the main tree, writes refs/heads/task/<ID> on origin and
#      nothing else, from the head of local branch tw/<ID> checked out in a linked worktree: create
#      on an absent ref, force-with-lease over an existing head that is not an ancestor, the pre-push
#      hook of the main tree running once for that ref. Anything else is refused with no ref moved:
#      a bad argument, a run from the linked worktree, no worktree, a branch with no fork point from
#      main, a path under .github/ in the diff from the fork point or the token in the record's front
#      matter (#365, #367), a missing record, a dirty worktree (#367), a pause in any of its three
#      places, a lease mismatch. No environment override and no clone config widens it.
#   B. .claude/settings.json: allow is the two tw-wip-push rules plus Bash(scripts/tw-task-push *),
#      no allow rule matches a git push, deny keeps every push form; the script text pins its
#      override value to the contour's and uses a lease, never a + refspec.
#   C. scripts/hooks/wip-push-guard.sh is silent on `scripts/tw-task-push T-900`.
#
# Safety (A): the script under test is COPIED into a temporary main tree (and a temporary linked
# worktree) and run from there, so neither its own location nor the working directory leads back to
# this repository. Its origin is a temporary bare repository, and GIT_ALLOW_PROTOCOL=file refuses any
# network transport, so even a wrong script with a hard-coded URL cannot reach GitHub. Nothing is
# pushed from this repository.
#
# Outside the contract: the text of a refusal (any exit other than 0, 126, 127 is a refusal; 126 and
# 127 mean the script did not run) and the list of remote-only commits the script may print.
#
# Usage: scripts/ci/tw-task-push.test.sh   (host bash, git, python3)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
script="$root/scripts/tw-task-push"
hook="$root/scripts/hooks/wip-push-guard.sh"
settings="$root/.claude/settings.json"
contour_push="$root/scripts/ci/contour-push.yml"
real_prepush="$root/scripts/hooks/pre-push"
for t in git python3 timeout; do
  command -v "$t" >/dev/null 2>&1 || { echo "tw-task-push.test: cannot run: $t missing" >&2; exit 3; }
done
for f in "$real_prepush" "$root/scripts/hooks/_task.py" "$settings" "$contour_push"; do
  [ -f "$f" ] || { echo "tw-task-push.test: cannot run: $f missing" >&2; exit 3; }
done
tmp="$(mktemp -d)" || { echo "tw-task-push.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

# No ambient git state: this test may run under a git hook or with a user's global config.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_PREFIX TEAMWRIGHT_ALLOW_PUSH TEAMWRIGHT_PAUSE_FILE
unset TEAMWRIGHT_WIP_REFS_RE TEAMWRIGHT_PUSH_STATUSES TEAMWRIGHT_DOC_PATHS_RE CLAUDECODE
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

# The one clear line when the script is not there; every case below reports on its own as well.
if [ ! -f "$script" ]; then
  fail "scripts/tw-task-push missing" "the script under test does not exist: every behavioural and static case below fails on it"
elif [ ! -x "$script" ]; then
  fail "scripts/tw-task-push is not executable"
else
  pass "scripts/tw-task-push exists and is executable"
fi

# ==== A. scripts/tw-task-push against a temporary bare remote ===================================
m() { git -C "$main" "$@"; }
w() { git -C "$wt" "$@"; }
refs() { git -C "$1" for-each-ref --format='%(refname) %(objectname)' | LC_ALL=C sort; }

# fixture <case>: remote history old -> fork (main). The main tree is a clone of it on main; a
# linked worktree $wt is on tw/T-900 with c1 -> c2 on top of fork. The record docs/tasks/T-900.md is
# IN_PROGRESS (pre-push's DRAIN rule would refuse a task/ push of it). Commits that are objects only:
# sib (child of fork, not an ancestor of c2), orphan (no parent). A wrapper pre-push hook records its
# run, its stdin and the environment it sees, can refuse (file refuse) or move the remote's task ref
# to the commit named in file move, then runs a copy of the real pre-push. The script is copied
# into the main tree and the worktree (both ignore it). Sets $bare $main $wt $hooks $old $fork $c1
# $c2 $sib $orphan.
fixture() {
  local d="$tmp/$1"
  mkdir -p "$d"
  bare="$d/origin.git"
  main="$d/main"
  wt="$d/wt"
  hooks="$d/hooks"
  git init -q --bare -b main "$bare"
  git clone -q "$bare" "$main" 2>/dev/null
  m symbolic-ref HEAD refs/heads/main
  mkdir -p "$main/docs/tasks" "$main/scripts"
  printf '%s\n' '---' 'id: T-900' 'status: IN_PROGRESS' 'title: "fixture"' '---' '' '## Context' 'body' >"$main/docs/tasks/T-900.md"
  printf 'TEAMWRIGHT_PAUSE\n/scripts/tw-task-push\n' >"$main/.gitignore"
  printf 'one\n' >"$main/file"
  m add -A
  m commit -q -m "chore: old"
  old="$(m rev-parse HEAD)"
  printf 'two\n' >"$main/file"
  m commit -q -am "chore: fork"
  fork="$(m rev-parse HEAD)"
  m push -q origin main
  m worktree add -q -b tw/T-900 "$wt" main
  printf 'three\n' >"$wt/file"
  w commit -q -am "T-900 first"
  c1="$(w rev-parse HEAD)"
  printf 'four\n' >"$wt/file"
  w commit -q -am "T-900 second"
  c2="$(w rev-parse HEAD)"
  sib="$(m commit-tree -p "$fork" -m "T-900 sibling" "$fork^{tree}")"
  orphan="$(m commit-tree -m "foreign, unrelated" "$fork^{tree}")"
  mkdir -p "$hooks"
  cp "$real_prepush" "$hooks/pre-push.real"
  cp "$root/scripts/hooks/_task.py" "$hooks/_task.py"
  cat >"$hooks/pre-push" <<HOOK
#!/usr/bin/env bash
here="\$(cd "\$(dirname "\$0")" && pwd)"
cat >"\$here/stdin"
printf 'ran\n' >>"\$here/ran"
{
  printf 'WIP=%s\n' "\${TEAMWRIGHT_WIP_REFS_RE-}"
  printf 'STATUSES=%s\n' "\${TEAMWRIGHT_PUSH_STATUSES-}"
  printf 'ALLOW=%s\n' "\${TEAMWRIGHT_ALLOW_PUSH-}"
  printf 'DOC=%s\n' "\${TEAMWRIGHT_DOC_PATHS_RE-}"
} >"\$here/env"
[ ! -e "\$here/refuse" ] || exit 1
if [ -e "\$here/move" ]; then
  GIT_DIR="$bare" git update-ref refs/heads/task/T-900 "\$(cat "\$here/move")"
fi
bash "\$here/pre-push.real" "\$@" <"\$here/stdin"
HOOK
  chmod +x "$hooks/pre-push"
  m config core.hooksPath "$hooks"
  # An upload-pack wrapper: whatever reads the remote's refs (the script's lease value) is served,
  # then the task ref moves to the commit named in file $d/stale. The later push sees the new value.
  cat >"$d/upload-wrapper" <<UPLOAD
#!/usr/bin/env bash
git upload-pack "\$@"
rc=\$?
if [ -e "$d/stale" ]; then
  GIT_DIR="$bare" git update-ref refs/heads/task/T-900 "\$(cat "$d/stale")"
fi
exit \$rc
UPLOAD
  chmod +x "$d/upload-wrapper"
  m config remote.origin.uploadpack "$d/upload-wrapper"
  # Copied, not linked: see Safety. A missing script stays missing and every case fails on it.
  if [ -e "$script" ]; then
    cp -p "$script" "$main/scripts/tw-task-push"
    mkdir -p "$wt/scripts"
    cp -p "$script" "$wt/scripts/tw-task-push"
  fi
}

# push_run <dir> <args...>: runs the copied script from <dir>. Sets $out, $got.
push_run() {
  local dir="$1"
  shift
  out="$(cd "$dir" && timeout 60 scripts/tw-task-push "$@" 2>&1 </dev/null)"
  got=$?
}
ran() {
  [ -e "$main/scripts/tw-task-push" ] && [ "$got" -ne 126 ] && [ "$got" -ne 127 ]
}

# remote_with <before> <ref> <sha>: the ref list <before> with <ref> set to <sha>.
remote_with() {
  { printf '%s\n' "$1" | grep -v "^$2 " || true; printf '%s %s\n' "$2" "$3"; } | LC_ALL=C sort
}

# want_task <case> <sha> <before refs>: exit 0; on the remote exactly refs/heads/task/T-900 changed, to <sha>.
want_task() {
  local c="$1" sha="$2" before="$3" want now
  now="$(refs "$bare")"
  want="$(remote_with "$before" refs/heads/task/T-900 "$sha")"
  if ! ran; then
    fail "$c: the script did not run (scripts/tw-task-push missing? exit $got)" "$out"
  elif [ "$got" -ne 0 ]; then
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
  if ! ran; then
    fail "$c: the script did not run (scripts/tw-task-push missing? exit $got)" "$out"
  elif [ "$got" -eq 0 ]; then
    fail "$c: exit 0 (want refused)" "$out"
  elif [ "$now" != "$before" ]; then
    fail "$c: a ref moved on refusal" "$(printf 'before:\n%s\nafter:\n%s' "$before" "$now")"
  else
    pass "$c"
  fi
}

# push_base <sha>: puts <sha> at task/T-900 on the remote by a plain push, hook skipped.
push_base() { m push -q --no-verify origin "$1:refs/heads/task/T-900" 2>/dev/null; }

# ---- Pushes ------------------------------------------------------------------------------------
fixture a-create
before="$(refs "$bare")"
push_run "$main" T-900
want_task a-creates-absent-task-ref "$c2" "$before"

# The hook of the main tree saw one line, for the task ref, of the commit, with no old value.
if [ -e "$hooks/ran" ] && [ "$(wc -l <"$hooks/ran")" -eq 1 ] && [ "$(wc -l <"$hooks/stdin")" -eq 1 ] \
  && [ "$(cut -d' ' -f2,3 "$hooks/stdin")" = "$c2 refs/heads/task/T-900" ] \
  && [ "$(cut -d' ' -f4 "$hooks/stdin")" = "0000000000000000000000000000000000000000" ]; then
  pass a-hook-ran-once-for-the-task-ref
else
  fail a-hook-ran-once-for-the-task-ref "$(cat "$hooks/stdin" 2>/dev/null || echo 'pre-push did not run')"
fi
# The hook saw the contour's literal, and none of the other overrides.
if [ "$(cat "$hooks/env" 2>/dev/null)" = "$(printf 'WIP=^refs/heads/(wip|task)/\nSTATUSES=\nALLOW=\nDOC=')" ]; then
  pass a-hook-env-is-the-fixed-override
else
  fail a-hook-env-is-the-fixed-override "$(cat "$hooks/env" 2>/dev/null || echo 'pre-push did not run')"
fi
# The wip and main refs, the tags and the other branches did not move: want_task compared the whole list.

fixture a-lease-over-non-ancestor
push_base "$sib"
before="$(refs "$bare")"
push_run "$main" T-900
want_task a-force-with-lease-over-a-non-ancestor-head "$c2" "$before"

fixture a-lease-over-ancestor
push_base "$c1"
before="$(refs "$bare")"
push_run "$main" T-900
want_task a-fast-forward-over-an-ancestor-head "$c2" "$before"

fixture a-same-head
push_base "$c2"
before="$(refs "$bare")"
push_run "$main" T-900
want_task a-push-of-the-head-already-there "$c2" "$before"

# Other refs on the remote stay put: wip/T-900, a tag and a branch, all by the whole-list comparison.
fixture a-other-refs
m push -q --no-verify origin "$c1:refs/heads/wip/T-900" "$sib:refs/heads/side"
m tag -a -m release v1 "$c1"
m push -q --no-verify origin v1
before="$(refs "$bare")"
push_run "$main" T-900
want_task a-wip-tags-and-branches-untouched "$c2" "$before"

# Pushes the head of the branch, not the main tree's HEAD or the worktree's uncommitted state.
fixture a-branch-head-not-main-head
m commit -q --allow-empty -m "chore: main moved on"
before="$(refs "$bare")"
push_run "$main" T-900
want_task a-pushes-the-branch-head-not-main-head "$c2" "$before"

# main advanced after the fork: the fork point stays, the branch head still descends from it.
fixture a-main-advanced
next="$(m commit-tree -p "$fork" -m "chore: main moved" "$fork^{tree}")"
m push -q origin "$next:refs/heads/main"
m update-ref refs/heads/main "$next"
before="$(refs "$bare")"
push_run "$main" T-900
want_task a-main-advanced-head-accepted "$c2" "$before"

# A main tree with no local main: the fork point comes from origin/main.
fixture a-no-local-main
m switch -q --detach
m branch -q -D main
before="$(refs "$bare")"
push_run "$main" T-900
want_task a-no-local-main-uses-origin-main "$c2" "$before"

# The hook refusing stops the push.
fixture a-hook-refusal
touch "$hooks/refuse"
before="$(refs "$bare")"
push_run "$main" T-900
want_refused a-hook-refusal-stops-push "$before"

# ---- Refused: arguments ------------------------------------------------------------------------
refuse_args() {
  local c="$1"
  shift
  fixture "$c"
  before="$(refs "$bare")"
  push_run "$main" "$@"
  want_refused "$c" "$before"
}
fixture r-arg-none
before="$(refs "$bare")"
push_run "$main"
want_refused r-arg-none "$before"
refuse_args r-arg-two T-900 T-900
refuse_args r-arg-two-other T-900 T-901
refuse_args r-arg-extra-refspec T-900 T-900:refs/heads/main
refuse_args r-arg-option-after T-900 --dry-run
refuse_args r-arg-option-before --dry-run T-900
refuse_args r-arg-option-force --force
refuse_args r-arg-option-lease --force-with-lease
refuse_args r-arg-option-all --all
refuse_args r-arg-option-mirror --mirror
refuse_args r-arg-option-tags --tags
refuse_args r-arg-option-delete --delete
refuse_args r-arg-option-no-verify --no-verify
refuse_args r-arg-option-receive-pack --receive-pack=true
refuse_args r-arg-double-dash -- T-900
refuse_args r-arg-refspec-main T-900:refs/heads/main
refuse_args r-arg-refspec-main-short T-900:main
refuse_args r-arg-refspec-force +T-900:refs/heads/task/T-900
refuse_args r-arg-refspec-tag T-900:refs/tags/v1
refuse_args r-arg-refspec-other-task HEAD:refs/heads/task/T-901
refuse_args r-arg-refspec-sha e69de29bb2d1d6434b8b29ae775ad8c2e48c5391:refs/heads/task/T-900
refuse_args r-arg-branch-name tw/T-900
refuse_args r-arg-ref-name refs/heads/tw/T-900
refuse_args r-arg-head HEAD
refuse_args r-arg-remote origin
refuse_args r-arg-empty ""
refuse_args r-arg-id-too-short T-9
refuse_args r-arg-id-two-digits T-90
refuse_args r-arg-id-lowercase t-900
refuse_args r-arg-id-other-prefix X-900
refuse_args r-arg-id-suffix T-900x
refuse_args r-arg-id-prefix xT-900
refuse_args r-arg-id-trailing-space "T-900 "
refuse_args r-arg-id-leading-space " T-900"
refuse_args r-arg-id-newline "$(printf 'T-900\nT-901')"
refuse_args r-arg-id-path "T-900/../T-901"
refuse_args r-arg-id-glob "T-90*"
refuse_args r-arg-id-no-branch T-901

# A short ID with everything else in place (branch, worktree, record): only the ID check refuses it.
fixture r-arg-short-id-complete
w branch -m tw/T-900 tw/T-90
cp "$main/docs/tasks/T-900.md" "$wt/docs/tasks/T-90.md"
w add docs/tasks/T-90.md
w commit -q -m "T-90 record"
before="$(refs "$bare")"
push_run "$main" T-90
want_refused r-arg-short-id-with-branch-worktree-and-record "$before"

# ---- Refused: where it runs, and which branch --------------------------------------------------
fixture r-from-linked-worktree
before="$(refs "$bare")"
push_run "$wt" T-900
want_refused r-run-from-the-linked-worktree "$before"

fixture r-from-subdir
before="$(refs "$bare")"
out="$(cd "$wt/scripts" && timeout 60 ../scripts/tw-task-push T-900 2>&1 </dev/null)"
got=$?
want_refused r-run-from-a-subdirectory-of-the-worktree "$before"

# Run from another linked worktree (not the one on tw/T-900): still not the main tree, so refused;
# the branch's own worktree is left alone, which only the main-tree check can tell apart.
fixture r-from-other-worktree
other="$tmp/r-from-other-worktree/other"
m worktree add -q -b tw/T-902 "$other" main
mkdir -p "$other/scripts"
[ ! -e "$script" ] || cp -p "$script" "$other/scripts/tw-task-push"
before="$(refs "$bare")"
push_run "$other" T-900
want_refused r-run-from-another-linked-worktree "$before"

fixture r-no-worktree
m worktree remove --force "$wt"
before="$(refs "$bare")"
push_run "$main" T-900
want_refused r-branch-with-no-worktree "$before"

# The branch is checked out in the main tree itself: no linked worktree holds it.
fixture r-branch-in-main-tree
m worktree remove --force "$wt"
m switch -q tw/T-900
before="$(refs "$bare")"
push_run "$main" T-900
want_refused r-branch-checked-out-in-the-main-tree-only "$before"

# The worktree is detached at the head: the branch is not checked out anywhere.
fixture r-detached-worktree
w switch -q --detach
before="$(refs "$bare")"
push_run "$main" T-900
want_refused r-worktree-detached-branch-not-checked-out "$before"

# The worktree holds another branch; tw/T-900 stays a branch with no worktree.
fixture r-worktree-other-branch
w switch -q -c tw/T-901
before="$(refs "$bare")"
push_run "$main" T-900
want_refused r-worktree-on-another-branch "$before"

# ---- Refused: a foreign head (no fork point from main) -----------------------------------------
fixture r-foreign-orphan
w reset -q --hard "$orphan"
before="$(refs "$bare")"
push_run "$main" T-900
want_refused r-foreign-orphan-head "$before"

fixture r-no-main
m switch -q --detach
m branch -q -D main
m update-ref -d refs/remotes/origin/main
before="$(refs "$bare")"
push_run "$main" T-900
want_refused r-no-main-anywhere "$before"

# ---- Refused: .github/ (#365, #367) ------------------------------------------------------------
fixture r-github-diff
mkdir -p "$wt/.github/workflows"
printf 'name: x\n' >"$wt/.github/workflows/x.yml"
w add -A
w commit -q -m "T-900 edits a workflow"
before="$(refs "$bare")"
push_run "$main" T-900
want_refused r-github-path-in-the-diff "$before"

# Added, then removed again in the next commit: the diff from the fork point is empty, the push goes.
fixture r-github-diff-reverted
mkdir -p "$wt/.github"
printf 'x\n' >"$wt/.github/a"
w add -A
w commit -q -m "T-900 adds"
w rm -q -r .github
w commit -q -m "T-900 removes"
c2="$(w rev-parse HEAD)"
before="$(refs "$bare")"
push_run "$main" T-900
want_task a-github-added-and-removed-leaves-no-diff "$c2" "$before"

fixture r-github-diff-deleted
# the fork point has the file, the branch deletes it: a path in the diff all the same
mkdir -p "$main/.github"
printf 'x\n' >"$main/.github/a"
m add -A
m commit -q -m "chore: github file on main"
m push -q origin main
w rebase -q main
w rm -q -r .github
w commit -q -m "T-900 deletes a workflow file"
before="$(refs "$bare")"
push_run "$main" T-900
want_refused r-github-path-deleted-in-the-diff "$before"

fixture r-github-prefix-only
mkdir -p "$wt/.githubx"
printf 'x\n' >"$wt/.githubx/a"
w add -A
w commit -q -m "T-900 a directory that only starts like it"
c2="$(w rev-parse HEAD)"
before="$(refs "$bare")"
push_run "$main" T-900
want_task a-dot-githubx-is-not-dot-github "$c2" "$before"

fixture r-github-front-matter
printf '%s\n' '---' 'id: T-900' 'status: IN_PROGRESS' 'seam: "edits .github/workflows/ci.yml"' 'title: "fixture"' '---' '' '## Context' 'body' >"$wt/docs/tasks/T-900.md"
w commit -q -am "T-900 names the directory in its seam"
before="$(refs "$bare")"
push_run "$main" T-900
want_refused r-github-token-in-the-front-matter "$before"

# The token as a bare mention anywhere in the front matter counts, whatever the key.
fixture r-github-front-matter-comment
printf '%s\n' '---' '# mentions .github/ in a comment' 'id: T-900' 'status: IN_PROGRESS' '---' '' 'body' >"$wt/docs/tasks/T-900.md"
w commit -q -am "T-900 comment"
before="$(refs "$bare")"
push_run "$main" T-900
want_refused r-github-token-in-a-front-matter-comment "$before"

# The token in the body only does not refuse (decisions #367).
fixture r-github-body-only
printf '%s\n' '---' 'id: T-900' 'status: IN_PROGRESS' 'title: "fixture"' '---' '' '## Context' 'The task keeps away from .github/ and says so.' >"$wt/docs/tasks/T-900.md"
w commit -q -am "T-900 body mentions the directory"
c2="$(w rev-parse HEAD)"
before="$(refs "$bare")"
push_run "$main" T-900
want_task a-token-in-the-body-only-is-not-refused "$c2" "$before"

# The record is read at the pushed commit, not in the worktree or in the main tree.
fixture r-github-record-in-main-tree
printf '%s\n' '---' 'id: T-900' 'seam: ".github/"' '---' >"$main/docs/tasks/T-900.md"
before="$(refs "$bare")"
push_run "$main" T-900
want_task a-record-is-read-at-the-pushed-commit "$c2" "$before"

fixture r-no-record
w rm -q docs/tasks/T-900.md
w commit -q -m "T-900 drops its record"
before="$(refs "$bare")"
push_run "$main" T-900
want_refused r-missing-record "$before"

# ---- Refused: a dirty worktree (decisions #367) ------------------------------------------------
fixture d-tracked
printf 'dirty\n' >>"$wt/file"
before="$(refs "$bare")"
push_run "$main" T-900
want_refused d-tracked-modification "$before"

fixture d-staged
printf 'new\n' >"$wt/new.txt"
w add new.txt
before="$(refs "$bare")"
push_run "$main" T-900
want_refused d-staged-new-file "$before"

fixture d-deleted
rm "$wt/file"
before="$(refs "$bare")"
push_run "$main" T-900
want_refused d-tracked-file-deleted "$before"

fixture d-untracked
printf 'stray\n' >"$wt/stray.txt"
before="$(refs "$bare")"
push_run "$main" T-900
want_refused d-untracked-file-outside-teamwright-tmp "$before"

fixture d-untracked-nested
mkdir -p "$wt/some/dir"
printf 'stray\n' >"$wt/some/dir/stray.txt"
before="$(refs "$bare")"
push_run "$main" T-900
want_refused d-untracked-file-in-a-new-directory "$before"

fixture d-untracked-teamwright-other
mkdir -p "$wt/.teamwright"
printf 'x\n' >"$wt/.teamwright/state.json"
before="$(refs "$bare")"
push_run "$main" T-900
want_refused d-untracked-file-in-teamwright-outside-tmp "$before"

fixture d-untracked-tmp-prefix
mkdir -p "$wt/.teamwright/tmpx"
printf 'x\n' >"$wt/.teamwright/tmpx/a"
before="$(refs "$bare")"
push_run "$main" T-900
want_refused d-untracked-teamwright-tmpx-is-not-tmp "$before"

fixture d-untracked-tmp-and-stray
mkdir -p "$wt/.teamwright/tmp"
printf 'x\n' >"$wt/.teamwright/tmp/a.log"
printf 'stray\n' >"$wt/stray.txt"
before="$(refs "$bare")"
push_run "$main" T-900
want_refused d-untracked-in-tmp-does-not-hide-a-stray-file "$before"

fixture d-untracked-tmp
mkdir -p "$wt/.teamwright/tmp/deep"
printf 'x\n' >"$wt/.teamwright/tmp/a.log"
printf 'x\n' >"$wt/.teamwright/tmp/deep/b.log"
before="$(refs "$bare")"
push_run "$main" T-900
want_task d-untracked-file-inside-teamwright-tmp-allowed "$c2" "$before"

# ---- TEAMWRIGHT_PAUSE, each place on its own, with the hook and without it ---------------------
pause_case() { # <case> <place: main|wt|file> <hook: yes|no> [<ALLOW_PUSH value>]
  local c="$1" place="$2" usehook="$3" allow="${4:-}"
  fixture "$c"
  [ "$usehook" = yes ] || m config --unset core.hooksPath
  case "$place" in
    main) touch "$main/TEAMWRIGHT_PAUSE" ;;
    wt) touch "$wt/TEAMWRIGHT_PAUSE" ;;
    file) touch "$tmp/$c/pause" ;;
  esac
  before="$(refs "$bare")"
  if [ "$place" = file ]; then
    if [ -n "$allow" ]; then
      TEAMWRIGHT_ALLOW_PUSH="$allow" TEAMWRIGHT_PAUSE_FILE="$tmp/$c/pause" push_run "$main" T-900
    else
      TEAMWRIGHT_PAUSE_FILE="$tmp/$c/pause" push_run "$main" T-900
    fi
  elif [ -n "$allow" ]; then
    TEAMWRIGHT_ALLOW_PUSH="$allow" push_run "$main" T-900
  else
    push_run "$main" T-900
  fi
  want_refused "$c" "$before"
}
pause_case p-main-hook main yes
pause_case p-main-nohook main no
pause_case p-wt-hook wt yes
pause_case p-wt-nohook wt no
pause_case p-file-hook file yes
pause_case p-file-nohook file no
pause_case p-main-allow-push main yes 1
pause_case p-main-allow-push-nohook main no 1
pause_case p-wt-allow-push wt yes 1
pause_case p-file-allow-push file yes 1

# ---- Environment: nothing inherited widens or drops anything -----------------------------------
# A hooksPath override through the environment must not drop the pre-push hook, whose refusal stops it.
fixture e-config-parameters
touch "$hooks/refuse"
before="$(refs "$bare")"
GIT_CONFIG_PARAMETERS="'core.hooksPath=/dev/null'" push_run "$main" T-900
want_refused e-config-parameters-keeps-the-hook "$before"

fixture e-config-count
touch "$hooks/refuse"
before="$(refs "$bare")"
GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.hooksPath GIT_CONFIG_VALUE_0=/dev/null push_run "$main" T-900
want_refused e-config-count-keeps-the-hook "$before"

# Inherited overrides: the hook sees the fixed literal and none of the others; the push is the same.
fixture e-inherited-overrides
before="$(refs "$bare")"
TEAMWRIGHT_WIP_REFS_RE='^refs/heads/nothing/' TEAMWRIGHT_PUSH_STATUSES='IN_PROGRESS TODO' \
  TEAMWRIGHT_DOC_PATHS_RE='.*' TEAMWRIGHT_ALLOW_PUSH=1 push_run "$main" T-900
want_task e-inherited-overrides-push-still-works "$c2" "$before"
if [ "$(cat "$hooks/env" 2>/dev/null)" = "$(printf 'WIP=^refs/heads/(wip|task)/\nSTATUSES=\nALLOW=\nDOC=')" ]; then
  pass e-inherited-overrides-replaced-by-the-fixed-value
else
  fail e-inherited-overrides-replaced-by-the-fixed-value "$(cat "$hooks/env" 2>/dev/null || echo 'pre-push did not run')"
fi

# A widening WIP value does not widen the push: only the task ref changes, whatever the value.
fixture e-env-git-dir
otherbare="$tmp/e-env-git-dir/other.git"
git init -q --bare -b main "$otherbare"
before="$(refs "$bare")"
GIT_DIR="$otherbare" GIT_WORK_TREE="$tmp" push_run "$main" T-900
want_task e-git-dir-from-the-environment-ignored "$c2" "$before"
if [ -z "$(refs "$otherbare")" ]; then
  pass e-git-dir-other-repository-untouched
else
  fail e-git-dir-other-repository-untouched "$(refs "$otherbare")"
fi

# ---- Clone config: another remote, push.followTags and a configured refspec do not widen it ----
fixture o-other-remote
other="$tmp/o-other-remote/other.git"
git init -q --bare -b main "$other"
m remote add other "$other"
m config remote.pushDefault other
m config branch.tw/T-900.remote other
m config branch.tw/T-900.pushRemote other
before="$(refs "$bare")"
push_run "$main" T-900
want_task o-origin-is-fixed "$c2" "$before"
if [ -z "$(refs "$other")" ]; then
  pass o-other-remote-untouched
else
  fail o-other-remote-untouched "$(refs "$other")"
fi

fixture o-follow-tags
m tag -a -m release v1 "$c1"
m config push.followTags true
before="$(refs "$bare")"
push_run "$main" T-900
want_task o-no-tag-with-follow-tags "$c2" "$before"

fixture o-config-refspec
m config --add remote.origin.push 'refs/heads/*:refs/heads/*'
m config push.default matching
m branch -q side "$c1"
before="$(refs "$bare")"
push_run "$main" T-900
want_task o-no-ref-from-configured-refspec "$c2" "$before"

# The ID is the argument, never the commit's message or another branch at the commit.
fixture o-id-from-argument
msg="$(m commit-tree -p "$fork" -m "T-901 belongs elsewhere" "$fork^{tree}")"
m branch -q tw/T-901 "$msg"
before="$(refs "$bare")"
push_run "$main" T-900
want_task o-id-is-the-argument "$c2" "$before"

# ---- Lease: the remote ref moves between the script's read and the update ----------------------
# The wrapper hook moves task/T-900 on the remote (to fork, an existing commit) after the script
# read it. The update must be refused by the remote's old-value check; the moved value stays.
fixture l-mismatch-absent
printf '%s' "$fork" >"$hooks/move"
before="$(refs "$bare")"
push_run "$main" T-900
now="$(refs "$bare")"
want="$(remote_with "$before" refs/heads/task/T-900 "$fork")"
if ! ran; then
  fail "l-mismatch-when-the-ref-was-absent: the script did not run (exit $got)" "$out"
elif [ "$got" -eq 0 ]; then
  fail "l-mismatch-when-the-ref-was-absent: exit 0 (want refused)" "$out"
elif [ "$now" != "$want" ]; then
  fail "l-mismatch-when-the-ref-was-absent: the remote ref was overwritten" "$(printf 'want:\n%s\ngot:\n%s' "$want" "$now")"
else
  pass l-mismatch-when-the-ref-was-absent
fi

fixture l-mismatch-present
push_base "$sib"
printf '%s' "$fork" >"$hooks/move"
before="$(refs "$bare")"
push_run "$main" T-900
now="$(refs "$bare")"
want="$(remote_with "$before" refs/heads/task/T-900 "$fork")"
if ! ran; then
  fail "l-mismatch-when-the-ref-existed: the script did not run (exit $got)" "$out"
elif [ "$got" -eq 0 ]; then
  fail "l-mismatch-when-the-ref-existed: exit 0 (want refused)" "$out"
elif [ "$now" != "$want" ]; then
  fail "l-mismatch-when-the-ref-existed: the remote ref was overwritten" "$(printf 'want:\n%s\ngot:\n%s' "$want" "$now")"
else
  pass l-mismatch-when-the-ref-existed
fi

# The ref moves right after the script read it, before the push connects: only the lease the script
# built from that read (and not a forced update, nor a blind create) refuses it; receive-pack's
# old-value check cannot, it is handed the moved value by the push's own advertisement.
stale_case() { # <case> <base commit or empty>
  local c="$1" base="$2" now want
  fixture "$c"
  [ -z "$base" ] || push_base "$base"
  printf '%s' "$fork" >"$tmp/$c/stale"
  before="$(refs "$bare")"
  push_run "$main" T-900
  now="$(refs "$bare")"
  want="$(remote_with "$before" refs/heads/task/T-900 "$fork")"
  if ! ran; then
    fail "$c: the script did not run (exit $got)" "$out"
  elif [ "$got" -eq 0 ]; then
    fail "$c: exit 0 (want refused)" "$out"
  elif [ "$now" != "$want" ]; then
    fail "$c: the remote ref was overwritten" "$(printf 'want:\n%s\ngot:\n%s' "$want" "$now")"
  else
    pass "$c"
  fi
}
stale_case l-stale-read-when-the-ref-was-absent ""
stale_case l-stale-read-when-the-ref-existed "$sib"
# Control: the same wrapper, nothing to move, is a plain push.
fixture l-wrapper-control
before="$(refs "$bare")"
push_run "$main" T-900
want_task l-upload-wrapper-without-a-move-is-a-plain-push "$c2" "$before"

# ==== B. static pins: the script text and .claude/settings.json ==================================
static_out="$(SCRIPT="$script" SETTINGS="$settings" CONTOUR="$contour_push" python3 -I - <<'PY'
import json, os, re

def check(name, ok, detail=""):
    print(("ok " if ok else "FAIL ") + name + ("" if ok else " :: " + detail.replace("\n", " ")))

# ---- the script text
path = os.environ["SCRIPT"]
if not os.path.isfile(path):
    for n in ("the script holds no + refspec", "the script pushes with a lease on the task ref",
              "the script's override value is the contour's, byte for byte",
              "the script never skips the hook", "the script refuses the tags and follow-tags config"):
        check(n, False, "scripts/tw-task-push missing")
else:
    text = open(path, encoding="utf-8").read()
    contour = open(os.environ["CONTOUR"], encoding="utf-8").read()
    m = re.search(r"TEAMWRIGHT_WIP_REFS_RE: '([^'\n]*)'", contour)
    literal = m.group(1) if m else None
    check("the contour's override value is found", literal == "^refs/heads/(wip|task)/", repr(literal))
    check("the script's override value is the contour's, byte for byte",
          bool(literal) and re.search(r"TEAMWRIGHT_WIP_REFS_RE=(['\"])" + re.escape(literal) + r"\1", text) is not None,
          "no assignment of " + repr(literal))
    # the push command, backslash continuations joined
    joined = re.sub(r"\\\n\s*", " ", text)
    pushes = [l for l in joined.splitlines() if re.search(r"(^|[\s;&|(])git\s+push\b", l) and not l.lstrip().startswith("#")]
    check("the script has one push command", len(pushes) == 1, repr(pushes))
    p = pushes[0] if pushes else ""
    toks = p.replace('"', "").replace("'", "").split()
    check("the script holds no + refspec", pushes != [] and not any(t.startswith("+") for t in toks), p)
    check("the script pushes with a lease on the task ref",
          re.search(r"--force-with-lease=\S*:", " ".join(toks)) is not None and "--force " not in p + " "
          and not re.search(r"\s-f(\s|$)", p) and "--force-with-lease " not in p + " ", p)
    check("the script never skips the hook", "--no-verify" not in text and "core.hooksPath" not in text, "no-verify or hooksPath in the text")
    check("the script refuses the tags and follow-tags config", "--no-follow-tags" in p and "--no-recurse-submodules" in p, p)

# ---- settings
s = json.load(open(os.environ["SETTINGS"], encoding="utf-8"))
perms = s.get("permissions") or {}
allow, deny = perms.get("allow") or [], perms.get("deny") or []

def matches(rule, command):
    """A Bash(...) rule's prefix-and-wildcard reading: `*` spans anything, spaces included, `:*` is the
    old prefix form (as scripts/ci/tw-wip-push.test.sh)."""
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

want_allow = {"Bash(scripts/tw-wip-push)", "Bash(scripts/tw-wip-push *)", "Bash(scripts/tw-task-push *)"}
check("allow holds exactly the two tw-wip-push rules and the tw-task-push rule",
      len(allow) == 3 and set(allow) == want_allow, repr(allow))
check("tw-task-push runs under allow with an ID", any(matches(r, "scripts/tw-task-push T-900") for r in allow), repr(allow))
named = [r for r in allow if re.search(r"git\b.*\bpush|\bmain\b|\btags?\b|refs/", r)]
check("no allow rule names a git push, main or a tag", not named, repr(named))
for cmd in ("git push origin main", "git push origin HEAD:refs/heads/task/T-900",
            "git push --force-with-lease origin HEAD:refs/heads/task/T-900", "git push origin refs/tags/v1",
            "git -C . push origin main", "git -c x=y push origin main",
            "TEAMWRIGHT_WIP_REFS_RE='^refs/heads/(wip|task)/' git push origin HEAD:refs/heads/task/T-900"):
    hit = [r for r in allow if matches(r, cmd)]
    check("no allow rule matches: " + cmd, not hit, repr(hit))
for cmd in ("scripts/tw-task-push T-900", "scripts/tw-wip-push", "git -C . status"):
    hit = [r for r in deny if matches(r, cmd)]
    check("deny does not match: " + cmd, not hit, repr(hit))
for cmd in ("git -C . push origin main", "git -c x=y push origin main",
            "git push --force-with-lease origin HEAD:refs/heads/task/T-900"):
    check("deny matches: " + cmd, any(matches(r, cmd) for r in deny), repr(deny))
for rule in ("Bash(git push --force *)", "Bash(git push --force-with-lease *)", "Bash(git push -f *)",
             "Bash(git push * --force*)", "Bash(git push * -f)", "Bash(git push * -f *)",
             "Bash(git -C * push*)", "Bash(git -c * push*)", "Bash(TEAMWRIGHT_ALLOW_PUSH=*)"):
    check("deny keeps: " + rule, rule in deny, repr(deny))
PY
)"
rc=$?
if [ "$rc" -ne 0 ] || [ -z "$static_out" ]; then
  fail "static: the check did not run (python exit $rc)" "$static_out"
fi
while IFS= read -r line; do
  case "$line" in
    "ok "*) pass "static: ${line#ok }" ;;
    "FAIL "*) l="${line#FAIL }"; fail "static: ${l%% :: *}" "${l#* :: }" ;;
  esac
done <<<"$static_out"

# ==== C. scripts/hooks/wip-push-guard.sh, fed Claude Code hook JSON ============================
hookdir="$tmp/hook-cwd"
mkdir -p "$hookdir"
hook_decision() { # <command>: prints deny | silent | other:<detail>
  local input o rc
  input="$(CMD="$1" CWD="$hookdir" python3 -I -c '
import json, os
print(json.dumps({"session_id": "fixture", "cwd": os.environ["CWD"], "hook_event_name": "PreToolUse",
                  "tool_name": "Bash", "tool_input": {"command": os.environ["CMD"]}}))')"
  o="$(cd "$hookdir" && CLAUDE_PROJECT_DIR="$hookdir" timeout 30 bash "$hook" <<<"$input" 2>/dev/null)"
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
  got_d="$(hook_decision "$cmd")"
  if [ "$got_d" = "$want" ]; then
    pass "hook $want: $cmd"
  else
    fail "hook $want: $cmd" "got: $got_d"
  fi
}
hook_case silent "scripts/tw-task-push T-900"
hook_case silent "scripts/tw-task-push T-265"
hook_case deny "git -C . push origin HEAD:refs/heads/task/T-900"
hook_case deny "git -c core.hooksPath=/dev/null push origin HEAD:refs/heads/task/T-900"

# The script's own text must not hold the token the hook refuses (a hook reading it would block a run).
if [ -f "$script" ]; then
  if grep -qE 'git[[:space:]]+-[A-Za-z-].*[[:space:]]push' "$script"; then
    fail "the script text holds no git-option push token" "$(grep -nE 'git[[:space:]]+-[A-Za-z-].*[[:space:]]push' "$script")"
  else
    pass "the script text holds no git-option push token"
  fi
else
  fail "the script text holds no git-option push token" "scripts/tw-task-push missing"
fi

echo "tw-task-push.test: $passed passed, $failed failed"
[ "$failed" -eq 0 ] || exit 1
