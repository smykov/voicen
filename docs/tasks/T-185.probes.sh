#!/usr/bin/env bash
# T-185 red tests: one probe per Acceptance line that is still stale at HEAD,
# plus the failure-branch checks. Read-only, no network. Run from the repo root:
#   bash docs/tasks/T-185.probes.sh
# Output: PASS id / FAIL id - <file>: <what> / MANUAL id - <reason>.
# Exit status 1 if any FAIL. Probes anchor on file + distinctive phrase, never on
# line numbers (P-018). "absent" probes name stale text that must be gone;
# "present" probes name text the fix adds.
#
# Resolved at HEAD (no probe, per ## Investigation): T-061 r3#7 (text removed in
# e98856a9), T-046 r2#8 (core-tests.md "Users of tests/common"), the T-068 script
# half (retired in 956a8bd1; only the F-011 Rule column is probed), the T-024 r4#1
# tasks.md half (T026 already says "called from release_parts"), the T-073 parts
# in 003 data-model (Timeout row) and 002 FR-019a.
# Not docs, split out: src-tauri/tests/cred_support/mod.rs header (T-069 r4#1
# second half) goes to T-167. The optional T-091 owned check is a guard (T-180).
set -u
cd "$(git rev-parse --show-toplevel)" || exit 2

pass=0 fail=0 manual=0
ok()   { printf 'PASS %s\n' "$1"; pass=$((pass+1)); }
bad()  { printf 'FAIL %s - %s: %s\n' "$1" "$2" "$3"; fail=$((fail+1)); }
man()  { printf 'MANUAL %s - %s\n' "$1" "$2"; manual=$((manual+1)); }

# exists FILE... : every path must exist, else the probe itself is wrong
need() { for f in "$@"; do [ -e "$f" ] || { bad "$ID" "$f" "path missing (probe error)"; return 1; }; done; }

# absent ID FILE FIXED WHAT : FAIL when FIXED is still in FILE
absent() { ID=$1; need "$2" || return; if grep -qF -- "$3" "$2"; then bad "$1" "$2" "$4"; else ok "$1"; fi; }
# present ID FILE FIXED WHAT : FAIL when FIXED is missing
present() { ID=$1; need "$2" || return; if grep -qF -- "$3" "$2"; then ok "$1"; else bad "$1" "$2" "$4"; fi; }
# absent_re / present_re : extended regex, case-sensitive
absent_re() { ID=$1; need "$2" || return; if grep -qE -- "$3" "$2"; then bad "$1" "$2" "$4"; else ok "$1"; fi; }
present_re() { ID=$1; need "$2" || return; if grep -qE -- "$3" "$2"; then ok "$1"; else bad "$1" "$2" "$4"; fi; }
# sect FILE START_RE [STOP_RE] : lines from the first START match up to (not incl.) next STOP (default next "## ")
sect() { awk -v s="$2" -v e="${3:-^## }" 'f&&$0~e{exit} $0~s{f=1} f' "$1"; }
sect_absent() { ID=$1; need "$2" || return; if sect "$2" "$3" "${6:-^## }" | grep -qiF -- "$4"; then bad "$1" "$2" "$5"; else ok "$1"; fi; }
sect_present() { ID=$1; need "$2" || return; if sect "$2" "$3" "${6:-^## }" | grep -qiF -- "$4"; then ok "$1"; else bad "$1" "$2" "$5"; fi; }
# lines containing ANCHOR (fixed) must contain NEEDLE (fixed); fails if no anchor line or none has the needle
line_has() { ID=$1; need "$2" || return; if grep -F -- "$3" "$2" | grep -qF -- "$4"; then ok "$1"; else bad "$1" "$2" "$5"; fi; }
line_has_re() { ID=$1; need "$2" || return; if grep -F -- "$3" "$2" | grep -qE -- "$4"; then ok "$1"; else bad "$1" "$2" "$5"; fi; }
line_lacks() { ID=$1; need "$2" || return; if grep -F -- "$3" "$2" | grep -qF -- "$4"; then bad "$1" "$2" "$5"; else ok "$1"; fi; }
fm() { awk 'NR==1&&$0!="---"{exit} NR>1&&$0=="---"{exit} NR>1' "$1"; }   # front matter of a record
fmval() { fm "$1" | sed -n "s/^$2: *//p" | head -1 | tr -d '"'; }

