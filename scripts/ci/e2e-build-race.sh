#!/usr/bin/env bash
# T-050 reproduction (M1, decision #77): the gate's e2e preview must be private to its run.
# It runs e2e/build-race.spec.ts through the gate's own Playwright config
# (playwright.config.ts webServer, BASE_URL unset, so the config starts its preview the
# way `make check-ui` / `pnpm e2e` do), one scenario per Playwright run:
#
#   build         while the preview serves, `pnpm build` runs in this tree; the preview
#                 still answers 200 for / and for the entry chunk it served at the start.
#   foreign-port  a foreign HTTP server holds port 4173 (the old fixed port); the run still
#                 serves its own build instead of reusing that server.
#
# Before T-050: build -> 500 ("Internal Error", cached failed import of a server node) or
# connection refused (preview died on ENOENT); foreign-port -> the foreign page is reused.
# No retries (--retries=0).
#
# Usage: scripts/ci/e2e-build-race.sh [build|foreign-port ...]   (default: both)
# Run it through the ui runner: scripts/tw-run ui -- scripts/ci/e2e-build-race.sh
# Exit 0: every scenario passed. Exit 1: a scenario failed. Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
unset BASE_URL CI

scenarios=("$@")
[ ${#scenarios[@]} -gt 0 ] || scenarios=(build foreign-port)

foreign_pid=""
cleanup() { [ -n "$foreign_pid" ] && kill "$foreign_pid" 2>/dev/null; }
trap cleanup EXIT

start_foreign() {
  # Port 4173 must be free so that the foreign server below is the one holding it.
  if node -e 'const s=require("net").createServer().once("error",()=>process.exit(1)).listen(4173,()=>s.close(()=>process.exit(0)))'; then :
  else
    echo "e2e-build-race: cannot run foreign-port: port 4173 is already in use" >&2
    return 3
  fi
  node -e '
    require("http").createServer((q, r) => {
      r.writeHead(200, { "content-type": "text/html" });
      r.end("<html><body>t050-foreign-server</body></html>");
    }).listen(4173);
  ' &
  foreign_pid=$!
  for _ in $(seq 1 50); do
    curl -fsS -o /dev/null http://localhost:4173/ 2>/dev/null && return 0
    sleep 0.1
  done
  echo "e2e-build-race: cannot run foreign-port: the foreign server did not start" >&2
  return 3
}

failed=0
for s in "${scenarios[@]}"; do
  case "$s" in
    build) ;;
    foreign-port) start_foreign || exit $? ;;
    *) echo "e2e-build-race: unknown scenario '$s'" >&2; exit 3 ;;
  esac
  echo "=== e2e-build-race: $s"
  if VOICEN_E2E_BUILD_RACE="$s" pnpm exec playwright test e2e/build-race.spec.ts \
      --retries=0 --workers=1 --reporter=list; then
    echo "=== e2e-build-race: $s PASS"
  else
    echo "=== e2e-build-race: $s FAIL"
    failed=1
  fi
  cleanup; foreign_pid=""
done
exit "$failed"
