#!/usr/bin/env bash
# verify-gate.sh — Claude Code PreToolUse gate (Edit|Write|Bash). Tier T3.
#
# WHY. "Done" was declared from green unit tests and an approved diff, and the
# feature did not work in the running system: the endpoint returned 500 behind the
# real config, the button was never wired, the failure branch from Acceptance was
# never exercised end to end. Review reads code; only running the system proves it.
#
# VERIFY RECORD: docs/tasks/<ID>.verify/<n>.md (n = round, 1, 2, ...), append-only:
#   ---
#   task: T-042
#   round: 1
#   kind: ui                   # api | ui | smoke | manual
#   tool: playwright           # REQUIRED for kind ui: playwright, flutter-integration-test,
#                              # patrol, maestro, espresso, xcuitest, ...
#   command: npx playwright test e2e/login.spec.ts
#   environment: staging       # local | staging | production-like (no hostnames)
#   commit: 3f2c1ab            # the revision that was verified
#   result: PASS               # PASS | FAIL
#   evidence: playwright-report/   # report / log / trace / junit xml (path or CI artifact URL)
#   verifier: task-validator   # writer's agent_type (`main` for a top-level session
#                              # without one), or human:<name> for a person
#   ---
#   body: scenarios checked; MUST contain a `Failure branch: ...` line (the task's
#   failure branch from Acceptance).
#
# BLOCKS
#   1. Writing a verify record that is not the next round, or editing/overwriting an
#      existing round (append-only: a new run is a new round).
#   2. A verify record with missing/invalid fields (above), a commit that does not
#      resolve, a local evidence path that does not exist, no failure-branch line,
#      `verifier` not matching the writer, a `human:` marker written by an agent, or
#      `kind: manual` on a task without `verify_exception: "<reason>"`.
#   3. docs/tasks/<ID>.md entering VERIFIED or DONE unless, per the task's `surface`:
#        ui   -> latest `ui` record PASS          api  -> latest `api` record PASS
#        both -> latest `api` AND latest `ui` PASS
#        none -> no record needed when `deploys: false`; else latest api|ui|smoke PASS
#      and the latest record overall is not FAIL (FAIL -> the task goes to VERIFY_FAIL),
#      each counted record is fresh (commit is an ancestor-or-equal of HEAD and contains
#      the task's last code commit - see FRESHNESS), was written through this gate with
#      the same kind/result/commit (ledger) or is a person's committed record.
#      `kind: manual` counts only with `verify_exception`, and at least one fresh manual
#      PASS must come from an agent that did not implement the task (or a person).
#   4. Shell commands that write verify records.
#   5. Acceptance verdicts - validation records docs/tasks/<ID>.validation/<n>.md
#      (verdict PASS|FAIL|NEEDS_OWNER, commit, validator) - written by anyone but
#      task-validator, by a shell command, out of round order, or over an existing round.
#      After two FAIL rounds in a row, the next round must be `method: sweep` (every
#      condition, guard, return and write of the changed code mutated) or NEEDS_OWNER:
#      sampled mutations find one survivor per round and the loop does not end.
#      Entering DONE needs the latest round PASS, written through this gate (ledger),
#      fresh for the task's code - for every surface, including `deploys: false` +
#      `surface: none`, where it is the only check.
#
# FRESHNESS. The task's code commits are commits whose message names the task id
# (subject, body or trailer - the commit convention requires it) and that touch files
# other than process records (docs/tasks/, docs/sprints/, docs/process-reviews/,
# docs/decisions.md, docs/open-questions.md: a decision naming the task is not code).
# The newest of them must be contained in the verified commit. If no commit names the
# task, the verified commit must equal HEAD, or every change since it must be a record.
#
# LEDGER (local, gitignored): .teamwright/sessions/<ID>.verify.jsonl - who wrote which
# round, kind, result, commit; <ID>.validation.jsonl - acceptance verdicts. Implementers: .teamwright/sessions/<ID>.dev (review gate).
# SAFETY: fail-open on any internal error; silent on allow. Freshness is skipped when
# git is unavailable.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export TEAMWRIGHT_HOOKS_DIR="$HERE"
command -v python3 >/dev/null 2>&1 || exit 0

