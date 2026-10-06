#!/usr/bin/env bash
# T-065 tripwire (rca, class smoke-window-predicate, F-008; docs/decisions/overlay.md §5,
# docs/decisions/ci-toolchain.md). The install smoke's window predicate
# (scripts/ci/visible-windows.ps1) excludes a framework helper window only when its class is a
# visible-helper class of the manifest scripts/ci/helper-windows.txt (AND the four helper
# ex-style bits). That manifest is only as good as its coverage of the shell's Windows
# dependency graph, which changes in src-tauri/Cargo.toml and Cargo.lock without touching the
# predicate. This check ties the two together on the Linux host, at the commit that changes a
# dependency: the set of keys (crate, version and, for a window-creating crate, its resolved
# features) of the graph's packages whose source mentions CreateWindowEx must equal the set of
# manifest keys. A new crate, version or feature set that may create windows, an entry whose
# crate left the graph or no longer mentions it, and a malformed entry all fail, so each is read
# and given a verdict before any push. It also refuses the F-003 direction: a visible-helper
# entry whose class is a product window class of the graph (a `window_classname("<class>")`
# literal, today tauri-runtime-wry's "Tauri Window") would exclude a product window, e.g. a
# click-through overlay shown at start. The Windows half (the reviewed verdicts against the
# real voicen.exe) is Get-HelperDrift in visible-windows.ps1, run by the install smoke.
#
# Manifest (default scripts/ci/helper-windows.txt): one entry per line, `#` comments and blank
# lines ignored, four fields separated by `|` and trimmed:
#   <key> | <verdict> | <class or -> | <source citation>
# verdict: visible-helper | hidden-helper (a class, `{identifier}` = src-tauri/tauri.conf.json
# `identifier`) or child | binding (class `-`). <key>:
#   visible-helper, hidden-helper, child: <crate>@<version> when the crate's resolved feature set
#     is empty, else <crate>@<version>[<f1>,<f2>,...], features sorted bytewise (LC_ALL=C),
#     comma separated, no spaces (a feature can change the windows a crate creates, e.g.
#     tauri-plugin-single-instance's `semver` suffixes its class);
#   binding: <crate>@<version> only, whatever its features (a bindings crate declares the Win32
#     function and creates no window under any feature).
# Any other verdict, a helper verdict without a class, a child or binding entry with one, a
# binding entry with a feature list, an unsorted or empty feature list, an empty citation, a field
# count other than four or a duplicate key is a violation.
#
# Graph:
#   --graph <file>  one line per package, `<name> <version> [<features>] <package dir>`
#                   (whitespace separated; the optional third token `[f1,f2,...]` is the resolved
#                   feature set in any order, absent or `[]` = none; the dir may hold spaces; a
#                   relative dir is relative to the graph file's own directory; blank and `#`
#                   lines ignored). Each package's src/ is scanned; a missing package dir or src/
#                   is "cannot run" (unscanned is not "does not create windows").
#   default         the shell's Windows dependency graph: `cargo metadata --filter-platform
#                   x86_64-pc-windows-msvc` of src-tauri in the core image (scripts/tw-run core,
#                   the registry check-shell-windows resolves), every package reachable from
#                   voicen over normal dependency edges (what voicen.exe links; build and dev
#                   dependencies never run in it), workspace members excluded (our own windows
#                   are never excluded by the predicate, so they need no entry); its features are
#                   `resolve.nodes[].features`. Each package's src/ is scanned, or its whole
#                   package dir when a lib, proc-macro or bin target lies outside src/ (a crate
#                   with lib.rs at its root). The scan runs in the core image, where the registry
#                   paths cargo reports exist.
# The scan is raw text: `grep -rF CreateWindowEx` (comments count, CreateWindowExA/W both
# match). It errs loud: any mention needs an entry, never a silent pass. The same scan collects
# the product window classes: every string literal passed as `window_classname("<class>")`. No
# such literal in the graph is "cannot run": the product-class rule is never skipped.
#
# Usage: scripts/ci/helper-windows.sh [--graph <file>] [--manifest <file>]
#        (internal: --scan-roots <file>, lines `<name> <version> [<features>] <dir>` with <dir>
#        scanned as is; prints `W <name> <version> [<features>]` for a dir that mentions
#        CreateWindowEx and `P <class>` per product class literal; used by the default mode
#        inside the core image)
# Exit 0: sets equal and no product class listed as a visible helper ("ok:"). Exit 1: drift, a
# malformed entry or a product class listed as a visible helper (each key named). Exit 3: cannot
# run (graph or manifest missing, a package dir or src/ missing, no product window class in the
# graph, a grep, cargo, python3 or docker error); never reported as a pass.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
me=helper-windows
cannot_run() { echo "$me: cannot run: $1" >&2; exit 3; }

