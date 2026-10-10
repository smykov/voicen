#!/usr/bin/env bash
# journal.sh — Claude Code journal hook: facts about what agents did. Never decides.
#
# WHY. "Measure from git and logs, not from the agent's self-report" needs logs.
# Without them a skipped review stage, a tool quietly cut off by permissions, or a
# session that ended without an outcome are invisible: the only record is what the
# agent chose to say. The review gate also reads this journal: "the reviewer was
# spawned for this task" is a journaled tool call, not a sentence in a report.
#
# EVENTS -> FILES (under .teamwright/logs/, or $TEAMWRIGHT_LOG_DIR; gitignored)
#   PreToolUse / PostToolUse / PostToolUseFailure -> tools.jsonl
#       one line per call: ts, event (pre|post|failure), session_id, agent_id,
#       agent_type, tool, detail; Agent calls also get subagent_type and task_ids
#       (ids in the spawn description, else the first id in the prompt).
#   PermissionRequest / PermissionDenied          -> perms.jsonl
#       a permission decision was needed / a call was denied (PermissionDenied fires
#       for auto-mode denials; deny-rule refusals show up as a `pre` line with no
#       matching `post`).
#   PostToolUse of Edit/Write on docs/tasks/<ID>.verify/<n>.md -> verify.jsonl
#       a verify record landed: task, round, kind, tool, environment, result, commit
#       (front-matter enums and the sha only - never the command or the body).
#   Stop / SubagentStop                           -> outcomes.jsonl
#       session (or subagent) outcome: the `Outcome: barrier|idle|escalate|
#       interrupted` line of the final message, the active task and its status,
#       and the number of tool calls this agent made. The final message is taken
#       from `last_assistant_message`; when the runtime does not send it (or it
#       has no Outcome line), from the last assistant text in the transcript
#       (`transcript_path`, `agent_transcript_path` for a subagent).
#       `outcome_source` says which: message | transcript | none. A Stop with no
#       Outcome line while an agent this session launched has not finished yet is
#       `outcome: waiting`, `outcome_source: open-agent` (the runtime ran the role in
#       the background; the session resumes on its result); so is a Stop whose final
#       message ends on the flow's "waiting for <role> on <ID>" line
#       (`outcome_source: waiting-line`). A SubagentStop without
#       `agent_type` takes it from this agent's own tool-call lines.
#
# PRIVACY. `detail` is a short classification, never a replay of the call: the
# program and subcommand of a shell command (no arguments, no VAR=value prefixes),
# a repo-relative file path, an agent type, a skill name, a URL host. No file
# contents, prompts, command arguments, error texts or denial reasons are stored.
#
# CONTRACT. Never influences a decision: prints nothing, always exits 0.
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
ev = data.get("hook_event_name") or ""
root = io.project_root(data)
logdir = io.log_dir(data)
who = io.actor(data)
tool = data.get("tool_name") or ""
ti = data.get("tool_input") if isinstance(data.get("tool_input"), dict) else {}
ID_RE = re.compile(r"\b" + task.TASK_ID + r"\b")
SAFE_TOKEN = re.compile(r"^[\w./:@+-]{1,40}$")

def shell_detail(cmd):
    cmd = (cmd or "").replace("\r", ";").replace("\n", ";")
    while True:  # leading `cd <dir> &&|;` hides the real program
        m = re.match(r"^\s*cd\s+[^;&|]+\s*(;|&&)\s*", cmd)
        if not m:
            break
        cmd = cmd[m.end():]
    first = re.split(r"[;&|]", cmd, 1)[0].split()
    first = [t for t in first if "=" not in t][:2]  # drop VAR=value (may be a secret)
    return " ".join(t for t in first if SAFE_TOKEN.match(t))

def detail():
    if tool == "Bash":
        return shell_detail(ti.get("command"))
    if tool in ("Read", "Edit", "Write", "MultiEdit", "NotebookEdit"):
        return io.rel_path(data, ti.get("file_path") or ti.get("notebook_path") or "")
    if tool in ("Agent", "Task"):
        return (ti.get("subagent_type") or "general-purpose").strip()
    if tool in ("Skill", "SlashCommand"):
        words = (ti.get("skill") or ti.get("command") or "").split()
        return words[0] if words else ""
    if tool == "WebFetch":
        m = re.match(r"^[a-z]+://([^/:?#]+)", ti.get("url") or "")
        return m.group(1) if m else ""
    return ""

def clean(s, n=80):
    s = re.sub(r"[\x00-\x1f\"\\\\]", " ", s or "")
    return re.sub(r"\s+", " ", s).strip()[:n]

base = {"ts": io.now(), "session_id": who["session_id"], "agent_id": who["agent_id"],
        "agent_type": who["agent_type"]}

