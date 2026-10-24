#!/usr/bin/env bash
# review-gate.sh — Claude Code PreToolUse gate (Edit|Write|Bash). Tier T3.
#
# WHY. A review step that is only written down gets skipped under time pressure,
# and a self-review ("I checked my own diff") catches almost nothing. Worse, a
# reviewer who accepts the Nth patch for the same class of bug lets the class grow.
#
# IDENTITY. Claude Code gives every hook call `session_id`; a subagent SHARES its
# parent's session_id and differs only by `agent_id` / `agent_type`. So an agent is
# identified by session_id + agent_id (top level = "main"), and "is this the
# reviewer" is answered by agent_type (default `code-reviewer`,
# $TEAMWRIGHT_REVIEWER_AGENT). These fields come from the runtime, not the agent.
#
# REVIEW RECORD: docs/tasks/<ID>.reviews/<n>.md (n = round, 1, 2, ...):
#   ---
#   task: T-042                 # must be the task of the .reviews/ directory
#   round: 1
#   reviewer: code-reviewer
#   verdict: APPROVE            # APPROVE | REQUEST_CHANGES | REJECT_RECURRENCE | ESCALATE
#   recurrence_of: [T-017]      # REJECT_RECURRENCE only
#   rca_task: T-050             # REQUIRED with REJECT_RECURRENCE
#   ---
#
# BLOCKS
#   1. Writing a review record from any agent whose agent_type is not the reviewer
#      type, or from an agent that implemented the task (self-review never counts).
#   2. docs/tasks/<ID>.md entering `status: CODE_COMPLETE` unless the latest round
#      says APPROVE and was written by a reviewer agent (ledger entry below), that
#      agent did not implement the task, and the journal (.teamwright/logs/
#      tools.jsonl, written by journal.sh) shows the reviewer was spawned for this
#      task in that session, and the approval is newer than the task's latest FAIL
#      verify/acceptance record (an APPROVE from before a VERIFY_FAIL does not carry
#      over). Who moves the status does not matter (normally the
#      orchestrator); who wrote the approval does.
#   3. A review record with verdict REJECT_RECURRENCE that does not link an
#      existing `type: rca` task via `rca_task:`.
#   6. With `process.review.blocking` set in .teamwright/config.yml: an APPROVE whose
#      findings table (first Severity table, under "Findings" if headed) holds a blocking row (severity at or above the threshold,
#      Critical, or an always-blocking category), or a REQUEST_CHANGES with none.
#      Without the key, findings are not checked (behaviour before settings existed).
#   4. Any verdict outside the four above; and, after 2 rounds of REQUEST_CHANGES in
#      a row, any verdict but ESCALATE or REJECT_RECURRENCE for the third record -
#      neither another send-back nor an APPROVE of the third attempt. Rounds are
#      counted from the .reviews/ files, never from the task's own field; an APPROVE
#      or ESCALATE ends a run (a task back after VERIFY_FAIL or an owner decision
#      starts a new count).
#   5. Shell commands that write review records or the local ledgers/journal, and any shell
#      write into a review/verify/validation record directory or a stamp, after `cd` too
#      (_hookio.shell_writes_records, best effort).
#   7. A review that spawns too many tasks: creating a new docs/tasks/<ID>.md in a
#      session whose latest journaled spawn of the main agent was a reviewer for
#      task R (tools.jsonl; a later spawn of another role ends that window), when
#      that review already created 2 new task records ($TEAMWRIGHT_REVIEW_NEW_TASKS)
#      - lifecycle.md section 6: one reviewed task spawns at most two new tasks;
#      more goes into one umbrella task. Counted per reviewed task and spawn in
#      .teamwright/sessions/<R>.new-tasks.jsonl; a retry of the same id is not new. And
#      at R's status move out of NEEDS_REVIEW (in that window) every task record new since
#      the reviewer's spawn counts, however it was written (git: added in a commit since
#      then, or untracked/added in the tree) - a record created by the shell or a script.
#   0. (state guard, _hookio.guard_state) Edit/Write by any agent to
#      .teamwright/config.yml or installed.json, and shell commands that actually
#      write, move or remove them (hard deny in every mode: agents cannot switch the
#      gates to gate-first; reading, `git add` / `git commit` of them is allowed;
#      changes go through config.next.yml + `tw-install.py apply --config`),
#      Edit/Write to the ledgers/journal, and tw-install.py run by a subagent.
#
# LEDGERS (local, gitignored, written by this hook = facts, not claims):
#   .teamwright/sessions/<ID>.dev            agents that started (-> IN_PROGRESS,
#       except a NEEDS_REVIEW -> IN_PROGRESS send-back) or submitted (-> NEEDS_REVIEW)
#   .teamwright/sessions/<ID>.reviews.jsonl  who wrote which review round, verdict (written
#       with the stamp in PostToolUse, once the record write landed)
# STAMP (committed with the record): docs/tasks/<ID>.reviews/<n>.gate.json - the same
#   ledger line plus whether the reviewer's spawn was journaled, the record it belongs to
#   (task, path, round: a copy into another task or round does not count) and its sha256.
#   Check 2 uses it only when this checkout has no review ledger for the task at all (a
#   fresh checkout, e.g. a CI job); where the ledger exists it alone decides. Only the gate
#   writes it; an edited record no longer matches its hash; pre-commit refuses a staged
#   stamp the local ledger does not have. Not signed: see gates.md, Gate stamps.
#
# HUMAN REVIEW. A person reviewing outside Claude Code writes the record with
# `reviewer: human:<name>` and commits it; an agent cannot write that marker.
# EXIT: `review: skipped:trivial` in the task record (visible to humans later).
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
try:
    import _config
