#!/usr/bin/env python3
"""Shared helpers for Claude Code PreToolUse gates (stdlib only).

Contract for every gate that imports this module:
  * fail-open: any internal error -> allow (exit 0, no output). A broken gate
    must never stall the team; a skipped check is cheaper than a dead session.
  * silence on allow: stdout is interpreted as the decision.
  * deny -> print one JSON object with permissionDecision=deny, exit 0.
"""
import json
import os
import re
import sys

# Only docs/tasks/<ID>.md is a task record (<ID> = $TEAMWRIGHT_TASK_ID_RE, as in _task.py):
# a template (_task-template.md) or a side file (T-089.history.md) is not.
TASK_ID = os.environ.get("TEAMWRIGHT_TASK_ID_RE", r"[A-Z][A-Z0-9]*-\d+")
TASK_FILE_RE = re.compile(r"(?:^|/)docs/tasks/(%s)\.md$" % TASK_ID)
REVIEW_FILE_RE = re.compile(r"(?:^|/)docs/tasks/(%s)\.reviews/(\d+)\.md$" % TASK_ID)
VERIFY_FILE_RE = re.compile(r"(?:^|/)docs/tasks/(%s)\.verify/(\d+)\.md$" % TASK_ID)
VALIDATION_FILE_RE = re.compile(r"(?:^|/)docs/tasks/(%s)\.validation/(\d+)\.md$" % TASK_ID)


REVIEWER_AGENT = os.environ.get("TEAMWRIGHT_REVIEWER_AGENT", "code-reviewer")

# ---- gate stamps: the ledger line that is committed with the record ----------------
# The local ledgers (.teamwright/sessions/, gitignored) do not survive a fresh checkout:
# in an autonomous run each job starts from a clean clone, so a job cut after the
# reviewer's APPROVE (or the validator's PASS) was committed but before the status move
# would loop forever - the next job's gate finds no ledger entry. When a gate lets a
# review, verify or validation record through it also writes <n>.gate.json next to it:
# who wrote it (session, agent, agent type), the verdict, whether the reviewer's spawn was
# journaled, and the sha256 of the record as written. The orchestrator commits it with the
# record; a later status move on a checkout without the task's ledger (a fresh clone)
# verifies against it - where the ledger exists it alone decides. Only the gates write
# stamps (the state guard refuses Edit/Write and running a hook by hand, the review gate's
# shell check refuses writes into the record directories, pre-commit refuses a stamp the
# local ledger does not have); a record changed after its stamp no longer matches the hash.
STAMP_RE = re.compile(r"(?:^|/)docs/tasks/[^/]+\.(reviews|verify|validation)/\d+\.gate\.json$")


def _digest(text):
    import hashlib
    norm = (text or "").replace("\r\n", "\n").rstrip("\n") + "\n"
    return hashlib.sha256(norm.encode("utf-8", "replace")).hexdigest()


def stamp_path(root, rel):
    """docs/tasks/<ID>.<kind>/<n>.md -> its stamp, docs/tasks/<ID>.<kind>/<n>.gate.json."""
    return os.path.join(root, re.sub(r"\.md$", ".gate.json", rel))


_RECORD_RE = re.compile(r"(?:^|/)(docs/tasks/(%s)\.(?:reviews|verify|validation)/(\d+)\.md)$" % TASK_ID)


def _record_id(rel):
    """docs/tasks/<ID>.<kind>/<n>.md -> (normalised path, ID, n), else (None, None, None)."""
    m = _RECORD_RE.search((rel or "").replace(os.sep, "/"))
    return (m.group(1), m.group(2), int(m.group(3))) if m else (None, None, None)


def write_stamp(root, rel, entry, text):
    """Write the committed stamp of a record the gate let through (best effort). The stamp
    names the record it belongs to - task, path, round - so a copy into another task's
    directory (or another round) does not count there."""
    path = stamp_path(root, rel)
    norm, tid, n = _record_id(rel)
    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as fh:
            json.dump(dict(entry, task=tid, path=norm, round=n, sha256=_digest(text)), fh,
                      ensure_ascii=False, sort_keys=True, indent=1)
            fh.write("\n")
    except OSError:
        pass


def _git_out(root, *args):
    import subprocess
    try:
        p = subprocess.run(("git",) + args, cwd=root, capture_output=True, text=True, timeout=20)
    except Exception:
        return None
    return p.stdout if p.returncode == 0 else None


def _stamped_kit(version):
    """Does a kit version write stamps (0.12.0 and later, or a channel build - a commit)?"""
    m = re.match(r"^(\d+)\.(\d+)\.(\d+)", str(version or "").strip())
    if not m:
        return bool(str(version or "").strip())
    return tuple(int(x) for x in m.groups()) >= (0, 12, 0)


