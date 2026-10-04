// T-039 red e2e: the settings page focuses a field on request (F), asks before a dirty
// draft is discarded on close (D), shows every field from the view and sends it back
// unchanged (R), and names a not-restored field it does not render by its label (L).
// Runs over the one IPC mock (e2e/support/tauriMock.ts); the locator contract of
// e2e/settings-first-run.spec.ts applies (tab-<tab> test ids, data-field controls,
// role="alert" for form-level messages, settings-save).
//
// Locator contract this spec adds (T-039 analysis):
// - focus request: `?tab=<tab>&field=<FieldId>` on load and `settings://focus
//   { tab, field? }` on an open page select tab `tabOf(tab)` (the requested tab if the
//   page has it, else Engine) and focus the control whose `data-field` equals `field`
//   exactly; no such control -> nothing focused, nothing shown;
// - close: the page listens to `tauri://close-requested`; with a dirty draft it shows an
//   in-page `role="alertdialog"` (with an accessible name) whose buttons have test ids
//   `settings-discard-keep` ("keep editing") and `settings-discard-confirm` ("discard");
//   the window closes only through `plugin:window|destroy { label: "settings" }`;
// - labels: a field of `not_restored` with no control on the page is named in the
//   form-level message by the text of `settings.field_label.<FieldId>`.
import { readFileSync } from "node:fs";
import { expect, test, type Page } from "@playwright/test";
import {
  calls,
  emit,
  firstRunView,
  installTauriMock,
  listeners,
  queueSaveOutcome,
  releaseSettingsGet,
  requestClose,
  type MockOptions,
  type SaveRequest,
  type Settings,
  type SettingsView,
} from "./support/tauriMock";

const EN = JSON.parse(readFileSync(new URL("../i18n/en.json", import.meta.url), "utf8")) as Record<string, string>;

/** The English catalog text of `id`; a missing or empty text fails here, by name. */
function en(id: string): string {
  const text = EN[id];
  if (typeof text !== "string" || text === "") throw new Error(`i18n/en.json has no text for ${id}`);
  return text;
}

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

async function open(
  page: Page,
  path: string,
  view: SettingsView = firstRunView(),
  options: Omit<MockOptions, "view"> = {},
): Promise<void> {
  await installTauriMock(page, { ...options, view });
  await page.goto(path);
  await expect(page.getByRole("tab", { selected: true })).toBeVisible();
}

/** Two animation frames: pending Svelte updates and effects have run (for "nothing happened" checks). */
async function settle(page: Page): Promise<void> {
  await page.evaluate(
    () => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))),
  );
}

/** The `data-field` of the focused element, or null when no field control has focus. */
async function focusedField(page: Page): Promise<string | null> {
  return page.evaluate(() => (document.activeElement as HTMLElement | null)?.dataset?.field ?? null);
}

async function waitForListener(page: Page, event: string): Promise<void> {
  await expect
    .poll(() => listeners(page, event), { message: `the settings page listens to ${event}` })
    .toBeGreaterThan(0);
}

/** A view with engine api (so `engine.api.key` has a control, on the Engine tab). */
function apiView(): SettingsView {
  const view = firstRunView();
  view.first_run = false;
  view.settings.engine = "api";
  return view;
}

// ---- (F) Focus a field on request -----------------------------------------------------

test("?tab=recording&field=recording.hotkey opens the Recording tab with the hotkey control focused", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page, "/settings?tab=recording&field=recording.hotkey");

  await expect(tab(page, "recording")).toHaveAttribute("aria-selected", "true");
  await expect(field(page, "recording.hotkey")).toBeFocused();
  expect(errors).toEqual([]);
});

test("a settings://focus event on an open page selects its tab and focuses its field", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page, "/settings");
  await expect(tab(page, "engine")).toHaveAttribute("aria-selected", "true");
  await waitForListener(page, "settings://focus");

  await emit(page, "settings://focus", { tab: "history", field: "history.size" });
  await expect(tab(page, "history")).toHaveAttribute("aria-selected", "true");
  await expect(field(page, "history.size")).toBeFocused();

  // A second request on the same page moves tab and focus again.
  await emit(page, "settings://focus", { tab: "recording", field: "recording.hotkey" });
  await expect(tab(page, "recording")).toHaveAttribute("aria-selected", "true");
  await expect(field(page, "recording.hotkey")).toBeFocused();

  expect(await calls(page, "settings_save")).toEqual([]);
  expect(errors).toEqual([]);
});