CI=docs/decisions/ci-toolchain.md
S1=specs/001-dictation-via-api
S2=specs/002-local-transcription
S3=specs/003-llm-post-processing
S6=specs/006-diagnostics-and-release

echo "== content probes =="

# T-060 r2#2: the two triage rows still say "likely done"
absent T-060r2#2a docs/tasks/T-060.md '| T-018 r2#1 | likely done' 'Investigation 2 triage row still says "likely done" for T-018 r2#1'
absent T-060r2#2b docs/tasks/T-060.md '| T-085 r4#1, T-113 r2#1 | likely done' 'Investigation 2 triage row still says "likely done" for T-085 r4#1 and T-113 r2#1'

# T-162 r1#2
present T-162r1#2 docs/process/metrics.md 't162-probe' 'no note of the probe session id t162-probe'

# T-061 r2#6: the "never stored elsewhere" clause must sit with the WinCredentialStore sentence, before "The second caller"
present_re T-061r2#6 docs/decisions/settings.md 'WinCredentialStore.*never stored elsewhere.*The second caller' 'clause "never stored elsewhere" is not on the WinCredentialStore sentence (before "The second caller")'
absent T-061r2#6b docs/decisions/settings.md 'and a credential failure is returned, never stored elsewhere' 'clause still trails the purge sentence'

# T-061 r6#1 (two lines, same quote)
absent T-061r6#1a docs/tasks/T-061.md "lists: …" 'the CI line cuts the listing to "lists: …"'
sect_absent T-061r6#1b docs/tasks/T-061.md '^### CI [(]P-016, after T-066[)]' '…' 'CI (P-016, after T-066) still has an ellipsis instead of the full listing' '^## '
# T-061 v4: name the step by its title
absent T-061v4a docs/decisions/windows-ci-runner.md 'step 18' 'installed-exe purge step is called "step 18" (run 37736079112: step 21)'
sect_absent T-061v4b docs/tasks/T-061.md '^## Tests' 'step 18' '## Tests calls the installed-exe purge step "step 18"' '^## Investigation'
# T-061 v4 record: correction goes to the owning record's Notes (validation/4.md is append-only)
sect_present T-061v4c docs/tasks/T-061.md '^## Notes' '112/116/109' 'Notes lacks the corrected secrets.rs lines 112/116/109 for M1/M2/M3'

# T-057 r3#3: F-006 says kill + wait bounds the hold on the runner's pipes
if need docs/failures.md; then ID=T-057r3#3
  if grep -F '| F-006 |' docs/failures.md | grep -qiE '(kill|killed).{0,60}wait.{0,200}pipes'; then ok T-057r3#3; else bad T-057r3#3 docs/failures.md 'F-006 does not say kill + wait bounds the hold on the runner pipes'; fi; fi

# T-058 r2#1
absent T-058r2#1 $CI '(T-058, decisions #81)' 'cold-time bullet cites only #81, not "#81, #88"'

# T-064 r1#7: which Node each command runs on
line_has T-064r1#7a CLAUDE.md 'pnpm install && make core-image' 'ode' 'setup command does not say which Node it runs on'
line_has T-064r1#7b CLAUDE.md 'pnpm test -- <file>' 'ode' 'single-unit-test command does not say which Node it runs on'
# T-064 r2#9
absent T-064r2#9a docs/decisions/ui-e2e.md 'There is no way to reuse an existing server' 'E1 says no server can be reused, contradicting E3 (BASE_URL inside the image)'
absent T-064r2#9b docs/decisions/ui-e2e.md 'is the only entry In the image' 'E3 run-on sentence "...the only entry In the image..."'

# T-068 r1#1 residual: F-011 Rule no longer offers cache: npm
if need docs/failures.md; then ID=T-068r1#1
  if grep -F '| F-011 |' docs/failures.md | grep -qF 'or `cache: npm`'; then bad T-068r1#1 docs/failures.md 'F-011 Rule still lists `cache: npm` as a fix'; else ok T-068r1#1; fi; fi

# T-070 r4#1
absent T-070r4#1a $CI 'P-016 (PRINCIPLES 1.1.0)' 'still cites PRINCIPLES 1.1.0 (1.1.1 since decisions #95)'
absent T-070r4#1b docs/failures.md 'PRINCIPLES 1.1.0' 'class row still cites PRINCIPLES 1.1.0'