graph=""
manifest=scripts/ci/helper-windows.txt
scan_roots=""
while [ $# -gt 0 ]; do
  case "$1" in
    --graph) [ $# -ge 2 ] || cannot_run "--graph needs a file"; graph="$2"; shift 2 ;;
    --manifest) [ $# -ge 2 ] || cannot_run "--manifest needs a file"; manifest="$2"; shift 2 ;;
    --scan-roots) [ $# -ge 2 ] || cannot_run "--scan-roots needs a file"; scan_roots="$2"; shift 2 ;;
    *) cannot_run "unknown argument '$1' (usage: $0 [--graph <file>] [--manifest <file>])" ;;
  esac
done

# sort_features <a,b,...>: the list sorted bytewise and deduplicated, comma separated.
sort_features() {
  [ -n "$1" ] || return 0
  tr ',' '\n' <<<"$1" | LC_ALL=C sort -u | paste -sd, -
}

# parse_graph_line <line>: sets g_name, g_version, g_feats (`[sorted,features]`, `[]` = none)
# and g_dir; returns 1 on a malformed line.
parse_graph_line() {
  local rest tok inner
  read -r g_name g_version rest <<<"$1"
  g_feats="[]"
  case "$rest" in
    \[*)
      tok="${rest%%[[:space:]]*}"
      [ "$tok" != "$rest" ] || return 1
      case "$tok" in *\]) ;; *) return 1 ;; esac
      inner="${tok#[}"; inner="${inner%]}"
      case "$inner" in *[][]* | ,* | *, | *,,*) return 1 ;; esac
      g_feats="[$(sort_features "$inner")]"
      rest="${rest#"$tok"}"
      rest="${rest#"${rest%%[![:space:]]*}"}"
      ;;
  esac
  g_dir="$rest"
  [ -n "$g_name" ] && [ -n "$g_version" ] && [ -n "$g_dir" ]
}

# scan_one <name> <version> <features> <dir>: prints `W <name> <version> <features>` when <dir>
# mentions CreateWindowEx, and `P <class>` per window_classname("<class>") literal in <dir>.
# grep: 0 = a match, 1 = none, anything else = an error (cannot run, never "none").
scan_one() {
  local hits
  grep -rqF -- CreateWindowEx "$4"
  case $? in
    0) echo "W $1 $2 $3" ;;
    1) ;;
    *) cannot_run "grep failed on $4 ($1@$2)" ;;
  esac
  hits="$(grep -rhoE -- 'window_classname\("[^"]*"\)' "$4")"
  case $? in
    0) sed -e 's/^window_classname("\(.*\)")$/P \1/' <<<"$hits" ;;
    1) ;;
    *) cannot_run "grep failed on $4 ($1@$2)" ;;
  esac
}

# Internal mode: the scan of the default graph, inside the core image.
if [ -n "$scan_roots" ]; then
  [ -f "$scan_roots" ] || cannot_run "scan roots file $scan_roots not found"
  while IFS= read -r line || [ -n "$line" ]; do
    [ -z "${line//[[:space:]]/}" ] && continue
    parse_graph_line "$line" || cannot_run "malformed scan roots line: $line"
    [ -d "$g_dir" ] || cannot_run "source dir $g_dir of $g_name@$g_version not found"
    scan_one "$g_name" "$g_version" "$g_feats" "$g_dir" || exit 3
  done <"$scan_roots"
  exit 0
