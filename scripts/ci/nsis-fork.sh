#!/usr/bin/env bash
# T-025 (decisions #76): the NSIS installer template is a fork of the template embedded in the
# pinned tauri-cli. The fork can follow a tauri-cli bump only by hand, so the gate refuses a tree
# where the fork, the CLI version package.json asks for and the CLI version pnpm installs can
# drift apart: a `pnpm install` that moves @tauri-apps/cli to another 2.x would otherwise build
# installers from a template written for a different bundler (docs/decisions/installer.md).
#
# Contract:
#   - <root>/src-tauri/windows/installer.nsi exists and, within its first 20 lines, has exactly
#     one header line (CR line ends tolerated)
#       ; upstream: tauri-cli X.Y.Z crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi
#     X.Y.Z decimal. A missing file, a missing, late or repeated header is a violation.
#   - <root>/src-tauri/tauri.conf.json bundle.windows.nsis.template is "windows/installer.nsi"
#     (resolved against src-tauri, so the fork is the template the bundler uses).
#   - <root>/package.json devDependencies "@tauri-apps/cli" is an exact X.Y.Z (no range, no
#     ^, ~, x, tag or pre-release).
#   - <root>/pnpm-lock.yaml, root importer (lockfile v6: the top-level devDependencies block),
#     '@tauri-apps/cli' has `specifier:` equal to package.json's value and `version:` (a peer
#     suffix "(...)" ignored) X.Y.Z.
#   - The header version, the package.json version and the lockfile version are equal.
#   Every violation is listed (one line each, naming the file and the values), not only the first.
#
# Usage: scripts/ci/nsis-fork.sh [root]   (root: the repository)
# Exit 0: ok. Exit 1: violation(s) listed. Exit 3: cannot run (package.json, pnpm-lock.yaml or
# tauri.conf.json missing or unreadable, python3 or awk missing); never reported as a pass.
# Host bash, awk, python3 (stdlib json only).
set -uo pipefail
root="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
cannot_run() { echo "nsis-fork: cannot run: $1" >&2; exit 3; }
[ -d "$root" ] || cannot_run "root $root is not a directory"
fork="src-tauri/windows/installer.nsi"
conf="src-tauri/tauri.conf.json"
pkg="package.json"
lock="pnpm-lock.yaml"
for f in "$pkg" "$lock" "$conf"; do
  [ -f "$root/$f" ] && [ -r "$root/$f" ] || cannot_run "$root/$f not found or not readable"
done
command -v python3 > /dev/null || cannot_run "python3 not found"
command -v awk > /dev/null || cannot_run "awk not found"

found=""
add() { found="$found  $1"$'\n'; }
exact() { [[ "$1" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; }

# ---- the fork's header ------------------------------------------------------------------------
header=""
if [ ! -f "$root/$fork" ]; then
  add "$fork: missing (the fork of the tauri-cli NSIS template, decisions #76)"
else
  hits="$(head -n 20 "$root/$fork" | tr -d '\r' |
    grep -cE '^; upstream: tauri-cli [0-9]+\.[0-9]+\.[0-9]+ crates/tauri-bundler/src/bundle/windows/nsis/installer\.nsi$')"
  if [ "$hits" = 1 ]; then
    header="$(head -n 20 "$root/$fork" | tr -d '\r' |
      sed -nE 's#^; upstream: tauri-cli ([0-9]+\.[0-9]+\.[0-9]+) crates/tauri-bundler/src/bundle/windows/nsis/installer\.nsi$#\1#p')"
  else
    add "$fork: $hits upstream header line(s) in the first 20 lines, want exactly 1: '; upstream: tauri-cli X.Y.Z crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi'"
  fi
fi

