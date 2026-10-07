#!/usr/bin/env bash
# T-070 (validation 1, finding 1, mutation M6): the installer may reach Telegram only from a push
# to main or a v* tag after every earlier step succeeded. That rests on the `if:` of each step
# that reads the Telegram secrets; this tripwire pins it (decisions #93, docs/decisions/ci-toolchain.md).
#
# This check fails when a step that references secrets.TELEGRAM_BOT_TOKEN or
# secrets.TELEGRAM_CHAT_ID (anywhere in the step: env:, with:, run:; also the secrets['...'] form):
#   1. has no `if:`;
#   2. has an `if:` (blanks removed, `${{ }}` optional) without both top-level `&&` operands
#        github.event_name=='push'
#        (github.ref=='refs/heads/main'||startsWith(github.ref,'refs/tags/v'))
#      or with a top-level `||` (then nothing is a top-level `&&` operand);
#   3. has a status function in its `if:`: always(), failure(), cancelled() (also !cancelled()),
#      success() next to `||` (each would let the step run after a red step);
#   4. has `continue-on-error:`.
# A reference to the secrets outside a step's block (workflow or job env:, a flow-style steps:)
# is a violation too: this reader cannot tie it to one step's `if:` (fail-closed).
# Steps that do not reference the secrets are not judged (they may use always() etc.).
# The rule reads what the workflow writes (F-003). Not caught: an `if:` that is always false,
# a secret reached through a reusable workflow's `secrets: inherit`.
#
# Usage: scripts/ci/telegram-guard.sh <workflow-file>
# Exit 0: ok ("<n> step(s) read the Telegram secrets"). Exit 1: a violation (listed:
# <workflow-file>:<line>: why, <line> = the step's "- " item). Exit 2: usage error.
# Exit 3: cannot run (file missing, unreadable, no jobs:, awk error); never a pass.
set -uo pipefail
if [ "$#" -ne 1 ]; then
  echo "usage: scripts/ci/telegram-guard.sh <workflow-file>" >&2
  exit 2
fi
wf="$1"
cannot_run() { echo "telegram-guard: cannot run: $1" >&2; exit 3; }
[ -f "$wf" ] && [ -r "$wf" ] || cannot_run "$wf not found or not readable"

