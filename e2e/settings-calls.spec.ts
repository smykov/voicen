// T-155 red e2e: the Calls tab of the settings window (spec 007 US9, FR-031, FR-038 to
// FR-040, FR-046; decision #319). Refusals, the provider host and the reasons are core's:
// the window holds no hotkey-equality, writability or loopback rule (settings-ui W, P-010).
// The outcomes and views are core's own wire (e2e/fixtures/settings-wire.json, pinned by
// core's tests/call_settings.rs), read through the accessors of e2e/support/tauriMock.ts.
//
// Locator contract this spec adds (T-155 analysis):
// - tab `tab-calls`, between History and General (General stays last);
// - four controls with `data-field`: calls.hotkey (readonly, captures keys like
//   recording.hotkey), calls.reminder (checkbox), calls.primary_language (select over
//   settings_speech_languages, no "auto"), calls.storage_root (text; "" shows null and an
//   empty entry is sent as null); each is named by `settings.field_label.calls.<field>`;
// - an error of a field with a control is shown next to it (aria-invalid=true, the text in
//   the control's accessible description) and is not in the form-level `settings-errors`
//   list; with the field's tab not shown it is in that list instead, exactly once (L);
// - the provider line is the text `t(settings.calls.provider_notice, {host})` from the saved
//   view's `provider_host` (core); absent for null.
// The English reason text is the owner's wording (decision #319 Q2); the Russian texts are new
// catalog content, so they are read from i18n/ru.json (a missing id fails by name).
import { readFileSync } from "node:fs";
import type { Page } from "@playwright/test";
import { bootPage, expect, test } from "./support/boot";
import {
  calls,
  coreSpeechLanguages,
  emitted,
  firstRunView,
  installTauriMock,
  loopbackApiView,
  queueSaveOutcome,
  refusedCallHotkey,
  refusedStorageUnwritable,
  storedView,
  type MockOptions,
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

function field(page: Page, id: string) {
  return page.locator(`[data-field="${id}"]`);
}

function tab(page: Page, name: string) {
  return page.getByTestId(`tab-${name}`);
}

function savedStatus(page: Page, text: string) {
  return page.getByRole("status").filter({ hasText: text });
}

function pageErrors(page: Page): Error[] {
  const errors: Error[] = [];
  page.on("pageerror", (error) => errors.push(error));
  return errors;
}

async function open(
  page: Page,
  path = "/settings",
  view: SettingsView = firstRunView(),
  options: Omit<MockOptions, "view"> = {},
): Promise<void> {
  await installTauriMock(page, { ...options, view });
  await bootPage(page, path, page.getByRole("tab", { selected: true }));
}

/** `view` as a window shows it with a provider host (T-155 wire field of core's view). */
function withHost(view: SettingsView, host: string | null): SettingsView {
  return Object.assign(view, { provider_host: host });
}

async function lastSaveRequest(page: Page): Promise<SaveRequest> {
  await expect.poll(async () => (await calls(page, "settings_save")).length).toBe(1);
  const [save] = await calls(page, "settings_save");
  return (save.args as { request: SaveRequest }).request;
}

const PROVIDER_LINE = "The other party's audio goes to provider api.openai.com";

// ---- (1) The tab and its four controls --------------------------------------------------

test("the Calls tab sits between History and General and shows the call hotkey, the reminder, the primary language and the storage folder with the first-run values", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page);

  const order = await page
    .getByRole("tab")
    .evaluateAll((els) => els.map((e) => (e as HTMLElement).dataset.testid ?? ""));
  expect(order).toEqual([
    "tab-engine",
    "tab-recording",
    "tab-output",
    "tab-post_processing",
    "tab-history",
    "tab-calls",
    "tab-general",
  ]);
  await expect(tab(page, "calls")).toHaveText(en("settings.tab.calls"));

  await tab(page, "calls").click();
  await expect(tab(page, "calls")).toHaveAttribute("aria-selected", "true");
  const controls = await page
    .locator("[data-field]")
    .evaluateAll((els) => els.map((e) => (e as HTMLElement).dataset.field ?? ""));
  expect(controls).toEqual(["calls.hotkey", "calls.reminder", "calls.primary_language", "calls.storage_root"]);

  const first = firstRunView().settings.calls;
  await expect(field(page, "calls.hotkey")).toHaveValue(first.hotkey);
  await expect(field(page, "calls.hotkey")).toHaveValue("Ctrl+Alt+R");
  await expect(field(page, "calls.reminder")).toBeChecked({ checked: first.reminder });
  await expect(field(page, "calls.primary_language")).toHaveValue(first.primary_language);
  await expect(field(page, "calls.storage_root")).toHaveValue("");

  // Each control is named by its catalog label.
  await expect(page.getByLabel(en("settings.field_label.calls.hotkey"))).toHaveAttribute("data-field", "calls.hotkey");
  await expect(page.getByLabel(en("settings.field_label.calls.reminder"))).toHaveAttribute("data-field", "calls.reminder");
  await expect(page.getByLabel(en("settings.field_label.calls.primary_language"))).toHaveAttribute(
    "data-field",
    "calls.primary_language",
  );
  await expect(page.getByLabel(en("settings.field_label.calls.storage_root"))).toHaveAttribute(
    "data-field",
    "calls.storage_root",
  );

  // The language select offers core's languages and no "auto" (a call has a primary language).
  const options = await field(page, "calls.primary_language")
    .locator("option")
    .evaluateAll((els) => els.map((e) => (e as HTMLOptionElement).value));
  expect(options).toEqual(coreSpeechLanguages());
  expect(options).not.toContain("auto");
  expect(errors).toEqual([]);
});

