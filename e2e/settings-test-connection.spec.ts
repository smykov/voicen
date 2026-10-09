// T-013 red e2e: "Test connection" for the api and local_server engines (spec 004 US4,
// FR-017; decision #51 UI half; orchestrator choices on the analysis: option A, (i), (ii)).
// Acceptance: valid key -> "OK" with latency, nothing saved; wrong host -> "cannot reach
// <host>"; wrong key -> "invalid API key"; window closed during the test -> result
// discarded. Runs over the one IPC mock (e2e/support/tauriMock.ts); every result is core's
// wire from e2e/fixtures/settings-wire.json (`testConnectionResult`), none hand-written.
// The locator contract of e2e/settings-first-run.spec.ts applies (tab-<tab> test ids,
// data-field controls, role="alert" for form-level messages, settings-save) and the close
// contract of e2e/settings-focus-close.spec.ts (alertdialog, settings-discard-keep /
// settings-discard-confirm, plugin:window|destroy).
//
// Locator contract this spec adds (T-013 analysis, seam):
// - the Engine tab renders a button `data-testid="settings-test-connection"` only for
//   engine api and local_server; its text is `settings.test_connection.button`; while a
//   test runs it is disabled, `aria-busy="true"` and reads `settings.test_connection.running`;
// - the result of the current run is rendered in `data-testid="settings-test-result"` with
//   `role="status"`, through `t` only: ok -> `settings.test_connection.ok {ms}`, the failure
//   kinds -> `failure.*`, invalid -> `settings.test_connection.invalid` plus the field
//   errors on their controls (aria-invalid, `error.<code>` text); a field of an invalid
//   result with no control on the page is listed in the result region by its label
//   (`settings.field_label.<field>`, one listitem each), a field with a control is not;
//   a rejected invoke shows `error.ipc_unavailable` in the result region (never the
//   rejection text, never the page-level role="alert");
// - the run belongs to the page: it survives a tab switch and an engine switch, and it is
//   dropped on every close the page does not prevent and on discard.
import { readFileSync } from "node:fs";
import type { Locator, Page } from "@playwright/test";
import { expect, test } from "./support/boot";
import {
  calls,
  emitted,
  firstRunView,
  holdTests,
  installTauriMock,
  listeners,
  queueTestRejection,
  queueTestResult,
  releaseTest,
  requestClose,
  storedView,
  testConnectionInvalidOffTab,
  testConnectionResult,
  type ConnectionTestRequest,
  type SettingsView,
  type TestResultKind,
} from "./support/tauriMock";

type Messages = Record<string, string>;
const catalog = (lang: "en" | "ru"): Messages =>
  JSON.parse(readFileSync(new URL(`../i18n/${lang}.json`, import.meta.url), "utf8")) as Messages;
const EN = catalog("en");
const RU = catalog("ru");

/** The catalog text of `id`; a missing or empty text fails here, by name. */
function text(messages: Messages, id: string, args: Record<string, string> = {}): string {
  const template = messages[id];
  if (typeof template !== "string" || template === "") throw new Error(`catalog has no text for ${id}`);
  return template.replace(/\{([a-z][a-z0-9_]*)\}/g, (whole, name: string) => args[name] ?? whole);
}

// Obvious fakes only (never a real-looking key).
const FAKE_KEY = "sk-test-FAKE-0013-not-a-real-key";
const FAKE_LOCAL_KEY = "local-FAKE-0013-not-a-real-key";
const REJECTION = "settings_test_connection could not run (fake rejection 0013)";

// The en texts the analysis fixes (Investigation › Details); the failure texts are the
// existing failure.* catalog entries. Asserted as literals so a red is the missing UI.
const OK_EN = "OK, 123 ms";
const RESULT_EN: Record<Exclude<TestResultKind, "ok" | "invalid">, string> = {
  cannot_reach: "Cannot reach 127.0.0.1:1",
  invalid_key: "Invalid API key",
  timeout: "The server did not answer in time",
  http: "Server error (HTTP 500)",
  unexpected_response: "Unexpected response from the server",
  key_store_unavailable: "The API key could not be read from Windows Credential Manager.",
};
const INVALID_EN = "Check the highlighted fields";
const RUNNING_EN = "Testing…";
const BUTTON_EN = "Test connection";

function field(page: Page, id: string): Locator {
  return page.locator(`[data-field="${id}"]`);
}

function testButton(page: Page): Locator {
  return page.getByTestId("settings-test-connection");
}

