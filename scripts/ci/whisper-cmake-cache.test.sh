#!/usr/bin/env bash
# T-099: host test of scripts/ci/whisper-cmake-cache.sh, the windows job's verdict on every
# whisper-rs-sys CMakeCache.txt under target/ (decisions #142, invariant 3), on fixture trees
# built in a temp dir (offline). Cases:
#   good           debug + release caches with the #142 lines (GGML_AVX_VNNI:BOOL=OFF and its
#                  help line included) -> exit 0, "2 CMakeCache.txt";
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
#   (T-099 review 1 finding 3: GGML_AVX_VNNI adds __AVXVNNI__ outside the AVX2 baseline on MSVC,
#   outside the AVX512 chain; its option() is unconditional in whisper-rs-sys 0.15.0, so the
#   cache always holds it and its help text "ggml: enable AVX-VNNI" names no token)
#   avx-vnni-on      GGML_AVX_VNNI:BOOL=ON -> exit 1 naming GGML_AVX_VNNI;
#   avx-vnni-absent  no GGML_AVX_VNNI entry -> exit 1 (refused, not allowed: the option always
#                    reaches the cache, so absence means an unreviewed whisper.cpp);
#   avx-vnni-uninit  GGML_AVX_VNNI:UNINITIALIZED=ON (a -D that no option() typed) -> exit 1;
#   avx-vnni-both    the good cache plus a second, unowned GGML_AVX_VNNI:BOOL=ON line -> exit 1;
#   avx-vnni-dup     GGML_AVX_VNNI:BOOL=OFF twice -> exit 1;
#   avx512-vnni      an extra GGML_AVX512_VNNI:BOOL=ON line -> exit 0 (another option, gated by
#                    GGML_AVX512, which is asserted OFF);
#   shared-on      BUILD_SHARED_LIBS:BOOL=ON -> exit 1;
#   dl-on          GGML_BACKEND_DL:BOOL=ON -> exit 1;
#   variants-on    GGML_CPU_ALL_VARIANTS:BOOL=ON -> exit 1;
#   indented       a leading blank before GGML_BACKEND_DL:BOOL=ON -> exit 1;
#   duplicate      GGML_NATIVE:BOOL=OFF twice -> exit 1;
#   one-bad        debug good, release with GGML_NATIVE:BOOL=ON -> exit 1 naming the release file;
#   new-desc       an unowned description naming GGML_BACKEND_DL -> exit 1, the line text printed;
#   absent-dir     good without the GGML_BACKEND_DIR help and entry -> exit 0;
#   dir-closed     the GGML_BACKEND_DIR help with a closing ")" (not cmake's text) -> exit 1;
#   dir-indented   the GGML_BACKEND_DIR help with a leading blank -> exit 1;
#   dir-trailing   the GGML_BACKEND_DIR help with a trailing blank -> exit 1;
#   advanced       an extra "GGML_BACKEND_DL-ADVANCED:INTERNAL=1" line -> exit 1;
#   line-text      GGML_BACKEND_DL:BOOL=ON -> the error ends with "line text [GGML_BACKEND_DL:BOOL=ON]";
#   usage          no argument, two arguments -> exit 2.
#   (T-099 validation 1, mutations M3-M6: every REQUIRED line has an absent and a duplicate case,
#   every TOKENS entry has a case that only the token check refuses, the REQUIRED count staying 1)
#   avx2-absent      no GGML_AVX2 entry -> exit 1, "GGML_AVX2:BOOL=ON occurs 0 time";
#   avx2-dup         GGML_AVX2:BOOL=ON twice -> exit 1, "occurs 2 time";
#   avx512-absent    no GGML_AVX512 entry -> exit 1, "GGML_AVX512:BOOL=OFF occurs 0 time";
#   avx512-dup       GGML_AVX512:BOOL=OFF twice -> exit 1, "occurs 2 time";
#   avx512-both      the good cache plus a second, unowned GGML_AVX512:BOOL=ON line -> exit 1;
#   shared-absent    no BUILD_SHARED_LIBS entry -> exit 1, "BUILD_SHARED_LIBS:BOOL=OFF occurs 0 time";
#   shared-dup       BUILD_SHARED_LIBS:BOOL=OFF twice -> exit 1, "occurs 2 time";
#   shared-both      the good cache plus a second, unowned BUILD_SHARED_LIBS:BOOL=ON line -> exit 1;
#   native-both      the good cache plus a second, unowned GGML_NATIVE:BOOL=ON line -> exit 1.
#   (T-099 validation 2, mutation F1 (V2-F1): the verdict of every cache counts, not only the
#   last one find lists. find's order is the filesystem's, so the bad cache is put in each
#   position by a pair of cases: one-bad (bad release) and one-bad-first (bad debug). Both trees
#   have the same directory names, so whichever of debug/ and release/ find lists first, one of
#   the two cases has its bad cache first. The line numbers pin the per-cache line counter.)
#   one-bad-first    debug with GGML_NATIVE:BOOL=ON, release good -> exit 1, the debug file named
#                    at line 33, the release file not named, no "ok:";
#   one-bad-line     debug good, release with GGML_NATIVE:BOOL=ON -> the release file at line 33;
#   both-bad         debug GGML_NATIVE:BOOL=ON, release GGML_BACKEND_DL:BOOL=ON -> exit 1, both
#                    files named, 3 errors (every cache is read to its end);
#   three-caches     good debug, bad release, good third cache -> exit 1, no "ok:";
#   mixed-crate      a good whisper-rs-sys cache plus another crate's cache with GGML_NATIVE:BOOL=ON
#                    -> exit 0, "ok: 1 CMakeCache.txt" (only whisper-rs-sys caches are read and counted);
#   two-errors       GGML_NATIVE:BOOL=ON and an extra GGML_BACKEND_DL:BOOL=ON in one cache -> 3 errors
#                    (both lines and the GGML_NATIVE:BOOL=OFF count);
#   multi-absent     no GGML_NATIVE and no GGML_AVX2 entry -> both "occurs 0 time" errors;
#   empty-cache      a zero-byte whisper-rs-sys CMakeCache.txt -> exit 1, 5 "occurs 0 time" errors;
#   no-eol-bad       an unowned last line GGML_BACKEND_DL:BOOL=ON without a final newline -> exit 1;
#   no-eol-good      the good cache whose last line GGML_NATIVE:BOOL=OFF has no final newline -> exit 0;
#   crlf-bad         the CRLF cache with GGML_NATIVE:BOOL=ON -> exit 1 (CR removal hides nothing);
#   crlf-double      GGML_NATIVE:BOOL=OFF followed by two CRs -> exit 1 (only one CR is removed);
#   dir-named-cache  a directory named CMakeCache.txt under whisper-rs-sys only -> exit 1, none found.
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

