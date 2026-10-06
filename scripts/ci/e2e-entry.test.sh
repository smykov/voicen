#!/usr/bin/env bash
# T-064 fixture tests of the ui e2e entry point (decisions #77, #89; T-064 analysis: seam,
# invariant). No e2e browser shares the host's network namespace: every e2e entry point runs
# Playwright, its webServer and Chromium in the ui image (Playwright image at the lockfile's
# @playwright/test version) with a network namespace of its own; a missing or mismatched image
# stops the run before any test and names the image; it never falls back to the host.
#
# Contract this file pins (the wrapper is project-owned; package.json `scripts.e2e` runs it,
# so `pnpm e2e` on the host and `scripts/tw-run ui -- pnpm e2e` both go through it):
#   pnpm e2e [playwright test args...]
#   Inside the Playwright image = the file $VOICEN_E2E_DOCKER_INFO exists (default
#   /ms-playwright/.docker-info, written by `playwright-core mark-docker-image` into the
#   official image; JSON with driverVersion and dockerImageName):
#     driverVersion == version of node_modules/@playwright/test/package.json (the lockfile's)
#       -> runs `playwright test <args>` directly, args and environment (CI included) unchanged;
#          no docker call.
#     different, or no driverVersion in the file
#       -> exit 3 before any test; the message names both versions and the image
#          (the ui area's image or the file's dockerImageName).
#   On the host (that file absent):
#     image = the ui area's `image` in the teamwright config (.teamwright/config.yml, read the
#     way scripts/tw-run reads it); docker = $TW_DOCKER (default docker).
#     `<docker> image inspect <image>` fails
#       -> exit 3 before any test; the message names the image; no `docker run` and no
#          `docker pull` (a `docker run` would pull it implicitly).
#     present
#       -> re-enters through scripts/tw-run ui (TW_DOCKER_ARGS) with `--network none` and the
#          caller's CI passed through when set (tw-run passes no environment but HOME; CI drives
#          forbidOnly and retries in playwright.config.ts); the image's own check above applies.
#     Never runs Playwright on the host.
#
# How: each case builds a scratch project root (this repo's scripts/, package.json,
# pnpm-lock.yaml; a ui area config with runner docker and the image below; a node_modules with
# @playwright/test's package.json at the real version and fake Playwright CLIs that only record
# their arguments and CI). A fake docker records every call and emulates `docker run` the way
# tw-run uses it: only -e variables reach the command, -v <root>:/work and -w map to the scratch
# root, and the emulated image has $VOICEN_E2E_DOCKER_INFO pointing at the case's .docker-info.
#
# Usage: scripts/ci/e2e-entry.test.sh   (host bash, python3, node)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
unset TEAMWRIGHT_ROOT TEAMWRIGHT_CONFIG TW_DOCKER_ARGS TW_RUN_PRINT BASE_URL VOICEN_E2E_DOCKER_INFO

name="e2e-entry.test"
for t in python3 node; do
  command -v "$t" >/dev/null 2>&1 || { echo "$name: cannot run: $t not found" >&2; exit 3; }
done
version="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' \
  node_modules/@playwright/test/package.json 2>/dev/null)" \
  || { echo "$name: cannot run: node_modules/@playwright/test/package.json not readable (pnpm install)" >&2; exit 3; }