function result(page: Page): Locator {
  return page.getByTestId("settings-test-result");
}

/** Collects uncaught page errors from before the first navigation. */
function pageErrors(page: Page): Error[] {
  const errors: Error[] = [];
  page.on("pageerror", (error) => errors.push(error));
  return errors;
}

/** Two animation frames: every microtask and render after the last mock answer has run. */
async function settle(page: Page): Promise<void> {
  await page.evaluate(
    () => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))),
  );
}

/** The trimmed text of the result region, "" when it is absent or empty. */
async function resultText(page: Page): Promise<string> {
  return page.evaluate(
    () => document.querySelector('[data-testid="settings-test-result"]')?.textContent?.trim() ?? "",
  );
}

function viewWith(engine: SettingsView["settings"]["engine"], opts: { key?: boolean; lang?: "en" | "ru" } = {}): SettingsView {
  const view = firstRunView();
  view.first_run = false;
  view.settings.engine = engine;
  view.settings.ui_language = opts.lang ?? "en";
  if (opts.key) {
    view.keys.transcription_api = true;
    view.keys.local_server = true;
  }
  return view;
}

async function open(page: Page, view: SettingsView): Promise<void> {
  await installTauriMock(page, { view });
  await page.goto("/settings");
  await expect(page.getByRole("tab", { selected: true })).toBeVisible();
  await expect.poll(() => listeners(page, "settings://changed")).toBeGreaterThan(0);
  await expect.poll(() => listeners(page, "tauri://close-requested")).toBeGreaterThan(0);
  await expect(field(page, "engine.kind")).toHaveValue(view.settings.engine);
}

async function requests(page: Page): Promise<ConnectionTestRequest[]> {
  return (await calls(page, "settings_test_connection")).map((c) => (c.args as { request: ConnectionTestRequest }).request);
}

/** The draft is dirty: a close request is prevented and asks to discard (then kept). */
async function expectDirty(page: Page): Promise<void> {
  await requestClose(page);
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toBeVisible();
  await dialog.getByTestId("settings-discard-keep").click();
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  expect(await calls(page, "plugin:window|destroy")).toEqual([]);
}

// ---- catalog ----------------------------------------------------------------------------

test("the catalogs have settings.test_connection.{button,running,ok,invalid} in en (decided wording) and ru", () => {
  expect(EN["settings.test_connection.button"]).toBe(BUTTON_EN);
  expect(EN["settings.test_connection.running"]).toBe(RUNNING_EN);
  expect(EN["settings.test_connection.ok"]).toBe("OK, {ms} ms");
  expect(EN["settings.test_connection.invalid"]).toBe(INVALID_EN);
  for (const id of ["button", "running", "ok", "invalid"]) {
    const ru = RU[`settings.test_connection.${id}`];
    expect(typeof ru === "string" && ru !== "", `ru has settings.test_connection.${id}`).toBe(true);
  }
  expect(RU["settings.test_connection.ok"]).toContain("{ms}");
});

// ---- (1) where the button is ---------------------------------------------------------

test("the Test connection button is shown for api and local_server and absent for none and builtin_local", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page, viewWith("none"));
  await expect(testButton(page)).toHaveCount(0);

  await field(page, "engine.kind").selectOption("api");
  await expect(testButton(page)).toBeVisible();
  await expect(testButton(page)).toHaveText(BUTTON_EN);
  await expect(testButton(page)).toBeEnabled();

  await field(page, "engine.kind").selectOption("local_server");
  await expect(testButton(page)).toBeVisible();
  await expect(testButton(page)).toBeEnabled();

  await field(page, "engine.kind").selectOption("builtin_local");
  await expect(field(page, "engine.kind")).toHaveValue("builtin_local");
  await expect(testButton(page)).toHaveCount(0);

  await field(page, "engine.kind").selectOption("none");
  await expect(testButton(page)).toHaveCount(0);
  // No test was sent by showing or hiding the button.
  expect(await calls(page, "settings_test_connection")).toEqual([]);
  expect(errors).toEqual([]);
});

// ---- (2) Acceptance: valid key -> "OK" with latency, nothing saved ----------------------

