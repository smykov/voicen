// T-021 red e2e: the Post-processing tab of the settings page (spec 003 US3, spec 004
// FR-001 / FR-004) over the one IPC mock (e2e/support/tauriMock.ts). The locator contract
// of e2e/settings-first-run.spec.ts applies (tab-<tab> test ids, data-field controls,
// aria-invalid + accessible description for a refused field, role="alert" for form-level
// messages, settings-save).
//
// Locator contract this spec adds (T-021 analysis, seam):
// - tab `tab-post_processing` (label `settings.tab.post_processing`), between Output and
//   History: Engine, Recording, Output, Post-processing, History, General;
// - its controls: checkbox `post_processing.enabled` (label
//   `settings.field.post_processing_enabled`), `<input type="url">`
//   `post_processing.base_url`, input `post_processing.model`, the key
//   `<input type="password">` `post_processing.key` (KeyField, slot post_processing), and
//   `<textarea>` `post_processing.prompt` (label `settings.field.prompt`);
// - the privacy note `settings.post_processing.privacy_note` is always visible on the tab;
// - the fields stay editable while the toggle is off; the UI holds no post-processing rule:
//   a refusal comes only from core (here core's own outcome, the fixture entry
//   `refused_post_processing_on_empty`, pinned by a core test).
import { readFileSync } from "node:fs";
import type { Page } from "@playwright/test";
import { expect, test } from "./support/boot";
import {
  calls,
  firstRunView,
  installTauriMock,
  queueSaveOutcome,
  refusedPostProcessingOnEmpty,
  storedView,
  type SaveRequest,
  type Settings,
  type SettingsView,
} from "./support/tauriMock";

type Messages = Record<string, string>;
const catalog = (lang: "en" | "ru"): Messages =>
  JSON.parse(readFileSync(new URL(`../i18n/${lang}.json`, import.meta.url), "utf8")) as Messages;
const CATALOGS = { en: catalog("en"), ru: catalog("ru") };

/** The catalog text of `id`; a missing or empty text fails here, by name. */
function text(lang: "en" | "ru", id: string): string {
  const value = CATALOGS[lang][id];
  if (typeof value !== "string" || value === "") throw new Error(`i18n/${lang}.json has no text for ${id}`);
  return value;
}

const en = (id: string) => text("en", id);

