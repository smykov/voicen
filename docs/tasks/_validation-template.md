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
---

## Claims

| Claim | Evidence | OK |
|---|---|---|
| <claim from Acceptance> | `path/file.ext:42`, `test_name`, verify/<n>.md | yes / no / partial |

**Verification**: <verify/<n>.md — kind — PASS/FAIL> | surface none
**Gate** (run by validator): <command — result>
**Mutations**: <n> applied, <k> killed — <surviving: file:line — what was changed>
**Review**: round N — APPROVE | missing
**Needs owner**: <action> | none
