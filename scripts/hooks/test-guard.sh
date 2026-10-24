#!/usr/bin/env bash
# test-guard.sh — Claude Code PreToolUse gate (Bash). Tier T3.
#
# WHY. In one project's autonomous run the test-writer ran the same 4-minute suite four
# times in a row — twice with `| grep … | tail` (the failures were cut off), once with a
# flag that removed a fixture (25 false errors), once right, with one worker instead of
# the configured ones — and the developer ran the gate script directly twice. The role
# files already said "never cut output, run long commands once with output to a file";
# agents ignored it. Rules in prose do not hold under pressure, so the tooling refuses.
#
# REFUSES a Bash command that
#   1. runs tests or a gate directly instead of through scripts/tw-test, scripts/tw-gate or
#      scripts/tw-mutants: pytest, python -m pytest|unittest, make test|tests|check|test-*,
#      flutter|dart|deno|bun|go test, cargo test|nextest, npm|pnpm|yarn [run] test,
#      vitest, jest, phpunit, pest, php artisan test, gradle(w) test|check, mvn test —
#      also behind uv/poetry/npx/... run, sh -c, eval, docker run|exec, and inside
#      `scripts/tw-run <area> -- <cmd>`; plus every command the config names as an area's
#      gate_command, test_command or test_select_command, the top-level gate_command
#      (a repository script such as tool/gate.sh by its name, `make check`-style commands
#      by their first words), and the regular expressions in `test_runner_patterns`;
#   2. pipes test or gate output — the wrappers' included — into grep, tail, head, sed,
#      awk, cut, wc, sort, less (the "cut output" pattern).
# The refusal names the command to run instead (scripts/tw-test <area> <tests>, or
# scripts/tw-gate <area> for a gate) and says to read the log it prints.
#
# ALLOWED: scripts/tw-test, scripts/tw-gate, scripts/tw-mutants themselves (their own runs
# go through scripts/tw-run inside the script, never through the Bash tool, so this hook
# does not see them); `scripts/tw-run <area> -- <cmd>` for anything that is not a test
# run (lint, build, codegen) and for the area's configured lint_command,
# generate_command, api_test_command and e2e_ui_command (verification against a running
# service); reading logs (grep, sed -n on a file).
#
# MODE — its own switch, not the project's gate mode: gate-first only warns, and the
# owner wants this one to stop agents. `process.test_guard: block | warn | off` in
# .teamwright/config.yml (default block); $TW_TEST_GUARD (block | warn | off), set by the
# person who starts Claude Code, wins. Agents cannot change either (the state guard
# protects config.yml; an assignment inside the command does not reach this hook).
#
# SAFETY: fail-open on any internal error; silent on allow.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export TEAMWRIGHT_HOOKS_DIR="$HERE"
command -v python3 >/dev/null 2>&1 || exit 0

IFS= read -r -d '' _tw_py <<'PY' || true
import os, re, shlex, sys
sys.dont_write_bytecode = True
sys.path.insert(0, os.environ["TEAMWRIGHT_HOOKS_DIR"])
try:
    import _hookio as io, _config
except Exception:
    sys.exit(0)

data = io.load_input()
if not data or io.event(data) != "PreToolUse" or (data.get("tool_name") or "") != "Bash":
    sys.exit(0)
cmd = (data.get("tool_input") or {}).get("command") or ""
if not cmd.strip():
    sys.exit(0)
root = io.project_root(data)
try:
    cfg = _config.parse_yaml(open(os.path.join(root, _config.CONFIG), encoding="utf-8").read())
    cfg = cfg if isinstance(cfg, dict) else {}
except Exception:
    cfg = {}
mode = (os.environ.get("TW_TEST_GUARD") or "").strip().lower()
if mode not in _config.TEST_GUARD:
    mode = _config.process_of(cfg).get("test_guard", "block")
if mode == "off":
    sys.exit(0)

