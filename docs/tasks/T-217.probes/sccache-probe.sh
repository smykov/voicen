#!/usr/bin/env bash
# T-217 probe: shared sccache cache for scripts/tw-run core (decisions #304).
#
#   docs/tasks/T-217.probes/sccache-probe.sh [--full] [--log <file>]
#
# Needs Docker and the image from the working tree's docker/rust.Dockerfile
# (`make core-image` -> voicen-rust:1.99). Exit 0 only when every check passes.
# Lives under docs/ and not wired into `make check` on purpose: wiring touches a guard
# path (Makefile) and the task has checker: false. Run it by hand on a host with Docker.
#
# Checks (RED = fails on the code before T-217; CHAR = characterization, green today and
# must stay green):
#   C1  AC-1   `sccache --show-stats` works in the main tree and in a worktree, same cache
#              location (/tw-sccache/cache), and tw-run mounts the MAIN checkout's shared dir in both
#   C2  AC-2   (--full only) two fresh worktrees run `cargo check -p voicen-core`; the second
#              reports non-zero cache hits; both wall times are printed
#   C3  AC-4   failure branch: shared dir missing, and shared dir unwritable -> command succeeds,
#              exactly one stderr line, no wrapper, plain rustc works  (RED)
#   C3c        stdout is exactly the command's stdout and the exit code is the command's, with
#              the dir absent / present / unwritable, with and without TW_SCCACHE_STATS  (CHAR)
#   C4  AC-5   SCCACHE_CACHE_SIZE and SCCACHE_DIR set in the image; RUSTC_WRAPPER set nowhere
#              but docker/voicen-core-entry.sh; docs say where the cache lives and how to clear it
#   C5  invariant  at most one READ_WRITE server on the shared dir at a time; the writer role is
#              given back when its container ends
#   C3e AC-4   failure branch: the server start HANGS -> bounded by TW_SCCACHE_START_TIMEOUT,
#              fallback line, exit code kept (review round 1, finding 4)
#   C6  invariant  review round 1 findings 1+3: a writer driven past a small SCCACHE_CACHE_SIZE
#              (lock older than every cache entry) keeps /tw-sccache/.rw.lock, and a second
#              container building at the same time is READ_ONLY and builds fine
#
# Contract the entry script (docker/voicen-core-entry.sh, the image ENTRYPOINT) must meet:
#   - every stderr line it adds starts with `tw-sccache: `
#   - sccache usable: with TW_SCCACHE_STATS=1 it prints exactly one `tw-sccache: mode=READ_WRITE`
#     or `tw-sccache: mode=READ_ONLY` line before the command, exports SCCACHE_LOCAL_RW_MODE with
#     the same value and RUSTC_WRAPPER=sccache; READ_WRITE only for the container that got
#     `flock -n /tw-sccache/.rw.lock`; at exit with TW_SCCACHE_STATS=1 it prints
#     `sccache --show-stats` to stderr. Without TW_SCCACHE_STATS it prints no tw-sccache line.
#   - fallback (/tw-sccache absent or not writable by the container user, flock or server
#     failure): exactly ONE `tw-sccache: ... plain rustc ...` line on stderr (with or without
#     TW_SCCACHE_STATS), RUSTC_WRAPPER empty, the command still runs.
#   - never writes to stdout, exit code = the command's, does not need a writable $HOME.
#   - layout: tw-run mounts the host dir at /tw-sccache; the image ENV is SCCACHE_DIR=/tw-sccache/cache
#     (the entry creates it when /tw-sccache is writable); the lock is /tw-sccache/.rw.lock, OUTSIDE
#     SCCACHE_DIR (sccache's LRU eviction deletes any file under SCCACHE_DIR, the lock included).
#     The fallback conditions are about /tw-sccache (absent / not writable).
#   - the server start is bounded by `timeout $TW_SCCACHE_START_TIMEOUT` (default 20 s); a start that
#     fails or times out takes the fallback (one line, exit code of the command kept).
#   - tw-run mounts <main checkout>/.teamwright/cache/core/shared/_tw-sccache at /tw-sccache
#     only when that host dir exists (main checkout = parent of the git common dir).
set -u

