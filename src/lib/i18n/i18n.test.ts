// T-005 red tests for the UI side of the message catalog.
// Rust runs the same fixture in crates/voicen-core/src/i18n.rs (tests::conformance_fixture).
import { afterEach, describe, expect, it, vi } from "vitest";
import conformance from "$i18n/conformance.json";
import en from "$i18n/en.json";
import ru from "$i18n/ru.json";
import { catalogProblems, text, type Catalog, type UiLanguage } from "./index";

/** Format: the `_format` key of `i18n/conformance.json`. */
interface ConformanceCase {
  name: string;
  en: Record<string, string>;
  ru: Record<string, string>;
  lang: UiLanguage;
  id: string;
  args: Record<string, string>;
  expected: string;
}

const cases = (conformance as { cases: unknown[] }).cases as ConformanceCase[];

/** A fresh copy of the module, so state left by one test does not leak into the next. */
async function freshI18n(): Promise<typeof import("./index")> {
  vi.resetModules();
  return import("./index");
}

afterEach(() => {
  vi.unstubAllGlobals();
});

// ---- AC1: one rendering rule, shared with Rust ---------------------------------

describe("i18n rendering", () => {
  it("conformance fixture", () => {
    // Guard: an emptied or truncated fixture must not pass vacuously.
    expect(cases.length).toBeGreaterThanOrEqual(15);
    for (const c of cases) {
      const catalog: Catalog = { en: c.en, ru: c.ru };
      expect(text(catalog, c.lang, c.id, c.args), `case ${JSON.stringify(c.name)}`).toBe(c.expected);
    }
  });
});

// ---- AC3: catalog parity names the offending id --------------------------------

describe("catalog parity", () => {
  it("accepts a consistent pair", () => {
    expect(catalogProblems({ en: { a: "Hello", b: "{x}" }, ru: { a: "Привет", b: "{x}" } })).toEqual([]);
  });

  it("names an id missing from ru", () => {
    expect(catalogProblems({ en: { a: "A", "b.id": "B" }, ru: { a: "А" } })).toEqual([
      { id: "b.id", kind: "missing", lang: "ru" },
    ]);
  });

  it("names an id missing from en", () => {
    expect(catalogProblems({ en: { a: "A" }, ru: { a: "А", "c.id": "В" } })).toEqual([
      { id: "c.id", kind: "missing", lang: "en" },
    ]);
  });

  it("names an id with an empty text", () => {
    expect(catalogProblems({ en: { a: "A", "b.id": "B" }, ru: { a: "А", "b.id": "" } })).toEqual([
      { id: "b.id", kind: "empty", lang: "ru" },
    ]);
    expect(catalogProblems({ en: { "a.id": "", b: "B" }, ru: { "a.id": "А", b: "Б" } })).toEqual([
      { id: "a.id", kind: "empty", lang: "en" },
    ]);
  });

  it("en and ru have the same ids and non-empty texts", () => {
    // On failure the diff lists each offending id with its kind and language.
    expect(catalogProblems({ en, ru })).toEqual([]);
  });
});

// ---- AC3 failure branch / AC1: t() over the real catalog and current language ----

describe("t", () => {
  it("renders English before any language is set", async () => {
    // The UI never derives a language itself (invariant 4): a Russian browser
    // locale must not change the starting language.
    vi.stubGlobal("navigator", { language: "ru-RU", languages: ["ru-RU", "ru"] });
    const { t } = await freshI18n();
    expect(t("app.build_info_error", { reason: "disk" })).toBe("Cannot read build info: disk");
    expect(t("app.build_info", { version: "1.2.3", commit: "abc1234" })).toBe("Voicen 1.2.3 (abc1234)");
  });

  it("renders ru after the language is set to ru", async () => {
    const { t, setLanguage } = await freshI18n();
    setLanguage("ru");
    // `$&` in the value pins that t() goes through the literal renderer.
    expect(t("app.build_info_error", { reason: "E$&1" })).toBe("Не удалось прочитать сведения о сборке: E$&1");
  });

  it("renders en again after switching back from ru", async () => {
    const { t, setLanguage } = await freshI18n();
    setLanguage("ru");
    setLanguage("en");
    expect(t("app.build_info_error", { reason: "disk" })).toBe("Cannot read build info: disk");
  });
});
