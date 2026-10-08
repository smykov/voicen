// T-073 red e2e: the five dictation timeouts are settings (decisions #97, #99; FR-24).
// Over the one IPC mock (e2e/support/tauriMock.ts). The locator contract of
// e2e/settings-first-run.spec.ts applies (tab-<tab> test ids, data-field controls,
// aria-invalid + aria-describedby for a refused field, role="alert" for form-level
// messages, settings-save, role="status" for Saved).
//
// Locator contract this spec adds (T-073 analysis, decision #99 Q2):
// - the General tab holds one group (role "group", e.g. <fieldset> + <legend>) named
//   "Timeouts (seconds)" / "Таймауты (секунды)" with exactly five controls, always shown
//   whatever the engine:
//   `<input type="number" data-field="timeouts.<role>">` for role in connect,
//   api_transcription, local_server, post_processing, builtin_local, bound to
//   `settings.timeouts.<role>_s` (whole seconds on the wire, core's TimeoutSettings);
// - each control is labelled with the analysis's en/ru label and has its range hint
//   `settings.timeouts.<role>.hint` visible and in its accessible description;
// - the range is core's rule only (`timeout.range`): the input sends what was typed as a
//   whole number, a non-whole or empty entry as 0 (History.svelte's entry rule), and a
//   Refused `timeouts.<role>: timeout.range` is shown on that control (U2).
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

type Lang = "en" | "ru";
type Messages = Record<string, string>;
const CATALOG: Record<Lang, Messages> = {
  en: JSON.parse(readFileSync(new URL("../i18n/en.json", import.meta.url), "utf8")) as Messages,
  ru: JSON.parse(readFileSync(new URL("../i18n/ru.json", import.meta.url), "utf8")) as Messages,
};

/** The catalog text of `id` in `lang`; a missing or empty text fails here, by name. */
function text(lang: Lang, id: string): string {
  const value = CATALOG[lang][id];
  if (typeof value !== "string" || value === "") throw new Error(`i18n/${lang}.json has no text for ${id}`);
  return value;
}