areas = [a for a in cfg.get("areas") or [] if isinstance(a, dict) and a.get("name")]
WRAPPERS = ("tw-test", "tw-gate", "tw-mutants")
SHELLS = ("sh", "bash", "zsh", "dash", "ksh")
RUNNERS = ("docker", "podman", "nerdctl", "kubectl", "ssh")
FILTERS = ("grep", "egrep", "fgrep", "rg", "ag", "tail", "head", "sed", "awk", "gawk", "mawk", "nawk",
           "cut", "wc", "less", "more", "sort", "uniq")
PREFIX = ("sudo", "doas", "command", "builtin", "exec", "nohup", "time", "nice", "ionice", "stdbuf",
          "env", "timeout", "xargs", "unbuffer", "caffeinate")
ASSIGN = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")
VALUE_OPTS = ("--with", "--group", "--extra", "-p", "--python", "--env-file", "--package", "--project",
              "--directory", "--filter", "-F", "--prefix", "-C", "--dir", "-w", "--workspace", "--cwd",
              "-f", "--file", "-j", "--jobs", "-E", "--env", "-e", "-u", "--user", "--name", "--entrypoint",
              "-v", "--volume", "--network", "--workdir", "--platform", "-c", "--config")

def base(w):
    return os.path.basename(w.rstrip("/"))

def positional(args):
    out, skip = [], False
    for a in args:
        if skip:
            skip = False
            continue
        if a.startswith("-"):
            skip = a in VALUE_OPTS
            continue
        out.append(a)
    return out

def strip(words):
    w = list(words)
    while w:
        if ASSIGN.match(w[0]):
            w.pop(0)
            continue
        if base(w[0]) in PREFIX:
            w.pop(0)
            while w and (w[0].startswith("-") or re.fullmatch(r"[0-9.]+[smhd]?", w[0]) or ASSIGN.match(w[0])):
                w.pop(0)
            continue
        break
    while w:   # tool runners: uv run, poetry run, npx, pnpm exec, bundle exec ...
        b = base(w[0])
        if b in ("uv", "poetry", "pipenv", "pdm", "hatch", "rye") and len(w) > 1 and w[1] == "run":
            w = w[2:]
        elif b in ("npx", "pnpx", "bunx"):
            w = w[1:]
        elif b in ("npm", "pnpm", "yarn", "bundle") and len(w) > 1 and w[1] in ("exec", "dlx", "x"):
            w = w[2:]
        else:
            break
        while w and (w[0].startswith("-")):
            w = w[2:] if w[0] in VALUE_OPTS else w[1:]
    if len(w) > 1 and base(w[0]) in SHELLS and not w[1].startswith("-"):
        w = w[1:]   # `bash tool/gate.sh` runs tool/gate.sh
    return w

def builtin(w):
    """Label of a direct test run, or None."""
    b, a = base(w[0]), w[1:]
    pos = [p for p in positional(a) if not p.startswith("+")]
    first = pos[0] if pos else ""
    if b in ("pytest", "py.test", "vitest", "jest", "phpunit", "pest"):
        return b
    if re.fullmatch(r"python[0-9.]*", b) and "-m" in a:
        i = a.index("-m")
        if i + 1 < len(a) and a[i + 1] in ("pytest", "unittest"):
            return "python -m " + a[i + 1]
    if b in ("make", "gmake") and any(re.fullmatch(r"(test|tests|check)|test[-_:].*", p) for p in pos if "=" not in p):
        return "make " + next(p for p in pos if re.fullmatch(r"(test|tests|check)|test[-_:].*", p))
    if b in ("flutter", "dart", "deno", "bun", "go") and first == "test":
        return b + " test"
    if b == "cargo" and first in ("test", "nextest"):
        return "cargo " + first
    if b in ("npm", "pnpm", "yarn", "bun"):
        if re.fullmatch(r"t|tst|test([:_-].*)?", first):
            return "%s %s" % (b, first)
        if first in ("run", "run-script") and len(pos) > 1 and re.fullmatch(r"test([:_-].*)?", pos[1]):
            return "%s run %s" % (b, pos[1])
    if b in ("gradlew", "gradle") and any(re.search(r"(^|:)(test|check)$", p) for p in pos):
        return b + " test"
    if b in ("mvn", "mvnw") and any(p in ("test", "surefire:test") for p in pos):
        return "mvn test"
    if b == "php" and pos[:2] == ["artisan", "test"]:
        return "php artisan test"
    return None

