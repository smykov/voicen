# Stage protocol

What every stage session (`/teamwright:analyze`, `red-tests`, `implement`, `review`, `verify`, and each step of `/teamwright:router` and `/teamwright:flow`) does. Kept short on purpose: a stage session reads this, not the long explanation in [sessions.md §8](sessions.md#8-stages-one-stage-per-session).

## 1. Ask

```
python3 "${CLAUDE_PLUGIN_ROOT}/scripts/tw-next.py" --brief --stage <stage> [--task <ids>]
```

`request` comes first. Named tasks are put in dependency order and capped at the stage's batch; read `request.warnings`. `request.ok` false → show `request.hint` (the command to run instead: another stage, the unfinished work in the tree, the pause), do nothing else, end with `Outcome: idle`. An explicit stage ignores `min`; it never skips unfinished work in the tree. Do not read task files to look for work.

## 2. Choose the batch

Take at most `request.batch_cap` tasks from `request.candidates` (router order; `candidates_total` says how many wait). `request.batch` is the default. Deviate when the facts say so, in one line:

- seams and areas that do not overlap (`overlaps` empty) and no dependency between them (`depends_on_candidates`): one cannot invalidate the other's work;
- the same area: shared context, a cheaper batch;
- a candidate that depends on another goes after it, or waits.

`settings.router.select: agent` → the router lists no candidates and picks no batch (it still decides whether the stage may run): read `docs/tasks/*.md`, apply the readiness rule (`lifecycle.md` §4) and the stage rule (sessions.md §8) yourself.

## 3. Per task, one after another

- Make it the active task first (an edit of its record), so the gates and the journal see it.
- Role agents run in the foreground; commit each result before the next step.
- Every status change is its own commit; the `stage:` field changes with the commit that finishes the stage's work. The analysis gate holds the order (`gates.md`, analysis gate check 4).
- One task `IN_PROGRESS` per working tree: an analyze batch analyses several, but moves one into `IN_PROGRESS`; the rest wait in `ANALYSIS` with the analysis committed.
- Decision points are asked when reached; a blocked task does not stop the batch. In a headless run (no one to ask) leave them for the owner and say so.
- Records the gates stamp (`<n>.gate.json` next to a review, verify or validation record) are committed with the record.

## 4. End block

The summary, then:

```
Stage: <stage>
Batch: <ids> - <why these>
Outcome: barrier | idle | escalate | interrupted
```

`Outcome:` is the last line, plain text. The journal reads all three (`outcomes.jsonl`: `stage`, `batch`, `batch_reason`).
