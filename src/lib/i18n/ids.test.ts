// No string-typed message-id path in the UI (docs/decisions/i18n.md; T-004 review r1 #1,
// T-039). These are type-level checks: svelte-check (`pnpm lint`, `pnpm check`)
// type-checks this file, and each `@ts-expect-error` line fails the check when the line
// below it compiles, that is when a string-typed path to a catalog text is accepted.
//
// What the lines below pin:
// - `t(id: MessageId)` is the only public renderer of a catalog id; an unknown literal
//   id is a type error.
// - No public renderer takes any string: `tWire` is gone, and the shared lookup + render
//   rule `text(catalog, lang, id: string)` lives in the module's own `render.ts`, not
//   exported from `index.ts`.
// - An id from the wire is typed `MessageId` at the boundary: `FormError.message`
//   (core's message_ids_exist_in_both_catalogs), or built by one of the two documented
//   casts, `errorMessageId(code)` -> `error.<code>` (core's
//   every_error_code_has_catalog_text) and `fieldLabelId(field)` ->
//   `settings.field_label.<FieldId>` (core's every_field_id_has_label_text).
//
// The vitest run only proves the file loads; the bodies are never executed.
import { describe, expect, it } from "vitest";
import * as i18n from "./index";
import type { FormError } from "../settings/settingsApi";

/** Type-checked, never called. */
function typeLevelOnly(): void {
  // `t` refuses an unknown literal id.
  // @ts-expect-error -- "settings.typo" is not a MessageId
  i18n.t("settings.typo");

  // No public renderer takes any string.
  // @ts-expect-error -- a string-typed renderer would render the raw id "settings.typo"
  i18n.tWire("settings.typo");

  // T-039 (red until text moves to render.ts and is no longer re-exported).
  // @ts-expect-error -- text() takes any string id; it is internal to the i18n module
  i18n.text({ en: {}, ru: {} }, "en", "settings.typo");

  // FormError.message is a catalog id, not any string.
  // @ts-expect-error -- "settings.typo" is not a MessageId
  const formError: FormError = { kind: "write_failed", message: "settings.typo" };
  void formError;
}

describe("message-id types (checked by svelte-check)", () => {
  it("the type-level checks of this file are loaded", () => {
    expect(typeof typeLevelOnly).toBe("function");
  });
});
