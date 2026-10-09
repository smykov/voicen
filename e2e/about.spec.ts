// T-023 red e2e: the About dialog of the settings window (FR-18; spec 006 US1, FR-002,
// Clarification Q1; contracts/ipc.md "About"). Runs over the one IPC mock
// (e2e/support/tauriMock.ts); the locator contract of e2e/settings-first-run.spec.ts
// applies (tab-<tab> test ids, data-field controls).
//
// Locator contract this spec adds (T-023 analysis, option A):
// - the General tab has a button named `about.button` ("About Voicen"); no other tab has it;
// - it opens a modal `role="dialog"` (a native <dialog>) named by its title `about.title`
//   ("About Voicen"), showing `Voicen <version> (<commit>)` (app.build_info, through
//   formatBuildInfo), "MIT License" (`about.license`) and a Close button (`about.close`);
// - get_build_info is called once per opening, never before the first one; while a
//   rejection is the answer the dialog shows `role="alert"` with exactly
//   `about.build_info_error` ("Cannot read build info"): no version text, no rejection text;
// - while the opening's get_build_info is pending the dialog shows no version line and no
//   alert (never a value from an earlier opening), and an earlier opening's answer that
//   arrives after a reopen is dropped;
// - Esc and Close close it and return focus to the opener;
// - the discard prompt wins: a close request with a dirty draft closes About first.
import { readFileSync } from "node:fs";
import type { Locator, Page } from "@playwright/test";
import { expect, test } from "./support/boot";
import {
  calls,
  emit,
  firstRunView,
  holdBuildInfo,
  installTauriMock,
  listeners,
  releaseBuildInfo,
  requestClose,
  setBuildInfo,
  type MockOptions,
  type SettingsView,
} from "./support/tauriMock";

const RU = JSON.parse(readFileSync(new URL("../i18n/ru.json", import.meta.url), "utf8")) as Record<string, string>;
const EN = JSON.parse(readFileSync(new URL("../i18n/en.json", import.meta.url), "utf8")) as Record<string, string>;

/** The Russian catalog text of `id`; a missing or empty text fails here, by name. */
function ru(id: string): string {
  const text = RU[id];
  if (typeof text !== "string" || text === "") throw new Error(`i18n/ru.json has no text for ${id}`);
  return text;
}

// English texts as contracts/ipc.md names them (spec 006 "Message catalog entries").
const ABOUT = "About Voicen";
const LICENSE = "MIT License";
const ERROR = "Cannot read build info";
const OK = { version: "0.1.0", commit: "abc1234" };
const LINE = "Voicen 0.1.0 (abc1234)";
const REJECTION = "ipc down";

function field(page: Page, id: string) {
  return page.locator(`[data-field="${id}"]`);
}

function tab(page: Page, name: string) {
  return page.getByTestId(`tab-${name}`);
}

function aboutButton(page: Page, name = ABOUT): Locator {
  return page.getByRole("button", { name, exact: true });
}

function aboutDialog(page: Page, name = ABOUT): Locator {
  return page.getByRole("dialog", { name, exact: true });
}

/** Collects uncaught page errors from before the first navigation. */
function pageErrors(page: Page): Error[] {
  const errors: Error[] = [];
  page.on("pageerror", (error) => errors.push(error));
  return errors;
}

/** Two animation frames: pending Svelte updates and effects have run (for "nothing happened" checks). */
async function settle(page: Page): Promise<void> {
  await page.evaluate(
    () => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))),
  );
}

async function buildInfoCalls(page: Page): Promise<number> {
  return (await calls(page, "get_build_info")).length;
}

/** Opens /settings with `options`, then the General tab. */
async function openGeneral(
  page: Page,
  options: Omit<MockOptions, "view"> = {},
  view: SettingsView = firstRunView(),
): Promise<void> {
  await installTauriMock(page, { ...options, view });
  await page.goto("/settings");
  await expect(page.getByRole("tab", { selected: true })).toBeVisible();
  await tab(page, "general").click();
  await expect(tab(page, "general")).toHaveAttribute("aria-selected", "true");
  await expect(field(page, "general.ui_language")).toBeVisible();
}

/** True when the focused element is inside `dialog`. */
async function focusInside(dialog: Locator): Promise<boolean> {
  return dialog.evaluate((el) => el.contains(document.activeElement));
}

// (1) ------------------------------------------------------------------------------