def legacy_record(root, rel):
    """True for a review / verify / validation record written before the project's kit wrote
    stamps (before 0.12): the record was last changed in a commit that precedes the commit
    which moved .teamwright/installed.json to a stamping kit, and the working copy is that
    committed version. On a fresh checkout such a record has no stamp and no ledger; the
    0.11 gates checked it in the session that wrote it."""
    if not _RECORD_RE.search((rel or "").replace(os.sep, "/")):
        return False
    hist = _git_out(root, "log", "--format=%H", "--", ".teamwright/installed.json")
    if not hist:
        return False
    upgrade, older = None, False
    for c in hist.split():      # newest first: the oldest stamping commit after a pre-0.12 one
        try:
            man = json.loads(_git_out(root, "show", "%s:.teamwright/installed.json" % c) or "{}")
        except ValueError:
            man = {}
        kit = man.get("@kit") if isinstance(man, dict) else None
        ver = kit.get("version") if isinstance(kit, dict) else ""
        if _stamped_kit(ver):
            upgrade = c
        else:
            older = True        # an install before stamps (a 0.11 version, or no @kit record)
            break
    if upgrade is None or not older:
        return False            # never upgraded from a kit without stamps: nothing is legacy
    parent = (_git_out(root, "rev-parse", "-q", "--verify", upgrade + "~1") or "").strip()
    last = (_git_out(root, "log", "-1", "--format=%H", "--", rel) or "").strip()
    if not parent or not last or _git_out(root, "merge-base", "--is-ancestor", last, parent) is None:
        return False
    # the working copy is the committed version (not edited since)
    return _git_out(root, "diff", "--quiet", "HEAD", "--", rel) is not None


def _pending_path(root, rel):
    norm, tid, n = _record_id(rel)
    name = re.sub(r"[^A-Za-z0-9_.-]", "_", norm or rel)
    return os.path.join(root, ".teamwright", "sessions", ".pending", name + ".json")


def stage_record(root, rel, ledger, line, stamp, text):
    """PreToolUse: the gate let a record write through. Its ledger line and stamp are kept
    pending until the write lands (commit_record, PostToolUse): a write that is then refused
    by another hook, cancelled by the person or fails leaves neither a ledger line nor an
    orphan stamp that would block the task."""
    path = _pending_path(root, rel)
    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as fh:
            json.dump({"ledger": ledger, "line": line, "stamp": stamp, "sha256": _digest(text)}, fh)
    except OSError:
        pass


def commit_record(root, rel):
    """PostToolUse: the record write landed - append the pending ledger line and write the
    stamp, when the record on disk is what the gate checked; drop the pending entry either
    way. False when nothing was pending."""
    path = _pending_path(root, rel)
    try:
        with open(path, encoding="utf-8") as fh:
            p = json.load(fh)
        os.remove(path)
        with open(os.path.join(root, rel), encoding="utf-8", errors="replace") as fh:
            text = fh.read()
    except (OSError, ValueError):
        return False
    if not isinstance(p, dict) or p.get("sha256") != _digest(text):
        return True            # the file is not what the gate let through: no evidence
    if p.get("ledger") and isinstance(p.get("line"), dict):
        led = os.path.join(root, p["ledger"])
        try:
            os.makedirs(os.path.dirname(led), exist_ok=True)
        except OSError:
            pass
        append_jsonl(led, p["line"])
    if isinstance(p.get("stamp"), dict):
        write_stamp(root, rel, p["stamp"], text)
    return True


def remove_orphan_stamps(root, tid, kind):
    """Stamps whose record is missing (written by a 0.12 pre-release gate before the write
    landed): ignored and removed."""
    d = os.path.join(root, "docs", "tasks", "%s.%s" % (tid, kind))
    try:
        names = os.listdir(d)
    except OSError:
        return
    for f in names:
        m = re.match(r"^(\d+)\.gate\.json$", f)
        if m and not os.path.exists(os.path.join(d, m.group(1) + ".md")):
            try:
                os.remove(os.path.join(d, f))
            except OSError:
                pass


def read_stamp(root, rel):
    """The stamp of a record when it matches the record on disk, else None."""
    try:
        with open(stamp_path(root, rel), encoding="utf-8") as fh:
            st = json.load(fh)
        with open(os.path.join(root, rel), encoding="utf-8", errors="replace") as fh:
            text = fh.read()
    except (OSError, ValueError):
        return None
    if not isinstance(st, dict) or st.get("sha256") != _digest(text):
        return None
    norm, tid, n = _record_id(rel)
    if norm is None or (st.get("task"), st.get("path"), st.get("round")) != (tid, norm, n):
        return None          # a stamp copied from another task or round, or one without its name
    return st


def event(data):
    """Hook event name; tests and old runtimes may omit it (treated as PreToolUse)."""
    return (data or {}).get("hook_event_name") or "PreToolUse"


def actor(data):
    """Who is calling, as reported by Claude Code (a fact, not a claim).

    Subagents share the parent's session_id; only agent_id / agent_type tell them
    apart. agent_type alone (no agent_id) means a whole session started with
    `claude --agent <name>`.
    """
    d = data or {}
    return {
        "session_id": (d.get("session_id") or "").strip(),
        "agent_id": (d.get("agent_id") or "").strip(),
        "agent_type": (d.get("agent_type") or "").strip(),
    }


def actor_key(a):
    """Stable identity of one agent: session_id + agent_id ('main' = top level)."""
    if not a.get("session_id"):
        return ""
    return "%s:%s" % (a["session_id"], a.get("agent_id") or "main")


