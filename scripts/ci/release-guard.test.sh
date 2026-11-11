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
#        byte-exact copy of one of the owned step fragments, one per regular file in
#        scripts/ci/cache-steps/ (each from its `      - ` line through the next step's first line or the next job's key
#        line, so no key can be appended; that last line only pins the end: it is not inside
#        the fragment, so the step it starts needs its own owned line or fragment). Listed line:
#        every such token line outside them.
#        The guard checks its own fragments before it reads the dir: a fragment not starting with
#        `      - ` or not ending on a `      - ` line or a job key line (`  <name>:`); not one
#        step (a second `      - ` line before the last, not exactly one line with the word
#        uses, or that line not `      - uses: ` / `        uses: ` plus actions/setup-node,
#        Swatinem/rust-cache, actions/cache/save or pnpm/action-setup, `@` and a ref of
#        [A-Za-z0-9._-]); a setup-node or rust-cache fragment whose line 1 is not the uses line,
#        line 2 not `        with:`, line 3 not `          package-manager-cache: false` /
#        `          save-if: ${{ github.ref == 'refs/heads/main' }}`, line 4 blank or indented
#        more than 10, the word with on a second line, or the raw cache (save-if) on another
#        line; a cache/save fragment whose line 1 is not `      - name: ` plus [A-Za-z0-9 ()._-],
#        line 2 not starting `        if: ${{ github.ref == 'refs/heads/main' && `, not ending in
#        `}}` or not holding exactly one `${{` and one `}}`, line 3 blank or indented more than 8,
#        the word if on a second line, or holding ||; a pnpm/action-setup fragment holding cache;
#        any fragment whose body (all but its last line) holds the raw `*` (a YAML alias: `*c :
#        pnpm` with the anchor `&c cache` outside the fragment spells cache with no cache on the
#        line, so the word counts rest on no alias as on R6 and R8) or the raw `&` outside `&&`
#        (an anchor; `&&` in a cache/save if: passes; T-088 r3, decision #128, review 2 finding 1):
#        exit 3 naming the fragment (even when the workflow carries the same edit, so the copy
#        and ci.yml cannot be loosened together; review 1 finding 1, probes A-G); the list or the
#        fragment dir missing or empty is exit 3 too.
#     R12 (T-088) after a file's `jobs:` line, every line that is not empty or spaces only starts
#        with a space (probe W3: a column-0 `on:` between two jobs turns the release job into a
#        key of it; a column-0 comment counts too, there is no comment reader). Listed line:
#        the column-0 line.
#     R13 (T-088) no YAML merge key: the raw text << not followed by <, a letter, _, -, ' or "
#        (bash here-docs <<EOF, <<-EOF, <<'EOF', <<"EOF", <<_X and here-strings <<< pass;
#        `cat << EOF` and a shift `$((1 << 2))` are refused, fail-closed).
#        Probe M2: an anchored step list starting with a byte-exact setup-node fragment, merged
#        into another job's step with cache: pnpm. Listed line: the occurrence.
# T-202 (decisions #249, #253; P-020) the contour exception, pinned below (red first):
#     R14 a file whose basename is exactly teamwright-agent.yml or teamwright-next.yml (string
#        equality with the two owned literals; no glob, suffix, case folding or regex) is not
#        subject to R1's block check; instead its header, line 1 to its first `jobs:` line, is
#        byte-identical to the owned copy <guard dir>/contour-heads/<name> (listed line: the first
#        header line that differs, as for R7). For those two files only: R3 counts the `permissions`
#        word only on that header's permissions line, R4 allows only the name's owned tokens
#        (secrets.CLAUDE_CODE_OAUTH_TOKEN in teamwright-agent.yml, vars.TEAMWRIGHT_AUTH in both,
#        each ended by a character outside [A-Za-z0-9_]) and R5 (gh) is waived. R2, R6-R13 hold for
#        them as for every file. Any other name keeps R1/R3/R4/R5 (v-contour-renamed-*,
#        v-contour-third-write). A head copy missing, empty, not ending in its only `jobs:` line or
#        holding no `permissions:` line is exit 3 naming contour-heads (read only when a file with
#        one of the two names is present). ci.yml's header gains `  workflow_dispatch:` (R7's owned
#        copy, decisions #253).
#   Exit 0: ok, one line containing "ok:".
#   Exit 1: a violation; each listed as "<workflows-dir>/<file>:<line>: R<n>: <why>". <line> is
#           the first line of the block that differs from the owned copy for R2 (for a second
#           `  release:`, that line), the line of the occurrence for R3, R4, R5, R6 and R8 (for an
#           escaped line break, the line ending in the backslash).
#   Exit 2: usage error (no argument, or more than one).
#   Exit 3: cannot run (the dir is missing or not a directory, holds no regular file, holds no
#           ci.yml (review 1 finding 2: fixture c-no-ci-file), or a file has no top-level `jobs:`
#           line; awk failed); never a pass.
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
  if [ "$want" = 3 ]; then
    run "$c [$rule]" 3 "$where" "$d"
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
check v-header-on-trigger        1 R7 hdiff   # on: gains workflow_call (T-202: workflow_dispatch is now in the owned header, decisions #253)
# Validation 1 survivors S3, S4: R7 applies to whichever file holds the release job, whatever its
# name and its place in the dir (ci.yml there has no release job; fake BASH_ENV path).
check v-header-env-release-file-second 1 R7 hdiff rel.yml     # S3, S4: rel.yml, sorted after ci.yml
check v-header-env-release-file-first  1 R7 hdiff a-rel.yml   # S3: a-rel.yml, sorted before ci.yml
# T-088 (probe W4): R7 also holds for ci.yml when the release job lives in another file.
check v-trigger-w4-release-moved       1 R7 hdiff             # W4: release job in release.yml (owned header), ci.yml push branches [main]
# T-088 review 1 finding 2: W4 with ci.yml renamed to build.yml; the guard itself needs ci.yml.
check c-no-ci-file                     3 'cannot run' 'cannot run.*ci\.yml'   # probe pt-W4R

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
# Review 1 finding 3: R13 is a raw-text rule; a here-doc with a space before its delimiter and a
# shift are refused too (fail-closed; write <<EOF).
check v-r13-heredoc-space-and-shift 1 R13 '@cat << EOF'
check v-r13-heredoc-space-and-shift 1 R13 '@$((1 << 2))'

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

# ---- T-202: the contour exception (R14) ----------------------------------------------------------
# ccheck <case> <allowed> <required...>: the case dir is refused (exit 1) with exactly the listed
# violations. A required item is "<file>|<where>|<rule>": the guard must list <rule> at <where> of
# <file> (where: "@text" the first line containing text, "jobs" the jobs: line, "cdiff:<ref>" the
# first header line that differs from the header of $fx/ok-contour/<ref>). <allowed> is a
# space-separated list of "<file>:<rule>" pairs that may also be listed (any line). Any other
# listed violation fails the case, so the two legitimate contour files must come out clean and
# the exception cannot hide behind a pile of refusals.
first_cdiff() {
  local f="$1" ref="$2" i n
  local -a a b
  mapfile -t a < <(head_lines "$fx/ok-contour/$ref")
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
ccheck() {
  local c="$1" allowed=" $2 teamwright-agent.yml:R9 " d="$fx/$1" out got line rest file tail num rule item f w r l f0 bad=0
  shift 2
  seen["$c"]=1
  if [ ! -x "$guard" ]; then
    echo "FAIL $c: $guard not found or not executable" >&2
    failed=$((failed + 1))
    return
  fi
  out="$("$guard" "$d" 2>&1)"
  got=$?
  if [ "$got" -ne 1 ]; then
    echo "FAIL $c: exit $got, want 1" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
    return
  fi
  for item in "$@"; do
    IFS='|' read -r f w r <<<"$item"
    allowed+="$f:$r "
  done
  while IFS= read -r line; do
    [[ "$line" == "$d/"* ]] || continue
    rest="${line#"$d/"}"
    file="${rest%%:*}"
    tail="${rest#*:}"
    num="${tail%%:*}"
    tail="${tail#*: }"
    rule="${tail%%:*}"
    if [[ "$allowed" != *" $file:$rule "* ]]; then
      echo "FAIL $c: unexpected violation $file $rule (line $num): only the intended refusal may be listed" >&2
      bad=1
    fi
  done <<<"$out"
  for item in "$@"; do
    IFS='|' read -r f w r <<<"$item"
    f0="$d/$f"
    case "$w" in
      @*) l="$(line_of "$f0" "${w#@}")" ;;
      jobs) l="$(grep -n -m 1 -x 'jobs:' "$f0" | cut -d: -f1)" ;;
      cdiff:*) l="$(first_cdiff "$f0" "${w#cdiff:}")" ;;
      *) l="$w" ;;
    esac
    if [ -z "$l" ]; then
      echo "FAIL $c: cannot locate the expected line ($w in $f0; fix the test)" >&2
      bad=1
    elif ! grep -qE -- "$(at "$f0" "$l" "$r")" <<<"$out"; then
      echo "FAIL $c: no $r listed at $f:$l" >&2
      bad=1
    fi
  done
  if [ "$bad" -ne 0 ]; then
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
  else
    echo "ok   $c [$*]: exit 1"
    passed=$((passed + 1))
  fi
}
A=teamwright-agent.yml
N=teamwright-next.yml
# T-206: ok-contour is the base of the runtime-built dirs of the T-206 section below (its agent file plus the
# owned tail scripts/ci/contour-upload.yml, which this test does not hold as text): ci.yml plus both contour
# files with their owned heads, workflow-level contents: write, gh, the OAuth secret and vars.TEAMWRIGHT_AUTH only there.
seen[ok-contour]=1
# A third write-capable file, and renamed copies of the two names, keep R1 (and R3-R5 of any other file).
ccheck v-contour-third-write "teamwright-third.yml:R3 teamwright-third.yml:R4 teamwright-third.yml:R5" "teamwright-third.yml|jobs|R1"
ccheck v-contour-renamed-agent2     "teamwright-agent2.yml:R3 teamwright-agent2.yml:R4 teamwright-agent2.yml:R5" "teamwright-agent2.yml|jobs|R1"
ccheck v-contour-renamed-yaml       "teamwright-agent.yaml:R3 teamwright-agent.yaml:R4 teamwright-agent.yaml:R5" "teamwright-agent.yaml|jobs|R1"
ccheck v-contour-renamed-prefix     "x-teamwright-agent.yml:R3 x-teamwright-agent.yml:R4 x-teamwright-agent.yml:R5" "x-teamwright-agent.yml|jobs|R1"
ccheck v-contour-renamed-dotfile    ".teamwright-agent.yml:R3 .teamwright-agent.yml:R4 .teamwright-agent.yml:R5" ".teamwright-agent.yml|jobs|R1"
ccheck v-contour-renamed-upper      "teamwright-AGENT.yml:R3 teamwright-AGENT.yml:R4 teamwright-AGENT.yml:R5" "teamwright-AGENT.yml|jobs|R1"
ccheck v-contour-renamed-underscore "teamwright-agent_yml:R3 teamwright-agent_yml:R4 teamwright-agent_yml:R5" "teamwright-agent_yml|jobs|R1"
ccheck v-contour-renamed-next-copy  "teamwright-next-copy.yml:R3 teamwright-next-copy.yml:R4 teamwright-next-copy.yml:R5" "teamwright-next-copy.yml|jobs|R1"
# A scope beyond the owned head, or any other byte of it: R14 at the first differing header line.
ccheck v-contour-extra-scope-pull-requests "" "$A|cdiff:$A|R14"
ccheck v-contour-extra-scope-id-token      "" "$N|cdiff:$N|R14"
ccheck v-contour-extra-scope-write-all     "$A:R3" "$A|cdiff:$A|R14"
ccheck v-contour-extra-scope-comment       "$N:R3" "$N|cdiff:$N|R14"
# A job-level block stays refused in the two files (R3 counts only the head's line).
ccheck v-contour-job-permissions "" "$A|@    permissions:|R3"
# Only the name's owned tokens (R4), whatever the other secrets, variables or prose.
ccheck v-contour-other-secret-github-token     "" "$A|@secrets.GITHUB_TOKEN|R4"
ccheck v-contour-other-secret-anthropic-key    "" "$A|@secrets.ANTHROPIC_API_KEY|R4"
ccheck v-contour-other-secret-tojson           "" "$A|@toJSON(secrets)|R4"
ccheck v-contour-other-secret-vars-parallel    "" "$A|@vars.TEAMWRIGHT_PARALLEL|R4"
ccheck v-contour-other-secret-oauth-longer     "" "$A|@secrets.CLAUDE_CODE_OAUTH_TOKEN_2|R4"
ccheck v-contour-other-secret-vars-longer      "" "$A|@vars.TEAMWRIGHT_AUTH_X|R4"
ccheck v-contour-other-secret-prose            "" "$A|@# Secrets here|R4"
ccheck v-contour-other-secret-oauth-in-next    "" "$N|@secrets.CLAUDE_CODE_OAUTH_TOKEN|R4"
ccheck v-contour-other-secret-oauth-in-ci      "" "ci.yml|@# \${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}|R4"
ccheck v-contour-other-secret-vars-in-ci       "" "ci.yml|@# \${{ vars.TEAMWRIGHT_AUTH }}|R4"
# The Telegram secrets, an escape, an unowned action, prose naming uses and a second release job.
ccheck v-contour-telegram       "$A:R4" "$A|@secrets.TELEGRAM_BOT_TOKEN|R10"
ccheck v-contour-escape         ""      "$A|@\\x41|R6"
ccheck v-contour-unowned-uses   ""      "$A|@anthropics/claude-code-action|R11"
ccheck v-contour-prose-uses     ""      "$A|@# this step uses the runner|R11"
ccheck v-contour-release        "$A:R2 $A:R7" "$A|@  release:|R2"

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
# T-101 (#200 D4): the one artifact carries the symbols zip. R9 compares bytes only, so without this
# line dropping the zip from the copy, ci.yml and the fixtures together would stay green.
expect "owned upload copy: its path holds the symbols zip" \
  grep -q -x -F '            target/release/bundle/nsis/voicen-${{ env.VOICEN_VERSION }}-symbols.zip' "$upl"
