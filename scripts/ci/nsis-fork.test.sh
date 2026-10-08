#!/usr/bin/env bash
# T-025 (decisions #76): host test of scripts/ci/nsis-fork.sh, the gate check that the NSIS
# template fork (src-tauri/windows/installer.nsi), the exact @tauri-apps/cli pin in package.json
# and the version pnpm-lock.yaml installs agree, and that tauri.conf.json uses the fork.
# Contract: the header of nsis-fork.sh. Cases are fixture trees built in a temp dir:
#   ok-*  exit 0 and "ok:";  v-*  exit 1, every listed needle in the output;  c-*  exit 3 and
#   "cannot run". Then the real repository must pass (exit 0): that case is red until T-025's fork
#   and pin land.
# Usage: scripts/ci/nsis-fork.test.sh   (host bash, awk, python3)
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
# A tree shaped like the repository, with decoys: @tauri-apps/api (^2) before the CLI in both
# package.json and the lockfile, the platform packages of the CLI in the packages section at
# another version, and a "; upstream:" decoy comment far below the header.
mk() {
  local i d="$tmp/$1" hdr="$2" spec="$3" lspec="$4" lver="$5" tpl="${6:-windows/installer.nsi}"
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
sed -i 's/$/\r/' "$tmp/ok-crlf/src-tauri/windows/installer.nsi" "$tmp/ok-crlf/pnpm-lock.yaml"
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

# ---- lockfile -------------------------------------------------------------------------------------
mk v-lock-stale-specifier "$H" 2.12.1 '^2' 2.12.1
check v-lock-stale-specifier 1 "pnpm-lock.yaml" "specifier is '^2'"
mk v-lock-version-differs "$H" 2.12.1 2.12.1 2.13.0
check v-lock-version-differs 1 "versions differ" "2.13.0"
mk v-lock-no-entry "$H" 2.12.1 2.12.1 2.12.1
sed -i "/^  '@tauri-apps\/cli':/,+2d" "$tmp/v-lock-no-entry/pnpm-lock.yaml"
check v-lock-no-entry 1 "pnpm-lock.yaml: no single '@tauri-apps/cli' entry"
# The CLI only under dependencies (not the root devDependencies): not the pinned dev tool.
mk v-lock-wrong-block "$H" 2.12.1 2.12.1 2.12.1
sed -i "/^  '@tauri-apps\/cli':/,+2d; s/^  '@tauri-apps\/api':/  '@tauri-apps\/cli':\n    specifier: 2.12.1\n    version: 2.12.1\n  '@tauri-apps\/api':/" "$tmp/v-lock-wrong-block/pnpm-lock.yaml"
check v-lock-wrong-block 1 "no single '@tauri-apps/cli' entry"

# ---- tauri.conf.json --------------------------------------------------------------------------------
mk v-template-unset "$H" 2.12.1 2.12.1 2.12.1 -
check v-template-unset 1 "bundle.windows.nsis.template is not set"
mk v-template-other "$H" 2.12.1 2.12.1 2.12.1 installer.nsi
check v-template-other 1 "template is \"installer.nsi\""

# ---- cannot run ---------------------------------------------------------------------------------------
mk c-no-lock "$H" 2.12.1 2.12.1 2.12.1
rm "$tmp/c-no-lock/pnpm-lock.yaml"
check c-no-lock 3 "cannot run" "pnpm-lock.yaml"
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
