// T-004 red e2e: the settings window over the mocked IPC (e2e/support/tauriMock.ts).
//
// Locator contract this spec pins for src/routes/settings (T-004 Investigation 5):
// - route `/settings` (the shell opens `settings?tab=<tab>`); no `?tab=` -> Engine tab;
// - each tab is `role="tab"` with test id `tab-<tab>`, <tab> one of the settings://focus
//   names (engine, recording, output, post_processing, history, general); the active one
//   has aria-selected="true";
// - every field's form control (input / select / textarea itself) carries
//   `data-field="<FieldId>"` (contracts/ipc.md FieldId, the dotted string), its value
//   is the wire value: `engine.kind` and `recording.mode` and `general.ui_language` are
//   <select>s with the wire values as option values; `engine.speech_language` is a
//   <select> with option `auto` (wire null) plus one option per code of
//   settings_speech_languages; booleans are checkboxes; `recording.hotkey` is an input
//   whose value is the hotkey text; key fields are <input type="password">;
// - a refused field has aria-invalid="true" and its accessible description
//   (aria-describedby) contains the text of `error.<code>`;
// - form-level messages (form_error, a rejected invoke) are in a `role="alert"`;
// - the Save button has test id `settings-save`.
import { readFileSync } from "node:fs";
import { expect, test, type Page } from "@playwright/test";
import {
  calls,
  coreSpeechLanguages,
  emit,
  emitted,
  firstRunView,
  installTauriMock,
  listeners,
  queueSaveOutcome,
  queueSaveRejection,
  storedView,
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

function escapeRegExp(s: string): RegExp {
  return new RegExp(s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
}

/** The form control of a field (the `data-field` contract above). */
function field(page: Page, id: string) {
  return page.locator(`[data-field="${id}"]`);
}

function tab(page: Page, name: string) {
  return page.getByTestId(`tab-${name}`);
}

/** The single settings_save request so far (fails unless exactly one was made). */
async function onlySaveRequest(page: Page): Promise<SaveRequest> {
  await expect.poll(async () => (await calls(page, "settings_save")).length).toBe(1);
  const [call] = await calls(page, "settings_save");
  return (call.args as { request: SaveRequest }).request;
}

/** Values of every input and textarea on the page (key values must never be among them). */
async function inputValues(page: Page): Promise<string[]> {
  return page.evaluate(() =>
    [...document.querySelectorAll<HTMLInputElement | HTMLTextAreaElement>("input, textarea")].map((e) => e.value),
  );
}

async function openSettings(page: Page, view?: SettingsView, path = "/settings"): Promise<void> {
  await installTauriMock(page, view ? { view } : {});
  await page.goto(path);
  await expect(page.getByRole("tab", { selected: true })).toBeVisible();
}

const FAKE_KEY = "sk-test-FAKE-0000-not-a-real-key";

// ---- Acceptance 1 ---------------------------------------------------------------

test("first run opens on the Engine tab with engine none and the v4 defaults shown", async ({ page }) => {
  const view = firstRunView();
  const s = view.settings;
  await openSettings(page, view);

  await expect(page.getByRole("tab", { selected: true })).toHaveCount(1);
  await expect(tab(page, "engine")).toHaveAttribute("role", "tab");
  await expect(tab(page, "engine")).toHaveAttribute("aria-selected", "true");
  await expect(field(page, "engine.kind")).toHaveValue("none");

  // Speech language: auto (null) selected; the list is exactly core's, from IPC.
  await expect(field(page, "engine.speech_language")).toHaveValue("auto");
  await expect(field(page, "engine.speech_language").locator("option")).toHaveCount(
    coreSpeechLanguages().length + 1,
  );
  expect((await calls(page, "settings_speech_languages")).length).toBeGreaterThanOrEqual(1);

  // Every other default comes from the view (core's defaults(), via the fixture).
  await tab(page, "recording").click();
  await expect(tab(page, "recording")).toHaveAttribute("aria-selected", "true");
  await expect(field(page, "recording.hotkey")).toHaveValue(s.hotkey);
  await expect(field(page, "recording.mode")).toHaveValue(s.mode);

  await tab(page, "output").click();
  if (s.auto_paste) await expect(field(page, "output.auto_paste")).toBeChecked();
  else await expect(field(page, "output.auto_paste")).not.toBeChecked();

  await tab(page, "history").click();
  if (s.history.enabled) await expect(field(page, "history.enabled")).toBeChecked();
  else await expect(field(page, "history.enabled")).not.toBeChecked();
  await expect(field(page, "history.size")).toHaveValue(String(s.history.size));

  await tab(page, "general").click();
  await expect(field(page, "general.ui_language")).toHaveValue(s.ui_language);

  // Not unavailable: no unavailable notice; nothing saved by opening the window.
  await expect(page.getByText(en("notice.settings_unavailable"))).toHaveCount(0);
  expect(await calls(page, "settings_save")).toEqual([]);
});

test("the speech-language picker lists exactly the codes settings_speech_languages returns", async ({ page }) => {
  // U1: the UI keeps no language list of its own (decision #30).
  await installTauriMock(page, { speechLanguages: ["en", "ru", "de"] });
  await page.goto("/settings");
  const options = field(page, "engine.speech_language").locator("option");
  await expect(options).toHaveCount(4);
  const values = await options.evaluateAll((els) => els.map((e) => (e as HTMLOptionElement).value));
  expect(values.filter((v) => v !== "auto").sort()).toEqual(["de", "en", "ru"]);
});

// ---- Acceptance 2 ---------------------------------------------------------------

test("choosing API, entering URL, model and key, then Save calls settings_save once with those values; the key field is masked", async ({ page }) => {
  const view = firstRunView();
  await openSettings(page, view);

  await field(page, "engine.kind").selectOption("api");
  // The API defaults are core's, shown before any edit.
  await expect(field(page, "engine.api.base_url")).toHaveValue(view.settings.api.base_url);
  await expect(field(page, "engine.api.model")).toHaveValue(view.settings.api.model);

  const key = field(page, "engine.api.key");
  await expect(key).toHaveAttribute("type", "password");
  await expect(key).toHaveValue(""); // presence only, never a value

  await field(page, "engine.api.base_url").fill("https://stt.example.com/v1");
  await field(page, "engine.api.model").fill("whisper-fake-1");
  await key.fill(FAKE_KEY);
  await expect(key).toHaveAttribute("type", "password");
  await page.getByTestId("settings-save").click();

  const request = await onlySaveRequest(page);
  const expected = structuredClone(view.settings);
  expected.engine = "api";
  expected.api = { base_url: "https://stt.example.com/v1", model: "whisper-fake-1" };
  expect(request.settings).toEqual(expected);
  expect(request.keys).toEqual({
    transcription_api: { Replace: FAKE_KEY },
    local_server: "Untouched",
    post_processing: "Untouched",
  });

  // U3: after Saved the key input is empty again and no key value is anywhere on the page.
  await expect(key).toHaveValue("");
  expect(await inputValues(page)).not.toContain(FAKE_KEY);
  await expect(page.locator("body")).not.toContainText(FAKE_KEY);
  // The saved view is the effect: engine api with presence, still exactly one save.
  const stored = await storedView(page);
  expect(stored.settings.engine).toBe("api");
  expect(stored.keys.transcription_api).toBe(true);
  expect(JSON.stringify(stored)).not.toContain(FAKE_KEY);
  expect((await calls(page, "settings_save")).length).toBe(1);
  await expect(field(page, "engine.kind")).toHaveValue("api");
});

// ---- Acceptance 3 ---------------------------------------------------------------

test("a saved change is reflected without reload, and ui_language ru re-renders visible text in Russian", async ({ page }) => {
  const view = firstRunView();
  await openSettings(page, view);
  await expect.poll(() => listeners(page, "settings://changed")).toBeGreaterThan(0);
  await page.evaluate(() => {
    (window as unknown as { __noReload: boolean }).__noReload = true;
  });

  // Two visible texts in English, each a catalog text (the ids are the UI's own).
  const engineTabText = (await tab(page, "engine").innerText()).trim();
  const saveText = (await page.getByTestId("settings-save").innerText()).trim();
  const engineTabId = Object.keys(EN).find((id) => EN[id] === engineTabText);
  const saveId = Object.keys(EN).find((id) => EN[id] === saveText);
  expect(engineTabId, `engine tab text ${JSON.stringify(engineTabText)} is not an en catalog text`).toBeDefined();
  expect(saveId, `save button text ${JSON.stringify(saveText)} is not an en catalog text`).toBeDefined();
  expect(RU[engineTabId!]).not.toBe(EN[engineTabId!]);
  expect(RU[saveId!]).not.toBe(EN[saveId!]);

  // Another window saved: ui_language ru, mode toggle, history size 35.
  const changed = structuredClone(view);
  changed.first_run = false;
  changed.settings.ui_language = "ru";
  changed.settings.mode = "toggle";
  changed.settings.history.size = 35;
  await emit(page, "settings://changed", changed);

  await expect(tab(page, "engine")).toHaveText(RU[engineTabId!]);
  await expect(page.getByTestId("settings-save")).toHaveText(RU[saveId!]);
  await tab(page, "recording").click();
  await expect(field(page, "recording.mode")).toHaveValue("toggle");
  await tab(page, "history").click();
  await expect(field(page, "history.size")).toHaveValue("35");
  await tab(page, "general").click();
  await expect(field(page, "general.ui_language")).toHaveValue("ru");

  // Without reload, and the event alone did it (no save from this window).
  expect(await page.evaluate(() => (window as unknown as { __noReload?: boolean }).__noReload)).toBe(true);
  expect(await calls(page, "settings_save")).toEqual([]);
});

// ---- Acceptance 4 (failure branch) -------------------------------------------------

test("Save with an empty base URL is refused by core: the field shows error.required, the draft is kept, the saved view is unchanged and no settings://changed follows", async ({ page }) => {
  const view = firstRunView();
  await openSettings(page, view);
  await queueSaveOutcome(page, {
    Refused: { errors: [{ field: "engine.api.base_url", code: "required" }], form_error: null },
  });

  await field(page, "engine.kind").selectOption("api");
  await field(page, "engine.api.base_url").fill("");
  await field(page, "engine.api.model").fill("whisper-fake-2");
  await page.getByTestId("settings-save").click();

  // The UI does not validate (P-010, decision #38): the empty URL reaches core once.
  const request = await onlySaveRequest(page);
  expect(request.settings.engine).toBe("api");
  expect(request.settings.api.base_url).toBe("");

  const url = field(page, "engine.api.base_url");
  await expect(url).toHaveAttribute("aria-invalid", "true");
  await expect(url).toHaveAccessibleDescription(escapeRegExp(en("error.required")));
  // Only the named field is highlighted.
  await expect(field(page, "engine.api.model")).not.toHaveAttribute("aria-invalid", "true");

  // The draft is kept.
  await expect(field(page, "engine.kind")).toHaveValue("api");
  await expect(url).toHaveValue("");
  await expect(field(page, "engine.api.model")).toHaveValue("whisper-fake-2");

  // Nothing was saved and nothing announced.
  expect(await storedView(page)).toEqual(view);
  expect(await emitted(page, "settings://changed")).toEqual([]);
  expect((await calls(page, "settings_save")).length).toBe(1);
});

test("a Refused recording.hotkey hotkey.unavailable shows error.hotkey.unavailable on the hotkey field and the saved value stays", async ({ page }) => {
  const view = firstRunView();
  await openSettings(page, view);
  await queueSaveOutcome(page, {
    Refused: { errors: [{ field: "recording.hotkey", code: "hotkey.unavailable" }], form_error: null },
  });

  await tab(page, "recording").click();
  await field(page, "recording.mode").selectOption("toggle");
  await page.getByTestId("settings-save").click();

  const request = await onlySaveRequest(page);
  expect(request.settings.hotkey).toBe(view.settings.hotkey);
  expect(request.settings.mode).toBe("toggle");

  const hotkey = field(page, "recording.hotkey");
  await expect(hotkey).toHaveAttribute("aria-invalid", "true");
  await expect(hotkey).toHaveAccessibleDescription(escapeRegExp(en("error.hotkey.unavailable")));
  await expect(hotkey).toHaveValue(view.settings.hotkey);
  await expect(field(page, "recording.mode")).toHaveValue("toggle"); // draft kept

  const stored = await storedView(page);
  expect(stored.settings.hotkey).toBe(view.settings.hotkey);
  expect(stored).toEqual(view);
  expect(await emitted(page, "settings://changed")).toEqual([]);
});

test("a form-level refusal shows its message and keeps the draft", async ({ page }) => {
  // U2: form_error.message is a catalog id the UI renders at form level.
  const view = firstRunView();
  await openSettings(page, view);
  await queueSaveOutcome(page, {
    Refused: {
      errors: [],
      form_error: { kind: "write_failed", message: "settings.write_failed" },
    },
  });
  await tab(page, "history").click();
  await field(page, "history.size").fill("30");
  await page.getByTestId("settings-save").click();

  await onlySaveRequest(page);
  await expect(page.getByRole("alert")).toContainText(en("settings.write_failed"));
  await expect(field(page, "history.size")).toHaveValue("30");
  expect(await storedView(page)).toEqual(view);
});

test("a rejected settings_save shows error.ipc_unavailable, never the rejection text, and keeps the draft", async ({ page }) => {
  // contracts/ipc.md "Errors": a command that cannot run rejects with { code: "ipc.unavailable" }.
  const view = firstRunView();
  await openSettings(page, view);
  await queueSaveRejection(page, { code: "ipc.unavailable", detail: "canary-rejection-text" });
  await tab(page, "history").click();
  await field(page, "history.size").fill("31");
  await page.getByTestId("settings-save").click();

  await onlySaveRequest(page);
  await expect(page.getByRole("alert")).toContainText(en("error.ipc_unavailable"));
  await expect(page.locator("body")).not.toContainText("canary-rejection-text");
  await expect(page.locator("body")).not.toContainText("ipc.unavailable");
  await expect(field(page, "history.size")).toHaveValue("31");
});

// ---- Acceptance 5 (UI half) ---------------------------------------------------------

test("with SettingsView.unavailable the window shows notice.settings_unavailable on open", async ({ page }) => {
  const view = firstRunView();
  view.first_run = false;
  view.unavailable = true;
  await openSettings(page, view);
  await expect(page.getByText(en("notice.settings_unavailable"))).toBeVisible();
  expect(await calls(page, "settings_save")).toEqual([]);
});
