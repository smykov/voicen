#!/usr/bin/env bash
# The one reader of a toolchain area's image tag (T-177 I5, I7; docs/decisions/ui-e2e.md E3).
# The tag is written once, as the area's `image` in the teamwright config; the Makefile
# (ui-image, core-image, check-ui) and the contour preflight (teamwright-agent.yml) take it
# from here, never from a literal.
#
# Usage: scripts/ci/area-image.sh [--present] <area>
#   <area>             prints the area's `image` from the config (TEAMWRIGHT_CONFIG, default
#                      <root>/.teamwright/config.yml), read with the kit parser
#                      scripts/hooks/_config.py (the one scripts/tw-run uses), alone on stdout.
#   --present <area>   the same, after `$TW_DOCKER image inspect <image>` (TW_DOCKER, default
#                      docker) succeeds. Only `image inspect` is ever called: `docker run` of a
#                      missing tag would pull it implicitly.
#   A ui image `voicen-ui:<tag>`: <tag> must equal `version` of
#   <root>/node_modules/@playwright/test/package.json (as scripts/e2e.sh compares inside the
#   image); before `pnpm install` (CI builds the ui image first and installs in it) the
#   lockfile's importer entry of @playwright/test stands in. Checked in both modes, before any
#   docker call. Other image names are not version-checked.
# <root> is the directory two levels above this script.
#
# Exit 0: printed. 1: the ui tag differs from the installed @playwright/test (names both).
# 2: usage, unreadable config or unknown area (nothing on stdout). 3: --present and the image
# is missing or docker cannot answer (names the image and `make <area>-image`).
set -uo pipefail

name="area-image"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

usage() { echo "usage: $name [--present] <area>" >&2; exit 2; }

present=0
if [ "${1:-}" = --present ]; then present=1; shift; fi
[ $# -eq 1 ] && [ -n "$1" ] || usage
area="$1"

command -v python3 >/dev/null 2>&1 || { echo "$name: python3 is required to read the teamwright config" >&2; exit 2; }
cfg="${TEAMWRIGHT_CONFIG:-$root/.teamwright/config.yml}"
conf_py="$root/scripts/hooks/_config.py"
[ -f "$conf_py" ] || { echo "$name: scripts/hooks/_config.py not found" >&2; exit 2; }

image="$(python3 -I -c '
import os, sys
sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(sys.argv[1]))
import _config
try:
    cfg = _config.parse_yaml(open(sys.argv[2], encoding="utf-8").read())
except OSError:
    sys.exit(2)
a = next((a for a in (cfg.get("areas") or []) if isinstance(a, dict) and a.get("name") == sys.argv[3]), None)
img = str((a or {}).get("image") or "")
if not img:
    sys.exit(1)
sys.stdout.write(img)
' "$conf_py" "$cfg" "$area")"
case $? in
  0) ;;
  1) echo "$name: no area '$area' with an image in $cfg" >&2; exit 2 ;;
  *) echo "$name: cannot read $cfg" >&2; exit 2 ;;
esac

case "$image" in
  voicen-ui:*)
    tag="${image#voicen-ui:}"
    installed="$(python3 -I -c '
import json, re, sys
try:
    print(json.load(open(sys.argv[1], encoding="utf-8"))["version"])
    sys.exit(0)
except (OSError, ValueError, KeyError):
    pass
try:
    lock = open(sys.argv[2], encoding="utf-8").read()
except OSError:
    sys.exit(1)
m = re.search(r"^  \x27@playwright/test\x27:\n    specifier: .*\n    version: ([^\s(]+)", lock, re.M)
if not m:
    sys.exit(1)
print(m.group(1))
' "$root/node_modules/@playwright/test/package.json" "$root/pnpm-lock.yaml")" \
      || { echo "$name: the ui image $image needs @playwright/test $tag, but it is neither installed (node_modules) nor in pnpm-lock.yaml" >&2; exit 1; }
    if [ "$tag" != "$installed" ]; then
      echo "$name: the ui image $image is Playwright $tag, the installed @playwright/test is $installed; set the ui area's image in $cfg to voicen-ui:$installed and rebuild (make ui-image)" >&2
      exit 1
    fi
    ;;
esac

if [ "$present" = 1 ]; then
  docker="${TW_DOCKER:-docker}"
  if ! command -v "$docker" >/dev/null 2>&1; then
    echo "$name: cannot check the image $image: $docker not found; rebuild with make $area-image once docker is available" >&2
    exit 3
  fi
  if ! "$docker" image inspect "$image" >/dev/null 2>&1; then
    echo "$name: the $area toolchain image $image is not present; rebuild it with make $area-image (it is never pulled)" >&2
    exit 3
  fi
fi

printf '%s\n' "$image"
