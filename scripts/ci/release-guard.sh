#!/usr/bin/env bash
# T-085 (rca, class guard-model, F-003, F-015; decisions #120, P-020) after T-026 (decisions #102,
# #103, #121): only the release job's token can write repository contents, and that job and the
# workflow header GitHub applies to it are exactly the reviewed text. A GitHub Release is written
# by `bash scripts/ci/release.sh` (release-check.sh, then SHA256SUMS.txt, the notes and
# `gh release create --verify-tag`; host test release.test.sh) in the release job, which is a v*
# tag push job that needs gate and windows.
#
# The guard decides on raw text and on literals it owns, never on a reading of YAML, shell or gh
# syntax (P-020): it has no tokenizer, no scalar reader and no comment reader, so a comment is
# text like any other and no rule depends on where a YAML construct ends. Every regular file
# directly in <workflows-dir> (any name, any extension) is read line by line. A "word" below is
# bounded by characters outside [A-Za-z0-9_] or the line's ends, matched in any case:
#   R1 the word jobs occurs on exactly one line of the file, that line is exactly `jobs:`, and the
#      two lines just before it are exactly `permissions:` and `  contents: read` (the
#      workflow-level token is read-only, in every file; GitHub applies a job's own
#      `permissions:` instead of it, so only a job that names one can write);
#   R2 the line `  release:` occurs exactly once in the dir, and its block (that line up to the
#      end of its file) is byte-identical to the guard's owned copy, scripts/ci/release-job.yml (a
#      trailing space, a CR or any text after the copy, a comment included, is a difference; so
#      the release job is its file's last job); the copy pins the tag `if:`, `needs: [gate,
#      windows]`, `contents: write`, `shell: bash` and the one release.sh step with GH_TOKEN;
#   R3 the substring `permissions` (any case) occurs only on R1's `permissions:` line and once
#      inside the release block;
#   R4 every occurrence of `secrets` (any case) is exactly secrets.TELEGRAM_BOT_TOKEN or
#      secrets.TELEGRAM_CHAT_ID, ended by a character outside [A-Za-z0-9_] or the line's end, and
#      the word vars occurs nowhere (no write-capable token from outside `permissions:`: no PAT,
#      no secrets['...'], no toJSON(secrets), no `secrets: inherit`, no repository variable);
#   R5 the word gh (GH_TOKEN, gh_pages and ghcr do not count; $GH, ${GH:-gh}, GH=gh, gh.exe and
#      /usr/bin/gh do) occurs nowhere outside the release block;
#   R6 no YAML escape that spells a character or joins two lines occurs anywhere: the raw text
#      \x plus 2 hex digits, \u or \U plus 4 hex digits, \0 (NUL, which bash drops from a script,
#      so g\0h runs as gh), or a backslash right before a line break (LF or the end of the file,
#      CR, NEL U+0085, LS U+2028, PS U+2029). Such an escape in a double-quoted scalar would hide
#      secrets, vars, gh or permissions from R3-R5; the raw bytes decide in any scalar style, so no
#      scalar reader is needed (a Windows path such as `Voicen\uninstall.exe` has no 4 hex digits
#      after \u and passes);
#   R7 every file that holds a `  release:` line starts with the guard's second owned copy,
#      scripts/ci/release-header.yml, byte for byte, from line 1 to the file's first line that is
#      exactly `jobs:` (comments included: a comment is text). GitHub applies the workflow-level
#      keys (env:, defaults:, on:, concurrency:, permissions: ...) to the release job, so a
#      BASH_ENV or working-directory there would run with its write token; the literal closes them
#      all without naming any key or variable.
#   R8 none of the raw texts `!!`, `!<` and `%TAG` (case-sensitive) occurs on any line, comments
#      included: a YAML tag (the shorthand `!!binary`, the verbatim `!<tag:...>`, or a handle
#      declared by a `%TAG` directive) could decode other text, e.g. base64, into secrets, vars, gh
#      or permissions and hide it from R3-R5. A raw index() search, no tag reader: a lone `!`
#      (`!cancelled()`, `!=`, `[ ! -f ]`) passes.
#   R9 (T-084, decision #119) the raw token upload-artifact (any case, comments included, a
#      sub-path such as upload-artifact/merge or a fork name too) occurs exactly once in the dir
#      (files in glob order, then line order), and the lines starting at that line are
#      byte-identical to the guard's third owned copy, scripts/ci/release-upload.yml (edit both
#      together): the installer upload step with the release job's own if: (v* tag push only),
#      retention-days: 1 and if-no-files-found: error, ended by the first line of the next step
#      so no key can be appended to it. Zero occurrences = a tag release with nothing to
#      download, two or more = a second upload. R9 finds the literal by the token's line and
#      cannot tell which job or YAML node holds it, so it pins the upload text, not its place
#      (see Not caught). A raw token plus an owned literal, no step reader.
#   R10 (T-087, decisions #93, #124; replaces telegram-guard.sh) the raw token secrets.telegram_
#      (any case, comments included) occurs in the dir (files in glob order, then line order)
#      exactly as often as in the guard's fourth owned copy, scripts/ci/telegram-send-step.yml
#      (edit both together), and the lines aligned on the dir's first token line (that line minus
#      the offset of the copy's first token line) are byte-identical to the copy: the Telegram
#      send step from its name line to its run: line, with its if: (push && (main || v* tag) &&
#      send == 'true', no status function, no continue-on-error), ended by the next step's first
#      line `      - name: SHA-256 of the installer`, so no key can be appended. GitHub expands
#      ${{ }} in run: text before the shell starts (T-086 Investigation section 2), so a comment
#      line is text here too. Zero tokens is refused (the build sends the installer,
#      #93). That the secret cannot be spelled without this raw text rests on R4 (every secrets
#      is one of the two names), R6 (no escape) and R8 (no tag) of this guard. A raw token plus
#      an owned literal, no YAML or expression reader.
# R1, R3 and R4 are the capability lock (GitHub enforces the token scope; it also covers gh api
# and curl); R2 and R7 pin the release job and the header it inherits as two literals; R5 is the
# early, fail-closed tripwire; R6 and R8 keep R3-R5's raw text the text a YAML reader sees; R9 and
# R10 pin the only upload step and the only step the Telegram secrets reach as literals.
# Not caught (deliberate only): a token written into the workflow as text (the gitleaks
# pre-commit scan refuses it); release.sh itself exiting 0 on a refusal (release.test.sh covers
# that); whether GitHub enforces `permissions:` as documented (platform semantics,
# docs/decisions/ci-toolchain.md); a repository default token scope flipped to write (R1 pins the
# workflow level, so it does not matter for jobs without their own block); a YAML parser that
# reads escapes, line breaks or tags beyond YAML 1.1/1.2 (GitHub's parser is not available on the
# host);
# a column-0 key such as `env:` put between two jobs before `  release:` (header and release block
# unchanged, so R2 and R7 pass): YAML then reads `  release:` as a key under that top-level key,
# so the release job no longer exists, no job holds contents: write, and the workflow is invalid
# or releases nothing (probed with PyYAML 6.0.1, not GitHub's parser; T-085 Red tests round 3);
# R9's limits: the owned upload literal moved into another job, or hidden inside other YAML such
# as a `name: |` block scalar, with the real upload step deleted, passes R9; loud, not silent: on
# a v* tag the release job's download fails and nothing is published (T-084 review 1); an upload
# through another action is refused only by ci-wip.sh's fail-closed action list, not by R9;
# R10's limits: the owned send-step literal moved, with the real send step deleted, into a place
# that is not that step (a `run: |` block scalar of a job whose keys are indented 3 columns, a
# step `name: |`) passes R10; whether GitHub then expands the secret there depends on that key's
# context availability, and the send no longer happens on main, so the owner gets no installer
# (loud, not silent); telegram-send.sh itself leaking the token (to GITHUB_ENV, a file or the log)
# is telegram-send.test.sh's concern, not this guard's.
#
# Usage: scripts/ci/release-guard.sh <workflows-dir>
# Exit 0: ok. Exit 1: a violation, each listed as "<workflows-dir>/<file>:<line>: R<n>: <why>"
# (R1: the second line with the word jobs, or the line that is not exactly `jobs:`, or the
# `jobs:` line when the block before it differs; R2: the first line of the block that differs
# from the owned copy, or the second `  release:` line; R3-R6: the line of the occurrence, for an
# escaped line break the line ending in the backslash; R7: the first header line that differs
# from scripts/ci/release-header.yml; R8: every line holding !!, !< or %TAG; R9: the second
# token occurrence, the first line from the token on that differs from release-upload.yml, or,
# with no token, the first file's jobs: line; R10: the token occurrence where the dir count first
# exceeds the copy's, the first aligned line that differs from telegram-send-step.yml, or, with no
# token, the first file's jobs: line). Exit 2: usage
# error. Exit 3: cannot run (the dir is missing or not a directory, holds no regular file, a file
# is unreadable or has no `jobs:` line, an owned copy is missing or malformed: release-job.yml not
# starting with `  release:`, release-header.yml not ending in its only `jobs:` line,
# release-upload.yml missing, empty or not starting with `      - uses: actions/upload-artifact@v4`;
# telegram-send-step.yml missing, empty, holding no secrets.TELEGRAM_ token or not starting with
# `      - name: Send the installer to Telegram (main and v* tags only)`; awk failed);
# never a pass.
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
ownedh="$here/release-header.yml"
[ -f "$ownedh" ] && [ -r "$ownedh" ] || cannot_run "owned header copy $ownedh not found or not readable"
[ "$(tail -n 1 -- "$ownedh")" = "jobs:" ] || cannot_run "owned header copy $ownedh does not end in the line 'jobs:'"
[ "$(grep -c -x 'jobs:' -- "$ownedh")" = "1" ] || cannot_run "owned header copy $ownedh has more than one line 'jobs:' (only its last line may be)"
ownedu="$here/release-upload.yml"
[ -f "$ownedu" ] && [ -r "$ownedu" ] || cannot_run "owned upload copy $ownedu not found or not readable"
[ "$(head -n 1 -- "$ownedu")" = "      - uses: actions/upload-artifact@v4" ] || cannot_run "owned upload copy $ownedu does not start with the line '      - uses: actions/upload-artifact@v4'"
ownedt="$here/telegram-send-step.yml"
[ -f "$ownedt" ] && [ -r "$ownedt" ] || cannot_run "owned send-step copy $ownedt not found or not readable"
[ "$(head -n 1 -- "$ownedt")" = "      - name: Send the installer to Telegram (main and v* tags only)" ] || cannot_run "owned send-step copy $ownedt does not start with the line '      - name: Send the installer to Telegram (main and v* tags only)'"
grep -q -i -F 'secrets.telegram_' -- "$ownedt" || cannot_run "owned send-step copy $ownedt holds no secrets.TELEGRAM_ token (nothing to align on)"

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

