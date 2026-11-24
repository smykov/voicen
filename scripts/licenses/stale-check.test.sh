#!/usr/bin/env bash
# T-181 tests of the make-database checks of scripts/licenses/stale-check.sh (T-063 guard).
# Host bash and make (the ui image has no make, so this is not a node test). Run by
# stale-check.sh at the end of its own run, so `make licenses-stale-guard` (in `make check`)
# covers it and the Makefile does not change; also runnable alone:
#   bash scripts/licenses/stale-check.test.sh
#
# Interface the developer implements (named here so the tests and the guard agree):
#
#   scripts/licenses/stale-check.sh --db-check <makefile>
#
# runs `make -pRrq -f <makefile> :` (MAKEFLAGS, MFLAGS, MAKELEVEL, MAKEOVERRIDES unset, the rest
# of the caller's environment kept, so a test can put a NOTICES sentinel there) and decides on
# make's printed database only:
#   - the exact line `NOTICES := THIRD-PARTY-NOTICES.txt` (`=` and `?=` both print `NOTICES = ...`
#     and are refused);
#   - `check` has `licenses-check` as a prerequisite, and `licenses-check` has each of
#     licenses-unit licenses-fixture licenses-stale-guard licenses-rust licenses-npm licenses-stale.
# Exit 0 and stdout holding `db-check ok` when both hold; exit 1 and stderr naming `NOTICES`, or
# the target and the missing prerequisite, otherwise; exit 3 when make cannot run or the file is
# missing. `--db-check` runs no other part of the guard and never calls this test (no recursion).
# No decision may rest on a pipe into an early-exit reader (I-1: SIGPIPE under pipefail).
#
# The normal run of stale-check.sh (no argument) runs --db-check on the real Makefile with a
# NOTICES sentinel in the environment, on every licenses/fixtures/stale-db/<case> listed in the
# script as a line `db_case <case> <0|nonzero>` (like the `check` lines of the stale cases), and
# then this test script.
#
# Exit 0: all hold. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
script="$root/scripts/licenses/stale-check.sh"
db="$root/licenses/fixtures/stale-db"
sentinel=/nonexistent/sentinel/NOTICES.txt
for tool in make grep wc mktemp; do
  command -v "$tool" >/dev/null 2>&1 || { echo "stale-check.test: cannot run: $tool not found" >&2; exit 3; }
