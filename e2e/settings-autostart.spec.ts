// T-034 red e2e: the General tab's "Start with Windows" toggle (FR-19, spec 004 US5, T059)
// over the one IPC mock (e2e/support/tauriMock.ts). The locator contract of
// e2e/settings-first-run.spec.ts applies (tab-<tab> test ids, data-field controls,
// aria-invalid + aria-describedby for a refused field, role="alert" for form-level
// messages, settings-save, role="status" for Saved).
//
// Locator contract this spec adds (T-034 analysis, option A):
// - the General tab renders `<input type="checkbox" data-field="general.start_with_windows">`
//   bound to the draft's `settings.start_with_windows`, labelled by the catalog text
//   `settings.field.start_with_windows` (one control-label id per control, as
//   `settings.field.auto_paste`);
// - a Refused `general.start_with_windows: autostart.failed` highlights it like every other
//   field (U2) and the draft keeps the user's value (docs/decisions/settings-ui.md, "The
//   draft is kept in every case"; T-034 Notes 2026-10-07 on Q1).
import { readFileSync } from "node:fs";
import type { Page } from "@playwright/test";
import { expect, test } from "./support/boot";
import {
  calls,
  emitted,
  firstRunView,
  installTauriMock,
  queueSaveOutcome,
  storedView,
  type SaveRequest,
  type SettingsView,
} from "./support/tauriMock";

const EN = JSON.parse(readFileSync(new URL("../i18n/en.json", import.meta.url), "utf8")) as Record<string, string>;

/** The English catalog text of `id`; a missing or empty text fails here, by name. */
function en(id: string): string {
  const text = EN[id];
  if (typeof text !== "string" || text === "") throw new Error(`i18n/en.json has no text for ${id}`);
  return text;
}

