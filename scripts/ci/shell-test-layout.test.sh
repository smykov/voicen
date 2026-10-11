#!/usr/bin/env bash
# Guard of the guard for scripts/ci/shell-test-layout.sh (make check-shell-layout), and proof of
# the compiler switch it pins (T-035, T-038, T-092; F-002; docs/decisions/ci-toolchain.md).
#
# T-092 (P-020 rewrite; supersedes the T-035/T-036/T-038 fixture set): CONTRACT CHANGE, not a
# weakened case. The guard no longer reads Rust, TOML or workflow text (the awk lexer, the TOML
# line tracker, the benches/examples greps and the workflow --doc/rustdoc grep are retired: R1, a
# macro-built #[cfg(test)] #[test], and L1, `cargo test --d"oc"`, passed them). It decides on
#   (a) cargo's own target list: `cargo metadata --no-deps --offline --format-version 1
#       --manifest-path <shell>/Cargo.toml` in the core image (scripts/tw-run core), the voicen
#       package's targets projected to kind, name, src_path (relative to the shell dir), test,
#       doctest, without kind-test rows (every integration test gets rustc-link-arg-tests),
#       compared byte-exactly with the owned scripts/ci/shell-targets.txt (build.rs custom-build,
#       lib voicen_lib src/lib.rs test=false doctest=false, bin voicen src/main.rs test=false
#       doctest=false); JSON read with host `python3 -I` (json module);
#   (b) line 1 of each crate root cargo reports, compared byte-exactly with an owned line:
#       lib `#![cfg(not(any(test, doctest)))]`, bin `#![cfg(not(test))]`. rustc applies the
#       crate-level cfg before name resolution, so every test exe and doctest of lib and bin is
#       empty for every cargo selector and every spelling of it (part 1 below proves it).
# The ~78 old fixture dirs (test attributes, cfg(test) shapes, lexer states, include!/#[path]/
# symlinks, Cargo.toml grammar whitelist, workflow --doc/rustdoc) were removed on purpose: each
# old guarantee now lives in the pin or in cargo's target list (T-092 Investigation §5):
#   test attributes, cfg(test), R1, include!, #[path], symlinks under src -> the crate cfg
#     empties them in every test mode (ok-r1-l1 passes; part 1 shows they never run);
#   [lib]/[[bin]] path                -> v-lib-path-moved, v-bin-path-moved (src_path in the set);
#   benches, examples, extra targets  -> v-bench-dir, v-example-target, v-second-bin (any target
#     cargo reports that is not in the owned set);
#   doctest = false / TOML shapes     -> v-doctest-on (cargo's own doctest value; cargo parses);
#   --doc / rustdoc in workflows (L1) -> the lib pin: 0 doctests for any spelling (part 1);
#   [[test]] path                     -> allowed now (ok-more-integration-tests): kind test gets
#     the -tests link args wherever its file lives;
#   cannot run on tool errors         -> kept: docker, cargo metadata or python3 failing is exit 3.
#
# Part 1, mechanism (needs the core image): the template scripts/ci/fixtures/shell-test-layout/
# mechanism/ is copied under target/ and its crate roots are written as <pin> + *.rs.body, where
# <pin> is line 1 of the ok-current-shape fixture's crate root (the line the guard must accept).
# The bodies plant R1, a plain #[test], a crate and an item doctest, each panicking with
# T092-PLANTED-*. With the pins, cargo test, --lib, --bins, --all-targets, --doc, L1 verbatim
# (`cargo test --d"oc"`) and `cargo rustdoc --lib -- --test` exit 0, print no planted text, and
# the selected exes run 0 tests. Without line 1 (bite): the planted panics show.
# Part 2, the real shell: line 1 of src-tauri/src/lib.rs and src-tauri/src/main.rs is the pin
# part 1 proved, and the guard on src-tauri exits 0.
# Part 3, guard cases (fixture dirs under scripts/ci/fixtures/shell-test-layout/, each a cargo
# package mimicking src-tauri with its own [workspace]; the guard gets the repo-relative dir):
#   ok-*  allowed: exit 0, the guard prints "ok:".
#   v-*   one violation each: exit 1, the listing names the offending target or crate root.
#   c-*   cannot run: exit 3, the guard says "cannot run" (never a pass). Tool errors are
#         simulated with TW_DOCKER (scripts/tw-run's docker binary) and a python3 shim that
#         fails on `python3 -I` calls only (scripts/tw-run's own python3 call has no -I).
# Every fixture dir must appear in the tables, so a case cannot be dropped silently.
#
# Usage: scripts/ci/shell-test-layout.test.sh   (host bash; parts 1-3 reach the core image)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
guard=scripts/ci/shell-test-layout.sh
fx=scripts/ci/fixtures/shell-test-layout
mech="$fx/mechanism"
[ -d "$fx" ] || { echo "shell-test-layout.test: cannot run: $fx not found" >&2; exit 3; }
[ -f "$mech/Cargo.toml" ] || { echo "shell-test-layout.test: cannot run: $mech/Cargo.toml not found" >&2; exit 3; }
shims="$(mktemp -d)" || { echo "shell-test-layout.test: cannot run: mktemp failed" >&2; exit 3; }
# Work dirs cargo must see live in the repo (scripts/tw-run mounts only the repo): target/ is ignored.
mkdir -p target || { echo "shell-test-layout.test: cannot run: mkdir target failed" >&2; exit 3; }
work="$(mktemp -d "$root/target/shell-test-layout.XXXXXX")" \
  || { echo "shell-test-layout.test: cannot run: mktemp in target/ failed" >&2; exit 3; }