fi

[ -f "$manifest" ] || cannot_run "manifest $manifest not found"

tmp="$(mktemp -d)" || cannot_run "mktemp failed"
trap 'rm -rf "$tmp"' EXIT
scanned="$tmp/scanned.txt"

if [ -n "$graph" ]; then
  [ -f "$graph" ] || cannot_run "graph file $graph not found"
  gdir="$(cd "$(dirname "$graph")" && pwd)" || cannot_run "cannot resolve the directory of $graph"
  : >"$scanned"
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in \#*) continue ;; esac
    [ -z "${line//[[:space:]]/}" ] && continue
    parse_graph_line "$line" || cannot_run "malformed graph line (want <name> <version> [<features>] <dir>): $line"
    dir="$g_dir"
    case "$dir" in /*) ;; *) dir="$gdir/$dir" ;; esac
    [ -d "$dir" ] || cannot_run "package dir $dir of $g_name@$g_version not found"
    [ -d "$dir/src" ] || cannot_run "$g_name@$g_version has no src/ under $dir"
    scan_one "$g_name" "$g_version" "$g_feats" "$dir/src" >>"$scanned" || exit 3
  done <"$graph"
else
  # Default: the shell's Windows dependency graph, resolved and scanned in the core image.
  command -v python3 >/dev/null 2>&1 || cannot_run "python3 not found (reads cargo metadata)"
  work="$root/target/helper-windows"
  mkdir -p "$work" || cannot_run "cannot create $work"
  run="$(mktemp -d "$work/run.XXXXXX")" || cannot_run "mktemp failed in $work"
  trap 'rm -rf "$tmp" "$run"' EXIT
  if ! scripts/tw-run core -- 'cargo metadata --format-version 1 --locked --filter-platform x86_64-pc-windows-msvc --manifest-path src-tauri/Cargo.toml' \
    >"$run/metadata.json" 2>"$run/metadata.err"; then
    sed 's/^/  | /' "$run/metadata.err" >&2
    cannot_run "cargo metadata for x86_64-pc-windows-msvc failed in the core image (scripts/tw-run core)"
  fi
  python3 - "$run/metadata.json" >"$run/roots.txt" <<'PY' || cannot_run "could not read the dependency graph from cargo metadata"
import json, os, sys

meta = json.load(open(sys.argv[1], encoding="utf-8"))
packages = {p["id"]: p for p in meta["packages"]}
resolve = meta.get("resolve") or {}
nodes = {n["id"]: n for n in resolve.get("nodes") or []}
top = resolve.get("root")
if not top or top not in nodes:
    sys.exit("no resolve root (src-tauri's voicen) in cargo metadata")
members = set(meta.get("workspace_members") or [])
seen, stack = set(), [top]
while stack:
    for dep in nodes[stack.pop()]["deps"]:
        if dep["pkg"] in seen:
            continue
        if any(k.get("kind") is None for k in dep.get("dep_kinds") or []):
            seen.add(dep["pkg"])
            stack.append(dep["pkg"])
code_kinds = {"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro", "bin"}
rows = []
for pid in seen - members:
    p = packages[pid]
    if "features" not in nodes[pid]:
        sys.exit("no resolved features for %s %s in cargo metadata" % (p["name"], p["version"]))
    features = sorted(set(nodes[pid]["features"]), key=lambda f: f.encode("utf-8"))
    pkg_dir = os.path.dirname(p["manifest_path"])
    src = os.path.join(pkg_dir, "src")
    paths = [t["src_path"] for t in p["targets"] if code_kinds & set(t["kind"])]
    inside = bool(paths) and all(x.startswith(src + os.sep) for x in paths)
    rows.append((p["name"], p["version"], "[" + ",".join(features) + "]", src if inside else pkg_dir))
for row in sorted(rows):
    print(*row)
PY
  [ -s "$run/roots.txt" ] || cannot_run "the dependency graph read from cargo metadata is empty"
  rel="${run#"$root"/}"
  if ! scripts/tw-run core -- scripts/ci/helper-windows.sh --scan-roots "$rel/roots.txt" \
    >"$scanned" 2>"$run/scan.err"; then
    sed 's/^/  | /' "$run/scan.err" >&2
    cannot_run "the scan of the dependency graph failed in the core image"
  fi
fi

# The window-creating packages (`<name>@<version> <[features]>`) and the product classes.
matched_raw="$tmp/matched-raw.txt"
products="$tmp/products.txt"
sed -n 's/^W \([^ ]*\) \([^ ]*\) \([^ ]*\)$/\1@\2 \3/p' "$scanned" >"$matched_raw" || cannot_run "sed failed"
sed -n 's/^P //p' "$scanned" | LC_ALL=C sort -u >"$products" || cannot_run "sort failed"
[ -s "$products" ] || cannot_run "no product window class in the graph: no window_classname(\"<class>\") literal in the scanned source (the product-class rule needs it; T-065 review 1 #2)"

# The app identifier, for `{identifier}` in a visible-helper class (read only when one has it).
identifier=""
app_identifier() {
  [ -n "$identifier" ] && return 0
  command -v python3 >/dev/null 2>&1 || cannot_run "python3 not found (reads src-tauri/tauri.conf.json identifier)"
  identifier="$(python3 -c 'import json,sys; v=json.load(open(sys.argv[1],encoding="utf-8")).get("identifier"); sys.exit(1) if not isinstance(v,str) or not v else print(v)' src-tauri/tauri.conf.json)" \
    || cannot_run "no string identifier in src-tauri/tauri.conf.json"
}

# The manifest: four trimmed fields per entry.
found=""
listed="$tmp/listed.txt"
: >"$listed"
trim() { local s="$1"; s="${s#"${s%%[![:space:]]*}"}"; printf '%s' "${s%"${s##*[![:space:]]}"}"; }
declare -A entries=()
declare -A bindings=()
n=0
while IFS= read -r line || [ -n "$line" ]; do
  n=$((n + 1))
  line="${line%$'\r'}"
  t="$(trim "$line")"
  case "$t" in '' | \#*) continue ;; esac
  IFS='|' read -r f1 f2 f3 f4 extra <<<"$t"
  key="$(trim "$f1")"; verdict="$(trim "${f2:-}")"; class="$(trim "${f3:-}")"
  cite="$(trim "${f4:-}")"
  base="${key%%\[*}"
  feats=""
  [ "$base" != "$key" ] && feats="${key#"$base"}"
  bad=""
  if [ -n "${extra:-}" ] || [ -z "$cite" ]; then
    bad="want four fields <key> | <verdict> | <class or -> | <citation>"
  elif [[ ! "$base" =~ ^[A-Za-z0-9_-]+@[^][[:space:]@]+$ ]]; then
    bad="the first field is not <crate>@<version> or <crate>@<version>[<features>]"
  elif [ -n "$feats" ] && [[ ! "$feats" =~ ^\[[^],[:space:][]+(,[^],[:space:][]+)*\]$ ]]; then
    bad="the feature list '$feats' is not [<f1>,<f2>,...] (no spaces, no empty feature; no features = no brackets)"
  elif [ -n "$feats" ] && [ "$feats" != "[$(sort_features "${feats:1:${#feats}-2}")]" ]; then
    bad="the feature list '$feats' is not sorted bytewise without duplicates (want [$(sort_features "${feats:1:${#feats}-2}")])"
  else
    case "$verdict" in
      visible-helper | hidden-helper) [ -n "$class" ] && [ "$class" != "-" ] || bad="a $verdict entry names its window class (pattern, {identifier} allowed), not '-'" ;;
      child) [ "$class" = "-" ] || bad="a $verdict entry gives '-' as its class" ;;
      binding)
        if [ "$class" != "-" ]; then bad="a binding entry gives '-' as its class"
        elif [ -n "$feats" ]; then bad="a binding entry is keyed by <crate>@<version> only, without a feature list (a bindings crate creates no window under any feature)"
        fi ;;
      *) bad="verdict '$verdict' is not one of visible-helper, hidden-helper, child, binding" ;;
    esac
  fi
  if [ -z "$bad" ] && [ -n "${entries[$key]:-}" ]; then
    bad="a second entry for $key (first on line ${entries[$key]})"
  fi
  [ -n "$key" ] && [ -z "${entries[$key]:-}" ] && entries[$key]=$n
  [ "$verdict" = binding ] && [ -n "$base" ] && bindings[$base]=1
  [ -n "$bad" ] && found="$found  malformed: ${key:-<no crate>} ($manifest line $n: $bad)"$'\n'
  [ -n "$key" ] && echo "$key" >>"$listed"
  # F-003 direction: a visible helper is excluded by the smoke; a product window never is.
  if [ -z "$bad" ] && [ "$verdict" = visible-helper ]; then
    resolved="$class"
    if [[ "$class" == *"{identifier}"* ]]; then
      app_identifier
      resolved="${class//\{identifier\}/$identifier}"
    fi
    if grep -qxF -- "$resolved" "$products"; then
      found="$found  product class: $key lists '$resolved' as a visible helper, but it is a product window class of the graph (a window_classname(\"$resolved\") literal): the smoke would exclude a product window, e.g. a click-through overlay shown at start (F-003); a product window is never a helper entry"$'\n'
    elif [ $? -ne 1 ]; then
      cannot_run "grep failed on $products"
    fi
  fi
done <"$manifest"
LC_ALL=C sort -u "$listed" -o "$listed" || cannot_run "sort failed"

# The graph's keys: a binding crate by <crate>@<version>, any other by its features too.
matched="$tmp/matched.txt"
: >"$matched"
while read -r base feats; do
  [ -n "$base" ] || continue
  if [ -n "${bindings[$base]:-}" ] || [ "$feats" = "[]" ]; then
    echo "$base"
  else
    echo "$base$feats"
  fi
done <"$matched_raw" >"$matched"
LC_ALL=C sort -u "$matched" -o "$matched" || cannot_run "sort failed"

unlisted="$(LC_ALL=C comm -23 "$matched" "$listed")" || cannot_run "comm failed"
stale="$(LC_ALL=C comm -13 "$matched" "$listed")" || cannot_run "comm failed"
while IFS= read -r c; do
  [ -n "$c" ] && found="$found  unlisted: $c (its source mentions CreateWindowEx and $manifest has no entry with this crate, version and feature set: read where it creates windows and add the entry with its verdict)"$'\n'
done <<<"$unlisted"
while IFS= read -r c; do
  [ -n "$c" ] && found="$found  stale: $c (listed, but not a crate of the graph whose source mentions CreateWindowEx with this version and feature set: removed, bumped, its features changed or no longer creating windows; re-read and update or drop the entry)"$'\n'
done <<<"$stale"

if [ -n "$found" ]; then
  {
    echo "$me: FAIL: $manifest does not match the window-creating crates of the shell's Windows dependency graph."
    echo "The install smoke excludes a framework window only by a visible-helper class of this manifest (AND the four"
    echo "helper ex-style bits); a crate@version[features] that creates windows must be read and given a verdict before"
    echo "it ships, and a product window class is never a visible helper."
    echo "Why and what to do: docs/decisions/overlay.md §5 (T-065, F-008)"
    printf '%s' "$found"
  } >&2
  exit 1
fi
count="$(wc -l <"$matched")" || cannot_run "wc failed"
echo "$me: ok: $((count)) window-creating crate(s) in the graph, each with one manifest entry in $manifest; no product window class ($(paste -sd, "$products")) listed as a visible helper"
