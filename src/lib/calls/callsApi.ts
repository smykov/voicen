// The calls IPC as the `calls` window sees it (specs/007-call-mode-record contracts/ipc.md,
// T-143; docs/decisions/calls-ui.md).
//
// One IPC module per contract (settings-ui.md › C): this module holds every invoke/listen of
// the Calls page: the calls_* commands it uses, calls://changed and calls://open. The
// settings://changed it needs for the language and the call hotkey is re-exported from the
// settings module, as `settings_get` is `getSettings` there (the one declaration of each). Every value is
// handed over as core sent it: no sort, no re-title, no catch (a rejection is core's
// `CallError`, given to the caller unchanged).
//
// The wire types below are the one TS declaration of the calls wire (contracts/ipc.md › Wire
// types; core's serde, pinned by e2e/fixtures/calls-wire.json); e2e/support/tauriMock.ts
// imports them, so the imports here stay relative, not `$lib`.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type CallState = "recording" | "queued" | "transcribing" | "ready" | "failed" | "recovered" | "damaged";

export type FailReason = "key" | "network" | "timeout" | "provider" | "storage" | "engineNotSetUp";

/**
 * One call of `calls_list`. `root` is the opaque root text Rust returned (sent back as is);
 * `startedAt` is RFC 3339 local time with its offset, or `YYYY-MM-DDTHH:MM:SS` without one
 * for a damaged call; `durationMs` is null when unknown.
 */
export interface CallSummary {
  id: string;
  root: string;
  title: string;
  startedAt: string;
  durationMs: number | null;
  state: CallState;
  failReason?: FailReason;
}

/** `calls_list`: the calls in core's order (newest first) and the roots that are not available. */
export interface CallList {
  calls: CallSummary[];
  unavailableRoots: string[];
}

export interface Speaker {
  id: "me" | "s1";
  name: string;
}

export interface Line {
  startMs: number;
  endMs: number;
  track: "mic" | "system";
  speaker: "me" | "s1";
  lang: string | null;
  text: string;
}

/** `calls_get`: one call with its speakers and transcript lines (empty without a transcript). */
export interface CallDetail {
  summary: CallSummary;
  speakers: Speaker[];
  lines: Line[];
}

/** What every calls_* command rejects with. `reason` is a snake_case `UnwritableReason` word of `ioFailed`. */
export interface CallError {
  code: "notFound" | "inUse" | "emptyName" | "recordingCall" | "ioFailed";
  file?: string;
  reason?: string;
}

/** The `{root, id}` of one call, as the events carry it. */
export interface CallRef {
  root: string;
  id: string;
}

export type ExportFormat = "md" | "json";

/** `calls_list`: core's list, order kept. */
export function listCalls(): Promise<CallList> {
  return invoke<CallList>("calls_list");
}

/** `calls_get { root, id }`. */
export function getCall(root: string, id: string): Promise<CallDetail> {
  return invoke<CallDetail>("calls_get", { root, id });
}

/** `calls_copy_markdown { root, id }`: the transcript as Markdown to the clipboard. */
export function copyMarkdown(root: string, id: string): Promise<null> {
  return invoke<null>("calls_copy_markdown", { root, id });
}

/** `calls_export { root, id, format }`: `saved: false` = the save dialog was cancelled. */
export function exportCall(root: string, id: string, format: ExportFormat): Promise<{ saved: boolean }> {
  return invoke<{ saved: boolean }>("calls_export", { root, id, format });
}

/** `calls_open_folder { root, id }`. */
export function openFolder(root: string, id: string): Promise<null> {
  return invoke<null>("calls_open_folder", { root, id });
}

// The listeners are generic in the payload type only so that a handler typed for a subtype
// (a test's `(p: never) => void`) type-checks; the payload is always the event's as sent.

/** `calls://changed {root, id}`: a call changed (state, rename, delete, pass progress). */
export function onCallsChanged<P extends CallRef = CallRef>(handler: (ref: P) => void): Promise<UnlistenFn> {
  return listen<P>("calls://changed", (event) => handler(event.payload));
}

/** `calls://open {root, id}`: the shell asks the window to show that call. */
export function onCallsOpen<P extends CallRef = CallRef>(handler: (ref: P) => void): Promise<UnlistenFn> {
  return listen<P>("calls://open", (event) => handler(event.payload));
}

// `settings://changed` (language and call hotkey) is spec 004's event: its one declaration is
// in the settings module, re-exported here so the Calls page reaches all its IPC through this
// module.
export { onSettingsChanged } from "../settings/settingsApi";