def words_of(text):
    """Simple commands of a shell string, grouped in pipelines: [[(words, cd), ...], ...]."""
    text, _ = io._split_heredocs(text)
    lex = shlex.shlex(text, posix=True, punctuation_chars=";&|<>()\n")
    lex.whitespace = " \t\r"
    lex.whitespace_split = True
    lex.commenters = ""
    pipes, cur, cmd, redirect = [], [], [], False
    for tok in lex:
        if tok and set(tok) <= set(";&|<>()\n"):
            if ">" in tok or set(tok) <= set("<"):
                redirect = True   # the next word is the redirect's file (or a heredoc marker)
                continue
            t = tok.replace("\n", "")
            if cmd:
                cur.append(cmd)
            cmd = []
            if t in ("|", "|&"):
                continue
            if cur:
                pipes.append(cur)
            cur = []
            continue
        if redirect:
            redirect = False
            continue
        if re.fullmatch(r"\d", tok):
            continue   # the fd of 2>&1
        cmd.append(tok)
    if cmd:
        cur.append(cmd)
    if cur:
        pipes.append(cur)
    return pipes

def config_matchers():
    out = []   # (kind, value, label, area, what)
    srcs = [(a["name"], k, str(a.get(k) or "").replace("{tests}", ""))
            for a in areas for k in ("gate_command", "test_command", "test_select_command")]
    srcs.append((None, "gate_command", str(cfg.get("gate_command") or "")))
    for area, key, c in srcs:
        if not c.strip():
            continue
        wd = next((str(a.get("workdir") or "") for a in areas if a["name"] == area), "")
        try:
            pipes = words_of(c)
        except ValueError:
            continue
        for pipe in pipes:
            for w in pipe:
                w = strip(w)
                if not w or base(w[0]) in ("cd", "pushd", "export", "set", "true", "echo", ":", "test", "[") \
                        or base(w[0]) in WRAPPERS or base(w[0]) == "tw-run":
                    continue
                what = "gate" if key == "gate_command" else "tests"
                label = "%s (the %s of %s)" % (" ".join(w[:3]), key, ("area " + area) if area else "project")
                p = w[0]
                if "/" in p or any(os.path.isfile(os.path.join(root, d, p)) for d in ("", wd)):
                    # a script that only wraps another command (`tool/gate.sh sh -c '...'`, a docker
                    # wrapper) is no signature: the project runs lint, codegen and migrations through it
                    # too. Its inner command is checked instead (pytest inside is still refused).
                    if any(base(x) in SHELLS and y == "-c" for x, y in zip(w[1:], w[2:])):
                        continue
                    out.append(("file", base(p), label, area, what))
                elif p not in SHELLS and not re.fullmatch(r"python[0-9.]*|node|ruby|perl|php", p):
                    pre = [p]
                    for x in w[1:3]:
                        if x.startswith("-") or not re.fullmatch(r"[A-Za-z0-9_.:+-]+", x):
                            break
                        pre.append(x)
                    if len(pre) >= 2:
                        out.append(("prefix", pre, label, area, what))
    return out

MATCHERS = config_matchers()
PATTERNS = []
for p in cfg.get("test_runner_patterns") or [] if isinstance(cfg.get("test_runner_patterns"), list) else []:
    try:
        PATTERNS.append(re.compile(str(p)))
    except re.error:
        pass

def allowed_in_run(area, code):
    """The area's own non-test commands through tw-run: lint, codegen, API and UI verification."""
    a = next((x for x in areas if x["name"] == area), None)
    if a is None:
        return False
    norm = " ".join(code.split())
    for k in ("lint_command", "generate_command", "api_test_command", "e2e_ui_command"):
        c = " ".join(str(a.get(k) or "").split())
        if c and (norm == c or norm.startswith(c + " ")):
            return True
    return False

findings = []   # dicts: kind direct|cut, label, words, area, cd, what

