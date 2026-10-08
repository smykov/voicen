// T-072 red e2e: the engine URL fields show an example hint (decision #96; Acceptance
// "The URL fields show an example hint (e.g. http://10.0.0.5:8000/v1) in en/ru").
// Core decides how the URL is joined (`engine::openai::transcription_url`); the page only
// tells the user which forms are accepted. The locator contract of
// e2e/settings-first-run.spec.ts applies (tab-<tab> test ids, data-field controls).
//
// Locator contract this spec adds (T-072 analysis):
// - under each engine URL input (`engine.api.base_url`, `engine.local_server.base_url`)
//   a visible paragraph holds `t(settings.base_url.hint)` in the UI language; it is part
//   of the input's accessible description (aria-describedby, built by
//   `fields.ts::describedBy`), and stays there next to a field warning;
// - the hint is shown only with a URL input: engine none and builtin_local show none;
// - the input's value is the stored setting as typed (no rewrite of a full endpoint URL).
import { readFileSync } from "node:fs";
import type { Page } from "@playwright/test";
import { expect, test } from "./support/boot";
import {
  calls,
  firstRunView,
  installTauriMock,
  listeners,
  queueSaveOutcome,
  savedInsecureApi,
  type SaveRequest,
  type SettingsView,
} from "./support/tauriMock";

type Messages = Record<string, string>;
const catalog = (lang: "en" | "ru"): Messages =>
  JSON.parse(readFileSync(new URL(`../i18n/${lang}.json`, import.meta.url), "utf8")) as Messages;

const HINT_ID = "settings.base_url.hint";
// The owner-decided wording (T-072 ## Investigation). The UI tests assert these literals,
// so they are red on the missing hint, not on a missing catalog entry.
const HINT = {
  en: "For example http://10.0.0.5:8000/v1. A full address ending in /audio/transcriptions also works.",
  ru: "Например, http://10.0.0.5:8000/v1. Подойдёт и полный адрес, оканчивающийся на /audio/transcriptions.",
} as const;

const ENGINES = [
  { engine: "api", url: "engine.api.base_url" },
  { engine: "local_server", url: "engine.local_server.base_url" },
] as const;

