#!/usr/bin/env bash
# T-035 guard of the guard (review round 1, findings 1 and 2): scripts/ci/shell-test-layout.sh
# must give the documented exit code on every committed fixture shell dir under
# scripts/ci/fixtures/shell-test-layout/<case>/ (each mimics src-tauri: src/, Cargo.toml,
# optional tests/, benches/, examples/). Without this, a weakened regex or awk branch keeps
# `make check` green, because the real src-tauri has no violation to catch.
#
#   ok-*  allowed shapes: exit 0, and the guard prints "ok:".
#   v-*   one violation each: exit 1, and the guard's listing names the offending path.
#   c-*   cannot run: exit 3, and the guard says "cannot run" (never a pass).
# Tool errors are simulated with a PATH shim: an awk, grep or find that always exits
# non-zero, or a selective one that fails only on the call of one scan (its arguments
# match) and runs the real tool otherwise, so the guard gets past the earlier scans and
# reaches that scan's own error branch.
# Every fixture dir must appear in the table below, so a case cannot be dropped silently.
#
# Usage: scripts/ci/shell-test-layout.test.sh
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
guard=scripts/ci/shell-test-layout.sh
fx=scripts/ci/fixtures/shell-test-layout
[ -x "$guard" ] || { echo "shell-test-layout.test: cannot run: $guard not found or not executable" >&2; exit 3; }
[ -d "$fx" ] || { echo "shell-test-layout.test: cannot run: $fx not found" >&2; exit 3; }
shims="$(mktemp -d)" || { echo "shell-test-layout.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$shims"' EXIT
for tool in awk grep find; do
  mkdir -p "$shims/$tool"
  printf '#!/bin/sh\necho "%s: simulated failure" >&2\nexit 2\n' "$tool" >"$shims/$tool/$tool"
  chmod +x "$shims/$tool/$tool"
done
# selective <shim-dir> <tool> <case pattern on "$*">: fail when the arguments match, else exec the real tool.
selective() {
  local real
  real="$(command -v "$2")" || { echo "shell-test-layout.test: cannot run: $2 not found" >&2; exit 3; }
  mkdir -p "$shims/$1"
  printf '#!/bin/sh\ncase "$*" in %s) echo "%s: simulated failure (%s)" >&2; exit 2;; esac\nexec "%s" "$@"\n' \
    "$3" "$2" "$1" "$real" >"$shims/$1/$2"
  chmod +x "$shims/$1/$2"
}
# find-rs: only the `-name '*.rs'` file scan fails; the symlink scan before it runs.
selective find-rs find "*'*.rs'*"
# cargo-path: only the Cargo.toml target-path scan fails, whichever tool reads it: a grep
# whose arguments name Cargo.toml and `path` (the bench/example grep names neither key),
# or an awk reading Cargo.toml (the source lexer reads only *.rs files).
selective cargo-path grep "*Cargo.toml*path*|*path*Cargo.toml*"
selective cargo-path awk "*Cargo.toml*"

failed=0
passed=0
declare -A seen=()
# check <case-or-path> <expected exit> <substring the output must contain> [failing tool] [dir]
# dir: run the guard on this shell dir instead of $fx/<case> (a prepared copy of the case).
check() {
  local case="$1" want="$2" needle="$3" tool="${4:-}" dir="${5:-$fx/$1}" out got path_env
  seen["$case"]=1
  path_env="$PATH"
  [ -n "$tool" ] && path_env="$shims/$tool:$PATH"
  out="$(PATH="$path_env" "$guard" "$dir" 2>&1)"
  got=$?
  local label="$case${tool:+ (failing $tool)}"
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

# Allowed: exit 0.
check ok-current-shape          0 "ok:"   # cfg(windows), cfg(not(windows)), tauri::command, cfg_attr(mobile, ...), tests/*.rs with #[test]
check ok-text-fence             0 "ok:"   # ```text in /// and //!
check ok-feature-test           0 "ok:"   # cfg(feature = "test"), cfg_attr(feature = "test", ...)
check ok-quad-slash             0 "ok:"   # //// fence and indented block: plain comment
check ok-plain-comment-attr     0 "ok:"   # // #[test], // #[cfg(test)], trailing // comment naming them

# Violations: exit 1, the listing names the offending file.
check v-cfg-test                1 "v-cfg-test/src/lib.rs"
check v-inner-cfg-test          1 "v-inner-cfg-test/src/lib.rs"
check v-cfg-all-test            1 "v-cfg-all-test/src/lib.rs"
check v-cfg-attr-test           1 "v-cfg-attr-test/src/lib.rs"
check v-cfg-attr-test-attr      1 "v-cfg-attr-test-attr/src/lib.rs"
check v-test-attr               1 "v-test-attr/src/lib.rs"
check v-test-attr-indented      1 "v-test-attr-indented/src/lib.rs"
check v-path-test-attr          1 "v-path-test-attr/src/lib.rs"
check v-test-case-attr          1 "v-test-case-attr/src/lib.rs"
check v-fence-bare              1 "v-fence-bare/src/lib.rs"
check v-fence-rust              1 "v-fence-rust/src/lib.rs"
check v-fence-tilde             1 "v-fence-tilde/src/lib.rs"
check v-inner-doc-fence         1 "v-inner-doc-fence/src/lib.rs"
check v-doc-include-str         1 "v-doc-include-str/src/lib.rs"
check v-benches-dir             1 "v-benches-dir/benches"
check v-examples-dir            1 "v-examples-dir/examples"
check v-bench-target            1 "v-bench-target/Cargo.toml"
check v-example-target          1 "v-example-target/Cargo.toml"
# Review round 1, finding 1: shapes rustdoc/rustc accept that the guard must also refuse.
check v-doc-indented-block       1 "v-doc-indented-block/src/lib.rs"        # (a) 4-space block after a blank ///
check v-inner-doc-indented-block 1 "v-inner-doc-indented-block/src/lib.rs"  # (a) same in //!
check v-block-doc-fence          1 "v-block-doc-fence/src/lib.rs"           # (b) fence in /** */
check v-block-inner-doc-fence    1 "v-block-inner-doc-fence/src/lib.rs"     # (b) fence in /*! */
check v-doc-attr-fence           1 "v-doc-attr-fence/src/lib.rs"            # (c) #[doc = "```"]
check v-cfg-test-multiline       1 "v-cfg-test-multiline/src/lib.rs"        # (d) #[cfg(\n test\n)]
check v-cfg-all-test-multiline   1 "v-cfg-all-test-multiline/src/lib.rs"    # (d) #[cfg(all(\n windows,\n test\n))]
check v-test-after-code          1 "v-test-after-code/src/lib.rs"           # (e) mod t { #[test] fn a() {} }
check v-cfg-test-after-code      1 "v-cfg-test-after-code/src/lib.rs"       # (e) code; #[cfg(test)] mod t {}
check v-test-hash-space          1 "v-test-hash-space/src/lib.rs"           # (e) # [test]: rustc allows space after #

# T-036 (T-035 review round 2). (1) Doc list and quote shapes. A list continuation or nested
# list at 4+ columns is not a doctest, but it stays refused (the guard tracks no list
# context) with a message saying how to indent it; a list or quote marker followed by 5+
# columns starts an indented code block inside the item or quote: a doctest.
check v-doc-list-continuation     1 "continues a list item"                      # P1 + P2: the note names the list case
check ok-doc-list-continuation    0 "ok:"                                        # continuations at 2 / 3 columns, nested list at 2
check v-doc-list-marker-code      1 "v-doc-list-marker-code/src/lib.rs:3:"       # P3: `-     code`
check v-doc-quote-code            1 "v-doc-quote-code/src/lib.rs:3:"             # P4: `>     code`
# (2) Lexer states: each case changes exit code when its lexer branch is deleted.
check ok-literals                 0 "ok:"                                        # strings, escapes, raw/byte/c-raw strings, chars, lifetimes, nested and /*** comments
check v-char-quote-cfg-test       1 "v-char-quote-cfg-test/src/lib.rs:2:"        # '"' must not open a string that hides #[cfg(test)]
check v-unterminated-string       1 "(unterminated string"
check v-unterminated-block-comment 1 "(unterminated block comment"
check v-unterminated-attribute    1 "(unterminated attribute"
# (3) Sources rustc compiles that are not regular *.rs files under src: refused.
check v-path-attr                 1 "v-path-attr/src/lib.rs:1:"                  # #[path = "../other/t.rs"] mod t;
check v-include-macro             1 "v-include-macro/src/lib.rs:2:"              # include!("../other/t.rs");
check v-lib-path-target           1 "v-lib-path-target/Cargo.toml:10:"           # [lib] path = "other/lib.rs"
# The symlink is made here, in a copy of the case, so the committed fixture tree stays plain files.
copies="$shims/fixture-copies"
mkdir -p "$copies" && cp -R "$fx/v-symlink-src" "$copies/" && ln -s ../other/t.rs "$copies/v-symlink-src/src/s.rs" \
  || { echo "shell-test-layout.test: cannot run: could not prepare the symlink case in $copies" >&2; exit 3; }
check v-symlink-src               1 "v-symlink-src/src/s.rs" "" "$copies/v-symlink-src"  # src/s.rs -> ../other/t.rs

# T-036 review round 1, under decision #39 (coarse fail-closed rule): no lazy-continuation
# or list modelling; every doc line indented 4+ columns (after rustdoc's unindent by the
# block minimum) is refused, and so is a container marker followed by 4+ columns or a tab.
# Finding 1: shapes rustdoc 1.99 runs as doctests that the paragraph model passed.
check v-doc-quote-lazy              1 "v-doc-quote-lazy/src/lib.rs:4:"              # (a) `> Note` / `>` / 4 columns
check v-doc-quote-empty-code        1 "v-doc-quote-empty-code/src/lib.rs:3:"        # (b) `>` alone / 4 columns
check v-doc-empty-item-code         1 "v-doc-empty-item-code/src/lib.rs:3:"         # (c) empty `-` item / 6 columns
check v-doc-empty-ordered-item-code 1 "v-doc-empty-ordered-item-code/src/lib.rs:3:" # (d) empty `1.` item / 7 columns
check v-doc-thematic-break-code     1 "v-doc-thematic-break-code/src/lib.rs:5:"     # (e) `---` / 4 columns
check v-doc-setext-code             1 "v-doc-setext-code/src/lib.rs:4:"             # (f) `Usage` / `=====` / 4 columns
check v-doc-star-break-code         1 "v-doc-star-break-code/src/lib.rs:4:"         # (g) text / `***` / 4 columns
check v-doc-indented-no-space       1 "v-doc-indented-no-space/src/lib.rs:4:"       # `///text` block: unindent by 0, so 4 spaces are code
# Deliberate contract change by #39, not a weakened case: T-035's ok-doc-lazy-continuation
# (a 4+ column line right after paragraph text, a lazy continuation for rustdoc) is now a
# violation; the author rule is to indent continuations by fewer than 4 columns
# (ok-doc-list-continuation above still passes).
check v-doc-lazy-continuation       1 "v-doc-lazy-continuation/src/lib.rs:2:"
# Finding 4: a marker followed by a tab (rustdoc 1.99 runs `-<TAB><TAB>x` as a doctest).
check v-doc-list-marker-tab         1 "v-doc-list-marker-tab/src/lib.rs:2:"
# Finding 2 (+4, 6): the `include` token anywhere in code, not only before `!`; identifiers
# that merely contain it, include_str!/include_bytes!, comments, strings and docs pass.
check v-include-renamed             1 "v-include-renamed/src/lib.rs:1:"             # use core::include as pull; pull!(..)
check v-include-eol                 1 "v-include-eol/src/lib.rs:2:"                 # `include` / `!(..)` on the next line
check v-include-raw-ident           1 "v-include-raw-ident/src/lib.rs:3:"           # r#include!(..): rustc 1.99 runs the outside test
check ok-include-substring          0 "ok:"                                          # included, include_count, INCLUDE_ALL, include_str!
# Finding 3 (+4): Cargo.toml `path` is refused only under [lib], [[bin]], [[test]],
# [[bench]], [[example]] (table-header tracking); dependency tables pass.
check ok-dependency-table-path      0 "ok:"                                          # [dependencies.x] / [dev-dependencies.x] / target deps
check v-lib-quoted-path             1 "v-lib-quoted-path/Cargo.toml:10:"             # [lib] "path" = ..
check v-lib-path-after-dependency   1 "v-lib-path-after-dependency/Cargo.toml:14:"   # [dependencies.x] path, then [ lib ] path = '..'
check v-bin-path-target             1 "v-bin-path-target/Cargo.toml:10:"             # [[bin]] path = ..
check v-test-target-path            1 "v-test-target-path/Cargo.toml:12:"            # [[test]] path = "tests/x.rs"

# Cannot run: exit 3.
check does-not-exist            3 "cannot run"
check c-no-src                  3 "cannot run"
check c-no-cargo-toml           3 "cannot run"
check ok-current-shape          3 "cannot run" awk
check ok-current-shape          3 "cannot run" grep
check c-find-fails-fence        3 "cannot run" find   # a find error must not skip the doc scan (the fixture holds a fence)
# Review round 1, finding 7: the always-failing shims stop at the first find (symlinks) and
# the first grep (benches); these reach the later calls' own error branches.
# The needle is the shim's own message: it appears only if the guard got past the earlier
# calls (run by the real tool) and made the matching call; exit 3 shows that call's error
# branch fails closed (without it: an empty list, so exit 0).
check c-find-fails-fence        3 "find: simulated failure (find-rs)" find-rs  # the *.rs find, after the symlink find
check ok-current-shape          3 "simulated failure (cargo-path)" cargo-path  # the Cargo.toml path scan, after the bench grep

# Every committed fixture dir is in the table.
for d in "$fx"/*/; do
  c="$(basename "$d")"
  if [ -z "${seen[$c]:-}" ]; then
    echo "FAIL $c: fixture dir has no expected exit code in $0" >&2
    failed=$((failed + 1))
  fi
done

if [ "$failed" -gt 0 ]; then
  echo "shell-test-layout.test: FAIL: $failed case(s) differ, $passed as expected; the guard does not hold its contract (docs/decisions/ci-toolchain.md)" >&2
  exit 1
fi
echo "shell-test-layout.test: ok: $passed cases as expected"
