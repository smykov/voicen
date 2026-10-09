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
  applyTestErrors,
  applyView,
  clearKey,
  draftFromView,
  errorMessageId,
  errorsByField,
  isDirty,
  resetKey,
  saveRequest,
  testRequest,
  typeKey,
  warningsByField,
} from "./draft";
import type { SaveOutcome, SettingsView, Warning } from "./settingsApi";

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

// ---- T-015: warnings of a Saved outcome (decision #52) ---------------------------------
//
// API pinned here for src/lib/settings/draft.ts:
//
//   warningsByField(warnings: readonly Warning[]): Record<string, MessageId>
//     // FieldId -> the warning's own `message` (a core MessageId, typed at the wire
//     // boundary: Warning.message: MessageId); the first warning of a field wins.
//
// Core decides which URL warns (settings::url::is_insecure_remote); the UI only maps
// what it is given, and keeps the warnings in page state, outside the Draft.

type SavedOutcome = Extract<SaveOutcome, { Saved: unknown }>;

/** Core's Saved outcome of an engine-api save of http://example.com/v1 (a fresh copy). */
function savedInsecureApi(): SavedOutcome {
  return structuredClone(wire.saved_insecure_api) as unknown as SavedOutcome;
}

describe("warningsByField (T-015)", () => {
  it("maps core's warning to its field with the message id as given; no warnings -> {}", () => {
    // Bite: a missing warningsByField, or one keyed by code instead of field.
    expect(warningsByField(savedInsecureApi().Saved.warnings)).toEqual({
      "engine.api.base_url": "settings.warning.endpoint_insecure",
    });
    expect(warningsByField([])).toEqual({});
  });

  it("uses each warning's message, never an id built from its code (no list of warning codes in the UI)", () => {
    // Bite: `warning.<code>` / `settings.warning.<code>` built in the UI, or a code ->
    // id table: a code added later, or a message core maps differently, must render as sent.
    const warnings = [
      { field: "post_processing.base_url", code: "future.code_added_later", message: "settings.warning.endpoint_insecure" },
      { field: "engine.local_server.base_url", code: "endpoint.insecure", message: "settings.saved" },
    ] as unknown as Warning[];
    expect(warningsByField(warnings)).toEqual({
      "post_processing.base_url": "settings.warning.endpoint_insecure",
      "engine.local_server.base_url": "settings.saved",
    });
  });

  it("the first warning of a field wins; every field keeps its own", () => {
    const warnings = [
      { field: "engine.api.base_url", code: "endpoint.insecure", message: "settings.warning.endpoint_insecure" },
      { field: "post_processing.base_url", code: "endpoint.insecure", message: "settings.warning.endpoint_insecure" },
      { field: "engine.api.base_url", code: "endpoint.insecure", message: "settings.saved" },
    ] as unknown as Warning[];
    expect(warningsByField(warnings)).toEqual({
      "engine.api.base_url": "settings.warning.endpoint_insecure",
      "post_processing.base_url": "settings.warning.endpoint_insecure",
    });
  });

  it("every warning message core sends has a text in both catalogs (en and ru differ)", () => {
    // Bite: the catalog text of settings.warning.endpoint_insecure missing in en or ru.
    for (const { message } of savedInsecureApi().Saved.warnings) {
      expectCatalogId(message);
      const en = enCatalog as Record<string, string>;
      const ru = ruCatalog as Record<string, string>;
      expect(ru[message]).not.toBe(en[message]);
    }
  });
});

describe("Saved warnings stay out of the Draft (T-015, characterization)", () => {
  it("applyOutcome on a Saved with warnings gives exactly the clean draft of its view", () => {
    // Pins today's behaviour: the Draft carries no warnings, because the
    // settings://changed echo of the same save goes through applyView -> draftFromView
    // and would wipe them; the page keeps them beside `saved` instead.
    const outcome = savedInsecureApi();
    expect(outcome.Saved.warnings).toHaveLength(1);
    let d = typeKey(draftFromView(firstRun()), "transcription_api", FAKE_KEY);
    d = applyOutcome(d, outcome);
    expect(d).toEqual(draftFromView(outcome.Saved.view));
    expect(Object.keys(d).sort()).toEqual(["baseline", "errors", "formError", "keys", "settings"]);
    // The echo of the save leaves the same clean draft.
    expect(applyView(d, outcome.Saved.view)).toEqual(draftFromView(outcome.Saved.view));
  });
});

// ---- T-073: the timeouts wire object (decisions #97, #99) --------------------------

