// T-195 red tests, invariant I4 extended by decision #246 (docs/tasks/T-195.md): the selector
// router scripts/ui-test-select.mjs and the unit runner scripts/ui-unit-test.mjs.
//
// Needs vitest's include widened by `scripts/*.test.ts` (vitest.config.ts, the developer's
// change); until then `scripts/tw-test ui scripts/ui-test-select.test.ts` finds no test file.
//
// API the developer implements (scripts/ui-test-select.mjs, typed by JSDoc: checkJs is on):
//
//   export function route(selectors, list)
//     selectors: string[]             the arguments after the script name, as given
//     list: (runner, selector) => number
//                                     injected lister: how many tests `selector` matches in
//                                     `runner` ("unit" = vitest list, "e2e" = playwright
//                                     --list). It also answers 0 for a file that does not
//                                     exist. route calls it once per selector, in order,
//                                     with the selector after a leading "./" is stripped.
//     returns { ok: true, unit: string[], e2e: string[] }
//                                     every selector in exactly one group, order kept,
//                                     "./" stripped; a selector starting with "e2e/" goes
//                                     to e2e, every other one to unit
//           | { ok: false, code: 1 | 2, message: string }
//                                     code 2 (usage): no selector at all, or a selector
//                                     starting with "-" (an option); list is never called.
//                                     code 1: some selector's count is 0; the message names
//                                     that selector and its runner ("unit" or "e2e").
//
// The count is per selector, never per group: Playwright and vitest run the union of their
// selectors and exit 0 when one of them matches, so a group total of 1 must not hide a
// selector with 0 (the mixed case below).
//
// ui-unit-test.mjs (spawned here, refusals only: they exit before vitest starts):
//   a selector outside src/ and scripts/ (vitest's include set), or one that names no existing
//   file (a trailing :line is stripped before the check), exits 2; the e2e/ case says
//   `pnpm e2e` on stderr. The first failing selector decides; vitest is not started.
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { route } from "./ui-test-select.mjs";

type Runner = "unit" | "e2e";

/** A lister over a fixed table; a selector not in the table matches nothing. */
function lister(table: Record<string, number>) {
  const asked: [Runner, string][] = [];
  const list = (runner: Runner, selector: string): number => {
    asked.push([runner, selector]);
    return table[`${runner}:${selector}`] ?? 0;
  };
  return { list, asked };
}

function refused(result: ReturnType<typeof route>) {
  if (result.ok) throw new Error(`expected a refusal, got groups ${JSON.stringify(result)}`);
  return result;
}

