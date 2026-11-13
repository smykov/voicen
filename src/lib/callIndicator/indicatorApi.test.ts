// T-131 red tests: the one IPC module of the call indicator (specs/007-call-mode-record
// contracts/ipc.md; analysis invariant I2, the wire of I1/I3).
//
// Export contract of src/lib/callIndicator/indicatorApi.ts (the developer implements against it):
//   stop(): Promise<null>                                    invoke("call_indicator_stop")      no args
//   dismiss(warningId): Promise<null>                        invoke("call_indicator_dismiss", { warningId })
//   indicatorState(): Promise<IndicatorState>                invoke("call_indicator_state")     no args
//   onIndicatorState(handler(state)): Promise<UnlistenFn>    listen("indicator://state")  payload IndicatorState
//   onIndicatorTick(handler(tick)): Promise<UnlistenFn>      listen("indicator://tick")   payload IndicatorTick
// plus the wire types (IndicatorState incl. lang, IndicatorTick incl. micSilent/systemSilent,
// IndicatorWarning, WarningKind). The module decides nothing: values reach the caller as the
// shell sent them. Data: core's own wire (e2e/fixtures/call-indicator-wire.json).
import { beforeEach, describe, expect, it, vi } from "vitest";
import wire from "../../../e2e/fixtures/call-indicator-wire.json";

const invoke = vi.fn();
const listen = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: (...args: unknown[]) => listen(...args) }));

const api = await import("./indicatorApi");

const fixture = wire as unknown as Record<string, unknown>;

beforeEach(() => {
  invoke.mockReset();
  listen.mockReset();
});

describe("commands: name and args exactly as the contract", () => {
  it("stop invokes call_indicator_stop with no args", async () => {
    invoke.mockResolvedValueOnce(null);
    await expect(api.stop()).resolves.toBeNull();
    expect(invoke.mock.calls).toEqual([["call_indicator_stop"]]);
  });

  it("dismiss(7) invokes call_indicator_dismiss { warningId: 7 }", async () => {
    invoke.mockResolvedValueOnce(null);
    await expect(api.dismiss(7)).resolves.toBeNull();
    expect(invoke.mock.calls).toEqual([["call_indicator_dismiss", { warningId: 7 }]]);
  });

  it("dismiss passes the id it was given, not a constant", async () => {
    invoke.mockResolvedValue(null);
    await api.dismiss(2);
    await api.dismiss(9);
    expect(invoke.mock.calls).toEqual([
      ["call_indicator_dismiss", { warningId: 2 }],
      ["call_indicator_dismiss", { warningId: 9 }],
    ]);
  });

  it("indicatorState invokes call_indicator_state with no args and resolves with the state unchanged", async () => {
    invoke.mockResolvedValueOnce(structuredClone(fixture.warnings_ru));
    await expect(api.indicatorState()).resolves.toEqual(fixture.warnings_ru);
    expect(invoke.mock.calls).toEqual([["call_indicator_state"]]);
  });
});

describe("failure branch: a rejection reaches the caller unchanged", () => {
  const cases: [string, () => Promise<unknown>][] = [
    ["stop", () => api.stop()],
    ["dismiss", () => api.dismiss(3)],
    ["indicatorState", () => api.indicatorState()],
  ];
  for (const [name, call] of cases) {
    it(`${name} rejects with the shell's rejection as is`, async () => {
      invoke.mockRejectedValueOnce("shell refused (fake)");
      await expect(call()).rejects.toBe("shell refused (fake)");
      expect(invoke).toHaveBeenCalledTimes(1);
    });
  }
});

describe("listeners", () => {
  const listeners: [string, string, string, (h: (p: never) => void) => Promise<unknown>][] = [
    ["onIndicatorState", "indicator://state", "recording_reminder_ru", (h) => api.onIndicatorState(h)],
    ["onIndicatorTick", "indicator://tick", "tick_levels", (h) => api.onIndicatorTick(h)],
  ];
  for (const [name, event, key, subscribe] of listeners) {
    it(`${name} listens to ${event}, hands the handler the event payload unchanged, and returns the unlisten`, async () => {
      const unlisten = vi.fn();
      listen.mockResolvedValueOnce(unlisten);
      const handler = vi.fn();
      const got = await subscribe(handler);
      expect(listen).toHaveBeenCalledTimes(1);
      expect(listen.mock.calls[0][0]).toBe(event);
      expect(got).toBe(unlisten);
      const payload = structuredClone(fixture[key]);
      listen.mock.calls[0][1]({ event, id: 1, payload });
      expect(handler.mock.calls).toEqual([[fixture[key]]]);
    });

    it(`${name}: a refused listen rejects to the caller`, async () => {
      listen.mockRejectedValueOnce("listen refused (fake)");
      await expect(subscribe(vi.fn())).rejects.toBe("listen refused (fake)");
    });
  }
});
