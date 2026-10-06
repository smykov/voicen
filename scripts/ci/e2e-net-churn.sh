#!/usr/bin/env bash
# T-064 reproduction (M2 of class e2e-boot-flake, decisions #77, #89): host Docker network
# churn must not reach the e2e browser. Run it on the HOST (it needs Docker and must not run
# inside the ui container).
#
# While the e2e entry point runs the whole suite (default `pnpm e2e`, with --retries=0 and
# --repeat-each=<n>), a background loop churns Docker user networks the way other jobs on a
# shared host do, the condition T-050 measured (Investigation M2): per cycle
#   docker network create <net>; docker run --rm --network <net> <image> true; docker network rm <net>
# A container must be attached: create/rm alone gave 40/40 clean runs in T-050.
#
# Fails (exit 1) when any of these holds:
#   - netns   the e2e run's Playwright worker (the probe e2e/netns-probe.spec.ts, opted in by
#             target/e2e-netns-probe/enabled) reports the host's /proc/self/ns/net, or reports
#             nothing (the probe did not run, so the run's namespace is unknown). Deterministic.
#   - boot    the run's output names net::ERR_NETWORK_CHANGED (a page boot aborted by a network
#             change). Statistical: before T-064 about 3 in 100 tests under churn.
#   - suite   the e2e run exits non-zero (any failed test).
#   - churn   zero completed churn cycles: a run without churn proves nothing.
# Exit 3: cannot run (no docker, churn image missing). Never a pass.
#
# Usage: scripts/ci/e2e-net-churn.sh
# Environment:
#   E2E_CHURN_REPEAT  --repeat-each for the suite (default 3, i.e. about 350 tests)
#   E2E_CHURN_IMAGE   image of the attached container (default voicen-rust:1.99, `make
#                     core-image`); must be present, it is never pulled
#   E2E_CHURN_ENTRY   the e2e entry point (default `pnpm e2e`); the script appends
#                     --retries=0 --repeat-each=<n> --reporter=list
# Prints one summary line:
#   e2e-net-churn: cycles=<n> churn_errors=<n> failed=<n> passed=<n> boot_failed=<n>
#                  err_network_changed_lines=<n> netns host=<..> run=<..>
#   (boot_failed: failed tests whose report names ERR_NETWORK_CHANGED)
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
unset BASE_URL

repeat="${E2E_CHURN_REPEAT:-3}"
image="${E2E_CHURN_IMAGE:-voicen-rust:1.99}"
entry="${E2E_CHURN_ENTRY:-pnpm e2e}"
docker="${TW_DOCKER:-docker}"

command -v "$docker" >/dev/null 2>&1 \
  || { echo "e2e-net-churn: cannot run: $docker not found" >&2; exit 3; }
"$docker" image inspect "$image" >/dev/null 2>&1 \
  || { echo "e2e-net-churn: cannot run: churn image $image is not present (make core-image, or set E2E_CHURN_IMAGE)" >&2; exit 3; }
host_ns="$(readlink /proc/self/ns/net)" \
  || { echo "e2e-net-churn: cannot run: cannot read /proc/self/ns/net" >&2; exit 3; }