IFS= read -r -d '' _tw_py <<'PY' || true
import os, re, subprocess, sys
sys.dont_write_bytecode = True
sys.path.insert(0, os.environ["TEAMWRIGHT_HOOKS_DIR"])
try:
    import _hookio as io, _task as task
except Exception:
    sys.exit(0)

KINDS = ("api", "ui", "smoke", "manual")
ENVS = ("local", "staging", "production-like")
RESULTS = ("PASS", "FAIL")
VERDICTS = ("PASS", "FAIL", "NEEDS_OWNER")
SURFACES = ("ui", "api", "both", "none")
VALIDATOR = os.environ.get("TEAMWRIGHT_VALIDATOR_AGENT", "task-validator")
REQUIRED = {"ui": [("ui",)], "api": [("api",)], "both": [("api",), ("ui",)],
            "none": [("api", "ui", "smoke")]}
SHA = re.compile(r"^[0-9a-fA-F]{7,40}$")
FB_LINE = re.compile(r"(?im)^\s*(?:[-*]\s*)?(?:\[[ xX]\]\s*)?\**failure branch\**\s*[:\-]\s*(.*)$")

data = io.load_input()
if not data or io.event(data) != "PreToolUse":
    sys.exit(0)
tool = data.get("tool_name") or ""
if tool not in ("Edit", "Write", "MultiEdit", "Bash"):
    sys.exit(0)
root = io.project_root(data)
ti = data.get("tool_input") or {}
who = io.actor(data)
key = io.actor_key(who)

def deny(msg):
    io.deny("VERIFY GATE: " + msg, "verify-gate")

# ---- 4. shell writes into verify records -----------------------------------------
if tool == "Bash":
    cmd = ti.get("command") or ""
    target = r"docs/tasks/[^\s'\"]*\.(verify|validation)(/|\b)"
    if re.search(target, cmd) and (
            re.search(r">\s*['\"]?\S*" + target, cmd)
            or re.search(r"\b(tee|cp|mv|rm|truncate|touch|ln|install|rsync)\b", cmd)
            or re.search(r"\b(sed|perl)\s+-[a-zA-Z]*i", cmd)):
        deny("verify and validation records are written only through Edit/Write, one new "
             "round per run, never by shell commands - otherwise the gate cannot check who wrote "
             "them or keep them append-only.")
    sys.exit(0)

rel = io.rel_path(data, ti.get("file_path") or "")

def git(*args):
    try:
        p = subprocess.run(("git",) + args, cwd=root, capture_output=True, text=True,
                           timeout=20)
    except Exception:
        return None
    return p.stdout.strip() if p.returncode == 0 else ""

def resolve(rev):
    out = git("rev-parse", "--verify", "--quiet", rev + "^{commit}")
    return out  # None: git unusable; "": unknown revision

def is_ancestor(a, b):
    try:
        return subprocess.run(("git", "merge-base", "--is-ancestor", a, b), cwd=root,
                              capture_output=True, timeout=20).returncode == 0
    except Exception:
        return True

def last_code_commit(tid):
    pat = r"(^|[^A-Za-z0-9_-])" + re.escape(tid) + r"([^A-Za-z0-9_]|$)"
    out = git("log", "--format=%H", "-E", "--grep=" + pat, "HEAD")
    for sha in (out or "").split():
        files = git("show", "--name-only", "--format=", sha) or ""
        if any(f and not is_record(f) for f in files.splitlines()):
            return sha
    return ""

def is_record(f):
    return task.is_record(f) if hasattr(task, "is_record") else f.startswith("docs/tasks/")

def records_only_since(c):
    """True when every change between c and HEAD is a process record (task, review,
    verify and validation records, decisions, open questions): the code that was
    checked is still HEAD's."""
    out = git("diff", "--name-only", c, "HEAD")
    return out is not None and all(is_record(f) for f in out.splitlines() if f)

def sessions_dir():
    return os.path.join(root, ".teamwright", "sessions")

