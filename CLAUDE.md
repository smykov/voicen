# CLAUDE.md

<!--
This file is a MAP, not an encyclopedia. It is loaded into every session in full,
so every line is paid for on every run.
Size budget: a hook warns when it exceeds 40 KB (TEAMWRIGHT_CLAUDE_MD_MAX_KB in
.claude/settings.json). That is a ceiling, not a target: a map is usually far shorter.
Put in here only what costs you BEFORE you'd think to open a file.
Everything else goes to docs/decisions/<area>.md and is linked from the index below.
Replace every <placeholder>; delete sections that don't apply.
-->

## Process

This repository follows the teamwright process: start with `AGENTS.md` (roles, sessions, rules). Statuses and transitions: `docs/process/lifecycle.md`. A hook refused or warned: `docs/process/gates.md` (each gate's exits). Who does what: `docs/process/roles.md`, `docs/process/sessions.md`. A `TEAMWRIGHT_PAUSE` file at the root stops all pushes. In Claude Code, `/teamwright:flow` picks the next step.

## Project

<One paragraph: what the system does, for whom, main components.>

**Stack:** <languages, frameworks, datastore, runtime>

## Where things live

| Path | What |
|---|---|
| `<src dir>/` | <application code> |
| `<tests dir>/` | <tests> |
| `docs/plan.md` | Stages and what is in each |
| `docs/requirements.md` | What must be true (FR-NN) |
| `docs/architecture.md` | Components, data flow, boundaries |
| `docs/decisions.md` | Append-only decision log |
| `docs/decisions/<area>.md` | Why an area is shaped the way it is: invariants and the defects behind them |
| `docs/open-questions.md` | Unresolved questions (OQ-NN) |
| `docs/failures.md` | What broke, why, which rule it produced (F-NNN) |
| `docs/tasks/` | One file per task (T-NNN); `<ID>.reviews/` review records, `<ID>.verify/` verification records |
| `<specs dir>/` | Specs and plans for large work (spec tool: `.teamwright/config.yml` → `tools.spec`); tasks link them via `design_ref` |
| `docs/sprints/` | One file per sprint |
| `PRINCIPLES.md` | Engineering principles with enforcement tiers |

### Decision index — open the file for the area **before** you edit it

| File | What it decides |
|---|---|
| [`docs/decisions/<area>.md`](docs/decisions/<area>.md) | <one line: the invariants it holds> |

## What you must know before opening any file

Each line has a defect behind it. Listed here because the cost is paid before anyone thinks to open a file.

- **<Invariant 1, imperative.>** <One sentence why.> Defect: F-NNN · details: `docs/decisions/<area>.md`
- **<Function X is not to be rewritten without an invariant and owner sign-off.>** <It was rewritten and reverted N times.> Defect: F-NNN
- **<New config values must be passed into the runtime, not only added to the config file.>** Defect: F-NNN
- **<Secrets: which ones exist, where they must never appear.>** Defect: F-NNN

## Commands

```sh
<setup command>
<gate command>          # e.g. make test — must be green before NEEDS_REVIEW
<single test command>
<run locally command>
<api test command>      # API tests against a running service (surface: api)
<e2e ui command>        # end-to-end UI run with the configured runner (tools.ui_verify)
```

Gotchas: <e.g. suites that must not run concurrently; services that must be up first>.

## Rules

- Follow the task lifecycle: no code before the task's `analysis` block is filled; status changes are commits `chore(task): T-NNN FROM→TO`.
- Gate green before `NEEDS_REVIEW`. Commit locally; **never push from a DEV session**.
- Self-review doesn't count — `CODE_COMPLETE` is set only by a separate review.
- Large work (more than one seam or service, or unsettled behaviour) starts in the spec tool; its tasks carry `design_ref`. Small work goes straight to a task.
- On a recurrence of a known defect class: stop patching, open a `type: rca` task (`recurrence_of: [...]`); exits — code fix, `/arch-review`, or a `PRINCIPLES.md` amendment (owner decides the last two).
- `VERIFIED` / `DONE` need a `PASS` verify record for the current code (per the task's `surface`); the verifier never edits code.
- Gather context with the configured context and docs tools (`AGENTS.md` › Tools) before text search or memory.
- Docs affected by a change are updated in the same commit.
- Never put secrets, tokens or customer data in code, logs, commits or docs.
- Unsure about a product decision → add to `docs/open-questions.md` and ask; don't guess.
- Owner decisions go into `docs/decisions.md` immediately (Was → Decided).
