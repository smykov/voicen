#!/usr/bin/env bash
# T-063 guard: the REAL Makefile recipe `licenses-stale` (the only staleness check of the
# committed THIRD-PARTY-NOTICES.txt) must hold its contract on the fixture cases in
# licenses/fixtures/stale/<case>/, run as
#   make -s -o licenses-generate licenses-stale LICENSES_DIR=<copy>/fresh NOTICES=<copy>/committed/THIRD-PARTY-NOTICES.txt
# on a temp copy of each case (-o: the generation is not run; the case's fresh/ stands for it):
#   equal   -> exit 0;
#   stale   -> non-zero, stderr names the NOTICES file with "is stale" and shows the changed
#              line from the diff ("+License: Apache-2.0");
#   missing -> non-zero, stderr names the NOTICES file with "is not committed".
# Also, from make's database (make -pRrq, not the Makefile text): licenses-check has
# licenses-stale as a prerequisite and NOTICES defaults to THIRD-PARTY-NOTICES.txt.
# No case may start scripts/tw-run or docker: a fake docker on PATH (and TW_DOCKER) records
# any call, and make's output must not name tw-run or docker. Offline, host bash/make/diff.
# Every case dir must be listed below, so a case cannot be dropped silently.
#
# Exit 0: every case as expected. Exit 1: a case differs (listed): the guard is broken.
# Exit 3: cannot run (make, diff or mktemp missing); never reported as a license result.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
fx=licenses/fixtures/stale
for tool in make diff mktemp; do
  command -v "$tool" >/dev/null 2>&1 || { echo "licenses-stale-guard: cannot run: $tool not found" >&2; exit 3; }
done
tmp="$(mktemp -d)" || { echo "licenses-stale-guard: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

# A docker that must never be called: it records the call and fails.
mkdir -p "$tmp/bin"
printf '#!/bin/sh\necho "$0 $*" >> "%s/docker-called"\nexit 99\n' "$tmp" > "$tmp/bin/docker"
chmod +x "$tmp/bin/docker"

# Make as the gate would run it, but with no inherited flags from an outer make.
run_make() {
  env -u MAKEFLAGS -u MFLAGS -u MAKELEVEL -u MAKEOVERRIDES PATH="$tmp/bin:$PATH" TW_DOCKER="$tmp/bin/docker" \
    make -C "$root" "$@"
}

failures=""
fail() { failures="$failures"$'\n'"  - $1"; }
declare -A seen=()

# check <case> <expect: 0 | nonzero> <needle>...: every needle must appear in make's output.
check() {
  local case="$1" want="$2" out status needle copy
  shift 2
  seen["$case"]=1
  [ -d "$fx/$case" ] || { fail "$case: fixture dir $fx/$case missing"; return; }
  copy="$tmp/cases/$case"
  mkdir -p "$copy"
  cp -R "$fx/$case/." "$copy/"
  mkdir -p "$copy/committed"
  local notices="$copy/committed/THIRD-PARTY-NOTICES.txt"
  rm -f "$tmp/docker-called"
  out="$(run_make -s -o licenses-generate licenses-stale LICENSES_DIR="$copy/fresh" NOTICES="$notices" 2>&1)"
  status=$?
  # Failure messages carry the first lines of make's output only.
  local shown
  shown="$(printf '%s\n' "$out" | head -n 12)"
  case "$want" in
    0) [ "$status" -eq 0 ] || fail "$case: exit $status, want 0; output (head): $shown" ;;
    nonzero) [ "$status" -ne 0 ] || fail "$case: exit 0, want non-zero; output (head): $shown" ;;
  esac
  for needle in "$@"; do
    needle="${needle//@NOTICES@/$notices}"
    case "$out" in
      *"$needle"*) ;;
      *) fail "$case: output lacks \"$needle\"; output (head): $shown" ;;
    esac
  done
  if [ -e "$tmp/docker-called" ]; then
    fail "$case: docker was started: $(cat "$tmp/docker-called")"
  fi
  if printf '%s\n' "$out" | grep -Eq 'tw-run|docker'; then
    fail "$case: output names tw-run or docker (the generation ran); output (head): $shown"
  fi
  # The recipe compares, it never writes the committed file or the fresh one.
  if ! diff -r "$fx/$case/fresh" "$copy/fresh" >/dev/null; then
    fail "$case: the recipe changed fresh/"
  fi
  if [ -d "$fx/$case/committed" ]; then
    diff -r "$fx/$case/committed" "$copy/committed" >/dev/null || fail "$case: the recipe changed committed/"
  elif [ -e "$notices" ]; then
    fail "$case: the recipe created $notices"
  fi
}

check equal   0
check stale   nonzero "@NOTICES@ is stale" "+License: Apache-2.0" "-License: MIT"
check missing nonzero "@NOTICES@ is not committed"

for dir in "$fx"/*/; do
  name="$(basename "$dir")"
  [ -n "${seen[$name]:-}" ] || fail "$name: fixture dir not listed in $0"
done

# make's database: licenses-check -> licenses-stale, and the default NOTICES.
db="$(run_make -pRrq -f Makefile : 2>/dev/null)"
if [ -z "$db" ]; then
  fail "make -pRrq printed no database"
else
  prereqs="$(printf '%s\n' "$db" | awk '/^licenses-check:/ { sub(/^licenses-check:[^ ]*/, ""); print; exit }')"
  case " $prereqs " in
    *" licenses-stale "*) ;;
    *) fail "licenses-check does not have licenses-stale as a prerequisite (prerequisites:$prereqs)" ;;
  esac
  printf '%s\n' "$db" | grep -Eq '^NOTICES :?= THIRD-PARTY-NOTICES\.txt$' \
    || fail "NOTICES does not default to THIRD-PARTY-NOTICES.txt in the Makefile"
fi

if [ -n "$failures" ]; then
  echo "licenses-check: FAIL: the licenses-stale recipe does not hold its contract:$failures" >&2
  exit 1
fi
echo "licenses-check: stale guard ok: licenses-stale passes equal notices, fails stale and missing ones naming the file"
