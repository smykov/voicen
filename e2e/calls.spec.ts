// T-143 red e2e: the Calls page (spec 007 FR-033, FR-034, FR-036, FR-046; contracts/ipc.md
// calls_* commands; T-143 Acceptance 1 to 7) over the one IPC mock (e2e/support/tauriMock.ts).
// Payloads are core's (e2e/fixtures/calls-wire.json, pinned by
// storage::tests::calls_wire_fixture_matches_core); every case is derived from it by the
// mock's helpers (D7), never hand-written.
//
// Locator contract this spec adds (the developer implements the page against it):
// - the route `/calls`, run in the window labelled `calls`; it calls `settings_get` (language
//   from `settings.ui_language`, hotkey from `settings.calls.hotkey`), `calls_list`, and listens
//   to `calls://changed`, `calls://open`, `settings://changed`;
// - root element, test id `calls-page`, present as soon as the page has booted;
// - `call-list` holds one `call-row` per call, in `calls_list` order (never re-sorted); a row
//   holds `call-title` (wire text), `call-date` ("DD.MM.YYYY HH:MM" from the written startedAt),
//   `call-duration` ("MM:SS", "H:MM:SS", "—" for null), `call-state` (calls.state.<state>),
//   for a failed call `call-reason` (calls.failed.<reason>), and `call-actions`, a container
//   of the row's action buttons in actionsFor(state) order, named by the catalog (calls.copy,
//   calls.export_md, calls.export_json, calls.open_folder, calls.delete);
// - a row of a non-damaged call has a button `call-select` that opens its detail
//   (`calls_get { root, id }`); a damaged row has none (calls_get is ioFailed for it);
// - `call-empty` (calls.empty with {hotkey}) only when the list has no calls; one
//   `call-root-unavailable` (calls.root_unavailable with {root}) per unavailable root;
// - `call-detail` holds one `call-line` per line, each with `line-time` ("MM:SS"/"H:MM:SS" of
//   startMs), `speaker-swatch` (a coloured element, distinct background per speaker id),
//   `line-speaker` (speakers[].name) and `line-text` (the text as a text node, matches in <mark>);
//   a search input labelled calls.search filters nothing (all lines stay) and marks matches;
//   zero matches over a non-empty query shows calls.no_matches;
// - an action failure shows one element with role `alert` holding the catalog text of
//   calls.error.io.<reason> (D4); a cancelled export ({saved: false}) shows none.
import { readFileSync } from "node:fs";
import type { Page } from "@playwright/test";
import { bootPage, expect, test } from "./support/boot";
import {
  calls,
  callsDetail,
  callsDetailWithTexts,
  callsEmptyList,
  callsError,
  callsList,
  callsListAll,
  callsListRu,
  emit,
  firstRunView,
  installTauriMock,
  listeners,
  queueCallsResult,
  setCallsList,
  type CallDetail,
  type CallList,
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
const ru = (id: string, args?: Record<string, string>) => msg("ru", id, args);

const HOTKEY = "Ctrl+Alt+K"; // not the default of the settings fixture: the page must read settings_get
const READY = { root: "C:\\Calls", id: "2026-10-29_100000" };

interface OpenOptions {
  list?: CallList;
  detail?: CallDetail;
  lang?: Lang;
}

async function openCalls(page: Page, options: OpenOptions = {}): Promise<void> {
  const view = firstRunView();
  view.settings.ui_language = options.lang ?? "en";
  view.settings.calls.hotkey = HOTKEY;
  await installTauriMock(page, {
    view,
    callsList: options.list ?? callsListAll(),
    callsDetail: options.detail ?? callsDetail(),
    windowLabel: "calls",
  });
  await bootPage(page, "/calls", page.getByTestId("calls-page"));
}

const rows = (page: Page) => page.getByTestId("call-row");
const readyRow = (page: Page) => rows(page).filter({ hasText: "Weekly sync" });
const alert = (page: Page) => page.getByRole("alert");

async function openReadyDetail(page: Page): Promise<void> {
  await readyRow(page).getByTestId("call-select").click();
  await expect(page.getByTestId("call-detail")).toBeVisible();
}

/** Lets the page's pending microtasks and a frame settle without a timer (for "nothing shown"). */
async function settle(page: Page): Promise<void> {
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
}

test("AC-1: 3 calls show newest first, in core order, with title, date and time, duration and state", async ({ page }) => {
  const list = callsList(3);
  await openCalls(page, { list });
  await expect(rows(page)).toHaveCount(3);
  const expected = [
    { title: "Call 30.10 14:05", date: "30.10.2026 14:05", duration: "—", state: en("calls.state.recording") },
    { title: "Call 30.10 13:00", date: "30.10.2026 13:00", duration: "30:00", state: en("calls.state.queued") },
    { title: "Call 30.10 12:00", date: "30.10.2026 12:00", duration: "40:00", state: en("calls.state.transcribing") },
  ];
  expect(list.calls.map((c) => c.title)).toEqual(expected.map((e) => e.title)); // the fixture is what the table says
  for (const [i, want] of expected.entries()) {
    const row = rows(page).nth(i);
    await expect(row.getByTestId("call-title")).toHaveText(want.title);
    await expect(row.getByTestId("call-date")).toHaveText(want.date);
    await expect(row.getByTestId("call-duration")).toHaveText(want.duration);
    await expect(row.getByTestId("call-state")).toHaveText(want.state);
  }
  expect((await calls(page, "calls_list")).length).toBeGreaterThanOrEqual(1);
});

test("AC-1 D1: the rows follow calls_list order verbatim, even when it is not newest first", async ({ page }) => {
  const list = callsList(3);
  list.calls.reverse(); // oldest first: any client-side sort would put 14:05 back on top
  await openCalls(page, { list });
  await expect(rows(page)).toHaveCount(3);
  await expect(page.getByTestId("call-title")).toHaveText(["Call 30.10 12:00", "Call 30.10 13:00", "Call 30.10 14:05"]);
});

test("AC-1 D2: the date and time are the written digits of startedAt, not converted", async ({ page }) => {
  const list = callsList(2);
  list.calls[0].startedAt = "2026-10-30T23:30:00+03:00"; // a Date conversion moves this on any other zone
  list.calls[1].startedAt = "2026-12-31T23:59:59-05:00";
  await openCalls(page, { list });
  await expect(page.getByTestId("call-date")).toHaveText(["30.10.2026 23:30", "31.12.2026 23:59"]);
});

test("AC-2: every state badge appears (12 rows), failed with each reason, recovered and damaged", async ({ page }) => {
  const list = callsListAll();
  expect(list.calls).toHaveLength(12);
  await openCalls(page, { list });
  await expect(rows(page)).toHaveCount(12);
  const failedIds: Record<string, string> = {
    key: "calls.failed.key",
    network: "calls.failed.network",
    timeout: "calls.failed.timeout",
    provider: "calls.failed.provider",
    storage: "calls.failed.storage",
    engineNotSetUp: "calls.failed.engine_not_set_up",
  };
  for (const [i, call] of list.calls.entries()) {
    const row = rows(page).nth(i);
    await expect(row.getByTestId("call-state")).toHaveText(en(`calls.state.${call.state}`));
    if (call.state === "failed") {
      await expect(row.getByTestId("call-reason")).toHaveText(en(failedIds[call.failReason as string]));
    } else {
      await expect(row.getByTestId("call-reason")).toHaveCount(0);
    }
  }
  await expect(rows(page).filter({ hasText: "Recovered after a crash" })).toHaveCount(1);
  await expect(rows(page).filter({ hasText: en("calls.state.queued") })).toHaveCount(1);
  await expect(page.getByTestId("call-reason")).toHaveText([
    en("calls.failed.key"),
    en("calls.failed.network"),
    en("calls.failed.timeout"),
    en("calls.failed.provider"),
    en("calls.failed.storage"),
    en("calls.failed.engine_not_set_up"),
  ]);
});

test("AC-2 D3: a damaged row offers Open folder and Delete only and does not open to detail", async ({ page }) => {
  await openCalls(page);
  const damaged = rows(page).filter({ hasText: en("calls.state.damaged") });
  await expect(damaged).toHaveCount(1);
  await expect(damaged.getByTestId("call-actions").getByRole("button")).toHaveText([
    en("calls.open_folder"),
    en("calls.delete"),
  ]);
  await expect(damaged.getByTestId("call-select")).toHaveCount(0);
  // The same page: a ready row has the full set, a recording row only Open folder.
  await expect(readyRow(page).getByTestId("call-actions").getByRole("button")).toHaveText([
    en("calls.copy"),
    en("calls.export_md"),
    en("calls.export_json"),
    en("calls.open_folder"),
    en("calls.delete"),
  ]);
  await expect(rows(page).first().getByTestId("call-actions").getByRole("button")).toHaveText([
    en("calls.open_folder"),
  ]);
});

test("AC-3: an empty list shows 'No calls yet' with the hotkey from settings, and the unavailable-root line", async ({ page }) => {
  await openCalls(page, { list: callsEmptyList() });
  await expect(page.getByTestId("call-empty")).toHaveText(en("calls.empty", { hotkey: HOTKEY }));
  await expect(page.getByTestId("call-empty")).toContainText("No calls yet");
  await expect(page.getByTestId("call-root-unavailable")).toHaveText(en("calls.root_unavailable", { root: "E:\\Gone" }));
  await expect(rows(page)).toHaveCount(0);
});

test("AC-3: a non-empty list shows no empty-state line, and still shows the unavailable root", async ({ page }) => {
  await openCalls(page, { list: callsList(1) });
  await expect(rows(page)).toHaveCount(1);
  await expect(page.getByTestId("call-empty")).toHaveCount(0);
  await expect(page.getByTestId("call-root-unavailable")).toHaveText(en("calls.root_unavailable", { root: "E:\\Gone" }));
});

test("AC-4: the detail shows time, speaker colour and label, and text", async ({ page }) => {
  await openCalls(page);
  await openReadyDetail(page);
  expect((await calls(page, "calls_get")).map((c) => c.args)).toEqual([READY]);
  const lines = page.getByTestId("call-line");
  await expect(lines).toHaveCount(2);
  await expect(page.getByTestId("line-time")).toHaveText(["00:00", "00:01"]);
  await expect(page.getByTestId("line-speaker")).toHaveText(["Me", "Other party"]);
  await expect(page.getByTestId("line-text")).toHaveText(["Hello there", "Hi"]);
  const colours = await page
    .getByTestId("speaker-swatch")
    .evaluateAll((els) => els.map((el) => getComputedStyle(el).backgroundColor));
  expect(colours).toHaveLength(2);
  expect(colours[0]).not.toBe(colours[1]);
  for (const colour of colours) expect(colour).not.toBe("rgba(0, 0, 0, 0)");
});

test("AC-5: a search for 'contract' highlights the matches in <mark> and keeps every line", async ({ page }) => {
  await openCalls(page, { detail: callsDetailWithTexts(["We sign the Contract today", "Hi"]) });
  await openReadyDetail(page);
  await page.getByLabel(en("calls.search")).fill("contract");
  const first = page.getByTestId("call-line").first();
  await expect(first.locator("mark")).toHaveCount(1);
  await expect(first.locator("mark")).toHaveText("Contract"); // original case kept
  await expect(page.getByTestId("call-line")).toHaveCount(2);
  await expect(page.getByTestId("line-text")).toHaveText(["We sign the Contract today", "Hi"]);
  await expect(page.getByText(en("calls.no_matches"))).toHaveCount(0);
});

test("AC-5: an unmatched search shows 'No matches' and no marks", async ({ page }) => {
  await openCalls(page, { detail: callsDetailWithTexts(["We sign the Contract today", "Hi"]) });
  await openReadyDetail(page);
  await expect(page.getByText(en("calls.no_matches"))).toHaveCount(0);
  await page.getByLabel(en("calls.search")).fill("zzz");
  await expect(page.getByText(en("calls.no_matches"))).toBeVisible();
  await expect(page.locator("mark")).toHaveCount(0);
});

test("AC-5 D5: transcript text reaches the page as text, never as markup", async ({ page }) => {
  await openCalls(page, { detail: callsDetailWithTexts(["<b>bold</b> contract <img src=x onerror=alert(1)>", "Hi"]) });
  await openReadyDetail(page);
  await page.getByLabel(en("calls.search")).fill("contract");
  const first = page.getByTestId("call-line").first();
  await expect(first.getByTestId("line-text")).toHaveText("<b>bold</b> contract <img src=x onerror=alert(1)>");
  await expect(first.locator("b, img")).toHaveCount(0);
  await expect(first.locator("mark")).toHaveText("contract");
});

test("AC-6: Copy invokes calls_copy_markdown { root, id } of that row", async ({ page }) => {
  await openCalls(page);
  await readyRow(page).getByRole("button", { name: en("calls.copy") }).click();
  await expect.poll(async () => (await calls(page, "calls_copy_markdown")).map((c) => c.args)).toEqual([READY]);
  await settle(page);
  await expect(alert(page)).toHaveCount(0);
});

test("AC-6 failure branch: an export refused with ioFailed access_denied shows the access_denied text", async ({ page }) => {
  await openCalls(page);
  await expect(readyRow(page)).toBeVisible();
  await expect(alert(page)).toHaveCount(0); // nothing is shown before the refusal
  await queueCallsResult(page, "calls_export", { reject: callsError("ioFailed") });
  await readyRow(page).getByRole("button", { name: en("calls.export_md") }).click();
  await expect(alert(page)).toHaveText(en("calls.error.io.access_denied"));
  expect((await calls(page, "calls_export")).map((c) => c.args)).toEqual([{ ...READY, format: "md" }]);
});

test("AC-6 failure branch: a refused copy shows its reason too", async ({ page }) => {
  await openCalls(page);
  await expect(readyRow(page)).toBeVisible();
  await queueCallsResult(page, "calls_copy_markdown", { reject: { code: "ioFailed", reason: "disk_full" } });
  await readyRow(page).getByRole("button", { name: en("calls.copy") }).click();
  await expect(alert(page)).toHaveText(en("calls.error.io.disk_full"));
});

test("AC-6 D4: an unknown reason shows the 'other' text, not the raw word", async ({ page }) => {
  await openCalls(page);
  await expect(readyRow(page)).toBeVisible();
  await queueCallsResult(page, "calls_export", { reject: { code: "ioFailed", reason: "relative" } });
  await readyRow(page).getByRole("button", { name: en("calls.export_json") }).click();
  await expect(alert(page)).toHaveText(en("calls.error.io.other"));
  expect((await calls(page, "calls_export")).map((c) => c.args)).toEqual([{ ...READY, format: "json" }]);
});

test("AC-6 D4: a cancelled export (saved: false) shows nothing", async ({ page }) => {
  await openCalls(page);
  await expect(readyRow(page)).toBeVisible();
  await queueCallsResult(page, "calls_export", { outcome: { saved: false } });
  await readyRow(page).getByRole("button", { name: en("calls.export_md") }).click();
  await expect.poll(async () => (await calls(page, "calls_export")).length).toBe(1);
  await settle(page);
  await expect(alert(page)).toHaveCount(0);
});

test("AC-6 failure branch: a refused Open folder shows its reason", async ({ page }) => {
  await openCalls(page);
  await expect(readyRow(page)).toBeVisible();
  await expect(alert(page)).toHaveCount(0);
  await queueCallsResult(page, "calls_open_folder", { reject: { code: "ioFailed", reason: "missing" } });
  await readyRow(page).getByRole("button", { name: en("calls.open_folder") }).click();
  await expect(alert(page)).toHaveText(en("calls.error.io.missing"));
  expect((await calls(page, "calls_open_folder")).map((c) => c.args)).toEqual([READY]);
});

test("AC-4 failure branch: a refused calls_get shows its reason and no detail", async ({ page }) => {
  await openCalls(page);
  await expect(readyRow(page)).toBeVisible();
  await expect(alert(page)).toHaveCount(0);
  await queueCallsResult(page, "calls_get", { reject: { code: "ioFailed", reason: "access_denied" } });
  await readyRow(page).getByTestId("call-select").click();
  await expect(alert(page)).toHaveText(en("calls.error.io.access_denied"));
  await expect(page.getByTestId("call-detail")).toHaveCount(0);
});

test("AC-1 failure branch: a refused calls_list (refetch after calls://changed) shows its reason", async ({ page }) => {
  await openCalls(page);
  await expect(rows(page)).toHaveCount(12);
  await expect(alert(page)).toHaveCount(0);
  await expect.poll(() => listeners(page, "calls://changed")).toBeGreaterThan(0);
  await queueCallsResult(page, "calls_list", { reject: { code: "ioFailed", reason: "disk_full" } });
  await emit(page, "calls://changed", READY);
  await expect(alert(page)).toHaveText(en("calls.error.io.disk_full"));
});

test("AC-1 failure branch: after a refused calls_list, the next successful refetch clears the alert and shows the list", async ({ page }) => {
  await openCalls(page);
  await expect(rows(page)).toHaveCount(12);
  await expect.poll(() => listeners(page, "calls://changed")).toBeGreaterThan(0);
  await queueCallsResult(page, "calls_list", { reject: { code: "ioFailed", reason: "disk_full" } });
  await emit(page, "calls://changed", READY);
  await expect(alert(page)).toHaveText(en("calls.error.io.disk_full"));
  await setCallsList(page, callsList(2));
  await emit(page, "calls://changed", READY);
  await expect(rows(page)).toHaveCount(2);
  await expect(alert(page)).toHaveCount(0);
});

test("AC-4 failure branch: after a refused calls_get, a successful detail refetch (calls://changed) clears the alert and shows the detail", async ({ page }) => {
  await openCalls(page);
  await expect(readyRow(page)).toBeVisible();
  await expect.poll(() => listeners(page, "calls://changed")).toBeGreaterThan(0);
  await queueCallsResult(page, "calls_get", { reject: { code: "ioFailed", reason: "access_denied" } });
  await readyRow(page).getByTestId("call-select").click();
  await expect(alert(page)).toHaveText(en("calls.error.io.access_denied"));
  await emit(page, "calls://changed", READY);
  await expect(page.getByTestId("call-detail")).toBeVisible();
  await expect(alert(page)).toHaveCount(0);
});

test("a refused action's alert is not cleared by a later successful list refetch", async ({ page }) => {
  await openCalls(page);
  await expect(readyRow(page)).toBeVisible();
  await expect.poll(() => listeners(page, "calls://changed")).toBeGreaterThan(0);
  await queueCallsResult(page, "calls_copy_markdown", { reject: { code: "ioFailed", reason: "disk_full" } });
  await readyRow(page).getByRole("button", { name: en("calls.copy") }).click();
  await expect(alert(page)).toHaveText(en("calls.error.io.disk_full"));
  await setCallsList(page, callsList(2));
  await emit(page, "calls://changed", READY);
  await expect(rows(page)).toHaveCount(2);
  await expect(alert(page)).toHaveText(en("calls.error.io.disk_full"));
});

test("AC-6: Open folder invokes calls_open_folder { root, id } of the damaged row's own root", async ({ page }) => {
  await openCalls(page);
  const damaged = rows(page).filter({ hasText: en("calls.state.damaged") });
  await damaged.getByRole("button", { name: en("calls.open_folder") }).click();
  await expect
    .poll(async () => (await calls(page, "calls_open_folder")).map((c) => c.args))
    .toEqual([{ root: "D:\\Old calls", id: "2026-10-26_081507-2" }]);
});

test("AC-7: RU renders: damaged title as core wrote it, state, actions and the root line", async ({ page }) => {
  await openCalls(page, { lang: "ru", list: callsListRu() });
  const damaged = rows(page).filter({ hasText: ru("calls.state.damaged") });
  await expect(damaged).toHaveCount(1);
  await expect(damaged.getByTestId("call-title")).toHaveText("Звонок 26.10 08:15");
  await expect(damaged.getByTestId("call-actions").getByRole("button")).toHaveText([
    ru("calls.open_folder"),
    ru("calls.delete"),
  ]);
  await expect(rows(page).filter({ hasText: ru("calls.state.recovered") })).toHaveCount(1);
  await expect(page.getByTestId("call-reason").first()).toHaveText(ru("calls.failed.key"));
  await expect(page.getByTestId("call-root-unavailable")).toHaveText(ru("calls.root_unavailable", { root: "E:\\Gone" }));
});

test("AC-7: RU renders the empty list with the hotkey", async ({ page }) => {
  await openCalls(page, { lang: "ru", list: callsEmptyList() });
  await expect(page.getByTestId("call-empty")).toHaveText(ru("calls.empty", { hotkey: HOTKEY }));
});

test("AC-7: RU renders the detail search texts", async ({ page }) => {
  await openCalls(page, { lang: "ru", list: callsListRu() });
  await readyRow(page).getByTestId("call-select").click();
  await page.getByLabel(ru("calls.search")).fill("zzz");
  await expect(page.getByText(ru("calls.no_matches"))).toBeVisible();
});

test("D8: calls://changed refetches the list", async ({ page }) => {
  await openCalls(page);
  await expect(rows(page)).toHaveCount(12);
  await expect.poll(() => listeners(page, "calls://changed")).toBeGreaterThan(0);
  await setCallsList(page, callsList(2));
  await emit(page, "calls://changed", READY);
  await expect(rows(page)).toHaveCount(2);
});

test("D8: calls://changed for the open call refetches its detail", async ({ page }) => {
  await openCalls(page);
  await openReadyDetail(page);
  await expect.poll(() => listeners(page, "calls://changed")).toBeGreaterThan(0);
  await emit(page, "calls://changed", READY);
  await expect.poll(async () => (await calls(page, "calls_get")).length).toBe(2);
});

test("D8: calls://open selects that call", async ({ page }) => {
  await openCalls(page);
  await expect(rows(page)).toHaveCount(12);
  await expect.poll(() => listeners(page, "calls://open")).toBeGreaterThan(0);
  await emit(page, "calls://open", READY);
  await expect(page.getByTestId("call-detail")).toBeVisible();
  expect((await calls(page, "calls_get")).map((c) => c.args)).toEqual([READY]);
});

test("D8: settings://changed with another language re-renders and refetches the list", async ({ page }) => {
  await openCalls(page);
  await expect(rows(page).first().getByTestId("call-state")).toHaveText(en("calls.state.recording"));
  await expect.poll(() => listeners(page, "settings://changed")).toBeGreaterThan(0);
  const before = (await calls(page, "calls_list")).length;
  const view = firstRunView();
  view.settings.ui_language = "ru";
  view.settings.calls.hotkey = HOTKEY;
  await setCallsList(page, callsListRu());
  await emit(page, "settings://changed", view);
  await expect(rows(page).first().getByTestId("call-state")).toHaveText(ru("calls.state.recording"));
  await expect(rows(page).filter({ hasText: "Звонок 26.10 08:15" })).toHaveCount(1);
  expect((await calls(page, "calls_list")).length).toBeGreaterThan(before);
});
