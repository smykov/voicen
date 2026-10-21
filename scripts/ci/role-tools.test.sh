#!/usr/bin/env bash
# T-082 (decisions #145), rewritten by T-113 (rca, class guard-model, decision #154, P-020
# decides_on: literal): host test of scripts/ci/role-tools.sh, the tripwire over the subagent role
# files in .claude/agents/. Claude Code reads their YAML front matter (tools, mcpServers,
# disallowedTools, permissionMode, ...); the guard never does. It decides on bytes it owns: one
# reviewed head per role file, scripts/ci/role-heads/<name>.md, byte-exact from byte 0 through the
# closing --- line, and the set of role files equal to the set of heads. T-082's guard read the
# front matter itself (first --- pair, first tools: line, comma split, files with a Tools block
# only) and passed every shape it did not model (T-113 Investigation, probes A-J).
#
# Contract of role-tools.sh [agents-dir [heads-dir]]
#   (defaults: .claude/agents, scripts/ci/role-heads)
#   Role files: every entry under agents-dir that is not a directory, recursively, whatever its
#     name (dotfiles, non-.md files and symlinks included); no Tools-block filter. Named by their
#     path relative to agents-dir.
#   Heads: the regular files directly in heads-dir; head <name> belongs to role file <name>.
#   H1 the set of role files equals the set of heads: a role file without a head (a file in a
#      subdirectory, an extra file) and a head without a role file are each a violation.
#   H2 a role file's first size(head) bytes are byte-equal to its head (no CRLF, BOM or blank
#      normalisation); a shorter role file is a violation.
#   H3 every head starts with the line "---", ends with the line "---" and a newline, and holds no
#      other "---" line (so every reading of the front matter ends where the head ends).
#   R4 a role file whose head holds the raw token mcp__ has a <!-- teamwright:tools:begin -->
#      line, and between it and the next <!-- teamwright:tools:end --> line both owned fallback
#      lines as full lines (byte-equal lines, no prefix, suffix or indentation): the role falls
#      back to built-in reads when a tool is not connected, it does not stop (T-082 Acceptance 3).
#      A role whose head holds no mcp__ needs no Tools block (technical-writer,
#      requirements-reviewer).
#   Exit 0: "role-tools: ok: <n> role file(s)" (n = number of role files compared).
#   Exit 1: one line per violation naming the role file (H1 extra, H2, R4) or the head (H1
#     missing role file, H3); R4 lines say "not connected".
#   Exit 3: "role-tools: cannot run: ..." - agents dir or heads dir missing, heads dir without a
#     head. Never reported as a pass.
# Replaced T-082 cases (manifest contract R1-R3, scripts/ci/role-tools.txt): every refused
# tools-line shape is kept as a refusal (now: the role file differs from its head). Two accepted
# cases became refusals, because the contract is stricter: ok-order-spacing (a reordered or
# re-spaced tools: line is not the reviewed text) and the role-without-Tools-block part of
# ok-block-roles-only (no marker filter any more). The manifest pin became the real-heads pin
# below (the 7 code-reading roles' heads hold exactly the T-082 allowlist line).
# Not caught (docs/decisions/context-tools.md): Claude Code reading YAML past the closing --- of
# a head; user-level (~/.claude/agents) and plugin agents; whether the pinned text grants what the
# tool journal shows (evidenced per role by the journal, not by this gate).
#
# Usage: scripts/ci/role-tools.test.sh
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
guard=scripts/ci/role-tools.sh
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

# The read/navigation tools of the context and docs providers, exactly (T-082 analysis, checked
# against serena-agent 2.0.0.dev0's claude-code context and the deferred-tool list of a session).
want_names='mcp__serena__get_symbols_overview
mcp__serena__find_symbol
mcp__serena__find_referencing_symbols
mcp__serena__find_implementations
mcp__serena__find_declaration
mcp__context7__resolve-library-id
mcp__context7__query-docs
ToolSearch'
all_names="$(paste -sd, <<<"$want_names" | sed 's/,/, /g')"
good_tools="tools: Read, Grep, Glob, Bash, Edit, Write, $all_names"

