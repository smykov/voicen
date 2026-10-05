// The pure half of the overlay page (spec 001 contracts/ipc.md, T-053 analysis
// invariant (5)).
//
// - The page shows the payload with the highest `seq`: the `overlay_ready` reply and the
//   `overlay://state` events can arrive in either order (`apply`).
// - The page changes nothing on its own: no expiry timer. Time only advances a
//   recording's elapsed time (`stateAt`), shown as m:ss (`formatElapsed`). Every expiry
//   is core's.
//
// Skeleton (T-053 red tests): the signatures are the contract; nothing is implemented yet.
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
  void current;
  void payload;
  void receivedAt;
  throw new Error("T-053: apply is not implemented");
}

/**
 * What the page draws at `now`: the shown payload's state, a recording's `elapsedMs`
 * advanced by the time since it arrived; `hidden` before any payload.
 */
export function stateAt(shown: Shown | null, now: number): OverlayView {
  void shown;
  void now;
  throw new Error("T-053: stateAt is not implemented");
}

/** An elapsed time as m:ss (whole seconds elapsed). */
export function formatElapsed(ms: number): string {
  void ms;
  throw new Error("T-053: formatElapsed is not implemented");
}
