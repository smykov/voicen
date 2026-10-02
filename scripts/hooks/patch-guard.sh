#!/usr/bin/env bash
# patch-guard.sh — commit-time check for "patch instead of root cause". Tier T2/T3.
#
# WHY. Agents tend to fix a class of bugs by adding one more `if`, one more
# allowlist entry, one more recogniser regex — each fix looks reasonable, review
# does not notice the pattern, and the class keeps coming back. This guard flags
# such additions in an area that has already been fixed before.
#
# BEHAVIOUR (patterns and thresholds: patch-guard.conf / .teamwright/patch-guard.conf)
#   * patch signature added in an area with >= PG_PRIOR_FIXES fix commits within the
#     last PG_LOOKBACK_DAYS days (default 90; 0 = whole history) -> warning by
#     default (PG_MODE=warn); a BLOCK only when the project opts in with PG_MODE=block;
#   * patch signature added anywhere else -> warning.
#   Area = the file's directory (PG_AREA=dir) or the file itself (PG_AREA=file);
#   a file at the repository root is always its own area, never ".".
#
# DELIBERATE OVERRIDE (stays in history):
#   * trailer `RCA: T-050` naming an existing `type: rca` task whose analysis block
#     (root_cause, evidence, decision) is filled; a task id mentioned anywhere else
#     in the message does not count;
#   * trailer `Principle-Override: P-001 - <why this is not a patch>`;
#   * one-off, human only: PATCH_GUARD=off git commit ...
#
# Usage: patch-guard.sh <commit-message-file>   (called from the commit-msg hook)
set -uo pipefail

[ "${PATCH_GUARD:-}" = "off" ] && exit 0
MSG_FILE="${1:-}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || exit 0
cd "$ROOT" || exit 0
command -v python3 >/dev/null 2>&1 || exit 0

# shellcheck source=patch-guard.conf
[ -f "$HERE/patch-guard.conf" ] && . "$HERE/patch-guard.conf"
# shellcheck disable=SC1091
[ -f "$ROOT/.teamwright/patch-guard.conf" ] && . "$ROOT/.teamwright/patch-guard.conf"

export PG_MODE="${PG_MODE:-warn}" PG_PRIOR_FIXES="${PG_PRIOR_FIXES:-2}" PG_AREA="${PG_AREA:-dir}"
export PG_LOOKBACK_DAYS="${PG_LOOKBACK_DAYS:-90}"
export PG_FIX_SUBJECT_RE="${PG_FIX_SUBJECT_RE:-^fix}" PG_EXCLUDE_RE="${PG_EXCLUDE_RE:-^$}"
export PG_SIGNATURES="${PG_SIGNATURES:-}" MSG_FILE TEAMWRIGHT_HOOKS_DIR="$HERE"

IFS= read -r -d '' _tw_py <<'PY' || true
import os, re, subprocess, sys
sys.dont_write_bytecode = True
sys.path.insert(0, os.environ["TEAMWRIGHT_HOOKS_DIR"])
try:
    import _task as task
except Exception:
    sys.exit(0)

def git(*args):
    return subprocess.run(("git",) + args, capture_output=True, text=True).stdout

msg = ""
try:
    msg = open(os.environ.get("MSG_FILE") or "", encoding="utf-8", errors="replace").read()
except OSError:
    pass
msg = "\n".join(l for l in msg.splitlines() if not l.startswith("#"))

# ---- deliberate overrides ------------------------------------------------------
if re.search(r"^Principle-Override:\s*P-\d+\s*\S.{3,}", msg, re.M | re.I):
    sys.exit(0)
rca_refs = re.findall(r"^RCA:\s*(" + task.TASK_ID + r")\s*$", msg, re.M)
rca_notes = []
for tid in rca_refs:
    try:
        rec = task.parse(open(os.path.join("docs", "tasks", tid + ".md"),
                              encoding="utf-8", errors="replace").read())
    except OSError:
        rca_notes.append("%s: no docs/tasks/%s.md" % (tid, tid))
        continue
    if not task.is_rca(rec):
        rca_notes.append("%s: not a `type: rca` task" % tid)
        continue
    missing = task.analysis_missing(rec)
    if missing:
        rca_notes.append("%s: analysis incomplete (missing: %s)" % (tid, ", ".join(missing)))
        continue
    sys.exit(0)

