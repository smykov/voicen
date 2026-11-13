// T-131 red e2e: the call-indicator page (specs/007-call-mode-record contracts/ipc.md
// "UI components"; FR-005, FR-006, FR-046; T-131 Acceptance 1 to 5) over the one IPC mock
// (e2e/support/tauriMock.ts). Payloads are core's (e2e/fixtures/call-indicator-wire.json,
// pinned by core's e2e_call_indicator_wire_fixture_matches_core): texts of the reminder, the
// headphones hint and every warning are rendered by Rust and shown as given. A test that needs
// another warning list derives it from a fixture value with a spread, never a hand-written wire.
//
// Locator contract this spec adds (T-131 analysis, invariants I1 to I5):
// - the route `/call-indicator`, run in the window labelled `call-indicator`;
// - the page registers its `indicator://state` and `indicator://tick` listeners first and only
//   then invokes `call_indicator_state` (I3); it invokes no settings command, listens to no
//   `settings://changed`, and never invokes `call_indicator_moved` (I4);
// - the page shows the newest state the shell sent: events and the reply alike, but a reply
//   that arrives after an event is dropped (I3); it hides nothing by itself (no timer for the
//   10 s reminder, no local hiding of a dismissed warning or kind; I1);
// - texts: the recording label `t("indicator.recording")`, the Stop button
//   `t("indicator.stop")`, each warning's button `t("indicator.dismiss")`, all in the language
//   that came with the state (`lang`, I4); the reminder text, the headphones hint and each
//   warning's text are the state's, as given;
// - the elapsed time is test id `indicator-timer`, "MM:SS" as the Calls page shows a duration
//   (src/lib/calls/view.ts `formatDuration`), from the last tick's `elapsedMs`;
// - the two level bars are elements with role `meter` or `progressbar`, named
//   `t("speaker.me")` (mic) and `t("speaker.other")` (system), with aria-valuemin 0,
//   aria-valuemax 1 and aria-valuenow = the tick's micLevel / systemLevel;
// - the warnings are the `listitem`s of the element with test id `indicator-warnings`, in the
//   state's order, each holding its text and one Dismiss button;
// - the drag region is an element with `data-tauri-drag-region`; it holds no control (I5);
//   every interactive element is a <button> with an accessible name.
//
// Time: Playwright's clock (`page.clock`, as e2e/overlay.spec.ts) for the no-UI-timer test.
import { readFileSync } from "node:fs";
import type { Locator, Page } from "@playwright/test";
import { bootPage, expect, test } from "./support/boot";
import {
  calls,
  indicatorRepliesAnswered,
  indicatorState,
  indicatorTick,
  indicatorTickWire,
  indicatorWire,
  INDICATOR_STATE_EVENT,
  INDICATOR_TICK_EVENT,
  installTauriMock,
  listeners,
  releaseIndicatorState,
  releaseListen,
  type IndicatorState,
  type MockOptions,
} from "./support/tauriMock";

type Lang = "en" | "ru";
type Messages = Record<string, string>;
const CATALOG: Record<Lang, Messages> = {
  en: JSON.parse(readFileSync(new URL("../i18n/en.json", import.meta.url), "utf8")) as Messages,
  ru: JSON.parse(readFileSync(new URL("../i18n/ru.json", import.meta.url), "utf8")) as Messages,
};

/** The catalog text of `id` in `lang`; a missing text fails here. */
function msg(lang: Lang, id: string): string {
  const text = CATALOG[lang][id];
  if (typeof text !== "string" || text === "") throw new Error(`i18n/${lang}.json has no text for ${id}`);
  return text;
}

/** Collects uncaught page errors from before the first navigation. */
function pageErrors(page: Page): Error[] {
  const errors: Error[] = [];
  page.on("pageerror", (error) => errors.push(error));
  return errors;
}

/**
 * Opens the indicator in the window labelled `call-indicator`. The page has booted once it
 * registered both listeners and asked for the state (the idle state may show nothing, so the
 * probe is the recorded IPC, not an element).
 */
