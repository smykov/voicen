#!/usr/bin/env bash
# T-035 guard of the guard (review round 1, findings 1 and 2): scripts/ci/shell-test-layout.sh
# must give the documented exit code on every committed fixture shell dir under
# scripts/ci/fixtures/shell-test-layout/<case>/ (each mimics src-tauri: src/, Cargo.toml,
# optional tests/, benches/, examples/, and workflows/ standing in for .github/workflows).
# The guard gets the case's workflows/ as its 2nd argument, or an empty dir when the case has
# none, so the real .github/workflows never decides a case. Without this, a weakened regex or awk branch keeps
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
# T-038 (rca, supersedes decision #39): the guard no longer reads doc comments; it pins the
# two cargo switches that can turn a doc block into an exe instead ([lib] doctest = false in
# Cargo.toml, no --doc / rustdoc in the workflows). The 25 doc-shape fixture dirs of T-035/T-036
# were removed on purpose, as a contract change and not a weakened case: their sources and
# the b8b0b81 probes live on in ok-doc-text, which must pass, and every remaining Cargo.toml
# carries the pinned `doctest = false` so each case passes or fails for its own reason only.
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
# workflow-grep: only the scan of the workflows dir fails: its arguments name the dir, and no
# other call of the guard does on the case it runs on (ok-current-shape: no `workflows` in
# its shell path).
selective workflow-grep grep "*workflows*"

failed=0
passed=0
declare -A seen=()
# Workflows dir for a case without workflows/: empty, so a clean case passes the workflow scan.
no_workflows="$shims/no-workflows"
mkdir -p "$no_workflows" || { echo "shell-test-layout.test: cannot run: mkdir $no_workflows failed" >&2; exit 3; }
# check <case-or-path> <expected exit> <substring the output must contain> [failing tool] [dir] [workflows]
# dir: run the guard on this shell dir instead of $fx/<case> (a prepared copy of the case).
# workflows: the guard's 2nd argument; default <dir>/workflows if it exists, else an empty dir.
check() {
  local case="$1" want="$2" needle="$3" tool="${4:-}" dir="${5:-$fx/$1}" wf="${6:-}" out got path_env
  seen["$case"]=1
  if [ -z "$wf" ]; then
    wf="$no_workflows"
    [ -d "$dir/workflows" ] && wf="$dir/workflows"
  fi
  path_env="$PATH"
  [ -n "$tool" ] && path_env="$shims/$tool:$PATH"
  out="$(PATH="$path_env" "$guard" "$dir" "$wf" 2>&1)"
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
check ok-current-shape          0 "ok:"   # cfg(windows), cfg(not(windows)), tauri::command, cfg_attr(mobile, ...), tests/*.rs with #[test]; doctest comments, `  doctest = false   # ..`; workflows/ci.yml mirror
check ok-doc-text               0 "ok:"   # T-038: every removed doc-shape fixture and b8b0b81 probe, code-looking text in ///, //!, /** */, /*! */, #[doc = ..], #![doc = include_str!(..)]
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
check v-benches-dir             1 "v-benches-dir/benches"
check v-examples-dir            1 "v-examples-dir/examples"
check v-bench-target            1 "v-bench-target/Cargo.toml"
check v-example-target          1 "v-example-target/Cargo.toml"
# Review round 1, finding 1 (d), (e): attribute shapes rustc accepts that the guard must also refuse.
check v-cfg-test-multiline       1 "v-cfg-test-multiline/src/lib.rs"        # (d) #[cfg(\n test\n)]
check v-cfg-all-test-multiline   1 "v-cfg-all-test-multiline/src/lib.rs"    # (d) #[cfg(all(\n windows,\n test\n))]
check v-test-after-code          1 "v-test-after-code/src/lib.rs"           # (e) mod t { #[test] fn a() {} }
check v-cfg-test-after-code      1 "v-cfg-test-after-code/src/lib.rs"       # (e) code; #[cfg(test)] mod t {}
check v-test-hash-space          1 "v-test-hash-space/src/lib.rs"           # (e) # [test]: rustc allows space after #

# T-036 (T-035 review round 2). (1) Doc list and quote shapes: removed by T-038 (header).
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

# T-036 review round 1. Findings 1 and 4 (doc shapes under decision #39): removed by T-038,
# folded into ok-doc-text (header).
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

# T-038: the doctest switches. (1) src-tauri/Cargo.toml: the [lib] table holds the line
# `doctest = false` (any spaces, a trailing comment allowed; ok-current-shape), no other
# non-comment line names `doctest`, and no multi-line string can fake a table header.
check v-doctest-missing             1 "v-doctest-missing/Cargo.toml"                 # [lib] without the key: cargo test runs lib doctests
check v-doctest-true                1 "v-doctest-true/Cargo.toml:12:"                # [lib] doctest = true
check v-doctest-no-lib-table        1 "v-doctest-no-lib-table/Cargo.toml"            # no [lib]: the implicit src/lib.rs lib has doctests on
check v-doctest-other-table         1 "v-doctest-other-table/Cargo.toml"             # doctest = false under [[bin]], [lib] has none
check v-doctest-extra-key           1 "v-doctest-extra-key/Cargo.toml:17:"           # [lib] pinned, plus "doctest" = true under [[test]]
check v-cargo-multiline-string      1 "v-cargo-multiline-string/Cargo.toml:9:"       # description = """ [lib] doctest = false """, no real [lib]
check v-cargo-multiline-literal     1 "v-cargo-multiline-literal/Cargo.toml:9:"      # same with ''' (literal string)
# (2) The workflows dir (2nd argument; .github/workflows by default): no `--doc` and no
# `rustdoc`, which build lib doctests whatever `doctest` says (cargo 1.99 unit_generator.rs:425-440).
check v-workflow-doc                1 "v-workflow-doc/workflows/ci.yml:16:"          # cargo test -p voicen --doc
check v-workflow-rustdoc            1 "v-workflow-rustdoc/workflows/release.yaml:14:" # cargo rustdoc -p voicen -- --test, in a *.yaml file

# Cannot run: exit 3.
check does-not-exist            3 "cannot run"
check c-no-src                  3 "cannot run"
check c-no-cargo-toml           3 "cannot run"
check ok-current-shape          3 "cannot run" awk
check ok-current-shape          3 "cannot run" grep
check c-find-fails-cfg-test     3 "cannot run" find   # a find error must not skip the source scan (the fixture holds #[cfg(test)])
# T-038: a missing workflows dir cannot be scanned, so it is never a pass (the shell is clean).
check c-no-workflows-dir        3 "cannot run" "" "$fx/c-no-workflows-dir" "$fx/c-no-workflows-dir/workflows"
# Review round 1, finding 7: the always-failing shims stop at the first find (symlinks) and
# the first grep (benches); these reach the later calls' own error branches.
# The needle is the shim's own message: it appears only if the guard got past the earlier
# calls (run by the real tool) and made the matching call; exit 3 shows that call's error
# branch fails closed (without it: an empty list, so exit 0).
check c-find-fails-cfg-test     3 "find: simulated failure (find-rs)" find-rs  # the *.rs find, after the symlink find
check ok-current-shape          3 "simulated failure (cargo-path)" cargo-path  # the Cargo.toml path scan, after the bench grep
check ok-current-shape          3 "simulated failure (workflow-grep)" workflow-grep  # T-038: the workflows scan, after the Cargo.toml scans

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
