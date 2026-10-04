// T-004 red tests for the settings draft model (spec 004 T037; T-004 Investigation 4-5).
//
// API pinned here for src/lib/settings/draft.ts (pure TS, no Svelte runes, so vitest
// can load it; the page holds the Draft in $state and calls these):
//
//   interface Draft {
//     baseline: SettingsView;                 // the last applied view (what is saved)
//     settings: Settings;                     // the editable draft, a copy (never aliased)
//     keys: Record<KeySlot, KeyEdit>;         // one KeyEdit per slot, "Untouched" at start
//     errors: Record<string, string>;         // FieldId -> message id `error.<code>`
//     formError: FormError | null;            // the last Refused form_error
//   }
//   draftFromView(view): Draft
//   applyView(draft, view): Draft             // U1: the one path for settings_get,
//                                             //     Saved.view and settings://changed
//   isDirty(draft): boolean                   // settings differ from baseline, or a key edit
//   typeKey(draft, slot, text): Draft         // -> { Replace: text }
//   clearKey(draft, slot): Draft              // -> "Clear"
//   saveRequest(draft): SaveRequest           // { settings, keys }
//   applyOutcome(draft, outcome): Draft       // Saved -> clean draft of the view; Refused ->
//                                             // errors/formError set, draft kept
//   errorsByField(errors: FieldError[]): Record<string, string>
//   errorMessageId(code: string): string      // `error.${code}`, the one helper
//
// The wire types come from src/lib/settings/settingsApi.ts (contracts/ipc.md).
// The view data is core's real first-run view (e2e/fixtures/settings-wire.json, kept
// in sync by the core test e2e_settings_wire_fixture_matches_core).
import { describe, expect, it } from "vitest";
import enCatalog from "../../../i18n/en.json";
import ruCatalog from "../../../i18n/ru.json";
import wire from "../../../e2e/fixtures/settings-wire.json";
import {
  applyOutcome,
  applyView,
  clearKey,
  draftFromView,
  errorMessageId,
  errorsByField,
  isDirty,
  resetKey,
  saveRequest,
  typeKey,
} from "./draft";
import type { SaveOutcome, SettingsView } from "./settingsApi";

const FAKE_KEY = "sk-test-FAKE-1111-not-a-real-key";

/** A fresh copy of core's first-run view. */
function firstRun(): SettingsView {
  return structuredClone(wire.first_run_view) as SettingsView;
}

/** A view another window saved: engine api, history size 35, presence of the API key. */
function savedElsewhere(): SettingsView {
  const v = firstRun();
  v.first_run = false;
  v.settings.engine = "api";
  v.settings.history.size = 35;
  v.keys.transcription_api = true;
  return v;
}

const UNTOUCHED = { transcription_api: "Untouched", local_server: "Untouched", post_processing: "Untouched" };

// ---- U1: one applyView ------------------------------------------------------------