FULL=0; LOG=""
while [ $# -gt 0 ]; do
  case "$1" in
    --full) FULL=1 ;;
    --log) LOG="$2"; shift ;;
    *) echo "usage: $0 [--full] [--log <file>]" >&2; exit 2 ;;
  esac
  shift
done
if [ -n "$LOG" ]; then exec > >(tee "$LOG") 2>&1; fi

MAIN="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$MAIN" || exit 2
SHARED_PARENT="$MAIN/.teamwright/cache/core/shared"
SHARED="$SHARED_PARENT/_tw-sccache"
WORK="$(mktemp -d)"
WT_A="$WORK/wt-a"; WT_B="$WORK/wt-b"
CREATED_SHARED=0; CREATED_PARENT=0
FAILS=0; CHARFAILS=0

cleanup() {
  for t in "$WT_A" "$WT_B"; do
    [ -d "$t" ] && git -C "$MAIN" worktree remove --force "$t" >/dev/null 2>&1
  done
  git -C "$MAIN" worktree prune >/dev/null 2>&1
  # the container runs as the caller, but a probe-made dir may hold root-owned files
  if [ "$CREATED_SHARED" = 1 ]; then chmod -R u+rwX "$SHARED" 2>/dev/null; rm -rf "$SHARED"; fi
  if [ "$CREATED_PARENT" = 1 ]; then rmdir "$SHARED_PARENT" 2>/dev/null; fi
  rm -rf "$WORK"
}
trap cleanup EXIT

pass() { echo "PASS  [$1] $2"; }
fail() {
  echo "FAIL  [$1] $2"
  case "$1" in CHAR*) CHARFAILS=$((CHARFAILS + 1)) ;; *) FAILS=$((FAILS + 1)) ;; esac
}
# check <id> <description> <0|nonzero status of the assertion>
check() { if [ "$3" = 0 ]; then pass "$1" "$2"; else fail "$1" "$2"; fi; }

# --- shared dir state (the probe owns the dir only when it created it) -------------------
OWN=1
if [ -e "$SHARED" ]; then OWN=0; echo "NOTE  $SHARED exists: probe will not remove or chmod it (C3 absent/unwritable cases skipped)"; fi
make_shared() {
  [ "$OWN" = 1 ] || return 0
  [ -d "$SHARED_PARENT" ] || { mkdir -p "$SHARED_PARENT" && CREATED_PARENT=1; }
  if [ ! -d "$SHARED" ]; then mkdir -p "$SHARED" && CREATED_SHARED=1; fi
  chmod 777 "$SHARED"
}
drop_shared() { [ "$OWN" = 1 ] && { chmod -R u+rwX "$SHARED" 2>/dev/null; rm -rf "$SHARED"; }; }
[ -d "$SHARED_PARENT" ] || CREATED_PARENT=$OWN
[ "$OWN" = 1 ] && CREATED_SHARED=1

# --- worktrees: HEAD plus the working copy of the files under test ------------------------
mkwt() {
  git -C "$MAIN" worktree add --detach "$1" HEAD >/dev/null 2>&1 || { echo "cannot create worktree $1"; exit 2; }
  cp "$MAIN/scripts/tw-run" "$1/scripts/tw-run"
  cp "$MAIN/.teamwright/config.yml" "$1/.teamwright/config.yml"
}

# run <tree> <outfile-prefix> <TW_DOCKER_ARGS or ""> -- <command>; sets RC, OUT, ERR
run() {
  local tree="$1" pfx="$2" dargs="$3"; shift 4
  OUT="$pfx.out"; ERR="$pfx.err"
  ( cd "$tree" && TW_DOCKER_ARGS="$dargs" scripts/tw-run core -- "$@" ) >"$OUT" 2>"$ERR"
  RC=$?
}
tw_lines() { grep -c '^tw-sccache: ' "$1"; }

make_shared
mkwt "$WT_A"
echo "== probe: main=$MAIN worktree=$WT_A shared=$SHARED =="

