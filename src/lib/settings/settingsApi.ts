// The settings IPC as the window sees it (spec 004 contracts/ipc.md, T-004).
//
// Wire types are the serde forms of voicen_core (contracts/ipc.md › Wire form). One IPC
// module per contract (settings-ui.md › C): this module holds every invoke/listen/window
// call of spec 004's contract; spec 002's local-model calls live in
// src/lib/local-models/localModelsApi.ts and the build info in src/lib/buildInfo.ts.
// The window never builds a SettingsView itself: every one it shows comes from here (U1).
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow, type CloseRequestedEvent } from "@tauri-apps/api/window";
// Relative, not `$lib`: e2e/support/tauriMock.ts imports these wire types too.
import type { MessageId, UiLanguage } from "../i18n";

export type EngineKind = "none" | "api" | "builtin_local" | "local_server";

/** `voicen_core::settings::Settings` (data-model.md); no key field. */
export interface Settings {
  schema_version: number;
  engine: EngineKind;
  api: { base_url: string; model: string };
  local_server: { base_url: string; model: string };
  builtin_local: { model_id: string | null };
  speech_language: string | null;
  microphone: { id: string; name: string } | null;
  hotkey: string;
  mode: "hold" | "toggle";
  auto_paste: boolean;
  post_processing: { enabled: boolean; base_url: string; model: string; prompt: string };
  history: { enabled: boolean; size: number };
  /** `voicen_core::settings::TimeoutSettings`: whole seconds; the range is core's rule (`timeout.range`). */
  timeouts: {
    connect_s: number;
    api_transcription_s: number;
    local_server_s: number;
    post_processing_s: number;
    builtin_local_s: number;
  };
  start_with_windows: boolean;
  ui_language: UiLanguage;
}

export type KeySlot = "transcription_api" | "local_server" | "post_processing";

/** The key slots, in the service's order (`KeySlot::all()`). */
export const KEY_SLOTS: readonly KeySlot[] = ["transcription_api", "local_server", "post_processing"];

/** What a window receives (`SettingsService::view()`): key presence only, never a key. */
export interface SettingsView {
  settings: Settings;
  keys: Record<KeySlot, boolean>;
  first_run: boolean;
  reset_notice: boolean;
  unavailable: boolean;
}

/** One key slot's edit: keep, delete, or replace with the typed key (UI -> shell only). */
export type KeyEdit = "Untouched" | "Clear" | { Replace: string };

export interface SaveRequest {
  settings: Settings;
  keys: Record<KeySlot, KeyEdit>;
}

/** A refused field: `field` is a FieldId, `code` an ErrorCode (both dotted strings). */
export interface FieldError {
  field: string;
  code: string;
}

/**
 * A refusal not tied to one field. `message` is a Rust `MessageId` (declared with
 * `messages!`), so it is typed `MessageId` here and rendered with `t`: core's
 * `message_ids_exist_in_both_catalogs` guarantees its text in both catalogs.
 */
export interface FormError {
  kind: "write_failed" | "settings_unavailable" | "partially_restored";
  message: MessageId;
  not_restored?: string[];
}

/**
 * A note on a saved field (`Saved.warnings`); never a refusal. `field` is a FieldId,
 * `code` a WarningCode (dotted string), and `message` the Rust `MessageId` core maps the
 * code to (`WarningCode::message_id`), typed `MessageId` here and rendered with `t` as
 * given: core decides which value warns and with which text (decision #52;
 * `message_ids_exist_in_both_catalogs` guarantees the text in both catalogs).
 */
export interface Warning {
  field: string;
  code: string;
  message: MessageId;
}

export type SaveOutcome =
  | { Saved: { view: SettingsView; warnings: Warning[] } }
  | { Refused: { errors: FieldError[]; form_error: FormError | null } };

/** `settings_get`: the saved settings, key presence and the load flags. */
export function getSettings(): Promise<SettingsView> {
  return invoke<SettingsView>("settings_get");
}

/** `settings_save { request }`: all-or-nothing; rejects only when the command cannot run. */
export function saveSettings(request: SaveRequest): Promise<SaveOutcome> {
  return invoke<SaveOutcome>("settings_save", { request });
}

/** `settings_speech_languages`: core's speech-language codes, the picker's only list. */
export function speechLanguages(): Promise<string[]> {
  return invoke<string[]>("settings_speech_languages");
}

/** `settings://changed`: the view after every save, from any window. */
export function onSettingsChanged(handler: (view: SettingsView) => void): Promise<UnlistenFn> {
  return listen<SettingsView>("settings://changed", (event) => handler(event.payload));
}

/**
 * `settings://focus {tab, field?}`: the shell asks the open window to show `tab` and
 * focus the control of `field` (settings_window::open; contracts/ipc.md). `tab` is a
 * `SettingsTab` token and `field` a FieldId, both as the shell sent them.
 */
export interface FocusRequest {
  tab: string;
  field?: string;
}

/** `settings://focus`: a focus request on the open window (see `FocusRequest`). */
export function onFocusRequest(handler: (request: FocusRequest) => void): Promise<UnlistenFn> {
  return listen<FocusRequest>("settings://focus", (event) => handler(event.payload));
}

/**
 * `tauri://close-requested` on this window. tauri destroys the window after `handler`
 * returns unless it called `event.preventDefault()` first (@tauri-apps/api 2.12.1
 * `onCloseRequested`), so a handler that keeps the window must prevent synchronously,
 * and must not throw (a throwing handler leaves the window impossible to close).
 */
export function onCloseRequested(handler: (event: CloseRequestedEvent) => void): Promise<UnlistenFn> {
  return getCurrentWindow().onCloseRequested(handler);
}

/**
 * Close this window for good (`plugin:window|destroy`, granted to label `settings`).
 * The window never calls `close()`: it re-emits close-requested and is not granted.
 */
export function destroyWindow(): Promise<void> {
  return getCurrentWindow().destroy();
}
