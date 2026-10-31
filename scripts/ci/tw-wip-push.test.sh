#!/usr/bin/env bash
# T-192 (decisions #208, #219; docs/tasks/T-192.md): host test of the only sanctioned CI push.
#
# Three subjects, one guarantee each:
#   A. scripts/tw-wip-push [<commit>] writes refs/heads/wip/<ID> on origin and nothing else, <ID>
#      from branch tw/<ID>, <commit> (default HEAD) descending from the fork point from main;
#      anything else is refused with no ref moved; the pre-push hook runs; TEAMWRIGHT_PAUSE stops it.
#   B. scripts/hooks/wip-push-guard.sh, a PreToolUse hook, refuses the `git -C` and `git -c` push
#      forms by reading the command (project hook contract, scripts/hooks/_hookio.py: deny = one JSON
#      object with permissionDecision=deny, exit 0; allow = no output, exit 0).
#   C. .claude/settings.json: allow holds the two script rules only, deny holds the push forms,
#      PreToolUse wires the hook.
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
CMDS
hook_case deny "$(printf 'true\ngit -C . push origin main')"
hook_case deny "$(printf 'true\ngit -c x=y push origin main')"

# Not refused: the script, and git with the same options doing something else.
while IFS= read -r c; do
  [ -z "$c" ] || hook_case silent "$c"
done <<'CMDS'
scripts/tw-wip-push
scripts/tw-wip-push 0123456789abcdef
git -C . status
git -c color.ui=never log --oneline
git -C . log --grep=push
git -C . stash push
git -C . remote get-url --push origin
git -C docs status && echo push
git -C . commit -m "do not push"
git status
echo git
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

echo "tw-wip-push.test: $passed passed, $failed failed"
[ "$failed" -eq 0 ] || exit 1
