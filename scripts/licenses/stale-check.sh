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
# Also, from make's database (make -pRrq, not the Makefile text), through `--db-check`
# (below): NOTICES is `NOTICES := THIRD-PARTY-NOTICES.txt` even with NOTICES set in the
# environment, `check` has licenses-check as a prerequisite, and licenses-check has
# licenses-unit licenses-fixture licenses-stale-guard licenses-rust licenses-npm licenses-stale.
# The same check runs on every licenses/fixtures/stale-db/<case> (a `db_case` line), then
# scripts/licenses/stale-check.test.sh (T-181).
#
#   stale-check.sh --db-check <makefile>
# runs only that database check on <makefile> (MAKEFLAGS, MFLAGS, MAKELEVEL, MAKEOVERRIDES
# unset, the rest of the environment kept): exit 0 and "db-check ok", exit 1 naming NOTICES or
# the target and its missing prerequisite, exit 3 when make cannot run or the file is missing.
#
# No decision here reads a pipe into an early-exit reader (grep -q, awk exit, head): under
# pipefail the writer's SIGPIPE would decide the verdict (T-181, docs/failures.md). Readers take
# a here-string or a file.
# No case may start scripts/tw-run or docker: a fake docker on PATH (and TW_DOCKER) records
# any call, and make's output must not name tw-run or docker. Offline, host bash/make/diff.
# Every case dir must be listed below, so a case cannot be dropped silently.
#
# Exit 0: every case as expected. Exit 1: a case differs (listed): the guard is broken.
# Exit 3: cannot run (make, diff or mktemp missing); never reported as a license result.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3

# --db-check <makefile>: make's database of <makefile>, nothing else of the guard.
db_check() {
  local mk="$1" db line target p
  command -v make >/dev/null 2>&1 || { echo "db-check: cannot run: make not found" >&2; return 3; }
  [ -f "$mk" ] && [ -r "$mk" ] || { echo "db-check: cannot run: no readable makefile $mk" >&2; return 3; }
  # make -q exits 2 here (no rule for ':'); the printed database is what is decided on.
  db="$(env -u MAKEFLAGS -u MFLAGS -u MAKELEVEL -u MAKEOVERRIDES make -pRrq -f "$mk" : 2>/dev/null)"
  [ -n "$db" ] || { echo "db-check: cannot run: make -pRrq printed no database for $mk" >&2; return 3; }
  local bad=""
  # `:=` only: `=` and `?=` print `NOTICES = ...`, and `?=` yields to the environment.
  grep -qxF 'NOTICES := THIRD-PARTY-NOTICES.txt' <<<"$db" \
    || bad="$bad"$'\n'"  - NOTICES is not 'NOTICES := THIRD-PARTY-NOTICES.txt' in make's database of $mk (with NOTICES=${NOTICES-<unset>} in the environment)"
  check_prereqs() { # <target> <prerequisite>...
    target="$1"
    shift
    line="$(grep -m 1 -E "^$target::? |^$target::?\$" <<<"$db")"
    if [ -z "$line" ]; then
      bad="$bad"$'\n'"  - $target is not a target in make's database of $mk (missing prerequisites: $*)"
      return
    fi
    line="${line#"$target"}"
    line="${line#:}"
    line="${line#:}"
    line="${line%%|*}" # order-only prerequisites do not count
    for p in "$@"; do
      case " $line " in
        *" $p "*) ;;
        *) bad="$bad"$'\n'"  - $target does not have $p as a prerequisite in $mk (prerequisites:$line)" ;;
      esac
    done
  }
  check_prereqs check licenses-check
  check_prereqs licenses-check licenses-unit licenses-fixture licenses-stale-guard licenses-rust licenses-npm licenses-stale
  if [ -n "$bad" ]; then
    echo "db-check: FAIL:$bad" >&2
    return 1
  fi
  echo "db-check ok: $mk"
}

if [ "${1:-}" = "--db-check" ]; then
  [ "$#" -eq 2 ] || { echo "usage: $0 --db-check <makefile>" >&2; exit 3; }
  db_check "$2"
  exit $?
fi

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
  shown="$(head -n 12 <<<"$out")"
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
  if grep -Eq 'tw-run|docker' <<<"$out"; then
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

# make's database (--db-check): the real Makefile, with a NOTICES sentinel in the environment
# (`:=` is env-immune; `?=` would print the sentinel), then every stale-db fixture.
sentinel=/nonexistent/sentinel/NOTICES.txt
dbout="$(NOTICES="$sentinel" bash "$0" --db-check Makefile 2>&1)"
status=$?
[ "$status" -eq 0 ] || fail "the Makefile's database (NOTICES=$sentinel in the environment), exit $status: $dbout"

fxdb=licenses/fixtures/stale-db
declare -A db_seen=()
# db_case <case> <expect: 0 | nonzero>: --db-check on licenses/fixtures/stale-db/<case>/Makefile;
# nonzero means a refusal (exit 1), not a cannot-run (exit 3).
db_case() {
  local case="$1" want="$2" st o
  db_seen["$case"]=1
  [ -f "$fxdb/$case/Makefile" ] || { fail "stale-db $case: $fxdb/$case/Makefile missing"; return; }
  o="$(bash "$0" --db-check "$fxdb/$case/Makefile" 2>&1)"
  st=$?
  case "$want" in
    0) [ "$st" -eq 0 ] || fail "stale-db $case: exit $st, want 0: $o" ;;
    nonzero) [ "$st" -eq 1 ] || fail "stale-db $case: exit $st, want a refusal (1): $o" ;;
  esac
}
db_case ok 0
db_case lazy nonzero
db_case recursive nonzero
db_case none nonzero

for dir in "$fxdb"/*/; do
  name="$(basename "$dir")"
  [ -n "${db_seen[$name]:-}" ] || fail "stale-db $name: fixture dir not listed in $0"
done

# The tests of --db-check itself (they call --db-check, which never calls them back).
testout="$(bash scripts/licenses/stale-check.test.sh 2>&1)"
status=$?
case "$status" in
  0) ;;
  3) echo "licenses-stale-guard: cannot run: $testout" >&2; exit 3 ;;
  *) fail "scripts/licenses/stale-check.test.sh exit $status: $testout" ;;
esac

if [ -n "$failures" ]; then
  echo "licenses-check: FAIL: the licenses-stale recipe does not hold its contract:$failures" >&2
  exit 1
fi
echo "licenses-check: stale guard ok: licenses-stale passes equal notices, fails stale and missing ones naming the file; make's database holds NOTICES := and the check -> licenses-check prerequisites"
