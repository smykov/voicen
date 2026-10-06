#!/usr/bin/env bash
# T-065 tripwire (rca, class smoke-window-predicate, F-008; docs/decisions/overlay.md §5,
# docs/decisions/ci-toolchain.md). The install smoke's window predicate
# (scripts/ci/visible-windows.ps1) excludes a framework helper window only when its class is a
# visible-helper class of the manifest scripts/ci/helper-windows.txt (AND the four helper
# ex-style bits). That manifest is only as good as its coverage of the shell's Windows
# dependency graph, which changes in src-tauri/Cargo.toml and Cargo.lock without touching the
# predicate. This check ties the two together on the Linux host, at the commit that changes a
# dependency: the set of <crate>@<version> of the graph whose source mentions CreateWindowEx
# must equal the set of manifest entries. A new crate or version that creates windows, an entry
# whose crate left the graph or no longer mentions it, and a malformed entry all fail, so each
# is read and given a verdict before any push. The Windows half (the reviewed verdicts against
# the real voicen.exe) is Get-HelperDrift in visible-windows.ps1, run by the install smoke.
#
# Manifest (default scripts/ci/helper-windows.txt): one entry per line, `#` comments and blank
# lines ignored, four fields separated by `|` and trimmed:
#   <crate>@<version> | <verdict> | <class or -> | <source citation>
# verdict: visible-helper | hidden-helper (a class pattern, `{identifier}` = tauri.conf.json
# `identifier`) or child | binding (class `-`). Any other verdict, a helper verdict without a
# class, a child or binding entry with one, an empty citation, a field count other than four or
# a duplicate entry is a violation.
#
# Graph:
#   --graph <file>  one line per package, `<name> <version> <package dir>` (whitespace
#                   separated, the dir may hold spaces; a relative dir is relative to the graph
#                   file's own directory; blank and `#` lines ignored). Each package's src/ is
#                   scanned; a missing package dir or src/ is "cannot run" (unscanned is not
#                   "does not create windows").
#   default         the shell's Windows dependency graph: `cargo metadata --filter-platform
#                   x86_64-pc-windows-msvc` of src-tauri in the core image (scripts/tw-run core,
#                   the registry check-shell-windows resolves), every package reachable from
#                   voicen over normal dependency edges (what voicen.exe links; build and dev
#                   dependencies never run in it), workspace members excluded (our own windows
#                   are never excluded by the predicate, so they need no entry). Each package's
#                   src/ is scanned, or its whole package dir when a lib, proc-macro or bin
#                   target lies outside src/ (a crate with lib.rs at its root). The scan runs in
#                   the core image, where the registry paths cargo reports exist.
# The scan is raw text: `grep -rF CreateWindowEx` (comments count, CreateWindowExA/W both
# match). It errs loud: any mention needs an entry, never a silent pass.
#
# Usage: scripts/ci/helper-windows.sh [--graph <file>] [--manifest <file>]
#        (internal: --scan-roots <file>, lines `<name> <version> <dir>`; prints the
#        <name>@<version> whose dir mentions CreateWindowEx; used by the default mode inside
#        the core image)
# Exit 0: sets equal ("ok:"). Exit 1: drift or a malformed entry (each <crate>@<version>
# listed). Exit 3: cannot run (graph or manifest missing, a package dir or src/ missing, a
# grep, cargo, python3 or docker error); never reported as a pass.
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

# scan_one <name> <version> <dir>: prints <name>@<version> when <dir> mentions CreateWindowEx.
# grep: 0 = a mention, 1 = none, anything else = an error (cannot run, never "none").
scan_one() {
  grep -rqF -- CreateWindowEx "$3"
  case $? in
    0) echo "$1@$2" ;;
    1) ;;
    *) cannot_run "grep failed on $3 ($1@$2)" ;;
  esac
}

# Internal mode: the scan of the default graph, inside the core image.
if [ -n "$scan_roots" ]; then
  [ -f "$scan_roots" ] || cannot_run "scan roots file $scan_roots not found"
  while IFS= read -r line || [ -n "$line" ]; do
    [ -z "${line//[[:space:]]/}" ] && continue
    read -r name version dir <<<"$line"
    [ -n "$dir" ] || cannot_run "malformed scan roots line: $line"
    [ -d "$dir" ] || cannot_run "source dir $dir of $name@$version not found"
    scan_one "$name" "$version" "$dir"
  done <"$scan_roots"
  exit 0
fi

[ -f "$manifest" ] || cannot_run "manifest $manifest not found"

tmp="$(mktemp -d)" || cannot_run "mktemp failed"
trap 'rm -rf "$tmp"' EXIT
matched="$tmp/matched.txt"