async function openIndicator(page: Page, options: MockOptions = {}, withClock = false): Promise<void> {
  if (withClock) await page.clock.install({ time: Date.UTC(2026, 10, 13, 9, 0, 0) });
  await installTauriMock(page, { windowLabel: "call-indicator", ...options });
  await bootPage(page, "/call-indicator", async () => {
    const events = (await calls(page, "plugin:event|listen")).map((c) => (c.args as { event?: string }).event);
    return (
      events.includes(INDICATOR_STATE_EVENT) &&
      events.includes(INDICATOR_TICK_EVENT) &&
      (await calls(page, "call_indicator_state")).length > 0
    );
  });
}

/** Lets the page run every pending microtask and one task (no timer: the page clock may be faked). */
async function settle(page: Page): Promise<void> {
  await page.evaluate(
    () =>
      new Promise<void>((resolve) => {
        const channel = new MessageChannel();
        channel.port1.onmessage = () => resolve();
        channel.port2.postMessage(null);
      }),
  );
}

const recordingLabel = (page: Page, lang: Lang = "en") => page.getByText(msg(lang, "indicator.recording"), { exact: true });
const stopButton = (page: Page, lang: Lang = "en") => page.getByRole("button", { name: msg(lang, "indicator.stop"), exact: true });
const timer = (page: Page) => page.getByTestId("indicator-timer");
const warningItems = (page: Page) => page.getByTestId("indicator-warnings").getByRole("listitem");
const dismissIn = (item: Locator, lang: Lang = "en") => item.getByRole("button", { name: msg(lang, "indicator.dismiss"), exact: true });

/** The level bar named `name`: a meter or a progressbar. */
function levelBar(page: Page, name: string): Locator {
  return page.getByRole("meter", { name, exact: true }).or(page.getByRole("progressbar", { name, exact: true }));
}

/** The warning item whose text is `text`. */
function warningWith(page: Page, text: string): Locator {
  return warningItems(page).filter({ hasText: text });
}

function warningText(state: IndicatorState, id: number): string {
  const found = state.warnings.find((w) => w.id === id);
  if (found === undefined) throw new Error(`the fixture state has no warning ${id}`);
  return found.text;
}

test.describe("call indicator: recording, timer, levels (Acceptance 1)", () => {
  test("recording shows the label, the timer from a tick and two level bars with their values", async ({ page }) => {
    const state = indicatorWire("recording_no_reminder_en");
    await openIndicator(page, { indicatorState: state });
    await expect(recordingLabel(page)).toBeVisible();

    await indicatorTick(page, indicatorTickWire("tick_levels"));
    await expect(timer(page)).toHaveText("01:01");
    const me = levelBar(page, msg("en", "speaker.me"));
    const other = levelBar(page, msg("en", "speaker.other"));
    await expect(me).toHaveAttribute("aria-valuenow", "0.5");
    await expect(other).toHaveAttribute("aria-valuenow", "0.25");
    for (const bar of [me, other]) {
      await expect(bar).toHaveAttribute("aria-valuemin", "0");
      await expect(bar).toHaveAttribute("aria-valuemax", "1");
    }

    // The newest tick wins: the page keeps no maximum and no old value.
    await indicatorTick(page, indicatorTickWire("tick_silent"));
    await expect(timer(page)).toHaveText("00:00");
    await expect(me).toHaveAttribute("aria-valuenow", "0");
    await expect(other).toHaveAttribute("aria-valuenow", "0");
  });

  test("a state event shows the recording label as well (the reply is not the only source)", async ({ page }) => {
    await openIndicator(page);
    await indicatorState(page, indicatorWire("recording_no_reminder_en"));
    await expect(recordingLabel(page)).toBeVisible();
  });
});