test("the General tab's About Voicen button opens a dialog with the version line and MIT License, calling get_build_info only on opening", async ({ page }) => {
  const errors = pageErrors(page);
  await installTauriMock(page, { buildInfo: OK });
  await page.goto("/settings");
  await expect(page.getByRole("tab", { selected: true })).toBeVisible();
  // Not on the other tabs (Clarification Q1: the General tab).
  await expect(tab(page, "engine")).toHaveAttribute("aria-selected", "true");
  await expect(aboutButton(page)).toHaveCount(0);

  await tab(page, "general").click();
  await expect(aboutButton(page)).toBeVisible();
  await settle(page);
  expect(await buildInfoCalls(page)).toBe(0);
  await expect(aboutDialog(page)).toHaveCount(0);

  await aboutButton(page).click();
  const dialog = aboutDialog(page);
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText(LINE, { exact: true })).toBeVisible();
  await expect(dialog.getByText(LICENSE, { exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Close", exact: true })).toBeVisible();
  await expect(dialog.getByRole("alert")).toHaveCount(0);
  await settle(page);
  expect(await buildInfoCalls(page)).toBe(1);
  expect(await calls(page, "settings_save")).toEqual([]);
  expect(errors).toEqual([]);
});

// (2) ------------------------------------------------------------------------------

test("failure branch: get_build_info rejects -> the dialog shows the alert Cannot read build info, no version text and no rejection text", async ({ page }) => {
  const errors = pageErrors(page);
  await openGeneral(page, { buildInfo: { reject: REJECTION } });

  await aboutButton(page).click();
  const dialog = aboutDialog(page);
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("alert")).toHaveText(ERROR);
  await expect(dialog).not.toContainText("Voicen 0.");
  await expect(dialog).not.toContainText(REJECTION);
  // The rest of the dialog is still there.
  await expect(dialog.getByText(LICENSE, { exact: true })).toBeVisible();
  // The rejection text appears nowhere on the page (settings-ui U2).
  await expect(page.locator("body")).not.toContainText(REJECTION);
  expect(await buildInfoCalls(page)).toBe(1);
  expect(errors).toEqual([]);
});

// (3) ------------------------------------------------------------------------------

test("no stale value: open (ok), close, the IPC fails, reopen -> the alert shows and the old version line is gone", async ({ page }) => {
  await openGeneral(page, { buildInfo: OK });

  await aboutButton(page).click();
  const dialog = aboutDialog(page);
  await expect(dialog.getByText(LINE, { exact: true })).toBeVisible();
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await expect(dialog).toBeHidden();

  await setBuildInfo(page, { reject: REJECTION });
  await aboutButton(page).click();
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("alert")).toHaveText(ERROR);
  await expect(dialog).not.toContainText(LINE);
  await expect(dialog).not.toContainText(REJECTION);
  expect(await buildInfoCalls(page)).toBe(2);
});

test("no stale error: open (IPC fails), close, the IPC recovers, reopen -> the new version line and no alert", async ({ page }) => {
  await openGeneral(page, { buildInfo: { reject: REJECTION } });

  await aboutButton(page).click();
  const dialog = aboutDialog(page);
  await expect(dialog.getByRole("alert")).toHaveText(ERROR);
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await expect(dialog).toBeHidden();

  // A different value than any earlier answer, so a cached line cannot pass.
  await setBuildInfo(page, { version: "0.2.0", commit: "def5678" });
  await aboutButton(page).click();
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText("Voicen 0.2.0 (def5678)", { exact: true })).toBeVisible();
  await expect(dialog.getByRole("alert")).toHaveCount(0);
  await expect(dialog).not.toContainText(ERROR);
  expect(await buildInfoCalls(page)).toBe(2);
});

// (4) ------------------------------------------------------------------------------

test("keyboard: Tab reaches the button, Enter opens with focus inside, Esc closes and returns focus; Close by Enter does the same", async ({ page }) => {
  await openGeneral(page, { buildInfo: OK });
  const button = aboutButton(page);
  await expect(button).toBeVisible();

  // Tab from the General tab itself until the button has focus (bounded).
  await tab(page, "general").focus();
  let reached = false;
  for (let i = 0; i < 40 && !reached; i++) {
    await page.keyboard.press("Tab");
    reached = await button.evaluate((el) => el === document.activeElement);
  }
  expect(reached, "Tab reaches the About Voicen button").toBe(true);

  await page.keyboard.press("Enter");
  const dialog = aboutDialog(page);
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText(LINE, { exact: true })).toBeVisible();
  await expect.poll(() => focusInside(dialog), { message: "focus moves into the dialog" }).toBe(true);

  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(button).toBeFocused();

  await page.keyboard.press("Enter");
  await expect(dialog).toBeVisible();
  const close = dialog.getByRole("button", { name: "Close", exact: true });
  await close.focus();
  await page.keyboard.press("Enter");
  await expect(dialog).toBeHidden();
  await expect(button).toBeFocused();
  expect(await buildInfoCalls(page)).toBe(2);
});

// (5) ------------------------------------------------------------------------------