# T-006 r5#1, r4#5
line_has T-006r5#1 docs/decisions/windows-ci-runner.md '| `hotkey`: registration' '1409' 'hotkey: registration row omits the 1409 check and that the hold steps go red'
if need docs/tasks/T-006.md; then ID=T-006r4#5
  if fm docs/tasks/T-006.md | grep -qF '192.0.2.1:9'; then bad T-006r4#5 docs/tasks/T-006.md 'analysis names 192.0.2.1:9 while the step uses 127.0.0.1:1'; else ok T-006r4#5; fi; fi

# T-067 r1#1
line_has T-067r1#1 $S1/research.md 'makes it click-through' 'overlay.md' '001 research.md click-through sentence is not marked superseded by docs/decisions/overlay.md'
# T-072 r1#1
line_has T-072r1#1a $S1/research.md 'The request is `POST {base_url without trailing' 'URL join' 'research.md still describes an always-appended path without pointing to contracts/openai-transcription.md URL join'
line_has T-072r1#1b $S1/data-model.md 'path joined as' 'URL join' 'data-model.md still describes an always-appended path without pointing to URL join'

# T-036 r2#1, r2#2
absent T-036r2#1 docs/tasks/T-036.md 'is refreshed when the task is started' 'note still promises a refresh that did not happen'
if need docs/tasks/T-036.md; then ID=T-036r2#2
  if awk '/^## Investigation/{ if (prev!="") bad=1 } {prev=$0} END{exit bad?1:0}' docs/tasks/T-036.md; then ok T-036r2#2; else bad T-036r2#2 docs/tasks/T-036.md 'no blank line before ## Investigation (### CI inside Acceptance)'; fi; fi

# T-073: default timeouts stated as fixed values
absent T-073a $S3/spec.md 'no complete reply within 15 s' '003 spec FR-003 states a fixed 15 s'
absent T-073b $S2/spec.md 'does not answer within 60 s' '002 spec scenario states a fixed 60 s'
absent T-073c $S2/spec.md 'cannot be connected to within 5 s' '002 spec scenario states a fixed 5 s'
absent T-073d $S2/spec.md 'bounded by 120 s' '002 spec edge case states a fixed 120 s'
absent T-073e $S2/tasks.md 'Production timeouts asserted: 5 s and 60 s from the shared module' '002 tasks.md states fixed 5 s / 60 s'
absent T-073f $S2/plan.md '| FR-24 local-server 60 s / connect 5 s |' '002 plan.md states fixed 60 s / 5 s'

# T-069 r3#7, r4#1 (docs half)
sect_absent T-069r3#7 docs/tasks/T-069.md '^### Validation 1 red tests' 'does not compile' 'Validation 1 red tests still says in the present tense that the file does not compile' '^#{2,3} '
sect_absent T-069r4#1a docs/tasks/T-069.md '^### Validation 2 red tests' 'Red state: the module holds only the tests' 'Validation 2 red tests still states the red state in the present tense' '^#{2,3} '
sect_absent T-069r4#1b docs/tasks/T-069.md '^### Validation 2 red tests' '`make check` is red' 'Validation 2 red tests still says make check is red' '^#{2,3} '
man T-069r4#1c 'src-tauri/tests/cred_support/mod.rs header comment is code, split to T-167'

# T-025 r2#5
sect_present T-025r2#5 docs/tasks/T-025.md '^## Tests' '7131ac66' '## Tests lacks the note that the missing-voicen.exe purge branch is verified by reading only (commit 7131ac66)' '^## Review'
# T-046 r2#7
absent T-046r2#7 specs/004-settings-and-first-run/plan.md 'test-clip.wav' 'source tree still lists assets/test-clip.wav'
# T-076 r3#1
line_lacks T-076r3#1 $S3/tasks.md 'T023' 'post_process_secrets.rs' 'T023 header still names post_process_secrets.rs'
# T-023 r3#1
line_has T-023r3#1a docs/decisions/settings-ui.md 'Tests: `e2e/about.spec.ts`' 'holdBuildInfo' 'section A Tests list lacks the holdBuildInfo contract case of tauri-mock.spec.ts'
line_has T-023r3#1b docs/decisions/settings-ui.md 'Tests: `e2e/about.spec.ts`' 'reopen while get_build_info is pending' 'section A Tests list lacks the reopen cases of about.spec.ts (by test title)'
# T-084 r2#1
line_has T-084r2#1 $S6/quickstart.md '## Windows CI runner' 'pull request' 'quickstart heading omits pull requests'

