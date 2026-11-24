#!/usr/bin/env bash
# T-261 (decisions #361, option A; docs/tasks/T-261.md): the contour's background prebuild. Started by the
# teamwright-agent.yml step "Prebuild before the stage" (owned copy scripts/ci/contour-prebuild.yml) with nohup,
# in the hop's tree as cwd; reported by the inline step "Prebuild report" (scripts/ci/contour-prebuild-report.yml).
#
# Runs the gate's own Rust commands, the Makefile recipe lines of check-core, check-whisper and
# check-shell-windows byte for byte (the cargo test line with --no-run appended), so the session's first gate
# finds their artefacts in target/ (same args and env, same fingerprints). The lines below that start with
# 'scripts/tw-run core -- ' are owned: make check-contour-prebuild compares them with the Makefile.
#
# Env: RUNNER_TEMP (required), GITHUB_RUN_ID, GITHUB_RUN_ATTEMPT (the container name), TW_PREBUILD_TIMEOUT
# (seconds for the whole sequence, default 1500), TW_DOCKER (docker client of tw-run and of the rm below).
# Writes the exit code of the sequence to $RUNNER_TEMP/prebuild.rc (0, non-zero, 124 on the timeout), the
# output to $RUNNER_TEMP/prebuild.log and, while the sequence runs, its process group to
# $RUNNER_TEMP/prebuild.pid. Never exits non-zero: the outcome is the report step's warning.
set -u

if [ "${1:-}" = --sequence ]; then
  # Every line runs, also after a failed one (a lint error in one crate still warms the others); the exit
  # code is the first non-zero one.
  rc=0
  trap 'r=$?; [ "$rc" != 0 ] || rc=$r' ERR
  scripts/tw-run core -- 'cargo fmt --check -p voicen-core && cargo clippy -p voicen-core --all-targets -- -D warnings'
  scripts/tw-run core -- cargo test -p voicen-core --no-run
  scripts/tw-run core -- 'cargo fmt --check -p voicen-whisper && DOCS_RS=1 cargo clippy -p voicen-whisper --all-targets -- -D warnings'
  scripts/tw-run core -- 'cargo fmt --check -p voicen && DOCS_RS=1 cargo clippy -p voicen --target x86_64-pc-windows-gnu -- -D warnings && DOCS_RS=1 cargo clippy -p voicen --target x86_64-pc-windows-gnu --all-targets -- -D warnings'
  trap - ERR
  exit "$rc"
fi

rt="${RUNNER_TEMP:-}"
[ -n "$rt" ] && [ -d "$rt" ] || { echo "contour-prebuild: RUNNER_TEMP is not a directory" >&2; exit 0; }
name="tw-prebuild-${GITHUB_RUN_ID:-0}-${GITHUB_RUN_ATTEMPT:-0}"
limit="${TW_PREBUILD_TIMEOUT:-1500}"
[[ "$limit" =~ ^[0-9]+$ ]] || limit=1500
dk="${TW_DOCKER:-docker}"
export TW_DOCKER_ARGS="--name $name -e CARGO_BUILD_JOBS=2"

# One host-side bound over the whole sequence; timeout leads its own process group and signals all of it.
# Its pid (= that group) is in prebuild.pid while it runs, so the report step can stop a sequence still running
# at the session's end before it removes the container.
timeout "$limit" bash "${BASH_SOURCE[0]}" --sequence > "$rt/prebuild.log" 2>&1 < /dev/null &
tpid=$!
echo "$tpid" > "$rt/prebuild.pid"
wait "$tpid"
rc=$?
rm -f "$rt/prebuild.pid"
if [ "$rc" = 124 ]; then
  # tw-run's docker run --rm has no init: killing its client does not stop the container.
  "$dk" rm -f "$name" >> "$rt/prebuild.log" 2>&1 || true
fi
echo "$rc" > "$rt/prebuild.rc"
exit 0
