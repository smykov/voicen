#!/usr/bin/env bash
# T-070 (review 2 #1): the verdict of the CI step "Telegram send failure branch (fake token,
# nothing reaches a chat)", which runs scripts/ci/telegram-send.sh with an obviously fake token
# off main and v* tags. The send script's contract is red; Telegram being unreachable is not.
#
# Red (exit 1, one ::error:: line per reason) only when the send script broke its contract:
#   - it exited non-zero;
#   - its output has no ::warning:: line at all;
#   - its output contains the fake token or the token's part after the colon.
# Otherwise green (exit 0):
#   - a "::warning::installer not sent to Telegram: curl exit code N, API description: <text>"
#     line with a non-zero N and a description other than "none": Telegram was reached and
#     refused the token -> "failure branch: ..." line;
#   - any other ::warning:: (description "none": DNS, connect, TLS, timeout, an HTML or empty
#     error body): Telegram was not reached -> a ::warning:: that the failure branch could not
#     reach Telegram, still exit 0, so a Telegram outage never turns a run red.
#
# Usage: scripts/ci/telegram-failure-branch.sh <send exit code> <fake token> < <send output>
# Exit 0: contract held. Exit 1: contract broken (reasons listed). Exit 2: wrong usage.
# Host-tested by scripts/ci/telegram-failure-branch.test.sh.
set -uo pipefail
if [ "$#" != 2 ] || ! [[ "$1" =~ ^[0-9]+$ ]] || [ -z "$2" ]; then
  echo "usage: telegram-failure-branch.sh <send exit code> <fake token> < <send output>" >&2
  exit 2
fi
rc="$1"
fake="$2"
out="$(cat)"

bad=0
if [ "$rc" != 0 ]; then
  echo "::error::telegram-send.sh exited $rc on a refused send, expected 0"
  bad=1
fi
if ! grep -q '^::warning::' <<<"$out"; then
  echo "::error::telegram-send.sh printed no ::warning:: line for a send with a fake token"
  bad=1
fi
if grep -qF -e "$fake" <<<"$out" || { [[ "$fake" == *:* ]] && [ -n "${fake#*:}" ] && grep -qF -e "${fake#*:}" <<<"$out"; }; then
  echo "::error::telegram-send.sh printed the (fake) token"
  bad=1
fi
[ "$bad" = 0 ] || exit 1

refused='^::warning::installer not sent to Telegram: curl exit code [1-9][0-9]*, API description: .+$'
if grep -E "$refused" <<<"$out" | grep -qvE 'API description: none$'; then
  echo "failure branch: exit 0, warning with Telegram's description, the fake token not printed"
else
  echo "::warning::Telegram failure branch: Telegram was not reached (no API description), so the refusal path was not exercised; exit 0 and no token printed still hold"
fi
exit 0
