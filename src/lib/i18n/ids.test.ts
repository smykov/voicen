// T-004 review r1 #1: no string-typed message-id path in the UI (docs/decisions/i18n.md,
// T-031 AC2 on the UI side). These are type-level checks: svelte-check (`pnpm lint`,
// `pnpm check`) type-checks this file, and each `@ts-expect-error` line fails the check
// when the line below it compiles, that is when an unknown literal id is accepted.
//
// Intended API (what the lines below pin):
// - `t(id: MessageId)` stays the only public renderer of a catalog id.
// - `tWire(id: string)` is removed (or narrowed so a literal like "settings.typo" no
//   longer fits, e.g. `` `error.${string}` ``); a field error renders as
//   `t(errorMessageId(code))`, with `errorMessageId(code: string): MessageId` holding the
//   one documented cast (core's every_error_code_has_catalog_text guarantees the text).
// - `FormError.message` is typed `MessageId` at the wire boundary (settingsApi.ts; core's
//   message_ids_exist_in_both_catalogs guarantees it), so it renders with `t`.
//
// The vitest run only proves the file loads; the bodies are never executed.
import { describe, expect, it } from "vitest";
import * as i18n from "./index";
import type { FormError } from "../settings/settingsApi";

/** Type-checked, never called. */
function typeLevelOnly(): void {
  // Characterization (green today): `t` refuses an unknown literal id.
  // @ts-expect-error -- "settings.typo" is not a MessageId
  i18n.t("settings.typo");

  // T-004 r1 #1 (red until tWire is removed or narrowed): no public renderer takes any string.
  // @ts-expect-error -- a string-typed renderer would render the raw id "settings.typo"
  i18n.tWire("settings.typo");

  // T-004 r1 #1 (red until FormError.message is a MessageId).
  // @ts-expect-error -- FormError.message is a catalog id, not any string
  const formError: FormError = { kind: "write_failed", message: "settings.typo" };
  void formError;
}

describe("message-id types (checked by svelte-check)", () => {
  it("the type-level checks of this file are loaded", () => {
    expect(typeof typeLevelOnly).toBe("function");
  });
});
