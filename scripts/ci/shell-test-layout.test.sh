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
# Tool errors are simulated with a PATH shim (an awk, grep or find that exits non-zero).
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

failed=0
passed=0
declare -A seen=()
# check <case-or-path> <expected exit> <substring the output must contain> [failing tool]
check() {
  local case="$1" want="$2" needle="$3" tool="${4:-}" dir out got path_env
  dir="$fx/$case"
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
check ok-doc-lazy-continuation  0 "ok:"   # indented /// line with no blank doc line before it: paragraph, not code

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

# Cannot run: exit 3.
check does-not-exist            3 "cannot run"
check c-no-src                  3 "cannot run"
check c-no-cargo-toml           3 "cannot run"
check ok-current-shape          3 "cannot run" awk
check ok-current-shape          3 "cannot run" grep
check c-find-fails-fence        3 "cannot run" find   # a find error must not skip the doc scan (the fixture holds a fence)

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
