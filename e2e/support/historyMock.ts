// T-022: the History window's IPC on top of the one Tauri mock (e2e/support/tauriMock.ts).
//
// `installTauriMock` knows no `history_*` command (any other command rejects), so a history spec
// calls it first (it supplies `plugin:event|listen`, `settings_get`, `emit`, `listeners`) and then
// `installHistoryMock`, a second init script that wraps `__TAURI_INTERNALS__.invoke`:
// - `history_get` returns a copy of the stored view (never fails: contract);
// - `history_copy { id }` is recorded and returns null, or rejects with the next queued
//   rejection (one shot: the call after it succeeds again, so a failing fake fails exactly one
//   call and a later call cannot re-raise the error); the mock never decides `not_found`;
// - `history_clear` is recorded, replaces the stored view with the next queued view (default:
//   enabled, no entries, no notice) and returns it; it emits nothing, so a page that ignores the
//   returned view and waits for an event stays stale;
// - `OPEN_SETTINGS` (the open-settings command at the History tab) is recorded and returns null.
// Every other command goes to the wrapped invoke. Payloads follow specs/005-transcript-history
// contracts/ipc.md (camelCase), which core's tests (crates/voicen-core/tests/history*.rs) pin on
// the Rust side.
import type { Page } from "@playwright/test";

export type HistoryEngineKind = "api" | "local" | "server";
export interface HistoryWireEntry {
  id: number;
  text: string;
  readyAtMs: number;
  engine: { kind: HistoryEngineKind; model: string };
}
export type HistoryWireNotice = "save_failed" | "delete_failed";
export interface HistoryWireView {
  enabled: boolean;
  entries: HistoryWireEntry[];
  notice: HistoryWireNotice | null;
}

/** The command the History page's "Open settings" button invokes, with `{ tab: "history" }`. */
export const OPEN_SETTINGS = "settings_open";
export const HISTORY_CHANGED = "history://changed";

const BASE_MS = Date.UTC(2026, 9, 30, 9, 0, 0); // 30 Oct 2026 09:00 UTC, fake

/** Entry `n` (1-based): unique text `Entry text 0007 (fake)`, newer for a larger `n`. */
export function entry(n: number, over: Partial<HistoryWireEntry> = {}): HistoryWireEntry {
  return {
    id: n,
    text: `Entry text ${String(n).padStart(4, "0")} (fake)`,
    readyAtMs: BASE_MS + n * 60_000,
    engine: { kind: "api", model: "whisper-1" },
    ...over,
  };
}

/** What core's view holds after `total` records with size `size`: the newest `size`, newest first. */
export function viewAfter(total: number, size: number, notice: HistoryWireNotice | null = null): HistoryWireView {
  const entries: HistoryWireEntry[] = [];
  for (let n = total; n > Math.max(0, total - size); n -= 1) entries.push(entry(n));
  return { enabled: true, entries, notice };
}

export const offView = (): HistoryWireView => ({ enabled: false, entries: [], notice: null });
export const emptyView = (): HistoryWireView => ({ enabled: true, entries: [], notice: null });

export interface HistoryCall {
  cmd: string;
  args: unknown;
}

interface HistoryMockWindow {
  __HISTORY_MOCK__: {
    calls: HistoryCall[];
    queueCopyRejection: (payload: unknown) => void;
    queueClear: (view: HistoryWireView) => void;
  };
}

/** Call after `installTauriMock`, before `page.goto`. */
export async function installHistoryMock(page: Page, view: HistoryWireView): Promise<void> {
  await page.addInitScript(
    ([initial, openSettings]: [HistoryWireView, string]) => {
      const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;
      const w = window as unknown as {
        __TAURI_INTERNALS__: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> };
        __HISTORY_MOCK__: unknown;
      };
      const inner = w.__TAURI_INTERNALS__.invoke;
      const state = {
        view: clone(initial),
        calls: [] as { cmd: string; args: unknown }[],
        copyRejections: [] as unknown[],
        clearViews: [] as unknown[],
      };
      w.__TAURI_INTERNALS__.invoke = async (cmd: string, args: Record<string, unknown> = {}) => {
        switch (cmd) {
          case "history_get":
            state.calls.push({ cmd, args: clone(args) });
            return clone(state.view);
          case "history_copy":
            state.calls.push({ cmd, args: clone(args) });
            if (state.copyRejections.length > 0) throw clone(state.copyRejections.shift());
            return null;
          case "history_clear": {
            state.calls.push({ cmd, args: clone(args) });
            const next = state.clearViews.shift();
            state.view = clone((next ?? { enabled: true, entries: [], notice: null }) as typeof state.view);
            return clone(state.view);
          }
          case openSettings:
            state.calls.push({ cmd, args: clone(args) });
            return null;
        }
        return inner(cmd, args);
      };
      w.__HISTORY_MOCK__ = {
        calls: state.calls,
        queueCopyRejection: (payload: unknown) => state.copyRejections.push(clone(payload)),
        queueClear: (v: unknown) => state.clearViews.push(clone(v)),
      };
    },
    [view, OPEN_SETTINGS] as [HistoryWireView, string],
  );
}

/** Every history call recorded so far (history_* and the open-settings command), or only `cmd`. */
export async function historyCalls(page: Page, cmd?: string): Promise<HistoryCall[]> {
  const all = await page.evaluate(() => (window as unknown as HistoryMockWindow).__HISTORY_MOCK__.calls);
  return cmd === undefined ? all : all.filter((c) => c.cmd === cmd);
}

/** The next `history_copy` rejects with `payload` (one call only). */
export async function queueCopyRejection(page: Page, payload: unknown): Promise<void> {
  await page.evaluate((p) => (window as unknown as HistoryMockWindow).__HISTORY_MOCK__.queueCopyRejection(p), payload);
}

/** The next `history_clear` returns (and stores) `view` instead of the empty one. */
export async function queueClearView(page: Page, view: HistoryWireView): Promise<void> {
  await page.evaluate((v) => (window as unknown as HistoryMockWindow).__HISTORY_MOCK__.queueClear(v), view);
}
