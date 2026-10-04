// T-045 red tests: the pure half of the local-model picker (spec 002 US1; decision #58).
//
// What they pin (T-045 analysis, invariants I2-I5):
// - `applyEvent`: a `local-model://progress` makes its row `downloading { received,
//   total }`, a `local-model://state` gives its row that state, an unknown id changes
//   nothing (I2: nothing else sets a row's state).
// - The list/event sequencer (I2): the rows shown are the newest issued
//   `local_models_list` response, overlaid in arrival order by every event received since
//   that request was issued. An older response is dropped, whenever it arrives.
// - `selectable` (I3): exactly the downloaded rows, in catalog order.
// - `downloadBlocked` (I4): true while any row is downloading or a download invoke is
//   pending.
// - `formatSize` (I5, OQ-08 default): binary units as Windows Explorer shows them
//   (1 kB = 1024 B, 1 MB = 1 048 576 B), labelled in the UI language: kB below 1 MB,
//   whole MB below 1 GB, one decimal for GB; rounded to the nearest unit. Whitespace is
//   compared normalized (Intl puts a no-break space before a ru unit).
// - `reasonArgs`: `needed` is a byte count shown through `formatSize`; other params pass
//   unchanged.
// - `asFailureReason`: a contract refusal (string `code` and `messageKey`) is kept; any
//   other rejection is not a FailureReason (the caller shows error.ipc_unavailable).
//
// Data: core's own wire (e2e/fixtures/local-models-wire.json, pinned by
// e2e_local_models_wire_fixture_matches_core).
import { describe, expect, it } from "vitest";
import wire from "../../../e2e/fixtures/local-models-wire.json";
import {
  applyEvent,
  downloadBlocked,
  emptyModels,
  eventReceived,
  formatSize,
  listReceived,
  listRequested,
  reasonArgs,
  selectable,
  type LocalModelEvent,
  type ModelsState,
} from "./models";
import {
  asFailureReason,
  type FailureReason,
  type LocalModelProgress,
  type LocalModelStateChange,
  type LocalModelView,
  type ModelId,
  type ModelState,
} from "./localModelsApi";

// ---- Fixture helpers ------------------------------------------------------------------

type ReasonCode = keyof typeof wire.reasons;

function firstRun(): LocalModelView[] {
  return structuredClone(wire.list_first_run) as unknown as LocalModelView[];
}

function reason(code: ReasonCode): FailureReason {
  return structuredClone(wire.reasons[code]) as unknown as FailureReason;
}

function failed(code: ReasonCode): ModelState {
  return { kind: "failed", reason: reason(code) };
}

const DOWNLOADED: ModelState = { kind: "downloaded" };
const NOT_DOWNLOADED: ModelState = { kind: "not_downloaded" };

/** Core's first-run list with the given rows' states replaced. */
function listWith(states: Partial<Record<string, ModelState>>): LocalModelView[] {
  return firstRun().map((row) => (states[row.id] ? { ...row, state: structuredClone(states[row.id]!) } : row));
}

function stateOf(rows: readonly LocalModelView[] | null, id: string): ModelState | undefined {
  return rows?.find((row) => row.id === id)?.state;
}

function sizeOf(id: string): number {
  const row = firstRun().find((model) => model.id === id);
  if (row === undefined) throw new Error(`no model ${id} in the fixture`);
  return row.sizeBytes;
}

function progress(id: string, received: number, total = sizeOf(id)): LocalModelEvent {
  return { event: "progress", payload: { id: id as ModelId, received, total } };
}

function stateEvent(id: string, state: ModelState): LocalModelEvent {
  return { event: "state", payload: { id: id as ModelId, state } };
}

/** Normalizes every whitespace character (Intl's U+00A0 / U+202F) to a plain space. */
function norm(text: string): string {
  return text.replace(/\s/gu, " ");
}

// ---- applyEvent ------------------------------------------------------------------------

