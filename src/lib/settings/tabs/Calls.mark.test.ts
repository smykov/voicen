// T-232 red tests (decisions #306, #373): the Calls tab marks calls.hotkey from the saved
// view's `call_hotkey_unavailable` (draft.baseline), without a save.
// - true in the baseline -> aria-invalid and the `error.hotkey.unavailable` text on the field;
// - false (or absent) -> no mark;
// - the mark stays while the draft's call hotkey is edited (baseline, not draft value);
// - a draft error of its own on calls.hotkey wins (one message, its own);
// - only calls.hotkey is marked; a Saved view with false clears it (draftFromView).
// Rendered on the server side (svelte/server), no DOM needed.
import { render } from "svelte/server";
import { describe, expect, it } from "vitest";
import enCatalog from "../../../../i18n/en.json";
import wire from "../../../../e2e/fixtures/settings-wire.json";
import { applyOutcome, draftFromView, type Draft } from "../draft";
import type { SettingsView } from "../settingsApi";
import Calls from "./Calls.svelte";

const EN = enCatalog as Record<string, string>;
const UNAVAILABLE = EN["error.hotkey.unavailable"];
const EQUALS = EN["error.hotkey.equals_dictation"];

/** A view of core's first-run sample with the T-232 field set to `mark` (spread + override). */
function view(mark: boolean | undefined): SettingsView {
  const base = structuredClone(wire.first_run_view) as unknown as SettingsView;
  const out: Record<string, unknown> = { ...base };
  if (mark === undefined) delete out.call_hotkey_unavailable;
  else out.call_hotkey_unavailable = mark;
  return out as unknown as SettingsView;
}

function html(draft: Draft): string {
  const { body } = render(Calls, { props: { draft, warnings: {}, languages: ["en", "de"] } });
  return body;
}

/** The opening tag of the control with `data-field="<id>"`. */
function control(body: string, id: string): string {
  const tag = body.match(new RegExp(`<input[^>]*data-field="${id}"[^>]*>`));
  if (tag === null) throw new Error(`no control ${id} in the rendered tab`);
  return tag[0];
}

function count(body: string, text: string): number {
  return body.split(text).length - 1;
}

describe("the Calls tab marks calls.hotkey from the saved view's call_hotkey_unavailable (T-232)", () => {
  it("a baseline with call_hotkey_unavailable true marks calls.hotkey with the unavailable text, without a save", () => {
    const body = html(draftFromView(view(true)));
    expect(control(body, "calls.hotkey")).toContain('aria-invalid="true"');
    expect(count(body, UNAVAILABLE)).toBe(1);
  });

  it("a baseline with false has no mark and no unavailable text", () => {
    const body = html(draftFromView(view(false)));
    expect(control(body, "calls.hotkey")).not.toContain("aria-invalid");
    expect(count(body, UNAVAILABLE)).toBe(0);
  });

  it("a baseline without the field has no mark (no mark is invented)", () => {
    const body = html(draftFromView(view(undefined)));
    expect(control(body, "calls.hotkey")).not.toContain("aria-invalid");
    expect(count(body, UNAVAILABLE)).toBe(0);
  });

  it("the mark stays while the draft's call hotkey differs from the saved one (decision #373)", () => {
    const draft = draftFromView(view(true));
    draft.settings.calls.hotkey = "Ctrl+Alt+F10";
    expect(draft.settings.calls.hotkey).not.toBe(draft.baseline.settings.calls.hotkey);
    const body = html(draft);
    expect(control(body, "calls.hotkey")).toContain('aria-invalid="true"');
    expect(count(body, UNAVAILABLE)).toBe(1);
  });

  it("a draft error of its own on calls.hotkey wins: its text only, one message", () => {
    const draft = draftFromView(view(true));
    draft.errors = { "calls.hotkey": "error.hotkey.equals_dictation" };
    const body = html(draft);
    expect(control(body, "calls.hotkey")).toContain('aria-invalid="true"');
    expect(count(body, EQUALS)).toBe(1);
    expect(count(body, UNAVAILABLE)).toBe(0);
  });

  it("only calls.hotkey is marked: the other three controls are not", () => {
    const body = html(draftFromView(view(true)));
    for (const id of ["calls.reminder", "calls.primary_language", "calls.storage_root"]) {
      const tag = body.match(new RegExp(`<(?:input|select)[^>]*data-field="${id}"[^>]*>`));
      expect(tag, id).not.toBeNull();
      expect(tag![0], id).not.toContain("aria-invalid");
    }
  });

  it("a Saved view with false clears the mark; a Refused outcome keeps it", () => {
    const marked = draftFromView(view(true));
    const saved = applyOutcome(marked, { Saved: { view: view(false), warnings: [] } });
    const cleared = html(saved);
    expect(control(cleared, "calls.hotkey")).not.toContain("aria-invalid");
    expect(count(cleared, UNAVAILABLE)).toBe(0);

    const refused = applyOutcome(marked, {
      Refused: { errors: [{ field: "calls.storage_root", code: "storage.unwritable" }], form_error: null },
    } as never);
    const kept = html(refused);
    expect(control(kept, "calls.hotkey")).toContain('aria-invalid="true"');
    expect(count(kept, UNAVAILABLE)).toBe(1);
  });
});
