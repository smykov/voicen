# <Area name>

<!-- Why this area of the code is shaped the way it is. Read BEFORE editing the area, not after a test fails.
     Copy to docs/decisions/<area>.md and add a row to the index in CLAUDE.md.
     Only the one-line must-knows go into CLAUDE.md; the reasoning lives here. -->

**Code:** `<path(s)>` · **Tests that pin it:** `<test names>`

## Invariants

### <Invariant, stated as what must stay true>

- **Defect that produced it:** F-NNN — <one or two lines: what broke>
- **What breaks if you violate it:** <observable consequence>
- **Where it is enforced:** <test / guard / single seam>
- **Don't:** <the tempting change that re-introduces the defect>

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| <e.g. rewrite with lock X> | <reverted N times, incident each time> | F-NNN, decisions.md #N |

## Open

- <link to OQ-NN if any>
