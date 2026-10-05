---
# Review record: save as docs/tasks/<ID>.reviews/<round>.md (1.md, 2.md, ...).
# Written only by a code-reviewer agent (the review gate checks agent_type) that did not
# implement the task. A person reviewing outside Claude Code sets `reviewer: human:<name>`
# and commits the file; agents cannot write that marker.
# Comments on their own lines only: hooks read values literally.
task: T-NNN
# must equal the file name; rounds are counted from the files, not from any field
round: 1
# code-reviewer, or human:<name>; must differ from the task's implemented_by
reviewer: code-reviewer
# APPROVE | REQUEST_CHANGES | REJECT_RECURRENCE | ESCALATE
# ESCALATE: after 2 rounds of REQUEST_CHANGES (a 3rd is refused), or a decision above the task — waits for a human
verdict: REQUEST_CHANGES
# REJECT_RECURRENCE only: earlier tasks fixed with the same pattern
recurrence_of: []
# REQUIRED with REJECT_RECURRENCE: the type: rca task that fixes the root
rca_task: ""
---

## Checked against

- [ ] docs/failures.md and recent tasks touching the same seam: same fix pattern as before?
- [ ] PRINCIPLES.md: any principle violated without a recorded exception?
- [ ] Acceptance hits the failure branch; a test fails without the change

## Findings

<!-- Severity: Critical | High | Medium | Low. Category: one of the names in code-reviewer.md
     (correctness, docs-drift, toothless-test, ...). The review gate reads these two columns:
     with `process.review.blocking` set, REQUEST_CHANGES needs a blocking row and APPROVE
     must have none. Rows below the threshold are follow-ups. -->

| # | Severity | Category | Where (file:line) | Finding | Required change |
|---|----------|----------|-------------------|---------|-----------------|
| 1 | | | | | |

## Verdict rationale

<One paragraph. For REJECT_RECURRENCE: which earlier fixes repeat and what the root is.>
