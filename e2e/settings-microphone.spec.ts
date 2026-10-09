// T-012 red e2e: the Recording-tab microphone field fed by `settings_list_microphones`
// (spec 004 contracts/ipc.md, spec 004 edge case "selected microphone not connected when
// settings open", tasks T036/T043; spec 001 FR-030 / req FR-27; decisions #38 Q5, #64)
// over the one IPC mock (e2e/support/tauriMock.ts). The device list is the mock's fake
// adapter data (`fakeMicrophones()`: three endpoints, the second the Windows default).
// The locator contract of e2e/settings-first-run.spec.ts applies (tab-<tab> test ids,
// data-field controls, the role="tabpanel" panel, settings-save).
//
// Locator contract this spec adds (T-012 analysis, seam: "the Recording tab shows
// 'System default' (null) plus the list, and a saved id that is absent as
// '<name> (not connected)'"):
// - the field: `<select data-field="recording.microphone">` on the Recording tab, with the
//   accessible name of `settings.field_label.recording.microphone` ("Microphone"); it is
//   what calls `settings_list_microphones`;
// - its options: first the system default, `value=""`, text
//   `t("settings.microphone.system_default")` (microphone `null`, data-model: Windows
//   default); then one option per listed device, in list order, `value=<device id>`, text
//   the device name, or `t("settings.microphone.windows_default", { name })` for the one
//   entry with `is_default` (and for no other);
// - a saved microphone whose id is not in the list: one more option, `value=<saved id>`,
//   text `t("settings.microphone.not_connected", { name: <saved name> })`, selected; the
//   draft keeps the saved `{ id, name }` (never replaced by the default or by another
//   device; R); its position among the options is not pinned;
// - choosing an option writes the draft: "" -> `null`, a device -> `{ id, name }` with the
//   listed name; matching is by the whole id, the name is for display only;
// - a rejected `settings_list_microphones` shows `t("error.ipc_unavailable")` in the
//   Recording panel, never the rejection text (settings-ui U2), and keeps the draft.
//
// New UI-only message ids (en + ru, the catalog rule of i18n.md): the three
// `settings.microphone.*` above. Not settled by the spec and therefore not pinned here:
// what the select shows while the list is pending or after it was rejected, whether the
// list is fetched again on a later visit of the tab (T-045's section re-lists on remount),
// and which name is shown for a listed device whose saved name differs.
import { readFileSync } from "node:fs";
import type { Page } from "@playwright/test";
import { expect, test } from "./support/boot";
import {
  calls,
  fakeMicrophones,
  firstRunView,
  installTauriMock,
  releaseMicrophones,
  type InputDevice,
  type MockOptions,
  type SaveRequest,
  type SettingsView,
} from "./support/tauriMock";

type Lang = "en" | "ru";
type Messages = Record<string, string>;
const CATALOG: Record<Lang, Messages> = {
  en: JSON.parse(readFileSync(new URL("../i18n/en.json", import.meta.url), "utf8")) as Messages,
  ru: JSON.parse(readFileSync(new URL("../i18n/ru.json", import.meta.url), "utf8")) as Messages,
};

/** The catalog text of `id` in `lang` with `{name}` placeholders filled; a missing text fails here, by name. */
function msg(lang: Lang, id: string, args: Record<string, string> = {}): string {
  const text = CATALOG[lang][id];
  if (typeof text !== "string" || text === "") throw new Error(`i18n/${lang}.json has no text for ${id}`);
  return text.replace(/\{([a-z][a-z0-9_]*)\}/g, (whole, name: string) => (Object.hasOwn(args, name) ? args[name] : whole));
}

const en = (id: string, args?: Record<string, string>) => msg("en", id, args);

const MICS = fakeMicrophones();
const [USB, HEADSET, LINE_IN] = MICS;
// The fake list's Windows default is its second entry.
const DEFAULT_MIC = MICS.find((m) => m.is_default)!;

/** A saved microphone no machine lists (unplugged); an obviously fake endpoint id. */
const UNPLUGGED = {
  id: "{0.0.1.00000000}.{00000000-0000-4000-8000-0000000fa0ff}",
  name: "Desk Microphone (Fake Unplugged)",
};

/** A rejection text that must never reach the page. */
const CANARY = "FAKE-LIST-MICROPHONES-REJECTION-7c1e";

function micSelect(page: Page) {
  return page.locator('[data-field="recording.microphone"]');
}

function panel(page: Page) {
  return page.getByRole("tabpanel");
}

/** The select's options as `{ value, text }`, in DOM order. */
async function options(page: Page): Promise<{ value: string; text: string }[]> {
  await expect(micSelect(page), "the microphone select is rendered").toHaveCount(1);
  return micSelect(page)
    .locator("option")
    .evaluateAll((els) =>
      els.map((e) => ({ value: (e as HTMLOptionElement).value, text: (e.textContent ?? "").trim() })),
    );
}