work_rel="${work#"$root"/}"
trap 'rm -rf "$shims" "$work"' EXIT

failed=0
passed=0
declare -A seen=()
seen[mechanism]=1
fail() { echo "FAIL $1" >&2; [ -n "${2:-}" ] && sed 's/^/       | /' <<<"$2" >&2; failed=$((failed + 1)); }
pass() { echo "ok   $1"; passed=$((passed + 1)); }

# ---------------------------------------------------------------------------------------------
# Part 1: the mechanism. The pins come from the ok fixture, so the line proved here is the line
# the guard accepts (part 3: ok-current-shape exit 0, v-pin-* exit 1).
lib_pin="$(head -n1 "$fx/ok-current-shape/src/lib.rs")"
bin_pin="$(head -n1 "$fx/ok-current-shape/src/main.rs")"
# make_crate <dir> <lib line 1 or empty> <bin line 1 or empty>
make_crate() {
  mkdir -p "$1/src" "$1/tests" && cp "$mech/Cargo.toml" "$1/" && cp "$mech/tests/it.rs" "$1/tests/" || return 1
  { [ -n "$2" ] && printf '%s\n' "$2"; cat "$mech/lib.rs.body"; } >"$1/src/lib.rs" || return 1
  { [ -n "$3" ] && printf '%s\n' "$3"; cat "$mech/main.rs.body"; } >"$1/src/main.rs"
}
make_crate "$work/pinned" "$lib_pin" "$bin_pin" && make_crate "$work/unpinned" "" "" \
  || { echo "shell-test-layout.test: cannot run: could not build the mechanism crates in $work" >&2; exit 3; }
# incrate <variant> <command string>: run in the core image inside the variant's crate (-j2, offline).
incrate() {
  scripts/tw-run core -- "cd $work_rel/$1 && CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/work/$work_rel/target-$1 $2" 2>&1
}
# The pinned crate builds and its bin runs unchanged (the pin is a no-op outside test/doctest).
out="$(incrate pinned 'cargo build && ../target-pinned/debug/voicen && echo T092-BIN-EXIT-0')"
if ! grep -qF T092-BIN-EXIT-0 <<<"$out"; then
  fail "mechanism: the pinned crate does not build or its bin does not run (core image: make core-image)" "$out"
else
  pass "mechanism: pinned crate builds, bin runs"
