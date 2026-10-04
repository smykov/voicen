// T-015 red e2e: the settings page renders `Saved.warnings` as core gives them
// (decision #52; spec 004 US6, FR-020). Core decides which base URL is insecure
// (`settings::url::is_insecure_remote`); the page holds no URL, scheme or host rule. The
// outcomes here are scripted (queueSaveOutcome): `savedInsecureApi()` is core's own wire
// (e2e/fixtures/settings-wire.json `saved_insecure_api`, checked by a core test).
// The locator contract of e2e/settings-first-run.spec.ts and
// e2e/settings-focus-close.spec.ts applies (tab-<tab> test ids, data-field controls,
// role="alert" for form-level errors, settings-save, role="status" for "Saved").
//
// Locator contract this spec adds (T-015 analysis):
// - a warning of a field whose control is on the page is shown next to that control:
//   a visible element with the text of `t(warning.message)`, referenced by the control's
//   aria-describedby (so it is in the control's accessible description); it is not an
//   error: the control's aria-invalid stays unset/false and nothing is in role="alert";
// - a warning of a field with no control on the page (another tab, another engine,
//   post_processing.base_url until T-021) is listed in the form-level element with test
//   id `settings-warnings`, one `role="listitem"` per such field, holding the field's
//   label `t(settings.field_label.<FieldId>)` and `t(warning.message)`, never the raw
//   FieldId; the list follows the controls the current tab renders (rule L); with no
//   such warning the element is absent or has no list item;
// - the warnings are those of the last Saved outcome, kept outside the Draft: the
//   `settings://changed` echo of that save (which rebuilds the draft) keeps them; the
//   next save's outcome replaces them (a Saved with no warnings shows none).
import { readFileSync } from "node:fs";
import { expect, test, type Page } from "@playwright/test";
import {
  calls,
  emit,
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
const EN = catalog("en");
const RU = catalog("ru");

/** The English catalog text of `id`; a missing or empty text fails here, by name. */
function en(id: string): string {
  const text = EN[id];
  if (typeof text !== "string" || text === "") throw new Error(`i18n/en.json has no text for ${id}`);
  return text;
}

/** The Russian catalog text of `id`; a missing or empty text fails here, by name. */
function ru(id: string): string {
  const text = RU[id];
  if (typeof text !== "string" || text === "") throw new Error(`i18n/ru.json has no text for ${id}`);
  return text;
}

function escapeRegExp(s: string): RegExp {
  return new RegExp(s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
}

const WARNING_ID = "settings.warning.endpoint_insecure";
const API_URL = "engine.api.base_url";
const PP_URL = "post_processing.base_url";

function field(page: Page, id: string) {
  return page.locator(`[data-field="${id}"]`);
}

function tab(page: Page, name: string) {
  return page.getByTestId(`tab-${name}`);
}

function savedStatus(page: Page, text: string) {
  return page.getByRole("status").filter({ hasText: text });
}

function formWarnings(page: Page) {
  return page.getByTestId("settings-warnings").getByRole("listitem");
}

/** Collects uncaught page errors from before the first navigation. */
function pageErrors(page: Page): Error[] {
  const errors: Error[] = [];
  page.on("pageerror", (error) => errors.push(error));
  return errors;
}

async function open(page: Page, view: SettingsView = firstRunView()): Promise<void> {
  await installTauriMock(page, { view });
  await page.goto("/settings");
  await expect(page.getByRole("tab", { selected: true })).toBeVisible();
  await expect.poll(() => listeners(page, "settings://changed")).toBeGreaterThan(0);
}

/** Two animation frames: pending Svelte updates and effects have run. */
async function settle(page: Page): Promise<void> {
  await page.evaluate(
    () => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))),
  );
}

/** The `n`-th settings_save request (fails unless exactly `n` were made). */
async function lastSaveRequest(page: Page, n: number): Promise<SaveRequest> {
  await expect.poll(async () => (await calls(page, "settings_save")).length).toBe(n);
  const all = await calls(page, "settings_save");
  return (all[n - 1].args as { request: SaveRequest }).request;
}

/** The user saves engine api with base URL http://example.com/v1 (core's fixture case). */
async function saveInsecureApi(page: Page): Promise<void> {
  await field(page, "engine.kind").selectOption("api");
  await field(page, API_URL).fill("http://example.com/v1");
  await field(page, "engine.api.key").fill("sk-test-FAKE-0015-not-a-real-key");
  await page.getByTestId("settings-save").click();
}