def state_dir(data, *parts):
    """Local, gitignored state under .teamwright/ (created on demand)."""
    d = os.path.join(project_root(data), ".teamwright", *parts)
    try:
        os.makedirs(d, exist_ok=True)
    except OSError:
        pass
    return d


def log_dir(data):
    """Journal directory shared by journal.sh and the gates that read it."""
    d = os.environ.get("TEAMWRIGHT_LOG_DIR") or os.path.join(
        project_root(data), ".teamwright", "logs")
    try:
        os.makedirs(d, exist_ok=True)
    except OSError:
        pass
    return d


def append_jsonl(path, obj):
    try:
        with open(path, "a", encoding="utf-8") as fh:
            fh.write(json.dumps(obj, ensure_ascii=False, sort_keys=True) + "\n")
    except OSError:
        pass


def read_jsonl(path):
    out = []
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                try:
                    out.append(json.loads(line))
                except ValueError:
                    continue
    except OSError:
        pass
    return out


def now():
    import datetime
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


_INPUT = {}


def load_input():
    try:
        raw = sys.stdin.read()
        data = json.loads(raw) if raw.strip() else None
    except Exception:
        return None
    if isinstance(data, dict):
        _INPUT.clear()
        _INPUT.update(data)
    return data


def project_root(data):
    root = os.environ.get("CLAUDE_PROJECT_DIR") or (data or {}).get("cwd") or os.getcwd()
    return os.path.abspath(root)


def rel_path(data, path):
    root = project_root(data)
    ap = os.path.abspath(os.path.join(root, path))
    try:
        rel = os.path.relpath(ap, root)
    except ValueError:
        return path
    return path if rel.startswith("..") else rel


def before_after(data):
    """(before, after) text of the target file for Edit/Write (and legacy MultiEdit).
    Returns (None, None) when the after-state cannot be modelled."""
    ti = data.get("tool_input") or {}
    fp = ti.get("file_path") or ""
    if not fp:
        return None, None
    path = fp if os.path.isabs(fp) else os.path.join(project_root(data), fp)
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            before = fh.read()
    except OSError:
        before = ""
    if "content" in ti and ti.get("content") is not None:
        return before, ti.get("content") or ""
    edits = ti.get("edits")
    if not isinstance(edits, list):
        edits = [ti]
    after = before
    for e in edits:
        if not isinstance(e, dict):
            continue
        old, new = e.get("old_string") or "", e.get("new_string") or ""
        if old and old in after:
            after = after.replace(old, new, -1 if e.get("replace_all") else 1)
        elif not old and not after:
            after = new
        else:
            return None, None  # the tool itself will fail; do not guess
    return before, after


def gate_mode(data=None):
    """'enforce' (deny) or 'warn' (gate-first adoption: report, do not block).

    $TEAMWRIGHT_GATE_MODE (set by the person who starts Claude Code) wins; otherwise
    the mode in .teamwright/config.yml, read with the installer's own parser
    (_config.py: YAML subset or JSON). Only an explicit `mode: gate-first` warns;
    a missing file, key or unknown value means enforce - the same rule tw-install.py
    reports.
    """
    env = (os.environ.get("TEAMWRIGHT_GATE_MODE") or "").strip().lower()
    if env in ("warn", "enforce"):
        return env
    try:
        sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
        import _config
        mode = _config.read_mode(os.path.join(project_root(data), _config.CONFIG))
    except Exception:
        return "enforce"
    return "warn" if mode == "gate-first" else "enforce"


# ---- teamwright's own state: agents never write it -------------------------------
# The mode switch and the install manifest decide what the gates do; the ledgers and
# the journal are the facts the gates rely on. An agent that could edit them could
# turn the gates off or forge a review. Config changes go through tw-install.py, run
# by the main session with the owner (it refuses enforce -> gate-first from inside
# Claude Code); a person may always edit the files by hand.
SWITCH_FILES = (".teamwright/config.yml", ".teamwright/installed.json")
SWITCH_NAMES = ("config.yml", "installed.json")
LEDGER_RE = re.compile(r"^\.teamwright/(sessions/|logs/|current-task$)")
_OWNER_CLAIM = re.compile(r"\bTEAMWRIGHT_OWNER\b|\bCLAUDECODE\b")
_INSTALLER_WORD = re.compile(r"(^|/)tw-install\.py$")
_SWITCH_TEXT = re.compile(r"(config\.yml|installed\.json)")
# commands whose every file argument may be written / removed
_WRITES_ARGS = {"tee", "rm", "unlink", "shred", "truncate", "touch", "chmod", "chown", "ln",
                "vi", "vim", "nvim", "nano", "emacs", "ed", "ex", "code"}
# commands whose last argument is the destination (mv also removes its sources)
_WRITES_DEST = {"cp", "install", "rsync", "scp"}
_INTERPRETERS = re.compile(r"^(python[0-9.]*|node|nodejs|deno|bun|ruby|perl|php|awk|gawk|mawk|"
                           r"lua|Rscript|pwsh|osascript)$")
