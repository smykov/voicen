// T-143 red tests: the one IPC module of the Calls contract (specs/007-call-mode-record
// contracts/ipc.md; Acceptance "callsApi.test.ts", analysis invariant, D1 and D8).
//
// Export contract of src/lib/calls/callsApi.ts (the developer implements against it):
//   listCalls(): Promise<CallList>                         invoke("calls_list")
//   getCall(root, id): Promise<CallDetail>                 invoke("calls_get", { root, id })
//   copyMarkdown(root, id): Promise<null>                  invoke("calls_copy_markdown", { root, id })
//   exportCall(root, id, format): Promise<{ saved }>       invoke("calls_export", { root, id, format })
//   openFolder(root, id): Promise<null>                    invoke("calls_open_folder", { root, id })
//   onCallsChanged(handler(ref)): Promise<UnlistenFn>      listen("calls://changed")  payload { root, id }
//   onCallsOpen(handler(ref)): Promise<UnlistenFn>         listen("calls://open")     payload { root, id }
//   onSettingsChanged(handler(view)): Promise<UnlistenFn>  listen("settings://changed") payload SettingsView
// plus the wire types (CallState, FailReason, CallSummary, CallList, Speaker, Line, CallDetail, CallError).
// Data: core's own wire (e2e/fixtures/calls-wire.json, pinned by storage::tests::calls_wire_fixture_matches_core).
import { beforeEach, describe, expect, it, vi } from "vitest";
import wire from "../../../e2e/fixtures/calls-wire.json";

const invoke = vi.fn();
const listen = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: (...args: unknown[]) => listen(...args) }));

const api = await import("./callsApi");

const fixture = wire as unknown as {
  list: { calls: { id: string; root: string }[]; unavailableRoots: string[] };
  detail: unknown;
  errors: Record<string, unknown>;
};
const REF = { root: "C:\\Calls", id: "2026-10-29_100000" };

beforeEach(() => {
  invoke.mockReset();
  listen.mockReset();
});

describe("commands: name and args exactly as the contract", () => {
  it("listCalls invokes calls_list with no args and resolves with core's list unchanged", async () => {
    const list = structuredClone(fixture.list);
    invoke.mockResolvedValueOnce(list);
    await expect(api.listCalls()).resolves.toEqual(fixture.list);
    expect(invoke.mock.calls).toEqual([["calls_list"]]);
  });

  it("D1: the list order is returned verbatim, even when it is not newest first (no client sort)", async () => {
    const scrambled = structuredClone(fixture.list);
    scrambled.calls.reverse();
    const ids = scrambled.calls.map((c) => c.id);
    invoke.mockResolvedValueOnce(scrambled);
    const got = await api.listCalls();
    expect(got.calls.map((c: { id: string }) => c.id)).toEqual(ids);
    expect(got.unavailableRoots).toEqual(fixture.list.unavailableRoots);
  });

  it("getCall invokes calls_get { root, id } and resolves with the detail unchanged", async () => {
    invoke.mockResolvedValueOnce(structuredClone(fixture.detail));
    await expect(api.getCall(REF.root, REF.id)).resolves.toEqual(fixture.detail);
    expect(invoke.mock.calls).toEqual([["calls_get", REF]]);
  });

  it("copyMarkdown invokes calls_copy_markdown { root, id }", async () => {
    invoke.mockResolvedValueOnce(null);
    await expect(api.copyMarkdown(REF.root, REF.id)).resolves.toBeNull();
    expect(invoke.mock.calls).toEqual([["calls_copy_markdown", REF]]);
  });

  for (const format of ["md", "json"] as const) {
    it(`exportCall invokes calls_export { root, id, format: "${format}" } and resolves with {saved}`, async () => {
      invoke.mockResolvedValueOnce({ saved: false });
      await expect(api.exportCall(REF.root, REF.id, format)).resolves.toEqual({ saved: false });
      expect(invoke.mock.calls).toEqual([["calls_export", { ...REF, format }]]);
    });
  }

  it("openFolder invokes calls_open_folder { root, id }", async () => {
    invoke.mockResolvedValueOnce(null);
    await api.openFolder(REF.root, REF.id);
    expect(invoke.mock.calls).toEqual([["calls_open_folder", REF]]);
  });
});

describe("failure branch: a rejection reaches the caller unchanged (core's CallError)", () => {
  const cases: [string, () => Promise<unknown>][] = [
    ["listCalls", () => api.listCalls()],
    ["getCall", () => api.getCall(REF.root, REF.id)],
    ["copyMarkdown", () => api.copyMarkdown(REF.root, REF.id)],
    ["exportCall", () => api.exportCall(REF.root, REF.id, "md")],
    ["openFolder", () => api.openFolder(REF.root, REF.id)],
  ];
  for (const [name, call] of cases) {
    it(`${name} rejects with errors.ioFailed as is`, async () => {
      invoke.mockRejectedValueOnce(structuredClone(fixture.errors.ioFailed));
      await expect(call()).rejects.toEqual({ code: "ioFailed", reason: "access_denied" });
      expect(invoke).toHaveBeenCalledTimes(1);
    });
  }
});

describe("listeners (D8)", () => {
  const listeners: [string, string, (h: (p: never) => void) => Promise<unknown>][] = [
    ["onCallsChanged", "calls://changed", (h) => api.onCallsChanged(h)],
    ["onCallsOpen", "calls://open", (h) => api.onCallsOpen(h)],
    ["onSettingsChanged", "settings://changed", (h) => api.onSettingsChanged(h)],
  ];
  for (const [name, event, subscribe] of listeners) {
    it(`${name} listens to ${event}, hands the handler the event payload, and returns the unlisten`, async () => {
      const unlisten = vi.fn();
      listen.mockResolvedValueOnce(unlisten);
      const handler = vi.fn();
      const got = await subscribe(handler);
      expect(listen).toHaveBeenCalledTimes(1);
      expect(listen.mock.calls[0][0]).toBe(event);
      expect(got).toBe(unlisten);
      const payload = event === "settings://changed" ? { settings: { ui_language: "ru" } } : REF;
      listen.mock.calls[0][1]({ event, id: 1, payload });
      expect(handler.mock.calls).toEqual([[payload]]);
    });

    it(`${name}: a refused listen rejects to the caller`, async () => {
      listen.mockRejectedValueOnce("listen refused (fake)");
      await expect(subscribe(vi.fn())).rejects.toBe("listen refused (fake)");
    });
  }
});
