// T-045 red e2e: the local-model picker on the Engine tab (spec 002 US1 AS1-AS7, FR-001,
// FR-002, FR-004 to FR-007; decision #58) over the one IPC mock (e2e/support/tauriMock.ts).
// Model data is core's (e2e/fixtures/local-models-wire.json). Sizes follow the OQ-08
// default: binary units as Windows Explorer shows them (base = 147 951 465 B = "141 MB").
// The locator contract of e2e/settings-first-run.spec.ts applies (tab-<tab> test ids,
// data-field controls, aria-invalid + aria-describedby for a refused field,
// settings-save).
//
// Locator contract this spec adds (T-045 analysis, seam (c)):
// - the section: test id `local-models`, rendered on the Engine tab while the draft's
//   engine is `builtin_local` (and only then: it is what calls `local_models_list`);
// - one row per listed model, in list (catalog) order, test id `local-model-<id>`, with
//   `data-state="<ModelState.kind>"` (not_downloaded | downloading | downloaded |
//   failed), showing `t(nameKey)` and `formatSize(sizeBytes)`; the only elements of the
//   section with a `data-state` attribute are the rows;
// - the recommended badge: test id `local-model-recommended`, inside the row, only where
//   `recommended` is true (small);
// - the row's actions are buttons inside the row: `local-model-download` (state
//   not_downloaded), `local-model-retry` (failed, never anywhere else), `local-model-cancel`
//   (downloading); a downloaded row has none of them. While any row is downloading or a
//   download invoke is pending, every Download / Retry button is disabled (I4), and the
//   invoke counts as pending until the re-list issued after it has settled (review r1 #3);
// - the command emits nothing: after a download invoke resolves, the rows follow a
//   re-list of `local_models_list` (no event needed to show `downloading`, review r1 #1);
// - a downloading row shows a `role="progressbar"` and, as text, formatSize(received),
//   formatSize(total) and the whole percentage ("25%" in en);
// - a failed row shows its reason in test id `local-model-reason` (inside the row; not
//   `role="alert"`): `t(messageKey, reasonArgs)`; no other row state has that element;
// - a refused download (the invoke rejects with a FailureReason) and a failed IPC call
//   (a download rejection that is not a FailureReason -> `error.ipc_unavailable`; a
//   rejected `local_models_list` -> `error.ipc_unavailable`, no rows) are shown in the
//   section, outside every row's `local-model-reason`, and never the rejection text; a
//   refusal changes no row's state (I2);
// - the model select: `<select data-field="engine.builtin_local.model_id">` whose options
//   with a catalog id as value are exactly the downloaded rows (I3), option value = model
//   id; `bind:value` on the draft's model_id; it carries aria-invalid / aria-describedby
//   like every control (U2).
import { readFileSync } from "node:fs";
import { expect, test, type Page } from "@playwright/test";
import {
  calls,
  emitted,
  failedState,
  failureReason,
  firstRunView,
  holdLists,
  installTauriMock,
  listeners,
  localModelProgress,
  localModelsFirstRun,
  localModelState,
  LOCAL_MODEL_EVENTS,
  modelsWith,
  queueDownloadRejection,
  queueSaveOutcome,
  releaseDownload,
  releaseList,
  storedModels,
  type LocalModelView,
  type MockOptions,
  type ModelState,
  type SaveRequest,
  type SettingsView,
} from "./support/tauriMock";

type Lang = "en" | "ru";
type Messages = Record<string, string>;
const CATALOG: Record<Lang, Messages> = {
  en: JSON.parse(readFileSync(new URL("../i18n/en.json", import.meta.url), "utf8")) as Messages,
  ru: JSON.parse(readFileSync(new URL("../i18n/ru.json", import.meta.url), "utf8")) as Messages,
};

/** The catalog text of `id` in `lang` with `{name}` placeholders filled; a missing text fails here. */
function msg(lang: Lang, id: string, args: Record<string, string> = {}): string {
  const text = CATALOG[lang][id];
  if (typeof text !== "string" || text === "") throw new Error(`i18n/${lang}.json has no text for ${id}`);
  return text.replace(/\{([a-z][a-z0-9_]*)\}/g, (whole, name: string) => (Object.hasOwn(args, name) ? args[name] : whole));
}

