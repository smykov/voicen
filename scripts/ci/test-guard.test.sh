#!/usr/bin/env bash
# T-189 (decisions #201; docs/tasks/T-189.md): host self-test of the kit-managed PreToolUse hook
# scripts/hooks/test-guard.sh, for the systemd-run scope prefix.
#
# The hook judges a Bash simple command by its first word after strip(). A command that starts
# with `systemd-run` (the host-load rule: `systemd-run --scope -q --slice=agent-tests.slice -- ...`)
# must be judged like the command it runs: a direct test or gate run behind it is refused with the
# same "Run instead" target as without the prefix; an allowed command stays allowed; an option the
# tables cannot read is judged by its suffixes (refusal-biased), never a pass for a test run.
#
# Contract (project hook contract, scripts/hooks/_hookio.py): the hook reads one PreToolUse JSON
# object on stdin; deny = one JSON object with permissionDecision=deny on stdout, exit 0; allow =
# no output, exit 0. Mode block comes from TW_TEST_GUARD, the area table from this repository's
# .teamwright/config.yml (CLAUDE_PROJECT_DIR). TEAMWRIGHT_LOG_DIR points into the temporary
# directory: a denial appends to gates.jsonl there, never to .teamwright/logs.
#
# Three groups: DENY (the prefixed forms), ALLOW (forms that must stay allowed), CONTROL (today's
# behaviour without the prefix: they prove the harness reads the hook right).
#
# Usage: scripts/ci/test-guard.test.sh   (host bash, python3)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
hook="$root/scripts/hooks/test-guard.sh"
for t in python3 bash; do
  command -v "$t" >/dev/null 2>&1 || { echo "test-guard.test: cannot run: $t missing" >&2; exit 3; }
done
for f in "$hook" "$root/scripts/hooks/_hookio.py" "$root/.teamwright/config.yml"; do
  [ -f "$f" ] || { echo "test-guard.test: cannot run: $f missing" >&2; exit 3; }
