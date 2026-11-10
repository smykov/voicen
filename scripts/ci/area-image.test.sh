#!/usr/bin/env bash
# T-177 fixture test of scripts/ci/area-image.sh (invariants I5, I7; decides_on: switch = the
# docker daemon's own `docker image inspect` status, the kit parser scripts/hooks/_config.py for
# the tag, literal = the tag after `voicen-ui:` equals the installed @playwright/test version).
#
# Contract of the reader (written before it exists):
#   scripts/ci/area-image.sh <area>             prints the area's `image` from the teamwright
#       config (TEAMWRIGHT_CONFIG, default <root>/.teamwright/config.yml) through
#       scripts/hooks/_config.py, alone on stdout, exit 0.
#   scripts/ci/area-image.sh --present <area>   the same, then `$TW_DOCKER image inspect <image>`
#       (TW_DOCKER, default docker): present -> prints the image, exit 0; absent (or no docker)
#       -> exit 3, stderr names the image and `make <area>-image`; it never runs `docker run` or
#       `docker pull` (a run would pull implicitly).
#   ui area with an image `voicen-ui:<tag>`: <tag> must equal `version` of
#       <root>/node_modules/@playwright/test/package.json, else exit 1 naming both versions
#       (in both modes, before any docker call). Not installed (before `pnpm install`): the
#       importer entry of @playwright/test in <root>/pnpm-lock.yaml stands in, same exits.
#       Other image names are not version-checked.
#   Unknown area or unreadable config: exit 2, nothing on stdout.
# <root> is the directory two levels above the script, so a copy of the script in a scratch root
# reads that root's config and node_modules.
#
# Also pinned here (single source of the tag, I5, I4): the Makefile, docker/ui.Dockerfile and
# the contour workflow hold no literal toolchain tag; `make -n ui-image` builds the config's tag
# with the installed Playwright version as build-arg; `make -n check-ui` runs the presence check
# before its first tw-run and passes --workers=2 (WORKERS=1 overrides).
#
# Usage: scripts/ci/area-image.test.sh   (host bash, python3, make)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
unset TEAMWRIGHT_ROOT TEAMWRIGHT_CONFIG TW_DOCKER TW_DOCKER_ARGS TW_RUN_PRINT

name="area-image.test"
for t in python3 make; do
  command -v "$t" >/dev/null 2>&1 || { echo "$name: cannot run: $t not found" >&2; exit 3; }
done
tmp="$(mktemp -d)" || { echo "$name: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

failed=0
passed=0
out=""
err=""
got=0
fail() {
  echo "FAIL $1: $2" >&2
  sed 's/^/       | /' <<<"exit $got
stdout: $out
stderr: $err" >&2
  [ -f "$tmp/docker.log" ] && sed 's/^/       | docker: /' "$tmp/docker.log" >&2
  failed=$((failed + 1))
}
ok() { echo "ok   $1"; passed=$((passed + 1)); }

if [ ! -f scripts/ci/area-image.sh ]; then
  echo "FAIL scripts/ci/area-image.sh does not exist (T-177 I5, I7: the one reader of an area's image tag)" >&2
  echo "$name: FAIL: the area image reader is missing" >&2
  exit 1
fi

# --- fakes and a scratch root ---------------------------------------------------------------
ver="9.8.7"
fakebin="$tmp/fakebin"
mkdir -p "$fakebin"
cat >"$fakebin/docker" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$FAKE_LOG/docker.log"
if [ "${1:-}" = image ] && [ "${2:-}" = inspect ]; then
  img="${!#}"
  case " $FAKE_IMAGES " in *" $img "*) echo '[{"Id":"sha256:fixture"}]'; exit 0 ;; esac
  echo "Error: No such image: $img" >&2; exit 1
fi
echo "fake docker: unsupported: $*" >&2
exit 99
SH
chmod +x "$fakebin/docker"

