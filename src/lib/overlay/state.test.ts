// T-053 red tests: the pure half of the overlay page (spec 001 contracts/ipc.md overlay;
// T-053 analysis invariant (5) and design 3; red-test table row 4).
//
// What they pin:
// - `apply` (the seq rule): the page shows the payload with the highest seq. The
//   `overlay_ready` reply and the `overlay://state` events can arrive in either order,
//   so an older or equal seq is dropped, whenever it arrives, and leaves the shown
//   payload and its arrival time unchanged (the same object).
// - `stateAt` (no expiry of its own): what the page draws at a time is the shown
//   payload's state; only a recording's elapsed time advances (elapsedMs + the time
//   since the payload arrived). A message, processing or hidden never changes with
//   time: every expiry is core's (recording/mod.rs `show`/`expire`, dictation.rs
//   `run_timer`), a UI timer beside it would disagree on replaced messages.
// - `formatElapsed`: m:ss of the whole seconds elapsed (a stopwatch: 1.5 s is 0:01),
//   minutes unpadded, seconds two digits.
//
// Data: core's own wire (e2e/fixtures/overlay-wire.json, pinned by
// overlay::tests::e2e_overlay_wire_fixture_matches_core); a test that needs another
// order overrides `seq` with a spread, never the fixture.
import { describe, expect, it } from "vitest";
import wire from "../../../e2e/fixtures/overlay-wire.json";
import type { OverlayPayload } from "./overlayApi";
import { apply, formatElapsed, stateAt, type Shown } from "./state";

type WireKey = Exclude<keyof typeof wire, "_format">;

/** Core's payload `key` (a fresh copy), optionally renumbered. */
function payload(key: WireKey, seq?: number): OverlayPayload {
  const value = structuredClone(wire[key]) as unknown as OverlayPayload;
  return seq === undefined ? value : { ...value, seq };
}

/** Freezes `value` and everything it holds: `apply` must not write to its inputs. */
function deepFreeze<T>(value: T): T {
  if (value !== null && typeof value === "object") {
    for (const inner of Object.values(value)) deepFreeze(inner);
    Object.freeze(value);
  }
  return value;
}

/** Every order of `items` (n! arrays). */
function permutations<T>(items: readonly T[]): T[][] {
  if (items.length <= 1) return [[...items]];
  return items.flatMap((item, at) =>
    permutations([...items.slice(0, at), ...items.slice(at + 1)]).map((rest) => [item, ...rest]),
  );
}

// ---- apply: the payload with the highest seq is shown ---------------------------------

describe("apply (the seq rule)", () => {
  it("shows the first payload, with the time it arrived", () => {
    const first = payload("processing_en");
    expect(apply(null, first, 1_000)).toEqual({ payload: first, receivedAt: 1_000 });
  });

  it("a payload with a higher seq replaces the shown one and its arrival time", () => {
    const shown = apply(null, payload("recording_en"), 1_000);
    const newer = payload("processing_en");
    expect(apply(shown, newer, 2_500)).toEqual({ payload: newer, receivedAt: 2_500 });
  });

  it("an older payload (a held overlay_ready reply after a newer event) is dropped: the shown state is returned unchanged", () => {
    const event = deepFreeze(apply(null, payload("message_microphone_access_denied_en"), 1_000));
    const olderReply = deepFreeze(payload("processing_en"));
    expect(olderReply.seq).toBeLessThan(event.payload.seq);
    expect(apply(event, olderReply, 2_000)).toBe(event);
  });

  it("a payload with the same seq is dropped (a reply repeating the last event does not restart the elapsed time)", () => {
    const shown = deepFreeze(apply(null, payload("recording_en"), 1_000));
    const sameSeq = deepFreeze(payload("message_no_speech_en", shown.payload.seq));
    expect(apply(shown, sameSeq, 5_000)).toBe(shown);
  });

  it("in every arrival order the highest seq is shown, with the time it arrived", () => {
    // The reply and the events race (R-13): whatever order they arrive in, the result is
    // the newest payload, and a later, older one never resets its arrival time.
    const all = (["recording_en", "processing_en", "message_microphone_access_denied_en", "hidden_en", "recording_ru"] as const).map(
      (key) => deepFreeze(payload(key)),
    );
    const newest = all.reduce((a, b) => (b.seq > a.seq ? b : a));
    for (const order of permutations(all)) {
      let shown: Shown | null = null;
      for (const [at, next] of order.entries()) shown = apply(shown, next, at * 100);
      const label = order.map((p) => p.seq).join(",");
      expect(shown, `arrival order ${label}`).toEqual({ payload: newest, receivedAt: order.indexOf(newest) * 100 });
    }
  });
});