describe("applyEvent", () => {
  it("a progress event makes its row downloading { received, total } and changes no other row", () => {
    // Core's own progress payload: { id: base, received: 32800, total: 65600 }.
    const rows = applyEvent(firstRun(), { event: "progress", payload: wire.events.progress as LocalModelProgress });
    expect(stateOf(rows, "base")).toEqual({ kind: "downloading", received: 32800, total: 65600 });
    const others = (list: readonly LocalModelView[]) => list.filter((row) => row.id !== "base");
    expect(others(rows)).toEqual(others(firstRun()));
  });

  it("a state event gives its row that state (core's state payload: base downloaded)", () => {
    const rows = applyEvent(firstRun(), { event: "state", payload: wire.events.state as unknown as LocalModelStateChange });
    expect(stateOf(rows, "base")).toEqual(DOWNLOADED);
    expect(rows.map((row) => row.id)).toEqual(firstRun().map((row) => row.id));
  });

  it("a failed state carries its reason; a later progress (Retry) replaces it with downloading, reason gone", () => {
    const failedRows = applyEvent(firstRun(), stateEvent("base", failed("download_interrupted")));
    expect(stateOf(failedRows, "base")).toEqual(failed("download_interrupted"));

    const retried = applyEvent(failedRows, progress("base", 4096));
    expect(stateOf(retried, "base")).toEqual({ kind: "downloading", received: 4096, total: sizeOf("base") });
  });

  it("a cancel's state not_downloaded leaves no reason and no progress on the row", () => {
    const downloading = applyEvent(firstRun(), progress("small", 1_000_000));
    const cancelled = applyEvent(downloading, stateEvent("small", NOT_DOWNLOADED));
    expect(stateOf(cancelled, "small")).toEqual(NOT_DOWNLOADED);
  });

  it("an event for an id that is not in the list changes nothing", () => {
    expect(applyEvent(firstRun(), progress("ggml-fake-unknown", 10, 20))).toEqual(firstRun());
    expect(applyEvent(firstRun(), stateEvent("ggml-fake-unknown", DOWNLOADED))).toEqual(firstRun());
  });
});

// ---- List / event ordering (I2) -----------------------------------------------------

/**
 * Drives the sequencer as the component does: one models state, replaced by each call.
 * `request()` issues a list request and returns its handle.
 */
function sequencer() {
  let state: ModelsState = emptyModels();
  return {
    rows: () => state.rows,
    request: (): number => {
      const issued = listRequested(state);
      state = issued.state;
      return issued.request;
    },
    respond: (request: number, rows: LocalModelView[]) => {
      state = listReceived(state, request, rows);
    },
    event: (event: LocalModelEvent) => {
      state = eventReceived(state, event);
    },
  };
}