# ---------------------------------------------------------------- C1
run "$MAIN" "$WORK/c1m" "" -- 'sccache --show-stats'
check C1a "main tree: sccache --show-stats exits 0 (rc=$RC)" "$RC"
run "$WT_A" "$WORK/c1w" "" -- 'sccache --show-stats'
check C1b "worktree: sccache --show-stats exits 0 (rc=$RC)" "$RC"
LM="$(grep -i 'cache location' "$WORK/c1m.out")"; LW="$(grep -i 'cache location' "$WORK/c1w.out")"
[ -n "$LM" ] && [ "$LM" = "$LW" ] && case "$LM" in *'"/tw-sccache/cache"'*|*' /tw-sccache/cache'*) true ;; *) false ;; esac
check C1c "main and worktree report the same cache location /tw-sccache/cache (main='$LM' worktree='$LW')" "$?"
for t in "$MAIN" "$WT_A"; do
  P="$(cd "$t" && TW_RUN_PRINT=1 scripts/tw-run core -- true 2>/dev/null)"
  case "$P" in *"$SHARED:/tw-sccache"*) st=0 ;; *) st=1 ;; esac
  check C1d "TW_RUN_PRINT in $(basename "$t") mounts the main checkout's $SHARED at /tw-sccache" "$st"
done

# ---------------------------------------------------------------- C3 / C3c / stats contract
echo "== C3: failure branch and wrapper hygiene =="
fallback_case() { # <label> <extra docker args>
  local label="$1" dargs="$2"
  run "$MAIN" "$WORK/fb" "$dargs" -- 'echo "w=${RUSTC_WRAPPER-}"; rustc --version'
  local n; n="$(tw_lines "$ERR")"
  { [ "$RC" = 0 ] && [ "$n" = 1 ] && grep -q '^tw-sccache: .*plain rustc' "$ERR" \
      && grep -qx 'w=' "$OUT" && grep -q '^rustc ' "$OUT"; }
  check C3 "$label: exit 0, exactly one 'tw-sccache: ... plain rustc' stderr line, RUSTC_WRAPPER empty, rustc runs (rc=$RC, tw-sccache lines=$n)" "$?"
  run "$MAIN" "$WORK/fb1" "-e TW_SCCACHE_STATS=1 $dargs" -- 'true'
  check C3 "$label + TW_SCCACHE_STATS=1: still exactly one tw-sccache line (lines=$(tw_lines "$ERR"))" "$([ "$(tw_lines "$ERR")" = 1 ] && echo 0 || echo 1)"
}
if [ "$OWN" = 1 ]; then
  drop_shared
  fallback_case "shared dir missing" ""
  make_shared
  chmod a-w "$SHARED"
  # root bypasses chmod: run as an unprivileged uid with a writable HOME
  fallback_case "shared dir read-only" "-u 65534:65534 -e HOME=/tmp"
  chmod 777 "$SHARED"
else
  echo "SKIP  C3 missing/unwritable (real shared dir present)"
fi

# C3d: the server does not start (a shim mounted over /usr/local/bin/sccache refuses --start-server)
make_shared
printf '#!/bin/sh\ncase "$1" in --start-server) echo "shim: refusing to start" >&2; exit 1 ;; esac\nexit 0\n' >"$WORK/sccache-shim"
chmod 755 "$WORK/sccache-shim"
run "$MAIN" "$WORK/c3d" "-e TW_SCCACHE_STATS=1 -v $WORK/sccache-shim:/usr/local/bin/sccache:ro" -- 'echo "w=${RUSTC_WRAPPER-}"; rustc --version; exit 5'
n="$(tw_lines "$ERR")"
{ [ "$RC" = 5 ] && [ "$n" = 1 ] && grep -q '^tw-sccache: .*plain rustc' "$ERR" \
    && grep -qx 'w=' "$OUT" && grep -q '^rustc ' "$OUT"; }
check C3d "server start fails: command still runs, its exit code (5) kept, exactly one 'tw-sccache: ... plain rustc' line, RUSTC_WRAPPER empty (rc=$RC, lines=$n)" "$?"