fi
# pinned <label> <command> [needle that must appear]: exit 0, no planted text, needle present.
pinned() {
  local out got
  out="$(incrate pinned "$2")"
  got=$?
  if [ "$got" -ne 0 ]; then fail "mechanism pinned '$2': exit $got, want 0" "$out"
  elif grep -qF T092-PLANTED <<<"$out"; then fail "mechanism pinned '$2': a planted test ran" "$(grep -F T092-PLANTED <<<"$out")"
  elif [ -n "${3:-}" ] && ! grep -qF -- "$3" <<<"$out"; then fail "mechanism pinned '$2': output lacks: $3" "$out"
  else pass "mechanism pinned: $1"; fi
}
# unpinned <label> <command> <needle>...: without line 1 the planted panics show (bite).
unpinned() {
  local label="$1" cmd="$2" out got n
  shift 2
  out="$(incrate unpinned "$cmd")"
  got=$?
  if [ "$got" -eq 0 ]; then fail "mechanism unpinned '$cmd': exit 0, want a failing planted test" "$out"; return; fi
  for n in "$@"; do
    grep -qF -- "$n" <<<"$out" || { fail "mechanism unpinned '$cmd': planted $n did not run" "$out"; return; }
  done
  pass "mechanism unpinned (bite): $label"
}
pinned "cargo test (integration test only)" 'cargo test'               "test integration_runs ... ok"
pinned "cargo test --lib"                   'cargo test --lib'         "running 0 tests"
pinned "cargo test --bins"                  'cargo test --bins'        "running 0 tests"
pinned "cargo test --all-targets"           'cargo test --all-targets' "running 0 tests"
pinned "cargo test --doc"                   'cargo test --doc'         "running 0 tests"
pinned "L1 cargo test --d\"oc\" verbatim"   'cargo test --d"oc"'       "running 0 tests"
pinned "cargo rustdoc --lib -- --test"      'cargo rustdoc --lib -- --test' "running 0 tests"
unpinned "--lib runs R1 and the plain test"  'cargo test --lib'         T092-PLANTED-R1 T092-PLANTED-LIB-TEST
unpinned "--bins runs the bin's tests"       'cargo test --bins'        T092-PLANTED-BIN-R1 T092-PLANTED-BIN-TEST
unpinned "--all-targets runs lib and bin"    'cargo test --all-targets --no-fail-fast' T092-PLANTED-R1 T092-PLANTED-BIN-TEST
unpinned "--doc runs both doctests"          'cargo test --doc'         T092-PLANTED-CRATE-DOCTEST T092-PLANTED-ITEM-DOCTEST
unpinned "L1 --d\"oc\" runs the doctests"    'cargo test --d"oc"'       T092-PLANTED-CRATE-DOCTEST T092-PLANTED-ITEM-DOCTEST

# ---------------------------------------------------------------------------------------------
# Part 2: the real shell carries the pins part 1 proved (byte-exact line 1), and the guard holds.
for pair in "src-tauri/src/lib.rs:$lib_pin" "src-tauri/src/main.rs:$bin_pin"; do
  f="${pair%%:*}" want="${pair#*:}"
  got_line="$(head -n1 "$f" 2>&1)"
  if [ "$got_line" = "$want" ]; then pass "real shell: line 1 of $f is the pin"
  else fail "real shell: line 1 of $f is not the pin (want: $want)" "$got_line"; fi
done

# ---------------------------------------------------------------------------------------------
# Part 3: guard cases.
mkdir -p "$shims/py-fail" "$shims/docker-missing-image" "$shims/docker-not-json" \
  || { echo "shell-test-layout.test: cannot run: mkdir in $shims failed" >&2; exit 3; }
real_py="$(command -v python3)" || { echo "shell-test-layout.test: cannot run: python3 not found" >&2; exit 3; }
# python3 shim: fails on `python3 -I ...` (the guard's JSON reader), runs the real python3 otherwise.
printf '#!/bin/sh\ncase " $* " in *" -I "*) echo "python3: simulated failure (py-fail)" >&2; exit 2;; esac\nexec "%s" "$@"\n' \
  "$real_py" >"$shims/py-fail/python3"
printf '#!/bin/sh\necho "Unable to find image '"'"'voicen-rust:1.99'"'"' locally (simulated)" >&2\nexit 125\n' \
  >"$shims/docker-missing-image/docker"
printf '#!/bin/sh\necho "this is not cargo metadata JSON (simulated)"\nexit 0\n' >"$shims/docker-not-json/docker"
chmod +x "$shims/py-fail/python3" "$shims/docker-missing-image/docker" "$shims/docker-not-json/docker"