serena_line='- **Serena not connected** (no `mcp__serena__*` tools): use grep and file reads, and say so in your report.'
c7_line='- **Not connected** (no `mcp__context7__*` tools) or rate-limited: same fallback; report it.'

# fm <name> <tools line or -> [extra front-matter lines]: a front matter, --- to ---, no final
# newline (callers add it).
fm() {
  printf -- '---\nname: %s\ndescription: A fixture role.\n' "${1%.md}"
  [ "$2" = "-" ] || printf '%s\n' "$2"
  [ -z "${3:-}" ] || printf '%s\n' "$3"
  printf 'model: opus\n---'
}
# body [serena fallback line or -] [context7 fallback line or -] [text before the block]
# [block or -]: the body after the front matter; the Tools block mirrors the rendered one.
body() {
  local sl="${1-$serena_line}" cl="${2-$c7_line}" pre="${3:-}" blk="${4:-}"
  echo
  echo '# Role'
  [ -z "$pre" ] || printf '%s\n' "$pre"
  if [ "$blk" != "-" ]; then
    echo '<!-- teamwright:tools:begin -->'
    echo '## Tools'
    echo '### context: serena'
    echo '- **Before reading files to understand code, ask Serena.** `get_symbols_overview`, `find_symbol`.'
    [ "$sl" = "-" ] || printf '%s\n' "$sl"
    echo '### docs: context7'
    echo '- **Before using a library or framework API** look it up: `resolve-library-id` → `query-docs`.'
    [ "$cl" = "-" ] || printf '%s\n' "$cl"
    echo '<!-- teamwright:tools:end -->'
  fi
  echo
  echo '## Rules'
}
# mkhead <case> <name> <front matter>: writes heads/<name> = front matter + newline.
mkhead() {
  mkdir -p "$tmp/$1/heads" "$tmp/$1/agents"
  printf '%s\n' "$3" >"$tmp/$1/heads/$2"
}
# agent <case> <relpath> <front matter> [body args...]: writes agents/<relpath>.
agent() {
  local c="$1" p="$2" f="$3"
  shift 3
  mkdir -p "$tmp/$c/agents/$(dirname "$p")" "$tmp/$c/heads"
  { printf '%s\n' "$f"; body "$@"; } >"$tmp/$c/agents/$p"
}
# pair <case> <name> <tools line or -> [block or -]: a head and its role file, front matter equal.
pair() {
  local f
  f="$(fm "$2" "$3")"
  mkhead "$1" "$2" "$f"
  if [ "${4:-}" = "-" ]; then agent "$1" "$2" "$f" "" "" "" -; else agent "$1" "$2" "$f"; fi
}
# check <case> <want exit> <needle> [not-needle] [agents dir] [heads dir]
check() {
  local c="$1" want="$2" needle="$3" notneedle="${4:-}" dir="${5:-$tmp/$1/agents}" hd="${6:-$tmp/$1/heads}" out got
  if [ ! -x "$guard" ]; then
    fail "$c: $guard not found or not executable (want exit $want, needle '$needle')"
    return
  fi
  out="$("$guard" "$dir" "$hd" 2>&1)"
  got=$?
  if [ "$got" -ne "$want" ] || ! grep -qF -- "$needle" <<<"$out"; then
    fail "$c: exit $got (want $want), needle '$needle'" "$out"
  elif [ -n "$notneedle" ] && grep -qF -- "$notneedle" <<<"$out"; then
    fail "$c: exit $got, output names '$notneedle'" "$out"
  else
    pass "$c: exit $got"
  fi
}
# variant <case> <name> <role-file front matter> [body args...]: the head is the reviewed front
# matter (developer-core-like, good_tools); the role file carries another front matter.
variant() {
  local c="$1" n="$2" f="$3"
  shift 3
  mkhead "$c" "$n" "$(fm "$n" "$good_tools")"
  agent "$c" "$n" "$f" "$@"
}

