#!/usr/bin/env bash
# The ui e2e entry point: package.json `scripts.e2e`, so `pnpm e2e [playwright test args...]`
# on the host, `scripts/tw-run ui -- pnpm e2e` and Makefile check-ui all come here (T-064,
# decisions #77 #89; docs/decisions/ui-e2e.md E3).
#
# Invariant: no e2e browser shares the host's network namespace. Playwright, its webServer and
# Chromium run only inside the ui image (the official Playwright image at the lockfile's
# @playwright/test version, docker/ui.Dockerfile), in a network namespace private to the run
# (--network none: loopback only). A missing or mismatched image stops the run before any test
# and names the image. Playwright never runs on the host instead.
#
# Inside the image (the file $VOICEN_E2E_DOCKER_INFO exists, default
# /ms-playwright/.docker-info, written by the official image's `mark-docker-image`):
#   its driverVersion must equal node_modules/@playwright/test's version, else exit 3;
#   equal -> `playwright test <args>` directly, environment unchanged.
# On the host:
#   image = the ui area's `image` in the teamwright config (read like scripts/tw-run);
#   `$TW_DOCKER image inspect` fails -> exit 3 (never pulled: `docker run` would pull it);
#   present -> re-enter through scripts/tw-run ui with TW_DOCKER_ARGS `--network none`, and
#   `-e CI` only when the caller has CI (tw-run passes no environment but HOME; CI drives
#   forbidOnly and retries in playwright.config.ts).
# BASE_URL does not reach the container: inside it, localhost is the container's own.
# Exit 3: the run cannot start in the image (nothing was tested). Otherwise Playwright's code.
set -uo pipefail

name="e2e"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
info="${VOICEN_E2E_DOCKER_INFO:-/ms-playwright/.docker-info}"

stop() { echo "$name: cannot run: $*" >&2; exit 3; }

if [ -e "$info" ]; then
  # --- inside the image: version preflight, then Playwright directly ---------------------
  command -v node >/dev/null 2>&1 || stop "node not found in the image"
  versions="$(node -e '
    const fs = require("node:fs");
    const read = (p) => { try { return JSON.parse(fs.readFileSync(p, "utf8")); } catch { return {}; } };
    const i = read(process.argv[1]);
    const l = read(process.argv[2]);
    process.stdout.write([i.driverVersion || "", i.dockerImageName || "", l.version || ""].join("\t"));
  ' "$info" "$root/node_modules/@playwright/test/package.json")" \
    || stop "cannot read $info"
  IFS=$'\t' read -r driver base lock <<<"$versions"
  image="${VOICEN_E2E_IMAGE:-}"
  shown="${image:+$image (}${base:-unknown base}${image:+)}"
  [ -n "$lock" ] || stop "node_modules/@playwright/test is not installed (pnpm install); image $shown"
  [ -n "$driver" ] || stop "image $shown has no Playwright version in $info (driverVersion); it must be the Playwright image at the lockfile's @playwright/test $lock"
  [ "$driver" = "$lock" ] \
    || stop "image $shown has Playwright $driver, the lockfile's @playwright/test is $lock; rebuild the ui image at $lock (docker/ui.Dockerfile, make ui-image) and set it as the ui area's image"
  bin="$root/node_modules/.bin/playwright"
  [ -x "$bin" ] || bin="playwright"
  exec "$bin" test "$@"
fi

# --- on the host: the image must be present; re-enter through tw-run ui ------------------
[ -z "${VOICEN_E2E_REENTERED:-}" ] \
  || stop "re-entered through scripts/tw-run ui but $info is missing: the ui area's image is not the Playwright image"
command -v python3 >/dev/null 2>&1 || stop "python3 is required to read the teamwright config"
cfg="${TEAMWRIGHT_CONFIG:-$root/.teamwright/config.yml}"
conf_py="$root/scripts/hooks/_config.py"
[ -f "$conf_py" ] || stop "scripts/hooks/_config.py not found"
area="$(python3 -c '
import os, sys
sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(sys.argv[1]))
import _config
cfg = _config.parse_yaml(open(sys.argv[2], encoding="utf-8").read())
a = next((a for a in cfg.get("areas") or [] if isinstance(a, dict) and a.get("name") == "ui"), None)
if a is None:
    sys.exit(1)
sys.stdout.write("%s\t%s" % (a.get("runner") or "host", a.get("image") or ""))
' "$conf_py" "$cfg")" || stop "no ui area readable in $cfg"
IFS=$'\t' read -r runner image <<<"$area"
[ -n "$image" ] || stop "the ui area in $cfg names no image (the Playwright image, docker/ui.Dockerfile)"
[ "$runner" = docker ] \
  || stop "the ui area in $cfg has runner '$runner'; e2e runs only in the image $image (runner: docker)"
docker="${TW_DOCKER:-docker}"
command -v "$docker" >/dev/null 2>&1 || stop "$docker not found; e2e runs only in the image $image"
"$docker" image inspect "$image" >/dev/null 2>&1 \
  || stop "image $image is not present (make ui-image); it is never pulled implicitly"

dargs="--network none -e VOICEN_E2E_REENTERED=1 -e VOICEN_E2E_IMAGE=$image"
[ -n "${CI+x}" ] && dargs="$dargs -e CI"
export TW_DOCKER_ARGS="$dargs"
exec "$root/scripts/tw-run" ui -- pnpm e2e "$@"