# C3f (round 2): the mount is writable but the cache subdirectory cannot be created (a regular
# file sits at $SHARED/cache) -> fallback, not a server on a missing cache dir
if [ "$OWN" = 1 ]; then
  chmod -R u+rwX "$SHARED/cache" 2>/dev/null; rm -rf "$SHARED/cache"; : >"$SHARED/cache"
  fallback_case "cache directory not creatable (regular file at the path)" ""
  rm -f "$SHARED/cache"
else
  echo "SKIP  C3f cache dir not creatable (real shared dir present)"
fi

# C3e: the server start hangs (a shim sleeps on --start-server); the entry must bound it
make_shared
printf '#!/bin/sh\ncase "$1" in --start-server) sleep 300 ;; esac\nexit 0\n' >"$WORK/sccache-hang"
chmod 755 "$WORK/sccache-hang"
CN="t217-c3e-$$"
t0=$(date +%s)
( cd "$MAIN" && TW_DOCKER_ARGS="--name $CN -e TW_SCCACHE_START_TIMEOUT=3 -e TW_SCCACHE_STATS=1 -v $WORK/sccache-hang:/usr/local/bin/sccache:ro" \
    timeout 90 scripts/tw-run core -- 'echo "w=${RUSTC_WRAPPER-}"; rustc --version; exit 5' ) >"$WORK/c3e.out" 2>"$WORK/c3e.err"
RC=$?; ERR="$WORK/c3e.err"; OUT="$WORK/c3e.out"
t1=$(date +%s)
docker rm -f "$CN" >/dev/null 2>&1
n="$(tw_lines "$ERR")"; el=$((t1 - t0))
{ [ "$RC" = 5 ] && [ "$n" = 1 ] && grep -q '^tw-sccache: .*plain rustc' "$ERR" \
    && grep -qx 'w=' "$OUT" && grep -q '^rustc ' "$OUT" && [ "$el" -lt 60 ]; }
check C3e "server start hangs (TW_SCCACHE_START_TIMEOUT=3): bounded, fallback, exit code 5 kept, exactly one 'plain rustc' line, RUSTC_WRAPPER empty, done in <60 s (rc=$RC, lines=$n, elapsed=${el}s)" "$?"

stdout_exit_case() { # <label> <extra docker args> <TW_SCCACHE_STATS value or "">
  local label="$1" dargs="$2" st="$3" envs=""
  [ -n "$st" ] && dargs="-e TW_SCCACHE_STATS=1 $dargs"
  envs="$dargs"
  run "$MAIN" "$WORK/so" "$envs" -- 'echo out'
  check CHAR3c "$label: stdout of 'echo out' is exactly 'out' and rc 0 (rc=$RC)" "$({ [ "$RC" = 0 ] && [ "$(cat "$OUT")" = out ]; } && echo 0 || echo 1)"
  run "$MAIN" "$WORK/ex" "$envs" -- 'exit 7'
  check CHAR3c "$label: 'exit 7' returns 7 (rc=$RC) and stdout empty" "$({ [ "$RC" = 7 ] && [ ! -s "$OUT" ]; } && echo 0 || echo 1)"
}
if [ "$OWN" = 1 ]; then
  drop_shared
  stdout_exit_case "dir absent" "" ""
  stdout_exit_case "dir absent, stats" "" 1
  make_shared
  stdout_exit_case "dir present" "" ""
  stdout_exit_case "dir present, stats" "" 1
  chmod a-w "$SHARED"
  stdout_exit_case "dir unwritable" "-u 65534:65534 -e HOME=/tmp" ""
  stdout_exit_case "dir unwritable, stats" "-u 65534:65534 -e HOME=/tmp" 1
  chmod 777 "$SHARED"
else
  stdout_exit_case "real shared dir present" "" ""
  stdout_exit_case "real shared dir present, stats" "" 1
fi

# stats contract on a usable dir
make_shared
run "$MAIN" "$WORK/st" "-e TW_SCCACHE_STATS=1" -- 'echo "w=${RUSTC_WRAPPER-} m=${SCCACHE_LOCAL_RW_MODE-}"'
{ [ "$RC" = 0 ] && [ "$(tw_lines "$ERR")" = 1 ] && grep -qE '^tw-sccache: mode=(READ_WRITE|READ_ONLY)$' "$ERR" \
    && grep -q 'Compile requests' "$ERR" && grep -q '^w=sccache' "$OUT" && grep -q 'm=READ_' "$OUT"; }