const en = (id: string, args?: Record<string, string>) => msg("en", id, args);

const IDS = localModelsFirstRun().map((model) => model.id);
const BASE_BYTES = localModelsFirstRun().find((model) => model.id === "base")!.sizeBytes;
/** A quarter of base and a byte (not on a rounding edge): 35 MB of 141 MB, 25%. */
const QUARTER_OF_BASE = 36_987_867;

/** formatSize literals for core's catalog sizes (binary MB, OQ-08 default). */
const SIZE_EN: Record<string, string> = {
  tiny: "74 MB",
  base: "141 MB",
  small: "465 MB",
  "medium-q5_0": "514 MB",
  "large-v3-turbo-q5_0": "547 MB",
};

function section(page: Page) {
  return page.getByTestId("local-models");
}

function row(page: Page, id: string) {
  return section(page).getByTestId(`local-model-${id}`);
}

function action(page: Page, id: string, kind: "download" | "retry" | "cancel") {
  return row(page, id).getByTestId(`local-model-${kind}`);
}

function modelSelect(page: Page) {
  return page.locator('[data-field="engine.builtin_local.model_id"]');
}

/** The select's option values that are catalog ids (a placeholder option is ignored). */
async function modelOptions(page: Page): Promise<string[]> {
  await expect(modelSelect(page), "the model select is rendered").toHaveCount(1);
  const values = await modelSelect(page)
    .locator("option")
    .evaluateAll((els) => els.map((e) => (e as HTMLOptionElement).value));
  return values.filter((value) => IDS.includes(value));
}

async function expectOptions(page: Page, ids: string[]): Promise<void> {
  await expect.poll(() => modelOptions(page), { message: "the select offers exactly the downloaded models" }).toEqual(ids);
}

/** Collects uncaught page errors from before the first navigation. */
function pageErrors(page: Page): Error[] {
  const errors: Error[] = [];
  page.on("pageerror", (error) => errors.push(error));
  return errors;
}

/** A saved view with engine builtin_local (not a first run). */
function localView(modelId: string | null = null, lang: Lang = "en"): SettingsView {
  const view = firstRunView();
  view.first_run = false;
  view.settings.engine = "builtin_local";
  view.settings.builtin_local.model_id = modelId;
  view.settings.ui_language = lang;
  return view;
}

async function open(page: Page, view: SettingsView = localView(), options: Omit<MockOptions, "view"> = {}): Promise<void> {
  await installTauriMock(page, { ...options, view });
  await page.goto("/settings");
  await expect(page.getByRole("tab", { selected: true })).toBeVisible();
}

/** Opens the page and waits for the five rows and both local-model listeners. */
async function openLoaded(page: Page, view?: SettingsView, options: Omit<MockOptions, "view"> = {}): Promise<void> {
  await open(page, view, options);
  for (const id of IDS) await expect(row(page, id), `row ${id}`).toBeVisible();
  for (const event of [LOCAL_MODEL_EVENTS.progress, LOCAL_MODEL_EVENTS.state]) {
    await expect.poll(() => listeners(page, event), { message: `the section listens to ${event}` }).toBeGreaterThan(0);
  }
}

async function downloadCalls(page: Page): Promise<unknown[]> {
  return (await calls(page, "local_model_download")).map((call) => call.args);
}

/** Every Download / Retry button of the rows other than `except`. */
function otherDownloads(page: Page, except: string) {
  return IDS.filter((id) => id !== except).map((id) => action(page, id, "download"));
}

const DOWNLOADED: ModelState = { kind: "downloaded" };

// ---- (1) The list ---------------------------------------------------------------------

