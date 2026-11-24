#!/usr/bin/env bash
# T-261 (decisions #361, option A; docs/tasks/T-261.md; checker: true, analysis.decides_on (2)): host test of the
# contour's background prebuild, scripts/ci/contour-prebuild.sh, of the report step's run: block and of the pins
# that tie the script to the gate's own Rust commands.
#
# INTERFACE ASSUMPTIONS (the developer implements to these; every line is pinned by a case below):
#
#   contour-prebuild.sh
#     - Is a file with the executable bit (the step may call it directly) and also runs as `bash <file>`. It runs
#       with the hop's tree as cwd (the step's working directory) and calls `scripts/tw-run core -- ...` by that
#       relative path, so a fixture tree with its own scripts/tw-run, scripts/hooks/_config.py and
#       .teamwright/config.yml is enough to run it against a stub docker.
#     - Reads RUNNER_TEMP (required), GITHUB_RUN_ID, GITHUB_RUN_ATTEMPT (the container name) and
#       TW_PREBUILD_TIMEOUT (seconds, default 1500). TW_DOCKER (honoured by scripts/tw-run: "docker-compatible
#       binary") is the docker client of the tw-run calls AND of the script's own `docker rm -f`.
#     - Exports TW_DOCKER_ARGS='--name tw-prebuild-<GITHUB_RUN_ID>-<GITHUB_RUN_ATTEMPT> -e CARGO_BUILD_JOBS=2' (so
#       every tw-run call gets them) and runs, in order, under ONE host-side timeout, the lines of the file that
#       (leading blanks stripped) start with 'scripts/tw-run core -- '. Those lines are the Makefile's recipe
#       lines of check-core, check-whisper and check-shell-windows byte for byte, the one line
#       'scripts/tw-run core -- cargo test -p voicen-core' with ' --no-run' appended (no make parsing: owned lines).
#     - Writes the exit code of the sequence to $RUNNER_TEMP/prebuild.rc (a single number: 0, or non-zero; 124 on
#       the timeout) and the output to $RUNNER_TEMP/prebuild.log. Never exits non-zero (any failed line, a missing
#       docker, a timeout). On the timeout it runs `<TW_DOCKER> rm -f tw-prebuild-<id>-<attempt>` (killing the
#       docker client does not stop the container).
#
#   contour-prebuild.yml (owned copy of the step "      - name: Prebuild before the stage")
#     - Line 1 is that title, `id: prebuild` is a line of it, it starts the script with nohup and a trailing `&`
#       (the step ends at once); no `if:` is required (default success()).
#
#   contour-prebuild-report.yml (owned copy of the step "      - name: Prebuild report")
#     - Line 1 is that title, a line `        if: always() && steps.prebuild.outcome == 'success'`, and a block
#       scalar `        run: |` whose body (10 spaces of indent) is run here as `bash -e <body>` with a stub `docker`
#       first on PATH and RUNNER_TEMP, GITHUB_RUN_ID, GITHUB_RUN_ATTEMPT set. It runs no script of the tree (the
#       session can edit the tree): the body names no `scripts/`. It calls `docker rm -f
#       tw-prebuild-<id>-<attempt>` (always) and prints exactly one `::warning::` line unless $RUNNER_TEMP/prebuild.rc
#       holds exactly 0 (missing, empty, non-zero, 124: one warning). It never exits non-zero (also when docker
#       rm fails).
#
# Usage: scripts/ci/contour-prebuild.test.sh   (host bash, awk)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
script=scripts/ci/contour-prebuild.sh
pbyml=scripts/ci/contour-prebuild.yml
repyml=scripts/ci/contour-prebuild-report.yml
mk=Makefile
tmp="$(mktemp -d)" || { echo "contour-prebuild.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

failed=0
passed=0
fail() {
  echo "FAIL $1" >&2
  [ -z "${2:-}" ] || sed 's/^/       | /' <<<"$2" >&2
  failed=$((failed + 1))
}
pass() {
  echo "ok   $1"
  passed=$((passed + 1))
}
# expect <label> <command...>
expect() {
  local label="$1"
  shift
  if "$@"; then pass "$label"; else fail "$label"; fi
}
# need <file> <label> <command...>: like expect, but a missing owned file is its own clear FAIL.
need() {
  local f="$1" label="$2"
  shift 2
  if [ ! -f "$f" ]; then
    fail "$label: missing $f (the developer creates it; see the interface assumptions at the top of $0)"
  else
    expect "$label" "$@"
  fi
}

# ---- owned lines: the script's tw-run lines against the Makefile's (decides_on (2)) -------------------------
# script_lines <file>: the lines that, leading blanks stripped, start with 'scripts/tw-run core -- '.
script_lines() { awk -v p='scripts/tw-run core -- ' '{ sub(/^[ \t]+/, "") } index($0, p) == 1 { print }' "$1"; }
# make_lines <file>: the TAB + 'scripts/tw-run core -- ' lines in the recipe blocks (TAB lines right after a
# header line) of check-core:, check-whisper:, check-shell-windows: (TAB stripped); the cargo test line gets
# ' --no-run'.
make_lines() {
  awk -v p='scripts/tw-run core -- ' '
    /^\t/ { if (blk && index(substr($0, 2), p) == 1) {
              l = substr($0, 2)
              if (l == "scripts/tw-run core -- cargo test -p voicen-core") l = l " --no-run"
              print l } next }
    { blk = ($0 ~ /^(check-core|check-whisper|check-shell-windows):/) }' "$1"
}
# owned_equal <script> <makefile>: same lines in the same order and count, and there are some.
owned_equal() {
  [ -n "$(make_lines "$2")" ] && cmp -s <(script_lines "$1") <(make_lines "$2")
}
expect "Makefile: the three recipe blocks hold exactly 4 'scripts/tw-run core -- ' lines" \
  test "$(make_lines "$mk" | wc -l | tr -d ' ')" = 4
expect "Makefile: the cargo test line of check-core is the one that gets ' --no-run'" \
  test "$(make_lines "$mk" | grep -c -x -F 'scripts/tw-run core -- cargo test -p voicen-core --no-run')" = 1
need "$script" "owned lines: the tw-run lines of $script equal the Makefile's recipe lines of check-core, check-whisper, check-shell-windows (cargo test line + ' --no-run'), in order and count" \
  owned_equal "$script" "$mk"
need "$script" "$script is executable" test -x "$script"

# A fixture only counts when the real pair is equal first (else a missing script would make every "red" vacuous).
fx_differs() { owned_equal "$script" "$mk" && ! owned_equal "$1" "$2"; }
fx_same() { owned_equal "$script" "$mk" && owned_equal "$1" "$2"; }
# mk_fixture <name> <awk program>: a Makefile copy through the awk program; FAIL when it did not change.
mk_fixture() {
  local n="$1"
  mkdir -p "$tmp/mk-$n"
  awk "$2" "$mk" > "$tmp/mk-$n/Makefile"
  if cmp -s "$mk" "$tmp/mk-$n/Makefile"; then
    fail "Makefile fixture $n did not apply (the mutation changed nothing)"
    return 1
  fi
}
if mk_fixture changed-lint '/^\t/ && /voicen-whisper/ && /-D warnings/ { sub(/-D warnings/, "-D warning") } { print }'; then
  need "$script" "owned lines: a Makefile copy with a changed check-whisper tw-run line is red" \
    fx_differs "$script" "$tmp/mk-changed-lint/Makefile"
fi
if mk_fixture changed-test '$0 == "\tscripts/tw-run core -- cargo test -p voicen-core" { print $0 " --lib"; next } { print }'; then
  need "$script" "owned lines: a Makefile copy with a changed cargo test line is red" \
    fx_differs "$script" "$tmp/mk-changed-test/Makefile"
fi
if mk_fixture added-line '{ print } $0 == "check-shell-windows:" { f = 1; next } f && /area-image.sh --present core/ { print "\tscripts/tw-run core -- '"'"'cargo build -p voicen-core'"'"'"; f = 0 }'; then
  need "$script" "owned lines: a Makefile copy with an added tw-run line in check-shell-windows is red" \
    fx_differs "$script" "$tmp/mk-added-line/Makefile"
fi
if mk_fixture other-target '{ print } $0 == "check-contour-busy:" { print "\tscripts/tw-run core -- true" }'; then
  need "$script" "owned lines: an added tw-run line in a recipe of another target is not owned (stays equal)" \
    fx_same "$script" "$tmp/mk-other-target/Makefile"
fi
# Script copies: a changed, a dropped and an added tw-run line against the real Makefile are red.
sc_fixture() { # <name> <awk program>
  mkdir -p "$tmp/sc-$1"
  [ -f "$script" ] && awk "$2" "$script" > "$tmp/sc-$1/s.sh"
  if [ ! -f "$script" ] || cmp -s "$script" "$tmp/sc-$1/s.sh"; then
    fail "script fixture $1 did not apply ($script missing or the mutation changed nothing)"
    return 1
  fi
}
if sc_fixture changed '/^[ \t]*scripts\/tw-run core -- / && !d { $0 = $0 " #x"; d = 1 } { print }'; then
  expect "owned lines: a script copy with a changed tw-run line is red" fx_differs "$tmp/sc-changed/s.sh" "$mk"
fi
if sc_fixture dropped '/^[ \t]*scripts\/tw-run core -- / && !d { d = 1; next } { print }'; then
  expect "owned lines: a script copy with a dropped tw-run line is red" fx_differs "$tmp/sc-dropped/s.sh" "$mk"
fi
if sc_fixture added '{ print } END { print "scripts/tw-run core -- true" }'; then
  expect "owned lines: a script copy with an added tw-run line is red" fx_differs "$tmp/sc-added/s.sh" "$mk"
fi

# ---- the script against a stub docker (TW_DOCKER) ----------------------------------------------------------------
# A fixture tree with the real tw-run, its config reader and the config, and the script under test.
fxt="$tmp/tree"
mkdir -p "$fxt/scripts/hooks" "$fxt/scripts/ci" "$fxt/.teamwright"
cp scripts/tw-run "$fxt/scripts/tw-run"
cp scripts/hooks/_config.py "$fxt/scripts/hooks/_config.py"
cp .teamwright/config.yml "$fxt/.teamwright/config.yml"
[ -f "$script" ] && cp "$script" "$fxt/$script"
stub="$tmp/docker-stub"
cat > "$stub" <<'EOF'
#!/usr/bin/env bash
# Records one line of argv per call; STUB_MODE: ok (exit 0), fail (exit 1, also for rm), sleep (sleeps; a kill ends it).
printf '%s\n' "$*" >> "$STUB_LOG"
[ "${1:-}" = rm ] && [ "${STUB_MODE:-ok}" != fail ] && exit 0
case "${STUB_MODE:-ok}" in
  fail) exit 1 ;;
  sleep) exec sleep 30 ;;
