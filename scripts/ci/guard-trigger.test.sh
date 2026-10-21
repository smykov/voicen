#!/usr/bin/env bash
# T-113 (rca, class guard-model, decisions #154 and #155, exit c): host test of the P-020
# mechanical trigger in the git pre-commit hook scripts/hooks/pre-commit. P-020's design-time step
# (`checker: true` plus `analysis.decides_on`) fired only when the analysis author remembered it:
# T-082 predates the template's `checker:` field, so its guard (role-tools.sh) was designed as a
# model of Claude Code's front matter and caught only in review (T-113 Investigation, hypothesis 5).
# T-113 review 1 #1 then found the first trigger deciding "no guard change" on its own model of
# Make and shell text (probes P1-P6 allowed); #155 replaces that model with a raw token.
#
# Contract (pre-commit, run in the repository being committed to), decides_on token (#155):
#   Guard change: a path listed by `git diff --cached --name-only --no-renames` (any status:
#     added, modified, deleted, type-changed; a rename counts as its deleted old path and its
#     added new path) that
#       - starts with scripts/ci/ or scripts/licenses/, or
#       - is the path Makefile, or
#       - ends in .mk (any depth).
#     Nothing else decides: no Make, shell or file-content reading. Every change to such a path is
#     a guard change, including one unrelated to any check (accepted cost of #155).
#   Active task: $TEAMWRIGHT_TASK, else the first word of .teamwright/current-task (as the
#     analysis gate reads it); its record docs/tasks/<ID>.md (read by scripts/hooks/_task.py).
#   A commit with a guard change is refused (exit != 0, a message naming the staged guard path and
#     the missing field, `checker` or `decides_on`) unless the active task's front matter has
#     `checker: true` and its `analysis:` block a non-empty, non-placeholder `decides_on`.
#     A missing `checker:` key counts as unset (false); no active task, or no record for it,
#     refuses too. Text in the record's body does not count.
#   A commit without a guard change is not affected (its result is the hook's other checks').
# Characterization (green before T-113, must stay green): the hook's existing refusal of a staged
#   .env file, and an ordinary commit passing.
# Pinning (refused before #155 too, must stay refused): a changed check recipe, a target removed
#   from `check:`, a deleted guard (git rm), a guard renamed in place or moved out of scripts/ci/.
# Outside the contract: the exit code a refusal uses (any non-zero); a root GNUmakefile or
#   makefile (not in the token; docs/process/gates.md › Guard trigger › Not caught).
#
# Usage: scripts/ci/guard-trigger.test.sh   (host bash, git, python3)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
hook="$root/scripts/hooks/pre-commit"
[ -f "$hook" ] || { echo "guard-trigger.test: cannot run: $hook missing" >&2; exit 3; }
command -v git >/dev/null 2>&1 || { echo "guard-trigger.test: cannot run: git missing" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "guard-trigger.test: cannot run: mktemp failed" >&2; exit 3; }
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

g() { git -C "$repo" -c user.name=fixture -c user.email=fixture@example.com -c commit.gpgsign=false "$@"; }

# task <file> <checker line or -> <decides_on line inside analysis or -> [extra top-level line]
# [body text]: a task record in IN_PROGRESS with a complete analysis block.
task() {
  {
    echo '---'
    echo "id: $(basename "$1" .md)"
    echo 'title: "Fixture task"'
    echo 'type: bug'
    echo 'status: IN_PROGRESS'
    [ "$2" = "-" ] || echo "$2"
    [ -z "${4:-}" ] || echo "$4"
    echo 'analysis:'
    echo '  root_cause: "a fixture root cause"'
    echo '  evidence: "a fixture evidence"'
    echo '  invariant: "a fixture invariant"'
    echo '  seam: "scripts/ci/foo.sh"'
    echo '  approach: "a fixture approach"'
    [ "$3" = "-" ] || echo "  $3"
    echo 'implemented_by: ""'
    echo '---'
    echo
    echo '## Context'
    [ -z "${5:-}" ] || printf '%s\n' "$5"
  } >"$1"
}

