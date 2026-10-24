#!/usr/bin/env bash
# session-check.sh — tell a fresh clone what this machine is missing. Tier T2.
#
# WHY. The process lives in two places: the repository (gates, roles, docs, config) and
# the machine (the teamwright plugin, core.hooksPath). A clone brings only the first:
# /teamwright:flow is unknown, the git hooks are off, and nothing says why.
#
# Claude Code SessionStart hook, never blocks. Checks:
#   - core.hooksPath is scripts/hooks (local git config, not cloned);
#   - the plugin recorded in .teamwright/installed.json ("@kit") is installed for this
#     project, at that version — read from Claude Code's plugin registry
#     ($CLAUDE_CONFIG_DIR or ~/.claude, plugins/installed_plugins.json). A registry in a
#     format it does not know is skipped, not reported.
#   A plugin loaded from a directory rather than the registry (`claude --plugin-dir`, the
#   Agent SDK's local plugin in an autonomous run) is not in the registry: set
#   TEAMWRIGHT_PLUGIN_DIR to that directory and the check takes the plugin from there -
#   and its version (plugin.json `version`, else the directory's git commit, 12 hex) is
#   compared with the recorded one the same way.
# Prints JSON (systemMessage for the owner, additionalContext for the agent), or nothing
# when all is well. TEAMWRIGHT_SESSION_CHECK=0 turns it off.
set -uo pipefail
cat >/dev/null 2>&1 || true   # hook input is not needed
[ "${TEAMWRIGHT_SESSION_CHECK:-1}" = "0" ] && exit 0
ROOT="${CLAUDE_PROJECT_DIR:-$(pwd)}"
command -v python3 >/dev/null 2>&1 || exit 0
HOOKS="-"
if git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1; then
  HOOKS="$(git -C "$ROOT" config --get core.hooksPath 2>/dev/null || true)"
fi
exec python3 - "$ROOT" "$HOOKS" <<'PY'
import json, os, sys

root, hooks = sys.argv[1], sys.argv[2]


def load(path):
    try:
        with open(path) as f:
            d = json.load(f)
    except (OSError, ValueError):
        return None
    return d if isinstance(d, dict) else None


def ver(v):
    try:
        return tuple(int(x) for x in str(v).split("-")[0].split("."))
    except ValueError:
        return None


manifest = load(os.path.join(root, ".teamwright", "installed.json"))
if manifest is None:
    sys.exit(0)
kit = manifest.get("@kit") if isinstance(manifest.get("@kit"), dict) else {}
pid = kit.get("plugin") or "teamwright@teamwright"
want = kit.get("version") or ""
mname = pid.split("@", 1)[-1]
issues = []

if hooks != "-" and hooks != "scripts/hooks":
    issues.append("git hooks are off on this clone (core.hooksPath is local git config): "
                  "run `bash scripts/install-hooks.sh`.")

settings = load(os.path.join(root, ".claude", "settings.json")) or {}
src = ((settings.get("extraKnownMarketplaces") or {}).get(mname) or {}).get("source") or {}
if src.get("source") == "github":
    add = "/plugin marketplace add %s" % src.get("repo")
    if src.get("ref"):
        add += " (branch %s: or clone it on that branch and add the local path)" % src["ref"]
elif src.get("url"):
    add = "/plugin marketplace add %s" % src["url"]
else:
    add = "/plugin marketplace add <teamwright repository or local checkout>"

def drift(have, want, where):
    """A message when the plugin's version differs from the one the project was set up with
    ('' when it matches or cannot be told). Release versions compare by number; a channel
    build has no version - its version is the 12-hex commit - and only equality is known."""
    if not want or not have or have == "?":
        return ""
    if ver(have) and ver(want):
        if ver(have) < ver(want):
            return ("the %s plugin %s is %s, older than the %s this project was set up with: "
                    "`/plugin marketplace update %s`, then `/plugin update %s` (or uninstall and install), "
                    "then restart." % (pid, where, have, want, mname, pid))
        if ver(have) > ver(want):
            return ("the %s plugin %s is %s, newer than the project files (%s): run "
                    "`/teamwright:reconfigure` to update them." % (pid, where, have, want))
        return ""
    if have[:12] != want[:12]:
        return ("the %s plugin %s is %s, the project was set up with %s: run `/teamwright:reconfigure` "
                "when the plugin is the newer one, or update the plugin to that version." % (pid, where, have, want))
    return ""


def dir_version(d, manifest):
    """A plugin directory's version: plugin.json `version`, else the 12-hex commit of the
    directory's git checkout (what Claude Code takes for a channel build); '' when neither."""
    if manifest.get("version"):
        return str(manifest["version"])
    import subprocess
    try:
        top = subprocess.run(["git", "-C", d, "rev-parse", "--show-toplevel"], capture_output=True, text=True)
        sha = subprocess.run(["git", "-C", d, "rev-parse", "HEAD"], capture_output=True, text=True)
    except OSError:
        return ""
    if top.returncode == 0 and os.path.realpath(top.stdout.strip()) == os.path.realpath(d) and sha.returncode == 0:
        return sha.stdout.strip()[:12]
    return ""


local_dir = os.environ.get("TEAMWRIGHT_PLUGIN_DIR") or ""
local_manifest = load(os.path.join(local_dir, ".claude-plugin", "plugin.json")) if local_dir else None
cdir = os.environ.get("CLAUDE_CONFIG_DIR") or os.path.join(os.path.expanduser("~"), ".claude")
reg_path = os.path.join(cdir, "plugins", "installed_plugins.json")
reg = load(reg_path) if os.path.exists(reg_path) else {"plugins": {}}
plugins = reg.get("plugins") if reg else None
if local_manifest is not None and local_manifest.get("name") == pid.split("@", 1)[0]:
    plugins = None  # loaded from a directory for this session: nothing to install, but its version counts
    msg = drift(dir_version(local_dir, local_manifest), want, "loaded from TEAMWRIGHT_PLUGIN_DIR (%s)" % local_dir)
    if msg:
        issues.append(msg)
if isinstance(plugins, dict):
    real = os.path.realpath(root)
    have = None
    for e in plugins.get(pid) or []:
        if not isinstance(e, dict):
            continue
        scope = e.get("scope")
        if scope in ("project", "local") and os.path.realpath(e.get("projectPath") or "") != real:
            continue
        have = e.get("version") or "?"
        break
    need = (" %s" % want) if want else ""
    if have is None:
        issues.append(
            "the %s plugin%s is not installed for this project on this machine, so /teamwright:* "
            "commands are unavailable. Restart Claude Code here and accept the marketplace and "
            "plugin offer, or in a terminal session: `%s`, then `/plugin install %s`, then restart."
            % (pid, need, add, pid))
    elif drift(have, want, "here"):
        issues.append(drift(have, want, "here"))

if issues:
    text = "teamwright: " + " ".join(issues)
    print(json.dumps({"systemMessage": text,
                      "hookSpecificOutput": {"hookEventName": "SessionStart",
                                             "additionalContext": text + " Tell the owner before starting work."}}))
PY
