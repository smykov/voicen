#!/usr/bin/env bash
# T-068 (F-011 candidate, class ci-toolchain): no actions/setup-node step needs pnpm on the
# host unless an earlier step of the same job put it there with pnpm/action-setup. CI run
# 37498788064 failed in the Gate job at setup-node@v5 ("Unable to locate executable file:
# pnpm") after T-064 removed pnpm/action-setup from that job: setup-node caches for the
# package manager by itself and that cache runs `pnpm store path --silent` on the host. The
# dev host has pnpm on PATH, so only CI could show it.
#
# setup-node v5 semantics this rule is written against (actions/setup-node tag v5, a0853c24:
# action.yml, src/main.ts, src/cache-utils.ts; read in the T-068 investigation):
#   - `cache: <pm>` set (non-empty) -> restoreCache(<pm>), whatever package-manager-cache says.
#     npm ships with node; pnpm runs `pnpm store path` (needs host pnpm); yarn needs host yarn.
#   - `cache` unset or empty, `package-manager-cache` not false (default true) and package.json
#     names a packageManager (here pnpm@8.15.0) -> the same restoreCache(pnpm): needs host pnpm.
#   - `cache` unset or empty and `package-manager-cache: false` -> no cache, no package manager.
# (v6 auto-caches npm only; the rule stays sufficient there, only stricter.)
#
# The rule decides on switches written in the workflow, never on a model of the action
# (F-003). It does not read package.json: it assumes the packageManager may be pnpm. For each
# job under `jobs:` of every *.yml / *.yaml at the top of the workflows dir, a step whose
# `uses:` is actions/setup-node@<ref> passes only when one of:
#   (a) its block `with:` has `cache:` unset or empty and `package-manager-cache: false`
#       (false, False or FALSE, optionally quoted; an expression `${{ ... }}` is not false);
#   (b) its block `with:` has `cache: npm`;
#   (c) an earlier step of the same job uses pnpm/action-setup@<ref>, and `cache:` is unset,
#       empty or pnpm.
# Anything else is a violation, including shapes this reader does not follow (fail-closed):
# a flow-style `with: {...}` or step `- {uses: ...}`, an anchor/merge key in `with:`,
# `cache:` of any other value (yarn, an expression), or a non-comment line naming
# actions/setup-node that is not the `uses:` of a step it read (e.g. inside a run script).
# Not caught: an `if:` that skips the pnpm/action-setup step, a composite action or reusable
# workflow that calls setup-node, a pnpm reached some other way (a run step installing it).
#
# Usage: scripts/ci/setup-node-cache.sh [workflows-dir]   (default .github/workflows)
# Exit 0: ok. Exit 1: a violation (listed: file:line, job, why). Exit 3: cannot run (dir
# missing, no workflow file, a file without `jobs:`, awk error); never reported as a pass.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
workflows="${1:-.github/workflows}"
cannot_run() { echo "setup-node-cache: cannot run: $1" >&2; exit 3; }
[ -d "$workflows" ] && [ -r "$workflows" ] || cannot_run "workflows dir $workflows not found or not readable"
shopt -s nullglob
files=("$workflows"/*.yml "$workflows"/*.yaml)
shopt -u nullglob
[ "${#files[@]}" -gt 0 ] || cannot_run "no *.yml or *.yaml in $workflows"
for f in "${files[@]}"; do
  [ -s "$f" ] && [ -r "$f" ] || cannot_run "$f is empty or not readable"
done

# Output lines: "V <file>:<line>: job <job>: <why>", "N <file>" (no jobs:), "S <file> <steps>".
report="$(awk '
function ind(s) { match(s, /^ */); return RLENGTH }
# The scalar after "key:" with a quoted value unquoted and a trailing comment dropped.
function val(s,   q, r, i) {
  sub(/^[^:]*:/, "", s); sub(/^ +/, "", s)
  if (s ~ /^#/) return ""
  q = substr(s, 1, 1)
  if (q == "\"" || q == "\047") { r = substr(s, 2); i = index(r, q); return i ? substr(r, 1, i - 1) : r }
  sub(/ +#.*$/, "", s); sub(/ +$/, "", s); return s
}
function key(s) { sub(/^ */, "", s); if (s !~ /^[A-Za-z0-9_.<-]+ *:( |$)/) return ""; sub(/ *:.*$/, "", s); return s }
function bad(why) { print "V " FILENAME ":" sline ": job " job ": actions/setup-node step " why; viol++ }
function endstep() {
  if (sline == 0) return
  if (uses ~ /^actions\/setup-node(@|$)/) {
    parsed++
    if (flow) bad("has a flow-style with: (write it as a block so the switches can be read)")
    else if (merge) bad("has an anchor or merge key in with: (write the switches out)")
    else if (cache == "npm") ;
    else if (cache == "pnpm" && pnpm) ;
    else if (cache == "pnpm") bad("has cache: pnpm and no earlier pnpm/action-setup step in the job (setup-node runs pnpm store path on the host)")
    else if (cache != "") bad("has cache: " cache " (only npm, or pnpm after pnpm/action-setup in the same job)")
    else if (pmc ~ /^(false|False|FALSE)$/) ;
    else if (pnpm) ;
    else bad("needs host pnpm: no cache: and no package-manager-cache: false, and no earlier pnpm/action-setup step in the job (setup-node v5 caches the package.json packageManager by default and runs pnpm store path on the host; T-068, run 37498788064)")
  }
  if (uses ~ /^pnpm\/action-setup(@|$)/) pnpm = 1
  sline = 0
}
function endjob() { endstep(); insteps = 0; pnpm = 0 }
FNR == 1 {
  if (NR > 1) fileend()
  injobs = 0; jobind = -1; job = ""; sline = 0; insteps = 0; pnpm = 0
  seenjobs = 0; parsed = 0; raw = 0; rawline = 0
}
function fileend() {
  endjob()
  if (!seenjobs) print "N " prevfile
  else {
    if (raw > parsed) { print "V " prevfile ":" rawline ": a non-comment line names actions/setup-node outside a step uses: this reader follows (" raw " such line(s), " parsed " step(s) read)"; viol++ }
    print "S " prevfile " " parsed
  }
}
{ prevfile = FILENAME; line = $0 }
line ~ /^ *(#.*)?$/ { next }
{
  t = line; sub(/[ \t]#.*$/, "", t)
  if (t ~ /actions\/setup-node/) { raw++; if (!rawline) rawline = FNR }
  i = ind(line)
}
i == 0 {
  endjob(); injobs = (key(line) == "jobs"); if (injobs) seenjobs = 1; jobind = -1; job = ""; next
}
!injobs { next }
jobind < 0 { jobind = i }
i <= jobind { endjob(); job = key(line); next }
job == "" { next }
# Inside a job.
!insteps {
  if (key(line) == "steps" && val(line) == "") { insteps = 1; stepsind = i; stepind = -1 }
  next
}
{
  isitem = (line ~ /^ *-( |$)/)
  if (isitem && stepind < 0 && i >= stepsind) stepind = i
  if (i < stepind || (i <= stepsind && !isitem)) {
    endstep(); insteps = 0
    if (key(line) == "steps" && val(line) == "") { insteps = 1; stepsind = i; stepind = -1 }
    next
  }
  if (isitem && i == stepind) {
    endstep()
    sline = FNR; uses = ""; cache = ""; pmc = ""; flow = 0; merge = 0; inwith = 0
    match(line, /^ *- */); keyind = RLENGTH
    rest = substr(line, keyind + 1)
    if (rest == "" ) next
    line = sprintf("%" keyind "s", "") rest; i = keyind
  }
  if (sline == 0) next
  if (inwith && i > withind) {
    if (withchild < 0) withchild = i
    if (i == withchild) {
      k = key(line)
      if (k == "cache") cache = val(line)
      else if (k == "package-manager-cache") pmc = val(line)
      else if (k == "<<" || line ~ /^ *<< *:/) merge = 1
    }
    next
  }
  inwith = 0
  if (i == keyind) {
    k = key(line)
    if (k == "uses") uses = val(line)
    else if (k == "with") {
      v = val(line)
      if (v != "") { flow = 1 } else { inwith = 1; withind = keyind; withchild = -1 }
    }
  }
}
END { if (NR > 0) fileend(); exit (viol > 0) }
' "${files[@]}")"
rc=$?
[ "$rc" -le 1 ] || cannot_run "awk failed on $workflows (exit $rc)"

nojobs="$(grep '^N ' <<<"$report" | cut -c3-)"
[ -z "$nojobs" ] || cannot_run "no jobs: key read in: $(tr '\n' ' ' <<<"$nojobs")"
viols="$(grep '^V ' <<<"$report" | cut -c3-)"
if [ -n "$viols" ]; then
  {
    echo "setup-node-cache: FAIL: an actions/setup-node step needs pnpm on the host and nothing in its job puts it"
    echo "there. Set package-manager-cache: false (and no cache:) under its with:, or run pnpm/action-setup"
    echo "earlier in the same job (T-068, CI run 37498788064; scripts/ci/setup-node-cache.sh header)."
    sed 's/^/  /' <<<"$viols"
  } >&2
  exit 1
fi
steps="$(awk '$1 == "S" { n += $3 } END { print n + 0 }' <<<"$report")"
echo "setup-node-cache: ok: $steps actions/setup-node step(s) in ${#files[@]} workflow file(s) in $workflows; none needs host pnpm that its job does not set up"