// ---- stateAt: only the elapsed time advances; no expiry of its own --------------------

describe("stateAt (no expiry of its own)", () => {
  it("before any payload the page shows nothing", () => {
    expect(stateAt(null, 0)).toEqual({ kind: "hidden" });
    expect(stateAt(null, 3_600_000)).toEqual({ kind: "hidden" });
  });

  it("a recording's elapsed time is the payload's elapsedMs plus the time since it arrived", () => {
    const shown = apply(null, payload("recording_en"), 10_000); // elapsedMs 61 000
    expect(stateAt(shown, 10_000)).toEqual({ kind: "recording", elapsedMs: 61_000 });
    expect(stateAt(shown, 12_500)).toEqual({ kind: "recording", elapsedMs: 63_500 });
    expect(stateAt(shown, 10_000 + 59_000)).toEqual({ kind: "recording", elapsedMs: 120_000 });
  });

  it("a time before the arrival counts as the arrival: the elapsed time never drops below the payload's elapsedMs", () => {
    const shown = apply(null, payload("recording_en"), 10_000); // elapsedMs 61 000
    expect(stateAt(shown, 10_000 - 500)).toEqual({ kind: "recording", elapsedMs: 61_000 });
  });

  it("a newer recording counts from its own elapsedMs and arrival, not the previous one's", () => {
    const first = apply(null, payload("recording_en"), 0); // seq 1, 61 000 ms
    const second = apply(first, payload("recording_ru"), 30_000); // seq 6, 5 000 ms
    expect(stateAt(second, 31_000)).toEqual({ kind: "recording", elapsedMs: 6_000 });
  });

  it("a message has no expiry: it is still shown 3 s, a minute and an hour after it arrived", () => {
    for (const key of ["message_no_speech_en", "message_microphone_access_denied_en", "message_microphone_access_denied_ru"] as const) {
      const message = payload(key);
      const shown = apply(null, message, 5_000);
      for (const after of [0, 1_200, 2_999, 3_000, 3_001, 60_000, 3_600_000]) {
        expect(stateAt(shown, 5_000 + after), `${key}, ${after} ms after it arrived`).toEqual(message.state);
      }
    }
  });

  it("processing and hidden do not change with time either", () => {
    for (const key of ["processing_en", "processing_ru", "hidden_en", "hidden_ru"] as const) {
      const value = payload(key);
      const shown = apply(null, value, 5_000);
      for (const after of [0, 3_000, 60_000, 3_600_000]) {
        expect(stateAt(shown, 5_000 + after), `${key}, ${after} ms after it arrived`).toEqual(value.state);
      }
    }
  });
});

// ---- formatElapsed: m:ss ---------------------------------------------------------------

describe("formatElapsed (m:ss)", () => {
  it.each([
    [0, "0:00"],
    [999, "0:00"],
    [1_000, "0:01"],
    [1_500, "0:01"],
    [5_000, "0:05"],
    [59_999, "0:59"],
    [60_000, "1:00"],
    [61_000, "1:01"],
    [61_999.9, "1:01"],
    [600_000, "10:00"],
    [3_599_999, "59:59"],
  ])("%s ms is %s", (ms, expected) => {
    expect(formatElapsed(ms)).toBe(expected);
  });
});
