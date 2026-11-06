// The pure half of the Calls page (T-143 D2 to D6; docs/decisions/calls-ui.md). No IPC, no
// clock, no timezone: every function maps wire values to what the page shows.
import type { MessageId } from "../i18n";
import type { CallState, FailReason, Speaker } from "./callsApi";

const pad2 = (n: number) => String(n).padStart(2, "0");

/**
 * D2: "DD.MM.YYYY HH:MM" from the digits of `startedAt` as written (with or without an
 * offset): the recording's own wall clock, never converted through `Date` or a timezone.
 * A value that does not start with `YYYY-MM-DDTHH:MM` is shown as given.
 */
export function formatStartedAt(startedAt: string): string {
  const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})/.exec(startedAt);
  if (m === null) return startedAt;
  return `${m[3]}.${m[2]}.${m[1]} ${m[4]}:${m[5]}`;
}

/** Whole seconds of `ms` (floored) as "MM:SS", or "H:MM:SS" from one hour. */
function clock(ms: number): string {
  const total = Math.floor(Math.max(0, ms) / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  return h > 0 ? `${h}:${pad2(m)}:${pad2(s)}` : `${pad2(m)}:${pad2(s)}`;
}

/** D2: a call's duration; null (unknown) shows a dash. */
export function formatDuration(ms: number | null): string {
  return ms === null ? "—" : clock(ms);
}

/** D2: a transcript line's start, in the duration's form. */
export function formatLineTime(startMs: number): string {
  return clock(startMs);
}

export type CallAction = "copy" | "exportMd" | "exportJson" | "openFolder" | "delete";

/**
 * D3: the actions a row offers, one function of its state, in display order. Transcribe
 * (T-148) and the behaviour of Delete (T-154) are not part of T-143.
 */
export function actionsFor(state: CallState): CallAction[] {
  switch (state) {
    case "ready":
      return ["copy", "exportMd", "exportJson", "openFolder", "delete"];
    case "recording":
      return ["openFolder"];
    case "queued":
    case "transcribing":
    case "failed":
    case "recovered":
    case "damaged":
      return ["openFolder", "delete"];
  }
}

/** The catalog id of each action's button. */
export const ACTION_LABEL: Readonly<Record<CallAction, MessageId>> = {
  copy: "calls.copy",
  exportMd: "calls.export_md",
  exportJson: "calls.export_json",
  openFolder: "calls.open_folder",
  delete: "calls.delete",
};

/** The catalog id of a state's badge. */
export function stateMessageId(state: CallState): MessageId {
  return `calls.state.${state}`;
}

/** One piece of a line's text: `mark` = a search match. Always plain text, never markup. */
export interface Segment {
  text: string;
  mark: boolean;
}

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/**
 * D5: `text` split into plain and matched segments for `query`, case-insensitive (Unicode
 * case folding, Cyrillic included), original case kept. The segments rebuild `text`
 * exactly; an empty query is one plain segment.
 */
export function segmentsFor(text: string, query: string): Segment[] {
  if (query === "") return [{ text, mark: false }];
  const re = new RegExp(escapeRegExp(query), "giu");
  const out: Segment[] = [];
  let at = 0;
  for (const match of text.matchAll(re)) {
    const start = match.index ?? 0;
    if (start > at) out.push({ text: text.slice(at, start), mark: false });
    out.push({ text: match[0], mark: true });
    at = start + match[0].length;
  }
  if (at < text.length || out.length === 0) out.push({ text: text.slice(at), mark: false });
  return out;
}

/** D5: the number of matches of `query` over every line. */
export function countMatches(lines: readonly { text: string }[], query: string): number {
  let n = 0;
  for (const line of lines) n += segmentsFor(line.text, query).filter((s) => s.mark).length;
  return n;
}

/** D5: "No matches" is shown for a non-empty query with no match. */
export function noMatches(lines: readonly { text: string }[], query: string): boolean {
  return query !== "" && countMatches(lines, query) === 0;
}

const IO_REASONS = ["missing", "not_a_folder", "access_denied", "disk_full", "other"] as const;
type IoReason = (typeof IO_REASONS)[number];

function isIoReason(reason: unknown): reason is IoReason {
  return typeof reason === "string" && (IO_REASONS as readonly string[]).includes(reason);
}

/**
 * D4: the catalog id for a refused calls_* command (copy, export, open folder, detail).
 * `ioFailed` maps to `calls.error.io.<reason>`, an unknown or absent reason to `.other` (the
 * raw word is never shown); anything else (notFound, a rejection that is not a CallError)
 * to `calls.error.io`.
 */
export function callErrorMessageId(error: unknown): MessageId {
  if (typeof error === "object" && error !== null && (error as { code?: unknown }).code === "ioFailed") {
    const reason = (error as { reason?: unknown }).reason;
    return isIoReason(reason) ? `calls.error.io.${reason}` : "calls.error.io.other";
  }
  return "calls.error.io";
}

const FAILED_ID: Readonly<Record<FailReason, MessageId>> = {
  key: "calls.failed.key",
  network: "calls.failed.network",
  timeout: "calls.failed.timeout",
  provider: "calls.failed.provider",
  storage: "calls.failed.storage",
  engineNotSetUp: "calls.failed.engine_not_set_up",
};

/** The catalog id of a failed call's reason. */
export function failedMessageId(reason: FailReason): MessageId {
  return FAILED_ID[reason];
}

const SPEAKER_COLOR: Readonly<Record<Speaker["id"], string>> = {
  me: "#1e88e5",
  s1: "#e65100",
};

/** D6: a fixed colour per speaker id, always shown next to the speaker's name. */
export function speakerColor(id: Speaker["id"]): string {
  return SPEAKER_COLOR[id];
}
