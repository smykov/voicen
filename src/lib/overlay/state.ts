// The pure half of the overlay page (spec 001 contracts/ipc.md, T-053 analysis
// invariant (5)).
//
// - The page shows the payload with the highest `seq`: the `overlay_ready` reply and the
//   `overlay://state` events can arrive in either order (`apply`).
// - The page changes nothing on its own: no expiry timer. Time only advances a
//   recording's elapsed time (`stateAt`), shown as m:ss (`formatElapsed`). Every expiry
//   is core's.
import type { OverlayPayload, OverlayView } from "./overlayApi";

/** The payload the page shows and when it arrived (ms on the page's monotonic clock). */
export interface Shown {
  readonly payload: OverlayPayload;
  readonly receivedAt: number;
}

/**
 * `payload` arrived at `receivedAt`: it is shown if its seq is higher than the shown
 * payload's (or nothing is shown yet); otherwise `current` is returned unchanged (the
 * same object).
 */
export function apply(current: Shown | null, payload: OverlayPayload, receivedAt: number): Shown {
  if (current !== null && payload.seq <= current.payload.seq) return current;
  return { payload, receivedAt };
}

/**
 * What the page draws at `now`: the shown payload's state, a recording's `elapsedMs`
 * advanced by the time since it arrived (never less than the payload's own: a `now`
 * before the arrival counts as the arrival); `hidden` before any payload.
 */
export function stateAt(shown: Shown | null, now: number): OverlayView {
  if (shown === null) return { kind: "hidden" };
  const state = shown.payload.state;
  if (state.kind !== "recording") return state;
  return { kind: "recording", elapsedMs: state.elapsedMs + Math.max(0, now - shown.receivedAt) };
}

/** An elapsed time as m:ss (whole seconds elapsed). */
export function formatElapsed(ms: number): string {
  const seconds = Math.floor(ms / 1000);
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${String(seconds % 60).padStart(2, "0")}`;
}