done
tmp="$(mktemp -d)" || { echo "stale-check.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

failures=""
fail() { failures="$failures"$'\n'"  - $1"; }

# dbc <makefile> [VAR=VALUE...]: sets out, err, status.
dbc() {
  local mk="$1"
  shift
  out="$(env "$@" bash "$script" --db-check "$mk" 2>"$tmp/err")"
  status=$?
  err="$(<"$tmp/err")"
}
detail() { echo "status $status; stdout: ${out:0:300}; stderr: ${err:0:600}"; }
# expect_pass <name> <makefile> [VAR=VALUE...]
expect_pass() {
  local name="$1"
  shift
  dbc "$@"
  { [ "$status" -eq 0 ] && [[ "$out" == *"db-check ok"* ]]; } || fail "$name: want exit 0 and 'db-check ok'; $(detail)"
}
# expect_refused <name> <needle> <makefile> [VAR=VALUE...]
expect_refused() {
  local name="$1" needle="$2"
  shift 2
  dbc "$@"
  if [ "$status" -ne 1 ] || [[ "$err" != *"$needle"* ]] || [[ "$out" == *"db-check ok"* ]]; then
    fail "$name: want exit 1 with stderr naming '$needle'; $(detail)"
  fi
}

# --- the ok fixture is what the analysis says: >= 300 KB of the dump after the NOTICES line ---
make -pRrq -f "$db/ok/Makefile" : >"$tmp/ok.dump" 2>/dev/null
total="$(wc -c <"$tmp/ok.dump")"
hit="$(grep -bo '^NOTICES := THIRD-PARTY-NOTICES\.txt$' "$tmp/ok.dump")"
off="${hit%%:*}"
if [ -z "$hit" ] || [ $((total - off)) -lt 300000 ]; then
  fail "fixture ok: want NOTICES := with >= 300000 bytes of the dump after it; offset '${off:-none}' of $total"
fi

# --- ok / lazy / recursive / none / real Makefile ---
for i in $(seq 1 30); do # a pipe-fed `grep -q` failed 100 of 100 here; a here-string never does
  expect_pass "ok (round $i, 800 KB after NOTICES)" "$db/ok/Makefile"
done
expect_pass "ok with NOTICES=sentinel in the environment (:= is env-immune)" "$db/ok/Makefile" "NOTICES=$sentinel"
expect_refused "lazy (NOTICES ?=)" NOTICES "$db/lazy/Makefile"
expect_refused "lazy (NOTICES ?=) with NOTICES=sentinel in the environment" NOTICES "$db/lazy/Makefile" "NOTICES=$sentinel"
expect_refused "recursive (NOTICES =)" NOTICES "$db/recursive/Makefile"
expect_refused "none (no NOTICES)" NOTICES "$db/none/Makefile"
expect_pass "the real Makefile with NOTICES=sentinel in the environment" "$root/Makefile" "NOTICES=$sentinel"

# --- prerequisites: one mutant per edge, derived from the ok fixture ---
ok_text="$(<"$db/ok/Makefile")"
prereqs="licenses-unit licenses-fixture licenses-stale-guard licenses-rust licenses-npm licenses-stale"
lc_line="licenses-check: $prereqs"
mutant() { # <name> <needle> <old> <new>
  local text="${ok_text/"$3"/"$4"}"
  if [ "$text" = "$ok_text" ]; then fail "$1: the mutation changed nothing (fixture edited?)"; return; fi
  mkdir -p "$tmp/m"
  printf '%s\n' "$text" >"$tmp/m/Makefile"
  expect_refused "$1" "$2" "$tmp/m/Makefile"
}
mutant "check without licenses-check" licenses-check "check: licenses-check" "check:"
for p in $prereqs; do
  rest=""
  for q in $prereqs; do [ "$q" = "$p" ] || rest="$rest $q"; done
  mutant "licenses-check without $p" "$p" "$lc_line" "licenses-check:$rest"
done

# `check` absent as a target, and `licenses-check` only as an order-only prerequisite (T-181 bite).
mutant "check with no prerequisite at all" check "check: licenses-check" ""
mutant "check has licenses-check only order-only" licenses-check "check: licenses-check" "check: | licenses-check"
mutant "licenses-check has licenses-unit only order-only" licenses-unit "$lc_line" "licenses-check: | $prereqs"

# --- cannot run is not a refusal ---
dbc "$tmp/absent/Makefile"
[ "$status" -eq 3 ] || fail "a missing Makefile: want exit 3; $(detail)"
# make not on PATH: exit 3
mkdir -p "$tmp/nomake"
for t in dirname env grep; do ln -sf "$(command -v "$t")" "$tmp/nomake/$t"; done
out="$(PATH="$tmp/nomake" "$BASH" "$script" --db-check "$db/ok/Makefile" 2>"$tmp/err.txt")"
status=$?
err="$(<"$tmp/err.txt")"
[ "$status" -eq 3 ] || fail "make not on PATH: want exit 3; $(detail)"

# --- the normal run lists every stale-db case ---
for c in "$db"/*/; do
  c="$(basename "$c")"
  want=nonzero
  [ "$c" = ok ] && want=0
  grep -Eq "^db_case $c $want( |$)" "$script" || fail "stale-check.sh lacks a line 'db_case $c $want'"
done
for c in ok lazy recursive none; do [ -d "$db/$c" ] || fail "fixture dir $db/$c missing"; done

# --- I-1: no pipe from printf/echo/cat into grep/awk/head/sed/sort in a licenses script ---
files=()
for f in "$root"/scripts/licenses/*.sh; do
  [ "$f" = "$root/scripts/licenses/stale-check.test.sh" ] || files+=("$f")
done
hits="$(grep -nE '\b(printf|echo|cat)\b[^|]*\|[[:space:]]*(grep|awk|head|sed|sort)\b' "${files[@]}" | grep -vE ':[0-9]+:[[:space:]]*#')"
[ -z "$hits" ] || fail "a writer is piped into an early-exit reader (I-1):"$'\n'"$hits"

if [ -n "$failures" ]; then
  echo "stale-check.test: FAIL:$failures" >&2
  exit 1
fi
echo "stale-check.test: ok"