//ggml: enable AVX-VNNI
GGML_AVX_VNNI:BOOL=OFF

//ggml: directory to load dynamic backends from (requires GGML_BACKEND_DL
GGML_BACKEND_DIR:PATH=

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
refuse new-desc "$(printf '%s\n//ggml: new text naming GGML_BACKEND_DL' "$g")" 'GGML_BACKEND_DL: .*line text \[//ggml: new text naming GGML_BACKEND_DL\]$'
dir_help='//ggml: directory to load dynamic backends from (requires GGML_BACKEND_DL'
refuse dir-closed "$(swap "$dir_help" "$dir_help)")" 'line text \[//ggml: directory to load dynamic backends from \(requires GGML_BACKEND_DL\)\]$'
refuse dir-indented "$(swap "$dir_help" " $dir_help")" 'GGML_BACKEND_DL: a line that is not an owned line'
refuse dir-trailing "$(swap "$dir_help" "$dir_help ")" 'GGML_BACKEND_DL: a line that is not an owned line'
refuse advanced "$(printf '%s\nGGML_BACKEND_DL-ADVANCED:INTERNAL=1' "$g")" 'line text \[GGML_BACKEND_DL-ADVANCED:INTERNAL=1\]$'
refuse avx-vnni-on "$(swap GGML_AVX_VNNI:BOOL=OFF GGML_AVX_VNNI:BOOL=ON)" 'GGML_AVX_VNNI: a line that is not an owned line .*line text \[GGML_AVX_VNNI:BOOL=ON\]$'
refuse avx-vnni-absent "$(drop GGML_AVX_VNNI:BOOL=OFF)" 'GGML_AVX_VNNI:BOOL=OFF occurs 0 time'
refuse avx-vnni-uninit "$(swap GGML_AVX_VNNI:BOOL=OFF GGML_AVX_VNNI:UNINITIALIZED=ON)" 'GGML_AVX_VNNI: a line that is not an owned line'
refuse avx-vnni-both "$(printf '%s\nGGML_AVX_VNNI:BOOL=ON' "$g")" 'GGML_AVX_VNNI: a line that is not an owned line'
refuse avx-vnni-dup "$(printf '%s\nGGML_AVX_VNNI:BOOL=OFF' "$g")" 'GGML_AVX_VNNI:BOOL=OFF occurs 2 time'
refuse avx2-absent "$(drop GGML_AVX2:BOOL=ON)" 'GGML_AVX2:BOOL=ON occurs 0 time'
refuse avx2-dup "$(printf '%s\nGGML_AVX2:BOOL=ON' "$g")" 'GGML_AVX2:BOOL=ON occurs 2 time'
refuse avx512-absent "$(drop GGML_AVX512:BOOL=OFF)" 'GGML_AVX512:BOOL=OFF occurs 0 time'
refuse avx512-dup "$(printf '%s\nGGML_AVX512:BOOL=OFF' "$g")" 'GGML_AVX512:BOOL=OFF occurs 2 time'
refuse avx512-both "$(printf '%s\nGGML_AVX512:BOOL=ON' "$g")" 'GGML_AVX512: a line that is not an owned line .*line text \[GGML_AVX512:BOOL=ON\]$'
refuse shared-absent "$(drop BUILD_SHARED_LIBS:BOOL=OFF)" 'BUILD_SHARED_LIBS:BOOL=OFF occurs 0 time'
refuse shared-dup "$(printf '%s\nBUILD_SHARED_LIBS:BOOL=OFF' "$g")" 'BUILD_SHARED_LIBS:BOOL=OFF occurs 2 time'
refuse shared-both "$(printf '%s\nBUILD_SHARED_LIBS:BOOL=ON' "$g")" 'BUILD_SHARED_LIBS: a line that is not an owned line .*line text \[BUILD_SHARED_LIBS:BOOL=ON\]$'
refuse native-both "$(printf '%s\nGGML_NATIVE:BOOL=ON' "$g")" 'GGML_NATIVE: a line that is not an owned line .*line text \[GGML_NATIVE:BOOL=ON\]$'
refuse line-text "$(swap GGML_BACKEND_DL:BOOL=OFF GGML_BACKEND_DL:BOOL=ON)" ':[0-9]+: GGML_BACKEND_DL: a line that is not an owned line .*: line text \[GGML_BACKEND_DL:BOOL=ON\]$'

