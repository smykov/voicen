#!/usr/bin/env bash
# T-085 (rca, class guard-model, F-003; decisions #120, P-020) after T-026 (decisions #102,
# #103): guard of the guard for scripts/ci/release-guard.sh. A GitHub Release is written only by
# the release job, and only that job's token can write repository contents; the guard decides on
# raw text and on a literal it owns, never on a model of YAML, shell or gh syntax
# (docs/tasks/T-085.md, Investigation section 6). Contract of the checker, pinned here:
#   Usage: scripts/ci/release-guard.sh <workflows-dir>
#   Every regular file directly in <workflows-dir> is read as raw text, comments included (no
#   extension filter: a .yaml file counts like a .yml one; a dotfile counts too):
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
#     R9 (T-084, decision #119) the raw token `upload-artifact` (any case, comments included, a
#        sub-path or a fork name too) occurs exactly once in the dir, and the lines starting at
#        that line are byte-identical to the owned copy scripts/ci/release-upload.yml: the
#        installer upload step with the release job's own if: (v* tag push only) and
#        retention-days: 1, ended by the first line of the next step, so no key can be appended
#        to the step. A raw-token rule plus an owned literal (P-020): no step reader. Listed line:
#        for zero occurrences any line of the file; for two or more, the second occurrence (in
#        dir order: files sorted, then line order); for a literal difference, the first line
#        that differs from the copy. The copy missing, unreadable or not starting with the line
#        `      - uses: actions/upload-artifact@v4` is exit 3, its message naming the copy.
#     R10 (T-087, decisions #124, P-020; replaces telegram-guard.sh) the raw token
#        `secrets.TELEGRAM_` (any case, comments included) occurs in the dir (files in glob order,
#        then line order) exactly as often as in the owned copy scripts/ci/telegram-send-step.yml
#        (the Telegram send step from its name line to its run: line, ended by the next step's
#        first line `      - name: SHA-256 of the installer`, so no key can be appended), and the
#        lines aligned on the first token line (that line minus the copy's first-token offset) are
#        byte-identical to the copy; and, going up from the copy's first line and skipping lines
#        of spaces only, the first line with fewer than 6 leading spaces is exactly `    steps:`,
#        the next with fewer than 4 is exactly `  windows:` and the next with fewer than 2 is
#        exactly `jobs:` (review 1 finding 1: the literal held by a block scalar, e.g. a job's
#        `env: X: |`, cannot meet this). So the only step GitHub hands the Telegram secrets to is
#        the reviewed send step of the windows job with its reviewed if: (push && (main || v* tag)
#        && send == 'true'). A raw-token rule, an owned literal and a leading-space count: no YAML
#        or expression reader. Listed line: the token occurrence where the dir count first exceeds
#        the copy's count; the first aligned line that differs from the copy (line 1 when the
#        alignment starts before line 1); the first line met going up that is not the expected
#        one (the copy's first line when none is met); with no token, the first file's `jobs:`
#        line. The copy
#        missing, empty, holding no token or not starting with the send step's name line is
#        exit 3, its message naming the copy.
#     R7 (T-088, widened) applies to ci.yml as well as to every file holding `  release:`: ci.yml
#        starts with the owned header even when the release job sits in another file (probe W4:
#        the job moved to release.yml with the owned header, ci.yml's trigger cut to [main]).
#     R11 (T-088, P-020; replaces ci-wip.sh and setup-node-cache.sh) in every regular file of the
#        dir, comments included, every line holding the word uses (any case, bounded as for R1)
#        or one of the action-name tokens actions/cache, setup-node, rust-cache, cache/save,
#        pnpm/action-setup (any case, substrings) is either byte-identical to a line of the owned
#        list scripts/ci/action-lines.txt (the never-saving uses lines) or lies inside a
#        byte-exact copy of one of the owned step fragments scripts/ci/cache-steps/*.yml (each
#        from its `      - ` line through the next step's first line or the next job's key
#        line, so no key can be appended; that last line only pins the end: it is not inside
#        the fragment, so the step it starts needs its own owned line or fragment). Listed line:
#        every such token line outside them.
#        The guard checks its own fragments before it reads the dir: a fragment not starting with
#        `      - ` or not ending on a `      - ` line or a job key line (`  <name>:`), a
#        setup-node fragment without `          package-manager-cache: false` or with the raw
#        cache on any other line, a rust-cache fragment without
#        `          save-if: ${{ github.ref == 'refs/heads/main' }}` or with a second save-if line,
#        a cache/save fragment without an if: line starting
#        `        if: ${{ github.ref == 'refs/heads/main' && `, with a second if: line or holding
#        ||, a pnpm/action-setup fragment holding cache, is exit 3 naming the fragment
#        (even when the workflow carries the same edit, so the copy and ci.yml cannot be
#        loosened together); the list or the fragment dir missing or empty is exit 3 too.
#     R12 (T-088) after a file's `jobs:` line, every line that is not empty or spaces only starts
#        with a space (probe W3: a column-0 `on:` between two jobs turns the release job into a
#        key of it; a column-0 comment counts too, there is no comment reader). Listed line:
#        the column-0 line.
#     R13 (T-088) no YAML merge key: the raw text << not followed by <, a letter, _, -, ' or "
#        (bash here-docs <<EOF, <<-EOF, <<'EOF', <<"EOF", <<_X and here-strings <<< pass).
#        Probe M2: an anchored step list starting with a byte-exact setup-node fragment, merged
#        into another job's step with cache: pnpm. Listed line: the occurrence.
#   Exit 0: ok, one line containing "ok:".
#   Exit 1: a violation; each listed as "<workflows-dir>/<file>:<line>: R<n>: <why>". <line> is
#           the first line of the block that differs from the owned copy for R2 (for a second
#           `  release:`, that line), the line of the occurrence for R3, R4, R5, R6 and R8 (for an
#           escaped line break, the line ending in the backslash).
#   Exit 2: usage error (no argument, or more than one).
#   Exit 3: cannot run (the dir is missing or not a directory, holds no regular file, or a file
#           has no top-level `jobs:` line; awk failed); never a pass.
# The owned copy is the release block of fixtures/release-guard/ok-release/ci.yml: ok-release
# and the real .github/workflows must both pass, so the guard's copy, the fixtures and ci.yml
# carry the same text (the fixtures are generated from one block; change them together). The
# same holds for the header: every fixture file that holds the release job starts with the real
# ci.yml header, line 1 to `jobs:` (the owned copy release-header.yml); an edit of the ci.yml
# header edits the owned copy and every such fixture together. The same holds for the upload
# step (R9): the ok-* fixtures and the real ci.yml carry scripts/ci/release-upload.yml verbatim,
# and the v-upload-* fixtures are ok-release with one edit each (each passes the guard without
# R9, so only R9 can refuse it).
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
upl=scripts/ci/release-upload.yml
tgs=scripts/ci/telegram-send-step.yml
tgs_name='      - name: Send the installer to Telegram (main and v* tags only)'
[ -d "$fx" ] || { echo "release-guard.test: cannot run: $fx not found" >&2; exit 3; }
[ -f "$real/ci.yml" ] || { echo "release-guard.test: cannot run: $real/ci.yml not found" >&2; exit 3; }
[ -f "$owned_src" ] || { echo "release-guard.test: cannot run: $owned_src not found" >&2; exit 3; }
[ -f "$upl" ] || { echo "release-guard.test: cannot run: $upl not found" >&2; exit 3; }
[ -f "$tgs" ] || { echo "release-guard.test: cannot run: $tgs not found" >&2; exit 3; }
# T-088: the owned never-saving uses lines and the owned cache / setup-node step fragments (R11).
alines=scripts/ci/action-lines.txt
csd=scripts/ci/cache-steps
[ -f "$alines" ] || { echo "release-guard.test: cannot run: $alines not found" >&2; exit 3; }
[ -d "$csd" ] || { echo "release-guard.test: cannot run: $csd not found" >&2; exit 3; }
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

