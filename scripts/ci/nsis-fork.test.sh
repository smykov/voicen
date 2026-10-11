#!/usr/bin/env bash
# T-025 (decisions #76): host test of scripts/ci/nsis-fork.sh, the gate check that the NSIS
# template fork (src-tauri/windows/installer.nsi), the exact @tauri-apps/cli pin in package.json
# and the @tauri-apps/cli version pnpm installed agree, and that tauri.conf.json uses the fork.
# Contract: the header of nsis-fork.sh. Cases are fixture trees built in a temp dir:
#   ok-*  exit 0 and "ok:";  v-*  exit 1, every listed needle in the output;  c-*  exit 3 and
#   "cannot run". Then the real repository must pass (exit 0): that case is red until T-025's fork
#   and pin land.
#
# Contract change (T-093, P-020, as T-092/T-038 did for their guards): the installed version is
# the "version" of <root>/node_modules/@tauri-apps/cli/package.json (the manifest pnpm wrote from
# the lockfile it read; its "name" must be "@tauri-apps/cli", its version an exact X.Y.Z), read
# with python3 stdlib json. pnpm-lock.yaml is no longer read at all: the awk reading of it decided
# on a model of YAML (v-lock-quote-split, v-lock-quote-split-2 passed the old guard with a lockfile
# pnpm reads as 2.13.0), and the lockfile specifier equality is pnpm's own --frozen-lockfile
# refusal. So the old cases that exercised only the awk reader went: v-lock-stale-specifier,
# v-lock-no-entry, v-lock-wrong-block (lockfile shape, not read any more), and c-no-lock became
# ok-no-lock (a missing lockfile is not the guard's concern). A missing installed manifest is
# c-not-installed (exit 3: run pnpm install --frozen-lockfile), never a pass. Every tree carries a
# decoy node_modules/@tauri-apps/cli-linux-x64-gnu/package.json at 2.99.0.
# Usage: scripts/ci/nsis-fork.test.sh   (host bash, python3)
# Exit 0: every case as expected. Exit 1: a case differs (listed), or the script is missing.
# Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
script="$root/scripts/ci/nsis-fork.sh"
if [ ! -x "$script" ]; then
  echo "nsis-fork.test: FAIL: $script not found or not executable" >&2
  exit 1
