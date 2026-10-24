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
#       `outcome_source` says which: message | transcript | none. `stage` is the
#       stage the session ran, from its `Stage: <analyze|red-tests|implement|review|
#       verify|flow>` line - the last one in the message, the end block ('' when none;
#       SubagentStop lines leave it empty; a flow session runs several stages and records
#       `flow`: its tokens are the whole session's, per-stage numbers come from stage
#       sessions); `batch`
#       and `batch_reason` are the tasks the session chose from the router's candidates
#       and its one-line reason, from `Batch: T-1 T-2 - <reason>`. A Stop with no
#       Outcome line while an agent this session launched has not finished yet is
#       `tokens` is the model usage of this agent's transcript so far, per model:
#       {model: {input, output, cache_read, cache_write}} - cumulative, from
#       `transcript_path` (Stop: the session's own lines, sidechains left out) or
#       `agent_transcript_path` (SubagentStop); {} when the runtime sends no path.
#       Read incrementally: the offset per transcript is kept in
#       .teamwright/logs/.tokens/ (a dot directory: tool state, not a journal).
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
    # the stage the session ran (`Stage: implement` in the end block of a stage command,
    # `Stage: flow` for a flow run of several stages)
    STAGE_RE = re.compile(r"(?im)^\W*stage\W*[:=]\s*\W*(analyze|red-tests|implement|review|verify|flow)\b")
    # the batch the session chose from the router's candidates, and why:
    # `Batch: T-1 T-2 - disjoint seams, same area`
    BATCH_RE = re.compile(r"(?im)^\W*batch\W*[:=]\s*(.+)$")
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
    # the LAST match: a message quotes an earlier `Stage:` / `Batch:` (the router's answer, a
    # previous step) above its own end block, which is always the bottom of the message
    sm = (list(STAGE_RE.finditer(text)) or [None])[-1] if ev == "Stop" else None
    bm = (list(BATCH_RE.finditer(text)) or [None])[-1] if ev == "Stop" else None
    batch = ID_RE.findall(bm.group(1).split(" - ")[0].split("\u2014")[0]) if bm else []
    reason = ""
    if bm:
        rest = re.split(r"\s(?:-|\u2014|:)\s", bm.group(1), 1)
        reason = clean(rest[1], 160) if len(rest) > 1 else ""
    def usage(path, sidechains):
        """Cumulative token usage of a transcript, per model. Reads only what was appended
        since the last call (offset kept per transcript); a message written as several
        lines (one per content block) is counted once."""
        import hashlib, json
        if not path:
            return {}
        path = os.path.expanduser(str(path))
        sdir = os.path.join(logdir, ".tokens")
        sfile = os.path.join(sdir, hashlib.sha1(path.encode("utf-8", "replace")).hexdigest()[:16] + ".json")
        try:
            stt = json.load(open(sfile))
        except (OSError, ValueError):
            stt = {}
        off, last, sums = int(stt.get("offset") or 0), stt.get("last") or "", stt.get("sums") or {}
        try:
            with open(path, "rb") as fh:
                fh.seek(0, 2)
                if fh.tell() < off:          # rewritten transcript: start over
                    off, last, sums = 0, "", {}
                fh.seek(off)
                chunk = fh.read()
        except (OSError, TypeError, ValueError):
            return sums
        end = chunk.rfind(b"\n") + 1        # a partly written last line waits for the next call
        for line in chunk[:end].decode("utf-8", "replace").splitlines():
            try:
                r = json.loads(line)
            except ValueError:
                continue
            msg = r.get("message") if isinstance(r, dict) else None
            if not isinstance(msg, dict) or not isinstance(msg.get("usage"), dict):
                continue
            if r.get("isSidechain") and not sidechains:
                continue
            mid = str(msg.get("id") or r.get("requestId") or r.get("uuid") or "")
            if mid and mid == last:
                continue
            last = mid
            u = msg["usage"]
            m = sums.setdefault(clean(str(msg.get("model") or "unknown"), 60),
                                {"input": 0, "output": 0, "cache_read": 0, "cache_write": 0})
            for k, f in (("input", "input_tokens"), ("output", "output_tokens"),
                         ("cache_read", "cache_read_input_tokens"),
                         ("cache_write", "cache_creation_input_tokens")):
                try:
                    m[k] += int(u.get(f) or 0)
                except (TypeError, ValueError):
                    pass
        try:
            os.makedirs(sdir, exist_ok=True)
            with open(sfile, "w") as fh:
                json.dump({"offset": off + end, "last": last, "sums": sums}, fh)
        except OSError:
            pass
        return sums

    try:
        tokens = usage(data.get("agent_transcript_path") if ev == "SubagentStop"
                       else data.get("transcript_path"), ev == "SubagentStop")
    except Exception:
        tokens = {}
    io.append_jsonl(os.path.join(logdir, "outcomes.jsonl"), dict(
        base, event=ev, outcome=outcome, outcome_source=src, stage=sm.group(1).lower() if sm else "",
        batch=batch[:10], batch_reason=reason,
        current_task=cur, task_status=st, tool_calls=calls, tokens=tokens))
PY
python3 -c "$_tw_py" >/dev/null 2>&1
exit 0