_SHELLS = {"sh", "bash", "zsh", "dash", "ksh", "eval"}
_PREFIX = {"sudo", "command", "builtin", "exec", "nohup", "time", "nice", "xargs", "doas",
           "stdbuf", "timeout"}
# writes inside interpreter code (python, node, ruby, perl, awk ...)
_CODE_WRITE = re.compile(r"write\w*|writeFile|appendFile|open\s*\([^)]*['\"][wax+]|rename|replace|"
                         r"unlink|remove|rmtree|copy\w*|move|truncate|dump|symlink|chmod|"
                         r"-i\b|>\s*['\"]?[^\s'\"]*(config\.yml|installed\.json)")
_HEREDOC = re.compile(r"<<-?\s*(['\"]?)([A-Za-z_][A-Za-z0-9_]*)\1")


def _split_heredocs(cmd):
    """(command text without heredoc bodies, [(line_index, body)])."""
    lines, out, bodies, i = cmd.split("\n"), [], [], 0
    while i < len(lines):
        line = lines[i]
        out.append(line)
        i += 1
        for m in _HEREDOC.finditer(line):
            body = []
            while i < len(lines) and lines[i].strip() != m.group(2):
                body.append(lines[i])
                i += 1
            i += 1  # terminator
            bodies.append((len(out) - 1, "\n".join(body)))
    return "\n".join(out), bodies


def _segments(text):
    """Shell words split into simple commands; redirect targets marked as (op, target),
    op being the redirect token ('>', '>>', '&>', '>|' ...). Raises ValueError on
    unbalanced quotes."""
    import shlex
    lex = shlex.shlex(text, posix=True, punctuation_chars=";&|<>()")
    lex.whitespace_split = True
    lex.commenters = ""
    segs, cur, redirect = [], [], ""
    for tok in lex:
        if tok in (";", "&", "&&", "|", "||", "|&", "(", ")", ";;") or tok == "\n":
            if cur:
                segs.append(cur)
            cur, redirect = [], ""
            continue
        if set(tok) <= set("<>&|") and ">" in tok:
            redirect = tok
            continue
        if set(tok) <= set("<"):
            continue                     # input redirect / heredoc marker: reading
        if redirect:
            cur.append((redirect, tok))
            redirect = ""
            continue
        if re.fullmatch(r"\d?>&?\d*", tok):
            continue
        cur.append(tok)
    if cur:
        segs.append(cur)
    return segs


def _is_switch(tok, in_tw):
    """Does a shell word name config.yml / installed.json (globs included)?"""
    import fnmatch
    t = tok.replace("\\", "/").rstrip("/")
    i = t.rfind(".teamwright/")
    if i >= 0 and (i == 0 or t[i - 1] == "/"):
        tail = t[i:]
        if any(fnmatch.fnmatchcase(name, tail) for name in SWITCH_FILES):
            return True
    return in_tw and "/" not in t and any(fnmatch.fnmatchcase(n, t) for n in SWITCH_NAMES)


def _is_tw_dir(tok):
    t = tok.rstrip("/")
    return t == ".teamwright" or t.endswith("/.teamwright")


def shell_writes_switch(cmd, _depth=0):
    """True when a shell command writes, moves or removes config.yml / installed.json.
    Reading, grepping, `git add` / `git commit` / `git diff` of them is not a write."""
    if not _SWITCH_TEXT.search(cmd) and ".teamwright" not in cmd:
        return False
    text, bodies = _split_heredocs(cmd)
    try:
        segs = _segments(text)
    except ValueError:
        # unparseable (unbalanced quotes): fall back to "mentions + write-ish word"
        return bool(re.search(r">|\b(tee|cp|mv|rm|sed|perl|python3?|node|ruby|awk|truncate|"
                              r"install|rsync|dd)\b", cmd))
    in_tw = False
    body_text = "\n".join(b for _, b in bodies)
    for seg in segs:
        words = [w for w in seg if isinstance(w, str)]
        targets = [w[1] for w in seg if isinstance(w, tuple)]
        if any(_is_switch(t, in_tw) for t in targets):
            return True
        while words and (re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*=.*", words[0]) or words[0] in _PREFIX
                         or words[0] == "env" or (words[0].startswith("-") and len(words) > 1
                                                  and words[0] != "-")):
            words = words[1:]
        if not words:
            continue
        prog, args = os.path.basename(words[0]), words[1:]
        if prog == "cd":
            in_tw = bool(args) and _is_tw_dir(args[0])
            continue
        hit = [a for a in args if _is_switch(a, in_tw)]
        if prog in _SHELLS:
            code = " ".join(args[args.index("-c") + 1:args.index("-c") + 2]) if "-c" in args else \
                (" ".join(args) if prog == "eval" else "")
            if code and _depth < 3 and shell_writes_switch(code, _depth + 1):
                return True
            if not code and (hit or _SWITCH_TEXT.search(body_text)) and _depth < 3 and \
                    shell_writes_switch(body_text, _depth + 1):
                return True
            continue
        if _INSTALLER_WORD.search(prog) or (args and _INSTALLER_WORD.search(args[0])
                                            and _INTERPRETERS.match(prog)):
            continue                   # tw-install.py: allowed for the main session
        if prog in _WRITES_ARGS and hit:
            return True
        if prog == "mv" and hit:
            return True
        if prog in _WRITES_DEST:
            files = [a for a in args if not a.startswith("-")]
            if files and (_is_switch(files[-1], in_tw) or
                          (_is_tw_dir(files[-1]) and any(os.path.basename(f) in SWITCH_NAMES
                                                         for f in files[:-1]))):
                return True
            continue
        if prog == "dd" and any(a.startswith("of=") and _is_switch(a[3:], in_tw) for a in args):
            return True
        if prog in ("sed", "gsed") and hit and any(re.match(r"^-[a-zA-Z]*i|^--in-place", a) for a in args):
            return True
        if prog in ("yq", "jq") and hit and any(re.match(r"^-[a-zA-Z]*i|^--in-place", a) for a in args):
            return True
        if prog == "git" and args:
            sub = args[0]
            if hit and (sub in ("checkout", "restore", "mv", "rm", "stash", "switch")
                        or (sub == "reset" and "--hard" in args)):
                return True
            if sub == "apply":
                return True            # a patch may touch them; the command named them
            continue
        if _INTERPRETERS.match(prog):
            code = " ".join(args) + "\n" + body_text
            if _SWITCH_TEXT.search(code) and _CODE_WRITE.search(code):
                return True
    return False


