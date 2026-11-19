// T-022 red tests: the `history.*` texts of the History window exist in the shared catalog in
// English and Russian (spec 005 FR-013; contracts/ipc.md "UI contract"). The English literals
// are the contract's: the e2e spec (e2e/history.spec.ts) locates by them.
import { describe, expect, it } from "vitest";
import en from "../../../i18n/en.json";
import ru from "../../../i18n/ru.json";

const enMap = en as Record<string, string>;
const ruMap = ru as Record<string, string>;

const ENGLISH: Record<string, string> = {
  "history.list_label": "History",
  "history.copy": "Copy",
  "history.copied": "Copied",
  "history.copy_failed": "Could not copy — try again",
  "history.clear": "Clear",
  "history.empty": "No transcripts yet",
  "history.off": "History is off",
  "history.open_settings": "Open settings",
  "history.notice.save_failed": "History could not be saved to disk",
  "history.notice.delete_failed": "History could not be deleted from disk",
  "history.engine.api": "API",
  "history.engine.local": "Local",
  "history.engine.server": "Server",
};
const MORE = ["history.title", "history.expand", "history.collapse"];

describe("history.* catalog keys", () => {
  for (const [id, text] of Object.entries(ENGLISH)) {
    it(`${id} is "${text}" in English and has its own Russian text`, () => {
      expect(enMap[id]).toBe(text);
      expect(ruMap[id], `${id} in ru.json`).toBeTruthy();
      if (id !== "history.engine.api") expect(ruMap[id]).not.toBe(text); // "API" is "API" in both
    });
  }

  for (const id of MORE) {
    it(`${id} exists and is non-empty in both catalogs`, () => {
      expect(enMap[id], `${id} in en.json`).toBeTruthy();
      expect(ruMap[id], `${id} in ru.json`).toBeTruthy();
    });
  }

  it("the two catalogs hold the same history.* ids", () => {
    const ids = (m: Record<string, string>) => Object.keys(m).filter((k) => k.startsWith("history.")).sort();
    expect(ids(ruMap)).toEqual(ids(enMap));
    expect(ids(enMap).length).toBeGreaterThanOrEqual(Object.keys(ENGLISH).length + MORE.length);
  });
});
