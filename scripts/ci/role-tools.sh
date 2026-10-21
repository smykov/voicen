#!/usr/bin/env bash
# T-082 tripwire (decisions #145, class context-tool-coverage): every subagent role whose rendered
# teamwright Tools block tells it to call the context tool (Serena) and the docs tool (context7)
# can reach them. Claude Code gives a subagent with a `tools:` front-matter line only the tools
# named there, so the kit's allowlist (Read, Grep, Glob, Bash, Edit[, Write]) sent every role down
# its "not connected" fallback, silently (T-082 Investigation). Only the read/navigation tools of
# those providers may be named: Serena's edit, memory and shell tools would bypass the teamwright
# Edit/Write gates.
#
# For every *.md directly in agents-dir that contains the line <!-- teamwright:tools:begin -->
# (a role file without it is ignored):
#   R1 its front matter (between the first two --- lines) has a `tools:` line, a comma-separated
#      list of tool names (blanks around names ignored); a `tools:` line in the body does not count;
#   R2 every manifest entry is one of those names (exact match);
#   R3 every name starting with mcp__ is a manifest entry (no other MCP tool, wildcard or bare
#      server name);
#   R4 inside the Tools block one line holds "Serena not connected" and "use grep and file reads",
#      and one line holds "Not connected", "mcp__context7__" and "same fallback" (the role falls
#      back to built-in reads, it does not stop: T-082 Acceptance 3).
# Manifest: one exact tool name per line; # lines and blank lines are not entries.
# Not caught: whether Claude Code actually offers the named tools to the subagent (the T-082
# Acceptance run); a code-reading role without a Tools block (technical-writer, see T-082).
#
# Usage: scripts/ci/role-tools.sh [agents-dir [manifest]]
#   (defaults .claude/agents, scripts/ci/role-tools.txt)
# Exit 0: "role-tools: ok: <n> role file(s) with a Tools block". Exit 1: one "<file>: ..." line per
# violation. Exit 3: cannot run (agents dir missing, manifest missing or without entries,
# unreadable file); never a pass.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
dir="${1:-.claude/agents}"
dir="${dir%/}"
manifest="${2:-scripts/ci/role-tools.txt}"
cannot_run() { echo "role-tools: cannot run: $1" >&2; exit 3; }
[ -d "$dir" ] && [ -r "$dir" ] || cannot_run "agents dir $dir not found or not readable"
[ -f "$manifest" ] && [ -r "$manifest" ] || cannot_run "manifest $manifest not found or not readable"

mapfile -t want < <(sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' "$manifest" | grep -vE '^(#|$)')
[ "${#want[@]}" -gt 0 ] || cannot_run "manifest $manifest has no entries"
want_csv="$(IFS=,; printf '%s' "${want[*]}")"

mapfile -d '' all < <(find "$dir" -mindepth 1 -maxdepth 1 -type f -name '*.md' -print0 | LC_ALL=C sort -z)
begin='<!-- teamwright:tools:begin -->'
files=()
for f in "${all[@]}"; do
  [ -r "$f" ] || cannot_run "$f is not readable"
  grep -qxF -- "$begin" "$f" && files+=("$f")
done

report=""
for f in "${files[@]}"; do
  out="$(LC_ALL=C awk -v want="$want_csv" -v f="$f" '
    function trim(s) { sub(/^[ \t]+/, "", s); sub(/[ \t\r]+$/, "", s); return s }
    BEGIN { nw = split(want, w, ","); for (i = 1; i <= nw; i++) inman[w[i]] = 1 }
    {
      line = $0; sub(/\r$/, "", line)
      if (dashes < 2 && line == "---") { dashes++; next }
      if (dashes == 1 && line ~ /^tools:/ && !seen) {
        seen = 1
        v = substr(line, 7)
        n = split(v, parts, ",")
        for (i = 1; i <= n; i++) { t = trim(parts[i]); if (t != "") have[t] = 1; names[i] = t }
        nn = n
      }
      if (line == "<!-- teamwright:tools:begin -->") { inblock = 1; next }
      if (line == "<!-- teamwright:tools:end -->") { inblock = 0; next }
      if (inblock) {
        if (index(line, "Serena not connected") && index(line, "use grep and file reads")) serena = 1
        if (index(line, "Not connected") && index(line, "mcp__context7__") && index(line, "same fallback")) c7 = 1
      }
    }
    END {
      if (!seen) {
        print f ": R1: no tools: line in the front matter (the role inherits every tool, Serena edit and shell tools included)"
      } else {
        for (i = 1; i <= nw; i++)
          if (!(w[i] in have)) print f ": R2: tools: lacks " w[i]
        for (i = 1; i <= nn; i++)
          if (names[i] ~ /^mcp__/ && !(names[i] in inman))
            print f ": R3: tools: names " names[i] ", which is not in the manifest (only read-only context/docs tools)"
      }
      if (!serena) print f ": R4: Tools block has no \"Serena not connected\" line that says \"use grep and file reads\" (not connected = fall back, not stop)"
      if (!c7) print f ": R4: Tools block has no context7 \"Not connected\" line that says \"same fallback\" (not connected = fall back, not stop)"
    }
  ' "$f")" || cannot_run "awk failed on $f"
  [ -z "$out" ] || report+="$out"$'\n'
done

if [ -n "$report" ]; then
  printf '%s' "$report"
  n="$(printf '%s' "$report" | wc -l)"
  echo "role-tools: FAIL: $n violation(s); role allowlists must name $manifest (T-082, decisions #145)" >&2
  exit 1
fi
echo "role-tools: ok: ${#files[@]} role file(s) with a Tools block"
