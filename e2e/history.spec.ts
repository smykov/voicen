// T-022 red e2e: the History window (spec 005 tasks T009, T015, T019; contracts/ipc.md "UI
// contract"; T-022 Acceptance 1 and 2) over the one IPC mock (e2e/support/tauriMock.ts) plus
// the history commands (e2e/support/historyMock.ts).
//
// Locator contract this spec adds (the developer implements the page against it):
// - the route `/history`, run in the window labelled `history`; it calls `history_get` and
//   `settings_get` (language from `settings.ui_language`), listens to `history://changed`
//   (renamed from the contract's `history-changed`: every other app event is `scheme://name`)
//   and replaces its whole state with each payload (no merging);
// - root element, test id `history-page`, present as soon as the page has booted;
// - the list is `role=list` named "History" (catalog `history.list_label`), one `role=listitem`
//   per entry in view order (never re-sorted); a row shows the text preview, a time and the
//   engine label "API · whisper-1" / "Local · small" / "Server · <model>"; Copy is a
//   `role=button` named "Copy" inside the row; an expand toggle is a button with `aria-expanded`,
//   present only for a text longer than the preview (3 lines or 200 characters);
// - Copy -> `history_copy { id }`, then one `role=status` "Copied" (for 2 s); a rejected copy
//   (`clipboard_unavailable`, `not_found`) shows one `role=alert` "Could not copy — try again"
//   and nothing else (no transcript text, no code); a later successful copy clears it;
// - Clear is a `role=button` "Clear", no confirmation; it calls `history_clear` and shows the
//   returned view (the mock emits no event);
// - empty state: text "No transcripts yet" (enabled, no entries); off state: "History is off" and
//   a `role=button` "Open settings" that invokes `settings_open { tab: "history" }` (the shell's
//   open-settings request; OPEN_SETTINGS in historyMock.ts is the one place that names it), no
//   entries and no history_copy / history_clear call;
// - `notice` set: one `role=alert` "History could not be saved to disk" / "History could not be
//   deleted from disk" while the view carries it.
// Every English text is the contract's literal; Russian texts come from i18n/ru.json (`history.*`).
import { readFileSync } from "node:fs";
import type { Page } from "@playwright/test";
import { bootPage, expect, test } from "./support/boot";
import { emit, firstRunView, installTauriMock, listeners } from "./support/tauriMock";
import {
  entry,
  emptyView,
  HISTORY_CHANGED,
  historyCalls,
  installHistoryMock,
  offView,
  OPEN_SETTINGS,
  queueClearView,
  queueCopyRejection,
  viewAfter,
  type HistoryWireView,
} from "./support/historyMock";

type Lang = "en" | "ru";
const RU = JSON.parse(readFileSync(new URL("../i18n/ru.json", import.meta.url), "utf8")) as Record<string, string>;
function ru(id: string): string {
  const text = RU[id];
  if (typeof text !== "string" || text === "") throw new Error(`i18n/ru.json has no text for ${id}`);
  return text;
}

const EN = {
  list: "History",
  copy: "Copy",
  copied: "Copied",
  copyFailed: "Could not copy — try again",
  clear: "Clear",
  empty: "No transcripts yet",
  off: "History is off",
  openSettings: "Open settings",
  saveFailed: "History could not be saved to disk",
  deleteFailed: "History could not be deleted from disk",
};

async function openHistory(page: Page, view: HistoryWireView, lang: Lang = "en"): Promise<void> {
  const settings = firstRunView();
  settings.settings.ui_language = lang;
  await installTauriMock(page, { view: settings, windowLabel: "history" });
  await installHistoryMock(page, view);
  await bootPage(page, "/history", page.getByTestId("history-page"));
  // The page has subscribed once it listens; an emit before that would be lost.
  await expect.poll(() => listeners(page, HISTORY_CHANGED)).toBeGreaterThan(0);
}

const items = (page: Page) => page.getByRole("list", { name: EN.list }).getByRole("listitem");
const alerts = (page: Page) => page.getByRole("alert");
const copyOf = (row: ReturnType<typeof items>) => row.getByRole("button", { name: EN.copy, exact: true });

