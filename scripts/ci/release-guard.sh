#!/usr/bin/env bash
# T-085 (rca, class guard-model, F-003, F-015; decisions #120, P-020) after T-026 (decisions #102,
# #103): only the release job's token can write repository contents, and that job is exactly the
# reviewed text. A GitHub Release is written by `bash scripts/ci/release.sh` (release-check.sh,
# then SHA256SUMS.txt, the notes and `gh release create --verify-tag`; host test release.test.sh)
# in the release job, which is a v* tag push job that needs gate and windows.
#
# The guard decides on raw text and on a literal it owns, never on a reading of YAML, shell or gh
# syntax (P-020): it has no tokenizer, no scalar reader and no comment reader, so a comment is
# text like any other. Every regular file directly in <workflows-dir> (any name, any extension)
# is read line by line:
#   R1 the two lines just before the file's first `jobs:` line are exactly `permissions:` and
#      `  contents: read` (the workflow-level token is read-only, in every file; GitHub applies a
#      job's own `permissions:` instead of it, so only a job that names one can write);
#   R2 the line `  release:` occurs exactly once in the dir, and its block (that line up to the
#      next line indented 0-2 columns, or the end of the file) is byte-identical to the guard's
#      owned copy, scripts/ci/release-job.yml (a trailing space or a CR is a difference); the copy
#      pins the tag `if:`, `needs: [gate, windows]`, `contents: write` and the one release.sh step
#      with GH_TOKEN;
#   R3 the substring `permissions` (any case) occurs only on R1's `permissions:` line and once
#      inside the release block;
#   R4 every occurrence of `secrets` (any case) is exactly secrets.TELEGRAM_BOT_TOKEN or
#      secrets.TELEGRAM_CHAT_ID, ended by a character outside [A-Za-z0-9_] or the line's end (no
#      write-capable token from outside `permissions:`: no PAT, no secrets['...'], no
#      toJSON(secrets), no `secrets: inherit`);
#   R5 the word gh (any case, bounded by characters outside [A-Za-z0-9_]: GH_TOKEN, gh_pages and
#      ghcr do not count; $GH, ${GH:-gh}, GH=gh, gh.exe and /usr/bin/gh do) occurs nowhere outside
#      the release block.
# R1, R3 and R4 are the capability lock (GitHub enforces the token scope; it also covers gh api
# and curl); R2 pins the release job as one literal; R5 is the early, fail-closed tripwire.
# Not caught (deliberate only): YAML escapes in a quoted key (`"permi\x73sions"`); a token written
# into the workflow as text (the gitleaks pre-commit scan refuses it); release.sh itself exiting 0
# on a refusal (release.test.sh covers that); whether GitHub enforces `permissions:` as documented
# (platform semantics, docs/decisions/ci-toolchain.md); a repository default token scope flipped
# to write (R1 pins the workflow level, so it does not matter for jobs without their own block).
#
# Usage: scripts/ci/release-guard.sh <workflows-dir>
# Exit 0: ok. Exit 1: a violation, each listed as "<workflows-dir>/<file>:<line>: R<n>: <why>"
# (R2: the first line of the block that differs from the owned copy, or the second `  release:`
# line; R3-R5: the line of the occurrence). Exit 2: usage error. Exit 3: cannot run (the dir is
# missing or not a directory, holds no regular file, a file is unreadable or has no top-level
# `jobs:` line, the owned copy is missing or malformed, awk failed); never a pass.
# Host test: scripts/ci/release-guard.test.sh (make check-release-guard).
set -uo pipefail
export LC_ALL=C
if [ "$#" -ne 1 ]; then
  echo "usage: scripts/ci/release-guard.sh <workflows-dir>" >&2
  exit 2
fi
cannot_run() { echo "release-guard: cannot run: $1" >&2; exit 3; }
dir="$1"
while [ "${#dir}" -gt 1 ] && [ "${dir%/}" != "$dir" ]; do dir="${dir%/}"; done
[ -d "$dir" ] || cannot_run "$dir is not a directory (missing, or a file: give the workflows dir)"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)" || cannot_run "cannot locate the script dir"
owned="$here/release-job.yml"
[ -f "$owned" ] && [ -r "$owned" ] || cannot_run "owned copy $owned not found or not readable"
[ "$(head -n 1 -- "$owned")" = "  release:" ] || cannot_run "owned copy $owned does not start with the line '  release:'"

