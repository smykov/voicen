#!/usr/bin/env bash
# T-099: host test of scripts/ci/crt-imports.sh, the windows job's verdict on the C runtime of
# every exe linking whisper.cpp (invariant 4), with a stub dumpbin (DUMPBIN) that prints a canned
# dependency list per exe (offline). Cases:
#   static        UCRT api sets and KERNEL32 only -> exit 0;
#   two-good      two static exes -> exit 0, "2 exe(s)";
#   msvcp         MSVCP140.dll imported -> exit 1 naming MSVCP140;
#   vcruntime     VCRUNTIME140.dll -> exit 1;
#   vcruntime-1   VCRUNTIME140_1.dll only -> exit 1;
#   lower         msvcp140.dll in lower case -> exit 1;
#   delay-load    MSVCP140.dll in the delay-load list only -> exit 1;
#   one-bad       a static exe and an MSVCP140 exe -> exit 1 naming only the bad exe;
#   no-file       an exe path that does not exist (an unmatched glob) -> exit 1;
#   empty         dumpbin prints nothing (exit 0) -> exit 1;
#   dumpbin-fail  dumpbin exits 1 -> exit 1;
#   dash-args     the stub sees -nologo -dependents <exe> (Git Bash keeps "-" arguments);
#   no-dumpbin    DUMPBIN unset, no dumpbin on PATH, no vswhere -> exit 3;
#   usage         no argument -> exit 2.
# Usage: scripts/ci/crt-imports.test.sh   (host bash)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
script="$root/scripts/ci/crt-imports.sh"
[ -x "$script" ] || { echo "crt-imports.test: cannot run: $script not found or not executable" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "crt-imports.test: cannot run: mktemp" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

# Stub dumpbin: prints <exe>.deps (canned body) and exits with <exe>.rc (default 0); logs argv.
stub="$tmp/dumpbin"
cat > "$stub" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${STUB_LOG:?}"
exe="${!#}"
cat "$exe.deps" 2>/dev/null
exit "$(cat "$exe.rc" 2>/dev/null || echo 0)"
EOF
chmod +x "$stub"
export STUB_LOG="$tmp/argv.log"

# exe <name> <dependency lines...>: a fake exe and the dump the stub prints for it.
exe() {
  local f="$tmp/$1.exe"; shift
  : > "$f"
  {
    printf 'Dump of file %s\r\n\r\nFile Type: EXECUTABLE IMAGE\r\n\r\n  Image has the following dependencies:\r\n\r\n' "$f"
    local d
    for d in "$@"; do printf '    %s\r\n' "$d"; done
    printf '\r\n  Summary\r\n\r\n        1000 .data\r\n'
  } > "$f.deps"
  echo "$f"
}
static=(KERNEL32.dll api-ms-win-crt-runtime-l1-1-0.dll api-ms-win-crt-heap-l1-1-0.dll USER32.dll)

failed=0
passed=0
fail() { echo "FAIL $label: $1" >&2; sed 's/^/       | /' <<<"$out" >&2; failed=$((failed + 1)); ok=0; }
run() { label="$1"; ok=1; shift; out="$(DUMPBIN="$stub" "$script" "$@" 2>&1)"; rc=$?; }
want_rc() { [ "$rc" = "$1" ] || fail "exit $rc, want $1"; }
has() { grep -qE -- "$1" <<<"$out" || fail "output lacks /$1/"; }
lacks() { ! grep -qE -- "$1" <<<"$out" || fail "output has /$1/"; }
done_case() { [ "$ok" = 1 ] && { echo "ok   $label"; passed=$((passed + 1)); }; }

run static "$(exe static "${static[@]}")"
want_rc 0; has '^crt-imports: ok: 1 exe'; lacks '::error::'; done_case

run two-good "$(exe good-a "${static[@]}")" "$(exe good-b "${static[@]}")"
want_rc 0; has '^crt-imports: ok: 2 exe'; done_case

run msvcp "$(exe msvcp "${static[@]}" MSVCP140.dll)"
want_rc 1; has '^::error::.*msvcp\.exe: imports the dynamic VC runtime \(MSVCP140\)'; lacks ': ok:'; done_case

run vcruntime "$(exe vcruntime "${static[@]}" VCRUNTIME140.dll)"
want_rc 1; has 'VCRUNTIME140'; lacks ': ok:'; done_case

run vcruntime-1 "$(exe vcruntime-1 "${static[@]}" VCRUNTIME140_1.dll)"
want_rc 1; has 'VCRUNTIME140'; lacks ': ok:'; done_case

run lower "$(exe lower "${static[@]}" msvcp140.dll)"
want_rc 1; has 'MSVCP140'; lacks ': ok:'; done_case

f="$(exe delay-load "${static[@]}")"
printf '  Image has the following delay load dependencies:\r\n\r\n    MSVCP140.dll\r\n' >> "$f.deps"
run delay-load "$f"
want_rc 1; has 'MSVCP140'; lacks ': ok:'; done_case

good="$(exe one-good "${static[@]}")"; bad="$(exe one-bad "${static[@]}" MSVCP140.dll)"
run one-bad "$good" "$bad"
want_rc 1; has 'one-bad\.exe: imports'; lacks '::error::.*one-good\.exe'; lacks ': ok:'; done_case

run no-file "$tmp/target/debug/deps/transcribe_wav-*.exe"
want_rc 1; has '^::error::.*not a file'; lacks ': ok:'; done_case

f="$tmp/empty.exe"; : > "$f"; : > "$f.deps"
run empty "$f"
want_rc 1; has 'no dependency list'; lacks ': ok:'; done_case

f="$(exe dumpbin-fail "${static[@]}")"; echo 1 > "$f.rc"
run dumpbin-fail "$f"
want_rc 1; has 'dumpbin exited 1'; lacks ': ok:'; done_case

: > "$STUB_LOG"
f="$(exe dash-args "${static[@]}")"
run dash-args "$f"
want_rc 0
grep -qxF -- "-nologo -dependents $f" "$STUB_LOG" || fail "stub argv $(cat "$STUB_LOG"), want -nologo -dependents $f"
done_case

label=no-dumpbin; ok=1
out="$(env -u DUMPBIN PATH="$tmp/empty-path" VSWHERE="$tmp/no-vswhere.exe" "$BASH" "$script" "$(exe nd "${static[@]}")" 2>&1)"; rc=$?
want_rc 3; has 'cannot run'; done_case

run usage
want_rc 2; done_case

echo "crt-imports.test: $passed passed, $failed failed"
[ "$failed" = 0 ]
