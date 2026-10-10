---
# Comments must be on their own lines: hooks read values literally, inline "# ..." would become part of the value.
id: T-NNN
title: <observable outcome in one line>
# type: feature | bug | rca | chore  (rca = root-cause analysis)
type: feature
# TODO → ANALYSIS → IN_PROGRESS → NEEDS_REVIEW → CODE_COMPLETE → DEPLOYED → VERIFIED → DONE
# side: BLOCKED | DEFERRED (+ absorbed_by) | VERIFY_FAIL (→ ANALYSIS)
status: TODO
# sprint the task was created in; never changes
sprint: <N>
# P0 prod down / data leak · P1 user-visible today · P2 hidden defect or proven-class debt · P3 cosmetic
priority: P2
depends_on: []
# false = no deployable artifact: CODE_COMPLETE → DONE directly
deploys: true
# what the running system exposes to verify: ui | api | both | none (checked by the verify gate)
# ui   = a UI record from the configured UI runner (tools.ui_verify in .teamwright/config.yml)
# api  = an API test record · both = one api AND one ui record
# none = pure internal / library / docs: with `deploys: false` no verify record is needed
# left empty, the task cannot reach VERIFIED or DONE: choose deliberately
surface: ""
# spec or plan this task implements (path + anchor, from the spec tool or docs/specs/); "" when there is none
design_ref: ""
# ONLY when the change cannot be checked by an api/ui test: why, in one line. Allows `kind: manual`
# verify records; reviewed like code, and a manual check must come from someone other than the implementer.
verify_exception: ""
# defect class, machine-readable (kebab-case, e.g. input-parsing, auth-session, retry-timeout).
# Same class on several tasks = recurrence candidates; metrics count tasks per class.
class: ""
# set by the reviewer when the same defect class was fixed before
recurrence_of: []
# true when the seam is a checker or guard - code whose whole guarantee is rejecting bad
# input (a validator, a manifest check, a permission guard). The test-writer's bite check
# is then an exhaustive sweep before the first review, not 2-3 wrong implementations.
checker: false
# rca task id, when this task is deferred into a root task
absorbed_by: ""
# soft hold reason; clear to "" when resolved
blocked_on: ""
# REQUIRED before IN_PROGRESS, or `analysis: skipped:trivial` for a change with no design freedom.
# evidence: file:line, log line, failing test · invariant: what must stay true · seam: where the change goes
analysis:
  root_cause: ""
  evidence: ""
  invariant: ""
  seam: ""
  approach: ""
# type: rca replaces the block above with this one (required: root_cause, evidence, decision;
# no skipped:trivial). decision = the exit and what it changes: for (a) code, the invariant and
# the seam; for (b) architecture or (c) process, the owner's decision (docs/decisions.md row).
# invariant and seam stay useful for exit (a).
# analysis:
#   root_cause: ""
#   evidence: ""
#   decision: ""
#   invariant: ""
#   seam: ""
# agent or person who implemented the task; the reviewer must differ (checked by the review gate)
implemented_by: ""
# Review rounds are not a field: the gates count docs/tasks/<ID>.reviews/<n>.md files.
# After 2 rounds of REQUEST_CHANGES the next verdict is ESCALATE, not another send-back.
# Exit for a change with no design freedom (typo, version bump, docs only):
#   review: skipped:trivial
# The line stays in the record and is visible at every later review of the history.
review: ""
# `ratified` = the owner accepts pushing this task's code although the task is not in an
# approved status (CODE_COMPLETE, DEPLOYED, VERIFIED, DONE). Any other value keeps pre-push blocking.
tail_code: ""
---

## Context

<Why this task exists. Links: FR-NN, OQ-NN, F-NNN, earlier tasks.>

## Acceptance

- [ ] <criterion, checkable>
- [ ] Failure branch: <input that reproduces the defect / edge case> → <expected>, verified by <data / log>
- [ ] Test that fails without the change: `<test name>`

## Tests

<test-writer: guarantee → test; bite check — each deliberately wrong implementation and the test that caught it.>

## Investigation

<problem-investigator: evidence and reasoning behind the analysis block.>

## Review

<One line per round: `Round N — <verdict> — reviews/<N>.md`. Full records: `docs/tasks/<ID>.reviews/<N>.md`.>

## Verification

<One line per round: `Round N — <kind> — <PASS|FAIL> — verify/<N>.md`. Full records (append-only):
`docs/tasks/<ID>.verify/<N>.md`, template `docs/tasks/_verify-template.md`.>

## Validation

<One line per round: `Round N — <PASS|FAIL|NEEDS_OWNER> — validation/<N>.md`. Full records (append-only, written by
task-validator): `docs/tasks/<ID>.validation/<N>.md`, template `docs/tasks/_validation-template.md`.>

## Notes

<Decisions made during work, review findings, what it turned out to be.>
