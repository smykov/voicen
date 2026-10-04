#!/usr/bin/env bash
# T-035 guard (class ci-toolchain, F-001/F-002; docs/decisions/ci-toolchain.md).
# src-tauri/build.rs gives the Windows link configuration of the release exe to the bin and,
# through rustc-link-arg-tests, to every src-tauri/tests/*.rs integration test. No cargo
# selector reaches the other test-exe kinds of the voicen package without also reaching the
# bin, so those kinds must contain no tests, and the lib doctest kind stays off. Doc comments
# are not read (T-038, decision #41): a doc block becomes an exe only through the two cargo
# switches pinned in steps 3 and 4. This check fails when any of the following is present:
#   1. under src-tauri/src, read by a lexer (scripts/ci/shell-test-layout.awk) that drops
#      every comment (doc comments included) and string/char literals, so text inside them
#      never counts:
#      - a test attribute anywhere on a line, with any whitespace or line breaks inside it:
#        #[test], # [test], #[<path>::test], #[test_case(..)] (any attribute whose name
#        starts with `test`), also as an attribute argument of cfg_attr;
#      - a cfg or cfg_attr predicate naming `test`, on one line or split across lines:
#        #[cfg(test)], #![cfg(test)], #[cfg(all(windows, test))], #[cfg_attr(test, ..)];
#      - a block comment, string or attribute left open at the end of a file;
#      - a source rustc compiles from outside the scanned files: #[path = ..] (also inside
#        cfg_attr), the token `include` anywhere in code (include!, renamed by `use`,
#        r#include; also an identifier named `include`, fail-closed), and any symlink
#        under src-tauri/src;
#   2. src-tauri/benches, src-tauri/examples, or [[bench]] / [[example]] in
#      src-tauri/Cargo.toml: bench and example exes;
#   3. in src-tauri/Cargo.toml (raw lines, table headers tracked):
#      - a line-start `path` key (bare or quoted) under a [lib], [[bin]], [[test]], [[bench]]
#        or [[example]] header (a target file the scan does not read); `path` in dependency
#        tables and inline `{ path = .. }` pass;
#      - no line `doctest = false` (spaces and a trailing comment allowed) in the [lib] table,
#        or no [lib] table: plain `cargo test` would build the lib doctests;
#      - any other line containing `doctest` that is not a full-line `#` comment (`true`, a
#        quoted or dotted key, an inline table, the key under another table);
#      - `"""` or `'''` anywhere: a multi-line string could fake a [lib] header for the line
#        tracker;
#      - a line that is not self-contained: once single-line strings (basic with \" and \\
#        escapes, literal without) and then the comment are removed, its [ ] and { } do not
#        nest and close on the line, or a string is left open. An element of a multi-line
#        array or inline table could fake a [lib] header or end a target table early. Fail-
#        closed false positive: an array or inline table split over lines, which TOML accepts;
#        keep arrays and inline tables on one line;
#      - a backslash in a quoted key (a string followed by = or .) or in a table header: an
#        escape could spell path, lib or bin past the tracker;
#      - a line starting with [ that the header pattern does not read;
#   4. `--doc` or `rustdoc` in a *.yml / *.yaml file of the workflows dir, for any package (the
#      scan cannot tell which one a step selects): `cargo test --doc` and
#      `cargo rustdoc -- --test` build the lib doctests whatever `doctest` says (cargo 1.99
#      unit_generator.rs:425-440).
# Not caught (outside what a raw scan can see): tests a proc macro generates from an
# attribute with another name; a dependency's macro that expands to include!; a target path
# set other than by a line-start `path` key under a target header (e.g. a root-level dotted
# `lib.path` or an inline `bin = [{ path = .. }]`); a workflow that reaches `--doc` through a
# script or a .cargo alias. Each is loud on the Windows job, not silent.
# Host bash, find, sort, grep and awk; no toolchain.
#
# Usage: scripts/ci/shell-test-layout.sh [shell-dir [workflows-dir]]
#        (defaults: src-tauri, .github/workflows)
# Exit 0: layout ok. Exit 1: a violation (listed). Exit 3: cannot run (shell dir or workflows
# dir missing, find, sort, grep or awk error); never reported as a pass.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
shell="${1:-src-tauri}"
workflows="${2:-.github/workflows}"
scan=scripts/ci/shell-test-layout.awk
if [ ! -d "$shell/src" ] || [ ! -f "$shell/Cargo.toml" ]; then
  echo "shell-test-layout: cannot run: $shell/src or $shell/Cargo.toml not found" >&2
  exit 3
