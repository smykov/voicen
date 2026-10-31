#!/usr/bin/env bash
# wip-push-guard.sh - Claude Code PreToolUse gate (Bash). T-192, decisions #219.
#
# WHY. A rule `Bash(git push *)` does not match `git -C . push origin main`, and `git -c
# core.hooksPath=/dev/null push ...` also drops the pre-push hook. A settings deny on these forms is
# no boundary (`sh -c` escapes it), so this hook reads the command.
#
# REFUSES a Bash command in which git runs `push` with any git option before the subcommand: `-C`,
# `-c`, `--git-dir` and the rest, also behind a prefix (env, an assignment), in a chain, a subshell,
# a command substitution (quoted or not; single quotes and a backslash make it plain text), `sh -c`,
# `eval`, or a script a shell reads from a heredoc. A plain `git push` is not this hook's business:
# the settings denies and the pre-push hook cover it.
#
# ALLOWED: scripts/tw-wip-push, and git with the same options doing something else
# (`git -C . stash push`, `git -C . commit -m "do not push"`).
#
# NOT SEEN (best effort, as in docs/process/gates.md, Known limitations): a command built at run time
# (`$X push`), a script file that pushes, `git -c alias.p=push p`, `find -exec`.
#
# A refusal blocks in every gate mode. SAFETY: fail-open on an internal error; silent on allow.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export TEAMWRIGHT_HOOKS_DIR="$HERE"
command -v python3 >/dev/null 2>&1 || exit 0

IFS= read -r -d '' _tw_py <<'PY' || true
import os, re, shlex, sys
sys.dont_write_bytecode = True
sys.path.insert(0, os.environ["TEAMWRIGHT_HOOKS_DIR"])
try:
    import _hookio as io
except Exception:
    sys.exit(0)

data = io.load_input()
if not data or io.event(data) != "PreToolUse" or (data.get("tool_name") or "") != "Bash":
    sys.exit(0)
cmd = (data.get("tool_input") or {}).get("command") or ""
if not cmd.strip():
    sys.exit(0)

SHELLS = ("sh", "bash", "zsh", "dash", "ksh")
PREFIX = ("sudo", "doas", "command", "builtin", "exec", "nohup", "time", "nice", "ionice", "stdbuf",
          "env", "timeout", "xargs", "unbuffer")
KEYWORDS = ("{", "!", "if", "then", "elif", "else", "while", "until", "do")
ASSIGN = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")
# git options that take the next word as their value
GIT_VALUE = ("-C", "-c", "--git-dir", "--work-tree", "--namespace", "--config-env", "--super-prefix",
             "--attr-source")
SUBST = re.compile(r"\$\(([^)]*)\)|`([^`]*)`")
SEPARATORS = set(";&|<>()\n")
# Unparseable text (unbalanced quotes): a git, options, then push.
FALLBACK = re.compile(r"\bgit(\s+-\S+(\s+[^\s-]\S*)?)+\s+push\b")


def base(word):
    return os.path.basename(word.rstrip("/"))


def commands(text):
    """Words of each simple command; newlines separate commands, redirect targets are dropped."""
    lex = shlex.shlex(text.replace("\\\n", ""), posix=True, punctuation_chars=";&|<>()\n")
    lex.whitespace = " \t\r"
    lex.whitespace_split = True
    lex.commenters = ""
    out, cur, redirect = [], [], False
    for tok in lex:
        if tok and set(tok) <= SEPARATORS:
            if ">" in tok or set(tok) <= set("<"):
                redirect = True
                continue
            if cur:
                out.append(cur)
            cur = []
            continue
        if redirect:
            redirect = False
            continue
        if re.fullmatch(r"\d", tok):
            continue   # the fd of 2>&1
        cur.append(tok)
    if cur:
        out.append(cur)
    return out


def unwrap(words):
    """The command a simple command really runs: past assignments, keywords and prefix commands."""
    w = list(words)
    while w:
        if ASSIGN.match(w[0]) or w[0] in KEYWORDS:
            w.pop(0)
        elif base(w[0]) in PREFIX:
            # A prefix's options may take values (`env -u X`, `timeout 5`): the next git or shell.
            at = next((i for i, x in enumerate(w) if i and base(x) in SHELLS + ("git", "eval")), None)
            w = w[at:] if at is not None else []
        else:
            break
    return w


def option_before_push(args):
    i, seen = 0, False
    while i < len(args) and args[i].startswith("-"):
        i += 2 if args[i] in GIT_VALUE else 1
        seen = True
    return seen and i < len(args) and args[i] == "push"


def substitution_end(text, i):
    """The index just past the substitution opening at i (a backtick or `$(`), its own quotes read."""
    j, n = i + 1, len(text)
    if text[i] == "`":
        while j < n and text[j] != "`":
            j += 2 if text[j] == "\\" else 1
        return j + 1
    depth, quote = 0, ""
    while j < n:
        c = text[j]
        if c == "\\" and quote != "'":
            j += 2
            continue
        if quote:
            quote = "" if c == quote else quote
        elif c in "'\"":
            quote = c
        elif c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return j + 1
        j += 1
    return n


def live_text(text):
    """The text with what the shell keeps literal blanked: single-quoted strings, backslash escapes.
    A substitution is kept whole: quotes inside it open a context of their own."""
    out, i, dquoted = [], 0, False
    while i < len(text):
        c, end = text[i], i + 1
        if c == "\\":
            end = i + 2
            out.append("  ")
        elif c == "'" and not dquoted:
            close = text.find("'", i + 1)
            end = close + 1 if close >= 0 else len(text)
            out.append(" " * (end - i))
        elif c == "`" or text.startswith("$(", i):
            end = substitution_end(text, i)
            out.append(text[i:end])
        else:
            dquoted = dquoted != (c == '"')
            out.append(c)
        i = end
    return "".join(out)


def refused(text, depth=0):
    if depth > 3:
        return False
    body_free, bodies = io._split_heredocs(text)
    try:
        cmds = commands(body_free)
    except ValueError:
        return bool(FALLBACK.search(body_free))
    # Before word splitting: `commands` cuts an unquoted substitution into several words.
    for m in SUBST.finditer(live_text(body_free)):
        if refused(m.group(1) or m.group(2) or "", depth + 1):
            return True
    for words in cmds:
        w = unwrap(words)
        if not w:
            continue
        prog = base(w[0])
        if prog == "git" and option_before_push(w[1:]):
            return True
        if prog == "eval" and refused(" ".join(w[1:]), depth + 1):
            return True
        if prog in SHELLS:
            at = next((i for i, x in enumerate(w) if i and re.fullmatch(r"-[A-Za-z]*c[A-Za-z]*", x)), None)
            if at is not None:
                if at + 1 < len(w) and refused(w[at + 1], depth + 1):
                    return True
            elif any(refused(b, depth + 1) for _, b in bodies):
                return True
    return False


try:
    hit = refused(cmd)
except Exception:
    sys.exit(0)
if hit:
    io.deny("WIP PUSH GUARD: this command runs `git push` with a git option before the subcommand "
            "(`git -C`, `git -c` ...), which the push rules do not match and which can drop the "
            "pre-push hook. The only push an agent makes is the CI-only one: `scripts/tw-wip-push "
            "[<commit>]` from the task's worktree (branch tw/<ID>). See docs/decisions.md #219.",
            "wip-push-guard", data, hard=True)
PY
python3 -c "$_tw_py"
exit 0