test("the Engine tab lists the five models in catalog order with names, sizes and the recommended badge on small only; the select offers no model", async ({ page }) => {
  const errors = pageErrors(page);
  await openLoaded(page);

  const order = await section(page)
    .locator("[data-state]")
    .evaluateAll((els) => els.map((e) => (e as HTMLElement).dataset.testid ?? ""));
  expect(order).toEqual(IDS.map((id) => `local-model-${id}`));

  for (const model of localModelsFirstRun()) {
    const r = row(page, model.id);
    await expect(r).toHaveAttribute("data-state", "not_downloaded");
    await expect(r).toContainText(en(model.nameKey));
    await expect(r).toContainText(SIZE_EN[model.id]);
    await expect(r.getByTestId("local-model-recommended")).toHaveCount(model.recommended ? 1 : 0);
    await expect(action(page, model.id, "download")).toBeEnabled();
    await expect(action(page, model.id, "retry")).toHaveCount(0);
    await expect(action(page, model.id, "cancel")).toHaveCount(0);
  }
  await expect(row(page, "small").getByTestId("local-model-recommended")).toBeVisible();
  expect(await modelOptions(page)).toEqual([]);
  // The raw byte count is never shown.
  await expect(section(page)).not.toContainText(String(BASE_BYTES));
  expect(errors).toEqual([]);
});

// ---- (2) Acceptance 1: happy path ----------------------------------------------------

test("picking base shows progress and makes base selectable; while it downloads the other Download buttons are disabled; saving sends model_id base", async ({ page }) => {
  const errors = pageErrors(page);
  await openLoaded(page);

  await action(page, "base", "download").click();
  await expect.poll(() => downloadCalls(page)).toEqual([{ id: "base" }]);

  await localModelProgress(page, "base", QUARTER_OF_BASE);
  const base = row(page, "base");
  await expect(base).toHaveAttribute("data-state", "downloading");
  await expect(base.getByRole("progressbar")).toBeVisible();
  await expect(base).toContainText("35 MB");
  await expect(base).toContainText("141 MB");
  await expect(base).toContainText("25%");
  await expect(action(page, "base", "cancel")).toBeEnabled();
  await expect(action(page, "base", "download")).toHaveCount(0);
  for (const other of otherDownloads(page, "base")) await expect(other).toBeDisabled();
  // Downloading is not downloaded: not selectable yet.
  expect(await modelOptions(page)).toEqual([]);

  await localModelState(page, "base", DOWNLOADED);
  await expect(base).toHaveAttribute("data-state", "downloaded");
  await expect(base.getByRole("progressbar")).toHaveCount(0);
  await expect(action(page, "base", "cancel")).toHaveCount(0);
  await expect(action(page, "base", "download")).toHaveCount(0);
  for (const other of otherDownloads(page, "base")) await expect(other).toBeEnabled();
  await expectOptions(page, ["base"]);
  // Becomes selectable, not selected: the draft's model_id changes only on the user's pick (I3).
  await expect(modelSelect(page)).not.toHaveValue("base");

  await modelSelect(page).selectOption("base");
  await page.getByTestId("settings-save").click();
  await expect.poll(async () => (await calls(page, "settings_save")).length).toBe(1);
  const [save] = await calls(page, "settings_save");
  const request = (save.args as { request: SaveRequest }).request;
  expect(request.settings.engine).toBe("builtin_local");
  expect(request.settings.builtin_local.model_id).toBe("base");
  // The Saved echo keeps the pick.
  await expect(modelSelect(page)).toHaveValue("base");
  expect(errors).toEqual([]);
});

// ---- (3)(4) Failure branch: a failed download shows the reason and Retry ---------------

