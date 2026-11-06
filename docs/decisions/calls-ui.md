# Calls window (UI)

**Code:** `src/routes/calls/+page.svelte`, `src/lib/calls/{callsApi,view}.ts`, the IPC mock handlers in `e2e/support/tauriMock.ts`, catalog ids `calls.error.io.{missing,not_a_folder,access_denied,disk_full,other}` in `i18n/{en,ru}.json` (T-143) · **Tests that pin it:** `src/lib/calls/callsApi.test.ts`, `src/lib/calls/view.test.ts`, `e2e/calls.spec.ts` (data derived from `e2e/fixtures/calls-wire.json`, pinned by T-141). Contract: `specs/007-call-mode-record/contracts/ipc.md`. Core side: `docs/decisions/call-storage.md`. Mutation record: `docs/tasks/T-143.md` › Tests.

The page renders only what the `calls_*` commands and `settings_get` return, through the one IPC module `callsApi.ts`. Transcript text reaches the DOM only as text nodes. Every rendered string is a catalog id present in EN and RU.

## Invariants

### D1 — List order is core's

- **What breaks if you violate it:** core sorts with `newest_first` (stamp, numeric suffix); a client sort puts calls in a different order than the rest of the app.
- **Where it is enforced:** `e2e/calls.spec.ts` (reversed-list case, AC-1, AC-2).
- **Don't:** sort, reverse or re-title the list in the page. Titles and names are wire text (a damaged title is rendered by core in the UI language).

### D2 — Date and time are the digits as written

- `YYYY-MM-DD` and `HH:MM` are taken from the `startedAt` text (with or without offset) and shown as `DD.MM.YYYY HH:MM` in both languages, matching `calls.title_default`. The wall clock of the recording is the local time FR-033 means, and the result does not depend on the host's timezone. Duration is `H:MM:SS` (`MM:SS` under 1 h), `durationMs: null` shows `—`. Line time from `startMs` has the same form.
- **Where it is enforced:** `view.test.ts` (`+03:00`, `-05:00`, `Z`, no offset, near midnight and year end), e2e AC-1.
- **Don't:** use `Date` or `Intl` on `startedAt`.

### D3 — The action set of a row is one pure function of its state

- `actionsFor(state)`: ready = copy, export md, export json, open folder, delete; recording = open folder; queued, transcribing, failed, recovered = open folder, delete; damaged = open folder, delete, and the row does not open the detail (`calls_get` is `ioFailed` for it).
- T-148 adds Transcribe / Transcribe again. Delete is rendered from `actionsFor` because the damaged row must offer it, but its behaviour (confirmation, `recordingCall`, `inUse`) is T-154's.
- **Where it is enforced:** `view.test.ts` (`actionsFor`), e2e D3.

### D4 — A failed action shows its reason

- `callErrorMessageId(error)`: `ioFailed{reason}` maps to `calls.error.io.<reason>`; an unknown or absent reason maps to `calls.error.io.other`; `notFound` maps to `calls.error.io`. One mapper serves list, detail, copy, export and open-folder failures; one `role=alert` shows the text. A cancelled export (`saved: false`) shows nothing.
- **Alert scoping:** the error is tagged by its source (`list`, `detail`, `other`). A successful list refetch clears only a `list` error, a successful `calls_get` only a `detail` error; an action or a selection clears any.
- **Where it is enforced:** `view.test.ts` (mapper, ids exist in EN and RU), the failure-branch e2e tests (T-143 mutant record).
- **Don't:** swallow a rejection at the call, or map every reason to a constant.

### D5 — Search is text, never markup

- Case-insensitive substring over `line.text` (`segmentsFor` matches `RegExp(escaped query, "giu")`: Unicode case folding, Cyrillic included, original case kept); all lines stay visible; matches are `<mark>` built from text segments (`segmentsFor`). Zero matches over a non-empty query shows `calls.no_matches`.
- **Don't:** `{@html}` for marks; transcript text is untrusted.

### D6 — Speaker colour is never alone

- The label is `speakers[].name` for `line.speaker`; a fixed colour per speaker id (`me`, `s1`) sits next to the label text.

### D7 — Test data comes from the wire fixture

- The `tauriMock` helpers derive every case from `calls-wire.json` (a slice of `list.calls`, `detail` with only `text` replaced, an empty list with the fixture's `unavailableRoots`, errors from `errors.*`). No hand-written wire object; the fixture is core-generated and is not edited by hand.

### D8 — Events

- `calls://changed` refetches the list and, if its `{root,id}` matches, the open detail; `calls://open` selects that call; `settings://changed` updates the language and the hotkey and refetches the list, because titles are rendered in the UI language.

## Deviations the developer made (T-143)

- **Listener payload types are generic** (`onCallsChanged<P extends CallRef = CallRef>`, same for the other listeners), so svelte-check accepts handlers typed on a narrower payload.
- **`callsApi.ts` re-exports `onSettingsChanged` from `settingsApi.ts`**, so `settings://changed` and `settings_get` (`getSettings`) are each declared once; the listener is generic in its payload type (`docs/decisions/settings-ui.md` C).
- **Any non-`ioFailed` code maps to `calls.error.io` for now.** More specific texts belong to T-154 (delete) and the rename tasks.
- **A failed `settings_get` or a refused listen shows `error.ipc_unavailable`** in the same alert.

## Open

- T-142 must grant `allow-settings-get` and the event listen to the `calls` label (`docs/decisions/ipc-acl.md`); T-143's tests use the mocked IPC and do not prove the grant.
