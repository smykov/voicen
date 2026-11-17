#!/bin/sh
# ENTRYPOINT of the voicen-rust image (docker/rust.Dockerfile; T-217, decisions #304,
# docs/decisions/ci-toolchain.md "One shared sccache cache for every tree's core runs").
#
#   voicen-core-entry.sh <command> [args...]      (scripts/tw-run passes `sh -c '<cmd>' sh ...`)
#
# The only place RUSTC_WRAPPER is set. With a usable shared cache directory ($SCCACHE_DIR,
# /tw-sccache, mounted by scripts/tw-run from <main checkout>/.teamwright/cache/core/shared/
# _tw-sccache only when that host dir exists) the command's rustc runs go through sccache:
# the container holding `flock -n $SCCACHE_DIR/.rw.lock` for its whole life runs its server
# READ_WRITE, every other container READ_ONLY (sccache's local cache allows one writer at a
# time). Otherwise (dir absent, not writable by the container user, lock or server failure)
# it prints exactly one `tw-sccache: ... plain rustc ...` line and runs the command as is.
#
# Contract: every stderr line it adds starts with `tw-sccache: ` (the stats table of
# TW_SCCACHE_STATS=1 is sccache's own output); it never writes to stdout (scripts/licenses/
# rust.sh, ban-canary.sh and shell-test-layout.sh parse tw-run's stdout); the exit code is the
# command's; it needs no writable $HOME. TW_SCCACHE_STATS=1: one `tw-sccache: mode=<mode>` line
# before the command and `sccache --show-stats` on stderr after it.

say() { printf 'tw-sccache: %s\n' "$*" >&2; }

# Fallback: one line, no wrapper, the command replaces this shell (exit code unchanged).
plain() {
  say "$1; running plain rustc (no compiler cache)"
  shift
  unset RUSTC_WRAPPER SCCACHE_LOCAL_RW_MODE
  exec 9>&-
  exec "$@"
}

[ $# -gt 0 ] || set -- bash

dir="${SCCACHE_DIR:-/tw-sccache}"
lock="$dir/.rw.lock"
command -v sccache >/dev/null 2>&1 || plain "sccache not installed" "$@"
[ -d "$dir" ] || plain "no shared cache directory $dir" "$@"
[ -w "$dir" ] || plain "shared cache directory $dir is not writable" "$@"
{ [ -e "$lock" ] || : >>"$lock"; } 2>/dev/null
[ -r "$lock" ] || plain "cannot open $lock" "$@"
# `command exec`: a failed redirection must not end the shell before the fallback line. The
# 2>/dev/null sits on the group, so it is undone after it (on `exec` it would stay for good).
{ command exec 9<"$lock"; } 2>/dev/null || plain "cannot open $lock" "$@"

flock -n 9
case $? in
  0) mode=READ_WRITE ;;
  1) mode=READ_ONLY ;;
  *) plain "cannot lock $lock" "$@" ;;
esac

export SCCACHE_DIR="$dir" SCCACHE_LOCAL_RW_MODE="$mode"
# A server I/O failure falls back to a local compile instead of failing the build; the server
# lives as long as this container (no idle shutdown before the final stats).
export SCCACHE_IGNORE_SERVER_IO_ERROR=1 SCCACHE_IDLE_TIMEOUT=0
# Only this shell holds the lock: neither the server nor the command inherits fd 9.
sccache --start-server >/dev/null 2>&1 </dev/null 9>&- || plain "sccache server did not start" "$@"

[ "${TW_SCCACHE_STATS:-}" = 1 ] && say "mode=$mode"
export RUSTC_WRAPPER=sccache
"$@" 9>&-
rc=$?
[ "${TW_SCCACHE_STATS:-}" = 1 ] && sccache --show-stats >&2 2>/dev/null 9>&-
sccache --stop-server >/dev/null 2>&1 9>&-
exit "$rc"
