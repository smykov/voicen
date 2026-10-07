#!/usr/bin/env bash
# T-066 (rca, class ci-premise-unchecked-before-main; P-016, decisions #90): every premise
# that only CI can run is run on a wip/<ID> push before review. That path exists only while
# the CI workflow triggers on wip/** pushes, and it stays cheap only while no wip run saves
# a cache: the repo cache is at its 10 GB limit (T-058), and entries saved from wip/** would
# evict main's.
#
# This check fails when:
#   1. the CI workflow (default ci.yml in the workflows dir) does not have a block `on:` whose
#      `push:` has `branches:` naming both `main` and `wip/**` (flow list or block list, quoted
#      or not), or its `push:` carries `branches-ignore:`, `paths:` or `paths-ignore:` (they can
#      drop a wip push), or `on:` / `push:` is written inline;
#   2. in any *.yml / *.yaml at the top of the workflows dir, a step can save a cache on a ref
#      other than main. Every `uses:` is classified (fail-closed: an action not listed here is
#      a violation until it is classified):
#        never saves:   actions/checkout, actions/cache/restore, actions/upload-artifact,
#                       actions/download-artifact, dtolnay/rust-toolchain;
#        pnpm/action-setup          only with `cache:` unset or false;
#        Swatinem/rust-cache        only with `save-if: ${{ github.ref == 'refs/heads/main' }}`;
#        actions/cache/save         only with a step `if:` that has `github.ref == 'refs/heads/main'`
#                                   as one `&&` operand and no `||`;
#        actions/setup-node         only with `cache:` unset or empty and
#                                   `package-manager-cache: false` (its post step saves on every
#                                   ref; v5 has no switch to limit it);
#        actions/cache              never (its post step saves on every ref): use
#                                   actions/cache/restore + actions/cache/save.
#      A job-level `uses:` (reusable workflow), a flow-style step or `with:`, an anchor or merge
#      key in `with:`, or a non-comment `uses:` line this reader did not take as a step's or a
#      job's `uses:` is a violation too. Expressions are compared with blanks removed.
# The rule reads what the workflow writes, never a model of an action (F-003).
# Not caught: a cache written by a run step (e.g. docker buildx --cache-to type=gha), a
# composite action that caches inside, an `if:` that is always false on main.
#
# Usage: scripts/ci/ci-wip.sh [workflows-dir [ci-file-name]]
#        (defaults: .github/workflows, ci.yml)
# Exit 0: ok. Exit 1: a violation (listed: file:line: why). Exit 3: cannot run (dir missing,
# no workflow file, the CI file missing, a file without `jobs:`, awk error); never a pass.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
workflows="${1:-.github/workflows}"
ciname="${2:-ci.yml}"
cannot_run() { echo "ci-wip: cannot run: $1" >&2; exit 3; }
[ -d "$workflows" ] && [ -r "$workflows" ] || cannot_run "workflows dir $workflows not found or not readable"
[ -f "$workflows/$ciname" ] || cannot_run "CI workflow $workflows/$ciname not found"
shopt -s nullglob
files=("$workflows"/*.yml "$workflows"/*.yaml)
shopt -u nullglob
for f in "${files[@]}"; do
  [ -s "$f" ] && [ -r "$f" ] || cannot_run "$f is empty or not readable"
done

# Output lines: "V <file>:<line>: <why>", "N <file>" (no jobs:), "S <file> <uses read>".
report="$(awk -v ciname="$ciname" '
function ind(s) { match(s, /^ */); return RLENGTH }
# The scalar after "key:" with a quoted value unquoted and a trailing comment dropped.
function val(s,   q, r, i) {
  if (s ~ /^ *["\047]/) { sub(/^ *["\047][^"\047]*["\047]/, "", s) } else { sub(/^[^:]*/, "", s) }
  sub(/^ *:/, "", s); sub(/^ +/, "", s)
  if (s ~ /^#/) return ""
  q = substr(s, 1, 1)
  if (q == "\"" || q == "\047") { r = substr(s, 2); i = index(r, q); return i ? substr(r, 1, i - 1) : r }
  sub(/ +#.*$/, "", s); sub(/ +$/, "", s); return s
}
# The key of "key: ..." (a quoted key unquoted), "" when the line is not a mapping entry.
function key(s,   q, i, k) {
  sub(/^ */, "", s)
  q = substr(s, 1, 1)
  if (q == "\"" || q == "\047") {
    s = substr(s, 2); i = index(s, q); if (!i) return ""
    k = substr(s, 1, i - 1); if (substr(s, i + 1) !~ /^ *:( |$)/) return ""; return k
  }
  if (s !~ /^[A-Za-z0-9_.<-]+ *:( |$)/) return ""
  sub(/ *:.*$/, "", s); return s
}
function nx(s) { gsub(/[ \t]/, "", s); return s }
function unwrap(s) { s = nx(s); if (s ~ /^\$\{\{.*\}\}$/) s = substr(s, 4, length(s) - 5); return s }
function ismain(s) { return unwrap(s) == "github.ref==\047refs/heads/main\047" }
function mainif(s,   n, parts, k) {
  s = unwrap(s)
  if (s == "" || index(s, "||")) return 0
  n = split(s, parts, "&&")
  for (k = 1; k <= n; k++) if (parts[k] == "github.ref==\047refs/heads/main\047") return 1
  return 0
}
function isfalse(s) { return s ~ /^(false|False|FALSE)$/ }
# prevfile: at FNR == 1 the next file is already open while the previous one is finished.
function bad(ln, why) { print "V " prevfile ":" ln ": " why; viol++ }
function endstep(   a) {
  if (sline == 0) return
  if (uses != "") {
    parsed++
    a = tolower(uses); sub(/@.*$/, "", a)
    if (a == "actions/checkout" || a == "actions/cache/restore" || a == "actions/upload-artifact" || a == "actions/download-artifact" || a == "dtolnay/rust-toolchain") ;
    else if (flow) bad(sline, "job " job ": " uses " has a flow-style with: (write it as a block so its switches can be read)")
    else if (merge) bad(sline, "job " job ": " uses " has an anchor or merge key in with: (write the switches out)")
    else if (a == "pnpm/action-setup") { if (cache != "" && !isfalse(cache)) bad(sline, "job " job ": pnpm/action-setup with cache: " cache " saves the pnpm store on every ref (restore and save it with actions/cache/restore and a main-only actions/cache/save)") }
    else if (a == "swatinem/rust-cache") { if (!ismain(saveif)) bad(sline, "job " job ": Swatinem/rust-cache saves on every ref: set save-if: ${{ github.ref == \047refs/heads/main\047 }}" (saveif == "" ? "" : " (it has save-if: " saveif ")")) }
    else if (a == "actions/cache/save") { if (!mainif(ifv)) bad(sline, "job " job ": actions/cache/save runs on refs other than main: its if: must have github.ref == \047refs/heads/main\047 as an && operand and no ||" (ifv == "" ? " (no if:)" : " (if: " ifv ")")) }
    else if (a == "actions/setup-node") { if (cache != "" || !isfalse(pmc)) bad(sline, "job " job ": actions/setup-node caches, and its post step saves on every ref with no switch to limit it to main: set package-manager-cache: false and no cache:, and cache with actions/cache/restore plus a main-only actions/cache/save") }
    else if (a == "actions/cache") bad(sline, "job " job ": actions/cache saves in its post step on every ref: use actions/cache/restore, and actions/cache/save with if: github.ref == \047refs/heads/main\047")
    else bad(sline, "job " job ": " uses " is not classified (does it save a cache on a wip ref?); classify it in scripts/ci/ci-wip.sh")
  }
  sline = 0
}
function endjob() { endstep(); insteps = 0 }
function fileend() {
  endjob(); endtrigger()
  if (!seenjobs) print "N " prevfile
  else {
    if (raw > parsed + jobuses) { bad(rawline, "a non-comment uses: line that this reader did not take as a step or job uses: (" raw " such line(s), " parsed + jobuses " read; write steps as blocks)") }
    print "S " prevfile " " parsed
  }
  if (isci) {
    if (!seenon) bad(1, "no block on: (the CI workflow must trigger on push to main and wip/**)")
    else if (!seenpush) bad(online, "on: has no push: (the CI workflow must trigger on push to main and wip/**)")
    else if (!hasmain || !haswip) bad(pushline, "push: branches does not name " (!haswip ? "wip/**" : "") (!haswip && !hasmain ? " and " : "") (!hasmain ? "main" : "") " (T-066, P-016: the orchestrator pushes wip/<ID> for a pre-review run)")
  }
}
function endtrigger() { inon = 0; inpush = 0; inbranches = 0 }
function branch(b) { sub(/^ +/, "", b); sub(/ +$/, "", b); q = substr(b, 1, 1); if (q == "\"" || q == "\047") { b = substr(b, 2); sub(/["\047]$/, "", b) } if (b == "main") hasmain = 1; if (b == "wip/**") haswip = 1 }
FNR == 1 {
  if (NR > 1) fileend()
  n = split(FILENAME, pp, "/"); isci = (pp[n] == ciname)
  injobs = 0; jobind = -1; job = ""; sline = 0; insteps = 0
  seenjobs = 0; parsed = 0; jobuses = 0; raw = 0; rawline = 0
  seenon = 0; seenpush = 0; hasmain = 0; haswip = 0; inon = 0; inpush = 0; inbranches = 0
}
{ prevfile = FILENAME; line = $0 }
line ~ /^ *(#.*)?$/ { next }
{
  t = line; sub(/[ \t]#.*$/, "", t)
  if (t ~ /(^|[ \t{,-])uses[ \t]*:/) { raw++; if (!rawline) rawline = FNR }
  i = ind(line)
}
i == 0 {
  endjob(); endtrigger()
  k = key(line)
  injobs = (k == "jobs"); if (injobs) seenjobs = 1; jobind = -1; job = ""
  if (k == "on") {
    seenon = 1; online = FNR
    if (val(line) != "") { if (isci) bad(FNR, "on: is written inline (write on: push: branches: as a block)") }
    else { inon = 1; onchild = -1 }
  }
  next
}
inon {
  if (onchild < 0) onchild = i
  if (i == onchild) {
    inpush = 0; inbranches = 0
    if (key(line) == "push") {
      seenpush = 1; pushline = FNR
      if (val(line) != "") { if (isci) bad(FNR, "push: is written inline (write push: branches: as a block)") }
      else { inpush = 1; pushchild = -1 }
    }
    next
  }
  if (inpush && i > onchild) {
    if (pushchild < 0) pushchild = i
    if (i == pushchild) {
      inbranches = 0; k = key(line); v = val(line)
      if (k == "branches") {
        if (v == "") inbranches = 1
        else if (v ~ /^\[.*\]$/) { v = substr(v, 2, length(v) - 2); nb = split(v, bs, ","); for (b = 1; b <= nb; b++) branch(bs[b]) }
        else branch(v)
      } else if (k == "branches-ignore" || k == "paths" || k == "paths-ignore") {
        if (isci) bad(FNR, "push: has " k ": (it can drop a wip/** push; the CI workflow runs on every push to main and wip/**)")
      }
      next
    }
    if (inbranches && i > pushchild && line ~ /^ *- /) { b = line; sub(/^ *- */, "", b); sub(/ +#.*$/, "", b); branch(b) }
  }
  next
}
!injobs { next }
jobind < 0 { jobind = i }
i <= jobind { endjob(); job = key(line); next }
job == "" { next }
# Inside a job.
!insteps {
  if (key(line) == "steps" && val(line) == "") { insteps = 1; stepsind = i; stepind = -1 }
  else if (key(line) == "uses") { jobuses++; bad(FNR, "job " job ": a job-level uses: (reusable workflow) is not classified; its caches cannot be read here") }
  next
}
{
  isitem = (line ~ /^ *-( |$)/)
  if (isitem && stepind < 0 && i >= stepsind) stepind = i
  if (i < stepind || (i <= stepsind && !isitem)) {
    endstep(); insteps = 0
    if (key(line) == "steps" && val(line) == "") { insteps = 1; stepsind = i; stepind = -1 }
    else if (key(line) == "uses") { jobuses++; bad(FNR, "job " job ": a job-level uses: (reusable workflow) is not classified; its caches cannot be read here") }
    next
  }
  if (isitem && i == stepind) {
    endstep()
    sline = FNR; uses = ""; ifv = ""; cache = ""; pmc = ""; saveif = ""; flow = 0; merge = 0; inwith = 0
    match(line, /^ *- */); keyind = RLENGTH
    rest = substr(line, keyind + 1)
    if (rest == "") next
    line = sprintf("%" keyind "s", "") rest; i = keyind
  }
  if (sline == 0) next
  if (inwith && i > withind) {
    if (withchild < 0) withchild = i
    if (i == withchild) {
      k = key(line)
      if (k == "cache") cache = val(line)
      else if (k == "package-manager-cache") pmc = val(line)
      else if (k == "save-if") saveif = val(line)
      else if (k == "<<" || line ~ /^ *<< *:/) merge = 1
    }
    next
  }
  inwith = 0
  if (i == keyind) {
    k = key(line)
    if (k == "uses") uses = val(line)
    else if (k == "if") ifv = val(line)
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
    echo "ci-wip: FAIL: the CI workflow must run on every push to main and wip/** (the pre-review run"
    echo "of P-016), and no step may save a cache on a ref other than main (T-066, decisions #90;"
    echo "scripts/ci/ci-wip.sh header)."
    sed 's/^/  /' <<<"$viols"
  } >&2
  exit 1
fi
uses="$(awk '$1 == "S" { n += $3 } END { print n + 0 }' <<<"$report")"
echo "ci-wip: ok: $ciname push triggers on main and wip/**; $uses step uses: in ${#files[@]} workflow file(s) in $workflows, none saves a cache outside main"