# Output lines: "V <file>:<line>: R<n>: <why>"; awk exit 3 = an owned copy could not be read.
report="$(awk -v owned="$owned" -v ownedh="$ownedh" -v ownedu="$ownedu" -v ownedt="$ownedt" '
function v(k, i, r, why) { print "V " F[k] ":" i ": " r ": " why; nv++ }
# count(s, t): occurrences of t in s.
function count(s, t,   n, p) { n = 0; while ((p = index(s, t)) > 0) { n++; s = substr(s, p + length(t)) } return n }
# word(lo, w): the lower-case line lo holds w bounded by characters outside [a-z0-9_] or its ends.
function word(lo, w) { return lo ~ ("(^|[^a-z0-9_])" w "([^a-z0-9_]|$)") }
BEGIN {
  no = 0
  while ((rc = (getline line < owned)) > 0) own[++no] = line
  if (rc < 0 || no == 0) { bad = 1; exit 3 }
  close(owned)
  nh = 0
  while ((rc = (getline line < ownedh)) > 0) oh[++nh] = line
  if (rc < 0 || nh == 0 || oh[nh] != "jobs:") { bad = 1; exit 3 }
  close(ownedh)
  nu = 0
  while ((rc = (getline line < ownedu)) > 0) ou[++nu] = line
  if (rc < 0 || nu == 0) { bad = 1; exit 3 }
  close(ownedu)
  # R10: the owned send-step copy, its token count tco and its first token line tof.
  nt = 0; tco = 0; tof = 0
  while ((rc = (getline line < ownedt)) > 0) {
    ot[++nt] = line
    c = count(tolower(line), "secrets.telegram_")
    if (c && !tof) tof = nt
    tco += c
  }
  if (rc < 0 || nt == 0 || tco == 0) { bad = 1; exit 3 }
  close(ownedt)
}
FNR == 1 { nf++; F[nf] = FILENAME }
{ N[nf] = FNR; L[nf, FNR] = $0 }
END {
  if (bad) exit 3
  nrel = 0; ntok = 0; ttc = 0
  for (k = 1; k <= nf; k++) {
    # R1: one line with the word jobs, exactly "jobs:", right after the read-only block.
    j = 0; jbad = 0
    for (i = 1; i <= N[k]; i++)
      if (word(tolower(L[k, i]), "jobs")) {
        if (!j && L[k, i] == "jobs:") j = i
        else if (!jbad) jbad = i
      }
    r1 = 0
    if (jbad) v(k, jbad, "R1", "the word jobs must occur on exactly one line, and that line must be exactly \"jobs:\"")
    else if (j >= 3 && L[k, j - 2] == "permissions:" && L[k, j - 1] == "  contents: read") r1 = j - 2
    else v(k, j, "R1", "the two lines before jobs: must be exactly \"permissions:\" and \"  contents: read\" (read-only token at workflow level)")
    # R7: a file holding a "  release:" line starts with the owned header copy, line 1 to its
    # first "jobs:" line, byte for byte (the copy has "jobs:" only as its last line).
    hasrel = 0
    for (i = 1; i <= N[k]; i++) if (L[k, i] == "  release:") { hasrel = 1; break }
    if (hasrel) {
      for (i = 1; i <= N[k]; i++) {
        if (i > nh || L[k, i] != oh[i]) break
        if (L[k, i] == "jobs:") break
      }
      if (i > N[k] || i > nh || L[k, i] != oh[i])
        v(k, i, "R7", "the workflow header, from line 1 to jobs:, differs from the owned copy scripts/ci/release-header.yml here (byte for byte; GitHub applies it to the release job)")
    }
    # inb: inside the release block, which runs from "  release:" to the end of the file (R2).
    inb = 0
    for (i = 1; i <= N[k]; i++) {
      s = L[k, i]
      if (s == "  release:") {
        nrel++
        if (nrel > 1) v(k, i, "R2", "a second \"  release:\" job line in the workflows dir (the release job occurs once)")
        if (!inb) { inb = 1; bs = i; boff = 0; bdiff = 0; bperm = 0 }
      }
      if (inb) {
        boff++
        if (!bdiff && (boff > no || s != own[boff])) bdiff = i
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
      if (word(lo, "vars"))
        v(k, i, "R4", "the word vars (a repository variable could hold a token from outside the job permissions)")
      if (!inb && word(lo, "gh"))
        v(k, i, "R5", "the word gh outside the release job (only scripts/ci/release.sh in that job may call gh)")
      if (s ~ /\\x[0-9A-Fa-f][0-9A-Fa-f]/ || s ~ /\\[uU][0-9A-Fa-f][0-9A-Fa-f][0-9A-Fa-f][0-9A-Fa-f]/ ||
          s ~ /\\$/ || index(s, "\\0") || s ~ /\\(\r|\302\205|\342\200\250|\342\200\251)/)
        v(k, i, "R6", "a YAML escape (\\x + 2 hex, \\u or \\U + 4 hex, \\0, or a backslash before a line break) could hide secrets, vars, gh or permissions")
      if (index(s, "!!") || index(s, "!<") || index(s, "%TAG"))
        v(k, i, "R8", "a YAML tag (the raw text !!, !< or %TAG) could turn other text into secrets, vars, gh or permissions")
      # R9: the raw token upload-artifact, any case, comments included, once in the dir.
      n = count(lo, "upload-artifact")
      if (n > 0) {
        if (ntok == 0) { tk = k; ti = i }
        if (ntok < 2 && ntok + n >= 2)
          v(k, i, "R9", "a second upload-artifact in the workflows dir (the installer upload occurs once, as the owned copy scripts/ci/release-upload.yml)")
        ntok += n
      }
      # R10: the raw token secrets.telegram_, any case, comments included, as often as in the
      # owned send-step copy.
      n = count(lo, "secrets.telegram_")
      if (n > 0) {
        if (ttc == 0) { ttk = k; tti = i }
        if (ttc <= tco && ttc + n > tco)
          v(k, i, "R10", "secrets.TELEGRAM_ beyond the owned send step scripts/ci/telegram-send-step.yml (the Telegram secrets reach only that step)")
        ttc += n
      }
    }
    if (inb) {
      if (!bdiff && boff < no) bdiff = bs + boff
      if (bdiff) v(k, bdiff, "R2", "the release job, from \"  release:\" to the end of its file, differs from the owned copy scripts/ci/release-job.yml here (byte for byte; edit both together)")
    }
  }
  if (nrel == 0)
    for (k = 1; k <= nf; k++) {
      for (i = 1; i <= N[k]; i++) if (L[k, i] == "jobs:") break
      v(k, i, "R2", "no \"  release:\" job line in the workflows dir (the release job is the owned copy scripts/ci/release-job.yml)")
    }
  # R10: the lines aligned on the first token line (minus the first-token offset of the copy) are the
  # owned send-step copy, byte for byte.
  if (ttc == 0) {
    for (i = 1; i <= N[1]; i++) if (L[1, i] == "jobs:") break
    v(1, i, "R10", "no secrets.TELEGRAM_ in the workflows dir (the owned send step scripts/ci/telegram-send-step.yml is missing; the build sends the installer, #93)")
  } else
    for (m = 1; m <= nt; m++) {
      i = tti - tof + m
      if (i < 1 || i > N[ttk] || L[ttk, i] != ot[m]) {
        v(ttk, (i < 1 ? 1 : i), "R10", "the Telegram send step differs from the owned copy scripts/ci/telegram-send-step.yml here (byte for byte; push && (main || v* tag) && send == true; edit both together)")
        break
      }
    }
  # R9: the lines from the first token line on are the owned upload copy, byte for byte.
  if (ntok == 0) {
    for (i = 1; i <= N[1]; i++) if (L[1, i] == "jobs:") break
    v(1, i, "R9", "no upload-artifact in the workflows dir (the release job downloads the installer the owned step scripts/ci/release-upload.yml uploads on a v* tag)")
  } else
    for (m = 1; m <= nu; m++) {
      i = ti + m - 1
      if (i > N[tk] || L[tk, i] != ou[m]) {
        v(tk, i, "R9", "the installer upload step differs from the owned copy scripts/ci/release-upload.yml here (byte for byte; v* tag push only, retention-days: 1; edit both together)")
        break
      }
    }
  exit (nv > 0 ? 1 : 0)
}
' "${files[@]}")"
rc=$?
case "$rc" in
  0) ;;
  1)
    sed -n 's/^V //p' <<<"$report" >&2
    echo "release-guard: FAIL: a token other than the release job's could write contents, the release job is not the owned copy, the installer upload is not the owned tag-only step, or the Telegram secrets reach a step other than the owned send step (T-085, T-084, T-087, P-020)" >&2
    exit 1
    ;;
  *) cannot_run "awk failed (exit $rc) on $dir or an owned copy ($owned, $ownedh, $ownedu, $ownedt)" ;;
esac
grep -q '^V ' <<<"$report" && cannot_run "awk listed violations but exited 0"
echo "release-guard: ok: $dir: ${#files[@]} file(s); token read-only but in the release job, which is the owned copy; no other secret, permissions or gh; one installer upload, the owned tag-only step; the Telegram secrets only in the owned send step"
