# PRINCIPLES.md

**Version:** 1.1.0 — bumped in a separate commit on every amendment, recorded in `docs/decisions.md` (major: removed or reversed · minor: added or moved up a tier · patch: wording). Amendments are owner decisions; see `docs/process/principles.md` §2a.

Engineering principles for this project. Each is enforced at a tier:
**T0** written guidance · **T1** reviewer checklist · **T2** hook warns · **T3** hook/CI blocks.
Rules move up a tier when their class recurs in this project (log each incident in `docs/failures.md`).

Accepted entries are not edited. To change one, add a new entry and mark the old one `Superseded by P-NNN`.
Each **Why** names, in one generic sentence, the kind of incident that produced the rule on earlier projects. When such an incident happens here, log it in `docs/failures.md` and add `Origin: F-NNN` to the principle. Delete principles that don't apply.

---

### P-001 — Fix the root cause, not the symptom
- **Why:** a defect class was patched point by point for months, each patch passing review, until splitting the recurrences out exposed several times more root-cause work than the plan showed.
- **Tier:** T1 + T3
- **How checked:** the `analysis` block (`root_cause`, `evidence`, …) is required before `IN_PROGRESS` and before code edits of the active task (analysis gate, T3; it warns instead while `mode: gate-first`); reviewer asks "what caused this, and does the change remove it?" A deliberate workaround is marked temporary with a linked root task.

### P-002 — An invariant, not an allowlist
- **Why:** a recurring defect was "fixed" by adding one more regex or allowlist entry each time — once even under a commit message claiming the list had been replaced by an invariant.
- **Tier:** T1 + T2
- **How checked:** reviewer flags diffs that only extend a list of known cases; patch-guard (`commit-msg`) warns on patch-like fixes to an already-fixed area — `warn` by default, `block` is opt-in; recurrence → `rca` task (see `docs/process/lifecycle.md` §6).

### P-003 — Escalate instead of patching a second time
- **Why:** tasks returned by review were retried by the same author in the same framing, and each retry produced another variant of the same patch.
- **Tier:** T1 + T3
- **How checked:** two review records in a row returned the task (`REQUEST_CHANGES`) → the third record is `ESCALATE` (owner) or `REJECT_RECURRENCE`, and the review gate refuses any other verdict for it; a detected recurrence → `REJECT_RECURRENCE`, task `DEFERRED` into an `rca` task (`absorbed_by`). `rca` depth ≤ 1.

### P-004 — A guarantee without a failing test doesn't exist
- **Why:** a guarantee stated only in a doc, comment or prompt was ignored by the first cheap code path that came along.
- **Tier:** T1
- **How checked:** every new invariant ships with a test that fails when the invariant is removed (red-before-fix); the reviewer and the validator re-run the gate and do not accept a red one. No hook runs the gate: a project that wants it blocking adds the gate command to its CI.