describe("list and event ordering (I2)", () => {
  it("no rows before the first list response, whatever events arrive", () => {
    const s = sequencer();
    expect(s.rows()).toBeNull();
    s.request();
    expect(s.rows()).toBeNull();
    s.event(progress("base", 100));
    expect(s.rows()).toBeNull();
  });

  it("the events received since the request was issued are replayed over its response, in arrival order", () => {
    const s = sequencer();
    const req = s.request();
    // A download (from before the window opened, OQ-09) fails, is retried and progresses
    // while the list is in flight; the response is a snapshot from before all of it.
    s.event(progress("base", 1000));
    s.event(stateEvent("base", failed("checksum_mismatch")));
    s.event(progress("base", 2000));
    s.event(stateEvent("tiny", DOWNLOADED));
    s.event(stateEvent("tiny", NOT_DOWNLOADED));
    s.respond(req, firstRun());

    expect(stateOf(s.rows(), "base")).toEqual({ kind: "downloading", received: 2000, total: sizeOf("base") });
    expect(stateOf(s.rows(), "tiny")).toEqual(NOT_DOWNLOADED);
    expect(s.rows()?.map((row) => row.id)).toEqual(firstRun().map((row) => row.id));
  });

  it("a fast failure that ends before the re-list is issued is not replayed over the newer response", () => {
    // Events before a request are older than its response: the response wins.
    const s = sequencer();
    const first = s.request();
    s.respond(first, firstRun());
    s.event(stateEvent("base", failed("download_interrupted")));
    expect(stateOf(s.rows(), "base")).toEqual(failed("download_interrupted"));

    const second = s.request();
    s.event(progress("small", 512));
    // The newer snapshot already shows base downloading again (a retry started meanwhile).
    s.respond(second, listWith({ base: { kind: "downloading", received: 0, total: sizeOf("base") } }));

    expect(stateOf(s.rows(), "base")).toEqual({ kind: "downloading", received: 0, total: sizeOf("base") });
    expect(stateOf(s.rows(), "small")).toEqual({ kind: "downloading", received: 512, total: sizeOf("small") });
  });

  it("events arriving while a newer request is pending update the rows shown now", () => {
    const s = sequencer();
    s.respond(s.request(), firstRun());
    s.request();

    s.event(progress("base", 36987867));
    expect(stateOf(s.rows(), "base")).toEqual({ kind: "downloading", received: 36987867, total: sizeOf("base") });
  });

  it("an older list response arriving after the newer one is dropped", () => {
    const s = sequencer();
    const older = s.request();
    const newer = s.request();
    s.respond(newer, listWith({ base: DOWNLOADED }));
    s.respond(older, firstRun());

    expect(stateOf(s.rows(), "base")).toEqual(DOWNLOADED);
  });

  it("an older list response arriving while the newer one is pending is dropped too", () => {
    const s = sequencer();
    s.respond(s.request(), listWith({ tiny: DOWNLOADED }));
    const older = s.request();
    const newer = s.request();

    s.respond(older, listWith({ tiny: NOT_DOWNLOADED, base: DOWNLOADED }));
    expect(stateOf(s.rows(), "tiny")).toEqual(DOWNLOADED);
    expect(stateOf(s.rows(), "base")).toEqual(NOT_DOWNLOADED);

    s.event(stateEvent("small", DOWNLOADED));
    s.respond(newer, listWith({ base: DOWNLOADED }));
    expect(stateOf(s.rows(), "tiny")).toEqual(NOT_DOWNLOADED);
    expect(stateOf(s.rows(), "base")).toEqual(DOWNLOADED);
    expect(stateOf(s.rows(), "small")).toEqual(DOWNLOADED);
  });

  it("the replayed events are those since the newest request, not since an older one", () => {
    const s = sequencer();
    const older = s.request();
    s.event(stateEvent("base", DOWNLOADED));
    const newer = s.request();
    s.respond(older, firstRun());
    // The newer snapshot was taken after base finished and was then deleted (T-019).
    s.respond(newer, firstRun());

    expect(stateOf(s.rows(), "base")).toEqual(NOT_DOWNLOADED);
  });
});

// ---- selectable (I3) -----------------------------------------------------------------

describe("selectable", () => {
  it("offers no model on the first run (all not_downloaded)", () => {
    expect(selectable(firstRun())).toEqual([]);
  });

  it("offers exactly the downloaded rows, in catalog order", () => {
    const rows = listWith({
      tiny: DOWNLOADED,
      base: { kind: "downloading", received: 10, total: sizeOf("base") },
      small: failed("checksum_mismatch"),
      "large-v3-turbo-q5_0": DOWNLOADED,
    });
    expect(selectable(rows).map((row) => row.id)).toEqual(["tiny", "large-v3-turbo-q5_0"]);
  });
});

// ---- downloadBlocked (I4) ------------------------------------------------------------

describe("downloadBlocked", () => {
  it("is false with no row downloading and no download invoke pending", () => {
    expect(downloadBlocked(firstRun(), false)).toBe(false);
    const mixed = listWith({ tiny: DOWNLOADED, base: failed("download_interrupted") });
    expect(downloadBlocked(mixed, false)).toBe(false);
  });

  it("is true while any row is downloading", () => {
    const rows = listWith({ small: { kind: "downloading", received: 0, total: sizeOf("small") } });
    expect(downloadBlocked(rows, false)).toBe(true);
  });

  it("is true while a local_model_download invoke is pending, before any row is downloading", () => {
    expect(downloadBlocked(firstRun(), true)).toBe(true);
  });
});

// ---- formatSize (I5; OQ-08 default: binary, Explorer-style) ------------------------------