# ---- signatures in the staged diff --------------------------------------------
sigs = []
for line in os.environ.get("PG_SIGNATURES", "").splitlines():
    if "|" in line:
        name, rx = line.split("|", 1)
        try:
            sigs.append((name.strip(), re.compile(rx)))
        except re.error:
            print("patch-guard: bad regex for %s ignored" % name, file=sys.stderr)
if not sigs:
    sys.exit(0)
exclude = re.compile(os.environ.get("PG_EXCLUDE_RE") or "^$")
# files the teamwright installer wrote (hooks, metrics, tw-run) are the kit's, not a patch
installed = set()
try:
    import json
    installed = {k for k in json.load(open(os.path.join(".teamwright", "installed.json")))
                 if not k.startswith("@")}
except Exception:
    pass
hits = {}   # file -> [(sig, line)]
cur = None
for line in git("diff", "--cached", "--unified=0", "--no-color", "--no-ext-diff").splitlines():
    if line.startswith("+++ "):
        p = line[4:].strip()
        cur = p[2:] if p.startswith("b/") else None
        if cur and (exclude.search(cur) or cur in installed):
            cur = None
        continue
    if cur and line.startswith("+") and not line.startswith("+++"):
        body = line[1:]
        for name, rx in sigs:
            if rx.search(body):
                hits.setdefault(cur, []).append((name, body.strip()[:90]))
                break
if not hits:
    sys.exit(0)

fix_re = re.compile(os.environ.get("PG_FIX_SUBJECT_RE") or "^fix", re.I)
threshold = int(os.environ.get("PG_PRIOR_FIXES") or 2)
area_mode = os.environ.get("PG_AREA", "dir")
try:
    lookback = int(os.environ.get("PG_LOOKBACK_DAYS") or 0)
except ValueError:
    lookback = 90
import time
# Filter by commit time here, not with `git log --since`: --since stops walking at
# the first older commit, so one old-dated commit (rebase, import) hides the rest.
cutoff = time.time() - lookback * 86400 if lookback > 0 else None
cache, blocking, warning = {}, [], []
for f, lst in sorted(hits.items()):
    # a root-level file is its own area: "." would count every fix in the repository
    area = f if area_mode == "file" else (os.path.dirname(f) or f)
    if area not in cache:
        n = 0
        for line in git("log", "--format=%ct %s", "--", area).splitlines():
            ts, _, subj = line.partition(" ")
            if cutoff is not None and ts.isdigit() and int(ts) < cutoff:
                continue
            if fix_re.search(subj):
                n += 1
        cache[area] = n
    (blocking if cache[area] >= threshold else warning).append((f, area, cache[area], lst))

def show(entries, limit=None):
    for f, area, n, lst in entries[:limit]:
        print("  %s  (area %s: %d fix commit(s)%s)" % (
        f, area, n, " in %d days" % lookback if lookback > 0 else ""), file=sys.stderr)
        for name, body in lst[:3]:
            print("      + [%s] %s" % (name, body), file=sys.stderr)

if warning:
    print("\npatch-guard: warning - patch-like additions (one more case / allowlist "
          "entry). Ask: what escapes next?", file=sys.stderr)
    show(warning, 5)
    if len(warning) > 5:
        print("  ... and %d more file(s)" % (len(warning) - 5), file=sys.stderr)
if not blocking:
    sys.exit(0)
mode = os.environ.get("PG_MODE", "warn")
print("\npatch-guard: %s - patch-like additions in an area that was already fixed "
      "%d+ times:" % ("BLOCKED" if mode == "block" else "warning", threshold), file=sys.stderr)
show(blocking)
for note in rca_notes:
    print("  RCA trailer not accepted - %s" % note, file=sys.stderr)
print("""
  A repeated fix in the same area is a recurrence. Do not add another case:
    1. open a `type: rca` task, fill its analysis (root_cause, evidence,
       decision) and add the trailer `RCA: T-050` to this commit message, or
    2. if this is genuinely not a patch (parsing, validation, data), add
       `Principle-Override: P-00X - <why>` to the commit message.
  Both remain in history for the reviewer.
""", file=sys.stderr)
sys.exit(1 if mode == "block" else 0)
PY
python3 -c "$_tw_py"