run absent-dir "$(tree absent-dir "$(drop "$dir_help" | awk '$0 != "GGML_BACKEND_DIR:PATH="')")"
want_rc 0; has 'ok: 1 CMakeCache.txt'; lacks '::error::'; done_case

run avx512-sub "$(tree avx512-sub "$(swap GGML_AVX512_VBMI:BOOL=OFF GGML_AVX512_VBMI:BOOL=ON)")"
want_rc 0; done_case

run avx512-vnni "$(tree avx512-vnni "$(printf '%s\nGGML_AVX512_VNNI:BOOL=ON' "$g")")"
want_rc 0; lacks '::error::'; done_case

run one-bad "$(tree one-bad "$g" "$(swap GGML_NATIVE:BOOL=OFF GGML_NATIVE:BOOL=ON)")"
want_rc 1; has 'release/build/whisper-rs-sys-fedcba9876543210/out/build/CMakeCache.txt'
lacks 'debug/build/whisper-rs-sys-0123456789abcdef/out/build/CMakeCache.txt'; done_case

# nerr <n>: the output holds exactly n "::error::" lines.
nerr() { local c; c="$(grep -c '^::error::' <<<"$out")"; [ "$c" = "$1" ] || fail "$c error line(s), want $1"; }
dbg='debug/build/whisper-rs-sys-0123456789abcdef/out/build/CMakeCache\.txt'
rel='release/build/whisper-rs-sys-fedcba9876543210/out/build/CMakeCache\.txt'
native_on="$(swap GGML_NATIVE:BOOL=OFF GGML_NATIVE:BOOL=ON)"

run one-bad-first "$(tree one-bad-first "$native_on" "$g")"
want_rc 1; has "$dbg:33: GGML_NATIVE: a line that is not an owned line"; lacks "$rel"; lacks ': ok:'; nerr 2; done_case

run one-bad-line "$(tree one-bad-line "$g" "$native_on")"
want_rc 1; has "$rel:33: GGML_NATIVE: a line that is not an owned line"; lacks "$dbg"; lacks ': ok:'; nerr 2; done_case

run both-bad "$(tree both-bad "$native_on" "$(swap GGML_BACKEND_DL:BOOL=OFF GGML_BACKEND_DL:BOOL=ON)")"
want_rc 1; has "$dbg:33: GGML_NATIVE"; has "$dbg: the line GGML_NATIVE:BOOL=OFF occurs 0 time"
has "$rel:27: GGML_BACKEND_DL: a line that is not an owned line"; lacks ': ok:'; nerr 3; done_case