probe="target/e2e-netns-probe"
tmp="$(mktemp -d)" || { echo "e2e-net-churn: cannot run: mktemp failed" >&2; exit 3; }
prefix="t064-churn-$$"
churn_pid=""
cleanup() {
  touch "$tmp/stop" 2>/dev/null
  [ -n "$churn_pid" ] && wait "$churn_pid" 2>/dev/null
  # Networks a stopped or interrupted cycle left behind.
  "$docker" network ls --format '{{.Name}}' 2>/dev/null | grep "^$prefix-" \
    | xargs -r "$docker" network rm >/dev/null 2>&1
  rm -f "$probe/enabled"
  rm -rf "$tmp"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

rm -rf "$probe"
mkdir -p "$probe" && touch "$probe/enabled" \
  || { echo "e2e-net-churn: cannot run: cannot create $probe" >&2; exit 3; }

churn() {
  local i=0 net
  while [ ! -e "$tmp/stop" ]; do
    i=$((i + 1))
    net="$prefix-$i"
    if "$docker" network create "$net" >/dev/null 2>&1; then
      if "$docker" run --rm --pull never --network "$net" "$image" true >/dev/null 2>&1; then
        "$docker" network rm "$net" >/dev/null 2>&1 && echo "$i" >>"$tmp/cycles"
      else
        "$docker" network rm "$net" >/dev/null 2>&1
        echo "$i" >>"$tmp/churn-errors"
      fi
    else
      echo "$i" >>"$tmp/churn-errors"
      sleep 1
    fi
  done
}
churn &
churn_pid=$!

echo "=== e2e-net-churn: $entry --retries=0 --repeat-each=$repeat (churn image $image)"
# shellcheck disable=SC2086 # the entry is a command line
$entry --retries=0 --repeat-each="$repeat" --reporter=list 2>&1 | tee "$tmp/out"
rc=${PIPESTATUS[0]}

touch "$tmp/stop"
wait "$churn_pid" 2>/dev/null
churn_pid=""

cycles=0; [ -f "$tmp/cycles" ] && cycles="$(wc -l <"$tmp/cycles")"
churn_errors=0; [ -f "$tmp/churn-errors" ] && churn_errors="$(wc -l <"$tmp/churn-errors")"
sed 's/\x1b\[[0-9;]*m//g' "$tmp/out" >"$tmp/plain"
nchanged="$(grep -c 'ERR_NETWORK_CHANGED' "$tmp/plain")"
nfailed="$(sed -n 's/^ *\([0-9][0-9]*\) failed.*/\1/p' "$tmp/plain" | tail -n1)"
npassed="$(sed -n 's/^ *\([0-9][0-9]*\) passed.*/\1/p' "$tmp/plain" | tail -n1)"
# Failed tests whose report names ERR_NETWORK_CHANGED (the list reporter's "  N) [project] ..."
# blocks up to the "N failed" summary).
nboot="$(awk '/^  [0-9]+\) \[/ { if (id) n += (c > 0); id = 1; c = 0 }
              /ERR_NETWORK_CHANGED/ { c++ }
              /^  [0-9]+ failed/ { if (id) n += (c > 0); id = 0 }
              END { print n + 0 }' "$tmp/plain")"

run_ns="(none)"
failed=0
shopt -s nullglob
reports=("$probe"/netns-*.txt)
if [ ${#reports[@]} -eq 0 ]; then
  echo "FAIL netns: the probe e2e/netns-probe.spec.ts wrote no report; the run's network namespace is unknown" >&2
  failed=1
else
  run_ns="$(sort -u "${reports[@]}" | paste -sd, -)"
  if grep -qxF -- "$host_ns" "${reports[@]}"; then
    echo "FAIL netns: the e2e run's Playwright worker shares the host's network namespace ($host_ns); host network events reach its browser" >&2
    failed=1
  fi
fi
if [ "$cycles" -eq 0 ]; then
  echo "FAIL churn: no churn cycle completed ($churn_errors failed); the run proves nothing" >&2
  failed=1
fi
if [ "$nchanged" -gt 0 ]; then
  echo "FAIL boot: $nboot failed test(s) name net::ERR_NETWORK_CHANGED ($nchanged line(s) in the run's output)" >&2
  failed=1
fi
if [ "$rc" -ne 0 ]; then
  echo "FAIL suite: the e2e run exited $rc (${nfailed:-?} failed)" >&2
  failed=1
fi
echo "e2e-net-churn: cycles=$cycles churn_errors=$churn_errors failed=${nfailed:-0} passed=${npassed:-0} boot_failed=$nboot err_network_changed_lines=$nchanged netns host=$host_ns run=$run_ns"
exit "$failed"