test("valid key -> OK with latency, nothing saved: the request carries the unsaved form values and the typed key", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page, viewWith("none"));
  const before = await storedView(page);

  // Unsaved edits on two tabs: the connect timeout (General) and the api fields (Engine).
  await page.getByTestId("tab-general").click();
  await field(page, "timeouts.connect").fill("7");
  await expect(field(page, "timeouts.connect")).toHaveValue("7");
  await page.getByTestId("tab-engine").click();
  await field(page, "engine.kind").selectOption("api");
  await field(page, "engine.api.base_url").fill("https://api.example.com/v1");
  await field(page, "engine.api.model").fill("whisper-fake-0013");
  await field(page, "engine.api.key").fill(FAKE_KEY);

  await queueTestResult(page, testConnectionResult("ok"));
  await testButton(page).click();

  await expect(result(page)).toHaveText(OK_EN);
  await expect(result(page)).toHaveRole("status");
  const timeouts = { ...firstRunView().settings.timeouts, connect_s: 7 };
  expect(await requests(page)).toEqual([
    {
      engine: "api",
      base_url: "https://api.example.com/v1",
      model: "whisper-fake-0013",
      key: { Replace: FAKE_KEY },
      timeouts,
    },
  ]);
  // Nothing saved: no settings_save, no settings://changed, the stored view unchanged.
  expect(await calls(page, "settings_save")).toEqual([]);
  expect(await emitted(page, "settings://changed")).toEqual([]);
  expect(await storedView(page)).toEqual(before);
  // The draft is untouched by the test: the key stays typed, the edits stay, still dirty.
  await expect(field(page, "engine.api.key")).toHaveValue(FAKE_KEY);
  await expect(field(page, "engine.api.model")).toHaveValue("whisper-fake-0013");
  await expect(testButton(page)).toBeEnabled();
  await expectDirty(page);
  // The key typed for the test is never rendered as text.
  await expect(page.locator("body")).not.toContainText(FAKE_KEY);
  expect(errors).toEqual([]);
});

test("the OK result is rendered in the UI language (ru)", async ({ page }) => {
  await open(page, viewWith("api", { key: true, lang: "ru" }));
  await queueTestResult(page, testConnectionResult("ok"));
  await testButton(page).click();
  const okRu = text(RU, "settings.test_connection.ok", { ms: "123" });
  await expect(result(page)).toHaveText(okRu);
  expect(okRu).toContain("123");
  await expect(page.getByText(OK_EN, { exact: true })).toHaveCount(0);
  await expect(testButton(page)).toHaveText(text(RU, "settings.test_connection.button"));
});

// ---- (3) key and model semantics ------------------------------------------------------

test("local_server without a key or model sends key Untouched and model as empty, with the saved URL and the draft's timeouts", async ({ page }) => {
  await open(page, viewWith("local_server"));
  await queueTestResult(page, testConnectionResult("ok"));
  await testButton(page).click();
  await expect(result(page)).toHaveText(OK_EN);
  const saved = firstRunView().settings;
  expect(await requests(page)).toEqual([
    {
      engine: "local_server",
      base_url: saved.local_server.base_url,
      model: "",
      key: "Untouched",
      timeouts: saved.timeouts,
    },
  ]);
});

test("api with a stored key and an untouched key field sends key Untouched; a key typed and emptied again is Untouched too", async ({ page }) => {
  await open(page, viewWith("api", { key: true }));
  await queueTestResult(page, testConnectionResult("ok"));
  await testButton(page).click();
  await expect(result(page)).toHaveText(OK_EN);

  // Typed, then emptied (resetKey): never a blank Replace.
  await field(page, "engine.api.key").fill(FAKE_KEY);
  await field(page, "engine.api.key").fill("");
  await queueTestResult(page, testConnectionResult("ok"));
  await testButton(page).click();
  await expect.poll(async () => (await requests(page)).length).toBe(2);
  const saved = firstRunView().settings;
  const expected = {
    engine: "api",
    base_url: saved.api.base_url,
    model: saved.api.model,
    key: "Untouched",
    timeouts: saved.timeouts,
  };
  expect(await requests(page)).toEqual([expected, expected]);
});

test("each engine sends its own slot's key: a key typed for local_server is not sent for api, and the reverse", async ({ page }) => {
  await open(page, viewWith("local_server"));
  await field(page, "engine.local_server.key").fill(FAKE_LOCAL_KEY);
  await field(page, "engine.kind").selectOption("api");
  await queueTestResult(page, testConnectionResult("ok"));
  await testButton(page).click();
  await expect(result(page)).toHaveText(OK_EN);

  await field(page, "engine.kind").selectOption("local_server");
  await queueTestResult(page, testConnectionResult("ok"));
  await testButton(page).click();
  await expect.poll(async () => (await requests(page)).length).toBe(2);
  const [api, local] = await requests(page);
  expect(api.engine).toBe("api");
  expect(api.key).toBe("Untouched");
  expect(local.engine).toBe("local_server");
  expect(local.key).toEqual({ Replace: FAKE_LOCAL_KEY });
});

