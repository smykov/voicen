// The pure half of the History window (specs/005-transcript-history spec FR-005, FR-013;
// T-022). No IPC, no global clock, time zone or language: each function is given them.
import { t, type MessageId, type UiLanguage } from "$lib/i18n";
import type { HistoryEngineKind } from "./historyApi";

const PREVIEW_LINES = 3;
const PREVIEW_CHARS = 200;
const LINE_BREAK = /\r\n|\n|\r/g;

export interface Preview {
  /** The leading part of the text, as written (line ends kept). */
  preview: string;
  /** True only when something was cut: the expand toggle exists only then. */
  truncated: boolean;
}

/** The first 3 lines or the first 200 characters, whichever is shorter. */
export function previewOf(text: string): Preview {
  let end = text.length;
  let lines = 0;
  for (const m of text.matchAll(LINE_BREAK)) {
    lines += 1;
    if (lines === PREVIEW_LINES) {
      end = m.index;
      break;
    }
  }
  if (end > PREVIEW_CHARS) {
    end = PREVIEW_CHARS;
    // Never split a surrogate pair.
    const code = text.charCodeAt(end - 1);
    if (code >= 0xd800 && code <= 0xdbff) end -= 1;
  }
  const preview = text.slice(0, end);
  return { preview, truncated: preview.length < text.length };
}

export interface ReadyAtOptions {
  /** The present, epoch milliseconds. */
  now: number;
  lang: UiLanguage;
  /** An IANA zone; the window passes the system's. */
  timeZone: string;
}

const LOCALE: Record<UiLanguage, string> = { en: "en-US", ru: "ru-RU" };

function dayKey(ms: number, timeZone: string): string {
  // en-CA writes YYYY-MM-DD: a calendar day of that zone.
  return new Intl.DateTimeFormat("en-CA", { timeZone, year: "numeric", month: "2-digit", day: "2-digit" }).format(ms);
}

/** Time only for an entry of the same calendar day as `now` in `timeZone`, else date and time. */
export function formatReadyAt(readyAtMs: number, { now, lang, timeZone }: ReadyAtOptions): string {
  const time: Intl.DateTimeFormatOptions = { hour: lang === "ru" ? "2-digit" : "numeric", minute: "2-digit" };
  const sameDay = dayKey(readyAtMs, timeZone) === dayKey(now, timeZone);
  const sameYear = dayKey(readyAtMs, timeZone).slice(0, 4) === dayKey(now, timeZone).slice(0, 4);
  const options: Intl.DateTimeFormatOptions = sameDay
    ? time
    : { ...time, day: "numeric", month: "short", ...(sameYear ? {} : { year: "numeric" }) };
  return new Intl.DateTimeFormat(LOCALE[lang], { ...options, timeZone }).format(readyAtMs);
}

function kindMessageId(kind: HistoryEngineKind): MessageId {
  return `history.engine.${kind}`;
}

/** "<kind name> · <model>", or the kind name alone when there is no model. */
export function engineLabel(engine: { kind: HistoryEngineKind; model: string }, lang: UiLanguage): string {
  const name = t(kindMessageId(engine.kind), {}, lang);
  return engine.model === "" ? name : `${name} · ${engine.model}`;
}
