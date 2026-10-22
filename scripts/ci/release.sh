#!/usr/bin/env bash
# T-085 (rca, decisions #120, P-020) after T-026 (spec 006 US5, decisions #102, #103): the one
# step of ci.yml's release job (scripts/ci/release-job.yml, pinned by release-guard.sh R2). The
# release check and the publish are one script, so their order and the exit status are proven by
# running it (scripts/ci/release.test.sh, make check-release), not by reading YAML.
#
# Contract:
#   - GH_TOKEN unset or empty: exit 1 naming GH_TOKEN, before any gh call and before anything is
#     written (gh would otherwise fall back to GITHUB_TOKEN or a stored login).
#   - scripts/ci/release-check.sh <tag> <run_number> <root> <installer dir> (the one release
#     verdict, unchanged; the committed, unstamped checkout gives the repository's MAJOR.MINOR):
#     a refusal exits 1 with no gh write call and nothing written.
#   - On its pass: <installer dir>/SHA256SUMS.txt in sha256sum format ("<sha256>  <name>"),
#     <installer dir>/release-notes.md (version X.Y.Z, the commit from VOICEN_COMMIT when set, the
#     installer's name, the SmartScreen note of FR-030, how to check the hash), then exactly one
#     `gh release create <tag> <installer> <dir>/SHA256SUMS.txt --verify-tag --title ...
#     --notes-file <notes>` (gh uploads to a draft and publishes it last). A failed create exits
#     non-zero. The repository is GH_REPO's (or the checkout's).
#   - GH_TOKEN is never printed: release-check.sh masks it in gh's output, and the output of
#     `gh --version` and `gh release create` (stdout and stderr, success or failure) is captured
#     and printed with GH_TOKEN replaced by `***` (gh_masked); gh's own exit status, not the
#     masking's, decides (a failed create is never reported ok).
#
# Usage: scripts/ci/release.sh <tag> <run_number> <root> <installer dir>
# Exit 0: published. Exit 1: refused or the publish failed. Exit 2: wrong usage.
set -euo pipefail
if [ "$#" -ne 4 ]; then
  echo "usage: release.sh <tag> <run_number> <root> <installer dir>" >&2
  exit 2
fi
tag="$1"
run_number="$2"
root="$3"
dist="${4%/}"
[ -n "$dist" ] || dist="$4"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ -z "${GH_TOKEN:-}" ]; then
  echo "::error::release: GH_TOKEN is not set or empty; nothing is checked or published (the release job sets it)" >&2
  exit 1
fi

if ! bash "$here/release-check.sh" "$tag" "$run_number" "$root" "$dist"; then
  echo "::error::release: release-check.sh refused $tag; nothing was written or published" >&2
  exit 1
fi

# release-check.sh passed: exactly one *-setup.exe carrying the tag's version is in $dist.
shopt -s nullglob
files=("$dist"/*-setup.exe)
shopt -u nullglob
if [ "${#files[@]}" -ne 1 ]; then
  echo "::error::release: ${#files[@]} *-setup.exe files in $dist after the check, expected exactly 1" >&2
  exit 1
fi
installer="$(basename -- "${files[0]}")"
version="${tag#v}"

(cd "$dist" && sha256sum -- "$installer" > SHA256SUMS.txt)
cat -- "$dist/SHA256SUMS.txt"

commit=""
[ -n "${VOICEN_COMMIT:-}" ] && commit=", commit ${VOICEN_COMMIT}"
notes="$dist/release-notes.md"
{
  echo "Voicen ${version} for Windows 10/11 x64${commit}."
  echo
  echo "Install: download \`${installer}\` and run it (per-user install, no administrator rights)."
  echo
  echo "The installer is not code-signed in this release: Windows SmartScreen may show \"Windows protected your PC\". To proceed, click \"More info\", then \"Run anyway\"."
  echo
  echo "Check the download: the SHA-256 in \`SHA256SUMS.txt\` must match \`Get-FileHash ${installer}\` (PowerShell) or \`sha256sum ${installer}\`."
} > "$notes"
cat -- "$notes"

# gh_masked <args...>: run gh with stdout and stderr captured, print that output with GH_TOKEN
# masked as *** (stdout on success, stderr on failure), return gh's exit status. No pipe, so the
# mask cannot hide gh's status.
gh_masked() {
  local out rc=0
  out="$(gh "$@" 2>&1)" || rc=$?
  out="${out//"$GH_TOKEN"/***}"
  if [ -n "$out" ]; then
    if [ "$rc" = 0 ]; then printf '%s\n' "$out"; else printf '%s\n' "$out" >&2; fi
  fi
  return "$rc"
}

gh_masked --version
if ! gh_masked release create "$tag" "$dist/$installer" "$dist/SHA256SUMS.txt" --verify-tag --title "Voicen $tag" --notes-file "$notes"; then
  echo "::error::release: gh release create $tag failed; the release is not published (a draft may remain)" >&2
  exit 1
fi
echo "release: ok: $tag published with $installer and SHA256SUMS.txt"