except Exception:
    _config = None

VERDICTS = ("APPROVE", "REQUEST_CHANGES", "REJECT_RECURRENCE", "ESCALATE")
try:
    ESCALATE_AFTER = int(os.environ.get("TEAMWRIGHT_ESCALATE_AFTER") or 2)
except ValueError:
    ESCALATE_AFTER = 2
REVIEWER = io.REVIEWER_AGENT
data = io.load_input()
if data and io.event(data) == "PostToolUse" and hasattr(io, "commit_record"):
    # the record write landed: its pending ledger line and stamp become evidence
    if (data.get("tool_name") or "") in ("Edit", "Write", "MultiEdit"):
        _rel = io.rel_path(data, (data.get("tool_input") or {}).get("file_path") or "")
        if io.REVIEW_FILE_RE.search(_rel) and _rel.startswith("docs/tasks/"):
            io.commit_record(io.project_root(data), _rel)
    sys.exit(0)
if not data or io.event(data) != "PreToolUse":
    sys.exit(0)
tool = data.get("tool_name") or ""
if tool not in ("Edit", "Write", "MultiEdit", "Bash"):
    sys.exit(0)
root = io.project_root(data)
ti = data.get("tool_input") or {}
who = io.actor(data)
key = io.actor_key(who)

# ---- 0. teamwright's own state (config, manifest, ledgers): never by agents ----
io.guard_state(data)

# ---- 5. shell writes into review records / ledgers ----------------------------
if tool == "Bash":
    cmd = ti.get("command") or ""
    target = r"(docs/tasks/[^\s'\"]*\.reviews/|\.teamwright/(sessions|logs|current-task))"
    if re.search(target, cmd) and (
            re.search(r">\s*['\"]?\S*" + target, cmd)
            or re.search(r"\b(tee|cp|mv|rm|truncate|touch|ln|install)\b", cmd)
            or re.search(r"\b(sed|perl)\s+-[a-zA-Z]*i", cmd)):
        io.deny("REVIEW GATE: review records and .teamwright/ ledgers are written only "
                "through Edit/Write (review records: by a %s agent), never by shell "
                "commands - otherwise the gate cannot tell who wrote them." % REVIEWER,
                "review-gate")
    try:
        rec_write = io.shell_writes_records(cmd, root) if hasattr(io, "shell_writes_records") else False
    except Exception:
        rec_write = False
    if rec_write:
        io.deny("REVIEW GATE: this command writes, copies, restores or removes a review, verify or "
                "validation record or a gate stamp (docs/tasks/<ID>.reviews|verify|validation/, "
                "*.gate.json), never by shell commands: records are written through Edit/Write by "
                "their agent, stamps only by the gates. A copy from another task or an old version "
                "does not count anyway.", "review-gate")
    sys.exit(0)