if [ -n "$graph" ]; then
  [ -f "$graph" ] || cannot_run "graph file $graph not found"
  gdir="$(cd "$(dirname "$graph")" && pwd)" || cannot_run "cannot resolve the directory of $graph"
  : >"$matched"
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in \#*) continue ;; esac
    [ -z "${line//[[:space:]]/}" ] && continue
    read -r name version dir <<<"$line"
    [ -n "$dir" ] || cannot_run "malformed graph line (want <name> <version> <dir>): $line"
    case "$dir" in /*) ;; *) dir="$gdir/$dir" ;; esac
    [ -d "$dir" ] || cannot_run "package dir $dir of $name@$version not found"
    [ -d "$dir/src" ] || cannot_run "$name@$version has no src/ under $dir"
    scan_one "$name" "$version" "$dir/src" >>"$matched" || exit 3
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
    pkg_dir = os.path.dirname(p["manifest_path"])
    src = os.path.join(pkg_dir, "src")
    paths = [t["src_path"] for t in p["targets"] if code_kinds & set(t["kind"])]
    inside = bool(paths) and all(x.startswith(src + os.sep) for x in paths)
    rows.append((p["name"], p["version"], src if inside else pkg_dir))
for name, version, path in sorted(rows):
    print(name, version, path)
PY
  [ -s "$run/roots.txt" ] || cannot_run "the dependency graph read from cargo metadata is empty"
  rel="${run#"$root"/}"
  if ! scripts/tw-run core -- scripts/ci/helper-windows.sh --scan-roots "$rel/roots.txt" \
    >"$matched" 2>"$run/scan.err"; then
    sed 's/^/  | /' "$run/scan.err" >&2
    cannot_run "the scan of the dependency graph failed in the core image"
  fi
fi
LC_ALL=C sort -u "$matched" -o "$matched" || cannot_run "sort failed"

# The manifest: four trimmed fields per entry.
found=""
listed="$tmp/listed.txt"
: >"$listed"
trim() { local s="$1"; s="${s#"${s%%[![:space:]]*}"}"; printf '%s' "${s%"${s##*[![:space:]]}"}"; }
declare -A entries=()
n=0
while IFS= read -r line || [ -n "$line" ]; do
  n=$((n + 1))
  line="${line%$'\r'}"
  t="$(trim "$line")"
  case "$t" in '' | \#*) continue ;; esac
  IFS='|' read -r f1 f2 f3 f4 extra <<<"$t"
  krate="$(trim "$f1")"; verdict="$(trim "${f2:-}")"; class="$(trim "${f3:-}")"
  cite="$(trim "${f4:-}")"
  bad=""
  if [ -n "${extra:-}" ] || [ -z "$cite" ]; then
    bad="want four fields <crate>@<version> | <verdict> | <class or -> | <citation>"
  elif [[ ! "$krate" =~ ^[A-Za-z0-9_-]+@[^[:space:]@]+$ ]]; then
    bad="the first field is not <crate>@<version>"
  else
    case "$verdict" in
      visible-helper | hidden-helper) [ -n "$class" ] && [ "$class" != "-" ] || bad="a $verdict entry names its window class (pattern, {identifier} allowed), not '-'" ;;
      child | binding) [ "$class" = "-" ] || bad="a $verdict entry gives '-' as its class" ;;
      *) bad="verdict '$verdict' is not one of visible-helper, hidden-helper, child, binding" ;;
    esac
  fi
  if [ -z "$bad" ] && [ -n "${entries[$krate]:-}" ]; then
    bad="a second entry for $krate (first on line ${entries[$krate]})"
  fi
  [ -n "$krate" ] && [ -z "${entries[$krate]:-}" ] && entries[$krate]=$n
  [ -n "$bad" ] && found="$found  malformed: ${krate:-<no crate>} ($manifest line $n: $bad)"$'\n'
  [ -n "$krate" ] && echo "$krate" >>"$listed"
done <"$manifest"
LC_ALL=C sort -u "$listed" -o "$listed" || cannot_run "sort failed"

unlisted="$(LC_ALL=C comm -23 "$matched" "$listed")" || cannot_run "comm failed"
stale="$(LC_ALL=C comm -13 "$matched" "$listed")" || cannot_run "comm failed"
while IFS= read -r c; do
  [ -n "$c" ] && found="$found  unlisted: $c (its source mentions CreateWindowEx and $manifest has no entry: read where it creates windows and add the entry with its verdict)"$'\n'
done <<<"$unlisted"
while IFS= read -r c; do
  [ -n "$c" ] && found="$found  stale: $c (listed, but not a crate of the graph whose source mentions CreateWindowEx: removed, bumped to another version or no longer creating windows; re-read and update or drop the entry)"$'\n'
done <<<"$stale"

if [ -n "$found" ]; then
  {
    echo "$me: FAIL: $manifest does not match the window-creating crates of the shell's Windows dependency graph."
    echo "The install smoke excludes a framework window only by a visible-helper class of this manifest (AND the four"
    echo "helper ex-style bits); a crate@version that creates windows must be read and given a verdict before it ships."
    echo "Why and what to do: docs/decisions/overlay.md §5 (T-065, F-008)"
    printf '%s' "$found"
  } >&2
  exit 1
fi
count="$(wc -l <"$matched")" || cannot_run "wc failed"
echo "$me: ok: $((count)) window-creating crate(s) in the graph, each with one manifest entry in $manifest"