describe("draftFromView / applyView", () => {
  it("starts from the view: settings copied, every key Untouched, no errors, not dirty", () => {
    const view = firstRun();
    const d = draftFromView(view);
    expect(d.settings).toEqual(view.settings);
    expect(d.baseline).toEqual(view);
    expect(d.keys).toEqual(UNTOUCHED);
    expect(d.errors).toEqual({});
    expect(d.formError).toBeNull();
    expect(isDirty(d)).toBe(false);
  });

  it("the draft is a copy: editing it changes neither the baseline nor the view", () => {
    // Bite: draft.settings aliased to view.settings (isDirty would never turn true).
    const view = firstRun();
    const d = draftFromView(view);
    d.settings.api.model = "whisper-edited";
    d.settings.history.size = 7;
    expect(isDirty(d)).toBe(true);
    expect(d.baseline.settings.api.model).toBe(wire.first_run_view.settings.api.model);
    expect(view.settings.api.model).toBe(wire.first_run_view.settings.api.model);
    expect(view.settings.history.size).toBe(wire.first_run_view.settings.history.size);
  });

  it("on a clean draft, applyView replaces both the draft and the baseline", () => {
    const next = savedElsewhere();
    const d = applyView(draftFromView(firstRun()), next);
    expect(d.baseline).toEqual(next);
    expect(d.settings).toEqual(next.settings);
    expect(isDirty(d)).toBe(false);
    // Not aliased to the applied view either.
    d.settings.api.model = "whisper-edited";
    expect(next.settings.api.model).toBe(wire.first_run_view.settings.api.model);
  });

  it("on a dirty draft, applyView keeps the edits and key edits and moves the baseline", () => {
    let d = draftFromView(firstRun());
    d.settings.api.model = "whisper-edited";
    d = typeKey(d, "transcription_api", FAKE_KEY);
    const next = savedElsewhere();
    d = applyView(d, next);
    expect(d.baseline).toEqual(next);
    expect(d.settings.api.model).toBe("whisper-edited");
    expect(d.settings.history.size).toBe(wire.first_run_view.settings.history.size);
    expect(d.keys.transcription_api).toEqual({ Replace: FAKE_KEY });
    expect(isDirty(d)).toBe(true);
  });
});

// ---- dirty tracking -----------------------------------------------------------------

describe("isDirty", () => {
  it("compares values, not a flag: a field edited back to its saved value is clean", () => {
    const d = draftFromView(firstRun());
    d.settings.api.base_url = "https://stt.example.com/v1";
    expect(isDirty(d)).toBe(true);
    d.settings.api.base_url = wire.first_run_view.settings.api.base_url;
    expect(isDirty(d)).toBe(false);
  });

  it("a key edit alone makes the draft dirty (Replace or Clear)", () => {
    const base = draftFromView(firstRun());
    expect(isDirty(typeKey(base, "local_server", FAKE_KEY))).toBe(true);
    expect(isDirty(clearKey(base, "post_processing"))).toBe(true);
  });
});

// ---- U3: KeyEdit state ------------------------------------------------------------------

describe("KeyEdit per slot", () => {
  it("typing a key gives Replace with the text as typed; the other slots stay Untouched", () => {
    const d = typeKey(draftFromView(firstRun()), "transcription_api", ` ${FAKE_KEY} `);
    // The service trims (contracts/ipc.md KeyEdit); the UI sends the key unchanged.
    expect(d.keys).toEqual({ ...UNTOUCHED, transcription_api: { Replace: ` ${FAKE_KEY} ` } });
  });

  it("Clear gives Clear; typing after Clear gives Replace", () => {
    let d = clearKey(draftFromView(firstRun()), "local_server");
    expect(d.keys).toEqual({ ...UNTOUCHED, local_server: "Clear" });
    d = typeKey(d, "local_server", FAKE_KEY);
    expect(d.keys.local_server).toEqual({ Replace: FAKE_KEY });
  });

  it("saveRequest carries the whole draft and exactly one KeyEdit per slot", () => {
    let d = draftFromView(firstRun());
    d.settings.engine = "api";
    d.settings.api.model = "whisper-fake-1";
    d = typeKey(d, "transcription_api", FAKE_KEY);
    d = clearKey(d, "post_processing");
    const req = saveRequest(d);
    const expected = structuredClone(wire.first_run_view.settings);
    expected.engine = "api";
    expected.api.model = "whisper-fake-1";
    expect(req).toEqual({
      settings: expected,
      keys: { transcription_api: { Replace: FAKE_KEY }, local_server: "Untouched", post_processing: "Clear" },
    });
  });
});

// ---- outcomes and U2: FieldError -> field ---------------------------------------------

