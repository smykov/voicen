// The one Tauri IPC mock of the e2e suite (spec 004 T013, T-004 Investigation 6).
//
// It implements contracts/ipc.md for the commands the UI uses today and nothing more:
//
// - `settings_get` returns the stored view; `settings_speech_languages` the list.
// - `settings_save { request }` records the call, then
//   - returns the next queued scripted outcome (`queueSaveOutcome`) and changes nothing,
//     or rejects with the next queued rejection (`queueSaveRejection`), or
//   - without one: returns `Saved { view, warnings: [] }`, where `view.settings` is
//     `request.settings` and key presence follows the KeyEdits (non-blank Replace ->
//     true, Clear -> false, Untouched or blank Replace -> unchanged); the flags
//     (first_run, reset_notice, unavailable) stay as they were, as in the service. It
//     stores that view and then emits `settings://changed` with it, as the shell's
//     bridge does (after the save returns, never on a refusal).
//   The mock never validates: a refusal is always scripted, so it holds no copy of core
//   rules (P-010, decision #38).
//   After `holdSaves()`, each `settings_save` stays in flight (recorded, not answered)
//   until `releaseSave()`, so a test can act while a save is pending (T-004 r1 #10).
// - `plugin:event|listen` / `plugin:event|unlisten` keep the handler ids registered by
//   `transformCallback`, so `listen()` from @tauri-apps/api works and `emit()` reaches it.
// - `get_build_info` answers for the build-info page.
// - Any other command rejects, so a call outside the contract fails the test.
//
// The init script must be self-contained (it is serialized into the page), so it cannot
// import @tauri-apps/api/mocks; its registry follows that module's mockIPC.
import { readFileSync } from "node:fs";
import type { Page } from "@playwright/test";

// ---- Wire types (contracts/ipc.md › Wire form; serde of voicen_core) -------------
// The one TS declaration of the wire is the window's own (src/lib/settings/settingsApi.ts);
// the mock re-exports it, so a wire change is made in one TS file (T-004 r1 #8).

import type { SaveOutcome, SettingsView } from "../../src/lib/settings/settingsApi";
export type {
  EngineKind,
  FieldError,
  FormError,
  KeyEdit,
  KeySlot,
  SaveOutcome,
  SaveRequest,
  Settings,
  SettingsView,
} from "../../src/lib/settings/settingsApi";

export interface BuildInfo {
  version: string;
  commit: string;
}

export interface RecordedCall {
  cmd: string;
  args: unknown;
}

export interface EmittedEvent {
  event: string;
  payload: unknown;
  /** `mock` = emitted by the mock after a Saved; `test` = by `emit()`. */
  source: "mock" | "test";
}

// ---- Core-checked data (e2e/fixtures/settings-wire.json, option A3) ---------------

interface WireFixture {
  first_run_view: SettingsView;
  speech_languages: string[];
}

const fixture = JSON.parse(
  readFileSync(new URL("../fixtures/settings-wire.json", import.meta.url), "utf8"),
) as WireFixture;

/** Core's first-run view: `defaults(None)`, no keys, `first_run: true` (a fresh copy). */
export function firstRunView(): SettingsView {
  return structuredClone(fixture.first_run_view);
}

/** Core's `WHISPER_ISO_639_1` in core order (a fresh copy). */
export function coreSpeechLanguages(): string[] {
  return [...fixture.speech_languages];
}

// ---- Install -------------------------------------------------------------------

export interface MockOptions {
  /** What `settings_get` returns first; default: `firstRunView()`. */
  view?: SettingsView;
  /** What `settings_speech_languages` returns; default: `coreSpeechLanguages()`. */
  speechLanguages?: string[];
  /** Outcomes returned by the next `settings_save` calls, in order. */
  saveOutcomes?: SaveOutcome[];
  /** `get_build_info`: a value, or `{ reject }` to make it reject with that text. */
  buildInfo?: BuildInfo | { reject: string };
}

interface InitArg {
  view: SettingsView;
  speechLanguages: string[];
  saveOutcomes: SaveOutcome[];
  buildInfo: BuildInfo | { reject: string } | null;
}