def dev_keys(tid):
    try:
        return {l.strip() for l in open(os.path.join(sessions_dir(), tid + ".dev")) if l.strip()}
    except OSError:
        return set()

def ledger(tid):
    return os.path.join(sessions_dir(), tid + ".verify.jsonl")

def validation_ledger(tid):
    return os.path.join(sessions_dir(), tid + ".validation.jsonl")

def ledger_entry(tid, n):
    hit = None
    for r in io.read_jsonl(ledger(tid)):
        if r.get("round") == n:
            hit = r
    return hit

def filled(v):
    v = (v or "").strip() if isinstance(v, str) else ""
    return bool(v) and not task.PLACEHOLDER.match(v)

def is_human(v):
    return (v or "").strip().lower().startswith("human")

def w(rec, k):
    return task.first_word(rec.get(k) if isinstance(rec.get(k), str) else "")

def field_problems(rec, tid, n):
    """Structural problems of one record (shared by write time and transition)."""
    p = []
    if (rec.get("task") or "").strip() != tid:
        p.append("`task:` must be %s" % tid)
    if (rec.get("round") or "").strip() != str(n):
        p.append("`round:` must equal the file name (%d)" % n)
    if w(rec, "kind") not in KINDS:
        p.append("`kind:` one of %s" % " | ".join(KINDS))
    if w(rec, "kind") == "ui" and not filled(rec.get("tool")):
        p.append("`tool:` for a ui record (playwright for web; flutter-integration-test, "
                 "patrol, maestro, espresso, xcuitest ... for mobile)")
    if not filled(rec.get("command")):
        p.append("`command:` the exact command that was run")
    if w(rec, "environment") not in ENVS:
        p.append("`environment:` one of %s (no hostnames)" % " | ".join(ENVS))
    if not SHA.match(w(rec, "commit")):
        p.append("`commit:` the verified sha (7-40 hex)")
    if w(rec, "result") not in RESULTS:
        p.append("`result:` PASS | FAIL")
    if not filled(rec.get("evidence")):
        p.append("`evidence:` path to the report / log / trace artifact")
    if not filled(rec.get("verifier")):
        p.append("`verifier:` agent_type or human:<name>")
    fb = [m.group(1).strip() for m in FB_LINE.finditer(rec.get("_body") or "")]
    if not any(filled(x) for x in fb):
        p.append("a `Failure branch: <input> -> <observed>` line in the body (the task's "
                 "failure branch from Acceptance)")
    return p

def exception_of(trec):
    v = trec.get("verify_exception")
    return v.strip() if filled(v) else ""

def read_task(tid):
    try:
        return task.parse(open(os.path.join(root, "docs", "tasks", tid + ".md"),
                               encoding="utf-8", errors="replace").read())
    except OSError:
        return {}