test("editing the four Calls controls and saving sends them in request.settings.calls and changes nothing else", async ({ page }) => {
  const errors = pageErrors(page);
  const view = firstRunView();
  await open(page, "/settings", view);
  await tab(page, "calls").click();

  const hotkey = field(page, "calls.hotkey");
  await hotkey.press("Control+Shift+F10");
  await expect(hotkey).toHaveValue("Ctrl+Shift+F10");
  await field(page, "calls.reminder").uncheck();
  await field(page, "calls.primary_language").selectOption("de");
  await field(page, "calls.storage_root").fill("D:\\Calls");
  await page.getByTestId("settings-save").click();

  const request = await lastSaveRequest(page);
  const expected = structuredClone(view.settings);
  expected.calls.hotkey = "Ctrl+Shift+F10";
  expected.calls.reminder = false;
  expected.calls.primary_language = "de";
  expected.calls.storage_root = "D:\\Calls";
  expect(request.settings).toEqual(expected);
  await expect(savedStatus(page, en("settings.saved"))).toBeVisible();
  expect(errors).toEqual([]);
});

test("an untouched Save after visiting the Calls tab sends calls unchanged: a null storage_root stays null", async ({ page }) => {
  const view = firstRunView();
  expect(view.settings.calls.storage_root).toBeNull();
  await open(page, "/settings", view);
  await tab(page, "calls").click();
  await expect(field(page, "calls.storage_root")).toHaveValue("");
  await page.getByTestId("settings-save").click();

  const request = await lastSaveRequest(page);
  expect(request.settings.calls).toEqual(view.settings.calls);
  expect(request.settings.calls.storage_root).toBeNull();
});

test("an empty storage folder entry is sent as null, not as an empty string", async ({ page }) => {
  await open(page);
  await tab(page, "calls").click();
  const root = field(page, "calls.storage_root");
  await root.fill("D:\\Calls");
  await root.fill("");
  await page.getByTestId("settings-save").click();

  const request = await lastSaveRequest(page);
  expect(request.settings.calls.storage_root).toBeNull();
});

// ---- (2) Failure branch: a refused calls.hotkey -----------------------------------------