# repo <case> <checker line or -> <decides_on line or -> [extra top-level line] [body text]:
# a fixture repository with one guard (check-foo runs scripts/ci/foo.test.sh, which runs
# scripts/ci/foo.sh), a non-check target, a source file, a doc, and task T-900 (record committed,
# .teamwright/current-task = T-900, untracked). Sets $repo.
repo() {
  repo="$tmp/$1"
  mkdir -p "$repo/scripts/ci" "$repo/src" "$repo/docs/tasks" "$repo/.teamwright"
  git init -q "$repo"
  printf '.teamwright/\n' >"$repo/.gitignore"
  cat >"$repo/Makefile" <<'MK'
.PHONY: check fmt
check: check-foo

# check-foo: the fixture guard.
check-foo:
	scripts/ci/foo.test.sh

fmt:
	true
MK
  printf '#!/usr/bin/env bash\n# fixture self-test\nscripts/ci/foo.sh fixtures\n' >"$repo/scripts/ci/foo.test.sh"
  printf '#!/usr/bin/env bash\n# fixture guard\ngrep -q owned "$1" || exit 1\n' >"$repo/scripts/ci/foo.sh"
  chmod +x "$repo/scripts/ci/foo.test.sh" "$repo/scripts/ci/foo.sh"
  mkdir -p "$repo/scripts/licenses"
  printf '// fixture license allowlist\nexport const accepted = ["MIT"];\n' \
    >"$repo/scripts/licenses/accepted.mjs"
  printf 'fn main() {}\n' >"$repo/src/app.rs"
  printf '# Notes\n' >"$repo/docs/notes.md"
  task "$repo/docs/tasks/T-900.md" "$2" "$3" "${4:-}" "${5:-}"
  printf 'T-900\n' >"$repo/.teamwright/current-task"
  g add -A >/dev/null
  g commit -q --no-verify -m init >/dev/null
}

# setmk <makefile text> [<path> <text>]...: replaces the committed Makefile (and writes each extra
# file) in the fixture's base commit, so the next staged change is measured against it. The
# review-1 probes P1-P6 (T-113 reviews/1.md #1) use it for the Make shapes the first trigger
# modelled away.
setmk() {
  printf '%s\n' "$1" >"$repo/Makefile"
  shift
  while [ "$#" -ge 2 ]; do
    mkdir -p "$(dirname "$repo/$1")"
    printf '%s\n' "$2" >"$repo/$1"
    shift 2
  done
  g add -A >/dev/null
  g commit -q --no-verify --amend -m init >/dev/null
}

# run <case> <want: allow|refuse> [needle...]: runs the hook in $repo on what is staged.
# Env: TEAMWRIGHT_TASK only when $env_task is set; gitleaks off (the fallback scanner runs).
env_task=""
run() {
  local c="$1" want="$2" out got n
  shift 2
  if [ -n "$env_task" ]; then
    out="$(cd "$repo" && TEAMWRIGHT_NO_GITLEAKS=1 TEAMWRIGHT_TASK="$env_task" bash "$hook" 2>&1)"
  else
    out="$(cd "$repo" && env -u TEAMWRIGHT_TASK TEAMWRIGHT_NO_GITLEAKS=1 bash "$hook" 2>&1)"
  fi
  got=$?
  if [ "$want" = allow ] && [ "$got" -ne 0 ]; then
    fail "$c: exit $got (want 0, commit allowed)" "$out"
    return
  fi
  if [ "$want" = refuse ] && [ "$got" -eq 0 ]; then
    fail "$c: exit 0 (want refused)" "$out"
    return
  fi
  for n in "$@"; do
    if ! grep -qF -- "$n" <<<"$out"; then
      fail "$c: exit $got, output lacks '$n'" "$out"
      return
    fi
  done
  pass "$c: exit $got"
}

ok_checker='checker: true'
ok_decides='decides_on: "literal: the owned line of scripts/ci/foo.sh"'

# ---- Characterization (pre-commit as it is before T-113) ------------------------------------
repo char-ordinary - -
printf 'fn main() { let _ = 1; }\n' >"$repo/src/app.rs"
g add src/app.rs
run char-ordinary allow
repo char-env-file "$ok_checker" "$ok_decides"
printf 'A=1\n' >"$repo/.env"
g add -f .env
run char-env-file refuse ".env"