function escapeRegExp(s: string): RegExp {
  return new RegExp(s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
}

const START = "general.start_with_windows";

function field(page: Page, id: string) {
  return page.locator(`[data-field="${id}"]`);
}

function tab(page: Page, name: string) {
  return page.getByTestId(`tab-${name}`);
}

/** Collects uncaught page errors from before the first navigation. */
function pageErrors(page: Page): Error[] {
  const errors: Error[] = [];
  page.on("pageerror", (error) => errors.push(error));
  return errors;
}

/** A saved (not first-run) view with `start_with_windows` as given. */
function savedView(startWithWindows: boolean): SettingsView {
  const view = firstRunView();
  view.first_run = false;
  view.settings.start_with_windows = startWithWindows;
  return view;
}

/** Opens /settings over `view` and selects the General tab. */
async function openGeneral(page: Page, view: SettingsView): Promise<void> {
  await installTauriMock(page, { view });
  await page.goto("/settings");
  await expect(page.getByRole("tab", { selected: true })).toBeVisible();
  await tab(page, "general").click();
  await expect(tab(page, "general")).toHaveAttribute("aria-selected", "true");
}

/**
 * The toggle: exactly one checkbox control for the field, named by its control label.
 * The control is looked up first, so a page without it fails as "control not found".
 */
async function startToggle(page: Page) {
  const toggle = field(page, START);
  await expect(toggle, `the General tab renders the ${START} control`).toHaveCount(1);
  await expect(toggle).toHaveAttribute("type", "checkbox");
  await expect(toggle).toHaveAccessibleName(en("settings.field.start_with_windows"));
  return toggle;
}

/** The single settings_save request so far (fails unless exactly one was made). */
async function onlySaveRequest(page: Page): Promise<SaveRequest> {
  await expect.poll(async () => (await calls(page, "settings_save")).length).toBe(1);
  const [call] = await calls(page, "settings_save");
  return (call.args as { request: SaveRequest }).request;
}

// ---- Acceptance 1 ------------------------------------------------------------------

for (const saved of [false, true]) {
  test(`the General tab shows Start with Windows ${saved ? "on" : "off"} from the saved view; toggling it and saving sends start_with_windows ${!saved} through settings_save once`, async ({ page }) => {
    const errors = pageErrors(page);
    const view = savedView(saved);
    await openGeneral(page, view);

    const toggle = await startToggle(page);
    await expect(toggle).toBeChecked({ checked: saved });
    // Showing the view writes nothing back: no save yet.
    expect(await calls(page, "settings_save")).toEqual([]);

    await toggle.click();
    await expect(toggle).toBeChecked({ checked: !saved });
    await page.getByTestId("settings-save").click();

    // The one Draft/settings_save path: the field flipped, every other value as the view.
    const request = await onlySaveRequest(page);
    expect(request.settings).toEqual({ ...view.settings, start_with_windows: !saved });
    expect(request.keys).toEqual({ transcription_api: "Untouched", local_server: "Untouched", post_processing: "Untouched" });

    // The mock's Saved stores what was sent; the window takes it as the new baseline.
    await expect(page.getByRole("status")).toContainText(en("settings.saved"));
    expect((await storedView(page)).settings.start_with_windows).toBe(!saved);
    await expect(toggle).toBeChecked({ checked: !saved });
    await expect(toggle).not.toHaveAttribute("aria-invalid", "true");
    expect(errors).toEqual([]);
  });
}

// ---- Acceptance 2 (failure branch) -------------------------------------------------

test("failure branch: settings_save Refused with general.start_with_windows autostart.failed highlights the toggle with error.autostart.failed, keeps the user's value and saves nothing", async ({ page }) => {
  const errors = pageErrors(page);
  const view = savedView(false);
  await openGeneral(page, view);
  await queueSaveOutcome(page, {
    Refused: { errors: [{ field: START, code: "autostart.failed" }], form_error: null },
  });

  const toggle = await startToggle(page);
  await expect(toggle).toBeChecked({ checked: false });
  await toggle.click();
  await page.getByTestId("settings-save").click();

  // The UI does not pre-empt the registry write: the request reaches core once.
  const request = await onlySaveRequest(page);
  expect(request.settings).toEqual({ ...view.settings, start_with_windows: true });

  // The reason is on the field (U2), not a form-level message.
  await expect(toggle).toHaveAttribute("aria-invalid", "true");
  await expect(toggle).toHaveAccessibleDescription(escapeRegExp(en("error.autostart.failed")));
  await expect(page.getByRole("alert")).toHaveCount(0);
  // Only the named field is highlighted.
  await expect(field(page, "general.ui_language")).not.toHaveAttribute("aria-invalid", "true");

  // The draft keeps the user's value (settings-ui.md U2), and Save can be pressed again.
  await expect(toggle).toBeChecked({ checked: true });
  await expect(page.getByTestId("settings-save")).toBeEnabled();

  // Nothing was saved and nothing announced.
  await expect(page.getByRole("status")).toHaveCount(0);
  expect(await storedView(page)).toEqual(view);
  expect(await emitted(page, "settings://changed")).toEqual([]);
  expect((await calls(page, "settings_save")).length).toBe(1);
  expect(errors).toEqual([]);
});

test("failure branch, then retry: after an autostart.failed refusal a second Save sends the kept value again and its Saved clears the highlight", async ({ page }) => {
  const errors = pageErrors(page);
  const view = savedView(false);
  await openGeneral(page, view);
  await queueSaveOutcome(page, {
    Refused: { errors: [{ field: START, code: "autostart.failed" }], form_error: null },
  });

  const toggle = await startToggle(page);
  await toggle.click();
  await page.getByTestId("settings-save").click();
  await expect(toggle).toHaveAttribute("aria-invalid", "true");

  // No scripted outcome left: the mock answers Saved with what was sent.
  await page.getByTestId("settings-save").click();
  await expect.poll(async () => (await calls(page, "settings_save")).length).toBe(2);
  const second = ((await calls(page, "settings_save"))[1].args as { request: SaveRequest }).request;
  expect(second.settings.start_with_windows).toBe(true);
  await expect(page.getByRole("status")).toContainText(en("settings.saved"));
  await expect(toggle).not.toHaveAttribute("aria-invalid", "true");
  await expect(toggle).toBeChecked({ checked: true });
  expect((await storedView(page)).settings.start_with_windows).toBe(true);
  expect(errors).toEqual([]);
});