fi
# A workflows dir that cannot be listed would hide a --doc step: never a pass.
if [ ! -d "$workflows" ] || [ ! -r "$workflows" ] || [ ! -x "$workflows" ]; then
  echo "shell-test-layout: cannot run: workflows dir $workflows not found or not readable" >&2
  exit 3
fi

# Each scan fails closed: a tool error is "cannot run" (exit 3), never a pass.
cannot_run() { echo "shell-test-layout: cannot run: $1" >&2; exit 3; }
[ -f "$scan" ] || cannot_run "$scan not found"
found=""
# add <note> <hits...>: one listed line per non-empty hit, prefixed with <prefix>.
add() {
  local note="$1" prefix="${3:-}" hit
  while IFS= read -r hit; do
    [ -n "$hit" ] && found="$found  $prefix$hit  ($note)"$'\n'
  done <<<"$2"
}

# 1. Test attributes, cfg(test) predicates and outside sources under src (one awk lexer).
# A path with a newline splits into names that do not exist, so awk fails: still exit 3.
# Symlinks are refused rather than followed: find -L would skip a dangling link silently
# and walk a linked dir outside src.
links="$(find "$shell/src" -type l)" || cannot_run "find failed on $shell/src"
links="$(LC_ALL=C sort <<<"$links")" || cannot_run "sort failed on the symlink list of $shell/src"
add "a symlink: rustc compiles its target, which the guard does not scan; keep a regular file under src" "$links"
list="$(find "$shell/src" -name '*.rs' -type f)" || cannot_run "find failed on $shell/src"
list="$(LC_ALL=C sort <<<"$list")" || cannot_run "sort failed on the file list of $shell/src"
files=()
[ -n "$list" ] && mapfile -t files <<<"$list"
if [ "${#files[@]}" -gt 0 ]; then
  hits="$(LC_ALL=C awk -f "$scan" "${files[@]}")" || cannot_run "awk failed on $shell/src"
  while IFS= read -r hit; do
    [ -n "$hit" ] && found="$found  $hit"$'\n'
  done <<<"$hits"
fi

# 2. Bench and example targets. grep: 0 = hits, 1 = none, 2 = error.
for d in benches examples; do
  [ -e "$shell/$d" ] && add "bench or example exes are not configured by build.rs" "$shell/$d/"
done
hits="$(grep -nE '^[[:space:]]*\[\[[[:space:]]*(bench|example)[[:space:]]*\]\]' "$shell/Cargo.toml")"
[ $? -le 1 ] || cannot_run "grep failed on $shell/Cargo.toml"
add "a bench or example target: its exe is not configured by build.rs" "$hits" "$shell/Cargo.toml:"