function escapeRegExp(s: string): RegExp {
  return new RegExp(s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
}

/** Core's wire shape (settings::TimeoutSettings), whole seconds. */
interface TimeoutsWire {
  connect_s: number;
  api_transcription_s: number;
  local_server_s: number;
  post_processing_s: number;
  builtin_local_s: number;
}
type Role = "connect" | "api_transcription" | "local_server" | "post_processing" | "builtin_local";

// Decision #99 Q3 bounds and the analysis's labels (T-073 ## Investigation, UI).
const ROLES: readonly { role: Role; key: keyof TimeoutsWire; def: number; min: number; max: number; label: Record<Lang, string> }[] = [
  { role: "connect", key: "connect_s", def: 5, min: 1, max: 60, label: { en: "Connect timeout", ru: "Таймаут подключения" } },
  { role: "api_transcription", key: "api_transcription_s", def: 30, min: 5, max: 600, label: { en: "Transcription via API", ru: "Распознавание через API" } },
  { role: "local_server", key: "local_server_s", def: 60, min: 5, max: 1800, label: { en: "Local server", ru: "Локальный сервер" } },
  { role: "post_processing", key: "post_processing_s", def: 15, min: 5, max: 300, label: { en: "Post-processing", ru: "Постобработка" } },
  { role: "builtin_local", key: "builtin_local_s", def: 120, min: 10, max: 1800, label: { en: "Built-in engine", ru: "Встроенный движок" } },
];
const GROUP: Record<Lang, string> = { en: "Timeouts (seconds)", ru: "Таймауты (секунды)" };

const FR24_DEFAULTS: TimeoutsWire = {
  connect_s: 5,
  api_transcription_s: 30,
  local_server_s: 60,
  post_processing_s: 15,
  builtin_local_s: 120,
};
// Stored, in range and different from every default, so a control showing the default
// (or another role's value) instead of the stored value is caught.
const STORED: TimeoutsWire = {
  connect_s: 7,
  api_transcription_s: 45,
  local_server_s: 90,
  post_processing_s: 20,
  builtin_local_s: 300,
};

function tab(page: Page, name: string) {
  return page.getByTestId(`tab-${name}`);
}

/** Collects uncaught page errors from before the first navigation. */
function pageErrors(page: Page): Error[] {
  const errors: Error[] = [];
  page.on("pageerror", (error) => errors.push(error));
  return errors;
}

function settingsTimeouts(view: SettingsView): TimeoutsWire | undefined {
  return (view.settings as unknown as { timeouts?: TimeoutsWire }).timeouts;
}

/** A saved (not first-run) view in `lang` with `timeouts` stored and engine `engine`. */
function savedView(timeouts: TimeoutsWire, lang: Lang = "en", engine = "none"): SettingsView {
  const view = firstRunView();
  view.first_run = false;
  view.settings.ui_language = lang;
  view.settings.engine = engine as SettingsView["settings"]["engine"];
  (view.settings as unknown as { timeouts: TimeoutsWire }).timeouts = { ...timeouts };
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

/** The group and its one number control per role, looked up first ("control not found"). */
async function timeoutControls(page: Page, lang: Lang) {
  const group = page.getByRole("group", { name: GROUP[lang], exact: true });
  await expect(group, `the General tab renders the "${GROUP[lang]}" group`).toHaveCount(1);
  await expect(group).toBeVisible();
  await expect(group.locator("[data-field^='timeouts.']")).toHaveCount(ROLES.length);
  const controls = {} as Record<Role, ReturnType<Page["locator"]>>;
  for (const { role } of ROLES) {
    const control = group.locator(`[data-field="timeouts.${role}"]`);
    await expect(control, `the group renders timeouts.${role}`).toHaveCount(1);
    await expect(control).toHaveAttribute("type", "number");
    controls[role] = control;
  }
  return controls;
}

/** The single settings_save request so far (fails unless exactly one was made). */
async function onlySaveRequest(page: Page): Promise<SaveRequest> {
  await expect.poll(async () => (await calls(page, "settings_save")).length).toBe(1);
  const [call] = await calls(page, "settings_save");
  return (call.args as { request: SaveRequest }).request;
}

function requestTimeouts(request: SaveRequest): TimeoutsWire | undefined {
  return (request.settings as unknown as { timeouts?: TimeoutsWire }).timeouts;
}

// ---- Acceptance 1: held with FR-24 defaults, editable in the settings window (en/ru) ----

test("the core first-run view carries the timeouts object with the FR-24 defaults, and the General tab shows them", async ({ page }) => {
  // The wire fixture is core's (settings::service::tests::e2e_settings_wire_fixture_matches_core);
  // red until it is regenerated from the implementation, never edited by hand.
  const errors = pageErrors(page);
  const view = firstRunView();
  expect(settingsTimeouts(view)).toEqual(FR24_DEFAULTS);
  await openGeneral(page, view);
  const controls = await timeoutControls(page, "en");
  for (const { role, def } of ROLES) await expect(controls[role]).toHaveValue(String(def));
  expect(await calls(page, "settings_save")).toEqual([]);
  expect(errors).toEqual([]);
});

for (const lang of ["en", "ru"] as const) {
  test(`the General tab shows the "${GROUP[lang]}" group with five number inputs, labelled and hinted in ${lang}, holding the stored values`, async ({ page }) => {
    const errors = pageErrors(page);
    await openGeneral(page, savedView(STORED, lang));
    const controls = await timeoutControls(page, lang);
    for (const { role, key, label } of ROLES) {
      const control = controls[role];
      await expect(control).toHaveAccessibleName(label[lang]);
      await expect(control).toHaveValue(String(STORED[key]));
      const hint = text(lang, `settings.timeouts.${role}.hint`);
      await expect(page.getByText(hint, { exact: true })).toBeVisible();
      await expect(control).toHaveAccessibleDescription(escapeRegExp(hint));
      // A hint is not an error.
      await expect(control).not.toHaveAttribute("aria-invalid", "true");
    }
    // Only the UI language's labels are shown.
    const other: Lang = lang === "en" ? "ru" : "en";
    await expect(page.getByText(GROUP[other], { exact: true })).toHaveCount(0);
    // Showing the view writes nothing back.
    expect(await calls(page, "settings_save")).toEqual([]);
    expect(errors).toEqual([]);
  });
}

for (const engine of ["api", "local_server", "builtin_local"] as const) {
  test(`the five timeout fields are shown whatever the engine (engine ${engine})`, async ({ page }) => {
    // #99 Q2: all five always shown, so a refusal on any of them is always reachable.
    await openGeneral(page, savedView(STORED, "en", engine));
    const controls = await timeoutControls(page, "en");
    for (const { role, key } of ROLES) await expect(controls[role]).toHaveValue(String(STORED[key]));
  });
}

test("edited timeouts are sent in the settings_save request's timeouts object, every other value as the view, and become the saved baseline", async ({ page }) => {
  const errors = pageErrors(page);
  const view = savedView(STORED);
  await openGeneral(page, view);
  const controls = await timeoutControls(page, "en");

  const edited: TimeoutsWire = { ...STORED, api_transcription_s: 120, local_server_s: 1800 };
  await controls.api_transcription.fill("120");
  await controls.local_server.fill("1800");
  await page.getByTestId("settings-save").click();

  const request = await onlySaveRequest(page);
  // Exactly the two edits, in the wire object core reads; nothing else changed.
  expect(requestTimeouts(request)).toEqual(edited);
  expect(request.settings).toEqual({ ...view.settings, timeouts: edited });
  expect(request.keys).toEqual({ transcription_api: "Untouched", local_server: "Untouched", post_processing: "Untouched" });

  // The mock's Saved stores what was sent; the window takes it as the new baseline.
  await expect(page.getByRole("status")).toContainText(text("en", "settings.saved"));
  expect(settingsTimeouts(await storedView(page))).toEqual(edited);
  await expect(controls.api_transcription).toHaveValue("120");
  await expect(controls.local_server).toHaveValue("1800");
  await expect(controls.connect).toHaveValue(String(STORED.connect_s));
  expect(errors).toEqual([]);
});

test("a non-whole or empty timeout entry is sent as 0 (core refuses it), and the input keeps what was typed", async ({ page }) => {
  // History.svelte's entry rule: the wire type is u32; the range is core's rule only.
  const view = savedView(STORED);
  await openGeneral(page, view);
  await queueSaveOutcome(page, {
    Refused: {
      errors: [
        { field: "timeouts.connect", code: "timeout.range" },
        { field: "timeouts.post_processing", code: "timeout.range" },
      ],
      form_error: null,
    },
  });
  const controls = await timeoutControls(page, "en");
  await controls.connect.fill("");
  await controls.post_processing.fill("2.5");
  await page.getByTestId("settings-save").click();

  const request = await onlySaveRequest(page);
  expect(requestTimeouts(request)).toEqual({ ...STORED, connect_s: 0, post_processing_s: 0 });
  await expect(controls.connect).toHaveValue("");
  await expect(controls.post_processing).toHaveValue("2.5");
  await expect(controls.connect).toHaveAttribute("aria-invalid", "true");
  await expect(controls.post_processing).toHaveAttribute("aria-invalid", "true");
});

// ---- Acceptance 2 (failure branch): out of range is refused on save, on its field ----

for (const { role, key, max } of ROLES) {
  test(`failure branch: ${role} above its maximum is sent as typed, core's Refused timeouts.${role} timeout.range is shown on that field only, the typed value stays and nothing is saved`, async ({ page }) => {
    const errors = pageErrors(page);
    const view = savedView(STORED);
    await openGeneral(page, view);
    await queueSaveOutcome(page, {
      Refused: { errors: [{ field: `timeouts.${role}`, code: "timeout.range" }], form_error: null },
    });

    const controls = await timeoutControls(page, "en");
    const typed = String(max + 1);
    await controls[role].fill(typed);
    await page.getByTestId("settings-save").click();

    // The UI does not pre-empt or clamp: the request reaches core once, with the value as typed.
    const request = await onlySaveRequest(page);
    expect(requestTimeouts(request)).toEqual({ ...STORED, [key]: max + 1 });

    // The reason is on the field (U2), localized, not a form-level message.
    const control = controls[role];
    await expect(control).toHaveAttribute("aria-invalid", "true");
    await expect(control).toHaveAccessibleDescription(escapeRegExp(text("en", "error.timeout.range")));
    await expect(page.getByText(text("en", "error.timeout.range"), { exact: true })).toHaveCount(1);
    await expect(page.getByRole("alert")).toHaveCount(0);
    // Only the named field is highlighted; the others keep their values.
    for (const other of ROLES) {
      if (other.role === role) continue;
      await expect(controls[other.role]).not.toHaveAttribute("aria-invalid", "true");
      await expect(controls[other.role]).toHaveValue(String(STORED[other.key]));
    }
    // The draft keeps the user's value, and Save can be pressed again.
    await expect(control).toHaveValue(typed);
    await expect(page.getByTestId("settings-save")).toBeEnabled();

    // Nothing was saved and nothing announced.
    await expect(page.getByRole("status")).toHaveCount(0);
    expect(await storedView(page)).toEqual(view);
    expect(await emitted(page, "settings://changed")).toEqual([]);
    expect((await calls(page, "settings_save")).length).toBe(1);
    expect(errors).toEqual([]);
  });
}

test("failure branch in ru: a Refused timeouts.api_transcription timeout.range shows the Russian error on that field", async ({ page }) => {
  const view = savedView(STORED, "ru");
  await openGeneral(page, view);
  await queueSaveOutcome(page, {
    Refused: { errors: [{ field: "timeouts.api_transcription", code: "timeout.range" }], form_error: null },
  });
  const controls = await timeoutControls(page, "ru");
  await controls.api_transcription.fill("4");
  await page.getByTestId("settings-save").click();
  await onlySaveRequest(page);
  const control = controls.api_transcription;
  await expect(control).toHaveAttribute("aria-invalid", "true");
  await expect(control).toHaveAccessibleDescription(escapeRegExp(text("ru", "error.timeout.range")));
  await expect(page.getByText(text("en", "error.timeout.range"), { exact: true })).toHaveCount(0);
  await expect(control).toHaveValue("4");
  await expect(page.getByRole("status")).toHaveCount(0);
  expect(await storedView(page)).toEqual(view);
});

test("after a timeout.range refusal, a corrected value is saved and the highlight clears", async ({ page }) => {
  const view = savedView(STORED);
  await openGeneral(page, view);
  await queueSaveOutcome(page, {
    Refused: { errors: [{ field: "timeouts.connect", code: "timeout.range" }], form_error: null },
  });
  const controls = await timeoutControls(page, "en");
  await controls.connect.fill("0");
  await page.getByTestId("settings-save").click();
  await expect(controls.connect).toHaveAttribute("aria-invalid", "true");

  await controls.connect.fill("60");
  // No scripted outcome left: the mock answers Saved with what was sent.
  await page.getByTestId("settings-save").click();
  await expect.poll(async () => (await calls(page, "settings_save")).length).toBe(2);
  const second = ((await calls(page, "settings_save"))[1].args as { request: SaveRequest }).request;
  expect(requestTimeouts(second)).toEqual({ ...STORED, connect_s: 60 });
  await expect(page.getByRole("status")).toContainText(text("en", "settings.saved"));
  await expect(controls.connect).not.toHaveAttribute("aria-invalid", "true");
  expect(settingsTimeouts(await storedView(page))).toEqual({ ...STORED, connect_s: 60 });
});
