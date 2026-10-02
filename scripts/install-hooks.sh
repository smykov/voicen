#!/usr/bin/env bash
# install-hooks.sh — enable the versioned git hooks in scripts/hooks.
# Uses core.hooksPath so hooks live in the repo, are reviewed like code and reach
# everyone through a normal pull (.git/hooks is not versioned).
#
#   bash scripts/install-hooks.sh            enable
#   bash scripts/install-hooks.sh --check    verify only (exit 1 if not enabled)
#   git config --unset core.hooksPath        disable
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"
HOOKS=scripts/hooks

if [ "${1:-}" = "--check" ]; then
  [ "$(git config --get core.hooksPath || true)" = "$HOOKS" ] && { echo "hooks: enabled"; exit 0; }
  echo "hooks: NOT enabled - run: bash scripts/install-hooks.sh" >&2
  exit 1
fi

chmod +x "$HOOKS"/pre-commit "$HOOKS"/commit-msg "$HOOKS"/pre-push "$HOOKS"/*.sh 2>/dev/null || true
git config core.hooksPath "$HOOKS"

# Local-only state must never be committed.
touch .gitignore
for pat in '.teamwright/sessions/' '.teamwright/logs/' '.teamwright/current-task' '.teamwright/cache/' '.teamwright/config.next.yml' 'TEAMWRIGHT_PAUSE' '*.local.json' '.env' '.env.*' '!.env.example'; do
  grep -qxF -- "$pat" .gitignore || printf '%s\n' "$pat" >> .gitignore
done

command -v gitleaks >/dev/null 2>&1 || echo "note: gitleaks not found - pre-commit uses the built-in regex fallback."
command -v python3  >/dev/null 2>&1 || echo "warning: python3 not found - gates and patch-guard will be skipped (fail-open)."

cat <<'EOF'
core.hooksPath = scripts/hooks
  pre-commit  secret scan (gitleaks or fallback), refuses .env / *.local.json, CLAUDE.md budget warning
  commit-msg  patch-guard, warn by default (override: `RCA: T-NNN` trailer to an rca task, or Principle-Override: P-NNN - why)
  pre-push    TEAMWRIGHT_PAUSE file blocks all pushes; DRAIN: task code leaves only in approved statuses
Claude Code gates (analysis, review, verify) and the journal are wired in .claude/settings.json.
EOF