_INPLACE = re.compile(r"^(-[a-zA-Z]*i|--in-place)")
# What makes a shell rewrite of a task record a state change: the keys, or any of the values
# they take - `perl -pi -e 's/red-tests/implement/'` names neither key.
_STATE_WORDS = re.compile(r"status|stage|red-tests|implement|\b(TODO|ANALYSIS|IN_PROGRESS|NEEDS_REVIEW|"
                          r"CODE_COMPLETE|DEPLOYED|VERIFIED|VERIFY_FAIL|DONE|BLOCKED|DEFERRED)\b")


def _is_task_record(tok, in_tasks):
    t = tok.replace("\\", "/")
    if TASK_FILE_RE.search(t):
        return True
    return in_tasks and "/" not in t and bool(re.fullmatch(r"(?:%s)\.md" % TASK_ID, t))


def shell_rewrites_task_record(cmd, root=None):
    """See _shell_rewrites_task_record; plus dd, git checkout/restore/stash/apply and
    interpreter one-liners that write a record."""
    if _shell_rewrites_task_record(cmd):
        return True
    try:
        return "docs/tasks" in cmd and _task_record_extra(cmd, root)
    except Exception:
        return False


def _shell_rewrites_task_record(cmd):
    """True when a shell command creates or replaces a task record (docs/tasks/<ID>.md) - a
    `>` redirect, a `tee` without -a, or cp / mv / install / rsync onto it: the whole front
    matter is written, with or without a status, out of the gates' sight - or edits one in
    place (sed -i, perl -i) and mentions `status` or `stage` or one of their values
    (IN_PROGRESS, red-tests, implement ...). Appends (>>, tee -a), reads, and writes to any
    other file are not - even when the command text names a task record."""
    mentions = bool(_STATE_WORDS.search(cmd))
    text, _ = _split_heredocs(cmd)
    if "docs/tasks" not in text:
        return False
    try:
        segs = _segments(text)
    except ValueError:
        return bool(re.search(r"((?<!>)>(?!>)\s*\S*docs/tasks/|\btee\b(?!\s+(-a|--append)\b))", text)) or \
            (mentions and bool(re.search(r"(sed\s+(-[a-zA-Z]*i|--in-place)|perl\s+-[a-zA-Z]*i)", text)))
    in_tasks = False
    for seg in segs:
        for op, target in (w for w in seg if isinstance(w, tuple)):
            if ">>" not in op and _is_task_record(target, in_tasks):
                return True
        words = [w for w in seg if isinstance(w, str)]
        while words and (re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*=.*", words[0]) or words[0] in _PREFIX
                         or words[0] == "env"):
            words = words[1:]
        if not words:
            continue
        prog, args = os.path.basename(words[0]), words[1:]
        if prog in ("cd", "pushd"):
            in_tasks = bool(args) and args[-1].rstrip("/").endswith("docs/tasks")
            continue
        files = [a for a in args if not a.startswith("-")]
        records = [a for a in files if _is_task_record(a, in_tasks)]
        if not records:
            continue
        if prog == "tee" and not any(a in ("-a", "--append") or re.fullmatch(r"-[a-zA-Z]*a[a-zA-Z]*", a)
                                     for a in args):
            return True
        if prog in ("cp", "mv", "install", "rsync", "ln") and files and _is_task_record(files[-1], in_tasks):
            return True
        if mentions and prog in ("sed", "gsed", "perl") and any(_INPLACE.match(a) for a in args):
            return True
        if prog == "git" and args and args[0] in ("checkout", "restore") and \
                any(_is_task_record(a, in_tasks) for a in files[1:] + (files[:1] if args[0] == "restore" else [])):
            return True          # an old version of the record: its status, out of the gates' sight
    return False


