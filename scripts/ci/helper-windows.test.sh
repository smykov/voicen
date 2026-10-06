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
#                 <key> | <verdict> | <class or -> | <source citation>
#               verdict: visible-helper | hidden-helper | child | binding; visible-helper and
#               hidden-helper name a class (pattern, `{identifier}` = tauri.conf.json
#               `identifier`), child and binding give `-`. Any other verdict, or a helper
#               verdict with `-`, is a violation (exit 1).
#               <key> (T-065 review 1 #1: a feature flip of a listed crate changes its windows
#               without touching crate@version, e.g. tauri-plugin-single-instance's `semver`
#               feature suffixes its `-sic` class):
#                 visible-helper, hidden-helper, child: <crate>@<version> when the crate's
#                   resolved feature set is empty, else <crate>@<version>[<f1>,<f2>,...] with
#                   the features sorted bytewise (LC_ALL=C), comma separated, no spaces;
#                 binding: <crate>@<version> only, whatever its features (a bindings crate
#                   declares CreateWindowEx and creates no window under any feature; keying it
#                   by features would trip on every Win32 feature src-tauri adds). A binding
#                   entry with a feature list is malformed (exit 1).
#               So a feature gained or lost by a non-binding crate shows as the new key
#               unlisted and the old one stale (exit 1).
#   --graph     default: the shell's Windows dependency graph (cargo metadata for
#               x86_64-pc-windows-msvc, as check-shell-windows resolves it; the features are
#               `resolve.nodes[].features` of each package). A graph file has one line per
#               package, `<name> <version> [<features>] <package dir>` (whitespace separated;
#               the optional third token `[f1,f2,...]` is the resolved feature set in any
#               order, absent or `[]` = none; a relative dir is relative to the graph file's
#               own directory).
#   The guard greps, recursively and as raw text (comments count), each package's src/ in
#   --graph mode; in default mode each package's src/, or its whole package dir when a lib,
#   proc-macro or bin target lies outside src/ (a crate with lib.rs at its root; review 1
#   #3). It compares the set of keys of the packages that mention `CreateWindowEx` with the
#   set of manifest keys.
#   Product window class (T-065 review 1 #2, F-003): a visible-helper class is never the
#   app's product window class. The product classes are the string literals passed as
#   `window_classname("<class>")` in the scanned source of the graph's packages (today
#   tauri-runtime-wry's WindowBuilderWrapper::new, "Tauri Window": every tauri window, the
#   click-through overlay included). A visible-helper entry whose class ({identifier}
#   resolved) equals one is a violation (exit 1, names the crate@version and the class). No
#   such literal anywhere in the graph is "cannot run" (exit 3): the rule is never skipped
#   because its input is missing.
#     exit 0  sets equal and no product class listed as a visible helper; prints "ok:".
#     exit 1  a matching package not in the manifest (unlisted; a version or feature change
#             shows as the new key unlisted and the old one stale), or a manifest entry that
#             is not a matching package of the graph (stale: gone from the graph, its source no
#             longer mentions CreateWindowEx, or its features changed), or a malformed entry,
#             or a product class listed as a visible helper. The listing names each key
#             ("unlisted: <key>", "stale: <key> ...").
#     exit 3  cannot run: graph or manifest missing, a package dir or its src/ missing, no
#             product window class in the graph, a grep/cargo/docker error. Never a pass.
#
# Cases (fixture dirs hold graph.txt, manifest.txt and crates/<name>-<version>/src/...; every
# fixture graph holds tauri-runtime-wry with its `window_classname("...")` literal, the input of
# the product-class rule):
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
# T-065 review 1 #1: resolved features are part of the key. Graph features in any order, `[]` =
# none; the manifest key lists them sorted.
check ok-features             0 "ok:"
# A binding crate is keyed by crate@version alone: its Win32 features change nothing.
check ok-binding-features     0 "ok:"

# Drift: exit 1, the listing names the crate@version.
check v-unlisted-crate        1 "newhelper@0.1.0"   # a new crate calls CreateWindowExW (deep under src/), not listed
check v-stale-entry           1 "gone@1.0.0"        # listed, not in the graph
check v-version-change        1 "tao@0.38.0"        # tao bumped: the new version is unlisted ...
check v-version-change        1 "tao@0.37.1"        # ... and the listed old one is stale
check v-no-longer-creates     1 "quiet@2.0.0"       # listed and in the graph, its src/ no longer mentions CreateWindowEx
check v-comment-mention       1 "mentions@0.3.0"    # CreateWindowExA only in a comment: raw text, any mention trips (errs loud)
check v-unknown-verdict       1 "wry@0.57.0"        # verdict `maybe-child` is not one of the four
check v-helper-without-class  1 "tao@0.37.1"        # visible-helper with class `-`: the census would have nothing to match
# T-065 review 1 #1: a feature flip of a listed crate, same crate@version, is drift at its commit.
check v-feature-change        1 "unlisted: tauri-plugin-single-instance@2.5.2[semver]"  # the plugin gains `semver` (suffixed -sic class) ...
check v-feature-change        1 "stale: tauri-plugin-single-instance@2.5.2 "            # ... and its featureless entry is stale
check v-feature-dropped       1 "tao@0.37.1[rwh_06]"                                     # the entry records a feature the graph no longer resolves
check v-binding-with-features 1 "windows-sys@0.59.0"                                     # a binding entry carries no feature list
# T-065 review 1 #2 (F-003): the product window class is never a visible helper.
check v-product-class-helper  1 "tauri-runtime-wry@2.12.1"   # `Tauri Window` listed as visible-helper ...
check v-product-class-helper  1 "Tauri Window"               # ... and the listing names the class
check v-product-class-from-source 1 "Voicen Main Window"     # the product class comes from the runtime's window_classname("..."), not a literal in the guard