test("AC-1: 25 dictations with N = 20 show the 20 newest, newest first, and nothing older", async ({ page }) => {
  const view = viewAfter(25, 20); // what core's view holds: ids 25..6
  expect(view.entries).toHaveLength(20);
  await openHistory(page, view);
  await expect(items(page)).toHaveCount(20);
  for (const [i, e] of view.entries.entries()) {
    await expect(items(page).nth(i)).toContainText(e.text);
  }
  await expect(items(page).first()).toContainText("Entry text 0025");
  await expect(items(page).last()).toContainText("Entry text 0006");
  await expect(page.getByText("Entry text 0005", { exact: false })).toHaveCount(0);
  expect((await historyCalls(page, "history_get")).length).toBeGreaterThanOrEqual(1);
});

test("AC-1: the rows follow the view order verbatim, not the clock (no client sort)", async ({ page }) => {
  const view: HistoryWireView = {
    enabled: true,
    notice: null,
    // insertion order is the order; the clock went back for the newest (data-model: ready_at_ms is display only)
    entries: [entry(3, { readyAtMs: 1_000 }), entry(2, { readyAtMs: 9_000_000_000_000 }), entry(1)],
  };
  await openHistory(page, view);
  await expect(items(page)).toHaveCount(3);
  await expect(items(page).nth(0)).toContainText("Entry text 0003");
  await expect(items(page).nth(1)).toContainText("Entry text 0002");
  await expect(items(page).nth(2)).toContainText("Entry text 0001");
});

test("AC-1: each row shows a time and the engine label of its kind", async ({ page }) => {
  const view: HistoryWireView = {
    enabled: true,
    notice: null,
    entries: [
      entry(3, { engine: { kind: "server", model: "llama-fake" } }),
      entry(2, { engine: { kind: "local", model: "small" } }),
      entry(1, { engine: { kind: "api", model: "whisper-1" } }),
    ],
  };
  await openHistory(page, view);
  await expect(items(page)).toHaveCount(3);
  await expect(items(page).nth(0)).toContainText("Server · llama-fake");
  await expect(items(page).nth(1)).toContainText("Local · small");
  await expect(items(page).nth(2)).toContainText("API · whisper-1");
  for (let i = 0; i < 3; i += 1) await expect(items(page).nth(i)).toContainText(/\d{1,2}:\d{2}/);
});

test("AC-1: a long text shows a 3-line preview with an expand toggle; a short one has none", async ({ page }) => {
  const long = entry(2, { text: "line one\nline two\nline three\nline four (fake)\nline five (fake)" });
  const short = entry(1);
  await openHistory(page, { enabled: true, notice: null, entries: [long, short] });
  await expect(items(page)).toHaveCount(2);
  const toggle = items(page).nth(0).getByRole("button", { expanded: false });
  await expect(toggle).toHaveCount(1);
  await expect(items(page).nth(0)).toContainText("line three");
  await expect(items(page).nth(0)).not.toContainText("line four");
  await expect(items(page).nth(1).getByRole("button", { expanded: false })).toHaveCount(0);
  await expect(items(page).nth(1).getByRole("button", { expanded: true })).toHaveCount(0);
  await toggle.click();
  await expect(items(page).nth(0).getByRole("button", { expanded: true })).toHaveCount(1);
  await expect(items(page).nth(0)).toContainText("line four (fake)");
  await expect(items(page).nth(0)).toContainText("line five (fake)");
});

test("AC-1: Copy invokes history_copy with that entry's id and shows Copied, which goes away", async ({ page }) => {
  await openHistory(page, viewAfter(25, 20));
  await expect(items(page)).toHaveCount(20);
  await copyOf(items(page).nth(1)).click(); // the second row: id 24, not the first
  await expect(page.getByRole("status")).toHaveText(EN.copied);
  expect(await historyCalls(page, "history_copy")).toEqual([{ cmd: "history_copy", args: { id: 24 } }]);
  await expect(alerts(page)).toHaveCount(0);
  await expect(page.getByRole("status")).toBeHidden({ timeout: 8_000 }); // 2 s
  await copyOf(items(page).first()).click();
  await expect.poll(async () => (await historyCalls(page, "history_copy")).length).toBe(2);
  expect((await historyCalls(page, "history_copy"))[1].args).toEqual({ id: 25 });
});