esac
exit 0
EOF
chmod +x "$stub"
# pb_run <name> <mode> <docker> [extra env...]: runs the script in the fixture tree; sets $pb_exit, $pb_rt (RUNNER_TEMP), $pb_log.
pb_run() {
  local n="$1" mode="$2" dk="$3" t0
  shift 3
  pb_rt="$tmp/rt-$n"
  pb_log="$tmp/docker-$n.log"
  mkdir -p "$pb_rt"
  : > "$pb_log"
  t0=$SECONDS
  (cd "$fxt" && env RUNNER_TEMP="$pb_rt" GITHUB_RUN_ID=4242 GITHUB_RUN_ATTEMPT=1 TW_DOCKER="$dk" \
    STUB_LOG="$pb_log" STUB_MODE="$mode" "$@" timeout 60 bash "$script" > "$tmp/pb-$n.out" 2>&1)
  pb_exit=$?
  pb_secs=$((SECONDS - t0))
}
rc_of() { cat "$pb_rt/prebuild.rc" 2>/dev/null; }
runs() { grep -c '^run ' "$pb_log"; }

ok_checks() { # the stub exits 0
  [ "$pb_exit" = 0 ] && [ "$(rc_of)" = 0 ] && [ -f "$pb_rt/prebuild.log" ]
}
if [ -f "$script" ]; then
  pb_run ok ok "$stub"
