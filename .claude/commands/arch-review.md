---
description: Architecture review for a recurring defect class (recurrence exit b) — options with trade-offs, owner decision, follow-up spec/plan or tasks
argument-hint: <rca task id, e.g. T-057>
---

# /arch-review — architecture review for a recurrence

Run this when an `rca` task's analysis recommends **exit (b)**: the defect class crosses seams or services, the seam itself sits in the wrong place, or the area is a hotspot. A local fix has already failed to hold at least once — this review decides the shape of the change before anyone writes code.

Task: `$ARGUMENTS`. Process: `docs/process/lifecycle.md` §6, `docs/process/principles.md`.

You (orchestrator) prepare the options; the **owner decides**. Do not implement anything in this session, and do not record an option as decided until the owner has chosen it.

## 1. Inputs — collect before proposing anything

1. **The recurrence class.** From the rca task: `class`, `recurrence_of` (every earlier task and commit of the class), the analysis block and `## Investigation` (why each earlier fix did not hold, recommended exit).
2. **Hotspots and recurring areas.** Run the metrics and keep only rows that touch the class:
   ```sh
   python3 scripts/metrics/teamwright-metrics
   ```
   Use *recurring areas*, *hotspots (churn × fix commits)*, *tasks fixed again*. Quote each number with its n; `no data` stays `no data`.
3. **Affected seams.** Every place that handles the concern today — found with the configured context tool (`AGENTS.md` › Tools: symbol references or graph queries), grep only as a fallback; an empty result is not proof of absence. For each seam: owner service/module, entry points, which earlier fixes touched it.
4. **Constraints.** `docs/architecture.md`, `docs/decisions.md` and `docs/decisions/<area>.md` for the area, `PRINCIPLES.md` (especially P-010, P-011, P-012), open questions in `docs/open-questions.md`.

Present the inputs in a short table before the options. If an input is missing (no metrics yet, analysis without evidence), say so — don't fill the gap with a guess.

## 2. Options — 2 to 4, including "keep the current shape"

For each option:

| Field | Content |
|---|---|
| Shape | What changes: which seam becomes the single place for the concern, what moves, what is removed |
| Invariant | The closed condition that makes the class impossible, phrased so a test can check it |
| Why it removes the class | Against each earlier recurrence: would it have been prevented? |
| Blast radius | Seams / services / contracts / data touched; migration needed? |
| Cost | Rough size in tasks; what must happen in order |
| Risk and rollback | What can break, how it is noticed (which verification), how to back out |
| Verification | Which API / end-to-end UI tests prove it on the running system |

"Keep the current shape" is always an option: its cost is the expected rate of further recurrences (from the metrics), stated honestly.

End with a **recommendation** and the question for the owner, one sentence.

## 3. Owner decision

After the owner answers, record it immediately in `docs/decisions.md` (new row: question, Was → **Decided**, why, rca task id, proposed by: agent, decided: owner) and, if the area has one, in `docs/decisions/<area>.md` (invariant + the defect class behind it + what breaks if violated). Never edit an earlier row.

## 4. Follow-up — by the size of the chosen change

- **Spans more than one seam or service** → a new pass through the configured spec tool (`AGENTS.md` › Tools): specify (the decision is the input) → clarify → plan → tasks; with no spec tool, a `docs/specs/<name>.md` the owner agrees. Every task created from it carries `design_ref: <path to spec/plan>#<section>` and goes through the teamwright lifecycle, not the spec tool's implement/apply step.
- **One seam** → ordinary task records (`type: feature` or `chore`), each with the invariant in its Acceptance and `design_ref` pointing to the decision.
- In both cases: link the new tasks from the rca task; the rca task is closed when the decision is recorded and the tasks exist. Tasks it absorbed stay `DEFERRED` until the new work is `DONE`, then are re-checked (`lifecycle.md` §2).
- If the review shows the class is really a process defect, stop and propose exit (c) instead: an amendment to `PRINCIPLES.md` in a separate commit with a version bump (`principles.md` §2a).

## Output

```
Arch review: <rca id> — class <class>
Inputs: <n> recurrences · hotspots <files/areas> · seams <list>
Options: A <one line> · B <one line> · C keep current
Recommended: <X> — <why>
Owner decision: <pending | X, recorded in docs/decisions.md #N>
Follow-up: <spec/plan → tasks | tasks T-NNN… | exit (c) proposed>
```