test("with ui_language ru the button and the dialog title are in Russian and the version line is unchanged", async ({ page }) => {
  const view = firstRunView();
  view.settings.ui_language = "ru";
  await openGeneral(page, { buildInfo: OK }, view);

  const ruButton = ru("about.button");
  const ruTitle = ru("about.title");
  expect(ruButton).not.toBe(ABOUT);
  expect(ruTitle).not.toBe(ABOUT);
  // The English and Russian catalogs both carry the new ids (parity).
  expect(EN["about.button"]).toBe(ABOUT);
  expect(EN["about.title"]).toBe(ABOUT);

  await expect(aboutButton(page)).toHaveCount(0);
  await aboutButton(page, ruButton).click();
  const dialog = aboutDialog(page, ruTitle);
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText(LINE, { exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: ru("about.close"), exact: true })).toBeVisible();
  await expect(dialog.getByText(ru("about.license"), { exact: true })).toBeVisible();
});

test("failure branch in Russian: the alert reads the ru about.build_info_error", async ({ page }) => {
  const view = firstRunView();
  view.settings.ui_language = "ru";
  await openGeneral(page, { buildInfo: { reject: REJECTION } }, view);

  const ruError = ru("about.build_info_error");
  expect(ruError).not.toBe(ERROR);
  await aboutButton(page, ru("about.button")).click();
  const dialog = aboutDialog(page, ru("about.title"));
  await expect(dialog.getByRole("alert")).toHaveText(ruError);
  await expect(dialog).not.toContainText(REJECTION);
});

// (6) ------------------------------------------------------------------------------

test("the discard prompt wins: About open with a dirty draft, a close request closes About and shows the discard alertdialog with Keep editing focused", async ({ page }) => {
  const errors = pageErrors(page);
  await installTauriMock(page, { view: firstRunView(), buildInfo: OK });
  await page.goto("/settings");
  await expect(page.getByRole("tab", { selected: true })).toBeVisible();
  await expect
    .poll(() => listeners(page, "tauri://close-requested"), { message: "the settings page listens to close requests" })
    .toBeGreaterThan(0);
  await tab(page, "history").click();
  await field(page, "history.size").fill("33");
  await expect(field(page, "history.size")).toHaveValue("33");
  await tab(page, "general").click();

  await aboutButton(page).click();
  const about = aboutDialog(page);
  await expect(about.getByText(LINE, { exact: true })).toBeVisible();

  await requestClose(page);
  const prompt = page.getByRole("alertdialog");
  await expect(prompt).toBeVisible();
  await expect(about).toBeHidden();
  await expect(prompt.getByTestId("settings-discard-keep")).toBeFocused();

  // The prompt is operable: Keep editing closes it and keeps the draft, nothing destroyed.
  await page.keyboard.press("Enter");
  await expect(prompt).toBeHidden();
  await tab(page, "history").click();
  await expect(field(page, "history.size")).toHaveValue("33");
  await settle(page);
  expect(await calls(page, "plugin:window|destroy")).toEqual([]);
  expect(errors).toEqual([]);
});

// (7) review 1 #1 ------------------------------------------------------------------
// A settings://focus request to another tab unmounts General and the portalled
// <dialog> while About is open; no `close` fires. Back on General, About must not
// pop up by itself, and the next opening by the button gets its own get_build_info
// answer (never an empty dialog with no call).

test("About open, a settings://focus request moves to another tab, back on General -> About is not shown unasked, and reopening by the button calls get_build_info again and shows the version line", async ({ page }) => {
  const errors = pageErrors(page);
  await openGeneral(page, { buildInfo: OK });
  await expect
    .poll(() => listeners(page, "settings://focus"), { message: "the settings page listens to settings://focus" })
    .toBeGreaterThan(0);

  await aboutButton(page).click();
  const dialog = aboutDialog(page);
  await expect(dialog.getByText(LINE, { exact: true })).toBeVisible();
  expect(await buildInfoCalls(page)).toBe(1);

  // The second path: the shell asks the open window for another tab while About is open.
  await emit(page, "settings://focus", { tab: "recording", field: "recording.hotkey" });
  await expect(tab(page, "recording")).toHaveAttribute("aria-selected", "true");
  await expect(dialog).toHaveCount(0);

  // A new answer, so a line kept from the first opening cannot pass.
  await setBuildInfo(page, { version: "0.3.0", commit: "fed9876" });
  await tab(page, "general").click();
  await expect(tab(page, "general")).toHaveAttribute("aria-selected", "true");
  await expect(aboutButton(page)).toBeVisible();
  await settle(page);
  // Not shown unasked, and no load nobody asked for.
  await expect(dialog).toBeHidden();
  expect(await buildInfoCalls(page)).toBe(1);

  // Opening by the button is a new showing: its own call, its own line.
  await aboutButton(page).click();
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText("Voicen 0.3.0 (fed9876)", { exact: true })).toBeVisible();
  await expect(dialog).not.toContainText(LINE);
  await expect(dialog.getByRole("alert")).toHaveCount(0);
  expect(await buildInfoCalls(page)).toBe(2);

  // And it still closes normally.
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(aboutButton(page)).toBeFocused();
  expect(errors).toEqual([]);
});

