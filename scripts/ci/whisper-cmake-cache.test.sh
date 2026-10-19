#!/usr/bin/env bash
# T-099: host test of scripts/ci/whisper-cmake-cache.sh, the windows job's verdict on every
# whisper-rs-sys CMakeCache.txt under target/ (decisions #142, invariant 3), on fixture trees
# built in a temp dir (offline). Cases:
#   good           debug + release caches with the #142 lines -> exit 0, "2 CMakeCache.txt";
#   crlf           the good cache with CRLF line ends -> exit 0;
#   absent-dl      good without GGML_BACKEND_DL / GGML_CPU_ALL_VARIANTS entries -> exit 0;
#   none           no CMakeCache.txt at all -> exit 1;
#   other-crate    a CMakeCache.txt of another crate only -> exit 1 (not counted);
#   no-dir         a target dir that does not exist -> exit 1;
#   native-on      GGML_NATIVE:BOOL=ON -> exit 1 naming GGML_NATIVE;
#   native-absent  no GGML_NATIVE entry -> exit 1;
#   native-string  GGML_NATIVE:STRING=OFF -> exit 1;
#   native-lower   GGML_NATIVE:BOOL=off -> exit 1;
#   avx2-off       GGML_AVX2:BOOL=OFF -> exit 1 naming GGML_AVX2;
#   avx2-both      the good cache plus a second, unowned GGML_AVX2:BOOL=OFF line -> exit 1;
#   avx512-on      GGML_AVX512:BOOL=ON -> exit 1 naming GGML_AVX512;
#   avx512-sub     an extra GGML_AVX512_VBMI:BOOL=ON line -> exit 0 (another option, whole word);
#   shared-on      BUILD_SHARED_LIBS:BOOL=ON -> exit 1;
#   dl-on          GGML_BACKEND_DL:BOOL=ON -> exit 1;
#   variants-on    GGML_CPU_ALL_VARIANTS:BOOL=ON -> exit 1;
#   indented       a leading blank before GGML_BACKEND_DL:BOOL=ON -> exit 1;
#   duplicate      GGML_NATIVE:BOOL=OFF twice -> exit 1;
#   one-bad        debug good, release with GGML_NATIVE:BOOL=ON -> exit 1 naming the release file;
#   new-desc       an unowned description naming GGML_BACKEND_DL -> exit 1;
#   usage          no argument, two arguments -> exit 2.
# Usage: scripts/ci/whisper-cmake-cache.test.sh   (host bash)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
script="$root/scripts/ci/whisper-cmake-cache.sh"
[ -x "$script" ] || { echo "whisper-cmake-cache.test: cannot run: $script not found or not executable" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "whisper-cmake-cache.test: cannot run: mktemp" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

failed=0
passed=0
fail() { echo "FAIL $label: $1" >&2; sed 's/^/       | /' <<<"$out" >&2; failed=$((failed + 1)); ok=0; }
run() { label="$1"; ok=1; shift; out="$("$script" "$@" 2>&1)"; rc=$?; }
want_rc() { [ "$rc" = "$1" ] || fail "exit $rc, want $1"; }
has() { grep -qE -- "$1" <<<"$out" || fail "output lacks /$1/"; }
lacks() { ! grep -qE -- "$1" <<<"$out" || fail "output has /$1/"; }
done_case() { [ "$ok" = 1 ] && { echo "ok   $label"; passed=$((passed + 1)); }; }

# A cache shaped like cmake's own (description line, entry), whisper.cpp 1.8.3 option texts.
good_cache() {
  cat <<'EOF'
# This is the CMakeCache file.
# For build in directory: C:/a/voicen/voicen/target/debug/build/whisper-rs-sys-0123456789abcdef/out/build

########################
# EXTERNAL cache entries
########################

//build shared libraries
BUILD_SHARED_LIBS:BOOL=OFF

//ggml: enable AVX2
GGML_AVX2:BOOL=ON

//ggml: enable AVX512F
GGML_AVX512:BOOL=OFF

//ggml: enable AVX512-VBMI
GGML_AVX512_VBMI:BOOL=OFF

//ggml: build backends as dynamic libraries (requires BUILD_SHARED_LIBS)
GGML_BACKEND_DL:BOOL=OFF

//ggml: build all variants of the CPU backend (requires GGML_BACKEND_DL)
GGML_CPU_ALL_VARIANTS:BOOL=OFF

//ggml: optimize the build for the current system
GGML_NATIVE:BOOL=OFF

//No help, variable specified on the command line.
GGML_SSE42:UNINITIALIZED=ON

########################
# INTERNAL cache entries
########################

CMAKE_CACHEFILE_DIR:INTERNAL=C:/a/voicen/voicen/target/debug/build/whisper-rs-sys-0123456789abcdef/out/build
EOF
}

# tree <name> <debug cache text or -> [<release cache text>]: a target dir with whisper-rs-sys
# caches at cmake-rs's OUT_DIR/build/CMakeCache.txt.
tree() {
  local t="$tmp/$1"
  mkdir -p "$t"
  if [ "$2" != - ]; then
    mkdir -p "$t/debug/build/whisper-rs-sys-0123456789abcdef/out/build"
    printf '%s\n' "$2" > "$t/debug/build/whisper-rs-sys-0123456789abcdef/out/build/CMakeCache.txt"
  fi
  if [ "$#" -ge 3 ]; then
    mkdir -p "$t/release/build/whisper-rs-sys-fedcba9876543210/out/build"
    printf '%s\n' "$3" > "$t/release/build/whisper-rs-sys-fedcba9876543210/out/build/CMakeCache.txt"
  fi
  echo "$t"
}

g="$(good_cache)"
# swap <from line> <to line>: the good cache with one whole line replaced.
swap() { awk -v a="$1" -v b="$2" '$0 == a { print b; next } { print }' <<<"$g"; }
drop() { awk -v a="$1" '$0 != a { print }' <<<"$g"; }

run good "$(tree good "$g" "$g")"
want_rc 0; has '^whisper-cmake-cache: ok: 2 CMakeCache.txt checked$'; lacks '::error::'; done_case

t="$(tree crlf -)"; mkdir -p "$t/debug/build/whisper-rs-sys-1/out/build"
sed 's/$/\r/' <<<"$g" > "$t/debug/build/whisper-rs-sys-1/out/build/CMakeCache.txt"
run crlf "$t"
want_rc 0; has 'ok: 1 CMakeCache.txt'; done_case

run absent-dl "$(tree absent-dl "$(drop GGML_BACKEND_DL:BOOL=OFF | awk '$0 != "GGML_CPU_ALL_VARIANTS:BOOL=OFF"')")"
want_rc 0; has 'ok: 1 CMakeCache.txt'; done_case

run none "$(tree none -)"
want_rc 1; has '^::error::whisper-cmake-cache: no whisper-rs-sys CMakeCache.txt'; lacks ': ok:'; done_case

t="$(tree other-crate -)"; mkdir -p "$t/debug/build/aws-lc-sys-1/out/build"
printf '%s\n' "$g" > "$t/debug/build/aws-lc-sys-1/out/build/CMakeCache.txt"
run other-crate "$t"
want_rc 1; has 'no whisper-rs-sys CMakeCache.txt'; done_case

run no-dir "$tmp/does-not-exist"
want_rc 1; has '^::error::'; lacks ': ok:'; done_case

refuse() { # refuse <label> <cache text> <regex the output must hold>
  run "$1" "$(tree "$1" "$2")"
  want_rc 1; has "$3"; lacks ': ok:'; done_case
}
refuse native-on "$(swap GGML_NATIVE:BOOL=OFF GGML_NATIVE:BOOL=ON)" 'GGML_NATIVE'
refuse native-absent "$(drop GGML_NATIVE:BOOL=OFF)" 'GGML_NATIVE:BOOL=OFF occurs 0 time'
refuse native-string "$(swap GGML_NATIVE:BOOL=OFF GGML_NATIVE:STRING=OFF)" 'GGML_NATIVE'
refuse native-lower "$(swap GGML_NATIVE:BOOL=OFF GGML_NATIVE:BOOL=off)" 'GGML_NATIVE'
refuse avx2-off "$(swap GGML_AVX2:BOOL=ON GGML_AVX2:BOOL=OFF)" 'GGML_AVX2'
refuse avx2-both "$(printf '%s\nGGML_AVX2:BOOL=OFF' "$g")" 'GGML_AVX2: a line that is not an owned line'
refuse avx512-on "$(swap GGML_AVX512:BOOL=OFF GGML_AVX512:BOOL=ON)" 'GGML_AVX512'
refuse shared-on "$(swap BUILD_SHARED_LIBS:BOOL=OFF BUILD_SHARED_LIBS:BOOL=ON)" 'BUILD_SHARED_LIBS'
refuse dl-on "$(swap GGML_BACKEND_DL:BOOL=OFF GGML_BACKEND_DL:BOOL=ON)" 'GGML_BACKEND_DL'
refuse variants-on "$(swap GGML_CPU_ALL_VARIANTS:BOOL=OFF GGML_CPU_ALL_VARIANTS:BOOL=ON)" 'GGML_CPU_ALL_VARIANTS'
refuse indented "$(swap GGML_BACKEND_DL:BOOL=OFF '  GGML_BACKEND_DL:BOOL=ON')" 'GGML_BACKEND_DL'
refuse duplicate "$(printf '%s\nGGML_NATIVE:BOOL=OFF' "$g")" 'GGML_NATIVE:BOOL=OFF occurs 2 time'
refuse new-desc "$(printf '%s\n//ggml: new text naming GGML_BACKEND_DL' "$g")" 'GGML_BACKEND_DL'

run avx512-sub "$(tree avx512-sub "$(swap GGML_AVX512_VBMI:BOOL=OFF GGML_AVX512_VBMI:BOOL=ON)")"
want_rc 0; done_case

run one-bad "$(tree one-bad "$g" "$(swap GGML_NATIVE:BOOL=OFF GGML_NATIVE:BOOL=ON)")"
want_rc 1; has 'release/build/whisper-rs-sys-fedcba9876543210/out/build/CMakeCache.txt'
lacks 'debug/build/whisper-rs-sys-0123456789abcdef/out/build/CMakeCache.txt'; done_case

run usage-none
want_rc 2; done_case
run usage-two "$tmp/good" "$tmp/good"
want_rc 2; done_case

echo "whisper-cmake-cache.test: $passed passed, $failed failed"
[ "$failed" = 0 ]
