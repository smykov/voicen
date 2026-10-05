// T-050 red e2e, failure branch (decision #77): a page that fails to boot fails the test
// with the boot cause (failed request URL and errorText, status >= 500, crash), never as
// an unrelated assertion, and without retries.
//
// Contract for the boot fixture (implementation: e2e/support/boot.ts):
// - it exports `test` and `expect`; its `test` overrides the `page` fixture, so every
//   spec that uses `page` gets the boot check by importing `test` from "./support/boot"
//   instead of "@playwright/test" (types such as `Page` may still come from there);
// - while the test runs it records the page's failed requests (URL + failure errorText),
//   responses with status >= 500 (URL + status) and a renderer crash;
// - a test during which any of those happened fails, even when its own assertions passed,
//   and its error message names each of them; a healthy boot adds nothing.
//
// How this is checked: the scenarios in e2e/fixtures/boot-failure/boot-failure.inner.ts
// are ordinary specs on the boot fixture. This test runs them in a child Playwright run
// (e2e/fixtures/boot-failure/playwright.config.ts) against this run's own preview
// (BASE_URL = this run's baseURL) and reads the JSON report: the reported failure must
// carry the cause, not only the unrelated assertion the scenario ends with.
import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, readdirSync, existsSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";

interface JsonResult {
  status: string;
  errors?: { message?: string }[];
  error?: { message?: string };
  annotations?: { type: string; description?: string }[];
}
interface JsonTest {
  annotations?: { type: string; description?: string }[];
  results: JsonResult[];
}
interface JsonSpec {
  title: string;
  tests: JsonTest[];
}
interface JsonSuite {
  specs?: JsonSpec[];
  suites?: JsonSuite[];
}
interface JsonReport {
  suites?: JsonSuite[];
  errors?: { message?: string }[];
}

interface Outcome {
  status: string;
  message: string;
  broken: string[]; // URLs the scenario broke on purpose
}

