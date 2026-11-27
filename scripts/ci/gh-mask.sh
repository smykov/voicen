# T-179 (T-026 r6#4): the one GH_TOKEN mask of the release scripts, sourced by
# scripts/ci/release-check.sh and scripts/ci/release.sh (pinned by their echo cases in
# release-check.test.sh and release.test.sh). Not run on its own.
#
# gh_mask_out <args...>: runs `gh <args...>` with stdout and stderr captured together and prints
# that output with every occurrence of GH_TOKEN's value replaced by `***` (a raw string
# replacement; gh's output syntax is not read). Returns gh's own exit status, not the masking's.
# GH_TOKEN must be set and non-empty (both callers check it first). Use it in a command
# substitution: out="$(gh_mask_out ...)" || rc=$?
gh_mask_out() {
  local out rc=0
  out="$(gh "$@" 2>&1)" || rc=$?
  out="${out//"$GH_TOKEN"/***}"
  printf '%s' "$out"
  return "$rc"
}