for (const [code, messageKey] of [
  ["download_interrupted", "download.interrupted"],
  ["checksum_mismatch", "download.checksum_mismatch"],
] as const) {
  test(`failure branch: a ${code} download shows the reason and Retry, and base is not selectable`, async ({ page }) => {
    const errors = pageErrors(page);
    await openLoaded(page);
    await action(page, "base", "download").click();
    await expect.poll(() => downloadCalls(page)).toEqual([{ id: "base" }]);
    await localModelProgress(page, "base", QUARTER_OF_BASE);
    await expect(row(page, "base")).toHaveAttribute("data-state", "downloading");

    await localModelState(page, "base", failedState(code));
    const base = row(page, "base");
    await expect(base).toHaveAttribute("data-state", "failed");
    await expect(base.getByTestId("local-model-reason")).toHaveText(en(messageKey));
    await expect(action(page, "base", "retry")).toBeEnabled();
    await expect(action(page, "base", "download")).toHaveCount(0);
    await expect(action(page, "base", "cancel")).toHaveCount(0);
    await expect(base.getByRole("progressbar")).toHaveCount(0);
    // The download ended: the other rows can download again.
    for (const other of otherDownloads(page, "base")) await expect(other).toBeEnabled();
    expect(await modelOptions(page)).toEqual([]);
    // A reason is not an alert (it arrives asynchronously).
    await expect(page.getByRole("alert")).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}

// ---- (5) Retry starts the download again ---------------------------------------------

test("failure branch: Retry starts the download again: a second local_model_download for base, and the reason is gone once progress arrives", async ({ page }) => {
  const errors = pageErrors(page);
  await openLoaded(page);
  await action(page, "base", "download").click();
  await expect.poll(() => downloadCalls(page)).toEqual([{ id: "base" }]);
  await localModelState(page, "base", failedState("checksum_mismatch"));
  await expect(row(page, "base").getByTestId("local-model-reason")).toHaveText(en("download.checksum_mismatch"));

  await action(page, "base", "retry").click();
  await expect.poll(() => downloadCalls(page)).toEqual([{ id: "base" }, { id: "base" }]);
  await localModelProgress(page, "base", 1_048_576);
  const base = row(page, "base");
  await expect(base).toHaveAttribute("data-state", "downloading");
  await expect(base.getByTestId("local-model-reason")).toHaveCount(0);
  await expect(action(page, "base", "retry")).toHaveCount(0);
  await expect(action(page, "base", "cancel")).toBeEnabled();
  await expect(base).toContainText("1 MB");
  for (const other of otherDownloads(page, "base")) await expect(other).toBeDisabled();
  expect(errors).toEqual([]);
});

test("failure branch: a failed row listed when the window opens shows its reason and Retry, and Retry downloads it again", async ({ page }) => {
  // Failed stays until a retry (contracts/ipc.md Wire form); a reopened window lists it.
  const errors = pageErrors(page);
  await openLoaded(page, localView(), { localModels: modelsWith("base", failedState("download_interrupted")) });
  await expect(row(page, "base")).toHaveAttribute("data-state", "failed");
  await expect(row(page, "base").getByTestId("local-model-reason")).toHaveText(en("download.interrupted"));
  expect(await modelOptions(page)).toEqual([]);

  await action(page, "base", "retry").click();
  await expect.poll(() => downloadCalls(page)).toEqual([{ id: "base" }]);
  expect(errors).toEqual([]);
});

// ---- (6) Cancel ------------------------------------------------------------------------

test("failure branch: Cancel returns base to not downloaded with no Retry and no reason, and base is not selectable", async ({ page }) => {
  const errors = pageErrors(page);
  await openLoaded(page);
  await action(page, "base", "download").click();
  await expect.poll(() => downloadCalls(page)).toEqual([{ id: "base" }]);
  await localModelProgress(page, "base", QUARTER_OF_BASE);
  await expect(action(page, "base", "cancel")).toBeEnabled();

  await action(page, "base", "cancel").click();
  await expect
    .poll(async () => (await calls(page, "local_model_cancel_download")).map((call) => call.args))
    .toEqual([{ id: "base" }]);
  const base = row(page, "base");
  await expect(base).toHaveAttribute("data-state", "not_downloaded");
  await expect(action(page, "base", "download")).toBeEnabled();
  await expect(action(page, "base", "retry")).toHaveCount(0);
  await expect(base.getByTestId("local-model-reason")).toHaveCount(0);
  await expect(base.getByRole("progressbar")).toHaveCount(0);
  for (const other of otherDownloads(page, "base")) await expect(other).toBeEnabled();
  expect(await modelOptions(page)).toEqual([]);
  expect((await storedModels(page)).find((model) => model.id === "base")?.state).toEqual({ kind: "not_downloaded" });
  expect(errors).toEqual([]);
});

// ---- (7) A refused download: the needed size is formatted, the row is unchanged ----------

for (const lang of ["en", "ru"] as const) {
  test(`failure branch: a download refused for disk space shows the needed size formatted (${lang}) and leaves the row not downloaded`, async ({ page }) => {
    const errors = pageErrors(page);
    await openLoaded(page, localView(null, lang));
    // Core's refusal: needed "66256" (bytes) = 65 kB in binary units.
    const refusal = failureReason("not_enough_disk_space");
    expect(refusal.params).toEqual({ needed: "66256" });
    await queueDownloadRejection(page, refusal);

    await action(page, "base", "download").click();
    await expect.poll(() => downloadCalls(page)).toEqual([{ id: "base" }]);
    const size = lang === "en" ? "65 kB" : "65 кБ";
    await expect(section(page)).toContainText(msg(lang, "download.not_enough_disk_space", { needed: size }));
    await expect(section(page)).toContainText(size);
    // Never the raw byte count, never a byte unit of the old text.
    await expect(section(page)).not.toContainText("66256");
    await expect(section(page)).not.toContainText(lang === "en" ? "bytes" : "байт");

    // A refusal changes no state (I2): no reason, no Retry, Download available again.
    const base = row(page, "base");
    await expect(base).toHaveAttribute("data-state", "not_downloaded");
    await expect(base.getByTestId("local-model-reason")).toHaveCount(0);
    await expect(action(page, "base", "retry")).toHaveCount(0);
    await expect(action(page, "base", "download")).toBeEnabled();
    for (const other of otherDownloads(page, "base")) await expect(other).toBeEnabled();
    expect(await modelOptions(page)).toEqual([]);
    expect(errors).toEqual([]);
  });
}

// ---- (8) A rejection that is not a FailureReason --------------------------------------

test("failure branch: a download rejection that is not a FailureReason shows error.ipc_unavailable and never the rejection text", async ({ page }) => {
  const errors = pageErrors(page);
  await openLoaded(page);
  await queueDownloadRejection(page, "local_model_download exploded (fake)");

  await action(page, "base", "download").click();
  await expect.poll(() => downloadCalls(page)).toEqual([{ id: "base" }]);
  await expect(section(page)).toContainText(en("error.ipc_unavailable"));
  await expect(page.locator("body")).not.toContainText("exploded (fake)");
  await expect(row(page, "base")).toHaveAttribute("data-state", "not_downloaded");
  await expect(row(page, "base").getByTestId("local-model-reason")).toHaveCount(0);
  await expect(action(page, "base", "download")).toBeEnabled();
  expect(errors).toEqual([]);
});

// ---- (9) Restart: a saved, downloaded model is selected ----------------------------------

test("a saved model base that the list shows downloaded (after a restart) is selected, and only it is offered", async ({ page }) => {
  const errors = pageErrors(page);
  await openLoaded(page, localView("base"), { localModels: modelsWith("base", DOWNLOADED) });

  await expect(row(page, "base")).toHaveAttribute("data-state", "downloaded");
  await expect(action(page, "base", "download")).toHaveCount(0);
  await expect(action(page, "base", "retry")).toHaveCount(0);
  await expect(action(page, "base", "cancel")).toHaveCount(0);
  await expectOptions(page, ["base"]);
  await expect(modelSelect(page)).toHaveValue("base");
  // Every other row can still be downloaded.
  for (const other of otherDownloads(page, "base")) await expect(other).toBeEnabled();
  expect(errors).toEqual([]);
});

// ---- (10) A rejected list ----------------------------------------------------------------

test("failure branch: a rejected local_models_list shows error.ipc_unavailable in the section, no rows, and never the rejection text", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page, localView(), { listRejection: "local_models_list failed (fake)" });

  await expect(section(page)).toContainText(en("error.ipc_unavailable"));
  await expect(section(page).locator("[data-state]")).toHaveCount(0);
  await expect(page.locator("body")).not.toContainText("local_models_list failed (fake)");
  expect(await calls(page, "local_models_list")).not.toHaveLength(0);
  expect(await modelOptions(page)).toEqual([]);
  expect(errors).toEqual([]);
});