/** What the options must be for `list` in `lang`, with no saved-but-absent entry. */
function listedOptions(list: InputDevice[], lang: Lang = "en"): { value: string; text: string }[] {
  return [
    { value: "", text: msg(lang, "settings.microphone.system_default") },
    ...list.map((m) => ({
      value: m.id,
      text: m.is_default ? msg(lang, "settings.microphone.windows_default", { name: m.name }) : m.name,
    })),
  ];
}

/** Collects uncaught page errors from before the first navigation. */
function pageErrors(page: Page): Error[] {
  const errors: Error[] = [];
  page.on("pageerror", (error) => errors.push(error));
  return errors;
}

/** A saved (not first-run) view with `microphone`. */
function savedView(microphone: SettingsView["settings"]["microphone"], lang: Lang = "en"): SettingsView {
  const view = firstRunView();
  view.first_run = false;
  view.settings.engine = "api";
  view.settings.microphone = microphone === null ? null : { ...microphone };
  view.settings.ui_language = lang;
  return view;
}

/** Opens the settings window on the Recording tab. */
async function openRecording(page: Page, view: SettingsView, options: Omit<MockOptions, "view"> = {}): Promise<void> {
  await installTauriMock(page, { ...options, view });
  await page.goto("/settings?tab=recording");
  await expect(page.getByTestId("tab-recording")).toHaveAttribute("aria-selected", "true");
}

/** Opens the Recording tab and waits until the select offers the listed devices. */
async function openListed(page: Page, view: SettingsView, options: Omit<MockOptions, "view"> = {}): Promise<void> {
  await openRecording(page, view, options);
  const list = Array.isArray(options.microphones) ? options.microphones : MICS;
  for (const m of list) {
    await expect(micSelect(page).locator(`option[value="${m.id}"]`), `${m.name} offered`).toHaveCount(1);
  }
}

/** Clicks Save and returns the one `settings_save` request. */
async function saveRequest(page: Page): Promise<SaveRequest> {
  await page.getByTestId("settings-save").click();
  await expect.poll(async () => (await calls(page, "settings_save")).length).toBe(1);
  const [save] = await calls(page, "settings_save");
  return (save.args as { request: SaveRequest }).request;
}

// ---- (1) The list, with the Windows default marked -------------------------------------

test("the Recording tab lists System default and every device of settings_list_microphones in list order by name, marking only the Windows default", async ({ page }) => {
  const errors = pageErrors(page);
  await openListed(page, firstRunView());

  await expect(micSelect(page)).toHaveAccessibleName(en("settings.field_label.recording.microphone"));
  expect(await options(page)).toEqual(listedOptions(MICS));
  // The mark follows is_default, not the list position.
  expect(MICS[0].is_default).toBe(false);
  expect((await options(page)).filter((o) => o.text.includes(DEFAULT_MIC.name))).toEqual([
    { value: DEFAULT_MIC.id, text: en("settings.microphone.windows_default", { name: DEFAULT_MIC.name }) },
  ]);
  // A first run (microphone null) shows the system default; nothing is shown as not connected.
  await expect(micSelect(page)).toHaveValue("");
  expect(await calls(page, "settings_list_microphones")).not.toHaveLength(0);
  expect(errors).toEqual([]);
});

// ---- (2) The saved selection shown and saved by id ---------------------------------------

test("a saved microphone that is listed is shown selected, nothing is marked not connected, and an untouched Save sends it unchanged", async ({ page }) => {
  const errors = pageErrors(page);
  const saved = { id: LINE_IN.id, name: LINE_IN.name };
  await openListed(page, savedView(saved));

  await expect(micSelect(page)).toHaveValue(LINE_IN.id);
  expect(await options(page)).toEqual(listedOptions(MICS));

  const request = await saveRequest(page);
  expect(request.settings.microphone).toEqual(saved);
  expect(errors).toEqual([]);
});

test("choosing a listed device and saving sends it by its id with its listed name", async ({ page }) => {
  const errors = pageErrors(page);
  await openListed(page, firstRunView());

  await micSelect(page).selectOption(USB.id);
  await expect(micSelect(page)).toHaveValue(USB.id);

  const request = await saveRequest(page);
  expect(request.settings.microphone).toEqual({ id: USB.id, name: USB.name });
  expect(errors).toEqual([]);
});

test("choosing the device marked Windows default saves that device by its id, not the system default", async ({ page }) => {
  // A named device stays selected when Windows' default changes later; null follows it.
  const errors = pageErrors(page);
  await openListed(page, savedView({ id: LINE_IN.id, name: LINE_IN.name }));

  await micSelect(page).selectOption(HEADSET.id);

  const request = await saveRequest(page);
  expect(request.settings.microphone).toEqual({ id: HEADSET.id, name: HEADSET.name });
  expect(errors).toEqual([]);
});

