// T-155 red tests (decision #319 Q2; settings-ui I2, I3, I5): how `errorsByField` maps the
// wire FieldErrors the Calls tab shows.
// - A FieldError that carries `reason` (core sends one only for storage.unwritable, closed
//   union UnwritableReason, snake_case) maps to that reason's own UI-only message id
//   `settings.calls.unwritable.<reason>`; one id per reason, chosen by one exhaustive switch.
// - Every other error (no reason) maps to `error.<code>`, a Refused calls.hotkey included.
// - The six ids have EN and RU text (FR-046); the generic `error.storage.unwritable` stays.
import { describe, expect, it } from "vitest";
import enCatalog from "../../../i18n/en.json";
import ruCatalog from "../../../i18n/ru.json";
import wire from "../../../e2e/fixtures/settings-wire.json";
import { applyOutcome, draftFromView, errorsByField } from "./draft";
import type { FieldError, SaveOutcome, SettingsView } from "./settingsApi";

const EN = enCatalog as Record<string, string>;
const RU = ruCatalog as Record<string, string>;

/** The closed UnwritableReason union of core (roots.rs), snake_case on the wire. */
const REASONS = ["missing", "not_a_folder", "relative", "access_denied", "disk_full", "other"] as const;

/** A wire FieldError with a reason (`reason` is typed by the T-155 change; cast until then). */
function withReason(field: string, code: string, reason: string): FieldError {
  return { field, code, reason } as unknown as FieldError;
}

describe("errorsByField: the reason of a storage.unwritable error (I3)", () => {
  it("core's refused_storage_unwritable sample maps calls.storage_root to settings.calls.unwritable.access_denied", () => {
    const outcome = (wire as unknown as { refused_storage_unwritable: { Refused: { errors: FieldError[] } } })
      .refused_storage_unwritable;
    expect(errorsByField(outcome.Refused.errors)).toEqual({
      "calls.storage_root": "settings.calls.unwritable.access_denied",
    });
  });

  it.each(REASONS)("reason %s maps to its own settings.calls.unwritable id", (reason) => {
    expect(errorsByField([withReason("calls.storage_root", "storage.unwritable", reason)])).toEqual({
      "calls.storage_root": `settings.calls.unwritable.${reason}`,
    });
  });

  it("the six reasons map to six distinct ids", () => {
    const ids = REASONS.map(
      (reason) => errorsByField([withReason("calls.storage_root", "storage.unwritable", reason)])["calls.storage_root"],
    );
    expect(new Set(ids).size).toBe(REASONS.length);
  });

  it("a storage.unwritable error without a reason maps to error.storage.unwritable (no reason is invented)", () => {
    expect(errorsByField([{ field: "calls.storage_root", code: "storage.unwritable" }])).toEqual({
      "calls.storage_root": "error.storage.unwritable",
    });
  });

  it("an error without a reason maps to error.<code>, whatever its field", () => {
    expect(
      errorsByField([
        { field: "history.size", code: "history.size_range" },
        { field: "calls.primary_language", code: "language.unsupported" },
      ]),
    ).toEqual({
      "history.size": "error.history.size_range",
      "calls.primary_language": "error.language.unsupported",
    });
  });

  it("the first error of a field wins, with or without a reason", () => {
    expect(
      errorsByField([
        withReason("calls.storage_root", "storage.unwritable", "disk_full"),
        withReason("calls.storage_root", "storage.unwritable", "missing"),
      ]),
    ).toEqual({ "calls.storage_root": "settings.calls.unwritable.disk_full" });
  });

  it("an applied Refused outcome keeps the reason's id in the draft's errors", () => {
    const view = structuredClone(wire.first_run_view) as unknown as SettingsView;
    const outcome = (wire as unknown as { refused_storage_unwritable: SaveOutcome }).refused_storage_unwritable;
    const next = applyOutcome(draftFromView(view), outcome);
    expect(next.errors).toEqual({ "calls.storage_root": "settings.calls.unwritable.access_denied" });
  });
});

describe("errorsByField: a Refused calls.hotkey (I2)", () => {
  it("hotkey.equals_dictation maps to error.hotkey.equals_dictation on calls.hotkey", () => {
    expect(errorsByField([{ field: "calls.hotkey", code: "hotkey.equals_dictation" }])).toEqual({
      "calls.hotkey": "error.hotkey.equals_dictation",
    });
  });

  it("hotkey.unavailable maps to error.hotkey.unavailable on calls.hotkey", () => {
    expect(errorsByField([{ field: "calls.hotkey", code: "hotkey.unavailable" }])).toEqual({
      "calls.hotkey": "error.hotkey.unavailable",
    });
  });
});

describe("the six unwritable-reason messages have EN and RU text (I5, FR-046)", () => {
  it.each(REASONS)("reason %s has a settings.calls.unwritable text, non-empty EN and RU text, no placeholder, RU differs from EN", (reason) => {
    const id = `settings.calls.unwritable.${reason}`;
    expect(typeof EN[id], `en.json ${id}`).toBe("string");
    expect(typeof RU[id], `ru.json ${id}`).toBe("string");
    expect(EN[id].trim()).not.toBe("");
    expect(RU[id].trim()).not.toBe("");
    expect(EN[id]).not.toMatch(/\{/);
    expect(RU[id]).not.toMatch(/\{/);
    expect(RU[id]).not.toBe(EN[id]);
    // A full sentence, not a fragment (decision #319 Q2).
    expect(EN[id]).toMatch(/^This folder can't be written: /);
    expect(RU[id]).toMatch(/[А-Яа-яЁё]/);
  });

  it("the access_denied text is the owner's example, verbatim", () => {
    expect(EN["settings.calls.unwritable.access_denied"]).toBe("This folder can't be written: access denied");
  });

  it("the six texts are distinct in EN and in RU", () => {
    for (const catalog of [EN, RU]) {
      const texts = REASONS.map((reason) => catalog[`settings.calls.unwritable.${reason}`]);
      expect(new Set(texts).size).toBe(REASONS.length);
    }
  });

  it("the generic error.storage.unwritable text stays, without a placeholder (core's i18n test pins it)", () => {
    expect(EN["error.storage.unwritable"]).not.toMatch(/\{/);
    expect(RU["error.storage.unwritable"]).not.toMatch(/\{/);
  });
});
