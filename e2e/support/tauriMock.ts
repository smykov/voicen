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
// - `plugin:window|destroy { label }` is recorded and returns null: the settings window's
//   capability grants destroy (src-tauri/capabilities/default.json), and tauri's
//   onCloseRequested calls it on every close it does not prevent (T-039).
//   `plugin:window|close` is not granted and rejects like any other command.
//   With `destroy: { reject }` destroy is recorded and then rejects with that text (the
//   shell refused or failed to destroy the window; T-039 r1 #3).
// - Failure and timing options (T-039 r1 #4): `holdSettingsGet` keeps every
//   `settings_get` in flight (recorded, not answered) until `releaseSettingsGet()`, and
//   the calls after the release are answered at once;
//   `rejectListen` names events whose `plugin:event|listen` rejects (nothing registered);
//   `holdListen` (T-045, rule D) names events whose `plugin:event|listen` stays in flight
//   (recorded, no handler registered) until `releaseListen()`, which registers the held
//   handlers and then answers them; later listens of those events are answered at once.
// - Local models (spec 002 contracts/ipc.md, T-045). The mock keeps one models list
//   (option `localModels`, default: core's `list_first_run` of
//   e2e/fixtures/local-models-wire.json):
//   - `local_models_list` returns a copy of it, taken when the call is made. With
//     `holdList`, every call stays in flight (recorded, its copy already taken) until
//     `releaseList()`, so a test can change the list or emit events before the (then
//     stale) response arrives; `holdLists(page)` starts the same holding mid-test (for a
//     re-list after a download); with `listRejection` it rejects with that text;
//   - `local_model_download { id }` records the call, then rejects with the next queued
//     `FailureReason` (`queueDownloadRejection`; any payload, for a non-contract
//     rejection too) and changes nothing, or sets the row to `downloading
//     { received: 0, total: sizeBytes }` and returns null. It emits nothing (the command
//     emits nothing; progress and the end state come from the download thread, which a
//     test plays with `localModelProgress` / `localModelState`). The mock never
//     validates (busy, already downloaded, disk space): a refusal is always scripted
//     (decision #38). With `holdDownload`, every call is recorded at once and stays in
//     flight until `releaseDownload()`; only then does it run (the scripted rejection or
//     the state change above), so while it is held no row has changed (a pending invoke);
//     the calls after the release run at once;
//   - `local_model_cancel_download { id }` returns true for a downloading row and false
//     otherwise. As in core, the row stays `downloading` when the command returns: the
//     download thread records the cancel later, so the listed state becomes
//     `not_downloaded` only at the `local-model://state { not_downloaded }` emit, which
//     comes after the command returned (T-045 review r1 #8);
//   - `localModelProgress(page, id, received)` and `localModelState(page, id, state)`
//     update the list first, then emit, as core does (so a list after an event agrees
//     with it).
// - Overlay (spec 001 contracts/ipc.md, T-053). Payloads are core's
//   (e2e/fixtures/overlay-wire.json, `overlayWire(key)`):
//   - `overlay_ready` returns the `overlayReady` option (default: core's hidden payload
//     numbered 0, older than every fixture payload, so it hides nothing a test emits).
//     The reply is taken when the call is made; with `holdOverlayReady` every call stays
//     in flight (recorded) until `releaseOverlayReady()`, so a test can emit a newer
//     event before the (then older) reply arrives; later calls are answered at once.
//     With `overlayReady: { reject }` every call is recorded and then rejects with that
//     text (the shell could not answer; T-053 r1 #1), after the hold if there is one;
//   - `overlayState(page, payload)` emits `overlay://state` (`OVERLAY_STATE_EVENT`) as
//     the shell does;
//   - `windowLabel` is the label tauri reports for the current window (default
//     `settings`; the overlay page runs in the window labelled `overlay`).
// - Microphones (spec 004 contracts/ipc.md `settings_list_microphones`, T-012):
//   `settings_list_microphones` returns a copy of the `microphones` option (default:
//   `fakeMicrophones()`, three fake endpoints, the second one the Windows default), taken
//   when the call is made. With `microphones: { reject }` every call is recorded and then
//   rejects with that text (the command could not run). With `holdMicrophones`, every
//   call stays in flight (recorded, its copy already taken) until `releaseMicrophones()`;
//   later calls are answered at once. The list is adapter data (cpal's enumeration), not
//   a core-derived value, so it is hand-written fake data, not a core-checked fixture.
// - Any other command rejects, so a call outside the contract fails the test.
//
// The init script must be self-contained (it is serialized into the page), so it cannot
// import @tauri-apps/api/mocks; its registry follows that module's mockIPC.
import { readFileSync } from "node:fs";
import type { Page } from "@playwright/test";

