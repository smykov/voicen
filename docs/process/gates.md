# Gates

A rule that is only written down gets skipped exactly when it matters: under time
pressure, late in a long session, by an agent that is sure it is right. Gates turn the
few rules that must hold into checks that run on their own.

Tiers (same scale as `PRINCIPLES.md`): **T0** written guidance · **T1** reviewer
checklist · **T2** hook warns · **T3** hook/CI blocks. The tiers below are those of
`enforce` mode: while a project runs in `gate-first`, the analysis, review and verify
gates warn instead (T2) until `enforce` is switched on. Git hooks, the deny list and
the state guard behave the same in both modes.

`tw-install.py` and `tw-next.py` live in the plugin (the kit), not in the project. Run
them as `python3 <kit>/scripts/tw-install.py ...`, where `<kit>` is the plugin directory
(`/teamwright:status` prints the full path), or through the skills that call them:
`/teamwright:status`, `/teamwright:flow`, `/teamwright:reconfigure`.

## Overview

| Gate | Where it runs | Tier | Prevents |
|------|---------------|------|----------|
| [Analysis gate](#analysis-gate) | Claude Code `PreToolUse` (Edit/Write/Bash) + `PostToolUse` | T3 | Coding before the root cause is known; a task handed to review from the red-tests stage |
| [Review gate](#review-gate) | Claude Code `PreToolUse` (Edit/Write/Bash) + `PostToolUse` | T3 | Self-review, skipped review, endless review loops, recurrence accepted as a patch |
| [Verify gate](#verify-gate) | Claude Code `PreToolUse` (Edit/Write/Bash) + `PostToolUse` | T3 | "Done" that does not work in the running system |
| [Journal](#journal) | Claude Code tool, permission and stop events | — (facts) | Metrics and gates built on self-report |
| [Deny list](#deny-list) | `.claude/settings.json` permissions | T3 | Destroying uncommitted work or shared history |
| [Patch guard](#patch-guard) | git `commit-msg` | T2 (warns; blocking is opt-in) | "One more `if`" fixes in an area already fixed before |
| [DRAIN / pause](#drain-and-pause) | git `pre-push` | T3 | Unapproved work leaving the machine; pushes during a pause |
| [WIP push guard](#wip-push-guard) | Claude Code `PreToolUse` (Bash), project hook | T3 (best effort) | The `git -C` / `git -c` push forms that the push denies and `pre-push` miss |
| [Secret scan](#secret-scan) | git `pre-commit` (CI only if the project adds it) | T3 | Tokens, keys, `.env`, local settings in history |
| [CLAUDE.md budget](#claudemd-budget) | Claude Code `SessionStart` + git `pre-commit` | T2 | A context file that grows into a second docs folder |
| [Gate runs](#gate-runs-tw-gate) | `scripts/tw-gate <area>`, called by the developer, the flow, the reviewer and the validator | T1 (tooling) | The same full gate run by four roles on the same code; gate output lost to `\| tail`; a docs commit invalidating the cache; a gate cut by the 10-minute Bash limit; the server suite run for a CI-only change |
| [Test guard](#test-guard) | Claude Code `PreToolUse` (Bash) + `scripts/tw-test <area> [tests]` | T3 (own switch `process.test_guard`, blocks by default) | Tests and gates run directly — with the wrong workers, flags that break fixtures — and their output cut with `\| tail`, then run again to see what failed |
| [Mutation checks](#mutation-checks-tw-mutants) | `scripts/tw-mutants <area> <ID>`, called by the test-writer and the validator; the stop rules in the verify gate | T1 (tooling) + T3 (stop rules) | Mutation rounds without an end: every sweep a new, larger list, any survivor a FAIL, the runner started once per mutant |
| [Guard trigger](#guard-trigger) | git `pre-commit` | T3 | A guard written without the P-020 design step (`checker: true`, `analysis.decides_on`) |

All files live in `scripts/hooks/`. Install the git hooks once per clone:

```bash
bash scripts/install-hooks.sh          # sets core.hooksPath, adds local-state paths to .gitignore
bash scripts/install-hooks.sh --check  # verify
```

The Claude Code gates are wired in `.claude/settings.json` (project level, so every
session in the repo loads them, whoever started it: human-driven, orchestrator or
autonomous). They are not in a per-flow settings file, so a manually started session
cannot bypass them. The file holds only settings keys; the reasons live here.

**Identity.** Every hook call carries `session_id`. A subagent shares its parent's
`session_id`; only `agent_id` (present inside a subagent) and `agent_type` (the agent's
name, also present for a session started with `claude --agent <name>`) tell agents
apart. The gates therefore identify an agent by `session_id` + `agent_id` and a role by
`agent_type` — fields set by the runtime, not written by the agent.

**Shared contract of the Claude Code gates:** fail open on any internal error (a broken
gate must not stall the team); silent on allow; on deny, the reason says what to add and
which exits exist. Requires `python3` (stdlib only); without it the gates are skipped.

**Mode.** `mode:` in `.teamwright/config.yml` decides what the analysis, review and
verify gates do: `gate-first` warns (a message to the user and the model, no decision),
`enforce` blocks. Only an explicit `gate-first` warns; a missing file, key or unknown
value means `enforce`. The gates read the file with the installer's own parser
(`scripts/hooks/_config.py`, YAML subset or JSON), so `tw-install.py plan` reports the
mode the gates apply. `TEAMWRIGHT_GATE_MODE=warn|enforce`, set by whoever starts Claude
Code, overrides the file.

**Who switches the mode.** Raising strictness (`gate-first → enforce`) is the owner's
call: the owner runs `python3 <kit>/scripts/tw-install.py enforce`, or explicitly
approves the main session running it (`/teamwright:status` and `/teamwright:flow` ask
first). Lowering it (`enforce → gate-first`) is owner-only and happens outside Claude
Code: the installer refuses it inside any Claude Code tool call (see the owner override
below).

### State guard

In `review-gate.sh` (`_hookio.guard_state`), T3 in every mode. The mode switch, the
install manifest and the ledgers are not the agents' to change:

- Edit/Write to `.teamwright/config.yml` or `.teamwright/installed.json`, from any agent
  including the main session, is **denied in every mode** — `gate-first` cannot be used
  to switch itself back on. So are shell commands that actually write, move or remove
  them: redirects (`>`, `>>`, heredocs), `tee`, `cp`/`install`/`rsync` onto them, `mv` or
  `rm` of them, `sed -i` / `perl -i` / `yq -i`, `dd of=`, `git checkout` / `restore` /
  `apply`, `sh -c` / `bash -c` with any of these, and interpreter code (python, node,
  ruby, awk, ...) that names them together with a write call. Globs (`.teamwright/*.yml`)
  and `cd .teamwright && ...` count.
- A command that only **names** the file is allowed: `cat`, `grep`, `sed -n`, `git add`,
  `git commit` (whatever the message says), `git diff`, `git log`, copying it *to*
  another file. The guard parses the command into simple commands, it does not search
  for the file name. Why: an earlier guard matched the name anywhere in the command
  and blocked an agent from committing the config it had been asked to change.
- Config changes, by any agent, at any time: write the proposal to
  `.teamwright/config.next.yml` (allowed, gitignored), then the main session runs
  `python3 <kit>/scripts/tw-install.py apply --config .teamwright/config.next.yml`. The installer validates
  it, prints the diff, applies it and removes the proposal. Lowering the mode
  (`enforce → gate-first`) is still refused inside Claude Code.
- Edit/Write to the ledgers and journal (`.teamwright/sessions/`, `.teamwright/logs/`,
  `.teamwright/current-task`) are denied like any gate decision; shell writes to them
  are covered by the review gate (5).
- `tw-install.py` runs only from the main session (a subagent is denied; it hands the
  proposal file to the main session).
- Running a hook is refused: `bash scripts/hooks/review-gate.sh`, `./scripts/hooks/…`,
  `cd scripts/hooks && …`, `sh -c` with one, and interpreter code that imports the hook
  helpers (`scripts/hooks`, `_hookio`). Hooks are run by Claude Code and git with the
  runtime's input; a gate fed a hand-made payload would write a genuine-looking ledger
  line or stamp. Reading them (`cat`, `grep`, `git diff`) is fine.
- Owner override: a person runs `TEAMWRIGHT_OWNER=1 python3 <kit>/scripts/tw-install.py apply ...`
  outside Claude Code, or edits the file by hand. The variable is ignored when
  `CLAUDECODE` is set (every Claude Code tool call), and agent commands that mention
  `TEAMWRIGHT_OWNER` or `CLAUDECODE` are denied.

---

## Analysis gate

`scripts/hooks/analysis-gate.sh` · T3

**Incident it prevents.** A developer agent takes a task and diagnoses, designs and
implements in one pass. The questions "what is the root cause, what is the evidence,
what must stay true" were asked only by the reviewer, after the code existed. A wrong
root cause cost a full review round; the same seam was reopened five times in a row.

**What it blocks**

1. An edit of `docs/tasks/<ID>.md` that moves the task into `status: IN_PROGRESS` while
   the `analysis:` block lacks any of `root_cause`, `evidence`, `invariant`, `seam`,
   `approach` (empty, `...`, `TODO`, `<placeholder>` count as missing). A `type: rca`
   task needs `root_cause`, `evidence`, `decision` instead — it is not exempt, and
   `skipped:trivial` does not apply to it. Only the entry is checked; edits of a task
   that is already `IN_PROGRESS` pass.
2. An edit of a code file while the active task (`.teamwright/current-task`, or
   `$TEAMWRIGHT_TASK`) is not `IN_PROGRESS` or has no complete analysis. Files under
   `docs/`, `.claude/`, `.github/`, `.teamwright/` and top-level `*.md` are not code.
   If no active task is declared, this check does nothing.
3. A shell command that creates or replaces a task record in `docs/tasks/` (`>`, `tee`,
   `cp` / `mv` / `install` onto it — the whole record, with or without a `status:`), or
   rewrites its status in place (`sed -i`, `perl -i`) — records and status changes go
   through Edit/Write so the gates see them.
   Appending (`>>`, `tee -a`) is allowed: it lands after the front matter, where `status:`
   is never read, so a note cannot change the task's state. The same holds for a command
   that rewrites the `stage:` field — recognised by the keys or by their values
   (`perl -pi -e 's/red-tests/implement/'`, `sed -i 's/ANALYSIS/IN_PROGRESS/'`).
   Also refused: an old version put back (`git checkout <rev> -- <record>`, `git restore`,
   `git stash pop|apply` or `git apply|am` of changes that touch a record), `dd of=<record>`,
   and interpreter one-liners that name a record and write (after `cd docs/tasks` too) —
   best effort, like every shell parser here.
4. **Stage order** — an edit of `docs/tasks/<ID>.md` that breaks the order of the
   `stage:` field, the sub-stage of `IN_PROGRESS` (`sessions.md` §8). Not opt-in:
   - a value other than `red-tests` or `implement`;
   - **entering `IN_PROGRESS` without a stage** in the same edit: from `TODO`, `ANALYSIS`
     or `VERIFY_FAIL` it is `stage: red-tests` (the red tests come first); a send-back
     (from `NEEDS_REVIEW`, a verified status, `BLOCKED`) may set `red-tests` or
     `implement`;
   - removing the field from an `IN_PROGRESS` task;
   - **`stage: implement` while `## Tests` is empty** (nothing but the template
     placeholder): the red-tests stage fills it and sets the stage in the same edit;
   - **`IN_PROGRESS → NEEDS_REVIEW`** unless the task is at `stage: implement` (a task
     started before stages sets it in that edit), `## Tests` is filled, and — when the
     task changes code — `## Tests` has the `### Failure branches` table of the
     test-writer's bite. *Changes code*: a commit whose message names the task id, or a
     tracked uncommitted change in the tree, touches a code file — anything outside
     `docs/`, `.teamwright/`, `.claude/`, `.github/` and top-level `*.md`
     (`TEAMWRIGHT_NONCODE_RE`, the rule of check 2; `_task.code_changed`). An rca exit
     (b)/(c), a docs-only task, or a task with `analysis: skipped:trivial` (a version bump,
     a typo in code — no failure branch to break; never an rca task) needs no table.
   `red-tests → implement` and back (`implement → red-tests`, or either value when review
   or verification return the task to `IN_PROGRESS`) pass. `gate-first` warns, `enforce`
   blocks, as for every check.

**Deliberate exits** (all stay visible in the task file)

| Situation | Exit |
|-----------|------|
| Root cause proven | Fill the block, then move to `IN_PROGRESS` |
| No design freedom (typo, version bump, docs) | `analysis: skipped:trivial` |
| Root cause cannot be proven with current evidence | `ANALYSIS → BLOCKED` with `blocked_on: <reason>` (what evidence or person it waits for) |
| Instance of a known root | `absorbed_by: <rca task>` + `DEFERRED` |
| The task *is* the investigation | `type: rca` with root cause + evidence + decision |
| A red test turns out wrong during implement | `stage: red-tests` in the same edit as the note why; the test-writer rewrites it |
| No red tests are possible (rca exit (b)/(c), no code) | `## Tests: none — <why>` and `stage: implement` |

**Active task.** The gate keeps `.teamwright/current-task` itself: on `PostToolUse` (the
edit has landed) it writes the task id when the task is `IN_PROGRESS` and clears the
file when that task leaves `IN_PROGRESS`. A denied or failed edit never changes it.
One active task per working tree: parallel sessions need separate worktrees — which is
how `/teamwright:parallel` runs tasks at the same time (one worktree on `tw/<ID>` and one
session per task; each has its own active task, journal and ledgers).

Configuration: `TEAMWRIGHT_ANALYSIS_FIELDS`, `TEAMWRIGHT_RCA_FIELDS`,
`TEAMWRIGHT_CODE_STATUSES`, `TEAMWRIGHT_NONCODE_RE` ([environment](#environment-variables)).

## Review gate

`scripts/hooks/review-gate.sh` · T3

**Incident it prevents.** The review stage existed in the protocol but ran in a small
minority of sessions: the "reviewed" flag was a self-declaration after the fact. Where
review did run, it accepted the Nth patch for the same class of bug because each patch
looked reasonable alone.

**Review record** — `docs/tasks/<ID>.reviews/<round>.md`, template
`docs/tasks/_review-template.md`:

```yaml
---
task: T-042
round: 1
reviewer: code-reviewer
verdict: APPROVE            # APPROVE | REQUEST_CHANGES | REJECT_RECURRENCE | ESCALATE
recurrence_of: [T-017]      # REJECT_RECURRENCE only
rca_task: T-050             # required with REJECT_RECURRENCE
---
```

**What it blocks**

1. Writing a review record unless the call comes from an agent with
   `agent_type: code-reviewer` (`$TEAMWRIGHT_REVIEWER_AGENT`) that did not implement the
   task. The hook records every agent (`session_id:agent_id`) that starts
   (`→ IN_PROGRESS`) or submits (`→ NEEDS_REVIEW`) a task in
   `.teamwright/sessions/<ID>.dev`, and every review round it lets through, with its
   author and verdict, in `.teamwright/sessions/<ID>.reviews.jsonl` — once the write
   has landed (`PostToolUse`). The record's
   `task:` must be the task whose `.reviews/` directory it is written to. A send-back
   (`NEEDS_REVIEW → IN_PROGRESS`) is not recorded as development.
2. `→ CODE_COMPLETE` unless the latest round says `APPROVE` and
   - its ledger entry shows it was written by a `code-reviewer` agent that did not
     implement the task, with that verdict (a record edited afterwards does not count);
   - the journal (`.teamwright/logs/tools.jsonl`) shows the reviewer subagent was
     spawned for this task in that session — the task id must appear in the spawn's
     description or prompt. A top-level `claude --agent code-reviewer` session has no
     spawn and needs none;
   - when this checkout has no review ledger for the task at all (a status move on a
     fresh checkout — an autonomous job), the round's committed stamp
     `docs/tasks/<ID>.reviews/<n>.gate.json` (*Gate stamps* below) stands in for the
     ledger entry and the journaled spawn, as long as it matches the record. Where the
     ledger exists it alone decides, as before stamps: a round it does not have is not
     recognised, whatever the stamp says;
   - the record's `task:` names this task (a review copied from another task's
     directory does not count), and the stamp names this task, record and round;
   - `reviewer` differs from the task's `implemented_by` (when set);
   - the approval is newer than the task's latest failed check (a `FAIL` verify or
     acceptance record): an `APPROVE` from before a `VERIFY_FAIL` does not carry over,
     even when the code did not change — the task needs a fresh review round.
   Who moves the status (normally the orchestrator) does not matter; who wrote the
   approval does.
3. A review with `REJECT_RECURRENCE` whose `rca_task` does not point to an existing
   `docs/tasks/<ID>.md` with `type: rca`.
4. Any verdict outside the four above. After two `REQUEST_CHANGES` rounds in a row
   (`TEAMWRIGHT_ESCALATE_AFTER`), the third record may only say `ESCALATE` or
   `REJECT_RECURRENCE` — neither another send-back nor an `APPROVE` of the third
   attempt. Rounds are counted from the `.reviews/<n>.md` files (never from a field in
   the task). An `APPROVE` or `ESCALATE` ends a run: a task that comes back after
   `VERIFY_FAIL`, or after the owner's decision on an escalation, starts a new count.
   Deny messages quote the counted rounds.
5. Shell commands that write review records or the `.teamwright/` ledgers and journal,
   and any shell write that targets a review, verify or validation record directory or a
   stamp (`docs/tasks/<ID>.reviews|verify|validation/`, `*.gate.json`): redirects, `tee`,
   `cp` / `mv` / `install` / `rsync` / `ln` onto them (also a directory copied *into*
   `docs/tasks/`), `rm`, `dd of=`, `sed -i` / `perl -i`, `tar` / `unzip` into them,
   `find … -delete|-exec`, `git checkout|restore|mv|rm` of them, `git stash pop|apply` and
   `git apply|am` of changes that touch them, `sh -c` with any of these, and interpreter
   one-liners (python, node, perl, ruby …) that name such a path and write — after a `cd`
   or `pushd` too. Reading, `ls`, `git add` and `git commit` are fine. Best effort: a path
   built at run time or a script file is not seen (*Known limitations*).
6. With `process.review.blocking` set in `.teamwright/config.yml` (the owner's threshold,
   `low` | `medium` | `high`): an `APPROVE` whose findings table holds a blocking
   row, or a `REQUEST_CHANGES` with none. The findings table is the first table with a
   Severity column under a heading starting with "Findings" (any level), or the first
   such table in the record when no heading says so. A row blocks when its Severity is at or above
   the threshold, when it is Critical, or when its Category is `secret-leak`,
   `security`, `weakened-test`, `done-not-working` or `recurrence` — at any threshold.
   Severities other than Critical | High | Medium | Low are refused. Rows below the
   threshold are follow-ups: the task is approved and the flow collects them into one
   `review-follow-up` task per sprint. Without the key nothing here is checked (every
   finding may send the task back, as before settings existed).
7. **A review that spawns more than two tasks** (`lifecycle.md` §6: one reviewed task
   spawns at most two new tasks; more goes into one umbrella task). Creating a new
   `docs/tasks/<ID>.md` counts toward the review of task R when the session's latest
   spawn by its main agent was a `code-reviewer` naming R (the journal); a later spawn
   of another role closes that window, a reviewer's own helper spawns do not. Both the
   reviewer (an rca task) and the orchestrator (the sprint's follow-up task) count. The
   third new record is refused — append to an existing record (the follow-up task, one
   umbrella task) instead, or ask the owner. Counted per reviewed task and spawn in
   `.teamwright/sessions/<R>.new-tasks.jsonl`; re-creating the same id is not a new
   task. In `gate-first` the third one warns and is counted. A record created outside
   Edit/Write (a shell redirect, `cp`, a script) is not seen when it is written, so the
   count is taken again when R's status leaves `NEEDS_REVIEW` in that window: every
   `docs/tasks/<ID>.md` added in a commit since the reviewer's spawn, or new in the tree
   (untracked or added to the index) and written since then, counts. Limit:
   `TEAMWRIGHT_REVIEW_NEW_TASKS` (2). The seam is the review gate because it already
   reads the reviewer spawns from the journal: the session's stage is only declared at
   its end (`Stage:`), so the reviewer spawn is the one fact available while the review
   runs.

**Human review.** A person reviewing outside Claude Code writes the record with
`reviewer: human:<name>` and commits it. The gate accepts that record only when no agent
wrote the round; agents cannot write the marker.

**Deliberate exit:** `review: skipped:trivial` in the task record. Any agent that edits
the task record can write it, the author included; it stays in the record, the
validator checks it (acceptance step "Review"), and metrics count tasks that reached
`CODE_COMPLETE` without a review record.

## Verify gate

`scripts/hooks/verify-gate.sh` · T3

**Incident it prevents.** A task was closed as "done" on green unit tests and an
approved diff, and the feature did not work in the running system: the endpoint failed
behind the real configuration, the screen was never wired to it, the failure branch
from Acceptance was never exercised end to end. Review reads code; only running the
system shows that it works. A "verified" flag written by the agent after the fact is the
same self-report the review gate exists to replace.

**Verify record** — `docs/tasks/<ID>.verify/<round>.md`, template
`docs/tasks/_verify-template.md`, **append-only** (a re-run or a correction is the next
round):

```yaml
---
task: T-042
round: 1
kind: ui                    # api | ui | smoke | manual
tool: playwright            # required for kind ui: the configured runner's value - playwright,
                            # patrol, flutter-integration-test, maestro, espresso, xcuitest
command: npx playwright test e2e/login.spec.ts
environment: staging        # local | staging | production-like (never a hostname)
commit: 3f2c1ab             # the revision that was verified
result: PASS                # PASS | FAIL
evidence: playwright-report/   # report dir, junit xml, trace, log, or a CI artifact URL
verifier: task-validator    # writer's agent_type (main: top-level session), or human:<name>
---
## Scenarios checked
- Happy path: ...
- Failure branch: <the failure branch from the task's Acceptance> -> <observed>
```

The gate checks that a UI record names the driver that ran it and points at its
artifact; it does not prescribe the tool. The runner is chosen per project
(`tools.ui_verify`, [tools.md](tools.md)): Playwright for web by default; for mobile,
whatever drives the real app (Patrol, Flutter integration tests, Maestro, Espresso, XCUITest).

**Task fields** (`docs/tasks/_task-template.md`): `surface: ui | api | both | none`
(what the running system exposes), `verify_exception: "<reason>"` (the explicit
exception, see below), `design_ref` (the spec or plan the task implements;
may be empty).

**What it blocks**

1. Writing a verify record that is not the next round, and any edit or overwrite of an
   existing round.
2. A record with a missing or invalid field, a `commit` that does not resolve, a local
   `evidence` path that does not exist, no `Failure branch:` line in the body,
   `verifier` not matching the writer's `agent_type`, or a `human:` marker written by an
   agent. Every accepted round is written to `.teamwright/sessions/<ID>.verify.jsonl`
   (who, kind, result, commit).
3. `docs/tasks/<ID>.md` entering `VERIFIED` or `DONE` unless:
   - the task has a valid `surface`, and the latest record of the kind it needs is
     `PASS`: `ui` → a `ui` record; `api` → an `api` record; `both` → one `api` **and**
     one `ui` record; `none` → nothing when `deploys: false`, otherwise `api`, `ui` or
     `smoke`;
   - the latest record overall is not `FAIL` — a failed run sends the task to
     `VERIFY_FAIL` and back to `ANALYSIS`;
   - each counted record is fresh: its `commit` is an ancestor of (or equal to) `HEAD`
     and contains the task's last code commit — the newest commit whose message names
     the task id and that touches files other than process records (`docs/tasks/`,
     `docs/sprints/`, `docs/process-reviews/`, `docs/decisions.md`,
     `docs/open-questions.md` — a decision naming the task is not a code change). If no
     commit names the task, the record must verify `HEAD` itself;
   - each counted record matches its ledger entry (kind, result, commit) — or, when this
     checkout has no verify ledger for the task at all, its committed stamp `<n>.gate.json`
     (*Gate stamps*) — or is a
     person's committed record (`verifier: human:<name>`, no ledger entry).
4. `kind: manual` without `verify_exception` in the task (at write time and at the
   transition); with the exception, at least one fresh manual `PASS` must come from an
   agent that did not implement the task (`.teamwright/sessions/<ID>.dev`,
   `implemented_by`) or from a person.
5. Shell commands that write verify records.
6. Acceptance verdicts are validation records, `docs/tasks/<ID>.validation/<n>.md`
   (`verdict: PASS | FAIL | NEEDS_OWNER`, `commit:`, `validator: task-validator`,
   template `docs/tasks/_validation-template.md`): written only by `task-validator`,
   only with Edit/Write, append-only (an edit or overwrite of an existing verify or
   acceptance round is denied in every mode, `gate-first` too: an edited record keeps
   no stamp), next round only, commit must resolve; ledger
   `.teamwright/sessions/<ID>.validation.jsonl`. Entering `DONE` needs the latest round
   `PASS`, matching its ledger entry (or, with no validation ledger for the task, its committed stamp), for the task's current code (same freshness rule as
   verify records) — for every `surface`, including `deploys: false` + `surface: none`,
   where it is the only check. A round whose file is gone while the ledger has it blocks
   `DONE` (`TEAMWRIGHT_VALIDATOR_AGENT`, default `task-validator`). After two `FAIL`
   rounds in a row the next round must be `method: sweep` (the catalog
   `docs/tasks/<ID>.mutants/` re-run, then every condition, guard, return and write on
   the task's changed lines mutated) or `verdict: NEEDS_OWNER`: sampled mutations find
   one survivor per round, so a third sample does not end the loop — the same stop rule
   as two `REQUEST_CHANGES` in review. After two **sweep** rounds with `FAIL` (since the
   last `PASS` or `NEEDS_OWNER` round) the next verdict must be `NEEDS_OWNER`, not a third
   sweep: each sweep finds new survivors on the same code. See
   [Mutation checks](#mutation-checks-tw-mutants).

**Deliberate exits** (all stay visible in the task file)

| Situation | Exit |
|-----------|------|
| Pure internal, library or docs work with nothing to run | `surface: none` + `deploys: false` |
| The behaviour cannot be driven by an API or UI test (hardware, a third party with no sandbox) | `verify_exception: "<why>"` in the task, reviewed like code; then `kind: manual` records, checked by someone other than the implementer |
| A person verified outside Claude Code | They commit the record with `verifier: human:<name>` |

An exception used on more than a few tasks means a missing test harness: record it in
`docs/failures.md` and build the harness.

Configuration: none; the journal adds a line per landed record to
`.teamwright/logs/verify.jsonl`.

## Journal

`scripts/hooks/journal.sh` · facts, never a decision

**Incident it prevents.** Process metrics and the review stage were built on what the
agent said it did. A skipped review, a tool cut off by permissions for weeks, and
sessions that ended without an outcome were all invisible, because the only record was
the agent's report.

| Event | File (`.teamwright/logs/`) | One line per |
|---|---|---|
| `PreToolUse`, `PostToolUse`, `PostToolUseFailure` | `tools.jsonl` | tool call: `ts`, `event` (`pre`/`post`/`failure`), `session_id`, `agent_id`, `agent_type`, `tool`, `detail`; Agent calls add `subagent_type`, `task_ids` (ids in the spawn description, else the first id in the prompt) |
| `PermissionRequest`, `PermissionDenied` | `perms.jsonl` | permission decision needed / call denied (`PermissionDenied` fires for auto-mode denials; a deny-rule refusal shows as a `pre` line with no `post`) |
| `PostToolUse` of Edit/Write on `docs/tasks/<ID>.verify/<n>.md` | `verify.jsonl` | verify record landed: task, round, kind, tool, environment, result, commit (never the command or the body) |
| `Stop`, `SubagentStop` | `outcomes.jsonl` | session or subagent end: `outcome` from the final message's `Outcome: barrier\|idle\|escalate\|interrupted` line — `last_assistant_message`, else the last assistant text in the transcript (`none` if missing; `outcome_source`: message / transcript / none); a `Stop` without the line while an agent the session launched has not finished is `waiting` (`outcome_source: open-agent`), and so is a `Stop` whose final message ends on the flow's `waiting for <role> on <ID>` line (`outcome_source: waiting-line`) — active task and its status, the agent's tool-call count; `stage` from a `Stage: analyze\|red-tests\|implement\|review\|verify\|flow` line, and `batch` / `batch_reason` from `Batch: <ids> - <why>` (the tasks the session chose from the router's candidates); `tokens` — model usage of the agent's transcript so far, per model (`input`, `output`, `cache_read`, `cache_write`; cumulative; `Stop`: the session's own lines from `transcript_path`, `SubagentStop`: `agent_transcript_path`; `{}` without a path), read incrementally with the offset kept under `.teamwright/logs/.tokens/` |
| any gate warning or denial | `gates.jsonl` | `event: gate`, `gate`, `decision` (`warn` / `deny`), session, agent, tool, first line of the reason — written by the gates themselves (`_hookio.deny`) |

`detail` classifies a call, it never replays it: program + subcommand of a shell command
(no arguments, no `VAR=value`), a repo-relative path, an agent type, a skill name, a URL
host. File contents, prompts, arguments, error texts and denial reasons are not stored.
The hook prints nothing and always exits 0. Directory: `$TEAMWRIGHT_LOG_DIR`.

## Deny list

`.claude/settings.json` → `permissions.deny` · T3

**Incident it prevents.** An agent "rolled back" its experiment with `git stash` /
`git checkout --` and wiped another session's uncommitted work in the same tree —
including tests written that day. Force-push and `reset --hard` do the same to shared
history.

Denied: `git checkout -- …`, `git checkout .`, `git restore`, `git stash`, `git clean`,
`git reset --hard`, `git push --force` / `--force-with-lease` / `-f` (flag before or
after the remote), `rm -rf` / `rm -fr`. Deny rules win over every allow rule and over
hook decisions. Experiments that need a clean tree run in a copy or a separate
worktree. A person who needs one of these commands runs it in their own shell.

Limits: rules match the command text after Claude Code splits compound commands, not
the program in every form (`sh -c`, a script, an absolute path), and `git checkout
<file>` without `--` cannot be told apart from switching branches. The list stops the
usual form of an accident, not a determined bypass.

## Patch guard

`scripts/hooks/patch-guard.sh` (called from `commit-msg`) · T2 (warns by default; `PG_MODE=block` is opt-in)

**Incident it prevents.** A recurring defect class was "fixed" by adding one more
recogniser regex or allowlist entry each time. One author honestly wrote "replaced the
allowlist with an invariant" while the diff added another list of phrases. Self-report
does not catch sincere mistakes; a mechanical check does.

**What it does.** Scans added lines of the staged diff (tests, docs and data files
excluded, and files the commit creates — a list in a new file is its content, not one
more case) for *patch signatures*: recogniser/allowlist constants (`*_RE =`,
`*_ALLOWLIST =` …) and bare string list items. If the file's area already has
`PG_PRIOR_FIXES` (2) or more commits from the last `PG_LOOKBACK_DAYS` (90; `0` = whole
history) whose subject looks like a fix, it prints a pointed warning; everywhere else a
short one.

Defaults are deliberately mild: on an existing codebase almost every area has fix
commits, and a guard that blocks everything is switched off within a week.

- `PG_MODE=warn` by default. **`PG_MODE=block` is opt-in**: turn it on once the
  signatures are tuned to your stack — and not while adopting a legacy codebase.
- The `literal-branch` signature (`if x == "foo"`) is off by default because it fires
  on ordinary code; enable it with `PG_SIG_LITERAL_BRANCH` (see `patch-guard.conf`).
- Area = the file's directory (`PG_AREA=dir`, default) or the file (`PG_AREA=file`). A
  file at the repository root is always its own area, never `.` (the whole repository).
  The metrics tool uses the file as its default area (`--area file`, see
  [metrics.md](metrics.md#1-git-history-works-on-any-repository)), and the two defaults differ on purpose: the
  guard asks "was this neighbourhood patched recently?", where a directory catches a
  fix in a sibling file, and a warning costs little; a recurrence *rate* over a whole
  history needs a narrow area, because almost every directory of a large codebase has a
  fix in the previous month.

**Deliberate overrides** (all stay in history):

- Trailer `RCA: T-050` on its own line, naming an existing `type: rca` task whose
  analysis (`root_cause`, `evidence`, `decision`) is filled. A task id mentioned
  anywhere else in the message does not count.
- `Principle-Override: P-001 - <why this is not a patch>` (parsing, validation, data).
- One-off, human only: `PATCH_GUARD=off git commit ...`

Tuning: copy `scripts/hooks/patch-guard.conf` to `.teamwright/patch-guard.conf` and
edit `PG_SIGNATURES`, `PG_EXCLUDE_RE`, `PG_PRIOR_FIXES`, `PG_LOOKBACK_DAYS`, `PG_MODE`,
`PG_AREA`, `PG_FIX_SUBJECT_RE` (the subject pattern that makes a commit "a fix").
Noisy signatures teach people to ignore the guard — trim them to your stack.

The same hook prints a note when a task status changes without a
`chore(task): T-042 FROM→TO` line (metrics parse that form).

## DRAIN and pause

`scripts/hooks/pre-push` · T3

**Incidents it prevents.** (1) Work pushed before its review: the reviewer then reads a
diff that is already on the shared branch, and a rejection means a revert. Blocking only
`NEEDS_REVIEW` was not enough: code sent back after `REQUEST_CHANGES` (task
`IN_PROGRESS` again) and patches left in `DEFERRED` tasks still shipped. (2) An
operator paused the team, but a self-continuing session pushed in the seconds before it
was stopped. Asking a model to stop is racy; a hook is not.

**What it blocks**

- Any push while `TEAMWRIGHT_PAUSE` exists at the repo root (or `$TEAMWRIGHT_PAUSE_FILE`),
  `wip/*` branches included.
- A push whose unpushed commits carry code of a task that is not approved for delivery
  at the pushed revision. Approved: `CODE_COMPLETE`, `DEPLOYED`, `VERIFIED`, `DONE`
  (`$TEAMWRIGHT_PUSH_STATUSES`). Everything else — `TODO`, `ANALYSIS`, `IN_PROGRESS`,
  `NEEDS_REVIEW`, `DEFERRED`, `BLOCKED`, `VERIFY_FAIL` — blocks, unless the task
  carries `tail_code: ratified`.

**Attribution** (one owner per commit):

- A commit that touches only `docs/**`, the spec tool's directories (`specs/**`,
  `openspec/**`) or the root process documents (`PRINCIPLES.md`, `AGENTS.md`,
  `CLAUDE.md`) — task and review records, specs, decisions, a process review from
  `/teamwright:improve` — belongs to no task: it
  ships no code, so opening a `TODO` task or recording a decision must not dirty the
  tail (`$TEAMWRIGHT_DOC_PATHS_RE`).
- Otherwise the owner is the first task id in the message that has a
  `docs/tasks/<ID>.md`; failing that, the first task record the commit changes. Ids
  without a record are skipped; a commit with no known id is not attributed.

**WIP branches — CI only, not a delivery.** A push to a remote branch matching
`$TEAMWRIGHT_WIP_REFS_RE` (default `^refs/heads/wip/`) skips the DRAIN check and changes
no status: it runs CI for tests the host cannot run (`areas[].ci_workflow`), so red and
green are seen before review. Commits that exist on the remote only in wip branches are
still undelivered: the DRAIN range of a later push ignores wip branches.

**Deliberate exits:** `tail_code: ratified` in the task record (owner's decision, stays
in history); human, one-off: `TEAMWRIGHT_ALLOW_PUSH=1 git push`.

## WIP push guard

`scripts/hooks/wip-push-guard.sh`, `scripts/tw-wip-push`, `scripts/tw-launch` · T3, best effort · project addition (T-192, decisions #219), not a kit gate

**Incident it prevents.** A headless stage session in a worktree must make the CI-only `wip/<ID>` push of P-016, but the kit's launch denies `git push` and tells the session not to push. A plain allow rule cannot override that deny, and a wildcard rule for the push also matches extra refspecs and `git -C <other repo>`. Only a program can pin source and destination.

**The sanctioned push.** `scripts/tw-wip-push [<commit>]`, from a worktree on branch `tw/<ID>`: it writes `refs/heads/wip/<ID>` on `origin` and no other ref (refspec `+<commit>:refs/heads/wip/<ID>`, so wip may move back). `<ID>` comes from the branch, never from an argument. The commit (default `HEAD`) must descend from the fork point from `main`. It refuses, with no ref moved: any other branch (the main-tree orchestrator keeps the plain `git push origin HEAD:wip/<ID>`, #202), a detached HEAD, more than one argument, an option or refspec as the argument, a foreign commit, and a `TEAMWRIGHT_PAUSE` in the worktree or the main tree. The `pre-push` hook still runs; `TEAMWRIGHT_ALLOW_PUSH` is not honoured. `.claude/settings.json` allows `Bash(scripts/tw-wip-push)` and `Bash(scripts/tw-wip-push *)` and no other push.

**Launch.** Parallel stage sessions are started with `scripts/tw-launch <ID>...`, which runs the kit's `launch` with `TEAMWRIGHT_CLAUDE_ARGS` set to the kit's defaults, denials kept, and the one prompt sentence changed to allow only `scripts/tw-wip-push`. It reads the arguments from the installed kit and refuses to start if the kit's sentence or a denial is gone. A launch through the kit's own `launch` keeps "Do not push" and the session stops at the push as before.

**Hook refuses** a Bash command whose raw text holds one token, at any occurrence: the word `git`, blanks or a backslash-newline, a dash, then on the same logical line the word `push` (T-198, P-020; the regex is `analysis.decides_on` of T-198 and lives only in the hook). No quote, separator, wrapper or heredoc is read, so a wrapper never named needs no clause: `setsid`, `flock`, `coproc`, a function, `<<<`, a pipe into `sh`, `env -S`, `find -exec` and `git -c alias.p=push p` are all refused. So is text that only mentions the token (`git -C . stash push`, `commit -m "do not push"`, an `echo`, a heredoc body, a `grep`, a chain such as `git -C . status; git push origin HEAD:wip/T-1`): split the call, or use `git commit -F` or the Write tool. Measured on 11250 past Bash calls, 19 (0.17%) were such new refusals (T-198 › Investigation). The settings also deny `Bash(git -C * push*)` and `Bash(git -c * push*)`. A refusal blocks in every gate mode; an internal error fails open.

**Hook passes** `scripts/tw-wip-push`; a plain `git push`, which is left to the push denies and `pre-push`; the word inside another (`--push`, `pre-push`, `docs/push.md`, `pushed`); and a real newline after `git` or before `push`. Passing the hook does not mean a command runs: the settings denies are a separate layer and refuse some of the same text.

**Limits.** A deny is no boundary: the hook is a best-effort match on the command text. What the rule gives up, because refusing it takes a reader of quotes or variables, or another word in the token:

- **The class: any spelling whose text does not hold the token literally**, because a quote, a backslash or an expansion sits in or before the word `git`, the dash or the word `push`, or between them in place of a blank. Quotes and backslashes: `"git" -C . push`, `'git' -C . push`, `g""it -C . push`, `g\it -C . push`, `git "-C" . push`, `git '-C' . push`, `git \-C . push`, `git -C . p\ush`, `git -C . pu""sh`, `git -C . pu's'h`. Expansions, a command built at run time: `git $'-C' . push`, `git${IFS}-C . push`, `git -C . ${X:-push}`, `git $OPT . push`, `X=push; git -C . $X`, `G=git; $G -C . push`. These are samples, the limit is the class (T-198 review 1 #1; the orchestrator's decision covers it, decisions #226).
- **Its worst case: the quoted `core.hooksPath` spelling**, `git "-c" core.hooksPath=/dev/null push` and `"git" -c core.hooksPath=/dev/null push`. It passes this hook and also drops `pre-push`, the hook this guard exists to keep. The settings deny `Bash(git -c * push*)` does not literally match it either (the real matcher was not probed on it).
- What the shell model this rule replaced did: it refused the quote and backslash spellings, read from its code and not run (T-198 review 1 #1), so those are refusals given up; the expansions passed both. Refusing either takes a reader of quotes or variables, which P-020 forbids.
- `git -C . send-pack origin main`: no word `push`.
- A script file that pushes, or a program that passes git its arguments apart: the token is not in the command text.

`git push --no-verify` and a `core.hooksPath` override that reaches git as above remain outside every gate here; CI repeats the checks (Known limitations).

**Deliberate exits:** a human, one-off: `TEAMWRIGHT_ALLOW_PUSH=1 git push` (DRAIN); the hook has no override, remove its entry from `.claude/settings.json` (owner). `make check-wip-push` pins the script, the hook and the settings (`docs/decisions/ci-toolchain.md` › Guard table).

## Secret scan

`scripts/hooks/pre-commit`, `.gitleaks.toml` · T3 (CI only if the project adds it)

**Incident it prevents.** Agents paste config values, tokens seen in logs, or local
settings files into commits. Once pushed, a secret has to be rotated, not deleted.

**What it blocks**

- Files `.env`, `.env.*` (except `.example` / `.sample` / `.template`), `*.local.json`.
- Secrets in added lines: `gitleaks` with `.gitleaks.toml` when installed, otherwise a
  built-in fallback (private key headers, common cloud/VCS/chat/LLM token formats,
  JWTs, credentials in URLs, `PASSWORD=` / `TOKEN=` / `API_KEY=` with a literal value;
  placeholders like `<...>`, `${VAR}`, `changeme` pass).
- Fallback findings are classified, and only **secret-like** ones block. The others are
  printed as notes: `test-fixture` (test directories and `*_test.*` / `*.spec.*` files),
  `docs-example` (Markdown, `docs/`, `examples/`, README) and `config-placeholder`
  (`*.example` / `*.sample` / compose files, local-dev values such as `localhost` or
  `postgres:postgres`). A known token format (private key, cloud, VCS, chat, LLM, bot
  token) is secret-like wherever it is, unless the line marks it as an example or fake.
  Why: a first scan of a real project gave 93 findings and 0 real secrets; a scan that
  cries wolf gets bypassed. gitleaks, when installed, applies its own rules unchanged.
  `python3 <kit>/scripts/tw-install.py scan` reports the same classes with a summary and fails only on
  secret-like findings.

**Gate stamps.** The same hook refuses a staged new or changed stamp
(`docs/tasks/<ID>.<reviews|verify|validation>/<n>.gate.json`) that matches no line of the
task's local ledger (`.teamwright/sessions/`: same round, writer, verdict), where that
ledger exists — the gate writes both together, so a stamp without its line was not
written by the gate on this machine. Without the ledger (a fresh clone) it is not
checked (*Gate stamps*).

**False positive:** add `teamwright:allow-secret` (or `gitleaks:allow`) to the line.
**Deliberate bypass** (human, rare): `git commit --no-verify` — and say why in review.
The kit installs no CI workflow. `--no-verify` skips the hook, so a project that wants a
backstop adds the scan to its own CI, for example a `make scan` target that runs
`gitleaks git --config .gitleaks.toml --redact .` over the full history.

## Guard trigger

`scripts/hooks/pre-commit` §4 (`guard_trigger`), self-test `scripts/ci/guard-trigger.test.sh` (`make check-guard-trigger`) · T3 (local project addition, not a kit gate; PRINCIPLES.md P-020, decisions #154, #155, T-113)

**Incident it prevents.** A new guard is designed as a model of the grammar of a tool that interprets the text (F-003, F-015, F-022), because the P-020 design step (`checker: true` and `analysis.decides_on`) fires only if the analysis author remembers it.

**What it refuses.** A commit that stages a guard change while the active task lacks `checker: true` or a non-empty, non-placeholder `analysis.decides_on` (a top-level `decides_on` or a `checker:` line in the body does not count; a missing `checker:` key counts as false). Guard change (decided on raw staged paths from `git diff --cached --name-only --no-renames -z`, any status, no Make or shell parsing; decision #155): a path under `scripts/ci/` or `scripts/licenses/`, the path `Makefile`, or a path ending in `.mk`. The refusal lists every guard path, including the old path of a rename. Over-inclusion is accepted: a change to `Makefile` or `scripts/ci/` unrelated to any check also needs `checker: true` and `decides_on`. The active task is `$TEAMWRIGHT_TASK`, else the first word of `.teamwright/current-task`; no active task or no record refuses too. Commits without a guard change are not affected.

**Exit.** Set `checker: true` in the task's front matter and fill `analysis.decides_on` (`switch | literal | token`, naming the switch, owned literal or raw token) before committing the guard; copy it into the `decides_on` column of the guard table in `docs/decisions/ci-toolchain.md`. If the commit is not a guard change, fix the task id in `.teamwright/current-task`. `git commit --no-verify` skips it (see Overriding deliberately); say why in review.

**Not caught.** A new root `GNUmakefile` or `makefile` (GNU make reads them before `Makefile`; the token is not widened, decision #155). `scripts/hooks/pre-commit` itself and `scripts/hooks/_task.py` (the shared task parser that reads `checker` and `analysis.decides_on` for the trigger) are not guard paths under the token; `make check-guard-trigger` runs their fixtures, but an edit of either alone does not need `checker: true`. Check code added outside the four path forms is not caught either; the guard table in `docs/decisions/ci-toolchain.md` lists the guard paths, and whether `scripts/hooks/` belongs in the token is OQ-71.

## CLAUDE.md budget

`scripts/hooks/claude-md-budget.sh` · T2 (warns, never blocks)

**Incident it prevents.** A context file that grew to hold every decision was loaded
into every session before any question was asked (P-014).

On `SessionStart` it adds a note to the session's context, and in `pre-commit` it
prints a warning, when `CLAUDE.md` exceeds `TEAMWRIGHT_CLAUDE_MD_MAX_KB` (default 40,
set in `.claude/settings.json` → `env`). Fix: move decisions to
`docs/decisions/<area>.md`, verbatim, in a separate commit.

## Session check

`scripts/hooks/session-check.sh` · T2 (reports, never blocks)

**Incident it prevents.** A repository cloned on a second machine had the gates and
the tasks but no plugin and no git hooks: `/teamwright:flow` was unknown, commits ran
without the secret scan, and nothing said why.

On `SessionStart` it checks that `core.hooksPath` is `scripts/hooks` and that the plugin
recorded in `.teamwright/installed.json` (`@kit`) is installed for this project at that
version (Claude Code's plugin registry; a registry it cannot read is skipped). Anything
missing goes to the owner (`systemMessage`) and to the agent's context, with the
commands that fix it. `TEAMWRIGHT_SESSION_CHECK=0` turns it off.

---

## Gate runs (tw-gate)

`scripts/tw-gate <area>|--top [--fresh]`, `--for-task <ID>`, `--for-diff <base>`,
`--stop <area>|--top` · T1 (tooling: nothing refuses a role that skips it)

**Incidents it prevents.** One small task took about 90 minutes in an autonomous run, and
the area's full gate (about 10 minutes) ran five times on the same code: the developer
twice (it cut the output with `tail -8` and could not see the result), the flow's Gate
step, the code-reviewer and the task-validator. In another project the gate ran five
times for one task, three of them on docs-only changes that invalidated a whole-tree
cache (10–14 minutes each); a validator ran the server suite for a CI-only change; and a
gate over Claude Code's 10-minute Bash limit was cut, its test container killed and the
run started over, or polled in `until grep …; sleep` loops.

**What it runs.** Exactly the gate the role files name, from `.teamwright/config.yml`:
the area's `generate_command` first when it has one, then the area's `gate_command`,
both through `scripts/tw-run <area> -- ...`; an area without its own `gate_command` runs
the top-level `gate_command` as written, from the project root.

**What it keeps.** The whole output in `<logs>/gate/<area>-<time>-<commit>.log`
(`<logs>` = `$TEAMWRIGHT_LOG_DIR` or `.teamwright/logs/`, gitignored); the last run in
`<logs>/gate/<area>.json` (`_top.json` for `--top`: area, root, head, clean, tree,
tree_after, key, key_after, scope, exit, pass, cacheable, duration_s, started, finished,
log, command); every run and every cached answer appended to `<logs>/gate/runs.jsonl`.
On screen: `PASS` or `FAIL`, exit code, duration, the last 30 lines of a failure, and the
log path. Exit code = the gate's (`generate_command`'s when that failed); 75 while it is
still running (below); 2 for a usage or config error.

**Detached run, bounded wait.** Claude Code stops a Bash call after 10 minutes, so the
run itself lives in its own session and process group (`scripts/hooks/_job.sh`; Python's
`start_new_session`, which works on macOS too) and writes to its log; the call waits up
to `TW_WAIT_SECONDS` (default 540). Finished in time: the normal result and exit code.
Not yet: `RUNNING: gate <area> (pid N), started <when>, log <path> - run the same command
again to keep waiting`, exit **75**. The same command again attaches to that run — same
area, same content key, same command — instead of starting another, and waits again; a
result that finished while nobody waited is printed by the next same call (once: a FAIL
is then never reused). A run of the same area on other content is waited for first (the
toolchain, containers and build directories are shared). `--fresh` never starts a second
run next to a running one: it attaches to it. A run whose process is gone without a
result (killed) is reported, cleaned up and started again; a killed or interrupted
caller leaves the run going; `--stop` sends TERM to its process group. Jobs live in
`<logs>/gate/jobs/`. Roles never wrap tw-gate in sleep loops or background calls.

**When it does not rerun.** The cache key is the content of the area's files: the
working tree as git sees it (HEAD plus every tracked and untracked change, ignored files
left out — a temporary index, `git add -A`, `git write-tree`), restricted to the area's
scope — its `source_dirs`, `test_dirs` and `workdir`; the dependency manifests and
lockfiles at the project root and the workdir (Makefile, justfile, package.json and the
lockfiles, pyproject.toml, Cargo.toml/.lock, go.mod/.sum, pubspec.yaml/.lock, Gradle and
Maven files, Gemfile, composer.json, Dockerfile, compose files, ...);
`.teamwright/config.yml`; the gate scripts (`tw-gate`, `tw-run`, `hooks/_config.py`,
`hooks/_job.sh`); and every repository file the area's `generate_command` or
`gate_command` names (`sh ../tools/check.sh` adds `tools/check.sh`). An area without
`source_dirs` and `test_dirs`, an area that runs the top-level gate, and `--top` key on
the whole tree, as before. When the area's last record is a PASS for the same key, root
and command, and the key did not change while it ran, tw-gate prints
`PASS (cached: same commit <sha>, <when>, log <path>)`, `same tree … as the run at
commit …` (changes not committed yet) or `<area> files unchanged since the run at commit
…` (only files outside the scope changed — docs, task records, another area) and exits 0.
So the developer's PASS before the commit answers the flow, the reviewer and the
validator, and a docs commit after it does not cost another run. Never reused: a FAIL, a
run during which the area's files changed (the gate or `generate_command` wrote files, or
someone edited meanwhile — tw-gate warns when the tree changed), another command, and
anything after `--fresh`.

**Which gates a task needs.** `scripts/tw-gate --for-task <ID>` (the files of the
commits whose message names the task, plus uncommitted and untracked ones) or
`--for-diff <base>` prints one line per gate: each area whose `source_dirs`, `test_dirs`
or `workdir` holds a changed file, and `--top` when a file lies outside every area (every
area's name instead when there is no top-level `gate_command`). Process records, `docs/`,
Markdown, CI configuration (`.github/`, `.gitlab-ci.yml`, `.circleci/`, ...) and tool
settings (`.claude/`, `.teamwright/`, `.serena/`, `.mcp.json`, licence and git
metadata files) outside the areas need no local gate — CI checks its own configuration;
`$TEAMWRIGHT_NO_GATE_RE` replaces that pattern. Nothing printed: no suite runs. The
reviewer, the validator and the flow's Gate step run exactly the listed gates.

A cached PASS is the tool's own record of a run on the same content, not an agent's
report; the reviewer and the validator still compare `git status` before and after, and
their verdict stays theirs. Files the gate depends on but git ignores (build caches,
downloaded dependencies) are outside the key — `--fresh` when they matter.

## Test guard

`scripts/hooks/test-guard.sh` (Claude Code `PreToolUse`, Bash) and `scripts/tw-test` · T3

**Incident it prevents.** In one project's autonomous run the test-writer, after fixing
one test, ran the same ~4-minute suite four times in a row: twice through
`… pytest -q -n 1 … tests/pipeline 2>&1 | grep -E "passed|failed" | tail -15` (the
failures were cut off and lost), once with its output in a file but with
`-p no:logging`, which removed the `caplog` fixture and produced 25 false errors, and
once right — about 12 minutes lost, and with one worker instead of the configured
`PYTEST_WORKERS`. The developer ran the project's gate script directly twice. The role
files already said "never cut output, run long commands once with output to a file";
agents ignored it, so the tooling enforces it.

**The rule.** Tests run only through `scripts/tw-test`, gates through `scripts/tw-gate`,
mutants through `scripts/tw-mutants`; lint, build and code generation through
`scripts/tw-run <area> -- <cmd>`.

**What the hook refuses** (one message naming the command to run instead and saying to
read the log it prints):

1. A test runner or gate called directly: `pytest`, `python -m pytest|unittest`,
   `make test|tests|check|test-*`, `flutter|dart|deno|bun|go test`, `cargo test|nextest`,
   `npm|pnpm|yarn [run] test[:*]`, `vitest`, `jest`, `phpunit`, `pest`,
   `php artisan test`, `gradle(w) test|check`, `mvn test` — also behind `uv run`,
   `poetry run`, `npx`, `pnpm exec` and the like, inside `sh -c '…'`, `eval`,
   `docker run|exec`, a wrapper script's arguments, and inside
   `scripts/tw-run <area> -- <cmd>`; plus every command the config names: each area's
   `gate_command`, `test_command` and `test_select_command` and the top-level
   `gate_command` (a repository script such as `tool/gate.sh` by its name, a
   `make check`-style command by its first words), and the regular expressions in the
   top-level `test_runner_patterns: [...]`.
2. Test or gate output — the wrappers' own included — piped into `grep`, `tail`, `head`,
   `sed`, `awk`, `cut`, `wc`, `sort`, `less`: the wrappers already print a summary and
   keep the whole output in a log. Reading that log with `grep` or `sed -n` is fine.

**What it allows.** The wrappers; `scripts/tw-run <area> -- <cmd>` for anything that is
not a test run, and for the area's own `lint_command`, `generate_command`,
`api_test_command` and `e2e_ui_command` (verification against a running service runs as
rendered); everything else. The wrappers' internal runs go through `scripts/tw-run`
inside the script, never through the Bash tool, so the hook never sees them. The command
is parsed (quotes, pipes, `&&`, heredocs), not grepped: `git commit -m "fix pytest"` or
`echo pytest` pass.

**Mode — its own switch.** `gate-first` only warns, and this guard exists to stop
agents, so it does not follow `mode:`. `process.test_guard: block | warn | off` in
`.teamwright/config.yml` (default `block`, also in `gate-first`); `TW_TEST_GUARD=block|warn|off`
in the environment Claude Code is started with wins — the human escape. An assignment
inside the agent's command (`TW_TEST_GUARD=off pytest`) does not reach the hook, and
agents cannot edit `config.yml` (state guard).

**`scripts/tw-test <area> [selector ...] [--fresh] [--tree <copy>]`**, `--stop <area>`:

- **Command:** the area's `test_select_command` with `{tests}` replaced by the selectors
  (test files, node ids, filters, shell-quoted), e.g.
  `test_select_command: "pytest -q -n ${PYTEST_WORKERS:-4} {tests}"`; without it the
  selectors are appended to `test_command`; with none, `test_command` as written. Workers
  and other knobs stay in the config string; tw-test knows no test framework. It runs
  through `scripts/tw-run <area>` from the area's workdir, so selectors are relative to it.
- **Output:** the whole output in `<logs>/test/<area>-<time>-<commit>-<id>.log`; on
  screen PASS/FAIL, the exit code, the duration, the counts when the output is
  recognisable (pytest, cargo test, go test, flutter/dart test, jest, vitest), the failed
  tests with their first error line (at most 30), otherwise the last 15 lines, and the
  log path. Exit code = the test command's.
- **Cache:** keyed by the area's files (the same area-scoped key as tw-gate, plus files
  the test command names), the resolved command and the selectors, in
  `<logs>/test/cache/`. An identical repeat prints the cached summary and the log path —
  a FAIL too, unlike the gate: the answer to a failure is its log, not another run.
  `--fresh` runs anyway. Not reused: a run during which the files changed, exit 126/127
  (not runnable), a killed run. Every run and cached answer is appended to
  `<logs>/test/runs.jsonl`.
- **Long runs:** detached and waited for up to `TW_WAIT_SECONDS`, like tw-gate: `RUNNING: tests <area> … - run the same command again to keep waiting`, exit 75; the
  same command attaches. Another test run of the same area is waited for first.
- **`--tree <copy>`:** runs in a throwaway copy (`TEAMWRIGHT_ROOT=<copy>`,
  `TW_RUN_COPY=1`, the main checkout's caches) — the test-writer's wrong implementations
  and the validator's hand-run tests; logs stay in the project.

## Mutation checks (tw-mutants)

`scripts/tw-mutants <area> <ID> --tree <copy> [--check] [--only <name>]` · T1 (tooling);
the stop rules are the verify gate's (T3, check 6 above)

**Incident it prevents.** Mutation testing in acceptance had no stopping point: every
sweep wrote a new, larger list of mutants and found new survivors, and any survivor
failed the task. One task failed acceptance four times, each time on a new gap in one CI
step; one sweep ran about 109 mutants × 40 s, over an hour. Each mutant also started the
area's runner (a container) again, which cost more than its tests.

**The rules** (the kit's default for every project; a project narrows them in its own
role files and records why in `docs/decisions.md`). Mutants on the task's diff are
evidence for the reviewer and the validator, not a gate on every line — the practice at
Google, where mutants on changed lines are shown in code review as hints (Petrović,
Ivanković, "State of Mutation Testing at Google", ICSE-SEIP 2018; Petrović, Ivanković,
Fraser, Just, "Practical Mutation Testing at Scale: A view from Google", IEEE TSE 2021,
arXiv 2102.11378).

1. **Severity.** Acceptance fails only on a surviving mutant that breaks a stated
   Acceptance/DoD guarantee, and the mutant names that guarantee. Other survivors are
   follow-ups in the validation record, not a `FAIL`.
2. **Budget.** A `sample` round: 15–30 mutants aimed at the guarantees and the task's
   changed lines. A `sweep` (and the test-writer's sweep for `checker: true`): every
   condition, guard, return and write on the task's changed lines, not the whole seam.
   A whole-seam sweep only for a `critical: true` task — code whose failure loses or
   corrupts data (sync, recovery after a restore, data deletion).
3. **Catalog in the repository**, `docs/tasks/<ID>.mutants/`: one `<NN>-<slug>.mutant`
   file per mutant — `guarantee:` (`-` for none), `file:`, `added_by:`, optional `test:`
   and `equivalent: <why>`, then the exact replacement between `--- old`, `--- new` and
   `--- end` (the old text must occur exactly once) — and `test-command`, the command
   that runs only the task's tests. The test-writer starts it; each validation round
   re-runs it first and then only adds. It is a process record (like the rest of
   `docs/tasks/`), so committing it does not make a verdict stale.
4. **Stop.** Two `FAIL` rounds in a row → the next is a sweep or `NEEDS_OWNER`; two
   sweep rounds with `FAIL` → the next verdict is `NEEDS_OWNER` (accept the rest as
   follow-ups, or rethink the task). The verify gate refuses anything else.
5. **Machine mutation tools** (mutmut, PIT, Stryker, cargo-mutants) stay off the
   acceptance path: a project that wants them runs them in a scheduled CI job whose
   survivors become follow-ups.

**What the tool does.** `--tree` is a throwaway copy (a `git worktree`), never the
working tree — tw-mutants refuses the project root. It checks every entry against the
copy, writes the mutated files and one POSIX `sh` loop into `<copy>/.tw-mutants/`, and
runs the loop in **one** `scripts/tw-run <area>` start with the copy as the project root:
the task's tests once unmutated (the baseline must be green, otherwise every mutant
would look killed), then for each mutant put the mutated file in place, run its tests,
put the original back. For `runner: docker` the container mounts the copy, so the copy
must be on a path the Docker host can see: create it with `mktemp -d` (it honours
`$TMPDIR`), never a hardcoded `/tmp` inside an agent's container. Output: one line per
mutant (`killed`, `SURVIVED … breaks: <guarantee>`, `survived, follow-up`, `equivalent`)
and a summary, also kept in `<logs>/mutants/<ID>-<time>.txt`. Exit 0 when no survivor
breaks a guarantee, 1 when one does, 2 for a usage error or a catalog entry that no
longer matches the code, 3 for a red baseline, 4 when `generate_command` failed in the
copy, 75 while it is still running, the runner's code (e.g. 127) when it cannot start.
`--check` validates the catalog without running anything.

**A copy that does not start from cold.** A fresh worktree has no generated code
(ignored files) and an empty build directory. When the area has a `generate_command`:
with `generated: [paths]` (its outputs, relative to the project root) declared and the
copy's inputs equal to the working tree's — `generate_inputs: [paths]` when declared,
else the whole tree without ignored files, the outputs and `.tw-mutants/` — tw-mutants
copies the outputs from the working tree into the copy; otherwise `generate_command`
runs in the copy inside the same runner start, before the baseline (one project spent
~5 minutes of `build_runner` on every clean copy). Runs in the copy get `TW_RUN_COPY=1`,
which applies the area's `copy_env` (e.g. `CARGO_TARGET_DIR: "{cache}/target"`; `{cache}`
is `.teamwright/cache/<area>/copy` of the main checkout, mounted at `/tw-copy-cache` for
`runner: docker`), and `TW_CACHE_ROOT=<project root>`, so docker areas reuse the main
checkout's package caches. `scripts/tw-mutants <area> --tree <copy> --prepare` does only
this step, for a copy whose tests are run by hand. The catalog run is detached and waited
for like tw-gate (exit 75, the same command again attaches; another run of the same task
is waited for first); a copy with a running catalog is kept until it returns.

## Environment variables

Set by the person who starts Claude Code or runs git; agents cannot change the state
guard's variables. Unset means the default.

| Variable | Read by | Effect (default) |
|---|---|---|
| `TEAMWRIGHT_GATE_MODE` | Claude Code gates | `warn` or `enforce`; overrides `mode:` in `.teamwright/config.yml` |
| `TEAMWRIGHT_TASK` | analysis gate | the active task, instead of `.teamwright/current-task` |
| `TEAMWRIGHT_TASK_ID_RE` | hooks that find task ids in commits and prompts (`_task.py`: journal, patch guard, `pre-push`) | task id format (`[A-Z][A-Z0-9]*-\d+`: `T-042`, `API-7`) |
| `TEAMWRIGHT_ANALYSIS_FIELDS` | analysis gate | required analysis fields (`root_cause evidence invariant seam approach`) |
| `TEAMWRIGHT_RCA_FIELDS` | analysis gate, patch guard (the `RCA:` trailer's task) | required fields for `type: rca` (`root_cause evidence decision`) |
| `TEAMWRIGHT_CODE_STATUSES` | analysis gate | statuses of the active task in which code may be edited (`IN_PROGRESS`) |
| `TEAMWRIGHT_NONCODE_RE` | analysis gate | paths that are not code (`docs/`, `.claude/`, `.github/`, `.teamwright/`, top-level `*.md`) |
| `TEAMWRIGHT_REVIEWER_AGENT` | review gate | the reviewer's `agent_type` (`code-reviewer`) |
| `TEAMWRIGHT_REVIEW_NEW_TASKS` | review gate | new task records one review may create (`2`) |
| `TEAMWRIGHT_ESCALATE_AFTER` | review gate | `REQUEST_CHANGES` rounds in a row before the next record must escalate (`2`) |
| `TEAMWRIGHT_LOG_DIR` | journal, review gate, `scripts/tw-gate`, `scripts/tw-test`, `scripts/tw-mutants` | journal directory (`.teamwright/logs`); gate logs and records go to its `gate/` subdirectory, test logs to `test/`, mutant reports to `mutants/` |
| `TEAMWRIGHT_GATE_LOG` | Claude Code gates | a file that gets one plain line per warning or denial: time, gate, `WARN`/`DENY`, first line of the reason (off) |
| `TEAMWRIGHT_PAUSE_FILE` | `pre-push`, router | pause file (`TEAMWRIGHT_PAUSE` at the repository root) |
| `TEAMWRIGHT_PUSH_STATUSES` | `pre-push` | statuses whose code may be pushed (`CODE_COMPLETE DEPLOYED VERIFIED DONE`) |
| `TEAMWRIGHT_DOC_PATHS_RE` | `pre-push` | commits touching only these paths belong to no task (`^(docs\|specs\|openspec)/\|^(PRINCIPLES\|AGENTS\|CLAUDE)\.md$`) |
| `TEAMWRIGHT_WIP_REFS_RE` | `pre-push` | remote refs that are CI-only pushes, not deliveries (`^refs/heads/wip/`) |
| `TEAMWRIGHT_TASK_BRANCH_RE` | router, progress report | a branch that names a task worked in its own worktree, group 1 = the id (`^(?:tw\|task)/(<task id>)(?:$\|[-_/.])`) |
| `TEAMWRIGHT_ALLOW_PUSH=1` | `pre-push` | one-off human override of DRAIN |
| `PATCH_GUARD=off` | patch guard | one-off human override |
| `TEAMWRIGHT_CLAUDE_MD_MAX_KB` | CLAUDE.md budget | warning threshold in KB (`40`, set in `.claude/settings.json` → `env`) |
| `TEAMWRIGHT_SESSION_CHECK` | session check | `0` turns it off (on) |
| `TEAMWRIGHT_PLUGIN_DIR` | session check | the directory a plugin was loaded from (`--plugin-dir`, Agent SDK): taken as installed, its version (`plugin.json`, else the directory's git commit) compared with the recorded one (unset) |
| `TW_TEST_GUARD` | test guard | `block`, `warn` or `off`, set by the person who starts Claude Code; wins over `process.test_guard` |
| `TEAMWRIGHT_ROOT`, `TEAMWRIGHT_CONFIG` | `scripts/tw-run`, `scripts/tw-gate`, `scripts/tw-test`, `scripts/tw-mutants` | another project root or config file |
| `TW_WAIT_SECONDS`, `TW_POLL_SECONDS` | `scripts/tw-gate`, `scripts/tw-test`, `scripts/tw-mutants` | how long a call waits for the detached run (default 540; `0` = start or attach and return 75 at once) and how often it looks (default 0.2 s, then 1 s) |
| `TEAMWRIGHT_NO_GATE_RE` | `scripts/tw-gate --for-task`, `--for-diff` | files outside every area that need no local gate (records, docs, Markdown, CI and tool configuration by default) |
| `TW_RUN_COPY`, `TW_CACHE_ROOT` | `scripts/tw-run` (set by `scripts/tw-mutants`) | apply the area's `copy_env`; whose `.teamwright/cache` holds the runner's caches |
| `TEAMWRIGHT_OWNER=1` | installer (in the kit) | owner override outside Claude Code; ignored when `CLAUDECODE` is set |

## Gate stamps

The ledgers (`.teamwright/sessions/`) and the journal (`.teamwright/logs/`) are local and
gitignored. A session that writes a review, verify or acceptance record and a later
session that moves the task's status do not always share them: in an autonomous run each
job starts from a fresh checkout. A job cut after the reviewer's `APPROVE` (or the
validator's `PASS`) was committed but before the status move would then loop — the next
job's gate finds no ledger entry and refuses, the router sends the same stage again.

So when the review gate lets a review record through, and the verify gate a verify or
validation record, the gate also writes `<n>.gate.json` next to it: the writer's
`session_id`, `agent_id` and `agent_type`, the round, the verdict (verify: kind, result,
commit; validation: verdict, commit), for a review whether the reviewer's spawn was
journaled, the record it belongs to (`task`, `path`, `round`) and the sha256 of the
record as written. The ledger line and the stamp are written when the write has landed
(`PostToolUse`, with the record on disk equal to what the gate checked): a write the gate
allowed that another hook then refused, the person cancelled or that failed leaves
neither — no orphan stamp, no ledger round "written but gone" that would block the task.
A stamp without its record (from a pre-release of 0.12) is ignored and removed by the
verify gate. A stamp counts only for the record it names: copied with its record
into another task's directory or another round it does not count, and the gates also
check that a review or acceptance record's `task:` is the task it is counted for. The stage commits it with the record.
A status move on a checkout that has **no ledger of that kind for the task at all** (a
fresh clone) uses the stamp instead, when it matches the record on disk. Where the ledger
exists it alone decides, as in 0.11: a round it lacks is not recognised whatever its stamp
says — so do not restore `.teamwright/sessions` from a cache that may be older than the
last job (`docs/usage.md`, *Autonomous runs*).

What it keeps, what it does not:

- **The same checks.** The stamp says what the ledger says: written by a `code-reviewer`
  (`task-validator`) through the gate, after the write-time checks (not the implementer,
  the verdict rules, the findings threshold), with the spawn journaled. A record changed
  after its stamp does not match the hash and does not count; a stamp with
  `spawned: false` does not pass without the journal.
- **Not tamper-proof.** Only the gates write stamps: the state guard refuses Edit/Write
  to them and running a hook by hand, and the review gate refuses shell writes into the
  record directories (check 5, a best-effort parser). The stamps are not signed: anyone
  who can run a script that computes a sha256 and writes a file can make one, and the
  gates cannot tell. What limits that: on a checkout with the task's ledger the stamp is
  not consulted at all; the git `pre-commit` hook refuses a staged stamp that matches no
  line of the local ledger, where that ledger exists; a stamp names its task, record and
  round, so a copy does not count elsewhere; and stamps are in git — a forged one is a
  reviewable file in a diff, and `git log` says which commit brought it. On a fresh
  checkout (CI) the stamp is all the gate has: review what the jobs commit.
- **Records from before stamps.** A review, verify or acceptance record written by a kit
  before 0.12 has no stamp. On a checkout without the task's ledger it still counts when
  git shows it was last changed in a commit *before* the commit that moved
  `.teamwright/installed.json` from a pre-0.12 kit to a stamping one, and the working copy
  is that committed version (`_hookio.legacy_record`): the 0.11 gate checked it in the
  session that wrote it. A project installed at 0.12 or later has no such records.
- **The implementer check stays at write time.** The status move in a later session has no
  `.dev` ledger; whether the reviewer implemented the task was checked when the review was
  written. One stage per job also means the reviewer's session is never the implementer's.
- **The journal is still local.** Tool calls, outcomes and tokens per stage do not survive
  a fresh checkout; keep `.teamwright/logs` (and `.teamwright/sessions`) between jobs with a
  cache or a workspace that is not cleaned (`docs/usage.md`, *Autonomous runs*).

## Overriding deliberately

Every gate has an exit that leaves a trace (a field in the task file, a trailer in the
commit, an environment variable a human types). That is the point: an override is
allowed, a silent one is not. When an override is used more than occasionally, the
gate is wrong or the process is — record it in `docs/failures.md` and fix the cause.

## Known limitations

- Claude Code gates see Edit/Write and obvious shell rewrites; a determined script
  (e.g. Python writing the file from a script file, or a path assembled at run time)
  bypasses them. Git hooks and review are the backstop. The same holds for gate stamps
  on a fresh checkout, where no local ledger can contradict them (*Gate stamps*).
- Ledgers and the journal are local (`.teamwright/`, gitignored). The gate stamps committed
  with review, verify and validation records carry what a later status move needs
  (*Gate stamps*). A review, verify or acceptance record written before 0.12 has no stamp;
  on a checkout without its ledger it counts when it was last changed in a commit before
  the one that moved `installed.json` from a pre-0.12 kit to a stamping one, and the tree
  holds that version (*Gate stamps*). A record without a stamp committed after the upgrade
  is not recognised — record it as `reviewer: human:<name>` after a person checked it, or
  run the review where the work was done.
- One `.teamwright/current-task` per working tree; parallel sessions need worktrees.
- The state guard parses shell commands; a script file that writes
  `.teamwright/config.yml` without the command naming it, or a path built at run time,
  slips through. Review changes to that
  file like code (it is committed), and `tw-install.py` refuses lowering the mode.
- The review gate trusts `agent_type`: anyone who can edit `.claude/agents/` can name
  an agent `code-reviewer`. Review changes to agent definitions like code.
- The review gate's new-task count (check 7) attributes a new record by the journal's
  spawns, not by its content: a task the orchestrator creates after the review for an
  unrelated reason, before spawning another role, counts toward that review; a task
  created through the shell is counted when the reviewed task's status leaves
  `NEEDS_REVIEW` (git), not when it is written; the analysis gate refuses shell writes that
  create or replace a task record.
- `review: skipped:trivial` is declared in the task record by whoever edits it, the
  author included. It is visible to the validator, to later reviews of the history and
  to metrics, not prevented.
- Role limits are written rules, not hooks: the reviewer and the validator have
  Edit/Write and no hook stops them from editing code, and no hook stops an agent from
  editing `PRINCIPLES.md`. Review the diff of every role's work.
- `git commit --no-verify` / `git push --no-verify` skip all git hooks; CI must repeat
  the checks that matter (secret scan, tests).
- Patch-guard signatures are heuristics: expect false positives until tuned.
- The verify gate checks that a run was recorded, not that it was honest: `evidence` is
  checked for existence, not for content, and CI artifact URLs are not fetched. Review
  the record with the diff, and let CI produce the artifact where it can.
- Verify freshness relies on the commit convention (task id in every code commit).
  Uncommitted changes are invisible to it, and a commit that forgets the id is not
  counted; with no id at all the record must verify `HEAD`.