describe("timeouts on the wire (T-073)", () => {
  const FR24 = { connect_s: 5, api_transcription_s: 30, local_server_s: 60, post_processing_s: 15, builtin_local_s: 120 };
  // In range and different from every default.
  const STORED = { connect_s: 7, api_transcription_s: 45, local_server_s: 90, post_processing_s: 20, builtin_local_s: 300 };

  function withTimeouts(timeouts: typeof STORED): SettingsView {
    const v = firstRun();
    v.first_run = false;
    v.settings.timeouts = { ...timeouts };
    return v;
  }

  it("core's first-run view carries the timeouts object with the FR-24 defaults", () => {
    // Red until e2e/fixtures/settings-wire.json is regenerated from core (never by hand).
    expect(firstRun().settings.timeouts).toEqual(FR24);
  });

  it("the timeouts object round-trips through the draft unchanged: view -> draft -> saveRequest -> Saved", () => {
    // Bite: a draft or saveRequest that rebuilds settings field by field and drops
    // (or defaults) the timeouts object.
    const view = withTimeouts(STORED);
    const d = draftFromView(view);
    expect(d.settings.timeouts).toEqual(STORED);
    expect(isDirty(d)).toBe(false);
    expect(saveRequest(d).settings.timeouts).toEqual(STORED);
    expect(saveRequest(d).settings).toEqual(view.settings);
    const saved = applyOutcome(d, { Saved: { view: withTimeouts(STORED), warnings: [] } });
    expect(saved.settings.timeouts).toEqual(STORED);
    expect(isDirty(saved)).toBe(false);
  });

  it("an edited timeout is dirty, sent as edited, and a copy (the view is not changed)", () => {
    const view = withTimeouts(STORED);
    const d = draftFromView(view);
    d.settings.timeouts.local_server_s = 1800;
    expect(isDirty(d)).toBe(true);
    expect(saveRequest(d).settings.timeouts).toEqual({ ...STORED, local_server_s: 1800 });
    expect(view.settings.timeouts).toEqual(STORED);
    expect(d.baseline.settings.timeouts).toEqual(STORED);
    d.settings.timeouts.local_server_s = STORED.local_server_s;
    expect(isDirty(d)).toBe(false);
  });

  it("a Refused timeouts.<role> timeout.range maps to error.timeout.range on that field; the typed timeouts are kept", () => {
    const d = draftFromView(withTimeouts(STORED));
    d.settings.timeouts.api_transcription_s = 601;
    const refused: SaveOutcome = {
      Refused: { errors: [{ field: "timeouts.api_transcription", code: "timeout.range" }], form_error: null },
    };
    const after = applyOutcome(d, refused);
    expect(after.errors).toEqual({ "timeouts.api_transcription": "error.timeout.range" });
    expect(after.settings.timeouts).toEqual({ ...STORED, api_transcription_s: 601 });
    expect(after.baseline.settings.timeouts).toEqual(STORED);
  });
});

// ---- T-013: the connection-test request and its Invalid errors ------------------------
//
//   testRequest(draft): ConnectionTestRequest | null
//     api          -> { engine: "api", base_url: settings.api.base_url, model: settings.api.model,
//                       key: keys.transcription_api, timeouts: settings.timeouts }
//     local_server -> { engine: "local_server", ...settings.local_server, key: keys.local_server, timeouts }
//     none, builtin_local -> null
//   Values are copied as in the draft (no trim, no validation: core normalizes and checks).
//
//   applyTestErrors(draft, errors: FieldError[]): Draft
//     errors = errorsByField(errors) (U2), replacing the previous highlights (choice (i));
//     settings, keys, baseline and formError are kept.

