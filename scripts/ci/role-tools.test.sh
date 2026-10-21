#!/usr/bin/env bash
# T-082 (decisions #145): host test of scripts/ci/role-tools.sh, the tripwire that keeps the context
# tool (Serena) and the docs tool (context7) reachable from the subagent roles whose rendered Tools
# block tells them to call it. Claude Code gives a subagent with a `tools:` front-matter line only
# the tools named there, so a Tools block without the names in `tools:` sends every role down its
# "not connected" fallback, silently (T-082 Investigation).
#
# Contract of role-tools.sh [agents-dir [manifest]]
#   (defaults: .claude/agents, scripts/ci/role-tools.txt)
#   Manifest: one exact tool name per line; lines starting with # and blank lines are not entries.
#   Checked: every *.md directly in agents-dir that contains the line
#     <!-- teamwright:tools:begin -->. A role file without it is ignored.
#   For each checked file:
#     R1 its front matter (between the first two --- lines) has a `tools:` line; its value is a
#        comma-separated list of tool names (blanks around names ignored). A `tools:` line in the
#        body does not count. No `tools:` line (= every tool, Serena's edit/shell tools included) is
#        a violation.
#     R2 every manifest entry is one of those names (exact match, not a substring).
#     R3 every name in that list starting with mcp__ is a manifest entry (no Serena edit, memory or
#        shell tool, no other MCP tool, no wildcard or bare server name).
#     R4 inside the Tools block (between the begin and the <!-- teamwright:tools:end --> line) one
#        line holds both "Serena not connected" and "use grep and file reads", and one line holds
#        "Not connected", "mcp__context7__" and "same fallback" (the fallback, not a stop:
#        Acceptance 3).
#   Exit 0: "role-tools: ok: <n> role file(s) with a Tools block" (n pins that files were read).
#   Exit 1: one line per violation, starting with the file's path and naming the missing or
#     refused tool name (R2, R3), "tools:" (R1) or "not connected" (R4).
#   Exit 3: "role-tools: cannot run: ..." - agents dir missing, manifest missing or without
#     entries. Never reported as a pass.
# Not caught: whether Claude Code actually offers the named tools to the subagent (shown by the
# T-082 Acceptance run: a journal line agent_type=<role>, tool=mcp__serena__*); a code-reading
# role that has no Tools block at all (technical-writer: T-082 records why).
#
# Usage: scripts/ci/role-tools.test.sh
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
guard=scripts/ci/role-tools.sh
manifest=scripts/ci/role-tools.txt
tmp="$(mktemp -d)" || { echo "role-tools.test: cannot run: mktemp failed" >&2; exit 3; }
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

# The names the manifest must hold, exactly (T-082 analysis, checked against serena-agent
# 2.0.0.dev0's claude-code context and the deferred-tool list of a main session).
want_names='mcp__context7__query-docs
mcp__context7__resolve-library-id
mcp__serena__find_declaration
mcp__serena__find_implementations
mcp__serena__find_referencing_symbols
mcp__serena__find_symbol
mcp__serena__get_symbols_overview
ToolSearch'
if [ ! -f "$manifest" ]; then
  fail "manifest: $manifest missing"
else
  got_names="$(grep -vE '^[[:space:]]*(#|$)' "$manifest" | LC_ALL=C sort)"
  if [ "$got_names" = "$(LC_ALL=C sort <<<"$want_names")" ]; then
    pass "manifest: exactly the pinned names"
  else
    fail "manifest: entries differ from the pinned names" "$(diff <(LC_ALL=C sort <<<"$want_names") <(printf '%s\n' "$got_names"))"
  fi
fi
all_names="$(paste -sd, <<<"$want_names" | sed 's/,/, /g')"

