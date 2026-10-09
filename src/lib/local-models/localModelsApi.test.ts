// T-019 red tests: `deleteLocalModel` in the one IPC module of spec 002 (settings-ui.md › C).
//
// What they pin (T-019 analysis, seam; contracts/ipc.md `local_model_delete`):
// - `deleteLocalModel(id)` is exactly one `invoke("local_model_delete", { id })` and resolves
//   with core's `DeleteOutcome { engineReset, resetFailed }` unchanged (nothing derived here);
// - a rejection reaches the caller unchanged, so `asFailureReason` decides what is shown;
// - each delete refusal core sends (model_in_use, not_downloaded, delete_failed) is a
//   FailureReason by `asFailureReason`, so it is shown by its messageKey and not as
//   `error.ipc_unavailable`.
// Data: core's own wire (e2e/fixtures/local-models-wire.json, pinned by
// e2e_local_models_wire_fixture_matches_core; the developer regenerates it for T-019).
import { beforeEach, describe, expect, it, vi } from "vitest";
import wire from "../../../e2e/fixtures/local-models-wire.json";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

const { asFailureReason, deleteLocalModel } = await import("./localModelsApi");

type Fixture = {
  reasons: Record<string, unknown>;
  delete_outcomes?: Record<"kept" | "engine_reset" | "reset_failed", unknown>;
};
const fixture = wire as unknown as Fixture;

function outcome(key: "kept" | "engine_reset" | "reset_failed"): unknown {
  const value = fixture.delete_outcomes?.[key];
  if (value === undefined) throw new Error(`local-models-wire.json has no delete_outcomes.${key}`);
  return structuredClone(value);
}

function reason(code: string): unknown {
  const value = fixture.reasons[code];
  if (value === undefined) throw new Error(`local-models-wire.json has no reason ${code}`);
  return structuredClone(value);
}

beforeEach(() => {
  invoke.mockReset();
});

describe("deleteLocalModel", () => {
  for (const key of ["kept", "engine_reset", "reset_failed"] as const) {
    it(`invokes local_model_delete { id } once and resolves with core's ${key} outcome unchanged`, async () => {
      const value = outcome(key);
      invoke.mockResolvedValueOnce(value);
      await expect(deleteLocalModel("small")).resolves.toEqual(value);
      expect(invoke.mock.calls).toEqual([["local_model_delete", { id: "small" }]]);
    });
  }

  it("passes the id through as is", async () => {
    invoke.mockResolvedValueOnce(outcome("kept"));
    await deleteLocalModel("large-v3-turbo-q5_0");
    expect(invoke.mock.calls).toEqual([["local_model_delete", { id: "large-v3-turbo-q5_0" }]]);
  });

  it("failure branch: a rejection reaches the caller unchanged (a FailureReason or anything else)", async () => {
    const inUse = reason("model_in_use");
    invoke.mockRejectedValueOnce(inUse);
    await expect(deleteLocalModel("small")).rejects.toEqual(inUse);
    invoke.mockRejectedValueOnce("local_model_delete exploded (fake)");
    await expect(deleteLocalModel("small")).rejects.toBe("local_model_delete exploded (fake)");
    expect(invoke).toHaveBeenCalledTimes(2);
  });
});

describe("delete refusals are FailureReasons", () => {
  for (const [code, messageKey] of [
    ["model_in_use", "delete.model_in_use"],
    ["not_downloaded", "delete.not_downloaded"],
    ["delete_failed", "delete.failed"],
  ] as const) {
    it(`core's ${code} is a FailureReason with messageKey ${messageKey}`, () => {
      const value = reason(code);
      expect(value).toEqual({ code, messageKey });
      expect(asFailureReason(value)).toEqual({ code, messageKey });
    });
  }
});