else
  pb_exit=127 pb_rt="$tmp/rt-ok" pb_log="$tmp/docker-ok.log"; mkdir -p "$pb_rt"; : > "$pb_log"
fi
need "$script" "stub docker exit 0: the script exits 0, prebuild.rc is 0 and prebuild.log exists" ok_checks
four_calls() { [ "$(runs)" = 4 ]; }
need "$script" "stub docker exit 0: exactly four docker run calls (one per owned tw-run line)" four_calls
named() { [ "$(runs)" = 4 ] && [ "$(grep '^run ' "$pb_log" | grep -c -F -e '--name tw-prebuild-4242-1 ')" = 4 ]; }
need "$script" "every call is named --name tw-prebuild-<GITHUB_RUN_ID>-<GITHUB_RUN_ATTEMPT> (4242-1)" named
jobs2() { [ "$(runs)" = 4 ] && [ "$(grep '^run ' "$pb_log" | grep -c -F -e '-e CARGO_BUILD_JOBS=2 ')" = 4 ]; }
need "$script" "every call carries -e CARGO_BUILD_JOBS=2" jobs2
order() {
  local i=0 l
  [ "$(runs)" = 4 ] || return 1
  while IFS= read -r l; do
    i=$((i + 1))
    case "$i" in
      1) grep -q -F 'cargo clippy -p voicen-core --all-targets' <<<"$l" || return 1 ;;
      2) grep -q -F 'test -p voicen-core --no-run' <<<"$l" || return 1 ;;
      3) grep -q -F 'voicen-whisper' <<<"$l" || return 1 ;;
      4) grep -q -F 'x86_64-pc-windows-gnu' <<<"$l" || return 1 ;;
    esac
  done < <(grep '^run ' "$pb_log")
}
need "$script" "the calls run in order: check-core lint, check-core test --no-run, check-whisper, check-shell-windows" order
# tw-run runs sh -c '<$1> "$@"' sh <rest>: for the unquoted cargo test line argv holds `cargo "$@" sh test -p voicen-core --no-run`.
norun_once() {
  [ "$(grep '^run ' "$pb_log" | grep -c -F -e '--no-run')" = 1 ] && grep '^run ' "$pb_log" | grep -F -e '--no-run' | grep -q -F 'test -p voicen-core --no-run'
}
need "$script" "exactly one call carries --no-run and it is the cargo test of voicen-core (without it the tests would run)" norun_once