function escapeRegExp(s: string): RegExp {
  return new RegExp(s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
}

function field(page: Page, id: string) {
  return page.locator(`[data-field="${id}"]`);
}

/** Collects uncaught page errors from before the first navigation. */
function pageErrors(page: Page): Error[] {
  const errors: Error[] = [];
  page.on("pageerror", (error) => errors.push(error));
  return errors;
}

function viewWith(engine: string, lang: "en" | "ru"): SettingsView {
  const view = firstRunView();
  view.settings.engine = engine as SettingsView["settings"]["engine"];
  view.settings.ui_language = lang;
  return view;
}

async function open(page: Page, view: SettingsView): Promise<void> {
  await installTauriMock(page, { view });
  await page.goto("/settings");
  await expect(page.getByRole("tab", { selected: true })).toBeVisible();
  await expect.poll(() => listeners(page, "settings://changed")).toBeGreaterThan(0);
}

/** The hint is visible under `url`'s input and is in that input's accessible description. */
async function expectHint(page: Page, url: string, text: string): Promise<void> {
  const control = field(page, url);
  await expect(control).toBeVisible();
  const hint = page.getByText(text, { exact: true });
  // Exactly one hint on the page: the one of the shown URL input.
  await expect(hint).toHaveCount(1);
  await expect(hint).toBeVisible();
  await expect(control).toHaveAccessibleDescription(escapeRegExp(text));
  // Under the input, readable (not a 1px visually-hidden description).
  const input = await control.boundingBox();
  const box = await hint.boundingBox();
  expect(input).not.toBeNull();
  expect(box).not.toBeNull();
  expect(box!.y).toBeGreaterThanOrEqual(input!.y + input!.height - 1);
  expect(box!.height).toBeGreaterThanOrEqual(8);
  expect(box!.width).toBeGreaterThanOrEqual(40);
  // A hint is not an error.
  await expect(control).not.toHaveAttribute("aria-invalid", "true");
}

test("the catalog has settings.base_url.hint in en and ru with the decided wording", () => {
  expect(catalog("en")[HINT_ID]).toBe(HINT.en);
  expect(catalog("ru")[HINT_ID]).toBe(HINT.ru);
});

for (const { engine, url } of ENGINES) {
  for (const lang of ["en", "ru"] as const) {
    test(`the ${engine} URL field shows the example hint in ${lang}, in its accessible description`, async ({ page }) => {
      const errors = pageErrors(page);
      await open(page, viewWith(engine, lang));
      await expect(field(page, "engine.kind")).toHaveValue(engine);
      // The page is in `lang` (so a red here is the missing hint, not the language).
      await expect(field(page, url)).toHaveAccessibleName(escapeRegExp(catalog(lang)["settings.field.base_url"]));
      await expectHint(page, url, HINT[lang]);
      // Only the UI language's text is shown.
      const other = lang === "en" ? HINT.ru : HINT.en;
      await expect(page.getByText(other, { exact: true })).toHaveCount(0);
      expect(errors).toEqual([]);
    });
  }

  test(`switching the engine to ${engine} shows the hint for its URL field`, async ({ page }) => {
    const errors = pageErrors(page);
    await open(page, firstRunView());
    await expect(field(page, "engine.kind")).toHaveValue("none");
    await field(page, "engine.kind").selectOption(engine);
    await expectHint(page, url, HINT.en);
    expect(errors).toEqual([]);
  });
}

test("engines without a URL field show no base URL hint", async ({ page }) => {
  await open(page, firstRunView());
  await expect(field(page, "engine.kind")).toHaveValue("none");
  await expect(page.getByText(HINT.en, { exact: true })).toHaveCount(0);
  await field(page, "engine.kind").selectOption("builtin_local");
  await expect(field(page, "engine.kind")).toHaveValue("builtin_local");
  await expect(page.getByText(HINT.en, { exact: true })).toHaveCount(0);
  await field(page, "engine.kind").selectOption("api");
  await expect(page.getByText(HINT.en, { exact: true })).toHaveCount(1);
});

test("a field warning and the hint are both in the URL input's accessible description", async ({ page }) => {
  // Bite: describedBy that returns only the hint id (or only the messages) once a hint is passed.
  await open(page, firstRunView());
  await queueSaveOutcome(page, savedInsecureApi());
  await field(page, "engine.kind").selectOption("api");
  await field(page, "engine.api.base_url").fill("http://example.com/v1");
  await field(page, "engine.api.key").fill("sk-test-FAKE-0072-not-a-real-key");
  await page.getByTestId("settings-save").click();
  const warning = catalog("en")["settings.warning.endpoint_insecure"];
  expect(typeof warning).toBe("string");
  const control = field(page, "engine.api.base_url");
  await expect(control).toHaveAccessibleDescription(escapeRegExp(warning));
  await expect(control).toHaveAccessibleDescription(escapeRegExp(HINT.en));
});

test("a full endpoint URL is shown and saved as typed, not rewritten to a base URL", async ({ page }) => {
  // Decision #96 / T-072 analysis: the stored value is what the user typed; core decides
  // the join at request time. Characterization of the UI side (green today, must stay).
  const full = "http://10.0.0.5:8000/v1/audio/transcriptions";
  await open(page, viewWith("local_server", "en"));
  await field(page, "engine.local_server.base_url").fill(full);
  await page.getByTestId("settings-save").click();
  await expect.poll(async () => (await calls(page, "settings_save")).length).toBe(1);
  const [save] = await calls(page, "settings_save");
  const request = (save.args as { request: SaveRequest }).request;
  expect(request.settings.local_server.base_url).toBe(full);
  await expect(field(page, "engine.local_server.base_url")).toHaveValue(full);
});
