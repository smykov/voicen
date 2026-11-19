#!/usr/bin/env bash
# T-237 (decisions #272, #321, #327, #328; P-020 decides_on: literal + token): tripwire over the
# contour stage session's Serena grant. The session reaches Serena only through the five owned read
# tools, and Serena starts with `--agent-interface tools` (docs/decisions/context-tools.md).
# No YAML, JSON or shell is parsed: cmp, fixed-string grep, raw token counts (comments included).
#
#   L1 .mcp.json is byte-equal to scripts/ci/mcp.json (cmp, no normalisation).
#   L2 the owned copy holds the full lines `        "--agent-interface",` and `        "tools"`
#      adjacent, and the raw token `"serena": {` exactly once.
#   L3 scripts/ci/serena-tools.txt holds five names; joined by `,` they occur exactly once in
#      .github/workflows/teamwright-agent.yml.
#   L4 joined by `, ` they occur exactly once in every role head holding mcp__serena (at least one).
#   L5 each of the two owned preflight jq lines occurs exactly once in teamwright-agent.yml.
#   T1 `mcp__serena` occurs exactly 5 times in all of .github/workflows/.
#   T2 `mcp__serena` occurs exactly 5 times in each head that holds it.
#   T3 `mcp__serena` absent from .claude/settings.json.
#   T4 `allowedTools` exactly once in .github/workflows/.
#   T5 `allowed-tools`, `bypassPermissions`, `dangerously-skip-permissions`, `--settings` absent
#      from .github/workflows/.
#
# Usage: scripts/ci/serena-pin.sh [root]   (default: the repository root)
# Exit 0: "serena-pin: ok". Exit 1: one line per violation, starting with the rule id.
# Exit 3: "serena-pin: cannot run: ..." (an owned file or a scanned path missing).
set -uo pipefail
root="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"

cannot() { echo "serena-pin: cannot run: $1" >&2; exit 3; }
[ -d "$root" ] || cannot "root $root is not a directory"
cd "$root" || cannot "cd $root failed"

mcp=.mcp.json
owned_mcp=scripts/ci/mcp.json
names=scripts/ci/serena-tools.txt
settings=.claude/settings.json
wfdir=.github/workflows
wf=$wfdir/teamwright-agent.yml
heads=scripts/ci/role-heads
for f in "$owned_mcp" "$names" "$mcp" "$settings" "$wf"; do
  [ -f "$f" ] && [ -r "$f" ] || cannot "$f missing"
done
for d in "$wfdir" "$heads"; do
  [ -d "$d" ] || cannot "$d missing"
done

bad=0
violation() { echo "$1" >&2; bad=1; }
# count <token> <path...>: raw non-overlapping occurrences of a fixed string (recursive for dirs).
count() {
  local tok="$1"; shift
  grep -r -o -F -- "$tok" "$@" 2>/dev/null | wc -l | tr -d ' '
}

# L1
cmp -s "$mcp" "$owned_mcp" || violation "L1 $mcp differs from $owned_mcp (edit both together)"

# L2
flag_a='        "--agent-interface",'
flag_b='        "tools"'
adj="$(awk -v a="$flag_a" -v b="$flag_b" 'prev == a && $0 == b { n++ } { prev = $0 } END { print n + 0 }' "$owned_mcp")"
[ "$adj" = 1 ] || violation "L2 $owned_mcp: the adjacent lines '$flag_a' / '$flag_b' occur $adj times, want 1"
n="$(count '"serena": {' "$owned_mcp")"
[ "$n" = 1 ] || violation "L2 $owned_mcp: '\"serena\": {' occurs $n times, want 1"

# L3
nnames="$(grep -c '' "$names")"
[ "$nnames" = 5 ] || violation "L3 $names holds $nnames names, want 5"
five_comma="$(paste -sd, "$names")"
five_space="${five_comma//,/, }"
if [ -n "$five_comma" ]; then
  n="$(count "$five_comma" "$wf")"
  [ "$n" = 1 ] || violation "L3 $wf: the owned allowlist '$five_comma' occurs $n times, want 1"
else
  violation "L3 $names is empty"
fi

# L4, T2
holders=0
for h in "$heads"/*; do
  [ -f "$h" ] || continue
  t="$(count mcp__serena "$h")"
  [ "$t" = 0 ] && continue
  holders=$((holders + 1))
  [ "$t" = 5 ] || violation "T2 $h: mcp__serena occurs $t times, want 5"
  if [ -n "$five_space" ]; then
    n="$(count "$five_space" "$h")"
    [ "$n" = 1 ] || violation "L4 $h: the owned names joined by ', ' occur $n times, want 1"
  fi
done
[ "$holders" -gt 0 ] || violation "L4 no head under $heads names mcp__serena"

# L5
pre_serena="jq '{mcpServers: (.mcpServers | {serena, context7})}' .mcp.json > \"\$RUNNER_TEMP/mcp.json\""
pre_context7="jq '{mcpServers: (.mcpServers | {context7})}' .mcp.json > \"\$RUNNER_TEMP/mcp.json\""
for line in "$pre_serena" "$pre_context7"; do
  n="$(count "$line" "$wf")"
  [ "$n" = 1 ] || violation "L5 $wf: preflight line '$line' occurs $n times, want 1"
done

# T1
n="$(count mcp__serena "$wfdir")"
[ "$n" = 5 ] || violation "T1 $wfdir: mcp__serena occurs $n times, want 5 (only inside the owned allowlist)"

# T3
n="$(count mcp__serena "$settings")"
[ "$n" = 0 ] || violation "T3 $settings: mcp__serena occurs $n times, want 0"

# T4
n="$(count allowedTools "$wfdir")"
[ "$n" = 1 ] || violation "T4 $wfdir: allowedTools occurs $n times, want 1"

# T5
for tok in allowed-tools bypassPermissions dangerously-skip-permissions --settings; do
  n="$(count "$tok" "$wfdir")"
  [ "$n" = 0 ] || violation "T5 $wfdir: '$tok' occurs $n times, want 0"
done

[ "$bad" = 0 ] || exit 1
echo "serena-pin: ok"
exit 0