test.describe("call indicator: reminder and headphones hint (Acceptance 2)", () => {
  test("the reminder text and the headphones hint show while reminderVisible and are gone when the next state has none", async ({ page }) => {
    const withReminder = indicatorWire("recording_reminder_en");
    await openIndicator(page, { indicatorState: withReminder });
    await expect(page.getByText(withReminder.reminderText, { exact: true })).toBeVisible();
    await expect(page.getByText(withReminder.headphonesHint, { exact: true })).toBeVisible();

    await indicatorState(page, indicatorWire("recording_no_reminder_en"));
    await expect(page.getByText(withReminder.reminderText, { exact: true })).toHaveCount(0);
    await expect(page.getByText(withReminder.headphonesHint, { exact: true })).toHaveCount(0);
    await expect(recordingLabel(page)).toBeVisible();
  });

  test("failure branch: no UI timer hides the reminder, only a state does (the 10 s is core's)", async ({ page }) => {
    const withReminder = indicatorWire("recording_reminder_en");
    await openIndicator(page, { indicatorState: withReminder }, true);
    await expect(page.getByText(withReminder.reminderText, { exact: true })).toBeVisible();

    await page.clock.runFor(30_000);
    await expect(page.getByText(withReminder.reminderText, { exact: true })).toBeVisible();
    await expect(page.getByText(withReminder.headphonesHint, { exact: true })).toBeVisible();

    await indicatorState(page, indicatorWire("recording_no_reminder_en"));
    await expect(page.getByText(withReminder.reminderText, { exact: true })).toHaveCount(0);
  });
});

test.describe("call indicator: Stop (Acceptance 3)", () => {
  test("clicking Stop invokes call_indicator_stop once, with no args, and nothing else of the contract", async ({ page }) => {
    await openIndicator(page, { indicatorState: indicatorWire("recording_no_reminder_en") });
    await expect(recordingLabel(page)).toBeVisible();
    expect(await calls(page, "call_indicator_stop")).toEqual([]);

    await stopButton(page).click();
    await expect.poll(async () => (await calls(page, "call_indicator_stop")).length).toBe(1);
    expect((await calls(page, "call_indicator_stop"))[0].args).toEqual({});
    expect(await calls(page, "call_indicator_dismiss")).toEqual([]);
    expect(await calls(page, "call_indicator_moved")).toEqual([]);
    // Stopping is the shell's: the page keeps showing the state it was given until the next one.
    await expect(recordingLabel(page)).toBeVisible();
  });
});

test.describe("call indicator: warnings (Acceptance 4)", () => {
  test("warnings_en lists the eight warnings in the state's id order, texts as given, each with a Dismiss button", async ({ page }) => {
    const state = indicatorWire("warnings_en");
    await openIndicator(page, { indicatorState: state });
    await expect(warningItems(page)).toHaveCount(8);
    for (const [i, warning] of state.warnings.entries()) {
      expect(warning.id).toBe(i + 1);
      const item = warningItems(page).nth(i);
      await expect(item).toContainText(warning.text);
      await expect(dismissIn(item)).toHaveCount(1);
    }
  });

  test("Dismiss on warning 2 invokes call_indicator_dismiss { warningId: 2 } and the warning stays until a state omits it", async ({ page }) => {
    const state = indicatorWire("warnings_en");
    const lostText = warningText(state, 2);
    await openIndicator(page, { indicatorState: state });
    await expect(warningItems(page)).toHaveCount(8);

    await dismissIn(warningWith(page, lostText)).click();
    await expect.poll(async () => (await calls(page, "call_indicator_dismiss")).map((c) => c.args)).toEqual([{ warningId: 2 }]);
    await settle(page);
    // No local hiding: the shell has not published a state without it yet.
    await expect(warningItems(page)).toHaveCount(8);
    await expect(warningWith(page, lostText)).toHaveCount(1);

    // The shell's republish without id 2: it leaves the list, the other seven stay in order.
    const without = { ...state, warnings: state.warnings.filter((w) => w.id !== 2) };
    await indicatorState(page, without);
    await expect(warningItems(page)).toHaveCount(7);
    await expect(warningWith(page, lostText)).toHaveCount(0);
    for (const [i, warning] of without.warnings.entries()) {
      await expect(warningItems(page).nth(i)).toContainText(warning.text);
    }
  });

  test("a dismissed kind re-arms: a later state with a new trackLost id shows it again, and it can be dismissed again", async ({ page }) => {
    const state = indicatorWire("warnings_en");
    const lost = state.warnings.find((w) => w.id === 2)!;
    await openIndicator(page, { indicatorState: state });
    await expect(warningItems(page)).toHaveCount(8);

    await dismissIn(warningWith(page, lost.text)).click();
    await expect.poll(async () => (await calls(page, "call_indicator_dismiss")).length).toBe(1);
    await indicatorState(page, { ...state, warnings: state.warnings.filter((w) => w.id !== 2) });
    await expect(warningWith(page, lost.text)).toHaveCount(0);
    // Id 3 is the other trackLost (system); the kind is still on screen through it, so the page
    // must not drop a new trackLost warning because "trackLost" was dismissed before.

    const rearmed = { ...state, warnings: [...state.warnings.filter((w) => w.id !== 2), { ...lost, id: 9 }] };
    await indicatorState(page, rearmed);
    await expect(warningItems(page)).toHaveCount(8);
    await expect(warningWith(page, lost.text)).toHaveCount(1);

    await dismissIn(warningWith(page, lost.text)).click();
    await expect.poll(async () => (await calls(page, "call_indicator_dismiss")).map((c) => c.args)).toEqual([
      { warningId: 2 },
      { warningId: 9 },
    ]);
    await settle(page);
    await expect(warningWith(page, lost.text)).toHaveCount(1);
  });

  test("a warning stays listed across ticks and a state that still has it (no expiry)", async ({ page }) => {
    const state = indicatorWire("warnings_en");
    await openIndicator(page, { indicatorState: state }, true);
    await expect(warningItems(page)).toHaveCount(8);
    await page.clock.runFor(60_000);
    await indicatorTick(page, indicatorTickWire("tick_levels"));
    await indicatorState(page, state);
    await expect(warningItems(page)).toHaveCount(8);
  });
});