# scratch <ui image> <core image>: a root with the reader, the kit parser, a config and an
# installed @playwright/test at $ver; sets $sroot.
sroot=""
scratch() {
  sroot="$(mktemp -d "$tmp/root.XXXXXX")"
  mkdir -p "$sroot/scripts/ci" "$sroot/scripts/hooks" "$sroot/.teamwright" \
    "$sroot/node_modules/@playwright/test"
  cp scripts/ci/area-image.sh "$sroot/scripts/ci/" && cp scripts/hooks/_config.py "$sroot/scripts/hooks/"
  cat >"$sroot/.teamwright/config.yml" <<YML
areas:
  - name: core
    runner: docker
    image: "$2"
  - name: ui
    runner: docker
    image: "$1"
gate_command: "make check"
YML
  printf '{"name":"@playwright/test","version":"%s"}\n' "$ver" >"$sroot/node_modules/@playwright/test/package.json"
  rm -f "$tmp/docker.log"
}

# reader <args...> [FAKE_IMAGES via $images]: runs the scratch root's reader; sets $out $err $got.
images=""
reader() {
  out="$(cd "$sroot" && env -i PATH="$fakebin:/usr/local/bin:/usr/bin:/bin" HOME="$tmp/home" \
    FAKE_LOG="$tmp" FAKE_IMAGES="$images" TW_DOCKER="${READER_DOCKER:-$fakebin/docker}" \
    bash scripts/ci/area-image.sh "$@" 2>"$tmp/err")"
  got=$?
  err="$(cat "$tmp/err")"
}
docker_ran() { [ -f "$tmp/docker.log" ] && grep -qE '^(run|pull|implicit-pull)( |$)' "$tmp/docker.log"; }

# --- printing ---------------------------------------------------------------------------------
c="prints the ui area's image from the config, alone on stdout"
scratch "voicen-ui:$ver" "voicen-rust-fixture:4.5"
reader ui
if [ "$got" -ne 0 ]; then fail "$c" "exit $got, want 0"
elif [ "$out" != "voicen-ui:$ver" ]; then fail "$c" "stdout is not the config's image voicen-ui:$ver"
else ok "$c"; fi

c="prints the core area's image from the config (not a literal in the script)"
scratch "voicen-ui:$ver" "voicen-rust-fixture:4.5"
reader core
if [ "$got" -ne 0 ]; then fail "$c" "exit $got, want 0"
elif [ "$out" != "voicen-rust-fixture:4.5" ]; then fail "$c" "stdout is not the config's core image"
else ok "$c"; fi

c="a ui image that is not voicen-ui:<tag> is printed without a version check"
scratch "voicen-ui-fixture:0.0.0-t177" "voicen-rust:1.99"
reader ui
if [ "$got" -ne 0 ]; then fail "$c" "exit $got, want 0"
elif [ "$out" != "voicen-ui-fixture:0.0.0-t177" ]; then fail "$c" "stdout is not the config's image"
else ok "$c"; fi

c="ui tag version != installed @playwright/test -> exit 1 naming both versions, nothing on stdout"
scratch "voicen-ui:1.0.0" "voicen-rust:1.99"
reader ui
if [ "$got" -ne 1 ]; then fail "$c" "exit $got, want 1"
elif [ -n "$out" ]; then fail "$c" "an image was printed for a mismatching tag"
elif [[ "$err" != *1.0.0* || "$err" != *$ver* ]]; then fail "$c" "the message does not name both versions (1.0.0, $ver)"
else ok "$c"; fi

c="--present, ui tag version != installed @playwright/test -> exit 1 before any docker call"
scratch "voicen-ui:1.0.0" "voicen-rust:1.99"
images="voicen-ui:1.0.0"
reader --present ui
if [ "$got" -ne 1 ]; then fail "$c" "exit $got, want 1"
elif [ -f "$tmp/docker.log" ]; then fail "$c" "docker was called before the version check"
else ok "$c"; fi