fi
tmp="$(mktemp -d)" || { echo "nsis-fork.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

up='crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi'

# mk <case> <header line or -> <package.json spec> <lock specifier> <lock version> [template or -]
#    [installed version or -]
# A tree shaped like the repository, with decoys: @tauri-apps/api (^2) before the CLI in both
# package.json and the lockfile, the platform packages of the CLI in the packages section and in
# node_modules at another version (2.99.0), and a "; upstream:" decoy comment far below the
# header. The installed manifest node_modules/@tauri-apps/cli/package.json gets the installed
# version (default: the lock version without a peer suffix or quotes, as pnpm would install it);
# "-" leaves it out.
mk() {
  local i d="$tmp/$1" hdr="$2" spec="$3" lspec="$4" lver="$5" tpl="${6:-windows/installer.nsi}"
  local inst
  if [ $# -ge 7 ]; then inst="$7"; else inst="${lver%%(*}"; inst="${inst#\'}"; inst="${inst%\'}"; fi
  mkdir -p "$d/src-tauri/windows"
  if [ "$hdr" != - ]; then
    {
      [ -z "$hdr" ] || printf '%s\n' "$hdr"
      printf '; Voicen fork. Edits: PageLeaveReinstall passes /UPDATE /P; data checkbox removed.\n'
      printf 'Unicode true\nManifestDPIAware true\n'
      for i in $(seq 1 30); do printf '; filler %s\n' "$i"; done
      printf '; upstream: tauri-cli 9.9.9 %s\n' "$up"
      printf 'Function PageLeaveReinstall\nFunctionEnd\n'
    } > "$d/src-tauri/windows/installer.nsi"
  fi
  local tline=""
  [ "$tpl" = - ] || tline="\"template\": \"$tpl\","
  cat > "$d/src-tauri/tauri.conf.json" <<EOF
{
  "productName": "Voicen",
  "version": "0.1.0",
  "bundle": {
    "targets": ["nsis"],
    "windows": {
      "wix": { "template": "decoy.wxs" },
      "nsis": {
        $tline
        "installMode": "currentUser",
        "compression": "zlib"
      }
    }
  }
}
EOF
  cat > "$d/package.json" <<EOF
{
  "name": "voicen",
  "version": "0.1.0",
  "dependencies": {
    "@tauri-apps/api": "^2"
  },
  "devDependencies": {
    "@playwright/test": "^1.63.0",
    "@tauri-apps/cli": "$spec",
    "vite": "^8.0.16"
  }
}
EOF
  cat > "$d/pnpm-lock.yaml" <<EOF
lockfileVersion: '6.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

dependencies:
  '@tauri-apps/api':
    specifier: ^2
    version: 2.12.1

devDependencies:
  '@playwright/test':
    specifier: ^1.63.0
    version: 1.63.0
  '@tauri-apps/cli':
    specifier: $lspec
    version: $lver
  vite:
    specifier: ^8.0.16
    version: 8.3.2

packages:

  /@tauri-apps/cli-linux-x64-gnu@2.99.0:
    resolution: {integrity: sha512-decoy}
    engines: {node: '>= 10'}
    dev: true

  /@tauri-apps/cli@$lver:
    resolution: {integrity: sha512-decoy}
    engines: {node: '>= 10'}
    hasBin: true
    dev: true
EOF
  installed "$1" @tauri-apps/cli-linux-x64-gnu 2.99.0 cli-linux-x64-gnu
  [ "$inst" = - ] || installed "$1" @tauri-apps/cli "$inst"
}

# installed <case> <name> <version> [dir under node_modules/@tauri-apps, default cli]: writes the
# package manifest pnpm leaves in node_modules.
installed() {
  local m="$tmp/$1/node_modules/@tauri-apps/${4:-cli}"
  mkdir -p "$m"
  cat > "$m/package.json" <<EOF
{
  "name": "$2",
  "version": "$3",
  "description": "Command line interface for building Tauri apps",
  "bin": { "tauri": "tauri.js" },
  "optionalDependencies": {
    "@tauri-apps/cli-linux-x64-gnu": "2.99.0"
  }
}
EOF
}

# lock_entry <case> <line>...: replaces the two body lines of the root devDependencies
# '@tauri-apps/cli' entry in the case's pnpm-lock.yaml with the given lines (written as is).
lock_entry() {
  local f="$tmp/$1/pnpm-lock.yaml" body
  shift
  body="$(printf '%s\n' "$@")"
  BODY="$body" python3 -I -c '
import os, sys
p = sys.argv[1]
lines = open(p).read().split("\n")
i = lines.index("devDependencies:")
k = lines.index("  \x27@tauri-apps/cli\x27:", i)
lines[k + 1:k + 3] = os.environ["BODY"].rstrip("\n").split("\n")
open(p, "w").write("\n".join(lines))
' "$f"
}

failed=0
passed=0
# check <case> <want exit> <needle>...: runs the script on the case's tree.
check() {
  local c="$1" want="$2" out rc n miss=""
  shift 2
  out="$("$script" "$tmp/$c" 2>&1)"
  rc=$?
  for n in "$@"; do grep -qF -- "$n" <<<"$out" || miss="$miss '$n'"; done
  if [ "$rc" -ne "$want" ] || [ -n "$miss" ]; then
    echo "FAIL $c: exit $rc (want $want)${miss:+, output lacks$miss}" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
  else
    echo "ok   $c: exit $rc"
    passed=$((passed + 1))
  fi
}
H="; upstream: tauri-cli 2.12.1 $up"

# ---- good trees ---------------------------------------------------------------------------------
mk ok-match "$H" 2.12.1 2.12.1 2.12.1
check ok-match 0 "ok:" "2.12.1"
mk ok-other-version "; upstream: tauri-cli 2.13.0 $up" 2.13.0 2.13.0 2.13.0
check ok-other-version 0 "ok:" "2.13.0"
mk ok-crlf "$H" 2.12.1 2.12.1 2.12.1
sed -i 's/$/\r/' "$tmp/ok-crlf/src-tauri/windows/installer.nsi" "$tmp/ok-crlf/pnpm-lock.yaml" \
  "$tmp/ok-crlf/node_modules/@tauri-apps/cli/package.json"
check ok-crlf 0 "ok:"
mk ok-lock-quoted "$H" 2.12.1 "'2.12.1'" "2.12.1(decoy@1.0.0)"
check ok-lock-quoted 0 "ok:"

# ---- header ---------------------------------------------------------------------------------------
mk v-header-lock-mismatch "; upstream: tauri-cli 2.12.0 $up" 2.12.1 2.12.1 2.12.1
check v-header-lock-mismatch 1 "versions differ" "2.12.0" "2.12.1"
mk v-header-missing "" 2.12.1 2.12.1 2.12.1
check v-header-missing 1 "installer.nsi" "upstream header"
mk v-fork-missing - 2.12.1 2.12.1 2.12.1
check v-fork-missing 1 "src-tauri/windows/installer.nsi: missing"
mk v-header-twice "$H"$'\n'"$H" 2.12.1 2.12.1 2.12.1
check v-header-twice 1 "2 upstream header"
mk v-header-other-path "; upstream: tauri-cli 2.12.1 crates/tauri-bundler/src/bundle/windows/nsis/utils.nsh" 2.12.1 2.12.1 2.12.1
check v-header-other-path 1 "upstream header"
mk v-header-not-semver "; upstream: tauri-cli 2.12 $up" 2.12.1 2.12.1 2.12.1
check v-header-not-semver 1 "upstream header"

# ---- package.json specifier -------------------------------------------------------------------------
i=0
for s in '^2' '~2.12.1' '^2.12.1' '>=2.12.1' '2.12.x' '2' 'latest' '2.12.1-rc.1' '02.12.1'; do
  i=$((i + 1))
  c="v-spec-$i"
  mk "$c" "$H" "$s" "$s" 2.12.1
  check "$c" 1 "package.json" "\"$s\", not an exact"
done
# Today's repository shape: no fork, caret specifier. Both are listed.
mk v-today - '^2' '^2' 2.12.1 -
check v-today 1 "installer.nsi: missing" "\"^2\", not an exact" "template is not set"

# ---- installed @tauri-apps/cli (node_modules manifest; the lockfile is not read, T-093) ------------
I="node_modules/@tauri-apps/cli/package.json"
# pnpm installed 2.13.0 from a lockfile that moved: the fork and the pin stay at 2.12.1.
mk v-lock-version-differs "$H" 2.12.1 2.12.1 2.13.0
check v-lock-version-differs 1 "versions differ" "2.13.0" "$I"
# The T-086 reviewer's lockfile (T-086.reviews/1.md finding 1): a quoted scalar spanning lines.
# YAML (and pnpm) read version 2.13.0, the old awk reader read 2.12.1 and passed (exit 0).
mk v-lock-quote-split "$H" 2.12.1 2.12.1 2.12.1 "" 2.13.0
lock_entry v-lock-quote-split "    version: 2.13.0" "    specifier: '2.12.1" "    version: 2.12.1'"
check v-lock-quote-split 1 "versions differ" "2.13.0" "$I"
# probe2 (T-093 investigation): passes pnpm's frozen specifier check, pnpm installs 2.13.0; the
# old guard passed it (exit 0).
mk v-lock-quote-split-2 "$H" 2.12.1 2.12.1 2.12.1 "" 2.13.0
lock_entry v-lock-quote-split-2 "    specifier: 2.12.1" "    version: 2.13.0" "    x: '" "    version: 2.12.1'"
check v-lock-quote-split-2 1 "versions differ" "2.13.0" "$I"
# The lockfile says 2.12.1, but node_modules holds 2.13.0: the installed one decides.
mk v-installed-differs "$H" 2.12.1 2.12.1 2.12.1 "" 2.13.0
check v-installed-differs 1 "versions differ" "2.13.0" "$I"
# The pin moved alone: fork and installed CLI at 2.12.1, package.json at 2.13.0.
mk v-spec-differs "$H" 2.13.0 2.13.0 2.13.0 "" 2.12.1
check v-spec-differs 1 "versions differ" "2.13.0" "package.json"
# The installed version is compared as an exact X.Y.Z, not normalised.
i=0
for s in '2.12.1-rc.1' '2.12.1+build.1' '^2.12.1' '2.12' '02.12.1' ' 2.12.1' '2.12.1 ' 'v2.12.1' ''; do
  i=$((i + 1))
  c="v-installed-not-exact-$i"
  mk "$c" "$H" 2.12.1 2.12.1 2.12.1 "" "$s"
  check "$c" 1 "$I"
done
# A version that is not a string (JSON number) is not X.Y.Z either.
mk v-installed-not-exact-number "$H" 2.12.1 2.12.1 2.12.1
sed -i 's/"version": "2.12.1"/"version": 2.12/' "$tmp/v-installed-not-exact-number/$I"
check v-installed-not-exact-number 1 "$I"
# No "version" key at all.
mk v-installed-no-version "$H" 2.12.1 2.12.1 2.12.1
sed -i '/"version":/d' "$tmp/v-installed-no-version/$I"
check v-installed-no-version 1 "$I"
# The manifest at the CLI's path is another package's (a platform package, a fork, a stub).
mk v-installed-wrong-name "$H" 2.12.1 2.12.1 2.12.1 "" -
installed v-installed-wrong-name @tauri-apps/cli-linux-x64-gnu 2.12.1
check v-installed-wrong-name 1 "$I" "@tauri-apps/cli-linux-x64-gnu"
mk v-installed-no-name "$H" 2.12.1 2.12.1 2.12.1
sed -i '/"name":/d' "$tmp/v-installed-no-name/$I"
check v-installed-no-name 1 "$I"
# The lockfile is not read: none, or one the awk reader could not parse, decides nothing.
mk ok-no-lock "$H" 2.12.1 2.12.1 2.12.1
rm "$tmp/ok-no-lock/pnpm-lock.yaml"
check ok-no-lock 0 "ok:" "2.12.1"
mk ok-lock-stale-not-read "$H" 2.12.1 '^2' 2.99.0 "" 2.12.1
check ok-lock-stale-not-read 0 "ok:" "2.12.1"

# ---- tauri.conf.json --------------------------------------------------------------------------------
mk v-template-unset "$H" 2.12.1 2.12.1 2.12.1 -
check v-template-unset 1 "bundle.windows.nsis.template is not set"
mk v-template-other "$H" 2.12.1 2.12.1 2.12.1 installer.nsi
check v-template-other 1 "template is \"installer.nsi\""

# ---- cannot run ---------------------------------------------------------------------------------------
# No installed CLI (pnpm install not run): the guard cannot tell what builds the installer.
mk c-not-installed "$H" 2.12.1 2.12.1 2.12.1 "" -
check c-not-installed 3 "cannot run" "$I" "pnpm install"
mk c-installed-bad-json "$H" 2.12.1 2.12.1 2.12.1
printf '{ "name": "@tauri-apps/cli", "version": ' > "$tmp/c-installed-bad-json/$I"
check c-installed-bad-json 3 "cannot run" "$I"
mk c-bad-json "$H" 2.12.1 2.12.1 2.12.1
printf '{ "devDependencies": ' > "$tmp/c-bad-json/package.json"
check c-bad-json 3 "cannot run" "package.json"

# ---- the real repository -------------------------------------------------------------------------------
label=real-repo
out="$("$script" "$root" 2>&1)"
rc=$?
if [ "$rc" -ne 0 ] || ! grep -qF "ok:" <<<"$out"; then
  echo "FAIL $label: exit $rc (want 0)" >&2
  sed 's/^/       | /' <<<"$out" >&2
  failed=$((failed + 1))
else
  echo "ok   $label: $out"
  passed=$((passed + 1))
fi

if [ "$failed" != 0 ]; then
  echo "nsis-fork.test: $failed case(s) failed, $passed ok" >&2
  exit 1
fi
echo "nsis-fork.test: ok: $passed cases as expected"
