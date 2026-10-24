# Context tools for roles: Serena language servers and subagent tool allowlists

**Code:** `.claude/agents/**` (front matter of the 9 roles), owned heads `scripts/ci/role-heads/<role>.md`, `scripts/ci/role-tools.sh` (Makefile `check-role-tools`, part of `make check`), host-local `.serena/project.yml` (`language_servers`; untracked, decisions #16) · **Tests that pin it:** `scripts/ci/role-tools.test.sh` (probes A-J and the real tree)

Tasks: T-082 (class `context-tool-coverage`; no `docs/failures.md` entry: first task of the class, a need found by process review 2026-10-09), T-113 (rca of T-082's guard, F-022, class `guard-model`). Decisions: #72 (language servers), #145 (allowlists), #146 (svelte, amends #72), #154 (owned heads, guard trigger).

## Invariants

### Every file under `.claude/agents/` has front matter byte-equal to an owned head, and no other file exists there; the heads grant Serena and context7 read/navigation tools only

- **Defect that produced it:** (1) none filed; process review 2026-10-09: 0 MCP calls from any subagent over 13 sessions. Claude Code gives a subagent with a `tools:` line only the tools named there, and the kit templates (teamwright 0.8.2) render the Tools text but not the allowlist entries it depends on, so every role took its "Serena not connected" fallback silently. (2) F-022 (T-113, review 1 of T-082): the first guard read the YAML front matter itself (first `---` pair, first `tools:` line, comma split, `mcp__` prefix test, only files holding the begin marker) and passed probes A-J with exit 0: A flow list, B duplicate `tools:` key, C continuation line, D quoted element, E inline `mcpServers:` server, F `permissionMode: bypassPermissions`, G `disallowedTools:`, H quoted second `"tools"` key, I anchor and complex key, J begin marker removed.
- **What breaks if you violate it:** roles read Rust, Svelte and specs with grep and whole-file reads (the cost #72 was meant to remove). A Serena edit, memory or shell tool, a private MCP server or bypassed permissions would bypass the Edit/Write gates, which must stay the only write path.
- **Where it is enforced:** `scripts/ci/role-tools.sh` (`make check-role-tools`), `decides_on: literal`, never reading YAML. H1 the set of files under `.claude/agents/` (recursive, every type, dotfiles and symlinks included) equals the set of heads in `scripts/ci/role-heads/`; H2 the first `size(head)` bytes of each file are `cmp`-equal to its head (no CRLF or BOM normalisation); H3 each head starts with `---`, ends with `\n---\n` and has no other `---` line; R4 a role whose head holds the raw token `mcp__` keeps both owned fallback lines (Serena: grep and file reads; context7: same fallback) as full lines inside the Tools markers. The tool names in the heads: 5 Serena navigation tools, the 2 context7 tools, `ToolSearch`. A kit re-render (`/teamwright:reconfigure`) changes only the Tools block, so the heads survive it; a kit upgrade that changes a front matter turns the gate red, which is intended (an owner-visible change of what a role can do). The P-020 trigger that keeps a new guard from skipping this design step is in `docs/process/gates.md` › Guard trigger.
- **ToolSearch is required** for a subagent (MCP tools arrive deferred). Evidence, `.teamwright/logs/tools.jsonl`: code-reviewer `ToolSearch` at 2026-10-21T12:50:45Z, then `mcp__serena__get_symbols_overview` at 12:50:48Z (the call itself failed server-side: rust language server); problem-investigator `mcp__context7__resolve-library-id` / `query-docs` at 13:15:27-33Z; ui role: task-validator `ToolSearch`, then `mcp__serena__get_symbols_overview` (two calls, no error), 2026-10-24T21:08:17Z (T-114 probe).
- **Don't:** add `search_for_pattern`, `list_dir` and the like (the `claude-code` context of the installed serena-agent excludes them), any Serena edit/memory/shell tool, or the Playwright MCP tools (left out by #145) without a head change and an owner decision; drop `tools:` instead (it would hand the subagent every tool, and Serena's `claude-code` prompt forbids `Edit`, contradicting the Tools block); edit a head to make the gate green without reading what the new text grants.
- **Not caught:** Claude Code reading YAML past the head's closing `---` (the heads end at the first closing line; whatever the body holds is not compared); user-level `~/.claude/agents/` and plugin agents (outside the repo; this host has a user-level `serena-wt-probe.md` with an inline `mcpServers` server); that the pinned text grants what the journal shows for each role, which is evidenced per role by the journal (`agent_type=<role>`, `tool=mcp__*`), not by the gate; a Serena call that fails server-side while the `mcp__serena__*` tools are present (a server that failed to start, the 2026-10-21 code-reviewer case): R4's pinned line covers "no `mcp__serena__*` tools", no pinned line names "a call fails". Recorded, not fixed (#200, T-114).

### `technical-writer` and `requirements-reviewer` have no allowlist entry

They have no `teamwright:tools` block, so their instructions never tell them to call Serena or context7, and their heads hold no `mcp__` token (R4 does not apply). They still have owned heads: H1 and H2 pin their front matter like the other seven. Adding a Tools block to either role is an upstream (kit) change; its head changes with it.

### Serena language servers are typescript, rust, markdown, svelte, in this order

- **Why:** #146 (S1) amends #72: Svelte (16 `.svelte` files) was not indexed. `svelte-language-server` 0.18.0 is installed by Serena itself into `~/.serena/language_servers` (host npm install, owner consent in #146).
- **Order matters:** Serena uses the first configured server whose matcher accepts a file (serena-agent 2.1.1.dev0 `LanguageServerManager._get_suitable_language_server`, servers kept in config order; routing unchanged since 2.0.0.dev0). The svelte server also matches `.ts`/`.js`; with typescript first those stay on the TypeScript server and only `.svelte` goes to svelte.
- **Fail-fast risk:** if any configured server fails to start (offline npm install of the svelte server on first start, rust-analyzer wrapper down), Serena stops all of them and every symbol tool fails. Exit: remove the failing line and record it here (for svelte, S3: read `.svelte` with grep/Read).
- **Where it is enforced:** nowhere; `.serena/project.yml` stays local (#16). Check with `get_symbols_overview` on one `.svelte` and one `.ts` file. Checked 2026-10-24 (T-114), from the orchestrator session and from a `task-validator` probe run (journal `agent_type=task-validator tool=mcp__serena__get_symbols_overview`, 2026-10-24T21:08:17Z): `src/lib/about/About.svelte` → `{"Field": ["script", "div.about", "style"]}` (svelte language server component outline); `src/lib/buildInfo.ts` → `{"Interface": ["BuildInfo"], "Function": ["formatBuildInfo", "loadBuildInfo"]}`.
- **Don't:** put `svelte` before `typescript`.

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| Omit `tools:` in the role files | the subagent gets every tool, including Serena's edit tools, whose prompt contradicts the Edit/Write gates | #145 (B), T-082 |
| Docker wrapper for the Svelte server (like rust-analyzer) | more host-local machinery than the npm install the owner consented to | #146 (S2) |
| Wildcard `mcp__serena__*` in `tools:` | would admit edit, memory and shell tools; the byte compare against the head refuses it | #145, T-082 |
| A guard that reads the YAML (first `tools:` line, token or name filter) | leaks every shape outside its model (probes A-J, F-022); the "tools line literal and no stray `mcp__` token" variant still passes E (`mcpServers` names no `mcp__` token), F and G | #154, T-113 |
| Run Claude Code's own parser in the gate | a closed binary; no offline dump of agent tool sets; absent in CI | T-113 |

## Open

- Closed (T-114, 2026-10-24): T-082 Acceptance 2 is shown, see "Where it is enforced" above.
- Playwright MCP tools for the ui roles' Tools block are left out by #145; kept out by owner decision #157 (OQ-40).
- Upstream: the kit never renders the allowlist entries; draft in `docs/process-reviews/2026-10-09.md` › Feedback drafts.
