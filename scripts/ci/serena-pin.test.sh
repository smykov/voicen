#!/usr/bin/env bash
# T-237 (decisions #327, #328; P-020 decides_on: literal + token): host test of
# scripts/ci/serena-pin.sh, the tripwire over the contour's Serena grant. Red first: the script
# does not exist yet, so every case fails.
#
# Contract of serena-pin.sh [root]   (default root: the repo root)
#   Reads under root: .mcp.json, .claude/settings.json, .github/workflows/*, scripts/ci/mcp.json,
#   scripts/ci/serena-tools.txt (five names, one per line), scripts/ci/role-heads/*.
#   L1 .mcp.json is byte-equal to scripts/ci/mcp.json (cmp).
#   L2 the owned copy holds the full lines `        "--agent-interface",` and `        "tools"`
#      adjacent, and the raw token `"serena": {` (the server key) exactly once (a bare "serena"
#      occurs twice: key and command, so the analysis text is corrected to the key form).
#   L3 the five names joined by `,` occur exactly once in .github/workflows/teamwright-agent.yml.
#   L4 the five names joined by `, ` occur exactly once in every role head holding mcp__serena.
#   L5 each of the two owned preflight jq lines occurs once in teamwright-agent.yml.
#   T1 `mcp__serena` occurs exactly 5 times in all of .github/workflows/.
#   T2 `mcp__serena` occurs exactly 5 times in each head that holds it.
#   T3 `mcp__serena` absent from .claude/settings.json.
#   T4 `allowedTools` exactly once in .github/workflows/.
#   T5 `allowed-tools`, `bypassPermissions`, `dangerously-skip-permissions`, `--settings` absent
#      from .github/workflows/.
#   Exit 0 "serena-pin: ok". Exit 1: one line per violation, starting with the rule id.
#   Exit 3: "serena-pin: cannot run: ..." (an owned file or a scanned path missing).
# role-tools.test.sh stays unchanged (#328).
#
# Usage: scripts/ci/serena-pin.test.sh
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
pin=scripts/ci/serena-pin.sh
base="${TMPDIR:-/tmp}"
tmp="$(mktemp -d "$base/serena-pin.XXXXXX")" || { echo "serena-pin.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

failed=0
passed=0
fail() {
  echo "FAIL $1" >&2
  [ -z "${2:-}" ] || sed 's/^/       | /' <<<"$2" >&2
  failed=$((failed + 1))
}
pass() { echo "ok   $1"; passed=$((passed + 1)); }

# fresh <case>: a copy of the files the pin reads (no target/, no caches) into $tmp/<case>.
fresh() {
  local d="$tmp/$1"
  mkdir -p "$d/.claude" "$d/.github" "$d/scripts/ci"
  cp .mcp.json "$d/"
  cp .claude/settings.json "$d/.claude/"
  cp -r .github/workflows "$d/.github/"
  cp scripts/ci/mcp.json scripts/ci/serena-tools.txt "$d/scripts/ci/"
  cp -r scripts/ci/role-heads "$d/scripts/ci/"
  echo "$d"
}
wf=.github/workflows/teamwright-agent.yml
five_comma="$(paste -sd, scripts/ci/serena-tools.txt 2>/dev/null)"
five_space="$(paste -sd, scripts/ci/serena-tools.txt 2>/dev/null | sed 's/,/, /g')"

# run <dir>: sets rc and out; the script is the real one, the root the copy.
run() { out="$(bash "$root/$pin" "$1" 2>&1)"; rc=$?; }
# expect <case> <dir> <rc> [rule id]
expect() {
  run "$2"
  if [ "$rc" != "$3" ]; then fail "$1: exit $rc, want $3" "$out"; return; fi
  if [ -n "${4:-}" ] && ! grep -q -- "$4" <<<"$out"; then fail "$1: output lacks rule $4" "$out"; return; fi
  pass "$1"
}
# edit <dir> <file> <old> <new>: replace every literal occurrence of old (must exist).
edit() {
  local f="$1/$2"
  grep -q -F -- "$3" "$f" || { fail "setup: '$3' not in $2"; return 1; }
  OLD="$3" NEW="$4" perl -0pi -e 's/\Q$ENV{OLD}\E/$ENV{NEW}/g' "$f"
}

# Real tree.
expect "real tree is clean" "$root" 0

# M1: server-wide token with a comma.
d="$(fresh m1)"; edit "$d" "$wf" "mcp__serena__get_symbols_overview" "mcp__serena,mcp__serena__get_symbols_overview" && expect "M1 allowlist mcp__serena," "$d" 1 "T1"
# M2: server-wide token closing the quote.
d="$(fresh m2)"; edit "$d" "$wf" "mcp__serena__find_declaration" "mcp__serena\"" && expect "M2 allowlist mcp__serena\"" "$d" 1 "L3"
# M3: wildcard.
d="$(fresh m3)"; edit "$d" "$wf" "mcp__serena__find_symbol," "mcp__serena__*," && expect "M3 allowlist mcp__serena__*" "$d" 1 "L3"
# M4a: flag removed from .mcp.json only.
d="$(fresh m4a)"; perl -0pi -e 's/,\n        "--agent-interface",\n        "tools"//' "$d/.mcp.json"
cmp -s "$d/.mcp.json" .mcp.json && fail "setup: M4a unchanged"
expect "M4a flag dropped from .mcp.json" "$d" 1 "L1"
# M4b: flag removed from both .mcp.json and the owned copy.
d="$(fresh m4b)"
for f in .mcp.json scripts/ci/mcp.json; do perl -0pi -e 's/,\n        "--agent-interface",\n        "tools"//' "$d/$f"; done
expect "M4b flag dropped from both" "$d" 1 "L2"
# M4c: a second "serena" key appended to both (last wins in jq).
d="$(fresh m4c)"
for f in .mcp.json scripts/ci/mcp.json; do perl -0pi -e 's/("context7": \{)/"serena": {"command": "serena", "args": ["start-mcp-server"]},\n    $1/' "$d/$f"; done
expect "M4c second serena key in both" "$d" 1 "L2"
# M5: a head drops find_declaration.
d="$(fresh m5)"; edit "$d" "scripts/ci/role-heads/developer-core.md" ", mcp__serena__find_declaration" "" && expect "M5 head drops find_declaration" "$d" 1 "L4"
# M6a: second --allowedTools.
d="$(fresh m6a)"; edit "$d" "$wf" "--strict-mcp-config" "--strict-mcp-config --allowedTools mcp__serena__x" && expect "M6a second --allowedTools" "$d" 1 "T4"
# M6b: bypass mode.
d="$(fresh m6b)"; edit "$d" "$wf" "--strict-mcp-config" "--strict-mcp-config --permission-mode bypassPermissions" && expect "M6b bypassPermissions" "$d" 1 "T5"
# M6c: another permission source, in a comment (raw, fail-closed).
d="$(fresh m6c)"; printf '# --dangerously-skip-permissions\n' >>"$d/$wf"; expect "M6c dangerously-skip-permissions" "$d" 1 "T5"
# M7: grant in committed settings.
d="$(fresh m7)"; edit "$d" ".claude/settings.json" '"Bash(scripts/tw-wip-push)",' '"Bash(scripts/tw-wip-push)", "mcp__serena__find_symbol",' && expect "M7 mcp__serena in settings.json" "$d" 1 "T3"
# M8: preflight jq line changed (both lines, one at a time).
d="$(fresh m8a)"; edit "$d" "$wf" '{serena, context7})}'"'"' .mcp.json' '{serena, context7, playwright})}'"'"' .mcp.json' && expect "M8a preflight serena line changed" "$d" 1 "L5"
d="$(fresh m8b)"; edit "$d" "$wf" '{context7})}'"'"' .mcp.json' '{context7, serena})}'"'"' .mcp.json' && expect "M8b preflight context7 line changed" "$d" 1 "L5"
# Extra: owned allowlist list changed, workflow left alone.
d="$(fresh m9)"; printf 'mcp__serena__extra\n' >>"$d/scripts/ci/serena-tools.txt"; expect "owned tool list gains a name" "$d" 1 "L3"

# Cannot run: owned files or scanned paths missing -> 3, never a pass.
for p in scripts/ci/serena-tools.txt scripts/ci/mcp.json .mcp.json .claude/settings.json .github/workflows scripts/ci/role-heads; do
  d="$(fresh miss)"; rm -rf "$d/$p"; expect "missing $p exits 3" "$d" 3 "cannot run"; rm -rf "$d"
done

echo "serena-pin.test: $passed passed, $failed failed"
[ "$failed" = 0 ] || exit 1
exit 0