// eslint-disable-next-line no-control-regex
const ANSI = /\u001b\[[0-9;]*m/g;

// Playwright appends the source around the failing line ("> 60 |   ...", "     |  ^") and
// stack lines; those echo the scenario's own code (e.g. "chrome://crash"), so they are
// dropped: only what the run reported at runtime may satisfy the checks below.
function withoutCodeFrames(message: string): string {
  return message
    .split("\n")
    .filter((line) => !/^\s*>?\s*\d*\s*\|/.test(line) && !/^\s*at\s/.test(line))
    .join("\n");
}

function collect(suites: JsonSuite[] | undefined, out: Map<string, Outcome>): void {
  for (const suite of suites ?? []) {
    for (const spec of suite.specs ?? []) {
      const t = spec.tests[0];
      const r = t?.results[0];
      if (!t || !r) continue;
      const messages = [...(r.errors ?? []).map((e) => e.message ?? ""), r.error?.message ?? ""];
      const annotations = [...(t.annotations ?? []), ...(r.annotations ?? [])];
      out.set(spec.title, {
        status: r.status,
        message: withoutCodeFrames(messages.join("\n").replace(ANSI, "")),
        broken: [
          ...new Set(
            annotations
              .filter((a) => a.type === "broken-request" && a.description)
              .map((a) => a.description as string),
          ),
        ],
      });
    }
    collect(suite.suites, out);
  }
}

let outcomes: Map<string, Outcome> | undefined;
let runLog = "";

// One child run per Playwright run: the tests below share its report. A failed test
// restarts its worker, so the report is cached in the run's output dir (cleaned by
// Playwright at the start of every run), keyed by the runner process all workers share.
async function innerRun(baseURL: string, outputDir: string): Promise<Map<string, Outcome>> {
  if (outcomes) return outcomes;
  const outDir = join(outputDir, `boot-failure-inner-${process.ppid}`);
  mkdirSync(outDir, { recursive: true });
  const report = join(outDir, "report.json");
  const cached = existsSync(report);
  const run = cached ? undefined : spawnSync(
    "pnpm",
    ["exec", "playwright", "test", "-c", "e2e/fixtures/boot-failure/playwright.config.ts"],
    {
      cwd: process.cwd(),
      encoding: "utf8",
      timeout: 240_000,
      env: {
        ...process.env,
        BASE_URL: baseURL,
        VOICEN_BOOT_INNER_REPORT: report,
        VOICEN_BOOT_INNER_OUT: join(outDir, "results"),
        CI: "",
      },
    },
  );
  runLog = run
    ? `exit ${run.status}\n${run.stdout ?? ""}\n${run.stderr ?? ""}`.replace(ANSI, "")
    : `(report of an earlier worker: ${report})`;
  const parsed: JsonReport = existsSync(report) ? JSON.parse(readFileSync(report, "utf8")) : {};
  const map = new Map<string, Outcome>();
  collect(parsed.suites, map);
  if (map.size === 0) {
    const loadErrors = (parsed.errors ?? []).map((e) => e.message ?? "").join("\n");
    runLog += `\nreport errors:\n${loadErrors.replace(ANSI, "")}`;
  }
  outcomes = map;
  return map;
}

function outcome(map: Map<string, Outcome>, title: string): Outcome {
  const o = map.get(title);
  expect(o, `inner scenario "${title}" ran (inner run log):\n${runLog}`).toBeDefined();
  return o!;
}

function expectFailedWith(o: Outcome, parts: (string | RegExp)[]): void {
  expect(["failed", "timedOut"], `the scenario fails; message:\n${o.message}`).toContain(
    o.status,
  );
  for (const p of parts) {
    if (typeof p === "string") expect(o.message, "the failure names the boot cause").toContain(p);
    else expect(o.message, "the failure names the boot cause").toMatch(p);
  }
}

test.describe("boot fixture reports the boot cause", () => {
  test.setTimeout(300_000);

  test("a healthy boot is not failed by the boot fixture", async ({ baseURL }, info) => {
    const map = await innerRun(baseURL!, info.project.outputDir);
    const o = outcome(map, "healthy boot passes");
    expect(o.status, o.message).toBe("passed");
  });

  test("an aborted page chunk is reported with its URL and errorText, not as the unrelated assertion", async ({
    baseURL,
  }, info) => {
    const map = await innerRun(baseURL!, info.project.outputDir);
    const o = outcome(map, "aborted page chunk, unrelated assertion");
    expect(o.broken.length, "the scenario broke at least one page chunk").toBeGreaterThan(0);
    expectFailedWith(o, [...o.broken, "net::ERR_INTERNET_DISCONNECTED"]);
  });

  test("a page chunk answering 500 is reported with its URL and status", async ({
    baseURL,
  }, info) => {
    const map = await innerRun(baseURL!, info.project.outputDir);
    const o = outcome(map, "page chunk answers 500, unrelated assertion");
    expect(o.broken.length, "the scenario broke at least one page chunk").toBeGreaterThan(0);
    expectFailedWith(o, [...o.broken, /\b500\b/]);
  });

  test("a boot failure fails the test even when its own assertions pass", async ({
    baseURL,
  }, info) => {
    const map = await innerRun(baseURL!, info.project.outputDir);
    const o = outcome(map, "aborted page chunk, assertions that pass anyway");
    expect(o.broken.length, "the scenario broke at least one page chunk").toBeGreaterThan(0);
    expectFailedWith(o, [...o.broken, "net::ERR_INTERNET_DISCONNECTED"]);
  });

  test("a renderer crash is reported as a crash, not as the unrelated assertion", async ({
    baseURL,
  }, info) => {
    const map = await innerRun(baseURL!, info.project.outputDir);
    const o = outcome(map, "renderer crash, unrelated assertion");
    expectFailedWith(o, [/crash/i]);
  });
});

// Every spec that uses the `page` fixture gets it from the boot fixture, so no page in the
// suite can fail to boot silently (25 goto sites in 8 specs at T-050 analysis time).
test("every spec that uses page imports test from ./support/boot", () => {
  const dir = "e2e";
  const offenders: string[] = [];
  for (const file of readdirSync(dir).filter((f) => f.endsWith(".spec.ts"))) {
    const src = readFileSync(join(dir, file), "utf8");
    const usesPage = /\(\s*\{[^}]*\bpage\b[^}]*\}/.test(src);
    if (!usesPage) continue;
    const fromBoot = /import\s*\{[^}]*\btest\b[^}]*\}\s*from\s*["']\.\/support\/boot["']/.test(src);
    const fromPlaywright =
      /import\s*\{[^}]*(?<![\w.])test\b(?!\s*as)[^}]*\}\s*from\s*["']@playwright\/test["']/.test(src);
    if (!fromBoot || fromPlaywright) offenders.push(file);
  }
  expect(offenders, "specs that use page without the boot fixture").toEqual([]);
});
