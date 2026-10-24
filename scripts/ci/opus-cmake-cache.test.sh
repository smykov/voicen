#!/usr/bin/env bash
# T-117 (decisions #197): host test of scripts/ci/opus-cmake-cache.sh, the windows job's verdict on
# every opusic-sys build under target/: the CMakeCache.txt cmake wrote (configuration: token +
# literal, the whisper-cmake-cache shape) and the objects of out/lib/opus.lib (effect, D2:
# `dumpbin -nologo -directives`). One script, one verdict. Fixture trees are built in a temp dir
# and dumpbin is a stub on PATH (and in DUMPBIN), offline.
#
# Contract (decides_on, copied from the T-117 analysis):
#   usage      opus-cmake-cache.sh <target dir>   (exit 2 on any other argument count)
#   caches     every file named CMakeCache.txt under the dir whose path matches */opusic-sys-*/*;
#              a cache outside opusic-sys-* is neither read nor counted; none found -> exit 1.
#   per cache  one trailing CR removed per line; "OPUS_STATIC_RUNTIME:BOOL=ON" and
#              "CMAKE_BUILD_TYPE:STRING=Release" each occur exactly once, byte for byte; tokens
#              OPUS_STATIC_RUNTIME and CMAKE_MSVC_RUNTIME_LIBRARY (whole word: not next to
#              [A-Za-z0-9_]; any line, comments included): a line holding one that is not an owned
#              line (CMAKE_MSVC_RUNTIME_LIBRARY has none) refuses.
#   per lib    each cache <P>/out/build/CMakeCache.txt has its lib at <P>/out/lib/opus.lib (a
#              missing lib refuses: fail closed). Every opus.lib under an opusic-sys-* path is
#              dumped with `dumpbin -nologo -directives <lib>`: dumpbin exit != 0 refuses, any
#              /DEFAULTLIB:MSVCRT (any case) refuses, no /DEFAULTLIB:LIBCMT refuses.
#   no dumpbin neither DUMPBIN nor dumpbin on PATH (vswhere is the script's business) -> exit 3
#              "cannot run", never a pass.
#   output     exit 0: "opus-cmake-cache: ok: <n> CMakeCache.txt checked". Exit 1: one
#              "::error::<file>[:<line>]: ..." per violation, naming the reason:
#                "no opusic-sys CMakeCache.txt under ..."
#                "<file>:<n>: <TOKEN>: a line that is not an owned line ...: line text [<line>]"
#                "<file>: the line <owned line> occurs <c> time(s), want exactly 1"
#                "<lib>: not a file" / "<lib>: dumpbin exited <rc>" / "<lib>: names /DEFAULTLIB:MSVCRT"
#                "<lib>: no /DEFAULTLIB:LIBCMT"
# Cases are named in the run lines below. Every refusal asserts its reason text.
# Usage: scripts/ci/opus-cmake-cache.test.sh   (host bash)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
script="$root/scripts/ci/opus-cmake-cache.sh"
[ -x "$script" ] || { echo "opus-cmake-cache.test: cannot run: $script not found or not executable" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "opus-cmake-cache.test: cannot run: mktemp" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

# Stub dumpbin: prints <lib>.deps (canned body) and exits with <lib>.rc (default 0); logs argv.
mkdir -p "$tmp/bin"
stub="$tmp/bin/dumpbin"
cat > "$stub" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${STUB_LOG:?}"
lib="${!#}"
cat "$lib.deps" 2>/dev/null
exit "$(cat "$lib.rc" 2>/dev/null || echo 0)"
EOF
chmod +x "$stub"
export STUB_LOG="$tmp/argv.log"

failed=0
passed=0
fail() { echo "FAIL $label: $1" >&2; sed 's/^/       | /' <<<"$out" >&2; failed=$((failed + 1)); ok=0; }
run() { label="$1"; ok=1; shift; out="$(DUMPBIN="$stub" PATH="$tmp/bin:$PATH" "$script" "$@" 2>&1)"; rc=$?; }
want_rc() { [ "$rc" = "$1" ] || fail "exit $rc, want $1"; }
has() { grep -qE -- "$1" <<<"$out" || fail "output lacks /$1/"; }
lacks() { ! grep -qE -- "$1" <<<"$out" || fail "output has /$1/"; }
nerr() { local c; c="$(grep -c '^::error::' <<<"$out")"; [ "$c" = "$1" ] || fail "$c error line(s), want $1"; }
done_case() { [ "$ok" = 1 ] && { echo "ok   $label"; passed=$((passed + 1)); }; }

# A cache shaped like cmake's own (description line, entry), opus 1.5 option texts.
good_cache() {
  cat <<'EOF'
# This is the CMakeCache file.
# For build in directory: C:/a/voicen/voicen/target/release/build/opusic-sys-0123456789abcdef/out/build

########################
# EXTERNAL cache entries
########################

//Build type
CMAKE_BUILD_TYPE:STRING=Release

//Build shared library
BUILD_SHARED_LIBS:BOOL=OFF

//Link CRT statically, if supported by the compiler
OPUS_STATIC_RUNTIME:BOOL=ON

//Enable fixed-point implementation
OPUS_FIXED_POINT:BOOL=OFF

//No help, variable specified on the command line.
CMAKE_TOOLCHAIN_FILE:UNINITIALIZED=C:/a/voicen/voicen/cmake/msvc-static-crt.cmake

########################
# INTERNAL cache entries
########################

CMAKE_CACHEFILE_DIR:INTERNAL=C:/a/voicen/voicen/target/release/build/opusic-sys-0123456789abcdef/out/build
EOF
}
g="$(good_cache)"
swap() { awk -v a="$1" -v b="$2" '$0 == a { print b; next } { print }' <<<"$g"; }
drop() { awk -v a="$1" '$0 != a { print }' <<<"$g"; }

# What `dumpbin -nologo -directives opus.lib` prints (CRLF), per kind of lib.
dump() { # dump <directive...>
  printf 'Microsoft (R) COFF/PE Dumper\r\n\r\nDump of file opus.lib\r\n\r\nFile Type: LIBRARY\r\n\r\n   Linker Directives\r\n   -----------------\r\n'
  local d
  for d in "$@"; do printf '   %s\r\n' "$d"; done
  printf '\r\n  Summary\r\n\r\n        1000 .debug$S\r\n'
}
good_dump="$(dump /DEFAULTLIB:LIBCMT /DEFAULTLIB:OLDNAMES)"

# put <target> <profile> <hash> <cache text or -> <dump text or -> : an opusic-sys build dir with
# its cache (out/build/CMakeCache.txt) and lib (out/lib/opus.lib + the stub's canned dump).
put() { putc "$1" "$2" "opusic-sys-$3" "$4" "$5"; }
putc() { # putc <target> <profile> <crate dir name> <cache> <dump>
  local d="$1/$2/build/$3/out"
  mkdir -p "$d/build" "$d/lib"
  if [ "$4" != - ]; then printf '%s\n' "$4" > "$d/build/CMakeCache.txt"; fi
  if [ "$5" != - ]; then : > "$d/lib/opus.lib"; printf '%s\n' "$5" > "$d/lib/opus.lib.deps"; fi
}
dbgd='debug/build/opusic-sys-0123456789abcdef/out'
reld='release/build/opusic-sys-fedcba9876543210/out'
# tree <name> <cache> <dump>: the debug build only; tree2 adds a release build of the same shape.
tree() { local t="$tmp/$1"; put "$t" debug 0123456789abcdef "$2" "$3"; echo "$t"; }
tree2() { local t="$tmp/$1"; put "$t" debug 0123456789abcdef "$2" "$3"; put "$t" release fedcba9876543210 "$4" "$5"; echo "$t"; }

# --- pass paths -------------------------------------------------------------------------------
: > "$STUB_LOG"
run good "$(tree2 good "$g" "$good_dump" "$g" "$good_dump")"
want_rc 0; has '^opus-cmake-cache: ok: 2 CMakeCache.txt checked$'; lacks '::error::'
# the effect check really ran on each lib, with dumpbin's own flags (Git Bash keeps "-" arguments)
grep -qxF -- "-nologo -directives $tmp/good/$dbgd/lib/opus.lib" "$STUB_LOG" || fail "stub argv lacks the debug lib: $(cat "$STUB_LOG")"
grep -qxF -- "-nologo -directives $tmp/good/$reld/lib/opus.lib" "$STUB_LOG" || fail "stub argv lacks the release lib: $(cat "$STUB_LOG")"
done_case

t="$tmp/crlf"; put "$t" debug 1 - "$good_dump"
sed 's/$/\r/' <<<"$g" > "$t/debug/build/opusic-sys-1/out/build/CMakeCache.txt"
run crlf "$t"
want_rc 0; has 'ok: 1 CMakeCache.txt'; lacks '::error::'; done_case

run no-eol-good "$(tree no-eol-good "$(printf '%s\nCMAKE_BUILD_TYPE:STRING=Release' "$(drop CMAKE_BUILD_TYPE:STRING=Release)")" "$good_dump")"
want_rc 0; has 'ok: 1 CMakeCache.txt'; done_case

# whole-word tokens: a longer name, or one with a letter/digit/underscore in front, is not a token
run adjacent-suffix "$(tree adjacent-suffix "$(printf '%s\nOPUS_STATIC_RUNTIME_X:BOOL=ON\nCMAKE_MSVC_RUNTIME_LIBRARY_X:STRING=a\nXOPUS_STATIC_RUNTIME:BOOL=OFF\nXCMAKE_MSVC_RUNTIME_LIBRARY:STRING=a\nOPUS_STATIC_RUNTIME2:BOOL=OFF' "$g")" "$good_dump")"
want_rc 0; has 'ok: 1 CMakeCache.txt'; lacks '::error::'; done_case

# only opusic-sys caches count: another crate's bad cache and bad lib are not read
t="$(tree mixed-crate "$g" "$good_dump")"
putc "$t" debug aws-lc-sys-1 "$(swap OPUS_STATIC_RUNTIME:BOOL=ON OPUS_STATIC_RUNTIME:BOOL=OFF)" "$(dump /DEFAULTLIB:MSVCRT)"
mkdir -p "$t/debug/build/whisper-rs-sys-1/out/build"; printf '%s\n' 'CMAKE_MSVC_RUNTIME_LIBRARY:STRING=MultiThreadedDLL' > "$t/debug/build/whisper-rs-sys-1/out/build/CMakeCache.txt"
run mixed-crate "$t"
want_rc 0; has '^opus-cmake-cache: ok: 1 CMakeCache.txt checked$'; lacks '::error::'; done_case

# --- no cache / bad argument ------------------------------------------------------------------
run none "$(tree none - -)"
want_rc 1; has '^::error::opus-cmake-cache: no opusic-sys CMakeCache.txt'; lacks ': ok:'; nerr 1; done_case

t="$tmp/other-crate"; putc "$t" debug aws-lc-sys-1 "$g" "$good_dump"
run other-crate "$t"
want_rc 1; has 'no opusic-sys CMakeCache.txt'; lacks ': ok:'; done_case

run no-dir "$tmp/does-not-exist"
want_rc 1; has '^::error::'; lacks ': ok:'; done_case

t="$tmp/dir-named-cache"; mkdir -p "$t/debug/build/opusic-sys-1/out/build/CMakeCache.txt"
run dir-named-cache "$t"
want_rc 1; has '^::error::opus-cmake-cache: no opusic-sys CMakeCache.txt'; lacks ': ok:'; done_case

run usage-none
want_rc 2; done_case
run usage-two "$tmp/good" "$tmp/good"
want_rc 2; done_case

# --- cache refusals (reason named) ------------------------------------------------------------
refuse() { # refuse <label> <cache text> <regex the output must hold>
  run "$1" "$(tree "$1" "$2" "$good_dump")"
  want_rc 1; has "$3"; lacks ': ok:'; done_case
}
refuse static-off "$(swap OPUS_STATIC_RUNTIME:BOOL=ON OPUS_STATIC_RUNTIME:BOOL=OFF)" 'OPUS_STATIC_RUNTIME: a line that is not an owned line.*line text \[OPUS_STATIC_RUNTIME:BOOL=OFF\]$'
refuse static-off-count "$(swap OPUS_STATIC_RUNTIME:BOOL=ON OPUS_STATIC_RUNTIME:BOOL=OFF)" 'the line OPUS_STATIC_RUNTIME:BOOL=ON occurs 0 time'
refuse static-lower "$(swap OPUS_STATIC_RUNTIME:BOOL=ON OPUS_STATIC_RUNTIME:BOOL=on)" 'OPUS_STATIC_RUNTIME'
refuse static-string "$(swap OPUS_STATIC_RUNTIME:BOOL=ON OPUS_STATIC_RUNTIME:STRING=ON)" 'OPUS_STATIC_RUNTIME'
refuse static-uninit "$(swap OPUS_STATIC_RUNTIME:BOOL=ON OPUS_STATIC_RUNTIME:UNINITIALIZED=ON)" 'OPUS_STATIC_RUNTIME: a line that is not an owned line'
refuse static-absent "$(drop OPUS_STATIC_RUNTIME:BOOL=ON)" 'the line OPUS_STATIC_RUNTIME:BOOL=ON occurs 0 time'
refuse static-dup "$(printf '%s\nOPUS_STATIC_RUNTIME:BOOL=ON' "$g")" 'the line OPUS_STATIC_RUNTIME:BOOL=ON occurs 2 time'
refuse static-both "$(printf '%s\nOPUS_STATIC_RUNTIME:BOOL=OFF' "$g")" 'OPUS_STATIC_RUNTIME: a line that is not an owned line.*line text \[OPUS_STATIC_RUNTIME:BOOL=OFF\]$'
refuse static-comment "$(printf '%s\n//turn OPUS_STATIC_RUNTIME off' "$g")" 'OPUS_STATIC_RUNTIME: a line that is not an owned line.*line text \[//turn OPUS_STATIC_RUNTIME off\]$'
refuse static-hyphen "$(printf '%s\nOPUS_STATIC_RUNTIME-ADVANCED:INTERNAL=1' "$g")" 'OPUS_STATIC_RUNTIME: a line that is not an owned line'
refuse static-indented "$(swap OPUS_STATIC_RUNTIME:BOOL=ON ' OPUS_STATIC_RUNTIME:BOOL=ON')" 'OPUS_STATIC_RUNTIME'
refuse static-trailing "$(swap OPUS_STATIC_RUNTIME:BOOL=ON 'OPUS_STATIC_RUNTIME:BOOL=ON ')" 'OPUS_STATIC_RUNTIME'
refuse msvc-dll "$(printf '%s\nCMAKE_MSVC_RUNTIME_LIBRARY:STRING=MultiThreadedDLL' "$g")" 'CMAKE_MSVC_RUNTIME_LIBRARY: a line that is not an owned line.*line text \[CMAKE_MSVC_RUNTIME_LIBRARY:STRING=MultiThreadedDLL\]$'
refuse msvc-mt "$(printf '%s\nCMAKE_MSVC_RUNTIME_LIBRARY:STRING=MultiThreaded' "$g")" 'CMAKE_MSVC_RUNTIME_LIBRARY: a line that is not an owned line.*line text \[CMAKE_MSVC_RUNTIME_LIBRARY:STRING=MultiThreaded\]$'
refuse msvc-uninit "$(printf '%s\nCMAKE_MSVC_RUNTIME_LIBRARY:UNINITIALIZED=MultiThreaded' "$g")" 'CMAKE_MSVC_RUNTIME_LIBRARY'
refuse msvc-comment "$(printf '%s\n//CMAKE_MSVC_RUNTIME_LIBRARY is set by the toolchain file' "$g")" 'CMAKE_MSVC_RUNTIME_LIBRARY: a line that is not an owned line'
refuse msvc-hash-comment "$(printf '%s\n# CMAKE_MSVC_RUNTIME_LIBRARY' "$g")" 'CMAKE_MSVC_RUNTIME_LIBRARY: a line that is not an owned line'
refuse msvc-internal "$(printf '%s\nCMAKE_MSVC_RUNTIME_LIBRARY-ADVANCED:INTERNAL=1' "$g")" 'CMAKE_MSVC_RUNTIME_LIBRARY'
refuse build-debug "$(swap CMAKE_BUILD_TYPE:STRING=Release CMAKE_BUILD_TYPE:STRING=Debug)" 'the line CMAKE_BUILD_TYPE:STRING=Release occurs 0 time'
refuse build-lower "$(swap CMAKE_BUILD_TYPE:STRING=Release CMAKE_BUILD_TYPE:STRING=release)" 'CMAKE_BUILD_TYPE:STRING=Release occurs 0 time'
refuse build-minsize "$(swap CMAKE_BUILD_TYPE:STRING=Release CMAKE_BUILD_TYPE:STRING=MinSizeRel)" 'CMAKE_BUILD_TYPE:STRING=Release occurs 0 time'
refuse build-absent "$(drop CMAKE_BUILD_TYPE:STRING=Release)" 'the line CMAKE_BUILD_TYPE:STRING=Release occurs 0 time'
refuse build-dup "$(printf '%s\nCMAKE_BUILD_TYPE:STRING=Release' "$g")" 'the line CMAKE_BUILD_TYPE:STRING=Release occurs 2 time'
run empty-cache "$(tree empty-cache '' "$good_dump")"
want_rc 1; has 'OPUS_STATIC_RUNTIME:BOOL=ON occurs 0 time'; has 'CMAKE_BUILD_TYPE:STRING=Release occurs 0 time'; lacks ': ok:'; nerr 2; done_case

# the error carries the file and the line number of the offending line
n="$(($(wc -l <<<"$g") + 1))"
run line-number "$(tree line-number "$(printf '%s\nCMAKE_MSVC_RUNTIME_LIBRARY:STRING=MultiThreadedDLL' "$g")" "$good_dump")"
want_rc 1; has "debug/build/opusic-sys-0123456789abcdef/out/build/CMakeCache\.txt:$n: CMAKE_MSVC_RUNTIME_LIBRARY: "; done_case

# CR handling: one CR is removed, no more, and removing it hides nothing
t="$tmp/crlf-bad"; put "$t" debug 1 - "$good_dump"
sed 's/$/\r/' <<<"$(swap OPUS_STATIC_RUNTIME:BOOL=ON OPUS_STATIC_RUNTIME:BOOL=OFF)" > "$t/debug/build/opusic-sys-1/out/build/CMakeCache.txt"
run crlf-bad "$t"
want_rc 1; has 'OPUS_STATIC_RUNTIME: a line that is not an owned line'; lacks ': ok:'; done_case

t="$tmp/crlf-double"; put "$t" debug 1 - "$good_dump"
sed 's/^OPUS_STATIC_RUNTIME:BOOL=ON$/&\r\r/' <<<"$g" > "$t/debug/build/opusic-sys-1/out/build/CMakeCache.txt"
run crlf-double "$t"
want_rc 1; has 'OPUS_STATIC_RUNTIME'; lacks ': ok:'; done_case

t="$tmp/no-eol-bad"; put "$t" debug 1 - "$good_dump"
printf '%s\nCMAKE_MSVC_RUNTIME_LIBRARY:STRING=MultiThreadedDLL' "$g" > "$t/debug/build/opusic-sys-1/out/build/CMakeCache.txt"
run no-eol-bad "$t"
want_rc 1; has 'CMAKE_MSVC_RUNTIME_LIBRARY: a line .*line text \[CMAKE_MSVC_RUNTIME_LIBRARY:STRING=MultiThreadedDLL\]$'; lacks ': ok:'; done_case

# --- every cache counts, in any position ------------------------------------------------------
bad_off="$(swap OPUS_STATIC_RUNTIME:BOOL=ON OPUS_STATIC_RUNTIME:BOOL=OFF)"
dbg='debug/build/opusic-sys-0123456789abcdef/out/build/CMakeCache\.txt'
rel='release/build/opusic-sys-fedcba9876543210/out/build/CMakeCache\.txt'
run one-bad "$(tree2 one-bad "$g" "$good_dump" "$bad_off" "$good_dump")"
want_rc 1; has "$rel"; lacks "$dbg"; lacks ': ok:'; done_case
run one-bad-first "$(tree2 one-bad-first "$bad_off" "$good_dump" "$g" "$good_dump")"
want_rc 1; has "$dbg"; lacks "$rel"; lacks ': ok:'; done_case
run both-bad "$(tree2 both-bad "$bad_off" "$good_dump" "$(printf '%s\nCMAKE_MSVC_RUNTIME_LIBRARY:STRING=MultiThreadedDLL' "$g")" "$good_dump")"
want_rc 1; has "$dbg"; has "$rel"; lacks ': ok:'; done_case
t="$(tree2 three-caches "$g" "$good_dump" "$bad_off" "$good_dump")"
put "$t" x86_64-pc-windows-msvc/release 00112233aabbccdd "$g" "$good_dump"
run three-caches "$t"
want_rc 1; has "$rel"; lacks 'x86_64-pc-windows-msvc'; lacks ': ok:'; done_case

# --- opus.lib: the effect (D2) ----------------------------------------------------------------
lib() { # lib <label> <dump text> <regex>: good cache, this dump
  run "$1" "$(tree "$1" "$g" "$2")"
  want_rc 1; has "$3"; lacks ': ok:'; done_case
}
lib lib-msvcrt "$(dump /DEFAULTLIB:MSVCRT /DEFAULTLIB:OLDNAMES)" 'debug/build/opusic-sys-0123456789abcdef/out/lib/opus\.lib: names /DEFAULTLIB:MSVCRT'
lib lib-msvcrt-lower "$(dump /defaultlib:msvcrt /DEFAULTLIB:OLDNAMES)" 'opus\.lib: names /DEFAULTLIB:MSVCRT'
lib lib-msvcrt-mixed "$(dump /DefaultLib:MsvcRt)" 'opus\.lib: names /DEFAULTLIB:MSVCRT'
lib lib-msvcrt-dot-lib "$(dump /DEFAULTLIB:msvcrt.lib)" 'opus\.lib: names /DEFAULTLIB:MSVCRT'
lib lib-both "$(dump /DEFAULTLIB:LIBCMT /DEFAULTLIB:MSVCRT)" 'opus\.lib: names /DEFAULTLIB:MSVCRT'
lib lib-both-last "$(dump /DEFAULTLIB:MSVCRT /DEFAULTLIB:LIBCMT /DEFAULTLIB:OLDNAMES)" 'opus\.lib: names /DEFAULTLIB:MSVCRT'
lib lib-no-libcmt "$(dump /DEFAULTLIB:OLDNAMES)" 'opus\.lib: no /DEFAULTLIB:LIBCMT'
lib lib-no-directives "$(dump)" 'opus\.lib: no /DEFAULTLIB:LIBCMT'
lib lib-empty-dump '' 'opus\.lib: no /DEFAULTLIB:LIBCMT'
# a directive that is not a DEFAULTLIB one, or names LIBCMT only as a path, is not the token
lib lib-libcmt-elsewhere "$(dump '/EXPORT:LIBCMT' /DEFAULTLIB:OLDNAMES)" 'opus\.lib: no /DEFAULTLIB:LIBCMT'

# dumpbin fails: its exit code decides even when its text looks good
t="$(tree dumpbin-fail "$g" "$good_dump")"; echo 1 > "$t/$dbgd/lib/opus.lib.rc"
run dumpbin-fail "$t"
want_rc 1; has 'opus\.lib: dumpbin exited 1'; lacks ': ok:'; nerr 1; done_case
t="$(tree dumpbin-fail-77 "$g" "$good_dump")"; echo 77 > "$t/$dbgd/lib/opus.lib.rc"
run dumpbin-fail-77 "$t"
want_rc 1; has 'opus\.lib: dumpbin exited 77'; lacks ': ok:'; done_case

# a cache without its lib (build failed after configure, or a stale half tree): fail closed
run lib-missing "$(tree lib-missing "$g" -)"
want_rc 1; has 'opus\.lib: not a file'; lacks ': ok:'; nerr 1; done_case

# the lib verdict of every build counts, in either position
run one-bad-lib "$(tree2 one-bad-lib "$g" "$good_dump" "$g" "$(dump /DEFAULTLIB:MSVCRT)")"
want_rc 1; has 'release/build/opusic-sys-fedcba9876543210/out/lib/opus\.lib: names /DEFAULTLIB:MSVCRT'; lacks 'debug/build/opusic-sys-0123456789abcdef/out/lib'; lacks ': ok:'; nerr 1; done_case
run one-bad-lib-first "$(tree2 one-bad-lib-first "$g" "$(dump /DEFAULTLIB:MSVCRT)" "$g" "$good_dump")"
want_rc 1; has 'debug/build/opusic-sys-0123456789abcdef/out/lib/opus\.lib: names /DEFAULTLIB:MSVCRT'; lacks 'release/build/opusic-sys-fedcba9876543210/out/lib'; lacks ': ok:'; nerr 1; done_case

# both halves are reported: a bad cache and a bad lib in one build are two errors
run cache-and-lib "$(tree cache-and-lib "$bad_off" "$(dump /DEFAULTLIB:MSVCRT)")"
want_rc 1; has 'OPUS_STATIC_RUNTIME'; has 'opus\.lib: names /DEFAULTLIB:MSVCRT'; lacks ': ok:'; done_case

# the effect check does not replace the cache check: a good lib with a bad cache still refuses
run good-lib-bad-cache "$(tree good-lib-bad-cache "$bad_off" "$good_dump")"
want_rc 1; has 'OPUS_STATIC_RUNTIME'; lacks 'opus\.lib'; lacks ': ok:'; done_case

# no dumpbin anywhere: cannot run (exit 3), never a pass. PATH holds the tools the script needs,
# not dumpbin.
mkdir -p "$tmp/nodump-path"
for tool in find grep sed awk cat dirname sort tr head tail cut wc basename readlink; do
  p="$(command -v "$tool" 2>/dev/null)" && ln -sf "$p" "$tmp/nodump-path/$tool"
done
label=no-dumpbin; ok=1
out="$(env -u DUMPBIN PATH="$tmp/nodump-path" VSWHERE="$tmp/no-vswhere.exe" "$BASH" "$script" "$(tree no-dumpbin "$g" "$good_dump")" 2>&1)"; rc=$?
want_rc 3; has 'cannot run'; has 'dumpbin'; lacks ': ok:'; done_case

echo "opus-cmake-cache.test: $passed passed, $failed failed"
[ "$failed" = 0 ]