if ev in ("PreToolUse", "PostToolUse", "PostToolUseFailure"):
    rec = dict(base, event={"PreToolUse": "pre", "PostToolUse": "post"}.get(ev, "failure"),
               tool=tool, detail=clean(detail()))
    if tool in ("Agent", "Task"):
        # The task the agent is spawned FOR: the join key for the review gate. The flow
        # names it in the short description; a prompt also mentions other tasks (owners
        # of a red gate, dependencies), so prompt ids are used only when the description
        # names none (older orchestrators) - a wider match, never a missed spawn.
        ids = []
        found = ID_RE.findall(str(ti.get("description") or "")) or \
            ID_RE.findall(str(ti.get("prompt") or ""))
        for t in found:
            if t not in ids:
                ids.append(t)
        rec["subagent_type"] = rec["detail"]
        rec["task_ids"] = ids[:5]
    io.append_jsonl(os.path.join(logdir, "tools.jsonl"), rec)
    vpath = io.rel_path(data, ti.get("file_path") or "") if tool in ("Edit", "Write", "MultiEdit") else ""
    vm = io.VERIFY_FILE_RE.search(vpath) if vpath.startswith("docs/tasks/") else None
    if ev == "PostToolUse" and vm:
        try:
            vr = task.parse(open(os.path.join(root, vpath), encoding="utf-8",
                                 errors="replace").read())
        except OSError:
            vr = {}
        fw = lambda k: clean(task.first_word(vr.get(k) if isinstance(vr.get(k), str) else ""), 40)
        io.append_jsonl(os.path.join(logdir, "verify.jsonl"), dict(
            base, event="verify_record", task=vm.group(1), round=int(vm.group(2)),
            kind=fw("kind"), tool=fw("tool"), environment=fw("environment"),
            result=fw("result"), commit=fw("commit")))

elif ev in ("PermissionRequest", "PermissionDenied"):
    rec = dict(base, event=ev, tool=tool, detail=clean(detail()))
    if ev == "PermissionDenied" and "classifier_verdict" in data:
        rec["classifier_verdict"] = bool(data.get("classifier_verdict"))
    io.append_jsonl(os.path.join(logdir, "perms.jsonl"), rec)

elif ev in ("Stop", "SubagentStop"):
    OUT_RE = re.compile(r"(?im)^\W*outcome\W*[:=]\s*\W*(barrier|idle|escalate|interrupted)\b")
    # the flow's line for a turn that ends while a role runs in the background
    WAIT_RE = re.compile(r"(?im)^\W*waiting for [\w.-]+ on [A-Za-z][\w.]*-\d+\W*$")

    def last_text(path):
        """Last assistant text block of a transcript (JSONL), read from the tail."""
        try:
            with open(path, "rb") as fh:
                fh.seek(0, 2)
                size = fh.tell()
                fh.seek(max(0, size - 2 * 1024 * 1024))
                lines = fh.read().decode("utf-8", "replace").splitlines()
        except (OSError, TypeError, ValueError):
            return ""
        import json
        for line in reversed(lines):
            try:
                r = json.loads(line)
            except ValueError:
                continue
            msg = r.get("message") if isinstance(r, dict) else None
            if not isinstance(msg, dict) or (r.get("type") or msg.get("role")) != "assistant":
                continue
            c = msg.get("content")
            texts = [c] if isinstance(c, str) else [b.get("text") or "" for b in c or []
                                                     if isinstance(b, dict) and b.get("type") == "text"]
            text = "\n".join(t for t in texts if t)
            if text.strip():
                return text
        return ""

    src, text = "none", data.get("last_assistant_message") or ""
    m = OUT_RE.search(text)
    if m:
        src = "message"
    else:
        tp = data.get("agent_transcript_path") if ev == "SubagentStop" else None
        tp = tp or (data.get("transcript_path") if ev == "Stop" else None)
        if tp and not WAIT_RE.search(text):
            text = last_text(os.path.expanduser(str(tp))) or text
            m = OUT_RE.search(text)
            if m:
                src = "transcript"
    cur, st = "", ""
    try:
        cur = open(os.path.join(root, ".teamwright", "current-task")).read().split()[0]
        st = task.status(task.parse(open(os.path.join(root, "docs", "tasks", cur + ".md"),
                                         encoding="utf-8", errors="replace").read()))
    except (OSError, IndexError):
        pass
    key = io.actor_key(who)
    sid = who["session_id"]
    calls, seen_type = 0, ""
    launched = {}   # subagent_type -> spawns by this session's main agent that returned
    for r in io.read_jsonl(os.path.join(logdir, "tools.jsonl")):
        if r.get("event") == "pre" and io.actor_key(r) == key:
            calls += 1
            seen_type = r.get("agent_type") or seen_type
        elif (ev == "Stop" and r.get("event") == "post" and r.get("tool") in ("Agent", "Task")
              and r.get("session_id") == sid and not r.get("agent_id")):
            t = r.get("subagent_type") or r.get("detail") or ""
            launched[t] = launched.get(t, 0) + 1
    if ev == "SubagentStop" and not base["agent_type"] and who["agent_id"]:
        base["agent_type"] = seen_type
    outcome = m.group(1).lower() if m else "none"
    if ev == "Stop" and not m and WAIT_RE.search(text):
        # the turn ended on the flow's waiting line: a clean wait, whatever the hook can
        # or cannot see of the background agent (its spawn may not be journaled as returned)
        outcome, src = "waiting", "waiting-line"
    elif ev == "Stop" and not m and launched:
        # The turn ended while an agent it launched is still running (the runtime ran it
        # in the background): the session is waiting, not stopped without an outcome.
        # Matched per agent type, so a helper or a nested agent of another type finishing
        # does not end the wait for a role.
        finished = {}
        for r in io.read_jsonl(os.path.join(logdir, "outcomes.jsonl")):
            if r.get("event") == "SubagentStop" and r.get("session_id") == sid and r.get("agent_type"):
                finished[r["agent_type"]] = finished.get(r["agent_type"], 0) + 1
        if any(n > finished.get(t, 0) for t, n in launched.items()):
            outcome, src = "waiting", "open-agent"
    io.append_jsonl(os.path.join(logdir, "outcomes.jsonl"), dict(
        base, event=ev, outcome=outcome, outcome_source=src,
        current_task=cur, task_status=st, tool_calls=calls))
PY
python3 -c "$_tw_py" >/dev/null 2>&1
exit 0
