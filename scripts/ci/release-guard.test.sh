#!/usr/bin/env bash
# T-085 (rca, class guard-model, F-003; decisions #120, P-020) after T-026 (decisions #102,
# #103): guard of the guard for scripts/ci/release-guard.sh. A GitHub Release is written only by
# the release job, and only that job's token can write repository contents; the guard decides on
# raw text and on a literal it owns, never on a model of YAML, shell or gh syntax
# (docs/tasks/T-085.md, Investigation section 6). Contract of the checker, pinned here:
#   Usage: scripts/ci/release-guard.sh <workflows-dir>
#   Every regular file directly in <workflows-dir> is read as raw text, comments included (no
#   extension filter: a .yaml file counts like a .yml one):
#     R1 each file has the word jobs (any case, bounded by characters outside [A-Za-z0-9_]) on
#        exactly one line, and that line is exactly `jobs:`; the two lines just before it are
#        exactly `permissions:` (column 0) and `  contents: read`, and the block has no third line
#        (a `  contents: write` after it is refused; a second `jobs:` line inside a quoted scalar,
#        or the real key spelled `"jobs":`, is refused: review 1 finding 2);
#     R2 the line `  release:` occurs exactly once in the dir, and the release-job block (that
#        line up to the end of its file) is byte-identical to the guard's owned copy (a trailing
#        space or a CR is a difference); the release job is its file's last job, and any text
#        after the owned copy, a comment included, is a difference (review 1 finding 1);
#     R3 the substring `permissions` (any case) occurs only in R1's block and once inside the
#        release block;
#     R4 every occurrence of `secrets` (any case) is exactly secrets.TELEGRAM_BOT_TOKEN or
#        secrets.TELEGRAM_CHAT_ID (not a longer name, not secrets['...']);
#     R5 the word gh (any case; a word = bounded by characters outside [A-Za-z0-9_], so GH_TOKEN,
#        gh_pages and ghcr do not count, $GH, ${GH:-gh} and gh.exe do) occurs nowhere outside the
#        release block;
#     R4 also: the word vars (any case, as for gh) occurs nowhere (a PAT kept in a repository
#        variable is a token from outside permissions:; review 1 finding 3);
#     R6 no YAML escape that spells a character or joins two lines: the raw text \x plus 2 hex
#        digits, \u or \U plus 4 hex digits, and a backslash right before a line break (LF, CR,
#        NEL U+0085, LS U+2028, PS U+2029) occur nowhere, comments included (an escape in a
#        double-quoted scalar would hide secrets, vars, gh or permissions from R3-R5; review 1
#        finding 3). No scalar reader: the raw bytes decide, in any scalar style. Also the raw
#        text \0 (a NUL to a YAML reader; bash drops NUL bytes, so "g\0h" runs gh: review 2
#        finding 2);
#     R7 in every file that holds a `  release:` line, the header (line 1 up to and including the
#        first line exactly `jobs:`) is byte-identical to the guard's owned copy
#        scripts/ci/release-header.yml (decision #121, review 2 finding 1: GitHub applies a
#        workflow-level env:, defaults: or on: to the release job; a comment byte is a
#        difference too). The listed line is the first header line that differs from the copy.
#        The owned copy missing, unreadable or not ending in the line `jobs:` is exit 3; so is a
#        copy with a line `jobs:` before its last line (review 3 finding 3);
#     R8 no YAML tag: the raw texts `!!`, `!<` and `%TAG` occur nowhere, in any file, comments
#        included (decision #122, review 3 finding 1: `!!binary <base64>` as a key or a run value,
#        the verbatim `!<tag:yaml.org,2002:binary>` and a `%TAG` directive decode to permissions
#        or a gh call that R3-R5 cannot see). A raw-token rule (P-020): no tag reader, so a lone
#        `!` (`!cancelled()`, `!=`, `[ ! -f ]`) and `% TAG` pass. Listed line: the occurrence.
#   Exit 0: ok, one line containing "ok:".
#   Exit 1: a violation; each listed as "<workflows-dir>/<file>:<line>: R<n>: <why>". <line> is
#           the first line of the block that differs from the owned copy for R2 (for a second
#           `  release:`, that line), the line of the occurrence for R3, R4, R5, R6 and R8 (for an
#           escaped line break, the line ending in the backslash).
#   Exit 2: usage error (no argument, or more than one).
#   Exit 3: cannot run (the dir is missing or not a directory, holds no regular file, or a file
#           has no top-level `jobs:` line); never a pass.
# The owned copy is the release block of fixtures/release-guard/ok-release/ci.yml: ok-release
# and the real .github/workflows must both pass, so the guard's copy, the fixtures and ci.yml
# carry the same text (the fixtures are generated from one block; change them together). The
# same holds for the header: every fixture file that holds the release job starts with the real
# ci.yml header, line 1 to `jobs:` (the owned copy release-header.yml); an edit of the ci.yml
# header edits the owned copy and every such fixture together.
# Cases: every dir scripts/ci/fixtures/release-guard/<case>/ (ok-* exit 0, v-* exit 1, each
# naming the rule that refuses it), the real .github/workflows (exit 0) and copies of it with the
# release job or the workflow-level block mutated, a gh step or a PAT secret added (exit 1 each),
# usage and cannot-run cases. Every fixture dir must appear in the table, so a case cannot be
# dropped silently.
# Usage: scripts/ci/release-guard.test.sh   (host bash, sed, grep, awk)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
guard=scripts/ci/release-guard.sh
fx=scripts/ci/fixtures/release-guard
real=.github/workflows
owned_src="$fx/ok-release/ci.yml"
[ -d "$fx" ] || { echo "release-guard.test: cannot run: $fx not found" >&2; exit 3; }
[ -f "$real/ci.yml" ] || { echo "release-guard.test: cannot run: $real/ci.yml not found" >&2; exit 3; }
[ -f "$owned_src" ] || { echo "release-guard.test: cannot run: $owned_src not found" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "release-guard.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

failed=0
passed=0
declare -A seen=()

# ere <text>: the text with ERE metacharacters escaped.
ere() { sed 's/[][\.*^$+?(){}|/]/\\&/g' <<<"$1"; }

# run <label> <expected exit> <ERE> [guard args...]: some output line must match the ERE.
run() {
  local label="$1" want="$2" re="$3" out got
  shift 3
  if [ ! -x "$guard" ]; then
    echo "FAIL $label: $guard not found or not executable (the tripwire does not exist)" >&2
    failed=$((failed + 1))
    return
  fi
  out="$("$guard" "$@" 2>&1)"
  got=$?
  if [ "$got" -ne "$want" ]; then
    echo "FAIL $label: exit $got, want $want" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
  elif ! grep -qiE -- "$re" <<<"$out"; then
    echo "FAIL $label: exit $got as wanted, but no output line matches: $re" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
  else
    echo "ok   $label: exit $got"
    passed=$((passed + 1))
  fi
}

# line_of <file> <fixed text>: the number of the first line containing the text (empty if none).
line_of() { grep -n -m 1 -F -- "$2" "$1" | cut -d: -f1; }

# block <file>: the release block's lines (from the line "  release:" to the end of the file),
# bytes kept (a CR stays in its line).
block() { awk '/^  release:$/ { inb = 1 } inb { print }' "$1"; }

# first_diff <file>: the file's line number of the first line of its release block that differs
# from the owned copy (empty when the blocks are equal).
first_diff() {
  local f="$1" start i n
  local -a a b
  start="$(grep -n -m 1 -x '  release:' "$f" | cut -d: -f1)"
  [ -n "$start" ] || return 0
  mapfile -t a < <(block "$owned_src")
  mapfile -t b < <(block "$f")
  n=${#a[@]}
  [ "${#b[@]}" -gt "$n" ] && n=${#b[@]}
  for ((i = 0; i < n; i++)); do
    if [ "${a[i]-<none>}" != "${b[i]-<none>}" ]; then
      echo $((start + i))
      return 0
    fi
  done
}

# head_lines <file>: the file's header, line 1 up to and including its first line exactly `jobs:`
# (bytes kept).
head_lines() { awk '{ print } $0 == "jobs:" { exit }' "$1"; }

# first_hdiff <file>: the file's line number of the first header line that differs from the
# header of ok-release (the owned header copy; empty when the headers are equal).
first_hdiff() {
  local f="$1" i n
  local -a a b
  mapfile -t a < <(head_lines "$owned_src")
  mapfile -t b < <(head_lines "$f")
  n=${#a[@]}
  [ "${#b[@]}" -gt "$n" ] && n=${#b[@]}
  for ((i = 0; i < n; i++)); do
    if [ "${a[i]-<none>}" != "${b[i]-<none>}" ]; then
      echo $((i + 1))
      return 0
    fi
  done
}

# at <path> <line> <rule>: the ERE of a listing line "<path>:<line>: R<n>"; line "*" = any.
at() {
  local l="$2"
  [ "$l" = "*" ] && l='[0-9]+' || l="$(ere "$l")"
  echo "^$(ere "$1"):$l: $3([^0-9]|\$)"
}

# check <case> <exit> [<rule> <where> [<file>]]: the case dir as the workflows dir. <where> is
# "@text" (the first line of <file> containing text), "diff" (first_diff of <file>), "hdiff"
# (first_hdiff of <file>), "*" (any
# line) or a number; <file> defaults to ci.yml.
check() {
  local c="$1" want="$2" rule="${3:-}" where="${4:-}" file="${5:-ci.yml}" d f l
  d="$fx/$c"
  f="$d/$file"
  seen["$c"]=1
  if [ "$want" = 0 ]; then
    run "$c" 0 'ok:' "$d"
    return
  fi
  case "$where" in
    @*) l="$(line_of "$f" "${where#@}")" ;;
    diff) l="$(first_diff "$f")" ;;
    hdiff) l="$(first_hdiff "$f")" ;;
    *) l="$where" ;;
  esac
  if [ -z "$l" ]; then
    echo "FAIL $c: cannot locate the expected line ($where in $f; fix the test)" >&2
    failed=$((failed + 1))
    return
  fi
  run "$c [$rule]" "$want" "$(at "$f" "$l" "$rule")" "$d"
}

# ---- allowed --------------------------------------------------------------------------------
check ok-release          0   # the target shape: workflow-level contents: read, the release job = the owned copy, Telegram secrets
check ok-lookalikes       0   # GH_TOKEN, GH_PROMPT_DISABLED, github, ghcr.io, gh_pages, high_gh, "a secret": none is the word gh or secrets

# ---- R2: the release job is the owned literal, byte for byte ---------------------------------
# T-026 job-level fixtures (old rules 1-3), now the same edit on the new block's lines.
check v-no-job-if                  1 R2 diff   # the release job runs on every ref, wip/** included
check v-no-tag-operand             1 R2 diff   # push on any ref
check v-no-event-name              1 R2 diff
check v-top-level-or               1 R2 diff
check v-tag-operand-widened        1 R2 diff   # refs/tags/ instead of refs/tags/v (one character)
check v-job-failure                1 R2 diff
check v-job-success-or             1 R2 diff
check v-job-or-success             1 R2 diff
check v-top-level-or-last          1 R2 diff
check v-job-or-in-parens           1 R2 diff
check v-job-or-after-quoted-paren  1 R2 diff
check v-job-or-two-groups          1 R2 diff
check v-job-or-quoted-paren-groups 1 R2 diff
check v-job-always                 1 R2 diff   # publishes after a red gate or windows job
check v-job-not-cancelled          1 R2 diff
check v-job-continue-on-error      1 R2 diff
check v-needs-gate-only            1 R2 diff   # publishes without the Windows build and smoke
check v-needs-windows-only         1 R2 diff
check v-no-needs                   1 R2 diff
check v-write-all                  1 R2 diff   # permissions: write-all in the release job (once inside the block, so R3 holds; R2 refuses)
# T-026 check-step fixtures (old rules 4-5, validation 1 M6, review 2, validation 2), now on the
# one release.sh step: the step's exit status swallowed, skipped or without the token.
check v-check-background           1 R2 diff   # ... &
check v-check-bang                 1 R2 diff   # ! bash ...
check v-check-block-or-true        1 R2 diff   # || continued on the next line
check v-check-continue-on-error    1 R2 diff
check v-check-dq-scalar-newline    1 R2 diff   # "...\nexit 0", shell without -e
check v-check-echo-only            1 R2 diff   # echo scripts/ci/release.sh ...
check v-check-if                   1 R2 diff   # a skipped step counts as success
check v-check-if-wrapped           1 R2 diff   # if ...; then ...; fi
check v-check-no-token             1 R2 diff   # the release-exists check would be skipped
check v-check-no-token-job-env     1 R2 diff
check v-check-or-colon             1 R2 diff
check v-check-or-exit-0            1 R2 diff
check v-check-or-true              1 R2 diff
check v-check-other-script         1 R2 diff   # release.sh.orig
check v-check-pipe-default-shell   1 R2 diff   # ... | tee, no shell: bash
check v-check-quoted-hash          1 R2 diff   # "dist then #" || true
check v-check-semicolon-true       1 R2 diff
check v-check-set-plus-e           1 R2 diff
check v-check-token-in-comment     1 R2 diff   # env: # GH_TOKEN: ...
check v-check-token-name-suffix    1 R2 diff   # NOT_GH_TOKEN
check v-check-trailing-no-errexit  1 R2 diff
check v-check-trap-exit-0          1 R2 diff
check v-publish-always             1 R2 diff   # if: always() on the step
check v-no-check                   1 R2 diff   # gh release create directly, no release.sh (gh inside the block: R2, not R5)
check v-publish-before-check       1 R2 diff   # gh release create before release.sh
check v-third-party-action         1 R2 diff   # softprops/action-gh-release appended to the job
check v-r4-d-token-in-quoted-flow-value 1 R2 diff   # review 4 probe D: env flow mapping, "x, GH_TOKEN: none", no token
# Former ok-* fixtures: the release job is no longer the owned literal.
check v-no-release-job             1 R2 '*'    # was ok-no-release: no release job at all
check v-release-reordered          1 R2 diff   # was ok-reordered-job-token-block-run
check v-release-check-block-run    1 R2 diff   # was ok-check-block-run
# Byte identity, not a reading of the block.
check v-release-one-char           1 R2 diff   # runs-on: ubuntu-24.01
check v-release-trailing-space     1 R2 diff   # "runs-on: ubuntu-24.04 "
check v-release-crlf-line          1 R2 diff   # one line ends in CR LF
check v-release-step-appended      1 R2 diff   # a step after the owned copy's last line, at EOF
check v-release-twice              1 R2 '@  release:' other.yml   # a second release block in another workflow file
# Review 1 finding 1 (probes P1, P1b, P1c): the block runs to the end of its file; a comment does
# not end it (the guard has no comment reader, P-020).
check v-release-job-after                 1 R2 diff   # was ok-release-not-last: a job after the release job
check v-release-comment-col0-then-step    1 R2 diff   # P1: "# end", then an always() step POSTing a release with the job token
check v-release-comment-indent2-then-step 1 R2 diff   # P1b: the same, the comment indented 2
check v-release-comment-then-job-env      1 R2 diff   # P1c: "#", then job-level env BASH_ENV: ./evil.sh

# ---- R5: the word gh outside the release block -----------------------------------------------
check v-unguarded-upload                   1 R5 '@gh release'
check v-unguarded-edit                     1 R5 '@gh release'
check v-unguarded-delete                   1 R5 '@gh release'
check v-unguarded-delete-asset             1 R5 '@gh release'
check v-unguarded-upload-after-cd          1 R5 '@gh release'   # cd ... && gh release upload
check v-unguarded-upload-repo-flag-first   1 R5 '@gh --repo'    # gh --repo R release upload
check v-unguarded-upload-repo-flag-between 1 R5 '@gh release'   # gh release --repo R upload
check v-gh-read-in-windows                 1 R5 '@gh release'   # was the probe step of ok-release: a read is still the word gh
check v-gh-in-comment                      1 R5 '@# gh release' # comments are raw text too
# Review 4 probes (docs/tasks/T-026.reviews/4.md), each passed by the 6e7e7a8 model.
check v-r4-a-folded-gh-in-windows          1 R5 '@gh release'   # A: run: > folds "gh release" / "upload ..."
check v-r4-b-param-expansion-gh            1 R5 '@${GH:-gh}'    # B: ${GH:-gh} release upload
check v-r4-c-plain-multiline-gh            1 R5 '@gh release'   # C: plain multi-line scalar
check v-r4-e-gh-alias-var                  1 R5 '@GH=gh'        # E: GH=gh; $GH release upload
check v-other-file-gh                      1 R5 '@gh release' publish.yaml   # another workflow file, .yaml

# ---- R4: no secret but the two Telegram ones ------------------------------------------------
check v-r4-a-plus-pat          1 R4 '@secrets.RELEASE_PAT'      # Investigation section 2, A-plus-PAT: the open hole at 6e7e7a8 (a token from outside permissions:)
check v-pat-curl               1 R4 '@secrets.RELEASE_PAT'      # the PAT with curl, no gh word: only the capability rule sees it
check v-secrets-index          1 R4 "@secrets['RELEASE_PAT']"
check v-secrets-allowed-prefix 1 R4 '@secrets.TELEGRAM_CHAT_ID_2' # an allowed name as a prefix
check v-secrets-uppercase      1 R4 '@SECRETS.RELEASE_PAT'      # expressions ignore case
# Review 1 finding 3 (probe P5): a token kept in a repository variable.
check v-vars-pat               1 R4 '@vars.RELEASE_PAT'         # P5: ${{ vars.RELEASE_PAT }}
check v-vars-index             1 R4 "@VARS['RELEASE_PAT']"      # the word vars, not only "vars."

# ---- R6: no YAML escape that spells a character or joins two lines ----------------------------
# Review 1 finding 3 (probes P3, P4): each reads, to a YAML reader, as text R3-R5 refuse.
check v-escape-x-secrets               1 R6 '@s\x65crets'      # P3: "${{ s\x65crets.RELEASE_PAT }}"
check v-escape-u-secrets               1 R6 '@s\u0065crets'    # \u, 4 hex digits
check v-escape-U-gh                    1 R6 '@\U00000067h'     # \U, 8 hex digits: gh release upload
check v-escape-x-gh                    1 R6 '@\x67h release'   # P4: "\x67h release upload v1 x"
check v-escape-line-break-secrets      1 R6 '@secr\'           # P4: "${{ secr\<LF>  ets.RELEASE_PAT }}"
check v-escape-line-break-crlf-secrets 1 R6 '@secr\'           # the same, the line break is CR LF
check v-escape-line-break-ls-secrets   1 R6 '@secr\'           # the same, the line break is U+2028 (a YAML 1.1 line break)
check v-escape-permissions-key         1 R6 '@permi\x73sions'  # "permi\x73sions": a job-level contents: write (was Not caught)
# Review 2 finding 2 (probe Q9-zero): \0 is a NUL, and bash drops NUL bytes from its input.
check v-escape-zero-gh                 1 R6 '@g\0h release'    # "g\0h release upload v1 x" runs gh
check v-escape-zero-secrets            1 R6 '@s\0ecrets'       # "${{ s\0ecrets.RELEASE_PAT }}"

# ---- R7: the header of the release job's file is the owned literal, byte for byte -------------
# Review 2 finding 1, decision #121: GitHub applies workflow-level env:, defaults: and on: to the
# release job, so line 1 to `jobs:` of its file must equal scripts/ci/release-header.yml.
check v-header-env-bash-env      1 R7 hdiff   # Q1: env: BASH_ENV: "$(curl ... $GH_TOKEN ...)" (fake URL, no token): runs before release.sh with the write token
check v-header-defaults-workdir  1 R7 hdiff   # Q2: defaults: run: working-directory: another release.sh runs
check v-header-env-node-options  1 R7 hdiff   # Q3: env: NODE_OPTIONS: --require (code in download-artifact, which gets github.token)
check v-header-comment-byte      1 R7 hdiff   # one comment byte (A -> a): no comment reader (P-020)
check v-header-on-trigger        1 R7 hdiff   # on: gains workflow_dispatch

# ---- R8: no YAML tag (raw `!!`, `!<`, `%TAG`), comments included ---------------------------
# Review 3 finding 1, decision #122 (probes C4c-C4e, /root/.claude/jobs/97682091/tmp/t085-r3review/
# s/probe4.py): in a second file (no release job, so R7 does not apply), a tag decodes base64 into
# a key `permissions` (contents: write) and a run value `gh release create v9.9.9 x --repo
# example/fake` (fake repo); PyYAML 6.0.1 and npm yaml 2.8.2 decode both.
check v-tag-binary-other-file    1 R8 '@!!binary cGVybWlzc2lvbnM=' other.yml   # C4c: the key line
check v-tag-binary-other-file    1 R8 '@run: !!binary' other.yml               # C4c: the run line, listed too
check v-tag-verbatim-other-file  1 R8 '@!<tag:yaml.org,2002:binary> cGVy' other.yml   # C4d: verbatim tag
check v-tag-directive-other-file 1 R8 '@%TAG !e!' other.yml                    # C4e: %TAG directive, then !e!ary (no !! and no !<)
check v-tag-in-comment           1 R8 '@# a !!binary'                          # a raw rule: a comment is text (no comment reader)
check ok-tag-lookalikes          0   # !cancelled(), !=, [ ! -f ], "100% TAG ! < ! !x": not a tag token

# ---- R3: permissions only in the workflow-level block and once in the release block ----------
check v-write-other-job        1 R3 '@    permissions:'          # windows job gets contents: write
check v-write-all-windows      1 R3 '@permissions: write-all'
check v-permissions-in-comment 1 R3 '@permissions come'        # comments are raw text too
check v-other-file-write-all   1 R3 '@permissions: write-all' other.yml

# ---- R1: the workflow-level block, in every file ---------------------------------------------
# Review 1 finding 2 (probe P11): the block and a jobs: line inside a double-quoted scalar.
check v-r1-jobs-in-quoted-scalar           1 R1 '*'   # P11: two lines `jobs:`, the first inside x-note: "..."
check v-r1-jobs-key-quoted                 1 R1 '*'   # one line `jobs:` (in the scalar), the real key spelled "jobs":
check v-workflow-write                     1 R1 '*'   # permissions: / contents: write at workflow level
check v-no-workflow-permissions            1 R1 '*'   # jobs would take the repository default
check v-workflow-permissions-dup-contents  1 R1 '*'   # contents: read, then contents: write in the same block
check v-other-file-no-workflow-permissions 1 R1 '*' other.yml

# ---- the real workflows dir -------------------------------------------------------------------
run "real $real" 0 'ok:' "$real"

# Copies of the real dir, ci.yml mutated. mutate <name> <awk body>: inrel is 1 on the release
# block's lines, inwin on the windows job's.
scope='/^  [A-Za-z0-9_-]+:[ \t]*$/ { inrel = ($0 ~ /^  release:[ \t]*$/); inwin = ($0 ~ /^  windows:[ \t]*$/) } /^[A-Za-z]/ { inrel = 0; inwin = 0 }'
mutate() {
  mkdir -p "$tmp/$1"
  cp -p "$real"/* "$tmp/$1/"
  awk "$scope $2" "$real/ci.yml" > "$tmp/$1/ci.yml"
  if cmp -s "$real/ci.yml" "$tmp/$1/ci.yml"; then
    echo "FAIL real $1: the mutation did not apply (update the test with the workflow)" >&2
    failed=$((failed + 1))
    return 1
  fi
}
# real_r2 <name> <label>: the mutated copy is refused by R2 at its first differing line.
real_r2() {
  local l
  l="$(first_diff "$tmp/$1/ci.yml")"
  run "$2" 1 "$(at "$tmp/$1/ci.yml" "${l:-0}" R2)" "$tmp/$1"
}
mutate no-if 'inrel && /^    if:/ { next } { print }' &&
  real_r2 no-if "real without the release job's if:"
mutate always 'inrel && /^    if: \$\{\{ / { sub(/^    if: \$\{\{ /, "    if: ${{ always() \\&\\& ") } { print }' &&
  real_r2 always "real with always() in the release job's if:"
mutate needs-gate 'inrel && /^    needs:/ { print "    needs: gate"; next } { print }' &&
  real_r2 needs-gate "real with the release job needing gate only"
mutate or-true 'inrel && /^        run: bash scripts\/ci\/release\.sh / { $0 = $0 " || true" } { print }' &&
  real_r2 or-true "real with || true after release.sh"
mutate semicolon-true 'inrel && /^        run: bash scripts\/ci\/release\.sh / { $0 = $0 "; true" } { print }' &&
  real_r2 semicolon-true "real with ; true after release.sh"
mutate background 'inrel && /^        run: bash scripts\/ci\/release\.sh / { $0 = $0 " &" } { print }' &&
  real_r2 background "real with & after release.sh"
mutate wf-write '!inrel && !inwin && /^  contents: read$/ { $0 = "  contents: write" } { print }' &&
  run "real with contents: write at workflow level" 1 "$(at "$tmp/wf-write/ci.yml" '*' R1)" "$tmp/wf-write" &&
  run "real with contents: write at workflow level [R7: the header runs to jobs:]" 1 \
    "$(at "$tmp/wf-write/ci.yml" "$(first_hdiff "$tmp/wf-write/ci.yml")" R7)" "$tmp/wf-write"
# Review 2 finding 1 on the real file: a workflow-level env: above the R1 block (fake path).
mutate wf-env '/^permissions:$/ && !d { print "env:"; print "  BASH_ENV: ./fake-env.sh"; d = 1 } { print }' &&
  run "real with a workflow-level env: (BASH_ENV)" 1 \
    "$(at "$tmp/wf-env/ci.yml" "$(line_of "$tmp/wf-env/ci.yml" 'env:')" R7)" "$tmp/wf-env"
mutate win-gh 'inwin && /^    steps:$/ && !d { print; print "      - run: gh release upload v0.1.0 x.exe"; d = 1; next } { print }' &&
  run "real with gh release upload in the windows job" 1 \
    "$(at "$tmp/win-gh/ci.yml" "$(line_of "$tmp/win-gh/ci.yml" 'gh release upload v0.1.0')" R5)" "$tmp/win-gh"
mutate comment-step '{ print } END { print "# end of the release job"; print "      - name: Publish anyway"; print "        if: always()"; print "        run: echo publish" }' &&
  real_r2 comment-step "real with a comment and a step appended after the release job"
mutate win-escape 'inwin && /^    steps:$/ && !d { print; print "      - run: \"\\x67h release upload v0.1.0 x.exe\""; d = 1; next } { print }' &&
  run "real with an escaped gh in the windows job" 1 \
    "$(at "$tmp/win-escape/ci.yml" "$(line_of "$tmp/win-escape/ci.yml" '\x67h release')" R6)" "$tmp/win-escape"
mutate win-pat 'inwin && /^    steps:$/ && !d { print; print "      - env:"; print "          TOKEN: ${{ secrets.RELEASE_PAT }}"; print "        run: echo pat"; d = 1; next } { print }' &&
  run "real with a PAT secret in the windows job" 1 \
    "$(at "$tmp/win-pat/ci.yml" "$(line_of "$tmp/win-pat/ci.yml" 'secrets.RELEASE_PAT')" R4)" "$tmp/win-pat"
# Review 3 finding 1 on the real file: a tag on a windows step's run value (decision #122).
mutate win-tag 'inwin && /^    steps:$/ && !d { print; print "      - run: !!str echo tagged"; d = 1; next } { print }' &&
  run "real with a YAML tag in the windows job" 1 \
    "$(at "$tmp/win-tag/ci.yml" "$(line_of "$tmp/win-tag/ci.yml" '!!str echo tagged')" R8)" "$tmp/win-tag"

# ---- usage and cannot run -------------------------------------------------------------------
run "usage: no argument"           2 'usage'
run "usage: two arguments"         2 'usage' "$real" "$real"
run "cannot run: missing dir"      3 'cannot run' "$tmp/absent"
run "cannot run: a file, not a dir" 3 'cannot run' "$real/ci.yml"
mkdir -p "$tmp/empty"
run "cannot run: no workflow file" 3 'cannot run' "$tmp/empty"
# A file with no top-level jobs: is not a workflow: never a pass (validation 2 sweep).
mkdir -p "$tmp/no-jobs"
printf 'on:\n  push:\npermissions:\n  contents: read\n' > "$tmp/no-jobs/ci.yml"
run "cannot run: no jobs:"         3 'no top-level jobs' "$tmp/no-jobs"
# The owned header copy (R7) is a guard input like release-job.yml: missing, or not ending in the
# line `jobs:` (a truncated copy would compare only part of the header), is never a pass. The
# guard is copied next to its release-job.yml; the header copy is left out or truncated.
owned_guard() {
  mkdir -p "$tmp/$1"
  cp -p "$guard" scripts/ci/release-job.yml "$tmp/$1/" 2>/dev/null
}
real_guard="$guard"
if owned_guard g-header-missing; then
  guard="$tmp/g-header-missing/release-guard.sh"
  run "cannot run: owned header copy missing" 3 'cannot run' "$real"
fi
guard="$real_guard"
if owned_guard g-header-no-jobs; then
  head_lines "$owned_src" | sed '$d' > "$tmp/g-header-no-jobs/release-header.yml"
  guard="$tmp/g-header-no-jobs/release-guard.sh"
  run "cannot run: owned header copy does not end in jobs:" 3 'cannot run' "$real"
fi
guard="$real_guard"
# Review 3 finding 3: a copy with a `jobs:` line before its last line would let R7 stop comparing
# at that line; never a pass (characterization: the guard already refuses it).
if owned_guard g-header-jobs-mid; then
  head_lines "$owned_src" | sed '1a jobs:' > "$tmp/g-header-jobs-mid/release-header.yml"
  guard="$tmp/g-header-jobs-mid/release-guard.sh"
  run "cannot run: owned header copy has jobs: before its last line" 3 'cannot run' "$real"
fi
guard="$real_guard"

# Every committed fixture dir is in the table.
for d in "$fx"/*/; do
  c="$(basename "$d")"
  if [ -z "${seen[$c]:-}" ]; then
    echo "FAIL $c: fixture dir has no expected exit code in $0" >&2
    failed=$((failed + 1))
  fi
done

if [ "$failed" -gt 0 ]; then
  echo "release-guard.test: FAIL: $failed case(s) differ, $passed as expected (T-085)" >&2
  exit 1
fi
echo "release-guard.test: ok: $passed cases as expected"
