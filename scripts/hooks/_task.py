#!/usr/bin/env python3
"""Minimal task-record reader shared by the teamwright hooks (stdlib only).

A task record is docs/tasks/<ID>.md with YAML-like front matter between two
`---` lines. Only the subset the hooks need is parsed: top-level `key: value`
pairs and one level of nested keys (e.g. the `analysis:` block). Values may
continue on deeper-indented lines (block scalars `|` / `>`).

CLI (used by the shell hooks):
    _task.py get <file|-> <key>          print a top-level value ('' if absent)
    _task.py analysis-ok <file|->        exit 0 if the analysis block is complete
"""
import os
import re
import sys

REQUIRED_ANALYSIS = os.environ.get(
    "TEAMWRIGHT_ANALYSIS_FIELDS", "root_cause evidence invariant seam approach"
).split()

# An rca task's deliverable is the analysis itself, so it cannot be exempt from it:
# root cause + evidence + the decision that removes the class.
REQUIRED_RCA = os.environ.get(
    "TEAMWRIGHT_RCA_FIELDS", "root_cause evidence decision"
).split()

PLACEHOLDER = re.compile(r"^(\.\.\.|…|todo|tbd|n/?a|none|-|<[^>]*>|\?+)$", re.I)
TASK_ID = os.environ.get("TEAMWRIGHT_TASK_ID_RE", r"[A-Z][A-Z0-9]*-\d+")


def is_task_file(name):
    """True for a task record's file name, `<ID>.md` (T-12.md) - not a template (_*.md) and
    not a side file such as T-089.history.md or T-12.notes.md."""
    return bool(re.fullmatch(r"(?:%s)\.md" % TASK_ID, name or ""))


def front_matter(text):
    """Return the front-matter lines (without the --- fences)."""
    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        return []
    for i in range(1, len(lines)):
        if lines[i].strip() == "---":
            return lines[1:i]
    return []


def _clean(value):
    value = value.strip()
    if value in ("|", ">", "|-", ">-"):
        return ""
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"":
        value = value[1:-1]
    return value.strip()


def parse(text):
    """-> dict: top-level key -> str value, or dict for nested blocks."""
    out = {}
    cur_key = None      # current top-level key with a nested/continued body
    cur_sub = None      # current nested key
    sub_indent = None
    for raw in front_matter(text):
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        indent = len(raw) - len(raw.lstrip(" "))
        line = raw.strip()
        m = re.match(r"^([A-Za-z_][\w-]*):(?:\s+(.*))?$", line)
        if indent == 0 and m:
            cur_key, cur_sub, sub_indent = m.group(1), None, None
            out[cur_key] = _clean(m.group(2) or "")
            continue
        if cur_key is None:
            continue
        val = out.get(cur_key)
        if m and (sub_indent is None or indent == sub_indent) and (
            isinstance(val, dict) or val == ""
        ):
            if not isinstance(val, dict):
                out[cur_key] = val = {}
            sub_indent = indent
            cur_sub = m.group(1)
            val[cur_sub] = _clean(m.group(2) or "")
            continue
        # continuation line
        if isinstance(val, dict) and cur_sub is not None:
            val[cur_sub] = (val[cur_sub] + " " + line).strip()
        elif isinstance(val, str):
            out[cur_key] = (val + " " + line).strip()
    return out


def status(rec):
    raw = rec.get("status") or ""
    return raw.split()[0] if isinstance(raw, str) and raw.split() else ""


def is_rca(rec):
    return bool(rec) and (rec.get("type") or "").strip() == "rca"


def required_fields(rec):
    return REQUIRED_RCA if is_rca(rec) else REQUIRED_ANALYSIS


def analysis_missing(rec):
    """List of missing analysis fields; [] when complete or legitimately exempt.

    `analysis: skipped:trivial` is the only exemption, and never for an rca task
    (declaring a task rca must not be a way around the analysis)."""
    a = rec.get("analysis")
    fields = required_fields(rec)
    if (isinstance(a, str) and a.replace(" ", "").startswith("skipped:trivial")
            and not is_rca(rec)):
        return []
    if not isinstance(a, dict):
        return list(fields)
    return [f for f in fields
            if not a.get(f) or PLACEHOLDER.match(a.get(f, "").strip())]


def first_word(value):
    return value.split()[0] if isinstance(value, str) and value.split() else ""


def body(text):
    """Text after the front matter ('' when there is none)."""
    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        return text
    for i in range(1, len(lines)):
        if lines[i].strip() == "---":
            return "\n".join(lines[i + 1:])
    return ""


def section(text, name):
    """Body of the `## <name>` section of a task record ('' when absent), up to the next
    `## ` heading."""
    out, inside = [], False
    for line in body(text).splitlines():
        if re.match(r"^##\s", line):
            if inside:
                break
            inside = line[2:].strip().lower() == name.lower()
            continue
        if inside:
            out.append(line)
    return "\n".join(out)


def _without_placeholders(text):
    """Text with HTML comments and template placeholder blocks removed - a block that
    starts a line with `<` and ends a line with `>` (`<test-writer: ...>`)."""
    text = re.sub(r"(?s)<!--.*?-->", "", text)
    return re.sub(r"(?ms)^<[^<\n][^\n]*?(\n[^\n]*?)*?>[ \t]*$", "", text)


def tests_written(text):
    """True when the record's `## Tests` section holds something besides the template
    placeholder: the test-writer's first spawn wrote the red tests down."""
    return bool(_without_placeholders(section(text, "Tests")).strip())