/** Installs the mock as an init script; call before `page.goto`. */
export async function installTauriMock(page: Page, options: MockOptions = {}): Promise<void> {
  const arg: InitArg = {
    view: options.view ?? firstRunView(),
    speechLanguages: options.speechLanguages ?? coreSpeechLanguages(),
    saveOutcomes: options.saveOutcomes ?? [],
    buildInfo: options.buildInfo ?? null,
  };
  await page.addInitScript((init: InitArg) => {
    type Handler = (data: unknown) => void;
    type Scripted = { outcome: unknown } | { reject: unknown };
    const clone = <T>(v: T): T => (v === undefined ? v : (JSON.parse(JSON.stringify(v)) as T));
    const SLOTS = ["transcription_api", "local_server", "post_processing"] as const;

    const state = {
      view: clone(init.view),
      speechLanguages: clone(init.speechLanguages),
      scripted: init.saveOutcomes.map((outcome) => ({ outcome }) as Scripted),
      calls: [] as { cmd: string; args: unknown }[],
      emitted: [] as { event: string; payload: unknown; source: "mock" | "test" }[],
      listeners: new Map<string, number[]>(),
      callbacks: new Map<number, Handler>(),
      nextId: 1,
      holding: false,
      held: [] as (() => void)[],
    };

    function transformCallback(callback?: Handler, once = false): number {
      const id = state.nextId++;
      state.callbacks.set(id, (data) => {
        if (once) state.callbacks.delete(id);
        if (callback) callback(data);
      });
      return id;
    }

    function unregisterCallback(id: number): void {
      state.callbacks.delete(id);
    }

    function emit(event: string, payload: unknown, source: "mock" | "test"): void {
      state.emitted.push({ event, payload: clone(payload), source });
      for (const id of [...(state.listeners.get(event) ?? [])]) {
        const cb = state.callbacks.get(id);
        if (cb) cb({ event, id, payload: clone(payload) });
      }
    }

    function presence(previous: boolean, edit: unknown): boolean {
      if (edit === "Clear") return false;
      if (edit !== null && typeof edit === "object" && "Replace" in edit) {
        const key = (edit as { Replace: unknown }).Replace;
        return typeof key === "string" && key.trim() !== "" ? true : previous;
      }
      return previous;
    }

    async function invoke(cmd: string, args: Record<string, unknown> = {}): Promise<unknown> {
      state.calls.push({ cmd, args: clone(args) });
      switch (cmd) {
        case "plugin:event|listen": {
          const event = args.event as string;
          const handler = args.handler as number;
          const list = state.listeners.get(event) ?? [];
          list.push(handler);
          state.listeners.set(event, list);
          return handler;
        }
        case "plugin:event|unlisten": {
          const list = state.listeners.get(args.event as string) ?? [];
          const at = list.indexOf(args.eventId as number);
          if (at !== -1) list.splice(at, 1);
          return null;
        }
        case "settings_get":
          return clone(state.view);
        case "settings_speech_languages":
          return clone(state.speechLanguages);
        case "settings_save": {
          if (state.holding) await new Promise<void>((resolve) => state.held.push(resolve));
          const next = state.scripted.shift();
          if (next && "reject" in next) throw clone(next.reject);
          if (next) return clone(next.outcome);
          const request = args.request as {
            settings: unknown;
            keys: Record<string, unknown>;
          };
          const keys = { ...state.view.keys };
          for (const slot of SLOTS) keys[slot] = presence(keys[slot], request.keys[slot]);
          const view = { ...clone(state.view), settings: clone(request.settings), keys };
          state.view = view as typeof state.view;
          // The bridge emits from another thread, after the save returned.
          setTimeout(() => emit("settings://changed", view, "mock"), 0);
          return { Saved: { view: clone(view), warnings: [] } };
        }
        case "get_build_info":
          if (init.buildInfo === null) break;
          if ("reject" in init.buildInfo) throw new Error(init.buildInfo.reject);
          return clone(init.buildInfo);
      }
      throw new Error(`unexpected command ${cmd}`);
    }

    const w = window as unknown as Record<string, unknown>;
    w.__TAURI_INTERNALS__ = {
      invoke,
      transformCallback,
      unregisterCallback,
      metadata: {
        currentWindow: { label: "settings" },
        currentWebview: { windowLabel: "settings", label: "settings" },
      },
    };
    w.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: (_event: string, id: number) => unregisterCallback(id),
    };
    w.__VOICEN_MOCK__ = {
      state,
      emit: (event: string, payload: unknown) => emit(event, payload, "test"),
      queue: (item: Scripted) => state.scripted.push(item),
      hold: () => {
        state.holding = true;
      },
      release: () => {
        state.holding = false;
        for (const resolve of state.held.splice(0)) resolve();
      },
    };
  }, arg);
}