# check <case-or-path> <expected exit> <substring the output must contain> [env assignment...]
# The guard gets a repo-relative dir: $fx/<case>, or src-tauri itself for the case src-tauri.
check() {
  local case="$1" want="$2" needle="$3" dir out got label
  shift 3
  dir="$fx/$case"
  [ "$case" = src-tauri ] && dir=src-tauri
  seen["$case"]=1
  label="$case"
  if [ $# -gt 0 ]; then local shim="${1#*"$shims"/}"; label="$case (shim ${shim%%[/:]*})"; fi
  if [ ! -x "$guard" ]; then fail "$label: $guard not found or not executable"; return; fi
  out="$(env "$@" "$guard" "$dir" 2>&1)"
  got=$?
  if [ "$got" -ne "$want" ]; then fail "$label: exit $got, want $want" "$out"
  elif ! grep -qF -- "$needle" <<<"$out"; then fail "$label: exit $got as wanted, but the output does not contain: $needle" "$out"
  else pass "$label: exit $got"; fi
}

# The real shell (part 2's pins plus test = false / doctest = false and the [[bin]] table).
check src-tauri                   0 "ok:"

# Allowed: exit 0.
check ok-current-shape            0 "ok:"   # the owned target set and both pins; tests/it.rs
check ok-more-integration-tests   0 "ok:"   # more tests/*.rs, tests/support/, a [[test]] path elsewhere: kind-test rows are not compared
check ok-r1-l1                    0 "ok:"   # R1 + a plain #[cfg(test)] #[test] in src/lib.rs, L1 in workflows/ci.yml: the guard reads neither (part 1)

# Violations: exit 1, the listing names the offending target or crate root.
check v-lib-tested                1 "voicen_lib"         # [lib] without test = false: --lib / plain cargo test build the lib unit-test exe
check v-bin-tested                1 "src/main.rs"        # no [[bin]] test = false: plain cargo test builds the bin unit-test exe
check v-doctest-on                1 "voicen_lib"         # [lib] without doctest = false
check v-pin-missing-lib           1 "src/lib.rs"         # lib.rs line 1 is not the lib pin
check v-pin-missing-main          1 "src/main.rs"        # main.rs line 1 is not the bin pin
check v-pin-not-line-1            1 "src/lib.rs"         # the lib pin on line 2, after a comment
check v-pin-lib-test-only         1 "src/lib.rs"         # lib.rs line 1 is the bin pin #![cfg(not(test))]: doctests not emptied
check v-lib-path-moved            1 "other/lib.rs"       # [lib] path = "other/lib.rs", pinned: not the owned crate root
check v-bin-path-moved            1 "src/app.rs"         # [[bin]] path = "src/app.rs", pinned: not the owned crate root
check v-second-bin                1 "t092_extra"         # src/bin/t092_extra.rs: a tested bin without a pin
check v-example-target            1 "t092_example"       # [[example]] path elsewhere, test = true
check v-bench-dir                 1 "t092_bench"         # benches/t092_bench.rs, auto-discovered
check v-build-rs-missing          1 "build"              # no build.rs: no custom-build target, so no -tests link args (F-002)

# Cannot run: exit 3.
check does-not-exist              3 "cannot run"
check c-no-cargo-toml             3 "cannot run"         # src/ without Cargo.toml
check c-no-src                    3 "cannot run"         # cargo metadata fails: no src/lib.rs for [lib]
check c-bad-manifest              3 "cannot run"         # cargo metadata fails: Cargo.toml does not parse
check ok-current-shape            3 "cannot run" TW_DOCKER="$shims/docker-missing-image/docker"  # core image missing
check ok-current-shape            3 "cannot run" TW_DOCKER="$shims/docker-not-json/docker"       # cargo metadata output is not JSON
check ok-current-shape            3 "cannot run" PATH="$shims/py-fail:$PATH"                      # the JSON reader (python3 -I) fails

# Every committed fixture dir is in the tables.
for d in "$fx"/*/; do
  c="$(basename "$d")"
  [ -n "${seen[$c]:-}" ] || fail "$c: fixture dir has no expected exit code in $0"
done

if [ "$failed" -gt 0 ]; then
  echo "shell-test-layout.test: FAIL: $failed case(s) differ, $passed as expected; the guard or the pin does not hold its contract (docs/decisions/ci-toolchain.md)" >&2
  exit 1
fi
echo "shell-test-layout.test: ok: $passed cases as expected"