# T-086 r2#1..#3
line_has T-086r2#1 docs/tasks/T-086.md '| `nsis-fork.sh` (check-nsis-fork)' 'superseded' 'Investigation 2 nsis-fork row has no "superseded, see T-093 / #125" pointer'
line_has T-086r2#2 $CI 'Not guards (programs with host tests' 'notices.test.mjs' 'Not guards line does not name notices.test.mjs'
absent T-086r2#3 docs/tasks/T-093.md 'from a tool, not from an awk' 'T-093 title still says "from a tool"'
# T-091 r1#1
if need docs/decisions/overlay.md; then ID=T-091r1#1
  if grep -F 'Not caught: a class written with escapes' docs/decisions/overlay.md | grep -oE 'Not caught:.*' | grep -qF 'Tauri Window'; then ok T-091r1#1; else bad T-091r1#1 docs/decisions/overlay.md 'Not caught does not name a product window class from a crate off the owned list (e.g. the default Tauri Window)'; fi; fi
# T-093 r1#2
line_has T-093r1#2 $CI '| `check-nsis-fork` / `nsis-fork.sh` |' 'literal + the tool itself' 'nsis-fork decides_on cell says only "literal"'
# T-103 r2#1
absent T-103r2#1 docs/tasks/T-103.md 'from 71 to 0' 'Acceptance still says 71 to 0 (correct: 73 to 0)'
# T-089 r2#2
line_has T-089r2#2 docs/tasks/T-089.md '  invariant:' 'SystemTime' 'invariant names only Instant::now/elapsed, not every clock read banned in clippy.toml'
# T-098 r2#1
if need $S2/contracts/core-traits.md; then ID=T-098r2#1
  if sect $S2/contracts/core-traits.md '^## ModelResidency' '^## ' | awk '/^```/{f=!f; next} f' | grep -qF 'transcribe_within'; then ok T-098r2#1; else bad T-098r2#1 $S2/contracts/core-traits.md 'ModelResidency code block lacks transcribe_within'; fi; fi
# T-100 r2#1
absent T-100r2#1 docs/tasks/T-100.md 'cleanup by `begin`: a proposal only, not pinned' 'Not covered still calls stale-tmp cleanup a proposal'

# T-024 r4#1 (diagnostics-log half)
line_has T-024r4#1 docs/decisions/diagnostics-log.md 'Shell wiring (T-024)' 'release_parts' 'Shell wiring still has begin_session calling diag::start (release_parts does)'
# T-024 r4#2
absent T-024r4#2 $S6/tasks.md "grep the log folder for the CI test credential's value (0 hits)" 'T030 still lists the credential 0-hit grep as planned'
# T-024 r4#5, r4#6, r5#1, r5#2, r5#4
[ "$(fmval docs/tasks/T-024.md surface)" = none ] && ok T-024r4#5 || bad T-024r4#5 docs/tasks/T-024.md 'front matter surface is not "none"'
sz=$(wc -c < docs/tasks/T-024.md); [ "$sz" -lt 40960 ] && ok T-024r4#6 || bad T-024r4#6 docs/tasks/T-024.md "record is $sz B, over 40960"
[ "$(fmval docs/tasks/T-105.md class)" = diagnostics-log ] && ok T-024r5#1 || bad T-024r5#1 docs/tasks/T-105.md 'class is not "diagnostics-log"'
[ "$(fmval docs/tasks/T-105.md surface)" = none ] && ok T-024r5#2 || bad T-024r5#2 docs/tasks/T-105.md 'front matter surface is not "none"'
ID=T-024r5#4
if cat docs/tasks/T-024.md docs/tasks/T-024.history.md 2>/dev/null | grep -F 'Out of this task (orchestrator):' | grep -qF 'T-105'; then ok T-024r5#4; else bad T-024r5#4 docs/tasks/T-024.md '"Out of this task" os= line does not point to T-105 / #140'; fi
# T-095 r1#1
absent T-095r1#1 docs/decisions/settings-ui.md 'F-017) and `aria-labelledby` its title' 'aria-labelledby clause still hangs off the T-095 sentence'
# T-099
ID=T-099r2#1
if need $CI; then if grep -F "**Don't:**" $CI | grep -F 'whisper-rs' | grep -qi 'bump'; then ok T-099r2#1; else bad T-099r2#1 $CI "no Don't about re-reading ggml option()s on a whisper-rs / whisper-rs-sys bump"; fi; fi
absent T-099r2#2 $CI 'its text printed in the error; no cache found refuses' 'AVX512/AMX clause still splits "its text printed in the error" from "refuses"'
absent T-099r3#2 docs/tasks/T-099.md '| M9: `GGML_AVX2` dropped from `TOKENS`' 'round-3 mutation table still reuses ids M9-M13 unprefixed'
# T-083 r1#2
present T-083r1#2a $CI 'the parent of the ancestor of the deepest commit pushed' 'new-branch diff rule is not quoted exactly'
absent T-083r1#2b $CI 'against the parent of its deepest pushed commit' 'new-branch diff rule is still paraphrased'
# T-113 r2#2
line_lacks T-113r2#2 $CI '| `check-guard-trigger` / ' '(a) check-role-tools' 'decides_on cell still copies part (a)'
# T-105 r1#2
absent T-105r1#2 $S6/research.md 'the shipped line has no `os=`' 'history bullet still says the start line has no os= and calls it planned'

