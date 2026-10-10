#!/usr/bin/env bash
# T-026 (decisions #102, #103): a GitHub Release may be created only by a push of a v* tag, after
# the gate and windows jobs passed and scripts/ci/release-check.sh accepted the tag in the same
# job; only that job may write contents. No wip/** run can show the publish step running, so this
# tripwire pins the workflow's shape (lesson of T-070 mutation M6; docs/decisions/ci-toolchain.md).
#
# A "publish step" is a step whose run: calls `gh release` create, upload, edit, delete or
# delete-asset (`gh release view` / `list` are reads and not judged). For every publish step:
#   1. its job has a job-level `if:` (blanks removed, `${{ }}` optional) with both top-level `&&`
#      operands github.event_name=='push' and startsWith(github.ref,'refs/tags/v'), no top-level
#      `||`, and no status function (always(), failure(), cancelled(), !cancelled(), success()
#      next to `||`);
#   2. its job `needs:` both gate and windows (flow or block list);
#   3. its job has no `continue-on-error:`;
#   4. an earlier step of the same job runs scripts/ci/release-check.sh with no `if:`, no
#      `continue-on-error:`, GH_TOKEN in its env: or the job's env: (so the release-exists
#      check runs), and a run: that, comment lines dropped, is exactly one
#      `bash scripts/ci/release-check.sh ...` line with no `;`, `|` or `&` (so no `||`, `;`,
#      pipe, `if`, `set +e` or later command can swallow the script's exit status; the step's
#      exit status is then the script's under any shell:);
#   5. its own `if:`, if any, has no status function;
# and:
#   6. `contents: write` appears only inside a job with a publish step (not at workflow level,
#      not in another job); `permissions: write-all` appears nowhere;
#   7. every `uses:` of a job with a publish step names an actions/* action (no third-party
#      release action).
# The rule reads what the workflow writes (F-003). Not caught: a release written through
# `gh api` or curl, an `if:` that is always false, a reusable workflow, a release-check.sh that
# itself exits 0 on a refusal (release-check.test.sh covers that), a call hidden in a quoted
# argument or `$(...)` on the one allowed line, a `shell:` that ignores the exit status.
#
# Usage: scripts/ci/release-guard.sh <workflow-file>
# Exit 0: ok ("<n> step(s) publish a release"). Exit 1: a violation, listed as
# <workflow-file>:<line>: why, <line> = the job key for 1-3; for 4 the release-check step's "- "
# item (bad if:, continue-on-error, no GH_TOKEN, run: not the call alone) or, with no release-check step before it, the
# publish step's; the step's "- " item for 5 and 7; the permissions line for 6.
# Exit 2: usage error. Exit 3: cannot run (file missing, unreadable, no jobs:, awk error);
# never a pass. Host test: scripts/ci/release-guard.test.sh (make check-release-guard).
set -uo pipefail
if [ "$#" -ne 1 ]; then
  echo "usage: scripts/ci/release-guard.sh <workflow-file>" >&2
  exit 2
fi
wf="$1"
cannot_run() { echo "release-guard: cannot run: $1" >&2; exit 3; }
[ -f "$wf" ] && [ -r "$wf" ] || cannot_run "$wf not found or not readable"

