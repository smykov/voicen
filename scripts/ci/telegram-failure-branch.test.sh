#!/usr/bin/env bash
# T-070 (review 2 #1): host test of scripts/ci/telegram-failure-branch.sh, the verdict of the
# CI failure-branch step, on canned outputs of scripts/ci/telegram-send.sh (offline). Cases:
#   refused       exit 0 + warning with Telegram's description -> exit 0, the "failure branch:" line;
#   unreachable   exit 0 + warning "API description: none" (curl 6) -> exit 0, a ::warning:: that
#                 Telegram was not reached, no ::error::;
#   timeout       same with curl 28;
#   other-warning exit 0 + another ::warning:: (not the curl form) -> exit 0, not-reached warning;
#   nonzero       send exit 1 (with a refused warning) -> exit 1, ::error:: naming the exit code;
#   no-warning    exit 0, "installer sent to Telegram: ..." only -> exit 1;
#   empty         exit 0, no output -> exit 1;
#   token         the full fake token in the output -> exit 1;
#   token-part    the token's part after the colon in the output -> exit 1;
#   token-down    the token's part in a "description: none" output -> exit 1 (a leak is red even
#                 when Telegram is not reached);
#   usage         one argument, or a non-numeric exit code -> exit 2.
# Usage: scripts/ci/telegram-failure-branch.test.sh   (host bash)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
script="$root/scripts/ci/telegram-failure-branch.sh"
[ -x "$script" ] || { echo "telegram-failure-branch.test: cannot run: $script not found or not executable" >&2; exit 3; }

fake='0:voicen-ci-example-fake-not-a-secret'
warn='::warning::installer not sent to Telegram: curl exit code'
failed=0
passed=0
fail() { echo "FAIL $label: $1" >&2; sed 's/^/       | /' <<<"$out" >&2; failed=$((failed + 1)); ok=0; }

# call <label> <input> args...: runs the checker with <input> on stdin, sets out/rc.
call() {
  label="$1"; ok=1
  local input="$2"
  shift 2
  out="$(printf '%s' "$input" | "$script" "$@" 2>&1)"
  rc=$?
}
want_rc() { [ "$rc" = "$1" ] || fail "exit $rc, want $1"; }
has() { grep -qE -- "$1" <<<"$out" || fail "output lacks /$1/"; }
lacks() { ! grep -qE -- "$1" <<<"$out" || fail "output has /$1/"; }
done_case() { [ "$ok" = 1 ] && { echo "ok   $label"; passed=$((passed + 1)); }; }

call refused "$warn 22, API description: Unauthorized: invalid token specified" 0 "$fake"
want_rc 0; has '^failure branch: exit 0'; lacks '^::(warning|error)::'; done_case

call unreachable "$warn 6, API description: none" 0 "$fake"
want_rc 0; has '^::warning::Telegram failure branch: Telegram was not reached'; lacks '^::error::'; lacks '^failure branch:'; done_case

call timeout "$warn 28, API description: none" 0 "$fake"
want_rc 0; has '^::warning::Telegram failure branch: Telegram was not reached'; lacks '^::error::'; done_case

call other-warning "::warning::installer not sent to Telegram: TELEGRAM_CHAT_ID is empty or contains a quote, backslash or control character" 0 "$fake"
want_rc 0; has '^::warning::Telegram failure branch: Telegram was not reached'; lacks '^::error::'; done_case

call nonzero "$warn 22, API description: Unauthorized" 1 "$fake"
want_rc 1; has '^::error::telegram-send.sh exited 1'; lacks '^failure branch:'; done_case

call no-warning "installer sent to Telegram: Voicen_0.1.0_x64-setup.exe" 0 "$fake"
want_rc 1; has '^::error::telegram-send.sh printed no ::warning::'; done_case

call empty "" 0 "$fake"
want_rc 1; has '^::error::telegram-send.sh printed no ::warning::'; done_case

call token "$warn 22, API description: Unauthorized"$'\n'"url https://api.telegram.org/bot$fake/sendDocument" 0 "$fake"
want_rc 1; has '^::error::telegram-send.sh printed the \(fake\) token'; done_case

call token-part "$warn 22, API description: bad voicen-ci-example-fake-not-a-secret" 0 "$fake"
want_rc 1; has '^::error::telegram-send.sh printed the \(fake\) token'; done_case

call token-down "$warn 6, API description: none"$'\n'"voicen-ci-example-fake-not-a-secret" 0 "$fake"
want_rc 1; has '^::error::telegram-send.sh printed the \(fake\) token'; lacks 'Telegram was not reached'; done_case

call usage "" 0
want_rc 2; done_case
call usage-rc "" x "$fake"
want_rc 2; done_case

if [ "$failed" != 0 ]; then
  echo "telegram-failure-branch.test: $failed case(s) failed, $passed ok" >&2
  exit 1
fi
echo "telegram-failure-branch.test: ok: $passed cases as expected"