test("failure branch: a call-side hotkey.equals_dictation refusal marks calls.hotkey with its error text, nothing is saved and no Saved status shows", async ({ page }) => {
  const errors = pageErrors(page);
  const view = firstRunView();
  await open(page, "/settings?tab=calls", view);
  await expect(tab(page, "calls")).toHaveAttribute("aria-selected", "true");
  const outcome = refusedCallHotkey("equals_dictation_call_side");
  expect(outcome.Refused.errors).toEqual([{ field: "calls.hotkey", code: "hotkey.equals_dictation" }]);
  await queueSaveOutcome(page, outcome);

  const hotkey = field(page, "calls.hotkey");
  await hotkey.press("Control+Alt+Space");
  await page.getByTestId("settings-save").click();

  // The edited hotkey reached core once; the window did not judge it.
  const request = await lastSaveRequest(page);
  expect(request.settings.calls.hotkey).toBe("Ctrl+Alt+Space");

  await expect(hotkey).toHaveAttribute("aria-invalid", "true");
  await expect(hotkey).toHaveAccessibleDescription(new RegExp(en("error.hotkey.equals_dictation").slice(0, 30)));
  await expect(page.getByText(en("error.hotkey.equals_dictation"))).toHaveCount(1);
  // On the field, not in the form-level list.
  await expect(page.getByTestId("settings-errors")).toHaveCount(0);
  // The dictation hotkey is not the marked field.
  await expect(hotkey).toHaveValue("Ctrl+Alt+Space"); // draft kept
  await expect(savedStatus(page, en("settings.saved"))).toHaveCount(0);

  expect(await storedView(page)).toEqual(view);
  expect(await emitted(page, "settings://changed")).toEqual([]);
  expect(errors).toEqual([]);
});

test("failure branch: a dictation-side hotkey.equals_dictation refusal on calls.hotkey is listed at form level on the Recording tab and marks the field on the Calls tab, exactly once at a time", async ({ page }) => {
  const errors = pageErrors(page);
  const view = firstRunView();
  await open(page, "/settings?tab=recording", view);
  const outcome = refusedCallHotkey("equals_dictation_dictation_side");
  expect(outcome.Refused.errors).toEqual([{ field: "calls.hotkey", code: "hotkey.equals_dictation" }]);
  await queueSaveOutcome(page, outcome);

  await field(page, "recording.hotkey").press("Control+Alt+R");
  await page.getByTestId("settings-save").click();
  const request = await lastSaveRequest(page);
  expect(request.settings.hotkey).toBe("Ctrl+Alt+R");

  // Recording tab: no control for calls.hotkey, so the form-level list names it once, by label.
  const list = page.getByTestId("settings-errors");
  await expect(list.getByRole("listitem")).toHaveText([
    `${en("settings.field_label.calls.hotkey")} ${en("error.hotkey.equals_dictation")}`,
  ]);
  await expect(page.getByText(en("error.hotkey.equals_dictation"))).toHaveCount(1);
  await expect(field(page, "recording.hotkey")).not.toHaveAttribute("aria-invalid", "true");

  // Calls tab: the control carries the mark and the list no longer names the field.
  await tab(page, "calls").click();
  const hotkey = field(page, "calls.hotkey");
  await expect(hotkey).toHaveAttribute("aria-invalid", "true");
  await expect(hotkey).toHaveAccessibleDescription(new RegExp(en("error.hotkey.equals_dictation").slice(0, 30)));
  await expect(page.getByText(en("error.hotkey.equals_dictation"))).toHaveCount(1);
  await expect(page.getByTestId("settings-errors")).toHaveCount(0);

  // Back on Recording it is listed again, once.
  await tab(page, "recording").click();
  await expect(page.getByTestId("settings-errors").getByRole("listitem")).toHaveCount(1);
  await expect(page.getByText(en("error.hotkey.equals_dictation"))).toHaveCount(1);

  await expect(savedStatus(page, en("settings.saved"))).toHaveCount(0);
  expect(await storedView(page)).toEqual(view);
  expect(errors).toEqual([]);
});

test("failure branch: a save-time registration conflict (hotkey.unavailable) marks calls.hotkey with the unavailable text and saves nothing", async ({ page }) => {
  const view = firstRunView();
  await open(page, "/settings?tab=calls", view);
  const outcome = refusedCallHotkey("unavailable");
  expect(outcome.Refused.errors).toEqual([{ field: "calls.hotkey", code: "hotkey.unavailable" }]);
  await queueSaveOutcome(page, outcome);

  const hotkey = field(page, "calls.hotkey");
  await hotkey.press("Control+Alt+F8");
  await page.getByTestId("settings-save").click();
  await lastSaveRequest(page);

  await expect(hotkey).toHaveAttribute("aria-invalid", "true");
  await expect(page.getByText(en("error.hotkey.unavailable"))).toHaveCount(1);
  await expect(page.getByText(en("error.hotkey.equals_dictation"))).toHaveCount(0);
  await expect(page.getByTestId("settings-errors")).toHaveCount(0);
  await expect(savedStatus(page, en("settings.saved"))).toHaveCount(0);
  expect(await storedView(page)).toEqual(view);
});

