---
# Verify record: save as docs/tasks/<ID>.verify/<round>.md (1.md, 2.md, ...). APPEND-ONLY:
# never edit a round; a re-run or a correction is the next round. Written with Edit/Write
# only (the verify gate refuses shell writes). Comments on their own lines only: hooks
# read values literally.
task: T-NNN
# must equal the file name; the next round is always (last round + 1)
round: 1
# api | ui | smoke | manual
# manual only when the task has `verify_exception: "<reason>"`; it must be checked by
# someone other than the implementer (another agent or human:<name>)
kind: ui
# REQUIRED for kind ui: the driver that ran the UI - the tool value of the configured UI
# runner (AGENTS.md > Tools): playwright | patrol | flutter-integration-test | maestro |
# espresso | xcuitest | ... The values below are an example.
tool: playwright
# the exact command that was run, copy-pasteable
command: npx playwright test e2e/<feature>.spec.ts
# local | staging | production-like (never a hostname or an address)
environment: staging
# sha of the revision that was verified; must contain the task's last code commit
commit: <sha>
# PASS | FAIL (FAIL -> the task goes to VERIFY_FAIL, then ANALYSIS)
result: PASS
# artifact the run produced: report dir, junit xml, trace, log (repo-relative path that
# exists when the record is written, or a CI artifact URL)
evidence: playwright-report/
# the writer's agent_type (main for a top-level session without one), or human:<name>
# for a person who ran the check outside Claude Code and commits this file
verifier: task-validator
---

## Scenarios checked

- Happy path: <input / steps> → <observed result, checked against data or logs>
- Failure branch: <the failure branch from the task's Acceptance> → <observed result>

## Notes

<What was not covered and why; links to traces or screenshots inside the evidence.>