// ---- (11) Core's refusal model.not_downloaded is shown on the select -------------------

test("failure branch: a save refused with model.not_downloaded marks the model select invalid with its message, and the saved value is sent unchanged", async ({ page }) => {
  const errors = pageErrors(page);
  // Saved model base, but the list has it not downloaded (deleted or missing on disk).
  await openLoaded(page, localView("base"));
  expect(await modelOptions(page)).toEqual([]);
  await queueSaveOutcome(page, {
    Refused: { errors: [{ field: "engine.builtin_local.model_id", code: "model.not_downloaded" }], form_error: null },
  });

  await page.getByTestId("settings-save").click();
  await expect(modelSelect(page)).toHaveAttribute("aria-invalid", "true");
  await expect(modelSelect(page)).toHaveAccessibleDescription(en("error.model.not_downloaded"));
  const [save] = await calls(page, "settings_save");
  // The draft kept the value no option shows (I3: options vanishing never rewrite it).
  expect((save.args as { request: SaveRequest }).request.settings.builtin_local.model_id).toBe("base");
  expect(errors).toEqual([]);
});

// ---- (12)(13) Ordering and a download already running (I2, OQ-09 default) ------------------

test("an event received while local_models_list is in flight is applied over its older response (I2)", async ({ page }) => {
  const errors = pageErrors(page);
  await open(page, localView(), { holdList: true });
  await expect.poll(async () => (await calls(page, "local_models_list")).length).toBeGreaterThan(0);
  await expect.poll(() => listeners(page, LOCAL_MODEL_EVENTS.state)).toBeGreaterThan(0);

  // The held response is a snapshot from before base finished downloading.
  await localModelState(page, "base", DOWNLOADED);
  await releaseList(page);

  await expect(row(page, "base")).toHaveAttribute("data-state", "downloaded");
  await expectOptions(page, ["base"]);
  expect(errors).toEqual([]);
});