def _task_record_extra(cmd, root=None):
    """Writes of a task record the segment parser above does not model: dd of=, interpreter
    one-liners that name a record and write, git stash pop|apply / git apply|am touching
    one (best effort)."""
    text, bodies = _split_heredocs(cmd)
    body_text = "\n".join(b for _, b in bodies)
    try:
        segs = _segments(text)
    except ValueError:
        return False
    in_tasks = False
    for seg in segs:
        words = [w for w in seg if isinstance(w, str)]
        while words and (re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*=.*", words[0]) or words[0] in _PREFIX
                         or words[0] == "env"):
            words = words[1:]
        if not words:
            continue
        prog, args = os.path.basename(words[0]), words[1:]
        if prog in ("cd", "pushd"):
            in_tasks = bool(args) and args[-1].rstrip("/").endswith("docs/tasks")
            continue
        if prog == "dd" and any(a.startswith("of=") and _is_task_record(a[3:], in_tasks) for a in args):
            return True
        if prog == "git" and args and ((args[0] == "stash" and len(args) > 1 and args[1] in ("pop", "apply"))
                                       or args[0] in ("apply", "am")):
            if any(TASK_FILE_RE.search(p) for p in _git_paths(root, args)):
                return True
        if _INTERPRETERS.match(prog):
            code = " ".join(args) + "\n" + body_text
            if (_CODE_WRITE.search(code) or re.search(r"open\s*\(?[^)]*['\"]\s*\+?>", code)) and \
                    (re.search(r"docs/tasks/%s\.md" % TASK_ID, code) or
                     (in_tasks and re.search(r"\b%s\.md" % TASK_ID, code))):
                return True
    return False


# ---- gate records and stamps: written by the gates' Edit/Write path only ------------
# Review, verify and validation records (docs/tasks/<ID>.<kind>/<n>.md) and their stamps
# (<n>.gate.json) are what the status gates count. A shell command that writes one - into
# the directory, a copy from another task, a checkout of an old version, a one-liner that
# opens it - is refused. Best effort: the parser follows `cd`/`pushd`, redirects, the file
# commands, git checkout/restore/stash/apply, rsync, dd, tar/unzip -C, find -delete/-exec and
# interpreter one-liners that name such a path and write; a path built at run time or a
# script file that writes it is not seen (gates.md, Known limitations). Git hooks and review
# are the backstop.
_REC_PATH = re.compile(r"(?:^|/)docs/tasks/[^/]+\.(?:reviews|verify|validation)(?:/|$)|\.gate\.json$")
_REC_TEXT = re.compile(r"\.(?:reviews|verify|validation)\b|gate\.json")
_HOOK_PATH = re.compile(r"(?:^|/)scripts/hooks(?:/|$)")


def _cd(cwd, args):
    import posixpath
    dest = [a for a in args if not a.startswith("-")]
    if not dest or dest[-1] in ("~", "-"):
        return ""
    t = dest[-1].replace("\\", "/")
    return t if t.startswith("/") else posixpath.normpath(posixpath.join(cwd or ".", t))


def _at(cwd, tok):
    """A shell word as a path relative to where the command runs (after `cd`)."""
    import posixpath
    t = tok.replace("\\", "/")
    if t.startswith("/") or not cwd:
        return t
    return posixpath.normpath(posixpath.join(cwd, t)) + ("/" if t.endswith("/") else "")


def _git_paths(root, args):
    """Files a `git stash pop|apply` or `git apply|am <patch>` would write (best effort)."""
    import subprocess
    sub = args[0]
    try:
        if sub == "stash":
            ref = [a for a in args[2:] if not a.startswith("-")]
            p = subprocess.run(["git", "stash", "show", "--include-untracked", "--name-only"] + ref[:1],
                               cwd=root or None, capture_output=True, text=True, timeout=10)
            return p.stdout.splitlines()
        out = []
        for a in args[1:]:
            if a.startswith("-"):
                continue
            with open(os.path.join(root or "", a), encoding="utf-8", errors="replace") as fh:
                out += [ln[6:].strip() for ln in fh if ln.startswith(("+++ b/", "--- a/"))]
        return out
    except Exception:
        return []


