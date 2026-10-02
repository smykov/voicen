#!/usr/bin/env bash
# analysis-gate.sh — Claude Code PreToolUse gate (Edit|Write|Bash) + PostToolUse
# state keeper (Edit|Write). Tier T3.
#
# WHY. Without a step between "took the task" and "writing code", the developer
# agent diagnoses, designs and implements in one pass. A wrong root cause is then
# found by review — after the implementation is paid for — and the fix becomes one
# more patch. This gate moves the questions left: no analysis, no work.
#
# BLOCKS
#   1. An edit of docs/tasks/<ID>.md that moves the task INTO `status: IN_PROGRESS`
#      while its `analysis:` block is incomplete (fields: root_cause evidence
#      invariant seam approach; override with TEAMWRIGHT_ANALYSIS_FIELDS).
#   2. An edit of a code file while the active task (.teamwright/current-task or
#      $TEAMWRIGHT_TASK) is not IN_PROGRESS or has an incomplete analysis.
#   3. A Bash command that rewrites a task status in docs/tasks/ (sed -i, >, tee…):
#      status changes must go through Edit/Write so the gates can see them.
#
# ACTIVE TASK (PostToolUse): after an edit of docs/tasks/<ID>.md lands, the gate
#   writes <ID> to .teamwright/current-task when the task is IN_PROGRESS, and clears
#   the file when that task has left IN_PROGRESS. Check 2 therefore needs no agent
#   cooperation. PostToolUse runs only when the edit succeeded, so a denied or
#   failed edit never changes the active task.
#
# EXITS (deliberate, stay visible in the task file)
#   * fill the analysis block;
#   * `analysis: skipped:trivial` for changes with no design freedom (typo, bump);
#     not accepted for `type: rca`;
#   * `type: rca` tasks need their own block: root_cause, evidence, decision
#     (TEAMWRIGHT_RCA_FIELDS) - declaring a task rca is not a way around analysis;
#   * cannot prove the root cause yet -> BLOCKED with `blocked_on: <reason>`.
#
# SAFETY: fail-open on any internal error; silent on allow.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export TEAMWRIGHT_HOOKS_DIR="$HERE"
command -v python3 >/dev/null 2>&1 || exit 0

IFS= read -r -d '' _tw_py <<'PY' || true
import os, re, sys
sys.dont_write_bytecode = True
sys.path.insert(0, os.environ["TEAMWRIGHT_HOOKS_DIR"])
try:
    import _hookio as io, _task as task
except Exception:
    sys.exit(0)

data = io.load_input()
if not data:
    sys.exit(0)
tool = data.get("tool_name") or ""
ti = data.get("tool_input") or {}
root = io.project_root(data)

EXITS = ("Exits: fill the block; `analysis: skipped:trivial` if the change has no "
         "design freedom (not for rca tasks); if the root cause cannot be proven yet, "
         "move the task to BLOCKED with `blocked_on: <reason>`; for a recurring class, open a "
         "`type: rca` task.")
TEMPLATE = ("analysis:\n  root_cause: <what breaks + why>\n  evidence: <file:line, log line, "
            "failing test>\n  invariant: <what must stay true afterwards>\n  seam: <where "
            "the change goes; other places doing the same>\n  approach: <2-5 lines, and "
            "what was rejected>")
RCA_TEMPLATE = ("analysis:\n  root_cause: <the class, not the instance: why it keeps "
                "coming back>\n  evidence: <the recurrences: task ids, file:line, logs>\n"
                "  decision: <the exit and what it changes: (a) invariant + seam, or the "
                "owner's (b)/(c) decision in docs/decisions.md>")

def template_for(rec):
    return RCA_TEMPLATE if task.is_rca(rec) else TEMPLATE

def current_task_file():
    return os.path.join(root, ".teamwright", "current-task")