test("a download already running when the window opens shows its progress and Cancel, and blocks every other Download", async ({ page }) => {
  const errors = pageErrors(page);
  const running: ModelState = { kind: "downloading", received: QUARTER_OF_BASE, total: BASE_BYTES };
  const models: LocalModelView[] = modelsWith("base", running);
  await openLoaded(page, localView(), { localModels: models });

  const base = row(page, "base");
  await expect(base).toHaveAttribute("data-state", "downloading");
  await expect(base.getByRole("progressbar")).toBeVisible();
  await expect(base).toContainText("35 MB");
  await expect(base).toContainText("25%");
  await expect(action(page, "base", "cancel")).toBeEnabled();
  for (const other of otherDownloads(page, "base")) await expect(other).toBeDisabled();
  expect(await calls(page, "local_model_download")).toEqual([]);
  expect(errors).toEqual([]);
});

// ---- Review round 1: the re-list after the invoke, and the pending invoke (I2, I4) -------

/** Every Download / Retry button the rows show now (whatever their states). */
function downloadOrRetry(page: Page) {
  return section(page).locator('[data-testid="local-model-download"], [data-testid="local-model-retry"]');
}

async function expectAllDownloadOrRetry(page: Page, state: "disabled" | "enabled", count: number): Promise<void> {
  const buttons = downloadOrRetry(page);
  await expect(buttons).toHaveCount(count);
  for (let i = 0; i < count; i++) {
    if (state === "disabled") await expect(buttons.nth(i), `Download/Retry #${i}`).toBeDisabled();
    else await expect(buttons.nth(i), `Download/Retry #${i}`).toBeEnabled();
  }
}

test("Download with no event after the click: the re-list alone shows base downloading (0 kB of 141 MB) with Cancel, and every other Download is disabled", async ({ page }) => {
  // Review r1 #1 (M2): the command emits nothing; the first progress can be 30 s away.
  const errors = pageErrors(page);
  await openLoaded(page);

  await action(page, "base", "download").click();
  await expect.poll(() => downloadCalls(page)).toEqual([{ id: "base" }]);
  const base = row(page, "base");
  await expect(base).toHaveAttribute("data-state", "downloading");
  await expect(base.getByRole("progressbar")).toBeVisible();
  await expect(base).toContainText("0 kB");
  await expect(base).toContainText("141 MB");
  await expect(action(page, "base", "cancel")).toBeEnabled();
  await expect(action(page, "base", "download")).toHaveCount(0);
  for (const other of otherDownloads(page, "base")) await expect(other).toBeDisabled();
  // No event was involved.
  expect(await emitted(page, LOCAL_MODEL_EVENTS.progress)).toEqual([]);
  expect(await emitted(page, LOCAL_MODEL_EVENTS.state)).toEqual([]);
  expect(errors).toEqual([]);
});

