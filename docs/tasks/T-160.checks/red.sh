#!/bin/sh
# T-160 red check: text checks on the test-writer role bodies. Docs-only task, no code test.
# Usage: sh docs/tasks/T-160.checks/red.sh [ROOT]   (ROOT defaults to the repo root; needs
# ROOT/.claude/agents/test-writer-{core,ui}.md, ROOT/docs/failures.md, ROOT/scripts/ci/role-heads/)
# Exit 0 only when every check passes. Phrases are case-insensitive key phrases, not full sentences.
here=$(cd "$(dirname "$0")" && pwd)
root=${1:-$(cd "$here/../../.." && pwd)}
fail=0
ok()  { echo "PASS $1"; }
bad() { echo "FAIL $1"; fail=1; }
# has <name> <file> <ere>: the file matches the pattern (case-insensitive)
has() { if grep -qiE -- "$3" "$2"; then ok "$1"; else bad "$1 (missing: $3)"; fi; }
# section <file> <start-ere> : print from the heading matching start to the next "## " heading
section() { awk -v s="$2" 'BEGIN{IGNORECASE=1} $0 ~ s {p=1; print; next} p && /^## /{exit} p' "$1"; }

for a in core ui; do
  f=$root/.claude/agents/test-writer-$a.md
  [ -f "$f" ] || { bad "[$a] role file exists"; continue; }
  steps=$(section "$f" '^## Steps')
  ret=$(section "$f" '^## Return to caller')

  # --- characterization (green now, must stay green) ---
  if head -8 "$f" | cmp -s - "$root/scripts/ci/role-heads/test-writer-$a.md"; then
    ok "[$a] char: front matter lines 1-8 equal role-heads"; else bad "[$a] char: front matter lines 1-8 equal role-heads"; fi
  if [ "$(sed -n '/teamwright:tools:begin/,/teamwright:tools:end/p' "$f")" = "$(cat "$here/fixtures/$a.tools.txt")" ]; then
    ok "[$a] char: Tools markers block byte-for-byte"; else bad "[$a] char: Tools markers block byte-for-byte"; fi
  if grep -qxF -- "$(cat "$here/fixtures/$a.deferred.txt")" "$f"; then
    ok "[$a] char: #194 Loading the tools (deferred) paragraph byte-for-byte"; else bad "[$a] char: #194 paragraph byte-for-byte"; fi

  # --- Acceptance 1: swallow at the call that first fails, one site per mutant, one-shot fakes ---
  has "[$a] A1 swallow at the call that first fails" "$f" 'swallow[^.]*(call|site)[^.]*first fail|first fail[^.]*swallow'
  has "[$a] A1 one site per mutant, other sites still propagate" "$f" 'one (propagation )?site per (mutant|wrong)'
  has "[$a] A1 other sites still propagate" "$f" 'other sites?[^.]*propagat'
  has "[$a] A1 error fakes fail exactly one call" "$f" 'fail(s)? exactly one call'
  has "[$a] A1 ...unless the guarantee is about repeated failures" "$f" 'unless the guarantee[^.]*repeated fail'
  if printf '%s\n' "$steps" | grep -qiE 'first fail'; then ok "[$a] A1 rule sits in Steps (bite check step)"; else bad "[$a] A1 rule sits in Steps (bite check step)"; fi
  sec2=$(section "$f" '^## Failure-branch bite')
  if [ -n "$sec2" ]; then ok "[$a] A1 second-spawn section exists"; else bad "[$a] A1 second-spawn section '## Failure-branch bite' exists"; fi
  if printf '%s\n' "$sec2" | grep -qiE 'first fail' && printf '%s\n' "$sec2" | grep -qiE 'exactly one call'; then
    ok "[$a] A1 second spawn carries per-site + one-shot rule"; else bad "[$a] A1 second spawn carries per-site + one-shot rule"; fi

  # --- Acceptance 2: bound and own target ---
  has "[$a] A2 bound: 2-3 plus one per failure branch" "$f" '2.3 [a-z ]*(wrong implementations|mutants)?[^.]*plus one per failure branch'
  has "[$a] A2 checker: true sweep kept" "$f" 'checker: true.*(sweep|every condition)|sweep.*every condition'
  has "[$a] A2 builds via scripts/tw-test" "$f" 'scripts/tw-test .*--tree|tw-test <?[a-z|]*>? --tree'
  has "[$a] A2 builds via scripts/tw-mutants --tree" "$f" 'scripts/tw-mutants .*--tree'
  has "[$a] A2 never the working tree's target" "$f" "never[^.]*(working tree|main tree)'?s?[^.]*target"
  if grep -qE 'tw-run (core|ui) -- (cargo|pnpm) test' "$f"; then bad "[$a] A2 no 'tw-run .. -- cargo|pnpm test' lines remain"; else ok "[$a] A2 no 'tw-run .. -- cargo|pnpm test' lines remain"; fi

  # --- Acceptance 4: expected, not run marker in step 3, bite-result paragraph, Return line ---
  s3=$(printf '%s\n' "$steps" | grep -E '^3\.')
  if printf '%s\n' "$s3" | grep -qiE 'expected, not run'; then ok "[$a] A4 step 3 has 'expected, not run'"; else bad "[$a] A4 step 3 has 'expected, not run'"; fi
  n=$(printf '%s\n' "$steps" | grep -ciE 'expected, not run')
  if [ "$n" -ge 2 ]; then ok "[$a] A4 step 3 + bite-result paragraph both in Steps ($n hits)"; else bad "[$a] A4 step 3 + bite-result paragraph both in Steps ($n hits, need >=2)"; fi
  if printf '%s\n' "$ret" | grep -iE '^Bite check:' | grep -qiE 'expected, not run'; then ok "[$a] A4 Return 'Bite check:' line has the not-run form"; else bad "[$a] A4 Return 'Bite check:' line has the not-run form"; fi
  has "[$a] A4 never as an observed result" "$f" 'never (as|written as)[^.]*observed'
  has "[$a] A4 ask orchestrator to push wip/<ID>" "$f" 'ask the orchestrator[^.]*wip/<ID>'
  has "[$a] A4 cite that run @ sha" "$f" 'run[^.]*@ ?<?sha'
  has "[$a] A4 You never push yourself" "$f" 'you never push yourself'
done
if sed -n '9,$p' "$root/.claude/agents/test-writer-core.md" | grep -qiE 'src-tauri/tests'; then ok "[core] A4 body names src-tauri/tests"; else bad "[core] A4 body (not front matter) names src-tauri/tests"; fi
if sed -n '9,$p' "$root/.claude/agents/test-writer-core.md" | grep -qiE 'cfg\(windows\)'; then ok "[core] A4 body names cfg(windows)"; else bad "[core] A4 body (not front matter) names cfg(windows)"; fi

# --- D1: docs/failures.md class + F-entry ---
fl=$root/docs/failures.md
has "failures.md class error-fake-masks-origin" "$fl" '^\| error-fake-masks-origin '
if grep -E '^\| F-[0-9]+ ' "$fl" | grep -E 'T-065' | grep -qE 'T-116'; then ok "failures.md F-entry names T-065 and T-116"; else bad "failures.md F-entry names T-065 and T-116"; fi
has "failures.md F-entry cites r1#5" "$fl" 'T-049[^|]*r1 ?#5|r1 ?#5[^|]*T-049'

[ "$fail" -eq 0 ] && echo "ALL PASS" || echo "RED: some checks failed"
exit $fail