function escapeRegExp(s: string): RegExp {
  return new RegExp(s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
}

function field(page: Page, id: string) {
  return page.locator(`[data-field="${id}"]`);
}

function tab(page: Page, name: string) {
  return page.getByTestId(`tab-${name}`);
}

const ENABLED = "post_processing.enabled";
const URL_FIELD = "post_processing.base_url";
const MODEL = "post_processing.model";
const KEY = "post_processing.key";
const PROMPT = "post_processing.prompt";
const PP_FIELDS = [ENABLED, URL_FIELD, MODEL, KEY, PROMPT] as const;

const FAKE_KEY = "sk-test-FAKE-pp-0000-not-a-real-key";
const FAKE_URL = "https://llm.example.com/v1";
const FAKE_MODEL = "llm-fake-1";
const FAKE_PROMPT = "Fake prompt: fix punctuation only.";

/** Collects uncaught page errors from before the first navigation. */
function pageErrors(page: Page): Error[] {
  const errors: Error[] = [];
  page.on("pageerror", (error) => errors.push(error));
  return errors;
}

async function open(page: Page, view: SettingsView = firstRunView(), path = "/settings"): Promise<void> {
  await installTauriMock(page, { view });
  await page.goto(path);
  await expect(page.getByRole("tab", { selected: true })).toBeVisible();
}

async function openPostProcessing(page: Page, view: SettingsView = firstRunView()): Promise<void> {
  await open(page, view);
  await tab(page, "post_processing").click();
  await expect(tab(page, "post_processing")).toHaveAttribute("aria-selected", "true");
}

/** The settings_save requests so far, waiting until there are `count` of them. */
async function saveRequests(page: Page, count: number): Promise<SaveRequest[]> {
  await expect.poll(async () => (await calls(page, "settings_save")).length).toBe(count);
  return (await calls(page, "settings_save")).map((c) => (c.args as { request: SaveRequest }).request);
}

/** Core's first-run settings with `post_processing` replaced. */
function firstRunWith(pp: Settings["post_processing"]): Settings {
  const settings = firstRunView().settings;
  settings.post_processing = pp;
  return settings;
}

// ---- Acceptance 1: the tab shows core's view ---------------------------------------------

test("first run: the Post-processing tab sits between Output and History and shows core's view: off, empty URL and model, the starter prompt in a textarea, an empty key and the privacy note", async ({ page }) => {
  const errors = pageErrors(page);
  const view = firstRunView();
  const starter = view.settings.post_processing.prompt;
  // Fixture guard: core's first run carries the starter prompt, URL and model empty, off.
  expect(starter.trim()).not.toBe("");
  expect(view.settings.post_processing).toEqual({ enabled: false, base_url: "", model: "", prompt: starter });
  await open(page, view);

  const order = await page
    .getByRole("tab")
    .evaluateAll((els) => els.map((e) => (e as HTMLElement).dataset.testid ?? ""));
  expect(order).toEqual([
    "tab-engine",
    "tab-recording",
    "tab-output",
    "tab-post_processing",
    "tab-history",
    "tab-general",
  ]);
  await expect(tab(page, "post_processing")).toHaveText(en("settings.tab.post_processing"));

  await tab(page, "post_processing").click();
  await expect(tab(page, "post_processing")).toHaveAttribute("aria-selected", "true");

  const enabled = field(page, ENABLED);
  await expect(enabled).toHaveAttribute("type", "checkbox");
  await expect(enabled).not.toBeChecked();
  await expect(enabled).toHaveAccessibleName(en("settings.field.post_processing_enabled"));

  await expect(field(page, URL_FIELD)).toHaveAttribute("type", "url");
  await expect(field(page, URL_FIELD)).toHaveValue("");
  await expect(field(page, MODEL)).toHaveValue("");

  const prompt = field(page, PROMPT);
  expect(await prompt.evaluate((e) => e.tagName)).toBe("TEXTAREA");
  await expect(prompt).toHaveValue(starter);
  await expect(prompt).toHaveAccessibleName(en("settings.field.prompt"));

  const key = field(page, KEY);
  await expect(key).toHaveAttribute("type", "password");
  await expect(key).toHaveValue("");
  await expect(page.getByText(en("settings.key.saved"))).toHaveCount(0);

  // The privacy note is on the tab while post-processing is off (always visible).
  const note = page.getByRole("tabpanel").getByText(en("settings.post_processing.privacy_note"));
  await expect(note).toBeVisible();
  await enabled.check();
  await expect(note).toBeVisible();

  // Every control is editable while the toggle is off (004 FR-004).
  await enabled.uncheck();
  for (const id of [URL_FIELD, MODEL, KEY, PROMPT]) await expect(field(page, id), id).toBeEditable();

  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("ui_language ru: the Post-processing tab label and the privacy note are the Russian catalog texts", async ({ page }) => {
  const errors = pageErrors(page);
  const view = firstRunView();
  view.first_run = false;
  view.settings.ui_language = "ru";
  await open(page, view);

  await expect(tab(page, "post_processing")).toHaveText(text("ru", "settings.tab.post_processing"));
  await tab(page, "post_processing").click();
  const panel = page.getByRole("tabpanel");
  await expect(panel.getByText(text("ru", "settings.post_processing.privacy_note"))).toBeVisible();
  await expect(panel).not.toContainText(en("settings.post_processing.privacy_note"));
  await expect(field(page, ENABLED)).toHaveAccessibleName(text("ru", "settings.field.post_processing_enabled"));
  await expect(field(page, PROMPT)).toHaveAccessibleName(text("ru", "settings.field.prompt"));
  expect(errors).toEqual([]);
});

// ---- Acceptance 1: the tab saves endpoint, model, masked key and prompt ---------------------

test("save path: post-processing on with endpoint, model, an edited prompt and a typed key sends exactly those settings and Replace on the post_processing key slot; after Saved the key shows as saved, and Remove key sends Clear", async ({ page }) => {
  const errors = pageErrors(page);
  const view = firstRunView();
  await openPostProcessing(page, view);

  await field(page, ENABLED).check();
  await field(page, URL_FIELD).fill(FAKE_URL);
  await field(page, MODEL).fill(FAKE_MODEL);
  await field(page, PROMPT).fill(FAKE_PROMPT);
  const key = field(page, KEY);
  await key.fill(FAKE_KEY);
  await expect(key).toHaveAttribute("type", "password");
  await page.getByTestId("settings-save").click();

  const [first] = await saveRequests(page, 1);
  const expected = firstRunWith({ enabled: true, base_url: FAKE_URL, model: FAKE_MODEL, prompt: FAKE_PROMPT });
  expect(first.settings).toEqual(expected);
  expect(first.keys).toEqual({
    transcription_api: "Untouched",
    local_server: "Untouched",
    post_processing: { Replace: FAKE_KEY },
  });

  // Saved: the stored view has the settings and a post-processing key (the effect); the
  // tab shows the saved view, the key as presence only (U3).
  await expect(page.getByRole("status").filter({ hasText: en("settings.saved") })).toBeVisible();
  const stored = await storedView(page);
  expect(stored.settings.post_processing).toEqual(expected.post_processing);
  expect(stored.keys).toEqual({ transcription_api: false, local_server: false, post_processing: true });
  await expect(field(page, ENABLED)).toBeChecked();
  await expect(field(page, URL_FIELD)).toHaveValue(FAKE_URL);
  await expect(field(page, MODEL)).toHaveValue(FAKE_MODEL);
  await expect(field(page, PROMPT)).toHaveValue(FAKE_PROMPT);
  await expect(key).toHaveValue("");
  await expect(page.getByText(en("settings.key.saved"))).toBeVisible();
  await expect(page.locator("body")).not.toContainText(FAKE_KEY);

  // Remove key -> Clear on the post_processing slot only; the settings are unchanged.
  await page.getByRole("button", { name: en("settings.key.clear") }).click();
  await page.getByTestId("settings-save").click();
  const [, second] = await saveRequests(page, 2);
  expect(second.settings).toEqual(expected);
  expect(second.keys).toEqual({ transcription_api: "Untouched", local_server: "Untouched", post_processing: "Clear" });
  await expect.poll(async () => (await storedView(page)).keys.post_processing).toBe(false);
  expect(errors).toEqual([]);
});

// ---- Acceptance 2: failure branch -------------------------------------------------------------

test("failure branch: post-processing on with an empty URL, model and prompt is refused by core: the three controls are highlighted with error.required, the inputs are kept and no form-level error is shown", async ({ page }) => {
  const errors = pageErrors(page);
  await openPostProcessing(page);
  await field(page, ENABLED).check();
  await field(page, PROMPT).fill("");

  // Core's own outcome for exactly this request (fixture refused_post_processing_on_empty).
  const refused = refusedPostProcessingOnEmpty();
  expect(refused.Refused.errors.map((e) => `${e.field}:${e.code}`)).toEqual([
    `${URL_FIELD}:required`,
    `${MODEL}:required`,
    `${PROMPT}:required`,
  ]);
  expect(refused.Refused.form_error).toBeNull();
  await queueSaveOutcome(page, refused);
  await page.getByTestId("settings-save").click();

  // The UI sends the save (no rule of its own) with what the fixture was computed from.
  const [request] = await saveRequests(page, 1);
  expect(request.settings).toEqual(firstRunWith({ enabled: true, base_url: "", model: "", prompt: "" }));
  expect(request.keys).toEqual({ transcription_api: "Untouched", local_server: "Untouched", post_processing: "Untouched" });

  const required = escapeRegExp(en("error.required"));
  for (const id of [URL_FIELD, MODEL, PROMPT]) {
    await expect(field(page, id), id).toHaveAttribute("aria-invalid", "true");
    await expect(field(page, id), id).toHaveAccessibleDescription(required);
  }
  for (const id of [ENABLED, KEY]) await expect(field(page, id), id).not.toHaveAttribute("aria-invalid", "true");

  // The draft is kept: still on, still empty, on the same tab; nothing was saved.
  await expect(tab(page, "post_processing")).toHaveAttribute("aria-selected", "true");
  await expect(field(page, ENABLED)).toBeChecked();
  for (const id of [URL_FIELD, MODEL, PROMPT]) await expect(field(page, id), id).toHaveValue("");
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(page.getByText(en("settings.saved"))).toHaveCount(0);
  expect((await storedView(page)).settings.post_processing.enabled).toBe(false);
  expect(errors).toEqual([]);
});

test("failure branch: a url.malformed refusal on the post-processing URL highlights only the URL control and keeps every entered value", async ({ page }) => {
  const errors = pageErrors(page);
  await openPostProcessing(page);
  await field(page, ENABLED).check();
  await field(page, URL_FIELD).fill("llm.example.com/v1");
  await field(page, MODEL).fill(FAKE_MODEL);
  await field(page, PROMPT).fill(FAKE_PROMPT);
  await field(page, KEY).fill(FAKE_KEY);
  await queueSaveOutcome(page, {
    Refused: { errors: [{ field: URL_FIELD, code: "url.malformed" }], form_error: null },
  });
  await page.getByTestId("settings-save").click();
  await saveRequests(page, 1);

  const url = field(page, URL_FIELD);
  await expect(url).toHaveAttribute("aria-invalid", "true");
  await expect(url).toHaveAccessibleDescription(escapeRegExp(en("error.url.malformed")));
  for (const id of [ENABLED, MODEL, KEY, PROMPT]) await expect(field(page, id), id).not.toHaveAttribute("aria-invalid", "true");
  await expect(url).toHaveValue("llm.example.com/v1");
  await expect(field(page, MODEL)).toHaveValue(FAKE_MODEL);
  await expect(field(page, PROMPT)).toHaveValue(FAKE_PROMPT);
  await expect(field(page, KEY)).toHaveValue(FAKE_KEY); // a refusal keeps the typed key (U3)
  await expect(page.getByRole("alert")).toHaveCount(0);

  // Fixed and saved again: core's Saved clears the highlight.
  await url.fill(FAKE_URL);
  await page.getByTestId("settings-save").click();
  const [, second] = await saveRequests(page, 2);
  expect(second.settings.post_processing).toEqual({
    enabled: true,
    base_url: FAKE_URL,
    model: FAKE_MODEL,
    prompt: FAKE_PROMPT,
  });
  expect(second.keys.post_processing).toEqual({ Replace: FAKE_KEY });
  await expect(page.getByRole("status").filter({ hasText: en("settings.saved") })).toBeVisible();
  await expect(url).not.toHaveAttribute("aria-invalid", "true");
  expect(errors).toEqual([]);
});

test("off branch: with post-processing off, an empty URL, model and prompt are sent as entered and core's Saved shows settings.saved with no field highlighted", async ({ page }) => {
  const errors = pageErrors(page);
  await openPostProcessing(page);
  await expect(field(page, ENABLED)).not.toBeChecked();
  await field(page, PROMPT).fill("");
  await page.getByTestId("settings-save").click();

  const [request] = await saveRequests(page, 1);
  const expected = firstRunWith({ enabled: false, base_url: "", model: "", prompt: "" });
  expect(request.settings).toEqual(expected);
  await expect(page.getByRole("status").filter({ hasText: en("settings.saved") })).toBeVisible();
  expect((await storedView(page)).settings.post_processing).toEqual(expected.post_processing);
  await expect(page.locator('[aria-invalid="true"]')).toHaveCount(0);
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(field(page, PROMPT)).toHaveValue("");
  expect(errors).toEqual([]);
});

// ---- Focus (F): every control carries its FieldId -----------------------------------------------

test("?tab=post_processing&field=<FieldId> selects the Post-processing tab and focuses that control, for each of its five fields", async ({ page }) => {
  const errors = pageErrors(page);
  await installTauriMock(page, { view: firstRunView() });
  for (const id of PP_FIELDS) {
    await page.goto(`/settings?tab=post_processing&field=${id}`);
    await expect(tab(page, "post_processing"), id).toHaveAttribute("aria-selected", "true");
    await expect(field(page, id), id).toBeFocused();
  }
  expect(errors).toEqual([]);
});
