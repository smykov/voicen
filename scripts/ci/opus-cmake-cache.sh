#!/usr/bin/env bash
# T-117 (decisions #197): the libopus that ships (opusic-sys 0.7.5, bundled) is built with the
# static CRT (/MT: libcmt), the CRT of the shell (tauri-build static_vcruntime). The CI step reads
# what the build really did, never the cargo config: the configuration cmake wrote (every
# opusic-sys CMakeCache.txt under the target dir) and the effect on the objects (the linker
# directives of each opusic-sys out/lib/opus.lib). opus sets CMAKE_MSVC_RUNTIME_LIBRARY as a normal
# variable from OPUS_STATIC_RUNTIME and CMAKE_BUILD_TYPE (opus CMakeLists.txt), so the cache holds
# those two and never CMAKE_MSVC_RUNTIME_LIBRARY; and cc-rs's -MD still reaches CMAKE_C_FLAGS, so
# only the objects show which runtime flag won. crt-imports.sh cannot tell: libopus is C only and
# the shell exes link with static_vcruntime's /NODEFAULTLIB:msvcrt.lib.
#
# P-020 (decides_on: token + literal). Per CMakeCache.txt whose path matches */opusic-sys-*/*
# (one trailing CR removed per line):
#   - each REQUIRED line occurs exactly once, byte for byte;
#   - every line holding one of the TOKENS (whole word: not next to [A-Za-z0-9_]; any line,
#     comments included) is an OWNED line; CMAKE_MSVC_RUNTIME_LIBRARY has no owned line.
#   CMAKE_BUILD_TYPE is not a token: a Debug (or other) build type only makes the Release count 0.
# Per lib (effect, D2): the lib of <P>/out/build/CMakeCache.txt is <P>/out/lib/opus.lib (missing:
# refused, fail closed); every opus.lib under an */opusic-sys-*/* path is dumped with
# `dumpbin -nologo -directives`: an exit other than 0 refuses, any /DEFAULTLIB:MSVCRT (any case,
# msvcrt.lib or msvcrtd too) refuses, and a pass needs at least one /DEFAULTLIB:LIBCMT.
# No CMakeCache.txt found is a refusal, never a pass. Every cache and every lib is judged.
#
# dumpbin: $DUMPBIN when set; else dumpbin on PATH; else the newest Visual Studio's x64 one via
# vswhere (as crt-imports.sh). None found: cannot run (exit 3).
#
# Usage: scripts/ci/opus-cmake-cache.sh <target dir>
# Exit 0: "opus-cmake-cache: ok: <n> CMakeCache.txt checked". Exit 1: one "::error::<file>[:<line>]:
# ..." per violation, ending "line text [<the line, CR removed>]" for an unowned line (a cache
# holds no secret), or none found. Exit 2: wrong usage. Exit 3: cannot run.
# Host-tested by scripts/ci/opus-cmake-cache.test.sh with a stub dumpbin (make check-opus-cmake-cache).
set -uo pipefail
if [ "$#" != 1 ] || [ -z "$1" ]; then
  echo "usage: opus-cmake-cache.sh <target dir>" >&2
  exit 2
fi
target="$1"

dumpbin="${DUMPBIN:-}"
if [ -z "$dumpbin" ] && command -v dumpbin >/dev/null 2>&1; then
  dumpbin=dumpbin
fi
if [ -z "$dumpbin" ]; then
  vswhere="${VSWHERE:-/c/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe}"
  if [ -f "$vswhere" ]; then
    dumpbin="$("$vswhere" -latest -products '*' -find 'VC/Tools/MSVC/**/bin/Hostx64/x64/dumpbin.exe' 2>/dev/null | tr -d '\r' | tail -n 1)"
  fi
fi
if [ -z "$dumpbin" ]; then
  echo "opus-cmake-cache: cannot run: no dumpbin (DUMPBIN unset, none on PATH, none found by vswhere)" >&2
  exit 3
fi

if [ ! -d "$target" ]; then
  echo "::error::opus-cmake-cache: $target is not a directory, so no opusic-sys build was checked"
  exit 1
fi

REQUIRED=(
  "OPUS_STATIC_RUNTIME:BOOL=ON"
  "CMAKE_BUILD_TYPE:STRING=Release"
)
OWNED=("OPUS_STATIC_RUNTIME:BOOL=ON")
TOKENS=(OPUS_STATIC_RUNTIME CMAKE_MSVC_RUNTIME_LIBRARY)

caches=()
libs=()
declare -A seen_lib=()
add_lib() {
  if [ -z "${seen_lib[$1]:-}" ]; then
    seen_lib[$1]=1
    libs+=("$1")
  fi
}
while IFS= read -r -d '' f; do
  case "$f" in
    */opusic-sys-*/*)
      caches+=("$f")
      case "$f" in
        */out/build/CMakeCache.txt) add_lib "${f%/out/build/CMakeCache.txt}/out/lib/opus.lib" ;;
      esac
      ;;
  esac
done < <(find "$target" -type f -name CMakeCache.txt -print0 2>/dev/null)
while IFS= read -r -d '' f; do
  case "$f" in
    */opusic-sys-*/*) add_lib "$f" ;;
  esac
done < <(find "$target" -type f -name opus.lib -print0 2>/dev/null)

if [ "${#caches[@]}" = 0 ]; then
  echo "::error::opus-cmake-cache: no opusic-sys CMakeCache.txt under $target, so the libopus build configuration is unproven"
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
        echo "::error::$f:$n: $t: a line that is not an owned line (want ${REQUIRED[*]}, each once; no line naming CMAKE_MSVC_RUNTIME_LIBRARY): line text [$line]"
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

for lib in "${libs[@]}"; do
  if [ ! -f "$lib" ]; then
    echo "::error::$lib: not a file (the opusic-sys build next to its CMakeCache.txt left no opus.lib), so its C runtime is unproven"
    bad=1
    continue
  fi
  # "-" options: Git Bash rewrites a leading "/" argument as a path.
  out="$("$dumpbin" -nologo -directives "$lib" 2>&1)"
  rc=$?
  if [ "$rc" != 0 ]; then
    echo "::error::$lib: dumpbin exited $rc"
    bad=1
    continue
  fi
  out="$(tr -d '\r' <<<"$out")"
  if grep -qiE '/DEFAULTLIB:"?MSVCRT' <<<"$out"; then
    echo "::error::$lib: names /DEFAULTLIB:MSVCRT (compiled /MD, the dynamic CRT); libopus must be /MT (libcmt)"
    bad=1
    continue
  fi
  if ! grep -qiE '/DEFAULTLIB:"?LIBCMT(\.lib)?"?($|[[:space:]])' <<<"$out"; then
    echo "::error::$lib: no /DEFAULTLIB:LIBCMT in dumpbin -directives, so its static CRT is unproven"
    bad=1
    continue
  fi
done

[ "$bad" = 0 ] || exit 1
echo "opus-cmake-cache: ok: ${#caches[@]} CMakeCache.txt checked"
exit 0