# ---- 1, 2. verify record being written ---------------------------------------------
m = io.VERIFY_FILE_RE.search(rel)
if m and rel.startswith("docs/tasks/"):
    tid, n = m.group(1), int(m.group(2))
    path = os.path.join(root, rel)
    if os.path.exists(path):
        deny("docs/tasks/%s.verify/%d.md already exists. Verify records are append-only: "
             "a new run (or a correction) is a new round, %s.verify/%d.md."
             % (tid, n, tid, max([r for r, _ in task.verify_rounds(root, tid)] + [n]) + 1))
    existing = [r for r, _ in task.verify_rounds(root, tid)]
    nxt = (max(existing) + 1) if existing else 1
    if n != nxt:
        deny("the next verify round of %s is %d, not %d (rounds are consecutive files)."
             % (tid, nxt, n))
    _, after = io.before_after(data)
    if after is None:
        sys.exit(0)
    rec = task.parse(after)
    rec["_body"] = task.body(after)
    probs = field_problems(rec, tid, n)
    if probs:
        deny("verify record %s round %d is incomplete: %s. Template: "
             "docs/tasks/_verify-template.md." % (tid, n, "; ".join(probs)))
    verifier = (rec.get("verifier") or "").strip()
    if is_human(verifier):
        deny("`verifier: human:...` marks a record written by a person outside Claude "
             "Code and committed; an agent cannot write it.")
    expected = who["agent_type"] or "main"
    if verifier != expected:
        deny("`verifier: %s` does not match the writer (agent_type=%r); write "
             "`verifier: %s`." % (verifier, who["agent_type"] or "<none>", expected))
    commit = w(rec, "commit")
    full = resolve(commit)
    if full == "":
        deny("commit %s does not resolve in this repository; verify a committed "
             "revision and record its sha." % commit)
    ev = (rec.get("evidence") or "").strip()
    if "://" not in ev and not os.path.exists(os.path.join(root, ev)):
        deny("evidence %r does not exist. Point it at the artifact the run produced "
             "(report dir, junit xml, trace, log) or at the CI artifact URL." % ev)
    kind = w(rec, "kind")
    if kind == "manual" and not exception_of(read_task(tid)):
        deny("`kind: manual` is allowed only when the task records an explicit, reviewed "
             "exception: `verify_exception: \"<why the system cannot be checked by an "
             "api/ui test>\"` in docs/tasks/%s.md. Otherwise write an API test or a UI "
             "test (Playwright for web, an integration/e2e driver for mobile)." % tid)
    io.state_dir(data, "sessions")
    io.append_jsonl(ledger(tid), dict(who, ts=io.now(), round=n, kind=kind,
                                      result=w(rec, "result"), commit=commit))
    sys.exit(0)

# ---- 6. validation record (acceptance verdict) being written ---------------------------
m = io.VALIDATION_FILE_RE.search(rel)
if m and rel.startswith("docs/tasks/"):
    tid, n = m.group(1), int(m.group(2))
    existing = [r for r, _ in task.validation_rounds(root, tid)]
    if os.path.exists(os.path.join(root, rel)):
        deny("docs/tasks/%s.validation/%d.md already exists. Acceptance verdicts are "
             "append-only: a new check is a new round, %s.validation/%d.md."
             % (tid, n, tid, max(existing + [n]) + 1))
    nxt = (max(existing) + 1) if existing else 1
    if n != nxt:
        deny("the next validation round of %s is %d, not %d (rounds are consecutive files)."
             % (tid, nxt, n))
    if (who["agent_type"] or "") != VALIDATOR:
        deny("acceptance verdicts are written by a fresh %s, not by %s. Spawn it with "
             "the task id." % (VALIDATOR, who["agent_type"] or "the main session"))
    _, after = io.before_after(data)
    if after is None:
        sys.exit(0)
    rec = task.parse(after)
    p = []
    if (rec.get("task") or "").strip() != tid:
        p.append("`task:` must be %s" % tid)
    if (rec.get("round") or "").strip() != str(n):
        p.append("`round:` must equal the file name (%d)" % n)
    if w(rec, "verdict") not in VERDICTS:
        p.append("`verdict:` one of %s" % " | ".join(VERDICTS))
    if not SHA.match(w(rec, "commit")):
        p.append("`commit:` the sha of the revision that was checked (7-40 hex)")
    method = w(rec, "method").lower() or "sample"
    if method not in ("sample", "sweep"):
        p.append("`method:` sample | sweep")
    if w(rec, "validator") != VALIDATOR:
        p.append("`validator: %s`" % VALIDATOR)
    if p:
        deny("validation record %s round %d is incomplete: %s. Template: "
             "docs/tasks/_validation-template.md." % (tid, n, "; ".join(p)))
    prev = [w(r, "verdict") for _, r in task.validation_rounds(root, tid)][-2:]
    if prev == ["FAIL", "FAIL"] and method != "sweep" and w(rec, "verdict") != "NEEDS_OWNER":
        # two sampling rounds in a row each found a different survivor: a third sample
        # would find a third. Like two REQUEST_CHANGES in review: stop sampling.
        deny("validation of %s failed twice in a row; round %d is not another sample of 3-6 "
             "mutations: either `method: sweep` - mutate every condition, guard, return and "
             "write in the task's changed code, re-run the test-writer's wrong implementations "
             "and every mutation of earlier rounds, record killed / survived / equivalent - or "
             "`verdict: NEEDS_OWNER` to escalate." % (tid, n))
    if resolve(w(rec, "commit")) == "":
        deny("commit %s does not resolve in this repository; check a committed revision "
             "and record its sha." % w(rec, "commit"))
    io.state_dir(data, "sessions")
    io.append_jsonl(validation_ledger(tid), dict(who, ts=io.now(), round=n,
                                                 verdict=w(rec, "verdict"), commit=w(rec, "commit")))
    sys.exit(0)

