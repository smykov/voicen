// The call-indicator IPC as the call-indicator page sees it (specs/007-call-mode-record
// contracts/ipc.md, T-131).
//
// One IPC module per contract: this module holds the only invoke/listen calls of the
// call-indicator page and the one TS declaration of its wire (the serde form of
// voicen_core::call::indicator IndicatorState / IndicatorTick, pinned by
// e2e/fixtures/call-indicator-wire.json); the e2e mock re-exports these types. Nothing here
// decides what is shown: values reach the caller as the shell sent them, and every timing
// (the 10 s reminder, a warning's re-arm) is core's.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
// Relative, not `$lib`: e2e/support/tauriMock.ts imports these wire types too.
import type { UiLanguage } from "../i18n";

/** core `IndicatorWarning::kind`: what the warning is about. */
export type WarningKind =
  | "communicationsDevice"
  | "trackLost"
  | "micChanged"
  | "cantHearOtherParty"
  | "cantHearYou"
  | "lowDisk"
  | "providerNotice";

/** One warning of the indicator (core `IndicatorWarning`). */
export interface IndicatorWarning {
  /** The shell's id; `call_indicator_dismiss` takes it. A re-raised kind gets a new id. */
  id: number;
  kind: WarningKind;
  /** Rendered by Rust in the state's `lang`; shown as given. */
  text: string;
}

/** One `indicator://state` event, or the `call_indicator_state` reply (core `IndicatorState`). */
export interface IndicatorState {
  /** The language of every text in this state and of the page's own texts. */
  lang: UiLanguage;
  recording: boolean;
  reminderVisible: boolean;
  /** Rendered by Rust; empty when `reminderVisible` is false. */
  reminderText: string;
  /** Rendered by Rust; empty when `reminderVisible` is false. */
  headphonesHint: string;
  /** In the shell's order. */
  warnings: IndicatorWarning[];
}

/** One `indicator://tick` event (core `IndicatorTick`). */
export interface IndicatorTick {
  elapsedMs: number;
  /** 0..1. */
  micLevel: number;
  /** 0..1. */
  systemLevel: number;
  micSilent: boolean;
  systemSilent: boolean;
}

/** contracts/ipc.md: the events the shell emits to the window labelled `call-indicator`. */
const STATE_EVENT = "indicator://state";
const TICK_EVENT = "indicator://tick";

// The listeners are generic in the payload type only so that a handler typed for a subtype
// (a test's `(p: never) => void`) type-checks, as callsApi's; the payload is always the
// event's as sent.

/** `indicator://state`: every change of the indicator state. */
export function onIndicatorState<P extends IndicatorState = IndicatorState>(handler: (state: P) => void): Promise<UnlistenFn> {
  return listen<P>(STATE_EVENT, (event) => handler(event.payload));
}

/** `indicator://tick`: the elapsed time and the two levels while recording. */
export function onIndicatorTick<P extends IndicatorTick = IndicatorTick>(handler: (tick: P) => void): Promise<UnlistenFn> {
  return listen<P>(TICK_EVENT, (event) => handler(event.payload));
}

/**
 * `call_indicator_state`: the current state, so a webview that loads late misses nothing.
 * Invoke it only once the `indicator://state` listener is registered.
 */
export function indicatorState(): Promise<IndicatorState> {
  return invoke<IndicatorState>("call_indicator_state");
}

/** `call_indicator_stop`: stop the call recording (the shell decides what follows). */
export function stop(): Promise<null> {
  return invoke<null>("call_indicator_stop");
}

/** `call_indicator_dismiss`: dismiss one warning; it leaves only when the shell's next state omits it. */
export function dismiss(warningId: number): Promise<null> {
  return invoke<null>("call_indicator_dismiss", { warningId });
}