// ---- Wire types (contracts/ipc.md › Wire form; serde of voicen_core) -------------
// The one TS declaration of the wire is the window's own (src/lib/settings/settingsApi.ts);
// the mock re-exports it, so a wire change is made in one TS file (T-004 r1 #8).

import type { InputDevice, SaveOutcome, SettingsView } from "../../src/lib/settings/settingsApi";
import type { FailureReason, LocalModelView, ModelState } from "../../src/lib/local-models/localModelsApi";
import type { OverlayPayload } from "../../src/lib/overlay/overlayApi";
export type {
  EngineKind,
  FieldError,
  FormError,
  InputDevice,
  KeyEdit,
  KeySlot,
  SaveOutcome,
  SaveRequest,
  Settings,
  SettingsView,
  Warning,
} from "../../src/lib/settings/settingsApi";
// The local-model wire (spec 002) is declared once, in the window's localModelsApi.ts.
export type {
  FailureReason,
  LocalModelProgress,
  LocalModelStateChange,
  LocalModelView,
  ModelId,
  ModelState,
} from "../../src/lib/local-models/localModelsApi";
// The overlay wire (spec 001) is declared once, in the overlay page's overlayApi.ts.
export type { OverlayPayload, OverlayView } from "../../src/lib/overlay/overlayApi";

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
  /** Core's Saved outcome of an engine-api save with base URL http://example.com/v1 (T-015). */
  saved_insecure_api: SaveOutcome;
  /**
   * Core's Refused outcome of a save, over the first run, with post_processing enabled and
   * base_url, model and prompt "" (T-021).
   */
  refused_post_processing_on_empty: SaveOutcome;
}

const fixture = JSON.parse(
  readFileSync(new URL("../fixtures/settings-wire.json", import.meta.url), "utf8"),
) as WireFixture;

/** Core's first-run view: `defaults(None)`, no keys, `first_run: true` (a fresh copy). */
export function firstRunView(): SettingsView {
  return structuredClone(fixture.first_run_view);
}

/**
 * Core's `SaveOutcome` for a save, over the first run, of engine api with base URL
 * `http://example.com/v1` and a new API key: a Saved with one `endpoint.insecure`
 * warning on `engine.api.base_url` (T-015; a fresh copy, for `queueSaveOutcome`).
 */
export function savedInsecureApi(): Extract<SaveOutcome, { Saved: unknown }> {
  return structuredClone(fixture.saved_insecure_api) as Extract<SaveOutcome, { Saved: unknown }>;
}

/**
 * Core's `SaveOutcome` for a save, over the first run (engine none, no key edits), with
 * post-processing on and base URL, model and prompt all "": a Refused with
 * `post_processing.base_url`, `.model` and `.prompt` `required` and no form error (T-021;
 * pinned by core's e2e_settings_wire_fixture_refused_post_processing_matches_core; a fresh
 * copy, for `queueSaveOutcome`).
 */
export function refusedPostProcessingOnEmpty(): Extract<SaveOutcome, { Refused: unknown }> {
  return structuredClone(fixture.refused_post_processing_on_empty) as Extract<SaveOutcome, { Refused: unknown }>;
}

/** Core's `WHISPER_ISO_639_1` in core order (a fresh copy). */
export function coreSpeechLanguages(): string[] {
  return [...fixture.speech_languages];
}

// ---- Core-checked local-model data (e2e/fixtures/local-models-wire.json, T-044) -------

/** The `ReasonView` codes the fixture holds (DownloadFailure and DownloadError). */
export type ReasonCode =
  | "download_interrupted"
  | "checksum_mismatch"
  | "not_enough_disk_space"
  | "source_unreachable"
  | "disk_error"
  | "http_status"
  | "download_busy"
  | "already_downloaded"
  | "not_in_catalog"
  | "download_cannot_start";