# Output lines: "V <file>:<line>: <why>", "C <n>" (publish steps), "N" (no jobs:).
report="$(awk -v file="$wf" '
function ind(s) { match(s, /^ */); return RLENGTH }
function nx(s) { gsub(/[ \t]/, "", s); return s }
function keyof(t,   k) { k = t; sub(/[ \t]*:.*$/, "", k); gsub(/["\047]/, "", k); return k }
function iskey(t) { return t ~ /^["\047]?[A-Za-z0-9_-]+["\047]?[ \t]*:([ \t]|$)/ }
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
function bare(expr,   e) {
  e = nx(expr)
  if (e ~ /^\$\{\{.*\}\}$/) e = substr(e, 4, length(e) - 5)
  while (wrapped(e)) e = substr(e, 2, length(e) - 2)
  return e
}
function statusfn(e,   l) {
  l = tolower(e)
  if (index(l, "always()")) why("its if: has always() (it would run after a red job or step)")
  if (index(l, "failure()")) why("its if: has failure() (it would run after a red job or step)")
  if (index(l, "cancelled()")) why("its if: has cancelled() or !cancelled() (it would run after a red job or step)")
  if (index(l, "success()||") || index(l, "||success()")) why("its if: has success() as an || operand (it would run after a red job or step)")
}
function judgejob(expr,   e, i, c, d, q, n, ops, cur, hasor, k, ev, rf) {
  e = bare(expr)
  if (e == "") { why("its if: is empty"); return }
  statusfn(e)
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
    if (ops[k] == "startsWith(github.ref,\047refs/tags/v\047)") rf = 1
  }
  if (!ev) why("its if: lacks github.event_name == \047push\047 as a top-level && operand")
  if (!rf) why("its if: lacks startsWith(github.ref, \047refs/tags/v\047) as a top-level && operand (it would publish off v* tags, wip/** included)")
}
function addneeds(s,   a, n, k) {
  gsub(/[][ \t"\047]/, "", s)
  n = split(s, a, ",")
  for (k = 1; k <= n; k++) if (a[k] != "") jneeds[nj] = jneeds[nj] " " a[k] " "
}
function endstep() {
  if (!cs) return
  if (srun[cs] ~ /(^|[^A-Za-z0-9_-])gh[ \t]+release[ \t]+(create|upload|edit|delete|delete-asset)([^A-Za-z0-9_-]|$)/) spub[cs] = 1
  if (srun[cs] ~ /release-check\.sh/) { scheck[cs] = 1; ssole[cs] = solecall(srun[cs]) }
  cs = 0; smode = ""
}
# 1 when run:, comment lines dropped, is exactly one `bash scripts/ci/release-check.sh ...` line
# with no ;, | or & (so no ||, &&, pipe or background): nothing can follow the call or swallow
# its exit status, and an `if`, `set +e` or second command makes a second line or no match.
function solecall(r,   a, n, k, m, one) {
  n = split(r, a, "\n"); m = 0; one = ""
  for (k = 1; k <= n; k++) {
    if (a[k] ~ /^[ \t]*(#.*)?$/) continue
    m++; one = a[k]
  }
  if (m != 1) return 0
  sub(/^[ \t]+/, "", one)
  if (one !~ /^bash[ \t]+scripts\/ci\/release-check\.sh([ \t]|$)/) return 0
  if (one ~ /[;|&]/) return 0
  return 1
}
function stepkey(t,   k) {
  k = keyof(t); smode = k
  if (k == "if") { sifh[cs] = 1; sif[cs] = val(t) }
  else if (k == "continue-on-error") scoe[cs] = 1
  else if (k == "run") srun[cs] = val(t)
  else if (k == "uses") suses[cs] = val(t)
  else if (k == "env") { if (t ~ /GH_TOKEN[ \t"\047]*:/) stok[cs] = 1 }
}
function jobkey(t,   k, v) {
  k = keyof(t); jmode = k
  if (k == "if") { jifh[nj] = 1; jif[nj] = val(t) }
  else if (k == "needs") { v = val(t); if (v != "") addneeds(v) }
  else if (k == "continue-on-error") jcoe[nj] = 1
  else if (k == "env") { if (t ~ /GH_TOKEN[ \t"\047]*:/) jtok[nj] = 1 }
  else if (k == "steps") { waitsteps = 1 }
}
{ line = $0; sub(/\r$/, "", line) }
line ~ /^[ \t]*(#.*)?$/ { next }
{
  i = ind(line); t = substr(line, i + 1)
  if (t ~ /^[^#]*["\047]?permissions["\047]?[ \t]*:[ \t]*["\047]?write-all/) { print "V " file ":" FNR ": permissions: write-all gives every scope write access"; viol++ }
  if (t ~ /^[^#]*["\047]?contents["\047]?[ \t]*:[ \t]*["\047]?write/) { nw++; wline[nw] = FNR; wjob[nw] = (injobs && i > jobind && nj) ? nj : 0 }
  if (i == 0) {
    endstep(); insteps = 0; waitsteps = 0
    injobs = (t ~ /^["\047]?jobs["\047]?[ \t]*:/)
    if (injobs) seenjobs = 1
    next
  }
  if (!injobs) next
  if (jobind == "") jobind = i
  if (i <= jobind) {
    endstep(); insteps = 0; waitsteps = 0
    if (i == jobind && iskey(t)) { nj++; jline[nj] = FNR; jkind = ""; jmode = "" }
    next
  }
  if (!nj) next
  if (jkind == "") jkind = i
  # The steps list ends at a line left of its items, or a non-item at the item indent.
  if (insteps && (i < itemind || (i == itemind && t !~ /^-([ \t]|$)/))) { endstep(); insteps = 0 }
  if (waitsteps && i > jkind) {
    waitsteps = 0
    if (t ~ /^-([ \t]|$)/) { insteps = 1; itemind = i }
  } else if (waitsteps) waitsteps = 0
  if (insteps) {
    if (i == itemind) {
      endstep()
      ns++; cs = ns; sjob[cs] = nj; sline[cs] = FNR; smode = ""
      match(t, /^-[ \t]*/); skind = i + RLENGTH
      rest = substr(t, RLENGTH + 1)
      if (rest != "") stepkey(rest)
      next
    }
    if (i == skind) { stepkey(t); next }
    if (smode == "run") srun[cs] = srun[cs] "\n" t
    else if (smode == "if") sif[cs] = sif[cs] " " t
    else if (smode == "env" && t ~ /^["\047]?GH_TOKEN["\047]?[ \t]*:/) stok[cs] = 1
    next
  }
  if (i == jkind) { jobkey(t); next }
  if (jmode == "if") jif[nj] = jif[nj] " " t
  else if (jmode == "needs" && t ~ /^-[ \t]/) { v = t; sub(/^-[ \t]*/, "", v); sub(/[ \t]+#.*$/, "", v); addneeds(v) }
  else if (jmode == "env" && t ~ /^["\047]?GH_TOKEN["\047]?[ \t]*:/) jtok[nj] = 1
}
END {
  endstep()
  if (!seenjobs) { print "N"; exit 0 }
  count = 0
  for (s = 1; s <= ns; s++) {
    if (!spub[s]) continue
    count++
    j = sjob[s]
    if (!jpub[j]) {
      jpub[j] = 1
      reasons = ""
      if (!jifh[j]) why("no job-level if: (it needs github.event_name == \047push\047 and startsWith(github.ref, \047refs/tags/v\047) as top-level && operands)")
      else judgejob(jif[j])
      if (index(jneeds[j], " gate ") == 0 || index(jneeds[j], " windows ") == 0) why("its needs: lacks gate or windows (needs:" jneeds[j] ")")
      if (jcoe[j]) why("has continue-on-error: (a failed publish would count as success)")
      if (reasons != "") { print "V " file ":" jline[j] ": job publishes a release, " reasons; viol++ }
      for (u = 1; u <= ns; u++)
        if (sjob[u] == j && suses[u] != "" && suses[u] !~ /^actions\//) { print "V " file ":" sline[u] ": uses " suses[u] " in the release job (gh only, actions/* only; no third-party release action)"; viol++ }
    }
    good = 0; nbad = 0
    for (u = 1; u < s; u++) {
      if (sjob[u] != j || !scheck[u]) continue
      reasons = ""
      if (sifh[u]) why("has an if: (a skipped check counts as success)")
      if (scoe[u]) why("has continue-on-error: (a refusal would not stop the publish)")
      if (!ssole[u]) why("its run: is not the release-check.sh call alone (comment lines aside, run: must be one `bash scripts/ci/release-check.sh ...` line with no ;, | or &; ||, ;, a pipe, if, set +e or a later command can swallow its exit status)")
      if (!stok[u] && !jtok[j]) why("has no GH_TOKEN in its env: or the job env: (the release-exists check would be skipped)")
      if (reasons == "") good = 1
      else { nbad++; bad[nbad] = "V " file ":" sline[u] ": release-check step before a publish step " reasons }
    }
    if (!good) {
      if (nbad) for (u = 1; u <= nbad; u++) { print bad[u]; viol++ }
      else { print "V " file ":" sline[s] ": publish step with no earlier scripts/ci/release-check.sh step in its job"; viol++ }
    }
    if (sifh[s]) {
      reasons = ""
      statusfn(bare(sif[s]))
      if (reasons != "") { print "V " file ":" sline[s] ": publish step " reasons; viol++ }
    }
  }
  for (k = 1; k <= nw; k++)
    if (!wjob[k] || !jpub[wjob[k]]) { print "V " file ":" wline[k] ": contents: write outside the job that publishes the release (" (wjob[k] ? "another job" : "workflow level") ")"; viol++ }
  print "C " count
}
' "$wf")" || cannot_run "awk failed on $wf"

grep -q '^N$' <<<"$report" && cannot_run "$wf has no top-level jobs: (not a workflow)"
n="$(sed -n 's/^C //p' <<<"$report")"
[ -n "$n" ] || cannot_run "no result from the reader for $wf"
if grep -q '^V ' <<<"$report"; then
  sed -n 's/^V //p' <<<"$report" >&2
  echo "release-guard: FAIL: a GitHub Release could be published off a v* tag push, after a red job or without release-check.sh, or contents: write is wider than the release job (T-026)" >&2
  exit 1
fi
echo "release-guard: ok: $wf: $n step(s) publish a release, each in a v* tag push job that needs gate and windows and runs release-check.sh first; contents: write only there"