def classify(words, cd, depth):
    """'test' | 'wrapper' | None; nested direct runs are added to findings."""
    w = strip(words)
    if not w:
        return None
    b = base(w[0])
    if b in WRAPPERS:
        return "wrapper"
    if b == "tw-run":
        area = w[1] if len(w) > 1 else ""
        rest = w[w.index("--") + 1:] if "--" in w else w[2:]
        if not rest or depth > 3 or allowed_in_run(area, " ".join(rest)):
            return None
        code = rest[0] + "".join(" " + shlex.quote(x) for x in rest[1:])
        n = len(findings)
        check(code, depth + 1, area)
        hit = [f for f in findings[n:] if f["kind"] == "direct"]
        for f in hit:
            f["area"] = f["area"] or area
            f["via"] = True
        return "test" if hit else None
    if (b in SHELLS and "-c" in w) or b == "eval":
        code = (w[w.index("-c") + 1] if w.index("-c") + 1 < len(w) else "") if b != "eval" else " ".join(w[1:])
        n = len(findings)
        if depth <= 3:
            check(code, depth + 1, None, cd)
        return "test" if any(f["kind"] == "direct" for f in findings[n:]) else None
    label, area, what = None, None, "tests"
    for kind, val, lab, ar, wh in MATCHERS:   # the config's own commands first: they name the area
        if (kind == "file" and base(w[0]) == val) or (kind == "prefix" and w[:len(val)] == val):
            label, area, what = lab, ar, wh
            break
    if not label:
        label = builtin(w)
    if not label:
        text = " ".join(w)
        for p in PATTERNS:
            if p.search(text):
                label = "%s (test_runner_patterns: %s)" % (" ".join(w[:3]), p.pattern)
                break
    # a command that runs another one: `tool/gate.sh sh -c '...'`, `docker run img pytest`
    n = len(findings)
    for j in range(1, len(w) - 2):
        if base(w[j]) in SHELLS and w[j + 1] == "-c" and depth <= 3:
            check(w[j + 2], depth + 1, None, cd)
    if label:
        findings.insert(n, {"kind": "direct", "label": label, "words": w, "area": area, "cd": cd, "what": what})
        return "test"
    if any(f["kind"] == "direct" for f in findings[n:]):
        return "test"
    if b in RUNNERS or "/" in w[0]:
        for j in range(1, len(w)):
            sub = strip(w[j:])
            if sub and base(sub[0]) not in WRAPPERS and builtin(sub):
                findings.append({"kind": "direct", "label": builtin(sub), "words": sub, "area": None, "cd": cd,
                                 "what": "tests"})
                return "test"
    return None

def check(text, depth=0, area=None, cd=""):
    for pipe in words_of(text):
        kinds = []
        for w in pipe:
            s = strip(w)
            if s and base(s[0]) in ("cd", "pushd") and len(s) > 1:
                cd = s[1]
            kinds.append(classify(w, cd, depth))
        for i, k in enumerate(kinds):
            if not k:
                continue
            cut = next((strip(x)[0] for x in pipe[i + 1:] if strip(x) and base(strip(x)[0]) in FILTERS), None)
            if cut:
                findings.append({"kind": "cut", "filter": base(cut), "words": strip(pipe[i]), "cd": cd,
                                 "area": area})
                break

try:
    check(cmd)
except Exception:
    sys.exit(0)
if not findings:
    sys.exit(0)

def guess_area(f, sels):
    if f.get("area"):
        return f["area"]
    cd = os.path.normpath(f.get("cd") or ".").strip("./")
    for a in areas:
        if cd and os.path.normpath(str(a.get("workdir") or ".")).strip("./") == cd:
            return a["name"]
    for s in sels:
        p = os.path.normpath(os.path.join(cd, s.split("::")[0]))
        for a in areas:
            dirs = [str(x) for k in ("source_dirs", "test_dirs") for x in
                    (a.get(k) if isinstance(a.get(k), list) else [a.get(k)] if a.get(k) else [])]
            dirs += [str(a["workdir"])] if a.get("workdir") else []
            if any(p == d.strip("/") or p.startswith(d.strip("/") + "/") for d in dirs):
                return a["name"]
    tested = [a["name"] for a in areas if a.get("test_command") or a.get("test_select_command")]
    return tested[0] if tested else (areas[0]["name"] if areas else "<area>")