/** The warning of `field` is shown next to its control, and is not an error. */
async function expectFieldWarning(page: Page, id: string, text: string): Promise<void> {
  const control = field(page, id);
  await expect(control).toBeVisible();
  await expect(control).toHaveAccessibleDescription(escapeRegExp(text));
  await expect(control).not.toHaveAttribute("aria-invalid", "true");
  // Visible on the page exactly once: next to the control, not also in the form list.
  await expect(page.getByText(text)).toHaveCount(1);
  await expect(page.getByText(text)).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
}

// ---- Acceptance 1: http://example.com/v1 -> warning; save still allowed --------------

test("http://example.com/v1 -> warning: core's Saved warning is shown next to the base URL control, not as an error, alongside Saved", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page);
  const outcome = savedInsecureApi();
  await queueSaveOutcome(page, outcome);

  await saveInsecureApi(page);

  // The save went through as typed: the UI does not hold it back or rewrite the URL.
  const request = await lastSaveRequest(page, 1);
  expect(request.settings.engine).toBe("api");
  expect(request.settings.api.base_url).toBe("http://example.com/v1");

  await expect(savedStatus(page, en("settings.saved"))).toBeVisible();
  await expect(field(page, API_URL)).toHaveValue(outcome.Saved.view.settings.api.base_url);
  await expectFieldWarning(page, API_URL, en(WARNING_ID));
  // The field has a control, so it is not in the form-level list.
  await expect(formWarnings(page)).toHaveCount(0);
  // Rendered by catalog text, never as the raw id, code or FieldId.
  await expect(page.locator("main")).not.toContainText(WARNING_ID);
  await expect(page.locator("main")).not.toContainText("endpoint.insecure");
  expect(errors).toEqual([]);
});

test("the warning stays after the settings://changed echo of our own save (the echo rebuilds the draft; warnings live outside it)", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page);
  const outcome = savedInsecureApi();
  await queueSaveOutcome(page, outcome);
  await saveInsecureApi(page);
  await expectFieldWarning(page, API_URL, en(WARNING_ID));

  // The shell's bridge emits the saved view after the save returned (the mock does not
  // for a scripted outcome, so the test plays the bridge).
  await emit(page, "settings://changed", outcome.Saved.view);
  await settle(page);

  await expectFieldWarning(page, API_URL, en(WARNING_ID));
  await expect(savedStatus(page, en("settings.saved"))).toBeVisible();
  expect(await calls(page, "settings_save")).toHaveLength(1);
  expect(errors).toEqual([]);
});

// ---- Acceptance 2 (failure branch): loopback -> no warning ----------------------------

test("failure branch: http://LOCALHOST:8000/v1 -> no warning: a Saved without warnings shows Saved and no warning anywhere", async ({ page }) => {
  // Guard: core sends no warning for loopback; the page must not add one by a URL rule of
  // its own (P-010, decision #52).
  const errors = pageErrors(page);
  await open(page);
  const outcome = savedInsecureApi();
  outcome.Saved.view.settings.api.base_url = "http://LOCALHOST:8000/v1";
  outcome.Saved.warnings = [];
  await queueSaveOutcome(page, outcome);

  await field(page, "engine.kind").selectOption("api");
  await field(page, API_URL).fill("http://LOCALHOST:8000/v1");
  await page.getByTestId("settings-save").click();

  await expect(savedStatus(page, en("settings.saved"))).toBeVisible();
  await expect(field(page, API_URL)).toHaveValue("http://LOCALHOST:8000/v1");
  await settle(page);
  await expect(page.getByText(en(WARNING_ID))).toHaveCount(0);
  await expect(field(page, API_URL)).not.toHaveAccessibleDescription(escapeRegExp(en(WARNING_ID)));
  await expect(field(page, API_URL)).not.toHaveAttribute("aria-invalid", "true");
  await expect(formWarnings(page)).toHaveCount(0);
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(errors).toEqual([]);
});

// ---- Localization ---------------------------------------------------------------------

test("with ui_language switched to ru, the warning and Saved are shown in Russian", async ({ page }) => {
  // Bite: a warning text rendered once at save time (the language switches only after
  // the Saved view is applied) or a hard-coded English text.
  const errors = pageErrors(page);
  await open(page);
  const outcome = savedInsecureApi();
  outcome.Saved.view.settings.ui_language = "ru";
  await queueSaveOutcome(page, outcome);

  await field(page, "engine.kind").selectOption("api");
  await field(page, API_URL).fill("http://example.com/v1");
  await tab(page, "general").click();
  await field(page, "general.ui_language").selectOption("ru");
  await tab(page, "engine").click();
  await page.getByTestId("settings-save").click();

  await expect(savedStatus(page, ru("settings.saved"))).toBeVisible();
  expect(ru(WARNING_ID)).not.toBe(en(WARNING_ID));
  await expectFieldWarning(page, API_URL, ru(WARNING_ID));
  await expect(page.getByText(en(WARNING_ID))).toHaveCount(0);

  // The echo of the save keeps it, still in Russian.
  await emit(page, "settings://changed", outcome.Saved.view);
  await settle(page);
  await expectFieldWarning(page, API_URL, ru(WARNING_ID));
  expect(errors).toEqual([]);
});