# ---- 3. task record entering VERIFIED / DONE ---------------------------------------
m = io.TASK_FILE_RE.search(rel)
if not (m and rel.startswith("docs/tasks/")):
    sys.exit(0)
tid = m.group(1)
before, after = io.before_after(data)
if after is None:
    sys.exit(0)
rb, ra = task.parse(before or ""), task.parse(after)
sb, sa = task.status(rb), task.status(ra)

if sa == sb or sa not in ("VERIFIED", "DONE"):
    sys.exit(0)
head = resolve("HEAD")
last_code = last_code_commit(tid) if head else ""

def stale(n, sha, what):
    """'' if the revision `sha` covers the task's current code; otherwise the reason."""
    if head is None or head == "":
        return ""  # git unusable or no commits: freshness cannot be checked
    c = resolve(sha)
    if not c:
        return "round %d: commit %s does not resolve" % (n, sha)
    if not is_ancestor(c, head):
        return "round %d: commit %s is not in the current history (HEAD)" % (n, c[:12])
    if last_code:
        if not is_ancestor(last_code, c):
            return ("round %d is stale: it %s %s, but the task's code changed later in %s. "
                    "Check the current revision as a new round" % (n, what, c[:12], last_code[:12]))
    elif c != head and not records_only_since(c):
        return ("round %d %s %s, not HEAD %s, and no commit names %s to tell whether the "
                "code changed since. Check HEAD, or name the task id in its commits"
                % (n, what, c[:12], head[:12], tid))
    return ""

def acceptance_check():
    """Entering DONE: the latest acceptance verdict is PASS, written by task-validator
    through this gate, for the task's current code."""
    if sa != "DONE":
        return
    vrounds = task.validation_rounds(root, tid)
    how = ("Spawn a fresh %s: it checks every Acceptance item against evidence and writes "
           "docs/tasks/%s.validation/<n>.md - with `deploys: false` and `surface: none` that "
           "is its whole job, no verify record needed" % (VALIDATOR, tid))
    written = [r.get("round") for r in io.read_jsonl(validation_ledger(tid))]
    if not vrounds:
        deny("%s cannot become DONE without an acceptance verdict PASS (no validation "
             "record). %s." % (tid, how))
    n, rec = vrounds[-1]
    later = [x for x in written if isinstance(x, int) and x > n]
    if later:
        deny("validation round %d of %s was written but its file is gone; records are "
             "append-only. %s." % (max(later), tid, how))
    if w(rec, "verdict") != "PASS":
        deny("%s cannot become DONE: latest acceptance verdict (round %d) is %s. %s."
             % (tid, n, w(rec, "verdict") or "missing", how))
    entry = None
    for r in io.read_jsonl(validation_ledger(tid)):
        if r.get("round") == n:
            entry = r
    if entry is None or (entry.get("verdict"), entry.get("commit")) != ("PASS", w(rec, "commit")):
        deny("validation round %d of %s was not written by %s through this gate, or was "
             "changed after it was written (ledger .teamwright/sessions/%s.validation.jsonl). "
             "%s." % (n, tid, VALIDATOR, tid, how))
    why = stale(n, w(rec, "commit"), "accepted")
    if why:
        deny("%s cannot become DONE: acceptance %s." % (tid, why))

surface = w(ra, "surface")
deploys_false = w(ra, "deploys").lower() == "false"
if surface not in SURFACES:
    deny("%s has no valid `surface:` (ui | api | both | none); the gate cannot tell "
         "which verification it needs. Set it in docs/tasks/%s.md." % (tid, tid))