test("AC-1: Copy is reachable by keyboard (Enter on the focused button)", async ({ page }) => {
  await openHistory(page, viewAfter(3, 20));
  await copyOf(items(page).first()).focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("status")).toHaveText(EN.copied);
  expect(await historyCalls(page, "history_copy")).toEqual([{ cmd: "history_copy", args: { id: 3 } }]);
});

for (const code of ["clipboard_unavailable", "not_found"]) {
  test(`AC-2 failure: history_copy rejecting ${code} shows one alert with no transcript text; the list is unchanged`, async ({
    page,
  }) => {
    const view = viewAfter(25, 20);
    await openHistory(page, view);
    await expect(items(page)).toHaveCount(20);
    await queueCopyRejection(page, { code }); // fails exactly the next call
    await copyOf(items(page).first()).click();
    await expect(alerts(page)).toHaveCount(1);
    await expect(alerts(page)).toHaveText(EN.copyFailed);
    for (const e of view.entries) await expect(alerts(page)).not.toContainText(e.text);
    await expect(alerts(page)).not.toContainText(code);
    await expect(page.getByRole("status")).toHaveCount(0);
    await expect(items(page)).toHaveCount(20);
    await expect(items(page).first()).toContainText("Entry text 0025");
    // The rejection was one shot: copying again works and the alert is gone.
    await copyOf(items(page).nth(2)).click();
    await expect(page.getByRole("status")).toHaveText(EN.copied);
    await expect(alerts(page)).toHaveCount(0);
    expect((await historyCalls(page, "history_copy")).map((c) => c.args)).toEqual([{ id: 25 }, { id: 23 }]);
  });
}

test("AC-2: Clear (no confirmation) invokes history_clear and shows the empty state with no entries", async ({
  page,
}) => {
  let dialogs = 0;
  page.on("dialog", (d) => {
    dialogs += 1;
    void d.dismiss();
  });
  await openHistory(page, viewAfter(25, 20));
  await expect(items(page)).toHaveCount(20);
  await page.getByRole("button", { name: EN.clear, exact: true }).click();
  await expect(page.getByText(EN.empty)).toBeVisible();
  await expect(items(page)).toHaveCount(0);
  expect(await historyCalls(page, "history_clear")).toHaveLength(1);
  expect(dialogs).toBe(0);
  await expect(alerts(page)).toHaveCount(0);
});

test("AC-2 failure: a clear whose disk deletion failed shows the delete alert until a later view without a notice", async ({
  page,
}) => {
  await openHistory(page, viewAfter(5, 20));
  await queueClearView(page, { enabled: true, entries: [], notice: "delete_failed" });
  await page.getByRole("button", { name: EN.clear, exact: true }).click();
  await expect(items(page)).toHaveCount(0);
  await expect(alerts(page)).toHaveCount(1);
  await expect(alerts(page)).toHaveText(EN.deleteFailed);
  await emit(page, HISTORY_CHANGED, { ...emptyView(), entries: [entry(6)] }); // a later change, retried and clean
  await expect(items(page)).toHaveCount(1);
  await expect(alerts(page)).toHaveCount(0);
});

test("AC-2 failure: a view with notice save_failed keeps the entries and shows the save alert", async ({ page }) => {
  await openHistory(page, viewAfter(3, 20, "save_failed"));
  await expect(items(page)).toHaveCount(3);
  await expect(alerts(page)).toHaveCount(1);
  await expect(alerts(page)).toHaveText(EN.saveFailed);
  await emit(page, HISTORY_CHANGED, viewAfter(4, 20));
  await expect(items(page)).toHaveCount(4);
  await expect(alerts(page)).toHaveCount(0);
});

test("AC-2: an enabled history with no entries shows 'No transcripts yet' and no list items", async ({ page }) => {
  await openHistory(page, emptyView());
  await expect(page.getByText(EN.empty)).toBeVisible();
  await expect(items(page)).toHaveCount(0);
  await expect(page.getByText(EN.off)).toHaveCount(0);
});