// ---- Rule L: a warning with no control on the page is listed at form level ------------

test("a warning for a field with no control on the current tab is listed at form level by its field label, and the list follows the rendered controls", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page);
  // Core's case of two URLs in use: the engine's and the post-processing one (no control
  // until T-021). The post-processing URL is a documentation address.
  const outcome = savedInsecureApi();
  outcome.Saved.view.settings.post_processing.enabled = true;
  outcome.Saved.view.settings.post_processing.base_url = "http://192.0.2.10/v1";
  outcome.Saved.warnings.push({ ...outcome.Saved.warnings[0], field: PP_URL });
  expect(outcome.Saved.warnings.map((w) => w.field)).toEqual([API_URL, PP_URL]);
  await queueSaveOutcome(page, outcome);

  await saveInsecureApi(page);
  await expect(savedStatus(page, en("settings.saved"))).toBeVisible();

  const text = en(WARNING_ID);
  const apiLabel = en(`settings.field_label.${API_URL}`);
  const ppLabel = en(`settings.field_label.${PP_URL}`);

  // Engine tab: the API URL has its control (warning beside it); the post-processing URL
  // has none, so it is listed by label with the warning text.
  await expect(field(page, API_URL)).toHaveAccessibleDescription(escapeRegExp(text));
  await expect(field(page, API_URL)).not.toHaveAttribute("aria-invalid", "true");
  await expect(field(page, PP_URL)).toHaveCount(0);
  await expect(formWarnings(page)).toHaveCount(1);
  await expect(formWarnings(page).first()).toContainText(ppLabel);
  await expect(formWarnings(page).first()).toContainText(text);
  await expect(page.getByTestId("settings-warnings")).not.toContainText(apiLabel);
  await expect(page.getByTestId("settings-warnings")).not.toContainText(PP_URL);
  // Warnings are not errors.
  await expect(page.getByRole("alert")).toHaveCount(0);

  // Recording tab: the API URL control is gone, so its label joins the list.
  await tab(page, "recording").click();
  await expect(field(page, API_URL)).toHaveCount(0);
  await expect(formWarnings(page)).toHaveCount(2);
  await expect(formWarnings(page).filter({ hasText: apiLabel })).toContainText(text);
  await expect(formWarnings(page).filter({ hasText: ppLabel })).toContainText(text);
  await expect(page.getByTestId("settings-warnings")).not.toContainText(API_URL);

  // Back on Engine: the control is rendered again, and the API URL leaves the list.
  await tab(page, "engine").click();
  await expect(field(page, API_URL)).toHaveAccessibleDescription(escapeRegExp(text));
  await expect(formWarnings(page)).toHaveCount(1);
  await expect(formWarnings(page).first()).toContainText(ppLabel);
  expect(errors).toEqual([]);
});

// ---- The next save replaces the warnings ----------------------------------------------

test("the next save clears old warnings: a Saved without warnings removes the field warning and the form-level list", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page);
  const outcome = savedInsecureApi();
  outcome.Saved.view.settings.post_processing.enabled = true;
  outcome.Saved.view.settings.post_processing.base_url = "http://192.0.2.10/v1";
  outcome.Saved.warnings.push({ ...outcome.Saved.warnings[0], field: PP_URL });
  await queueSaveOutcome(page, outcome);
  await saveInsecureApi(page);
  await expect(field(page, API_URL)).toHaveAccessibleDescription(escapeRegExp(en(WARNING_ID)));
  await expect(formWarnings(page)).toHaveCount(1);

  // The user switches to https and saves again; core (here the unscripted mock) returns
  // a Saved with no warnings.
  await field(page, API_URL).fill("https://example.com/v1");
  await page.getByTestId("settings-save").click();
  const request = await lastSaveRequest(page, 2);
  expect(request.settings.api.base_url).toBe("https://example.com/v1");

  await expect(savedStatus(page, en("settings.saved"))).toBeVisible();
  await expect(page.getByText(en(WARNING_ID))).toHaveCount(0);
  await expect(field(page, API_URL)).not.toHaveAccessibleDescription(escapeRegExp(en(WARNING_ID)));
  await expect(formWarnings(page)).toHaveCount(0);
  expect(errors).toEqual([]);
});