# --- before `pnpm install`: the lockfile stands in for node_modules ------------------------------
# ci.yml runs `make ui-image` before `pnpm install`, the contour preflight before its install:
# with no node_modules/@playwright/test the tag is compared with the lockfile's importer entry.
# lockfile_only <lock version>: the scratch root loses node_modules and gets a pnpm-lock.yaml
# excerpt in the real shape (lockfileVersion 6.0, devDependencies importer entry, a neighbour with
# a peer suffix, the packages entry).
lockfile_only() {
  rm -rf "$sroot/node_modules"
  cat >"$sroot/pnpm-lock.yaml" <<YML
lockfileVersion: '6.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

dependencies:
  '@tauri-apps/api':
    specifier: ^2
    version: 2.12.1

devDependencies:
  '@eslint/js':
    specifier: ^10.0.1
    version: 10.0.1(eslint@10.11.0)
  '@playwright/test':
    specifier: ^$1
    version: $1
  '@sveltejs/adapter-static':
    specifier: ^3.0.10
    version: 3.0.10(@sveltejs/kit@2.70.3)

packages:

  /@playwright/test@$1:
    resolution: {integrity: sha512-fixture}
    engines: {node: '>=20'}
    hasBin: true
    dependencies:
      playwright: $1
    dev: true
YML
}

c="no node_modules, lockfile @playwright/test == ui tag -> prints the tag, exit 0"
scratch "voicen-ui:$ver" "voicen-rust:1.99"
lockfile_only "$ver"
reader ui
if [ "$got" -ne 0 ]; then fail "$c" "exit $got, want 0"
elif [ "$out" != "voicen-ui:$ver" ]; then fail "$c" "stdout is not the config's image voicen-ui:$ver"
else ok "$c"; fi

c="no node_modules, lockfile @playwright/test != ui tag -> exit 1 naming both versions, nothing on stdout"
scratch "voicen-ui:$ver" "voicen-rust:1.99"
lockfile_only "1.2.3"
reader ui
if [ "$got" -ne 1 ]; then fail "$c" "exit $got, want 1"
elif [ -n "$out" ]; then fail "$c" "an image was printed for a mismatching tag"
elif [[ "$err" != *1.2.3* || "$err" != *$ver* ]]; then fail "$c" "the message does not name both versions (1.2.3, $ver)"
else ok "$c"; fi

c="unknown area -> exit 2, nothing on stdout"
scratch "voicen-ui:$ver" "voicen-rust:1.99"
reader nosuch
if [ "$got" -ne 2 ]; then fail "$c" "exit $got, want 2"
elif [ -n "$out" ]; then fail "$c" "something was printed"
else ok "$c"; fi

# --- --present --------------------------------------------------------------------------------
c="--present, image present -> exit 0, prints the image, only 'image inspect' reaches docker"
scratch "voicen-ui:$ver" "voicen-rust:1.99"
images="voicen-ui:$ver"
reader --present ui
if [ "$got" -ne 0 ]; then fail "$c" "exit $got, want 0"
elif [ "$out" != "voicen-ui:$ver" ]; then fail "$c" "stdout is not the image"
elif [ "$(cat "$tmp/docker.log" 2>/dev/null)" != "image inspect voicen-ui:$ver" ]; then fail "$c" "docker got something other than one 'image inspect <image>'"
else ok "$c"; fi

c="--present, ui image missing -> exit 3, names the image and 'make ui-image', no docker run or pull"
scratch "voicen-ui:$ver" "voicen-rust:1.99"
images=""
reader --present ui
if [ "$got" -ne 3 ]; then fail "$c" "exit $got, want 3"
elif docker_ran; then fail "$c" "docker run/pull was called for a missing image (it would be pulled implicitly)"
elif [[ "$err" != *"voicen-ui:$ver"* ]]; then fail "$c" "the message does not name the image"
elif [[ "$err" != *"make ui-image"* ]]; then fail "$c" "the message does not say to rebuild with make ui-image"
else ok "$c"; fi

