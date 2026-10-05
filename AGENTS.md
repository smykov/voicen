# AGENTS.md

Entry point for any AI coding agent working in this repository (Claude Code, OpenCode, others). Tool-specific role files (`.claude/agents/*.md`) implement the roles below; the rules here apply to all of them.

**Read before any work:** `PRINCIPLES.md` (engineering rules with IDs `P-NNN`), `docs/process/lifecycle.md` (task statuses and transitions), `docs/failures.md` (what went wrong in this project and the rule it produced; starts empty). When a hook refuses or warns: `docs/process/gates.md` — what each gate checks and its deliberate exits. A `TEAMWRIGHT_PAUSE` file at the repository root means the owner paused the team: no push, finish the current local commit and stop.

## Gate

```
make check
```

A task never leaves `IN_PROGRESS` with a red gate. The gate is run again by the reviewer and the validator; nobody's report is taken on trust.

Commands run exactly as rendered, through `scripts/tw-run <area> -- <cmd>`: on the host, or for an area with `runner: docker` in the same image as CI — never with the bare tool instead. Code generation (`generate_command`) runs before the gate, separately; the gate itself must leave `git status` unchanged.

## Roles

| Role | Does | Never | Claude Code file |
|---|---|---|---|
| Orchestrator (main session) | picks the task, calls roles, changes statuses, commits | reviews its own work | — |
| problem-investigator | writes the `analysis` block: root cause + evidence, invariant, seam, approach; for a recurrence recommends the exit (code / architecture / process) | fixes; decides exits (b) or (c) | `.claude/agents/problem-investigator.md` |
| test-writer | red test per guarantee; reproducing test per bug; end-to-end test for the Acceptance failure branch | implementation | `.claude/agents/test-writer.md` |
| developer | implements in the seam until tests are green | starts without `analysis`; weakens tests | `.claude/agents/developer.md` |
| technical-writer | syncs `docs/`, appends decisions / failures | changes code | `.claude/agents/technical-writer.md` |
| code-reviewer | fresh-context review, recurrence detection; writes the review record | runs as a fork of the author; edits code; changes statuses | `.claude/agents/code-reviewer.md` |
| task-validator | VERIFY session: runs API tests / end-to-end UI tests / deploy smoke against the running system, writes the verify record; DoD vs reality, gate, targeted mutations on a throwaway copy | edits code or tests; mutates the working tree; counts an interactive exploration as evidence | `.claude/agents/task-validator.md` |
| Owner (human) | decides escalations, architecture reviews (`/arch-review`), amendments to `PRINCIPLES.md` | reviews every diff | — |

Tools without subagents: run each role as a **separate session** with its file as the prompt. The reviewer must never share context with the author.

Why these roles and how they hand over: `docs/process/roles.md`.

## Sessions

| Session | Does | Ends with |
|---|---|---|
| DEV | plan → analysis → red tests → implement → gate → docs → local commit (no push) | task in `NEEDS_REVIEW` |
| REVIEW | `code-reviewer` subagent, fresh context, reviews one task | review record `docs/tasks/<id>.reviews/<n>.md` with verdict `APPROVE \| REQUEST_CHANGES \| REJECT_RECURRENCE \| ESCALATE` |
| DRAIN | push, only if every task in the unpushed tail is `CODE_COMPLETE`, `DEPLOYED`, `VERIFIED` or `DONE` (or `tail_code: ratified`) | pushed |
| VERIFY | `task-validator`: deploy smoke + API tests / end-to-end UI run per the task's `surface`, failure branch included | verify record `docs/tasks/<id>.verify/<n>.md` with `result: PASS \| FAIL` → `VERIFIED` or `VERIFY_FAIL` |
| ARCH-REVIEW (on demand) | `/arch-review` with the owner, for a recurrence whose exit is architecture | owner decision in `docs/decisions.md`; spec/plan or tasks |

**Where to start:** `/teamwright:flow` (Claude Code) computes the next action deterministically — resume an interrupted task, continue, review, verify, re-analyse, ask the owner, or start the next ready task — and drives the lifecycle from there. Other runtimes: `python3 <kit>/scripts/tw-next.py --json` prints the same decision (`<kit>` = the plugin directory or a checkout of the kit; the router is not copied into the project).