// ---- (4) failure branches: one per kind ------------------------------------------------

for (const [kind, expected] of Object.entries(RESULT_EN) as [keyof typeof RESULT_EN, string][]) {
  test(`failure branch: ${kind} -> "${expected}", nothing saved, no field highlighted`, async ({ page }) => {
    const errors = pageErrors(page);
    await open(page, viewWith("api", { key: true }));
    await queueTestResult(page, testConnectionResult(kind));
    await testButton(page).click();
    await expect(result(page)).toHaveText(expected);
    await expect(result(page)).toHaveRole("status");
    await expect(testButton(page)).toBeEnabled();
    expect(await calls(page, "settings_save")).toEqual([]);
    for (const id of ["engine.api.base_url", "engine.api.model", "engine.api.key"]) {
      await expect(field(page, id)).not.toHaveAttribute("aria-invalid", "true");
    }
    expect(errors).toEqual([]);
  });
}

test("failure branch: invalid -> the base URL is highlighted with error.url.malformed and the result asks to check the fields; nothing saved", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page, viewWith("api", { key: true }));
  await field(page, "engine.api.base_url").fill("not a url (fake)");
  await queueTestResult(page, testConnectionResult("invalid"));
  await testButton(page).click();
  // Exactly the message: the base URL has a control on the page, so it is not listed.
  await expect(result(page)).toHaveText(INVALID_EN);
  await expect(result(page).getByRole("listitem")).toHaveCount(0);
  const url = field(page, "engine.api.base_url");
  await expect(url).toHaveAttribute("aria-invalid", "true");
  await expect(url).toHaveAccessibleDescription(new RegExp(EN["error.url.malformed"].replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  // The typed value is kept (no UI-side rewrite), and nothing was saved.
  await expect(url).toHaveValue("not a url (fake)");
  expect(await calls(page, "settings_save")).toEqual([]);
  expect(await requests(page)).toHaveLength(1);
  expect((await requests(page))[0].base_url).toBe("not a url (fake)");
  expect(errors).toEqual([]);
});

test("failure branch: invalid naming a field on another tab -> the result lists that field by label (only it), and General highlights it", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page, viewWith("api", { key: true }));
  // Both fields as typed: the connect timeout on General, the base URL on Engine.
  await page.getByTestId("tab-general").click();
  await field(page, "timeouts.connect").fill("0");
  await expect(field(page, "timeouts.connect")).toHaveValue("0");
  await page.getByTestId("tab-engine").click();
  await field(page, "engine.api.base_url").fill("not a url (fake)");
  const invalid = testConnectionInvalidOffTab();
  expect((invalid as { errors: { field: string }[] }).errors.map((e) => e.field)).toEqual([
    "engine.api.base_url",
    "timeouts.connect",
  ]);
  await queueTestResult(page, invalid);
  await testButton(page).click();

  // On Engine: the message, then exactly one listed field, the one without a control here.
  const connectLabel = text(EN, "settings.field_label.timeouts.connect");
  const urlLabel = text(EN, "settings.field_label.engine.api.base_url");
  await expect(result(page).getByText(INVALID_EN, { exact: true })).toBeVisible();
  await expect(result(page).getByRole("listitem")).toHaveText([connectLabel]);
  await expect(result(page)).not.toContainText(urlLabel);
  await expect(field(page, "engine.api.base_url")).toHaveAttribute("aria-invalid", "true");
  await expect(field(page, "timeouts.connect")).toHaveCount(0);
  expect((await requests(page))[0].timeouts.connect_s).toBe(0);

  // On General: the listed field carries its own error.
  await page.getByTestId("tab-general").click();
  const connect = field(page, "timeouts.connect");
  await expect(connect).toHaveAttribute("aria-invalid", "true");
  await expect(connect).toHaveAccessibleDescription(new RegExp(EN["error.timeout.range"].replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  await expect(connect).toHaveValue("0");

  // Back on Engine: the same list (the result belongs to the page, not to the tab).
  await page.getByTestId("tab-engine").click();
  await expect(result(page).getByRole("listitem")).toHaveText([connectLabel]);
  expect(await calls(page, "settings_save")).toEqual([]);
  expect(await requests(page)).toHaveLength(1);
  expect(errors).toEqual([]);
});

