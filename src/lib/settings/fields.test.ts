// wholeNumber: the one entry rule of the whole-number settings fields (timeouts, history
// size). Pins today's behaviour (T-073 review 1 finding #4): an entry that is not a
// non-empty, safe, non-negative integer is sent as 0; anything Number() reads as such an
// integer is taken as is. The range is core's rule, never clamped here.
import { describe, expect, it } from "vitest";
import { wholeNumber } from "./fields";

describe("wholeNumber", () => {
  it.each([
    ["0", 0],
    ["7", 7],
    ["120", 120],
    ["9007199254740991", Number.MAX_SAFE_INTEGER],
    [" 30 ", 30],
    ["5.0", 5],
    ["1e3", 1000],
  ])("a whole number %j is sent as %d", (entry, expected) => {
    expect(wholeNumber(entry)).toBe(expected);
  });

  it.each(["", "   ", "1.5", "0.1", "-1", "-30", "abc", "12abc", "NaN", "Infinity", "9007199254740992"])(
    "an entry that is not a whole number (%j) is sent as 0",
    (entry) => {
      expect(wholeNumber(entry)).toBe(0);
    },
  );
});
