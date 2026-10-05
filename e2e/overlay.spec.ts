// T-053 red e2e: the overlay page (spec 001 contracts/ipc.md overlay; FR-008, FR-028;
// T-053 Acceptance 1 and 2; analysis red-test table rows 5, 6, 7 and 9) over the one IPC
// mock (e2e/support/tauriMock.ts). Payloads are core's (e2e/fixtures/overlay-wire.json,
// pinned by overlay::tests::e2e_overlay_wire_fixture_matches_core): message text is
// rendered by Rust, nested mic_reason.* ids included. A test that needs another order
// overrides `seq` with a spread, never the fixture.
//
// Locator contract this spec adds (T-053 analysis, design 3):
// - the route `/overlay`, run in the window labelled `overlay`;
// - display only: the page calls `plugin:event|listen` for `overlay://state` and, once
//   that listener is registered, `overlay_ready`; it calls nothing else;
// - it shows the payload with the highest seq so far, from the events and the reply
//   alike. While that payload is recording, processing or a message the page renders
//   one element with test id `overlay` and `data-state="<kind>"`; before any payload
//   and for hidden it renders none, removed at once (no exit transition: the shell
//   destroys the window after hidden);
// - inside it, test id `overlay-text` holds exactly the text (decoration such as the
//   red dot carries no text): recording `t("overlay.recording", { elapsed: m:ss })`,
//   processing `t("overlay.processing")`, both in the payload's `lang`; a message the
//   payload's `text` as given;
// - no expiry timer: the elapsed m:ss is redrawn at least once a second (it may lag the
//   real elapsed time by up to a second, never lead it); nothing else changes without a
//   payload;
// - a rejected IPC call shows nothing of its own, never the rejection text, and raises
//   no page error (T-053 r1 #1): after a rejected listen the page invokes nothing more
//   (overlay_ready only once the listener is registered); after a rejected overlay_ready
//   the listener stays registered, so the shell's next change is shown.
//
// Time: Playwright's clock (@playwright/test 1.63.0; `Clock` in playwright-core
// types/types.d.ts): `install` before the navigation, `pauseAt` once the page has
// loaded (so waiting in an assertion does not move the page's time), `runFor` to
// advance it and fire every due timer.
import { readFileSync } from "node:fs";
import type { Page } from "@playwright/test";
import { expect, test } from "./support/boot";
import {
  calls,
  emitted,
  installTauriMock,
  listeners,
  OVERLAY_STATE_EVENT,
  overlayState,
  overlayWire,
  releaseListen,
  releaseOverlayReady,
  type MockOptions,
  type OverlayPayload,
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

/** The text of a message payload, as core rendered it. */
function messageText(payload: OverlayPayload): string {
  if (payload.state.kind !== "message") throw new Error(`payload seq ${payload.seq} is not a message`);
  return payload.state.text;
}

/** The page's clock starts here (any fixed instant: only differences matter). */
const T0 = Date.UTC(2026, 9, 5, 9, 0, 0);

/** Collects uncaught page errors from before the first navigation. */
function pageErrors(page: Page): Error[] {
  const errors: Error[] = [];
  page.on("pageerror", (error) => errors.push(error));
  return errors;
}

/** Installs the clock (flowing) and the mock for the window labelled overlay, then opens the page. */
async function openOverlay(page: Page, options: MockOptions = {}): Promise<void> {
  await page.clock.install({ time: T0 });
  await installTauriMock(page, { windowLabel: "overlay", ...options });
  await page.goto("/overlay");
}

/** The page listens to overlay://state and has invoked overlay_ready once. */
async function waitForReady(page: Page): Promise<void> {
  await expect
    .poll(() => listeners(page, OVERLAY_STATE_EVENT), { message: "the overlay page listens to overlay://state" })
    .toBeGreaterThan(0);
  await expect
    .poll(async () => (await calls(page, "overlay_ready")).length, { message: "the overlay page invokes overlay_ready" })
    .toBe(1);
}

/**
 * How far past the page's time `pauseClock` pauses. playwright-core 1.63.0 `pauseAt`
 * throws "Cannot fast-forward to the past" if the page's flowing time has passed the
 * target by then, so the margin is longer than a test may run (the 30 s default
 * timeout): the real time between reading the page's time and the pause cannot reach
 * it. The jump fires each pending timer at most once; every call comes before the
 * first payload, when the page has no timer of its own (its only one redraws a shown
 * recording).
 */
const PAUSE_AHEAD_MS = 60_000;

/** Stops the page's clock: from now on the page's time moves only by `runFor`. */
async function pauseClock(page: Page): Promise<void> {
  const now = await page.evaluate(() => Date.now());
  await page.clock.pauseAt(now + PAUSE_AHEAD_MS);
}

/**
 * Lets the page run every pending microtask and one task, without a timer (the
 * installed clock fakes timers and animation frames, not MessageChannel).
 */
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

function overlay(page: Page) {
  return page.getByTestId("overlay");
}

function overlayText(page: Page) {
  return overlay(page).getByTestId("overlay-text");
}

async function expectShows(page: Page, kind: "recording" | "processing" | "message", text: string): Promise<void> {
  await expect(overlay(page), "exactly one overlay element").toHaveCount(1);
  await expect(overlay(page)).toHaveAttribute("data-state", kind);
  await expect(overlayText(page)).toHaveText(text);
}

async function expectNothingShown(page: Page): Promise<void> {
  await expect(overlay(page)).toHaveCount(0);
}

/** The recording text shows one of `elapsed` (m:ss), with the clock paused. */
async function expectRecordingOneOf(page: Page, lang: Lang, elapsed: string[]): Promise<void> {
  await settle(page);
  await expect(overlay(page)).toHaveAttribute("data-state", "recording");
  const shown = ((await overlayText(page).textContent()) ?? "").replace(/\s+/g, " ").trim();
  const allowed = elapsed.map((value) => msg(lang, "overlay.recording", { elapsed: value }));
  expect(allowed, `the recording text "${shown}"`).toContain(shown);
}

// ---- Acceptance 1: states from overlay://state events ----------------------------------

test("shows recording with the elapsed m:ss, advancing with the clock, from an overlay://state event", async ({ page }) => {
  const errors = pageErrors(page);
  await openOverlay(page);
  await waitForReady(page);
  await pauseClock(page);
  await expectNothingShown(page);

  await overlayState(page, overlayWire("recording_en")); // seq 1, elapsedMs 61 000
  await expectShows(page, "recording", en("overlay.recording", { elapsed: "1:01" }));

  // 63 s since the start: redrawn at least once a second, never ahead.
  await page.clock.runFor(2_000);
  await expectRecordingOneOf(page, "en", ["1:02", "1:03"]);
  // 120 s since the start.
  await page.clock.runFor(57_000);
  await expectRecordingOneOf(page, "en", ["1:59", "2:00"]);
  expect(errors).toEqual([]);
});

test("shows processing, then a message, then nothing on hidden, from overlay://state events", async ({ page }) => {
  const errors = pageErrors(page);
  await openOverlay(page);
  await waitForReady(page);
  await pauseClock(page);

  await overlayState(page, overlayWire("processing_en")); // seq 2
  await expectShows(page, "processing", en("overlay.processing"));

  const message = overlayWire("message_no_speech_en"); // seq 3
  await overlayState(page, message);
  await expectShows(page, "message", messageText(message));

  await overlayState(page, overlayWire("hidden_en")); // seq 5
  await expectNothingShown(page);
  await expect(page.getByText(messageText(message))).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("renders its own recording and processing texts in the payload's lang", async ({ page }) => {
  // The UI derives no language (i18n.md): each payload carries the snapshot's ui_language.
  const errors = pageErrors(page);
  await openOverlay(page);
  await waitForReady(page);
  await pauseClock(page);

  await overlayState(page, overlayWire("recording_en")); // seq 1
  await expectShows(page, "recording", en("overlay.recording", { elapsed: "1:01" }));
  await overlayState(page, overlayWire("recording_ru")); // seq 6, elapsedMs 5 000
  await expectShows(page, "recording", ru("overlay.recording", { elapsed: "0:05" }));
  await overlayState(page, overlayWire("processing_ru")); // seq 7
  await expectShows(page, "processing", ru("overlay.processing"));
  expect(errors).toEqual([]);
});

// ---- Acceptance 1: states from the overlay_ready reply ---------------------------------

const REPLIES = [
  { key: "recording_en", kind: "recording", text: () => en("overlay.recording", { elapsed: "1:01" }) },
  { key: "processing_en", kind: "processing", text: () => en("overlay.processing") },
  { key: "message_no_speech_en", kind: "message", text: () => messageText(overlayWire("message_no_speech_en")) },
] as const;

for (const reply of REPLIES) {
  test(`shows ${reply.kind} from the overlay_ready reply`, async ({ page }) => {
    // A webview that loads after the state changed gets it from the reply alone (R-13);
    // a recording's reply carries the elapsed time so far.
    const errors = pageErrors(page);
    await openOverlay(page, { overlayReady: overlayWire(reply.key), holdOverlayReady: true });
    await waitForReady(page);
    await pauseClock(page);
    await expectNothingShown(page);

    await releaseOverlayReady(page);
    await expectShows(page, reply.kind, reply.text());
    expect(await emitted(page, OVERLAY_STATE_EVENT), "no event was involved").toEqual([]);
    expect(errors).toEqual([]);
  });
}

// ---- Acceptance 1: the reply and the events race (the seq rule) -------------------------

test("an older overlay_ready reply arriving after a newer overlay://state event is dropped", async ({ page }) => {
  const errors = pageErrors(page);
  const olderReply = overlayWire("processing_en"); // seq 2
  const newerEvent = overlayWire("message_microphone_access_denied_en"); // seq 4
  await openOverlay(page, { overlayReady: olderReply, holdOverlayReady: true });
  await waitForReady(page);

  await overlayState(page, newerEvent);
  await expectShows(page, "message", messageText(newerEvent));

  await releaseOverlayReady(page);
  await settle(page);
  await settle(page);
  await expectShows(page, "message", messageText(newerEvent));
  await expect(page.getByText(en("overlay.processing"))).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("a held overlay_ready reply newer than an earlier overlay://state event is applied", async ({ page }) => {
  // The other half of the seq rule: the reply is not dropped just because an event came first.
  const errors = pageErrors(page);
  const newerReply = overlayWire("message_microphone_access_denied_en"); // seq 4
  await openOverlay(page, { overlayReady: newerReply, holdOverlayReady: true });
  await waitForReady(page);

  await overlayState(page, overlayWire("processing_en")); // seq 2
  await expectShows(page, "processing", en("overlay.processing"));

  await releaseOverlayReady(page);
  await expectShows(page, "message", messageText(newerReply));
  expect(errors).toEqual([]);
});

test("the overlay://state listener is registered before overlay_ready is invoked", async ({ page }) => {
  // Otherwise a change emitted between the reply and the registration is lost.
  const errors = pageErrors(page);
  await openOverlay(page, { holdListen: [OVERLAY_STATE_EVENT], overlayReady: overlayWire("processing_en") });
  await expect
    .poll(
      async () =>
        (await calls(page, "plugin:event|listen")).some(
          (call) => (call.args as { event?: string }).event === OVERLAY_STATE_EVENT,
        ),
      { message: "the overlay page listens to overlay://state" },
    )
    .toBe(true);
  await settle(page);
  await settle(page);
  expect(await calls(page, "overlay_ready"), "overlay_ready while the listen is still pending").toEqual([]);

  await releaseListen(page);
  await expectShows(page, "processing", en("overlay.processing"));
  const order = (await calls(page)).map((call) =>
    call.cmd === "plugin:event|listen" ? `listen ${(call.args as { event?: string }).event}` : call.cmd,
  );
  expect(order).toEqual([`listen ${OVERLAY_STATE_EVENT}`, "overlay_ready"]);
  expect(errors).toEqual([]);
});

// ---- Failure branch: a rejected listen or reply shows nothing of its own (T-053 r1 #1) ----

/** The recorded calls in order, a listen shown with its event. */
async function callOrder(page: Page): Promise<string[]> {
  return (await calls(page)).map((call) =>
    call.cmd === "plugin:event|listen" ? `listen ${(call.args as { event?: string }).event}` : call.cmd,
  );
}

test("failure branch: a rejected overlay://state listen shows nothing, raises no page error and never invokes overlay_ready", async ({ page }) => {
  // ipc.md: overlay_ready only once the listener is registered. Without a listener a
  // reply would show a state nothing can update, so the page stops there. The reply is
  // one that would show, so invoking it anyway is visible too.
  const errors = pageErrors(page);
  await openOverlay(page, { rejectListen: [OVERLAY_STATE_EVENT], overlayReady: overlayWire("processing_en") });
  await expect
    .poll(async () => (await calls(page, "plugin:event|listen")).length, {
      message: "the overlay page tries to listen to overlay://state",
    })
    .toBe(1);
  await settle(page);
  await settle(page);

  await expectNothingShown(page);
  // The mock's rejection text (e2e/support/tauriMock.ts `rejectListen`).
  await expect(page.locator("body")).not.toContainText(`listen ${OVERLAY_STATE_EVENT} refused`);
  expect(await listeners(page, OVERLAY_STATE_EVENT)).toBe(0);
  expect(await callOrder(page), "no overlay_ready after a rejected listen").toEqual([`listen ${OVERLAY_STATE_EVENT}`]);
  expect(errors).toEqual([]);
});

test("failure branch: a rejected overlay_ready shows nothing of its own, never the rejection text, and a later overlay://state event is still shown", async ({ page }) => {
  const errors = pageErrors(page);
  const rejection = "canary-overlay-ready-rejection-0053";
  // Held, so the page is ready (listener registered, overlay_ready invoked) before the
  // reply rejects.
  await openOverlay(page, { overlayReady: { reject: rejection }, holdOverlayReady: true });
  await waitForReady(page);
  await releaseOverlayReady(page);
  await settle(page);
  await settle(page);

  await expectNothingShown(page);
  await expect(page.locator("body")).not.toContainText(rejection);

  // The listener stays registered: the shell's next changes are shown.
  expect(await listeners(page, OVERLAY_STATE_EVENT)).toBe(1);
  await overlayState(page, overlayWire("processing_en")); // seq 2
  await expectShows(page, "processing", en("overlay.processing"));
  const message = overlayWire("message_no_speech_en"); // seq 3
  await overlayState(page, message);
  await expectShows(page, "message", messageText(message));
  await expect(page.locator("body")).not.toContainText(rejection);

  // Display only: its listener and the one reply; no unlisten.
  expect(await callOrder(page)).toEqual([`listen ${OVERLAY_STATE_EVENT}`, "overlay_ready"]);
  expect(errors).toEqual([]);
});

// ---- Acceptance 2 (failure branch): a nested id shows as Rust rendered it -----------------

for (const lang of ["en", "ru"] as const) {
  test(`failure branch: a microphone message with a nested mic_reason id shows the full Rust-rendered text in ${lang}, never a raw id`, async ({ page }) => {
    const errors = pageErrors(page);
    const message = overlayWire(`message_microphone_access_denied_${lang}`);
    // The fixture holds the whole text core rendered, nested reason included, so this
    // test cannot pass on a fixture that carries an id.
    expect(message.state).toEqual({
      kind: "message",
      text: msg(lang, "failure.microphone_unavailable", { reason: msg(lang, "mic_reason.access_denied") }),
    });
    await openOverlay(page);
    await waitForReady(page);

    await overlayState(page, message);
    await expectShows(page, "message", messageText(message));
    const body = page.locator("body");
    await expect(body).not.toContainText("mic_reason.");
    await expect(body).not.toContainText("failure.");
    expect(errors).toEqual([]);
  });
}

// ---- Acceptance 2 (failure branch): no expiry timer in the page ---------------------------

test("failure branch: a message followed by hidden 1.2 s later is gone at 1.2 s (no UI timer)", async ({ page }) => {
  const errors = pageErrors(page);
  await openOverlay(page);
  await waitForReady(page);
  await pauseClock(page);

  const message = overlayWire("message_microphone_access_denied_en"); // seq 4
  await overlayState(page, message);
  await expectShows(page, "message", messageText(message));
  await page.clock.runFor(1_200);
  await settle(page);
  await expectShows(page, "message", messageText(message));

  // Core replaced the message 1.2 s in. The page's clock stays at 1.2 s while this
  // waits, so only the event can hide the message.
  await overlayState(page, overlayWire("hidden_en")); // seq 5
  await expectNothingShown(page);
  await expect(page.getByText(messageText(message))).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("failure branch: a message with no follow-up is still shown after 3 s (no UI timer)", async ({ page }) => {
  const errors = pageErrors(page);
  await openOverlay(page);
  await waitForReady(page);
  await pauseClock(page);

  const message = overlayWire("message_microphone_access_denied_en"); // seq 4
  await overlayState(page, message);
  await expectShows(page, "message", messageText(message));

  await page.clock.runFor(3_000);
  await settle(page);
  await expectShows(page, "message", messageText(message));
  await page.clock.runFor(57_000);
  await settle(page);
  await expectShows(page, "message", messageText(message));

  // Display only: nothing but its listener and the reply (no hide, close or destroy of its own).
  const commands = [...new Set((await calls(page)).map((call) => call.cmd))].sort();
  expect(commands).toEqual(["overlay_ready", "plugin:event|listen"]);
  expect(errors).toEqual([]);
});
