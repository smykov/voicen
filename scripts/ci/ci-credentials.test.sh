#!/usr/bin/env bash
# T-061 guard of the guard: scripts/ci/ci-credentials.sh must give its documented exit code on
# each case below (built in a temp dir, so no committed fixture tree) and on the real repo.
#   ok-*  exit 0 and "ok:";  v-*  exit 1 and the listing names the file;  c-*  exit 3, "cannot run".
# Usage: scripts/ci/ci-credentials.test.sh
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
guard=scripts/ci/ci-credentials.sh
[ -x "$guard" ] || { echo "ci-credentials.test: cannot run: $guard not found or not executable" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "ci-credentials.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

failed=0
passed=0
good_helper='Add-Type @"
  CredWriteW CredReadW CredEnumerateW CredDeleteW
"@'
# mk <case> <workflow text> [helper text] [extra ps1 text]: <case>/workflows/ci.yml, <case>/ps1/credentials.ps1.
mk() {
  mkdir -p "$tmp/$1/workflows" "$tmp/$1/ps1"
  printf '%s\n' "$2" >"$tmp/$1/workflows/ci.yml"
  [ "${3-x}" = "-" ] || printf '%s\n' "${3:-$good_helper}" >"$tmp/$1/ps1/credentials.ps1"
  [ -z "${4:-}" ] || printf '%s\n' "$4" >"$tmp/$1/ps1/other.ps1"
}
# check <case> <want exit> <needle> [workflows dir]
check() {
  local c="$1" want="$2" needle="$3" wf="${4:-$tmp/$1/workflows}" out got
  out="$("$guard" "$wf" "$tmp/$c/ps1" "$tmp/$c/ps1/credentials.ps1" 2>&1)"
  got=$?
  if [ "$got" -ne "$want" ] || ! grep -qF -- "$needle" <<<"$out"; then
    echo "FAIL $c: exit $got (want $want), needle '$needle'" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
  else
    echo "ok   $c: exit $got"
    passed=$((passed + 1))
  fi
}

mk ok-comment-only '      run: |
          # never cmdkey here
          . ./scripts/ci/credentials.ps1'
check ok-comment-only 0 "ok:"
mk v-cmdkey-step '      run: |
          cmdkey "/generic:Voicen/a" /user:voicen "/pass:x" | Out-Null'
check v-cmdkey-step 1 "workflows/ci.yml:2:"
mk v-cmdkey-upper-trailing '      run: |
          & CMDKEY.EXE /list   # list'
check v-cmdkey-upper-trailing 1 "workflows/ci.yml:2:"
mk v-cmdkey-in-ps1 '      run: echo ok' '' 'function T { cmdkey /list }'
check v-cmdkey-in-ps1 1 "ps1/other.ps1:1:"
mk v-helper-missing '      run: echo ok' -
check v-helper-missing 1 "credentials.ps1: missing"
mk v-helper-no-enumerate '      run: echo ok' 'CredWriteW CredReadW CredDeleteW'
check v-helper-no-enumerate 1 "does not declare CredEnumerateW"
mk c-no-workflows '      run: echo ok'
check c-no-workflows 3 "cannot run" "$tmp/c-no-workflows/absent"

# The real repo with the guard's defaults.
out="$("$guard" 2>&1)"
got=$?
if [ "$got" -eq 0 ] && grep -qF "ok:" <<<"$out"; then
  echo "ok   real repo: exit 0"
  passed=$((passed + 1))
else
  echo "FAIL real repo: exit $got (want 0)" >&2
  sed 's/^/       | /' <<<"$out" >&2
  failed=$((failed + 1))
fi

echo "ci-credentials.test: $passed passed, $failed failed"
[ "$failed" -eq 0 ] || exit 1
