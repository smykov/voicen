# Data Model: Transcript History (005)

Types are described language-neutrally; Rust names in `voicen-core`, TypeScript names in `src/lib/history/`. The IPC shapes are in [contracts/ipc.md](contracts/ipc.md); the core API is in [contracts/core-history.md](contracts/core-history.md).

## HistoryEntry

One delivered final transcript (requirements §7 glossary "History entry").

| Field | Type | Rules | Source |
|---|---|---|---|
| `id` | u64 | unique, strictly increasing across the life of the file; never reused after Clear or off/on (taken from `next_id`) | FR-001, R5 |
| `text` | string | the delivered final text, full; non-empty after trimming whitespace; ≤ text of a 10-minute recording (~15 000 chars, not enforced) | FR-001 |
| `ready_at_ms` | i64 | UTC milliseconds since the Unix epoch when the final text was ready; display only — never used for ordering | FR-001, FR-005, R5 |
| `engine.kind` | enum `api` \| `local` \| `server` | the engine the dictation ran with | Clarification Q4 |
| `engine.model` | string | model name (`whisper-1`, `small`, …); may be empty for a local server with no model configured | Clarification Q4 |

Nothing else is stored: no audio, keys, base URL, target window or language (FR-001, NFR-06).

## History (in memory)

| Field | Type | Meaning |
|---|---|---|
| `enabled` | bool | from 004 settings |
| `size` | u8 (1–100, validated by 004) | N |
| `entries` | list of HistoryEntry, newest first, `len ≤ size` | the list shown |
| `next_id` | u64 | next id to assign |
| `pending_disk` | none \| `write` \| `delete` | a disk operation that failed and must be retried at the next change (FR-009) |
| `notice` | none \| `save_failed` \| `delete_failed` | shown in the window until the next successful write or deletion |

### Invariants

1. `entries.len() <= size` at all times, after every operation (FR-002, SC-002).
2. `enabled == false` ⇒ `entries` is empty and no history operation creates or writes a file (FR-008, NFR-06). The only disk operations while off are deletions.
3. Ids in `entries` are strictly decreasing from first to last (newest first by insertion).
4. After a successful disk operation, the stored document equals `{ next_id, entries }` exactly; no other file holds entry text (SC-003).
5. Errors and notices never contain entry text (FR-011).

## Stored document (`history.json`, schema 1)

```json
{ "schema": 1, "next_id": 26, "entries": [ { "id": 25, "text": "…", "ready_at_ms": 1790000000000, "engine": { "kind": "api", "model": "whisper-1" } } ] }
```

- Written only while enabled, atomically via `history.json.tmp` + rename (R3).
- Unknown `schema`, invalid JSON, or an entry violating the rules above ⇒ the whole document is treated as corrupt ⇒ empty history, `load` warning (FR-010). Not backed up.
- Entries beyond `size` in a loaded file are trimmed and the file rewritten (N may have been lowered while a deletion was failing).

## State transitions

```text
                 apply_settings(enabled=false)        [delete files; entries=[]]
     ┌──────────────────────────────────────────────────────────────┐
     │                                                              ▼
   [ON] ──record(text) → push front, trim to size, write ──┐      [OFF]
     ▲  ──clear() → entries=[], delete files ──────────────┤        │ record(): no-op
     │  ──apply_settings(size↓) → trim, write ─────────────┘        │ clear(): delete files (idempotent)
     │                                                              │
     └──────────── apply_settings(enabled=true) → entries=[] ───────┘

  start: enabled=false → delete files → OFF
         enabled=true  → load (corrupt/unreadable ⇒ empty + warning; delete stale .tmp) → trim to size → ON
```

Disk failure on any transition: memory changes anyway; fallback per R4; `pending_disk` + `notice` set; the next transition first retries `pending_disk`; success clears `notice`.

## HistoryView (sent to the UI)

| Field | Type | Meaning |
|---|---|---|
| `enabled` | bool | `false` ⇒ the window shows the "History is off" state (FR-012) |
| `entries` | HistoryEntry[] | newest first; empty when off |
| `notice` | `null` \| `"save_failed"` \| `"delete_failed"` | persistent warning in the window |

## History settings (owned by 004, consumed)

`history.enabled: bool` (default `true`), `history.size: integer 1–100` (default `20`). Range and defaults are defined once in 004's settings model (P-010).
