# Metrics

What the process is actually doing, computed from records the agents cannot rewrite
after the fact: commits, task-transition commits, review records committed in git, and
the hook journals. Tool: `scripts/metrics/teamwright-metrics` (Python 3, stdlib only).

```bash
python3 scripts/metrics/teamwright-metrics                          # Markdown, whole history
python3 scripts/metrics/teamwright-metrics --json -o metrics.json   # same data as JSON
python3 scripts/metrics/teamwright-metrics --since 2026-01-01 --until 2026-06-30
python3 scripts/metrics/teamwright-metrics --split-date 2026-04-01  # before / after + delta
python3 scripts/metrics/teamwright-metrics --repo ../other-repo     # any git repository
python3 scripts/metrics/teamwright-metrics --debug                  # + unparsed ledger subjects
```

`--help` lists every option. Options can also live in `.teamwright/metrics.json`
(keys = long option names with underscores; see `scripts/metrics/metrics.example.json`).
Unknown keys are an error, so a typo does not silently fall back to a default.

## Why git and journals, not self-report

An agent's report is written by the party being measured, at the moment it is most
confident. In the process this template comes from, the review stage was "done" in the
reports and absent in the journal for most sessions; a tool was cut off by permissions
for weeks while reports said nothing; "replaced the allowlist with an invariant" came
with a diff adding another allowlist. None of these were lies — they were sincere and
wrong. So the rule (`PRINCIPLES.md`) is: numbers come from commits, committed records
and hook journals. The tool never reads an agent's summary, a task's hand-kept counter
or a status field as proof that something happened.

## Honest output

- Every number is printed with its **n** and its **source**. A share is `42.0% (21/50)`.
- A missing source prints **`no data`** (JSON: `null`), never `0`. A real zero —
  the source exists and nothing matched — prints `0`.
- A zero in a metric that should not be zero is first a question about the extractor
  (wrong regex, agents committing without the trailer, journal not wired), then about
  the team (`principles.md`: suspect the path before the agent).

## 1. Git history (works on any repository)

| Metric | Meaning |
|---|---|
| Commits, per month, per active month | `git log <rev> --no-merges` (`--merges` to include merges). Dates are author dates in UTC |
| Ledger commits | Commits whose subject matches `--ledger-regex` (default `^\w+\((?:tasks?\|sprint)\w*\)!?:` — `chore(task):`, `docs(sprint3):`). They record status changes, not work, so **every share below is over the other ("work") commits** |
| Agent co-authored share | Work commits with a `Co-Authored-By:` trailer matching an agent pattern (`--agent label=regex`, repeatable; defaults cover common coding agents). Co-author names are normalized so model variants (`Claude Opus 4.1`, `claude-3-5-sonnet-20241022`) group into one family and a version-free model name; `--model-alias regex=name` overrides. Printed as `unreliable` for a period that overlaps a trailer-coverage break (below) |
| Agent co-authored (weeks with trailer coverage) | Shown only when a break was found: the same share with the break weeks left out |
| Fix share | Subject matches `--fix-regex` (default: conventional `fix:` / `bugfix` / `hotfix`, or the words fix/fixes/fixed — the same default as patch-guard). Reverts and ledger commits are never fixes |
| Revert share | Subject starts with `Revert` or the body says `This reverts commit` |
| Recurrence rate | Of the fix commits that touch at least one counted file: the share that touch an **area** which already received a fix commit in the previous `--recurrence-days` (30). The look-back reaches before `--since` |
| Recurring areas | Areas with at least `--min-fixes` (3) fix commits inside any `--window` (30) day window, with that window's dates |
| Tasks fixed again | Of the task ids named in fix-commit subjects (`--task-id-regex`): the share whose fix commits fall on days at least `--refix-days` (2) apart. The task-level "patched again" signal: it does not depend on files or area size. `--refix-ids single` counts only fix commits naming exactly one id |
| Hotspots | Top areas by churn (lines added + deleted, all commits in range) × fix commits |

