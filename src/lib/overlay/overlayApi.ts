// The overlay IPC as the overlay page sees it (spec 001 contracts/ipc.md, T-053).
//
// One IPC module per contract (settings-ui.md › C): this module holds the only
// listen/invoke calls of the overlay page and the one TS declaration of its wire (the
// serde form of voicen_core::overlay::OverlayPayload, pinned by
// e2e/fixtures/overlay-wire.json); the e2e mock re-exports these types.
//
// Skeleton (T-053 red tests): the wire types are the contract; the two calls are not
// implemented yet.
import type { UnlistenFn } from "@tauri-apps/api/event";
// Relative, not `$lib`: e2e/support/tauriMock.ts imports these wire types too.
import type { UiLanguage } from "../i18n";

/** `state` of a payload (core `overlay::OverlayView`): what the page shows. */
export type OverlayView =
  /** Shown as `overlay.recording` with `{elapsed}` = m:ss. */
  | { kind: "recording"; elapsedMs: number }
  /** Shown as `overlay.processing`. */
  | { kind: "processing" }
  /** The full text, rendered by Rust in the payload's `lang`; shown as given. */
  | { kind: "message"; text: string }
  /** Nothing is shown (the shell destroys the window after this). */
  | { kind: "hidden" };

/** One `overlay://state` event, or the `overlay_ready` reply (core `overlay::OverlayPayload`). */
export interface OverlayPayload {
  /** The shell's number for this state; higher = newer. */
  seq: number;
  /** The language of `state`'s text and of the page's own texts. */
  lang: UiLanguage;
  state: OverlayView;
}

/** `overlay://state`: every change of the overlay state, emitted to the window labelled `overlay`. */
export function onOverlayState(handler: (payload: OverlayPayload) => void): Promise<UnlistenFn> {
  void handler;
  throw new Error("T-053: onOverlayState is not implemented");
}

/** `overlay_ready`: the current state, so a webview that loads late misses nothing. */
export function overlayReady(): Promise<OverlayPayload> {
  throw new Error("T-053: overlayReady is not implemented");
}