# T-026 r5#1..#4, r6#1, r6#2 (T-026.md may move history to T-026.history.md)
ID=T-026r5#1
if cat docs/tasks/T-026.md docs/tasks/T-026.history.md 2>/dev/null | grep -qF 'Not exercised before a real tag'; then bad T-026r5#1 docs/tasks/T-026.md 'stale "Not exercised before a real tag" sentence'; else ok T-026r5#1; fi
ID=T-026r5#2a
if sect docs/tasks/T-026.md '^## Acceptance' '^## ' | grep -q '^- \[ \]'; then bad T-026r5#2a docs/tasks/T-026.md 'Acceptance boxes unticked'; else ok T-026r5#2a; fi
ID=T-026r5#2b
if cat docs/tasks/T-026.md docs/tasks/T-026.history.md 2>/dev/null | grep -F 'actions/runs/39661044004' | grep -qF 'attempt 1'; then ok T-026r5#2b; else bad T-026r5#2b docs/tasks/T-026.md 'run #207 CI line does not say "attempt 1"'; fi
absent T-026r5#3a $CI "owner's first tag" 'still says the publish first runs on the owner's first tag'
absent T-026r5#3b $CI 'the run proof for tags is the next real release' 'still says the tag run proof is the next real release'
present T-026r5#3c $CI '207' 'run #207 is not named as the tag-run proof'
sz=$(wc -c < docs/tasks/T-026.md); [ "$sz" -lt 40960 ] && ok T-026r5#4a || bad T-026r5#4a docs/tasks/T-026.md "record is $sz B, over 40960"
[ -s docs/tasks/T-026.history.md ] && ok T-026r5#4b || bad T-026r5#4b docs/tasks/T-026.history.md 'history file does not exist'
[ -s docs/tasks/T-024.history.md ] && ok T-024r4#6b || bad T-024r4#6b docs/tasks/T-024.history.md 'history file does not exist'
if need docs/tasks/T-026.md; then ID=T-026r6#1a
  if fm docs/tasks/T-026.md | grep -qE 'release(-check)?\.sh:[0-9]'; then bad T-026r6#1a docs/tasks/T-026.md 'analysis.decides_on cites line numbers (P-018)'; else ok T-026r6#1a; fi; fi
if need $CI; then ID=T-026r6#1b
  if grep -F '| `check-release-check` / `release-check.sh`' $CI | grep -qE '\.sh:[0-9]|[ (]:[0-9]'; then bad T-026r6#1b $CI 'guard table row (check-release-check) still cites line numbers (P-018)'; else ok T-026r6#1b; fi; fi
if need $CI; then ID=T-026r6#2
  l=$(grep -F -- '- **Where it is enforced:** `make check-release-check`' $CI)
  if printf '%s' "$l" | grep -qi 'checksum' && printf '%s' "$l" | grep -qi 'echo'; then ok T-026r6#2; else bad T-026r6#2 $CI 'Where it is enforced omits the release.test.sh cases (checksum write fails, gh echoes the token)'; fi; fi