describe("applyOutcome", () => {
  it("Saved: the saved view becomes baseline and draft, keys reset, errors cleared, no key kept", () => {
    let d = draftFromView(firstRun());
    d.settings.engine = "api";
    d = typeKey(d, "transcription_api", FAKE_KEY);
    d = applyOutcome(d, {
      Refused: { errors: [{ field: "engine.api.model", code: "required" }], form_error: null },
    });
    const view = savedElsewhere();
    d = applyOutcome(d, { Saved: { view, warnings: [] } });
    expect(d.baseline).toEqual(view);
    expect(d.settings).toEqual(view.settings);
    expect(d.keys).toEqual(UNTOUCHED);
    expect(d.errors).toEqual({});
    expect(d.formError).toBeNull();
    expect(isDirty(d)).toBe(false);
    expect(JSON.stringify(d)).not.toContain(FAKE_KEY);
  });

  it("Refused: every field error is mapped to error.<code>; draft, key edits and baseline are kept", () => {
    const view = firstRun();
    let d = draftFromView(view);
    d.settings.engine = "api";
    d.settings.api.base_url = "";
    d = typeKey(d, "transcription_api", FAKE_KEY);
    const refused: SaveOutcome = {
      Refused: {
        errors: [
          { field: "engine.api.base_url", code: "required" },
          { field: "recording.hotkey", code: "hotkey.unavailable" },
        ],
        form_error: null,
      },
    };
    d = applyOutcome(d, refused);
    expect(d.errors).toEqual({
      "engine.api.base_url": "error.required",
      "recording.hotkey": "error.hotkey.unavailable",
    });
    expect(d.formError).toBeNull();
    expect(d.settings.engine).toBe("api");
    expect(d.settings.api.base_url).toBe("");
    expect(d.keys.transcription_api).toEqual({ Replace: FAKE_KEY });
    expect(d.baseline).toEqual(view);
    expect(isDirty(d)).toBe(true);
  });

  it("Refused with a form error keeps it; the next Refused replaces the field errors", () => {
    let d = draftFromView(firstRun());
    d = applyOutcome(d, {
      Refused: {
        errors: [{ field: "engine.api.key", code: "key.store_failed" }],
        form_error: {
          kind: "partially_restored",
          message: "settings.partially_restored",
          not_restored: ["engine.api.key"],
        },
      },
    });
    expect(d.formError).toEqual({
      kind: "partially_restored",
      message: "settings.partially_restored",
      not_restored: ["engine.api.key"],
    });
    expect(d.errors).toEqual({ "engine.api.key": "error.key.store_failed" });
    d = applyOutcome(d, {
      Refused: { errors: [{ field: "history.size", code: "history.size_range" }], form_error: null },
    });
    expect(d.errors).toEqual({ "history.size": "error.history.size_range" });
    expect(d.formError).toBeNull();
  });
});

describe("error ids", () => {
  it("errorsByField maps each FieldId to error.<code>", () => {
    expect(
      errorsByField([
        { field: "engine.api.base_url", code: "url.malformed" },
        { field: "history.size", code: "history.size_range" },
      ]),
    ).toEqual({ "engine.api.base_url": "error.url.malformed", "history.size": "error.history.size_range" });
    expect(errorsByField([])).toEqual({});
  });

  it("errorMessageId builds error.<code> for any code, so the UI holds no list of codes", () => {
    // Bite: a lookup table of known codes (a third copy of ErrorCode); a code added by a
    // later task (T-010, T-020, T-021) must map without a UI change.
    for (const code of ["required", "hotkey.unavailable", "language.unsupported", "future.code_added_later"]) {
      expect(errorMessageId(code)).toBe(`error.${code}`);
    }
  });
});

// ---- T-004 review r1 #4: resetKey (characterization) ----------------------------------