check C3s "usable dir + TW_SCCACHE_STATS=1: one 'tw-sccache: mode=' line, stats on stderr, RUSTC_WRAPPER=sccache (rc=$RC)" "$?"
run "$MAIN" "$WORK/st0" "" -- 'echo out'
check C3s "usable dir, no TW_SCCACHE_STATS: no tw-sccache line, stdout 'out' (lines=$(tw_lines "$ERR"))" "$({ [ "$(tw_lines "$ERR")" = 0 ] && [ "$(cat "$OUT")" = out ]; } && echo 0 || echo 1)"

# ---------------------------------------------------------------- C4
echo "== C4: size cap, wrapper placement, docs =="
run "$MAIN" "$WORK/c4" "" -- 'printenv SCCACHE_CACHE_SIZE'
check C4a "SCCACHE_CACHE_SIZE is set and non-empty in the image (rc=$RC, value='$(cat "$OUT")')" "$({ [ "$RC" = 0 ] && [ -s "$OUT" ]; } && echo 0 || echo 1)"
run "$MAIN" "$WORK/c4b" "" -- 'printenv SCCACHE_DIR'
check C4b "SCCACHE_DIR is /tw-sccache/cache, below the mount point (value='$(cat "$OUT")')" "$([ "$(cat "$OUT")" = /tw-sccache/cache ] && echo 0 || echo 1)"
BAD="$(git -C "$MAIN" grep -nEi 'RUSTC_WRAPPER|rustc-wrapper' -- .cargo Makefile '*.mk' scripts/ci .github src-tauri crates 2>/dev/null)"
check CHAR4c "RUSTC_WRAPPER not set in .cargo, Makefile, scripts/ci, .github, crates ('${BAD:-none}')" "$([ -z "$BAD" ] && echo 0 || echo 1)"
{ [ -f docker/voicen-core-entry.sh ] && grep -q 'RUSTC_WRAPPER' docker/voicen-core-entry.sh \
    && ! grep -Eq '^[[:space:]]*ENV[[:space:]].*RUSTC_WRAPPER' docker/rust.Dockerfile \
    && grep -q 'voicen-core-entry.sh' docker/rust.Dockerfile && grep -Eq '^ENTRYPOINT' docker/rust.Dockerfile \
    && grep -q 'sha256sum -c' docker/rust.Dockerfile && grep -q 'SCCACHE_CACHE_SIZE' docker/rust.Dockerfile; }
check C4d "RUSTC_WRAPPER only in docker/voicen-core-entry.sh; Dockerfile has ENTRYPOINT, sha256-checked sccache, SCCACHE_CACHE_SIZE, no ENV RUSTC_WRAPPER" "$?"
D=docs/decisions/ci-toolchain.md
{ grep -q 'SCCACHE_CACHE_SIZE' $D && grep -q 'shared/_tw-sccache' $D && grep -Eq 'rm -rf .*shared/_tw-sccache' $D \
    && grep -Eq 'mkdir -p .*shared/_tw-sccache' $D; }
check C4e "$D names SCCACHE_CACHE_SIZE, the host dir, the one-time mkdir and the clear command (rm -rf .../shared/_tw-sccache)" "$?"
grep -q 'sccache' CLAUDE.md
check C4f "CLAUDE.md Commands mentions the shared sccache cache" "$?"
grep -q 'shared_cache' .teamwright/config.yml && grep -q 'shared_cache' scripts/tw-run && grep -q 'shared_cache' docs/kit-feedback.md
check C4g "shared_cache key in .teamwright/config.yml, read by scripts/tw-run, upstream request in docs/kit-feedback.md" "$?"

# ---------------------------------------------------------------- C5
echo "== C5: single writer =="
make_shared
pids=()
for i in 1 2 3 4; do
  ( TW_DOCKER_ARGS="-e TW_SCCACHE_STATS=1" "$MAIN/scripts/tw-run" core -- 'echo "$SCCACHE_LOCAL_RW_MODE"; sleep 20' >"$WORK/c5.$i.out" 2>"$WORK/c5.$i.err" ) &
  pids+=($!)