rel = io.rel_path(data, ti.get("file_path") or "")

def sessions_dir():
    return os.path.join(root, ".teamwright", "sessions")

def dev_keys(tid):
    try:
        return {l.strip() for l in open(os.path.join(sessions_dir(), tid + ".dev")) if l.strip()}
    except OSError:
        return set()

def record_dev(tid):
    if not key or key in dev_keys(tid):
        return
    try:
        io.state_dir(data, "sessions")
        with open(os.path.join(sessions_dir(), tid + ".dev"), "a") as fh:
            fh.write(key + "\n")
    except OSError:
        pass

def review_ledger(tid):
    return os.path.join(sessions_dir(), tid + ".reviews.jsonl")

def ledger_entry(tid, n):
    """Latest ledger entry for round n (a later rewrite of the round wins)."""
    hit = None
    for r in io.read_jsonl(review_ledger(tid)):
        if r.get("round") == n:
            hit = r
    return hit

def spawned_reviewer(tid, sid):
    for r in io.read_jsonl(os.path.join(io.log_dir(data), "tools.jsonl")):
        if (r.get("event") == "pre" and r.get("tool") in ("Agent", "Task")
                and r.get("session_id") == sid and r.get("subagent_type") == REVIEWER
                and tid in (r.get("task_ids") or [])):
            return True
    return False

def read_rec(path):
    try:
        return task.parse(open(path, encoding="utf-8", errors="replace").read())
    except OSError:
        return None

def rca_ok(rca):
    rca = (rca or "").strip().strip("[]").split(",")[0].strip()
    if not rca:
        return False
    return task.is_rca(read_rec(os.path.join(root, "docs", "tasks", rca + ".md")))

def is_human(reviewer):
    return reviewer.strip().lower().startswith("human")

def changes_in_a_row(recs):
    """REQUEST_CHANGES rounds since the last APPROVE / ESCALATE / REJECT_RECURRENCE:
    the send-backs of the current review run."""
    k = 0
    for rec in recs:
        k = k + 1 if task.first_word(rec.get("verdict")) == "REQUEST_CHANGES" else 0
    return k

RCA_MSG = ("A recurrence is not fixed with another patch: open a separate task with "
           "`type: rca` (root cause + architectural decision in docs/decisions), link "
           "it here with `rca_task: <ID>`, and set `recurrence_of:` on the original task.")

