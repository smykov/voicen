#!/usr/bin/env bash
# wip-push-guard.sh - Claude Code PreToolUse gate (Bash). T-192, T-198, decisions #219.
#
# WHY. A rule `Bash(git push *)` does not match the git-option forms of a push, and an option such as
# `core.hooksPath` also drops the pre-push hook. A settings deny on these forms is no boundary (a pipe
# into `sh` escapes it), so this hook looks at the command text.
#
# RULE (P-020, token; `analysis.decides_on` of T-198). One regex over the raw command text, any
# occurrence refuses: the word git, blanks or a backslash-newline, a dash, then on the same logical
# line the word push. No quote, separator, wrapper or heredoc is read, so a wrapper never named needs
# no clause, and text that only mentions the token is refused as well (a commit message, an echo, a
# heredoc body, a grep, `git -C . stash push`): split the call, or use `git commit -F` or the Write tool.
#
# PASSES: scripts/tw-wip-push, a plain `git push` (the settings denies and the pre-push hook cover
# it), the word inside another (`--push`, `pre-push`, `docs/push.md`, `pushed`), and a real newline
# after git or before push.
#
# NOT SEEN, what the rule gives up (docs/process/gates.md, WIP push guard), a class: any spelling whose
# text does not hold the token literally, because a quote, a backslash or an expansion sits in or
# before the word git, the dash or the word push, or between them in place of a blank: `"git" -C . push`,
# `git "-C" . push`, `git \-C . push`, `git -C . pu""sh`, `git${IFS}-C . push`, `X=push; git -C . $X`,
# `G=git; $G -C . push`. Worst case: `git "-c" core.hooksPath=/dev/null push`, which also drops the
# pre-push hook. The previous shell model refused the quote and backslash spellings; refusing them
# takes a reader of quotes. Also not seen: `git -C . send-pack`, a script file, a program that passes
# git its arguments apart.
#
# A refusal blocks in every gate mode. SAFETY: fail-open on an internal error; silent on allow.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export TEAMWRIGHT_HOOKS_DIR="$HERE"
command -v python3 >/dev/null 2>&1 || exit 0

IFS= read -r -d '' _tw_py <<'PY' || true
import os, re, sys
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

if re.compile(r"\bgit(?:[ \t]|\\\n)+-(?:[^\n]|\\\n)*?(?<![\w/-])push(?![\w-])").search(cmd):
    io.deny("WIP PUSH GUARD: this command text holds the word git, a dash, then the word push on one "
            "logical line: the shape of `git push` with a git option before the subcommand, which the "
            "push rules do not match and which can drop the pre-push hook. Quotes and wrappers are not "
            "read, so a mention counts too: split the call, or use `git commit -F` or the Write tool. "
            "The CI-only wip push goes through `scripts/tw-wip-push [<commit>]` from the task's worktree "
            "(branch tw/<ID>), or the plain `git push origin HEAD:wip/<ID>` from the main-tree "
            "orchestrator. See docs/decisions.md #219, #202.",
            "wip-push-guard", data, hard=True)
PY
python3 -c "$_tw_py"
exit 0
