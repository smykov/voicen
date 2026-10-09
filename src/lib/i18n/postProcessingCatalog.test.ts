// T-021 red unit test: the catalog texts of the Post-processing tab (spec 003 US3, T-021
// analysis "seam"). The e2e spec e2e/settings-post-processing.spec.ts renders these ids
// from the catalog; this file pins what the catalog itself must hold:
// - every id the tab renders has a non-empty text in en and in ru;
// - the privacy note is the text of specs/003-llm-post-processing/contracts/ipc.md
//   (`settings.post_processing.privacy_note`), word for word, in both languages.
import { describe, expect, it } from "vitest";
import en from "$i18n/en.json";
import ru from "$i18n/ru.json";

const CATALOGS: Record<"en" | "ru", Record<string, unknown>> = { en, ru };

/** The ids the Post-processing tab renders (beyond the shared field, key and error texts). */
const TAB_IDS = [
  "settings.tab.post_processing",
  "settings.post_processing.privacy_note",
  "settings.field.post_processing_enabled",
  "settings.field.prompt",
  // The toggle's FieldId label (core i18n::tests::every_field_id_has_label_text, rule L).
  "settings.field_label.post_processing.enabled",
] as const;

/** specs/003-llm-post-processing/contracts/ipc.md, catalog table. */
const PRIVACY_NOTE = {
  en: "When post-processing is on, the transcript text is sent to this endpoint.",
  ru: "Когда постобработка включена, текст расшифровки отправляется на этот адрес.",
} as const;

describe("Post-processing tab catalog texts (T-021)", () => {
  for (const lang of ["en", "ru"] as const) {
    it(`${lang}: every id the tab renders has a non-empty text`, () => {
      const missing = TAB_IDS.filter((id) => {
        const value = CATALOGS[lang][id];
        return typeof value !== "string" || value.trim() === "";
      });
      expect(missing).toEqual([]);
    });

    it(`${lang}: the privacy note is the 003 contract text`, () => {
      expect(CATALOGS[lang]["settings.post_processing.privacy_note"]).toBe(PRIVACY_NOTE[lang]);
    });
  }

  it("ru texts are translations, not copies of en", () => {
    const copied = TAB_IDS.filter((id) => typeof en[id as keyof typeof en] === "string" && ru[id as keyof typeof ru] === en[id as keyof typeof en]);
    expect(copied).toEqual([]);
  });
});