// ---- (5) one test at a time, owned by the page -----------------------------------------

test("while a test runs the button is disabled and busy, a second click sends nothing, and a tab round trip keeps it running", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page, viewWith("api", { key: true }));
  await holdTests(page);
  await queueTestResult(page, testConnectionResult("ok"));
  await testButton(page).click();

  await expect(testButton(page)).toBeDisabled();
  await expect(testButton(page)).toHaveAttribute("aria-busy", "true");
  await expect(testButton(page)).toHaveText(RUNNING_EN);
  await testButton(page).click({ force: true });
  await settle(page);
  expect(await calls(page, "settings_test_connection")).toHaveLength(1);

  // The Engine tab is unmounted and mounted again: the run is still the page's.
  await page.getByTestId("tab-general").click();
  await expect(testButton(page)).toHaveCount(0);
  await page.getByTestId("tab-engine").click();
  await expect(testButton(page)).toBeDisabled();
  await expect(testButton(page)).toHaveAttribute("aria-busy", "true");
  await testButton(page).click({ force: true });
  await settle(page);
  expect(await calls(page, "settings_test_connection")).toHaveLength(1);

  await releaseTest(page);
  await expect(result(page)).toHaveText(OK_EN);
  await expect(testButton(page)).toBeEnabled();
  await expect(testButton(page)).not.toHaveAttribute("aria-busy", "true");
  await expect(testButton(page)).toHaveText(BUTTON_EN);
  expect(errors).toEqual([]);
});

test("a result that arrives while another tab is shown is there when the Engine tab is shown again", async ({ page }) => {
  await open(page, viewWith("api", { key: true }));
  await holdTests(page);
  await queueTestResult(page, testConnectionResult("cannot_reach"));
  await testButton(page).click();
  await expect(testButton(page)).toBeDisabled();
  await page.getByTestId("tab-general").click();
  await expect(testButton(page)).toHaveCount(0);
  await releaseTest(page);
  await settle(page);
  await page.getByTestId("tab-engine").click();
  await expect(result(page)).toHaveText(RESULT_EN.cannot_reach);
  await expect(testButton(page)).toBeEnabled();
  expect(await calls(page, "settings_test_connection")).toHaveLength(1);
});

// ---- (6) Acceptance: window closed during the test -> result discarded ------------------

test("failure branch: closing a dirty window during a test and discarding drops the result: no text, no highlight", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page, viewWith("api", { key: true }));
  await field(page, "engine.api.base_url").fill("not a url (fake)");
  await holdTests(page);
  await queueTestResult(page, testConnectionResult("invalid"));
  await testButton(page).click();
  await expect(testButton(page)).toBeDisabled();

  await requestClose(page);
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toBeVisible();
  await dialog.getByTestId("settings-discard-confirm").click();
  await expect.poll(async () => (await calls(page, "plugin:window|destroy")).length).toBe(1);

  await releaseTest(page);
  await settle(page);
  expect(await resultText(page)).toBe("");
  await expect(page.getByText(INVALID_EN)).toHaveCount(0);
  await expect(field(page, "engine.api.base_url")).not.toHaveAttribute("aria-invalid", "true");
  await expect(page.getByText(EN["error.url.malformed"])).toHaveCount(0);
  expect(await calls(page, "settings_save")).toEqual([]);
  expect(errors).toEqual([]);
});

test("failure branch: a clean window closed during a test (not prevented, destroyed) drops the result", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page, viewWith("api", { key: true }));
  await holdTests(page);
  await queueTestResult(page, testConnectionResult("ok"));
  await testButton(page).click();
  await expect(testButton(page)).toBeDisabled();

  await requestClose(page);
  await expect.poll(async () => (await calls(page, "plugin:window|destroy")).length).toBe(1);
  await expect(page.getByRole("alertdialog")).toHaveCount(0);

  await releaseTest(page);
  await settle(page);
  expect(await resultText(page)).toBe("");
  await expect(page.getByText(OK_EN)).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("keep editing during a test keeps the run: its result is shown when it arrives", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page, viewWith("api", { key: true }));
  await field(page, "engine.api.model").fill("whisper-fake-edited");
  await holdTests(page);
  await queueTestResult(page, testConnectionResult("ok"));
  await testButton(page).click();
  await expect(testButton(page)).toBeDisabled();

  await requestClose(page);
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toBeVisible();
  await dialog.getByTestId("settings-discard-keep").click();
  await expect(page.getByRole("alertdialog")).toHaveCount(0);

  await releaseTest(page);
  await expect(result(page)).toHaveText(OK_EN);
  await expect(testButton(page)).toBeEnabled();
  expect(await calls(page, "plugin:window|destroy")).toEqual([]);
  expect(errors).toEqual([]);
});