test("failure branch: Retry on a failed row with no event after the click: the re-list alone removes the old reason and Retry and shows downloading", async ({ page }) => {
  // Review r1 #1 (M2): "Retry starts the download again" must show before any progress.
  const errors = pageErrors(page);
  await openLoaded(page, localView(), { localModels: modelsWith("base", failedState("checksum_mismatch")) });
  await expect(row(page, "base").getByTestId("local-model-reason")).toHaveText(en("download.checksum_mismatch"));

  await action(page, "base", "retry").click();
  await expect.poll(() => downloadCalls(page)).toEqual([{ id: "base" }]);
  const base = row(page, "base");
  await expect(base).toHaveAttribute("data-state", "downloading");
  await expect(base.getByTestId("local-model-reason")).toHaveCount(0);
  await expect(action(page, "base", "retry")).toHaveCount(0);
  await expect(action(page, "base", "cancel")).toBeEnabled();
  await expect(section(page)).not.toContainText(en("download.checksum_mismatch"));
  expect(await emitted(page, LOCAL_MODEL_EVENTS.progress)).toEqual([]);
  expect(await emitted(page, LOCAL_MODEL_EVENTS.state)).toEqual([]);
  expect(errors).toEqual([]);
});

test("while a download invoke is pending, every Download and Retry button is disabled, though no row is downloading yet", async ({ page }) => {
  // Review r1 #2 (M1): I4's "or a local_model_download invoke is pending".
  const errors = pageErrors(page);
  await openLoaded(page, localView(), {
    holdDownload: true,
    localModels: modelsWith("tiny", failedState("download_interrupted")),
  });
  await expectAllDownloadOrRetry(page, "enabled", 5);

  await action(page, "base", "download").click();
  await expect.poll(() => downloadCalls(page)).toEqual([{ id: "base" }]);
  // Held: core has not answered, so no row has changed; only the pending invoke blocks.
  await expect(row(page, "base")).toHaveAttribute("data-state", "not_downloaded");
  await expect(row(page, "tiny")).toHaveAttribute("data-state", "failed");
  await expectAllDownloadOrRetry(page, "disabled", 5);
  expect(await downloadCalls(page)).toEqual([{ id: "base" }]);

  await releaseDownload(page);
  await expect(row(page, "base")).toHaveAttribute("data-state", "downloading");
  await expectAllDownloadOrRetry(page, "disabled", 4);
  expect(await downloadCalls(page)).toEqual([{ id: "base" }]);
  expect(errors).toEqual([]);
});

test("the Download and Retry buttons stay disabled after the invoke resolves until the re-list after it has settled, also while that re-list is held", async ({ page }) => {
  // Review r1 #3: core set Downloading before the command returned; until the re-list
  // shows it, the rows still say not_downloaded, so only the pending flag blocks.
  const errors = pageErrors(page);
  await openLoaded(page, localView(), {
    holdDownload: true,
    localModels: modelsWith("tiny", failedState("download_interrupted")),
  });
  const listsBefore = (await calls(page, "local_models_list")).length;
  await holdLists(page);

  await action(page, "base", "download").click();
  await expect.poll(() => downloadCalls(page)).toEqual([{ id: "base" }]);
  await releaseDownload(page);
  // The invoke resolved and its re-list was issued (and is held).
  await expect.poll(async () => (await calls(page, "local_models_list")).length).toBe(listsBefore + 1);
  await expect(row(page, "base")).toHaveAttribute("data-state", "not_downloaded");
  await expectAllDownloadOrRetry(page, "disabled", 5);

  await releaseList(page);
  await expect(row(page, "base")).toHaveAttribute("data-state", "downloading");
  await expectAllDownloadOrRetry(page, "disabled", 4);
  expect(await downloadCalls(page)).toEqual([{ id: "base" }]);
  expect(errors).toEqual([]);
});
