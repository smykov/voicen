// The settings window's draft model (spec 004 T037, T-004 U1-U3). Pure TS: the page
// holds a Draft in $state and replaces it with what these functions return.
//
// - U1: every SettingsView the window gets goes through `applyView` (settings_get,
//   settings://changed) or `applyOutcome` (Saved.view -> `draftFromView`, the same
//   clean draft `applyView` gives a clean draft, and it must also drop key edits); the
//   window builds no defaults, no validation rule and no language list of its own
//   (docs/decisions/settings-ui.md).
// - U2: a refusal maps each FieldError to its field as message id `error.<code>`
//   (`errorMessageId`, the one helper; no list of codes here), and each field of a
//   partially_restored form error that has no FieldError of its own to
//   `NOT_RESTORED_ID`.
// - U3: a typed key lives only in `keys` until the next Saved, which resets every
//   slot to "Untouched"; a view carries presence only.
import type {
  ConnectionTestRequest,
  FieldError,
  FormError,
  KeyEdit,
  KeySlot,
  SaveOutcome,
  SaveRequest,
  Settings,
  SettingsView,
  Warning,
} from "./settingsApi";
import { KEY_SLOTS } from "./settingsApi";
import type { MessageId } from "../i18n";

/** The message of a field named in `FormError.not_restored` (UI-only id). */
export const NOT_RESTORED_ID: MessageId = "settings.field.not_restored";

export interface Draft {
  /** The last applied view: what is saved. */
  baseline: SettingsView;
  /** The editable copy of the settings (never aliased to a view). */
  settings: Settings;
  /** One edit per key slot; "Untouched" until the user types or clears. */
  keys: Record<KeySlot, KeyEdit>;
  /**
   * FieldId -> message id of the last refusal: `error.<code>` for a FieldError, else
   * `NOT_RESTORED_ID` for a field named in a partially_restored form error.
   */
  errors: Record<string, MessageId>;
  /** The last refusal's form error. */
  formError: FormError | null;
}

/** A deep copy of wire data (JSON values only). Also reads through Svelte state proxies. */
function copy<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function deepEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const ka = Object.keys(a);
  const kb = Object.keys(b);
  if (ka.length !== kb.length) return false;
  return ka.every(
    (k) => Object.hasOwn(b, k) && deepEqual((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k]),
  );
}

function untouched(): Record<KeySlot, KeyEdit> {
  return Object.fromEntries(KEY_SLOTS.map((slot) => [slot, "Untouched"])) as Record<KeySlot, KeyEdit>;
}

function copyKeys(keys: Record<KeySlot, KeyEdit>): Record<KeySlot, KeyEdit> {
  return Object.fromEntries(KEY_SLOTS.map((slot) => [slot, copy(keys[slot])])) as Record<KeySlot, KeyEdit>;
}

/** A clean draft of `view`: settings copied, every key Untouched, no errors. */
export function draftFromView(view: SettingsView): Draft {
  return {
    baseline: copy(view),
    settings: copy(view.settings),
    keys: untouched(),
    errors: {},
    formError: null,
  };
}

/**
 * The one path for a view from the shell (U1). A clean draft is replaced by the view;
 * a dirty one keeps its edits, key edits and errors, and only its baseline moves.
 */
export function applyView(draft: Draft, view: SettingsView): Draft {
  if (!isDirty(draft)) return draftFromView(view);
  return {
    baseline: copy(view),
    settings: copy(draft.settings),
    keys: copyKeys(draft.keys),
    errors: { ...draft.errors },
    formError: draft.formError === null ? null : copy(draft.formError),
  };
}

/** The settings differ from the saved ones (by value), or a key slot has an edit. */
export function isDirty(draft: Draft): boolean {
  return !deepEqual(draft.settings, draft.baseline.settings) || KEY_SLOTS.some((slot) => draft.keys[slot] !== "Untouched");
}

function withKey(draft: Draft, slot: KeySlot, edit: KeyEdit): Draft {
  const keys = copyKeys(draft.keys);
  keys[slot] = edit;
  return { ...draft, keys };
}

/** The user typed `text` into a key field: Replace with the text as typed (the service trims). */
export function typeKey(draft: Draft, slot: KeySlot, text: string): Draft {
  return withKey(draft, slot, { Replace: text });
}

/** The user asked to delete the stored key of `slot`. */
export function clearKey(draft: Draft, slot: KeySlot): Draft {
  return withKey(draft, slot, "Clear");
}

