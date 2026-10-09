// Self-test of the one IPC mock (e2e/support/tauriMock.ts) against contracts/ipc.md,
// so a settings test fails on the UI, never on the mock. The page is driven the way
// @tauri-apps/api 2.12.1 does it: listen() = invoke("plugin:event|listen",
// { event, target, handler: transformCallback(cb) }), unlisten = unregisterListener +
// invoke("plugin:event|unlisten", { event, eventId }) (node_modules/@tauri-apps/api/event.js).
import type { Page } from "@playwright/test";
import { expect, test } from "./support/boot";
import {
  calls,
  emit,
  emitted,
  firstRunView,
  installTauriMock,
  failedState,
  failureReason,
  fakeMicrophones,
  holdLists,
  listeners,
  localModelProgress,
  localModelsFirstRun,
  localModelState,
  LOCAL_MODEL_EVENTS,
  modelsWith,
  OVERLAY_STATE_EVENT,
  overlayNothingYet,
  overlayState,
  overlayWire,
  queueDownloadRejection,
  queueSaveOutcome,
  releaseDownload,
  releaseList,
  releaseListen,
  releaseMicrophones,
  releaseOverlayReady,
  releaseSettingsGet,
  setBuildInfo,
  storedModels,
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

// T-039 r1 #3/#4 options. Each runs on a fresh page of the test's context, so only this
// test's mock is installed (the beforeEach mock is page-level, on the `page` fixture).

test("destroy: { reject } records plugin:window|destroy and rejects with its text", async ({ context }) => {
  const page = await context.newPage();
  await installTauriMock(page, { destroy: { reject: "destroy refused (fake)" } });
  await page.goto("/");
  expect(await invokeInPage(page, "plugin:window|destroy", { label: "settings" })).toEqual({
    err: "destroy refused (fake)",
  });
  expect(await calls(page, "plugin:window|destroy")).toEqual([
    { cmd: "plugin:window|destroy", args: { label: "settings" } },
  ]);
});

test("holdSettingsGet keeps every settings_get in flight (recorded) until releaseSettingsGet", async ({ context }) => {
  const page = await context.newPage();
  await installTauriMock(page, { holdSettingsGet: true });
  await page.goto("/");
  await page.evaluate(() => {
    const w = window as unknown as { __TAURI_INTERNALS__: Internals; __got?: unknown };
    void w.__TAURI_INTERNALS__.invoke("settings_get", {}).then((v) => (w.__got = v));
  });
  await expect.poll(async () => (await calls(page, "settings_get")).length).toBe(1);
  const pending = () => page.evaluate(() => (window as unknown as { __got?: unknown }).__got);
  expect(await pending()).toBeUndefined();
  // Not only the first one: a second call is held too (T-039 r2 Low 2, header fixed).
  await page.evaluate(() => {
    const w = window as unknown as { __TAURI_INTERNALS__: Internals; __got2?: unknown };
    void w.__TAURI_INTERNALS__.invoke("settings_get", {}).then((v) => (w.__got2 = v));
  });
  await expect.poll(async () => (await calls(page, "settings_get")).length).toBe(2);
  const second = () => page.evaluate(() => (window as unknown as { __got2?: unknown }).__got2);
  expect(await second()).toBeUndefined();
  await releaseSettingsGet(page);
  await expect.poll(pending).toEqual(firstRunView());
  await expect.poll(second).toEqual(firstRunView());
  // After the release, settings_get is answered at once.
  expect(await invokeInPage(page, "settings_get")).toEqual({ ok: firstRunView() });
});

test("rejectListen makes listen of the named event reject and register nothing; other events still listen", async ({ context }) => {
  const page = await context.newPage();
  await installTauriMock(page, { rejectListen: ["settings://focus"] });
  await page.goto("/");
  const result = await invokeInPage(page, "plugin:event|listen", {
    event: "settings://focus",
    target: { kind: "Any" },
    handler: 1,
  });
  expect(result).toEqual({ err: "listen settings://focus refused" });
  expect(await listeners(page, "settings://focus")).toBe(0);
  await listenInPage(page, "settings://changed");
  expect(await listeners(page, "settings://changed")).toBe(1);
});

test("holdListen keeps listen of the named event in flight and unregistered until releaseListen; other events listen at once", async ({ context }) => {
  // T-045 (settings-ui.md D): a test can act while the close guard's listen is pending.
  const page = await context.newPage();
  await installTauriMock(page, { holdListen: ["tauri://close-requested"] });
  await page.goto("/");
  await page.evaluate(() => {
    const w = window as unknown as { __TAURI_INTERNALS__: Internals; __heldId?: unknown };
    const handler = w.__TAURI_INTERNALS__.transformCallback(() => undefined);
    void w.__TAURI_INTERNALS__
      .invoke("plugin:event|listen", { event: "tauri://close-requested", target: { kind: "Any" }, handler })
      .then((id) => (w.__heldId = id));
  });
  await expect.poll(async () => (await calls(page, "plugin:event|listen")).length).toBe(1);
  const heldId = () => page.evaluate(() => (window as unknown as { __heldId?: unknown }).__heldId);
  expect(await heldId()).toBeUndefined();
  expect(await listeners(page, "tauri://close-requested")).toBe(0);
  // Another event is not held.
  await listenInPage(page, "settings://changed");
  expect(await listeners(page, "settings://changed")).toBe(1);

  await releaseListen(page);
  await expect.poll(heldId).toEqual(expect.any(Number));
  expect(await listeners(page, "tauri://close-requested")).toBe(1);
  // After the release, a listen of that event is answered at once.
  await listenInPage(page, "tauri://close-requested");
  expect(await listeners(page, "tauri://close-requested")).toBe(2);
});

// ---- Local models (spec 002 contracts/ipc.md, T-045) ----------------------------------

test("local_models_list serves core's first-run list by default, or the localModels option", async ({ context }) => {
  const page = await context.newPage();
  await installTauriMock(page);
  await page.goto("/");
  const list = (await invokeInPage(page, "local_models_list")) as { ok: unknown[] };
  expect(list.ok).toEqual(localModelsFirstRun());
  expect(list.ok).toHaveLength(5);
  expect(localModelsFirstRun().map((m) => m.id)).toEqual(["tiny", "base", "small", "medium-q5_0", "large-v3-turbo-q5_0"]);
  expect(localModelsFirstRun().filter((m) => m.recommended).map((m) => m.id)).toEqual(["small"]);

  const other = await context.newPage();
  const models = modelsWith("base", { kind: "downloaded" });
  await installTauriMock(other, { localModels: models });
  await other.goto("/");
  expect(await invokeInPage(other, "local_models_list")).toEqual({ ok: models });
});

test("local_model_download records the call, sets the row downloading {0, sizeBytes}, returns null and emits nothing", async ({ page }) => {
  await listenInPage(page, LOCAL_MODEL_EVENTS.progress);
  await listenInPage(page, LOCAL_MODEL_EVENTS.state);
  expect(await invokeInPage(page, "local_model_download", { id: "base" })).toEqual({ ok: null });
  expect((await calls(page, "local_model_download")).map((c) => c.args)).toEqual([{ id: "base" }]);
  const base = (await storedModels(page)).find((m) => m.id === "base")!;
  expect(base.state).toEqual({ kind: "downloading", received: 0, total: base.sizeBytes });
  expect(await emitted(page)).toEqual([]);
  // The mock never validates: a second download is not refused unless scripted (#38).
  expect(await invokeInPage(page, "local_model_download", { id: "small" })).toEqual({ ok: null });
});

test("a queued download rejection is thrown as is and changes nothing; the queue is consumed", async ({ page }) => {
  await listenInPage(page, LOCAL_MODEL_EVENTS.state);
  const refusal = failureReason("not_enough_disk_space");
  await queueDownloadRejection(page, refusal);
  await queueDownloadRejection(page, "not a FailureReason (fake)");
  const thrown = await page.evaluate(async () => {
    const w = window as unknown as { __TAURI_INTERNALS__: Internals };
    try {
      await w.__TAURI_INTERNALS__.invoke("local_model_download", { id: "base" });
      return "resolved";
    } catch (e) {
      return e;
    }
  });
  expect(thrown).toEqual(refusal);
  expect(await invokeInPage(page, "local_model_download", { id: "base" })).toEqual({ err: "\"not a FailureReason (fake)\"" });
  expect(await storedModels(page)).toEqual(localModelsFirstRun());
  expect(await emitted(page)).toEqual([]);
  expect(await invokeInPage(page, "local_model_download", { id: "base" })).toEqual({ ok: null });
  expect(await calls(page, "local_model_download")).toHaveLength(3);
});

test("local_model_cancel_download: a downloading row returns true and stays downloading until the download thread emits state not_downloaded, when the list changes; otherwise false", async ({ page }) => {
  // T-045 review r1 #8: core keeps Downloading until the download thread records the
  // cancel (service.rs `record`, Cancelled), then emits.
  expect(await invokeInPage(page, "local_model_cancel_download", { id: "base" })).toEqual({ ok: false });
  await invokeInPage(page, "local_model_download", { id: "base" });
  const base = localModelsFirstRun().find((m) => m.id === "base")!;
  const downloading = { kind: "downloading", received: 0, total: base.sizeBytes };

  // In one page task: the answer, the listed state right after it returns, and (from a
  // listener) the listed state when the event is delivered.
  const observed = await page.evaluate(async (stateEvent) => {
    type Mock = { state: { models: { id: string; state: unknown }[] } };
    const w = window as unknown as { __TAURI_INTERNALS__: Internals; __VOICEN_MOCK__: Mock; __atEmit?: unknown[] };
    const listed = () => JSON.parse(JSON.stringify(w.__VOICEN_MOCK__.state.models.find((m) => m.id === "base")!.state));
    w.__atEmit = [];
    const handler = w.__TAURI_INTERNALS__.transformCallback((e) =>
      w.__atEmit!.push({ payload: (e as { payload: unknown }).payload, listed: listed() }),
    );
    await w.__TAURI_INTERNALS__.invoke("plugin:event|listen", { event: stateEvent, target: { kind: "Any" }, handler });
    const answer = await w.__TAURI_INTERNALS__.invoke("local_model_cancel_download", { id: "base" });
    return { answer, afterReturn: listed(), emittedYet: w.__atEmit.length };
  }, LOCAL_MODEL_EVENTS.state);
  expect(observed).toEqual({ answer: true, afterReturn: downloading, emittedYet: 0 });

  const atEmit = () => page.evaluate(() => (window as unknown as { __atEmit: unknown[] }).__atEmit);
  await expect.poll(atEmit).toEqual([
    { payload: { id: "base", state: { kind: "not_downloaded" } }, listed: { kind: "not_downloaded" } },
  ]);
  expect((await storedModels(page)).find((m) => m.id === "base")!.state).toEqual({ kind: "not_downloaded" });
  expect(await invokeInPage(page, "local_model_cancel_download", { id: "base" })).toEqual({ ok: false });
  expect(await invokeInPage(page, "local_model_cancel_download", { id: "ggml-fake-unknown" })).toEqual({ ok: false });
  expect((await calls(page, "local_model_cancel_download")).map((c) => c.args)).toEqual([
    { id: "base" },
    { id: "base" },
    { id: "base" },
    { id: "ggml-fake-unknown" },
  ]);
});

test("localModelProgress and localModelState update the list first, then emit (a handler already sees the new state)", async ({ page }) => {
  // In-page listener that records, at delivery, the mock's row state of the event's id.
  await page.evaluate(async (names) => {
    type Mock = { state: { models: { id: string; state: unknown }[] } };
    const w = window as unknown as { __TAURI_INTERNALS__: Internals; __VOICEN_MOCK__: Mock; __seen?: unknown[] };
    w.__seen = [];
    for (const name of names) {
      const handler = w.__TAURI_INTERNALS__.transformCallback((e) => {
        const payload = (e as { payload: { id: string } }).payload;
        const row = w.__VOICEN_MOCK__.state.models.find((m) => m.id === payload.id);
        w.__seen!.push({ payload, listed: JSON.parse(JSON.stringify(row?.state)) });
      });
      await w.__TAURI_INTERNALS__.invoke("plugin:event|listen", { event: name, target: { kind: "Any" }, handler });
    }
  }, [LOCAL_MODEL_EVENTS.progress, LOCAL_MODEL_EVENTS.state]);
  const base = localModelsFirstRun().find((m) => m.id === "base")!;

  await localModelProgress(page, "base", 1234);
  await localModelState(page, "base", failedState("checksum_mismatch"));
  const seen = await page.evaluate(() => (window as unknown as { __seen: unknown[] }).__seen);
  const downloading = { kind: "downloading", received: 1234, total: base.sizeBytes };
  expect(seen).toEqual([
    { payload: { id: "base", received: 1234, total: base.sizeBytes }, listed: downloading },
    { payload: { id: "base", state: failedState("checksum_mismatch") }, listed: failedState("checksum_mismatch") },
  ]);
  expect((await emitted(page)).map((e) => e.event)).toEqual([LOCAL_MODEL_EVENTS.progress, LOCAL_MODEL_EVENTS.state]);
  expect(LOCAL_MODEL_EVENTS).toEqual({ progress: "local-model://progress", state: "local-model://state" });
});

test("holdList keeps local_models_list in flight until releaseList and answers with the list as it was when called", async ({ context }) => {
  const page = await context.newPage();
  await installTauriMock(page, { holdList: true });
  await page.goto("/");
  await page.evaluate(() => {
    const w = window as unknown as { __TAURI_INTERNALS__: Internals; __list?: unknown };
    void w.__TAURI_INTERNALS__.invoke("local_models_list", {}).then((v) => (w.__list = v));
  });
  await expect.poll(async () => (await calls(page, "local_models_list")).length).toBe(1);
  const answer = () => page.evaluate(() => (window as unknown as { __list?: unknown }).__list);
  expect(await answer()).toBeUndefined();

  await localModelState(page, "base", { kind: "downloaded" });
  await releaseList(page);
  // The stale snapshot (taken at the call), not the list after the event.
  await expect.poll(answer).toEqual(localModelsFirstRun());
  // After the release, the next list is answered at once, with the current list.
  expect(await invokeInPage(page, "local_models_list")).toEqual({ ok: modelsWith("base", { kind: "downloaded" }) });
});

test("listRejection makes local_models_list reject with its text (recorded)", async ({ context }) => {
  const page = await context.newPage();
  await installTauriMock(page, { listRejection: "list refused (fake)" });
  await page.goto("/");
  expect(await invokeInPage(page, "local_models_list")).toEqual({ err: "list refused (fake)" });
  expect(await calls(page, "local_models_list")).toHaveLength(1);
});

test("holdDownload keeps local_model_download in flight and not yet run (no row changes) until releaseDownload; later calls run at once", async ({ context }) => {
  // T-045 review r1 #2: a test can see the UI while a download invoke is pending.
  const page = await context.newPage();
  await installTauriMock(page, { holdDownload: true });
  await page.goto("/");
  await queueDownloadRejection(page, failureReason("download_busy"));
  await page.evaluate(() => {
    const w = window as unknown as { __TAURI_INTERNALS__: Internals; __first?: unknown; __second?: unknown };
    void w.__TAURI_INTERNALS__
      .invoke("local_model_download", { id: "base" })
      .then((v) => (w.__first = { ok: v }), (e) => (w.__first = { err: e }));
    void w.__TAURI_INTERNALS__
      .invoke("local_model_download", { id: "small" })
      .then((v) => (w.__second = { ok: v }), (e) => (w.__second = { err: e }));
  });
  await expect.poll(async () => (await calls(page, "local_model_download")).length).toBe(2);
  const answers = () =>
    page.evaluate(() => {
      const w = window as unknown as { __first?: unknown; __second?: unknown };
      return [w.__first ?? null, w.__second ?? null];
    });
  expect(await answers()).toEqual([null, null]);
  // Not run yet: no row changed, the scripted rejection is still queued.
  expect(await storedModels(page)).toEqual(localModelsFirstRun());

  await releaseDownload(page);
  // Run in call order: the first takes the scripted rejection, the second starts.
  await expect.poll(answers).toEqual([{ err: failureReason("download_busy") }, { ok: null }]);
  const models = await storedModels(page);
  expect(models.find((m) => m.id === "base")!.state).toEqual({ kind: "not_downloaded" });
  const small = models.find((m) => m.id === "small")!;
  expect(small.state).toEqual({ kind: "downloading", received: 0, total: small.sizeBytes });
  expect(await invokeInPage(page, "local_model_download", { id: "tiny" })).toEqual({ ok: null });
});

test("holdLists starts holding local_models_list mid-test, each answered with its call-time copy at releaseList", async ({ page }) => {
  expect(await invokeInPage(page, "local_models_list")).toEqual({ ok: localModelsFirstRun() });
  await holdLists(page);
  await page.evaluate(() => {
    const w = window as unknown as { __TAURI_INTERNALS__: Internals; __held?: unknown };
    void w.__TAURI_INTERNALS__.invoke("local_models_list", {}).then((v) => (w.__held = v));
  });
  await expect.poll(async () => (await calls(page, "local_models_list")).length).toBe(2);
  const held = () => page.evaluate(() => (window as unknown as { __held?: unknown }).__held);
  expect(await held()).toBeUndefined();
  await localModelState(page, "base", { kind: "downloaded" });
  await releaseList(page);
  await expect.poll(held).toEqual(localModelsFirstRun());
  expect(await invokeInPage(page, "local_models_list")).toEqual({ ok: modelsWith("base", { kind: "downloaded" }) });
});

// ---- Overlay (spec 001 contracts/ipc.md, T-053) ---------------------------------------

test("overlay_ready answers core's hidden payload numbered 0 by default; overlayState emits overlay://state", async ({ page }) => {
  expect(overlayNothingYet()).toEqual({ ...overlayWire("hidden_en"), seq: 0 });
  expect(await invokeInPage(page, "overlay_ready")).toEqual({ ok: overlayNothingYet() });
  expect(await calls(page, "overlay_ready")).toEqual([{ cmd: "overlay_ready", args: {} }]);

  const id = await listenInPage(page, OVERLAY_STATE_EVENT);
  const payload = overlayWire("message_microphone_access_denied_ru");
  await overlayState(page, payload);
  expect(await received(page)).toEqual([{ event: "overlay://state", id, payload }]);
  expect(await emitted(page, OVERLAY_STATE_EVENT)).toEqual([{ event: "overlay://state", payload, source: "test" }]);
});

test("overlay_ready answers the overlayReady option; windowLabel is the label tauri reports (default settings)", async ({ context, page }) => {
  const label = (p: Page) =>
    p.evaluate(() => {
      const w = window as unknown as {
        __TAURI_INTERNALS__: { metadata: { currentWindow: { label: string }; currentWebview: { windowLabel: string; label: string } } };
      };
      const { currentWindow, currentWebview } = w.__TAURI_INTERNALS__.metadata;
      return [currentWindow.label, currentWebview.windowLabel, currentWebview.label];
    });
  expect(await label(page)).toEqual(["settings", "settings", "settings"]);

  const overlayPage = await context.newPage();
  await installTauriMock(overlayPage, { overlayReady: overlayWire("recording_ru"), windowLabel: "overlay" });
  await overlayPage.goto("/");
  expect(await invokeInPage(overlayPage, "overlay_ready")).toEqual({ ok: overlayWire("recording_ru") });
  expect(await label(overlayPage)).toEqual(["overlay", "overlay", "overlay"]);
});

test("holdOverlayReady keeps overlay_ready in flight (recorded) until releaseOverlayReady; later calls are answered at once", async ({ context }) => {
  const page = await context.newPage();
  await installTauriMock(page, { overlayReady: overlayWire("processing_en"), holdOverlayReady: true });
  await page.goto("/");
  await page.evaluate(() => {
    const w = window as unknown as { __TAURI_INTERNALS__: Internals; __reply?: unknown };
    void w.__TAURI_INTERNALS__.invoke("overlay_ready", {}).then((v) => (w.__reply = v));
  });
  await expect.poll(async () => (await calls(page, "overlay_ready")).length).toBe(1);
  const reply = () => page.evaluate(() => (window as unknown as { __reply?: unknown }).__reply);
  expect(await reply()).toBeUndefined();

  await releaseOverlayReady(page);
  await expect.poll(reply).toEqual(overlayWire("processing_en"));
  expect(await invokeInPage(page, "overlay_ready")).toEqual({ ok: overlayWire("processing_en") });
});

test("overlayReady: { reject } records every overlay_ready and rejects it with its text, after the hold if there is one", async ({ context }) => {
  // T-053 r1 #1: the overlay page's rejected-reply branch.
  const page = await context.newPage();
  await installTauriMock(page, { overlayReady: { reject: "overlay_ready refused (fake)" } });
  await page.goto("/");
  expect(await invokeInPage(page, "overlay_ready")).toEqual({ err: "overlay_ready refused (fake)" });
  expect(await invokeInPage(page, "overlay_ready")).toEqual({ err: "overlay_ready refused (fake)" });
  expect(await calls(page, "overlay_ready")).toEqual([
    { cmd: "overlay_ready", args: {} },
    { cmd: "overlay_ready", args: {} },
  ]);

  const held = await context.newPage();
  await installTauriMock(held, { overlayReady: { reject: "overlay_ready refused (fake)" }, holdOverlayReady: true });
  await held.goto("/");
  await held.evaluate(() => {
    const w = window as unknown as { __TAURI_INTERNALS__: Internals; __answer?: unknown };
    void w.__TAURI_INTERNALS__.invoke("overlay_ready", {}).then(
      (v) => (w.__answer = { ok: v }),
      (e: unknown) => (w.__answer = { err: e instanceof Error ? e.message : String(e) }),
    );
  });
  await expect.poll(async () => (await calls(held, "overlay_ready")).length).toBe(1);
  const answer = () => held.evaluate(() => (window as unknown as { __answer?: unknown }).__answer);
  expect(await answer()).toBeUndefined();

  await releaseOverlayReady(held);
  await expect.poll(answer).toEqual({ err: "overlay_ready refused (fake)" });
});

// ---- Microphones (spec 004 contracts/ipc.md settings_list_microphones, T-012) -----------

test("settings_list_microphones serves fakeMicrophones by default (one default, not the first, unique ids), or the microphones option", async ({ context }) => {
  const page = await context.newPage();
  await installTauriMock(page);
  await page.goto("/");
  const list = (await invokeInPage(page, "settings_list_microphones")) as { ok: unknown[] };
  expect(list).toEqual({ ok: fakeMicrophones() });
  const mics = fakeMicrophones();
  expect(mics.filter((m) => m.is_default)).toHaveLength(1);
  expect(mics[0].is_default).toBe(false);
  expect(new Set(mics.map((m) => m.id)).size).toBe(mics.length);
  for (const m of mics) expect(Object.keys(m).sort()).toEqual(["id", "is_default", "name"]);
  expect(await calls(page, "settings_list_microphones")).toEqual([{ cmd: "settings_list_microphones", args: {} }]);

  const other = await context.newPage();
  await installTauriMock(other, { microphones: [] });
  await other.goto("/");
  expect(await invokeInPage(other, "settings_list_microphones")).toEqual({ ok: [] });
});

test("microphones: { reject } records every settings_list_microphones and rejects it with its text", async ({ context }) => {
  const page = await context.newPage();
  await installTauriMock(page, { microphones: { reject: "list_microphones refused (fake)" } });
  await page.goto("/");
  expect(await invokeInPage(page, "settings_list_microphones")).toEqual({ err: "list_microphones refused (fake)" });
  expect(await invokeInPage(page, "settings_list_microphones")).toEqual({ err: "list_microphones refused (fake)" });
  expect(await calls(page, "settings_list_microphones")).toHaveLength(2);
});

test("holdMicrophones keeps settings_list_microphones in flight (recorded) until releaseMicrophones; later calls are answered at once", async ({ context }) => {
  const page = await context.newPage();
  await installTauriMock(page, { holdMicrophones: true });
  await page.goto("/");
  await page.evaluate(() => {
    const w = window as unknown as { __TAURI_INTERNALS__: Internals; __mics?: unknown };
    void w.__TAURI_INTERNALS__.invoke("settings_list_microphones", {}).then((v) => (w.__mics = v));
  });
  await expect.poll(async () => (await calls(page, "settings_list_microphones")).length).toBe(1);
  const answer = () => page.evaluate(() => (window as unknown as { __mics?: unknown }).__mics);
  expect(await answer()).toBeUndefined();

  await releaseMicrophones(page);
  await expect.poll(answer).toEqual(fakeMicrophones());
  expect(await invokeInPage(page, "settings_list_microphones")).toEqual({ ok: fakeMicrophones() });
});

// T-023: the About dialog reads get_build_info on each opening; setBuildInfo changes the
// answer of every later call, a value or a rejection, in either order.
test("setBuildInfo replaces the get_build_info answer for every later call: a value, then a rejection, then a value", async ({ page }) => {
  // "/" reads get_build_info once in onMount; that call can land after goto() returns,
  // so wait for it before taking the baseline (else the count below is off by one).
  await expect.poll(async () => (await calls(page, "get_build_info")).length).toBe(1);
  const before = (await calls(page, "get_build_info")).length;
  expect(await invokeInPage(page, "get_build_info")).toEqual({ ok: { version: "0.0.0", commit: "mock" } });

  await setBuildInfo(page, { reject: "ipc down (fake)" });
  expect(await invokeInPage(page, "get_build_info")).toEqual({ err: "ipc down (fake)" });
  expect(await invokeInPage(page, "get_build_info")).toEqual({ err: "ipc down (fake)" });

  await setBuildInfo(page, { version: "9.8.7", commit: "def5678" });
  expect(await invokeInPage(page, "get_build_info")).toEqual({ ok: { version: "9.8.7", commit: "def5678" } });
  expect((await calls(page, "get_build_info")).length - before).toBe(4);
});

test("setBuildInfo answers get_build_info on a page installed without the buildInfo option", async ({ context }) => {
  const page = await context.newPage();
  await installTauriMock(page);
  await page.goto("/");
  expect(await invokeInPage(page, "get_build_info")).toEqual({ err: "unexpected command get_build_info" });
  await setBuildInfo(page, { version: "0.1.0", commit: "abc1234" });
  expect(await invokeInPage(page, "get_build_info")).toEqual({ ok: { version: "0.1.0", commit: "abc1234" } });
});
