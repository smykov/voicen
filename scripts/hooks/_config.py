#!/usr/bin/env python3
"""Reader for .teamwright/config.yml, shared by the gates and tw-install.py (stdlib only).

One parsing rule for both, so the mode the installer reports is the mode the gates
apply. The file is a small YAML subset or JSON (see docs/process/plugin.md in the kit).

Mode rule: `gate-first` (gates warn) only when the config says so explicitly;
a missing, unknown or unreadable mode is `enforce` (gates block). Failing towards
blocking is the safe side: a broken config must not silently switch gates off.
"""
import json
import os
import re

CONFIG = os.path.join(".teamwright", "config.yml")
MODES = ("gate-first", "enforce")
DEFAULT_MODE = "enforce"


# ---------------------------------------------------------------- YAML subset
def _scalar(v):
    v = v.strip()
    if not v:
        return ""
    if v[0] in "{[":
        try:
            return json.loads(v)
        except ValueError:
            if v[0] == "[" and v.endswith("]"):
                return [_scalar(x) for x in _split_flow(v[1:-1])]
            raise ValueError("bad flow value: %s" % v)
    if v[0] in "\"'" and v.endswith(v[0]) and len(v) >= 2:
        return json.loads(v) if v[0] == '"' else v[1:-1].replace("''", "'")
    low = v.lower()
    if low in ("true", "yes"):
        return True
    if low in ("false", "no"):
        return False
    if low in ("null", "~"):
        return None
    if re.fullmatch(r"-?\d+", v):
        return int(v)
    return v


def _split_flow(s):
    out, cur, q = [], "", None
    for ch in s:
        if q:
            cur += ch
            if ch == q:
                q = None
        elif ch in "\"'":
            q = ch
            cur += ch
        elif ch == ",":
            out.append(cur)
            cur = ""
        else:
            cur += ch
    if cur.strip():
        out.append(cur)
    return [x.strip() for x in out]


def _strip_comment(line):
    q = None
    for i, ch in enumerate(line):
        if q:
            if ch == q:
                q = None
        elif ch in "\"'":
            q = ch
        elif ch == "#" and (i == 0 or line[i - 1] in " \t"):
            return line[:i].rstrip()
    return line.rstrip()


def is_json(text):
    return text.lstrip().startswith("{")


def parse_yaml(text):
    """Block maps, `- ` lists (of scalars or maps), flow lists, one-line JSON values.
    A document starting with `{` is read as JSON."""
    if is_json(text):
        return json.loads(text)
    lines = []
    for raw in text.splitlines():
        if raw.strip() in ("---", "..."):
            continue
        line = _strip_comment(raw)
        if line.strip():
            lines.append((len(line) - len(line.lstrip(" ")), line.strip()))
    pos = [0]

    def block(indent):
        if pos[0] >= len(lines):
            return None
        if lines[pos[0]][1].startswith("- ") or lines[pos[0]][1] == "-":
            return seq(lines[pos[0]][0])
        return mapping(lines[pos[0]][0])

    def mapping(indent, into=None):
        out = into if into is not None else {}
        while pos[0] < len(lines):
            ind, s = lines[pos[0]]
            if ind < indent or s.startswith("- "):
                break
            if ind > indent:
                raise ValueError("unexpected indent: %s" % s)
            m = re.match(r"^([^:\s][^:]*?)\s*:(\s+(.*))?$", s)
            if not m:
                raise ValueError("expected key: value, got: %s" % s)
            key, val = m.group(1).strip().strip("\"'"), (m.group(3) or "")
            pos[0] += 1
            if val.strip():
                out[key] = _scalar(val)
            elif pos[0] < len(lines) and (lines[pos[0]][0] > ind or
                                          (lines[pos[0]][0] == ind and lines[pos[0]][1].startswith("- "))):
                out[key] = block(lines[pos[0]][0])
            else:
                out[key] = None
        return out

    def seq(indent):
        out = []
        while pos[0] < len(lines):
            ind, s = lines[pos[0]]
            if ind != indent or not (s.startswith("- ") or s == "-"):
                break
            rest = s[1:].strip()
            if re.match(r"^[^:\s\[{\"'][^:]*?\s*:(\s|$)", rest):
                # "- key: v" -> map whose keys continue at indent + 2
                lines[pos[0]] = (indent + 2, rest)
                out.append(mapping(indent + 2))
            elif rest:
                pos[0] += 1
                out.append(_scalar(rest))
            else:
                pos[0] += 1
                out.append(block(lines[pos[0]][0]) if pos[0] < len(lines) else None)
        return out

    res = block(0)
    if pos[0] < len(lines):
        raise ValueError("could not parse line: %s" % lines[pos[0]][1])
    return res or {}


