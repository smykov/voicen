#!/usr/bin/env bash
# T-061, T-066: CI reaches Windows Credential Manager only through scripts/ci/credentials.ps1,
# which uses the product's own Win32 calls (CredWriteW / CredReadW / CredEnumerateW /
# CredDeleteW, GENERIC) and prints every Win32 error. The T-061 step that planted with cmdkey
# and read its premise from cmdkey's filtered /list text failed with "could not plant" and
# could not say why (run 37444780779).
#
# Invariant: outside the helper, nothing CI runs names a Credential Manager entry point. The
# entry points are the OS's ways in, not a list of tools seen so far: any Win32 credential call
# (an identifier Cred<Upper>..., e.g. CredWriteW, CredEnumerateA, CredUIPromptForCredentialsW),
# the console tools cmdkey and vaultcmd, the keymgr.dll UI, the WinRT PasswordVault and the
# CredentialManager PowerShell module (its *-StoredCredential cmdlets). The Cred<Upper> match is
# case-sensitive; the other names match in any case.
#
# This check fails when:
#   1. a non-comment line of a *.yml / *.yaml file at the top of the workflows dir, or of any
#      regular file at the top of the CI scripts dir other than the helper and this tripwire's
#      two files (ci-credentials.sh, ci-credentials.test.sh), names such an entry point (a line
#      whose first non-blank character is `#` is a comment and passes; a trailing comment does
#      not: fail-closed). A step names the helper's functions (Add-VoicenCredential, ...);
#   2. the helper (default scripts/ci/credentials.ps1) is missing, or does not declare all four
#      calls CredWriteW, CredReadW, CredEnumerateW and CredDeleteW.
# Not caught: an entry point reached by a name built at run time, or from a file outside these
# two dirs (src-tauri/tests use the product's own code, not CI helpers). Whether the helper's
# calls work is not modelled here: it is shown by the wip/<ID> CI run before review (P-016).
#
# Usage: scripts/ci/ci-credentials.sh [workflows-dir [ci-dir [helper]]]
#        (defaults: .github/workflows, scripts/ci, scripts/ci/credentials.ps1)
# Exit 0: ok. Exit 1: a violation (listed). Exit 3: cannot run (a dir missing, grep error);
# never reported as a pass.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
workflows="${1:-.github/workflows}"
cidir="${2:-scripts/ci}"
helper="${3:-scripts/ci/credentials.ps1}"
cannot_run() { echo "ci-credentials: cannot run: $1" >&2; exit 3; }
[ -d "$workflows" ] && [ -r "$workflows" ] || cannot_run "workflows dir $workflows not found or not readable"
[ -d "$cidir" ] && [ -r "$cidir" ] || cannot_run "CI scripts dir $cidir not found or not readable"

found=""
shopt -s nullglob
files=("$workflows"/*.yml "$workflows"/*.yaml)
for f in "$cidir"/* "$cidir"/.[!.]*; do
  [ -f "$f" ] || continue
  [ "$f" -ef "$helper" ] && continue
  case "${f##*/}" in ci-credentials.sh | ci-credentials.test.sh) continue ;; esac
  files+=("$f")
done
shopt -u nullglob
if [ "${#files[@]}" -gt 0 ]; then
  # /dev/null makes grep name the file even when there is only one. 0 = hits, 1 = none, 2 = error.
  api="$(grep -nE '(^|[^A-Za-z0-9_])Cred[A-Z][A-Za-z0-9_]*' -- "${files[@]}" /dev/null)"
  [ $? -le 1 ] || cannot_run "grep failed on $workflows or $cidir"
  tools="$(grep -niE 'cmdkey|vaultcmd|keymgr|passwordvault|storedcredential|credentialmanager' -- "${files[@]}" /dev/null)"
  [ $? -le 1 ] || cannot_run "grep failed on $workflows or $cidir"
  while IFS= read -r hit; do
    [ -n "$hit" ] || continue
    text="${hit#*:*:}"
    [[ "$text" =~ ^[[:space:]]*# ]] && continue
    found="$found  $hit  (a Credential Manager entry point outside $helper: use its Add-/Test-/Remove-VoicenCredential, Get-VoicenCredentials)"$'\n'
  done < <(printf '%s\n%s\n' "$api" "$tools" | sort -t: -k1,1 -k2,2n -u)
fi

if [ ! -f "$helper" ]; then
  found="$found  $helper: missing (the one Credential Manager helper of the CI steps)"$'\n'
else
  for call in CredWriteW CredReadW CredEnumerateW CredDeleteW; do
    grep -qF -- "$call" "$helper"
    rc=$?
    [ "$rc" -le 1 ] || cannot_run "grep failed on $helper"
    [ "$rc" -eq 0 ] || found="$found  $helper: does not declare $call"$'\n'
  done
fi

if [ -n "$found" ]; then
  {
    echo "ci-credentials: FAIL: CI reaches Credential Manager only through scripts/ci/credentials.ps1"
    echo "(the product's CredWriteW/CredReadW/CredEnumerateW/CredDeleteW, GENERIC; every Win32 error"
    echo "printed), never through another entry point or another tool's text output (T-061, run 37444780779)."
    printf '%s' "$found"
  } >&2
  exit 1
fi
echo "ci-credentials: ok: no Credential Manager entry point outside $helper in $workflows or $cidir; $helper declares CredWriteW, CredReadW, CredEnumerateW, CredDeleteW"
