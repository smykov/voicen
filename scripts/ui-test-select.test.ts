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
// Round 2 (validation 1, survivors VA and VB): the router's e2e side, reached by spawning `main`
// against a stand-in e2e script. SEAM CONTRACT for the developer (scripts/ui-test-select.mjs):
//   env VOICEN_E2E_SCRIPT, when set and non-empty, is the path of the executable the router
//   spawns wherever it now spawns `scripts/e2e.sh` (listE2e and the e2e group's run, both);
//   unset or empty, it is scripts/e2e.sh. The stand-in is spawned directly (no shell), with the
//   same arguments and cwd, and the same env as today (BASE_URL placeholder only in the list run
//   and only inside the image).
//
// ui-unit-test.mjs (spawned here, refusals only: they exit before vitest starts):
//   a selector outside src/ and scripts/ (vitest's include set), or one that names no existing
//   file (a trailing :line is stripped before the check), exits 2; the e2e/ case says
//   `pnpm e2e` on stderr. The first failing selector decides; vitest is not started.
import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
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

// The exit code of a runner the scripts started, and the router's own count: `main` is spawned
// (T-195, invariant I4: the router exits 0 only if every runner it started exits 0; a selector
// the runner's list cannot count is refused, never taken as a match).
describe("exit codes of the spawned scripts", () => {
  const root = fileURLToPath(new URL("..", import.meta.url));
  const fixture = "scripts/ui-fixture-fail.test.ts";
  // The nested vitest must not believe it runs inside this one.
  const cleanEnv = (extra: Record<string, string>) => {
    const env: NodeJS.ProcessEnv = { ...process.env, ...extra };
    for (const k of Object.keys(env)) if (k.startsWith("VITEST") || k === "TEST") delete env[k];
    return env;
  };
  const spawn = (script: string, args: string[], extra: Record<string, string> = {}) =>
    spawnSync(process.execPath, [fileURLToPath(new URL(script, import.meta.url)), ...args], {
      cwd: root,
      encoding: "utf8",
      timeout: 120_000,
      env: cleanEnv(extra),
    });

  it("ui-unit-test.mjs exits with vitest's non-zero code when the selected test fails", () => {
    const r = spawn("./ui-unit-test.mjs", [fixture], { VOICEN_FIXTURE_FAIL: "1" });
    expect(r.stdout + r.stderr).toContain("fails on purpose");
    expect(r.status).not.toBe(0);
    expect(r.status).not.toBe(2);
  }, 100_000);

  it("ui-unit-test.mjs exits 0 when the same fixture runs without the switch (the fixture is the cause)", () => {
    expect(spawn("./ui-unit-test.mjs", [fixture]).status).toBe(0);
  }, 100_000);

  it("ui-test-select.mjs exits non-zero when the unit group it started fails", () => {
    const r = spawn("./ui-test-select.mjs", [fixture], { VOICEN_FIXTURE_FAIL: "1" });
    expect(r.stdout + r.stderr).toContain("fails on purpose");
    expect(r.status).not.toBe(0);
  }, 100_000);

  it("ui-test-select.mjs exits 0 when every selector matches and the group passes", () => {
    expect(spawn("./ui-test-select.mjs", [fixture]).status).toBe(0);
  }, 100_000);

  it("ui-test-select.mjs exits 1 naming an existing file that holds no test (the list counts 0, not a match)", () => {
    const r = spawn("./ui-test-select.mjs", ["scripts/ui-unit-test.mjs"]);
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("selectors that match no test");
    expect(r.stderr).toContain("scripts/ui-unit-test.mjs (unit)");
  }, 100_000);

  it("ui-test-select.mjs exits 1 naming a missing file, and runs nothing", () => {
    const r = spawn("./ui-test-select.mjs", [fixture, "src/nope.test.ts"], { VOICEN_FIXTURE_FAIL: "1" });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("src/nope.test.ts (unit)");
    expect(r.stdout + r.stderr).not.toContain("fails on purpose");
  }, 100_000);
});