// ---- (3) Failure branch: an unwritable storage folder -----------------------------------

test("failure branch: an unwritable storage root highlights calls.storage_root with the full reason message \"This folder can't be written: access denied\"", async ({ page }) => {
  const errors = pageErrors(page);
  const view = firstRunView();
  await open(page, "/settings?tab=calls", view);
  const outcome = refusedStorageUnwritable();
  expect(outcome.Refused.errors).toEqual([
    { field: "calls.storage_root", code: "storage.unwritable", reason: "access_denied" },
  ]);
  await queueSaveOutcome(page, outcome);

  const root = field(page, "calls.storage_root");
  await root.fill("C:\\Windows\\System32");
  await page.getByTestId("settings-save").click();
  const request = await lastSaveRequest(page);
  expect(request.settings.calls.storage_root).toBe("C:\\Windows\\System32");

  await expect(root).toHaveAttribute("aria-invalid", "true");
  await expect(page.getByText("This folder can't be written: access denied")).toHaveCount(1);
  await expect(root).toHaveAccessibleDescription(/This folder can't be written: access denied/);
  // The reason's own message replaces the generic one; no raw code or reason id is shown.
  await expect(page.getByText(en("error.storage.unwritable"))).toHaveCount(0);
  await expect(page.getByText("storage.unwritable")).toHaveCount(0);
  await expect(page.getByText("access_denied")).toHaveCount(0);
  await expect(page.getByTestId("settings-errors")).toHaveCount(0);
  await expect(root).toHaveValue("C:\\Windows\\System32"); // draft kept
  await expect(savedStatus(page, en("settings.saved"))).toHaveCount(0);
  expect(await storedView(page)).toEqual(view);
  expect(errors).toEqual([]);
});

// ---- (4) The provider line ---------------------------------------------------------------

test("the provider line names the host core computed for a remote API, and is absent for a loopback API", async ({ page }) => {
  // First-run view: api.base_url is OpenAI's, core's provider_host is api.openai.com.
  expect(firstRunView(), "settings-wire.json first_run_view carries provider_host").toHaveProperty(
    "provider_host",
    "api.openai.com",
  );
  await open(page);
  await tab(page, "calls").click();
  await expect(page.getByText(PROVIDER_LINE)).toHaveCount(1);
  await expect(page.getByText(en("settings.calls.provider_notice").replace("{host}", "api.openai.com"))).toHaveCount(1);
});

test("the provider line is absent when core's view has provider_host null (http://localhost:8000/v1)", async ({ page }) => {
  const view = loopbackApiView();
  expect(view.settings.api.base_url).toBe("http://localhost:8000/v1");
  expect(view, "loopback_api_view carries provider_host null").toHaveProperty("provider_host", null);
  await open(page, "/settings", view);
  await tab(page, "calls").click();
  await expect(field(page, "calls.hotkey")).toBeVisible();
  await expect(page.getByText(/The other party's audio goes to provider/)).toHaveCount(0);
});

test("the provider line shows the view's own host, not a fixed one, and follows the saved view, not the typed Engine URL", async ({ page }) => {
  await open(page, "/settings", withHost(firstRunView(), "stt.example.com"));
  await tab(page, "calls").click();
  await expect(page.getByText("The other party's audio goes to provider stt.example.com")).toHaveCount(1);
  await expect(page.getByText(/provider api\.openai\.com/)).toHaveCount(0);

  // Typing a loopback URL on the Engine tab changes the draft, not the saved view: the window
  // holds no URL rule, so the line stays until a Save returns a new view.
  await tab(page, "engine").click();
  await field(page, "engine.kind").selectOption("api");
  await field(page, "engine.api.base_url").fill("http://localhost:8000/v1");
  await tab(page, "calls").click();
  await expect(page.getByText("The other party's audio goes to provider stt.example.com")).toHaveCount(1);
});

test("a Saved view with provider_host null removes the line, a Saved view with a host shows it", async ({ page }) => {
  await open(page, "/settings", withHost(firstRunView(), "api.openai.com"));
  await tab(page, "calls").click();
  await expect(page.getByText(PROVIDER_LINE)).toHaveCount(1);

  const saved = withHost(firstRunView(), null);
  await queueSaveOutcome(page, { Saved: { view: saved, warnings: [] } });
  await page.getByTestId("settings-save").click();
  await expect(savedStatus(page, en("settings.saved"))).toBeVisible();
  await expect(page.getByText(/The other party's audio goes to provider/)).toHaveCount(0);
});

// ---- (5) A saved value persists; Russian -------------------------------------------------

test("a saved Calls value is shown again when the window is reopened over the stored view", async ({ page, context }) => {
  // The mock lives in the page, so a reload would reset it; "reopen" boots a fresh page over
  // the view the mock stored (what settings_get returns after the save).
  await open(page);
  await tab(page, "calls").click();
  await field(page, "calls.reminder").uncheck();
  await field(page, "calls.primary_language").selectOption("de");
  await field(page, "calls.storage_root").fill("D:\\Calls");
  await page.getByTestId("settings-save").click();
  await expect(savedStatus(page, en("settings.saved"))).toBeVisible();
  const stored = await storedView(page);
  expect(stored.settings.calls.reminder).toBe(false);
  expect(stored.settings.calls.primary_language).toBe("de");
  expect(stored.settings.calls.storage_root).toBe("D:\\Calls");

  // Same page, after the Saved view rebuilt the draft: away and back, still shown.
  await tab(page, "general").click();
  await tab(page, "calls").click();
  await expect(field(page, "calls.reminder")).not.toBeChecked();
  await expect(field(page, "calls.primary_language")).toHaveValue("de");
  await expect(field(page, "calls.storage_root")).toHaveValue("D:\\Calls");

  const reopened = await context.newPage();
  await installTauriMock(reopened, { view: stored });
  await bootPage(reopened, "/settings?tab=calls", reopened.getByRole("tab", { selected: true }));
  await expect(field(reopened, "calls.reminder")).not.toBeChecked();
  await expect(field(reopened, "calls.primary_language")).toHaveValue("de");
  await expect(field(reopened, "calls.storage_root")).toHaveValue("D:\\Calls");
  // An untouched Save of the reopened window sends the stored values back.
  await reopened.getByTestId("settings-save").click();
  await expect.poll(async () => (await calls(reopened, "settings_save")).length).toBe(1);
  const [save] = await calls(reopened, "settings_save");
  expect((save.args as { request: SaveRequest }).request.settings.calls).toEqual(stored.settings.calls);
});

test("with ui_language ru the Calls tab, its labels, the provider line and the reason message render in Russian", async ({ page }) => {
  const errors = pageErrors(page);
  const view = firstRunView();
  view.settings.ui_language = "ru";
  await open(page, "/settings?tab=calls", withHost(view, "api.openai.com"));

  expect(ru("settings.tab.calls")).not.toBe(en("settings.tab.calls"));
  await expect(tab(page, "calls")).toHaveText(ru("settings.tab.calls"));
  for (const id of ["hotkey", "reminder", "primary_language", "storage_root"]) {
    const text = ru(`settings.field_label.calls.${id}`);
    expect(text).not.toBe(en(`settings.field_label.calls.${id}`));
    await expect(page.getByLabel(text)).toHaveAttribute("data-field", `calls.${id}`);
    await expect(page.getByText(en(`settings.field_label.calls.${id}`))).toHaveCount(0);
  }
  const line = ru("settings.calls.provider_notice").replace("{host}", "api.openai.com");
  await expect(page.getByText(line)).toHaveCount(1);
  await expect(page.getByText(PROVIDER_LINE)).toHaveCount(0);

  await queueSaveOutcome(page, refusedStorageUnwritable());
  await field(page, "calls.storage_root").fill("C:\\Windows\\System32");
  await page.getByTestId("settings-save").click();
  await expect.poll(async () => (await calls(page, "settings_save")).length).toBe(1);

  const reason = ru("settings.calls.unwritable.access_denied");
  expect(reason).not.toBe(en("settings.calls.unwritable.access_denied"));
  await expect(page.getByText(reason)).toHaveCount(1);
  await expect(page.getByText("This folder can't be written")).toHaveCount(0);
  await expect(field(page, "calls.storage_root")).toHaveAttribute("aria-invalid", "true");
  expect(errors).toEqual([]);
});