describe("formatSize", () => {
  // Core's catalog sizes (list_first_run) in binary MB, rounded to whole MB.
  const CATALOG: [string, string][] = [
    ["tiny", "74"],
    ["base", "141"],
    ["small", "465"],
    ["medium-q5_0", "514"],
    ["large-v3-turbo-q5_0", "547"],
  ];

  it("en: every catalog size in whole binary MB (base is 141 MB, not 148 MB)", () => {
    for (const [id, mb] of CATALOG) expect(norm(formatSize(sizeOf(id), "en")), id).toBe(`${mb} MB`);
  });

  it("ru: every catalog size in whole binary МБ", () => {
    for (const [id, mb] of CATALOG) expect(norm(formatSize(sizeOf(id), "ru")), id).toBe(`${mb} МБ`);
  });

  it("below 1 MB in whole kB (1 kB = 1024 B): core's needed 66256 is 65 kB / 65 кБ", () => {
    expect(norm(formatSize(66256, "en"))).toBe("65 kB");
    expect(norm(formatSize(66256, "ru"))).toBe("65 кБ");
    // 1 000 000 B is below 1 MB in binary units.
    expect(norm(formatSize(1_000_000, "en"))).toBe("977 kB");
    // A download's first progress (received 0).
    expect(norm(formatSize(0, "en"))).toBe("0 kB");
    expect(norm(formatSize(0, "ru"))).toBe("0 кБ");
  });

  it("exactly 1 MB is 1 MB", () => {
    expect(norm(formatSize(1_048_576, "en"))).toBe("1 MB");
    expect(norm(formatSize(1_048_576, "ru"))).toBe("1 МБ");
  });

  it("from 1 GB with one decimal, the decimal separator of the UI language", () => {
    expect(norm(formatSize(1_610_612_736, "en"))).toBe("1.5 GB");
    expect(norm(formatSize(1_610_612_736, "ru"))).toBe("1,5 ГБ");
  });
});

// ---- reasonArgs ----------------------------------------------------------------------

describe("reasonArgs", () => {
  function normArgs(args: Readonly<Record<string, string>>): Record<string, string> {
    return Object.fromEntries(Object.entries(args).map(([name, value]) => [name, norm(value)]));
  }

  it("formats needed (a byte count) through formatSize in the UI language", () => {
    expect(normArgs(reasonArgs(reason("not_enough_disk_space"), "en"))).toEqual({ needed: "65 kB" });
    expect(normArgs(reasonArgs(reason("not_enough_disk_space"), "ru"))).toEqual({ needed: "65 кБ" });

    const base = reason("not_enough_disk_space");
    base.params = { needed: String(sizeOf("base")) };
    expect(normArgs(reasonArgs(base, "en"))).toEqual({ needed: "141 MB" });
    expect(normArgs(reasonArgs(base, "ru"))).toEqual({ needed: "141 МБ" });
  });

  it("passes host and the HTTP code unchanged (only needed is a size)", () => {
    expect(reasonArgs(reason("source_unreachable"), "ru")).toEqual({ host: "huggingface.co" });
    expect(reasonArgs(reason("http_status"), "en")).toEqual({ code: "503" });
  });

  it("gives no args for a reason without params", () => {
    expect(reasonArgs(reason("checksum_mismatch"), "en")).toEqual({});
    expect(reasonArgs(reason("download_busy"), "ru")).toEqual({});
  });
});

// ---- asFailureReason -----------------------------------------------------------------

describe("asFailureReason", () => {
  it("keeps every contract refusal and failure reason of core's wire as it is", () => {
    for (const code of Object.keys(wire.reasons) as ReasonCode[]) {
      expect(asFailureReason(structuredClone(wire.reasons[code])), code).toEqual(reason(code));
    }
  });

  it("refuses any other rejection: a string, an Error, a partial or mistyped object, null", () => {
    const others: unknown[] = [
      "local_model_download failed (fake)",
      new Error("ipc down (fake)"),
      { code: "download_busy" },
      { messageKey: "download.busy" },
      { code: 503, messageKey: "download.http_status" },
      { code: "download_busy", messageKey: null },
      null,
      undefined,
    ];
    for (const other of others) expect(asFailureReason(other), String(other)).toBeNull();
  });
});