t="$(tree three-caches "$g" "$native_on")"
mkdir -p "$t/x86_64-pc-windows-msvc/release/build/whisper-rs-sys-00112233aabbccdd/out/build"
printf '%s\n' "$g" > "$t/x86_64-pc-windows-msvc/release/build/whisper-rs-sys-00112233aabbccdd/out/build/CMakeCache.txt"
run three-caches "$t"
want_rc 1; has "$rel:33: GGML_NATIVE"; lacks 'x86_64-pc-windows-msvc'; lacks ': ok:'; nerr 2; done_case

t="$(tree mixed-crate "$g")"; mkdir -p "$t/debug/build/aws-lc-sys-1/out/build"
printf '%s\n' "$native_on" > "$t/debug/build/aws-lc-sys-1/out/build/CMakeCache.txt"
run mixed-crate "$t"
want_rc 0; has '^whisper-cmake-cache: ok: 1 CMakeCache.txt checked$'; lacks '::error::'; done_case

run two-errors "$(tree two-errors "$(printf '%s\nGGML_BACKEND_DL:BOOL=ON' "$native_on")")"
want_rc 1; has ':33: GGML_NATIVE: a line'; has ':4[0-9]: GGML_BACKEND_DL: a line .*line text \[GGML_BACKEND_DL:BOOL=ON\]$'
has 'GGML_NATIVE:BOOL=OFF occurs 0 time'; lacks ': ok:'; nerr 3; done_case

run multi-absent "$(tree multi-absent "$(drop GGML_NATIVE:BOOL=OFF | awk '$0 != "GGML_AVX2:BOOL=ON"')")"
want_rc 1; has 'GGML_NATIVE:BOOL=OFF occurs 0 time'; has 'GGML_AVX2:BOOL=ON occurs 0 time'; lacks ': ok:'; nerr 2; done_case

t="$(tree empty-cache -)"; mkdir -p "$t/debug/build/whisper-rs-sys-1/out/build"
: > "$t/debug/build/whisper-rs-sys-1/out/build/CMakeCache.txt"
run empty-cache "$t"
want_rc 1; has 'occurs 0 time'; lacks ': ok:'; nerr 5; done_case

t="$(tree no-eol-bad -)"; mkdir -p "$t/debug/build/whisper-rs-sys-1/out/build"
printf '%s\nGGML_BACKEND_DL:BOOL=ON' "$g" > "$t/debug/build/whisper-rs-sys-1/out/build/CMakeCache.txt"
run no-eol-bad "$t"
want_rc 1; has 'GGML_BACKEND_DL: a line .*line text \[GGML_BACKEND_DL:BOOL=ON\]$'; lacks ': ok:'; done_case

t="$(tree no-eol-good -)"; mkdir -p "$t/debug/build/whisper-rs-sys-1/out/build"
printf '%s\nGGML_NATIVE:BOOL=OFF' "$(drop GGML_NATIVE:BOOL=OFF)" > "$t/debug/build/whisper-rs-sys-1/out/build/CMakeCache.txt"
run no-eol-good "$t"
want_rc 0; has 'ok: 1 CMakeCache.txt'; lacks '::error::'; done_case

t="$(tree crlf-bad -)"; mkdir -p "$t/debug/build/whisper-rs-sys-1/out/build"
sed 's/$/\r/' <<<"$native_on" > "$t/debug/build/whisper-rs-sys-1/out/build/CMakeCache.txt"
run crlf-bad "$t"
want_rc 1; has ':33: GGML_NATIVE: a line'; lacks ': ok:'; done_case

t="$(tree crlf-double -)"; mkdir -p "$t/debug/build/whisper-rs-sys-1/out/build"
sed 's/^GGML_NATIVE:BOOL=OFF$/&\r\r/' <<<"$g" > "$t/debug/build/whisper-rs-sys-1/out/build/CMakeCache.txt"
run crlf-double "$t"
want_rc 1; has ':33: GGML_NATIVE: a line that is not an owned line'; lacks ': ok:'; done_case

t="$(tree dir-named-cache -)"; mkdir -p "$t/debug/build/whisper-rs-sys-1/out/build/CMakeCache.txt"
run dir-named-cache "$t"
want_rc 1; has '^::error::whisper-cmake-cache: no whisper-rs-sys CMakeCache.txt'; lacks ': ok:'; nerr 1; done_case

run usage-none
want_rc 2; done_case
run usage-two "$tmp/good" "$tmp/good"
want_rc 2; done_case

echo "whisper-cmake-cache.test: $passed passed, $failed failed"
[ "$failed" = 0 ]
