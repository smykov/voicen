#!/usr/bin/env bash
# T-061 (re-analysis after verify 3, run 37444780779): CI steps plant and observe Windows
# Credential Manager entries only through scripts/ci/credentials.ps1, which uses the product's
# own Win32 calls (CredWriteW / CredReadW / CredEnumerateW / CredDeleteW, GENERIC) and prints
# every Win32 error. The step that planted with cmdkey and read its premise from cmdkey's
# filtered /list text failed with "could not plant" and could not say why.
#
# This check fails when:
#   1. a non-comment line of a *.yml / *.yaml file at the top of the workflows dir, or of a
#      scripts/ci/*.ps1 helper, names cmdkey (any case; a line whose first non-blank character
#      is `#` is a comment and passes, a trailing comment does not: fail-closed);
#   2. the helper (default scripts/ci/credentials.ps1) is missing, or does not declare all four
#      calls CredWriteW, CredReadW, CredEnumerateW and CredDeleteW.
# Not caught: a tool reached through another script or an alias; cmdkey in a run step is the
# shape this repo had.
#
# Usage: scripts/ci/ci-credentials.sh [workflows-dir [ps1-dir [helper]]]
#        (defaults: .github/workflows, scripts/ci, scripts/ci/credentials.ps1)
# Exit 0: ok. Exit 1: a violation (listed). Exit 3: cannot run (a dir missing, grep error);
# never reported as a pass.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
workflows="${1:-.github/workflows}"
ps1dir="${2:-scripts/ci}"
helper="${3:-scripts/ci/credentials.ps1}"
cannot_run() { echo "ci-credentials: cannot run: $1" >&2; exit 3; }
[ -d "$workflows" ] && [ -r "$workflows" ] || cannot_run "workflows dir $workflows not found or not readable"
[ -d "$ps1dir" ] && [ -r "$ps1dir" ] || cannot_run "helper dir $ps1dir not found or not readable"

found=""
shopt -s nullglob
files=("$workflows"/*.yml "$workflows"/*.yaml "$ps1dir"/*.ps1)
shopt -u nullglob
if [ "${#files[@]}" -gt 0 ]; then
  # /dev/null makes grep name the file even when there is only one. 0 = hits, 1 = none, 2 = error.
  hits="$(grep -niE 'cmdkey' -- "${files[@]}" /dev/null)"
  [ $? -le 1 ] || cannot_run "grep failed on $workflows or $ps1dir"
  while IFS= read -r hit; do
    [ -n "$hit" ] || continue
    text="${hit#*:*:}"
    [[ "$text" =~ ^[[:space:]]*# ]] && continue
    found="$found  $hit  (cmdkey: plant, check and remove Credential Manager entries with scripts/ci/credentials.ps1)"$'\n'
  done <<<"$hits"
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
    echo "ci-credentials: FAIL: CI steps plant and observe Credential Manager entries only through"
    echo "scripts/ci/credentials.ps1 (the product's CredWriteW/CredReadW/CredEnumerateW/CredDeleteW, GENERIC;"
    echo "every Win32 error printed), never through another tool's text output (T-061, run 37444780779)."
    printf '%s' "$found"
  } >&2
  exit 1
fi
echo "ci-credentials: ok: no cmdkey in $workflows or $ps1dir/*.ps1; $helper declares CredWriteW, CredReadW, CredEnumerateW, CredDeleteW"
