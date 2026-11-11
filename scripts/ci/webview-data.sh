#!/usr/bin/env bash
# T-062 (P-020, decides_on: token): every webview of the shell keeps its WebView2 data under
# %LOCALAPPDATA%\Voicen\webview. The one mechanism is the app-level override of the release context
# (paths::route_app_local_data, called once in run()): tauri's app_local_data_dir, the default data
# directory of every window that sets none, then resolves there. The only way past it is a
# window's own data directory (tauri-utils config.rs: "The only exception is a window's
# dataDirectory config"), so the guard reads three raw tokens and no Rust or JSON syntax (F-003):
#   data_directory           zero occurrences in <shell-dir>/src/**/*.rs (a builder's
#                            .data_directory(..), any line, comments included);
#   dataDirectory            zero occurrences in <shell-dir>/*.json (a config window's setting,
#                            tauri.conf.json and any other config file next to it);
#   app_directories_override exactly one occurrence in <shell-dir>/src/**/*.rs, in
#                            <shell-dir>/src/paths.rs (the override has one writer, P-010;
#                            none means the override was lost, a second one is a second owner).
# Occurrences are counted as raw substrings, comments included; the guard knows no word boundary.
#
# Usage: scripts/ci/webview-data.sh [shell-dir]   (default src-tauri)
# Exit 0: "webview-data: ok: <n> .rs, <m> .json file(s)". Exit 1: one "<file>:<line>: webview-data:
# ..." per offending line (or one "<path>: webview-data: ..." line for a missing/duplicated
# override). Exit 3: cannot run (no src dir, no *.rs, no *.json, an unreadable file); never a pass.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
shell="${1:-src-tauri}"
shell="${shell%/}"
cannot_run() { echo "webview-data: cannot run: $1" >&2; exit 3; }
[ -d "$shell/src" ] && [ -r "$shell/src" ] || cannot_run "$shell/src not found or not readable"

mapfile -d '' rs < <(find "$shell/src" -type f -name '*.rs' -print0 | sort -z)
[ "${#rs[@]}" -gt 0 ] || cannot_run "no *.rs under $shell/src"
mapfile -d '' js < <(find "$shell" -maxdepth 1 -type f -name '*.json' -print0 | sort -z)
[ "${#js[@]}" -gt 0 ] || cannot_run "no *.json directly under $shell"
for f in "${rs[@]}" "${js[@]}"; do
  [ -r "$f" ] || cannot_run "$f is not readable"
done

bad=0
report="$(LC_ALL=C awk '
  index($0, "data_directory") { print FILENAME ":" FNR ": webview-data: data_directory in shell code; a window must take the app-level data directory (paths::route_app_local_data), never its own" }
' "${rs[@]}")" || cannot_run "awk failed"
if [ -n "$report" ]; then printf '%s\n' "$report"; bad=1; fi

report="$(LC_ALL=C awk '
  index($0, "dataDirectory") { print FILENAME ":" FNR ": webview-data: dataDirectory in a shell config file; a config window with its own data directory leaves the app-level override" }
' "${js[@]}")" || cannot_run "awk failed"
if [ -n "$report" ]; then printf '%s\n' "$report"; bad=1; fi

# Occurrences of the override token per .rs file (a line with it twice counts twice).
total=0
in_paths=0
for f in "${rs[@]}"; do
  n="$(LC_ALL=C awk '{ s = $0; while ((i = index(s, "app_directories_override")) > 0) { c++; s = substr(s, i + 23) } } END { print c + 0 }' "$f")" \
    || cannot_run "awk failed"
  total=$((total + n))
  if [ "$f" = "$shell/src/paths.rs" ]; then
    in_paths=$n
  elif [ "$n" -gt 0 ]; then
    echo "$f: webview-data: app_directories_override outside $shell/src/paths.rs ($n occurrence(s)); the override has one writer"
    bad=1
  fi
done
if [ "$in_paths" != 1 ] || [ "$total" != 1 ]; then
  if [ "$in_paths" != 1 ]; then
    echo "$shell/src/paths.rs: webview-data: app_directories_override occurs $in_paths time(s), want exactly 1 (the release context's webview data override)"
  fi
  bad=1
fi

if [ "$bad" != 0 ]; then
  echo "webview-data: FAIL (T-062: WebView2 data of every window under Voicen\\webview)" >&2
  exit 1
fi
echo "webview-data: ok: ${#rs[@]} .rs, ${#js[@]} .json file(s)"
