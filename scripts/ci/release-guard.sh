#!/usr/bin/env bash
# T-085 (rca, class guard-model, F-003, F-015; decisions #120, P-020) after T-026 (decisions #102,
# #103, #121): only the release job's token can write repository contents, and that job and the
# workflow header GitHub applies to it are exactly the reviewed text. A GitHub Release is written
# by `bash scripts/ci/release.sh` (release-check.sh, then SHA256SUMS.txt, the notes and
# `gh release create --verify-tag`; host test release.test.sh) in the release job, which is a v*
# tag push job that needs gate and windows. Since T-088 (which retired ci-wip.sh and
# setup-node-cache.sh) it also pins every action a workflow runs and every step that can save a
# cache to owned text (R11-R13) and ci.yml's trigger to the owned header (R7).
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
#   R7 ci.yml and every file that holds a `  release:` line start with the guard's second owned copy,
#      scripts/ci/release-header.yml, byte for byte, from line 1 to the file's first line that is
#      exactly `jobs:` (comments included: a comment is text). GitHub applies the workflow-level
#      keys (env:, defaults:, on:, concurrency:, permissions: ...) to the release job, so a
#      BASH_ENV or working-directory there would run with its write token; the literal closes them
#      all without naming any key or variable. ci.yml is named (T-088) so that its trigger
#      (push to main and wip/**, the P-016 pre-review run) holds even with the release job moved
#      to another file.
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
#      line `      - name: SHA-256 of the installer`, so no key can be appended. And, going up
#      from the copy's first line and skipping lines of spaces only, the first line with fewer
#      than 6 leading spaces is exactly `    steps:`, the next with fewer than 4 is exactly
#      `  windows:` and the next with fewer than 2 is exactly `jobs:` (T-087 review 1 finding 1):
#      a YAML block scalar ends at its first line indented less than its content, so a block
#      scalar holding the literal (a job's or the workflow's `env: X: |`, a `name: |`) puts its
#      own header line, not one of these three, first on the way up; a quoted or plain scalar
#      cannot hold the literal (its raw ' and " end a quoted one, its `: ` a plain one), and an
#      anchor that could alias the step would sit on one of the pinned lines. So the literal is
#      a step of the windows job. A comment line is text here too: GitHub expands ${{ }} in run:
#      text, comment lines included, before the shell starts (premise from T-086 Investigation
#      section 2; not checked against GitHub's docs in T-087: no docs tool or web access), and
#      a comment line with fewer than 6 leading spaces between `    steps:` and the send step is
#      refused. Zero tokens is refused (the build sends the installer, #93). That the secret
#      cannot be spelled without this raw text rests on R4 (every secrets is one of the two
#      names), R6 (no escape) and R8 (no tag) of this guard. A raw token, an owned literal and a
#      leading-space count, no YAML or expression reader.
#   R11 (T-088, decision #124, P-020; replaces ci-wip.sh and setup-node-cache.sh) every line
#      holding the word uses or one of the raw tokens actions/cache, setup-node, rust-cache,
#      cache/save, pnpm/action-setup (any case, comments included) is byte-identical to a line of
#      the owned list scripts/ci/action-lines.txt (the never-saving uses lines: checkout,
#      cache/restore, rust-toolchain, upload-artifact, download-artifact) or lies inside a
#      byte-exact copy of an owned step fragment, one per regular file in scripts/ci/cache-steps/
#      (any name; edit each together with ci.yml). A fragment runs from its step's `      - ` line
#      through the first line of the next step or the next job's key line, so no key can be
#      appended to the step; that last
#      line only pins the end and is not inside the fragment (the step it starts needs its own
#      owned line or fragment). Before it reads the dir, the guard checks its own fragments by
#      literals and positions only (exit 3 on a bad one, so the copy and ci.yml cannot be
#      loosened together; T-088 review 1 finding 1, probes A-G): each starts with `      - ` and
#      ends on a `      - ` line or a job key line `  <name>:` (a comment line does not end a
#      step: the key after it joins the step); it is one step: line 1 is the only `      - ` line
#      before the last, no body line holds `*` (a YAML alias) or `&` outside `&&` (an anchor;
#      review 2 finding 1: an alias key `*c : pnpm`, with `&c cache` anchored outside the
#      fragment, spells cache with no cache on the line), exactly one line holds the word
#      uses, and that line is `      - uses: `
#      or `        uses: ` followed by actions/setup-node, Swatinem/rust-cache,
#      actions/cache/save or pnpm/action-setup, `@` and a ref of [A-Za-z0-9._-] (no quote or
#      comment); a setup-node or rust-cache fragment has the uses line as line 1, exactly
#      `        with:` as line 2 and the pinned input as line 3 (`          package-manager-cache:
#      false`, or `          save-if: ${{ github.ref == 'refs/heads/main' }}`), so no line above
#      it can open a scalar that holds it (PyYAML 6.0.1 reads `x: "y` above it as one string) and
#      no key other than with: holds it; line 4 is not blank and has at most 10 leading spaces
#      (the value does not continue), the word with is on one line (a second with: replaces the
#      first) and the raw cache (save-if) on no other line; a cache/save fragment has line 1
#      `      - name: ` plus [A-Za-z0-9 ()._-] only (no scalar opens there), line 2 starting
#      `        if: ${{ github.ref == 'refs/heads/main' && ` (at the step's key column), ending
#      in `}}` and holding one `${{` and one `}}` (text outside the expression would make the
#      condition a non-empty string, always true), line 3 not blank with at most 8 leading
#      spaces (the if: value does not continue), the word if on one line and no ||; a
#      pnpm/action-setup fragment holds no cache. So every action a workflow runs is reviewed
#      text, every step that can save a cache saves from main only, and no setup-node step needs
#      host pnpm. That uses, the action names and the counted words (cache, save-if, with, if)
#      cannot be spelled another way rests on R6 (no escape), R8 (no tag) and no alias or anchor
#      in a fragment; the word uses is the load-bearing token (every action run, reusable-workflow call
#      and local action is a uses key), the five names are a belt (comments must not name them).
#   R12 (T-088) after a file's `jobs:` line, every line that is not empty or spaces only starts
#      with a space, comments included: a column-0 key between two jobs (`on:`, `env:`) would
#      turn the jobs after it, the release job included, into its keys (probe W3).
#   R13 (T-088) no YAML merge key: the raw text << not followed by <, a letter, _, -, ' or " (a
#      bash here-doc <<EOF, <<-EOF, <<'EOF', <<"EOF", <<_X or a here-string <<< passes; a
#      here-doc with a space before its delimiter, `cat << EOF`, and a shift such as
#      $((1 << 2)) are refused, fail-closed: write <<EOF). A merge
#      key could pull an owned fragment, anchored elsewhere, into a step with other keys (probe
#      M2); R11 needs every step's keys to be text on its own lines.
# R1, R3 and R4 are the capability lock (GitHub enforces the token scope; it also covers gh api
# and curl); R2 and R7 pin the release job and the header it inherits as two literals; R5 is the
# early, fail-closed tripwire; R6 and R8 keep R3-R5's raw text the text a YAML reader sees; R9 and
# R10 pin the upload step and the windows job's send step, the only step the Telegram secrets
# reach, as literals; R11-R13 pin every action and every cache-saving step to owned text, and
# R7 on ci.yml its trigger.
# Not caught (deliberate only): a token written into the workflow as text (the gitleaks
# pre-commit scan refuses it); release.sh itself exiting 0 on a refusal (release.test.sh covers
# that); whether GitHub enforces `permissions:` as documented (platform semantics,
# docs/decisions/ci-toolchain.md); a repository default token scope flipped to write (R1 pins the
# workflow level, so it does not matter for jobs without their own block); a YAML parser that
# reads escapes, line breaks or tags beyond YAML 1.1/1.2 (GitHub's parser is not available on the
# host);
# R9's limits: the owned upload literal moved into another job, or hidden inside other YAML such
# as a `name: |` block scalar, with the real upload step deleted, passes R9; loud, not silent: on
# a v* tag the release job's download fails and nothing is published (T-084 review 1); an upload
# through another action is refused by R11 (no unlisted uses line), not by R9;
# R10's limits: R10 pins the send step and its place, not the job around it: code that runs in
# the windows job with the secrets in the send step's environment, on the refs its if: allows
# (push to main or a v* tag) - a job-level or (in another file) workflow-level env: such as
# BASH_ENV, an earlier step writing GITHUB_ENV or GITHUB_PATH, an edit of
# scripts/ci/telegram-send.sh - is not refused (the retired telegram-guard did not refuse it
# either); telegram-send.sh itself leaking the token (to GITHUB_ENV, a file or the log) is
# telegram-send.test.sh's concern, not this guard's; a YAML parser that lets a quoted scalar end
# without its closing quote, or a block scalar hold a less-indented line, is outside YAML 1.1/1.2
# (GitHub's parser is not available on the host; probed with PyYAML 6, T-087 Red tests round 2);
# R11-R13's limits (T-088 Investigation section 5): a cache written by a run step (docker buildx
# --cache-to type=gha, a script calling the cache service); a save if: that is always false on
# main (loud: main saves nothing); an action input supplied through the environment (a job- or
# workflow-level env: INPUT_CACHE: pnpm, or a run step writing it to GITHUB_ENV) - whether the
# runner reads inputs from INPUT_* variables set this way is unverified, not ruled out: github/docs
# metadata-syntax.md (inputs) and workflow-syntax.md (jobs.<job_id>.steps[*].with) say a with:
# input reaches the action as an INPUT_<NAME> variable, not whether a job-level env: INPUT_CACHE
# reaches an action whose with: leaves the input unset (T-088 review 2 finding 2); an owned
# never-saving line reused with its own with: or an owned fragment copied whole into another job (neither can save
# a cache or reach host pnpm; the place of a fragment is not pinned on purpose); whether GitHub
# honours YAML merge keys at all was NOT checked against GitHub's docs either: R13 refuses them
# fail-closed, so the answer does not matter for the guard; a quoted "<<" key passes R13 as
# not a merge key (PyYAML 6.0.1 reads it as a plain string key; GitHub's parser was not checked);
# the line before a fragment in ci.yml is not pinned: a scalar opened there that swallows the
# fragment's first lines either swallows its `      - ` line (no step: nothing runs) or ends on
# a quote inside the fragment, which leaves text after a closed scalar (invalid, loud).
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
# exceeds the copy's, the first aligned line that differs from telegram-send-step.yml (line 1 when
# the alignment starts before line 1), the first line met going up from the send step that is not
# `    steps:`, `  windows:` or `jobs:` as due (the step's first line when none is met), or, with
# no token, the first file's jobs: line; R11: every token line outside the owned lines and
# fragments; R12: the column-0 line; R13: the line of the occurrence). Exit 2: usage
# error. Exit 3: cannot run (the dir is missing or not a directory, holds no regular file, a file
# is unreadable or has no `jobs:` line, an owned copy is missing or malformed: release-job.yml not
# starting with `  release:`, release-header.yml not ending in its only `jobs:` line,
# release-upload.yml missing, empty or not starting with `      - uses: actions/upload-artifact@v4`;
# telegram-send-step.yml missing, empty, holding no secrets.TELEGRAM_ token or not starting with
# `      - name: Send the installer to Telegram (main and v* tags only)`; action-lines.txt missing or
# empty; cache-steps/ missing or holding no fragment, or a fragment failing R11's checks of its
# own text; the dir holding no ci.yml (R7 pins its trigger); awk failed);
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
# R11: the owned never-saving uses lines and the owned step fragments; each fragment's own text
# is checked here, so the copy and ci.yml cannot be loosened together.
ownedl="$here/action-lines.txt"
[ -f "$ownedl" ] && [ -r "$ownedl" ] || cannot_run "owned action lines $ownedl not found or not readable"
[ -s "$ownedl" ] || cannot_run "owned action lines $ownedl is empty"
csd="$here/cache-steps"
[ -d "$csd" ] || cannot_run "owned fragment dir $csd not found or not a directory"
frags=()
for fr in "$csd"/*; do
  [ -f "$fr" ] || continue
  [ -r "$fr" ] || cannot_run "owned fragment $fr is not readable"
  frags+=("$fr")
done
[ "${#frags[@]}" -gt 0 ] || cannot_run "owned fragment dir $csd holds no fragment"
# wcount <word> <text>: the number of lines of <text> holding <word>, any case, bounded by
# characters outside [A-Za-z0-9_] or the line's ends.
wcount() { grep -c -i -E "(^|[^A-Za-z0-9_])$1([^A-Za-z0-9_]|\$)" <<<"$2"; }
# lead <line>: the number of leading spaces of <line>.
lead() { local p="${1%%[! ]*}"; echo "${#p}"; }
# The action of a fragment's step: its one uses line, at the step's key column, exactly one of
# the four owned action names, @ and a ref of [A-Za-z0-9._-] (no quote, comment or other byte
# that could open a scalar or hide another action).
uses_re='^      (- |  )uses: (actions/setup-node|Swatinem/rust-cache|actions/cache/save|pnpm/action-setup)@[A-Za-z0-9._-]+$'
# A cache/save step's name line: plain text that cannot open a quoted, flow or block scalar.
name_re='^      - name: [A-Za-z0-9 ()._-]+$'
save_if="        if: \${{ github.ref == 'refs/heads/main' && "
for fr in "${frags[@]}"; do
  mapfile -t A < "$fr"
  n="${#A[@]}"
  [ "$n" -ge 2 ] || cannot_run "owned fragment $fr has fewer than two lines (a step line and a step-ending line)"
  first="${A[0]}"; last="${A[n - 1]}"; body="$(printf '%s\n' "${A[@]:0:n-1}")"
  [[ "$first" == "      - "* ]] || cannot_run "owned fragment $fr does not start with a step line '      - '"
  [[ "$last" == "      - "* || "$last" =~ ^\ \ [A-Za-z0-9_-]+:$ ]] ||
    cannot_run "owned fragment $fr does not end on a step-ending line (the next step's '      - ' line or a job key line '  <name>:')"
  # One step: line 1 is the body's only '      - ' line, and one body line holds the word uses.
  [ "$(grep -c '^      - ' <<<"$body")" = 1 ] || cannot_run "owned fragment $fr holds more than one step (a '      - ' line after its first line)"
  # No alias or anchor: an alias key (*c : pnpm, with &c cache anchored outside the fragment)
  # spells a word the counts below look for without the word (review 2 finding 1, probes N1-N4).
  ! grep -q -F '*' <<<"$body" || cannot_run "owned fragment $fr holds * (a YAML alias)"
  ! grep -q -F '&' <<<"${body//&&/}" || cannot_run "owned fragment $fr holds & outside && (a YAML anchor)"
  [ "$(wcount uses "$body")" = 1 ] || cannot_run "owned fragment $fr: not exactly one line holds the word uses (one step, one action)"
  ul="$(grep -i -E '(^|[^A-Za-z0-9_])uses([^A-Za-z0-9_]|$)' <<<"$body")"
  [[ "$ul" =~ $uses_re ]] ||
    cannot_run "owned fragment $fr: its uses line is not '      - uses: ' or '        uses: ' followed by actions/setup-node, Swatinem/rust-cache, actions/cache/save or pnpm/action-setup, '@' and a ref of [A-Za-z0-9._-]"
  kind="${BASH_REMATCH[2]}"
  case "$kind" in
    actions/setup-node | Swatinem/rust-cache)
      # The pinned input is line 3, right under line 2 '        with:', right under the uses line
      # (line 1): no line above it can open a scalar that holds it, and no other key holds it. The
      # next line is not blank and has at most 10 leading spaces, so its value does not continue.
      if [ "$kind" = actions/setup-node ]; then pin='          package-manager-cache: false'; tok=cache
      else pin="          save-if: \${{ github.ref == 'refs/heads/main' }}"; tok=save-if; fi
      [[ "$first" == "      - uses: "* ]] || cannot_run "owned fragment $fr ($kind): line 1 is not its uses line"
      [ "$n" -ge 4 ] && [ "${A[1]}" = "        with:" ] && [ "${A[2]}" = "$pin" ] ||
        cannot_run "owned fragment $fr ($kind): line 2 is not '        with:' or line 3 is not '$pin'"
      [[ ! "${A[3]}" =~ ^\ *$ ]] && [ "$(lead "${A[3]}")" -le 10 ] ||
        cannot_run "owned fragment $fr ($kind): line 4 is blank or indented more than line 3 (the pinned value could continue)"
      [ "$(wcount with "$body")" = 1 ] || cannot_run "owned fragment $fr ($kind): the word with on more than one line (a second with: replaces the first)"
      [ "$(grep -c -i -F "$tok" <<<"$body")" = 1 ] ||
        cannot_run "owned fragment $fr ($kind) holds $tok on a line other than line 3"
      ;;
    actions/cache/save)
      # Line 1 a plain name, line 2 the step-level if: (the main operand, one ${{ }} ending the
      # line, so no text outside it), line 3 not blank and at most 8 leading spaces (the if: value
      # does not continue); the word if on one line; no ||.
      [[ "$first" =~ $name_re ]] || cannot_run "owned fragment $fr (cache/save): line 1 is not '      - name: ' followed by [A-Za-z0-9 ()._-] only"
      l2="${A[1]}"
      [ "$n" -ge 4 ] && [ "${l2:0:${#save_if}}" = "$save_if" ] && [ "${l2: -2}" = "}}" ] &&
        [ "$(grep -o -F '${{' <<<"$l2" | wc -l)" -eq 1 ] && [ "$(grep -o -F '}}' <<<"$l2" | wc -l)" -eq 1 ] ||
        cannot_run "owned fragment $fr (cache/save): line 2 is not '$save_if...}}' with exactly one \${{ and one }}"
      [[ ! "${A[2]}" =~ ^\ *$ ]] && [ "$(lead "${A[2]}")" -le 8 ] ||
        cannot_run "owned fragment $fr (cache/save): line 3 is blank or indented more than 8 (the if: value could continue)"
      [ "$(wcount if "$body")" = 1 ] || cannot_run "owned fragment $fr (cache/save): the word if on more than one line"
      ! grep -q -F '||' <<<"$body" || cannot_run "owned fragment $fr (cache/save) holds ||"
      ;;
    pnpm/action-setup)
      ! grep -q -i 'cache' <<<"$body" || cannot_run "owned fragment $fr (pnpm/action-setup) holds cache"
      ;;
  esac
done
fraglist="$(printf '%s\n' "${frags[@]}")"

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
# R7 pins the trigger of ci.yml: a dir without it (the release job and the header moved to another
# name) is not the workflows dir this guard checks.
[ -f "$dir/ci.yml" ] || cannot_run "no ci.yml in $dir (R7 pins its trigger to the owned header)"

# Output lines: "V <file>:<line>: R<n>: <why>"; awk exit 3 = an owned copy could not be read.
report="$(RG_FRAGS="$fraglist" awk -v owned="$owned" -v ownedh="$ownedh" -v ownedu="$ownedu" -v ownedt="$ownedt" -v ownedl="$ownedl" '
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
  # R11: the owned lines (a set) and the owned fragments FR[f, m], FN[f] lines each.
  while ((rc = (getline line < ownedl)) > 0) OL[line] = 1
  if (rc < 0) { bad = 1; exit 3 }
  close(ownedl)
  nfr = split(ENVIRON["RG_FRAGS"], FP, "\n")
  for (f = 1; f <= nfr; f++) {
    FN[f] = 0
    while ((rc = (getline line < FP[f])) > 0) FR[f, ++FN[f]] = line
    if (rc < 0 || FN[f] < 2) { bad = 1; exit 3 }
    close(FP[f])
  }
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
    if (hasrel || F[k] ~ /(^|\/)ci\.yml$/) {
      for (i = 1; i <= N[k]; i++) {
        if (i > nh || L[k, i] != oh[i]) break
        if (L[k, i] == "jobs:") break
      }
      if (i > N[k] || i > nh || L[k, i] != oh[i])
        v(k, i, "R7", "the workflow header, from line 1 to jobs:, differs from the owned copy scripts/ci/release-header.yml here (byte for byte; GitHub applies it to the release job)")
    }
    # R11: cov[k, i] = line i lies inside a byte-exact copy of an owned fragment, its last line
    # (the first line of the next step, or a job key) excluded: that line only pins the end.
    for (f = 1; f <= nfr; f++)
      for (st = 1; st + FN[f] - 1 <= N[k]; st++) {
        for (m = 1; m <= FN[f] && L[k, st + m - 1] == FR[f, m]; m++);
        if (m > FN[f]) for (m = 1; m < FN[f]; m++) cov[k, st + m - 1] = 1
      }
    jl = 0
    for (i = 1; i <= N[k]; i++) {
      s = L[k, i]; lo = tolower(s)
      # R11: the word uses or an action-name token, any case, comments included.
      if ((word(lo, "uses") || index(lo, "actions/cache") || index(lo, "setup-node") ||
           index(lo, "rust-cache") || index(lo, "cache/save") || index(lo, "pnpm/action-setup")) &&
          !cov[k, i] && !(s in OL))
        v(k, i, "R11", "an action (the word uses or actions/cache, setup-node, rust-cache, cache/save, pnpm/action-setup) outside the owned lines scripts/ci/action-lines.txt and the owned step fragments scripts/ci/cache-steps/ (byte for byte; edit both together)")
      # R12: after jobs:, no line that is not empty or spaces only starts in column 0.
      if (jl && s !~ /^ *$/ && s !~ /^ /)
        v(k, i, "R12", "a column-0 line after jobs: (a top-level key between jobs would turn the jobs after it into its keys)")
      if (!jl && s == "jobs:") jl = i
      # R13: << not followed by <, a letter, _, -, \047 or " is a YAML merge key.
      mt = s
      while ((mp = index(mt, "<<")) > 0) {
        if (substr(mt, mp + 2, 1) !~ /[<A-Za-z_\047"-]/) {
          v(k, i, "R13", "a YAML merge key << (it could merge an owned fragment into a step with other keys)")
          break
        }
        mt = substr(mt, mp + 2)
      }
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
  } else {
    tbad = 0
    for (m = 1; m <= nt; m++) {
      i = tti - tof + m
      if (i < 1 || i > N[ttk] || L[ttk, i] != ot[m]) {
        v(ttk, (i < 1 ? 1 : i), "R10", "the Telegram send step differs from the owned copy scripts/ci/telegram-send-step.yml here (byte for byte; push && (main || v* tag) && send == true; edit both together)")
        tbad = 1
        break
      }
    }
    # R10: going up from the first line of the copy (lines of spaces only skipped), the first line with
    # fewer than 6, then 4, then 2 leading spaces is "    steps:", "  windows:", "jobs:". A
    # block scalar ends at its first less-indented line, so one holding the literal cannot hold
    # all three, and its header line is met first.
    if (!tbad) {
      lim[1] = 6; want[1] = "    steps:"; lim[2] = 4; want[2] = "  windows:"; lim[3] = 2; want[3] = "jobs:"
      lv = 1; s0 = tti - tof + 1
      for (i = s0 - 1; i >= 1 && lv <= 3; i--) {
        s = L[ttk, i]
        if (s ~ /^ *$/) continue
        match(s, /^ */)
        if (RLENGTH >= lim[lv]) continue
        if (s != want[lv]) break
        lv++
      }
      if (lv <= 3)
        v(ttk, (i >= 1 ? i : s0), "R10", "the Telegram send step is not a step of the windows job here: going up from it, the first line with fewer than " lim[lv] " leading spaces must be \"" want[lv] "\" (a block scalar or another job could hold the owned literal and hand the secrets to every step of a job, on every ref)")
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
    echo "release-guard: FAIL: a token other than the release job's could write contents, the release job or the ci.yml header is not the owned copy, the installer upload is not the owned tag-only step, the Telegram secrets reach a step other than the owned send step, or an action is not an owned line or step fragment (T-085, T-084, T-087, T-088, P-020)" >&2
    exit 1
    ;;
  *) cannot_run "awk failed (exit $rc) on $dir or an owned copy ($owned, $ownedh, $ownedu, $ownedt, $ownedl, $csd)" ;;
esac
grep -q '^V ' <<<"$report" && cannot_run "awk listed violations but exited 0"
echo "release-guard: ok: $dir: ${#files[@]} file(s); token read-only but in the release job, which is the owned copy; no other secret, permissions or gh; one installer upload, the owned tag-only step; the Telegram secrets only in the owned send step of the windows job; every action an owned line or step fragment, no column-0 line after jobs:, no merge key"