def selectors(f, area):
    w, out = f.get("words") or [], []
    args = w[1:]
    if re.fullmatch(r"python[0-9.]*", base(w[0]) if w else "") and "-m" in args:
        args = args[args.index("-m") + 2:]
    i = 0
    while i < len(args):
        x = args[i]
        if x in ("-k", "-m", "--name", "-t", "--testNamePattern", "--plain-name", "--run") and i + 1 < len(args):
            out += [x, args[i + 1]]
            i += 2
            continue
        if not x.startswith("-") and ("/" in x or "::" in x or re.search(
                r"\.(py|dart|rs|go|ts|tsx|js|jsx|mjs|cjs|php|rb|java|kt|swift)$", x)):
            out.append(x)
        i += 1
    a = next((x for x in areas if x["name"] == area), None)
    wd = str(a.get("workdir") or "").strip("/") if a else ""
    cd = os.path.normpath(f.get("cd") or ".").strip("./")
    if wd and not cd:
        out = [x[len(wd) + 1:] if x.startswith(wd + "/") else x for x in out]
    return out

def tw_test(f):
    area = guess_area(f, [x for x in (f.get("words") or [])[1:] if not x.startswith("-")])
    if f.get("what") == "gate":
        return "scripts/tw-gate %s" % (f["area"] if f.get("area") else "--top")
    return " ".join(["scripts/tw-test", area] + [shlex.quote(x) for x in selectors(f, area)])

direct = [f for f in findings if f["kind"] == "direct"]
cuts = [f for f in findings if f["kind"] == "cut"]
excerpt = " ".join(cmd.split())
excerpt = excerpt if len(excerpt) <= 160 else excerpt[:157] + "..."
TAIL = ("Full gate: `scripts/tw-gate <area>`; mutants: `scripts/tw-mutants <area> <ID> --tree <copy>`; "
        "lint, build, codegen: `scripts/tw-run <area> -- <cmd>`. See docs/process/gates.md, Test guard.")
if direct:
    f = direct[0]
    instead = tw_test(f)
    gate = f.get("what") == "gate"
    reason = ("TEST GUARD: `%s` runs %s directly (%s). Run instead:\n  %s\n%s" % (
        f["label"], "a gate" if gate else "tests", excerpt, instead,
        ("It runs the gate once per state of the code, keeps the whole output in a log and prints "
         "PASS/FAIL and the log path; a PASS on the same files is reused." if gate else
         "It runs the area's configured test command (its workers, its runner) once, writes the whole "
         "output to a log and prints a short summary - counts, the failed tests with their first error "
         "line, the log path. Read that log (grep or sed -n on the file) instead of running again: the "
         "same call answers from its cache until the code changes. Selectors are test files, node ids "
         "or filters, relative to the area's workdir.")))
    if cuts:
        reason += " Never pipe test output into %s." % cuts[0]["filter"]
    reason += " " + TAIL
else:
    f = cuts[0]
    w = f["words"]
    b = base(w[0]) if w else ""
    if b in WRAPPERS:
        instead = " ".join(shlex.quote(x) if not re.fullmatch(r"[A-Za-z0-9_./:@=+-]+", x) else x for x in w)
    else:
        instead = tw_test(f)
    reason = ("TEST GUARD: do not cut test or gate output (`| %s`): when something fails the cut part is "
              "what was needed, and the run gets repeated. tw-test and tw-gate already print a short summary "
              "and keep the whole output in a log. Run without the pipe:\n  %s\nand read the log it prints "
              "(grep or sed -n on that file is fine). %s" % (f["filter"], instead, TAIL))
io.deny(reason, "test-guard", data, mode="warn" if mode == "warn" else "enforce",
        warn_note="teamwright test guard (warn only: process.test_guard is warn): ")
PY
python3 -c "$_tw_py"
exit 0