// The router's e2e side. A stand-in e2e script (written per test run, spawned directly by the
// router through VOICEN_E2E_SCRIPT) answers `--list --reporter=json <selector>` with a canned
// Playwright report, and any other call as a run that exits with a chosen code. Every call is
// appended to a log, so a test sees what the router started (and what it did not).
describe("ui-test-select.mjs e2e side (stand-in e2e script, VOICEN_E2E_SCRIPT)", () => {
  const root = fileURLToPath(new URL("..", import.meta.url));
  const router = fileURLToPath(new URL("./ui-test-select.mjs", import.meta.url));
  const fixture = "scripts/ui-fixture-fail.test.ts";
  // Existing files under e2e/ (the router checks the file exists before it lists).
  const A = "e2e/fixtures/settings-wire.json";
  const B = "e2e/fixtures/calls-wire.json";
  const dir = mkdtempSync(join(tmpdir(), "ui-select-e2e-"));
  const stand = join(dir, "e2e-stand-in.cjs");
  const logFile = join(dir, "calls.jsonl");
  writeFileSync(
    stand,
    `#!/usr/bin/env node
const fs = require("node:fs");
const args = process.argv.slice(2);
const counts = JSON.parse(process.env.STAND_COUNTS || "{}");
fs.appendFileSync(process.env.STAND_LOG, JSON.stringify({ args, baseUrl: process.env.BASE_URL ?? null }) + "\\n");
if (args.includes("--list")) {
  const sel = args[args.length - 1];
  if (process.env.STAND_LIST === "garbage") { console.log("browserType.launch: boom {not json"); process.exit(1); }
  const n = counts[sel] ?? 0;
  const spec = (i) => ({ title: "t" + i, file: sel, line: i + 1, tests: [] });
  const top = n > 1 ? 1 : 0;
  const report = { config: {}, suites: [{ title: sel, specs: Array.from({ length: top }, (_, i) => spec(i)),
    suites: [{ title: "group", specs: Array.from({ length: n - top }, (_, i) => spec(i + top)) }] }] };
  console.log("wrapper noise before");
  console.log(JSON.stringify(report));
  console.log("wrapper noise after");
  process.exit(0);
}
process.exit(Number(process.env.STAND_RUN_EXIT ?? 0));
`,
  );
  chmodSync(stand, 0o755);
  process.on("exit", () => rmSync(dir, { recursive: true, force: true }));

  const go = (args: string[], extra: Record<string, string> = {}) => {
    writeFileSync(logFile, "");
    const env: NodeJS.ProcessEnv = {
      ...process.env,
      VOICEN_E2E_SCRIPT: stand,
      STAND_LOG: logFile,
      ...extra,
    };
    for (const k of Object.keys(env)) if (k.startsWith("VITEST") || k === "TEST") delete env[k];
    const r = spawnSync(process.execPath, [router, ...args], { cwd: root, encoding: "utf8", timeout: 120_000, env });
    const calls = readFileSync(logFile, "utf8")
      .split("\n")
      .filter(Boolean)
      .map((l) => JSON.parse(l) as { args: string[]; baseUrl: string | null });
    return {
      r,
      lists: calls.filter((c) => c.args.includes("--list")),
      runs: calls.filter((c) => !c.args.includes("--list")),
    };
  };

  it("an e2e selector the list counts (specs nested in suites) and a run that exits 0: router exits 0, e2e group run once with the selector", () => {
    const { r, lists, runs } = go([`./${A}:3`], { STAND_COUNTS: JSON.stringify({ [`${A}:3`]: 1 }) });
    expect(r.status).toBe(0);
    expect(lists.map((c) => c.args)).toEqual([["--list", "--reporter=json", `${A}:3`]]);
    expect(runs.map((c) => c.args)).toEqual([[`${A}:3`]]);
  }, 100_000);

  it("counts specs at every suite depth: 3 specs (1 top-level, 2 nested) match", () => {
    const { r, runs } = go([A], { STAND_COUNTS: JSON.stringify({ [A]: 3 }) });
    expect(r.status).toBe(0);
    expect(runs).toHaveLength(1);
  }, 100_000);

  it("an e2e selector the list counts as 0 exits 1 naming it and its runner, and the e2e group is not run", () => {
    const { r, runs } = go([`${A}:1`], { STAND_COUNTS: JSON.stringify({ [A]: 40 }) });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("selectors that match no test");
    expect(r.stderr).toContain(`${A}:1 (e2e)`);
    expect(runs).toEqual([]);
  }, 100_000);

  it("the mixed set [matched, unmatched] in the e2e group exits 1 naming only the unmatched one, nothing run (the count is per selector)", () => {
    const { r, runs } = go([`${A}:285`, `${A}:1`], { STAND_COUNTS: JSON.stringify({ [`${A}:285`]: 1 }) });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain(`${A}:1 (e2e)`);
    expect(r.stderr).not.toContain(`${A}:285`);
    expect(runs).toEqual([]);
  }, 100_000);

  it("a matched unit selector next to an unmatched e2e one exits 1 and runs nothing (no unit run either)", () => {
    const { r, runs } = go([fixture, `${B}:9`], { VOICEN_FIXTURE_FAIL: "1", STAND_COUNTS: "{}" });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain(`${B}:9 (e2e)`);
    expect(runs).toEqual([]);
    expect(r.stdout + r.stderr).not.toContain("fails on purpose");
  }, 100_000);

  it("a list run that fails and prints no JSON report is not a match: exit 1, e2e group not run", () => {
    const { r, runs } = go([A], { STAND_LIST: "garbage", STAND_COUNTS: JSON.stringify({ [A]: 5 }) });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain(`${A} (e2e)`);
    expect(runs).toEqual([]);
  }, 100_000);

  it("each selector is counted on its own: two e2e selectors make two list runs, then one run with both, './' stripped", () => {
    const { r, lists, runs } = go([`./${A}:2`, B], {
      STAND_COUNTS: JSON.stringify({ [`${A}:2`]: 1, [B]: 2 }),
    });
    expect(r.status).toBe(0);
    expect(lists.map((c) => c.args[c.args.length - 1])).toEqual([`${A}:2`, B]);
    expect(runs.map((c) => c.args)).toEqual([[`${A}:2`, B]]);
  }, 100_000);

  it("the e2e group's own non-zero exit is the router's exit code (only an e2e selector)", () => {
    const { r, runs } = go([A], { STAND_COUNTS: JSON.stringify({ [A]: 1 }), STAND_RUN_EXIT: "7" });
    expect(runs).toHaveLength(1);
    expect(r.status).toBe(7);
  }, 100_000);

  it("the unit group passes and the e2e group fails: the router exits with the e2e code, not 0", () => {
    const { r, runs } = go([fixture, A], { STAND_COUNTS: JSON.stringify({ [A]: 1 }), STAND_RUN_EXIT: "7" });
    expect(runs).toHaveLength(1);
    expect(r.status).toBe(7);
  }, 100_000);

  it("both groups fail: the first non-zero code (the unit group's, run first) wins, and the e2e group still runs", () => {
    const { r, runs } = go([fixture, A], {
      VOICEN_FIXTURE_FAIL: "1",
      STAND_COUNTS: JSON.stringify({ [A]: 1 }),
      STAND_RUN_EXIT: "7",
    });
    expect(r.stdout + r.stderr).toContain("fails on purpose");
    expect(runs).toHaveLength(1);
    expect(r.status).not.toBe(0);
    expect(r.status).not.toBe(7);
  }, 100_000);

  it("inside the image the list run gets the placeholder BASE_URL and the group run keeps the caller's environment", () => {
    const { r, lists, runs } = go([A], {
      VOICEN_E2E_DOCKER_INFO: fileURLToPath(new URL("../package.json", import.meta.url)),
      STAND_COUNTS: JSON.stringify({ [A]: 1 }),
      BASE_URL: "",
    });
    expect(r.status).toBe(0);
    expect(lists[0]?.baseUrl).toBe("http://localhost:9");
    expect(runs[0]?.baseUrl).not.toBe("http://localhost:9");
  }, 100_000);
});