# ---- 1, 3, 4. review record being written --------------------------------------
m = io.REVIEW_FILE_RE.search(rel)
if m and rel.startswith("docs/tasks/"):
    tid, n = m.group(1), int(m.group(2))
    _, after = io.before_after(data)
    if after is None:
        sys.exit(0)
    rv = task.parse(after)
    verdict = task.first_word(rv.get("verdict"))
    if is_human(rv.get("reviewer") or ""):
        io.deny("REVIEW GATE: `reviewer: human:...` marks a review written by a person "
                "outside Claude Code; an agent cannot write it.", "review-gate")
    if key and key in dev_keys(tid):
        io.deny("REVIEW GATE: this agent implemented %s; its review does not count. "
                "Spawn a fresh %s subagent (not a fork of the author) to review it."
                % (tid, REVIEWER), "review-gate")
    if who["agent_type"] != REVIEWER:
        io.deny("REVIEW GATE: only a %s agent writes review records (this call comes "
                "from agent_type=%r). Spawn a fresh %s subagent and name %s in its "
                "prompt. A person reviewing outside Claude Code writes the file with "
                "`reviewer: human:<name>` and commits it."
                % (REVIEWER, who["agent_type"] or "<main session>", REVIEWER, tid),
                "review-gate")
    if (rv.get("task") or "").strip() != tid:
        io.deny("REVIEW GATE: review record docs/tasks/%s.reviews/%d.md needs `task: %s` in its "
                "front matter (got %r): a review counts only for the task it names."
                % (tid, n, tid, (rv.get("task") or "").strip()), "review-gate")
    if verdict not in VERDICTS:
        io.deny("REVIEW GATE: review record needs front matter with `verdict:` one of "
                "%s (got %r)." % (" | ".join(VERDICTS), verdict), "review-gate")
    if verdict == "REJECT_RECURRENCE" and not rca_ok(rv.get("rca_task")):
        io.deny("REVIEW GATE: verdict REJECT_RECURRENCE without a linked rca task "
                "(rca_task: %r does not point to an existing docs/tasks/<ID>.md with "
                "`type: rca`). %s" % (rv.get("rca_task", ""), RCA_MSG), "review-gate")
    proc = _config.read_process(os.path.join(root, _config.CONFIG)) if _config and hasattr(_config, "read_process") else None
    if proc and "review.blocking" in proc["explicit"] and verdict in ("APPROVE", "REQUEST_CHANGES"):
        # 6. the verdict must match the findings table under the owner's threshold
        th = proc["review"]["blocking"]
        rows = task.findings(after) if hasattr(task, "findings") else []
        bad_sev = sorted({sv for sv, _ in rows if sv.lower() not in ("critical", "high", "medium", "low")})
        if bad_sev:
            io.deny("REVIEW GATE: findings need Severity Critical | High | Medium | Low (got %s)."
                    % ", ".join(bad_sev), "review-gate")
        blocking = [(sv, c) for sv, c in rows if _config.severity_blocks(sv, c, th)]
        if verdict == "APPROVE" and blocking:
            io.deny("REVIEW GATE: APPROVE with %d blocking finding(s) (%s) under threshold %r: "
                    "a finding at or above the threshold, Critical, or of category %s returns "
                    "the task - write REQUEST_CHANGES." % (
                        len(blocking), ", ".join("%s/%s" % (sv, c or "-") for sv, c in blocking), th,
                        "/".join(_config.ALWAYS_BLOCKING)), "review-gate")
        if verdict == "REQUEST_CHANGES" and not blocking:
            io.deny("REVIEW GATE: REQUEST_CHANGES without a blocking finding under threshold "
                    "%r (process.review.blocking): findings below it are follow-ups, not a "
                    "send-back - write APPROVE and keep them in the table. A finding that must "
                    "return the task needs its real severity." % th, "review-gate")
    prior_rc = changes_in_a_row([rec for r, rec in task.review_rounds(root, tid) if r < n])
    if verdict not in ("ESCALATE", "REJECT_RECURRENCE") and prior_rc >= ESCALATE_AFTER:
        io.deny("REVIEW GATE: %s already has %d review round(s) with REQUEST_CHANGES in a "
                "row (counted from docs/tasks/%s.reviews/), so round %d is the third "
                "attempt: write verdict ESCALATE (the owner decides) or REJECT_RECURRENCE, "
                "not %s." % (tid, prior_rc, tid, n, verdict), "review-gate")
    io.state_dir(data, "sessions")
    line = dict(who, ts=io.now(), round=n, verdict=verdict)
    if hasattr(io, "stage_record"):
        # ledger line and stamp once the write lands (PostToolUse); the stamp is committed with
        # the record: a status move on a fresh checkout (a CI job, no ledger) checks it instead
        io.stage_record(root, rel, ".teamwright/sessions/%s.reviews.jsonl" % tid, line,
                        dict(line, gate="review", spawned=bool(not who["agent_id"] or
                                                               spawned_reviewer(tid, who["session_id"]))),
                        after)
    else:
        io.append_jsonl(review_ledger(tid), line)
    sys.exit(0)

# ---- 2. task record transition --------------------------------------------------
m = io.TASK_FILE_RE.search(rel)
if not (m and rel.startswith("docs/tasks/")):
    sys.exit(0)
tid = m.group(1)