# ---- Accepted ---------------------------------------------------------------------------------
pair ok-all developer-core.md "$good_tools"
check ok-all 0 "ok: 1 role file"
pair ok-two developer-ui.md "$good_tools"
pair ok-two test-writer-ui.md "$good_tools"
check ok-two 0 "ok: 2 role file"
# A role whose head names no mcp__ tool needs no Tools block; an empty subdirectory is no role file.
pair ok-no-block technical-writer.md 'tools: Read, Grep, Glob, Bash, Edit, Write' -
pair ok-no-block code-reviewer.md "$good_tools"
mkdir -p "$tmp/ok-no-block/agents/notes"
check ok-no-block 0 "ok: 2 role file"
# What follows the head is the body: a body that differs between roles, or holds YAML-like lines,
# does not matter (the head ends at its closing --- line).
pair ok-body developer-core.md "$good_tools"
printf 'tools: Read, mcp__serena__write_memory\n' >>"$tmp/ok-body/agents/developer-core.md"
check ok-body 0 "ok: 1 role file"

# ---- Refused: the role file's front matter is not its head (T-082 R1-R3 shapes, kept) ---------
variant v-kit-allowlist task-validator.md "$(fm task-validator.md 'tools: Read, Grep, Glob, Bash, Edit, Write')"
check v-kit-allowlist 1 "task-validator.md"
i=0
while IFS= read -r miss; do
  i=$((i + 1))
  rest="$(grep -vxF -- "$miss" <<<"$want_names" | paste -sd, | sed 's/,/, /g')"
  variant "v-missing-$i" test-writer-core.md "$(fm test-writer-core.md "tools: Read, Grep, Glob, Bash, Edit, Write, $rest")"
  check "v-missing-$i" 1 "test-writer-core.md"
done <<<"$want_names"
variant v-substring developer-core.md "$(fm developer-core.md "tools: Read, Grep, Glob, Bash, Edit, Write, ${all_names/ToolSearch/ToolSearchAll}")"
check v-substring 1 "developer-core.md"
for bad in mcp__serena__replace_symbol_body mcp__serena__write_memory mcp__serena__execute_shell_command \
  mcp__serena__rename_symbol mcp__serena__insert_after_symbol mcp__serena__replace_content \
  mcp__serena__delete_memory 'mcp__serena__*' mcp__serena mcp__context7__get-library-docs; do
  c="v-extra-$(tr -c 'A-Za-z0-9_\n-' '_' <<<"$bad")"
  variant "$c" developer-core.md "$(fm developer-core.md "$good_tools, $bad")"
  check "$c" 1 "developer-core.md"
done
variant v-no-tools-line developer-core.md "$(fm developer-core.md -)"
check v-no-tools-line 1 "developer-core.md"
variant v-tools-in-body code-reviewer.md "$(fm code-reviewer.md 'tools: Read, Grep, Glob, Bash, Edit, Write')" "$serena_line" "$c7_line" "tools: $all_names"
check v-tools-in-body 1 "code-reviewer.md"
# Was accepted by T-082 (ok-order-spacing): the same names reordered and re-spaced are not the
# reviewed text.
variant v-order-spacing problem-investigator.md "$(fm problem-investigator.md "tools:  $(tac <<<"$want_names" | paste -sd, | sed 's/,/ ,  /g') ,Read,Grep,Glob,Bash,Edit,Write")"
check v-order-spacing 1 "problem-investigator.md"
# Was ignored by T-082 (no Tools block): requirements-reviewer granting a Serena write tool.
pair v-no-block-role requirements-reviewer.md 'tools: Read, Grep, Glob, Bash, Write' -
agent v-no-block-role requirements-reviewer.md "$(fm requirements-reviewer.md 'tools: Read, Grep, mcp__serena__write_memory')" "" "" "" -
check v-no-block-role 1 "requirements-reviewer.md"

