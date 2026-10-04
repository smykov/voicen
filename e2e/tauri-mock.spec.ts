// Self-test of the one IPC mock (e2e/support/tauriMock.ts) against contracts/ipc.md,
// so a settings test fails on the UI, never on the mock. The page is driven the way
// @tauri-apps/api 2.12.1 does it: listen() = invoke("plugin:event|listen",
// { event, target, handler: transformCallback(cb) }), unlisten = unregisterListener +
// invoke("plugin:event|unlisten", { event, eventId }) (node_modules/@tauri-apps/api/event.js).
import { expect, test, type Page } from "@playwright/test";
import {
  calls,
  emit,
  emitted,
  firstRunView,
  installTauriMock,
  listeners,
  queueSaveOutcome,
  storedView,
  type SaveOutcome,
  type SaveRequest,
} from "./support/tauriMock";

type Internals = {
  invoke: (cmd: string, args?: unknown) => Promise<unknown>;
  transformCallback: (cb: (e: unknown) => void, once?: boolean) => number;
};

/** listen() as event.js does it; received events go to window.__received. */
async function listenInPage(page: Page, event: string): Promise<number> {
  return page.evaluate(async (name) => {
    const w = window as unknown as { __TAURI_INTERNALS__: Internals; __received?: unknown[] };
    w.__received = w.__received ?? [];
    const handler = w.__TAURI_INTERNALS__.transformCallback((e) => w.__received!.push(e));
    return (await w.__TAURI_INTERNALS__.invoke("plugin:event|listen", {
      event: name,
      target: { kind: "Any" },
      handler,
    })) as number;
  }, event);
}

async function received(page: Page): Promise<{ event: string; payload: unknown }[]> {
  return page.evaluate(() => (window as unknown as { __received?: { event: string; payload: unknown }[] }).__received ?? []);
}

async function invokeInPage(page: Page, cmd: string, args?: unknown): Promise<{ ok: unknown } | { err: string }> {
  return page.evaluate(
    async ([c, a]) => {
      try {
        const w = window as unknown as { __TAURI_INTERNALS__: Internals };
        return { ok: await w.__TAURI_INTERNALS__.invoke(c, a ?? {}) };
      } catch (e) {
        return { err: String(e instanceof Error ? e.message : JSON.stringify(e)) };
      }
    },
    [cmd, args] as const,
  );
}

function request(edits: SaveRequest["keys"]): SaveRequest {
  const settings = firstRunView().settings;
  settings.engine = "api";
  return { settings, keys: edits };
}

test.beforeEach(async ({ page }) => {
  await installTauriMock(page, { buildInfo: { version: "0.0.0", commit: "mock" } });
  await page.goto("/");
});

test("settings_get and settings_speech_languages serve core's fixture data", async ({ page }) => {
  expect(await invokeInPage(page, "settings_get")).toEqual({ ok: firstRunView() });
  const langs = (await invokeInPage(page, "settings_speech_languages")) as { ok: string[] };
  expect(langs.ok).toHaveLength(97);
  expect(langs.ok.slice(0, 3)).toEqual(["en", "zh", "de"]);
});

test("an unscripted settings_save is Saved with key presence only and is followed by settings://changed", async ({ page }) => {
  await listenInPage(page, "settings://changed");
  const req = request({ transcription_api: { Replace: "sk-test-FAKE-2222" }, local_server: "Clear", post_processing: "Untouched" });
  const result = (await invokeInPage(page, "settings_save", { request: req })) as { ok: SaveOutcome };

  const expectedView = { ...firstRunView(), settings: req.settings };
  expectedView.keys = { transcription_api: true, local_server: false, post_processing: false };
  expect(result.ok).toEqual({ Saved: { view: expectedView, warnings: [] } });
  expect(await storedView(page)).toEqual(expectedView);
  expect(JSON.stringify(await storedView(page))).not.toContain("sk-test-FAKE-2222");
  await expect.poll(async () => (await received(page)).map((e) => e.event)).toEqual(["settings://changed"]);
  expect((await received(page))[0].payload).toEqual(expectedView);
  expect((await calls(page, "settings_save")).map((c) => c.args)).toEqual([{ request: req }]);
});

test("a blank Replace keeps presence; the first_run, reset_notice and unavailable flags are kept", async ({ page }) => {
  const req = request({ transcription_api: { Replace: "   " }, local_server: "Untouched", post_processing: "Untouched" });
  const result = (await invokeInPage(page, "settings_save", { request: req })) as {
    ok: { Saved: { view: ReturnType<typeof firstRunView> } };
  };
  const view = result.ok.Saved.view;
  expect(view.keys).toEqual(firstRunView().keys);
  expect([view.first_run, view.reset_notice, view.unavailable]).toEqual([true, false, false]);
});

test("a scripted refusal is returned as is: nothing stored, no settings://changed", async ({ page }) => {
  await listenInPage(page, "settings://changed");
  const refused: SaveOutcome = {
    Refused: { errors: [{ field: "engine.api.base_url", code: "required" }], form_error: null },
  };
  await queueSaveOutcome(page, refused);
  const req = request({ transcription_api: "Untouched", local_server: "Untouched", post_processing: "Untouched" });
  expect(await invokeInPage(page, "settings_save", { request: req })).toEqual({ ok: refused });
  expect(await storedView(page)).toEqual(firstRunView());
  expect(await emitted(page, "settings://changed")).toEqual([]);
  // The queue is consumed: the next save is the unscripted Saved.
  const next = (await invokeInPage(page, "settings_save", { request: req })) as { ok: SaveOutcome };
  expect(Object.keys(next.ok)).toEqual(["Saved"]);
});

test("emit reaches listeners until they unlisten; other commands reject", async ({ page }) => {
  const id = await listenInPage(page, "settings://changed");
  expect(await listeners(page, "settings://changed")).toBe(1);
  await emit(page, "settings://changed", { probe: 1 });
  expect(await received(page)).toEqual([{ event: "settings://changed", id, payload: { probe: 1 } }]);

  await page.evaluate(async (eventId) => {
    const w = window as unknown as {
      __TAURI_INTERNALS__: Internals;
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: (e: string, i: number) => void };
    };
    w.__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener("settings://changed", eventId);
    await w.__TAURI_INTERNALS__.invoke("plugin:event|unlisten", { event: "settings://changed", eventId });
  }, id);
  expect(await listeners(page, "settings://changed")).toBe(0);
  await emit(page, "settings://changed", { probe: 2 });
  expect(await received(page)).toHaveLength(1);

  expect(await invokeInPage(page, "settings_test_connection", {})).toEqual({
    err: "unexpected command settings_test_connection",
  });
});

test("plugin:window|destroy is recorded and returns null; plugin:window|close rejects (not granted)", async ({ page }) => {
  // T-039: tauri's onCloseRequested destroys the window on every close it does not
  // prevent (window.js 2.12.1, onCloseRequested -> destroy -> plugin:window|destroy
  // { label }); close() is not in the settings capability, so the mock refuses it.
  expect(await invokeInPage(page, "plugin:window|destroy", { label: "settings" })).toEqual({ ok: null });
  expect(await calls(page, "plugin:window|destroy")).toEqual([
    { cmd: "plugin:window|destroy", args: { label: "settings" } },
  ]);
  expect(await invokeInPage(page, "plugin:window|close", { label: "settings" })).toEqual({
    err: "unexpected command plugin:window|close",
  });
});
