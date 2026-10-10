#!/usr/bin/env bash
# T-026 (decisions #102, #103): guard of the guard for scripts/ci/release-guard.sh. A GitHub
# Release may be created only by a push of a v* tag, after the gate and windows jobs passed and
# scripts/ci/release-check.sh accepted the tag in the same job; only that job may write contents.
# No wip/** run can show the publish step running, so the workflow's shape is pinned here
# (lesson of T-070 mutation M6). Contract of the checker, pinned here:
#   Usage: scripts/ci/release-guard.sh <workflow-file>
#   A "publish step" is a step whose run: calls `gh release` create, upload, edit, delete or
#   delete-asset (`gh release view` / `list` are reads, not judged). For every publish step:
#     1. its job has a job-level `if:` (blanks removed, `${{ }}` optional) with both top-level
#        `&&` operands github.event_name=='push' and startsWith(github.ref,'refs/tags/v'), no
#        top-level `||`, and no status function (always(), failure(), cancelled(), !cancelled(),
#        success()||);
#     2. its job `needs:` both gate and windows (flow or block list);
#     3. its job has no `continue-on-error:`;
#     4. an earlier step of the same job runs scripts/ci/release-check.sh, with no `if:` and no
#        `continue-on-error:`, and GH_TOKEN in its env: or the job's env: (so the release-exists
#        check runs); and its run: cannot swallow the script's exit status: no `||`, `;`, `if`,
#        `set +e` or pipe around the call, and the call is the step's last command (validation 1
#        M6: `... || true` let a refused tag reach `gh release create`). A `run: |` block holding
#        only comments and the call is fine; a `#` line after the call is not a comment when a
#        quote is open (`"dist` then `#" || true`), and a double-quoted run: scalar's `\n` is a
#        second shell line (review 2 finding 2): neither may pass;
#     5. its own `if:`, if any, has no status function;
#   and:
#     6. `contents: write` appears only in the permissions: of a job with a publish step (not at
#        workflow level, not in another job); `write-all` appears nowhere;
#     7. every `uses:` of a job with a publish step names an actions/* action (no third-party
#        release action).
#   Exit 0: ok, one line with "ok:" and "<n> step(s) publish a release" (0 for a workflow
#           without one).
#   Exit 1: a violation; each listed as "<workflow-file>:<line>: <why>", <line> = the job key for
#           1-3; for 4 the release-check step's "- " item (its if:, continue-on-error, no
#           GH_TOKEN) or, with no release-check step before it, the publish step's; the step's
#           "- " item for 5 and 7; the offending permissions line for 6.
#   Exit 2: usage error (no argument, or more than one).
#   Exit 3: cannot run (the file is missing or unreadable); never a pass.
# Cases: every dir scripts/ci/fixtures/release-guard/<case>/ci.yml (ok-* exit 0, v-* exit 1),
# the real .github/workflows/ci.yml (exit 0, 1 publish step) and the real ci.yml with its
# release job's if: dropped, always() added, needs without windows, contents: write at
# workflow level, and its release-check run: ending in `|| true` or `; true` (exit 1 each), usage
# and a missing file. Every fixture dir must appear in the
# table, so a case cannot be dropped silently.
# Usage: scripts/ci/release-guard.test.sh   (host bash, sed, grep, awk)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
guard=scripts/ci/release-guard.sh
fx=scripts/ci/fixtures/release-guard
real=.github/workflows/ci.yml
[ -d "$fx" ] || { echo "release-guard.test: cannot run: $fx not found" >&2; exit 3; }
[ -f "$real" ] || { echo "release-guard.test: cannot run: $real not found" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "release-guard.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

failed=0
passed=0
declare -A seen=()

# run <label> <expected exit> <needle> [guard args...]; the needle must be in the output
# (case-insensitive).
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
  else
    echo "ok   $label: exit $got"
    passed=$((passed + 1))
  fi
}

# line_of <file> <fixed text>: the number of the first line containing the text (empty if none).
line_of() { grep -n -m 1 -F -- "$2" "$1" | cut -d: -f1; }

