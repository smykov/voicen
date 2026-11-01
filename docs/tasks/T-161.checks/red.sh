#!/bin/sh
# T-161 red check: text checks on the role bodies plus a dry run of the named tripwires.
# Docs-only task, no code test, no mutants catalog.
# Usage: sh docs/tasks/T-161.checks/red.sh [ROOT]
#   ROOT defaults to the repo root; it needs ROOT/.claude/agents/{developer,test-writer}-{core,ui}.md,
#   ROOT/scripts/ci/role-heads/ and ROOT/docs/tasks/T-114.md. The dry run (A4) always uses the real
#   repository's git and scripts, and extracts the second-spawn copy script from ROOT's
#   test-writer-core.md. Every A4 step happens in a throwaway worktree under /tmp, removed by a trap.
# Exit 0 only when every check passes. Phrases are case-insensitive key phrases, not full sentences.
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
root=${1:-$repo}
fail=0
ok()   { echo "PASS $1"; }
bad()  { echo "FAIL $1"; fail=1; }
note() { echo "NOTE $1"; }
# has <name> <file> <ere>: the file matches the pattern (case-insensitive)
has() { if grep -qiE -- "$3" "$2"; then ok "$1"; else bad "$1 (missing: $3)"; fi; }
# hasin <name> <text> <ere>: the text matches the pattern (case-insensitive)
hasin() { if printf '%s\n' "$2" | grep -qiE -- "$3"; then ok "$1"; else bad "$1 (missing: $3)"; fi; }
# section <file> <start-ere>: print from the heading matching start to the next "## " heading
section() { awk -v s="$2" 'BEGIN{IGNORECASE=1} $0 ~ s {p=1; print; next} p && /^## /{exit} p' "$1"; }
# item <text> <n>: step n of a numbered list (from "n." to the next top-level "n+1." line)
item() { printf '%s\n' "$1" | awk -v n="$2" -v m="$(($2 + 1))" '$0 ~ "^" n "\\." {p=1; print; next} $0 ~ "^" m "\\." {p=0} p'; }

ag=$root/.claude/agents

# ---------------------------------------------------------------- characterization (green now)
for r in developer-core developer-ui test-writer-core test-writer-ui; do
  f=$ag/$r.md
  [ -f "$f" ] || { bad "[$r] role file exists"; continue; }
  if head -8 "$f" | cmp -s - "$root/scripts/ci/role-heads/$r.md"; then
    ok "[$r] char: front matter (lines 1-8) equals role-heads"; else bad "[$r] char: front matter (lines 1-8) equals role-heads"; fi
  if [ "$(sed -n '/teamwright:tools:begin/,/teamwright:tools:end/p' "$f")" = "$(cat "$here/fixtures/$r.tools.txt")" ]; then
    ok "[$r] char: Tools markers block byte-for-byte"; else bad "[$r] char: Tools markers block byte-for-byte"; fi
done
if (cd "$repo" && scripts/ci/role-tools.sh "$ag" "$root/scripts/ci/role-heads" >/dev/null 2>&1); then
  ok "char: scripts/ci/role-tools.sh exit 0"; else bad "char: scripts/ci/role-tools.sh exit 0"; fi

# ---------------------------------------------------------------- A1: T-114 tick
t114=$root/docs/tasks/T-114.md
acc=$(section "$t114" '^## Acceptance')
tick=$(printf '%s\n' "$acc" | grep -iE '^- \[.\].*(language server|fails to start)')
if [ -n "$tick" ]; then ok "A1 the failure-branch tick is found in T-114 Acceptance"; else bad "A1 the failure-branch tick ('fails to start') is found in T-114 Acceptance"; fi
if printf '%s\n' "$tick" | grep -qiE 'already pinned by'; then bad "A1 tick no longer claims R4 pins the server-fails-to-start case (still 'already pinned by')"; else ok "A1 tick no longer claims R4 pins the server-fails-to-start case"; fi
hasin "A1 tick says it is a recorded gap" "$tick" 'recorded gap'
hasin "A1 tick cites #200 or context-tools Not caught" "$tick" '#200|not caught'