# ---- Allowed: guard change in a checker task with decides_on --------------------------------
repo allow-guard-checker "$ok_checker" "$ok_decides"
printf 'grep -qxF owned "$1" || exit 1\n' >>"$repo/scripts/ci/foo.sh"
g add scripts/ci/foo.sh
run allow-guard-checker allow
repo allow-new-target-checker "$ok_checker" "$ok_decides"
printf '\ncheck-bar:\n\tscripts/ci/bar.sh\n' >>"$repo/Makefile"
printf '#!/usr/bin/env bash\nexit 0\n' >"$repo/scripts/ci/bar.sh"
g add Makefile scripts/ci/bar.sh
run allow-new-target-checker allow
# The active task from $TEAMWRIGHT_TASK (no current-task file).
repo allow-env-task "$ok_checker" "$ok_decides"
rm "$repo/.teamwright/current-task"
printf '# changed\n' >>"$repo/scripts/ci/foo.test.sh"
g add scripts/ci/foo.test.sh
env_task=T-900
run allow-env-task allow
env_task=""

# ---- Refused: guard change, checker unset or decides_on empty --------------------------------
# The T-082 shape: a record with no checker: key at all, a guard run through the self-test.
repo refuse-checker-missing - "$ok_decides"
printf 'exit 0\n' >>"$repo/scripts/ci/foo.sh"
g add scripts/ci/foo.sh
run refuse-checker-missing refuse "scripts/ci/foo.sh" "checker"
repo refuse-checker-false 'checker: false' "$ok_decides"
printf 'exit 0\n' >>"$repo/scripts/ci/foo.sh"
g add scripts/ci/foo.sh
run refuse-checker-false refuse "scripts/ci/foo.sh" "checker"
repo refuse-checker-in-body - "$ok_decides" "" 'checker: true'
printf 'exit 0\n' >>"$repo/scripts/ci/foo.sh"
g add scripts/ci/foo.sh
run refuse-checker-in-body refuse "scripts/ci/foo.sh" "checker"
# The script named directly in the check recipe.
repo refuse-recipe-script - "$ok_decides"
printf 'exit 0\n' >>"$repo/scripts/ci/foo.test.sh"
g add scripts/ci/foo.test.sh
run refuse-recipe-script refuse "scripts/ci/foo.test.sh" "checker"
repo refuse-decides-empty "$ok_checker" 'decides_on: ""'
printf 'exit 0\n' >>"$repo/scripts/ci/foo.sh"
g add scripts/ci/foo.sh
run refuse-decides-empty refuse "scripts/ci/foo.sh" "decides_on"
repo refuse-decides-missing "$ok_checker" -
printf 'exit 0\n' >>"$repo/scripts/ci/foo.sh"
g add scripts/ci/foo.sh
run refuse-decides-missing refuse "scripts/ci/foo.sh" "decides_on"
repo refuse-decides-placeholder "$ok_checker" 'decides_on: "<switch | literal | token>"'
printf 'exit 0\n' >>"$repo/scripts/ci/foo.sh"
g add scripts/ci/foo.sh
run refuse-decides-placeholder refuse "scripts/ci/foo.sh" "decides_on"
# decides_on outside the analysis block is not analysis.decides_on.
repo refuse-decides-top-level "$ok_checker" - 'decides_on: "literal: x"'
printf 'exit 0\n' >>"$repo/scripts/ci/foo.sh"
g add scripts/ci/foo.sh
run refuse-decides-top-level refuse "scripts/ci/foo.sh" "decides_on"
# A new check-* target with a new script (both staged), checker unset.
repo refuse-new-target - -
printf '\ncheck-bar:\n\tscripts/ci/bar.sh\n' >>"$repo/Makefile"
printf '#!/usr/bin/env bash\nexit 0\n' >"$repo/scripts/ci/bar.sh"
g add Makefile scripts/ci/bar.sh
run refuse-new-target refuse "checker"
# A new check-* target alone (recipe outside scripts/ci).
repo refuse-new-target-only - -
printf '\ncheck-baz:\n\ttrue\n' >>"$repo/Makefile"
g add Makefile
run refuse-new-target-only refuse "Makefile" "checker"
# A guard change mixed with ordinary changes.
repo refuse-mixed - -
printf 'exit 0\n' >>"$repo/scripts/ci/foo.sh"
printf 'fn main() { let _ = 2; }\n' >"$repo/src/app.rs"
g add scripts/ci/foo.sh src/app.rs
run refuse-mixed refuse "scripts/ci/foo.sh"
# No active task, or an active task without a record.
repo refuse-no-active-task "$ok_checker" "$ok_decides"
rm "$repo/.teamwright/current-task"
printf 'exit 0\n' >>"$repo/scripts/ci/foo.sh"
g add scripts/ci/foo.sh
run refuse-no-active-task refuse "scripts/ci/foo.sh"
repo refuse-no-record "$ok_checker" "$ok_decides"
printf 'T-901\n' >"$repo/.teamwright/current-task"
printf 'exit 0\n' >>"$repo/scripts/ci/foo.sh"
g add scripts/ci/foo.sh
run refuse-no-record refuse "scripts/ci/foo.sh"