Every session ends with an outcome: `barrier | idle | escalate | interrupted`; hooks journal tool calls and outcomes into `.teamwright/logs/`. Details: `docs/process/sessions.md` §2.

Batch size: 1 task by default, at most 3, and only `P2`/`P3` tasks with no blockers; a red gate fails the whole batch. Project override (decision #71): up to two `P1` tasks in parallel when their seams share no file, each in its own worktree.

## Where tasks come from

- **Large** work — spans more than one seam or service, or the product behaviour is not settled — goes through the configured spec tool first (Tools, below): specify → clarify → plan → tasks; with no spec tool, a plain `docs/specs/<name>.md` the owner agrees. Each resulting task record carries `design_ref` (link to its spec/plan section). The spec tool's own implement/apply step is not used: implementation always follows the lifecycle below.
- **Small** work — one seam, behaviour agreed — goes straight into a task record.

## Tasks

One file per task in `docs/tasks/<id>.md` (template: `docs/tasks/_task-template.md`). Statuses:

`TODO → ANALYSIS → IN_PROGRESS → NEEDS_REVIEW → CODE_COMPLETE → DEPLOYED → VERIFIED → DONE`; side: `BLOCKED` (waiting for a person or evidence), `DEFERRED` (with `absorbed_by: <rca id>`), `VERIFY_FAIL` (→ `ANALYSIS`). Review rounds = number of review records. The `task-validator` runs the verification and writes the verify record that the verify gate checks for `VERIFIED` / `DONE`; its acceptance verdict (`PASS` / `FAIL` / `NEEDS_OWNER`) is acted on by the orchestrator: `FAIL` sends the task back to `IN_PROGRESS`, `NEEDS_OWNER` waits for the owner.

Verification: every task declares `surface: api | ui | both | none`. `VERIFIED` and `DONE` require a `PASS` verify record for the current code — `api` → API tests against the running service; `ui` → an end-to-end UI test run with the configured UI runner (Tools › ui_verify); `both` → both; `none` → a deploy smoke, or nothing when `deploys: false`. The record (`task`, `round`, `kind`, `tool`, `command`, `environment`, `commit`, `result`, `evidence`, `verifier`, plus a `Failure branch:` line) is append-only, written with Edit/Write, and cites a test run, not an interactive session. `kind: manual` only with an explicit `verify_exception`, checked by someone other than the implementer. Details: `docs/process/lifecycle.md` §7.

Status change commit: `chore(task): T-042 NEEDS_REVIEW→CODE_COMPLETE`. Agent-written commits carry a `Co-Authored-By:` trailer.

## Rules

1. **No implementation without an `analysis` block** — `rca` tasks included. Bugs and rca tasks go through `problem-investigator` first.
2. **Red test first** for every guarantee and every bug.
3. **Self-review does not count.** Review is a fresh agent, never a fork of the author.
4. **Recurrence → rca task, not another patch.** Same class of bug again (one more `if`, one more allowlist entry, same area for the same reason) → reviewer returns `REJECT_RECURRENCE` and opens a `type: rca` task with `recurrence_of: [...]`; the original task goes to `DEFERRED` with `absorbed_by: <rca id>` (never `BLOCKED`). A workaround commit that links a root task carries an `RCA: T-NNN` trailer. The investigator recommends one of three exits: **(a)** root cause in code — the rca task is fixed in one seam; **(b)** architecture review — `/arch-review`, owner decides, a change spanning more than one seam or service goes through the spec tool as a new spec/plan → tasks with `design_ref`; **(c)** process amendment — the owner amends `PRINCIPLES.md` in a separate commit with a version bump. Every exit is recorded in `docs/decisions.md`.
5. **Two review rounds max.** After two `REQUEST_CHANGES` in a row the third record is `ESCALATE` to a human (or `REJECT_RECURRENCE`), never another send-back or an `APPROVE`; the review gate enforces it. The reviewer never edits code: every finding goes back to the author.
6. **Docs in the same task.** Drift between `docs/` and code is a review finding.
7. **No secrets or personal data** in code, logs, fixtures, docs or commit messages.
8. **Push only in DRAIN.** Never push unreviewed work.
9. **Verify the running system, don't fix it.** The verifier runs committed tests against the running system and records the result; it never edits code or tests. A `FAIL` goes to `VERIFY_FAIL` → `ANALYSIS`, not to a quick patch.
10. **Never roll back with destructive git.** No `git stash`, `checkout -- <path>`, `restore`, `clean`, `reset --hard` or forced push to undo an experiment: in a shared working tree they silently destroy uncommitted work of other sessions (this has wiped large parts of a test file before). Experiment on a throwaway copy (`cp -a` into a temp dir, or `git worktree add`) and discard the copy. These commands are in the deny list.
11. **Config is changed only through the installer.** Nobody edits `.teamwright/config.yml` by hand: the orchestrator writes `.teamwright/config.next.yml` and runs `python3 <kit>/scripts/tw-install.py apply --config .teamwright/config.next.yml` (the installer lives in the plugin, `<kit>` = its directory; `/teamwright:reconfigure` does this; it validates, shows the diff, applies). Staging and committing `config.yml` is allowed. Lowering `enforce` to `gate-first` is the owner's step, outside the agent runtime.
12. **Stage by path** (`git add <path>`), not `git add -A`, when other sessions may be working in the same tree.
13. **Gather context with the configured tools** (Tools, below; no hook depends on them). Navigate code with the context tool before plain text search, and check a package API against the pinned version's documentation through the docs tool instead of relying on memory. An empty result from any tool is not proof that something does not exist; say where you looked. Tools are chosen per project in `.teamwright/config.yml`; the process above does not change with them.

<!-- teamwright:tools:begin -->
## Tools

Configured by teamwright from `.teamwright/config.yml` (spec=spec-kit, context=serena, docs=context7, ui_verify=playwright, api_verify=project-runner). Re-rendered by `/teamwright:reconfigure`; edit outside the markers.

### spec: spec-kit

- **When:** a task is large (more than one seam or service, or product behaviour not settled) → it starts in Spec Kit, not in a task record. Small work skips Spec Kit.
- **Commands** (agent skills, run in the chat one at a time, review each result): `/speckit-specify <what and why>` → `/speckit-clarify` → `/speckit-plan <stack and constraints>` → `/speckit-tasks`. Some integrations name them `/speckit.specify` etc.; use whichever the project's `.claude/` provides. `/speckit-analyze` is optional before `tasks`.
- **Never run `/speckit-implement` or `/speckit-converge`.** Implementation goes through the teamwright lifecycle.
- **Where the output lives:** one directory per feature under the Spec Kit specs directory (usually `specs/<NNN-feature>/` with `spec.md`, `plan.md`, `tasks.md`). Each item of `tasks.md` becomes a `docs/tasks/T-NNN.md` record with `design_ref: specs/<NNN-feature>/plan.md#<section>` (relative path + anchor).
- **Constitution:** `.specify/memory/constitution.md` points to `PRINCIPLES.md`; if it has to change, it changes in the same commit and with the same version as `PRINCIPLES.md`. Agents do not edit either.
- **No `specify` CLI or no `.specify/` directory:** do not improvise a spec format; write the spec as `docs/specs/<name>.md` (problem, behaviour, decisions, open questions) and link it with `design_ref`, and tell the orchestrator Spec Kit is missing.

### context: serena

- **Before reading files to understand code, ask Serena.** `get_symbols_overview` for a file's structure; `find_symbol` for a definition (pass `relative_path` to scope it, `include_body: true` only for the symbol you need); `find_referencing_symbols` for every caller of a seam; `find_implementations` / `find_declaration` for interfaces.
- **Who uses it for what:** investigator — every path that handles the concern (hypothesis "a second path"); developer — callers of the seam before changing it; test-writer — the seam's public surface; reviewer — call sites and sibling paths of every changed symbol.
- **Documents** (`docs/`, specs): `get_symbols_overview` on the file lists its headings; read only the section you need (`find_symbol` with the heading name, or the line range it reports). A requirement id: `scripts/tw-req <ID>`.
- **Text search** (`search_for_pattern`, or grep) is for strings, config keys, SQL, templates and files the language server does not parse; use it too when a symbol tool returns nothing or times out.
- **An empty result is not proof of absence** — an unindexed file, an unsupported language or a dynamic call also return nothing. Say which tool and scope you used.
- **Stale index** (results miss code you can see): tell the orchestrator to run `serena project index`; meanwhile fall back to grep and say so.
- **Edit with the normal Edit/Write tools**, not Serena's editing tools (`replace_symbol_body`, `insert_*`, `rename_symbol`): the teamwright gates see only Edit/Write. Do not write Serena memories for process facts — those live in `docs/`.
- **Serena not connected** (no `mcp__serena__*` tools): use grep and file reads, and say so in your report.

### docs: context7

- **Before using a library or framework API** you have not already verified in this task — a new call, a changed signature, a config option, a migration — look it up: `resolve-library-id` (`libraryName`, `query`) → `query-docs` (`libraryId`, `query`). Skip the first step when the ID is known (e.g. `/vercel/next.js`).
- **Pin the version:** read the version from the project's lock file or manifest and put it in the query; docs for another major version are a finding, not an answer.
- **Cite it:** name the library ID and version in your report when a decision depends on the docs. The reviewer checks changed calls against the same source.
- **No match or empty result:** the package is not indexed — read the package's own docs or source in the dependency directory, say so, and never fill the gap from memory.
- **Not connected** (no `mcp__context7__*` tools) or rate-limited: same fallback; report it.

### ui_verify: playwright

- **Test files:** under the project's Playwright `testDir` (see `playwright.config.*`), one scenario per test named after the Acceptance line. Locators: `getByRole`, `getByLabel`, `getByTestId` — no CSS chains or XPath tied to layout. Wait with web-first assertions (`await expect(locator).toHaveText(...)`), never `waitForTimeout`.
- **Run:** the area's `e2e_ui_command` (typically `npx --no playwright test <file>` — `--no` stops npx from downloading a package that is not installed); a single test with `-g "<title>"`; against a running system with its base URL from the environment (`BASE_URL=... npx playwright test`). Red first: show the test fails on the missing behaviour, not on a locator typo.
- **Assert the effect,** not only the page text: where the test can reach it, check the API response (`page.waitForResponse`, `request` fixture) or the stored state.
- **Explore with Playwright MCP** (`browser_navigate`, `browser_snapshot` for the accessibility tree and refs, `browser_click`, `browser_fill_form`, `browser_console_messages`, `browser_network_requests`) to reproduce a bug or find locators. Snapshot before acting; never type real credentials; close the browser (`browser_close`) when done. What you learn goes into a committed test.
- **Validator:** record `tool: playwright` and the exact command; `evidence` is the HTML report directory (`playwright-report/`) or a JUnit file, plus `test-results/` traces for failures.
- **Browsers missing** (`Executable doesn't exist`): installing them (`npx playwright install`) is setup, not a test failure — ask the owner first, it downloads browsers; never install when the owner declined downloads. **Flaky** (passes on retry): report it as a finding; a retry pass is not a clean PASS.

### api_verify: project-runner

- **Test files** live with the area's existing API tests; follow their fixtures and naming. One test per Acceptance scenario, failure branch included (invalid input, missing auth, dependency down, limit exceeded).
- **Against the running system:** the base URL comes from an environment variable (e.g. `BASE_URL`), never a host in code or in the record. In-process tests (app booted inside the test) count for the gate, not for verification of a deploy — say which kind you ran.
- **Assert the effect:** status code and body **and** the stored state, sent message or log line where reachable — not the API's own success message alone.
- **Test data:** fake values only (`example.com`, documentation IP ranges, obvious fake tokens); create what you need and clean it up; never point at production data.
- **Run:** the area's `api_test_command`, narrowed to the task's tests with the runner's own filter (`-t`, `-k`, `--grep`). Save machine-readable output (JUnit XML or JSON reporter) when the runner supports it.
- **Validator:** `kind: api`, `tool:` the runner name (e.g. `jest`, `pytest`), the exact command, `environment: local | staging | production-like`.
<!-- teamwright:tools:end -->