if [ -f "$script" ]; then pb_run fail fail "$stub"; fi
fail_checks() {
  local rc
  rc="$(rc_of)"
  [ "$pb_exit" = 0 ] && [[ "$rc" =~ ^[0-9]+$ ]] && [ "$rc" != 0 ]
}
need "$script" "stub docker exit 1: the script still exits 0 and prebuild.rc is a non-zero number" fail_checks

if [ -f "$script" ]; then pb_run nodocker ok "$tmp/no-such-docker"; fi
nodocker_checks() {
  local rc
  rc="$(rc_of)"
  [ "$pb_exit" = 0 ] && [[ "$rc" =~ ^[0-9]+$ ]] && [ "$rc" != 0 ]
}
need "$script" "no docker binary: the script exits 0 and prebuild.rc is a non-zero number" nodocker_checks

if [ -f "$script" ]; then pb_run timeout sleep "$stub" TW_PREBUILD_TIMEOUT=2; fi
timeout_checks() { [ "$pb_exit" = 0 ] && [ "$(rc_of)" = 124 ]; }
need "$script" "stub docker sleeping past TW_PREBUILD_TIMEOUT=2: the script exits 0 and prebuild.rc is 124" timeout_checks
rm_called() { grep -q -x -F 'rm -f tw-prebuild-4242-1' "$pb_log"; }
need "$script" "on the timeout the script removes the named container: a call 'rm -f tw-prebuild-4242-1'" rm_called
bounded() { [ "$pb_secs" -lt 25 ]; }
need "$script" "the timeout bounds the whole sequence (it returned in under 25 s, four sleeps would take 120 s)" bounded

# ---- the report step's run: block (from the owned copy) -------------------------------------------------------------
rep_body() { awk '/^        run: \|[-+]?$/ { r = 1; next } r { if ($0 ~ /^          /) print substr($0, 11); else if ($0 ~ /^ *$/) print ""; else exit }' "$1" 2>/dev/null; }
need "$pbyml" "$pbyml: line 1 is the step title '      - name: Prebuild before the stage'" \
  test "$(head -n 1 "$pbyml" 2>/dev/null)" = '      - name: Prebuild before the stage'