# ---- Unaffected: no guard change, checker unset ---------------------------------------------
repo none-source - -
printf 'fn main() { let _ = 3; }\n' >"$repo/src/app.rs"
g add src/app.rs
run none-source allow
repo none-docs - -
printf 'More.\n' >>"$repo/docs/notes.md"
g add docs/notes.md
run none-docs allow
repo none-no-active-task - -
rm "$repo/.teamwright/current-task"
printf 'fn main() { let _ = 4; }\n' >"$repo/src/app.rs"
g add src/app.rs
run none-no-active-task allow
# Near misses of the token: not under scripts/ci/ or scripts/licenses/ (a sibling with the same
# stem, the prefix nested under docs/), not the path Makefile, not ending in .mk.
repo none-near-miss - -
mkdir -p "$repo/scripts/ci-notes" "$repo/docs/scripts/ci" "$repo/docs/scripts/licenses"
printf 'x\n' >"$repo/scripts/ci-notes/a.md"
printf 'x\n' >"$repo/scripts/licenses.md"
printf 'x\n' >"$repo/docs/scripts/ci/b.md"
printf 'x\n' >"$repo/docs/scripts/licenses/c.md"
printf 'x\n' >"$repo/docs/Makefile.md"
printf 'x\n' >"$repo/docs/rules.mkd"
g add -A
run none-near-miss allow

# ---- #155 token: refused although no check reads the path (red against the Make model) -------
# The review-1 probes P1-P6 (T-113 reviews/1.md #1), task without checker:. P1-P5 edit
# scripts/ci/foo.sh, which the first trigger's Make reading did not connect to `check`.
p_edit_foo() { printf 'exit 0\n' >>"$repo/scripts/ci/foo.sh"; g add scripts/ci/foo.sh; }
repo refuse-p1-delegate - -
setmk $'.PHONY: check\ncheck: check-foo\ncheck-foo: foo-run\nfoo-run:\n\tscripts/ci/foo.sh x'
p_edit_foo
run refuse-p1-delegate refuse "scripts/ci/foo.sh" "checker"
repo refuse-p2-variable - -
setmk $'CI := scripts/ci\n.PHONY: check\ncheck: check-foo\ncheck-foo:\n\t$(CI)/foo.sh x'
p_edit_foo
run refuse-p2-variable refuse "scripts/ci/foo.sh" "checker"
repo refuse-p3-include - -
setmk $'include guards.mk\n.PHONY: check\ncheck: check-foo' \
  guards.mk $'check-foo:\n\tscripts/ci/foo.sh x'
p_edit_foo
run refuse-p3-include refuse "scripts/ci/foo.sh" "checker"
repo refuse-p4-quoted - -
setmk $'.PHONY: check\ncheck: check-foo\ncheck-foo:\n\tscripts/"ci"/foo.sh x'
p_edit_foo
run refuse-p4-quoted refuse "scripts/ci/foo.sh" "checker"
repo refuse-p5-check-prereq - -
setmk $'.PHONY: check\ncheck: guard-foo\nguard-foo:\n\tscripts/ci/foo.sh x'
p_edit_foo
run refuse-p5-check-prereq refuse "scripts/ci/foo.sh" "checker"
# P6: a Makefile-only edit that sets the recipe of check-foo's prerequisite to `true`.
repo refuse-p6-prereq-recipe - -
setmk $'.PHONY: check\ncheck: check-foo\ncheck-foo: foo-run\nfoo-run:\n\tscripts/ci/foo.sh x'
sed -i 's|scripts/ci/foo.sh x|true|' "$repo/Makefile"
g add Makefile
run refuse-p6-prereq-recipe refuse "Makefile" "checker"
# A Makefile change unrelated to any check (accepted cost of #155; was allowed before).
repo refuse-makefile-other-target - -
printf '\nlint:\n\ttrue\n' >>"$repo/Makefile"
g add Makefile
run refuse-makefile-other-target refuse "Makefile" "checker"
# A *.mk file: the included guards.mk edited alone, and a new *.mk two levels down.
repo refuse-mk-included - -
setmk $'include guards.mk\n.PHONY: check\ncheck: check-foo' \
  guards.mk $'check-foo:\n\tscripts/ci/foo.sh x'