shopt -s nullglob dotglob
files=()
for f in "$dir"/*; do
  [ -f "$f" ] || continue
  [ -r "$f" ] || cannot_run "$f is not readable"
  grep -q -x 'jobs:' -- "$f" || cannot_run "$f has no top-level jobs: line (not a workflow)"
  files+=("$f")
done
shopt -u nullglob dotglob
[ "${#files[@]}" -gt 0 ] || cannot_run "no regular file in $dir"

# Output lines: "V <file>:<line>: R<n>: <why>"; awk exit 3 = the owned copy could not be read.
report="$(awk -v owned="$owned" '
function v(k, i, r, why) { print "V " F[k] ":" i ": " r ": " why; nv++ }
# count(s, t): occurrences of t in s.
function count(s, t,   n, p) { n = 0; while ((p = index(s, t)) > 0) { n++; s = substr(s, p + length(t)) } return n }
function closeblock(k) {
  if (!inb) return
  if (!bdiff && boff < no) bdiff = bs + boff
  if (bdiff) v(k, bdiff, "R2", "the release job differs from the owned copy scripts/ci/release-job.yml here (byte for byte; edit both together)")
  inb = 0
}
BEGIN {
  no = 0
  while ((rc = (getline line < owned)) > 0) own[++no] = line
  if (rc < 0 || no == 0) { bad = 1; exit 3 }
  close(owned)
}
FNR == 1 { nf++; F[nf] = FILENAME }
{ N[nf] = FNR; L[nf, FNR] = $0 }
END {
  if (bad) exit 3
  nrel = 0
  for (k = 1; k <= nf; k++) {
    j = 0; p = 0
    for (i = 1; i <= N[k]; i++) {
      if (L[k, i] == "jobs:") { j = i; break }
      if (!p && L[k, i] == "permissions:") p = i
    }
    r1 = 0
    if (j >= 3 && L[k, j - 2] == "permissions:" && L[k, j - 1] == "  contents: read") r1 = j - 2
    else v(k, (p ? p : j), "R1", "the two lines before jobs: must be exactly \"permissions:\" and \"  contents: read\" (read-only token at workflow level)")
    inb = 0
    for (i = 1; i <= N[k]; i++) {
      s = L[k, i]
      if (inb && (s ~ /^[^ ]/ || s ~ /^ [^ ]/ || s ~ /^  [^ ]/)) closeblock(k)
      if (s == "  release:") {
        nrel++
        if (nrel > 1) v(k, i, "R2", "a second \"  release:\" job line in the workflows dir (the release job occurs once)")
        inb = 1; bs = i; boff = 0; bdiff = 0; bperm = 0
      }
      lo = tolower(s)
      n = count(lo, "permissions")
      if (n > 0) {
        if (i == r1) n--
        else if (inb && !bperm) { n--; bperm = 1 }
        if (n > 0) v(k, i, "R3", "\"permissions\" outside the workflow-level read block and the release job (no other job or file may widen the token)")
      }
      t = lo; c = 0
      while ((q = index(t, "secrets")) > 0) {
        if (substr(s, c + q) !~ /^secrets\.TELEGRAM_(BOT_TOKEN|CHAT_ID)([^A-Za-z0-9_]|$)/) {
          v(k, i, "R4", "a secret other than secrets.TELEGRAM_BOT_TOKEN / secrets.TELEGRAM_CHAT_ID (a token from outside the job permissions)")
          break
        }
        c += q + 6; t = substr(t, q + 7)
      }
      if (!inb && lo ~ /(^|[^a-z0-9_])gh([^a-z0-9_]|$)/)
        v(k, i, "R5", "the word gh outside the release job (only scripts/ci/release.sh in that job may call gh)")
      if (inb) {
        boff++
        if (!bdiff && (boff > no || s != own[boff])) bdiff = i
      }
    }
    closeblock(k)
  }
  if (nrel == 0)
    for (k = 1; k <= nf; k++) {
      for (i = 1; i <= N[k]; i++) if (L[k, i] == "jobs:") break
      v(k, i, "R2", "no \"  release:\" job line in the workflows dir (the release job is the owned copy scripts/ci/release-job.yml)")
    }
  exit (nv > 0 ? 1 : 0)
}
' "${files[@]}")"
rc=$?
case "$rc" in
  0) ;;
  1)
    sed -n 's/^V //p' <<<"$report" >&2
    echo "release-guard: FAIL: a token other than the release job's could write contents, or the release job is not the owned copy (T-085, P-020)" >&2
    exit 1
    ;;
  *) cannot_run "awk failed (exit $rc) on $dir or the owned copy $owned" ;;
esac
grep -q '^V ' <<<"$report" && cannot_run "awk listed violations but exited 0"
echo "release-guard: ok: $dir: ${#files[@]} file(s); token read-only but in the release job, which is the owned copy; no other secret, permissions or gh"