interface LocalModelsFixture {
  event_names: { progress: string; state: string };
  list_first_run: LocalModelView[];
  states: Record<"not_downloaded" | "downloading" | "downloaded" | "failed", ModelState>;
  reasons: Record<ReasonCode, FailureReason>;
}

const modelsFixture = JSON.parse(
  readFileSync(new URL("../fixtures/local-models-wire.json", import.meta.url), "utf8"),
) as LocalModelsFixture;

/** Core's event names: `local-model://progress` and `local-model://state`. */
export const LOCAL_MODEL_EVENTS: Readonly<{ progress: string; state: string }> = Object.freeze({
  ...modelsFixture.event_names,
});

/** Core's `local_models_list` before any download: five rows, catalog order, all not_downloaded. */
export function localModelsFirstRun(): LocalModelView[] {
  return structuredClone(modelsFixture.list_first_run);
}

/** Core's `ReasonView` for `code` (a fresh copy). */
export function failureReason(code: ReasonCode): FailureReason {
  return structuredClone(modelsFixture.reasons[code]);
}

/** A `failed` state with core's reason for `code`. */
export function failedState(code: ReasonCode): ModelState {
  return { kind: "failed", reason: failureReason(code) } as ModelState;
}

/** `localModelsFirstRun()` with the row `id` in `state` (an unknown id fails here, by name). */
export function modelsWith(id: string, state: ModelState): LocalModelView[] {
  const list = localModelsFirstRun();
  const row = list.find((model) => model.id === id);
  if (row === undefined) throw new Error(`local-models-wire.json has no model ${id}`);
  row.state = structuredClone(state);
  return list;
}

// ---- Core-checked overlay data (e2e/fixtures/overlay-wire.json, T-053) ------------------

/** The payloads the fixture holds: seq 1-5 in en, 6-10 in ru, in that order. */
export type OverlayWireKey =
  | "recording_en"
  | "processing_en"
  | "message_no_speech_en"
  | "message_microphone_access_denied_en"
  | "hidden_en"
  | "recording_ru"
  | "processing_ru"
  | "message_no_speech_ru"
  | "message_microphone_access_denied_ru"
  | "hidden_ru";

const overlayFixture = JSON.parse(
  readFileSync(new URL("../fixtures/overlay-wire.json", import.meta.url), "utf8"),
) as Partial<Record<OverlayWireKey, OverlayPayload>>;

/** contracts/ipc.md: the event the shell emits to the window labelled `overlay`. */
export const OVERLAY_STATE_EVENT = "overlay://state";

/** Core's payload `key` (a fresh copy); a key the fixture lacks fails here, by name. */
export function overlayWire(key: OverlayWireKey): OverlayPayload {
  const value = overlayFixture[key];
  if (value === undefined) throw new Error(`overlay-wire.json has no ${key}`);
  return structuredClone(value);
}

/** The default `overlay_ready` reply: core's hidden payload numbered 0 (nothing shown yet). */
export function overlayNothingYet(): OverlayPayload {
  return { ...overlayWire("hidden_en"), seq: 0 };
}

// ---- Microphones (settings_list_microphones, T-012) -------------------------------

/**
 * The default `settings_list_microphones` answer (a fresh copy): three fake WASAPI-style
 * endpoint ids in enumeration order, the second one flagged as the Windows default (so
 * "the first entry is the default" is not true of it).
 */