test("choosing System default over a saved device saves microphone null", async ({ page }) => {
  const errors = pageErrors(page);
  await openListed(page, savedView({ id: USB.id, name: USB.name }));
  await expect(micSelect(page)).toHaveValue(USB.id);

  await micSelect(page).selectOption("");

  const request = await saveRequest(page);
  expect(request.settings.microphone).toBeNull();
  expect(errors).toEqual([]);
});

// ---- (3) A saved device that is not listed (unplugged) ------------------------------------

for (const lang of ["en", "ru"] as const) {
  test(`failure branch: a saved microphone missing from the list stays selected, shown as not connected by its saved name, and an untouched Save sends it unchanged (${lang})`, async ({ page }) => {
    const errors = pageErrors(page);
    await openListed(page, savedView(UNPLUGGED, lang));

    // Not replaced by the Windows default or any listed device: the saved id stays selected.
    await expect(micSelect(page)).toHaveValue(UNPLUGGED.id);
    const shown = await options(page);
    const absent = { value: UNPLUGGED.id, text: msg(lang, "settings.microphone.not_connected", { name: UNPLUGGED.name }) };
    expect(shown.filter((o) => o.value === UNPLUGGED.id)).toEqual([absent]);
    // Every listed device is still offered, with the same marks.
    expect(shown.filter((o) => o.value !== UNPLUGGED.id)).toEqual(listedOptions(MICS, lang));

    const request = await saveRequest(page);
    expect(request.settings.microphone).toEqual(UNPLUGGED);
    expect(errors).toEqual([]);
  });
}

test("failure branch: with no input device listed at all, a saved microphone is shown not connected beside System default and is sent back unchanged", async ({ page }) => {
  const errors = pageErrors(page);
  await openRecording(page, savedView(UNPLUGGED), { microphones: [] });
  await expect(micSelect(page).locator(`option[value="${UNPLUGGED.id}"]`)).toHaveCount(1);

  await expect(micSelect(page)).toHaveValue(UNPLUGGED.id);
  const shown = await options(page);
  expect([...shown].sort((a, b) => a.value.localeCompare(b.value))).toEqual([
    { value: "", text: en("settings.microphone.system_default") },
    { value: UNPLUGGED.id, text: en("settings.microphone.not_connected", { name: UNPLUGGED.name }) },
  ]);
  await expect(panel(page)).not.toContainText(en("error.ipc_unavailable"));

  const request = await saveRequest(page);
  expect(request.settings.microphone).toEqual(UNPLUGGED);
  expect(errors).toEqual([]);
});

test("a list arriving after the draft never rewrites the saved microphone: Save while the list is pending sends it unchanged", async ({ page }) => {
  // R (settings-ui.md): options appearing later do not write the draft.
  const errors = pageErrors(page);
  const saved = { id: LINE_IN.id, name: LINE_IN.name };
  await openRecording(page, savedView(saved), { holdMicrophones: true });
  await expect.poll(async () => (await calls(page, "settings_list_microphones")).length).toBeGreaterThan(0);

  const request = await saveRequest(page);
  expect(request.settings.microphone).toEqual(saved);

  await releaseMicrophones(page);
  await expect(micSelect(page).locator(`option[value="${LINE_IN.id}"]`)).toHaveCount(1);
  await expect(micSelect(page)).toHaveValue(LINE_IN.id);
  expect(errors).toEqual([]);
});

// ---- (4) The list cannot be read --------------------------------------------------------

test("failure branch: a rejected settings_list_microphones shows error.ipc_unavailable on the Recording tab, never the rejection text, and an untouched Save sends the saved microphone unchanged", async ({ page }) => {
  const errors = pageErrors(page);
  const saved = { id: USB.id, name: USB.name };
  await openRecording(page, savedView(saved), { microphones: { reject: CANARY } });

  await expect(panel(page)).toContainText(en("error.ipc_unavailable"));
  await expect(page.locator("body")).not.toContainText(CANARY);
  expect(await calls(page, "settings_list_microphones")).not.toHaveLength(0);
  // The rest of the tab is still usable.
  await expect(page.locator('[data-field="recording.hotkey"]')).toHaveCount(1);
  await expect(page.locator('[data-field="recording.mode"]')).toHaveCount(1);
  // The microphone control is rendered and offers no device the list did not give (what it
  // shows for the saved one while the list is unknown is not pinned).
  await expect(micSelect(page)).toHaveCount(1);
  for (const m of MICS.filter((d) => d.id !== saved.id)) {
    await expect(micSelect(page).locator(`option[value="${m.id}"]`), `${m.name} not offered`).toHaveCount(0);
  }

  const request = await saveRequest(page);
  expect(request.settings.microphone).toEqual(saved);
  await expect(page.locator("body")).not.toContainText(CANARY);
  expect(errors).toEqual([]);
});