tok_count() { cat "$1"/* "$1"/.[!.]* 2>/dev/null | grep -i -o -F 'upload-artifact' | wc -l | tr -d ' '; }
# T-206: one per owned copy: the installer upload (release-upload.yml) and the contour tail (contour-upload.yml).
expect "real: the token upload-artifact occurs twice in $real, one per owned copy" test "$(tok_count "$real")" = 2
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
# T-083 (decision #144): the push trigger skips a push whose whole diff lies under docs/tasks/**,
# nothing wider. R7 only compares ci.yml with the owned header, so the owned copy's content is
# pinned here (a widening must edit ci.yml, the owned copy, the fixtures and this test together).
# Raw text, comments included: exactly one header line holds paths or branches-ignore (any case),
# it is the owned filter line, and it sits right after the push trigger's tags line.
hdr=scripts/ci/release-header.yml
pig='    paths-ignore: ["docs/tasks/**"]'
expect "owned header $hdr: the line '$pig' occurs exactly once" \
  test "$(grep -c -x -F -- "$pig" "$hdr")" = 1
expect "owned header $hdr: the line before '$pig' is the push trigger's '    tags: [\"v*\"]'" \
  test "$(grep -x -F -B 1 -- "$pig" "$hdr" | head -n 1)" = '    tags: ["v*"]'
expect "owned header $hdr: no other line holds paths or branches-ignore (raw text, comments included)" \
  test "$(grep -i -E 'paths|branches-ignore' "$hdr")" = "$pig"
mutate trigger-no-paths-ignore '$0 == "    paths-ignore: [\"docs/tasks/**\"]" { next } { print }' &&
  run "real with the push paths-ignore line deleted [R7]" 1 "$(at "$tmp/trigger-no-paths-ignore/ci.yml" "$(first_hdiff "$tmp/trigger-no-paths-ignore/ci.yml")" R7)" "$tmp/trigger-no-paths-ignore"
mutate trigger-paths-wide '$0 == "    paths-ignore: [\"docs/tasks/**\"]" { $0 = "    paths-ignore: [\"docs/**\"]" } { print }' &&
  run "real with push paths-ignore widened to docs/** [R7]" 1 "$(at "$tmp/trigger-paths-wide/ci.yml" "$(first_hdiff "$tmp/trigger-paths-wide/ci.yml")" R7)" "$tmp/trigger-paths-wide"

# T-202 (decisions #249, #253): the contour files in the real dir, and the dispatch trigger.
# The owned head copies agree with the real files and with the fixtures (R14 cannot pass on a stale
# copy), the dir holds exactly the three files, and ci.yml's header carries workflow_dispatch.
chd=scripts/ci/contour-heads
wd='  workflow_dispatch:'
expect "real: the workflows dir holds exactly ci.yml, teamwright-agent.yml and teamwright-next.yml" \
  test "$(ls -A "$real" | tr '\n' ' ')" = "ci.yml teamwright-agent.yml teamwright-next.yml "
for cn in teamwright-agent.yml teamwright-next.yml; do
  expect "owned head $chd/$cn exists, ends in its only jobs: line, holds one permissions: line" \
    bash -c '[ -f "$1" ] && [ "$(tail -n 1 "$1")" = "jobs:" ] && [ "$(grep -c -x "jobs:" "$1")" = 1 ] && [ "$(grep -c -x "permissions:" "$1")" = 1 ]' _ "$chd/$cn"
  expect "real: $cn header is the owned head $chd/$cn" \
    bash -c '[ -f "$1" ] && [ -f "$2" ] && [ "$(awk "{ print } \$0 == \"jobs:\" { exit }" "$1")" = "$(cat "$2")" ]' _ "$real/$cn" "$chd/$cn"
  expect "fixture ok-contour/$cn header is the owned head $chd/$cn" \
    bash -c '[ -f "$2" ] && [ "$(awk "{ print } \$0 == \"jobs:\" { exit }" "$1")" = "$(cat "$2")" ]' _ "$fx/ok-contour/$cn" "$chd/$cn"
  expect "real: $cn runs on the self-hosted runner and keeps no glob-able twin" \
    bash -c 'grep -q -x -F "    runs-on: [self-hosted, Linux, X64, claude]" "$1"' _ "$real/$cn"
done
expect "owned head $chd/teamwright-agent.yml: no pull-requests, no id-token, no write-all" \
  bash -c '[ -f "$1" ] && ! grep -q -E "pull-requests|id-token|write-all" "$1"' _ "$chd/teamwright-agent.yml"
# T-202 review 1 findings 1 and 2: the stage session never holds the job token, and the push
# step's pause check reads a file the owner controls, with the hooks of the pre-session commit.
# ag_step <title>: the lines of the agent's step "      - name: <title>" up to the next step.
ag=$real/teamwright-agent.yml
ag_step() { awk -v t="      - name: $1" '$0 == t { s = 1; print; next } s && /^      - / { exit } s { print }' "$ag"; }
ag_push='Push task/<ID>, and wip/<ID> after implement'
expect "real agent: one checkout, and its with: holds persist-credentials: false (the token stays out of .git/config)" \
  bash -c '[ "$(grep -c -F "uses: actions/checkout@" "$1")" = 1 ] && awk "/^      - uses: actions\/checkout@/ { s = 1; next } s && /^      - / { exit } s && \$0 == \"          persist-credentials: false\" { f = 1 } END { exit !f }" "$1"' _ "$ag"
expect "real agent: github.token occurs only in the steps after the stage (push and chain), never at job level" \
  bash -c 'c="$(grep -n -m 1 -x -F "      - name: Did the stage commit?" "$1" | cut -d: -f1)"; [ -n "$c" ] && ! awk -v c="$c" "NR < c && /github\\.token|GH_TOKEN|GITHUB_TOKEN/ { f = 1 } END { exit !f }" "$1"' _ "$ag"
expect "real agent: the stage step runs claude without the runner's environment-file paths" \
  bash -c 'grep -q -E "^ +env -u GITHUB_ENV -u GITHUB_PATH -u GITHUB_OUTPUT -u GITHUB_STATE claude " "$1"' _ "$ag"
expect "real agent: the push step's env sets TEAMWRIGHT_PAUSE_FILE to the owner's runner-host file /etc/teamwright/voicen.pause" \
  bash -c 'grep -q -x -F "          TEAMWRIGHT_PAUSE_FILE: /etc/teamwright/voicen.pause" <<<"$1"' _ "$(ag_step "$ag_push")"
expect "real agent: the push step takes its token from its own env and the hooks from the pre-session commit, not install-hooks.sh" \
  bash -c 'grep -q -x -F "          PUSH_TOKEN: \${{ github.token }}" <<<"$1" && grep -q -F "git archive \"\$BEFORE\" scripts/hooks" <<<"$1" && grep -q -F "core.hooksPath=" <<<"$1" && ! grep -q -F "install-hooks.sh" <<<"$1"' _ "$(ag_step "$ag_push")"
expect "real agent: the push step reads BEFORE from the branch step's output (an env file the session can write does not decide)" \
  bash -c 'grep -q -x -F "          BEFORE: \${{ steps.branch.outputs.before }}" <<<"$1"' _ "$(ag_step "$ag_push")"
# A scope added to a real head is refused at the first changed header line (R14).
for cn in teamwright-agent.yml teamwright-next.yml; do
  mkdir -p "$tmp/real-scope-$cn"
  cp -p "$real"/* "$tmp/real-scope-$cn/"
  if [ -f "$real/$cn" ]; then
    awk '{ print } !d && $0 == "  actions: write" { print "  packages: write"; d = 1 }' "$real/$cn" > "$tmp/real-scope-$cn/$cn"
  fi
  if [ -f "$real/$cn" ] && ! cmp -s "$real/$cn" "$tmp/real-scope-$cn/$cn"; then
    l="$(grep -n -m 1 -x '  packages: write' "$tmp/real-scope-$cn/$cn" | cut -d: -f1)"
    run "real $cn with packages: write added to its head [R14]" 1 "$(at "$tmp/real-scope-$cn/$cn" "${l:-0}" R14)" "$tmp/real-scope-$cn"
  else
    echo "FAIL real $cn with a scope added: the mutation did not apply ($real/$cn missing or has no '  actions: write' line)" >&2
    failed=$((failed + 1))
  fi
done
# The dispatch trigger (decisions #253, OQ-78 (a)): the owned header and the real ci.yml carry the
# one line, every fixture header with the release job carries it (R7 compares bytes), the release
# job, the upload and the Telegram send still require event_name == push (a dispatch publishes,
# uploads and sends nothing), and ci.yml alone losing or doubling the line is refused by R7.
expect "owned header $hdr: the line '$wd' occurs exactly once" test "$(grep -c -x -F -- "$wd" "$hdr")" = 1
expect "real: ci.yml holds the line '$wd' exactly once, in its header" \
  test "$(head_lines "$real/ci.yml" | grep -c -x -F -- "$wd")" = 1 -a "$(grep -c -x -F -- "$wd" "$real/ci.yml")" = 1
nodisp=""
for f in "$fx"/*/*; do
  [ -f "$f" ] && grep -q -x '  release:' "$f" && ! head_lines "$f" | grep -q -x -F -- "$wd" && nodisp="$nodisp ${f#"$fx/"}"
done
expect "every fixture file holding the release job has '$wd' in its header (missing:${nodisp:- none})" test -z "$nodisp"
for pf in "$upl" scripts/ci/release-job.yml; do
  expect "$pf: still requires github.event_name == 'push' (a dispatch publishes and uploads nothing)" grep -q -F "github.event_name == 'push'" "$pf"
done
expect "$tgs: still requires github.event_name == 'push' (a dispatch sends nothing)" grep -q -F "github.event_name == 'push'" "$tgs"
mutate dispatch-removed '$0 == "  workflow_dispatch:" { next } { print }' &&
  run "real with the '$wd' line deleted from ci.yml alone [R7]" 1 "$(at "$tmp/dispatch-removed/ci.yml" "$(first_hdiff "$tmp/dispatch-removed/ci.yml")" R7)" "$tmp/dispatch-removed"
mutate dispatch-doubled '{ print } $0 == "  pull_request:" { print "  workflow_dispatch:" }' &&
  run "real with a second '$wd' line added to ci.yml alone [R7]" 1 "$(at "$tmp/dispatch-doubled/ci.yml" "$(first_hdiff "$tmp/dispatch-doubled/ci.yml")" R7)" "$tmp/dispatch-doubled"
mutate dispatch-pr-target '{ print } $0 == "  pull_request:" { print "  pull_request_target:" }' &&
  run "real with pull_request_target added to ci.yml alone [R7]" 1 "$(at "$tmp/dispatch-pr-target/ci.yml" "$(first_hdiff "$tmp/dispatch-pr-target/ci.yml")" R7)" "$tmp/dispatch-pr-target"

# ---- T-206 (decisions #265, #266): the contour's owned scan-and-upload tail ----------------------------
# The tail of teamwright-agent.yml, from the step 'Stage the session for upload' to the end of the file, is
# a byte-identical copy of scripts/ci/contour-upload.yml (R9, keyed by the basename like R14). The copy is
# the developer's text, so the dirs below are built at run time from the static base ok-contour plus that
# copy, and every case that needs the copy says "missing scripts/ci/contour-upload.yml" when it is not
# there, never a bash error. No tracked file holds a string that matches a secret format: every fake
# value is assembled here from split literals (the self-test of the scan is the point of the task).
cu=scripts/ci/contour-upload.yml
cu_step='      - name: Stage the session for upload'
cu_need() { # <label>: 0 (and a FAIL) when the owned copy does not exist yet
  [ -f "$cu" ] && return 1
  echo "FAIL $1: missing $cu (the owned copy of the contour's scan and upload tail; the developer creates it)" >&2
  failed=$((failed + 1))
  return 0
}
cu_expect() { # <label> <expect args...>
  local l="$1"
  shift
  if [ ! -f "$cu" ]; then
    echo "FAIL $l: missing $cu (the owned copy of the contour's scan and upload tail; the developer creates it)" >&2
    failed=$((failed + 1))
  else
    expect "$l" "$@"
  fi
}
cu_tail() { awk -v t="$cu_step" '$0 == t { s = 1 } s { print }' "$cu"; }
agent_base() { awk -v t="$cu_step" '$0 == t { exit } { print }' "$1"; }

# ---- (b) the guard: R9 per name -------------------------------------------------------------------
# mk_ct <name> [filter...]: $tmp/<name> = ok-contour with its agent file's tail (the owned copy) passed
# through the filter, a command that reads the tail on stdin and writes the changed tail.
mk_ct() {
  local d="$tmp/$1"
  shift
  mkdir -p "$d" && cp -p "$fx/ok-contour"/* "$d/" || return 1
  agent_base "$fx/ok-contour/teamwright-agent.yml" > "$d/teamwright-agent.yml"
  if [ "$#" -gt 0 ]; then cu_tail | "$@" >> "$d/teamwright-agent.yml"; else cu_tail >> "$d/teamwright-agent.yml"; fi
}
# dcheck <label> <dir> [<allowed other rules>]: exit 1, R9 listed at least once and no other rule listed.
dcheck() {
  local label="$1" d="$2" extra=" ${3:-} " out got line rule n9=0 bad=""
  if [ ! -x "$guard" ]; then
    echo "FAIL $label: $guard not found or not executable" >&2
    failed=$((failed + 1))
    return
  fi
  out="$("$guard" "$d" 2>&1)"
  got=$?
  if [ "$got" -ne 1 ]; then
    echo "FAIL $label: exit $got, want 1 (an R9 refusal)" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
    return
  fi
  while IFS= read -r line; do
    [[ "$line" == "$d/"* ]] || continue
    rule="$(sed -E 's/^.*:[0-9]+: (R[0-9]+): .*$/\1/' <<<"$line")"
    if [ "$rule" = R9 ]; then n9=$((n9 + 1)); elif [[ "$extra" != *" $rule "* ]]; then bad="$bad $rule"; fi
  done <<<"$out"
  if [ "$n9" -eq 0 ] || [ -n "$bad" ]; then
    echo "FAIL $label: want R9 listed (listed $n9 times) and no other rule (unexpected:${bad:- none})" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
  else
    echo "ok   $label [R9]: exit 1"
    passed=$((passed + 1))
  fi
}
f_none() { cat > /dev/null; }
f_no_always() { awk '!d && $0 == "        if: always()" { d = 1; next } { print }'; }
f_up_no_always() { awk '!d && /^        if: always\(\) && / { sub(/always\(\) && /, ""); d = 1 } { print }'; }
f_ret90() { sed 's/^\( *retention-days:\) 3$/\1 90/'; }
f_hidden() { awk '{ print } /^ *retention-days: / { print "          include-hidden-files: true" }'; }
f_path_home() { sed 's|^          path: .*|          path: /root/.claude|'; }
f_no_scan() { grep -v 'grep'; }
f_drop_scan() { awk -v t="$cu_step" '$0 == t { s = 1; next } s && /^      - / { s = 0 } !s { print }'; }
f_key_appended() { cat; echo '          overwrite: true'; }
f_step_appended() { cat; printf '%s\n' '      - name: Extra' '        run: true'; }
f_comment_first() { echo '      # a second upload-artifact'; cat; }
f_twice() { local t; t="$(cat)"; printf '%s\n%s\n' "$t" "$t"; }
f_outside_job() { printf '%s\n' '  other:' '    runs-on: [self-hosted, Linux, X64, claude]' '    steps:'; cat; }
# vct <name> <allowed other rules> <filter...>: the dir with the changed tail is refused by R9.
vct() {
  local label="v-contour-upload-$1" extra="$2"
  shift 2
  cu_need "$label" && return
  if ! mk_ct "$label" "$@"; then
    echo "FAIL $label: cannot build the dir (fix the test)" >&2
    failed=$((failed + 1))
    return
  fi
  if cmp -s "$tmp/$label/teamwright-agent.yml" "$tmp/ok-contour-tail/teamwright-agent.yml"; then
    echo "FAIL $label: the mutation of the tail did not apply (fix the test or the copy)" >&2
    failed=$((failed + 1))
    return
  fi
  dcheck "$label" "$tmp/$label" "$extra"
}
if ! cu_need "ok-contour-tail"; then
  mk_ct ok-contour-tail
  run "ok-contour with the owned tail $cu (the guard's R9 per name)" 0 'ok:' "$tmp/ok-contour-tail"
  expect "ok-contour-tail: exactly two upload-artifact tokens in the dir (the installer upload and the tail)" test "$(tok_count "$tmp/ok-contour-tail")" = 2
fi
vct tail-missing "" f_none
vct no-always "" f_no_always
vct if-no-always "" f_up_no_always
vct retention-90 "" f_ret90
vct include-hidden-files "" f_hidden
vct path-home "" f_path_home
vct scan-removed "" f_no_scan
vct scan-step-removed "" f_drop_scan
vct key-appended "" f_key_appended
vct step-appended "" f_step_appended
vct token-in-comment-first "" f_comment_first
vct tail-twice "" f_twice
vct tail-outside-agent-job "" f_outside_job
# The tail in a job other than agent: (the up-walk from the copy's first line meets '  stage:').
if ! cu_need "v-contour-upload-job-renamed"; then
  mk_ct v-contour-upload-job-renamed && sed -i 's/^  agent:$/  stage:/' "$tmp/v-contour-upload-job-renamed/teamwright-agent.yml"
  if cmp -s "$tmp/v-contour-upload-job-renamed/teamwright-agent.yml" "$tmp/ok-contour-tail/teamwright-agent.yml"; then
    echo "FAIL v-contour-upload-job-renamed: the mutation did not apply (no '  agent:' line)" >&2
    failed=$((failed + 1))
  else
    dcheck v-contour-upload-job-renamed "$tmp/v-contour-upload-job-renamed"
  fi
fi
# Only the exact name carries the tail: it in teamwright-next.yml, with none in the agent file, is refused
# (R4 also lists the secret there, which the guard may report beside R9).
if ! cu_need "v-contour-upload-in-next-only"; then
  d="$tmp/v-contour-upload-in-next-only"
  mk_ct v-contour-upload-in-next-only f_none && cu_tail >> "$d/teamwright-next.yml"
  dcheck v-contour-upload-in-next-only "$d" "R4"
fi
# The installer upload of ci.yml is still required: the tail alone is one token, not two.
if ! cu_need "v-contour-upload-installer-upload-missing"; then
  d="$tmp/v-contour-upload-installer-upload-missing"
  mk_ct v-contour-upload-installer-upload-missing
  awk -v up="$upl" 'BEGIN { while ((getline l < up) > 0) U[++n] = l } { L[++m] = $0 }
    END { for (i = 1; i <= m; i++) { ok = 1; for (k = 1; k <= n; k++) if (L[i + k - 1] != U[k]) { ok = 0; break } if (ok) { s = i; break } }
          for (i = 1; i <= m; i++) if (!s || i < s || i > s + n - 2) print L[i] }' "$d/ci.yml" > "$d/ci.yml.new" && mv "$d/ci.yml.new" "$d/ci.yml"
  if cmp -s "$d/ci.yml" "$fx/ok-contour/ci.yml"; then
    echo "FAIL v-contour-upload-installer-upload-missing: the installer upload block was not found in ok-contour/ci.yml (fix the test)" >&2
    failed=$((failed + 1))
  else
    dcheck v-contour-upload-installer-upload-missing "$d"
  fi
fi

# ---- (b) pins of the owned copy and of the real files (c) -----------------------------------------
cu_expect "$cu starts with the step line '$cu_step'" test "$(head -n 1 "$cu")" = "$cu_step"
cu_expect "$cu: the scan step runs under if: always()" grep -q -x -F '        if: always()' "$cu"
cu_expect "$cu: the owned upload action line occurs once and no other line names upload-artifact" \
  bash -c '[ "$(grep -c -x -F "      - uses: actions/upload-artifact@v4" "$1")" = 1 ] && [ "$(grep -i -o -F upload-artifact "$1" | wc -l | tr -d " ")" = 1 ]' _ "$cu"
cu_expect "$cu: the upload runs whenever the staging step passed (if: always() && steps.staged.outcome == 'success')" \
  bash -c 'grep -q -x -F "        id: staged" "$1" && grep -q -x -F "        if: always() && steps.staged.outcome == '"'"'success'"'"'" "$1"' _ "$cu"
# Acceptance 1: success or failure of The stage (id claude) -> the upload still runs; no condition names it.
cu_expect "$cu: no condition names steps.claude (a failed stage still uploads)" \
  bash -c '! grep -q -F "steps.claude" "$1"' _ "$cu"
cu_expect "$cu: retention-days: 3 and if-no-files-found: error, no include-hidden-files, no overwrite" \
  bash -c 'grep -q -x -F "          retention-days: 3" "$1" && grep -q -x -F "          if-no-files-found: error" "$1" && ! grep -q -i -E "include-hidden-files|overwrite" "$1"' _ "$cu"
cu_expect "$cu: the artifact path is the staging dir session-upload, not a path under HOME" \
  bash -c 'l="$(grep -E "^          path:" "$1")"; [ "$(grep -c -E "^          path:" "$1")" = 1 ] && [[ "$l" == *session-upload* ]] && ! grep -q -E "\.claude|HOME" <<<"$l"' _ "$cu"
cu_expect "$cu: the artifact is named after the task, the run and the attempt" \
  bash -c 'l="$(grep -E "^          name:" "$1")"; [[ "$l" == *teamwright-agent-* && "$l" == *github.run_id* && "$l" == *github.run_attempt* && ( "$l" == *task_id* || "$l" == *TASK_ID* ) ]]' _ "$cu"
cu_expect "$cu: the scan step env holds the OAuth secret and the job token (the values pass 1 matches)" \
  bash -c 'grep -q -F "\${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}" "$1" && grep -q -F "\${{ github.token }}" "$1"' _ "$cu"
cu_expect "$cu: the four format patterns are written in the step text (decision 266)" \
  bash -c 'for p in "-----BEGIN [A-Z0-9 ]*PRIVATE KEY( BLOCK)?-----" "gh[pousr]_[A-Za-z0-9]{36,}" "github_pat_[A-Za-z0-9_]{20,}" "sk-ant-[A-Za-z0-9_-]{20,}"; do grep -q -F -e "$p" "$1" || exit 1; done' _ "$cu"
cu_expect "$cu: no set -x (a trace would print the values) and no RUNS_DIR" bash -c '! grep -q -E "set -[a-z]*x|RUNS_DIR" "$1"' _ "$cu"
cu_expect "real agent: from the line '$cu_step' to the end of the file it is $cu, byte for byte" \
  bash -c 'cmp -s <(awk -v t="$3" "\$0 == t { s = 1 } s { print }" "$1") "$2"' _ "$ag" "$cu" "$cu_step"
expect "real agent: the token upload-artifact occurs once (the tail), the installer upload stays in ci.yml" \
  test "$(grep -i -o -F upload-artifact "$ag" | wc -l | tr -d ' ')" = 1
expect "real agent: no RUNS_DIR (the host-only copy of the session is gone)" bash -c '! grep -q -F RUNS_DIR "$1"' _ "$ag"
expect "real agent: no step 'Keep the session for analysis' (it exited 0 having kept nothing)" bash -c '! grep -q -F "Keep the session for analysis" "$1"' _ "$ag"
expect "real agent: the tail steps come after 'Did the stage commit?' (the job token appears only there)" \
  bash -c 'c="$(grep -n -m 1 -x -F "      - name: Did the stage commit?" "$1" | cut -d: -f1)"; s="$(grep -n -m 1 -x -F "$2" "$1" | cut -d: -f1)"; [ -n "$c" ] && [ -n "$s" ] && [ "$s" -gt "$c" ]' _ "$ag" "$cu_step"

# ---- (a) the scan step, run: its run: body extracted from the owned copy and executed ---------------
# sc_prep reads the step 'Stage the session for upload' of the copy: its run: body, its env: (name: value
# lines) and its shell:. The env values are the fakes below: the expression of secrets.CLAUDE_CODE_OAUTH_TOKEN
# becomes C_OAUTH, that of github.token becomes C_JOB (both obvious fakes that match none of the four format
# patterns, so that only pass 1 can refuse them), any other expression a plain word. Without a shell: line the
# body runs as GitHub runs it by default, bash -e.
sc_state=""
sc_err=""
sc_prep() {
  if [ -z "$sc_state" ]; then
    sc_state=bad
    if [ ! -f "$cu" ]; then
      sc_err="missing $cu (the owned copy of the contour's scan and upload tail; the developer creates it)"
    else
      mkdir -p "$tmp/sc/shim"
      awk -v t="$cu_step" '
        $0 == t { s = 1; next }
        s && /^      - / { exit }
        s && !r { if ($0 ~ /^        run: \|[-+]?$/) r = 1; next }
        s && r { if ($0 ~ /^          /) print substr($0, 11); else if ($0 ~ /^ *$/) print ""; else exit }' "$cu" > "$tmp/sc/body.sh"
      awk -v t="$cu_step" '
        $0 == t { s = 1; next }
        s && /^      - / { exit }
        s && $0 == "        env:" { e = 1; next }
        s && e { if ($0 ~ /^          [A-Za-z_][A-Za-z0-9_]*:/) { sub(/^          /, ""); print } else if ($0 ~ /^ *#/ || $0 ~ /^ *$/) next; else e = 0 }' "$cu" > "$tmp/sc/env.txt"
      awk -v t="$cu_step" '
        $0 == t { s = 1; next }
        s && /^      - / { exit }
        s && /^        shell: / { sub(/^        shell: /, ""); print; exit }' "$cu" > "$tmp/sc/shell.txt"
      if [ ! -s "$tmp/sc/body.sh" ]; then
        sc_err="$cu: no step '${cu_step#      - name: }' with a 'run: |' body"
      elif ! grep -q -F 'secrets.CLAUDE_CODE_OAUTH_TOKEN' "$tmp/sc/env.txt"; then
        sc_err="$cu: the scan step's env: holds no \${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }} value"
      elif ! grep -q -F 'github.token' "$tmp/sc/env.txt"; then
        sc_err="$cu: the scan step's env: holds no \${{ github.token }} value"
      elif grep -q -F '${{' "$tmp/sc/body.sh"; then
        sc_err="$cu: the run: body holds a \${{ }} expression (values reach the scan only through the step env:)"
      else
        sc_state=ok
      fi
      cat > "$tmp/sc/shim/grep" <<'SHIM'
#!/usr/bin/env bash
# The scan with one pass removed: a grep that finds nothing when it is that pass (value pass: fixed strings
# read from a file; format pass: a pattern naming one of the four secret formats), the real grep otherwise.
a=""; hasF=0; hasf=0; hasc=0; pk=0
for a in "$@"; do
  case "$a" in
    --fixed-strings) hasF=1 ;;
    --file=*) hasf=1 ;;
    --count) hasc=1 ;;
    --*) ;;
    -[A-Za-z]*) f="${a#-}"; [[ "$f" == *F* ]] && hasF=1; [[ "$f" == *f* ]] && hasf=1; [[ "$f" == *c* ]] && hasc=1 ;;
  esac
  case "$a" in *"PRIVATE KEY"* | *"gh[pousr]"* | *github_pat_* | *sk-ant-*) pk=1 ;; esac
done
if [ "${SC_MUT:-}" = value ] && [ "$hasF" = 1 ] && [ "$hasf" = 1 ]; then [ "$hasc" = 1 ] && echo 0; exit 1; fi
if [ "${SC_MUT:-}" = format ] && [ "$pk" = 1 ]; then [ "$hasc" = 1 ] && echo 0; exit 1; fi
exec "$GREP_REAL" "$@"
SHIM
      chmod +x "$tmp/sc/shim/grep"
    fi
  fi
  [ "$sc_state" = ok ]
}
rep() { head -c "$2" /dev/zero | tr '\0' "$1"; }
C_WHY=""
SC_MUT=""
# sc_new <name> [<slug length>]: a workspace (a git repo with origin/main), a HOME, a RUNNER_TEMP and the three
# sources with a marker each, plus the things that must never be staged. With a length, GITHUB_WORKSPACE is a
# path whose slug (every char outside [A-Za-z0-9] becomes '-') has that many chars.
sc_new() {
  C="$tmp/sc-$1"
  rm -rf "$C"
  C_H="$C/home"
  C_RT="$C/rt"
  C_OUTSIDE="$C/outside"
  C_WS="$C/ws"
  [ -n "${2:-}" ] && C_WS="$C/ws/$(rep x $(($2 - ${#C} - 4)))"
  C_SLUG="$(printf '%s' "$C_WS" | tr -c 'A-Za-z0-9' '-')"
  C_ID=0a1b2c3d-1111-4222-8333-444455556666
  C_OTHER=9f8e7d6c-5555-4444-8333-222211110000
  C_PD="$C_H/.claude/projects/$C_SLUG"
  C_OAUTH="zzcanary-oauth-$RANDOM$RANDOM$RANDOM"
  C_JOB="zzcanary-job-$RANDOM$RANDOM$RANDOM"
  mkdir -p "$C_WS/.teamwright-agent" "$C_WS/.teamwright/logs" "$C_PD/$C_ID/subagents" "$C_PD/$C_OTHER/subagents" \
    "$C_PD-other" "$C_H/.claude/projects/-tmp-x" "$C_RT" "$C_OUTSIDE/dir" "$C/tmpdir"
  (cd "$C_WS" && env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE git init -q . &&
    env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE git -c user.name=t -c user.email=t@example.com -c commit.gpgsign=false commit -q --allow-empty -m x &&
    env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE git update-ref refs/remotes/origin/main HEAD) > /dev/null 2>&1
  sc_init "$C_ID" "$C_WS"
  echo MARK-session >> "$C_WS/.teamwright-agent/session.jsonl"
  echo MARK-preflight > "$C_WS/.teamwright-agent/preflight.md"
  echo MARK-tools > "$C_WS/.teamwright/logs/tools.jsonl"
  echo MARK-pj-main > "$C_PD/$C_ID.jsonl"
  echo MARK-pj-sub > "$C_PD/$C_ID/subagents/agent-a1.jsonl"
  echo MARK-pj-other > "$C_PD/$C_OTHER.jsonl"
  echo MARK-pj-other-sub > "$C_PD/$C_OTHER/subagents/agent-b1.jsonl"
  echo MARK-pj-sibling > "$C_PD-other/$C_ID.jsonl"
  echo MARK-pj-unrelated > "$C_H/.claude/projects/-tmp-x/$C_ID.jsonl"
  echo MARK-home-cred > "$C_H/.claude/.credentials.json"
  echo MARK-home-settings > "$C_H/.claude/settings.json"
  echo MARK-outside > "$C_OUTSIDE/file.txt"
  echo MARK-outside-dir > "$C_OUTSIDE/dir/inner.txt"
  ln -s "$C_OUTSIDE/file.txt" "$C_PD/link-pj.jsonl"
  ln -s "$C_OUTSIDE/file.txt" "$C_PD/$C_ID/subagents/link-sub.jsonl"
}
# sc_init <id> <cwd>: the first line of session.jsonl, the stream-json init event (session_id and cwd).
sc_init() {
  printf '{"type":"system","subtype":"init","cwd":"%s","session_id":"%s"}\n' "$2" "$1" > "$C_WS/.teamwright-agent/session.jsonl"
  echo MARK-session >> "$C_WS/.teamwright-agent/session.jsonl"
}
# sc_plant <where> <value>: the value as contents or as a file name in one of the sources.
sc_plant() {
  case "$1" in
    session) printf '%s\n' "$2" >> "$C_WS/.teamwright-agent/session.jsonl" ;;
    out) printf '%s\n' "$2" > "$C_WS/.teamwright-agent/extra.txt" ;;
    outname) : > "$C_WS/.teamwright-agent/n-$2.txt" ;;
    logs) printf '%s\n' "$2" >> "$C_WS/.teamwright/logs/tools.jsonl" ;;
    logname) : > "$C_WS/.teamwright/logs/n-$2.log" ;;
    pj) printf '%s\n' "$2" >> "$C_PD/$C_ID.jsonl" ;;
    sub) printf '%s\n' "$2" >> "$C_PD/$C_ID/subagents/agent-a1.jsonl" ;;
    pjname) : > "$C_PD/$C_ID/subagents/n-$2.jsonl" ;;
  esac
}
# sc_exec: run the extracted body in the workspace with a clean environment; C_RC is its exit code, $C/out.txt
# its stdout and stderr, C_STAGE the staging dir ($RUNNER_TEMP/session-upload).
sc_exec() {
  local line name val flags sh
  local -a args
  C_STAGE="$C_RT/session-upload"
  if ! sc_prep; then C_WHY="$sc_err"; return 1; fi
  args=(PATH="$PATH" HOME="$C_H" GITHUB_WORKSPACE="$C_WS" RUNNER_TEMP="$C_RT" TMPDIR="$C/tmpdir" LC_ALL=C
    GITHUB_OUTPUT="$C/gh_output" GITHUB_ENV="$C/gh_env" GITHUB_STEP_SUMMARY="$C/gh_summary" GITHUB_RUN_ATTEMPT=1
    OUT=.teamwright-agent TASK_ID=T-999 STAGE=analyze RUN_ID=1 HOP=1 MAIN=main REPO=example/repo ISSUE= RUN_URL=https://example.com/run)
  while IFS= read -r line; do
    name="${line%%:*}"
    val="${line#*:}"
    val="${val# }"
    val="${val#\"}"
    val="${val%\"}"
    case "$val" in
      *secrets.CLAUDE_CODE_OAUTH_TOKEN*) val="$C_OAUTH" ;;
      *github.token*) val="$C_JOB" ;;
      *'${{'*) val="$(sed -e 's/\${{ *steps\.[^}]*}}/success/g' -e 's/\${{ *github\.workspace *}}/'"${C_WS//\//\\/}"'/g' -e 's/\${{ *github\.run_id *}}/1/g' -e 's/\${{[^}]*}}/x/g' <<<"$val")" ;;
    esac
    args+=("$name=$val")
  done < "$tmp/sc/env.txt"
  if [ -n "$SC_MUT" ]; then
    args=("${args[@]}" "SC_MUT=$SC_MUT" "GREP_REAL=$(command -v grep)")
    args[0]="PATH=$tmp/sc/shim:$PATH"
  fi
  sh="$(cat "$tmp/sc/shell.txt")"
  if [ -z "$sh" ]; then flags="-e"
  elif [ "$sh" = bash ]; then flags="--noprofile --norc -eo pipefail"
  else flags="${sh#bash}"; flags="${flags//\{0\}/}"; fi
  (cd "$C_WS" && env -i "${args[@]}" timeout 120 bash $flags "$tmp/sc/body.sh") > "$C/out.txt" 2>&1
  C_RC=$?
  return 0
}
sc_staged_has() { grep -r -q -F -- "$1" "$C_STAGE" 2>/dev/null; }
# sc_no_leak: neither fake secret value in the step's output or in the runner's files.
sc_no_leak() {
  local f v
  for f in out.txt gh_output gh_env gh_summary; do
    [ -e "$C/$f" ] || continue
    for v in "$C_OAUTH" "$C_JOB"; do
      [ -n "$v" ] || continue
      if grep -q -F -- "$v" "$C/$f"; then C_WHY="a secret value of the job is in $f"; return 1; fi
    done
  done
  return 0
}
sc_check() { # <label> <fn> [args...]: a test function returns 0 or sets C_WHY and returns 1
  local label="$1"
  shift
  C_WHY=""
  if "$@"; then
    echo "ok   $label"
    passed=$((passed + 1))
    return 0
  fi
  echo "FAIL $label: $C_WHY" >&2
  failed=$((failed + 1))
  return 1
}
declare -A sc_pass=()
# sc_mut_check <label> <mutation> <fn> [args...]: with one pass of the scan removed (the grep shim), the case
# that guards that pass must come out red; the case is only judged when it is green unmutated.
sc_mut_check() {
  local label="$1" mut="$2" id="$3"
  shift 2
  if [ "${sc_pass[$id]:-0}" != 1 ]; then
    echo "FAIL mutated ($mut pass removed) $label: the unmutated case is not green, so the mutation cannot be judged" >&2
    failed=$((failed + 1))
    return
  fi
  shift
  SC_MUT="$mut"
  C_WHY=""
  if "$@"; then
    SC_MUT=""
    echo "FAIL mutated ($mut pass removed) $label: the case stays green, the scan line is not pinned by it" >&2
    failed=$((failed + 1))
  else
    SC_MUT=""
    echo "ok   mutated ($mut pass removed) $label: red as wanted ($C_WHY)"
    passed=$((passed + 1))
  fi
}

t_canary() { # <oauth|job> <where>: the job's own secret value in a source: refused, nothing left, nothing printed
  local v
  sc_new "canary-$1-$2"
  if [ "$1" = oauth ]; then v="$C_OAUTH"; else v="$C_JOB"; fi
  sc_plant "$2" "$v"
  sc_exec || return 1
  [ "$C_RC" = 1 ] || { C_WHY="exit $C_RC, want 1 (a refusal)"; return 1; }
  [ ! -e "$C_STAGE" ] || { C_WHY="the staging dir is still there after a refusal"; return 1; }
  sc_no_leak
}
belt_val() { # <kind>: a fake of one secret format, assembled from split literals
  case "$1" in
    ghp | ghs | gho | ghu | ghr) printf 'gh%s_%s' "${1#gh}" "$(rep A 36)" ;;
    ghp35) printf 'gh%s_%s' p "$(rep A 35)" ;;
    ghx) printf 'gh%s_%s' x "$(rep A 40)" ;;
    pat) printf 'github_%s_%s' pat A1b2C3d4E5f6G7h8I9j0K1 ;;
    pat19) printf 'github_%s_%s' pat A1b2C3d4E5f6G7h8I9j ;;
    ant) printf 'sk%sant%s%s' - - api03_ABCDEFGH-ijklmnopqrstuv ;;
    ant19) printf 'sk%sant%s%s' - - abcdefghijklmnopqrs ;;
    key-rsa) printf '%s%s' '-----BEG' 'IN RSA PRIVATE KEY-----' ;;
    key-plain) printf '%s%s' '-----BEG' 'IN PRIVATE KEY-----' ;;
    key-ssh) printf '%s%s' '-----BEG' 'IN OPENSSH PRIVATE KEY-----' ;;
    key-block) printf '%s%s' '-----BEG' 'IN PGP PRIVATE KEY BLOCK-----' ;;
    key-four-dashes) printf '%s%s' '----BEG' 'IN PRIVATE KEY-----' ;;
    key-public) printf '%s%s' '-----BEG' 'IN PUBLIC KEY-----' ;;
  esac
}
t_belt() { # <kind> <where>: a value of a known secret format, not the job's own: refused, only a count printed
  local v
  v="$(belt_val "$1")"
  sc_new "belt-$1-$2"
  sc_plant "$2" "$v"
  sc_exec || return 1
  [ "$C_RC" = 1 ] || { C_WHY="exit $C_RC, want 1 (a refusal)"; return 1; }
  [ ! -e "$C_STAGE" ] || { C_WHY="the staging dir is still there after a refusal"; return 1; }
  ! grep -q -F -- "$v" "$C/out.txt" || { C_WHY="the matching value is in the output"; return 1; }
  ! grep -q -F -- "${v:5:20}" "$C/out.txt" || { C_WHY="a part of the matching value or file name is in the output"; return 1; }
  grep -q '[0-9]' "$C/out.txt" || { C_WHY="no count printed"; return 1; }
}
t_near() { # <kind>: a near miss of a format (one char short, another prefix): not a match, the run goes on
  sc_new "near-$1"
  sc_plant logs "$(belt_val "$1")"
  sc_exec || return 1
  [ "$C_RC" = 0 ] || { C_WHY="exit $C_RC, want 0 (not a secret format)"; return 1; }
  [ -d "$C_STAGE" ] || { C_WHY="no staging dir"; return 1; }
}
t_clean() {
  local m f
  sc_new clean
  sc_exec || return 1
  [ "$C_RC" = 0 ] || { C_WHY="exit $C_RC, want 0 for a clean tree"; return 1; }
  [ -d "$C_STAGE" ] || { C_WHY="no staging dir $C_STAGE"; return 1; }
  [ -f "$C_STAGE/manifest.txt" ] || { C_WHY="no manifest.txt in the staging dir"; return 1; }
  [ -f "$C_STAGE/commits.txt" ] || { C_WHY="no commits.txt in the staging dir"; return 1; }
  for m in MARK-session MARK-preflight MARK-tools MARK-pj-main MARK-pj-sub; do
    sc_staged_has "$m" || { C_WHY="$m was not staged"; return 1; }
  done
  [ -z "$(find "$C_STAGE" -mindepth 1 \( -name '.*' -o -type l \) | head -n 1)" ] || { C_WHY="a staged name starts with a dot (upload-artifact skips hidden files) or a symlink was staged"; return 1; }
  [ -z "$(find "$C_STAGE" -mindepth 1 ! -type f ! -type d | head -n 1)" ] || { C_WHY="a staged entry is not a regular file"; return 1; }
  ! grep -q -F '::warning' "$C/out.txt" || { C_WHY="a ::warning with nothing missing"; return 1; }
  sc_no_leak
}
t_symlink() {
  local m
  sc_new symlink
  ln -s "$C_OUTSIDE/file.txt" "$C_WS/.teamwright-agent/link-file.txt"
  ln -s "$C_OUTSIDE/dir" "$C_WS/.teamwright-agent/link-dir"
  ln -s "$C_OUTSIDE/file.txt" "$C_WS/.teamwright/logs/link-log.txt"
  sc_exec || return 1
  [ "$C_RC" = 0 ] || { C_WHY="exit $C_RC, want 0"; return 1; }
  for m in MARK-outside MARK-outside-dir MARK-outside-dir; do
    ! sc_staged_has "$m" || { C_WHY="the content behind a symlink ($m) was staged"; return 1; }
  done
  [ -z "$(find "$C_STAGE" -name 'link-*' | head -n 1)" ] || { C_WHY="a symlink was staged"; return 1; }
  sc_staged_has MARK-session && sc_staged_has MARK-tools || { C_WHY="the regular files beside the symlinks were not staged"; return 1; }
}
t_out_missing() {
  sc_new out-missing
  rm -rf "$C_WS/.teamwright-agent"
  sc_exec || return 1
  [ "$C_RC" = 0 ] || { C_WHY="exit $C_RC, want 0"; return 1; }
  grep -F -- '.teamwright-agent' "$C_STAGE/manifest.txt" 2>/dev/null | grep -q -i -E 'miss|absent|not |no |none|skip|lack' || { C_WHY="manifest.txt does not name the missing .teamwright-agent"; return 1; }
  grep -q -F '::warning' "$C/out.txt" || { C_WHY="no ::warning for the missing source"; return 1; }
  sc_staged_has MARK-tools || { C_WHY="the journal was not staged"; return 1; }
  ! sc_staged_has MARK-pj-main || { C_WHY="the project dir was staged without a session id"; return 1; }
}
t_logs_missing() {
  sc_new logs-missing
  rm -rf "$C_WS/.teamwright"
  sc_exec || return 1
  [ "$C_RC" = 0 ] || { C_WHY="exit $C_RC, want 0"; return 1; }
  grep -F -- '.teamwright/logs' "$C_STAGE/manifest.txt" 2>/dev/null | grep -q -i -E 'miss|absent|not |no |none|skip|lack' || { C_WHY="manifest.txt does not name the missing .teamwright/logs"; return 1; }
  grep -q -F '::warning' "$C/out.txt" || { C_WHY="no ::warning for the missing source"; return 1; }
  sc_staged_has MARK-session || { C_WHY="the session was not staged"; return 1; }
}
t_empty_oauth() { # an empty secret value is dropped, not matched against everything
  sc_new empty-oauth
  C_OAUTH=""
  sc_exec || return 1
  [ "$C_RC" = 0 ] || { C_WHY="exit $C_RC, want 0 (an empty value must be dropped, not matched)"; return 1; }
  sc_staged_has MARK-session || { C_WHY="nothing staged"; return 1; }
}
t_empty_job() { # an empty job token: the scan cannot cover it, the step refuses
  sc_new empty-job
  C_JOB=""
  sc_exec || return 1
  [ "$C_RC" = 1 ] || { C_WHY="exit $C_RC, want 1 (an empty job token is refused)"; return 1; }
  [ ! -e "$C_STAGE" ] || { C_WHY="the staging dir is still there"; return 1; }
}
t_pd_ok() { # [<slug length>]: the project dir of this checkout, this run's session only
  local m
  sc_new "pd-ok${1:-}" "${1:-}"
  sc_exec || return 1
  [ "$C_RC" = 0 ] || { C_WHY="exit $C_RC, want 0"; return 1; }
  sc_staged_has MARK-pj-main || { C_WHY="<slug>/<id>.jsonl was not staged"; return 1; }
  sc_staged_has MARK-pj-sub || { C_WHY="<slug>/<id>/subagents/ was not staged"; return 1; }
  for m in MARK-pj-other MARK-pj-other-sub MARK-pj-sibling MARK-pj-unrelated MARK-home-cred MARK-home-settings MARK-outside; do
    ! sc_staged_has "$m" || { C_WHY="$m was staged (another session, a sibling or foreign dir, a symlink target or a file of ~/.claude)"; return 1; }
  done
  [ -z "$(find "$C_STAGE" -name 'link-*' | head -n 1)" ] || { C_WHY="a symlink inside the project dir was staged"; return 1; }
  ! grep -q -F '::warning' "$C/out.txt" || { C_WHY="a ::warning with nothing missing"; return 1; }
}
t_pd_miss() { # <variant> [<slug length>]: no project dir is staged; the miss is in manifest.txt and a ::warning
  local m
  sc_new "pd-$1" "${2:-}"
  mkdir -p "$C_H/.claude/projects"
  echo MARK-trav > "$C_H/.claude/projects/x.jsonl"
  case "$1" in
    symdir) mv "$C_PD" "$C/realpd" && ln -s "$C/realpd" "$C_PD" ;;
    nodir) rm -rf "$C_PD" ;;
    badid-word) sc_init "session-1" "$C_WS" ;;
    badid-upper) sc_init "0A1B2C3D-1111-4222-8333-444455556666" "$C_WS" ;;
    badid-trail) sc_init "${C_ID}x" "$C_WS" ;;
    badid-trav) sc_init "../x" "$C_WS" ;;
    badid-glob) sc_init "*" "$C_WS" ;;
    cwd) sc_init "$C_ID" "/somewhere/else" ;;
    long) ;;
  esac
  sc_exec || return 1
  [ "$C_RC" = 0 ] || { C_WHY="exit $C_RC, want 0 (a miss is not a refusal)"; return 1; }
  for m in MARK-pj-main MARK-pj-sub MARK-pj-other MARK-pj-other-sub MARK-pj-sibling MARK-pj-unrelated MARK-home-cred MARK-home-settings MARK-trav; do
    ! sc_staged_has "$m" || { C_WHY="$m was staged from HOME"; return 1; }
  done
  sc_staged_has MARK-session || { C_WHY="the session dir was not staged: the miss must not stop the rest"; return 1; }
  grep -i -E 'claude|project|session' "$C_STAGE/manifest.txt" 2>/dev/null | grep -q -i -E 'miss|absent|not |no |none|skip|lack|refus|reject|invalid' || { C_WHY="manifest.txt does not name the miss"; return 1; }
  grep -q -F '::warning' "$C/out.txt" || { C_WHY="no ::warning for the miss"; return 1; }
}

sc_run() { # <label> <fn> [args...]: the case, then its result is remembered for the mutation runs
  local label="$1"
  shift
  if sc_check "scan: $label" "$@"; then sc_pass["$label"]=1; else sc_pass["$label"]=0; fi
}
canary_specs=("oauth session" "oauth outname" "oauth logs" "oauth logname" "oauth pj" "oauth sub" "oauth pjname" "job session" "job outname" "job logs" "job pj")
belt_kinds=(ghp ghs gho ghu ghr pat ant key-rsa key-plain key-ssh key-block)
for spec in "${canary_specs[@]}"; do
  set -- $spec
  sc_run "canary $1 in $2 refuses the upload (exit 1, no staging dir, no value in the output)" t_canary "$1" "$2"
done
for k in "${belt_kinds[@]}"; do
  for w in out outname; do
    sc_run "format $k in $w refuses the upload (exit 1, no staging dir, only a count)" t_belt "$k" "$w"
  done
done
for k in ghp ant key-rsa; do
  for w in session logs logname pj sub pjname; do
    sc_run "format $k in $w refuses the upload" t_belt "$k" "$w"
  done
done
for k in ghp35 ghx pat19 ant19 key-four-dashes key-public; do
  sc_run "near miss $k is not a secret format (exit 0)" t_near "$k"
done
sc_run "clean tree: exit 0, manifest.txt, commits.txt, the three sources staged, no dot name, no symlink, no output of a value" t_clean
sc_run "a symlink in the sources is not copied or followed" t_symlink
sc_run "missing .teamwright-agent: exit 0, named in manifest.txt, a ::warning" t_out_missing
sc_run "missing .teamwright/logs: exit 0, named in manifest.txt, a ::warning" t_logs_missing
sc_run "an empty OAuth value is dropped (an empty pattern would match every file)" t_empty_oauth
sc_run "an empty job token value is refused (exit 1)" t_empty_job
sc_run "project dir: <slug>/<id>.jsonl and <id>/subagents copied; other session, sibling and foreign dirs, symlinks, ~/.claude files not" t_pd_ok
sc_run "project dir with a slug of exactly 200 chars is copied" t_pd_ok 200
for v in symdir nodir badid-word badid-upper badid-trail badid-trav badid-glob cwd; do
  sc_run "project dir miss ($v): exit 0, nothing from HOME staged, miss in manifest.txt and a ::warning" t_pd_miss "$v"
done
sc_run "project dir miss (slug of 201 chars): exit 0, nothing from HOME staged, miss in manifest.txt and a ::warning" t_pd_miss long 201

# The self-test of the scan: the same cases against the step with one pass removed must be red.
for spec in "${canary_specs[@]}"; do
  set -- $spec
  sc_mut_check "canary $1 in $2" value "canary $1 in $2 refuses the upload (exit 1, no staging dir, no value in the output)" t_canary "$1" "$2"
done
for k in "${belt_kinds[@]}"; do
  for w in out outname; do
    sc_mut_check "format $k in $w" format "format $k in $w refuses the upload (exit 1, no staging dir, only a count)" t_belt "$k" "$w"
  done
done
for k in ghp ant key-rsa; do
  for w in session logs logname pj sub pjname; do
    sc_mut_check "format $k in $w" format "format $k in $w refuses the upload" t_belt "$k" "$w"
  done
done

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
  # Review 1 finding 1: one step (line 1 the only '      - ' line of the body, one uses line, the
  # uses line one owned action at the step's key column), and per kind the pinned lines at fixed
  # positions, so no scalar opened above can hold them and no value continues below them.
  expect "fragment $fr: one step line" test "$(grep -c '^      - ' <<<"$body")" = 1
  expect "fragment $fr: one line holds the word uses" test "$(grep -c -i -E '(^|[^A-Za-z0-9_])uses([^A-Za-z0-9_]|$)' <<<"$body")" = 1
  expect "fragment $fr: the uses line is an owned action at the step's key column" \
    bash -c 'grep -i -E "(^|[^A-Za-z0-9_])uses([^A-Za-z0-9_]|\$)" <<<"$1" | grep -q -x -E "      (- |  )uses: (actions/setup-node|Swatinem/rust-cache|actions/cache/save|pnpm/action-setup)@[A-Za-z0-9._-]+"' _ "$body"
  l1="$(sed -n 1p "$fr")"; l2="$(sed -n 2p "$fr")"; l3="$(sed -n 3p "$fr")"; l4="$(sed -n 4p "$fr")"
  case "$(frag_kind "$fr")" in
    setup-node)
      expect "setup-node fragment $fr: line 1 the uses line, line 2 with:, line 3 package-manager-cache: false" \
        test "${l1:0:14}|$l2|$l3" = "      - uses: |        with:|          package-manager-cache: false"
      expect "setup-node fragment $fr: line 4 not blank, at most 10 leading spaces" bash -c '[[ "$1" =~ ^\ {0,10}[^\ ] ]]' _ "$l4"
      expect "setup-node fragment $fr: one with line" test "$(grep -c -i -E '(^|[^A-Za-z0-9_])with([^A-Za-z0-9_]|$)' <<<"$body")" = 1
      expect "setup-node fragment $fr: the raw cache on no other line" test "$(grep -c -i 'cache' <<<"$body")" = 1 ;;
    rust-cache)
      expect "rust-cache fragment $fr: line 1 the uses line, line 2 with:, line 3 save-if on main" \
        test "${l1:0:14}|$l2|$l3" = "      - uses: |        with:|          save-if: \${{ github.ref == 'refs/heads/main' }}"
      expect "rust-cache fragment $fr: line 4 not blank, at most 10 leading spaces" bash -c '[[ "$1" =~ ^\ {0,10}[^\ ] ]]' _ "$l4"
      expect "rust-cache fragment $fr: one with line" test "$(grep -c -i -E '(^|[^A-Za-z0-9_])with([^A-Za-z0-9_]|$)' <<<"$body")" = 1
      expect "rust-cache fragment $fr: one save-if line" test "$(grep -c -i 'save-if' <<<"$body")" = 1 ;;
    cache/save)
      expect "cache/save fragment $fr: line 1 a plain name" grep -q -x -E '      - name: [A-Za-z0-9 ()._-]+' <<<"$l1"
      expect "cache/save fragment $fr: line 2 the main if:, one \${{ }} ending the line" \
        bash -c '[[ "$1" == "        if: \${{ github.ref == '"'"'refs/heads/main'"'"' && "*"}}" ]] && [ "$(grep -o -F "\${{" <<<"$1" | wc -l)" -eq 1 ] && [ "$(grep -o -F "}}" <<<"$1" | wc -l)" -eq 1 ]' _ "$l2"
      expect "cache/save fragment $fr: line 3 not blank, at most 8 leading spaces" bash -c '[[ "$1" =~ ^\ {0,8}[^\ ] ]]' _ "$l3"
      expect "cache/save fragment $fr: one if line" test "$(grep -c -i -E '(^|[^A-Za-z0-9_])if([^A-Za-z0-9_]|$)' <<<"$body")" = 1
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
# frag_build <name> <fragment basename> <sed script> [<env block>]: the fragment edited by the sed
# script in the guard's copy AND inserted at the end of the gate job of the workflow copy, so the
# edited fragment occurs byte for byte and only the guard's check of its own copy can refuse it
# (the copy and ci.yml loosened together). A fragment ending on a job key line `  windows:` is
# completed by ok-release's own `  windows:` line; any other is followed by a run: line. An empty
# sed script leaves the fragment as it is. The env block (T-088 r3, decision #128), when given, is
# inserted as the gate job's env: right under its runs-on: line, outside every fragment: the place
# an anchor (`&c cache`) sits for an alias key inside the fragment (review 2 finding 1, N1-N4).
frag_build() {
  local n="$1" base="$2" sc="$3" envb="${4:-}" fr t e c nx
  r11_guard "$n" || return 1
  fr="$tmp/$n/g/cache-steps/$base"
  if [ ! -f "$csd/$base" ]; then
    echo "FAIL $n: $csd/$base not found (fix the test with the owned fragments)" >&2
    failed=$((failed + 1))
    return 1
  fi
  if [ -n "$sc" ]; then
    sed -e "$sc" "$csd/$base" > "$fr"
    if cmp -s "$csd/$base" "$fr"; then
      echo "FAIL $n: the edit did not apply to $base (fix the test)" >&2
      failed=$((failed + 1))
      return 1
    fi
  fi
  t="$tmp/$n/ins"
  e="$tmp/$n/env"
  if [ -n "$envb" ]; then printf '    env:\n%s\n' "$envb" > "$e"; else : > "$e"; fi
  # A fragment whose last line is the first line of another owned fragment (pnpm/action-setup
  # ends on the windows setup-node line) is completed by that fragment, so its end line is covered
  # as in the real ci.yml and only the self-check can refuse the case.
  nx=""
  for c in "$csd"/*; do
    [ "$(basename "$c")" != "$base" ] && [ "$(head -n 1 "$c")" = "$(tail -n 1 "$fr")" ] && nx="$c"
  done
  if [ "$(tail -n 1 "$fr")" = "  windows:" ]; then sed '$d' "$fr" > "$t"
  elif [ -n "$nx" ]; then { sed '$d' "$fr"; cat "$nx"; echo "        run: echo next"; } > "$t"
  else { cat "$fr"; echo "        run: echo next"; } > "$t"; fi
  if [ "$(sed -n 29p "$owned_src")" != "    runs-on: ubuntu-24.04" ] || [ "$(sed -n 32p "$owned_src")" != "      - run: make check" ]; then
    echo "FAIL $n: ok-release gate job moved (lines 29 and 32; fix the test)" >&2
    failed=$((failed + 1))
    return 1
  fi
  awk -v t="$t" -v e="$e" '{ print }
    NR == 29 { while ((getline l < e) > 0) print l }
    NR == 32 { while ((getline l < t) > 0) print l }' "$owned_src" > "$tmp/$n/wf/ci.yml"
}
# frag_case <name> <fragment basename> <label> <sed script> [<env block>]: frag_build, then exit 3
# naming the fragment.
frag_case() {
  frag_build "$1" "$2" "$4" "${5:-}" || return
  r11_cannot "$1" "$3" "cannot run.*$(ere "$2")"
}
# frag_ok <name> <fragment basename> <label> <sed script> [<env block>]: frag_build, then exit 0
# (an edit the self-check must not refuse: no over-reach).
frag_ok() {
  frag_build "$1" "$2" "$4" "${5:-}" || return
  guard="$tmp/$1/g/release-guard.sh"
  run "$3" 0 'ok:' "$tmp/$1/wf"
  guard="$real_guard"
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
# T-088 review 1 finding 1 (probes A-G, /root/.claude/jobs/97682091/tmp/t088-review/probe.py): the
# fragment and the workflow edited together; each passed the line-presence checks of round 1.
# One step per fragment: line 1 is the only `      - ` line of the body, one line holds uses.
frag_case g-frag-second-step-actions-cache windows-rust-cache.yml "cannot run: rust-cache fragment with a second step actions/cache (probe A: saves on every ref)" \
  '$i\      - uses: actions/cache@v4\n        with:\n          path: x\n          key: y'
frag_case g-frag-second-step-rust-cache windows-pnpm-save.yml "cannot run: cache/save fragment with a second step rust-cache (probe D: default save-if true)" \
  '$i\      - uses: Swatinem/rust-cache@v2'
frag_case g-frag-second-step-setup-node windows-pnpm-action-setup.yml "cannot run: pnpm/action-setup fragment with a second step setup-node without package-manager-cache (probe E: auto-cache)" \
  '$c\      - uses: actions/setup-node@v5\n        with:\n          node-version: "20.19"\n      - name: Path of the pnpm store'
frag_case g-frag-second-step-run windows-rust-cache.yml "cannot run: a second step line inside a fragment (one step per fragment)" \
  '$i\      - run: echo second'
frag_case g-frag-uses-other-action gate-setup-node.yml "cannot run: a fragment whose uses line runs an unlisted action naming setup-node in a comment" \
  '1s/^.*$/      - uses: someone\/saver@v1 # setup-node/'
# The pinned with: key right under `        with:` (probes F, G: moved to env:, the input unset).
frag_case g-frag-sn-pmc-under-env gate-setup-node.yml "cannot run: setup-node fragment with package-manager-cache: false under env:, not with: (probe F)" \
  '/^          package-manager-cache: false$/i\        env:'
frag_case g-frag-rust-save-if-under-env windows-rust-cache.yml "cannot run: rust-cache fragment with save-if under env:, not with: (probe G)" \
  's/^        with:$/        env:/'
frag_case g-frag-sn-second-with gate-setup-node.yml "cannot run: setup-node fragment with a second with: (the last one wins, package-manager-cache unset)" \
  '$i\        with:\n          node-version: "20"'
# The pinned line held by a multi-line scalar opened on the line above, or continued on the line
# below (PyYAML 6.0.1: x = "y package-manager-cache: false z"; save-if = "${{ ... }} || true").
frag_case g-frag-sn-pmc-in-quoted-scalar gate-setup-node.yml "cannot run: setup-node fragment whose package-manager-cache line is inside a double-quoted scalar" \
  '/^          package-manager-cache: false$/{
i\          x: "y
a\          z"
}'
frag_case g-frag-rust-save-if-continued windows-rust-cache.yml "cannot run: rust-cache fragment whose save-if value continues on the next line" \
  '/^          save-if: /a\            || true'
# The cache/save step-level if: (probes B, C): at the step's key column, one ${{ }} that ends the line.
frag_case g-frag-sn-name-quote gate-setup-node.yml "cannot run: setup-node fragment whose line 1 is a name opening a double-quoted scalar over with: and package-manager-cache (the uses line below)" \
  '1c\      - name: "x\n        with:\n          package-manager-cache: false\n        y"\n        uses: actions/setup-node@v5
2,4d'
frag_case g-frag-save-if-under-with windows-pnpm-save.yml "cannot run: cache/save fragment with its if: under with: (probe B: no step-level if)" \
  '/^        if: /{
h
d
}
/^        with:$/{
G
s/\n        if: /\n          if: /
}'
frag_case g-frag-save-if-string windows-pnpm-save.yml "cannot run: cache/save fragment whose if: has text outside \${{ }} (probe C: always true)" \
  's/^\(        if: .*\) }}$/\1 }} \&\& ${{ true }}/'
frag_case g-frag-save-if-continued windows-pnpm-save.yml "cannot run: cache/save fragment whose if: value continues on the next line (text outside \${{ }})" \
  '/^        if: /a\          ${{ true }}'
frag_case g-frag-save-if-indented windows-pnpm-save.yml "cannot run: cache/save fragment whose if: line is indented past the step's key column (the main prefix only as a substring)" \
  's/^        if: /          if: /'
frag_case g-frag-save-name-quote windows-pnpm-save.yml "cannot run: cache/save fragment whose name opens a double-quoted scalar holding the if: line" \
  '1s/name: /name: "/
/^        if: /a\        z"'

# T-088 r3 (decision #128, review 2 finding 1, probes N1-N4 in reviews/2.md): a YAML alias used as
# a key spells cache, with, if or save-if with none of those words on the fragment's line, so the
# self-check's word counts (premises R6, R8) do not see it; the anchor sits in ci.yml outside every
# fragment (the gate job's env:). No alias in a fragment: the raw `*` in a fragment body is exit 3.
# No anchor in a fragment: the raw `&` outside `&&` in a fragment body is exit 3. `&&` (the real
# cache/save if: lines) stays accepted.
frag_case g-frag-alias-sn-cache-pnpm gate-setup-node.yml "cannot run: setup-node fragment with an alias key *c : pnpm, anchor &c cache outside the fragment (N1: with.cache = pnpm)" \
  '/^          node-version: "20.19"$/a\          *c : pnpm' \
  '      VOICEN_K: &c cache'
frag_case g-frag-alias-pnpm-cache windows-pnpm-action-setup.yml "cannot run: pnpm/action-setup fragment with with: and an alias key *c : true, anchor &c cache outside (N2: with.cache = true)" \
  '1a\        with:\n          *c : true' \
  '      VOICEN_K: &c cache'
frag_case g-frag-alias-rust-save-if windows-rust-cache.yml "cannot run: rust-cache fragment with an alias key *s : true after save-if, anchor &s save-if outside (N3: the last save-if wins)" \
  '/^          save-if: /a\          *s : true' \
  '      VOICEN_K: &s save-if'
frag_case g-frag-alias-save-if windows-pnpm-save.yml "cannot run: cache/save fragment with an alias key *i : \${{ always() }}, anchor &i if outside (N4: the last if wins)" \
  '/^        uses: actions\/cache\/save@/a\        *i : ${{ always() }}' \
  '      VOICEN_K: &i if'
frag_case g-frag-alias-value gate-setup-node.yml "cannot run: setup-node fragment with an alias as a value (node-version: *v, anchor &v outside; no alias in a fragment at all)" \
  's/^          node-version: "20.19"$/          node-version: *v/' \
  '      VOICEN_V: &v "20.19"'
frag_case g-frag-anchor-sn gate-setup-node.yml "cannot run: setup-node fragment holding an anchor (node-version: &v ...)" \
  's/^          node-version: "20.19"$/          node-version: \&v "20.19"/'
frag_case g-frag-anchor-save-beside-and gate-cargo-save.yml "cannot run: cache/save fragment holding an anchor (key: &k ...) beside the && of its if: line" \
  's/^          key: /          key: \&k /'
frag_ok g-frag-ok-save-if-second-and windows-pnpm-save.yml "ok: cache/save fragment whose if: holds a second && (&& is not an anchor)" \
  "2s/ }}\$/ \\&\\& github.event_name == 'push' }}/"
frag_ok g-frag-ok-anchor-alias-outside gate-setup-node.yml "ok: an anchor and an alias in ci.yml outside every fragment, fragments as owned (the rule reads fragments only)" \
  '' \
  '      VOICEN_V: &v "20.19"
      VOICEN_W: *v'

# T-202: the owned head copies (R14) are guard inputs too: missing, empty, not ending in their only
# jobs: line or holding no permissions: line is never a pass. cguard <name> builds the guard next to
# its owned copies and a contour-heads dir made from the heads of ok-contour (the real copies are the
# developer's; fixtures and copies are checked against each other above).
cguard() {
  mkdir -p "$tmp/$1/contour-heads"
  if ! cp -p "$real_guard" scripts/ci/release-job.yml scripts/ci/release-header.yml "$upl" "$tgs" "$alines" "$tmp/$1/" ||
     ! cp -pR "$csd" "$tmp/$1/" || { [ -f "$cu" ] && ! cp -p "$cu" "$tmp/$1/"; }; then
    echo "FAIL $1: cannot copy the guard and its owned copies (fix the test)" >&2
    failed=$((failed + 1))
    return 1
  fi
  head_lines "$fx/ok-contour/teamwright-agent.yml" > "$tmp/$1/contour-heads/teamwright-agent.yml"
  head_lines "$fx/ok-contour/teamwright-next.yml" > "$tmp/$1/contour-heads/teamwright-next.yml"
}
# ccan <name> <label> <ERE>: the guard copy of $tmp/<name> on ok-contour is exit 3 naming contour-heads.
ccan() {
  guard="$tmp/$1/release-guard.sh"
  run "$2" 3 "$3" "$fx/ok-contour"
  guard="$real_guard"
}
if cguard g-contour-ok; then
  guard="$tmp/g-contour-ok/release-guard.sh"
  if [ -d "$tmp/ok-contour-tail" ]; then
    run "contour: the guard next to well-formed owned heads and the owned tail copy accepts ok-contour with the tail" 0 'ok:' "$tmp/ok-contour-tail"
  else
    echo "FAIL contour: the guard next to well-formed owned heads accepts ok-contour with the tail: the dir is not built (missing $cu)" >&2
    failed=$((failed + 1))
  fi
  guard="$real_guard"
fi
# T-206: the owned copy contour-upload.yml is a guard input (preamble: cannot run, exit 3), like the heads.
cucan() { # <name> <label> <ERE>
  guard="$tmp/$1/release-guard.sh"
  run "$2" 3 "$3" "$fx/ok-contour"
  guard="$real_guard"
}
if [ -f "$cu" ]; then
  if cguard g-cu-missing; then
    rm -f "$tmp/g-cu-missing/contour-upload.yml"
    cucan g-cu-missing "cannot run: owned copy contour-upload.yml missing" 'cannot run.*contour-upload'
  fi
  if cguard g-cu-empty; then
    : > "$tmp/g-cu-empty/contour-upload.yml"
    cucan g-cu-empty "cannot run: owned copy contour-upload.yml empty" 'cannot run.*contour-upload'
  fi
  if cguard g-cu-bad-first; then
    sed -i '1s/.*/      - name: Something else/' "$tmp/g-cu-bad-first/contour-upload.yml"
    cucan g-cu-bad-first "cannot run: owned copy contour-upload.yml does not start with the step line" 'cannot run.*contour-upload'
  fi
  if cguard g-cu-no-token; then
    sed -i 's/upload-artifact/upload-thing/g' "$tmp/g-cu-no-token/contour-upload.yml"
    cucan g-cu-no-token "cannot run: owned copy contour-upload.yml names no upload-artifact" 'cannot run.*contour-upload'
  fi
else
  echo "FAIL cannot-run cases of contour-upload.yml: missing $cu (the developer creates it)" >&2
  failed=$((failed + 1))
fi
if cguard g-contour-no-files; then
  rm -rf "$tmp/g-contour-no-files/contour-heads"
  guard="$tmp/g-contour-no-files/release-guard.sh"
  run "contour: no contour file in the dir, so the heads are not read (ok-release without the heads dir)" 0 'ok:' "$fx/ok-release"
  guard="$real_guard"
fi
if cguard g-contour-lookalike-no-heads; then
  rm -rf "$tmp/g-contour-lookalike-no-heads/contour-heads"
  guard="$tmp/g-contour-lookalike-no-heads/release-guard.sh"
  seen[ok-contour-lookalike-name]=1
  run "contour: a read-only teamwright-agent2.yml is not a contour file, so the heads are not read (a glob or suffix name test would exit 3)" 0 'ok:' "$fx/ok-contour-lookalike-name"
  guard="$real_guard"
fi
if cguard g-contour-dir-missing; then
  rm -rf "$tmp/g-contour-dir-missing/contour-heads"
  ccan g-contour-dir-missing "cannot run: contour-heads dir missing" 'cannot run.*contour-heads.*not found or not a directory'
fi
if cguard g-contour-agent-missing; then
  rm -f "$tmp/g-contour-agent-missing/contour-heads/teamwright-agent.yml"
  ccan g-contour-agent-missing "cannot run: head copy of teamwright-agent.yml missing" 'cannot run.*contour-heads.*teamwright-agent\.yml.*not found or not readable'
fi
if cguard g-contour-next-missing; then
  rm -f "$tmp/g-contour-next-missing/contour-heads/teamwright-next.yml"
  ccan g-contour-next-missing "cannot run: head copy of teamwright-next.yml missing" 'cannot run.*contour-heads.*teamwright-next\.yml.*not found or not readable'
fi
if cguard g-contour-agent-empty; then
  : > "$tmp/g-contour-agent-empty/contour-heads/teamwright-agent.yml"
  ccan g-contour-agent-empty "cannot run: head copy of teamwright-agent.yml empty" 'cannot run.*contour-heads.*teamwright-agent\.yml.*is empty'
fi
if cguard g-contour-agent-no-jobs; then
  sed -i '$d' "$tmp/g-contour-agent-no-jobs/contour-heads/teamwright-agent.yml"
  ccan g-contour-agent-no-jobs "cannot run: head copy of teamwright-agent.yml does not end in jobs:" 'cannot run.*contour-heads.*teamwright-agent\.yml.*does not end in'
fi
if cguard g-contour-next-jobs-mid; then
  sed -i '1a jobs:' "$tmp/g-contour-next-jobs-mid/contour-heads/teamwright-next.yml"
  ccan g-contour-next-jobs-mid "cannot run: head copy of teamwright-next.yml has jobs: before its last line" 'cannot run.*contour-heads.*teamwright-next\.yml.*more than one line'
fi
if cguard g-contour-next-no-permissions; then
  sed -i '/^permissions:$/d' "$tmp/g-contour-next-no-permissions/contour-heads/teamwright-next.yml"
  ccan g-contour-next-no-permissions "cannot run: head copy of teamwright-next.yml holds no permissions: line" 'cannot run.*contour-heads.*teamwright-next\.yml.*exactly one line'
fi

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