def shell_writes_records(cmd, root=None, _depth=0, _cwd=""):
    """True when a shell command writes, moves, removes or restores a review / verify /
    validation record or a gate stamp (see the comment above)."""
    text, bodies = _split_heredocs(cmd)
    body_text = "\n".join(b for _, b in bodies)
    if not (_REC_TEXT.search(cmd) or "docs" in cmd or "tasks" in cmd or "git" in cmd):
        return False
    try:
        segs = _segments(text)
    except ValueError:
        return bool(_REC_TEXT.search(cmd) and re.search(
            r">|\b(tee|cp|mv|rm|sed|perl|python3?|node|ruby|truncate|install|rsync|dd|git|tar|unzip)\b", cmd))
    cwd = _cwd

    def hit(tok):
        return bool(_REC_PATH.search(_at(cwd, tok)))
    for seg in segs:
        words = [w for w in seg if isinstance(w, str)]
        targets = [w[1] for w in seg if isinstance(w, tuple)]
        if any(hit(t) for t in targets):
            return True
        while words and (re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*=.*", words[0]) or words[0] in _PREFIX
                         or words[0] == "env" or (words[0].startswith("-") and len(words) > 1
                                                  and words[0] != "-")):
            words = words[1:]
        if not words:
            continue
        prog, args = os.path.basename(words[0]), words[1:]
        if prog in ("cd", "pushd"):
            cwd = _cd(cwd, args)
            continue
        if prog == "popd":
            cwd = ""
            continue
        files = [a for a in args if not a.startswith("-")]
        if prog in _SHELLS:
            code = " ".join(args[args.index("-c") + 1:args.index("-c") + 2]) if "-c" in args else \
                (" ".join(args) if prog == "eval" else body_text)
            if code and _depth < 3 and shell_writes_records(code, root, _depth + 1, cwd):
                return True
            continue
        if prog in _WRITES_ARGS and any(hit(a) for a in files):
            return True
        if prog in _WRITES_DEST or prog == "mv":
            dests = [a.split("=", 1)[1] for a in args if a.startswith("--target-directory=")]
            if "-t" in args and args.index("-t") + 1 < len(args):
                dests.append(args[args.index("-t") + 1])
            if not dests and files:
                dests = [files[-1]]
            srcs = [f for f in files if f not in dests]
            if any(hit(d) or any(hit(d.rstrip("/") + "/" + os.path.basename(s.rstrip("/"))) for s in srcs)
                   for d in dests):
                return True
            if prog == "mv" and any(hit(s) for s in srcs):
                return True
            continue
        if prog == "dd" and any(a.startswith("of=") and hit(a[3:]) for a in args):
            return True
        if prog in ("sed", "gsed", "perl", "ruby") and any(_INPLACE.match(a) for a in args) \
                and any(hit(a) for a in files):
            return True
        if prog in ("tar", "bsdtar", "unzip") and (cwd and _REC_PATH.search(cwd + "/") or any(
                hit(args[i + 1]) for i, a in enumerate(args[:-1]) if a in ("-C", "-d", "--directory"))):
            return True
        if prog == "find" and any(hit(a) for a in files) and any(a in ("-delete", "-exec", "-execdir")
                                                                  for a in args):
            return True
        if prog == "git" and args:
            sub = args[0]
            if sub in ("checkout", "restore", "mv", "rm") and any(hit(a) for a in files[1:] + (
                    files[:1] if sub in ("restore", "rm", "mv") else [])):
                return True
            if (sub == "stash" and len(args) > 1 and args[1] in ("pop", "apply")) or sub in ("apply", "am"):
                if any(_REC_PATH.search(p) for p in _git_paths(root, args)):
                    return True
            continue
        if _INTERPRETERS.match(prog):
            code = " ".join(args) + "\n" + body_text
            if (_CODE_WRITE.search(code) or re.search(r"open\s*\(?[^)]*['\"]\s*\+?>", code)) and (
                    _REC_TEXT.search(code) or
                                             (cwd and _REC_PATH.search(cwd + "/"))):
                return True
    return False


def runs_hook_script(cmd, _depth=0, _cwd=""):
    """True when a shell command runs one of the project's hooks (scripts/hooks/*) or
    imports their helpers in an interpreter one-liner. Hooks are run by Claude Code and git
    with the runtime's own input; an agent piping a crafted payload into a gate would get a
    genuine-looking stamp or ledger line. Reading them (cat, grep, git diff) is fine."""
    if "hooks" not in cmd:
        return False
    text, bodies = _split_heredocs(cmd)
    body_text = "\n".join(b for _, b in bodies)
    try:
        segs = _segments(text)
    except ValueError:
        return bool(re.search(r"(bash|sh|python3?|source|\.)\s+\S*scripts/hooks/", cmd))
    cwd = _cwd

    def hook(tok):
        return bool(_HOOK_PATH.search(os.path.dirname(_at(cwd, tok)) + "/")) and not tok.startswith("-")
    for seg in segs:
        words = [w for w in seg if isinstance(w, str)]
        while words and (re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*=.*", words[0]) or words[0] in _PREFIX
                         or words[0] == "env"):
            words = words[1:]
        if not words:
            continue
        prog, args = words[0], words[1:]
        base = os.path.basename(prog)
        if base in ("cd", "pushd"):
            cwd = _cd(cwd, args)
            continue
        if "/" in prog and hook(prog):
            return True                     # ./scripts/hooks/review-gate.sh, an absolute path
        if base in _SHELLS or base in ("source", ".") or _INTERPRETERS.match(base):
            if "-c" in args and base in _SHELLS:
                code = " ".join(args[args.index("-c") + 1:args.index("-c") + 2])
                if _depth < 3 and runs_hook_script(code, _depth + 1, cwd):
                    return True
                continue
            files = [a for a in args if not a.startswith("-")]
            if files and hook(files[0]):
                return True
            code = " ".join(args) + "\n" + body_text
            if _INTERPRETERS.match(base) and re.search(r"scripts/hooks|_hookio", code):
                return True
            if base in _SHELLS and not files and body_text and _depth < 3 and \
                    runs_hook_script(body_text, _depth + 1, cwd):
                return True
    return False

