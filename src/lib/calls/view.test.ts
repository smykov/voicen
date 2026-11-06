// T-143 red tests: the pure half of the Calls page (D2 to D5; analysis invariant).
//
// Export contract of src/lib/calls/view.ts:
//   formatStartedAt(startedAt: string): string      "DD.MM.YYYY HH:MM" from the written digits (D2)
//   formatDuration(ms: number | null): string        "H:MM:SS", "MM:SS" under 1 h, null -> "—" (D2)
//   formatLineTime(startMs: number): string          same form as the duration (D2)
//   actionsFor(state: CallState): CallAction[]       CallAction = "copy" | "exportMd" | "exportJson" | "openFolder" | "delete" (D3)
//   segmentsFor(text: string, query: string): { text: string; mark: boolean }[]   case-insensitive (D5)
//   countMatches(lines: { text: string }[], query: string): number
//   noMatches(lines: { text: string }[], query: string): boolean   non-empty query and zero matches (D5)
//   callErrorMessageId(error: unknown): string       ioFailed{reason} -> calls.error.io.<reason>|.other, notFound -> calls.error.io (D4)
//   failedMessageId(reason: FailReason): string      "engineNotSetUp" -> calls.failed.engine_not_set_up
//   speakerColor(id: "me" | "s1"): string            a fixed CSS colour per speaker id (D6)
import { describe, expect, it } from "vitest";
import en from "../../../i18n/en.json";
import ru from "../../../i18n/ru.json";
import wire from "../../../e2e/fixtures/calls-wire.json";
import {
  actionsFor,
  callErrorMessageId,
  countMatches,
  failedMessageId,
  formatDuration,
  formatLineTime,
  formatStartedAt,
  noMatches,
  segmentsFor,
  speakerColor,
} from "./view";

const EN = en as Record<string, string>;
const RU = ru as Record<string, string>;
const REASONS = ["missing", "not_a_folder", "access_denied", "disk_full", "other"] as const;

describe("D2 formatStartedAt: the written digits, no timezone conversion", () => {
  // A value near midnight: a Date/UTC conversion shifts day or hour on any host zone other
  // than the written offset, so the check does not depend on the test host's TZ.
  it("with an offset: +03:00 late evening stays 23:30 on the same day", () => {
    expect(formatStartedAt("2026-10-30T23:30:00+03:00")).toBe("30.10.2026 23:30");
  });
  it("with a negative offset across a year end", () => {
    expect(formatStartedAt("2026-12-31T23:59:59-05:00")).toBe("31.12.2026 23:59");
  });
  it("with Z", () => {
    expect(formatStartedAt("2026-01-01T00:05:00Z")).toBe("01.01.2026 00:05");
  });
  it("without an offset (damaged call)", () => {
    expect(formatStartedAt("2026-10-26T08:15:07")).toBe("26.10.2026 08:15");
  });
  it("the fixture's every startedAt matches its own title digits when the title is the default", () => {
    for (const c of wire.list.calls) {
      const m = /^Call (\d\d)\.(\d\d) (\d\d):(\d\d)$/.exec(c.title);
      if (m === null) continue;
      expect(formatStartedAt(c.startedAt)).toBe(`${m[1]}.${m[2]}.${c.startedAt.slice(0, 4)} ${m[3]}:${m[4]}`);
    }
  });
});

describe("D2 durations and line times", () => {
  it("MM:SS under one hour, H:MM:SS from one hour, whole seconds floored", () => {
    expect(formatDuration(1_800_000)).toBe("30:00");
    expect(formatDuration(60_000)).toBe("01:00");
    expect(formatDuration(59_999)).toBe("00:59");
    expect(formatDuration(3_600_000)).toBe("1:00:00");
    expect(formatDuration(3_725_000)).toBe("1:02:05");
  });
  it("null shows a dash", () => {
    expect(formatDuration(null)).toBe("—");
  });
  it("a line time is startMs in the same form", () => {
    expect(formatLineTime(0)).toBe("00:00");
    expect(formatLineTime(1500)).toBe("00:01");
    expect(formatLineTime(3_661_000)).toBe("1:01:01");
  });
});

describe("D3 actionsFor: one pure function of the state", () => {
  it("ready: copy, both exports, open folder, delete", () => {
    expect(actionsFor("ready")).toEqual(["copy", "exportMd", "exportJson", "openFolder", "delete"]);
  });
  it("recording: open folder only", () => {
    expect(actionsFor("recording")).toEqual(["openFolder"]);
  });
  it("damaged: open folder and delete only", () => {
    expect(actionsFor("damaged")).toEqual(["openFolder", "delete"]);
  });
  for (const state of ["queued", "transcribing", "failed", "recovered"] as const) {
    it(`${state}: open folder and delete`, () => {
      expect(actionsFor(state)).toEqual(["openFolder", "delete"]);
    });
  }
});

