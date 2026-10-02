# Tools

The process is fixed: task lifecycle, analysis block, red tests first, a separate reviewer with recurrence exits, a recorded verification of the running system, journals and metrics. The **tools** that serve it are chosen per project, one per capability, in `.teamwright/config.yml`:

```yaml
tools:
  spec: spec-kit
  context: serena
  docs: context7
  ui_verify: playwright
  api_verify: project-runner
```

No hook depends on a tool. Gates read task files, review and verify records and journals; a verify record names the tool that produced it (`tool:`), and the gate checks that the record exists for the current code, not which tool wrote it.

## Capabilities

| Capability | What it serves in the process | Default | Alternatives |
|---|---|---|---|
| `spec` | Large work: specify → clarify → plan → tasks; every task gets `design_ref` | [Spec Kit](https://github.com/github/spec-kit) | [OpenSpec](https://github.com/Fission-AI/OpenSpec); `none` — a plain `docs/specs/<name>.md` |
| `context` | Code-aware context before analysis, implementation and review: definitions, every caller of a seam | [Serena](https://github.com/oraios/serena) (MCP) | [CodeGraphContext](https://github.com/CodeGraphContext/CodeGraphContext) (MCP, `codegraph`); `builtin` — grep, glob, LSP |
| `docs` | Package API lookup for the pinned version before use | [Context7](https://github.com/upstash/context7) (MCP) | `none` — the package's own docs for that version |
| `ui_verify` | End-to-end UI test runs for `kind: ui` verify records | [Playwright](https://playwright.dev/) (web; + Playwright MCP for exploration) | [Patrol](https://github.com/leancodepl/patrol), [Flutter `integration_test`](https://docs.flutter.dev/testing/integration-tests), [Maestro](https://github.com/mobile-dev-inc/maestro), Espresso / XCUITest (`espresso-xcuitest`), `none` |
| `api_verify` | API test runs for `kind: api` verify records | `project-runner` — the project's own suite (Jest + supertest, pytest + httpx, …) | — |

## How a tool reaches the agents

Each provider has an adapter in the kit, `providers/<capability>/<provider>.md`: how to detect and install it (with links to its official docs), its MCP server entry, and a **Rules for agents** block — when to use it, which of its tools or commands, what to do when it is missing, and what an empty result means. Onboarding (`/teamwright:adopt`, `/teamwright:init`) renders the chosen blocks into `AGENTS.md` and every role file between the markers:

```
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
```

`/teamwright:reconfigure` changes a capability's provider: it updates the config, re-renders only the text between the markers, and adds the provider's MCP server to `.mcp.json` with the owner's consent. Everything outside the markers is the project's own text and is never rewritten.

## Toolchain runners

Tools above serve the process; the **toolchain** (compilers, SDKs, test runners) is what every area's commands need to run at all. Each area declares how its commands run:

```yaml
areas:
  - name: app
    runner: docker            # host | docker (default host)
    image: "<the image CI uses for this area>"   # required with runner: docker
    cache: [".pub-cache"]     # package caches kept between runs (named volumes)
    workdir: app
    test_command: "flutter test"          # written without a wrapper
    generate_command: "flutter gen-l10n"  # optional: codegen, run before the gate
```

- `scripts/tw-run <area> -- <cmd>` (installed into the project) runs `<cmd>` on the host or in `docker run --rm` with the caller's uid/gid, the repository mounted, `workdir` as the working directory and the caches as volumes, and exits with the command's code. Rendered commands in `AGENTS.md` and the role files already go through it; `gate_command` is the aggregate and calls `scripts/tw-run` itself for docker areas.
- **Same image as CI.** When CI runs an area in a container, that image is the default for `runner: docker`, so a local green means a CI green. Otherwise the official image pinned to the version in the manifest or lock file, or a host install — the owner chooses at onboarding.
- **The gate does not modify tracked files.** Code generation that rewrites tracked files goes into `generate_command`, run separately; `python3 <kit>/scripts/tw-install.py check-commands` (the installer lives in the plugin; `/teamwright:status --gate` runs it) runs every configured command once and reports pass / fail / not runnable and whether `git status` changed.
- **No bare command that does not run.** A command is in the config only after it ran once, or it is marked not runnable with a first-sprint task.
- **UI runs need a device.** A container rarely has an emulator: `e2e_ui_command` usually runs on the host with a device or emulator attached; without one it is a first-sprint task, not a silent gap.
- Runner and image changes go through `.teamwright/config.next.yml` + `python3 <kit>/scripts/tw-install.py apply --config` like any config change (`/teamwright:reconfigure`).

## Choosing a UI runner

An existing suite wins over the default only if it covers the need. For mobile apps the deciding question is whether verified flows go through **system dialogs or native UI** (permissions, notifications, pickers, another app): then `patrol` (Flutter; builds on `integration_test`, existing tests carry over) or `maestro` (any framework, black-box), never plain `flutter-integration-test`, which cannot press native dialogs. Each `providers/ui_verify/*.md` adapter has a `## When it fits` section. Whichever is chosen, `e2e_ui_command` is run once at onboarding, or recorded as a first-sprint task with the exact blocker (no device, native setup missing).

## Rules that do not depend on the tool

- **Evidence is a test run**, never an interactive session: an MCP-driven browser or device helps to explore and reproduce; the verify record cites a committed test executed by its runner.
- **An empty search result is not proof of absence**, from any tool. Reports say where and how the agent looked.
- **Package APIs are checked against the pinned version's documentation**, never from memory.
- **Installing a tool is the owner's call.** Onboarding asks before adding an MCP server or installing a CLI; secrets such as API keys come from the environment, never from a committed file.
- A project with a UI and no UI runner is brought up to the process (a first-sprint task to add one), not verified by hand.