describe("resetKey", () => {
  it("typing a key and emptying the field again leaves the slot Untouched: no blank Replace is ever sent", () => {
    // Bite: KeyField always calling typeKey (an emptied field -> { Replace: "" }, which
    // core's validate reads as key.required for engine api despite a stored key).
    const view = savedElsewhere(); // engine api, a stored API key
    let d = draftFromView(view);
    d = typeKey(d, "transcription_api", FAKE_KEY);
    d = resetKey(d, "transcription_api");
    expect(d.keys).toEqual(UNTOUCHED);
    expect(isDirty(d)).toBe(false);
    expect(saveRequest(d).keys.transcription_api).toBe("Untouched");
    expect(JSON.stringify(d)).not.toContain(FAKE_KEY);
  });

  it("resetKey after Clear keeps the stored key (Untouched), and touches no other slot", () => {
    let d = clearKey(draftFromView(savedElsewhere()), "transcription_api");
    d = typeKey(d, "local_server", FAKE_KEY);
    d = resetKey(d, "transcription_api");
    expect(d.keys).toEqual({ ...UNTOUCHED, local_server: { Replace: FAKE_KEY } });
  });
});

// ---- T-004 review r1 #3: PartiallyRestored -> every not_restored field highlighted -----

/** The id is a catalog id with a non-empty text in both languages. */
function expectCatalogId(id: string | undefined): void {
  expect(id, "a highlighted field needs a message id").toBeDefined();
  const en = enCatalog as Record<string, string>;
  const ru = ruCatalog as Record<string, string>;
  expect(typeof en[id!] === "string" && en[id!] !== "", `i18n/en.json has no text for ${id}`).toBe(true);
  expect(typeof ru[id!] === "string" && ru[id!] !== "", `i18n/ru.json has no text for ${id}`).toBe(true);
}

describe("applyOutcome with partially_restored", () => {
  it("highlights each not_restored field (core's case: the error is on the local-server key, the API key is left changed)", () => {
    // Bite: applyOutcome mapping only `errors` (the field left changed is not highlighted,
    // while the alert says "check the highlighted fields").
    let d = draftFromView(savedElsewhere());
    d = typeKey(d, "transcription_api", FAKE_KEY);
    d = applyOutcome(d, {
      Refused: {
        errors: [{ field: "engine.local_server.key", code: "key.store_failed" }],
        form_error: {
          kind: "partially_restored",
          message: "settings.partially_restored",
          not_restored: ["engine.api.key"],
        },
      },
    });
    expect(d.errors["engine.local_server.key"]).toBe("error.key.store_failed");
    expectCatalogId(d.errors["engine.api.key"]);
    expect(Object.keys(d.errors).sort()).toEqual(["engine.api.key", "engine.local_server.key"]);
    expect(d.formError?.not_restored).toEqual(["engine.api.key"]);
    // The draft and its key edit are kept, as for any refusal.
    expect(d.keys.transcription_api).toEqual({ Replace: FAKE_KEY });
  });

  it("highlights every not_restored field when there is no field error (a file write whose undo failed)", () => {
    const d = applyOutcome(draftFromView(savedElsewhere()), {
      Refused: {
        errors: [],
        form_error: {
          kind: "partially_restored",
          message: "settings.partially_restored",
          not_restored: ["engine.api.key", "engine.local_server.key", "general.start_with_windows"],
        },
      },
    });
    expect(Object.keys(d.errors).sort()).toEqual([
      "engine.api.key",
      "engine.local_server.key",
      "general.start_with_windows",
    ]);
    for (const field of Object.keys(d.errors)) expectCatalogId(d.errors[field]);
  });

  it("the next outcome drops the not_restored highlights (Refused without form error, or Saved)", () => {
    const partial: SaveOutcome = {
      Refused: {
        errors: [],
        form_error: { kind: "partially_restored", message: "settings.partially_restored", not_restored: ["engine.api.key"] },
      },
    };
    let d = applyOutcome(draftFromView(savedElsewhere()), partial);
    d = applyOutcome(d, { Refused: { errors: [{ field: "history.size", code: "history.size_range" }], form_error: null } });
    expect(d.errors).toEqual({ "history.size": "error.history.size_range" });
    d = applyOutcome(applyOutcome(d, partial), { Saved: { view: savedElsewhere(), warnings: [] } });
    expect(d.errors).toEqual({});
  });
});