# check <case> <expected exit> <needle | @text>: the case's ci.yml as the workflow. "@text"
# means "<file>:<line of the first line containing text>:".
check() {
  local f="$fx/$1/ci.yml" n="$3" l
  seen["$1"]=1
  if [ "${n#@}" != "$n" ]; then
    l="$(line_of "$f" "${n#@}")"
    if [ -z "$l" ]; then
      echo "FAIL $1: the fixture has no line containing: ${n#@} (fix the test)" >&2
      failed=$((failed + 1))
      return
    fi
    n="$f:$l:"
  fi
  run "$1" "$2" "$n" "$f"
}

ok_n() { echo "$1 step(s) publish a release"; }

# Allowed.
check ok-release                       0 "$(ok_n 1)"   # the shipped shape; a windows probe with gh release view/list, !cancelled() and continue-on-error is not judged
check ok-no-release                    0 "$(ok_n 0)"   # no publish step (ci.yml before T-026)
check ok-reordered-job-token-block-run 0 "$(ok_n 1)"   # no ${{ }}, operands reordered plus one more, needs as a block list, GH_TOKEN in the job env, gh release create in a run: | block
check ok-check-block-run              0 "$(ok_n 1)"   # the release-check call alone in a run: | block after a comment

# Violations: exit 1, the listing names file:line.
check v-no-job-if               1 "@  release:"                  # the publish job runs on every ref, wip/** included
check v-no-tag-operand          1 "@  release:"                  # push on any ref
check v-no-event-name           1 "@  release:"
check v-top-level-or            1 "@  release:"                  # both operands present as text, not top-level &&
check v-tag-operand-widened     1 "@  release:"                  # refs/tags/ instead of refs/tags/v
check v-job-always              1 "@  release:"                  # publishes after a red gate or windows job
check v-job-not-cancelled       1 "@  release:"
check v-job-continue-on-error   1 "@  release:"
check v-needs-gate-only         1 "@  release:"                  # publishes without the Windows build and smoke
check v-needs-windows-only      1 "@  release:"
check v-no-needs                1 "@  release:"
check v-publish-before-check    1 "@- name: Publish the release" # the check runs after the release exists
check v-no-check                1 "@- name: Publish the release" # only the windows premise runs release-check.sh
check v-check-if                1 "@- name: Release check"       # a skipped step counts as success
check v-check-continue-on-error 1 "@- name: Release check"       # a refusal would not stop the publish
check v-check-no-token          1 "@- name: Release check"       # the release-exists check would be skipped
# The check's exit status swallowed in its run: (validation 1 M6): a refused tag is published.
check v-check-or-true             1 "@- name: Release check"     # ... || true
check v-check-semicolon-true      1 "@- name: Release check"     # ...; true
check v-check-or-colon            1 "@- name: Release check"     # ... || :
check v-check-or-exit-0           1 "@- name: Release check"     # ... || exit 0
check v-check-if-wrapped          1 "@- name: Release check"     # if ...; then ...; fi (errexit ignores an if condition)
check v-check-set-plus-e          1 "@- name: Release check"     # run: | set +e, the call, then echo
check v-check-block-or-true       1 "@- name: Release check"     # run: | the call with || continued on the next line
check v-check-trailing-no-errexit 1 "@- name: Release check"     # shell without -e, the call followed by echo
check v-check-pipe-default-shell  1 "@- name: Release check"     # ... | tee, default shell bash -e {0} has no pipefail
# Review 2 finding 1: each needs one condition of the sole-call rule on its own.
check v-check-bang                1 "@- name: Release check"     # run: | ! bash ... (one line, no ;|&; ! inverts the exit) - the bash anchor
check v-check-echo-only           1 "@- name: Release check"     # run: echo scripts/ci/release-check.sh ... (never runs it) - the bash anchor
check v-check-trap-exit-0         1 "@- name: Release check"     # run: | trap 'exit 0' EXIT, then the call alone - one line only
# Review 2 finding 2: the run's own lines, not a comment-dropped model of them.
check v-check-quoted-hash         1 "@- name: Release check"     # run: | the call ending in an open "dist, then a #" || true line
check v-check-dq-scalar-newline   1 "@- name: Release check"     # run: "... dist\nexit 0" (YAML \n = 2 shell lines), shell without -e
check v-publish-always          1 "@- name: Publish the release" # publishes after a refused check
check v-workflow-write          1 "@contents: write"             # every job, wip/** runs included, gets contents: write
check v-write-other-job         1 "@contents: write"             # the windows job gets contents: write
check v-write-all               1 "@write-all"
check v-third-party-action      1 "@softprops/action-gh-release" # a release action instead of gh
check v-unguarded-upload        1 "@  windows:"                  # gh release upload from the windows job (no tag if:, needs gate only)

# The real workflow: ok, and exactly its one publish step is read.
run "real $real" 0 "$(ok_n 1)" "$real"

# The real workflow mutated. The release job is the job key "  release:" (docs/tasks/T-026.md).
job_line="$(grep -n -m 1 -x '  release:' "$real" | cut -d: -f1)"
if [ -z "$job_line" ]; then
  echo "FAIL real mutations: no job 'release:' in $real (the release job does not exist)" >&2
  failed=$((failed + 1))
else
  # awk program prefix: inrel is 1 on the lines of the release job.
  scope='/^  [A-Za-z0-9_-]+:[ \t]*$/ { inrel = ($0 ~ /^  release:[ \t]*$/) } /^[A-Za-z]/ { inrel = 0 }'
  mutate() { # mutate <name> <awk body>
    awk "$scope $2" "$real" > "$tmp/$1.yml"
    if cmp -s "$real" "$tmp/$1.yml"; then
      echo "FAIL real $1: the mutation did not apply (update the test with the workflow)" >&2
      failed=$((failed + 1))
      return 1
    fi
  }
  mutate no-if 'inrel && /^    if:/ { next } { print }' &&
    run "real without the release job's if:" 1 "$tmp/no-if.yml:$job_line:" "$tmp/no-if.yml"
  mutate always 'inrel && /^    if:/ { if (!sub(/^    if: \$\{\{ /, "    if: ${{ always() \\&\\& ")) sub(/^    if: /, "    if: always() \\&\\& ") } { print }' &&
    run "real with always() in the release job's if:" 1 "$tmp/always.yml:$job_line:" "$tmp/always.yml"
  mutate needs-gate 'inrel && /^    needs:/ { print "    needs: gate"; skip = 1; next } skip && /^      - / { next } { skip = 0; print }' &&
    run "real with the release job needing gate only" 1 "$tmp/needs-gate.yml:$job_line:" "$tmp/needs-gate.yml"
  awk '/^jobs:/ && !done { print "permissions:"; print "  contents: write"; done = 1 } { print }' "$real" > "$tmp/wf-write.yml"
  wl="$(grep -n -m 1 -x '  contents: write' "$tmp/wf-write.yml" | cut -d: -f1)"
  run "real with contents: write at workflow level" 1 "$tmp/wf-write.yml:$wl:" "$tmp/wf-write.yml"
  # The release job's release-check run: with its exit swallowed (validation 1 M6); the listing
  # names the check step's "- " item.
  check_line="$(awk "$scope"' inrel && /^      - name: Release check/ { print FNR; exit }' "$real")"
  mutate or-true 'inrel && /^        run: bash scripts\/ci\/release-check\.sh / { $0 = $0 " || true" } { print }' &&
    run "real with || true after the release job's release-check" 1 "$tmp/or-true.yml:$check_line:" "$tmp/or-true.yml"
  mutate semicolon-true 'inrel && /^        run: bash scripts\/ci\/release-check\.sh / { $0 = $0 "; true" } { print }' &&
    run "real with ; true after the release job's release-check" 1 "$tmp/semicolon-true.yml:$check_line:" "$tmp/semicolon-true.yml"
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
  echo "release-guard.test: FAIL: $failed case(s) differ, $passed as expected (T-026)" >&2
  exit 1
fi
echo "release-guard.test: ok: $passed cases as expected"