# ---- Refused: YAML shapes T-082's reading passed (review probes A-D, investigator E-I, J) ----
# Each role file starts with the developer-core-like head's lines and changes only the front
# matter; the body keeps a complete Tools block, so only the front matter can refuse it.
variant p-A-flow-list x.md "$(fm x.md "tools: [mcp__serena__write_memory, $all_names, Read, Grep, Glob, Bash, Edit, Write]")"
check p-A-flow-list 1 "x.md"
variant p-B-duplicate-key x.md "$(fm x.md "$good_tools" 'tools: Read, Edit, mcp__serena__execute_shell_command')"
check p-B-duplicate-key 1 "x.md"
variant p-C-continuation x.md "$(fm x.md "$good_tools," '  mcp__serena__replace_symbol_body')"
check p-C-continuation 1 "x.md"
variant p-D-quoted-element x.md "$(fm x.md "$good_tools, 'mcp__serena__write_memory'")"
check p-D-quoted-element 1 "x.md"
variant p-E-mcp-servers x.md "$(fm x.md "$good_tools" 'mcpServers:
  - serena-rw:
      type: stdio
      command: serena
      args: [start-mcp-server, --context, ide]')"
check p-E-mcp-servers 1 "x.md"
variant p-F-permission-mode x.md "$(fm x.md "$good_tools" 'permissionMode: bypassPermissions')"
check p-F-permission-mode 1 "x.md"
variant p-G-disallowed-tools x.md "$(fm x.md "$good_tools" 'disallowedTools: mcp__serena__find_symbol, ToolSearch')"
check p-G-disallowed-tools 1 "x.md"
variant p-H-quoted-key x.md "$(fm x.md "$good_tools" '"tools": Read, Edit, Write, mcp__serena__write_memory')"
check p-H-quoted-key 1 "x.md"
variant p-I-anchor-alias x.md "$(fm x.md "$good_tools" 'x: &a Read, Edit, Write, mcp__serena__execute_shell_command
? tools
: *a')"
check p-I-anchor-alias 1 "x.md"
variant p-block-sequence x.md "$(fm x.md 'tools:
  - Read
  - Edit
  - mcp__serena__write_memory')"
check p-block-sequence 1 "x.md"
# J: begin marker removed and a Serena write tool appended (T-082 skipped the file).
variant p-J-marker-removed-write x.md "$(fm x.md "$good_tools, mcp__serena__write_memory")"
sed -i '/^<!-- teamwright:tools:begin -->$/d' "$tmp/p-J-marker-removed-write/agents/x.md"
check p-J-marker-removed-write 1 "x.md"
# One byte of the description changed.
pair v-description-byte developer-core.md "$good_tools"
sed -i '0,/^description: A fixture role\.$/s//description: A fixture role!/' "$tmp/v-description-byte/agents/developer-core.md"
check v-description-byte 1 "developer-core.md"
# Raw bytes, no normalisation: a CRLF copy and a BOM-prefixed copy of a matching role file.
pair v-crlf developer-core.md "$good_tools"
sed -i 's/$/\r/' "$tmp/v-crlf/agents/developer-core.md"
check v-crlf 1 "developer-core.md"
# The same on a role whose head names no mcp__ tool (no Tools block, so only the head compare can
# refuse it).
pair v-crlf-no-block technical-writer.md 'tools: Read, Grep, Glob, Bash, Edit, Write' -
sed -i 's/$/\r/' "$tmp/v-crlf-no-block/agents/technical-writer.md"
check v-crlf-no-block 1 "technical-writer.md"
pair v-bom developer-core.md "$good_tools"
{ printf '\357\273\277'; cat "$tmp/v-bom/heads/developer-core.md"; body; } >"$tmp/v-bom/agents/developer-core.md"
check v-bom 1 "developer-core.md"
# A role file shorter than its head (cut inside the front matter).
pair v-truncated developer-core.md "$good_tools"
head -c 40 "$tmp/v-truncated/heads/developer-core.md" >"$tmp/v-truncated/agents/developer-core.md"
check v-truncated 1 "developer-core.md"