test.describe("call indicator: drag region and buttons (Acceptance 5)", () => {
  test("there is a drag region holding no control, and every interactive element is a named button", async ({ page }) => {
    await openIndicator(page, { indicatorState: indicatorWire("warnings_en") });
    await expect(warningItems(page)).toHaveCount(8);

    const region = page.locator("[data-tauri-drag-region]");
    await expect(region.first()).toBeVisible();
    await expect(page.locator("[data-tauri-drag-region] button, button[data-tauri-drag-region]")).toHaveCount(0);
    await expect(page.locator("[data-tauri-drag-region] [tabindex], [data-tauri-drag-region] a[href], [data-tauri-drag-region] input")).toHaveCount(0);

    // Stop + eight Dismiss, each with a non-empty accessible name.
    const buttons = page.getByRole("button");
    await expect(buttons).toHaveCount(9);
    for (const button of await buttons.all()) {
      await expect(button).toHaveAccessibleName(/\S/);
    }
    // No other kind of control: links, inputs, selects, role=button on a non-button.
    await expect(page.locator("a[href], input, select, textarea, [role=button]:not(button)")).toHaveCount(0);
  });
});

test.describe("call indicator: language comes with the state (I4, Acceptance 5 RU)", () => {
  test("RU state renders RU page labels without any settings read, then a later EN state switches back", async ({ page }) => {
    const ruState = indicatorWire("recording_reminder_ru");
    await openIndicator(page, { indicatorState: ruState });
    await expect(recordingLabel(page, "ru")).toBeVisible();
    await expect(stopButton(page, "ru")).toBeVisible();
    await expect(page.getByText(ruState.reminderText, { exact: true })).toBeVisible();
    await expect(page.getByText(ruState.headphonesHint, { exact: true })).toBeVisible();

    await indicatorTick(page, indicatorTickWire("tick_levels"));
    await expect(levelBar(page, msg("ru", "speaker.me"))).toHaveAttribute("aria-valuenow", "0.5");
    await expect(levelBar(page, msg("ru", "speaker.other"))).toHaveAttribute("aria-valuenow", "0.25");

    await indicatorState(page, indicatorWire("warnings_ru"));
    await expect(warningItems(page)).toHaveCount(8);
    await expect(dismissIn(warningItems(page).first(), "ru")).toBeVisible();
    await expect(page.getByRole("button", { name: msg("ru", "indicator.dismiss"), exact: true })).toHaveCount(8);

    // The language moves with the state: EN again, nothing of RU left.
    await indicatorState(page, indicatorWire("recording_reminder_en"));
    await expect(recordingLabel(page, "en")).toBeVisible();
    await expect(stopButton(page, "en")).toBeVisible();
    await expect(recordingLabel(page, "ru")).toHaveCount(0);
    await expect(stopButton(page, "ru")).toHaveCount(0);

    // The page never asked the settings for a language.
    expect(await calls(page, "settings_get")).toEqual([]);
    expect(await listeners(page, "settings://changed")).toBe(0);
  });

  test.describe("OS locale is not a language source", () => {
    test.use({ locale: "ru-RU" });

    test("an EN state renders EN labels in a ru-RU browser", async ({ page }) => {
      await openIndicator(page, { indicatorState: indicatorWire("recording_reminder_en") });
      await expect(recordingLabel(page, "en")).toBeVisible();
      await expect(stopButton(page, "en")).toBeVisible();
      expect(await calls(page, "settings_get")).toEqual([]);
    });
  });
});