def failure_branches(text):
    """True when `## Tests` has a `### Failure branches` heading followed by a table row
    (the test-writer's second spawn, in the implement stage)."""
    sec = section(text, "Tests")
    m = re.search(r"(?im)^###\s+failure branches\b.*$", sec)
    if not m:
        return False
    rows = [l for l in sec[m.end():].splitlines() if l.strip().startswith("|")
            and not set(l.strip().strip("|").replace("|", "")) <= set("-: ")]
    return len(rows) >= 2  # header + one row


def task_stage(rec):
    """The `stage:` field of a task record, lower-case ('' when absent)."""
    v = rec.get("stage") if isinstance(rec.get("stage"), str) else ""
    return v.strip().lower()


# A code file, as the analysis gate's check 2 reads it: anything outside docs/, .teamwright/,
# .claude/, .github/ and top-level Markdown files ($TEAMWRIGHT_NONCODE_RE overrides).
NONCODE_RE = os.environ.get("TEAMWRIGHT_NONCODE_RE",
                            r"^(docs/|\.teamwright/|\.claude/|\.github/|[^/]*\.md$)")


def is_code(path):
    return bool(path) and not path.startswith("..") and not re.search(NONCODE_RE, path)


def code_changed(root, tid):
    """True when the task changes code: a commit whose message names the task id (subject,
    body or trailer - the commit convention), or a tracked uncommitted change in the tree,
    touches a code file (is_code). An rca exit (b)/(c) or a docs-only task does not. False
    when git is unavailable (nothing to check against)."""
    import subprocess

    def git(*args):
        try:
            p = subprocess.run(("git",) + args, cwd=root, capture_output=True, text=True, timeout=20)
        except Exception:
            return ""
        return p.stdout if p.returncode == 0 else ""

    pat = r"(^|[^A-Za-z0-9_-])" + re.escape(tid) + r"([^A-Za-z0-9_]|$)"
    files = git("log", "--format=", "--name-only", "-E", "--grep=" + pat, "HEAD").splitlines()
    files += git("diff", "--name-only", "HEAD").splitlines()
    return any(is_code(f.strip()) for f in files)


def _rounds(root, tid, suffix, with_body=False):
    d = os.path.join(root, "docs", "tasks", tid + suffix)
    try:
        names = [f for f in os.listdir(d) if re.match(r"^\d+\.md$", f)]
    except OSError:
        return []
    out = []
    for n in sorted(int(f[:-3]) for f in names):
        try:
            with open(os.path.join(d, "%d.md" % n), encoding="utf-8", errors="replace") as fh:
                text = fh.read()
        except OSError:
            text = ""
        rec = parse(text)
        if with_body:
            rec["_body"] = body(text)
        out.append((n, rec))
    return out


def verify_rounds(root, tid):
    """[(round, record)] for docs/tasks/<tid>.verify/<n>.md, sorted by round.
    Each record also carries its body under the key `_body`."""
    return _rounds(root, tid, ".verify", with_body=True)


def validation_rounds(root, tid):
    """[(round, record)] for docs/tasks/<tid>.validation/<n>.md: task-validator's
    acceptance verdicts (`verdict:`, `commit:`), sorted by round."""
    return _rounds(root, tid, ".validation")


def review_rounds(root, tid):
    """[(round, record)] for docs/tasks/<tid>.reviews/<n>.md, sorted by round.
    The count of these files IS review_rounds; the task field is never trusted."""
    return _rounds(root, tid, ".reviews")



# Process records, not code: task, review, verify and validation records, sprints,
# process reviews, decisions and open questions. A commit that touches only these
# (a decision naming a task, a status change) does not make the task's checks stale.
# Same set as scripts/tw-ci-changes and scripts/tw-next.py (RECORD_RE).
RECORD_RE = re.compile(r"^docs/tasks/|^docs/sprints/|^docs/process-reviews/"
                       r"|^docs/(decisions|open-questions)\.md$")


def is_record(path):
    return bool(RECORD_RE.search(path or ""))


def findings(text):
    """[(severity, category)] from the findings table of a review record: the first table
    with a Severity column (Category optional) under a heading that starts with "Findings"
    at any level, or - when no heading says so - the first such table in the record.
    Reviewers often put the table straight under `### Review round N`; reading only
    `## Findings` saw no findings there, so the blocking-threshold rule refused honest
    send-backs and let an APPROVE with blocking rows through. Rows without a severity
    are skipped."""
    tables, cols, under, current = [], None, False, None
    for line in text.splitlines():
        m = re.match(r"^(#{1,6})\s+(.*)$", line)
        if m:
            under = m.group(2).strip().lower().startswith("findings")
            cols, current = None, None
            continue
        if not line.strip().startswith("|"):
            cols, current = None, None
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if cols is None:
            if current is not None:
                continue
            low = [c.lower() for c in cells]
            current = []
            if "severity" in low:
                cols = (low.index("severity"), low.index("category") if "category" in low else None)
                tables.append((under, current))
            continue
        if all(set(c) <= set("-: ") for c in cells):
            continue
        sev = cells[cols[0]] if cols[0] < len(cells) else ""
        if sev:
            cat = cells[cols[1]] if cols[1] is not None and cols[1] < len(cells) else ""
            current.append((sev, cat))
    for under, rows in tables:
        if under:
            return rows
    return tables[0][1] if tables else []

def read(path):
    if path == "-":
        return sys.stdin.read()
    with open(path, encoding="utf-8", errors="replace") as fh:
        return fh.read()


def main(argv):
    if len(argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    cmd, src = argv[1], argv[2]
    try:
        rec = parse(read(src))
    except OSError:
        return 3
    if cmd == "get" and len(argv) > 3:
        v = rec.get(argv[3], "")
        print(v if isinstance(v, str) else "<block>")
        return 0
    if cmd == "analysis-ok":
        missing = analysis_missing(rec)
        if missing:
            print(" ".join(missing))
            return 1
        return 0
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