# ---- H1: the set of role files equals the set of heads ------------------------------------
# An extra agent in a subdirectory, even a byte copy of a reviewed role.
pair v-subdir x.md "$good_tools"
mkdir -p "$tmp/v-subdir/agents/sub"
cp "$tmp/v-subdir/agents/x.md" "$tmp/v-subdir/agents/sub/x.md"
check v-subdir 1 "sub/x.md"
pair v-subdir-only x.md "$good_tools"
mkdir -p "$tmp/v-subdir-only/agents/sub"
mv "$tmp/v-subdir-only/agents/x.md" "$tmp/v-subdir-only/agents/sub/x.md"
check v-subdir-only 1 "x.md"
pair v-dotfile x.md "$good_tools"
cp "$tmp/v-dotfile/agents/x.md" "$tmp/v-dotfile/agents/.y.md"
check v-dotfile 1 ".y.md"
pair v-other-ext x.md "$good_tools"
printf -- '---\nname: notes\n---\n' >"$tmp/v-other-ext/agents/notes.txt"
check v-other-ext 1 "notes.txt"
pair v-symlink x.md "$good_tools"
ln -s x.md "$tmp/v-symlink/agents/y.md"
check v-symlink 1 "y.md"
pair v-extra-role x.md "$good_tools"
agent v-extra-role rogue.md "$(fm rogue.md "$good_tools")"
check v-extra-role 1 "rogue.md"
# A head without its role file.
pair v-missing-role x.md "$good_tools"
mkhead v-missing-role gone.md "$(fm gone.md "$good_tools")"
check v-missing-role 1 "gone.md"
mkdir -p "$tmp/v-no-roles/agents"
mkhead v-no-roles x.md "$(fm x.md "$good_tools")"
check v-no-roles 1 "x.md"

# ---- H3: the owned heads end where the front matter ends -------------------------------------
# A head with a third --- line: the role's front matter (first --- pair) is a prefix of it, the
# rest is YAML Claude Code may read.
f3="$(fm x.md "$good_tools")"$'\ntools: Read, Edit, mcp__serena__execute_shell_command\n---'
mkhead v-head-three-dashes x.md "$f3"
agent v-head-three-dashes x.md "$f3"
check v-head-three-dashes 1 "x.md"
# A head without its closing --- line: the role file may continue the front matter.
fo="$(fm x.md "$good_tools")"
fo="${fo%$'\n---'}"
mkhead v-head-unclosed x.md "$fo"
agent v-head-unclosed x.md "$fo"$'\npermissionMode: bypassPermissions\n---'
check v-head-unclosed 1 "x.md"
# A head whose closing --- has no newline: the role file may continue that line.
pair v-head-no-newline x.md "$good_tools"
printf '%s' "$(fm x.md "$good_tools")" >"$tmp/v-head-no-newline/heads/x.md"
check v-head-no-newline 1 "x.md"
# A head with CRLF lines (its role file a byte copy plus body) and a BOM-prefixed head.
pair v-head-crlf x.md "$good_tools"
sed -i 's/$/\r/' "$tmp/v-head-crlf/heads/x.md" "$tmp/v-head-crlf/agents/x.md"
check v-head-crlf 1 "x.md"
pair v-head-bom x.md "$good_tools"
{ printf '\357\273\277'; cat "$tmp/v-head-bom/agents/x.md"; } >"$tmp/v-head-bom/agents/x.tmp"
mv "$tmp/v-head-bom/agents/x.tmp" "$tmp/v-head-bom/agents/x.md"
{ printf '\357\273\277'; printf '%s\n' "$(fm x.md "$good_tools")"; } >"$tmp/v-head-bom/heads/x.md"
check v-head-bom 1 "x.md"

