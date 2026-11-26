// The history IPC as the `history` window sees it (specs/005-transcript-history
// contracts/ipc.md, T-022).
//
// One IPC module per contract (settings-ui.md › C): every invoke/listen of the History page
// goes through here: the three history_* commands, `history://changed` and the open-settings
// request at the History tab (`settings_open { tab: "history" }`, pinned by T-022). The
// `settings_get` / `settings://changed` it needs for the UI language are the settings module's
// (the one declaration of each), re-exported below. Every value is handed over as the shell
// sent it: no sort, no filter, no catch (a rejection reaches the caller unchanged).
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type HistoryEngineKind = "api" | "local" | "server";

/** One kept transcript; `readyAtMs` is for display only (the order is the view's). */
export interface HistoryEntry {
  id: number;
  text: string;
  readyAtMs: number;
  engine: { kind: HistoryEngineKind; model: string };
}

export type HistoryNotice = "save_failed" | "delete_failed";

/** The whole history state: entries newest first, in core's order. */
export interface HistoryView {
  enabled: boolean;
  entries: HistoryEntry[];
  notice: HistoryNotice | null;
}

/** What `history_copy` rejects with; it carries a code only, never entry text. */
export interface HistoryCopyError {
  code: "not_found" | "clipboard_unavailable";
}

export const HISTORY_CHANGED = "history://changed";

/** `history_get`: the current view (memory only; never fails in the shell). */
export function getHistory(): Promise<HistoryView> {
  return invoke<HistoryView>("history_get");
}

/** `history_copy { id }`: that entry's text to the clipboard (never a paste). */
export function copyEntry(id: number): Promise<null> {
  return invoke<null>("history_copy", { id });
}

/** `history_clear`: the view after clearing (`notice` may be `delete_failed`). */
export function clearHistory(): Promise<HistoryView> {
  return invoke<HistoryView>("history_clear");
}

/** `history://changed`: the whole new view after every change. */
export function onHistoryChanged(handler: (view: HistoryView) => void): Promise<UnlistenFn> {
  return listen<HistoryView>(HISTORY_CHANGED, (event) => handler(event.payload));
}

/** The shell's open-settings request at the History tab. */
export function openHistorySettings(): Promise<null> {
  return invoke<null>("settings_open", { tab: "history" });
}

// spec 004's settings read and event: their one declaration is in the settings module.
export { getSettings, onSettingsChanged } from "../settings/settingsApi";