serena_line='- **Serena not connected** (no `mcp__serena__*` tools): use grep and file reads, and say so in your report.'
c7_line='- **Not connected** (no `mcp__context7__*` tools) or rate-limited: same fallback; report it.'
# role <case> <file> <front-matter tools line or -> [serena fallback line or -] [context7 fallback
# line or -] [body text]. "-" leaves that part out. The Tools block mirrors the rendered one.
role() {
  local d="$tmp/$1" f="$tmp/$1/$2" tl="$3" sl="${4-$serena_line}" cl="${5-$c7_line}" body="${6:-}"
  mkdir -p "$d"
  {
    echo '---'
    echo "name: ${2%.md}"
    echo 'description: A fixture role.'
    [ "$tl" = "-" ] || echo "$tl"
    echo 'model: opus'
    echo '---'
    echo
    echo "# ${2%.md}"
    [ -z "$body" ] || printf '%s\n' "$body"
    echo '<!-- teamwright:tools:begin -->'
    echo '## Tools'
    echo '### context: serena'
    echo '- **Before reading files to understand code, ask Serena.** `get_symbols_overview`, `find_symbol`.'
    [ "$sl" = "-" ] || echo "$sl"
    echo '### docs: context7'
    echo '- **Before using a library or framework API** look it up: `resolve-library-id` → `query-docs`.'
    [ "$cl" = "-" ] || echo "$cl"
    echo '<!-- teamwright:tools:end -->'
    echo
    echo '## Rules'
  } >"$f"
}
# plain <case> <file> <tools line>: a role file without a Tools block.
plain() {
  mkdir -p "$tmp/$1"
  printf -- '---\nname: %s\ndescription: No Tools block.\n%s\n---\n\n# Role\n' "${2%.md}" "$3" >"$tmp/$1/$2"
}
# check <case> <want exit> <needle> [not-needle] [agents dir]
check() {
  local c="$1" want="$2" needle="$3" notneedle="${4:-}" dir="${5:-$tmp/$1}" out got
  if [ ! -x "$guard" ]; then
    fail "$c: $guard not found or not executable (want exit $want, needle '$needle')"
    return
  fi
  out="$("$guard" "$dir" "$manifest" 2>&1)"
  got=$?
  if [ "$got" -ne "$want" ] || ! grep -qF -- "$needle" <<<"$out"; then
    fail "$c: exit $got (want $want), needle '$needle'" "$out"
  elif [ -n "$notneedle" ] && grep -qF -- "$notneedle" <<<"$out"; then
    fail "$c: exit $got, output names '$notneedle'" "$out"
  else
    pass "$c: exit $got"
  fi
}

# Accepted.
role ok-all developer-core.md "tools: Read, Grep, Glob, Bash, Edit, Write, $all_names"
check ok-all 0 "ok: 1 role file"
role ok-order-spacing problem-investigator.md "tools:  $(tac <<<"$want_names" | paste -sd, | sed 's/,/ ,  /g') ,Read,Grep,Glob,Bash,Edit"
check ok-order-spacing 0 "ok: 1 role file"
# A role without a Tools block is ignored, whatever its tools: line says.
role ok-block-roles-only code-reviewer.md "tools: Read, Grep, Glob, Bash, Edit, Write, $all_names"
plain ok-block-roles-only technical-writer.md 'tools: Read, Grep, Glob, Bash, Edit, Write'
plain ok-block-roles-only requirements-reviewer.md 'tools: Read, Grep, mcp__serena__write_memory'
mkdir -p "$tmp/ok-block-roles-only/notes"
check ok-block-roles-only 0 "ok: 1 role file"
role ok-two developer-ui.md "tools: Read, Grep, Glob, Bash, Edit, Write, $all_names"
role ok-two test-writer-ui.md "tools: Read, Grep, Glob, Bash, Edit, Write, $all_names"
check ok-two 0 "ok: 2 role file"

# Refused: the allowlist as the kit renders it today (no MCP name at all).
role v-kit-allowlist task-validator.md 'tools: Read, Grep, Glob, Bash, Edit, Write'
check v-kit-allowlist 1 "mcp__serena__find_symbol"
check v-kit-allowlist 1 "task-validator.md"
# Refused: each pinned name missing on its own (one role, all other names present).
i=0
while IFS= read -r miss; do
  i=$((i + 1))
  rest="$(grep -vxF -- "$miss" <<<"$want_names" | paste -sd, | sed 's/,/, /g')"
  role "v-missing-$i" test-writer-core.md "tools: Read, Grep, Glob, Bash, Edit, Write, $rest"
  check "v-missing-$i" 1 "$miss"