# ---- R4: the fallback lines inside the Tools block (head names mcp__) ------------------------
pair v-no-serena-fallback developer-core.md "$good_tools"
agent v-no-serena-fallback developer-core.md "$(fm developer-core.md "$good_tools")" -
check v-no-serena-fallback 1 "not connected"
pair v-serena-fallback-stops developer-core.md "$good_tools"
agent v-serena-fallback-stops developer-core.md "$(fm developer-core.md "$good_tools")" '- **Serena not connected** (no `mcp__serena__*` tools): stop and report.'
check v-serena-fallback-stops 1 "not connected"
pair v-no-context7-fallback developer-core.md "$good_tools"
agent v-no-context7-fallback developer-core.md "$(fm developer-core.md "$good_tools")" "$serena_line" -
check v-no-context7-fallback 1 "not connected"
pair v-fallback-outside-block developer-core.md "$good_tools"
agent v-fallback-outside-block developer-core.md "$(fm developer-core.md "$good_tools")" - "$c7_line" "$serena_line"
check v-fallback-outside-block 1 "not connected"
# Full lines: the owned text with a suffix, or indented, is not the owned line.
pair v-fallback-suffix developer-core.md "$good_tools"
agent v-fallback-suffix developer-core.md "$(fm developer-core.md "$good_tools")" "$serena_line Then stop."
check v-fallback-suffix 1 "not connected"
pair v-fallback-indented developer-core.md "$good_tools"
agent v-fallback-indented developer-core.md "$(fm developer-core.md "$good_tools")" "$serena_line" "  $c7_line"
check v-fallback-indented 1 "not connected"
# Marker removed, front matter unchanged: the role keeps its MCP tools, so it keeps the block.
pair v-marker-removed developer-core.md "$good_tools"
sed -i '/^<!-- teamwright:tools:begin -->$/d' "$tmp/v-marker-removed/agents/developer-core.md"
check v-marker-removed 1 "not connected"
pair v-block-removed developer-core.md "$good_tools"
agent v-block-removed developer-core.md "$(fm developer-core.md "$good_tools")" "" "" "" -
check v-block-removed 1 "not connected"
# Refused files are named; accepted ones are not.
pair v-mixed developer-core.md "$good_tools"
variant v-mixed problem-investigator.md "$(fm problem-investigator.md 'tools: Read, Grep, Glob, Bash, Edit')"
check v-mixed 1 "problem-investigator.md" "developer-core.md"

# ---- Cannot run ---------------------------------------------------------------------------
pair c-no-dir x.md "$good_tools"
check c-no-dir 3 "cannot run" "" "$tmp/absent" "$tmp/c-no-dir/heads"
pair c-no-heads x.md "$good_tools"
check c-no-heads 3 "cannot run" "" "$tmp/c-no-heads/agents" "$tmp/absent"
mkdir -p "$tmp/c-empty-heads/agents" "$tmp/c-empty-heads/heads"
check c-empty-heads 3 "cannot run"
pair c-empty-heads-roles x.md "$good_tools"
rm "$tmp/c-empty-heads-roles/heads/x.md"
check c-empty-heads-roles 3 "cannot run"

# ---- The real tree ------------------------------------------------------------------------
# The owned heads keep T-082's allowlist: the 7 code-reading roles' heads hold exactly the pinned
# tools line, and it is their only line with the raw token mcp__; the other heads hold no mcp__.
heads=scripts/ci/role-heads
for r in code-reviewer developer-core developer-ui problem-investigator task-validator test-writer-core test-writer-ui; do
  want="$good_tools"
  [ "$r" != problem-investigator ] || want="tools: Read, Grep, Glob, Bash, Edit, $all_names"
  if [ ! -f "$heads/$r.md" ]; then
    fail "real head $r: $heads/$r.md missing"
  elif ! grep -qxF -- "$want" "$heads/$r.md" || [ "$(grep -c 'mcp__' "$heads/$r.md")" -ne 1 ]; then
    fail "real head $r: not exactly one mcp__ line, the pinned tools line" "$(grep -n 'mcp__' "$heads/$r.md")"
  else
    pass "real head $r: pinned tools line"
  fi
done
for r in requirements-reviewer technical-writer; do
  if [ ! -f "$heads/$r.md" ]; then
    fail "real head $r: $heads/$r.md missing"
  elif grep -q 'mcp__' "$heads/$r.md"; then
    fail "real head $r: names an mcp__ tool" "$(grep -n 'mcp__' "$heads/$r.md")"
  else
    pass "real head $r: no mcp__ tool"
  fi
done
# The guard with its defaults on the real .claude/agents: all 9 role files match their heads.
if [ ! -x "$guard" ]; then
  fail "real tree: $guard not found or not executable (want exit 0, 'ok: 9 role file(s)')"
else
  out="$("$guard" 2>&1)"
  got=$?
  if [ "$got" -eq 0 ] && grep -qF "ok: 9 role file" <<<"$out"; then
    pass "real tree: exit 0"
  else
    fail "real tree: exit $got (want 0, 'ok: 9 role file(s)')" "$out"
  fi
fi

echo "role-tools.test: $passed passed, $failed failed"
[ "$failed" -eq 0 ] || exit 1
