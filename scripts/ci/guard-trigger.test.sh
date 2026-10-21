#!/usr/bin/env bash
# T-113 (rca, class guard-model, decision #154, exit c): host test of the P-020 mechanical trigger
# in the git pre-commit hook scripts/hooks/pre-commit. P-020's design-time step (`checker: true`
# plus `analysis.decides_on`) fired only when the analysis author remembered it: T-082 predates the
# template's `checker:` field, so its guard (role-tools.sh) was designed as a model of Claude
# Code's front matter and caught only in review (T-113 Investigation, hypothesis 5).
#
# Contract (pre-commit, run in the repository being committed to):
#   Guard change: a staged path (added or modified) that is
#     - a file under scripts/ci/ run by a check* target of the Makefile: named in a check* recipe,
#       or run by such a file (e.g. scripts/ci/foo.sh run by scripts/ci/foo.test.sh), or
#     - the Makefile, when the staged version adds a check-* target that HEAD lacks.
#   Active task: $TEAMWRIGHT_TASK, else the first word of .teamwright/current-task (as the
#     analysis gate reads it); its record docs/tasks/<ID>.md.
#   A commit with a guard change is refused (exit != 0, a message naming the staged guard path and
#     the missing field, `checker` or `decides_on`) unless the active task's front matter has
#     `checker: true` and its `analysis:` block a non-empty, non-placeholder `decides_on`.
#     A missing `checker:` key counts as unset (false); no active task, or no record for it,
#     refuses too. Text in the record's body does not count.
#   A commit without a guard change is not affected (its result is the hook's other checks').
# Characterization (green before T-113, must stay green): the hook's existing refusal of a staged
#   .env file, and an ordinary commit passing.
# Not pinned: whether a deleted guard file or a changed recipe of an existing check target counts
#   as a guard change; what exit code other than 0 a refusal uses.
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
  printf 'fn main() {}\n' >"$repo/src/app.rs"
  printf '# Notes\n' >"$repo/docs/notes.md"
  task "$repo/docs/tasks/T-900.md" "$2" "$3" "${4:-}" "${5:-}"
  printf 'T-900\n' >"$repo/.teamwright/current-task"
  g add -A >/dev/null
  g commit -q --no-verify -m init >/dev/null
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
repo none-makefile-other-target - -
printf '\nlint:\n\ttrue\n' >>"$repo/Makefile"
g add Makefile
run none-makefile-other-target allow
repo none-no-active-task - -
rm "$repo/.teamwright/current-task"
printf 'fn main() { let _ = 4; }\n' >"$repo/src/app.rs"
g add src/app.rs
run none-no-active-task allow

echo "guard-trigger.test: $passed passed, $failed failed"
[ "$failed" -eq 0 ] || exit 1
