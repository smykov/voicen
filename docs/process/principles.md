# Principles and enforcement tiers

A principle that nobody checks decays into a wish. Each principle in `PRINCIPLES.md` therefore carries an **enforcement tier**: how, concretely, a violation gets caught.

## 1. The tiers

| Tier | Mechanism | Catches a violation… | Cost |
|---|---|---|---|
| **T0** | Written guidance (`PRINCIPLES.md`, `CLAUDE.md`, decision files) | only if the agent reads and remembers it | Free, weakest |
| **T1** | Reviewer checklist item | at review, if the reviewer applies judgment | One review pass |
| **T2** | Hook warns (pre-commit, tool hook, commit-msg) | at the moment of the act, with a message | Script + false-positive noise |
| **T3** | Hook or CI **blocks** | always; bypass requires an explicit, recorded override | Script + maintenance + an exit path |

Order of strength: **T3 > T2 > T1 > T0**. Prefer the "golden path" too: make the right thing the easy thing (a shared helper, a template field), which often beats any tier.

## 2. Why rules migrate up the tiers

Rules start at T0 because that's cheap. They move up after incidents:

```
incident → failures.md entry (F-NNN) → principle at T0/T1
         → same class recurs → T2 warning
         → recurs despite the warning, or the cost of one miss is high → T3 block
```

Signals that a rule must move up:
- the same class appears in `docs/failures.md` twice;
- a reviewer checklist item was missed on a merged change;
- measurement (git, logs) shows the rule is followed less than people believe;
- one miss costs a production incident, a data leak or a secret exposure (these start at T3).

Every T3 needs an **exit state**: a documented, recorded way to proceed when the block is wrong (for example a commit trailer with a reason). A gate without an exit gets bypassed silently or stalls the whole flow — both are worse than no gate.

Rules may also move **down** or be retired when measurement shows they no longer fire. Record either move in `docs/decisions.md`.

## 2a. Amending the principles (recurrence exit c)

A recurrence is sometimes a **process** defect: the code did what the process allowed — a template lacked a field, a rule had no check, a gate had no exit so it was bypassed. Fixing the code again changes nothing; the rule has to change. This is exit (c) of the recurrence path (`lifecycle.md` §6).

- **The owner decides.** The investigator proposes the amendment (the new or superseding entry, its tier and check); agents never amend the principles on their own.
- **Separate commit.** The amendment is its own commit — `docs(principles): P-NNN <what> (v1.3.0)` — not mixed with the code of any task, so the history shows when the rule changed and what changed with it.
- **Version bump** in the header of `PRINCIPLES.md`: *major* — a principle removed or its meaning reversed; *minor* — a principle added or moved up a tier; *patch* — wording, links, "how checked" details.
- **Recorded** in `docs/decisions.md` (Was → Decided, with the rca task id) and, when it came from an incident here, linked from the `docs/failures.md` entry.
- **If the project's spec tool keeps a constitution** (Spec Kit: `.specify/memory/constitution.md`), `PRINCIPLES.md` is the source: the constitution references it or is updated in the same commit with the same version, never diverges.
- **Make it bite.** An amendment that stays at T0 is a wish; say which checklist line, template field or hook makes the new rule the default (§3).

## 3. Canon vs working path

> If the canon requires X but the working path doesn't allow X, fix the path — don't blame the agent.

The canon (docs, protocol, principles) and the working path (permissions, templates, tool availability, scripts) live in different files and change at different times. When they disagree, the agent follows the only open path, and a metric that measures X reads 0% as "the agent ignores the rule".

Observed forms of this defect:
- a required tool was cut off by a permission change — usage dropped to zero for weeks while audits kept flagging "rule weakly followed";
- a required output field was described in the protocol but absent from the session template — filled in 0% of sessions; a field merely *mentioned* in the template was filled in most;
- a parser matched an old id format — its report silently showed "nothing to do" while real items waited.

Checks:
- For each rule, ask: **what in the working path makes this the default?** If nothing, the rule is T0 at best.
- **A template beats prose**: what's not in the template doesn't happen.
- When a metric reports zero, first suspect the extractor or the path, then the agent.
- When an agent routes around a tool by hand repeatedly, the tool is the bug.

## 4. Writing a principle

Each entry in `PRINCIPLES.md`:

| Field | Content |
|---|---|
| `id` | `P-NNN`, never reused; changes = new entry, old one marked `Superseded by P-NNN` |
| Statement | One imperative sentence |
| Why | One sentence: the kind of incident that produced the rule, generic (no names, no numbers) |
| Tier | `T0`–`T3` (may list two, e.g. `T1 + T3`) |
| How checked | The concrete check: checklist line, hook name, CI job |
| Origin | Optional. `F-NNN` in `docs/failures.md` once the incident happens in *your* project. The kit ships `docs/failures.md` empty: the stories behind the shipped principles are told in articles, not in the repository |