# ---------------------------------------------------------------- mode
def mode_of(cfg):
    """Effective mode of a parsed config: explicit `gate-first`, otherwise enforce."""
    m = cfg.get("mode") if isinstance(cfg, dict) else None
    return m if m in MODES else DEFAULT_MODE


def read_mode(path):
    """Effective mode of a config file; missing or unreadable -> enforce."""
    try:
        with open(path, encoding="utf-8") as fh:
            return mode_of(parse_yaml(fh.read()))
    except (OSError, ValueError, UnicodeDecodeError):
        return DEFAULT_MODE


def set_mode(text, mode):
    """Config text with the top-level `mode` set to `mode` (JSON or YAML subset).

    JSON: the key is rewritten and the file re-serialised. YAML: an existing top-level
    `mode:` line is replaced (a trailing comment is kept); otherwise the key is inserted
    at the top level, after leading comments and a `version:` line when present."""
    if is_json(text):
        cfg = json.loads(text)
        cfg["mode"] = mode
        return json.dumps(cfg, indent=2, ensure_ascii=False) + "\n"
    if re.search(r"^mode:", text, re.M):
        return re.sub(r"^mode:[ \t]*(\"[^\"\n]*\"|'[^'\n]*'|[^\s#]*)", "mode: " + mode, text,
                      count=1, flags=re.M)
    lines = text.splitlines(True)
    at = 0
    for i, line in enumerate(lines):
        s = line.strip()
        if not s or s.startswith("#") or s == "---":
            at = i + 1
            continue
        if re.match(r"^version:", line):
            at = i + 1
        break
    if at and not lines[at - 1].endswith("\n"):
        lines[at - 1] += "\n"
    lines.insert(at, "mode: %s\n" % mode)
    return "".join(lines)


# ---------------------------------------------------------------- process settings
# Parameters of fixed pipeline steps (`process:` in config.yml). The pipeline itself -
# statuses, roles, gates, records - never depends on them. A missing key means the
# behaviour before settings existed, so updating the kit changes nothing by itself.
SEVERITIES = ("low", "medium", "high")
PRIORITIES = ("P0", "P1", "P2", "P3")
PROCESS_DEFAULTS = {
    "review": {"blocking": "low"},
    "batch": {"max_tasks": 1, "by_priority": {"P0": 1, "P1": 1, "P2": 3, "P3": 3}, "parallel": 1},
    "autonomy": {"start": "ask", "continue": False},
}
# Recommended for new projects (offered by adopt, init and /teamwright:settings).
PROCESS_RECOMMENDED = {
    "review": {"blocking": "medium"},
    "batch": {"max_tasks": 3, "by_priority": {"P0": 1, "P1": 2, "P2": 3, "P3": 3}, "parallel": 1},
    "autonomy": {"start": "auto", "continue": True},
}
# Findings that block whatever the threshold: the threshold saves rounds on style and
# small drift, never on what is dangerous.
ALWAYS_BLOCKING = ("secret-leak", "security", "weakened-test", "done-not-working", "recurrence")
MAX_BATCH = 5
MAX_PARALLEL = 4


def _bool(v):
    if isinstance(v, bool):
        return v
    return str(v).strip().lower() in ("true", "yes", "on", "1")


