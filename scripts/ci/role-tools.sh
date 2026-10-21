#!/usr/bin/env bash
# Role-file tripwire (T-082, decisions #145; rewritten by T-113, rca class guard-model, decisions
# #154, P-020 decides_on: literal). Claude Code reads the YAML front matter of every file under
# .claude/agents/ (tools, mcpServers, disallowedTools, permissionMode, ...) to decide what a
# subagent may do; this guard never reads that YAML. It decides on bytes the project owns: one
# reviewed head per role file, scripts/ci/role-heads/<name>.md, byte-exact from byte 0 through the
# closing --- line, and the set of role files equal to the set of heads. T-082's version read the
# front matter itself and passed every shape it did not model (T-113 Investigation, probes A-J).
#
# Contract (pinned by scripts/ci/role-tools.test.sh, whose header is authoritative):
#   Role files: every entry under agents-dir that is not a directory, recursively (dotfiles,
#     non-.md files and symlinks included), named by its path relative to agents-dir.
#   Heads: the regular files directly in heads-dir; head <name> belongs to role file <name>.
#   H1 the set of role files equals the set of heads.
#   H2 a role file's first size(head) bytes are byte-equal to its head (cmp; no CRLF, BOM or blank
#      normalisation); a shorter role file is a violation.
#   H3 every head starts with the line "---", ends with the line "---" plus a newline, and holds no
#      other "---" line.
#   R4 a role file whose head holds the raw token mcp__ has, between a line
#      <!-- teamwright:tools:begin --> and the next <!-- teamwright:tools:end --> line, both owned
#      fallback lines below as full lines (a missing tool means fall back, not stop).
# Not caught (docs/decisions/context-tools.md): Claude Code reading YAML past the closing --- of a
# head; user-level and plugin agents; whether the pinned text grants what the tool journal shows.
#
# Usage: scripts/ci/role-tools.sh [agents-dir [heads-dir]]
#   (defaults .claude/agents, scripts/ci/role-heads)
# Exit 0: "role-tools: ok: <n> role file(s)". Exit 1: one "<file>: ..." line per violation.
# Exit 3: cannot run (agents dir or heads dir missing, heads dir without a head); never a pass.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
dir="${1:-.claude/agents}"
dir="${dir%/}"
heads="${2:-scripts/ci/role-heads}"
heads="${heads%/}"
cannot_run() { echo "role-tools: cannot run: $1" >&2; exit 3; }
[ -d "$dir" ] && [ -r "$dir" ] || cannot_run "agents dir $dir not found or not readable"
[ -d "$heads" ] && [ -r "$heads" ] || cannot_run "heads dir $heads not found or not readable"

# The owned fallback lines of the rendered Tools block (full lines, byte-equal).
fallback_serena='- **Serena not connected** (no `mcp__serena__*` tools): use grep and file reads, and say so in your report.'
fallback_c7='- **Not connected** (no `mcp__context7__*` tools) or rate-limited: same fallback; report it.'
begin='<!-- teamwright:tools:begin -->'
end='<!-- teamwright:tools:end -->'

declare -A head_of=() role_of=()
mapfile -d '' hlist < <(find "$heads" -mindepth 1 -maxdepth 1 -type f -print0 | LC_ALL=C sort -z)
[ "${#hlist[@]}" -gt 0 ] || cannot_run "heads dir $heads holds no head"
for h in "${hlist[@]}"; do head_of["${h##*/}"]="$h"; done
mapfile -d '' rlist < <(find "$dir" -mindepth 1 ! -type d -print0 | LC_ALL=C sort -z)
for f in "${rlist[@]}"; do role_of["${f#"$dir"/}"]="$f"; done

report=""
add() { report+="$1"$'\n'; }

# H3: the owned heads end where the front matter ends.
for name in "${!head_of[@]}"; do
  h="${head_of[$name]}"
  [ -r "$h" ] || cannot_run "head $h is not readable"
  if ! cmp -s <(head -c 4 "$h") <(printf -- '---\n') ||
    ! cmp -s <(tail -c 5 "$h") <(printf '\n---\n') ||
    [ "$(LC_ALL=C grep -cxF -- '---' "$h")" -ne 2 ]; then
    add "$h: H3: the head must start with a --- line, end with a --- line and a newline, and hold no other --- line"
  fi
  [ -n "${role_of[$name]+x}" ] || add "$h: H1: no role file $dir/$name for this head"
done

n=0
for rel in "${!role_of[@]}"; do
  f="${role_of[$rel]}"
  if [ -z "${head_of[$rel]+x}" ]; then
    add "$f: H1: no owned head $heads/$rel (every file under $dir needs a reviewed head)"
    continue
  fi
  n=$((n + 1))
  h="${head_of[$rel]}"
  size="$(wc -c <"$h")"
  if ! cmp -s -n "$size" -- "$h" "$f"; then
    add "$f: H2: front matter is not byte-equal to its owned head $h"
  fi
  if LC_ALL=C grep -qF -- 'mcp__' "$h"; then
    if ! LC_ALL=C awk -v b="$begin" -v e="$end" -v s="$fallback_serena" -v c="$fallback_c7" '
      $0 == b { inb = 1; hs = 0; hc = 0; next }
      $0 == e { if (inb && hs && hc) ok = 1; inb = 0; next }
      inb && $0 == s { hs = 1 }
      inb && $0 == c { hc = 1 }
      END { exit ok ? 0 : 1 }
    ' "$f" 2>/dev/null; then
      add "$f: R4: no Tools block holding both owned \"not connected\" fallback lines as full lines (not connected = fall back, not stop)"
    fi
  fi
done

if [ -n "$report" ]; then
  printf '%s' "$report" | LC_ALL=C sort
  v="$(printf '%s' "$report" | wc -l)"
  echo "role-tools: FAIL: $v violation(s); a role file changes only with its owned head in $heads (T-113, decisions #154)" >&2
  exit 1
fi
echo "role-tools: ok: $n role file(s)"