# ---- active task bookkeeping (PostToolUse: the edit has landed) ---------------
if io.event(data) == "PostToolUse":
    if tool not in ("Edit", "Write", "MultiEdit"):
        sys.exit(0)
    rel = io.rel_path(data, ti.get("file_path") or "")
    m = io.TASK_FILE_RE.search(rel)
    if not (m and rel.startswith("docs/tasks/")):
        sys.exit(0)
    tid = m.group(1)
    try:
        rec = task.parse(open(os.path.join(root, rel), encoding="utf-8", errors="replace").read())
    except OSError:
        sys.exit(0)
    cur = current_task_file()
    try:
        active = open(cur).read().strip()
    except OSError:
        active = ""
    try:
        if task.status(rec) == "IN_PROGRESS":
            if active != tid:
                io.state_dir(data)
                with open(cur, "w") as fh:
                    fh.write(tid + "\n")
        elif active == tid:
            os.remove(cur)
    except OSError:
        pass
    sys.exit(0)
if io.event(data) != "PreToolUse":
    sys.exit(0)

# ---- 3. status rewrite through the shell -------------------------------------
if tool == "Bash":
    cmd = ti.get("command") or ""
    if "docs/tasks/" in cmd and re.search(r"status", cmd) and re.search(
            r"(sed\s+(-[a-zA-Z]*i|--in-place)|perl\s+-[a-zA-Z]*i|>\s*\S*docs/tasks/|\btee\b)", cmd):
        io.deny("ANALYSIS GATE: task status must be changed with Edit/Write on "
                "docs/tasks/<ID>.md, not through the shell - otherwise the analysis and "
                "review gates cannot see the transition.", "analysis-gate")
    sys.exit(0)

if tool not in ("Edit", "Write", "MultiEdit"):
    sys.exit(0)
rel = io.rel_path(data, ti.get("file_path") or "")

# ---- 1. entering IN_PROGRESS -------------------------------------------------
m = io.TASK_FILE_RE.search(rel)
if m and rel.startswith("docs/tasks/"):
    before, after = io.before_after(data)
    if after is None:
        sys.exit(0)
    rb, ra = task.parse(before or ""), task.parse(after)
    if task.status(ra) == "IN_PROGRESS" and task.status(rb) != "IN_PROGRESS":
        missing = task.analysis_missing(ra)
        if missing:
            io.deny("ANALYSIS GATE: %s cannot enter IN_PROGRESS - the `analysis:` block "
                    "is incomplete (missing: %s). Add it in the same edit:\n%s\n%s"
                    % (m.group(1), ", ".join(missing), template_for(ra), EXITS), "analysis-gate")
    sys.exit(0)

# ---- 2. code edit for an active task -----------------------------------------
noncode = os.environ.get("TEAMWRIGHT_NONCODE_RE",
                         r"^(docs/|\.teamwright/|\.claude/|\.github/|[^/]*\.md$)")
if rel.startswith("..") or re.search(noncode, rel):
    sys.exit(0)
tid = os.environ.get("TEAMWRIGHT_TASK", "").strip()
if not tid:
    try:
        with open(os.path.join(root, ".teamwright", "current-task")) as fh:
            tid = fh.read().strip().split()[0]
    except (OSError, IndexError):
        tid = ""
if not tid:
    sys.exit(0)  # no active task declared: nothing to check against
tf = os.path.join(root, "docs", "tasks", tid + ".md")
try:
    rec = task.parse(open(tf, encoding="utf-8", errors="replace").read())
except OSError:
    io.deny("ANALYSIS GATE: active task %s has no record at docs/tasks/%s.md. Create "
            "the task record (or clear .teamwright/current-task) before editing code."
            % (tid, tid), "analysis-gate")
allowed = os.environ.get("TEAMWRIGHT_CODE_STATUSES", "IN_PROGRESS").split()
st = task.status(rec)
if st not in allowed:
    io.deny("ANALYSIS GATE: editing %s for task %s, but its status is %s (code edits "
            "are allowed only in: %s). Move the task to IN_PROGRESS first - that "
            "transition requires a complete analysis." % (rel, tid, st or "<none>",
            " ".join(allowed)), "analysis-gate")
missing = task.analysis_missing(rec)
if missing:
    io.deny("ANALYSIS GATE: task %s has no complete analysis (missing: %s); code edits "
            "are blocked until it does.\n%s\n%s" % (tid, ", ".join(missing), template_for(rec), EXITS),
            "analysis-gate")
PY
python3 -c "$_tw_py"
exit 0
