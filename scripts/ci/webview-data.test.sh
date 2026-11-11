#!/usr/bin/env bash
# T-062 (P-020, decides_on: token) guard of the guard: the tripwire scripts/ci/webview-data.sh must
# give its documented exit code on every committed fixture shell dir under
# scripts/ci/fixtures/webview-data/<case>/ and on the real src-tauri.
#
# The rule: over <shell-dir>/src/**/*.rs the raw token data_directory occurs zero times; over
# <shell-dir>/*.json the raw token dataDirectory occurs zero times; the raw token
# app_directories_override occurs exactly once over <shell-dir>/src/**/*.rs, in
# <shell-dir>/src/paths.rs. Comments count (no syntax is read).
#   ok-*  exit 0, "ok: <n> .rs, <m> .json file(s)" (the counts pin that the files were read);
#   v-*   exit 1, the listing names <file>:<line>: webview-data (or <path>: webview-data for the
#         override), one line per offending line;
#   c-*   exit 3, "cannot run".
# Every fixture dir must appear in the table, so a case cannot be dropped silently. The real tree
# is red until the override exists in paths.rs (T-062).
# Usage: scripts/ci/webview-data.test.sh   (host bash)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
guard=scripts/ci/webview-data.sh
fx=scripts/ci/fixtures/webview-data
[ -d "$fx" ] || { echo "webview-data.test: cannot run: $fx not found" >&2; exit 3; }

failed=0
passed=0
declare -A seen=()

# run <label> <expected exit> <needle> [guard args...]; the needle must be in the output.
run() {
  local label="$1" want="$2" needle="$3" out got
  shift 3
  if [ ! -x "$guard" ]; then
    echo "FAIL $label: $guard not found or not executable (the tripwire does not exist)" >&2
    failed=$((failed + 1))
    return
  fi
  out="$("$guard" "$@" 2>&1)"
  got=$?
  if [ "$got" -ne "$want" ]; then
    echo "FAIL $label: exit $got, want $want" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
  elif ! grep -qF -- "$needle" <<<"$out"; then
    echo "FAIL $label: exit $got as wanted, but the output does not contain: $needle" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
  else
    echo "ok   $label: exit $got"
    passed=$((passed + 1))
  fi
}

# check <case> <expected exit> <needle>: the case's dir as the shell dir. A needle starting
# with "/" is taken relative to the case dir (file:line: kind).
check() {
  local n="$3"
  seen["$1"]=1
  [ "${n#/}" = "$n" ] || n="$fx/$1$n"
  run "$1" "$2" "$n" "$fx/$1"
}

# Allowed.
check ok-routed     0 "ok: 2 .rs, 1 .json file(s)"   # the override once in paths.rs, no per-window directory
check ok-near-misses 0 "ok: 3 .rs, 2 .json file(s)"  # data_dir, app_data_dir, dataDir are other words

# data_directory in code, comment or a nested dir.
check v-data-directory-in-code     1 "/src/overlay.rs:1: webview-data: data_directory"
check v-data-directory-in-comment  1 "/src/overlay.rs:1: webview-data: data_directory"
check v-data-directory-nested      1 "/src/win/hwnd.rs:2: webview-data: data_directory"

# dataDirectory in a config file next to tauri.conf.json.
check v-datadirectory-in-conf        1 "/tauri.conf.json:1: webview-data: dataDirectory"
check v-datadirectory-in-second-json 1 "/tauri.windows.conf.json:1: webview-data: dataDirectory"

# The override: lost, doubled, or written outside paths.rs.
check v-override-missing                1 "/src/paths.rs: webview-data: app_directories_override occurs 0 time(s)"
check v-override-twice-in-paths         1 "/src/paths.rs: webview-data: app_directories_override occurs 2 time(s)"
check v-override-in-other-file          1 "/src/lib.rs: webview-data: app_directories_override outside"
check v-override-in-other-file          1 "/src/paths.rs: webview-data: app_directories_override occurs 0 time(s)"
check v-override-in-paths-and-other-file 1 "/src/win/mod.rs: webview-data: app_directories_override outside"

# Cannot run.
check c-no-src  3 "cannot run"
check c-no-json 3 "cannot run"
check c-no-rs   3 "cannot run"

# The real shell: red until paths.rs holds the override once (T-062).
run "real tree (src-tauri)" 0 "webview-data: ok:"

# Every committed fixture dir is in the table.
for d in "$fx"/*/; do
  c="$(basename "$d")"
  if [ -z "${seen[$c]:-}" ]; then
    echo "FAIL $c: fixture dir has no expected exit code in $0" >&2
    failed=$((failed + 1))
  fi
done

if [ "$failed" -gt 0 ]; then
  echo "webview-data.test: FAIL: $failed case(s) differ, $passed as expected (T-062)" >&2
  exit 1
fi
echo "webview-data.test: ok: $passed cases as expected"
