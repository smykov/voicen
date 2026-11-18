#!/bin/sh
# ENTRYPOINT of the voicen-rust image (docker/rust.Dockerfile; T-217, decisions #304,
# docs/decisions/ci-toolchain.md "One shared sccache cache for every tree's core runs").
#
#   voicen-core-entry.sh <command> [args...]      (scripts/tw-run passes `sh -c '<cmd>' sh ...`)
#
# The only place RUSTC_WRAPPER is set. Layout: scripts/tw-run mounts <main checkout>/
# .teamwright/cache/core/shared/_tw-sccache at the mount root /tw-sccache ($TW_SCCACHE_MOUNT)
# only when that host dir exists. sccache's cache is the subdirectory $SCCACHE_DIR
# (/tw-sccache/cache, image ENV; created here); the single-writer lock is /tw-sccache/.rw.lock,
# OUTSIDE $SCCACHE_DIR, because sccache's LRU eviction deletes every file under $SCCACHE_DIR and
# a lock that is never accessed would be its first victim (review round 1, finding 1). With a
# usable mount the command's rustc runs go through sccache: the container holding `flock -n` on
# the lock for its whole life runs its server READ_WRITE, every other container READ_ONLY
# (sccache's local cache allows one writer at a time). Otherwise (mount absent, not writable by
# the container user, cache dir, lock or server failure, a server start that takes longer than
# $TW_SCCACHE_START_TIMEOUT seconds, default 20) it prints exactly one
# `tw-sccache: ... plain rustc ...` line and runs the command as is.
#
# Contract: every stderr line it adds starts with `tw-sccache: ` (the stats table of
# TW_SCCACHE_STATS=1 is sccache's own output); it never writes to stdout (scripts/licenses/
# rust.sh, ban-canary.sh and shell-test-layout.sh parse tw-run's stdout); the exit code is the
# command's; it needs no writable $HOME. TW_SCCACHE_STATS=1: one `tw-sccache: mode=<mode>` line
# before the command and `sccache --show-stats` on stderr after it.

say() { printf 'tw-sccache: %s\n' "$*" >&2; }

# Fallback: one line, no wrapper, the command replaces this shell (exit code unchanged). The
# lock is given back, except after a failed server start (keep_lock=1): a half-started server
# of this container may still be alive, so the writer role stays here until the container ends.
keep_lock=0
plain() {
  say "$1; running plain rustc (no compiler cache)"
  shift
  unset RUSTC_WRAPPER SCCACHE_LOCAL_RW_MODE
  [ "$keep_lock" = 1 ] || exec 9>&-
  exec "$@"
}

[ $# -gt 0 ] || set -- bash

mount="${TW_SCCACHE_MOUNT:-/tw-sccache}"
dir="${SCCACHE_DIR:-$mount/cache}"
lock="$mount/.rw.lock"
command -v sccache >/dev/null 2>&1 || plain "sccache not installed" "$@"
[ -d "$mount" ] || plain "no shared cache directory $mount" "$@"
[ -w "$mount" ] || plain "shared cache directory $mount is not writable" "$@"
mkdir -p "$dir" 2>/dev/null && [ -w "$dir" ] || plain "cannot create cache directory $dir" "$@"
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
# Only this shell holds the lock: neither the server nor the command inherits fd 9. The start
# is bounded (coreutils timeout signals its whole process group, KILL 2 s after TERM); on a
# failure or timeout a bounded best-effort stop, then the fallback. Any server left over dies
# with the container, which ends when the command (PID 1 after exec) ends.
t="${TW_SCCACHE_START_TIMEOUT:-20}"
if ! timeout -k 2 "$t" sccache --start-server >/dev/null 2>&1 </dev/null 9>&-; then
  keep_lock=1
  timeout -k 1 5 sccache --stop-server >/dev/null 2>&1 </dev/null 9>&-
  plain "sccache server did not start within ${t}s" "$@"
fi

[ "${TW_SCCACHE_STATS:-}" = 1 ] && say "mode=$mode"
export RUSTC_WRAPPER=sccache
"$@" 9>&-
rc=$?
[ "${TW_SCCACHE_STATS:-}" = 1 ] && sccache --show-stats >&2 2>/dev/null 9>&-
sccache --stop-server >/dev/null 2>&1 9>&-
exit "$rc"