// ---- Node-side helpers ---------------------------------------------------------
// Each runs in the page and reaches the mock through `window.__VOICEN_MOCK__` (set by
// the init script above); page.evaluate serializes the callback, so it uses no closure.

interface MockHandle {
  state: {
    view: SettingsView;
    calls: RecordedCall[];
    emitted: EmittedEvent[];
    listeners: Map<string, number[]>;
  };
  emit: (event: string, payload: unknown) => void;
  queue: (item: { outcome: unknown } | { reject: unknown }) => void;
  hold: () => void;
  release: () => void;
}

type MockWindow = { __VOICEN_MOCK__: MockHandle };

/** Every recorded invoke, or only those of `cmd`, in call order. */
export async function calls(page: Page, cmd?: string): Promise<RecordedCall[]> {
  const all = await page.evaluate(() => (window as unknown as MockWindow).__VOICEN_MOCK__.state.calls);
  return cmd === undefined ? all : all.filter((c) => c.cmd === cmd);
}

/** The view the mock holds now (what the next `settings_get` would return). */
export async function storedView(page: Page): Promise<SettingsView> {
  return page.evaluate(() => (window as unknown as MockWindow).__VOICEN_MOCK__.state.view);
}

/** Events emitted so far (by the mock after a Saved, or by `emit`), optionally of one name. */
export async function emitted(page: Page, event?: string): Promise<EmittedEvent[]> {
  const all = await page.evaluate(() => (window as unknown as MockWindow).__VOICEN_MOCK__.state.emitted);
  return event === undefined ? all : all.filter((e) => e.event === event);
}

/** How many handlers listen to `event` now (wait for > 0 before `emit`). */
export async function listeners(page: Page, event: string): Promise<number> {
  return page.evaluate(
    (name) => (window as unknown as MockWindow).__VOICEN_MOCK__.state.listeners.get(name)?.length ?? 0,
    event,
  );
}

/** Runs every handler registered for `event` with `{ event, id, payload }`, as the shell would. */
export async function emit(page: Page, event: string, payload: unknown): Promise<void> {
  await page.evaluate(
    ([name, data]) => (window as unknown as MockWindow).__VOICEN_MOCK__.emit(name, data),
    [event, payload] as const,
  );
}

/** The next `settings_save` returns `outcome` (and changes nothing). */
export async function queueSaveOutcome(page: Page, outcome: SaveOutcome): Promise<void> {
  await page.evaluate(
    (value) => (window as unknown as MockWindow).__VOICEN_MOCK__.queue({ outcome: value }),
    outcome,
  );
}

/** The next `settings_save` rejects with `payload` (contracts/ipc.md "Errors"). */
export async function queueSaveRejection(page: Page, payload: unknown): Promise<void> {
  await page.evaluate(
    (value) => (window as unknown as MockWindow).__VOICEN_MOCK__.queue({ reject: value }),
    payload,
  );
}

/** From now on each `settings_save` stays in flight until `releaseSave` (recorded at once). */
export async function holdSaves(page: Page): Promise<void> {
  await page.evaluate(() => (window as unknown as MockWindow).__VOICEN_MOCK__.hold());
}

/** Answers every held `settings_save` and stops holding. */
export async function releaseSave(page: Page): Promise<void> {
  await page.evaluate(() => (window as unknown as MockWindow).__VOICEN_MOCK__.release());
}
