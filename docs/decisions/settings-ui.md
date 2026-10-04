# Settings window (UI)

**Code:** `src/routes/settings/+page.svelte`, `src/lib/settings/{draft,settingsApi,fields}.ts`, `src/lib/settings/{FieldMessage,KeyField,HotkeyField}.svelte`, `src/lib/settings/tabs/*.svelte`, the IPC mock `e2e/support/tauriMock.ts` (T-004) · **Tests that pin it:** `e2e/settings-first-run.spec.ts` (every case; per invariant below), `e2e/tauri-mock.spec.ts` (the mock's own contract), `src/lib/settings/draft.test.ts`, `src/lib/i18n/ids.test.ts` (type-level, run by svelte-check), core `i18n::tests::every_error_code_has_catalog_text` and `settings::service::tests::{e2e_settings_wire_fixture_matches_core, settings_view_wire_form_carries_unavailable}`

Task: T-004. Contract: `specs/004-settings-and-first-run/contracts/ipc.md` (commands, wire form, FieldId, errors). Decisions: #30, #38. Core side: `docs/decisions/settings.md`; message ids: `docs/decisions/i18n.md`. Tabs added later (T-013, T-016, T-021, T-034) live in this route and keep these invariants.

## Invariants

### U1 — The window derives nothing: every value comes from a SettingsView

- **Defect that produced it:** none in the failure log (found in analysis, T-004). Specs 001–003 each planned UI defaults or language lists beside core's (P-010).
- **What breaks if you violate it:** the window shows a default, a rule or a language that core does not have, so a value the window offers is refused or silently differs from what is stored.
- **Where it is enforced:** every view reaches the draft through `draft.ts`. `settings_get` and `settings://changed` go through `applyView`. A `Saved` outcome goes through `applyOutcome` → `draftFromView`. This is the same clean draft that `applyView` returns for a clean draft, and it must also drop the key edits (U3), so "one applyView" in the T-004 analysis is carried out as these two equivalent entry points. The UI language follows `draft.baseline.settings.ui_language` through one `$effect`, whatever path set the baseline. The speech-language options are exactly `settings_speech_languages` (decision #30; sorting by localized name changes the order only). The interface-language options iterate `LANGUAGES` from `$lib/i18n`, the one list in the UI. The e2e view data is core's own (`e2e/fixtures/settings-wire.json`, pinned by `e2e_settings_wire_fixture_matches_core`). The mock never validates: every refusal is scripted. Tests: the first-run, speech-language, "reflected without reload" and empty-URL cases of `settings-first-run.spec.ts`; `draft.test.ts` › `draftFromView / applyView`.
- **Don't:** build a default, a range or a required-field check in the UI, copy `WHISPER_ISO_639_1`, read the OS locale, or add a third entry point that sets the baseline without going through `draft.ts`.

### U2 — Every refusal reaches the user, localized, on its field

- **Defect that produced it:** T-004 review round 1 #3. A `partially_restored` refusal said "check the highlighted fields", but the field actually left changed (`not_restored`) was not highlighted.
- **What breaks if you violate it:** the user cannot see which value core refused or which value differs from before the save.
- **Where it is enforced:** `applyOutcome` (Refused) is the one mapping. Each `FieldError` becomes `error.<code>` (`errorMessageId`, no list of codes in the UI). Each `form_error.not_restored` field without a `FieldError` of its own becomes `settings.field.not_restored` (UI-only id). The next outcome replaces all of them. Each control carries `data-field="<FieldId>"`, its id comes from `controlId(field)`, and `aria-invalid` / `aria-describedby` point at `FieldMessage` (`errorId(field)`). The form error's `message` (typed `MessageId`) shows at form level. A rejected invoke shows `error.ipc_unavailable` and never the rejection text. The draft is kept in every case. Core guarantees the texts: `every_error_code_has_catalog_text` over `ErrorCode::ALL` (kept complete by hand, see `settings/mod.rs`) and `message_ids_exist_in_both_catalogs`. Tests: the refusal, form-level, rejected-invoke, unavailable and `partially_restored` cases of `settings-first-run.spec.ts`; `draft.test.ts` › `applyOutcome with partially_restored`, `error ids`.
- **Don't:** map only `errors` and drop `not_restored`, show a FieldId or a rejection text, keep a UI table of error codes, or render a wire id with anything but `t` (see i18n.md).

### U3 — No key value is shown or kept

- **Defect that produced it:** none in the failure log (found in analysis, T-004; NFR-04, FR-20). The `resetKey` path came from the implementation and was pinned by T-004 review round 1 #4.
- **What breaks if you violate it:** a key in the DOM, in a view or in a later request, or a stored key refused as missing.
- **Where it is enforced:** `KeyField` input is `type="password"` and starts empty. Presence shows as text only (`settings.key.saved`). The slot's `KeyEdit` is `Untouched` by default, `{Replace: typed}` once typed, and `Clear` from the "Remove key" button. An input emptied again goes back to `Untouched` (`resetKey`). A blank `Replace` would reach core's `validate`, which reads it as `key.required` for engine api even though a key is stored (settings.md I5). A `Saved` resets every slot to `Untouched`. Tests: the "masked", "stored API key" (Untouched, emptied → Untouched, Remove key → Clear) cases of `settings-first-run.spec.ts`; `draft.test.ts` › `resetKey`.
- **Don't:** prefill the input, keep a typed key after `Saved`, or send `{Replace: ""}`.

### No edit is lost to an in-flight save

- **Defect that produced it:** T-004 review round 1 #10. The inputs stayed editable while `settings_save` was pending, and the `Saved` draft replaced edits made in the meantime.
- **What breaks if you violate it:** a change the user made is silently dropped.
- **Where it is enforced:** the panel's `<fieldset disabled={saving}>` in `+page.svelte`. It unlocks with the saved values. Test: "an edit made while a save is in flight is not lost when Saved arrives".
- **Don't:** move a field outside the fieldset, or replace the draft with an outcome while it can still be edited.

### A number input shows what the user typed

- **Defect that produced it:** T-004 review round 1 #9. The history size used a `bind:value` accessor that coerced an empty entry to `0`, and Svelte wrote it back, so clearing and typing `5` gave `05`.
- **What breaks if you violate it:** the input fights the user.
- **Where it is enforced:** `tabs/History.svelte` keeps the typed entry. The draft holds the wire number, which is `0` for an entry that is not an unsigned whole number. Core refuses `0` with `history.size_range`, and the range is core's rule only. Test: "clearing the history size and typing a digit gives that digit, not a leading 0".
- **Don't:** use a coercing `bind:value` accessor on a number input.

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| `tWire(id: string)` to render ids that arrive over IPC | a `string`-typed renderer beside `t(id: MessageId)`: an unknown literal id compiles and renders as the raw id (i18n.md) | T-004 review r1 #1 |
| Validating in the mock or in the UI before `settings_save` | a second copy of core rules (P-010) | decision #38 |
| Wire types declared again in the e2e mock | two TS declarations of one wire; the mock re-exports `settingsApi.ts` | T-004 review r1 #8 |

## Open

- A `not_restored` field is highlighted only where its control is rendered. Examples: `engine.api.key` while another engine is selected, and `general.start_with_windows` until T-034 adds its control. In those cases the form message alone tells the user. Listing such fields by label in the form message would need a FieldId → label map (no owner yet).