if surface == "none" and deploys_false:
    acceptance_check()
    sys.exit(0)
rounds = task.verify_rounds(root, tid)
how = ("run the system and record the result in docs/tasks/%s.verify/<n>.md "
       "(template docs/tasks/_verify-template.md)" % tid)
if not rounds:
    deny("%s cannot become %s - no verify record. %s. Exit only for pure internal/docs "
         "work: `surface: none` with `deploys: false`." % (tid, sa, how))
last_n, last = rounds[-1]
if w(last, "result") != "PASS":
    deny("latest verify record of %s (round %d, kind %s) is %s. The task goes to "
         "VERIFY_FAIL and back to ANALYSIS - a failed verify means the analysis was "
         "wrong; do not re-patch." % (tid, last_n, w(last, "kind") or "?",
                                      w(last, "result") or "not PASS"))
exc = exception_of(ra)
devs = dev_keys(tid)
impl = (ra.get("implemented_by") or "").strip()

def check_record(n, rec):
    """'' if the record is a valid, genuine, fresh PASS; otherwise the reason."""
    probs = field_problems(rec, tid, n)
    if probs:
        return "round %d is incomplete: %s" % (n, "; ".join(probs))
    if w(rec, "result") != "PASS":
        return "round %d is %s" % (n, w(rec, "result"))
    entry = ledger_entry(tid, n)
    if is_human(rec.get("verifier")):
        if entry is not None:
            return "round %d says `verifier: human` but was written by an agent" % n
    elif entry is None:
        return ("round %d was not written through the gate (no entry in "
                ".teamwright/sessions/%s.verify.jsonl); a person's record uses "
                "`verifier: human:<name>`" % (n, tid))
    elif (entry.get("kind"), entry.get("result"), entry.get("commit")) != (
            w(rec, "kind"), w(rec, "result"), w(rec, "commit")):
        return ("round %d was changed after it was written (ledger: %s/%s/%s); records are "
                "append-only - run again as a new round"
                % (n, entry.get("kind"), entry.get("result"), entry.get("commit")))
    return stale(n, w(rec, "commit"), "verified")

def author_is_impl(n, rec):
    if is_human(rec.get("verifier")):
        return False
    entry = ledger_entry(tid, n) or {}
    return io.actor_key(entry) in devs or (impl and rec.get("verifier", "").strip() == impl)

manual_used = []
for kinds in REQUIRED[surface]:
    allowed = kinds + (("manual",) if exc else ())
    cands = [(n, r) for n, r in rounds if w(r, "kind") in allowed]
    if not cands:
        if any(w(r, "kind") == "manual" for _, r in rounds) and not exc:
            deny("%s has only `kind: manual` verification but no `verify_exception:` in "
                 "the task. Manual checks count only as a recorded, reviewed exception." % tid)
        need = " or ".join(kinds)
        extra = (" (surface: ui needs a ui record with `tool:` - playwright for web, a "
                 "mobile integration/e2e driver for apps)" if "ui" in kinds and len(kinds) == 1 else "")
        deny("%s (surface: %s) needs a PASS verify record of kind %s%s. %s."
             % (tid, surface, need, extra, how))
    n, rec = cands[-1]
    why = check_record(n, rec)
    if why:
        deny("%s cannot become %s: latest %s verify record - %s." % (tid, sa, "/".join(kinds), why))
    if w(rec, "kind") == "manual":
        manual_used.append((n, rec))

if manual_used:
    ok = [(n, r) for n, r in rounds if w(r, "kind") == "manual" and not check_record(n, r)
          and not author_is_impl(n, r)]
    if not ok:
        deny("%s relies on manual verification (exception: %r), but every fresh manual PASS "
             "was written by an agent that implemented the task. A manual check needs a "
             "second pair of eyes: another agent (not the author) or a person "
             "(`verifier: human:<name>`) records it as a new round." % (tid, exc))
acceptance_check()
PY
python3 -c "$_tw_py"
exit 0