grep -qF "/@playwright/test@$version:" pnpm-lock.yaml \
  || { echo "$name: cannot run: installed @playwright/test $version is not the lockfile's (pnpm install)" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "$name: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

image="voicen-ui-fixture:0.0.0-t064"
other="1.62.9"
nodedir="$(dirname "$(command -v node)")"
fakebin="$tmp/fakebin"
mkdir -p "$fakebin"

# --- fakes ---------------------------------------------------------------------------------
# Fake Playwright CLI: records where it ran, its argv and CI; runs no test.
cat >"$fakebin/playwright" <<'SH'
#!/bin/sh
{
  printf 'container=%s\n' "${FAKE_IN_CONTAINER:-no}"
  printf 'argv:'; for a in "$@"; do printf ' %s' "$a"; done; printf '\n'
  printf 'CI=%s\n' "${CI-__unset__}"
} >>"$FAKE_LOG/playwright.log"
exit 0
SH
fake_cli_js='const fs = require("node:fs");
const e = process.env;
fs.appendFileSync(`${e.FAKE_LOG}/playwright.log`,
  `container=${e.FAKE_IN_CONTAINER || "no"}\nargv: ${process.argv.slice(2).join(" ")}\nCI=${e.CI === undefined ? "__unset__" : e.CI}\n`);'

# Fake pnpm: `pnpm [run] <script>` runs package.json's script with node_modules/.bin on PATH
# (as pnpm does), `pnpm exec <bin>` and `pnpm <bin>` run node_modules/.bin/<bin>.
cat >"$fakebin/pnpm" <<'SH'
#!/usr/bin/env bash
[ "${1:-}" = run ] && shift
export PATH="$PWD/node_modules/.bin:$PATH"
if [ "${1:-}" = exec ]; then shift; exec "$@"; fi
s="$(python3 -c 'import json,sys; print(json.load(open("package.json")).get("scripts",{}).get(sys.argv[1],""))' "${1:-}")"
if [ -n "$s" ]; then shift; exec sh -c "$s \"\$@\"" sh "$@"; fi
[ -x "node_modules/.bin/${1:-}" ] && exec "$@"
echo "fake pnpm: unknown command: $*" >&2
exit 1
SH
cat >"$fakebin/npx" <<'SH'
#!/usr/bin/env bash
while [ "${1:-}" != "${1#-}" ]; do shift; done
export PATH="$PWD/node_modules/.bin:$PATH"
exec "$@"
SH

# Fake docker: records every call to $FAKE_LOG/docker.log. Images present: $FAKE_IMAGES.
cat >"$fakebin/docker" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$FAKE_LOG/docker.log"
present() { case " $FAKE_IMAGES " in *" $1 "*) return 0 ;; esac; return 1; }
case "${1:-}" in
  image|inspect|images)
    sub="$1"; shift
    [ "$sub" = image ] && { sub="${1:-}"; shift; }
    img="${!#}"
    if present "$img"; then
      [ "$sub" = images ] || [ "$sub" = ls ] && { echo "sha256:fixture"; exit 0; }
      echo '[{"Id":"sha256:fixture","RepoTags":["'"$img"'"]}]'; exit 0
    fi
    [ "$sub" = images ] || [ "$sub" = ls ] && exit 0
    echo "Error: No such image: $img" >&2; exit 1 ;;
  pull)
    echo "fake docker: pull refused in fixtures" >&2; exit 1 ;;
  run)
    shift
    if [ -e "$FAKE_LOG/in-run" ]; then
      echo "fake docker: nested docker run (the wrapper re-entered from inside the image)" >&2
      exit 99
    fi
    workdir="/work"; mount=""; network="(default bridge)"; envs=()
    while [ $# -gt 0 ]; do
      case "$1" in
        -e|--env) envs+=("$2"); shift 2 ;;
        --env=*) envs+=("${1#--env=}"); shift ;;
        --network|--net) network="$2"; shift 2 ;;
        --network=*|--net=*) network="${1#*=}"; shift ;;
        -v|--volume) case "$2" in *:/work) mount="${2%:/work}" ;; esac; shift 2 ;;
        -w|--workdir) workdir="$2"; shift 2 ;;
        -u|--user|--pull|--name|--entrypoint|--platform|--add-host|--ipc|--shm-size) shift 2 ;;
        --) shift; break ;;
        -*) shift ;;
        *) break ;;
      esac
    done
    img="$1"; shift
    if ! present "$img"; then
      echo "Unable to find image '$img' locally" >&2
      echo "implicit-pull $img" >>"$FAKE_LOG/docker.log"
      exit 125
    fi
    printf '%s\n' "$network" >>"$FAKE_LOG/network"
    [ -n "$mount" ] || { echo "fake docker: no -v <root>:/work" >&2; exit 125; }
    dir="$mount${workdir#/work}"
    cenv=(PATH="$FAKE_BIN:$FAKE_NODEDIR:/usr/local/bin:/usr/bin:/bin" HOME="$FAKE_LOG/home"
          FAKE_LOG="$FAKE_LOG" FAKE_BIN="$FAKE_BIN" FAKE_NODEDIR="$FAKE_NODEDIR"
          FAKE_IMAGES="$FAKE_IMAGES" FAKE_IN_CONTAINER=1
          VOICEN_E2E_DOCKER_INFO="$FAKE_DOCKER_INFO")
    for kv in ${envs[@]+"${envs[@]}"}; do
      case "$kv" in
        *=*) cenv+=("$kv") ;;
        *) [ -n "${!kv+x}" ] && cenv+=("$kv=${!kv}") ;;
      esac
    done
    # The image's marker file lives at the case's .docker-info.
    args=()
    for a in "$@"; do args+=("${a//\/ms-playwright\/.docker-info/$FAKE_DOCKER_INFO}"); done
    mkdir -p "$FAKE_LOG/home"
    : >"$FAKE_LOG/in-run"
    cd "$dir" || exit 125
    env -i "${cenv[@]}" "${args[@]}"
    rc=$?
    rm -f "$FAKE_LOG/in-run"
    exit "$rc" ;;
  *)
    echo "fake docker: unsupported: $*" >&2; exit 1 ;;