# 3. src-tauri/Cargo.toml, raw lines with table-header tracking, in one awk. Each finding is
# tagged with its rule: P target path, D a doctest line other than the pin, M a multi-line
# string, B a line that is not self-contained, K a backslash in a quoted key or a header,
# H a line starting with [ that is not a plain header, L a [lib] table without the pin, N no
# [lib] table.
# The tracker reads TOML's lines exactly only when every line is self-contained and every key
# literal, so B, K, H and M make that so, fail-closed (T-038 review round 1, finding 1):
# - B: each line is scanned left to right; single-line basic strings ("..", with \" and \\
#   escapes) and literal strings ('..', no escapes) are removed, then the comment from the
#   first # left. The [ ] and { } left must nest and close on the line, and no string may be
#   left open. So no element of a multi-line array or inline table can pose as a header (a
#   fake [lib]) or end a target table early. The false positive: an array or inline table
#   split over lines is refused although TOML accepts it; keep them on one line.
# - K: a quoted key (a string followed by = or .) or a header line holding a backslash; an
#   escape such as "p\u0061th" or [["b\u0069n"]] spells path or bin past the tracker.
# - H: with B, a line starting with [ can only be a header, so one the header pattern does not
#   read is refused rather than leaving the tracker in the previous table.
# A line counts as a header only when it is one; dependency tables (`[dependencies.x]`,
# `[target.'cfg(..)'.dev-dependencies.x]`) pass.
# Target paths: a line-start `path` key (bare or quoted) under a [lib], [[bin]], [[test]],
# [[bench]] or [[example]] header (spaces and quotes allowed inside the brackets).
# The doctest pin: the line `doctest = false` inside [lib]. Any other line naming `doctest`
# that is not a full-line comment is refused, and so is any `"""` or `'''`, since a
# multi-line string could hold a fake [lib] header and pin.
hits="$(LC_ALL=C awk '
  BEGIN { key = "([A-Za-z0-9_-]+|\"[^\"]*\"|\047[^\047]*\047)"
          hdr = "^[ \t]*\\[\\[?[ \t]*" key "([ \t]*\\.[ \t]*" key ")*[ \t]*\\]\\]?[ \t]*(#.*)?$"
          pin = "^[ \t]*doctest[ \t]*=[ \t]*false[ \t]*(#.*)?$" }
  { sub(/\r$/, "") }
  index($0, "\"\"\"") || index($0, "\047\047\047") { print "M" FNR ":" $0 }
  { code = ""; open = 0; esckey = 0; n = length($0); i = 1
    while (i <= n) {
      c = substr($0, i, 1)
      if (c == "#") break
      if (c != "\"" && c != "\047") { code = code c; i++; continue }
      q = c; str = ""; closed = 0; i++
      while (i <= n) {
        c = substr($0, i, 1)
        if (q == "\"" && c == "\\") { str = str substr($0, i, 2); i += 2; continue }
        i++
        if (c == q) { closed = 1; break }
        str = str c
      }
      if (!closed) { open = 1; break }
      after = substr($0, i); sub(/^[ \t]+/, "", after); c = substr(after, 1, 1)
      if (index(str, "\\") && (c == "=" || c == ".")) esckey = 1
      code = code " "
    }
    stack = ""; bad = open
    for (j = 1; j <= length(code) && !bad; j++) {
      c = substr(code, j, 1)
      if (c == "[" || c == "{") stack = stack c
      else if (c == "]" || c == "}") {
        top = substr(stack, length(stack), 1)
        if ((c == "]" && top != "[") || (c == "}" && top != "{")) bad = 1
        else stack = substr(stack, 1, length(stack) - 1)
      }
    }
    if (bad || stack != "") print "B" FNR ":" $0
    header = (code ~ /^[ \t]*\[/)
    if (esckey || (header && index(substr($0, 1, i - 1), "\\"))) print "K" FNR ":" $0
    if (header && $0 !~ hdr) print "H" FNR ":" $0 }
  $0 ~ hdr { t = $0; sub(/#.*/, "", t); gsub(/[][ \t"\047]/, "", t)
             target = (t == "lib" || t == "bin" || t == "test" || t == "bench" || t == "example")
             inlib = (t == "lib"); if (inlib) { libs++; libline = FNR; libtext = $0 }; next }
  target && /^[ \t]*("path"|\047path\047|path)[ \t]*=/ { print "P" FNR ":" $0 }
  /doctest/ && !/^[ \t]*#/ { if (inlib && $0 ~ pin) pinned = 1; else print "D" FNR ":" $0 }
  END { if (!libs) print "N"; else if (!pinned) print "L" libline ":" libtext }
' "$shell/Cargo.toml")" || cannot_run "awk failed on $shell/Cargo.toml"
cargo="$shell/Cargo.toml"
while IFS= read -r hit; do
  case "$hit" in
    P*) add "a target path under [lib], [[bin]], [[test]], [[bench]] or [[example]]: rustc compiles a file the guard does not scan; keep the default src/lib.rs, src/main.rs and tests/<name>.rs, without a path key" "${hit#P}" "$cargo:" ;;
    D*) add "a doctest key other than the line \`doctest = false\` in [lib]: keep exactly that one line" "${hit#D}" "$cargo:" ;;
    M*) add "a multi-line string: it could hide or fake a [lib] header; write the value on one line" "${hit#M}" "$cargo:" ;;
    B*) add "a line that is not self-contained ([ ] or { } do not close on the line once single-line strings and the comment are removed, or a string is left open): an array or inline table split over lines can fake or hide a table header for the line tracker; keep arrays and inline tables on one line" "${hit#B}" "$cargo:" ;;
    K*) add "a backslash in a quoted key or a table header: an escape can spell path, lib or bin past the line tracker; write keys and headers without escapes" "${hit#K}" "$cargo:" ;;
    H*) add "a line starting with [ that is not a plain table header: the line tracker cannot tell which table follows; write [name] or [[name]] with bare or quoted dotted keys" "${hit#H}" "$cargo:" ;;
    L*) add "[lib] has no line \`doctest = false\`: plain cargo test would build the lib doctests, an exe build.rs does not configure" "${hit#L}" "$cargo:" ;;
    N) add "no [lib] table with \`doctest = false\`: the lib (src/lib.rs) keeps doctests on, an exe build.rs does not configure" "$cargo: no [lib] table" ;;
  esac