done
wait "${pids[@]}"
RW=0; RO=0; MISMATCH=0
for i in 1 2 3 4; do
  m="$(sed -n 's/^tw-sccache: mode=//p' "$WORK/c5.$i.err")"
  case "$m" in READ_WRITE) RW=$((RW + 1)) ;; READ_ONLY) RO=$((RO + 1)) ;; esac
  { [ -n "$m" ] && [ "$m" = "$(head -n1 "$WORK/c5.$i.out")" ]; } || MISMATCH=$((MISMATCH + 1))
done
check C5a "4 concurrent containers: exactly one READ_WRITE and three READ_ONLY (rw=$RW ro=$RO)" "$({ [ "$RW" = 1 ] && [ "$RO" = 3 ]; } && echo 0 || echo 1)"
check C5b "each container's SCCACHE_LOCAL_RW_MODE equals its tw-sccache mode line (mismatches=$MISMATCH)" "$([ "$MISMATCH" = 0 ] && echo 0 || echo 1)"
run "$MAIN" "$WORK/c5r" "-e TW_SCCACHE_STATS=1" -- 'true'
check C5c "after all containers ended, the next one is READ_WRITE again (lock released: '$(grep '^tw-sccache:' "$ERR")')" "$(grep -qx 'tw-sccache: mode=READ_WRITE' "$ERR" && echo 0 || echo 1)"

# ---------------------------------------------------------------- C6
# Review round 1 findings 1+3: the lock must survive the writer's LRU eviction, and a reader
# building while the writer inserts/evicts must be READ_ONLY and succeed.
echo "== C6: writer past the cap, concurrent reader =="
if [ "$OWN" = 1 ]; then
  drop_shared; make_shared
  mktoy() { # <dir> <n>: three chained rlib crates, cacheable with CARGO_INCREMENTAL=0
    local d="$1" n="$2" c prev=""
    mkdir -p "$d" && printf '[workspace]\nmembers = ["a", "b", "c"]\nresolver = "2"\n' >"$d/Cargo.toml"
    for c in a b c; do
      mkdir -p "$d/$c/src"
      printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2021"\n' "$c" >"$d/$c/Cargo.toml"
      if [ -n "$prev" ]; then
        printf '[dependencies]\n%s = { path = "../%s" }\n' "$prev" "$prev" >>"$d/$c/Cargo.toml"
        printf 'pub fn v() -> u32 { %s::v() + %s }\n' "$prev" "$n" >"$d/$c/src/lib.rs"
      else
        printf 'pub fn v() -> u32 { %s }\n' "$n" >"$d/$c/src/lib.rs"
      fi
      prev="$c"
    done
  }
  mktoy "$WORK/toy-seed" 1; mktoy "$WORK/toy-w" 2; mktoy "$WORK/toy-r" 3
  mkdir -p "$WORK/sig"
  C6ENV="-e CARGO_INCREMENTAL=0 -e SCCACHE_CACHE_SIZE=14K -e TW_SCCACHE_STATS=1"
  # seed: a writer fills the cache (big cap) with the entries of the first sources
  run "$MAIN" "$WORK/c6s" "-e CARGO_INCREMENTAL=0 -e SCCACHE_CACHE_SIZE=10G -v $WORK/toy-seed:/toy" -- 'cd /toy && cargo build'
  seed_rc=$RC
  find "$SHARED" -type f ! -name .rw.lock | sort >"$WORK/c6.seed"
  seeded=$(wc -l <"$WORK/c6.seed")
  check C6s "seed build succeeds and leaves cache entries (rc=$seed_rc, entries=$seeded)" "$({ [ "$seed_rc" = 0 ] && [ "$seeded" -gt 0 ]; } && echo 0 || echo 1)"
  # steady state: the lock is older than every entry
  touch -d '2000-01-01 00:00:00' "$SHARED/.rw.lock" 2>/dev/null
  # writer: new sources past the 14K cap, then holds its container (and the lock) until told to stop
  ( TW_DOCKER_ARGS="$C6ENV -v $WORK/toy-w:/toy -v $WORK/sig:/sig" "$MAIN/scripts/tw-run" core -- \
      'cd /toy && cargo build; rc=$?; touch /sig/built; i=0; while [ ! -e /sig/done ] && [ $i -lt 150 ]; do sleep 1; i=$((i+1)); done; exit $rc' \
      >"$WORK/c6w.out" 2>"$WORK/c6w.err" ) &
  wpid=$!
  i=0; while [ ! -e "$WORK/sig/built" ] && [ $i -lt 180 ]; do sleep 1; i=$((i + 1)); done
  lock_after_writer=0; [ -e "$SHARED/.rw.lock" ] && lock_after_writer=1
  gone=0; while IFS= read -r f; do [ -e "$f" ] || gone=$((gone + 1)); done <"$WORK/c6.seed"
  # reader: a second container, started while the writer still runs, builds other sources
  ( TW_DOCKER_ARGS="$C6ENV -v $WORK/toy-r:/toy" "$MAIN/scripts/tw-run" core -- 'cd /toy && cargo build' \
      >"$WORK/c6r.out" 2>"$WORK/c6r.err" ) &
  rpid=$!
  wait "$rpid"; r_rc=$?
  lock_after_reader=0; [ -e "$SHARED/.rw.lock" ] && lock_after_reader=1
  touch "$WORK/sig/done"
  wait "$wpid"; w_rc=$?
  wm="$(sed -n 's/^tw-sccache: mode=//p' "$WORK/c6w.err")"; rm_="$(sed -n 's/^tw-sccache: mode=//p' "$WORK/c6r.err")"
  check C6a "writer past the cap: eviction removed seeded entries (evicted=$gone of $seeded), writer is READ_WRITE (mode='$wm'), build rc=$w_rc" "$({ [ "$gone" -gt 0 ] && [ "$wm" = READ_WRITE ] && [ "$w_rc" = 0 ]; } && echo 0 || echo 1)"
  check C6b "/tw-sccache/.rw.lock still exists after the writer's eviction (host: $SHARED/.rw.lock; exists=$lock_after_writer) and after the concurrent reader (exists=$lock_after_reader)" "$({ [ "$lock_after_writer" = 1 ] && [ "$lock_after_reader" = 1 ]; } && echo 0 || echo 1)"
  check C6c "second container, concurrent with the writer, is READ_ONLY (mode='$rm_') and its build exits 0 (rc=$r_rc)" "$({ [ "$rm_" = READ_ONLY ] && [ "$r_rc" = 0 ]; } && echo 0 || echo 1)"