# ---- 7. new task records created by one review ----------------------------------
def review_window(sid):
    """(reviewed task, window id) when this session's latest spawn by its main agent was
    a reviewer naming a task; None otherwise (no review running, or the session moved on
    to another role)."""
    win = None
    for i, r in enumerate(io.read_jsonl(os.path.join(io.log_dir(data), "tools.jsonl"))):
        if (r.get("event") != "pre" or r.get("tool") not in ("Agent", "Task")
                or r.get("session_id") != sid or r.get("agent_id")):
            continue
        ids = r.get("task_ids") or []
        win = (ids[0], "%s:%d" % (sid, i), r.get("ts") or "") if r.get("subagent_type") == REVIEWER and ids \
            else None
    return win

def _epoch(ts):
    import calendar, time
    try:
        return calendar.timegm(time.strptime(ts, "%Y-%m-%dT%H:%M:%SZ"))
    except (TypeError, ValueError):
        return None

def new_records_since(since, reviewed):
    """Task records docs/tasks/<ID>.md that appeared since `since` (epoch seconds), however
    they were written (Edit/Write, a shell redirect, cp, a script): added in a commit since
    then, or new in the tree (untracked or added to the index) and modified since then."""
    import subprocess
    rx = re.compile(r"^docs/tasks/(%s)\.md$" % task.TASK_ID)

    def git(*args):
        try:
            p = subprocess.run(("git",) + args, cwd=root, capture_output=True, text=True, timeout=20)
        except Exception:
            return ""
        return p.stdout if p.returncode == 0 else ""
    found, when = set(), None
    for line in git("log", "-n", "200", "--diff-filter=A", "--name-only", "--format=@%ct", "--",
                    "docs/tasks").splitlines():
        if line.startswith("@"):
            when = int(line[1:]) if line[1:].isdigit() else None
        elif when is not None and when >= since and rx.match(line.strip()):
            found.add(rx.match(line.strip()).group(1))
    for line in git("status", "--porcelain", "--untracked-files=all", "--", "docs/tasks").splitlines():
        code, path = line[:2], line[3:].strip()
        m2 = rx.match(path)
        if m2 and (code == "??" or code[0] == "A"):
            try:
                if os.path.getmtime(os.path.join(root, path)) >= since:
                    found.add(m2.group(1))
            except OSError:
                pass
    found.discard(reviewed)
    return sorted(found)

try:
    NEW_LIMIT = int(os.environ.get("TEAMWRIGHT_REVIEW_NEW_TASKS") or 2)
except ValueError:
    NEW_LIMIT = 2
if (who["session_id"] and re.match(r"^%s$" % task.TASK_ID, tid)
        and not os.path.exists(os.path.join(root, rel))):
    win = review_window(who["session_id"])
    if win and win[0] != tid:
        reviewed, wid = win[0], win[1]
        led = os.path.join(sessions_dir(), reviewed + ".new-tasks.jsonl")
        made = []
        for r in io.read_jsonl(led):
            if r.get("window") == wid and r.get("task") not in made:
                made.append(r.get("task"))
        if tid not in made:
            if len(made) >= NEW_LIMIT and io.gate_mode(data) != "warn":
                io.deny("REVIEW GATE: the review of %s already created %d new task record(s) in "
                        "this session (%s); %s would be one more. One reviewed task spawns at most "
                        "%d new tasks (lifecycle.md section 6): put the rest into one umbrella task "
                        "or the sprint's review follow-up task (edit an existing record), or ask "
                        "the owner. Limit: TEAMWRIGHT_REVIEW_NEW_TASKS." % (
                            reviewed, len(made), ", ".join(made), tid, NEW_LIMIT), "review-gate")
            io.state_dir(data, "sessions")
            io.append_jsonl(led, dict(who, ts=io.now(), window=wid, task=tid))
            if len(made) >= NEW_LIMIT:
                io.deny("REVIEW GATE: the review of %s creates its new task number %d (%s; before: "
                        "%s); one reviewed task spawns at most %d (lifecycle.md section 6) - one "
                        "umbrella task instead." % (reviewed, len(made) + 1, tid, ", ".join(made),
                                                    NEW_LIMIT), "review-gate")

before, after = io.before_after(data)
if after is None:
    sys.exit(0)
rb, ra = task.parse(before or ""), task.parse(after)
sb, sa = task.status(rb), task.status(ra)
if sa == sb:
    sys.exit(0)