# Cannot run: exit 3, never a pass.
check c-missing-crate-dir     3 "cannot run"        # tray-icon's dir is gone: unscanned is not "does not call"
check c-crate-without-src     3 "cannot run"        # serde has no src/
check c-no-product-class      3 "cannot run"        # no window_classname("...") literal in the graph: the product-class rule cannot run
run "graph file missing" 3 "cannot run" --graph "$tmp/no-such-graph.txt" --manifest "$fx/ok-listed/manifest.txt"
run "manifest missing"   3 "cannot run" --graph "$fx/ok-listed/graph.txt" --manifest "$tmp/no-such-manifest.txt"
# A grep that fails must not read as "no package calls CreateWindowEx" or "no product class".
# Each case reaches one grep-error branch of scan_one and asserts the line only that branch
# prints (`grep failed on <dir> (<crate>@<version>)`), never the generic "cannot run" that the
# missing-product-class exit also prints (T-065 validation 1: M5a-c survived that needle).
# grep_shim <name> <pattern> <dir part>: a grep that fails (exit 2) when its arguments hold both
# <pattern> and <dir part> ("" = any dir) and runs the real grep otherwise.
real_grep="$(command -v grep)" || { echo "helper-windows.test: cannot run: grep not found" >&2; exit 3; }
grep_shim() {
  mkdir -p "$tmp/$1" && cat >"$tmp/$1/grep" <<SHIM && chmod +x "$tmp/$1/grep" \
    || { echo "helper-windows.test: cannot run: could not write the grep shim $1" >&2; exit 3; }
#!/bin/sh
case "\$*" in
  *'$2'*) case "\$*" in *'$3'*) echo "grep: simulated failure" >&2; exit 2 ;; esac ;;
esac
exec '$real_grep' "\$@"
SHIM
}
ok_src="$root/$fx/ok-listed/crates"
# Every grep fails: the first scan (tao's CreateWindowEx search) is the error named.
grep_shim grep-fails "" ""
GUARD_PATH="$tmp/grep-fails:$PATH"
run "ok-listed (failing grep)" 3 "cannot run: grep failed on $ok_src/tao-0.37.1/src (tao@0.37.1)" \
  --graph "$fx/ok-listed/graph.txt" --manifest "$fx/ok-listed/manifest.txt"
# Only tao's CreateWindowEx search fails: read as "no match", tao's entry would be stale (exit 1).
grep_shim grep-fails-createwindowex CreateWindowEx /tao-0.37.1/src
GUARD_PATH="$tmp/grep-fails-createwindowex:$PATH"
run "ok-listed (CreateWindowEx grep fails on tao)" 3 "cannot run: grep failed on $ok_src/tao-0.37.1/src (tao@0.37.1)" \
  --graph "$fx/ok-listed/graph.txt" --manifest "$fx/ok-listed/manifest.txt"
# Only tao's window_classname search fails (tao has no such literal): read as "none", the
# guard would pass (exit 0).
grep_shim grep-fails-classname-tao window_classname /tao-0.37.1/src
GUARD_PATH="$tmp/grep-fails-classname-tao:$PATH"
run "ok-listed (window_classname grep fails on tao)" 3 "cannot run: grep failed on $ok_src/tao-0.37.1/src (tao@0.37.1)" \
  --graph "$fx/ok-listed/graph.txt" --manifest "$fx/ok-listed/manifest.txt"
# Only the runtime's window_classname search fails, the dir holding the product class.
grep_shim grep-fails-classname-runtime window_classname /tauri-runtime-wry-2.12.1/src
GUARD_PATH="$tmp/grep-fails-classname-runtime:$PATH"
run "ok-listed (window_classname grep fails on tauri-runtime-wry)" 3 \
  "cannot run: grep failed on $ok_src/tauri-runtime-wry-2.12.1/src (tauri-runtime-wry@2.12.1)" \
  --graph "$fx/ok-listed/graph.txt" --manifest "$fx/ok-listed/manifest.txt"
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
    # T-065 review 1 #1: the default graph's keys carry resolve.nodes[].features. tao resolves
    # with features in the shell's graph (dbus, rwh_06, x11 at 0.37.1), so its committed entry
    # carries a feature list; the same manifest with that list dropped is drift naming tao.
    if grep -qE '^tao@[^ |]*\[[^]]+\]' "$manifest"; then
      sed -E 's/^(tao@[^ |[]*)\[[^]]*\]/\1/' "$manifest" >"$tmp/manifest-tao-no-features.txt"
      run "real graph, manifest with tao's features dropped" 1 "tao@$tao_version" --manifest "$tmp/manifest-tao-no-features.txt"
    else
      echo "FAIL real graph: the tao entry of $manifest has no feature list (want tao@<version>[<features>]: tao resolves with features in the shell's Windows graph, resolve.nodes[].features of cargo metadata; T-065 review 1 #1)" >&2
      failed=$((failed + 1))
    fi
    # T-065 review 1 #2: the default graph's product class (tauri-runtime-wry's
    # window_classname literal) is read; listing it as a visible helper is refused.
    if grep -qE '^tauri-runtime-wry@' "$manifest"; then
      sed -E 's/^(tauri-runtime-wry@[^|]*)\|[^|]*\|[^|]*\|/\1| visible-helper | Tauri Window |/' "$manifest" >"$tmp/manifest-product-helper.txt"
      run "real graph, product class listed as visible helper" 1 "Tauri Window" --manifest "$tmp/manifest-product-helper.txt"
    else
      echo "FAIL real graph: $manifest has no tauri-runtime-wry entry (it creates the drag-resize child windows of every undecorated tauri window)" >&2
      failed=$((failed + 1))
    fi
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
