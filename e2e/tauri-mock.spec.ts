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
  failedState,
  failureReason,
  listeners,
  localModelProgress,
  localModelsFirstRun,
  localModelState,
  LOCAL_MODEL_EVENTS,
  modelsWith,
  queueDownloadRejection,
  queueSaveOutcome,
  releaseList,
  releaseListen,
  releaseSettingsGet,
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

test("local_model_cancel_download: a downloading row returns true, becomes not_downloaded, then state not_downloaded is emitted; otherwise false", async ({ page }) => {
  await listenInPage(page, LOCAL_MODEL_EVENTS.state);
  expect(await invokeInPage(page, "local_model_cancel_download", { id: "base" })).toEqual({ ok: false });
  await invokeInPage(page, "local_model_download", { id: "base" });

  expect(await invokeInPage(page, "local_model_cancel_download", { id: "base" })).toEqual({ ok: true });
  expect((await storedModels(page)).find((m) => m.id === "base")!.state).toEqual({ kind: "not_downloaded" });
  await expect.poll(async () => (await received(page)).map((e) => e.payload)).toEqual([
    { id: "base", state: { kind: "not_downloaded" } },
  ]);
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