esac
SH
chmod +x "$fakebin"/*

# --- scratch roots -------------------------------------------------------------------------
base="$tmp/base"
mkdir -p "$base/.teamwright" "$base/node_modules/.bin" "$base/node_modules/@playwright/test" \
  "$base/node_modules/playwright"
cp -r scripts "$base/" && cp package.json pnpm-lock.yaml "$base/" \
  || { echo "$name: cannot run: cannot copy the project files" >&2; exit 3; }
find "$base/scripts" -name __pycache__ -prune -exec rm -rf {} +
cat >"$base/.teamwright/config.yml" <<YML
areas:
  - name: ui
    source_dirs: [src]
    runner: docker
    image: "$image"
    test_command: "pnpm test"
    e2e_ui_command: "pnpm e2e"
gate_command: "make check"
YML
printf '{"name":"@playwright/test","version":"%s","bin":{"playwright":"cli.js"}}\n' "$version" \
  >"$base/node_modules/@playwright/test/package.json"
printf '{"name":"playwright","version":"%s","bin":{"playwright":"cli.js"}}\n' "$version" \
  >"$base/node_modules/playwright/package.json"
printf '%s\n' "$fake_cli_js" >"$base/node_modules/@playwright/test/cli.js"
printf '%s\n' "$fake_cli_js" >"$base/node_modules/playwright/cli.js"
cp "$fakebin/playwright" "$base/node_modules/.bin/playwright"

failed=0
passed=0
n=0
case_root=""
log=""
info=""

# new_case <driverVersion or "-" for none>: a fresh scratch root, log dir and .docker-info.
new_case() {
  n=$((n + 1))
  case_root="$tmp/case-$n/root"
  log="$tmp/case-$n/log"
  info="$tmp/case-$n/docker-info.json"
  mkdir -p "$log" && cp -r "$base" "$case_root"
  if [ "$1" = "-" ]; then
    printf '{"dockerImageName":"mcr.microsoft.com/playwright:v%s-noble"}\n' "$version" >"$info"
  else
    printf '{"driverVersion":"%s","dockerImageName":"mcr.microsoft.com/playwright:v%s-noble"}\n' "$1" "$1" >"$info"
  fi
}

# entry <where: host|image> <CI value or "-" for unset> <images present> [args...]: runs
# `pnpm e2e args` in the scratch root; sets $out and $got.
out=""
got=0
entry() {
  local where="$1" ci="$2" images="$3"
  shift 3
  local envs=(PATH="$fakebin:$nodedir:/usr/local/bin:/usr/bin:/bin" HOME="$log/host-home"
              FAKE_LOG="$log" FAKE_BIN="$fakebin" FAKE_NODEDIR="$nodedir" FAKE_IMAGES="$images"
              FAKE_DOCKER_INFO="$info" TW_DOCKER="$fakebin/docker")
  if [ "$where" = image ]; then
    envs+=(VOICEN_E2E_DOCKER_INFO="$info" FAKE_IN_CONTAINER=1)
  else
    envs+=(VOICEN_E2E_DOCKER_INFO="$tmp/no-such-docker-info.json")
  fi
  [ "$ci" = "-" ] || envs+=(CI="$ci")
  mkdir -p "$log/host-home"
  out="$(cd "$case_root" && env -i "${envs[@]}" timeout 60 pnpm e2e "$@" 2>&1)"
  got=$?
}

fail() {
  echo "FAIL $1: $2" >&2
  sed 's/^/       | /' <<<"exit $got
$out" >&2
  for f in docker.log playwright.log network; do
    [ -f "$log/$f" ] && sed "s/^/       | $f: /" "$log/$f" >&2
  done
  failed=$((failed + 1))
}
ok() { echo "ok   $1"; passed=$((passed + 1)); }

no_test() { [ ! -e "$log/playwright.log" ]; }
docker_ran() { [ -f "$log/docker.log" ] && grep -qE '^(run|pull|implicit-pull)( |$)' "$log/docker.log"; }
has() { grep -qF -- "$1" <<<"$out"; }

# --- cases ---------------------------------------------------------------------------------
c="host: missing image -> exit 3 before any test, names the image, no docker run or pull"
new_case "$version"
entry host - "" --retries=0
if [ "$got" -ne 3 ]; then fail "$c" "exit $got, want 3"
elif ! no_test; then fail "$c" "Playwright was started (a test run instead of a preflight stop)"
elif docker_ran; then fail "$c" "docker run/pull was called for a missing image (it would be pulled implicitly)"
elif ! has "$image"; then fail "$c" "the message does not name the image $image"
else ok "$c"; fi

c="host: image version != lockfile @playwright/test -> exit 3 before any test, names both versions and the image"
new_case "$other"
entry host - "$image" --retries=0
if [ "$got" -ne 3 ]; then fail "$c" "exit $got, want 3"
elif ! no_test; then fail "$c" "Playwright was started"
elif ! has "$other" || ! has "$version"; then fail "$c" "the message does not name both versions ($other, $version)"
elif ! has "$image" && ! has "mcr.microsoft.com/playwright:v$other-noble"; then fail "$c" "the message does not name the image"
else ok "$c"; fi

c="host: image without a Playwright version (no driverVersion) -> exit 3 before any test"
new_case -
entry host - "$image"
if [ "$got" -ne 3 ]; then fail "$c" "exit $got, want 3"
elif ! no_test; then fail "$c" "Playwright was started"
else ok "$c"; fi

c="host: image present, version equal -> tests run in the image with --network none, args passed, CI reaches the config"
new_case "$version"
entry host true "$image" --retries=0 --grep t064-fixture-arg
if [ "$got" -ne 0 ]; then fail "$c" "exit $got, want 0"
elif [ ! -f "$log/playwright.log" ]; then fail "$c" "Playwright did not run"
elif grep -q '^container=no' "$log/playwright.log"; then fail "$c" "Playwright ran on the host, not in the image"
elif [ "$(sort -u "$log/network" 2>/dev/null)" != "none" ]; then fail "$c" "docker run network is '$(paste -sd, "$log/network" 2>/dev/null)', want none"
elif ! grep -qE '^argv: .*test .*--retries=0 --grep t064-fixture-arg' "$log/playwright.log"; then fail "$c" "Playwright did not get 'test' and the caller's arguments"
elif ! grep -qx 'CI=true' "$log/playwright.log"; then fail "$c" "CI=true did not reach Playwright in the image"
else ok "$c"; fi

c="host: CI unset -> CI stays unset in the image (passed through, not invented)"
new_case "$version"
entry host - "$image"
if [ "$got" -ne 0 ]; then fail "$c" "exit $got, want 0"
elif [ ! -f "$log/playwright.log" ]; then fail "$c" "Playwright did not run"
elif grep -q '^container=no' "$log/playwright.log"; then fail "$c" "Playwright ran on the host, not in the image"
elif ! grep -qx 'CI=__unset__' "$log/playwright.log"; then fail "$c" "CI is set in the image although the caller had none: $(grep '^CI=' "$log/playwright.log")"
else ok "$c"; fi

c="image (tw-run ui -- pnpm e2e): version equal -> runs Playwright directly with args and CI, no docker call"
new_case "$version"
entry image 1 "" --grep t064-fixture-arg
if [ "$got" -ne 0 ]; then fail "$c" "exit $got, want 0"
elif [ -f "$log/docker.log" ]; then fail "$c" "docker was called from inside the image"
elif [ ! -f "$log/playwright.log" ]; then fail "$c" "Playwright did not run"
elif ! grep -qE '^argv: .*test .*--grep t064-fixture-arg' "$log/playwright.log"; then fail "$c" "Playwright did not get 'test' and the caller's arguments"
elif ! grep -qx 'CI=1' "$log/playwright.log"; then fail "$c" "CI=1 did not reach Playwright"
else ok "$c"; fi

c="image (tw-run ui -- pnpm e2e): version != lockfile -> exit 3 before any test, names both versions"
new_case "$other"
entry image - ""
if [ "$got" -ne 3 ]; then fail "$c" "exit $got, want 3"
elif ! no_test; then fail "$c" "Playwright was started"
elif [ -f "$log/docker.log" ]; then fail "$c" "docker was called from inside the image"
elif ! has "$other" || ! has "$version"; then fail "$c" "the message does not name both versions ($other, $version)"
else ok "$c"; fi

if [ "$failed" -gt 0 ]; then
  echo "$name: FAIL: $failed case(s) differ, $passed as expected; the e2e entry point does not hold its contract (T-064)" >&2
  exit 1
fi
echo "$name: ok: $passed cases as expected"