# ---------------------------------------------------------------- A2: test-writer pre-check
for a in core ui; do
  f=$ag/test-writer-$a.md
  [ -f "$f" ] || { bad "[tw-$a] role file exists"; continue; }
  steps=$(section "$f" '^## Steps')
  ret=$(section "$f" '^## Return to caller')
  s5=$(item "$steps" 5)
  hasin "[tw-$a] A2 step 5 keeps the kit 'Fast checks' lint text" "$s5" 'fast checks on what you wrote, before handing over'
  if [ "$a" = core ]; then
    hasin "[tw-core] A2 step 5 keeps the clippy lint command" "$s5" 'cargo clippy -p voicen-core --all-targets -- -D warnings'
    hasin "[tw-core] A2 step 5 names git add -N" "$s5" 'git add -N'
    hasin "[tw-core] A2 step 5 names scripts/ci/clippy-lines.sh" "$s5" 'scripts/ci/clippy-lines\.sh'
    hasin "[tw-core] A2 step 5 names scripts/ci/core-test-os-answer.sh" "$s5" 'scripts/ci/core-test-os-answer\.sh'
    hasin "[tw-core] A2 step 5 says a compile error on a missing item is the red" "$s5" 'compile error'
    hasin "[tw-core] A2 step 5 ...is the red" "$s5" '(is|are) the red'
    hasin "[tw-core] A2 step 5 ...reported as such" "$s5" 'report(ed)?[^.]*as such|as such'
  else
    hasin "[tw-ui] A2 step 5 keeps pnpm lint" "$s5" 'pnpm lint'
  fi
  if printf '%s\n' "$ret" | grep -qE '^Pre-check:'; then ok "[tw-$a] A2 Return carries a 'Pre-check:' line"; else bad "[tw-$a] A2 Return carries a 'Pre-check:' line"; fi
done

# ---------------------------------------------------------------- A3: developer roles
for a in core ui; do
  f=$ag/developer-$a.md
  [ -f "$f" ] || { bad "[dev-$a] role file exists"; continue; }
  steps=$(section "$f" '^## Steps')
  has "[dev-$a] A3 'Run tests' row names scripts/tw-test $a" "$f" "^\\| Run tests \\|.*scripts/tw-test $a"
  has "[dev-$a] A3 'Area gate' row names scripts/tw-gate $a" "$f" "^\\| Area gate \\|.*scripts/tw-gate $a"
  has "[dev-$a] A3 run paragraph: the gate only through scripts/tw-gate" "$f" 'the gate only through .?scripts/tw-gate'
  s1=$(item "$steps" 1)
  hasin "[dev-$a] A3 step 1 runs targeted tests through scripts/tw-test" "$s1" "scripts/tw-test $a"
  hasin "[dev-$a] A3 step 1 limits to the task's tests" "$s1" "only the task's tests"
  hasin "[dev-$a] A3 step 1 ...not the full gate" "$s1" 'not the full gate'
  s5=$(item "$steps" 5)
  hasin "[dev-$a] A3 step 5 runs the full gate through scripts/tw-gate $a" "$s5" "scripts/tw-gate $a"
  hasin "[dev-$a] A3 step 5 runs it once" "$s5" 'gate[^.]*\bonce\b|\bonce\b[^.]*gate'
  hasin "[dev-$a] A3 step 5 reuses the recorded PASS instead of re-running" "$s5" 'instead of running the gate again'
  hasin "[dev-$a] A3 step 5 a red gate is re-run only after a fix" "$s5" 're-?run[^.]*only after (a |the )?(fix|change)'
  if printf '%s\n' "$steps" | grep -qE 'make (-k )?check\b'; then bad "[dev-$a] A3 bare 'make check' absent from Steps"; else ok "[dev-$a] A3 bare 'make check' absent from Steps"; fi
done

# ---------------------------------------------------------------- A4: dry run (throwaway worktree)
tmpd=$(mktemp -d /tmp/t161red.XXXXXX) || { bad "A4 mktemp"; echo "RED: some checks failed"; exit 1; }
tree=$tmpd/tree
cleanup() { git -C "$repo" worktree remove --force "$tree" >/dev/null 2>&1; rm -rf "$tmpd"; }
trap cleanup EXIT
before=$(git -C "$repo" status --porcelain; git -C "$repo" diff --cached | cksum; git -C "$repo" rev-parse HEAD)
if ! git -C "$repo" worktree add --detach "$tree" HEAD >/dev/null 2>&1; then
  bad "A4 throwaway worktree"
