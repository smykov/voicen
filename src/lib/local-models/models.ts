// The pure half of the local-model picker (spec 002 US1, T-045; settings-ui.md › M).
//
// - I2: the rows shown are the newest issued `local_models_list` response, overlaid in
//   arrival order by every `local-model://` event received since that request was
//   issued. Nothing else sets a row's state (no optimistic state, no UI rule).
// - I3: the model select offers exactly the downloaded rows (`selectable`).
// - I4: Download / Retry are blocked while any row downloads or a download invoke is
//   pending (`downloadBlocked`).
// - I5: every byte count is shown only through `formatSize`, in the UI language.
import type { UiLanguage } from "../i18n";
import type {
  FailureReason,
  LocalModelProgress,
  LocalModelStateChange,
  LocalModelView,
} from "./localModelsApi";

/** One `local-model://` event as received. */
export type LocalModelEvent =
  | { event: "progress"; payload: LocalModelProgress }
  | { event: "state"; payload: LocalModelStateChange };

/** The rows a payload gives: its row takes the new state; an unknown id changes nothing. */
export function applyEvent(rows: readonly LocalModelView[], event: LocalModelEvent): LocalModelView[] {
  const { id } = event.payload;
  const state =
    event.event === "progress"
      ? { kind: "downloading" as const, received: event.payload.received, total: event.payload.total }
      : event.payload.state;
  return rows.map((row) => (row.id === id ? { ...row, state } : row));
}

/**
 * The list/event sequencer (I2). `rows` is null until the first response of the newest
 * request. `latest` numbers the newest issued request; `since` holds the events
 * received since it was issued while it is unanswered (null when none is pending).
 */
export interface ModelsState {
  readonly rows: readonly LocalModelView[] | null;
  readonly latest: number;
  readonly since: readonly LocalModelEvent[] | null;
}

export function emptyModels(): ModelsState {
  return { rows: null, latest: 0, since: null };
}

/** A list request is issued: it becomes the newest; events from now on are kept for it. */
export function listRequested(state: ModelsState): { state: ModelsState; request: number } {
  const request = state.latest + 1;
  return { state: { ...state, latest: request, since: [] }, request };
}

/** A list response: the newest request's replaces the rows, overlaid by the events since; any older one is dropped. */
export function listReceived(state: ModelsState, request: number, rows: readonly LocalModelView[]): ModelsState {
  if (request !== state.latest || state.since === null) return state;
  return { ...state, rows: state.since.reduce(applyEvent, [...rows]), since: null };
}

/** A list request rejected: the rows stay; true when it was the newest (its failure is shown). */
export function listFailed(state: ModelsState, request: number): { state: ModelsState; newest: boolean } {
  if (request !== state.latest) return { state, newest: false };
  return { state: { ...state, since: null }, newest: true };
}

/** An event: applied to the rows shown now and kept for the pending request, if any. */
export function eventReceived(state: ModelsState, event: LocalModelEvent): ModelsState {
  return {
    ...state,
    rows: state.rows === null ? null : applyEvent(state.rows, event),
    since: state.since === null ? null : [...state.since, event],
  };
}

/** The rows the model select offers (I3): exactly the downloaded ones, in list order. */
export function selectable(rows: readonly LocalModelView[]): LocalModelView[] {
  return rows.filter((row) => row.state.kind === "downloaded");
}

/** Download / Retry are disabled (I4): a row is downloading or a download invoke is pending. */
export function downloadBlocked(rows: readonly LocalModelView[], invokePending: boolean): boolean {
  return invokePending || rows.some((row) => row.state.kind === "downloading");
}

const KIB = 1024;
const MIB = KIB * 1024;
const GIB = MIB * 1024;

function unitFormat(lang: UiLanguage, unit: "kilobyte" | "megabyte" | "gigabyte", decimals: number, value: number) {
  return new Intl.NumberFormat(lang, {
    style: "unit",
    unit,
    unitDisplay: "short",
    minimumFractionDigits: decimals,
    maximumFractionDigits: decimals,
  }).format(value);
}

/**
 * A byte count as Windows Explorer shows it (binary units, OQ-08 default), labelled in
 * `lang` through Intl: whole kB below 1 MB, whole MB below 1 GB, GB with one decimal;
 * rounded to the nearest unit (a value that rounds up to 1024 moves to the next unit).
 * The one size rule of the UI (I5).
 */
export function formatSize(bytes: number, lang: UiLanguage): string {
  const kib = Math.round(bytes / KIB);
  if (kib < 1024) return unitFormat(lang, "kilobyte", 0, kib);
  const mib = Math.round(bytes / MIB);
  if (mib < 1024) return unitFormat(lang, "megabyte", 0, mib);
  return unitFormat(lang, "gigabyte", 1, Math.round((bytes / GIB) * 10) / 10);
}

/**
 * The placeholder args of a reason's text in `lang`: `needed` is a byte count
 * (contracts/ipc.md) shown through `formatSize`; every other param as sent.
 */
export function reasonArgs(reason: FailureReason, lang: UiLanguage): Record<string, string> {
  const args: Record<string, string> = {};
  for (const [name, value] of Object.entries(reason.params ?? {})) {
    const bytes = Number(value);
    args[name] = name === "needed" && value.trim() !== "" && Number.isFinite(bytes) ? formatSize(bytes, lang) : value;
  }
  return args;
}
