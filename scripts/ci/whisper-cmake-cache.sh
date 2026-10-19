#!/usr/bin/env bash
# T-099 (decisions #142, OQ-37 option ii; invariant 3): the whisper.cpp that ships is one static
# build that never depends on the build machine's CPU and targets the AVX2 baseline. The CI step
# reads the effective configuration cmake wrote, never the cargo config: whisper-rs-sys declares
# no rerun-if-env-changed for GGML_* / CMAKE_*, and the build cache can bring back an older
# OUT_DIR, so every whisper-rs-sys CMakeCache.txt under the target dir is checked (the debug
# build of cargo test and the release build of pnpm tauri build each have one).
#
# P-020 (decides_on: token + literal). Per CMakeCache.txt (one trailing CR removed per line):
#   - each REQUIRED line below occurs exactly once, byte for byte;
#   - every line holding one of the TOKENS (whole word: not next to [A-Za-z0-9_]) is one of
#     the OWNED lines below; anything else (another type, value or spelling, an ON) refuses.
#     GGML_BACKEND_DL and GGML_CPU_ALL_VARIANTS may be absent or OFF; the two ggml option
#     descriptions that name an option (whisper.cpp 1.8.3, whisper-rs-sys 0.15.0) are owned
#     lines, so a new description holding a token is refused until it is reviewed and owned.
# GGML_FMA / GGML_F16C are not cache options on MSVC (option() only if NOT MSVC), so they are
# not asserted. No CMakeCache.txt found is a refusal, never a pass.
#
# Usage: scripts/ci/whisper-cmake-cache.sh <target dir>
# Exit 0: "whisper-cmake-cache: ok: <n> CMakeCache.txt checked". Exit 1: one
# "::error::<file>:<line>: ..." per violation (or none found). Exit 2: wrong usage.
# Host-tested by scripts/ci/whisper-cmake-cache.test.sh (make check-whisper-cmake-cache).
set -uo pipefail
if [ "$#" != 1 ] || [ -z "$1" ]; then
  echo "usage: whisper-cmake-cache.sh <target dir>" >&2
  exit 2
fi
target="$1"
if [ ! -d "$target" ]; then
  echo "::error::whisper-cmake-cache: $target is not a directory, so no whisper-rs-sys build was checked"
  exit 1
fi

REQUIRED=(
  "GGML_NATIVE:BOOL=OFF"
  "GGML_AVX2:BOOL=ON"
  "GGML_AVX512:BOOL=OFF"
  "BUILD_SHARED_LIBS:BOOL=OFF"
)
OWNED=(
  "${REQUIRED[@]}"
  "GGML_BACKEND_DL:BOOL=OFF"
  "GGML_CPU_ALL_VARIANTS:BOOL=OFF"
  "//ggml: build backends as dynamic libraries (requires BUILD_SHARED_LIBS)"
  "//ggml: build all variants of the CPU backend (requires GGML_BACKEND_DL)"
)
TOKENS=(GGML_NATIVE GGML_AVX2 GGML_AVX512 BUILD_SHARED_LIBS GGML_BACKEND_DL GGML_CPU_ALL_VARIANTS)

caches=()
while IFS= read -r -d '' f; do
  case "$f" in
    */whisper-rs-sys-*/*) caches+=("$f") ;;
  esac
done < <(find "$target" -type f -name CMakeCache.txt -print0 2>/dev/null)

if [ "${#caches[@]}" = 0 ]; then
  echo "::error::whisper-cmake-cache: no whisper-rs-sys CMakeCache.txt under $target, so the whisper.cpp build configuration is unproven"
  exit 1
fi

is_owned() {
  local l="$1" o
  for o in "${OWNED[@]}"; do
    [ "$l" = "$o" ] && return 0
  done
  return 1
}

bad=0
for f in "${caches[@]}"; do
  declare -A count=()
  n=0
  while IFS= read -r line || [ -n "$line" ]; do
    n=$((n + 1))
    line="${line%$'\r'}"
    for r in "${REQUIRED[@]}"; do
      [ "$line" = "$r" ] && count[$r]=$((${count[$r]:-0} + 1))
    done
    for t in "${TOKENS[@]}"; do
      if [[ "$line" =~ (^|[^A-Za-z0-9_])${t}($|[^A-Za-z0-9_]) ]] && ! is_owned "$line"; then
        echo "::error::$f:$n: $t: a line that is not an owned line (want ${REQUIRED[*]}; GGML_BACKEND_DL / GGML_CPU_ALL_VARIANTS absent or OFF)"
        bad=1
        break
      fi
    done
  done < "$f"
  for r in "${REQUIRED[@]}"; do
    c="${count[$r]:-0}"
    if [ "$c" != 1 ]; then
      echo "::error::$f: the line $r occurs $c time(s), want exactly 1"
      bad=1
    fi
  done
  unset count
done

[ "$bad" = 0 ] || exit 1
echo "whisper-cmake-cache: ok: ${#caches[@]} CMakeCache.txt checked"
exit 0
