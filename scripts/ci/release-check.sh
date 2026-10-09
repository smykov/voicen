#!/usr/bin/env bash
# T-026 (decisions #102, #103): the one release verdict of .github/workflows/ci.yml. The release
# job runs it before sha256sum and gh release create; the windows job runs it on every ref as a
# premise (tag v$VOICEN_VERSION, no token) and, off v* tags, as a failure branch with synthetic bad
# tags. A refusal is exit 1 before any gh write call; this script never writes anything.
#
# Contract (host test: scripts/ci/release-check.test.sh, make check-release-check):
#   - <tag> must be vX.Y.Z (decimal, no leading zeros, each part <= 65535), else exit 1 naming it.
#   - The committed MAJOR.MINOR is read from <root> by scripts/ci/version-stamp.sh itself (the one
#     reader of Cargo.toml [workspace.package] version, the top-level "version" of
#     src-tauri/tauri.conf.json and package.json, and the Cargo.lock workspace members), run on a
#     temporary copy of those files, so the checkout is never written. Files that disagree or are
#     not X.Y.Z -> exit 1. X.Y must equal that MAJOR.MINOR (#102), else exit 1 naming both.
#   - <run_number> must be a decimal number (github.run_number), else exit 1 naming it. Z must be
#     >= it as a number (#103; Z = run number passes: no earlier run had it), else exit 1 naming
#     Z and the run number.
#   - <installer dir> must hold exactly one *-setup.exe whose name contains _X.Y.Z_, else exit 1
#     (a wrong version names the installer's version and the tag's).
#   - With GH_TOKEN set: `gh release view <tag>` (repository from GH_REPO or the checkout) decides
#     whether the release exists: exit 0 -> exit 1 (never overwrite); a failure saying "release not
#     found" -> no release; any other failure -> exit 1 (cannot tell, never a pass). Without
#     GH_TOKEN (the windows premise) gh is not called and the output says so.
#   - gh is asked only after every other check passed; no gh write subcommand is ever called and
#     GH_TOKEN is never printed (masked in gh's own output).
#   - Exit 0: the release may be published; prints "installer: <dir>/<file>".
# Every refusal reason is printed as a ::error:: line; all reasons found are listed.
#
# Usage: scripts/ci/release-check.sh <tag> <run_number> <root> <installer dir>
# Exit 0: releasable. Exit 1: refused (reasons listed). Exit 2: wrong usage.
set -uo pipefail
if [ "$#" -ne 4 ]; then
  echo "usage: release-check.sh <tag> <run_number> <root> <installer dir>" >&2
  exit 2
fi
tag="$1"
run_number="$2"
root="$3"
dir="${4%/}"
[ -n "$dir" ] || dir="$4"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

refused=0
refuse() { echo "::error::release-check: $*" >&2; refused=1; }

# A number part of a version: decimal, no leading zero, <= 65535 (as version-stamp.sh).
part_ok() { [[ "$1" =~ ^(0|[1-9][0-9]{0,4})$ ]] && [ "$1" -le 65535 ]; }