# tok_lines <file>: the numbers of the lines holding the token upload-artifact, in any case.
tok_lines() { grep -n -i -F -- 'upload-artifact' "$1" | cut -d: -f1; }

# first_udiff <file>: the file's line number of the first line, from its first token line on,
# that differs from the owned upload copy (empty when equal; with no token line, empty).
first_udiff() {
  local f="$1" start i n
  local -a a b
  start="$(tok_lines "$f" | head -n 1)"
  [ -n "$start" ] || return 0
  mapfile -t a < "$upl"
  mapfile -t b < <(tail -n "+$start" "$f")
  n=${#a[@]}
  for ((i = 0; i < n; i++)); do
    if [ "${a[i]}" != "${b[i]-<none>}" ]; then
      echo $((start + i))
      return 0
    fi
  done
}

# ttok_n <file>: the number of occurrences of the Telegram token secrets.telegram_ (any case) in
# the file, comments included.
ttok_n() { grep -i -o -F -- 'secrets.telegram_' "$1" | wc -l | tr -d ' '; }

# ttok_lines <file>: the numbers of the lines holding the Telegram token, in any case.
ttok_lines() { grep -n -i -F -- 'secrets.telegram_' "$1" | cut -d: -f1; }

# tgs_off: the copy's first token line minus 1 (the token line's offset inside the literal).
tgs_off() { echo $(( $(ttok_lines "$tgs" | head -n 1) - 1 )); }

# first_tdiff <file>: the file's line number of the first line, from (its first token line minus
# the copy's offset) on, that differs from the owned send-step copy (empty when equal or no token).
first_tdiff() {
  local f="$1" start i n
  local -a a b
  start="$(ttok_lines "$f" | head -n 1)"
  [ -n "$start" ] || return 0
  start=$((start - $(tgs_off)))
  mapfile -t a < "$tgs"
  if [ "$start" -lt 1 ]; then echo 1; return 0; fi
  mapfile -t b < <(tail -n "+$start" "$f")
  n=${#a[@]}
  for ((i = 0; i < n; i++)); do
    if [ "${a[i]}" != "${b[i]-<none>}" ]; then
      echo $((start + i))
      return 0
    fi
  done
}

# texcess <file>: the line of <file> where the Telegram token count of its dir (files in glob
# order, dotfiles included, then line order) first exceeds the owned copy's count (empty if it
# never does, or does in another file).
texcess() {
  local f="$1" d g k=0 max i c
  d="$(dirname "$f")"
  max="$(ttok_n "$tgs")"
  shopt -s nullglob dotglob
  for g in "$d"/*; do
    [ -f "$g" ] || continue
    i=0
    while IFS= read -r line || [ -n "$line" ]; do
      i=$((i + 1))
      c="$(grep -i -o -F -- 'secrets.telegram_' <<<"$line" | wc -l | tr -d ' ')"
      k=$((k + c))
      if [ "$k" -gt "$max" ]; then
        shopt -u nullglob dotglob
        [ "$g" = "$f" ] && echo "$i"
        return 0
      fi
    done < "$g"
  done
  shopt -u nullglob dotglob
}

# at <path> <line> <rule>: the ERE of a listing line "<path>:<line>: R<n>"; line "*" = any.
at() {
  local l="$2"
  [ "$l" = "*" ] && l='[0-9]+' || l="$(ere "$l")"
  echo "^$(ere "$1"):$l: $3([^0-9]|\$)"
}

# check <case> <exit> [<rule> <where> [<file>]]: the case dir as the workflows dir. <where> is
# "@text" (the first line of <file> containing text), "diff" (first_diff of <file>), "hdiff"
# (first_hdiff of <file>), "udiff" (first_udiff of <file>), "tok2" (the second token line of
# <file>), "tdiff" (first_tdiff of <file>), "texcess" (texcess of <file>), "jobs" (the line of
# <file> that is exactly jobs:), "*" (any line) or a number; <file> defaults to ci.yml.
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
    udiff) l="$(first_udiff "$f")" ;;
    tok2) l="$(tok_lines "$f" | sed -n 2p)" ;;
    tdiff) l="$(first_tdiff "$f")" ;;
    texcess) l="$(texcess "$f")" ;;
    jobs) l="$(grep -n -m 1 -x 'jobs:' "$f" | cut -d: -f1)" ;;
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
# Validation 1 survivors (docs/tasks/T-085.validation/1.md): R5 is matched in any case (S11), and
# every regular file is read, a dotfile included (S12; the guard's dotglob).
check v-gh-uppercase-pwsh                  1 R5 '@GH release'   # S11: pwsh resolves GH as gh (word matched in any case)
check v-dotfile-workflow                   1 R5 '@gh release' .hidden.yml        # S12: a dotfile in the workflows dir is scanned
check v-dotfile-workflow                   1 R3 '@permissions: write-all' .hidden.yml

# ---- R4: no secret but the two Telegram ones ------------------------------------------------
check v-r4-a-plus-pat          1 R4 '@secrets.RELEASE_PAT'      # Investigation section 2, A-plus-PAT: the open hole at 6e7e7a8 (a token from outside permissions:)
check v-pat-curl               1 R4 '@secrets.RELEASE_PAT'      # the PAT with curl, no gh word: only the capability rule sees it
check v-secrets-index          1 R4 "@secrets['RELEASE_PAT']"
check v-secrets-allowed-prefix 1 R4 '@secrets.TELEGRAM_CHAT_ID_2' # an allowed name as a prefix
check v-secrets-uppercase      1 R4 '@SECRETS.RELEASE_PAT'      # expressions ignore case
# Review 1 finding 3 (probe P5): a token kept in a repository variable.
check v-vars-pat               1 R4 '@vars.RELEASE_PAT'         # P5: ${{ vars.RELEASE_PAT }}
check v-vars-index             1 R4 "@VARS['RELEASE_PAT']"      # the word vars, not only "vars."
# Validation 1 survivors S1, S2: every occurrence on a line is checked, not only up to the first
# allowed one; a line starting with # is text (inside a multi-line double-quoted scalar it is
# part of the value, so the PAT expression is live).
check v-secrets-after-allowed   1 R4 '@}}${{ secrets.RELEASE_PAT' # S1: ${{ secrets.TELEGRAM_CHAT_ID }}${{ secrets.RELEASE_PAT }}
check v-secrets-in-comment-line 1 R4 '@# ${{ secrets.RELEASE_PAT' # S2: T: "x / # ${{ secrets.RELEASE_PAT }}"

# ---- R6: no YAML escape that spells a character or joins two lines ----------------------------
# Review 1 finding 3 (probes P3, P4): each reads, to a YAML reader, as text R3-R5 refuse.
check v-escape-x-secrets               1 R6 '@s\x65crets'      # P3: "${{ s\x65crets.RELEASE_PAT }}"
check v-escape-u-secrets               1 R6 '@s\u0065crets'    # \u, 4 hex digits
check v-escape-U-gh                    1 R6 '@\U00000067h'     # \U, 8 hex digits: gh release upload
check v-escape-x-gh                    1 R6 '@\x67h release'   # P4: "\x67h release upload v1 x"
check v-escape-line-break-secrets      1 R6 '@secr\'           # P4: "${{ secr\<LF>  ets.RELEASE_PAT }}"
check v-escape-line-break-crlf-secrets 1 R6 '@secr\'           # the same, the line break is CR LF
check v-escape-line-break-ls-secrets   1 R6 '@secr\'           # the same, the line break is U+2028 (a YAML 1.1 line break)
# Validation 1 survivors S6-S8: each escaped line break, not only LS and CR LF.
check v-escape-line-break-nel-secrets  1 R6 '@secr\'           # S6: the line break is NEL U+0085
check v-escape-line-break-ps-secrets   1 R6 '@secr\'           # S7: the line break is PS U+2029
check v-escape-line-break-cr-secrets   1 R6 '@secr\'           # S8: a lone CR inside the line (not at its end)
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
# Validation 1 survivors S3, S4: R7 applies to whichever file holds the release job, whatever its
# name and its place in the dir (ci.yml there has no release job; fake BASH_ENV path).
check v-header-env-release-file-second 1 R7 hdiff rel.yml     # S3, S4: rel.yml, sorted after ci.yml
check v-header-env-release-file-first  1 R7 hdiff a-rel.yml   # S3: a-rel.yml, sorted before ci.yml
# T-088 (probe W4): R7 also holds for ci.yml when the release job lives in another file.
check v-trigger-w4-release-moved       1 R7 hdiff             # W4: release job in release.yml (owned header), ci.yml push branches [main]

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

# ---- R9: the installer upload step is the owned literal, and its token occurs once -----------
# T-084 (decision #119): the artifact exists only on the runs where the release job runs.
check v-upload-no-if            1 R9 udiff   # the upload runs on every ref, wip and main included
check v-upload-main-or-tag-if   1 R9 udiff   # main || (tag push): widened by one operand
check v-upload-retention-90     1 R9 udiff   # retention-days: 90 instead of 1
check v-upload-trailing-space   1 R9 udiff   # "retention-days: 1 ": byte identity, not a reading
check v-upload-key-appended     1 R9 udiff   # continue-on-error after the step: the copy's last line pins the step end
check v-upload-missing          1 R9 '*'     # no upload: the release job would download nothing on a tag
check v-upload-twice            1 R9 tok2    # a second, unconditional upload in the same job
check v-upload-uppercase-action 1 R9 '@Actions/Upload-Artifact'   # owner/repo resolve in any case
check v-upload-token-in-comment 1 R9 '@# a second upload-artifact' # a raw rule: a comment is text
check v-upload-in-other-file    1 R9 '@uses: actions/upload-artifact' other.yml   # the count is dir-wide

# ---- R10: the Telegram send step is the owned literal, and its token occurs only there --------
# T-087 (decisions #93, #124; P-020): telegram-guard.sh read YAML and expressions and one file;
# probes T1-T4 (docs/tasks/T-087.md, Investigation section 2) passed it and release-guard R4.
# Each fixture is ok-release (which carries scripts/ci/telegram-send-step.yml verbatim) with one
# edit, and each passes R1-R9, so only R10 can refuse it.
check v-telegram-t1              1 R10 texcess   # T1: no if:, run: | line "# ${{ secrets... }}" (GitHub expands it; a comment is text)
check v-telegram-second-step     1 R10 texcess   # T2: a second step reading the token, guard-shaped if:, before the send step
check v-telegram-if-changed      1 R10 tdiff     # T3: send == 'true' dropped from the send step's if:
check v-telegram-second-file     1 R10 texcess other.yml   # T4: other.yml, a step reading the token, no if: (the count is dir-wide)
check v-telegram-m6              1 R10 tdiff     # T-070 validation 1 M6: always() && send, the ref guard gone
check v-telegram-no-event-name   1 R10 tdiff     # event_name == 'push' dropped: a pull_request from main
check v-telegram-key-appended    1 R10 tdiff     # continue-on-error after run:: the copy's last line pins the step end
check v-telegram-trailing-space  1 R10 tdiff     # "if: ... }} ": byte identity, not a reading
check v-telegram-none            1 R10 jobs      # the send step deleted: the build's send (#93) is gone
check v-telegram-twice           1 R10 texcess   # the owned step twice: the literal occurs exactly once
# T-087 review 1 finding 1: the owned literal moved, the real send step deleted, so count and
# alignment pass. The nearest lines above the literal with fewer than 6, 4 and 2 leading spaces
# must be `    steps:`, `  windows:` and `jobs:`; a block scalar holding the literal cannot have them.
check v-telegram-env-block         1 R10 '@     X: |'   # review 1 probe: jobs.other.env.X: | holds the literal (both secrets to every step of other, every ref)
check v-telegram-other-job         1 R10 '@  other:'    # the owned step in another job's steps: not the reviewed job
check v-telegram-steps-in-scalar   1 R10 '@   name: |'  # a fake "    steps:" inside a job's name: | block scalar
check v-telegram-workflow-env-file 1 R10 '@ X: |' other.yml   # workflow-level env: X: | holds fake windows:/steps: lines and the literal
check v-telegram-first-file-line-2 1 R10 tdiff a.yml   # review 1 finding 2: a.yml sorts first, token on line 2; the alignment starts before line 1, listed as line 1

# ---- R11: action tokens only on owned lines or inside owned step fragments -------------------
# T-088 (decisions #124, P-020; replaces ci-wip.sh and setup-node-cache.sh): every fixture is
# ok-release plus one edit and passes R1-R10, so only R11-R13 can refuse it. Probes W1, W2, S1, S2
# passed both old guards and release-guard (docs/tasks/T-088.md, Investigation section 1).
check ok-action-lookalikes            0   # reuses, users, causes, uses_x, xuses; <<EOF, <<-EOF, <<'EOF', <<"EOF", <<_X, <<<; a spaces-only line
check v-cache-w1-flow-step            1 R11 '@- {name: "cache #1", uses: actions/cache@v4'   # W1: flow-mapping step, " #" inside a quoted value
check v-cache-w2-explicit-key         1 R11 '@? uses'               # W2: explicit key ? uses / : actions/cache@v4
check v-cache-w2-explicit-key         1 R11 '@: actions/cache@v4'   # the action-name line is listed too (no uses word on it)
check v-setup-node-s1-quoted-key      1 R11 '@- uses: actions/setup-node@v5'   # S1: the owned gate fragment plus "cache": pnpm
check v-setup-node-s2-explicit-key    1 R11 '@- uses: actions/setup-node@v5'   # S2: the owned gate fragment plus ? cache / : pnpm
check v-setup-node-cache-pnpm         1 R11 '@- uses: actions/setup-node@v5'   # the owned gate fragment, package-manager-cache: false replaced by cache: pnpm
check v-cache-actions-cache           1 R11 '@- uses: actions/cache@v4'        # actions/cache saves in its post step on every ref
check v-cache-save-not-main           1 R11 '@uses: actions/cache/save@v6'     # the owned cargo-save fragment, its if: without the main operand
check v-cache-rust-save-if-true       1 R11 '@- uses: Swatinem/rust-cache@v2'  # the owned rust-cache fragment, save-if: true
check v-cache-fragment-key-inserted   1 R11 '@- uses: Swatinem/rust-cache@v2'  # a key inside an owned fragment: a second save-if: true (the last one wins)
check v-cache-pnpm-action-cache       1 R11 '@- uses: pnpm/action-setup@v4'    # pnpm/action-setup with cache: true
check v-setup-node-after-fragment-end 1 R11 '@- uses: actions/setup-node@v5'  # the byte-exact pnpm/action-setup fragment ends on this uses line; the step it starts (cache: pnpm) is not vouched for
check v-action-unlisted               1 R11 '@- uses: someone/cache-thing@v1'  # an action that is neither an owned line nor in a fragment
check v-action-job-uses               1 R11 '@    uses: ./.github/workflows/other.yml'   # a job-level uses: (reusable workflow)
check v-action-owned-line-trailing-space 1 R11 '@- uses: actions/checkout@v5 '   # byte identity: an owned line plus a trailing space
check v-cache-second-file             1 R11 '@- uses: Swatinem/rust-cache@v2' other.yaml   # every regular file (.yaml too, no release job)
# The five action-name tokens, any case, comments included (no comment reader): each line listed.
check v-action-name-tokens-in-comments 1 R11 '@# Actions/Cache saves'
check v-action-name-tokens-in-comments 1 R11 '@# setup-node caches'
check v-action-name-tokens-in-comments 1 R11 '@# Swatinem/Rust-Cache saves'
check v-action-name-tokens-in-comments 1 R11 '@# cache/save from main'
check v-action-name-tokens-in-comments 1 R11 '@# PNPM/action-setup with'

# ---- R12: no column-0 line after jobs: ------------------------------------------------------
check v-trigger-w3-col0-on            1 R12 "$(grep -n -x 'on:' "$fx/v-trigger-w3-col0-on/ci.yml" | sed -n 2p | cut -d: -f1)"   # W3: the second on: line, column 0 before "  release:" (+ push: / branches: [main])
check v-r12-col0-comment              1 R12 '@# a comment in column 0'   # a comment is text: no comment reader

# ---- R13: no YAML merge key ------------------------------------------------------------------
check v-cache-merge-key               1 R13 '@- <<: *s'   # M2: steps: &s starts with the byte-exact gate setup-node fragment; <<: *s plus cache: pnpm
check v-cache-merge-key-spaced        1 R13 '@- << : *s'  # the same, written "<< :" (a raw rule: not only the text "<<:")

# ---- R3: permissions only in the workflow-level block and once in the release block ----------
check v-write-other-job        1 R3 '@    permissions:'          # windows job gets contents: write
check v-write-all-windows      1 R3 '@permissions: write-all'
check v-permissions-in-comment 1 R3 '@permissions come'        # comments are raw text too
check v-other-file-write-all   1 R3 '@permissions: write-all' other.yml
check v-permissions-mixed-case-windows 1 R3 '@Permissions: {contents: write}'   # S5 (review 1 probe P7): any case

# ---- R1: the workflow-level block, in every file ---------------------------------------------
# Review 1 finding 2 (probe P11): the block and a jobs: line inside a double-quoted scalar.
check v-r1-jobs-in-quoted-scalar           1 R1 '*'   # P11: two lines `jobs:`, the first inside x-note: "..."
check v-r1-jobs-key-quoted                 1 R1 '*'   # one line `jobs:` (in the scalar), the real key spelled "jobs":
check v-workflow-write                     1 R1 '*'   # permissions: / contents: write at workflow level
check v-no-workflow-permissions            1 R1 '*'   # jobs would take the repository default
check v-workflow-permissions-dup-contents  1 R1 '*'   # contents: read, then contents: write in the same block
check v-other-file-no-workflow-permissions 1 R1 '*' other.yml
# Validation 1 survivors S9, S10.
check v-other-file-env-not-permissions 1 R1 '@jobs:' other.yml   # S9: env: / contents: read / jobs: (the permissions: line replaced)
check v-r1-jobs-uppercase-key          1 R1 '@JOBS:' other.yml   # S10: the block and jobs: in x-note: "...", the real key JOBS:

# ---- the real workflows dir -------------------------------------------------------------------
run "real $real" 0 'ok:' "$real"

# expect <label> <command...>: the command must succeed (a test-side check, not a guard run).
expect() {
  local label="$1"
  shift
  if "$@"; then
    echo "ok   $label"
    passed=$((passed + 1))
  else
    echo "FAIL $label" >&2
    failed=$((failed + 1))
  fi
}
# The owned upload copy, the real dir and ok-release agree (R9 cannot pass on a stale copy).
upl_if() { sed -n 2p "$upl" | sed 's/^ *//'; }
job_if() { grep -m 1 '^    if: ' scripts/ci/release-job.yml | sed 's/^ *//'; }
expect "owned upload copy starts with the upload-artifact@v4 uses line" \
  test "$(head -n 1 "$upl")" = "      - uses: actions/upload-artifact@v4"
expect "owned upload copy: its if: is the release job's if:" test "$(upl_if)" = "$(job_if)"
expect "owned upload copy: retention-days: 1" grep -q -x '          retention-days: 1' "$upl"
tok_count() { cat "$1"/* "$1"/.[!.]* 2>/dev/null | grep -i -o -F 'upload-artifact' | wc -l | tr -d ' '; }
expect "real: the token upload-artifact occurs once in $real" test "$(tok_count "$real")" = 1
expect "real: the upload step is the owned copy $upl" test -z "$(first_udiff "$real/ci.yml")" -a -n "$(tok_lines "$real/ci.yml")"
expect "ok-release: the upload step is the owned copy $upl" test -z "$(first_udiff "$owned_src")" -a -n "$(tok_lines "$owned_src")"
# T-087: the owned send-step copy, the real dir and the ok-* fixtures agree (R10 cannot pass on a
# stale copy, and the real send step is the copy byte for byte, from its name line to the next
# step's first line).
tgs_if="        if: \${{ github.event_name == 'push' && (github.ref == 'refs/heads/main' || startsWith(github.ref, 'refs/tags/v')) && steps.telegram-installer.outputs.send == 'true' }}"
expect "owned send-step copy starts with the send step's name line" test "$(head -n 1 "$tgs")" = "$tgs_name"
expect "owned send-step copy: its if: is push && (main || v* tag) && send == 'true'" test "$(sed -n 2p "$tgs")" = "$tgs_if"
expect "owned send-step copy: no continue-on-error, no status function" test -z "$(grep -i -E 'continue-on-error|always\(|failure\(|cancelled\(|success\(' "$tgs")"
expect "owned send-step copy ends with the next step's first line" test "$(tail -n 1 "$tgs")" = "      - name: SHA-256 of the installer"
expect "owned send-step copy: the token secrets.TELEGRAM_ occurs twice (bot token, chat id)" test "$(ttok_n "$tgs")" = 2
ttok_dir() { cat "$1"/* "$1"/.[!.]* 2>/dev/null | grep -i -o -F 'secrets.telegram_' | wc -l | tr -d ' '; }
expect "real: the Telegram token occurs in $real as often as in $tgs" test "$(ttok_dir "$real")" = "$(ttok_n "$tgs")"
expect "real: the send step is the owned copy $tgs (byte for byte, through the SHA-256 name line)" \
  test -z "$(first_tdiff "$real/ci.yml")" -a -n "$(ttok_lines "$real/ci.yml")"
for c in ok-release ok-lookalikes ok-tag-lookalikes; do
  expect "$c: the send step is the owned copy $tgs" test -z "$(first_tdiff "$fx/$c/ci.yml")" -a "$(ttok_dir "$fx/$c")" = "$(ttok_n "$tgs")"
done

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
# T-084 R9 on the real file: the upload step's if: dropped or widened, or the step deleted.
# inup is 1 from the upload step's uses line through its body (lines indented 8 or more).
mutate up-no-if 'inwin && /upload-artifact/ { inup = 1; print; next } inup && !/^        / { inup = 0 } inup && /^        if:/ { next } { print }' &&
  run "real without the upload step's if:" 1 "$(at "$tmp/up-no-if/ci.yml" "$(first_udiff "$tmp/up-no-if/ci.yml")" R9)" "$tmp/up-no-if"
mutate up-always 'inwin && /upload-artifact/ { inup = 1; print; next } inup && !/^        / { inup = 0 } inup && /^        if: \$\{\{ / { sub(/^        if: \$\{\{ /, "        if: ${{ always() || ") } { print }' &&
  run "real with always() || in the upload step's if:" 1 "$(at "$tmp/up-always/ci.yml" "$(first_udiff "$tmp/up-always/ci.yml")" R9)" "$tmp/up-always"
mutate up-delete 'inwin && /upload-artifact/ { inup = 1; next } inup && /^        / { next } { inup = 0; print }' &&
  run "real with the upload step deleted" 1 "$(at "$tmp/up-delete/ci.yml" '*' R9)" "$tmp/up-delete"
# T-087 R10 on the real file, ported from telegram-guard.test.sh: M6 exactly as T-070 validation 1
# applied it, and the event_name operand dropped. Listed: the send step's if: line.
tg_if_line() { grep -n -m 1 -x -F -- "$tgs_if" "$1" | cut -d: -f1; }
mutate tg-m6 'inwin && /^        if: \$\{\{ github\.event_name == .push. && \(github\.ref == .refs\/heads\/main. \|\| startsWith\(github\.ref, .refs\/tags\/v.\)\) && steps\.telegram-installer\.outputs\.send == .true. \}\}$/ { $0 = "        if: ${{ always() && steps.telegram-installer.outputs.send == '"'"'true'"'"' }}" } { print }' &&
  run "real M6 (always() && send) [R10]" 1 "$(at "$tmp/tg-m6/ci.yml" "$(tg_if_line "$real/ci.yml")" R10)" "$tmp/tg-m6"
mutate tg-no-event 'inwin && /^        if: \$\{\{ github\.event_name == .push. && \(github\.ref == .refs\/heads\/main. \|\| startsWith/ { sub(/github\.event_name == .push. && /, "") } { print }' &&
  run "real without event_name [R10]" 1 "$(at "$tmp/tg-no-event/ci.yml" "$(tg_if_line "$real/ci.yml")" R10)" "$tmp/tg-no-event"
# T-088 R11-R13 and R7 on the real file (ported from ci-wip and setup-node-cache): a cache step
# loosened in ci.yml alone no longer matches its owned fragment, so its uses line is listed.
# ingate is 1 on the gate job's lines.
nth_line() { grep -n -F -- "$2" "$1" | sed -n "${3}p" | cut -d: -f1; }
mutate gate-cache-pnpm '/^  gate:$/ { ingate = 1 } /^  windows:$/ { ingate = 0 } { print } ingate && $0 == "          package-manager-cache: false" && !d { print "          cache: pnpm"; d = 1 }' &&
  run "real with cache: pnpm added to the gate setup-node [R11]" 1 \
    "$(at "$tmp/gate-cache-pnpm/ci.yml" "$(nth_line "$tmp/gate-cache-pnpm/ci.yml" '      - uses: actions/setup-node@v5' 1)" R11)" "$tmp/gate-cache-pnpm"
mutate win-setup-node-pnpm 'inwin && $0 == "          package-manager-cache: false" && !d { $0 = "          cache: pnpm"; d = 1 } { print }' &&
  run "real with the windows setup-node's package-manager-cache: false replaced by cache: pnpm [R11]" 1 \
    "$(at "$tmp/win-setup-node-pnpm/ci.yml" "$(nth_line "$tmp/win-setup-node-pnpm/ci.yml" '      - uses: actions/setup-node@v5' 2)" R11)" "$tmp/win-setup-node-pnpm"
mutate cargo-save-or '/^        if: \$\{\{ github\.ref == .refs\/heads\/main. && steps\.cargo-registry\./ { sub(/ \}\}$/, " || always() }}") } { print }' &&
  run "real with || always() in the cargo save if: [R11]" 1 \
    "$(at "$tmp/cargo-save-or/ci.yml" "$(nth_line "$tmp/cargo-save-or/ci.yml" 'uses: actions/cache/save@v6' 1)" R11)" "$tmp/cargo-save-or"
mutate pnpm-save-no-main '/^        if: \$\{\{ github\.ref == .refs\/heads\/main. && steps\.pnpm-cache\./ { sub(/github\.ref == .refs\/heads\/main. && /, "") } { print }' &&
  run "real with the pnpm save if: without the main operand [R11]" 1 \
    "$(at "$tmp/pnpm-save-no-main/ci.yml" "$(nth_line "$tmp/pnpm-save-no-main/ci.yml" 'uses: actions/cache/save@v6' 2)" R11)" "$tmp/pnpm-save-no-main"
mutate rust-save-if-true 'inwin && /^          save-if: / { $0 = "          save-if: true" } { print }' &&
  run "real with rust-cache save-if: true [R11]" 1 \
    "$(at "$tmp/rust-save-if-true/ci.yml" "$(nth_line "$tmp/rust-save-if-true/ci.yml" '      - uses: Swatinem/rust-cache@v2' 1)" R11)" "$tmp/rust-save-if-true"
mutate actions-cache 'inwin && /^    steps:$/ && !d { print; print "      - uses: actions/cache@v4"; print "        with:"; print "          path: x"; print "          key: y"; d = 1; next } { print }' &&
  run "real with an actions/cache step in the windows job [R11]" 1 \
    "$(at "$tmp/actions-cache/ci.yml" "$(nth_line "$tmp/actions-cache/ci.yml" '      - uses: actions/cache@v4' 1)" R11)" "$tmp/actions-cache"
mutate col0-on '$0 == "  release:" && !d { print "on:"; print "  push:"; print "    branches: [main]"; d = 1 } { print }' &&
  run "real with a column-0 on: before the release job [R12]" 1 \
    "$(at "$tmp/col0-on/ci.yml" "$(grep -n -x 'on:' "$tmp/col0-on/ci.yml" | sed -n 2p | cut -d: -f1)" R12)" "$tmp/col0-on"
# Characterization (already R7 before T-088): the ci.yml trigger cut to [main] or given paths-ignore.
mutate trigger-main '!d && $0 == "    branches: [main, \"wip/**\"]" { $0 = "    branches: [main]"; d = 1 } { print }' &&
  run "real with push branches [main] [R7]" 1 "$(at "$tmp/trigger-main/ci.yml" "$(first_hdiff "$tmp/trigger-main/ci.yml")" R7)" "$tmp/trigger-main"
mutate trigger-paths '{ print } !d && $0 == "    branches: [main, \"wip/**\"]" { print "    paths-ignore: [\"docs/**\"]"; d = 1 }' &&
  run "real with push paths-ignore [R7]" 1 "$(at "$tmp/trigger-paths/ci.yml" "$(first_hdiff "$tmp/trigger-paths/ci.yml")" R7)" "$tmp/trigger-paths"

# T-088: the owned list and fragments (R11) agree with the real ci.yml, and say what the retired
# guards said (test-side reads of the guard's own copies; the guard checks them too, see below).
# has_block <file> <fragment>: the fragment's lines occur in the file as consecutive lines.
has_block() {
  awk -v frag="$2" 'BEGIN { while ((getline l < frag) > 0) F[++n] = l } { L[++m] = $0 }
    END { if (n == 0) exit 1; for (s = 1; s + n - 1 <= m; s++) { for (k = 1; k <= n && L[s + k - 1] == F[k]; k++); if (k > n) exit 0 } exit 1 }' "$1"
}
# frag_kind <fragment>: the action of the fragment's step (its first line holding the word uses,
# the step-ending last line excluded): setup-node, rust-cache, cache/save, pnpm/action-setup.
frag_kind() {
  sed '$d' "$1" | grep -i -E '(^|[^A-Za-z0-9_])uses([^A-Za-z0-9_]|$)' | head -n 1 |
    grep -o -i -E 'setup-node|rust-cache|cache/save|pnpm/action-setup' | head -n 1 | tr 'A-Z' 'a-z'
}
expect "owned list $alines is not empty" test -s "$alines"
while IFS= read -r al || [ -n "$al" ]; do
  expect "owned line '$al' occurs in the real ci.yml" grep -q -x -F -- "$al" "$real/ci.yml"
  expect "owned line '$al' holds the word uses and names no saving action" \
    bash -c 'grep -q -i -E "(^|[^a-z0-9_])uses([^a-z0-9_]|\$)" <<<"$1" && ! grep -q -i -E "actions/cache@|actions/cache/save|cache/save|rust-cache|setup-node|pnpm/action-setup" <<<"$1"' _ "$al"
done < "$alines"
nfrag=0
for fr in "$csd"/*; do
  [ -f "$fr" ] || continue
  nfrag=$((nfrag + 1))
  expect "fragment $fr starts with a step line" bash -c '[[ "$(head -n 1 "$1")" == "      - "* ]]' _ "$fr"
  expect "fragment $fr ends on a step line or a job key line" bash -c 'l="$(tail -n 1 "$1")"; [[ "$l" == "      - "* ]] || [[ "$l" =~ ^\ \ [A-Za-z0-9_-]+:$ ]]' _ "$fr"
  expect "fragment $fr occurs byte for byte in the real ci.yml" has_block "$real/ci.yml" "$fr"
  body="$(sed '$d' "$fr")"
  case "$(frag_kind "$fr")" in
    setup-node)
      expect "setup-node fragment $fr: package-manager-cache: false" grep -q -x -F '          package-manager-cache: false' <<<"$body"
      expect "setup-node fragment $fr: the raw cache on no other line" test "$(grep -c -i 'cache' <<<"$body")" = 1 ;;
    rust-cache)
      expect "rust-cache fragment $fr: save-if on main only" grep -q -x -F "          save-if: \${{ github.ref == 'refs/heads/main' }}" <<<"$body"
      expect "rust-cache fragment $fr: one save-if line" test "$(grep -c -i 'save-if' <<<"$body")" = 1 ;;
    cache/save)
      expect "cache/save fragment $fr: if: on main only" grep -q -F "        if: \${{ github.ref == 'refs/heads/main' && " <<<"$body"
      expect "cache/save fragment $fr: one if: line" test "$(grep -c -E '^ *if:' <<<"$body")" = 1
      expect "cache/save fragment $fr: no ||" test -z "$(grep -F '||' <<<"$body")" ;;
    pnpm/action-setup)
      expect "pnpm/action-setup fragment $fr: no cache" test -z "$(grep -i 'cache' <<<"$body")" ;;
    *)
      expect "fragment $fr: its step uses setup-node, rust-cache, cache/save or pnpm/action-setup" false ;;
  esac
done
expect "owned fragments in $csd: at least one" test "$nfrag" -gt 0
# The real cache steps are all owned: each kind the retired guards covered has a fragment.
for kind in setup-node rust-cache cache/save pnpm/action-setup; do
  expect "owned fragments in $csd: one for $kind" bash -c 'for f in "$1"/*; do [ -f "$f" ] || continue; sed "\$d" "$f" | grep -q -i -F -- "$2" && exit 0; done; exit 1' _ "$csd" "$kind"
done

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
# Validation 1 survivor S13: awk failing is exit 3, never a pass. A fake awk (exit 2) first on
# PATH, only for this one guard run; nothing else in the guard's path is replaced.
mkdir -p "$tmp/fake-awk"
printf '#!/bin/sh\nexit 2\n' > "$tmp/fake-awk/awk"
chmod +x "$tmp/fake-awk/awk"
saved_path="$PATH"
PATH="$tmp/fake-awk:$PATH"
run "cannot run: awk fails (exit 2)" 3 'cannot run: awk failed \(exit 2\)' "$real"
PATH="$saved_path"
# The owned header copy (R7) is a guard input like release-job.yml: missing, or not ending in the
# line `jobs:` (a truncated copy would compare only part of the header), is never a pass. The
# guard is copied next to its release-job.yml; the header copy is left out or truncated.
owned_guard() {
  mkdir -p "$tmp/$1"
  cp -p "$guard" scripts/ci/release-job.yml "$upl" "$tgs" "$alines" "$tmp/$1/" 2>/dev/null
  cp -pR "$csd" "$tmp/$1/" 2>/dev/null
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
# T-084: the owned upload copy (R9) is a guard input too: missing, empty, or not starting with the
# uses line (a copy cut at the top would compare the wrong lines) is never a pass. The guard is
# copied next to its other two owned copies; the upload copy is left out or broken.
upload_guard() {
  mkdir -p "$tmp/$1"
  if ! cp -p "$guard" scripts/ci/release-job.yml scripts/ci/release-header.yml "$tgs" "$alines" "$tmp/$1/" || ! cp -pR "$csd" "$tmp/$1/"; then
    echo "FAIL $1: cannot copy the guard and its owned copies (fix the test)" >&2
    failed=$((failed + 1))
    return 1
  fi
}
if upload_guard g-upload-missing; then
  guard="$tmp/g-upload-missing/release-guard.sh"
  run "cannot run: owned upload copy missing" 3 'cannot run.*release-upload\.yml' "$real"
fi
guard="$real_guard"
if upload_guard g-upload-empty; then
  : > "$tmp/g-upload-empty/release-upload.yml"
  guard="$tmp/g-upload-empty/release-guard.sh"
  run "cannot run: owned upload copy empty" 3 'cannot run.*release-upload\.yml' "$real"
fi
guard="$real_guard"
if upload_guard g-upload-no-uses; then
  sed 1d "$upl" > "$tmp/g-upload-no-uses/release-upload.yml"
  guard="$tmp/g-upload-no-uses/release-guard.sh"
  run "cannot run: owned upload copy does not start with the uses line" 3 'cannot run.*release-upload\.yml' "$real"
fi
guard="$real_guard"
# T-087: the owned send-step copy (R10) is a guard input too: missing, empty, holding no token (a
# copy with nothing to align on) or not starting with the send step's name line (a copy cut at
# the top) is never a pass. The guard is copied next to its other three owned copies.
tgs_guard() {
  mkdir -p "$tmp/$1"
  if ! cp -p "$guard" scripts/ci/release-job.yml scripts/ci/release-header.yml "$upl" "$alines" "$tmp/$1/" || ! cp -pR "$csd" "$tmp/$1/"; then
    echo "FAIL $1: cannot copy the guard and its owned copies (fix the test)" >&2
    failed=$((failed + 1))
    return 1
  fi
}
if tgs_guard g-tgs-missing; then
  guard="$tmp/g-tgs-missing/release-guard.sh"
  run "cannot run: owned send-step copy missing" 3 'cannot run.*telegram-send-step\.yml' "$real"
fi
guard="$real_guard"
if tgs_guard g-tgs-empty; then
  : > "$tmp/g-tgs-empty/telegram-send-step.yml"
  guard="$tmp/g-tgs-empty/release-guard.sh"
  run "cannot run: owned send-step copy empty" 3 'cannot run.*telegram-send-step\.yml' "$real"
fi
guard="$real_guard"
if tgs_guard g-tgs-no-token; then
  grep -v -i -F 'secrets.telegram_' "$tgs" > "$tmp/g-tgs-no-token/telegram-send-step.yml"
  guard="$tmp/g-tgs-no-token/release-guard.sh"
  run "cannot run: owned send-step copy holds no token" 3 'cannot run.*telegram-send-step\.yml' "$real"
fi
guard="$real_guard"
if tgs_guard g-tgs-no-name; then
  sed 1d "$tgs" > "$tmp/g-tgs-no-name/telegram-send-step.yml"
  guard="$tmp/g-tgs-no-name/release-guard.sh"
  run "cannot run: owned send-step copy does not start with the name line" 3 'cannot run.*telegram-send-step\.yml' "$real"
fi
guard="$real_guard"

# T-088: the owned list scripts/ci/action-lines.txt and the owned fragments scripts/ci/cache-steps/
# (R11) are guard inputs too. r11_guard <name>: the guard and all its owned copies in
# $tmp/<name>/g, a copy of ok-release in $tmp/<name>/wf.
r11_guard() {
  mkdir -p "$tmp/$1/g" "$tmp/$1/wf"
  if ! cp -p "$real_guard" scripts/ci/release-job.yml scripts/ci/release-header.yml "$upl" "$tgs" "$alines" "$tmp/$1/g/" ||
     ! cp -pR "$csd" "$tmp/$1/g/" || ! cp -p "$owned_src" "$tmp/$1/wf/"; then
    echo "FAIL $1: cannot copy the guard and its owned copies (fix the test)" >&2
    failed=$((failed + 1))
    return 1
  fi
}
# r11_cannot <name> <label> <ERE>: the guard copy in $tmp/<name>/g on $tmp/<name>/wf is exit 3.
r11_cannot() {
  guard="$tmp/$1/g/release-guard.sh"
  run "$2" 3 "$3" "$tmp/$1/wf"
  guard="$real_guard"
}
if r11_guard g-alines-missing; then
  rm -f "$tmp/g-alines-missing/g/action-lines.txt"
  r11_cannot g-alines-missing "cannot run: owned action lines missing" 'cannot run.*action-lines\.txt'
fi
if r11_guard g-alines-empty; then
  : > "$tmp/g-alines-empty/g/action-lines.txt"
  r11_cannot g-alines-empty "cannot run: owned action lines empty" 'cannot run.*action-lines\.txt'
fi
if r11_guard g-steps-missing; then
  rm -rf "$tmp/g-steps-missing/g/cache-steps"
  r11_cannot g-steps-missing "cannot run: owned fragment dir missing" 'cannot run.*cache-steps'
fi
if r11_guard g-steps-empty; then
  rm -f "$tmp/g-steps-empty/g/cache-steps"/*
  r11_cannot g-steps-empty "cannot run: owned fragment dir empty" 'cannot run.*cache-steps'
fi
# frag_case <name> <fragment basename> <label> <sed script>: the fragment edited by the sed script
# in the guard's copy AND inserted at the end of the gate job of the workflow copy, so the edited
# fragment occurs byte for byte and only the guard's check of its own copy can refuse it (the copy
# and ci.yml loosened together). A fragment ending on a job key line `  windows:` is completed by
# ok-release's own `  windows:` line; any other is followed by a run: line. Exit 3 naming it.
frag_case() {
  local n="$1" base="$2" label="$3" sc="$4" fr t
  r11_guard "$n" || return
  fr="$tmp/$n/g/cache-steps/$base"
  if [ ! -f "$csd/$base" ]; then
    echo "FAIL $n: $csd/$base not found (fix the test with the owned fragments)" >&2
    failed=$((failed + 1))
    return
  fi
  sed -e "$sc" "$csd/$base" > "$fr"
  if cmp -s "$csd/$base" "$fr"; then
    echo "FAIL $n: the edit did not apply to $base (fix the test)" >&2
    failed=$((failed + 1))
    return
  fi
  t="$tmp/$n/ins"
  if [ "$(tail -n 1 "$fr")" = "  windows:" ]; then sed '$d' "$fr" > "$t"; else { cat "$fr"; echo "        run: echo next"; } > "$t"; fi
  awk -v t="$t" '{ print } NR == 31 { while ((getline l < t) > 0) print l }' "$owned_src" > "$tmp/$n/wf/ci.yml"
  r11_cannot "$n" "$label" "cannot run.*$(ere "$base")"
}
frag_case g-frag-sn-cache-pnpm gate-setup-node.yml "cannot run: setup-node fragment with cache: pnpm instead of package-manager-cache: false" \
  's/^          package-manager-cache: false$/          cache: pnpm/'
frag_case g-frag-sn-cache-added gate-setup-node.yml "cannot run: setup-node fragment with cache: pnpm beside package-manager-cache: false" \
  '/^          package-manager-cache: false$/a\          cache: pnpm'
frag_case g-frag-rust-save-if-true windows-rust-cache.yml "cannot run: rust-cache fragment with save-if: true" \
  's/^          save-if: .*$/          save-if: true/'
frag_case g-frag-rust-two-save-if windows-rust-cache.yml "cannot run: rust-cache fragment with a second save-if: true after the main one (the last one wins)" \
  '/^          save-if: /a\          save-if: true'
frag_case g-frag-save-two-if gate-cargo-save.yml "cannot run: cache/save fragment with a second if: without the main operand (the last one wins)" \
  '2a\        if: ${{ steps.cargo-registry.outputs.cache-hit != '"'"'true'"'"' }}'
frag_case g-frag-save-no-main gate-cargo-save.yml "cannot run: cache/save fragment whose if: lacks the main operand" \
  "s/github.ref == 'refs\/heads\/main' && //"
frag_case g-frag-save-or gate-cargo-save.yml "cannot run: cache/save fragment whose if: holds ||" \
  's/^\(        if: .*\) }}$/\1 || always() }}/'
frag_case g-frag-pnpm-cache windows-pnpm-action-setup.yml "cannot run: pnpm/action-setup fragment with cache: true" \
  '1a\        with:\n          cache: true'
frag_case g-frag-short-end gate-setup-node.yml "cannot run: fragment not ending on a step-ending line (a key could be appended)" \
  '$d'
frag_case g-frag-comment-end gate-setup-node.yml "cannot run: fragment ending on a comment line (the next key joins the step)" \
  '$s/^.*$/      # end of the step/'
frag_case g-frag-no-step-start gate-cargo-save.yml "cannot run: fragment not starting with a step line (keys before it are unpinned)" \
  '1d'

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
