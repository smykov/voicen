#!/usr/bin/env bash
# T-099 (invariant 4; F-001 class): every Windows exe that links whisper.cpp has the shell's C
# runtime: static vcruntime / libcmt / libcpmt, dynamic UCRT (tauri-build static_vcruntime). A C++
# library built /MD would import MSVCP140.dll and VCRUNTIME140.dll; the GitHub runner has the VC
# redistributable installed, so the install smoke would still pass. The proof is the import
# table: dumpbin -dependents of each exe.
#
# P-020 (decides_on: token): the run refuses when dumpbin's output for an exe contains the raw
# token MSVCP140 or VCRUNTIME140 (any case, any line, delay-load imports included). A pass also
# needs dumpbin to exit 0 and to print its literal line "Image has the following dependencies:"
# for the exe, so an empty or failed dump is never a pass. No exe, or an exe path that is not a
# file (an unmatched glob), refuses.
#
# dumpbin: $DUMPBIN when set; else dumpbin on PATH; else the newest Visual Studio's x64 one via
# vswhere. None found: cannot run (exit 3).
#
# Usage: scripts/ci/crt-imports.sh <exe>...
# Exit 0: "crt-imports: ok: <n> exe(s) without MSVCP140 / VCRUNTIME140 imports". Exit 1: one
# "::error::<exe>: ..." per refused exe. Exit 2: wrong usage. Exit 3: cannot run.
# Host-tested by scripts/ci/crt-imports.test.sh with a stub dumpbin (make check-crt-imports).
set -uo pipefail
if [ "$#" = 0 ]; then
  echo "usage: crt-imports.sh <exe>..." >&2
  exit 2
fi

dumpbin="${DUMPBIN:-}"
if [ -z "$dumpbin" ] && command -v dumpbin >/dev/null 2>&1; then
  dumpbin=dumpbin
fi
if [ -z "$dumpbin" ]; then
  vswhere="${VSWHERE:-/c/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe}"
  if [ -x "$vswhere" ] || [ -f "$vswhere" ]; then
    dumpbin="$("$vswhere" -latest -products '*' -find 'VC/Tools/MSVC/**/bin/Hostx64/x64/dumpbin.exe' 2>/dev/null | tr -d '\r' | tail -n 1)"
  fi
fi
if [ -z "$dumpbin" ]; then
  echo "crt-imports: cannot run: no dumpbin (DUMPBIN unset, none on PATH, none found by vswhere)" >&2
  exit 3
fi

bad=0
for exe in "$@"; do
  if [ ! -f "$exe" ]; then
    echo "::error::$exe: not a file (no exe was built, or the pattern matched nothing), so its imports are unproven"
    bad=1
    continue
  fi
  # "-" options: Git Bash rewrites a leading "/" argument as a path.
  out="$("$dumpbin" -nologo -dependents "$exe" 2>&1)"
  rc=$?
  if [ "$rc" != 0 ]; then
    echo "::error::$exe: dumpbin exited $rc"
    bad=1
    continue
  fi
  if ! tr -d '\r' <<<"$out" | grep -qxE '[[:space:]]*Image has the following dependencies:'; then
    echo "::error::$exe: dumpbin printed no dependency list, so its imports are unproven"
    bad=1
    continue
  fi
  hits="$(grep -ioE 'MSVCP140|VCRUNTIME140' <<<"$out" | tr '[:lower:]' '[:upper:]' | sort -u | tr '\n' ' ')"
  if [ -n "$hits" ]; then
    echo "::error::$exe: imports the dynamic VC runtime (${hits% }); the shell's CRT is static vcruntime / libcmt / libcpmt with dynamic UCRT"
    bad=1
    continue
  fi
  echo "crt-imports: $exe: no MSVCP140 / VCRUNTIME140 import"
done

[ "$bad" = 0 ] || exit 1
echo "crt-imports: ok: $# exe(s) without MSVCP140 / VCRUNTIME140 imports"
exit 0
