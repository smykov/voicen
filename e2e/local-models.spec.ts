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
import type { Page } from "@playwright/test";
import { expect, test } from "./support/boot";
import {
  calls,
  deleteOutcome,
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
  queueDeleteOutcome,
  queueDeleteRejection,
  queueDownloadRejection,
  queueSaveOutcome,
  releaseDelete,
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

// ---- T-019: delete a downloaded model (spec 002 US4, FR-021 to FR-023; option A) -------
//
// Locator contract this part adds (T-019 analysis, Investigation 7):
// - Delete is a button with test id `local-model-delete` inside a row, offered on a
//   `downloaded` row only (never on not_downloaded, downloading or failed);
// - it asks first in an in-page `role="alertdialog"` with an accessible name that names
//   the model (`t(nameKey)`); its buttons are `local-model-delete-keep` (nothing is sent)
//   and `local-model-delete-confirm` (one `local_model_delete { id }`); no window.confirm;
// - the command emits no event: after the invoke settles the rows follow a re-list of
//   `local_models_list` (no optimistic state). Every Delete button is disabled from the
//   confirmed invoke until the re-list after it has settled;
// - a refusal (`model_in_use`, `delete_failed`, `not_downloaded`) is shown by its
//   messageKey in the section's `role="alert"`, a rejection that is not a FailureReason as
//   `error.ipc_unavailable` (never its text); a refusal changes no row and no selection;
// - `engineReset: true` reaches the window only as `settings://changed` (the mock emits
//   it, as the bridge does): the UI writes nothing to the draft (U1); a clean draft then
//   shows engine none and the section unmounts; a dirty draft keeps its edits;
// - `resetFailed: true` (OQ-26 (a)): the row follows the re-list (not downloaded) and the
//   section shows `settings.write_failed` in its alert.

const ru = (id: string, args?: Record<string, string>) => msg("ru", id, args);

function deleteButton(page: Page, id: string) {
  return row(page, id).getByTestId("local-model-delete");
}

function deleteDialog(page: Page) {
  return page.getByRole("alertdialog");
}

async function deleteCalls(page: Page): Promise<unknown[]> {
  return (await calls(page, "local_model_delete")).map((call) => call.args);
}

async function listCount(page: Page): Promise<number> {
  return (await calls(page, "local_models_list")).length;
}

/** Clicks Delete on `id`, checks the confirmation names the model, and confirms. */
async function confirmDelete(page: Page, id: string, lang: Lang = "en"): Promise<void> {
  await deleteButton(page, id).click();
  const dialog = deleteDialog(page);
  await expect(dialog, "Delete asks first in an in-page alertdialog").toBeVisible();
  const nameKey = localModelsFirstRun().find((model) => model.id === id)!.nameKey;
  await expect(dialog).toContainText(msg(lang, nameKey));
  await dialog.getByTestId("local-model-delete-confirm").click();
  await expect(deleteDialog(page)).toHaveCount(0);
}

/** `localModelsFirstRun()` with each listed row in its state. */
function modelsIn(states: Record<string, ModelState>): LocalModelView[] {
  const list = localModelsFirstRun();
  for (const [id, state] of Object.entries(states)) {
    const r = list.find((model) => model.id === id);
    if (r === undefined) throw new Error(`local-models-wire.json has no model ${id}`);
    r.state = structuredClone(state);
  }
  return list;
}

/** The view core publishes after deleting the selected built-in model: engine none, no model. */
function resetView(from: SettingsView): SettingsView {
  const view = structuredClone(from);
  view.settings.engine = "none";
  view.settings.builtin_local.model_id = null;
  return view;
}

test("Delete is offered only on downloaded rows: not on a not downloaded, downloading or failed row, and it appears when a row becomes downloaded", async ({ page }) => {
  const errors = pageErrors(page);
  await openLoaded(page, localView(), {
    localModels: modelsIn({
      tiny: DOWNLOADED,
      base: { kind: "downloading", received: QUARTER_OF_BASE, total: BASE_BYTES },
      small: DOWNLOADED,
      "medium-q5_0": failedState("checksum_mismatch"),
    }),
  });
  await expect(row(page, "small")).toHaveAttribute("data-state", "downloaded");

  await expect(deleteButton(page, "tiny")).toBeEnabled();
  await expect(deleteButton(page, "small")).toBeEnabled();
  await expect(deleteButton(page, "base")).toHaveCount(0);
  await expect(deleteButton(page, "medium-q5_0")).toHaveCount(0);
  await expect(deleteButton(page, "large-v3-turbo-q5_0")).toHaveCount(0);
  // Exactly the two downloaded rows carry one, and nothing outside the rows does.
  await expect(section(page).getByTestId("local-model-delete")).toHaveCount(2);

  // The rows decide it: base finishing makes it deletable.
  await localModelState(page, "base", DOWNLOADED);
  await expect(deleteButton(page, "base")).toBeEnabled();
  await expect(section(page).getByTestId("local-model-delete")).toHaveCount(3);
  expect(await calls(page, "local_model_delete")).toEqual([]);
  expect(errors).toEqual([]);
});

test("Keep in the delete confirmation sends nothing and leaves small downloaded and selected", async ({ page }) => {
  const errors = pageErrors(page);
  await openLoaded(page, localView("small"), { localModels: modelsWith("small", DOWNLOADED) });
  const lists = await listCount(page);

  await deleteButton(page, "small").click();
  const dialog = deleteDialog(page);
  await expect(dialog).toBeVisible();
  await expect(dialog).toHaveAccessibleName(/\S/);
  await expect(dialog).toContainText(en("local_model.name.small"));
  await dialog.getByTestId("local-model-delete-keep").click();

  await expect(deleteDialog(page)).toHaveCount(0);
  expect(await calls(page, "local_model_delete")).toEqual([]);
  await expect(row(page, "small")).toHaveAttribute("data-state", "downloaded");
  await expect(deleteButton(page, "small")).toBeEnabled();
  await expect(modelSelect(page)).toHaveValue("small");
  expect(await listCount(page)).toBe(lists);
  expect(errors).toEqual([]);
});

test("Acceptance 1: confirming Delete on small sends local_model_delete {id: small} once, and the re-list shows small not downloaded with Download and not selectable; the selection and settings are untouched", async ({ page }) => {
  const errors = pageErrors(page);
  await openLoaded(page, localView("base"), { localModels: modelsIn({ base: DOWNLOADED, small: DOWNLOADED }) });
  await expectOptions(page, ["base", "small"]);
  const lists = await listCount(page);

  await confirmDelete(page, "small");
  await expect.poll(() => deleteCalls(page)).toEqual([{ id: "small" }]);
  // The command emits nothing (option A): only a re-list can show the removal.
  await expect.poll(() => listCount(page)).toBeGreaterThan(lists);
  const small = row(page, "small");
  await expect(small).toHaveAttribute("data-state", "not_downloaded");
  await expect(action(page, "small", "download")).toBeEnabled();
  await expect(deleteButton(page, "small")).toHaveCount(0);
  await expect(small.getByTestId("local-model-reason")).toHaveCount(0);
  await expectOptions(page, ["base"]);
  await expect(modelSelect(page)).toHaveValue("base");
  await expect(deleteButton(page, "base")).toBeEnabled();
  expect(await emitted(page, LOCAL_MODEL_EVENTS.state)).toEqual([]);
  expect(await calls(page, "settings_save")).toEqual([]);
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(await deleteCalls(page)).toEqual([{ id: "small" }]);
  expect(errors).toEqual([]);
});

test("failure branch: deleting the selected model with a clean draft: engineReset arrives as settings://changed, the Engine select shows none and the model section is gone; the UI saves nothing", async ({ page }) => {
  const errors = pageErrors(page);
  const view = localView("small");
  await openLoaded(page, view, { localModels: modelsWith("small", DOWNLOADED) });
  await expect(modelSelect(page)).toHaveValue("small");
  await queueDeleteOutcome(page, deleteOutcome("engine_reset"), resetView(view));

  await confirmDelete(page, "small");
  await expect.poll(() => deleteCalls(page)).toEqual([{ id: "small" }]);
  await expect(page.locator('[data-field="engine.kind"]')).toHaveValue("none");
  await expect(section(page)).toHaveCount(0);
  await expect(modelSelect(page)).toHaveCount(0);
  // The reset is core's (persisted and published); the window writes nothing (U1).
  expect(await calls(page, "settings_save")).toEqual([]);
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(await deleteCalls(page)).toEqual([{ id: "small" }]);
  expect(errors).toEqual([]);
});

test("failure branch: deleting the selected model with a dirty draft keeps the draft's builtin_local and small (no UI write); the re-list shows small not downloaded and Save sends the draft as edited", async ({ page }) => {
  const errors = pageErrors(page);
  const view = localView("small");
  view.settings.speech_language = "en";
  await openLoaded(page, view, { localModels: modelsWith("small", DOWNLOADED) });
  // A pending edit elsewhere on the tab makes the draft dirty.
  await page.locator('[data-field="engine.speech_language"]').selectOption("de");
  await queueDeleteOutcome(page, deleteOutcome("engine_reset"), resetView(view));

  await confirmDelete(page, "small");
  await expect.poll(() => deleteCalls(page)).toEqual([{ id: "small" }]);
  await expect.poll(async () => (await emitted(page, "settings://changed")).length).toBe(1);
  await expect(row(page, "small")).toHaveAttribute("data-state", "not_downloaded");
  await expect(page.locator('[data-field="engine.kind"]')).toHaveValue("builtin_local");
  await expectOptions(page, []);
  expect(await calls(page, "settings_save")).toEqual([]);

  await page.getByTestId("settings-save").click();
  await expect.poll(async () => (await calls(page, "settings_save")).length).toBe(1);
  const [save] = await calls(page, "settings_save");
  const request = (save.args as { request: SaveRequest }).request;
  expect(request.settings.engine).toBe("builtin_local");
  expect(request.settings.builtin_local.model_id).toBe("small");
  expect(request.settings.speech_language).toBe("de");
  expect(errors).toEqual([]);
});

test("failure branch: resetFailed (OQ-26 a): the file is gone, the re-list shows small not downloaded, and the section shows settings.write_failed; the engine stays builtin_local", async ({ page }) => {
  const errors = pageErrors(page);
  await openLoaded(page, localView("small"), { localModels: modelsWith("small", DOWNLOADED) });
  await queueDeleteOutcome(page, deleteOutcome("reset_failed"));

  await confirmDelete(page, "small");
  await expect.poll(() => deleteCalls(page)).toEqual([{ id: "small" }]);
  await expect(section(page).getByRole("alert")).toContainText(en("settings.write_failed"));
  await expect(row(page, "small")).toHaveAttribute("data-state", "not_downloaded");
  await expect(action(page, "small", "download")).toBeEnabled();
  await expect(page.locator('[data-field="engine.kind"]')).toHaveValue("builtin_local");
  await expect(section(page).getByRole("alert")).not.toContainText(en("error.ipc_unavailable"));
  expect(await calls(page, "settings_save")).toEqual([]);
  expect(errors).toEqual([]);
});

for (const [code, messageKey] of [
  ["model_in_use", "delete.model_in_use"],
  ["delete_failed", "delete.failed"],
  ["not_downloaded", "delete.not_downloaded"],
] as const) {
  test(`failure branch: a delete refused with ${code} shows ${messageKey} in the section, and small stays downloaded, selected and deletable`, async ({ page }) => {
    const errors = pageErrors(page);
    await openLoaded(page, localView("small"), { localModels: modelsWith("small", DOWNLOADED) });
    await queueDeleteRejection(page, failureReason(code));

    await confirmDelete(page, "small");
    await expect.poll(() => deleteCalls(page)).toEqual([{ id: "small" }]);
    const alert = section(page).getByRole("alert");
    await expect(alert).toContainText(en(messageKey));
    await expect(alert).not.toContainText(en("error.ipc_unavailable"));
    // Never the code or the key as text.
    await expect(section(page)).not.toContainText(code);
    await expect(section(page)).not.toContainText(messageKey);
    // A refusal changes no row and no selection.
    const small = row(page, "small");
    await expect(small).toHaveAttribute("data-state", "downloaded");
    await expect(small.getByTestId("local-model-reason")).toHaveCount(0);
    await expect(deleteButton(page, "small")).toBeEnabled();
    await expectOptions(page, ["small"]);
    await expect(modelSelect(page)).toHaveValue("small");
    await expect(page.locator('[data-field="engine.kind"]')).toHaveValue("builtin_local");
    expect(await calls(page, "settings_save")).toEqual([]);
    expect(errors).toEqual([]);
  });
}

test("failure branch: a delete refused with model_in_use is shown in Russian when the UI language is ru", async ({ page }) => {
  const errors = pageErrors(page);
  await openLoaded(page, localView("small", "ru"), { localModels: modelsWith("small", DOWNLOADED) });
  await queueDeleteRejection(page, failureReason("model_in_use"));

  await confirmDelete(page, "small", "ru");
  await expect.poll(() => deleteCalls(page)).toEqual([{ id: "small" }]);
  await expect(section(page).getByRole("alert")).toContainText(ru("delete.model_in_use"));
  await expect(section(page)).not.toContainText(en("delete.model_in_use"));
  await expect(row(page, "small")).toHaveAttribute("data-state", "downloaded");
  expect(errors).toEqual([]);
});

test("failure branch: a delete rejection that is not a FailureReason shows error.ipc_unavailable and never the rejection text; small stays downloaded", async ({ page }) => {
  const errors = pageErrors(page);
  await openLoaded(page, localView("small"), { localModels: modelsWith("small", DOWNLOADED) });
  await queueDeleteRejection(page, "local_model_delete exploded (fake)");

  await confirmDelete(page, "small");
  await expect.poll(() => deleteCalls(page)).toEqual([{ id: "small" }]);
  await expect(section(page).getByRole("alert")).toContainText(en("error.ipc_unavailable"));
  await expect(page.locator("body")).not.toContainText("exploded (fake)");
  await expect(row(page, "small")).toHaveAttribute("data-state", "downloaded");
  await expect(deleteButton(page, "small")).toBeEnabled();
  await expect(modelSelect(page)).toHaveValue("small");
  expect(errors).toEqual([]);
});

test("every Delete button stays disabled while the delete invoke and then the re-list after it are held, and the row changes only with that re-list", async ({ page }) => {
  const errors = pageErrors(page);
  await openLoaded(page, localView("base"), {
    holdDelete: true,
    localModels: modelsIn({ base: DOWNLOADED, small: DOWNLOADED }),
  });
  const lists = await listCount(page);

  await confirmDelete(page, "small");
  await expect.poll(() => deleteCalls(page)).toEqual([{ id: "small" }]);
  // Held: core has not answered, so no row has changed; the pending invoke blocks.
  await expect(deleteButton(page, "small")).toBeDisabled();
  await expect(deleteButton(page, "base")).toBeDisabled();
  await expect(row(page, "small")).toHaveAttribute("data-state", "downloaded");

  await holdLists(page);
  await releaseDelete(page);
  // The invoke resolved and its re-list was issued (and is held): still blocked, and the
  // row is not changed by the UI itself (no optimistic not_downloaded).
  await expect.poll(() => listCount(page)).toBe(lists + 1);
  await expect(row(page, "small")).toHaveAttribute("data-state", "downloaded");
  await expect(deleteButton(page, "small")).toBeDisabled();
  await expect(deleteButton(page, "base")).toBeDisabled();

  await releaseList(page);
  await expect(row(page, "small")).toHaveAttribute("data-state", "not_downloaded");
  await expect(deleteButton(page, "small")).toHaveCount(0);
  await expect(deleteButton(page, "base")).toBeEnabled();
  expect(await deleteCalls(page)).toEqual([{ id: "small" }]);
  expect(errors).toEqual([]);
});

// ---- T-045 review r2 Low #9: the per-row polite live region (characterization) -------

test("a row turning failed shows its reason inside the polite live region that was already mounted with the row (the same element)", async ({ page }) => {
  const errors = pageErrors(page);
  await openLoaded(page);
  const regions = row(page, "base").locator('[aria-live="polite"]');
  await expect(regions).toHaveCount(1);
  await expect(regions.getByTestId("local-model-reason")).toHaveCount(0);
  // Mark the element that exists while base is not downloaded.
  await regions.evaluate((el) => el.setAttribute("data-t045-marker", "before-failed"));

  await localModelState(page, "base", failedState("download_interrupted"));
  const marked = section(page).locator('[data-t045-marker="before-failed"]');
  await expect(marked).toHaveCount(1);
  await expect(marked).toHaveAttribute("aria-live", "polite");
  await expect(marked.getByTestId("local-model-reason")).toHaveText(en("download.interrupted"));
  await expect(row(page, "base").locator('[aria-live="polite"]')).toHaveCount(1);
  expect(errors).toEqual([]);
});
