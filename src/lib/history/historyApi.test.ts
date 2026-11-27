// T-022 red tests: the one IPC module of the History window (specs/005-transcript-history
// contracts/ipc.md). Export contract of src/lib/history/historyApi.ts (the developer implements
// against it):
//   getHistory(): Promise<HistoryView>              invoke("history_get")
//   copyEntry(id): Promise<null>                    invoke("history_copy", { id })
//   clearHistory(): Promise<HistoryView>            invoke("history_clear")
//   onHistoryChanged(handler(view)): Promise<UnlistenFn>   listen("history://changed")  payload HistoryView
//   openHistorySettings(): Promise<null>            invoke("settings_open", { tab: "history" })
// plus the wire types (HistoryEngineKind, HistoryEntry, HistoryNotice, HistoryView, HistoryCopyError).
// The view is the shell's: handed on unchanged (no sort, no filter); an error reaches the caller
// as it came (no swallow, no rewrite).
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
const listen = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: (...args: unknown[]) => listen(...args) }));

const api = await import("./historyApi");

const view = () => ({
  enabled: true,
  entries: [
    { id: 3, text: "third (fake)", readyAtMs: 1, engine: { kind: "api", model: "whisper-1" } },
    { id: 2, text: "second (fake)", readyAtMs: 9_000_000, engine: { kind: "local", model: "small" } },
    { id: 1, text: "first (fake)", readyAtMs: 5, engine: { kind: "server", model: "" } },
  ],
  notice: null,
});

beforeEach(() => {
  invoke.mockReset();
  listen.mockReset();
});

describe("commands: name and args exactly as the contract", () => {
  it("getHistory invokes history_get with no args and resolves with the view unchanged (order included)", async () => {
    invoke.mockResolvedValueOnce(view());
    await expect(api.getHistory()).resolves.toEqual(view());
    expect(invoke.mock.calls).toEqual([["history_get"]]);
  });

  it("getHistory: an off view stays off and a notice stays set", async () => {
    invoke.mockResolvedValueOnce({ enabled: false, entries: [], notice: "delete_failed" });
    await expect(api.getHistory()).resolves.toEqual({ enabled: false, entries: [], notice: "delete_failed" });
  });

  it("copyEntry invokes history_copy { id } with the id as given", async () => {
    invoke.mockResolvedValueOnce(null);
    await expect(api.copyEntry(24)).resolves.toBeNull();
    expect(invoke.mock.calls).toEqual([["history_copy", { id: 24 }]]);
  });

  for (const code of ["clipboard_unavailable", "not_found"]) {
    it(`copyEntry: a rejection ${code} reaches the caller as it came`, async () => {
      const rejection = { code };
      invoke.mockRejectedValueOnce(rejection);
      await expect(api.copyEntry(1)).rejects.toBe(rejection);
    });
  }

  it("clearHistory invokes history_clear with no args and resolves with the returned view", async () => {
    const after = { enabled: true, entries: [], notice: "delete_failed" };
    invoke.mockResolvedValueOnce(after);
    await expect(api.clearHistory()).resolves.toEqual(after);
    expect(invoke.mock.calls).toEqual([["history_clear"]]);
  });

  it("openHistorySettings invokes the open-settings command at the History tab", async () => {
    invoke.mockResolvedValueOnce(null);
    await api.openHistorySettings();
    expect(invoke.mock.calls).toEqual([["settings_open", { tab: "history" }]]);
  });

  it("a rejected history_get / history_clear reaches the caller", async () => {
    invoke.mockRejectedValueOnce("refused (fake)");
    await expect(api.getHistory()).rejects.toBe("refused (fake)");
    invoke.mockRejectedValueOnce("refused (fake)");
    await expect(api.clearHistory()).rejects.toBe("refused (fake)");
  });
});

describe("onHistoryChanged", () => {
  it("listens to history://changed, hands the handler the event payload, and returns the unlisten", async () => {
    const unlisten = vi.fn();
    listen.mockResolvedValueOnce(unlisten);
    const handler = vi.fn();
    const got = await api.onHistoryChanged(handler);
    expect(listen).toHaveBeenCalledTimes(1);
    expect(listen.mock.calls[0][0]).toBe("history://changed");
    expect(got).toBe(unlisten);
    const payload = view();
    listen.mock.calls[0][1]({ event: "history://changed", id: 1, payload });
    expect(handler).toHaveBeenCalledTimes(1);
    expect(handler).toHaveBeenCalledWith(payload);
  });

  it("a refused listen rejects to the caller", async () => {
    listen.mockRejectedValueOnce("listen refused (fake)");
    await expect(api.onHistoryChanged(vi.fn())).rejects.toBe("listen refused (fake)");
  });
});