// (8) validation 1 M2 ----------------------------------------------------------------
// Each opening clears the previous result; while its own get_build_info is pending the
// dialog shows nothing (settings-ui.md § A), and an answer to an earlier opening is
// dropped (`current === opening`).

/** No build-info text at all: no version line of any value and no alert. */
async function expectNoBuildInfo(dialog: Locator): Promise<void> {
  await expect(dialog.getByText(/^Voicen \S+ \(\S+\)$/)).toHaveCount(0);
  await expect(dialog.getByRole("alert")).toHaveCount(0);
}

test("reopen while get_build_info is pending: open (ok), close, the next answer is held, reopen -> no version line and no alert until the answer, then the new line", async ({ page }) => {
  const errors = pageErrors(page);
  await openGeneral(page, { buildInfo: OK });

  await aboutButton(page).click();
  const dialog = aboutDialog(page);
  await expect(dialog.getByText(LINE, { exact: true })).toBeVisible();
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await expect(dialog).toBeHidden();

  await holdBuildInfo(page);
  await setBuildInfo(page, { version: "0.4.0", commit: "0a1b2c3" });
  await aboutButton(page).click();
  await expect(dialog).toBeVisible();
  // The reopen's call is made and still in flight.
  await expect.poll(() => buildInfoCalls(page)).toBe(2);
  await settle(page);
  await expect(dialog).not.toContainText(LINE);
  await expectNoBuildInfo(dialog);
  // The rest of the dialog is there while pending.
  await expect(dialog.getByText(LICENSE, { exact: true })).toBeVisible();

  await releaseBuildInfo(page);
  await expect(dialog.getByText("Voicen 0.4.0 (0a1b2c3)", { exact: true })).toBeVisible();
  await expect(dialog).not.toContainText(LINE);
  await expect(dialog.getByRole("alert")).toHaveCount(0);
  expect(await buildInfoCalls(page)).toBe(2);
  expect(errors).toEqual([]);
});

test("a late answer of an earlier opening is dropped: open (held), close, reopen (held), the reopen's answer arrives, then the first one -> only the reopen's line is shown", async ({ page }) => {
  const errors = pageErrors(page);
  await openGeneral(page, { buildInfo: OK });
  await holdBuildInfo(page);

  await aboutButton(page).click();
  const dialog = aboutDialog(page);
  await expect(dialog).toBeVisible();
  await expect.poll(() => buildInfoCalls(page)).toBe(1);
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await expect(dialog).toBeHidden();

  await setBuildInfo(page, { version: "0.5.0", commit: "5e6f7a8" });
  await aboutButton(page).click();
  await expect(dialog).toBeVisible();
  await expect.poll(() => buildInfoCalls(page)).toBe(2);
  await settle(page);
  await expectNoBuildInfo(dialog);

  // The reopen's answer first, then the first opening's (OK) arrives late.
  await releaseBuildInfo(page, "newest");
  await expect(dialog.getByText("Voicen 0.5.0 (5e6f7a8)", { exact: true })).toBeVisible();
  await releaseBuildInfo(page, "oldest");
  // The release's handlers ran with the evaluate; two frames let the render land.
  await settle(page);
  await expect(dialog.getByText("Voicen 0.5.0 (5e6f7a8)", { exact: true })).toBeVisible();
  await expect(dialog).not.toContainText(LINE);
  await expect(dialog.getByRole("alert")).toHaveCount(0);

  // A late rejection of an earlier opening is dropped too: hold, close, reopen with a
  // new answer, then let a rejection meant for a closed opening arrive last.
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await expect(dialog).toBeHidden();
  await setBuildInfo(page, { reject: REJECTION });
  await aboutButton(page).click();
  await expect.poll(() => buildInfoCalls(page)).toBe(3);
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await expect(dialog).toBeHidden();
  await setBuildInfo(page, { version: "0.6.0", commit: "6b7c8d9" });
  await aboutButton(page).click();
  await expect.poll(() => buildInfoCalls(page)).toBe(4);
  await releaseBuildInfo(page, "newest");
  await expect(dialog.getByText("Voicen 0.6.0 (6b7c8d9)", { exact: true })).toBeVisible();
  await releaseBuildInfo(page, "oldest");
  await settle(page);
  await expect(dialog.getByRole("alert")).toHaveCount(0);
  await expect(dialog.getByText("Voicen 0.6.0 (6b7c8d9)", { exact: true })).toBeVisible();
  expect(errors).toEqual([]);
});
