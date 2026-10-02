import { describe, expect, it } from "vitest";
import { formatBuildInfo } from "./buildInfo";

describe("formatBuildInfo", () => {
  it("shows the product name, version and commit", () => {
    expect(formatBuildInfo({ version: "0.1.0", commit: "abc1234" })).toBe("Voicen 0.1.0 (abc1234)");
  });
});