describe("D5 search: case-insensitive substring, text and mark segments", () => {
  it("an empty query is one plain segment", () => {
    expect(segmentsFor("Hello there", "")).toEqual([{ text: "Hello there", mark: false }]);
  });
  it("marks every match and keeps the original case", () => {
    expect(segmentsFor("The Contract and the contract", "CONTRACT")).toEqual([
      { text: "The ", mark: false },
      { text: "Contract", mark: true },
      { text: " and the ", mark: false },
      { text: "contract", mark: true },
    ]);
  });
  it("Cyrillic is case-insensitive", () => {
    expect(segmentsFor("Договор подписан. договор", "ДОГОВОР")).toEqual([
      { text: "Договор", mark: true },
      { text: " подписан. ", mark: false },
      { text: "договор", mark: true },
    ]);
  });
  it("markup in the text stays text (segments are plain strings)", () => {
    const segs = segmentsFor("<b>x</b> contract", "contract");
    expect(segs.map((s) => s.text).join("")).toBe("<b>x</b> contract");
    expect(segs.filter((s) => s.mark)).toEqual([{ text: "contract", mark: true }]);
  });
  it("the segments always rebuild the text", () => {
    for (const q of ["", "e", "ll", "zzz", "HELLO THERE"]) {
      expect(segmentsFor("Hello there", q).map((s) => s.text).join("")).toBe("Hello there");
    }
  });
  it("counts matches over all lines; noMatches only for a non-empty query with none", () => {
    const lines = [{ text: "Hello there" }, { text: "Hi" }];
    expect(countMatches(lines, "h")).toBe(3);
    expect(noMatches(lines, "h")).toBe(false);
    expect(countMatches(lines, "contract")).toBe(0);
    expect(noMatches(lines, "contract")).toBe(true);
    expect(noMatches(lines, "")).toBe(false);
    expect(noMatches([], "")).toBe(false);
  });
});

describe("D4 error to message id", () => {
  for (const reason of REASONS) {
    it(`ioFailed ${reason} -> calls.error.io.${reason}`, () => {
      expect(callErrorMessageId({ code: "ioFailed", reason })).toBe(`calls.error.io.${reason}`);
    });
  }
  it("the fixture's ioFailed (access_denied) maps to its own id", () => {
    expect(callErrorMessageId(wire.errors.ioFailed)).toBe("calls.error.io.access_denied");
  });
  it("an unknown or absent reason maps to .other", () => {
    expect(callErrorMessageId({ code: "ioFailed", reason: "relative" })).toBe("calls.error.io.other");
    expect(callErrorMessageId({ code: "ioFailed", reason: "../x" })).toBe("calls.error.io.other");
    expect(callErrorMessageId({ code: "ioFailed" })).toBe("calls.error.io.other");
  });
  it("notFound maps to calls.error.io", () => {
    expect(callErrorMessageId(wire.errors.notFound)).toBe("calls.error.io");
  });
  it("every id the mapper can return exists in EN and RU with a non-empty text", () => {
    const inputs: unknown[] = [
      ...REASONS.map((reason) => ({ code: "ioFailed", reason })),
      { code: "ioFailed" },
      { code: "ioFailed", reason: "nope" },
      wire.errors.notFound,
    ];
    for (const input of inputs) {
      const id = callErrorMessageId(input);
      expect(EN[id], `en ${id}`).toBeTruthy();
      expect(RU[id], `ru ${id}`).toBeTruthy();
    }
  });
});

describe("catalog: the new calls.error.io.* ids exist in EN and RU", () => {
  for (const reason of REASONS) {
    it(`calls.error.io.${reason}`, () => {
      expect(EN[`calls.error.io.${reason}`]).toBeTruthy();
      expect(RU[`calls.error.io.${reason}`]).toBeTruthy();
    });
  }
});

describe("failed reasons and speaker colours", () => {
  it("every FailReason of the fixture maps to an existing calls.failed.* id", () => {
    const reasons = wire.list.calls.flatMap((c) => ("failReason" in c ? [c.failReason as never] : []));
    expect(reasons.length).toBe(6);
    for (const r of reasons) {
      const id = failedMessageId(r);
      expect(EN[id], `en ${id}`).toBeTruthy();
      expect(RU[id], `ru ${id}`).toBeTruthy();
    }
    expect(failedMessageId("engineNotSetUp" as never)).toBe("calls.failed.engine_not_set_up");
    expect(failedMessageId("key" as never)).toBe("calls.failed.key");
  });
  it("D6: a fixed, distinct colour per speaker id", () => {
    expect(speakerColor("me")).toBeTruthy();
    expect(speakerColor("s1")).toBeTruthy();
    expect(speakerColor("me")).not.toBe(speakerColor("s1"));
    expect(speakerColor("me")).toBe(speakerColor("me"));
  });
});