# ---- the tag -----------------------------------------------------------------------------------
tx="" ty="" tz=""
if [[ "$tag" =~ ^v([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
  # part_ok's own =~ overwrites BASH_REMATCH: take the parts first.
  tx="${BASH_REMATCH[1]}" ty="${BASH_REMATCH[2]}" tz="${BASH_REMATCH[3]}"
  part_ok "$tx" && part_ok "$ty" && part_ok "$tz" || tx="" ty="" tz=""
fi
[ -n "$tx" ] || refuse "tag '$tag' is not vX.Y.Z (decimal, no leading zeros, each part <= 65535)"

# ---- the run number ----------------------------------------------------------------------------
run_ok=0
if [ -z "$run_number" ]; then
  refuse "run number is empty (github.run_number of this run is needed for decision #103)"
elif [[ "$run_number" =~ ^[1-9][0-9]{0,9}$ ]]; then
  run_ok=1
else
  refuse "run number '$run_number' is not a decimal number"
fi

# ---- the committed MAJOR.MINOR (read by version-stamp.sh on a copy) ----------------------------
repo_mm=""
work="$(mktemp -d)" || { echo "::error::release-check: mktemp failed" >&2; exit 1; }
trap 'rm -rf "$work"' EXIT
if mkdir -p "$work/src-tauri" &&
  cp "$root/Cargo.toml" "$root/Cargo.lock" "$root/package.json" "$work/" 2>/dev/null &&
  cp "$root/src-tauri/tauri.conf.json" "$work/src-tauri/" 2>/dev/null; then
  # A branch stamp with run 1 gives <repo MAJOR.MINOR>.1 after version-stamp.sh checked the files.
  if stamp="$(env -u GITHUB_ENV bash "$here/version-stamp.sh" branch release-check 1 "$work" 2>&1)"; then
    v="$(sed -n 's/^version: //p' <<<"$stamp")"
    repo_mm="${v%.*}"
    [ -n "$repo_mm" ] || refuse "no MAJOR.MINOR read from the version files under $root"
  else
    sed 's/^/release-check: /' <<<"$stamp" >&2
    refuse "the committed version files under $root give no single MAJOR.MINOR (see above)"
  fi
else
  refuse "cannot read the version files under $root (Cargo.toml, Cargo.lock, package.json, src-tauri/tauri.conf.json)"
fi

# ---- #102 and #103 -----------------------------------------------------------------------------
if [ -n "$tx" ] && [ -n "$repo_mm" ] && [ "$tx.$ty" != "$repo_mm" ]; then
  refuse "tag $tag has MAJOR.MINOR $tx.$ty, the repository's MAJOR.MINOR is $repo_mm (decision #102)"
fi
if [ -n "$tz" ] && [ "$run_ok" = 1 ] && [ "$tz" -lt "$run_number" ]; then
  refuse "tag patch $tz is below this run's number $run_number; the patch must be >= the tag run's github.run_number (decision #103)"
fi

# ---- the installer -----------------------------------------------------------------------------
installer=""
if [ ! -d "$dir" ]; then
  refuse "installer dir $dir not found"
else
  shopt -s nullglob
  files=("$dir"/*-setup.exe)
  shopt -u nullglob
  if [ "${#files[@]}" -ne 1 ]; then
    refuse "${#files[@]} *-setup.exe files in $dir, expected exactly 1"
  else
    installer="${files[0]}"
    name="$(basename "$installer")"
    if [ -n "$tx" ] && [[ "$name" != *"_$tx.$ty.${tz}_"* ]]; then
      if [[ "$name" =~ _([0-9]+\.[0-9]+\.[0-9]+)_ ]]; then
        refuse "installer $name has version ${BASH_REMATCH[1]}, the tag $tag wants $tx.$ty.$tz"
      else
        refuse "installer $name does not contain _${tx}.${ty}.${tz}_ (the tag $tag)"
      fi
    fi
  fi
fi

if [ "$refused" != 0 ]; then
  echo "release-check: refused: nothing may be published for '$tag'" >&2
  exit 1
fi

# ---- does the release exist? -------------------------------------------------------------------
if [ -z "${GH_TOKEN:-}" ]; then
  echo "release-check: release-exists check not made: GH_TOKEN is not set (the windows premise has no token; the release job sets it)"
else
  rc=0
  out="$(gh release view "$tag" 2>&1)" || rc=$?
  out="${out//"$GH_TOKEN"/***}"
  if [ "$rc" = 0 ]; then
    echo "::error::release-check: a release for tag $tag already exists; it is never overwritten" >&2
    exit 1
  elif grep -qi 'release not found' <<<"$out"; then
    echo "release-check: no release for tag $tag yet"
  else
    sed 's/^/gh: /' <<<"$out" >&2
    echo "::error::release-check: gh release view $tag failed (exit $rc) for another reason than 'release not found'; whether the release exists is unknown" >&2
    exit 1
  fi
fi

echo "release-check: ok: tag $tag, repository MAJOR.MINOR $repo_mm, run number $run_number"
echo "installer: $installer"
