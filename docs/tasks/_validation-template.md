---
# Validation record (acceptance verdict): save as docs/tasks/<ID>.validation/<round>.md.
# Written only by task-validator, with Edit/Write (the verify gate refuses shell writes
# and any other writer). APPEND-ONLY: a new check is the next round. Comments on their
# own lines only: hooks read values literally.
task: T-NNN
# must equal the file name; the next round is always (last round + 1)
round: 1
# PASS | FAIL | NEEDS_OWNER. DONE needs the latest round PASS for the task's current code.
verdict: PASS
# sha of the revision that was checked; must contain the task's last code commit,
# otherwise the verdict is stale and a new round is needed
commit: <sha>
validator: task-validator
# sample (the catalog re-run + 15-30 new mutants aimed at the guarantees and the task's
# changed lines) | sweep (the catalog re-run + every condition, guard, return and write on
# the task's changed lines; the whole seam only for a `critical: true` task).
# After two FAIL rounds in a row the gate accepts only sweep or NEEDS_OWNER; after two
# sweep rounds with FAIL, only NEEDS_OWNER.
method: sample
---

## Claims

| Claim | Evidence | OK |
|---|---|---|
| <claim from Acceptance> | `path/file.ext:42`, `test_name`, verify/<n>.md | yes / no / partial |

**Verification**: <verify/<n>.md — kind — PASS/FAIL> | surface none
**Gate** (run by validator): <command — result>
**Mutations** (<sample|sweep>): catalog `docs/tasks/<ID>.mutants/` — <n> re-run, <a> added this round; <k> killed, <e> equivalent — `scripts/tw-mutants` summary: <line> — also re-run by hand: <test-writer's wrong implementations from `## Tests`> | none
**Survivors that break a guarantee** (each fails the round): <mutant — the Acceptance/DoD guarantee it breaks> | none
**Follow-ups** (survivors that break no stated guarantee; not a FAIL): <mutant — what it changed> | none
**Review**: round N — APPROVE | missing
**Needs owner**: <action> | none