test("AC-2: history off shows 'History is off' and no entries; Open settings opens the History tab", async ({
  page,
}) => {
  await openHistory(page, offView());
  await expect(page.getByText(EN.off)).toBeVisible();
  await expect(items(page)).toHaveCount(0);
  await expect(page.getByText(EN.empty)).toHaveCount(0);
  await expect(page.getByRole("button", { name: EN.copy })).toHaveCount(0);
  await page.getByRole("button", { name: EN.openSettings, exact: true }).click();
  await expect.poll(async () => (await historyCalls(page, OPEN_SETTINGS)).length).toBe(1);
  expect((await historyCalls(page, OPEN_SETTINGS))[0].args).toEqual({ tab: "history" });
  // Nothing but the one read: the off window asks for no copy and no clear.
  expect((await historyCalls(page)).map((c) => c.cmd).filter((c) => c !== OPEN_SETTINGS)).toEqual(["history_get"]);
});

test("AC-2: a history://changed with enabled false switches an open window to the off state at once, and back", async ({
  page,
}) => {
  await openHistory(page, viewAfter(5, 20));
  await expect(items(page)).toHaveCount(5);
  await emit(page, HISTORY_CHANGED, offView());
  await expect(page.getByText(EN.off)).toBeVisible();
  await expect(items(page)).toHaveCount(0);
  await expect(page.getByText("Entry text 0005", { exact: false })).toHaveCount(0);
  await emit(page, HISTORY_CHANGED, { ...emptyView(), entries: [entry(9)] });
  await expect(page.getByText(EN.off)).toHaveCount(0);
  await expect(items(page)).toHaveCount(1);
  await expect(items(page).first()).toContainText("Entry text 0009");
});

test("AC-1: a history://changed puts a new entry on top, drops the oldest at N, and replaces the state (no merge)", async ({
  page,
}) => {
  await openHistory(page, viewAfter(20, 20));
  await expect(items(page)).toHaveCount(20);
  await emit(page, HISTORY_CHANGED, viewAfter(21, 20));
  await expect(items(page).first()).toContainText("Entry text 0021");
  await expect(items(page)).toHaveCount(20);
  await expect(items(page).last()).toContainText("Entry text 0002");
  await expect(page.getByText("Entry text 0001", { exact: false })).toHaveCount(0);
  // A smaller payload is the whole new state: the rest disappears, nothing is merged back.
  await emit(page, HISTORY_CHANGED, viewAfter(21, 2));
  await expect(items(page)).toHaveCount(2);
  await expect(items(page).first()).toContainText("Entry text 0021");
  await expect(items(page).last()).toContainText("Entry text 0020");
  await emit(page, HISTORY_CHANGED, emptyView());
  await expect(page.getByText(EN.empty)).toBeVisible();
  await expect(items(page)).toHaveCount(0);
});

test("AC-2: with the UI language ru every state shows the Russian catalog texts", async ({ page }) => {
  await openHistory(page, viewAfter(3, 20), "ru");
  const list = page.getByRole("list", { name: ru("history.list_label") });
  await expect(list.getByRole("listitem")).toHaveCount(3);
  await expect(list.getByRole("listitem").first().getByRole("button", { name: ru("history.copy"), exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: ru("history.clear"), exact: true })).toBeVisible();
  await expect(page.getByText(EN.copy, { exact: true })).toHaveCount(0);
  await list.getByRole("listitem").first().getByRole("button", { name: ru("history.copy"), exact: true }).click();
  await expect(page.getByRole("status")).toHaveText(ru("history.copied"));
  await emit(page, HISTORY_CHANGED, viewAfter(3, 20, "save_failed"));
  await expect(alerts(page)).toHaveText(ru("history.notice.save_failed"));
  await emit(page, HISTORY_CHANGED, { ...emptyView(), notice: "delete_failed" });
  await expect(page.getByText(ru("history.empty"))).toBeVisible();
  await expect(alerts(page)).toHaveText(ru("history.notice.delete_failed"));
  await emit(page, HISTORY_CHANGED, offView());
  await expect(page.getByText(ru("history.off"))).toBeVisible();
  await expect(page.getByRole("button", { name: ru("history.open_settings"), exact: true })).toBeVisible();
  await emit(page, HISTORY_CHANGED, viewAfter(3, 20));
  await queueCopyRejection(page, { code: "clipboard_unavailable" });
  await list.getByRole("listitem").nth(1).getByRole("button", { name: ru("history.copy"), exact: true }).click();
  await expect(alerts(page)).toHaveText(ru("history.copy_failed"));
});