test("failure branch: a field that names no control on the requested tab focuses nothing, shows no error, and the requested tab stays selected", async ({ page }) => {
  // engine.api.key has a control, but on the Engine tab: the UI must not derive the tab
  // from the FieldId (one rule, the shell's tab).
  const errors = pageErrors(page);
  await open(page, "/settings?tab=output&field=engine.api.key", apiView());

  await expect(tab(page, "output")).toHaveAttribute("aria-selected", "true");
  await expect(field(page, "output.auto_paste")).toBeVisible();
  await settle(page);
  await expect(tab(page, "output")).toHaveAttribute("aria-selected", "true");
  expect(await focusedField(page)).toBeNull();
  await expect(page.getByRole("alert")).toHaveCount(0);

  // The same request as an event on the open page.
  await waitForListener(page, "settings://focus");
  await emit(page, "settings://focus", { tab: "output", field: "engine.api.key" });
  await settle(page);
  await expect(tab(page, "output")).toHaveAttribute("aria-selected", "true");
  expect(await focusedField(page)).toBeNull();
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("failure branch: a malformed field (selector syntax) raises no page error and focuses nothing", async ({ page }) => {
  // `x"]` would break a selector built from the field; the page compares dataset.field.
  const errors = pageErrors(page);
  await open(page, "/settings?tab=general&field=x%22%5D");

  await expect(tab(page, "general")).toHaveAttribute("aria-selected", "true");
  await expect(field(page, "general.ui_language")).toBeVisible();
  await settle(page);
  expect(await focusedField(page)).toBeNull();

  await waitForListener(page, "settings://focus");
  for (const bad of ['x"]', "'][", "[data-field]", "", "history.size\\"]) {
    await emit(page, "settings://focus", { tab: "history", field: bad });
  }
  await expect(tab(page, "history")).toHaveAttribute("aria-selected", "true");
  await settle(page);
  expect(await focusedField(page)).toBeNull();
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("tab post_processing (no page tab yet) selects Engine, from the URL and from settings://focus", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page, "/settings?tab=post_processing&field=post_processing.prompt");
  await expect(tab(page, "engine")).toHaveAttribute("aria-selected", "true");
  await settle(page);
  expect(await focusedField(page)).toBeNull();

  await tab(page, "recording").click();
  await expect(tab(page, "recording")).toHaveAttribute("aria-selected", "true");
  await waitForListener(page, "settings://focus");
  // `field` is optional in the event.
  await emit(page, "settings://focus", { tab: "post_processing" });
  await expect(tab(page, "engine")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("a settings://focus sent while settings_get is still pending is honoured once the draft loads, and replaces the URL's request", async ({ page }) => {
  // T-039 r1 #4a: settings_window::open emits settings://focus instead of loading a URL
  // whenever the window exists, including while its page is still loading
  // (src-tauri/src/settings_window.rs). The page must listen before the draft arrives.
  const errors = pageErrors(page);
  await installTauriMock(page, { holdSettingsGet: true });
  await page.goto("/settings?tab=recording&field=recording.hotkey");
  await expect.poll(async () => (await calls(page, "settings_get")).length).toBe(1);
  await waitForListener(page, "settings://focus");
  // Still loading: no draft, so no tabs and no controls yet.
  await expect(page.getByRole("tab")).toHaveCount(0);

  await emit(page, "settings://focus", { tab: "history", field: "history.size" });
  await releaseSettingsGet(page);

  await expect(tab(page, "history")).toHaveAttribute("aria-selected", "true");
  await expect(field(page, "history.size")).toBeFocused();
  expect(errors).toEqual([]);
});

// ---- (D) Discard prompt on close --------------------------------------------------------

/** Opens the page, waits for the close listener, and makes the draft dirty (history size 33). */
async function openDirty(page: Page, options: Omit<MockOptions, "view"> = {}): Promise<void> {
  await open(page, "/settings", firstRunView(), options);
  await waitForListener(page, "tauri://close-requested");
  await tab(page, "history").click();
  await field(page, "history.size").fill("33");
  await expect(field(page, "history.size")).toHaveValue("33");
}

test("closing with a dirty draft asks to discard and does not destroy the window", async ({ page }) => {
  const errors = pageErrors(page);
  await openDirty(page);

  await requestClose(page);
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toBeVisible();
  await expect(dialog).toHaveAccessibleName(/\S/);
  await expect(dialog.getByTestId("settings-discard-keep")).toBeVisible();
  await expect(dialog.getByTestId("settings-discard-confirm")).toBeVisible();
  await settle(page);
  expect(await calls(page, "plugin:window|destroy")).toEqual([]);
  expect(await calls(page, "settings_save")).toEqual([]);
  expect(errors).toEqual([]);
});

test("keep editing closes the dialog, keeps the window and the edited value", async ({ page }) => {
  const errors = pageErrors(page);
  await openDirty(page);
  await requestClose(page);
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toBeVisible();

  await dialog.getByTestId("settings-discard-keep").click();
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  await expect(field(page, "history.size")).toHaveValue("33");
  await settle(page);
  expect(await calls(page, "plugin:window|destroy")).toEqual([]);
  expect(await calls(page, "settings_save")).toEqual([]);

  // Still dirty: the next close asks again.
  await requestClose(page);
  await expect(page.getByRole("alertdialog")).toBeVisible();
  await settle(page);
  expect(await calls(page, "plugin:window|destroy")).toEqual([]);

  // The kept edit is still what Save would send.
  await page.getByRole("alertdialog").getByTestId("settings-discard-keep").click();
  await page.getByTestId("settings-save").click();
  await expect.poll(async () => (await calls(page, "settings_save")).length).toBe(1);
  const [save] = await calls(page, "settings_save");
  expect((save.args as { request: SaveRequest }).request.settings.history.size).toBe(33);
  expect(errors).toEqual([]);
});

test("discard destroys the window exactly once, without saving", async ({ page }) => {
  const errors = pageErrors(page);
  await openDirty(page);
  await requestClose(page);
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toBeVisible();

  await dialog.getByTestId("settings-discard-confirm").click();
  await expect.poll(async () => (await calls(page, "plugin:window|destroy")).length).toBe(1);
  await settle(page);
  expect(await calls(page, "plugin:window|destroy")).toEqual([
    { cmd: "plugin:window|destroy", args: { label: "settings" } },
  ]);
  // Never close(): it would emit close-requested again and is not granted.
  expect(await calls(page, "plugin:window|close")).toEqual([]);
  expect(await calls(page, "settings_save")).toEqual([]);
  expect(errors).toEqual([]);
});

test("a clean draft closes without asking: one destroy, no dialog", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page, "/settings");
  await expect(field(page, "engine.kind")).toBeVisible();
  await waitForListener(page, "tauri://close-requested");

  await requestClose(page);
  await expect.poll(async () => (await calls(page, "plugin:window|destroy")).length).toBe(1);
  await settle(page);
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  expect(await calls(page, "plugin:window|destroy")).toEqual([
    { cmd: "plugin:window|destroy", args: { label: "settings" } },
  ]);
  expect(await calls(page, "plugin:window|close")).toEqual([]);
  expect(errors).toEqual([]);
});

test("a draft edited back to its saved value is clean and closes without asking", async ({ page }) => {
  const errors = pageErrors(page);
  const view = firstRunView();
  await openDirty(page);
  await field(page, "history.size").fill(String(view.settings.history.size));

  await requestClose(page);
  await expect.poll(async () => (await calls(page, "plugin:window|destroy")).length).toBe(1);
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("failure branch: discard with a failing destroy closes the dialog, shows error.ipc_unavailable, keeps the draft, and a later close asks again", async ({ page }) => {
  // T-039 r1 #3 (settings-ui.md D): the window cannot do better than report and stay.
  const errors = pageErrors(page);
  await openDirty(page, { destroy: { reject: "destroy failed (fake)" } });
  await requestClose(page);
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toBeVisible();

  await dialog.getByTestId("settings-discard-confirm").click();
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  const alert = page.getByRole("alert");
  await expect(alert).toContainText(en("error.ipc_unavailable"));
  // The rejection text is never shown.
  await expect(alert).not.toContainText("destroy failed (fake)");
  expect(await calls(page, "plugin:window|destroy")).toEqual([
    { cmd: "plugin:window|destroy", args: { label: "settings" } },
  ]);
  // The draft is kept and still editable (the page is not left inert).
  await expect(field(page, "history.size")).toHaveValue("33");
  await field(page, "history.size").fill("34");
  await expect(field(page, "history.size")).toHaveValue("34");
  expect(await calls(page, "settings_save")).toEqual([]);

  // Still dirty: a later close asks again instead of closing.
  await requestClose(page);
  await expect(page.getByRole("alertdialog")).toBeVisible();
  await settle(page);
  expect(await calls(page, "plugin:window|destroy")).toHaveLength(1);
  expect(errors).toEqual([]);
});

test("Escape in the discard dialog keeps editing: the dialog closes, the edit is kept, nothing is destroyed", async ({ page }) => {
  const errors = pageErrors(page);
  await openDirty(page);
  await requestClose(page);
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toBeVisible();
  await expect(dialog.getByTestId("settings-discard-keep")).toBeFocused();

  await page.keyboard.press("Escape");
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  await expect(field(page, "history.size")).toHaveValue("33");
  await settle(page);
  expect(await calls(page, "plugin:window|destroy")).toEqual([]);

  // The page is usable again and the kept edit is what Save sends.
  await page.getByTestId("settings-save").click();
  await expect.poll(async () => (await calls(page, "settings_save")).length).toBe(1);
  const [save] = await calls(page, "settings_save");
  expect((save.args as { request: SaveRequest }).request.settings.history.size).toBe(33);
  expect(await calls(page, "plugin:window|destroy")).toEqual([]);
  expect(errors).toEqual([]);
});

test("failure branch: a rejected settings://focus listen never leaves an editable draft without the close guard", async ({ page }) => {
  // T-039 r1 #4b: the close guard is the data-loss path, so a failure of the (optional)
  // focus listener must not skip it. Either the draft does not load at all, or the
  // close-requested listener exists whenever a control can be edited.
  const errors = pageErrors(page);
  await installTauriMock(page, { rejectListen: ["settings://focus"] });
  await page.goto("/settings");
  // The listen failure is reported; once shown, the page's listener setup has finished.
  await expect(page.getByRole("alert")).toContainText(en("error.ipc_unavailable"));
  await settle(page);

  const controls = await page.locator("[data-field]").count();
  const guards = await listeners(page, "tauri://close-requested");
  expect(
    controls === 0 || guards > 0,
    `${controls} controls rendered with ${guards} tauri://close-requested listeners: an editable draft must be close-guarded`,
  ).toBe(true);
  expect(errors).toEqual([]);
});

// ---- (R) Every field rendered from the view and sent back unchanged ---------------------

/**
 * Every leaf differs from core's defaults (schema_version excepted: there is one), so a
 * control that rewrites its value on mount or on a tab switch changes the request.
 * Typed `Settings` and written out in full (no spread): a new Settings field fails to
 * compile here; the key shape is also checked at runtime against core's fixture.
 */
const NON_DEFAULT: Settings = {
  schema_version: 1,
  engine: "api",
  api: { base_url: "https://stt.example.com/v2", model: "whisper-fake-rt" },
  local_server: { base_url: "http://192.0.2.10:9000/v1", model: "local-fake-rt" },
  builtin_local: { model_id: "ggml-fake-small" },
  speech_language: "de",
  microphone: { id: "mic-fake-0001", name: "Fake USB Microphone" },
  hotkey: "Ctrl+Shift+F9",
  mode: "toggle",
  auto_paste: false,
  post_processing: {
    enabled: true,
    base_url: "https://llm.example.com/v1",
    model: "llm-fake-rt",
    prompt: "Fake round-trip prompt: fix punctuation only.",
  },
  history: { enabled: false, size: 42 },
  start_with_windows: true,
  ui_language: "ru",
};

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

function isObject(v: unknown): v is Record<string, Json> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** Key differences between `mine` and core's `def` (a null default accepts any value). */
function shapeDiff(def: unknown, mine: unknown, path = ""): string[] {
  if (!isObject(def)) return [];
  if (!isObject(mine)) return [`${path || "<root>"}: not an object`];
  const out: string[] = [];
  for (const k of Object.keys(def)) {
    const p = path ? `${path}.${k}` : k;
    if (!Object.hasOwn(mine, k)) out.push(`${p}: missing`);
    else out.push(...shapeDiff(def[k], mine[k], p));
  }
  for (const k of Object.keys(mine)) if (!Object.hasOwn(def, k)) out.push(`${path ? `${path}.` : ""}${k}: extra`);
  return out;
}

/** Leaf paths of `def` (a null counts as a leaf) whose value `mine` keeps unchanged. */
function unchangedLeaves(def: unknown, mine: unknown, path = ""): string[] {
  if (isObject(def) && isObject(mine)) {
    return Object.keys(def).flatMap((k) => unchangedLeaves(def[k], mine[k], path ? `${path}.${k}` : k));
  }
  return JSON.stringify(def) === JSON.stringify(mine) ? [path] : [];
}

/**
 * NON_DEFAULT with engine local_server (T-039 r1 #2), so the Engine tab renders the
 * engine.local_server.* controls. The spread is safe here: NON_DEFAULT itself is the
 * full typed literal, and the runtime shape and non-default checks run on this one too.
 */
const NON_DEFAULT_LOCAL_SERVER: Settings = { ...structuredClone(NON_DEFAULT), engine: "local_server" };

/**
 * What the control of each FieldId shows for `settings`: the wire value (a string for
 * inputs and selects, a boolean for checkboxes); key inputs are always empty (U3). A
 * control the page renders that is not listed here fails the test: add it here.
 */
function shownFor(settings: Settings): Record<string, string | boolean> {
  return {
    "engine.kind": settings.engine,
    "engine.api.base_url": settings.api.base_url,
    "engine.api.model": settings.api.model,
    "engine.api.key": "",
    "engine.local_server.base_url": settings.local_server.base_url,
    "engine.local_server.model": settings.local_server.model,
    "engine.local_server.key": "",
    "engine.speech_language": settings.speech_language ?? "auto",
    "recording.hotkey": settings.hotkey,
    "recording.mode": settings.mode,
    "output.auto_paste": settings.auto_paste,
    "history.enabled": settings.history.enabled,
    "history.size": String(settings.history.size),
    "general.ui_language": settings.ui_language,
  };
}

/** The controls each release-1 tab must render for engine api or local_server. */
function tabFieldsFor(engine: "api" | "local_server"): Record<string, string[]> {
  return {
    engine: [
      "engine.kind",
      `engine.${engine}.base_url`,
      `engine.${engine}.model`,
      `engine.${engine}.key`,
      "engine.speech_language",
    ],
    recording: ["recording.hotkey", "recording.mode"],
    output: ["output.auto_paste"],
    history: ["history.enabled", "history.size"],
    general: ["general.ui_language"],
  };
}

async function expectTabShowsView(
  page: Page,
  name: string,
  tabFields: Record<string, string[]>,
  shown: Record<string, string | boolean>,
): Promise<void> {
  await tab(page, name).click();
  await expect(tab(page, name)).toHaveAttribute("aria-selected", "true");
  for (const id of tabFields[name]) await expect(field(page, id), `${name}: ${id} rendered`).toHaveCount(1);
  const rendered = await page
    .locator("[data-field]")
    .evaluateAll((els) => els.map((e) => (e as HTMLElement).dataset.field ?? ""));
  for (const id of rendered) {
    expect(Object.hasOwn(shown, id), `${name}: control ${id} is rendered but this test does not know its value`).toBe(true);
    const value = shown[id];
    if (typeof value === "boolean") await expect(field(page, id), `${name}: ${id}`).toBeChecked({ checked: value });
    else await expect(field(page, id), `${name}: ${id}`).toHaveValue(value);
  }
}

/** Acceptance 3 for `settings` (engine api or local_server): visit every tab, then an untouched Save. */
async function expectRoundTrip(page: Page, settings: Settings, engine: "api" | "local_server"): Promise<void> {
  const errors = pageErrors(page);
  expect(settings.engine).toBe(engine);
  const core = firstRunView().settings;
  expect(shapeDiff(core, settings), "the literal has core's Settings keys").toEqual([]);
  expect(unchangedLeaves(core, settings), "every leaf but schema_version differs from core's defaults").toEqual([
    "schema_version",
  ]);

  const view: SettingsView = {
    settings,
    keys: { transcription_api: true, local_server: true, post_processing: true },
    first_run: false,
    reset_notice: false,
    unavailable: false,
  };
  await open(page, "/settings", view);

  const tabFields = tabFieldsFor(engine);
  const shown = shownFor(settings);
  for (const name of ["engine", "recording", "output", "history", "general", "engine"]) {
    await expectTabShowsView(page, name, tabFields, shown);
  }

  await page.getByTestId("settings-save").click();
  await expect.poll(async () => (await calls(page, "settings_save")).length).toBe(1);
  const [save] = await calls(page, "settings_save");
  const request = (save.args as { request: SaveRequest }).request;
  expect(request.settings).toEqual(view.settings);
  expect(request.keys).toEqual({ transcription_api: "Untouched", local_server: "Untouched", post_processing: "Untouched" });
  expect(errors).toEqual([]);
}

test("every field of the release-1 tabs is rendered from the view and an untouched Save sends the view's settings unchanged with every key Untouched", async ({ page }) => {
  await expectRoundTrip(page, NON_DEFAULT, "api");
});

test("engine local_server: the engine.local_server base URL, model and key controls show the view and an untouched Save sends the view's settings unchanged with every key Untouched", async ({ page }) => {
  await expectRoundTrip(page, NON_DEFAULT_LOCAL_SERVER, "local_server");
});

// ---- (L) A not-restored field without a control is named by its label --------------------

test("a partially_restored refusal naming engine.api.key while engine is local_server lists it by its label text in the form-level message", async ({ page }) => {
  const errors = pageErrors(page);
  const view = firstRunView();
  view.first_run = false;
  view.settings.engine = "local_server";
  view.keys.transcription_api = true;
  await open(page, "/settings", view);
  await expect(field(page, "engine.local_server.base_url")).toBeVisible();
  await queueSaveOutcome(page, {
    Refused: {
      errors: [{ field: "engine.local_server.key", code: "key.store_failed" }],
      form_error: {
        kind: "partially_restored",
        message: "settings.partially_restored",
        // Neither has a control on this page: the API key (engine local_server) and
        // start-with-Windows (no control until T-034).
        not_restored: ["engine.api.key", "general.start_with_windows"],
      },
    },
  });

  await page.getByTestId("settings-save").click();
  const alert = page.getByRole("alert");
  await expect(alert).toContainText(en("settings.partially_restored"));
  await expect(field(page, "engine.api.key")).toHaveCount(0);
  await expect(alert).toContainText(en("settings.field_label.engine.api.key"));
  await expect(alert).toContainText(en("settings.field_label.general.start_with_windows"));
  // By label, never by the raw FieldId.
  await expect(alert).not.toContainText("engine.api.key");
  await expect(alert).not.toContainText("general.start_with_windows");
  expect(errors).toEqual([]);
});

test("a partially_restored refusal lists by label only the not_restored fields with no control on the page, and the list follows the controls the current tab renders", async ({ page }) => {
  // T-039 r1 #1 (invariant L): engine.api.key has a control on the Engine tab (engine
  // api), general.start_with_windows has none anywhere (until T-034).
  const errors = pageErrors(page);
  const view = apiView();
  view.keys.transcription_api = true;
  await open(page, "/settings", view);
  await expect(field(page, "engine.api.key")).toBeVisible();
  await queueSaveOutcome(page, {
    Refused: {
      errors: [{ field: "engine.local_server.key", code: "key.store_failed" }],
      form_error: {
        kind: "partially_restored",
        message: "settings.partially_restored",
        not_restored: ["engine.api.key", "general.start_with_windows"],
      },
    },
  });

  await page.getByTestId("settings-save").click();
  const alert = page.getByRole("alert");
  await expect(alert).toContainText(en("settings.partially_restored"));
  const listed = alert.getByRole("listitem");
  const apiKeyLabel = en("settings.field_label.engine.api.key");
  const startLabel = en("settings.field_label.general.start_with_windows");

  // Engine tab: the API key has its control, so only start-with-Windows is listed; the
  // control keeps its field-level highlight.
  await expect(listed).toHaveText([startLabel]);
  await expect(alert).not.toContainText(apiKeyLabel);
  await expect(field(page, "engine.api.key")).toHaveAttribute("aria-invalid", "true");

  // Recording tab: the API key control is gone, so its label joins the list.
  await tab(page, "recording").click();
  await expect(field(page, "engine.api.key")).toHaveCount(0);
  await expect(listed).toHaveText([apiKeyLabel, startLabel]);

  // Back on Engine: the control is rendered again and leaves the list.
  await tab(page, "engine").click();
  await expect(field(page, "engine.api.key")).toHaveAttribute("aria-invalid", "true");
  await expect(listed).toHaveText([startLabel]);

  // Same tab, another engine: the API key control is unmounted, so it is listed again.
  await field(page, "engine.kind").selectOption("local_server");
  await expect(field(page, "engine.api.key")).toHaveCount(0);
  await expect(listed).toHaveText([apiKeyLabel, startLabel]);
  expect(errors).toEqual([]);
});