# T-049
absent T-049r4#1 docs/tasks/T-049.md 'Bite check (throwaway worktree, wrong implementations of the seam)' 'bite-check header still says throwaway worktree (reds read as observed)'
if need docs/tasks/T-049.md; then ID=T-049r4#1b
  n=$(grep -E '^- (W[1-6] |No change at all)' docs/tasks/T-049.md | grep -vc 'expected')
  if [ "$n" -eq 0 ]; then ok T-049r4#1b; else bad T-049r4#1b docs/tasks/T-049.md "$n bite-check lines (W1-W4, W6, No change at all) read as observed Windows reds, not expected (not run on Windows)"; fi; fi
absent T-049r4#2 docs/tasks/T-049.md 'resolves to the `__app__` key' 'a prefixless permission id is said to resolve to __app__ (tauri: __app-acl__)'
line_has T-049v1 docs/decisions/ipc-acl.md '**Generated files:**' 'clean checkout' 'ipc-acl.md does not say stale autogenerated files hide a missing app permission list locally (only a clean checkout / CI fails)'
# T-065 r2#3
present T-065r2#3 docs/decisions.md '#78 confirmed by T-065 review 2' 'no appended row confirming #78 with the refined key'
# T-108 r2#2: a family note (not a table row) naming F-004, F-012 and F-020
if need docs/failures.md; then ID=T-108r2#2
  if grep -v '^| ' docs/failures.md | grep -F 'F-004' | grep -F 'F-012' | grep -qF 'F-020'; then ok T-108r2#2; else bad T-108r2#2 docs/failures.md 'no family note naming F-004, F-012 and F-020'; fi; fi
# T-018 r2#1
absent T-018r2#1 $S1/research.md 'each request with `.timeout(api_transcription)`' 'R-8 still says each request uses .timeout(api_transcription)'
# T-085 r4#1
absent T-085r4#1 $CI 'R8 every tag form' 'Not caught still says R8 refuses every tag form'
# T-113 r2#1
absent T-113r2#1a docs/process/gates.md 'Guards outside the four path forms (none known' 'Not caught still says (none known)'
line_has T-113r2#1b docs/process/gates.md '**Not caught.** A new root `GNUmakefile`' '_task.py' 'Not caught does not list scripts/hooks/_task.py'
if need docs/open-questions.md; then ID=T-113r2#1c
  if grep -F 'scripts/hooks/' docs/open-questions.md | grep -qF 'T-113'; then ok T-113r2#1c; else bad T-113r2#1c docs/open-questions.md 'no OQ asking whether scripts/hooks/ belongs in the guard token'; fi; fi

echo "== failure-branch checks =="
# (a) the diff touches only docs/**, specs/** and CLAUDE.md
ID=FB-a
{ git diff --name-only main...HEAD; git diff --name-only HEAD; git diff --name-only --cached; git ls-files --others --exclude-standard; } | sort -u > /tmp/t185.paths.$$
outside=$(grep -vE '^(docs/|specs/)|^CLAUDE\.md$' /tmp/t185.paths.$$ | grep -v '^$' | head -5 | tr '\n' ' ')
[ -z "$outside" ] && ok FB-a || bad FB-a 'diff' "paths outside docs/, specs/, CLAUDE.md: $outside"
# (b) records over P-019 are under 40 KB
for f in docs/tasks/T-024.md docs/tasks/T-026.md; do
  sz=$(wc -c < "$f"); [ "$sz" -lt 40960 ] && ok "FB-b:$(basename "$f")" || bad "FB-b:$(basename "$f")" "$f" "$sz B, not under 40960"
done
# (c) append-only records only gain lines
{ git diff --name-only main...HEAD; git diff --name-only HEAD; } | sort -u | grep -E '^docs/tasks/[^/]+\.(reviews|verify|validation)/|^docs/decisions\.md$' > /tmp/t185.ao.$$
n=0
while IFS= read -r f; do
  [ -n "$f" ] || continue; n=$((n+1))
  rm1=$( { git diff main...HEAD -- "$f"; git diff HEAD -- "$f"; } | grep -E '^-' | grep -cvE '^--- (a/|/dev/null)' )
  [ "$rm1" -eq 0 ] && ok "FB-c:$f" || bad "FB-c:$f" "$f" "$rm1 removed line(s) in an append-only record"
done < /tmp/t185.ao.$$
[ "$n" -eq 0 ] && ok "FB-c:no-append-only-file-changed"
rm -f /tmp/t185.paths.$$ /tmp/t185.ao.$$

echo "== summary: $pass PASS, $fail FAIL, $manual MANUAL =="
[ "$fail" -eq 0 ]