else
  echo "SKIP  C6 (real shared dir present: the probe does not seed or evict it)"
fi

# ---------------------------------------------------------------- C2 (opt-in)
if [ "$FULL" = 1 ]; then
  echo "== C2 (--full): cache hits across fresh worktrees =="
  mkwt "$WT_B"
  t0=$(date +%s)
  run "$WT_A" "$WORK/c2a" "-e TW_SCCACHE_STATS=1" -- 'cargo check -p voicen-core'
  t1=$(date +%s); echo "TIME  worktree A cold: $((t1 - t0)) s (rc=$RC)"
  check C2a "worktree A cargo check succeeds (rc=$RC)" "$RC"
  run "$WT_B" "$WORK/c2b" "-e TW_SCCACHE_STATS=1" -- 'cargo check -p voicen-core'
  t2=$(date +%s); echo "TIME  worktree B with warm cache: $((t2 - t1)) s (rc=$RC)"
  HITS="$(sed -n 's/^Cache hits  *\([0-9][0-9]*\).*/\1/p' "$ERR" | head -n1)"
  check C2b "worktree B reports non-zero Cache hits (hits='${HITS:-none}')" "$([ -n "$HITS" ] && [ "$HITS" -gt 0 ] && echo 0 || echo 1)"
else
  echo "SKIP  C2 (cache hits across worktrees): pass --full (full dependency compile, minutes)"
fi

echo "== result: $FAILS check(s) failed, $CHARFAILS characterization check(s) failed =="
[ "$FAILS" = 0 ] && [ "$CHARFAILS" = 0 ]
