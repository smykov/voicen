#!/usr/bin/env bash
# T-065 (rca, class smoke-window-predicate) guard of the guard: the helper-window tripwire
# scripts/ci/helper-windows.sh must give its documented exit code on every committed fixture
# under scripts/ci/fixtures/helper-windows/<case>/, on today's real Windows dependency graph
# with the committed manifest, and the predicate's copies must read that one manifest.
#
# Contract this file pins (T-065 analysis: seam, invariant):
#   scripts/ci/helper-windows.sh [--graph <file>] [--manifest <file>]
#   --manifest  default scripts/ci/helper-windows.txt. One entry per line, `#` comments and
#               blank lines ignored, fields separated by `|` and trimmed:
#                 <crate>@<version> | <verdict> | <class or -> | <source citation>
#               verdict: visible-helper | hidden-helper | child | binding; visible-helper and
#               hidden-helper name a class (pattern, `{identifier}` = tauri.conf.json
#               `identifier`), child and binding give `-`. Any other verdict, or a helper
#               verdict with `-`, is a violation (exit 1).
#   --graph     default: the shell's Windows dependency graph (cargo metadata for
#               x86_64-pc-windows-msvc, as check-shell-windows resolves it). A graph file has
#               one line per package, `<name> <version> <package dir>` (whitespace separated;
#               a relative dir is relative to the graph file's own directory).
#   The guard greps each package's src/ (recursively, raw text: comments count) for
#   `CreateWindowEx` and compares the set of <name>@<version> that match with the set of
#   manifest entries:
#     exit 0  sets equal; prints "ok:".
#     exit 1  a matching package not in the manifest (unlisted; a version change shows as the
#             new version unlisted and the old one stale), or a manifest entry that is not a
#             matching package of the graph (stale: gone from the graph, or its src/ no longer
#             mentions CreateWindowEx), or a malformed entry. The listing names each
#             <crate>@<version>.
#     exit 3  cannot run: graph or manifest missing, a package dir or its src/ missing, a
#             grep/cargo/docker error. Never a pass.
#
# Cases (fixture dirs hold graph.txt, manifest.txt and crates/<name>-<version>/src/...):
#   ok-*  every window-creating crate listed, every entry matched: exit 0.
#   v-*   one drift each: exit 1, the listing names the crate@version.
#   c-*   cannot run: exit 3, "cannot run".
# Every fixture dir must appear in the table, so a case cannot be dropped silently.
#
# Usage: scripts/ci/helper-windows.test.sh   (host bash; the real-graph case runs the guard
# with its defaults, which reach the core image through scripts/tw-run)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
guard=scripts/ci/helper-windows.sh
manifest=scripts/ci/helper-windows.txt
fx=scripts/ci/fixtures/helper-windows
[ -d "$fx" ] || { echo "helper-windows.test: cannot run: $fx not found" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "helper-windows.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

failed=0
passed=0
cannot=0
declare -A seen=()

# run <label> <expected exit> <needle> [guard args...]
run() {
  local label="$1" want="$2" needle="$3" out got
  shift 3
  if [ ! -x "$guard" ]; then
    echo "FAIL $label: $guard not found or not executable (the tripwire does not exist)" >&2
    failed=$((failed + 1))
    return
  fi
  # GUARD_PATH: a PATH for the guard only (a tool shim), never for this harness's own grep.
  out="$(PATH="${GUARD_PATH:-$PATH}" "$guard" "$@" 2>&1)"
  got=$?
  if [ "$got" -ne "$want" ]; then
    echo "FAIL $label: exit $got, want $want" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
    [ "$got" -eq 3 ] && [ "$want" -ne 3 ] && cannot=$((cannot + 1))
  elif ! grep -qF -- "$needle" <<<"$out"; then
    echo "FAIL $label: exit $got as wanted, but the output does not contain: $needle" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
  else
    echo "ok   $label: exit $got"
    passed=$((passed + 1))
  fi
}

# check <case> <expected exit> <needle>: the case's own graph.txt and manifest.txt.
check() {
  seen["$1"]=1
  run "$1" "$2" "$3" --graph "$fx/$1/graph.txt" --manifest "$fx/$1/manifest.txt"
}

# Allowed: a visible helper, a hidden helper, a child-window crate and a binding crate, all
# listed with their exact version; a crate without CreateWindowEx needs no entry.
check ok-listed               0 "ok:"

# Drift: exit 1, the listing names the crate@version.
check v-unlisted-crate        1 "newhelper@0.1.0"   # a new crate calls CreateWindowExW (deep under src/), not listed
check v-stale-entry           1 "gone@1.0.0"        # listed, not in the graph
check v-version-change        1 "tao@0.38.0"        # tao bumped: the new version is unlisted ...
check v-version-change        1 "tao@0.37.1"        # ... and the listed old one is stale
check v-no-longer-creates     1 "quiet@2.0.0"       # listed and in the graph, its src/ no longer mentions CreateWindowEx
check v-comment-mention       1 "mentions@0.3.0"    # CreateWindowExA only in a comment: raw text, any mention trips (errs loud)
check v-unknown-verdict       1 "wry@0.57.0"        # verdict `maybe-child` is not one of the four
check v-helper-without-class  1 "tao@0.37.1"        # visible-helper with class `-`: the census would have nothing to match

# Cannot run: exit 3, never a pass.
check c-missing-crate-dir     3 "cannot run"        # tray-icon's dir is gone: unscanned is not "does not call"
check c-crate-without-src     3 "cannot run"        # serde has no src/
run "graph file missing" 3 "cannot run" --graph "$tmp/no-such-graph.txt" --manifest "$fx/ok-listed/manifest.txt"
run "manifest missing"   3 "cannot run" --graph "$fx/ok-listed/graph.txt" --manifest "$tmp/no-such-manifest.txt"
# A grep that fails must not read as "no package calls CreateWindowEx".
mkdir -p "$tmp/grep-fails" && printf '#!/bin/sh\necho "grep: simulated failure" >&2\nexit 2\n' >"$tmp/grep-fails/grep" \
  && chmod +x "$tmp/grep-fails/grep" || { echo "helper-windows.test: cannot run: could not write the grep shim" >&2; exit 3; }
GUARD_PATH="$tmp/grep-fails:$PATH"
run "ok-listed (failing grep)" 3 "cannot run" --graph "$fx/ok-listed/graph.txt" --manifest "$fx/ok-listed/manifest.txt"
GUARD_PATH=""

# Today's real graph with the committed manifest: green. Then the same graph with the
# manifest minus tao's entry: red naming tao, so the default graph is really resolved and
# scanned (an empty or partial graph cannot pass the first case only by luck).
if [ -f "$manifest" ]; then
  run "real graph, committed manifest" 0 "ok:"
  tao_version="$(sed -n 's/^tao@\([^ |]*\).*/\1/p' "$manifest" | head -n1)"
  if [ -n "$tao_version" ]; then
    grep -v '^tao@' "$manifest" >"$tmp/manifest-without-tao.txt"
    run "real graph, manifest without tao" 1 "tao@$tao_version" --manifest "$tmp/manifest-without-tao.txt"
  else
    echo "FAIL real graph: $manifest has no tao@<version> entry (tao creates the event-target window in every tauri process)" >&2
    failed=$((failed + 1))
  fi
else
  echo "FAIL real graph: $manifest not found (the helper-window manifest does not exist)" >&2
  failed=$((failed + 1))
fi

# One source (Acceptance 3): the predicate's copies read the manifest; no copy keeps its own
# class literal. visible-windows.ps1 and overlay.rs must not name a helper class; overlay.rs
# goes through the shared test support module. (smoke_predicate.rs names the classes as test
# data for the windows it creates; it is not a copy of the rule.)
one_source() {
  local file="$1" lit
  for lit in "Tao Thread Event Target" "-sic\""; do
    if grep -nF -- "$lit" "$file" >"$tmp/hits" 2>&1; then
      echo "FAIL one source: $file keeps its own helper class literal ($lit) instead of reading $manifest:" >&2
      sed 's/^/       | /' "$tmp/hits" >&2
      failed=$((failed + 1))
      return
    fi
  done
  echo "ok   one source: $file names no helper class"
  passed=$((passed + 1))
}
one_source scripts/ci/visible-windows.ps1
one_source src-tauri/tests/overlay.rs
if grep -qE '^[[:space:]]*mod helper_windows;' src-tauri/tests/overlay.rs \
  && grep -qE 'helper_windows::' src-tauri/tests/overlay.rs; then
  echo "ok   one source: overlay.rs uses the shared helper_windows support module"
  passed=$((passed + 1))
else
  echo "FAIL one source: src-tauri/tests/overlay.rs does not use the shared support module (mod helper_windows; helper_windows::...) for shown_windows" >&2
  failed=$((failed + 1))
fi
if grep -qF 'helper-windows.txt' scripts/ci/visible-windows.ps1 \
  && grep -qF 'helper-windows.txt' src-tauri/tests/helper_windows/mod.rs 2>/dev/null; then
  echo "ok   one source: visible-windows.ps1 and tests/helper_windows/mod.rs read $manifest"
  passed=$((passed + 1))
else
  echo "FAIL one source: visible-windows.ps1 and src-tauri/tests/helper_windows/mod.rs must both read $manifest" >&2
  failed=$((failed + 1))
fi

# Every committed fixture dir is in the table.
for d in "$fx"/*/; do
  c="$(basename "$d")"
  if [ -z "${seen[$c]:-}" ]; then
    echo "FAIL $c: fixture dir has no expected exit code in $0" >&2
    failed=$((failed + 1))
  fi
done

if [ "$failed" -gt 0 ]; then
  if [ "$failed" -eq "$cannot" ]; then
    echo "helper-windows.test: cannot run: the tripwire could not run ($cannot case(s) exit 3); not a pass" >&2
    exit 3
  fi
  echo "helper-windows.test: FAIL: $failed case(s) differ, $passed as expected; the helper-window tripwire does not hold its contract (T-065)" >&2
  exit 1
fi
echo "helper-windows.test: ok: $passed cases as expected"
