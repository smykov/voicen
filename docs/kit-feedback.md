# Feedback for the teamwright kit

Written by an agent for the owner to send upstream (decisions #206, OQ-70). The owner sends it; OQ-70 closes when it is sent. Kit version in this project: 0.13.0.

## From reviews (OQ-70)

1. **Installer-rendered text recommends refused commands** (T-064 r2#11). The rendered text suggests `npx --no playwright test` and `BASE_URL=... npx playwright test`. This project refuses both: e2e runs only through `pnpm e2e` in the ui image. Affected files: `AGENTS.md` (ui_verify), `docs/process/tools.md`, `.claude/agents/test-writer-ui.md`, `docs/tasks/_verify-template.md`. Ask: render the project's own `e2e_ui_command` instead of the provider's generic command.
2. **The guard trigger fails open** (T-113 r2#3). When `git diff --cached` fails, the guard-change trigger allows the commit. Ask: fail closed, or name the case under "Not caught".
3. **The guard trigger reads the worktree record** (T-113 r2#5). It reads the task record in the worktree, not the staged one, so a staged record differs from what it checks. Ask: read the staged blob (`git show :docs/tasks/<ID>.md`).

## From the parallel run of 2026-10-24/25 (`/teamwright:parallel auto`)

4. **Loops burn the stage limit waiting for a person.** When a task waits for something only the owner can do (here: the CI-only `wip/<ID>` push for P-016), each headless `implement` session re-checks, finds nothing to do and appends a Notes line. The loop counts that as moved and runs again until `TW_PARALLEL_MAX_STAGES` (12). This happened on T-120, T-133, T-134, T-140, T-160, T-185 and T-186. Ask: an `escalate` outcome, or a stage that only adds a Notes line, ends the loop, or the session sets `BLOCKED` with `blocked_on`.
5. **`plan` ignores `depends_on` readiness.** It grouped T-122 although its dependency T-119 was still in `ANALYSIS`. Ask: `plan` applies the router's readiness rule.
6. **No `stop` command in `tw-parallel.py`.** Pausing one pipeline took touching `TEAMWRIGHT_PAUSE` in the worktree, `kill -TERM -- -<pgid>` and `docker stop` of its containers. Ask: `tw-parallel.py stop <ID>`.
7. **Plugin version drift in headless sessions.** The `claude -p` sessions reported plugin 0.11.0 while the project is on 0.13.0. Ask: launch pins the project's plugin version, or warns before starting.
8. **No cap on concurrent pipelines by machine load.** `batch.parallel: 10` started 8 pipelines; each runs its own gates and containers, and the host nearly ran out of memory (2.2 GB of 23 GB free, swap full, load 45, `target/` growing ~20 GB/h). Ask: a separate `parallel.max_running` (or a load/memory check before `launch`), and a note that every worktree has its own `target/`.
9. **Worktree setup skips dependency install** (T-114 r1#2). A worktree made by `start` has no `pnpm install`, so `make check` stops at `check-nsis-fork` "cannot run". Ask: `prepare` runs the project's setup command.
10. **tw-next `own=` patch.** A local patch to `tw-next.py` (`own=` in candidates) was needed so a worktree router offers only its own task; the original is kept by the session. Ask: upstream the behaviour.