# ---- 7 (cont.). the reviewed task's status move closes its review: count every new record --
# A record created through the shell, cp or a script never passes the Edit/Write check above;
# git sees it whatever wrote it.
if sb == "NEEDS_REVIEW" and who["session_id"]:
    win = review_window(who["session_id"])
    since = _epoch(win[2]) if win and win[0] == tid else None
    if since is not None:
        made = new_records_since(since - 1, tid)
        if len(made) > NEW_LIMIT:
            io.deny("REVIEW GATE: the review of %s created %d new task records (%s), however they were "
                    "written; one reviewed task spawns at most %d (lifecycle.md section 6) - fold the "
                    "rest into one umbrella task or the sprint's review follow-up task, or ask the owner. "
                    "Limit: TEAMWRIGHT_REVIEW_NEW_TASKS." % (tid, len(made), ", ".join(made), NEW_LIMIT),
                    "review-gate")
if sa == "NEEDS_REVIEW" or (sa == "IN_PROGRESS" and sb != "NEEDS_REVIEW"):
    # a send-back (NEEDS_REVIEW -> IN_PROGRESS) is not development
    record_dev(tid)
    sys.exit(0)
if sa != "CODE_COMPLETE":
    sys.exit(0)
if (ra.get("review") or "").replace(" ", "").startswith("skipped:trivial"):
    sys.exit(0)
def record_time(rel):
    """When a record appeared: the commit that added it, else (not committed yet) the file's
    mtime. Records are append-only, so this orders reviews against verify failures."""
    import subprocess
    try:
        p = subprocess.run(("git", "log", "--diff-filter=A", "--format=%ct", "-1", "--", rel),
                           cwd=root, capture_output=True, text=True, timeout=20)
        if p.returncode == 0 and p.stdout.strip():
            return float(p.stdout.split()[0])
    except Exception:
        pass
    try:
        return os.path.getmtime(os.path.join(root, rel))
    except OSError:
        return 0.0


def latest_failed_check(tid):
    """(time, record) of the newest FAIL among the task's verify and acceptance records, or None."""
    out = None
    for suffix, rounds, key in ((".verify", task.verify_rounds(root, tid), "result"),
                                (".validation", task.validation_rounds(root, tid), "verdict")):
        for n_, r in rounds:
            if task.first_word(r.get(key)) == "FAIL":
                rel = "docs/tasks/%s%s/%d.md" % (tid, suffix, n_)
                t = record_time(rel)
                if out is None or t > out[0]:
                    out = (t, rel)
    return out


rounds = task.review_rounds(root, tid)
if not rounds:
    io.deny("REVIEW GATE: %s cannot become CODE_COMPLETE - no review record in "
            "docs/tasks/%s.reviews/. Spawn a fresh %s subagent for %s. Exit for trivial "
            "changes: `review: skipped:trivial`." % (tid, tid, REVIEWER, tid), "review-gate")
n, rv = rounds[-1]
computed = len(rounds)
changes = changes_in_a_row([r for _, r in rounds])
verdict = task.first_word(rv.get("verdict"))
if verdict == "REJECT_RECURRENCE":
    extra = "" if rca_ok(rv.get("rca_task")) else " The review also lacks a linked rca task."
    io.deny("REVIEW GATE: latest review (round %d of %d) of %s is REJECT_RECURRENCE.%s The "
            "task goes to DEFERRED with `absorbed_by: <rca task>`. %s"
            % (n, computed, tid, extra, RCA_MSG), "review-gate")
if verdict != "APPROVE":
    if verdict == "ESCALATE":
        nxt = "the owner decides: move the task to BLOCKED with the question in blocked_on"
    elif changes >= ESCALATE_AFTER:
        nxt = "escalate to a human: the next review must be ESCALATE, not another send-back"
    else:
        nxt = "fix the findings (status back to IN_PROGRESS) and request round %d" % (computed + 1)
    io.deny("REVIEW GATE: latest review (round %d) of %s is %r, not APPROVE. Review rounds "
            "so far: %d, REQUEST_CHANGES in a row: %d (counted from docs/tasks/%s.reviews/, "
            "not from the task's review_rounds field); %s."
            % (n, tid, verdict, computed, changes, tid, nxt), "review-gate")