export function fakeMicrophones(): InputDevice[] {
  return [
    {
      id: "{0.0.1.00000000}.{00000000-0000-4000-8000-0000000fa001}",
      name: "Microphone (Fake USB Audio)",
      is_default: false,
    },
    {
      id: "{0.0.1.00000000}.{00000000-0000-4000-8000-0000000fa002}",
      name: "Headset Microphone (Fake Bluetooth)",
      is_default: true,
    },
    {
      id: "{0.0.1.00000000}.{00000000-0000-4000-8000-0000000fa003}",
      name: "Line In (Fake HD Audio)",
      is_default: false,
    },
  ];
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
  /** `plugin:window|destroy`: `{ reject }` makes it reject with that text (still recorded). */
  destroy?: { reject: string };
  /** Keep `settings_get` in flight until `releaseSettingsGet` (recorded at once). */
  holdSettingsGet?: boolean;
  /** Events whose `plugin:event|listen` rejects; no handler is registered for them. */
  rejectListen?: string[];
  /** Events whose `plugin:event|listen` stays in flight, unregistered, until `releaseListen`. */
  holdListen?: string[];
  /** What `local_models_list` returns; default: `localModelsFirstRun()`. */
  localModels?: LocalModelView[];
  /** Keep every `local_models_list` in flight until `releaseList` (recorded at once). */
  holdList?: boolean;
  /** Keep every `local_model_download` in flight, not yet run, until `releaseDownload` (recorded at once). */
  holdDownload?: boolean;
  /** `local_models_list` rejects with this text (recorded); the command cannot run. */
  listRejection?: string;
  /**
   * What `overlay_ready` returns, or `{ reject }` to make every call reject with that
   * text (still recorded); default: `overlayNothingYet()`.
   */
  overlayReady?: OverlayPayload | { reject: string };
  /** Keep every `overlay_ready` in flight until `releaseOverlayReady` (recorded at once, reply taken then). */
  holdOverlayReady?: boolean;
  /** The label of the current window as tauri reports it; default `settings`. */
  windowLabel?: string;
  /**
   * What `settings_list_microphones` returns, or `{ reject }` to make every call reject
   * with that text (still recorded); default: `fakeMicrophones()`.
   */
  microphones?: InputDevice[] | { reject: string };
  /** Keep every `settings_list_microphones` in flight until `releaseMicrophones` (recorded at once, copy taken then). */
  holdMicrophones?: boolean;
}

interface InitArg {
  view: SettingsView;
  speechLanguages: string[];
  saveOutcomes: SaveOutcome[];
  buildInfo: BuildInfo | { reject: string } | null;
  destroy: { reject: string } | null;
  holdSettingsGet: boolean;
  rejectListen: string[];
  holdListen: string[];
  localModels: LocalModelView[];
  holdList: boolean;
  holdDownload: boolean;
  listRejection: string | null;
  modelEvents: { progress: string; state: string };
  overlayReady: OverlayPayload | { reject: string };
  holdOverlayReady: boolean;
  windowLabel: string;
  microphones: InputDevice[] | { reject: string };
  holdMicrophones: boolean;
}