test.describe("call indicator: reply versus event (I3, failure branches)", () => {
  test("the listeners are registered before call_indicator_state is invoked", async ({ page }) => {
    await installTauriMock(page, {
      windowLabel: "call-indicator",
      indicatorState: indicatorWire("recording_no_reminder_en"),
      holdListen: [INDICATOR_STATE_EVENT],
    });
    // Booted once the held state listen is recorded (it is in flight, not registered).
    await bootPage(page, "/call-indicator", async () =>
      (await calls(page, "plugin:event|listen")).some((c) => (c.args as { event?: string }).event === INDICATOR_STATE_EVENT),
    );
    await settle(page);
    // The state listener is still pending: the reply must not have been asked for yet.
    expect(await calls(page, "call_indicator_state")).toEqual([]);

    await releaseListen(page);
    await expect.poll(async () => (await calls(page, "call_indicator_state")).length).toBe(1);
    await expect(recordingLabel(page)).toBeVisible();
  });

  test("a state event that arrives before the reply wins: the stale reply is dropped", async ({ page }) => {
    const stale = indicatorWire("warnings_en");
    const newer = { ...stale, warnings: stale.warnings.filter((w) => w.id !== 2) };
    await openIndicator(page, { indicatorState: stale, holdIndicatorState: true });

    // The user dismissed warning 2; the shell's republish reached the page before the reply did.
    await indicatorState(page, newer);
    await expect(warningItems(page)).toHaveCount(7);

    await releaseIndicatorState(page);
    await expect.poll(() => indicatorRepliesAnswered(page)).toBe(1);
    await settle(page);
    await settle(page);
    await expect(warningItems(page)).toHaveCount(7);
    await expect(warningWith(page, warningText(stale, 2))).toHaveCount(0);
  });

  test("a reply with no event before it is applied (the late-loading webview misses nothing)", async ({ page }) => {
    await openIndicator(page, { indicatorState: indicatorWire("warnings_en"), holdIndicatorState: true });
    await settle(page);
    await expect(warningItems(page)).toHaveCount(0);
    await releaseIndicatorState(page);
    await expect(warningItems(page)).toHaveCount(8);
  });

  test("a rejected call_indicator_state shows nothing of its own, raises no page error, and later events still show", async ({ page }) => {
    const errors = pageErrors(page);
    await openIndicator(page, { indicatorState: { reject: "shell refused (fake)" } });
    await expect.poll(() => indicatorRepliesAnswered(page)).toBe(1);
    await settle(page);
    await expect(page.getByText("shell refused (fake)")).toHaveCount(0);
    await expect(recordingLabel(page)).toHaveCount(0);

    await indicatorState(page, indicatorWire("recording_no_reminder_en"));
    await expect(recordingLabel(page)).toBeVisible();
    expect(errors.map((e) => e.message)).toEqual([]);
  });
});
