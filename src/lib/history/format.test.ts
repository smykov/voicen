// T-022 red tests: the pure half of the History window (specs/005-transcript-history tasks
// T008, FR-005, FR-013). Export contract of src/lib/history/format.ts (the developer implements
// against it); no IPC, no global clock, no global time zone, no global language:
//   previewOf(text): { preview: string; truncated: boolean }
//       the first 3 lines or the first 200 characters, whichever is shorter; truncated is true
//       only when something was cut (the expand toggle exists only then)
//   formatReadyAt(readyAtMs, { now, lang, timeZone }): string
//       the time only for an entry of the same calendar day as `now` in `timeZone`, date and
//       time otherwise; local time of that zone, in the language's format
//   engineLabel({ kind, model }, lang): string
//       "<kind name> · <model>" with the kind names of the catalog (history.engine.api|local|server);
//       an empty model (a local server with none configured) shows the kind name alone
import { describe, expect, it } from "vitest";
import en from "../../../i18n/en.json";
import ru from "../../../i18n/ru.json";
import { engineLabel, formatReadyAt, previewOf } from "./format";

const catalogs = { en, ru } as Record<"en" | "ru", Record<string, string>>;

describe("previewOf: 3 lines or 200 characters, whichever is shorter", () => {
  it("a short one-line text is its own preview and needs no expand", () => {
    expect(previewOf("hello world")).toEqual({ preview: "hello world", truncated: false });
  });

  it("exactly 3 lines are shown whole", () => {
    expect(previewOf("a\nb\nc")).toEqual({ preview: "a\nb\nc", truncated: false });
  });

  it("a fourth line is cut and the text needs expanding", () => {
    expect(previewOf("a\nb\nc\nd")).toEqual({ preview: "a\nb\nc", truncated: true });
  });

  it("CRLF line ends count as one line break each", () => {
    const got = previewOf("a\r\nb\r\nc\r\nd (fake)");
    expect(got.truncated).toBe(true);
    expect(got.preview).not.toContain("d (fake)");
    expect(got.preview.startsWith("a")).toBe(true);
    expect(got.preview).toContain("c");
  });

  it("exactly 200 characters on one line are shown whole", () => {
    const text = "x".repeat(200);
    expect(previewOf(text)).toEqual({ preview: text, truncated: false });
  });

  it("201 characters are cut to the first 200 and need expanding", () => {
    const text = `${"x".repeat(200)}y`;
    expect(previewOf(text)).toEqual({ preview: "x".repeat(200), truncated: true });
  });

  it("the character limit wins over the line limit: three 100-character lines give 200 characters", () => {
    const text = ["a".repeat(100), "b".repeat(100), "c".repeat(100)].join("\n");
    const got = previewOf(text);
    expect(got.truncated).toBe(true);
    expect(got.preview.length).toBeLessThanOrEqual(200);
    expect(text.startsWith(got.preview)).toBe(true);
    expect(got.preview.length).toBeGreaterThan(150); // not cut at the first line
  });

  it("the line limit wins over the character limit: four short lines give 3", () => {
    expect(previewOf("one\ntwo\nthree\nfour")).toEqual({ preview: "one\ntwo\nthree", truncated: true });
  });

  it("a long first line is cut to 200 characters with no later line", () => {
    const got = previewOf(`${"z".repeat(300)}\nsecond line (fake)`);
    expect(got).toEqual({ preview: "z".repeat(200), truncated: true });
  });
});

describe("formatReadyAt: time only for today, date and time otherwise, in the given zone and language", () => {
  const at = Date.UTC(2026, 9, 30, 14, 5); // 30 Oct 2026 14:05 UTC
  const sameDay = Date.UTC(2026, 9, 30, 20, 0);
  const nextDay = Date.UTC(2026, 9, 31, 10, 0);

  it("ru, today: the 24-hour time alone", () => {
    expect(formatReadyAt(at, { now: sameDay, lang: "ru", timeZone: "UTC" })).toBe("14:05");
  });

  it("en, today: the time without a date or year", () => {
    const got = formatReadyAt(at, { now: sameDay, lang: "en", timeZone: "UTC" });
    expect(got).toMatch(/(^|\D)(0?2:05|14:05)(\D|$)/);
    expect(got).not.toMatch(/2026|30/);
  });

  it("an earlier day adds the date to the time, in ru and en", () => {
    for (const lang of ["ru", "en"] as const) {
      const got = formatReadyAt(at, { now: nextDay, lang, timeZone: "UTC" });
      expect(got).toMatch(/(2:05|14:05)/);
      expect(got).toContain("30");
      expect(got).not.toBe(formatReadyAt(at, { now: sameDay, lang, timeZone: "UTC" }));
    }
  });

  it("the time is local to the zone, not UTC", () => {
    const now = Date.UTC(2026, 9, 30, 14, 30); // 23:30 the same day in Tokyo
    expect(formatReadyAt(at, { now, lang: "ru", timeZone: "Asia/Tokyo" })).toBe("23:05");
  });

  it("'today' is the zone's day: 23:30 UTC and 00:10 UTC are one day in Moscow (UTC+3), two in UTC", () => {
    const late = Date.UTC(2026, 9, 30, 23, 30);
    const now = Date.UTC(2026, 9, 31, 0, 10);
    expect(formatReadyAt(late, { now, lang: "ru", timeZone: "Europe/Moscow" })).toBe("02:30");
    const utc = formatReadyAt(late, { now, lang: "ru", timeZone: "UTC" });
    expect(utc).toContain("23:30");
    expect(utc).not.toBe("23:30");
  });
});

describe("engineLabel: catalog kind name, a middle dot, the model", () => {
  it("en: API · whisper-1, Local · small, Server · <model>", () => {
    expect(engineLabel({ kind: "api", model: "whisper-1" }, "en")).toBe("API · whisper-1");
    expect(engineLabel({ kind: "local", model: "small" }, "en")).toBe("Local · small");
    expect(engineLabel({ kind: "server", model: "llama-fake" }, "en")).toBe("Server · llama-fake");
  });

  it("ru: the kind names come from the Russian catalog", () => {
    for (const kind of ["api", "local", "server"] as const) {
      const name = catalogs.ru[`history.engine.${kind}`];
      expect(name, `history.engine.${kind} in ru.json`).toBeTruthy();
      expect(engineLabel({ kind, model: "m-fake" }, "ru")).toBe(`${name} · m-fake`);
    }
  });

  it("an empty model shows the kind name alone, with no dangling separator", () => {
    expect(engineLabel({ kind: "server", model: "" }, "en")).toBe("Server");
  });
});