done <<<"$want_names"
# Exact names, not substrings: a longer name holding a pinned one does not count.
# (ToolSearch, not an mcp__ name, so only R2 can refuse it.)
role v-substring developer-core.md "tools: Read, Bash, $(sed 's/^ToolSearch$/ToolSearchAll/' <<<"$want_names" | paste -sd, | sed 's/,/, /g')"
check v-substring 1 "ToolSearch"
# Refused: a Serena edit, memory or shell tool, another MCP tool, a wildcard or the bare server.
for bad in mcp__serena__replace_symbol_body mcp__serena__write_memory mcp__serena__execute_shell_command \
  mcp__serena__rename_symbol mcp__serena__insert_after_symbol mcp__serena__replace_content \
  mcp__serena__delete_memory 'mcp__serena__*' mcp__serena mcp__context7__get-library-docs; do
  c="v-extra-$(tr -c 'A-Za-z0-9_\n-' '_' <<<"$bad")"
  role "$c" developer-core.md "tools: Read, Grep, Glob, Bash, Edit, Write, $all_names, $bad"
  check "$c" 1 "$bad"
done
# Refused: no tools: line in the front matter (the role inherits every tool, Serena's edit tools too).
role v-no-tools-line developer-core.md -
check v-no-tools-line 1 "tools:"
# Refused: the names only on a tools: line in the body, the front-matter line without them.
role v-tools-in-body code-reviewer.md 'tools: Read, Grep, Glob, Bash, Edit, Write' "$serena_line" "$c7_line" "tools: $all_names"
check v-tools-in-body 1 "mcp__serena__get_symbols_overview"
# Refused: a fallback line of the Tools block missing (Acceptance 3: fall back, do not stop).
role v-no-serena-fallback developer-core.md "tools: Read, Grep, Glob, Bash, Edit, Write, $all_names" -
check v-no-serena-fallback 1 "not connected"
role v-serena-fallback-stops developer-core.md "tools: Read, Grep, Glob, Bash, Edit, Write, $all_names" \
  '- **Serena not connected** (no `mcp__serena__*` tools): stop and report.'
check v-serena-fallback-stops 1 "not connected"
role v-no-context7-fallback developer-core.md "tools: Read, Grep, Glob, Bash, Edit, Write, $all_names" "$serena_line" -
check v-no-context7-fallback 1 "not connected"
# The fallback must be inside the Tools block: a copy elsewhere in the body does not count.
role v-fallback-outside-block developer-core.md "tools: Read, Grep, Glob, Bash, Edit, Write, $all_names" - "$c7_line" "$serena_line"
check v-fallback-outside-block 1 "not connected"
# Refused files are named; accepted ones are not.
role v-mixed problem-investigator.md 'tools: Read, Grep, Glob, Bash, Edit'
role v-mixed developer-core.md "tools: Read, Grep, Glob, Bash, Edit, Write, $all_names"
check v-mixed 1 "problem-investigator.md" "developer-core.md"

# Cannot run.
check c-no-dir 3 "cannot run" "" "$tmp/absent"
if [ -x "$guard" ]; then
  role c-manifest ok.md "tools: Read, $all_names"
  for m in absent comments-only; do
    [ "$m" = absent ] || printf '# only a comment\n\n' >"$tmp/$m.txt"
    out="$("$guard" "$tmp/c-manifest" "$tmp/$m.txt" 2>&1)"
    got=$?
    if [ "$got" -eq 3 ] && grep -qF "cannot run" <<<"$out"; then pass "c-manifest-$m: exit $got"; else fail "c-manifest-$m: exit $got (want 3), needle 'cannot run'" "$out"; fi
  done
else
  fail "c-manifest: $guard not found or not executable"
fi

# The real tree with the guard's defaults: the 7 code-reading roles carry a Tools block and pass.
if [ ! -x "$guard" ]; then
  fail "real tree: $guard not found or not executable (want exit 0, 'ok: 7 role file(s)')"
else
  out="$("$guard" 2>&1)"
  got=$?
  if [ "$got" -eq 0 ] && grep -qF "ok: 7 role file" <<<"$out"; then
    pass "real tree: exit 0"
  else
    fail "real tree: exit $got (want 0, 'ok: 7 role file(s)')" "$out"
  fi
fi

echo "role-tools.test: $passed passed, $failed failed"
[ "$failed" -eq 0 ] || exit 1