describe("route (the router's pure step)", () => {
  it("sends e2e/about.spec.ts:285 to the e2e group and not to vitest (the reproduced false PASS)", () => {
    const { list } = lister({ "e2e:e2e/about.spec.ts:285": 1 });
    const result = route(["e2e/about.spec.ts:285"], list);
    expect(result).toEqual({ ok: true, unit: [], e2e: ["e2e/about.spec.ts:285"] });
  });

  it("sends src/ and scripts/ selectors to the unit group, e2e/ ones to the e2e group, each once, order kept, './' stripped", () => {
    const { list, asked } = lister({
      "unit:src/lib/buildInfo.test.ts": 3,
      "e2e:e2e/overlay.spec.ts": 5,
      "unit:scripts/ui-test-select.test.ts": 2,
    });
    const result = route(
      ["./src/lib/buildInfo.test.ts", "./e2e/overlay.spec.ts", "scripts/ui-test-select.test.ts"],
      list,
    );
    expect(result).toEqual({
      ok: true,
      unit: ["src/lib/buildInfo.test.ts", "scripts/ui-test-select.test.ts"],
      e2e: ["e2e/overlay.spec.ts"],
    });
    expect(asked).toEqual([
      ["unit", "src/lib/buildInfo.test.ts"],
      ["e2e", "e2e/overlay.spec.ts"],
      ["unit", "scripts/ui-test-select.test.ts"],
    ]);
  });

  it("refuses a unit selector that matches no test (src/nope.test.ts), naming it and its runner", () => {
    const { list } = lister({});
    const r = refused(route(["src/nope.test.ts"], list));
    expect(r.code).toBe(1);
    expect(r.message).toContain("src/nope.test.ts");
    expect(r.message).toContain("unit");
  });

  it("refuses an e2e selector that matches no test (e2e/about.spec.ts:1, a line with no test), naming it and its runner", () => {
    const { list } = lister({ "e2e:e2e/about.spec.ts": 40 });
    const r = refused(route(["e2e/about.spec.ts:1"], list));
    expect(r.code).toBe(1);
    expect(r.message).toContain("e2e/about.spec.ts:1");
    expect(r.message).toContain("e2e");
  });

  it("refuses the mixed set [e2e/about.spec.ts:285, e2e/about.spec.ts:1]: the count is per selector, not per group", () => {
    const { list } = lister({ "e2e:e2e/about.spec.ts:285": 1 });
    const r = refused(route(["e2e/about.spec.ts:285", "e2e/about.spec.ts:1"], list));
    expect(r.code).toBe(1);
    expect(r.message).toContain("e2e/about.spec.ts:1");
    expect(r.message).not.toContain("e2e/about.spec.ts:285");
  });

  it("refuses a matched unit selector next to an unmatched one in the other group", () => {
    const { list } = lister({ "unit:src/lib/buildInfo.test.ts": 3 });
    const r = refused(route(["src/lib/buildInfo.test.ts", "e2e/missing.spec.ts"], list));
    expect(r.code).toBe(1);
    expect(r.message).toContain("e2e/missing.spec.ts");
  });

  it("exits with usage (2) for no selector, calling the lister never", () => {
    const { list, asked } = lister({});
    expect(refused(route([], list)).code).toBe(2);
    expect(asked).toEqual([]);
  });

  it("exits with usage (2) for an option selector (-t ...), also next to a good selector, calling the lister never", () => {
    const { list, asked } = lister({ "unit:src/lib/buildInfo.test.ts": 3 });
    expect(refused(route(["-t", "formats"], list)).code).toBe(2);
    expect(refused(route(["src/lib/buildInfo.test.ts", "-t", "formats"], list)).code).toBe(2);
    expect(refused(route(["--grep=x", "src/lib/buildInfo.test.ts"], list)).code).toBe(2);
    expect(asked).toEqual([]);
  });
});

describe("scripts/ui-unit-test.mjs refusals (the false PASS of `pnpm test e2e/about.spec.ts:285`)", () => {
  const script = fileURLToPath(new URL("./ui-unit-test.mjs", import.meta.url));
  const root = fileURLToPath(new URL("..", import.meta.url));
  const run = (...args: string[]) =>
    spawnSync(process.execPath, [script, ...args], { cwd: root, encoding: "utf8", timeout: 60_000 });

  it("an e2e/ selector with a line exits 2 and names `pnpm e2e` (vitest said 'Couldn't find file' and exited 0)", () => {
    const r = run("e2e/about.spec.ts:285");
    expect(r.status).toBe(2);
    expect(r.stderr).toContain("pnpm e2e");
    expect(r.stdout).not.toContain("RUN");
  });

  it("a selector outside src/ and scripts/ exits 2", () => {
    expect(run("README.md").status).toBe(2);
  });

  it("a src/ selector that names no existing file exits 2, with and without a line", () => {
    expect(run("src/nope.test.ts").status).toBe(2);
    expect(run("src/nope.test.ts:3").status).toBe(2);
  });

  it("one bad selector next to a good one refuses the whole call before vitest starts", () => {
    const r = run("src/lib/buildInfo.test.ts", "e2e/about.spec.ts:285");
    expect(r.status).toBe(2);
    expect(r.stdout).not.toContain("RUN");
  });
});