### P-005 — Tests must bite
- **Why:** green, happy-path-only tests kept passing while the code they were meant to guard was broken on the failure branch.
- **Tier:** T1
- **How checked:** reviewer names the line whose removal turns each test red; the task-validator applies 1–3 mutations to critical guarantees only (secret or transcript leak, data loss, the Acceptance failure branch, the task's invariant; decision #32) on a throwaway copy of the tree and fails the task if one survives; acceptance includes the failure branch (empty result, partial write, timeout, duplicate), not only the happy path.

### P-006 — Verify against data and logs, not against the system's own words
- **Why:** a pipeline reported success while every deploy job had been skipped, and the failed delivery went unnoticed because nobody checked the running version.
- **Tier:** T1 + T3
- **How checked:** `VERIFIED` / `DONE` require a `PASS` verify record (`docs/tasks/<ID>.verify/<n>.md`) for the current commit — API tests, an end-to-end UI test run with the project's configured UI runner or a deploy smoke that checks the running version — exercising the Acceptance failure branch and naming a data/log observation (verify gate). `kind: manual` only with a `verify_exception`.

### P-007 — Self-review doesn't count
- **Why:** when "reviewed" was a flag the author set, it was set in far more sessions than a reviewer was actually invoked.
- **Tier:** T3
- **How checked:** `CODE_COMPLETE` requires a review record with `APPROVE` written by the `code-reviewer` agent, not the author (review gate); pre-push lets a task's code out only in `CODE_COMPLETE`, `DEPLOYED`, `VERIFIED` or `DONE`, or with `tail_code: ratified`.

### P-008 — Measure from git and logs, not self-reports
- **Why:** agents reported "idle" while the queue had ready work, and a required tool silently disappeared behind a permission change for weeks — both visible only in logs, not in self-reports.
- **Tier:** T0
- **How checked:** nothing warns or blocks; the rule is kept by where the numbers come from. Hooks journal tool calls and session outcomes into `.teamwright/logs/`; metrics (rework rate, review rounds, recurrence count, cycle time) are computed from commit history, review records and those logs using the commit convention. No single metric is a target. A zero reading is first checked for a broken extractor.

### P-009 — Secrets never reach logs, commits or docs
- **Why:** keys reached plaintext logs through a third-party library's request logging and through exception text, not through the project's own log lines.
- **Tier:** T1 + T3
- **How checked:** secret scanner in the `pre-commit` hook (blocks; `git commit --no-verify` skips it, so a project that needs a backstop runs the same scan in its CI, e.g. `gitleaks git --redact .`); log context is an allowlist; before promising "not logged", name **every** path (own logs, libraries, exception handlers). The component that holds a secret masks it — callers don't.

### P-010 — One source of truth per cross-cutting value
- **Why:** a cross-cutting value computed by two separate chains gave two different answers inside one response.
- **Tier:** T1
- **How checked:** reviewer asks "is this value already resolved somewhere?"; one resolver, others consume it.

### P-011 — One shared seam, not N parallel paths
- **Why:** a guard added to one of several parallel write paths left the other paths open to the same defect.
- **Tier:** T1
- **How checked:** reviewer rejects a new path beside existing ones for the same concern; files that always change together are candidates to merge.

### P-012 — Don't rewrite a known-dangerous function without an invariant
- **Why:** a critical function was rewritten and reverted repeatedly, each rewrite causing a new production incident.
- **Tier:** T0 + T1
- **How checked:** such functions are listed in `CLAUDE.md` → "What you must know" with a link to their decision file; changing one requires a stated invariant, a test that pins it, and owner sign-off.

### P-013 — New config must reach the runtime
- **Why:** new settings were added to a config file but never passed into the running service, and were silently ignored in production.
- **Tier:** T1
- **How checked:** DoD for config changes includes proof the runtime reads the new value (startup log, health output, test through the real loader).

### P-014 — Docs follow code in the same task; CLAUDE.md is a map with a budget
- **Why:** a context file grew to hold every decision and had to be split, because it was loaded into every session before any question was asked.
- **Tier:** T1 + T2
- **How checked:** reviewer checks that touched contracts/decisions are updated in the same commit; a hook warns when `CLAUDE.md` exceeds its size budget (default 40 KB, configurable); it never blocks. `CLAUDE.md` keeps only what costs you **before** you'd think to open a file; the rest lives in `docs/decisions/<area>.md`.

### P-015 — Resolve a recurrence at the level that produced it
- **Why:** a defect class kept returning after each root-cause fix in code, because the cause was a boundary drawn in the wrong place — or a process rule that allowed the defect — not a line of code.
- **Tier:** T1
- **How checked:** every `rca` analysis names its exit — (a) code: invariant in one seam; (b) architecture: `/arch-review` → owner decision, a spec/plan through the project's spec tool when more than one seam or service changes; (c) process: amendment of this file in a separate commit with a version bump. (b) and (c) are owner decisions recorded in `docs/decisions.md`.

### P-016 — A premise only CI can run is run on CI before review
- **Why:** every red CI run on `main` here (10 of 10) was a premise that only CI could run (a Win32 contract, a runner tool, pwsh/C# the host cannot parse, link args, Windows timing, an action default), first executed by the delivery push, and each local fix was again first run on `main`. Origin: F-001, F-002, F-005, F-007, F-008, F-009, F-011 (class `ci-premise-unchecked-before-main`, rca T-066, decisions #90).
- **Tier:** T1 + T2 (T3 pending kit feedback)
- **How checked:** every task whose diff touches anything outside `docs/**` cites under `## Tests` a line `CI: <run url or id> @ <sha>`: a green run of the full CI workflow (Gate and Windows jobs) on `wip/<ID>` at the reviewed head commit, before `NEEDS_REVIEW → CODE_COMPLETE`. A red `wip` run returns the task before review. The code-reviewer refuses `APPROVE` without that line (T1); "proven by the next push to `main`" is never evidence. The orchestrator's `git push origin HEAD:wip/<ID>` is CI only, not a delivery (pre-push skips it; the branch is deleted at `CODE_COMPLETE`). T2: the flow pushes `wip/<ID>` for areas with `ci_workflow`, and `make check` fails when `ci.yml` stops triggering on `wip/**` or a cache save is not limited to `main`. T3 (the review gate requiring the run id) is pending kit feedback.

---

## Registry

| ID | Principle | Tier | Status |
|---|---|---|---|
| P-001 | Root cause, not symptom | T1+T3 | Accepted |
| P-002 | Invariant, not allowlist | T1+T2 | Accepted |
| P-003 | Escalate instead of patching twice | T1+T3 | Accepted |
| P-004 | Guarantee needs a failing test | T1 | Accepted |
| P-005 | Tests must bite | T1 | Accepted |
| P-006 | Verify the running system against data/logs | T1+T3 | Accepted |
| P-007 | Self-review doesn't count | T3 | Accepted |
| P-008 | Measure from git/logs | T0 | Accepted |
| P-009 | Secrets never in logs | T1+T3 | Accepted |
| P-010 | Single source of truth | T1 | Accepted |
| P-011 | One shared seam | T1 | Accepted |
| P-012 | Invariant before rewriting dangerous code | T0+T1 | Accepted |
| P-013 | Config must reach runtime | T1 | Accepted |
| P-014 | Docs follow code; CLAUDE.md is a map | T1+T2 | Accepted |
| P-015 | Recurrence resolved at its level (code / architecture / process) | T1 | Accepted |
| P-016 | CI-only premise run on CI before review | T1+T2 | Accepted |