// ---- (7) a rejected invoke ---------------------------------------------------------------

test("failure branch: a rejected test shows error.ipc_unavailable, never the rejection text, and the button is enabled again", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page, viewWith("api", { key: true }));
  await field(page, "engine.api.key").fill(FAKE_KEY);
  await queueTestRejection(page, REJECTION);
  await testButton(page).click();

  // In the result region, not as the page-level alert.
  await expect(result(page)).toHaveText(EN["error.ipc_unavailable"]);
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(page.getByText(EN["error.ipc_unavailable"])).toHaveCount(1);
  await expect(page.locator("body")).not.toContainText(REJECTION);
  await expect(page.locator("body")).not.toContainText(OK_EN);
  await expect(testButton(page)).toBeEnabled();
  // The draft is kept: the key is still typed, nothing was saved.
  await expect(field(page, "engine.api.key")).toHaveValue(FAKE_KEY);
  expect(await calls(page, "settings_save")).toEqual([]);

  // The button works again: a second test runs and shows its result.
  await queueTestResult(page, testConnectionResult("ok"));
  await testButton(page).click();
  await expect(result(page)).toHaveText(OK_EN);
  expect(await calls(page, "settings_test_connection")).toHaveLength(2);
  expect(errors).toEqual([]);
});

test("failure branch: a rejection that arrives after the window was discarded shows nothing", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page, viewWith("api", { key: true }));
  await field(page, "engine.api.model").fill("whisper-fake-dirty");
  await holdTests(page);
  await queueTestRejection(page, REJECTION);
  await testButton(page).click();
  await expect(testButton(page)).toBeDisabled();

  await requestClose(page);
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toBeVisible();
  await dialog.getByTestId("settings-discard-confirm").click();
  await expect.poll(async () => (await calls(page, "plugin:window|destroy")).length).toBe(1);

  await releaseTest(page);
  await settle(page);
  expect(await resultText(page)).toBe("");
  await expect(page.getByText(EN["error.ipc_unavailable"])).toHaveCount(0);
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(page.locator("body")).not.toContainText(REJECTION);
  expect(errors).toEqual([]);
});

// ---- (8) the result refers to the values at the click -----------------------------------

test("a result that arrives after a draft edit is still shown, the edit is kept, and the request had the values at the click", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page, viewWith("api", { key: true }));
  await holdTests(page);
  await queueTestResult(page, testConnectionResult("ok"));
  await testButton(page).click();
  await expect(testButton(page)).toBeDisabled();

  // The button is disabled, the fields are not: the user keeps editing.
  await field(page, "engine.api.model").fill("whisper-fake-after-click");
  await releaseTest(page);
  await expect(result(page)).toHaveText(OK_EN);
  await expect(field(page, "engine.api.model")).toHaveValue("whisper-fake-after-click");
  expect((await requests(page))[0].model).toBe(firstRunView().settings.api.model);
  expect(errors).toEqual([]);
});

test("a new click clears the previous result; switching the engine keeps the shown result until the next click", async ({ page }) => {
  await open(page, viewWith("api", { key: true }));
  await queueTestResult(page, testConnectionResult("ok"));
  await testButton(page).click();
  await expect(result(page)).toHaveText(OK_EN);

  // Choice (ii): the result stays after an engine switch, until the next click.
  await field(page, "engine.kind").selectOption("local_server");
  await expect(result(page)).toHaveText(OK_EN);

  await holdTests(page);
  await queueTestResult(page, testConnectionResult("timeout"));
  await testButton(page).click();
  await expect(testButton(page)).toBeDisabled();
  await expect(page.getByText(OK_EN)).toHaveCount(0);
  await releaseTest(page);
  await expect(result(page)).toHaveText(RESULT_EN.timeout);
  expect((await requests(page)).map((r) => r.engine)).toEqual(["api", "local_server"]);
});
