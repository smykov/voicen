#!/usr/bin/env bash
# T-070: host test of scripts/ci/telegram-send.sh with a stub curl first on PATH (offline). The
# stub records its argv (one argument per line) and its stdin, prints a canned body and exits
# with a canned code. Cases:
#   no-token      exit 0, one line "no TELEGRAM_BOT_TOKEN: nothing is sent", curl not run;
#   refused       curl exits 22 with a Telegram error body: exit 0, exactly one line, the
#                 ::warning:: with "curl exit code 22" and Telegram's description; the token and
#                 the chat id are in no curl argument and nowhere in the output, and both reach
#                 curl on stdin (url line, form-string chat_id); the caption is a --form-string,
#                 the installer the only -F part;
#   unreachable   curl exits 6 with no body: the warning says "API description: none", exit 0;
#   sent          curl exits 0: "installer sent to Telegram: <file name>", exit 0;
#   unsafe-token  a token with a quote: ::warning:: without the value, exit 0, curl not run;
#   unsafe-chat   a chat id with a newline: same;
#   usage         one argument: exit 2.
# Usage: scripts/ci/telegram-send.test.sh   (host bash)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
script="$root/scripts/ci/telegram-send.sh"
[ -x "$script" ] || { echo "telegram-send.test: cannot run: $script not found or not executable" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "telegram-send.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

mkdir -p "$tmp/bin"
cat > "$tmp/bin/curl" <<'EOF'
#!/usr/bin/env bash
: > "$STUB_DIR/argv"
for a in "$@"; do printf '%s\n' "$a" >> "$STUB_DIR/argv"; done
cat > "$STUB_DIR/stdin"
printf '%s' "${STUB_BODY:-}"
exit "${STUB_RC:-0}"
EOF
chmod +x "$tmp/bin/curl"
installer="$tmp/Voicen_0.1.0_x64-setup.exe"
printf 'MZ' > "$installer"
caption='Voicen 0.1.0, v0.1.0;type=text/html @ abc1234, run https://example.com/r/1'
token='123456:example-fake-not-a-secret'
chat='-1009876543210'

failed=0
passed=0
fail() { echo "FAIL $label: $1" >&2; sed 's/^/       | /' <<<"$out" >&2; failed=$((failed + 1)); ok=0; }

# call <label> <stub rc> <stub body> <token> <chat> [args...]: runs the script, sets out/rc.
call() {
  label="$1"; ok=1
  local stub_rc="$2" body="$3" tok="$4" cid="$5"
  shift 5
  rm -f "$tmp/argv" "$tmp/stdin"
  out="$(PATH="$tmp/bin:$PATH" STUB_DIR="$tmp" STUB_RC="$stub_rc" STUB_BODY="$body" \
    TELEGRAM_BOT_TOKEN="$tok" TELEGRAM_CHAT_ID="$cid" "$script" "$@" 2>&1)"
  rc=$?
}
want_rc() { [ "$rc" = "$1" ] || fail "exit $rc, want $1"; }
want_line() { grep -qxF -- "$1" <<<"$out" || fail "output lacks the line: $1"; }
want_lines() { [ "$(grep -c '' <<<"$out")" = "$1" ] || fail "$(grep -c '' <<<"$out") output lines, want $1"; }
no_curl() { [ ! -e "$tmp/argv" ] || fail "curl was run"; }
absent() { ! grep -qF -- "$1" <<<"$out" || fail "the output contains $2"; }
done_case() { [ "$ok" = 1 ] && { echo "ok   $label"; passed=$((passed + 1)); }; }

call no-token 0 '' '' "$chat" "$installer" "$caption"
want_rc 0; want_lines 1; want_line 'no TELEGRAM_BOT_TOKEN: nothing is sent'; no_curl; done_case

call refused 22 '{"ok":false,"error_code":400,"description":"Bad Request: chat not found"}' "$token" "$chat" "$installer" "$caption"
want_rc 0; want_lines 1
want_line '::warning::installer not sent to Telegram: curl exit code 22, API description: Bad Request: chat not found'
absent "$token" "the token"; absent "example-fake-not-a-secret" "the token's secret part"; absent "$chat" "the chat id"
if [ ! -e "$tmp/argv" ]; then fail "curl was not run"; else
  ! grep -qF -- "$token" "$tmp/argv" || fail "the token is in curl's argv"
  ! grep -qF -- "$chat" "$tmp/argv" || fail "the chat id is in curl's argv"
  grep -qxF -- "url = \"https://api.telegram.org/bot$token/sendDocument\"" "$tmp/stdin" || fail "no url line with the token on curl's stdin"
  grep -qxF -- "form-string = \"chat_id=$chat\"" "$tmp/stdin" || fail "no form-string chat_id line on curl's stdin"
  argv="$(tr '\n' ' ' < "$tmp/argv")"
  [[ "$argv" == *"-K - "* ]] || fail "curl does not read its config from stdin (-K -): $argv"
  [[ "$argv" == *"--fail-with-body"* ]] || fail "curl runs without --fail-with-body: $argv"
  [[ "$argv" == *"--form-string caption=$caption "* ]] || fail "the caption is not a --form-string: $argv"
  [ "$(grep -cxF -- '-F' "$tmp/argv")" = 1 ] || fail "not exactly one -F part: $argv"
  [[ "$argv" == *"-F document=@$installer "* ]] || fail "the -F part is not document=@<installer>: $argv"
fi
done_case

call unreachable 6 '' "$token" "$chat" "$installer" "$caption"
want_rc 0; want_lines 1
want_line '::warning::installer not sent to Telegram: curl exit code 6, API description: none'
done_case

call sent 0 '{"ok":true,"result":{}}' "$token" "$chat" "$installer" "$caption"
want_rc 0; want_lines 1; want_line 'installer sent to Telegram: Voicen_0.1.0_x64-setup.exe'; done_case

call unsafe-token 0 '' '12:ab"cd-example' "$chat" "$installer" "$caption"
want_rc 0; want_lines 1; no_curl; absent 'ab"cd' "the token"
grep -q '^::warning::installer not sent to Telegram: TELEGRAM_BOT_TOKEN' <<<"$out" || fail "no ::warning:: naming TELEGRAM_BOT_TOKEN"
done_case

call unsafe-chat 0 '' "$token" $'-100\nurl = "http://example.com/"' "$installer" "$caption"
want_rc 0; want_lines 1; no_curl; absent 'example.com' "the chat id"
grep -q '^::warning::installer not sent to Telegram: TELEGRAM_CHAT_ID' <<<"$out" || fail "no ::warning:: naming TELEGRAM_CHAT_ID"
done_case

call usage 0 '' "$token" "$chat" "$installer"
want_rc 2; no_curl; done_case

if [ "$failed" != 0 ]; then
  echo "telegram-send.test: $failed case(s) failed, $passed ok" >&2
  exit 1
fi
echo "telegram-send.test: ok: $passed cases as expected"