c="--present, core image missing -> exit 3, names the image and 'make core-image'"
scratch "voicen-ui:$ver" "voicen-rust-fixture:4.5"
images="voicen-ui:$ver"
reader --present core
if [ "$got" -ne 3 ]; then fail "$c" "exit $got, want 3"
elif docker_ran; then fail "$c" "docker run/pull was called"
elif [[ "$err" != *"voicen-rust-fixture:4.5"* || "$err" != *"make core-image"* ]]; then fail "$c" "the message does not name the image and make core-image"
else ok "$c"; fi

c="--present, no docker at all -> exit 3 (cannot run), never a pass"
scratch "voicen-ui:$ver" "voicen-rust:1.99"
READER_DOCKER="$tmp/no-such-docker" reader --present ui
if [ "$got" -ne 3 ]; then fail "$c" "exit $got, want 3"
else ok "$c"; fi

# --- the real repository: one source of the tag ----------------------------------------------
cfg_image() {
  python3 -I -c '
import sys
sys.dont_write_bytecode = True
sys.path.insert(0, "scripts/hooks")
import _config
cfg = _config.parse_yaml(open(".teamwright/config.yml", encoding="utf-8").read())
print(next(a for a in cfg["areas"] if a["name"] == sys.argv[1])["image"])
' "$1"
}
pwv="$(python3 -c 'import json; print(json.load(open("node_modules/@playwright/test/package.json"))["version"])' 2>/dev/null)" \
  || { echo "$name: cannot run: node_modules/@playwright/test/package.json not readable (pnpm install)" >&2; exit 3; }

for area in ui core; do
  c="real config: the reader prints the $area image the config names"
  out="$(bash scripts/ci/area-image.sh "$area" 2>"$tmp/err")"; got=$?; err="$(cat "$tmp/err")"
  if [ "$got" -ne 0 ]; then fail "$c" "exit $got, want 0"
  elif [ "$out" != "$(cfg_image "$area")" ]; then fail "$c" "stdout differs from .teamwright/config.yml"
  else ok "$c"; fi
done

c="real config: the ui image tag is the installed @playwright/test version ($pwv)"
if [ "$(cfg_image ui)" != "voicen-ui:$pwv" ]; then got=0; out="$(cfg_image ui)"; err=""; fail "$c" "the ui image is not voicen-ui:$pwv"
else ok "$c"; fi

mk() { make -n "$@" 2>&1; }

c="Makefile ui-image builds the config's tag with the installed Playwright version as build-arg"
out="$(mk ui-image)"; got=$?; err=""
if [ "$got" -ne 0 ]; then fail "$c" "make -n ui-image exit $got"
elif [[ "$out" != *"-t $(cfg_image ui)"* ]]; then fail "$c" "the build does not tag $(cfg_image ui)"
elif [[ "$out" != *"PLAYWRIGHT_VERSION=$pwv"* ]]; then fail "$c" "no --build-arg PLAYWRIGHT_VERSION=$pwv"
elif [[ "$out" != *"VOICEN_UI_IMAGE=$(cfg_image ui)"* ]]; then fail "$c" "no --build-arg VOICEN_UI_IMAGE=$(cfg_image ui)"
else ok "$c"; fi

c="Makefile core-image builds the config's core tag"
out="$(mk core-image)"; got=$?; err=""
if [ "$got" -ne 0 ]; then fail "$c" "make -n core-image exit $got"
elif [[ "$out" != *"-t $(cfg_image core)"* ]]; then fail "$c" "the build does not tag $(cfg_image core)"
else ok "$c"; fi

c="no literal toolchain tag in the Makefile, docker/ui.Dockerfile or the contour workflow"
out=""; got=0; err=""
lit="$(grep -nE 'voicen-(ui|rust):[0-9]' Makefile docker/ui.Dockerfile .github/workflows/teamwright-agent.yml)"
if [ -n "$lit" ]; then out="$lit"; fail "$c" "the tag is written outside .teamwright/config.yml"
else ok "$c"; fi