failed = latest_failed_check(tid)
if failed is not None and record_time("docs/tasks/%s.reviews/%d.md" % (tid, n)) < failed[0]:
    io.deny("REVIEW GATE: the latest review (round %d) of %s is older than its failed check %s - "
            "an approval from before a VERIFY_FAIL does not carry over, even when the code did "
            "not change. Spawn a fresh %s subagent for %s (round %d)."
            % (n, tid, failed[1], REVIEWER, tid, n + 1), "review-gate")
if (rv.get("task") or "").strip() != tid:
    io.deny("REVIEW GATE: review round %d in docs/tasks/%s.reviews/ names task %r, not %s - a "
            "review copied from another task does not count. Spawn a fresh %s subagent for %s."
            % (n, tid, (rv.get("task") or "").strip(), tid, REVIEWER, tid), "review-gate")
reviewer = (rv.get("reviewer") or "").strip()
impl = (ra.get("implemented_by") or "").strip()
if not reviewer:
    io.deny("REVIEW GATE: review round %d of %s has no `reviewer:`." % (n, tid), "review-gate")
if impl and reviewer == impl:
    io.deny("REVIEW GATE: reviewer %r is also implemented_by of %s - self-review never "
            "counts." % (reviewer, tid), "review-gate")
entry = ledger_entry(tid, n)
stamp = None
if not os.path.exists(review_ledger(tid)):
    # a fresh checkout (no ledger for this task at all): the committed stamp stands in. Where
    # the ledger exists it alone decides - a stamp is never a way around a local ledger.
    st = io.read_stamp(root, "docs/tasks/%s.reviews/%d.md" % (tid, n)) if hasattr(io, "read_stamp") else None
    if st is not None and st.get("gate") == "review" and st.get("round") == n:
        entry = stamp = st
    elif st is None and hasattr(io, "legacy_record") and \
            io.legacy_record(root, "docs/tasks/%s.reviews/%d.md" % (tid, n)):
        # written before the kit wrote stamps (pre-0.12), committed before the upgrade and
        # unchanged since: the 0.11 gate checked it in the session that wrote it
        sys.exit(0)
if is_human(reviewer):
    if entry is not None:
        io.deny("REVIEW GATE: round %d of %s says `reviewer: human` but was written by an "
                "agent in Claude Code." % (n, tid), "review-gate")
    sys.exit(0)  # a person's review, committed outside Claude Code
if entry is None:
    io.deny("REVIEW GATE: round %d of %s was not written by a %s agent in Claude Code "
            "(no entry in .teamwright/sessions/%s.reviews.jsonl). Spawn a fresh %s "
            "subagent for %s; a person reviewing outside Claude Code uses "
            "`reviewer: human:<name>`." % (n, tid, REVIEWER, tid, REVIEWER, tid), "review-gate")
if entry.get("agent_type") != REVIEWER or io.actor_key(entry) in dev_keys(tid):
    io.deny("REVIEW GATE: round %d of %s was written by an agent that implemented the "
            "task or is not a %s - self-review never counts." % (n, tid, REVIEWER),
            "review-gate")
if entry.get("verdict") != "APPROVE":
    io.deny("REVIEW GATE: round %d of %s was written with verdict %r and later changed "
            "to APPROVE outside the reviewer. Run a new review round."
            % (n, tid, entry.get("verdict")), "review-gate")
if entry.get("agent_id") and not spawned_reviewer(tid, entry.get("session_id")) and \
        not (stamp is not None and stamp.get("spawned") is True):
    io.deny("REVIEW GATE: no journaled spawn of a %s subagent for %s in session %s "
            "(.teamwright/logs/tools.jsonl). Spawn the reviewer with the task id in its "
            "prompt; the journal hook (journal.sh) must be enabled in .claude/settings.json."
            % (REVIEWER, tid, (entry.get("session_id") or "?")[:8]), "review-gate")
PY
python3 -c "$_tw_py"
exit 0