/** Installs the mock as an init script; call before `page.goto`. */
export async function installTauriMock(page: Page, options: MockOptions = {}): Promise<void> {
  const arg: InitArg = {
    view: options.view ?? firstRunView(),
    speechLanguages: options.speechLanguages ?? coreSpeechLanguages(),
    saveOutcomes: options.saveOutcomes ?? [],
    buildInfo: options.buildInfo ?? null,
    destroy: options.destroy ?? null,
    holdSettingsGet: options.holdSettingsGet ?? false,
    rejectListen: options.rejectListen ?? [],
    holdListen: options.holdListen ?? [],
    localModels: options.localModels ?? localModelsFirstRun(),
    holdList: options.holdList ?? false,
    holdDownload: options.holdDownload ?? false,
    listRejection: options.listRejection ?? null,
    modelEvents: { ...modelsFixture.event_names },
    overlayReady: options.overlayReady ?? overlayNothingYet(),
    holdOverlayReady: options.holdOverlayReady ?? false,
    windowLabel: options.windowLabel ?? "settings",
    microphones: options.microphones ?? fakeMicrophones(),
    holdMicrophones: options.holdMicrophones ?? false,
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
      holdingGet: init.holdSettingsGet,
      heldGet: [] as (() => void)[],
      holdingListen: [...init.holdListen],
      heldListen: [] as (() => void)[],
      models: clone(init.localModels) as { id: string; sizeBytes: number; state: unknown }[],
      downloadRejections: [] as unknown[],
      holdingList: init.holdList,
      heldList: [] as (() => void)[],
      holdingDownload: init.holdDownload,
      heldDownload: [] as (() => void)[],
      holdingReady: init.holdOverlayReady,
      heldReady: [] as (() => void)[],
      holdingMics: init.holdMicrophones,
      heldMics: [] as (() => void)[],
    };

    function setModelState(id: string, modelState: unknown): void {
      const row = state.models.find((model) => model.id === id);
      if (row) row.state = clone(modelState);
    }

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
          if (init.rejectListen.includes(event)) throw new Error(`listen ${event} refused`);
          if (state.holdingListen.includes(event)) {
            await new Promise<void>((resolve) => state.heldListen.push(resolve));
          }
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
          if (state.holdingGet) await new Promise<void>((resolve) => state.heldGet.push(resolve));
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
        case "local_models_list": {
          if (init.listRejection !== null) throw new Error(init.listRejection);
          const snapshot = clone(state.models);
          if (state.holdingList) await new Promise<void>((resolve) => state.heldList.push(resolve));
          return snapshot;
        }
        case "local_model_download": {
          if (state.holdingDownload) await new Promise<void>((resolve) => state.heldDownload.push(resolve));
          if (state.downloadRejections.length > 0) throw clone(state.downloadRejections.shift());
          const row = state.models.find((model) => model.id === args.id);
          if (row) row.state = { kind: "downloading", received: 0, total: row.sizeBytes };
          return null;
        }
        case "local_model_cancel_download": {
          const id = args.id as string;
          const row = state.models.find((model) => model.id === id);
          const running =
            row !== undefined && (row.state as { kind?: unknown } | null)?.kind === "downloading";
          if (!running) return false;
          const cancelled = { kind: "not_downloaded" };
          // The download thread records the cancel after the command returned: the listed
          // state changes there, then the event is emitted (core service.rs `record`).
          setTimeout(() => {
            setModelState(id, cancelled);
            emit(init.modelEvents.state, { id, state: cancelled }, "mock");
          }, 0);
          return true;
        }
        case "overlay_ready": {
          // The answer is the state when the shell ran the command (or its refusal); a
          // hold only delays it.
          const reply = clone(init.overlayReady);
          if (state.holdingReady) await new Promise<void>((resolve) => state.heldReady.push(resolve));
          if ("reject" in reply) throw new Error(reply.reject);
          return reply;
        }
        case "settings_list_microphones": {
          // The answer is the device list when the shell ran the command; a hold only delays it.
          const reply = clone(init.microphones);
          if (state.holdingMics) await new Promise<void>((resolve) => state.heldMics.push(resolve));
          if (!Array.isArray(reply)) throw new Error(reply.reject);
          return reply;
        }
        case "plugin:window|destroy":
          if (init.destroy !== null) throw new Error(init.destroy.reject);
          return null;
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
        currentWindow: { label: init.windowLabel },
        currentWebview: { windowLabel: init.windowLabel, label: init.windowLabel },
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
      releaseGet: () => {
        state.holdingGet = false;
        for (const resolve of state.heldGet.splice(0)) resolve();
      },
      releaseListen: () => {
        state.holdingListen = [];
        for (const resolve of state.heldListen.splice(0)) resolve();
      },
      releaseList: () => {
        state.holdingList = false;
        for (const resolve of state.heldList.splice(0)) resolve();
      },
      holdLists: () => {
        state.holdingList = true;
      },
      releaseDownload: () => {
        state.holdingDownload = false;
        for (const resolve of state.heldDownload.splice(0)) resolve();
      },
      queueDownloadRejection: (payload: unknown) => state.downloadRejections.push(clone(payload)),
      releaseOverlayReady: () => {
        state.holdingReady = false;
        for (const resolve of state.heldReady.splice(0)) resolve();
      },
      releaseMicrophones: () => {
        state.holdingMics = false;
        for (const resolve of state.heldMics.splice(0)) resolve();
      },
      progress: (id: string, received: number) => {
        const row = state.models.find((model) => model.id === id);
        if (!row) throw new Error(`no local model ${id}`);
        row.state = { kind: "downloading", received, total: row.sizeBytes };
        emit(init.modelEvents.progress, { id, received, total: row.sizeBytes }, "test");
      },
      modelState: (id: string, modelState: unknown) => {
        if (!state.models.some((model) => model.id === id)) throw new Error(`no local model ${id}`);
        setModelState(id, modelState);
        emit(init.modelEvents.state, { id, state: modelState }, "test");
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
    models: LocalModelView[];
  };
  emit: (event: string, payload: unknown) => void;
  queue: (item: { outcome: unknown } | { reject: unknown }) => void;
  hold: () => void;
  release: () => void;
  releaseGet: () => void;
  releaseListen: () => void;
  releaseList: () => void;
  holdLists: () => void;
  releaseDownload: () => void;
  queueDownloadRejection: (payload: unknown) => void;
  releaseOverlayReady: () => void;
  releaseMicrophones: () => void;
  progress: (id: string, received: number) => void;
  modelState: (id: string, state: ModelState) => void;
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

/**
 * The user closes the window: the shell emits `tauri://close-requested` (payload null),
 * as tauri does when a JS listener for it exists (T-039 Investigation, Close).
 */
export async function requestClose(page: Page): Promise<void> {
  await emit(page, "tauri://close-requested", null);
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

/** Answers every held `settings_get` and stops holding (see `holdSettingsGet`). */
export async function releaseSettingsGet(page: Page): Promise<void> {
  await page.evaluate(() => (window as unknown as MockWindow).__VOICEN_MOCK__.releaseGet());
}

/** Registers every held `plugin:event|listen` (see `holdListen`), answers it, and stops holding. */
export async function releaseListen(page: Page): Promise<void> {
  await page.evaluate(() => (window as unknown as MockWindow).__VOICEN_MOCK__.releaseListen());
}

// ---- Local models (spec 002, T-045) ----------------------------------------------

/** The models list the mock holds now (what the next `local_models_list` returns). */
export async function storedModels(page: Page): Promise<LocalModelView[]> {
  return page.evaluate(() => (window as unknown as MockWindow).__VOICEN_MOCK__.state.models);
}

/** Answers every held `local_models_list` with the copy taken at its call, and stops holding. */
export async function releaseList(page: Page): Promise<void> {
  await page.evaluate(() => (window as unknown as MockWindow).__VOICEN_MOCK__.releaseList());
}

/** From now on each `local_models_list` stays in flight until `releaseList` (its copy taken at the call). */
export async function holdLists(page: Page): Promise<void> {
  await page.evaluate(() => (window as unknown as MockWindow).__VOICEN_MOCK__.holdLists());
}

/** Runs every held `local_model_download` (see `holdDownload`), answers it, and stops holding. */
export async function releaseDownload(page: Page): Promise<void> {
  await page.evaluate(() => (window as unknown as MockWindow).__VOICEN_MOCK__.releaseDownload());
}

/**
 * The next `local_model_download` rejects with `payload` and changes nothing: a
 * `FailureReason` for a contract refusal (`failureReason(code)`), anything else for a
 * failure of the invoke itself.
 */
export async function queueDownloadRejection(page: Page, payload: unknown): Promise<void> {
  await page.evaluate(
    (value) => (window as unknown as MockWindow).__VOICEN_MOCK__.queueDownloadRejection(value),
    payload,
  );
}

/**
 * The download thread reports progress: the row becomes `downloading { received,
 * total: sizeBytes }`, then `local-model://progress { id, received, total }` is emitted.
 */
export async function localModelProgress(page: Page, id: string, received: number): Promise<void> {
  await page.evaluate(
    ([model, bytes]) => (window as unknown as MockWindow).__VOICEN_MOCK__.progress(model, bytes),
    [id, received] as const,
  );
}

/** A state transition: the row takes `state`, then `local-model://state { id, state }` is emitted. */
export async function localModelState(page: Page, id: string, state: ModelState): Promise<void> {
  await page.evaluate(
    ([model, next]) => (window as unknown as MockWindow).__VOICEN_MOCK__.modelState(model, next),
    [id, state] as const,
  );
}

// ---- Overlay (spec 001, T-053) -----------------------------------------------------

/** The shell sends `payload` to the overlay: `overlay://state` reaches every registered handler. */
export async function overlayState(page: Page, payload: OverlayPayload): Promise<void> {
  await emit(page, OVERLAY_STATE_EVENT, payload);
}

/** Answers every held `overlay_ready` with the reply taken at its call, and stops holding. */
export async function releaseOverlayReady(page: Page): Promise<void> {
  await page.evaluate(() => (window as unknown as MockWindow).__VOICEN_MOCK__.releaseOverlayReady());
}

// ---- Microphones (spec 004, T-012) ---------------------------------------------------

/** Answers every held `settings_list_microphones` with the list taken at its call, and stops holding. */
export async function releaseMicrophones(page: Page): Promise<void> {
  await page.evaluate(() => (window as unknown as MockWindow).__VOICEN_MOCK__.releaseMicrophones());
}
