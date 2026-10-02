#!/usr/bin/env bash
# claude-md-budget.sh — warn when CLAUDE.md outgrows its size budget (P-014). Tier T2.
#
# WHY. CLAUDE.md is loaded into every session before any question is asked. A file
# that grew to hold every decision costs context in every session and buries the
# few rules that matter. CLAUDE.md is a map: what costs you before you would think
# to open a file. The rest belongs in docs/decisions/<area>.md.
#
# Budget: $TEAMWRIGHT_CLAUDE_MD_MAX_KB (default 40, i.e. 40 KB). Set it for every
# session in .claude/settings.json -> "env": {"TEAMWRIGHT_CLAUDE_MD_MAX_KB": "40"}.
#
# Modes (never blocks):
#   --session-start   Claude Code SessionStart hook: JSON additionalContext for the
#                     session when over budget, nothing otherwise.
#   (default)         git pre-commit: warning on stderr when the STAGED CLAUDE.md is
#                     over budget.
set -uo pipefail
MAX_KB="${TEAMWRIGHT_CLAUDE_MD_MAX_KB:-40}"
case "$MAX_KB" in ''|*[!0-9]*) MAX_KB=40 ;; esac
MAX=$((MAX_KB * 1024))

if [ "${1:-}" = "--session-start" ]; then
  cat >/dev/null 2>&1 || true   # hook input is not needed
  ROOT="${CLAUDE_PROJECT_DIR:-$(pwd)}"
  f="$ROOT/CLAUDE.md"
  [ -f "$f" ] || exit 0
  size=$(wc -c < "$f" | tr -d ' ')
  [ "$size" -gt "$MAX" ] || exit 0
  msg="CLAUDE.md is $((size / 1024)) KB, over its ${MAX_KB} KB budget (P-014). Keep it a map; move decisions to docs/decisions/<area>.md, verbatim, in a separate commit."
  printf '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"%s"}}\n' "$msg"
  exit 0
fi

ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || exit 0
cd "$ROOT" || exit 0
git diff --cached --name-only --diff-filter=ACMR | grep -qx 'CLAUDE.md' || exit 0
size=$(git cat-file -s ":CLAUDE.md" 2>/dev/null) || exit 0
if [ "$size" -gt "$MAX" ]; then
  echo "pre-commit: warning - CLAUDE.md is $((size / 1024)) KB, over its ${MAX_KB} KB budget (P-014)." >&2
  echo "  Keep it a map; move decisions to docs/decisions/<area>.md. Budget: TEAMWRIGHT_CLAUDE_MD_MAX_KB." >&2
fi
exit 0