# Output lines: "V <file>:<line>: <why>", "C <n>" (steps that read the secrets), "N" (no jobs:).
report="$(awk -v file="$wf" '
function ind(s) { match(s, /^ */); return RLENGTH }
function nx(s) { gsub(/[ \t]/, "", s); return s }
function reads(s) {
  s = tolower(s)
  return s ~ /secrets[ \t]*\.[ \t]*telegram_(bot_token|chat_id)([^a-z0-9_]|$)/ ||
         s ~ /secrets[ \t]*\[[ \t]*["\047]telegram_(bot_token|chat_id)["\047][ \t]*\]/
}
# The scalar after "key:": quotes removed, a trailing comment dropped, a block indicator -> "".
function val(s,   q, n) {
  sub(/^[^:]*:/, "", s); sub(/^[ \t]+/, "", s); sub(/[ \t]+$/, "", s)
  if (s ~ /^#/ || s ~ /^[|>][-+0-9]*([ \t]+#.*)?$/) return ""
  q = substr(s, 1, 1)
  if (q == "\"") { s = substr(s, 2); n = match(s, /"[^"]*$/); return n ? substr(s, 1, n - 1) : s }
  if (q == "\047") { s = substr(s, 2); n = match(s, /\047[^\047]*$/); s = n ? substr(s, 1, n - 1) : s; gsub(/\047\047/, "\047", s); return s }
  sub(/[ \t]+#.*$/, "", s); return s
}
# 1 when s is "(...)" with the first "(" closing at the last character.
function wrapped(s,   i, c, d, q) {
  if (substr(s, 1, 1) != "(" || substr(s, length(s)) != ")") return 0
  d = 0; q = 0
  for (i = 1; i <= length(s); i++) {
    c = substr(s, i, 1)
    if (c == "\047") q = !q
    else if (!q && c == "(") d++
    else if (!q && c == ")") { d--; if (d == 0 && i < length(s)) return 0 }
  }
  return 1
}
function why(s) { reasons = reasons (reasons == "" ? "" : "; ") s }
function judge(expr,   e, l, i, c, d, q, n, ops, cur, hasor, k, ev, rf) {
  e = nx(expr)
  if (e ~ /^\$\{\{.*\}\}$/) e = substr(e, 4, length(e) - 5)
  while (wrapped(e)) e = substr(e, 2, length(e) - 2)
  if (e == "") { why("its if: is empty"); return }
  l = tolower(e)
  if (index(l, "always()")) why("its if: has always() (the step would run after a red step)")
  if (index(l, "failure()")) why("its if: has failure() (the step would run after a red step)")
  if (index(l, "cancelled()")) why("its if: has cancelled() or !cancelled() (the step would run after a red step)")
  if (index(l, "success()||") || index(l, "||success()")) why("its if: has success() as an || operand (the step would run after a red step)")
  n = 0; cur = ""; d = 0; q = 0; hasor = 0
  for (i = 1; i <= length(e); i++) {
    c = substr(e, i, 1)
    if (c == "\047") q = !q
    else if (!q && c == "(") d++
    else if (!q && c == ")") d--
    if (!q && d == 0 && substr(e, i, 2) == "&&") { ops[++n] = cur; cur = ""; i++; continue }
    if (!q && d == 0 && substr(e, i, 2) == "||") hasor = 1
    cur = cur c
  }
  ops[++n] = cur
  if (hasor) { why("its if: has a top-level || (the guard must be top-level && operands)"); return }
  ev = 0; rf = 0
  for (k = 1; k <= n; k++) {
    if (ops[k] == "github.event_name==\047push\047") ev = 1
    if (ops[k] == "(github.ref==\047refs/heads/main\047||startsWith(github.ref,\047refs/tags/v\047))") rf = 1
  }
  if (!ev) why("its if: lacks github.event_name == \047push\047 as a top-level && operand (a pull_request from main would send)")
  if (!rf) why("its if: lacks (github.ref == \047refs/heads/main\047 || startsWith(github.ref, \047refs/tags/v\047)) as a top-level && operand")
}
function endstep() {
  if (!sline) return
  collecting = 0
  if (sread) {
    count++
    reasons = ""
    if (!hasif) why("no if: (it needs github.event_name == \047push\047 and (main || v* tag) as top-level && operands)")
    else judge(ifv)
    if (coe) why("has continue-on-error: (a failed send must not be hidden as success)")
    if (reasons != "") { print "V " file ":" sline ": step reads the Telegram secrets, " reasons; viol++ }
  }
  sline = 0; sread = 0; hasif = 0; ifv = ""; coe = 0
}
# One key line of a step (already without its "- "), at the step key indent.
function stepkey(t) {
  if (t ~ /^["\047]?if["\047]?[ \t]*:([ \t]|$)/) { hasif = 1; ifv = val(t); collecting = 1 }
  else if (t ~ /^["\047]?continue-on-error["\047]?[ \t]*:([ \t]|$)/) coe = 1
}
{ line = $0; sub(/\r$/, "", line) }
line ~ /^[ \t]*(#.*)?$/ { next }
{
  i = ind(line); t = substr(line, i + 1)
  if (i == 0 && t ~ /^jobs[ \t]*:/) seenjobs = 1
  # The steps list ends at a line left of its items, or a non-item at the item indent.
  if (insteps && (i < itemind || (i == itemind && t !~ /^-([ \t]|$)/))) { endstep(); insteps = 0 }
  if (waitsteps) {
    waitsteps = 0
    if (t ~ /^-([ \t]|$)/ && i >= stepskeyind) { insteps = 1; itemind = i }
  }
  if (insteps && i == itemind) {
    endstep()
    sline = FNR
    match(t, /^-[ \t]*/); keyind = i + RLENGTH
    rest = substr(t, RLENGTH + 1)
    if (reads(rest)) sread = 1
    if (rest != "") stepkey(rest)
    next
  }
  if (sline) {
    if (reads(line)) sread = 1
    if (collecting && i > keyind) { ifv = ifv " " t; next }
    collecting = 0
    if (i == keyind) stepkey(t)
    next
  }
  if (reads(line)) { print "V " file ":" FNR ": references the Telegram secrets outside a step block (workflow or job env:, flow-style steps:): no single step if: guards it"; viol++ }
  if (t ~ /^(-[ \t]+)?steps[ \t]*:[ \t]*(#.*)?$/) { waitsteps = 1; stepskeyind = i }
}
END {
  endstep()
  if (!seenjobs) { print "N"; exit 0 }
  print "C " count + 0
}
' "$wf")" || cannot_run "awk failed on $wf"

grep -q '^N$' <<<"$report" && cannot_run "$wf has no top-level jobs: (not a workflow)"
n="$(sed -n 's/^C //p' <<<"$report")"
[ -n "$n" ] || cannot_run "no result from the reader for $wf"
if grep -q '^V ' <<<"$report"; then
  sed -n 's/^V //p' <<<"$report" >&2
  echo "telegram-guard: FAIL: a step that reads the Telegram secrets can run off main/v* tags or after a red step (T-070)" >&2
  exit 1
fi
echo "telegram-guard: ok: $wf: $n step(s) read the Telegram secrets, each guarded by push && (main || v* tag) with no status function"
