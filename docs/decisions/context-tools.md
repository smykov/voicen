# Context tools for roles: Serena language servers and subagent tool allowlists

**Code:** `.claude/agents/*.md` (front-matter `tools:` of the 7 roles with a `teamwright:tools` block), `scripts/ci/role-tools.sh` with the owned manifest `scripts/ci/role-tools.txt` (Makefile `check-role-tools`, part of `make check`), host-local `.serena/project.yml` (`language_servers`; untracked, decisions #16) · **Tests that pin it:** `scripts/ci/role-tools.test.sh` (fixtures R1-R4 and the real tree)

Task: T-082 (class `context-tool-coverage`; no `docs/failures.md` entry: first task of the class, a need found by process review 2026-10-09). Decisions: #72 (language servers), #145 (allowlists), #146 (svelte, amends #72).

## Invariants

### Every MCP tool that a role's rendered Tools block tells it to call is named in that role's `tools:` allowlist, and only read/navigation tools are named

- **Defect that produced it:** none filed; process review 2026-10-09: 0 MCP calls from any subagent over 13 sessions. Claude Code gives a subagent with a `tools:` line only the tools named there, and the kit templates (teamwright 0.8.2) render the Tools text but not the allowlist entries it depends on, so every role took its "Serena not connected" fallback silently.
- **What breaks if you violate it:** roles read Rust, Svelte and specs with grep and whole-file reads (the cost #72 was meant to remove). A Serena edit, memory or shell tool in `tools:` would bypass the Edit/Write gates, which must stay the only write path.
- **Where it is enforced:** `scripts/ci/role-tools.sh` (`make check-role-tools`), for every `*.md` in `.claude/agents/` that holds the `teamwright:tools:begin` line: R1 a front-matter `tools:` line; R2 every manifest entry is named; R3 no other `mcp__` name (wildcards and bare server names included); R4 the Tools block keeps both fallback lines (Serena: grep and file reads; context7: same fallback), so an unavailable tool never stops the role. Manifest: `scripts/ci/role-tools.txt` (5 Serena navigation tools, the 2 context7 tools, `ToolSearch`).
- **Don't:** add `search_for_pattern`, `list_dir` and the like (the `claude-code` context of the installed serena-agent excludes them), any Serena edit/memory/shell tool, or the Playwright MCP tools (left out by #145) without a manifest change and an owner decision; drop `tools:` instead (it would hand the subagent every tool, and Serena's `claude-code` prompt forbids `Edit`, contradicting the Tools block).
- **Not caught:** whether Claude Code offers the named tools to a subagent (settled by the T-082 Acceptance run: a journal line with `agent_type=<role>`, `tool=mcp__serena__*`); whether `ToolSearch` is needed there (if not, drop it from the manifest).

### `technical-writer` and `requirements-reviewer` have no allowlist entry

They have no `teamwright:tools` block, so their instructions never tell them to call Serena or context7; the invariant (a named tool is reachable) has nothing to protect for them, and the tripwire ignores files without the marker. Adding a Tools block to either role is an upstream (kit) change; then the manifest check applies to it automatically.

### Serena language servers are typescript, rust, markdown, svelte, in this order

- **Why:** #146 (S1) amends #72: Svelte (16 `.svelte` files) was not indexed. `svelte-language-server` 0.18.0 is installed by Serena itself into `~/.serena/language_servers` (host npm install, owner consent in #146).
- **Order matters:** Serena uses the first configured server whose matcher accepts a file (serena-agent 2.0.0.dev0 `LanguageServerManager._get_suitable_language_server`, servers kept in config order). The svelte server also matches `.ts`/`.js`; with typescript first those stay on the TypeScript server and only `.svelte` goes to svelte.
- **Fail-fast risk:** if any configured server fails to start (offline npm install of the svelte server on first start, rust-analyzer wrapper down), Serena stops all of them and every symbol tool fails. Exit: remove the failing line and record it here (for svelte, S3: read `.svelte` with grep/Read).
- **Where it is enforced:** nowhere; `.serena/project.yml` stays local (#16). Check with `get_symbols_overview` on one `.svelte` and one `.ts` file.
- **Don't:** put `svelte` before `typescript`.

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| Omit `tools:` in the role files | the subagent gets every tool, including Serena's edit tools, whose prompt contradicts the Edit/Write gates | #145 (B), T-082 |
| Docker wrapper for the Svelte server (like rust-analyzer) | more host-local machinery than the npm install the owner consented to | #146 (S2) |
| Wildcard `mcp__serena__*` in `tools:` | would admit edit, memory and shell tools; R3 refuses it | #145, T-082 |

## Open

- T-082 Acceptance 1 and 2 (subagent Serena call in the journal; `get_symbols_overview` on `.svelte` and `.ts`) need a restarted session with a working Serena; see the task's Notes (2026-10-21).
- Upstream: the kit never renders the allowlist entries; draft in `docs/process-reviews/2026-10-09.md` › Feedback drafts.