c="the contour preflight reads the tags through scripts/ci/area-image.sh"
out=""; got=0; err=""
if ! grep -q 'scripts/ci/area-image.sh' .github/workflows/teamwright-agent.yml; then fail "$c" "the workflow does not call the reader"
else ok "$c"; fi

c="docker/ui.Dockerfile: base pinned by digest, version and digest as ARGs before FROM"
out=""; got=0; err=""
df=docker/ui.Dockerfile
from_line="$(grep -n '^FROM ' "$df" | head -1)"
from_n="${from_line%%:*}"
if [ "${from_line#*:}" != 'FROM mcr.microsoft.com/playwright:v${PLAYWRIGHT_VERSION}-noble@sha256:${PLAYWRIGHT_DIGEST}' ]; then fail "$c" "FROM is not the owned literal: ${from_line#*:}"
elif ! grep -qE '^ARG PLAYWRIGHT_VERSION' "$df" || [ "$(grep -nE '^ARG PLAYWRIGHT_VERSION' "$df" | head -1 | cut -d: -f1)" -gt "$from_n" ]; then fail "$c" "no ARG PLAYWRIGHT_VERSION before FROM"
elif ! grep -qE '^ARG PLAYWRIGHT_DIGEST=[0-9a-f]{64}$' "$df" || [ "$(grep -nE '^ARG PLAYWRIGHT_DIGEST' "$df" | head -1 | cut -d: -f1)" -gt "$from_n" ]; then fail "$c" "no ARG PLAYWRIGHT_DIGEST=<64 hex, no sha256: prefix: FROM adds @sha256:> before FROM"
elif ! grep -qE '^ARG VOICEN_UI_IMAGE' "$df" || ! grep -qE '^ENV VOICEN_UI_IMAGE=\$\{?VOICEN_UI_IMAGE\}?$' "$df"; then fail "$c" "ENV VOICEN_UI_IMAGE is not taken from ARG VOICEN_UI_IMAGE"
else ok "$c"; fi

# --- check-ui: presence check first, explicit workers (I4, I5) ---------------------------------
c="make check-ui checks the ui image is present before its first container, and says make ui-image through the reader"
out="$(mk check-ui)"; got=$?; err=""
first_run="$(grep -n 'tw-run' <<<"$out" | head -1 | cut -d: -f1)"
first_chk="$(grep -n 'area-image.sh.*--present\|--present.*area-image.sh' <<<"$out" | head -1 | cut -d: -f1)"
if [ "$got" -ne 0 ]; then fail "$c" "make -n check-ui exit $got"
elif [ -z "$first_chk" ]; then fail "$c" "no scripts/ci/area-image.sh --present line in the recipe"
elif [ -n "$first_run" ] && [ "$first_chk" -gt "$first_run" ]; then fail "$c" "the first tw-run (line $first_run) comes before the presence check (line $first_chk)"
else ok "$c"; fi

c="make check-ui passes --workers=2 to the e2e run by default"
out="$(mk check-ui)"; got=$?; err=""
if ! grep -qE 'scripts/e2e\.sh .*--workers=2( |$)' <<<"$out"; then fail "$c" "the e2e line has no --workers=2"
else ok "$c"; fi

c="make check-ui WORKERS=1 overrides the worker count"
out="$(mk check-ui WORKERS=1)"; got=$?; err=""
if ! grep -qE 'scripts/e2e\.sh .*--workers=1( |$)' <<<"$out"; then fail "$c" "the e2e line has no --workers=1"
elif grep -q -- '--workers=2' <<<"$out"; then fail "$c" "--workers=2 is still passed"
else ok "$c"; fi

if [ "$failed" -gt 0 ]; then
  echo "$name: FAIL: $failed case(s) differ, $passed as expected; the area image reader does not hold its contract (T-177)" >&2
  exit 1
fi
echo "$name: ok: $passed cases as expected"