def guard_state(data):
    """Deny agent writes to teamwright's own state; return silently otherwise.

    config.yml / installed.json: always a hard deny, whatever the mode (gate-first
    must not be able to switch itself on). Ledgers and journal: a gate decision
    (warns in gate-first). Only real writes count: a command that reads the file or
    merely names it (`git add`, `git commit`, `cat`, `grep`) is allowed. Config changes
    go to .teamwright/config.next.yml and through `tw-install.py apply --config`."""
    tool = data.get("tool_name") or ""
    ti = data.get("tool_input") or {}
    who = actor(data)
    gate = "state-guard"
    if tool == "Bash":
        cmd = ti.get("command") or ""
        if _OWNER_CLAIM.search(cmd):
            deny("STATE GUARD: TEAMWRIGHT_OWNER and CLAUDECODE are not set or unset by "
                 "agents. The owner override applies only when a person runs tw-install.py "
                 "outside Claude Code.", gate, data, hard=True)
        if who["agent_id"] and re.search(r"tw-install\.py\b", cmd):
            deny("STATE GUARD: only the main session runs tw-install.py, with the "
                 "owner (e.g. /teamwright:reconfigure); a subagent does not change the "
                 "teamwright setup. Write the proposal to .teamwright/config.next.yml and "
                 "hand it to the main session.", gate, data, hard=True)
        try:
            runs_hook = runs_hook_script(cmd)
        except Exception:
            runs_hook = False
        if runs_hook:
            deny("STATE GUARD: scripts/hooks/* are run by Claude Code and git with the runtime's "
                 "own input, never by an agent: a gate fed a hand-made payload would write a "
                 "genuine-looking ledger line or stamp. Read them (cat, grep) as you like; to test "
                 "a gate, a person runs the kit's tests outside Claude Code.", gate, data)
        try:
            writes = shell_writes_switch(cmd)
        except Exception:
            writes = bool(re.search(r"\.teamwright/(config\.yml|installed\.json)", cmd)
                          and re.search(r">|\b(tee|cp|mv|rm|sed)\b", cmd))
        if writes:
            deny("STATE GUARD: this command writes .teamwright/config.yml or "
                 ".teamwright/installed.json. Write the proposal to .teamwright/config.next.yml "
                 "and run `tw-install.py apply --config .teamwright/config.next.yml` from the "
                 "main session (it shows the diff, applies and removes the proposal). Reading, "
                 "`git add` and `git commit` of the config are allowed.", gate, data, hard=True)
        return
    if tool not in ("Edit", "Write", "MultiEdit", "NotebookEdit"):
        return
    rel = rel_path(data, ti.get("file_path") or ti.get("notebook_path") or "").replace(os.sep, "/")
    if rel in SWITCH_FILES:
        deny("STATE GUARD: %s is changed only through tw-install.py: write the proposal to "
             ".teamwright/config.next.yml (allowed) and run `tw-install.py apply --config "
             ".teamwright/config.next.yml` from the main session, or a person edits it outside "
             "Claude Code. Agents cannot switch the gates to gate-first." % rel, gate, data, hard=True)
    if LEDGER_RE.search(rel) or STAMP_RE.search(rel):
        deny("STATE GUARD: %s is written only by the hooks (ledgers, gate stamps and the journal "
             "are facts the gates rely on), never by an agent." % rel, gate, data)


def deny(reason, gate, data=None, hard=False, mode=None, warn_note=None):
    """Block (enforce) or warn (gate-first). hard=True blocks in every mode; mode
    ('enforce' | 'warn') overrides the project's gate mode for a gate with its own switch
    (the test guard), warn_note then replaces the gate-first prefix of a warning."""
    mode = "enforce" if hard else (mode or gate_mode(data))
    if mode == "warn":
        msg = (warn_note or "teamwright gate-first (warn only, run `tw-install.py enforce` to block): ") + reason
        print(json.dumps({
            "systemMessage": msg,
            "hookSpecificOutput": {"hookEventName": "PreToolUse", "additionalContext": msg},
        }))
    else:
        print(json.dumps({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        }))
    # Every warning and denial is a fact for the metrics (gates.jsonl, gitignored): how
    # often each gate fired, in which mode - the evidence for switching to enforce.
    src = data if isinstance(data, dict) else _INPUT
    try:
        who = actor(src)
        append_jsonl(os.path.join(log_dir(src), "gates.jsonl"), {
            "ts": now(), "event": "gate", "gate": gate, "decision": "warn" if mode == "warn" else "deny",
            "session_id": who.get("session_id", ""), "agent_id": who.get("agent_id", ""),
            "agent_type": who.get("agent_type", ""), "tool": src.get("tool_name") or "",
            "reason": reason.splitlines()[0][:200]})
    except Exception:
        pass
    log = os.environ.get("TEAMWRIGHT_GATE_LOG")
    if log:
        try:
            ts = now()
            with open(log, "a", encoding="utf-8") as fh:
                fh.write("%s %s %s %s\n" % (ts, gate, "WARN" if mode == "warn" else "DENY",
                                            reason.splitlines()[0][:160]))
        except OSError:
            pass
    sys.exit(0)