/** The key field of `slot` was emptied again: no edit, the stored key stays. */
export function resetKey(draft: Draft, slot: KeySlot): Draft {
  return withKey(draft, slot, "Untouched");
}

/** The `settings_save` request: the whole draft plus one KeyEdit per slot. */
export function saveRequest(draft: Draft): SaveRequest {
  return { settings: copy(draft.settings), keys: copyKeys(draft.keys) };
}

/**
 * The `settings_test_connection` request of the draft's selected engine, or null when
 * that engine has no test (none, builtin_local). api: `settings.api` and the
 * `transcription_api` KeyEdit; local_server: `settings.local_server` and the
 * `local_server` KeyEdit; both with the draft's timeouts. Values are copies, as typed
 * (no trim, no validation: core normalizes and checks); the draft is not changed.
 */
export function testRequest(draft: Draft): ConnectionTestRequest | null {
  const { engine } = draft.settings;
  if (engine !== "api" && engine !== "local_server") return null;
  const slot: KeySlot = engine === "api" ? "transcription_api" : "local_server";
  const target = engine === "api" ? draft.settings.api : draft.settings.local_server;
  return {
    engine,
    base_url: target.base_url,
    model: target.model,
    key: copy(draft.keys[slot]),
    timeouts: copy(draft.settings.timeouts),
  };
}

/**
 * The draft after an `invalid` connection-test result: its errors replace the
 * highlights (`errorsByField`, the U2 path); settings, key edits, baseline and the form
 * error are kept. The input draft is not changed.
 */
export function applyTestErrors(draft: Draft, errors: readonly FieldError[]): Draft {
  return { ...draft, errors: errorsByField(errors) };
}

/**
 * The draft after a `settings_save` outcome. Saved: a clean draft of the saved view
 * (no key kept, no highlight). Refused: the highlights of the previous outcome are
 * dropped and replaced by this one's (each FieldError, plus each `not_restored` field
 * without a FieldError); the draft, its key edits and the baseline are kept.
 */
export function applyOutcome(draft: Draft, outcome: SaveOutcome): Draft {
  if ("Saved" in outcome) return draftFromView(outcome.Saved.view);
  const { errors, form_error } = outcome.Refused;
  const byField = errorsByField(errors);
  for (const field of form_error?.not_restored ?? []) {
    if (!Object.hasOwn(byField, field)) byField[field] = NOT_RESTORED_ID;
  }
  return {
    baseline: draft.baseline,
    settings: draft.settings,
    keys: draft.keys,
    errors: byField,
    formError: form_error === null ? null : copy(form_error),
  };
}

/** FieldId -> `error.<code>`; the first error of a field wins. */
export function errorsByField(errors: readonly FieldError[]): Record<string, MessageId> {
  const byField: Record<string, MessageId> = {};
  for (const { field, code } of errors) {
    if (!Object.hasOwn(byField, field)) byField[field] = errorMessageId(code);
  }
  return byField;
}

/**
 * FieldId -> the warning's own `message`, as core sent it; the first warning of a field
 * wins. No id is built from the code and no URL rule is applied here: core decides which
 * field warns and with which text (decision #52). The page keeps the result of the last
 * Saved outcome beside `saved`, never in the Draft: the settings://changed echo of the
 * same save rebuilds the Draft (`applyView` -> `draftFromView`) and would drop it.
 */
export function warningsByField(warnings: readonly Warning[]): Record<string, MessageId> {
  const byField: Record<string, MessageId> = {};
  for (const { field, message } of warnings) {
    if (!Object.hasOwn(byField, field)) byField[field] = message;
  }
  return byField;
}

/**
 * The catalog id of an ErrorCode: `error.<code>`, for any code (no list of codes in
 * the UI). One of the two casts from a wire string to `MessageId` in the UI (the other
 * is `fieldLabelId`): core's `every_error_code_has_catalog_text` guarantees that
 * `error.<code>` has a text in both catalogs for every `ErrorCode` it can send
 * (docs/decisions/i18n.md).
 */
export function errorMessageId(code: string): MessageId {
  return `error.${code}` as MessageId;
}

/**
 * The catalog id of a FieldId's label: `settings.field_label.<FieldId>`, for any field
 * (no FieldId -> label table in the UI). The second documented cast from a wire string
 * to `MessageId`: core's `every_field_id_has_label_text` guarantees that the id has a
 * text in both catalogs for every `FieldId` it can send (docs/decisions/i18n.md).
 */
export function fieldLabelId(field: string): MessageId {
  return `settings.field_label.${field}` as MessageId;
}