done
tmp="$(mktemp -d)" || { echo "test-guard.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/logs"

export CLAUDE_PROJECT_DIR="$root" TEAMWRIGHT_LOG_DIR="$tmp/logs" TW_TEST_GUARD=block
export PYTHONDONTWRITEBYTECODE=1
unset TEAMWRIGHT_GATE_LOG

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

P='systemd-run --scope -q --slice=agent-tests.slice --'

# run_hook <command>: sets $out (stdout) and $rc (exit code) for a Bash tool call of <command>.
run_hook() {
  local payload
  payload="$(CMD="$1" python3 -I -c '
import json, os
print(json.dumps({"hook_event_name": "PreToolUse", "tool_name": "Bash",
                  "tool_input": {"command": os.environ["CMD"]}, "cwd": os.getcwd()}))')" || return 3
  out="$(printf '%s' "$payload" | bash "$hook" 2>"$tmp/stderr")"
  rc=$?
}

# reason_of: the permissionDecisionReason of $out ("" when it is not a deny object).
reason_of() {
  printf '%s' "$out" | python3 -I -c '
import json, sys
try:
    d = json.loads(sys.stdin.read())
    h = d["hookSpecificOutput"]
    if h.get("permissionDecision") == "deny":
        print(h.get("permissionDecisionReason", ""))
except Exception:
    pass'
}

# deny <name> <command> [<run-instead target>]: the hook denies, and its reason offers the target
# (a substring of the indented "Run instead" / "Run without the pipe" line) when one is given.
deny() {
  local name="$1" cmd="$2" target="${3:-}" reason
  run_hook "$cmd" || { fail "$name" "harness could not build the payload"; return; }
  if [ "$rc" != 0 ]; then
    fail "$name" "exit $rc, want 0; stderr: $(cat "$tmp/stderr")"
    return
  fi
  reason="$(reason_of)"
  if [ -z "$reason" ]; then
    fail "$name" "ALLOWED, want DENY: $cmd"
    return
  fi
  if [ -n "$target" ] && ! grep -qF -- "  $target" <<<"$reason"; then
    fail "$name" "denied, but the reason does not name \"$target\": $reason"
    return
  fi
  pass "$name"
}

# allow <name> <command>: no output and exit 0.
allow() {
  local name="$1" cmd="$2"
  run_hook "$cmd" || { fail "$name" "harness could not build the payload"; return; }
  if [ "$rc" != 0 ]; then
    fail "$name" "exit $rc, want 0; stderr: $(cat "$tmp/stderr")"
    return
  fi
  if [ -n "$out" ]; then
    fail "$name" "DENIED or warned, want ALLOW: $cmd -> $out"
    return
  fi
  pass "$name"
}

# ---- CONTROL: today's behaviour without the prefix (green today) --------------------------------
deny  "control: cargo test"               'cargo test' 'scripts/tw-test core'
deny  "control: make check"               'make check' 'scripts/tw-gate --top'
deny  "control: pnpm test"                'pnpm test' 'scripts/tw-test ui'
allow "control: tw-test core foo"         'scripts/tw-test core foo'

# ---- DENY: the prefixed forms (red today) -------------------------------------------------------
deny "deny: prefix cargo test"            "$P cargo test" 'scripts/tw-test core'
deny "deny: prefix make check"            "$P make check" 'scripts/tw-gate --top'
deny "deny: prefix make -k check"         "$P make -k check" 'scripts/tw-gate --top'
deny "deny: prefix pnpm test"             "$P pnpm test" 'scripts/tw-test ui'
deny "deny: prefix tw-run core cargo test" "$P scripts/tw-run core -- cargo test -p voicen-core" 'scripts/tw-test core'
deny "deny: prefix tw-test piped to tail (cut)" "$P scripts/tw-test core foo 2>&1 | tail -5" 'scripts/tw-test core foo'
deny "deny: prefix cargo test -- --nocapture" 'systemd-run --scope -q -- cargo test -- --nocapture' 'scripts/tw-test core'
deny "deny: --user --scope without --"   'systemd-run --user --scope cargo test' 'scripts/tw-test core'
deny "deny: --slice value in next word, with --" 'systemd-run --scope --slice agent-tests.slice -- cargo test' 'scripts/tw-test core'
deny "deny: --slice value in next word, no --" 'systemd-run --scope --slice agent-tests.slice cargo test' 'scripts/tw-test core'
deny "deny: -p CPUQuota=50%"              'systemd-run --scope -p CPUQuota=50% -- cargo test' 'scripts/tw-test core'
deny "deny: --property=CPUQuota=50%"      'systemd-run --scope --property=CPUQuota=50% -- cargo test' 'scripts/tw-test core'
deny "deny: unknown option --frobnicate x" 'systemd-run --frobnicate x cargo test' 'scripts/tw-test core'
deny "deny: combined short flags -qG"     'systemd-run -qG cargo test' 'scripts/tw-test core'
deny "deny: nested nice -n 19"            'nice -n 19 systemd-run --scope -- cargo test' 'scripts/tw-test core'
deny "deny: nested sudo"                  'sudo systemd-run --scope -- cargo test' 'scripts/tw-test core'
deny "deny: absolute path /usr/bin/systemd-run" '/usr/bin/systemd-run --scope -- cargo test' 'scripts/tw-test core'
deny "deny: absolute path, cut of tw-test output" '/usr/bin/systemd-run --scope -q -- scripts/tw-test core foo | tail -5' 'scripts/tw-test core foo'
deny "deny: prefix bash -c 'make check'"  "systemd-run --scope -- bash -c 'make check'" 'scripts/tw-gate --top'

# ---- ALLOW: must stay allowed -------------------------------------------------------------------
allow "allow: prefix tw-test core foo"    "$P scripts/tw-test core foo"
allow "allow: prefix tw-gate --top"       "$P scripts/tw-gate --top"
allow "allow: prefix tw-run core cargo build" "$P scripts/tw-run core -- cargo build"
allow "allow: prefix find / -name whisper-rs-sys" "$P find / -type d -name whisper-rs-sys"
allow "allow: prefix echo cargo test"     "$P echo cargo test"
allow "allow: --user --scope, no command" 'systemd-run --user --scope'
allow "allow: git commit -m naming the prefixed form" "git commit -m \"$P cargo test\""

echo "test-guard.test: $passed ok, $failed failed"
[ "$failed" = 0 ] || exit 1
exit 0