# ---- tauri.conf.json template and package.json specifier (stdlib json) -------------------------
json_get() {
  python3 -I -c '
import json, sys
try:
    with open(sys.argv[1], encoding="utf-8") as f:
        node = json.load(f)
except Exception as e:
    print("ERROR %s" % e); sys.exit(0)
for key in sys.argv[2:]:
    if not isinstance(node, dict) or key not in node:
        print("ABSENT"); sys.exit(0)
    node = node[key]
print("VALUE %s" % (node if isinstance(node, str) else json.dumps(node)))
' "$@"
}
tpl="$(json_get "$root/$conf" bundle windows nsis template)" || cannot_run "python3 failed on $conf"
case "$tpl" in
  "VALUE windows/installer.nsi") ;;
  ERROR*) cannot_run "$conf is not valid JSON (${tpl#ERROR })" ;;
  ABSENT) add "$conf: bundle.windows.nsis.template is not set, want \"windows/installer.nsi\" (the bundler would use its own template, not the fork)" ;;
  *) add "$conf: bundle.windows.nsis.template is \"${tpl#VALUE }\", want \"windows/installer.nsi\"" ;;
esac
spec="$(json_get "$root/$pkg" devDependencies @tauri-apps/cli)" || cannot_run "python3 failed on $pkg"
case "$spec" in
  ERROR*) cannot_run "$pkg is not valid JSON (${spec#ERROR })" ;;
  ABSENT) spec=""; add "$pkg: devDependencies has no \"@tauri-apps/cli\"" ;;
  *)
    spec="${spec#VALUE }"
    exact "$spec" || add "$pkg: devDependencies \"@tauri-apps/cli\" is \"$spec\", not an exact X.Y.Z version (the fork follows one tauri-cli version)"
    ;;
esac

# ---- pnpm-lock.yaml root importer ----------------------------------------------------------------
# Lockfile v6: top-level "devDependencies:" block, entry "  '@tauri-apps/cli':" with
# "    specifier: ..." and "    version: ...". Prints "<specifier>\t<version>" or nothing.
lockrow="$(tr -d '\r' < "$root/$lock" | awk '
  /^[^ ]/ { top = $0; entry = 0; next }
  top == "devDependencies:" && /^  [^ ]/ { entry = ($0 == "  '"'"'@tauri-apps/cli'"'"':"); next }
  entry && /^    specifier: / { s = $0; sub(/^    specifier: */, "", s); n++ }
  entry && /^    version: / { v = $0; sub(/^    version: */, "", v) }
  END { if (n == 1) printf "%s\t%s\n", s, v }
')" || cannot_run "awk failed on $lock"
lock_spec=""; lock_ver=""
if [ -z "$lockrow" ]; then
  add "$lock: no single '@tauri-apps/cli' entry in the root devDependencies"
else
  lock_spec="${lockrow%%$'\t'*}"; lock_ver="${lockrow#*$'\t'}"
  lock_spec="${lock_spec#\'}"; lock_spec="${lock_spec%\'}"
  lock_ver="${lock_ver%%(*}"; lock_ver="${lock_ver#\'}"; lock_ver="${lock_ver%\'}"
  [ -z "$spec" ] || [ "$lock_spec" = "$spec" ] ||
    add "$lock: '@tauri-apps/cli' specifier is '$lock_spec', $pkg says '$spec' (run pnpm install and commit the lockfile)"
  exact "$lock_ver" || add "$lock: '@tauri-apps/cli' version '$lock_ver' is not X.Y.Z"
fi

# ---- the three versions agree ---------------------------------------------------------------------
if [ -n "$header" ] && exact "$spec" && exact "$lock_ver"; then
  if [ "$header" != "$spec" ] || [ "$header" != "$lock_ver" ]; then
    add "versions differ: $fork header tauri-cli $header, $pkg @tauri-apps/cli $spec, $lock @tauri-apps/cli $lock_ver (re-fork the template from the new tauri-cli or pin the CLI back)"
  fi
elif [ -n "$header" ] && [ -n "$lock_ver" ] && [ "$header" != "$lock_ver" ]; then
  add "versions differ: $fork header tauri-cli $header, $lock @tauri-apps/cli $lock_ver"
fi

if [ -n "$found" ]; then
  {
    echo "nsis-fork: FAIL: the NSIS template fork must match the exactly pinned tauri-cli (decisions #76):"
    printf '%s' "$found"
  } >&2
  exit 1
fi
echo "nsis-fork: ok: $fork is the fork of tauri-cli $header, pinned exactly in $pkg and $lock"
