#!/usr/bin/env bash
# T-070 (decisions #93): send one installer to the owner's Telegram chat (Bot API sendDocument).
# The one send path: the main/v* step of .github/workflows/ci.yml calls it with the repository
# secrets, and the every-ref step "Telegram send failure branch" calls it with an obviously
# fake token, so the failure branch and the no-leak guarantee run on every wip run (P-016).
#
# Contract:
#   - The token and the chat id reach curl only as config on stdin (`printf` is a bash builtin,
#     so neither is in any process argv, a file or a URL on a command line); curl's own stderr
#     is discarded, so a config-parse message cannot echo the URL. No set -x.
#   - The caption and the chat id are sent as form strings (no `;type=`, `@`, `<` parsing);
#     only the document is a file part.
#   - No TELEGRAM_BOT_TOKEN -> one line, exit 0, curl not run.
#   - A token or chat id that cannot be put in a quoted curl config string (a `"`, `\` or a
#     control character) -> ::warning:: (value not printed), exit 0, curl not run.
#   - curl fails (network, HTTP error from Telegram) -> one ::warning:: line with curl's exit
#     code and Telegram's `description` (CR/LF stripped, or "none"), exit 0: a notification
#     failure never turns a delivery run red; the installer is in the run artifact.
#   - Success -> "installer sent to Telegram: <file name>", exit 0.
# Reads only its two arguments and the environment variables TELEGRAM_BOT_TOKEN and
# TELEGRAM_CHAT_ID. Host-tested by scripts/ci/telegram-send.test.sh (stub curl).
#
# Usage: TELEGRAM_BOT_TOKEN=... TELEGRAM_CHAT_ID=... scripts/ci/telegram-send.sh <installer> <caption>
# Exit 0: sent, not sent with a warning, or no token. Exit 2: wrong usage.
set -uo pipefail
if [ "$#" != 2 ]; then
  echo "usage: telegram-send.sh <installer> <caption>" >&2
  exit 2
fi
installer="$1"
caption="$2"

if [ -z "${TELEGRAM_BOT_TOKEN:-}" ]; then
  echo "no TELEGRAM_BOT_TOKEN: nothing is sent"
  exit 0
fi
chat_id="${TELEGRAM_CHAT_ID:-}"

# A value goes into a double-quoted curl config string: refuse what would need escaping or end
# the line, without printing the value.
unsafe() { [[ "$1" == *[\"\\[:cntrl:]]* ]]; }
if unsafe "$TELEGRAM_BOT_TOKEN"; then
  echo "::warning::installer not sent to Telegram: TELEGRAM_BOT_TOKEN contains a quote, backslash or control character"
  exit 0
fi
if [ -z "$chat_id" ] || unsafe "$chat_id"; then
  echo "::warning::installer not sent to Telegram: TELEGRAM_CHAT_ID is empty or contains a quote, backslash or control character"
  exit 0
fi

rc=0
response="$(printf 'url = "https://api.telegram.org/bot%s/sendDocument"\nform-string = "chat_id=%s"\n' \
  "$TELEGRAM_BOT_TOKEN" "$chat_id" \
  | curl -s --fail-with-body --max-time 300 -K - \
    -F "document=@${installer}" \
    --form-string "caption=${caption}" 2>/dev/null)" || rc=$?
if [ "$rc" != 0 ]; then
  description="$(printf '%s' "$response" | tr -d '\r\n' | sed -n 's/.*"description"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
  echo "::warning::installer not sent to Telegram: curl exit code $rc, API description: ${description:-none}"
  exit 0
fi
echo "installer sent to Telegram: $(basename "$installer")"