else
  probe=crates/voicen-core/src/t161_probe.rs
  # the T-116 line, built so this script does not hold the token on one scanned line by itself
  printf '%s\n%s\n' '// T-161 probe' '#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]' > "$tree/$probe"
  out=$(cd "$tree" && scripts/ci/clippy-lines.sh 2>&1); e=$?
  if [ "$e" -eq 0 ]; then
    ok "A4 char: clippy-lines on an untracked probe with the T-116 allow line exits 0 (the blind spot)"
  elif [ "$e" -eq 1 ]; then
    note "T-164 seems to have landed: clippy-lines now refuses the untracked probe (exit 1); the 'git add -N' clause is dead text, update this check"
    ok "A4 clippy-lines on an untracked probe exits 1 (T-164 landed)"
  else
    bad "A4 clippy-lines on an untracked probe: exit $e ($out)"
  fi
  (cd "$tree" && git add -N "$probe")
  out=$(cd "$tree" && scripts/ci/clippy-lines.sh 2>&1); e=$?
  if [ "$e" -eq 1 ] && printf '%s\n' "$out" | grep -q 't161_probe\.rs'; then
    ok "A4 clippy-lines after git add -N exits 1 and names the probe"
  else
    bad "A4 clippy-lines after git add -N exits 1 and names the probe (exit $e)"
  fi
  tp=crates/voicen-core/tests/t161_probe_os.rs
  printf '%s\n' '// T-161 probe' 'const ADDR: &str = "127.0.0.1:1";' > "$tree/$tp"
  out=$(cd "$tree" && scripts/ci/core-test-os-answer.sh 2>&1); e=$?
  if [ "$e" -eq 1 ] && printf '%s\n' "$out" | grep -q 't161_probe_os\.rs'; then
    ok "A4 core-test-os-answer on an untracked test file with 127.0.0.1:1 exits 1 (no git add -N needed)"
  else
    bad "A4 core-test-os-answer on an untracked test file with 127.0.0.1:1 exits 1 (exit $e)"
  fi

  # the second-spawn copy script, extracted from ROOT's test-writer-core.md, on a tree whose new file
  # is intent-to-added: does the file's content reach the copy?
  np=crates/voicen-core/src/t161_new.rs
  printf '%s\n' '// T-161 new file' 'pub fn t161_new() -> u32 { 161 }' > "$tree/$np"
  (cd "$tree" && git add -N "$np")
  twc=$ag/test-writer-core.md
  awk '/^The copy carries/ {a=1} a && /^```sh/ {p=1; next} p && /^```/ {exit} p' "$twc" > "$tmpd/spawn.sh"
  if [ -s "$tmpd/spawn.sh" ] && grep -q 'git diff HEAD --binary' "$tmpd/spawn.sh" && grep -q 'git ls-files -o' "$tmpd/spawn.sh"; then
    ok "A4 second-spawn copy script found in test-writer-core.md"
    # the last command (the catalog run) is replaced by the observation; the trap still removes the copy
    sed 's|^scripts/tw-mutants .*|if cmp -s "$PROBE" "$copy/tree/$PROBE"; then echo SPAWN_COPY_OK; else echo SPAWN_COPY_MISSING; fi|' "$tmpd/spawn.sh" > "$tmpd/spawn2.sh"
    out=$(cd "$tree" && TMPDIR=$tmpd PROBE=$np sh "$tmpd/spawn2.sh" 2>&1)
    if printf '%s\n' "$out" | grep -q SPAWN_COPY_OK; then
      ok "A4 second-spawn copy carries the intent-to-added new file with its content"
    else
      bad "A4 second-spawn copy carries the intent-to-added new file (fallback: run the tripwire with a temporary GIT_INDEX_FILE) [$out]"
    fi
  else
    bad "A4 second-spawn copy script found in test-writer-core.md"
  fi
  (cd "$tree" && git worktree list >/dev/null 2>&1)
fi
after=$(git -C "$repo" status --porcelain; git -C "$repo" diff --cached | cksum; git -C "$repo" rev-parse HEAD)
if [ "$before" = "$after" ]; then ok "A4 the real tree, its index and HEAD are untouched by the dry run"; else bad "A4 the real tree, its index and HEAD are untouched by the dry run"; fi

[ "$fail" -eq 0 ] && echo "ALL PASS" || echo "RED: some checks failed"
exit $fail
