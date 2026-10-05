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
| [Analysis gate](#analysis-gate) | Claude Code `PreToolUse` (Edit/Write/Bash) + `PostToolUse` | T3 | Coding before the root cause is known |
| [Review gate](#review-gate) | Claude Code `PreToolUse` (Edit/Write/Bash) | T3 | Self-review, skipped review, endless review loops, recurrence accepted as a patch |
| [Verify gate](#verify-gate) | Claude Code `PreToolUse` (Edit/Write/Bash) | T3 | "Done" that does not work in the running system |
| [Journal](#journal) | Claude Code tool, permission and stop events | — (facts) | Metrics and gates built on self-report |
| [Deny list](#deny-list) | `.claude/settings.json` permissions | T3 | Destroying uncommitted work or shared history |
| [Patch guard](#patch-guard) | git `commit-msg` | T2 (warns; blocking is opt-in) | "One more `if`" fixes in an area already fixed before |
| [DRAIN / pause](#drain-and-pause) | git `pre-push` | T3 | Unapproved work leaving the machine; pushes during a pause |
| [Secret scan](#secret-scan) | git `pre-commit` (CI only if the project adds it) | T3 | Tokens, keys, `.env`, local settings in history |
| [CLAUDE.md budget](#claudemd-budget) | Claude Code `SessionStart` + git `pre-commit` | T2 | A context file that grows into a second docs folder |

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
3. A shell command that rewrites a task status in `docs/tasks/` (`sed -i`, `perl -i`,
   `>`, `tee`) — status changes must go through Edit/Write so the gates see them.
   Appending (`>>`, `tee -a`) is allowed: it lands after the front matter, where `status:`
   is never read, so a note cannot change the task's state.

**Deliberate exits** (all stay visible in the task file)

| Situation | Exit |
|-----------|------|
| Root cause proven | Fill the block, then move to `IN_PROGRESS` |
| No design freedom (typo, version bump, docs) | `analysis: skipped:trivial` |
| Root cause cannot be proven with current evidence | `ANALYSIS → BLOCKED` with `blocked_on: <reason>` (what evidence or person it waits for) |
| Instance of a known root | `absorbed_by: <rca task>` + `DEFERRED` |
| The task *is* the investigation | `type: rca` with root cause + evidence + decision |

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
   author and verdict, in `.teamwright/sessions/<ID>.reviews.jsonl`. A send-back
   (`NEEDS_REVIEW → IN_PROGRESS`) is not recorded as development.
2. `→ CODE_COMPLETE` unless the latest round says `APPROVE` and
   - its ledger entry shows it was written by a `code-reviewer` agent that did not
     implement the task, with that verdict (a record edited afterwards does not count);
   - the journal (`.teamwright/logs/tools.jsonl`) shows the reviewer subagent was
     spawned for this task in that session — the task id must appear in the spawn's
     description or prompt. A top-level `claude --agent code-reviewer` session has no
     spawn and needs none;
   - `reviewer` differs from the task's `implemented_by` (when set).
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
5. Shell commands that write review records or the `.teamwright/` ledgers and journal.
6. With `process.review.blocking` set in `.teamwright/config.yml` (the owner's threshold,
   `low` | `medium` | `high`): an `APPROVE` whose `## Findings` table holds a blocking
   row, or a `REQUEST_CHANGES` with none. A row blocks when its Severity is at or above
   the threshold, when it is Critical, or when its Category is `secret-leak`,
   `security`, `weakened-test`, `done-not-working` or `recurrence` — at any threshold.
   Severities other than Critical | High | Medium | Low are refused. Rows below the
   threshold are follow-ups: the task is approved and the flow collects them into one
   `review-follow-up` task per sprint. Without the key nothing here is checked (every
   finding may send the task back, as before settings existed).

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
     the task id and that touches files outside `docs/tasks/`. If no commit names the
     task, the record must verify `HEAD` itself;
   - each counted record matches its ledger entry (kind, result, commit), or is a
     person's committed record (`verifier: human:<name>`, no ledger entry).
4. `kind: manual` without `verify_exception` in the task (at write time and at the
   transition); with the exception, at least one fresh manual `PASS` must come from an
   agent that did not implement the task (`.teamwright/sessions/<ID>.dev`,
   `implemented_by`) or from a person.
5. Shell commands that write verify records.
6. Acceptance verdicts are validation records, `docs/tasks/<ID>.validation/<n>.md`
   (`verdict: PASS | FAIL | NEEDS_OWNER`, `commit:`, `validator: task-validator`,
   template `docs/tasks/_validation-template.md`): written only by `task-validator`,
   only with Edit/Write, append-only, next round only, commit must resolve; ledger
   `.teamwright/sessions/<ID>.validation.jsonl`. Entering `DONE` needs the latest round
   `PASS`, matching its ledger entry, for the task's current code (same freshness rule as
   verify records) — for every `surface`, including `deploys: false` + `surface: none`,
   where it is the only check. A round whose file is gone while the ledger has it blocks
   `DONE` (`TEAMWRIGHT_VALIDATOR_AGENT`, default `task-validator`).

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
| `Stop`, `SubagentStop` | `outcomes.jsonl` | session or subagent end: `outcome` from the final message's `Outcome: barrier\|idle\|escalate\|interrupted` line — `last_assistant_message`, else the last assistant text in the transcript (`none` if missing; `outcome_source`: message / transcript / none); a `Stop` without the line while an agent the session launched has not finished is `waiting` (`outcome_source: open-agent`) — active task and its status, the agent's tool-call count |
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

- A commit that touches only `docs/**` or the spec tool's directories (`specs/**`,
  `openspec/**`) — task and review records, specs, decisions — belongs to no task: it
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

**False positive:** add `teamwright:allow-secret` (or `gitleaks:allow`) to the line.
**Deliberate bypass** (human, rare): `git commit --no-verify` — and say why in review.
The kit installs no CI workflow. `--no-verify` skips the hook, so a project that wants a
backstop adds the scan to its own CI, for example a `make scan` target that runs
`gitleaks git --config .gitleaks.toml --redact .` over the full history.

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
| `TEAMWRIGHT_ESCALATE_AFTER` | review gate | `REQUEST_CHANGES` rounds in a row before the next record must escalate (`2`) |
| `TEAMWRIGHT_LOG_DIR` | journal, review gate | journal directory (`.teamwright/logs`) |
| `TEAMWRIGHT_GATE_LOG` | Claude Code gates | a file that gets one plain line per warning or denial: time, gate, `WARN`/`DENY`, first line of the reason (off) |
| `TEAMWRIGHT_PAUSE_FILE` | `pre-push`, router | pause file (`TEAMWRIGHT_PAUSE` at the repository root) |
| `TEAMWRIGHT_PUSH_STATUSES` | `pre-push` | statuses whose code may be pushed (`CODE_COMPLETE DEPLOYED VERIFIED DONE`) |
| `TEAMWRIGHT_DOC_PATHS_RE` | `pre-push` | commits touching only these paths belong to no task (`^(docs\|specs\|openspec)/`) |
| `TEAMWRIGHT_WIP_REFS_RE` | `pre-push` | remote refs that are CI-only pushes, not deliveries (`^refs/heads/wip/`) |
| `TEAMWRIGHT_ALLOW_PUSH=1` | `pre-push` | one-off human override of DRAIN |
| `PATCH_GUARD=off` | patch guard | one-off human override |
| `TEAMWRIGHT_CLAUDE_MD_MAX_KB` | CLAUDE.md budget | warning threshold in KB (`40`, set in `.claude/settings.json` → `env`) |
| `TEAMWRIGHT_SESSION_CHECK` | session check | `0` turns it off (on) |
| `TEAMWRIGHT_ROOT`, `TEAMWRIGHT_CONFIG` | `scripts/tw-run` | another project root or config file |
| `TEAMWRIGHT_OWNER=1` | installer (in the kit) | owner override outside Claude Code; ignored when `CLAUDECODE` is set |

## Overriding deliberately

Every gate has an exit that leaves a trace (a field in the task file, a trailer in the
commit, an environment variable a human types). That is the point: an override is
allowed, a silent one is not. When an override is used more than occasionally, the
gate is wrong or the process is — record it in `docs/failures.md` and fix the cause.

## Known limitations

- Claude Code gates see Edit/Write and obvious shell rewrites; a determined script
  (e.g. Python writing the file) bypasses them. Git hooks and review are the backstop.
- Ledgers and the journal are local (`.teamwright/`, gitignored): a review written by
  an agent on another machine is not recognised — record it as `reviewer: human:<name>`
  after a person checked it, or run the review where the work was done.
- One `.teamwright/current-task` per working tree; parallel sessions need worktrees.
- The state guard parses shell commands; a script file that writes
  `.teamwright/config.yml` without the command naming it, or a path built at run time,
  slips through. Review changes to that
  file like code (it is committed), and `tw-install.py` refuses lowering the mode.
- The review gate trusts `agent_type`: anyone who can edit `.claude/agents/` can name
  an agent `code-reviewer`. Review changes to agent definitions like code.
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