Area = the file (`--area file`, default) or its directory (`--area dir`, optionally cut
to `--area-depth N` components; a root-level file is its own area). Directory areas
were the old default and proved useless on a real history: in a monorepo with a few
large services almost every fix lands in a directory fixed the month before, and the
rate sat at ~95% before and after a process change. The patch guard keeps the directory
as its default area (`PG_AREA=dir`): it asks a narrower question at commit time — "was
this neighbourhood patched recently?" — where a fix in a sibling file counts and a false
warning is cheap ([gates.md](gates.md#patch-guard)).

Paths matching `--exclude-regex` count for nothing — neither areas, churn nor
recurrence. Default: `docs/`, every Markdown / reStructuredText / AsciiDoc file
(planning files such as `SPRINT_*.md` and `TODO.md` otherwise top the recurring-areas
list, because every ledger update touches them) and lockfiles. A one-line change
(+1 −1) to a manifest matching `--version-bump-regex` (default `package.json`,
`pyproject.toml`, `Cargo.toml`, `*.csproj`, `Chart.yaml`, `VERSION`, …) is a version
bump riding along with the fix and is not an area either; any larger manifest change
still counts. Pass `''` to either option to switch it off.

### Trailer-coverage breaks

Agents sign commits only while the harness is configured to add the trailer. When
that setting is switched off (or a squash policy drops trailers), the agent share falls
to near zero while the same agents keep committing. The tool looks for that shape per
ISO week: a week with at least `--trailer-min-commits` (10) work commits is *low* when
its agent share is below `--trailer-break-ratio` (0.5) × the median of the previous four
non-low weeks (and that baseline is at least 30%). Two consecutive low weeks — or low
weeks up to the end of history — form a break, which ends at the first week that
recovers. A single low week (a holiday, a week of hand-fixed CI) is not a break.

The report prints a warning with the break's dates and shares, marks the months in the
per-month table, prints the agent share of any overlapping period as
`unreliable (raw …)` (JSON: `value: null`, `unreliable: true`, `raw_value`) and adds the
share over the covered weeks. It cannot tell a settings change from agents really
leaving; the warning says which question to ask.

The recurrence rate is the git-side view of the failure this process exists for: the
same seam patched again and again. It needs no teamwright conventions, so it can be
measured on a repository's history **before** the process was adopted.

## 2. Task lifecycle (task-transition commits)

Parsed from lines like `chore(task): T-042 NEEDS_REVIEW→CODE_COMPLETE`, anywhere in the
subject or body. The default parser is tolerant of hand-written ledgers:

```
(?<![\w-])(?P<ids><ID>(?:[ \t]*(?:[,&+/]|\band\b)?[ \t]*<ID>)*)[ \t]*:?[ \t]*
(?:(?P<from>[A-Za-z][A-Za-z_]*)[ \t]*)?(?:→|->|=>)[ \t]*(?P<to>[A-Za-z][A-Za-z_]*)\b
```

- one or more ids (`T-1, T-2`, `T-1+T-2`, `T-1/T-2`), several transitions per line
  (`T-1 A→B · T-2 C→D`), words before the id (`REVIEW T-7 NEEDS_REVIEW → DONE`);
- `from` is optional (`T-9 → NEEDS_REVIEW`); a missing `from` is **inferred** from the
  task's previous status, so `T-9 → IN_PROGRESS` after `→ NEEDS_REVIEW` is rework;
- statuses: upper-case words of three or more letters, or any spelling that maps (via
  `--status-aliases`) to a known status — so `T-3 wip -> done` parses with an alias,
  while `T-1 foo -> bar` and `T-4 P2→P1` (a priority) do not;
- **clause fallback**, ledger commits only: when the regex finds nothing, an arrow with
  a status is attributed to the ids written before it in the same clause (clauses end
  at `;`, `·`, `|` and line ends): `T-5 analysis + root found → NEEDS_REVIEW`.

**Parse coverage** is printed under the section: the share of ledger commits with at
least one parsed transition, the number of transitions, and the ledger commits that
contain an arrow and a task id but yielded nothing. Many ledger commits legitimately
carry no transition (a review draft, grooming), so coverage is a prompt to look, not
a target. `--debug` adds a sample of the unparsed ledger subjects (arrow-bearing ones
first, `--debug-sample`, 20) to the report — read it before trusting the task numbers
of a foreign history, and widen `--task-id-regex` first (sub-task ids such as `S4-85.4`
are a common miss).

For another project's historical convention:

- `--transition-regex` with named groups `ids` (or `id`), `to` and optionally `from`;
  `<ID>` inside it expands to `--task-id-regex`.
- `--status-aliases FILE` maps that project's words onto these statuses: JSON
  `{"wip": "IN_PROGRESS"}` or lines `wip=IN_PROGRESS`. Statuses are upper-cased and
  spaces/hyphens become `_` before mapping.

| Metric | Meaning |
|---|---|
| Tasks created / done per month | Created = the earlier of the commit adding `docs/tasks/<ID>.md` and the first transition. Done = the last transition, if it is `→ DONE` (a task reopened after `DONE` is not done) |
| Cycle time (median, p75) | First entry into `ANALYSIS`/`IN_PROGRESS` → last `DONE`, in days |
| Lead time (median, p75) | First seen → last `DONE` |
| Rework rate | Of the tasks that reached review (entered `NEEDS_REVIEW`; in a history without that status, any status from review onwards): the share sent back from `NEEDS_REVIEW` / `CODE_COMPLETE` / `DEPLOYED` / `VERIFIED` / `VERIFY_FAIL` to `IN_PROGRESS` / `ANALYSIS` at least once |
| Review rounds per task | Number of committed `docs/tasks/<ID>.reviews/<n>.md` files, over the tasks that reached review. Without committed review records: entries into `NEEDS_REVIEW` (or exits, if more were recorded; the source column says which) |
| Approved without a review record | Tasks with a `→ CODE_COMPLETE` transition but no committed review record — declared approval without the artifact |
| Escalations | `NEEDS_REVIEW → BLOCKED` transitions; plus `ESCALATE` verdicts in review records |
| Recurrence rejects | `NEEDS_REVIEW → DEFERRED` (a `REJECT_RECURRENCE` verdict parks the task under an rca task) |
| Verify failures | `→ VERIFY_FAIL` transitions |
| Tasks per sprint | From the `sprint:` field of the committed task records; otherwise `--sprint-from-id REGEX` over the task id (group 1 = label: `'^S(\d+)-'` puts `S4-12` in sprint 4); otherwise `--sprint-regex` (group 1 = label) over transition commits. Counts tasks seen in transitions — a task planned in a sprint file but never moved is not counted |

A task belongs to the period of its last `DONE` (or last transition). `--task-period
start` groups tasks by the period they started in (first `ANALYSIS`/`IN_PROGRESS`, else
first seen) — the cohort view, which keeps the review rounds of tasks started before a
process change from leaking into the "after" column.

Task and review records are read **as committed at `--rev`**, not from the working tree:
the numbers are reproducible and uncommitted edits do not count.

## 3. Hook journals

Default `.teamwright/logs/*.jsonl` as written by `scripts/hooks/journal.sh`; `--logs`
(repeatable, file or directory) reads other paths. The reader accepts JSON Lines,
concatenated or pretty-printed JSON objects and JSON arrays, and common key spellings
(`session_id`/`sessionId`, `ts`/`timestamp`, `tool`/`tool_name`, ISO or epoch times).
Fragments that do not parse are skipped and counted in the report.

| Metric | Meaning |
|---|---|
| Sessions | Distinct `session_id`. A session belongs to the period of its first timestamp |
| Session outcomes | `Stop` records by `outcome`; `none` = the turn ended without the outcome line, `waiting` = it ended while a background agent was still running (`sessions.md` §2). Sessions without outcome are counted by each session's **last** `Stop` (`none` or still `waiting`). Subagent outcomes are listed separately; runtime helper stops (no agent type, no tool calls) are left out and counted on their own |
| Gate warnings / denials | `gates.jsonl` records by `decision`, and per gate in the breakdown: how often the gates fired in `gate-first`, the evidence for switching to `enforce` |
| Tool calls per session | `pre` events per session (median, p75, max) and per tool. Journals without `pre`/`post` events: every record with a tool counts |
| Sessions with reviewer evidence | Sessions containing a spawn of the reviewer agent (`--reviewer-agent-regex`, default `^code-reviewer$`) or records from an agent of that type |
| Declared vs actual review (sessions) | Of the sessions whose records claim a review (a `--review-claim-field`, defaults `review_claimed`, `reviewed`, …, that is `true`/`1` or a string matching `--review-claim-regex`, default `^(true\|yes\|y\|1)$\|\breviewer\b` — so `inline-self-review` is not a claim of a reviewer): the share with reviewer evidence. teamwright's own journal has no claim field — `no data` there is expected |
| Declared review with any subagent (upper bound) | Of the same sessions: the share with any subagent spawn. For journals that did not record the subagent type, this is the only honest bound |
| Declared vs actual review (tasks) | Of the `→ CODE_COMPLETE` transitions that fall inside the journal's time span: the share with a journaled reviewer spawn naming the task id. Approvals outside the span are not counted, so a journal started last week does not make older approvals look unreviewed |
| Permission denials per tool | `PermissionDenied` records (auto-mode denials) |
| `pre` without `post` per tool | Heuristic: deny-rule refusals leave a `pre` with no `post`; so do interrupted calls |

Journals of other harnesses need two adjustments, both automatic:

- **Claims without a session key.** When outcome/claim records share no session id with
  any tool call and carry at most one distinct session value (a fixed label, or none),
  each is attached to the tool session that ended nearest to it, within
  `--claim-window` (10) minutes (`--claim-join auto|session|time`). The report names the
  join used. Unattached records are not a session.
- **Subagent type recorded late.** If `Agent`/`Task` calls without a type precede the
  first typed one, sessions that started before it are left out of reviewer evidence
  (the report prints the date): "no reviewer" there would be a false zero. The upper
  bound above still covers them. A journal whose spawns never name a task makes the
  task-level approval match `no data`, not 0.

## Before / after: `--split-date`

`--split-date YYYY-MM-DD` computes every summary metric twice — before that day and from
that day on — and prints the delta (percentage points for shares). Typical uses: the
day the process (or one gate) was adopted, a model change, a team change.

- Git metrics use commit dates; task metrics put a task in the period of its last
  `DONE` (or its last transition, if not done); events (escalations, verify failures)
  use their own dates; journal metrics use the session's first timestamp.
- Compare shares and medians, not raw counts: periods differ in length (commits per
  active month is given for that reason).
- Check n on both sides. A delta over five tasks is an anecdote.
- Recurrence needs history: the look-back crosses the split, so the first weeks after
  the split still "see" fixes from before it — which is the honest reading.

## Known biases

- **Trailer coverage.** Agent share counts only commits that carry the trailer. A
  harness setting that stops adding it, or squashing, makes the share collapse with no
  change in who writes the code. The break detector catches a sustained drop against the
  history's own baseline; a gradual drift, a break in the first weeks of history (no
  baseline yet) or a partial week around the switch is not caught, and a real exodus of
  agents looks the same as a setting change. Human co-authors whose names match a
  pattern would overcount; the defaults avoid plain first names (a human "Claude" does
  not match without a model word or the vendor's e-mail).
- **Fix detection by wording.** A fix is a commit that says so. Commits that fix without
  saying so are missed; a "fixes #12" in a feature subject is counted; a team that
  starts writing `fix:` for every review round (a defect-and-root-cause sprint) doubles
  the fix share with no change in quality. The default also matches the bare word "fix"
  anywhere in the subject, which reads a few points higher than the conventional type
  alone (`--fix-regex '^fix(\(|:|!)'`). Tasks fixed again is less sensitive to wording
  volume but still needs the task id in the fix subject.
- **Squash and rebase** rewrite dates and merge several fixes into one commit; the
  recurrence rate then drops for reasons unrelated to quality.
- **Area granularity** changes the recurrence rate: `dir` groups unrelated files in big
  directories (the rate saturates near 100% in a monorepo), `file` misses fixes that
  move between files of one module, and one very large file that most fixes touch keeps
  even the file-level rate high — then read the hotspots and tasks fixed again, not the
  rate. Files touched only by tooling (version bumps, generated files, snapshots) inflate
  recurrence unless excluded (`--exclude-regex`, `--version-bump-regex`). Report the
  setting with the number (the report prints it).
- **Tolerant parsing reads prose.** The default transition parser accepts any shouted
  word after an arrow, so a ledger line like `T-1 → ROOT_ESCALATION` becomes a status,
  and the clause fallback can attribute an arrow to the wrong id in a long subject.
  On a real history this was about 1% of transitions. Pass a strict
  `--transition-regex` when the ledger is machine-written.
- **Transition commits** count as commits; in teamwright repositories they inflate
  commit counts and dilute the agent and fix shares. Compare like with like.
- **Journals are local** (`.teamwright/logs/` is gitignored): a machine that did not
  collect them is absent, not zero. Merge journals from several machines with `--logs`.
- **Goodhart.** No metric here is a target. Rework rate falls fastest by not sending
  work back; recurrence rate falls by not calling fixes fixes. Read the numbers
  together and against the diffs.