done <<<"$hits"

# 4. The workflows dir: `--doc` or `rustdoc` (cargo test --doc, cargo rustdoc -- --test)
# builds the lib doctests even with `doctest = false`. Refused for every package: a raw scan
# cannot tell which package a step selects. GitHub reads *.yml and *.yaml at the
# top of the dir; an empty dir has nothing to scan. /dev/null makes grep name the file even
# when there is only one. grep: 0 = hits, 1 = none, 2 = error.
shopt -s nullglob
wfiles=("$workflows"/*.yml "$workflows"/*.yaml)
shopt -u nullglob
if [ "${#wfiles[@]}" -gt 0 ]; then
  hits="$(grep -nE -e '--doc([^A-Za-z0-9_-]|$)|rustdoc' -- "${wfiles[@]}" /dev/null)"
  [ $? -le 1 ] || cannot_run "grep failed on $workflows"
  add "--doc or rustdoc in a workflow step, for any package, since the scan cannot tell which package a step selects: for voicen it builds the lib doctests whatever [lib] doctest says, an exe build.rs does not configure; test the shell with cargo test -p voicen, and other packages' doctests run in cargo test --workspace --exclude voicen" "$hits"
fi

if [ -n "$found" ]; then
  {
    echo "shell-test-layout: FAIL: shell tests belong only in $shell/tests/*.rs (or, for platform-independent logic, in crates/voicen-core)."
    echo "Only integration tests get the release exe's Windows link configuration from $shell/build.rs;"
    echo "any other test exe of the voicen package links or starts only partly on Windows CI (F-001, F-002)."
    echo "Lib doctests stay off: [lib] doctest = false in $shell/Cargo.toml, and no --doc or rustdoc in $workflows."
    echo "Why and what to do instead: docs/decisions/ci-toolchain.md"
    printf '%s' "$found"
  } >&2
  exit 1
fi
echo "shell-test-layout: ok: no tests in $shell/src (no test attributes or cfg(test)), no sources outside it, no benches or examples, [lib] doctest = false, no --doc or rustdoc in $workflows"