printf 'check-foo:\n\ttrue\n' >"$repo/guards.mk"
g add guards.mk
run refuse-mk-included refuse "guards.mk" "checker"
repo refuse-mk-nested - -
mkdir -p "$repo/build/mk"
printf 'lint:\n\ttrue\n' >"$repo/build/mk/rules.mk"
g add build/mk/rules.mk
run refuse-mk-nested refuse "build/mk/rules.mk" "checker"
# scripts/licenses/ (review 1 #4): a license guard edited by a task without checker:.
repo refuse-licenses - -
printf 'export const extra = ["GPL-3.0"];\n' >>"$repo/scripts/licenses/accepted.mjs"
g add scripts/licenses/accepted.mjs
run refuse-licenses refuse "scripts/licenses/accepted.mjs" "checker"
# A file under scripts/ci/ that no check runs (a data fragment), added.
repo refuse-ci-fragment - -
printf 'Some Window Class\n' >"$repo/scripts/ci/helper-windows.txt"
g add scripts/ci/helper-windows.txt
run refuse-ci-fragment refuse "scripts/ci/helper-windows.txt" "checker"
# A file moved into scripts/ci/ from outside (the added side of a rename).
repo refuse-moved-in - -
printf '#!/usr/bin/env bash\nexit 0\n' >"$repo/src/tool.sh"
g add src/tool.sh
g commit -q --no-verify --amend -m init >/dev/null
g mv src/tool.sh scripts/ci/tool.sh
run refuse-moved-in refuse "scripts/ci/tool.sh" "checker"
# A type change (T): the guard replaced by a symlink to an always-passing script.
repo refuse-type-change - -
printf '#!/usr/bin/env bash\nexit 0\n' >"$repo/src/ok.sh"
g add src/ok.sh
g commit -q --no-verify --amend -m init >/dev/null
rm "$repo/scripts/ci/foo.sh"
ln -s ../../src/ok.sh "$repo/scripts/ci/foo.sh"
g add scripts/ci/foo.sh
run refuse-type-change refuse "scripts/ci/foo.sh" "checker"

# ---- Pinning (review 1 #3): refused before #155 too, must stay refused -----------------------
repo pin-recipe-changed - -
sed -i 's|^\tscripts/ci/foo.test.sh$|\ttrue|' "$repo/Makefile"
g add Makefile
run pin-recipe-changed refuse "Makefile" "checker"
repo pin-check-prereq-removed - -
sed -i 's|^check: check-foo$|check:|' "$repo/Makefile"
g add Makefile
run pin-check-prereq-removed refuse "Makefile" "checker"
repo pin-guard-deleted - -
g rm -q scripts/ci/foo.sh
run pin-guard-deleted refuse "scripts/ci/foo.sh" "checker"
repo pin-guard-renamed - -
g mv scripts/ci/foo.sh scripts/ci/foo2.sh
run pin-guard-renamed refuse "scripts/ci/foo.sh" "checker"
# Moved out of scripts/ci/: with rename detection only the new path src/foo.sh would be listed.
repo pin-guard-moved-out - -
g mv scripts/ci/foo.sh src/foo.sh
run pin-guard-moved-out refuse "scripts/ci/foo.sh" "checker"

# ---- Allowed under the token: the same shapes in a checker task with decides_on --------------
repo allow-licenses-checker "$ok_checker" "$ok_decides"
printf 'export const extra = ["BSD-2-Clause"];\n' >>"$repo/scripts/licenses/accepted.mjs"
g add scripts/licenses/accepted.mjs
run allow-licenses-checker allow
repo allow-mk-checker "$ok_checker" "$ok_decides"
printf 'lint:\n\ttrue\n' >"$repo/rules.mk"
printf '\nlint:\n\ttrue\n' >>"$repo/Makefile"
g add rules.mk Makefile
run allow-mk-checker allow

echo "guard-trigger.test: $passed passed, $failed failed"
[ "$failed" -eq 0 ] || exit 1