def process_errors(cfg):
    """Validation messages for the `process:` block (closed list of keys)."""
    p = cfg.get("process") if isinstance(cfg, dict) else None
    if p is None:
        return []
    if not isinstance(p, dict):
        return ["process must be a mapping"]
    errs = []
    allowed = {"review": ("blocking",), "batch": ("max_tasks", "by_priority", "parallel"),
               "autonomy": ("start", "continue")}
    for k, v in p.items():
        if k not in allowed:
            errs.append("process.%s: unknown key (known: %s)" % (k, ", ".join(allowed)))
            continue
        if not isinstance(v, dict):
            errs.append("process.%s must be a mapping" % k)
            continue
        for sk in v:
            if sk not in allowed[k]:
                errs.append("process.%s.%s: unknown key (known: %s)" % (k, sk, ", ".join(allowed[k])))
    s = p.get("review") if isinstance(p.get("review"), dict) else {}
    if "blocking" in s and str(s["blocking"]).lower() not in SEVERITIES:
        errs.append("process.review.blocking must be one of %s" % ", ".join(SEVERITIES))
    b = p.get("batch") if isinstance(p.get("batch"), dict) else {}
    if "max_tasks" in b and not (str(b["max_tasks"]).isdigit() and 1 <= int(b["max_tasks"]) <= MAX_BATCH):
        errs.append("process.batch.max_tasks must be 1..%d" % MAX_BATCH)
    if "parallel" in b and not (str(b["parallel"]).isdigit() and 1 <= int(b["parallel"]) <= MAX_PARALLEL):
        errs.append("process.batch.parallel must be 1..%d" % MAX_PARALLEL)
    bp = b.get("by_priority")
    if bp is not None:
        if not isinstance(bp, dict):
            errs.append("process.batch.by_priority must be a mapping P0..P3 -> 1..%d" % MAX_BATCH)
        else:
            for pk, pv in bp.items():
                if pk not in PRIORITIES or not (str(pv).isdigit() and 1 <= int(pv) <= MAX_BATCH):
                    errs.append("process.batch.by_priority.%s: priority P0..P3, value 1..%d" % (pk, MAX_BATCH))
    a = p.get("autonomy") if isinstance(p.get("autonomy"), dict) else {}
    if "start" in a and str(a["start"]).lower() not in ("ask", "auto"):
        errs.append("process.autonomy.start must be ask or auto")
    if "continue" in a and str(a["continue"]).strip().lower() not in ("true", "false", "yes", "no"):
        errs.append("process.autonomy.continue must be true or false")
    return errs


def process_of(cfg):
    """Effective settings: {review: {blocking}, batch: {max_tasks, by_priority},
    autonomy: {start, continue}, explicit: [dotted keys set in the config]}.
    Invalid values fall back to the defaults (the installer refuses them on apply)."""
    p = cfg.get("process") if isinstance(cfg, dict) and isinstance(cfg.get("process"), dict) else {}
    out = json.loads(json.dumps(PROCESS_DEFAULTS))
    explicit = []
    r, b, a = (p.get(k) if isinstance(p.get(k), dict) else {} for k in ("review", "batch", "autonomy"))
    if str(r.get("blocking", "")).lower() in SEVERITIES:
        out["review"]["blocking"] = str(r["blocking"]).lower()
        explicit.append("review.blocking")
    if str(b.get("max_tasks", "")).isdigit() and 1 <= int(b["max_tasks"]) <= MAX_BATCH:
        out["batch"]["max_tasks"] = int(b["max_tasks"])
        explicit.append("batch.max_tasks")
    if str(b.get("parallel", "")).isdigit() and 1 <= int(b["parallel"]) <= MAX_PARALLEL:
        out["batch"]["parallel"] = int(b["parallel"])
        explicit.append("batch.parallel")
    if isinstance(b.get("by_priority"), dict):
        for pk, pv in b["by_priority"].items():
            if pk in PRIORITIES and str(pv).isdigit() and 1 <= int(pv) <= MAX_BATCH:
                out["batch"]["by_priority"][pk] = int(pv)
                explicit.append("batch.by_priority.%s" % pk)
    if str(a.get("start", "")).lower() in ("ask", "auto"):
        out["autonomy"]["start"] = str(a["start"]).lower()
        explicit.append("autonomy.start")
    if "continue" in a:
        out["autonomy"]["continue"] = _bool(a["continue"])
        explicit.append("autonomy.continue")
    out["explicit"] = explicit
    return out


def read_process(path):
    """Effective process settings of a config file; missing or unreadable -> defaults."""
    try:
        with open(path, encoding="utf-8") as fh:
            return process_of(parse_yaml(fh.read()))
    except (OSError, ValueError, UnicodeDecodeError):
        return process_of({})


def severity_blocks(severity, category, blocking):
    """Does a review finding return the task under threshold `blocking`?"""
    sev = str(severity or "").strip().lower()
    cat = str(category or "").strip().lower()
    if sev == "critical" or cat in ALWAYS_BLOCKING:
        return True
    rank = {"low": 0, "medium": 1, "high": 2}
    return sev in rank and rank[sev] >= rank.get(blocking, 0)