describe("testRequest (T-013)", () => {
  function apiDraft() {
    const view = firstRun();
    view.settings.engine = "api";
    return draftFromView(view);
  }

  it("api: the draft's api URL and model, the transcription_api KeyEdit and the draft's timeouts, nothing else", () => {
    let d = apiDraft();
    d.settings.api.base_url = "https://api.example.com/v1";
    d.settings.api.model = "whisper-fake-0013";
    d.settings.timeouts.connect_s = 7;
    d = typeKey(d, "transcription_api", FAKE_KEY);
    const request = testRequest(d);
    expect(request).toEqual({
      engine: "api",
      base_url: "https://api.example.com/v1",
      model: "whisper-fake-0013",
      key: { Replace: FAKE_KEY },
      timeouts: { ...wire.first_run_view.settings.timeouts, connect_s: 7 },
    });
    expect(Object.keys(request!).sort()).toEqual(["base_url", "engine", "key", "model", "timeouts"]);
  });

  it("local_server: the local_server URL and model and the local_server KeyEdit; the api key slot is never used", () => {
    const view = firstRun();
    view.settings.engine = "local_server";
    let d = draftFromView(view);
    d = typeKey(d, "transcription_api", FAKE_KEY);
    expect(testRequest(d)).toEqual({
      engine: "local_server",
      base_url: wire.first_run_view.settings.local_server.base_url,
      model: "",
      key: "Untouched",
      timeouts: wire.first_run_view.settings.timeouts,
    });
    d = typeKey(d, "local_server", "local-FAKE-0013");
    expect(testRequest(d)!.key).toEqual({ Replace: "local-FAKE-0013" });
  });

  it("api ignores a key typed for local_server or post_processing; Clear is sent as Clear (the save's semantics)", () => {
    let d = apiDraft();
    d = typeKey(d, "local_server", "local-FAKE-0013");
    d = typeKey(d, "post_processing", "pp-FAKE-0013");
    expect(testRequest(d)!.key).toBe("Untouched");
    d = clearKey(d, "transcription_api");
    expect(testRequest(d)!.key).toBe("Clear");
  });

  it("a key typed and emptied again is Untouched (never a blank Replace)", () => {
    let d = apiDraft();
    d = typeKey(d, "transcription_api", FAKE_KEY);
    d = resetKey(d, "transcription_api");
    expect(testRequest(d)!.key).toBe("Untouched");
  });

  it("none and builtin_local have no test: null", () => {
    const d = draftFromView(firstRun());
    expect(d.settings.engine).toBe("none");
    expect(testRequest(d)).toBeNull();
    d.settings.engine = "builtin_local";
    expect(testRequest(d)).toBeNull();
  });

  it("values are sent as typed: no trim and no validation in the UI", () => {
    const d = apiDraft();
    d.settings.api.base_url = "  not a url (fake)  ";
    d.settings.api.model = " whisper ";
    d.settings.timeouts.connect_s = 0;
    const request = testRequest(d)!;
    expect(request.base_url).toBe("  not a url (fake)  ");
    expect(request.model).toBe(" whisper ");
    expect(request.timeouts.connect_s).toBe(0);
  });

  it("the request is a copy: later edits of the draft do not change it, and building it changes nothing in the draft", () => {
    let d = apiDraft();
    d = typeKey(d, "transcription_api", FAKE_KEY);
    const before = structuredClone(d);
    const request = testRequest(d)!;
    expect(d).toEqual(before);
    d.settings.api.model = "edited-after-click";
    d.settings.timeouts.connect_s = 99;
    (d.keys.transcription_api as { Replace: string }).Replace = "changed";
    expect(request.model).toBe(wire.first_run_view.settings.api.model);
    expect(request.timeouts.connect_s).toBe(wire.first_run_view.settings.timeouts.connect_s);
    expect(request.key).toEqual({ Replace: FAKE_KEY });
  });
});

describe("applyTestErrors (T-013)", () => {
  const malformed = wire.test_connection_results.invalid.errors;

  it("core's invalid result highlights its field with error.<code> (the U2 path, errorsByField)", () => {
    const d = applyTestErrors(draftFromView(firstRun()), malformed);
    expect(d.errors).toEqual({ "engine.api.base_url": "error.url.malformed" });
    expect(d.errors).toEqual(errorsByField(malformed));
  });

  it("a field on another tab (timeouts.connect) is highlighted too; the first error of a field wins", () => {
    const d = applyTestErrors(draftFromView(firstRun()), [
      { field: "timeouts.connect", code: "timeout.range" },
      { field: "engine.api.model", code: "required" },
      { field: "timeouts.connect", code: "required" },
    ]);
    expect(d.errors).toEqual({ "timeouts.connect": "error.timeout.range", "engine.api.model": "error.required" });
  });

  it("replaces the previous highlights (choice (i)) and keeps the form error, settings, key edits and baseline", () => {
    let d = draftFromView(firstRun());
    d.settings.api.model = "whisper-edited";
    d = typeKey(d, "transcription_api", FAKE_KEY);
    d = applyOutcome(d, {
      Refused: {
        errors: [{ field: "history.size", code: "range" }],
        form_error: { kind: "write_failed", message: "settings.write_failed" },
      },
    } as SaveOutcome);
    const before = structuredClone(d);
    const next = applyTestErrors(d, malformed);
    expect(next.errors).toEqual({ "engine.api.base_url": "error.url.malformed" });
    expect(next.formError).toEqual(before.formError);
    expect(next.settings).toEqual(before.settings);
    expect(next.keys).toEqual(before.keys);
    expect(next.baseline).toEqual(before.baseline);
    expect(isDirty(next)).toBe(true);
    // The input draft is not changed in place.
    expect(d).toEqual(before);
  });

  it("no errors clears the highlights", () => {
    const d = applyTestErrors(applyTestErrors(draftFromView(firstRun()), malformed), []);
    expect(d.errors).toEqual({});
  });
});