need "$pbyml" "$pbyml: the step has id: prebuild" grep -q -x -F '        id: prebuild' "$pbyml"
need "$pbyml" "$pbyml: the step starts the script with nohup, in the background (a line with nohup and contour-prebuild.sh, a line ending in ' &')" \
  bash -c 'grep -q -F nohup "$1" && grep -q -F scripts/ci/contour-prebuild.sh "$1" && grep -q -E " &$" "$1"' _ "$pbyml"
need "$repyml" "$repyml: line 1 is the step title '      - name: Prebuild report'" \
  test "$(head -n 1 "$repyml" 2>/dev/null)" = '      - name: Prebuild report'
need "$repyml" "$repyml: if: always() && steps.prebuild.outcome == 'success'" \
  grep -q -x -F "        if: always() && steps.prebuild.outcome == 'success'" "$repyml"
need "$repyml" "$repyml: the run: body runs no script of the tree (no 'scripts/') and has a body" \
  bash -c '[ -n "$1" ] && ! grep -q -F "scripts/" <<<"$1"' _ "$(rep_body "$repyml")"

stubbin="$tmp/stubbin"
mkdir -p "$stubbin" "$tmp/empty"
cp "$stub" "$stubbin/docker"
printf '%s\n' "$(rep_body "$repyml")" > "$tmp/report.sh"
# rep_run <name> <rc content | "-" for a missing file> [docker mode]: runs the body as bash -e.
rep_run() {
  local n="$1" rcv="$2" mode="${3:-ok}"
  rp_rt="$tmp/rrt-$n"
  rp_log="$tmp/rdocker-$n.log"
  mkdir -p "$rp_rt"
  : > "$rp_log"
  [ "$rcv" = "-" ] || printf '%s' "$rcv" > "$rp_rt/prebuild.rc"
  (cd "$tmp/empty" && env PATH="$stubbin:$PATH" RUNNER_TEMP="$rp_rt" GITHUB_RUN_ID=4242 GITHUB_RUN_ATTEMPT=1 \
    STUB_LOG="$rp_log" STUB_MODE="$mode" bash -e "$tmp/report.sh" > "$tmp/rep-$n.out" 2>&1)
  rp_exit=$?
}
warns() { grep -c '^::warning::' "$tmp/rep-$1.out"; }
rep_case() { # <name> <rc content> <want warnings> <docker mode> <what>
  local n="$1" want="$3"
  if [ ! -f "$repyml" ] || [ -z "$(rep_body "$repyml")" ]; then
    fail "report step, $5: missing or empty run: body in $repyml (the developer creates it)"
    return
  fi
  rep_run "$n" "$2" "$4"
  if [ "$rp_exit" = 0 ] && [ "$(warns "$n")" = "$want" ] && grep -q -x -F 'rm -f tw-prebuild-4242-1' "$rp_log"; then
    pass "report step, $5: exit 0, $want ::warning:: line(s), docker rm -f tw-prebuild-4242-1 called"
  else
    fail "report step, $5: want exit 0, $want ::warning:: line(s), the rm call; got exit $rp_exit, $(warns "$n") warning(s), docker log: $(tr '\n' '|' < "$rp_log")" "$(cat "$tmp/rep-$n.out")"
  fi
}
rep_case missing - 1 ok "prebuild.rc missing"
rep_case empty "" 1 ok "prebuild.rc empty"
rep_case one 1 1 ok "prebuild.rc is 1"
rep_case t124 124 1 ok "prebuild.rc is 124 (timeout)"
rep_case t124-rmfail 124 1 fail "prebuild.rc is 124 and docker rm -f itself fails"
rep_case zero 0 0 ok "prebuild.rc is 0 (no warning)"
rep_case zero-nl $'0\n' 0 ok "prebuild.rc is 0 with a trailing newline (as echo writes it)"

# ---- summary --------------------------------------------------------------------------------------------------------
if [ "$failed" -gt 0 ]; then
  echo "contour-prebuild.test: FAIL: $failed case(s) differ, $passed as expected (T-261)" >&2
  exit 1
fi
echo "contour-prebuild.test: ok: $passed cases as expected"
