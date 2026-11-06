// T-050 red e2e, failure branch (decision #77): a page that fails to boot fails the test
// with the boot cause (failed request URL and errorText, status >= 500, crash), never as
// an unrelated assertion, and without retries.
//
// Contract for the boot fixture (implementation: e2e/support/boot.ts):
// - it exports `test` and `expect`; every spec that uses `page` or `context` gets the
//   boot check by importing `test` from "./support/boot" instead of "@playwright/test"
//   (types such as `Page` may still come from there);
// - it covers every page of the test's context: the `page` fixture's page and pages the
//   test opens with `context.newPage()` (T-050 review 1 #1);
// - while the test runs it records those pages' failed requests (URL + failure errorText),
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
import { join, sep } from "node:path";
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

  // T-050 review 1 #1: pages opened with context.newPage() are covered too.
  test("an aborted page chunk on a context.newPage() page is reported with its URL and errorText", async ({
    baseURL,
  }, info) => {
    const map = await innerRun(baseURL!, info.project.outputDir);
    const o = outcome(map, "context.newPage: aborted page chunk, unrelated assertion");
    expect(o.broken.length, "the scenario broke at least one page chunk").toBeGreaterThan(0);
    expectFailedWith(o, [...o.broken, "net::ERR_INTERNET_DISCONNECTED"]);
  });

  test("a page chunk answering 500 on a context.newPage() page fails the test with its URL and status even when its assertions pass", async ({
    baseURL,
  }, info) => {
    const map = await innerRun(baseURL!, info.project.outputDir);
    const o = outcome(map, "context.newPage: page chunk answers 500, assertions that pass anyway");
    expect(o.broken.length, "the scenario broke at least one page chunk").toBeGreaterThan(0);
    expectFailedWith(o, [...o.broken, /\b500\b/]);
  });

  test("a renderer crash of a context.newPage() page is reported as a crash", async ({
    baseURL,
  }, info) => {
    const map = await innerRun(baseURL!, info.project.outputDir);
    const o = outcome(map, "context.newPage: renderer crash, unrelated assertion");
    expectFailedWith(o, [/crash/i]);
  });
});

// Every spec that uses the `page` or the `context` fixture (pages opened with
// context.newPage(), T-050 review 1 #1) gets it from the boot fixture, so no page in the
// suite can fail to boot silently (25 goto sites in 8 specs at T-050 analysis time, plus
// 13 context.newPage() sites in tauri-mock.spec.ts).
test("every spec that uses page or context imports test from ./support/boot", () => {
  const dir = "e2e";
  const offenders: string[] = [];
  for (const file of readdirSync(dir).filter((f) => f.endsWith(".spec.ts"))) {
    const src = readFileSync(join(dir, file), "utf8");
    const usesPage = /\(\s*\{[^}]*\b(?:page|context)\b[^}]*\}/.test(src);
    if (!usesPage) continue;
    const fromBoot = /import\s*\{[^}]*\btest\b[^}]*\}\s*from\s*["']\.\/support\/boot["']/.test(src);
    const fromPlaywright =
      /import\s*\{[^}]*(?<![\w.])test\b(?!\s*as)[^}]*\}\s*from\s*["']@playwright\/test["']/.test(src);
    if (!fromBoot || fromPlaywright) offenders.push(file);
  }
  expect(offenders, "specs that use page or context without the boot fixture").toEqual([]);
});

// T-195 (invariant I1, decides_on: the navigation call "goto" on a page): no e2e verdict depends on
// how fast the page boots, so no spec navigates by itself and then waits on Playwright's 5 s
// expect default. Every navigation goes through the boot helper in e2e/support/boot.ts, whose
// wait is bounded by the test's own timeout (contract: e2e/boot-delay.spec.ts). So the token
// of a navigation call appears in no e2e file except e2e/support/boot.ts, with one exemption:
// the failure-expecting scenarios of e2e/fixtures/boot-failure/boot-failure.inner.ts, whose
// verdict is that the boot fails and which keep their short waits (Investigation 1). The inner
// run's healthy scenario is not exempt: it boots through the helper.
const NAVIGATION = /\.\s*goto\s*\(/;
const BOOT_HELPER = join("e2e", "support", "boot.ts");
const FAILURE_SCENARIOS = new Set([
  "aborted page chunk, unrelated assertion",
  "page chunk answers 500, unrelated assertion",
  "aborted page chunk, assertions that pass anyway",
  "renderer crash, unrelated assertion",
  "context.newPage: aborted page chunk, unrelated assertion",
  "context.newPage: page chunk answers 500, assertions that pass anyway",
  "context.newPage: renderer crash, unrelated assertion",
]);

// The parts of a source that are not an exempt failure scenario: for a fixture file, the text
// outside the exempt `test("<title>", ...)` blocks; any other file whole.
function nonExemptSource(file: string, src: string): string {
  if (!file.split(sep).includes("fixtures")) return src;
  return src
    .split(/^(?=test\()/m)
    .filter((block) => {
      const title = /^test\(\s*"([^"]+)"/.exec(block)?.[1];
      return title === undefined || !FAILURE_SCENARIOS.has(title);
    })
    .join("");
}

function navigationSites(root: string): { files: number; offenders: string[] } {
  const files = (readdirSync(root, { recursive: true }) as string[])
    .map((f) => join(root, f))
    .filter((f) => f.endsWith(".ts") && f !== BOOT_HELPER);
  const offenders = files.filter((f) => NAVIGATION.test(nonExemptSource(f, readFileSync(f, "utf8"))));
  return { files: files.length, offenders };
}

test("no e2e file navigates outside the boot helper (the failure scenarios of the inner run excepted)", () => {
  const { files, offenders } = navigationSites("e2e");
  expect(files, "the scan saw the specs, fixtures and support files").toBeGreaterThan(15);
  expect(offenders, "files that navigate without the boot helper in e2e/support/boot.ts").toEqual([]);
});

// Characterization of the scan (green today): spellings it must catch, and what it lets by.
test("the navigation scan catches every spelling of the call and exempts only the named failure scenarios", () => {
  // Built from parts: this file is scanned too and must not hold the token itself.
  const go = "go" + "to";
  for (const spelled of [
    `await page.${go}("/")`,
    `await page . ${go} ("/")`,
    `await page\n    .${go}(\`/settings\`)`,
    `await context.pages()[0].${go}("/")`,
  ]) {
    expect(NAVIGATION.test(spelled), spelled).toBe(true);
  }
  expect(NAVIGATION.test(`await page.${go}.bind(page)`)).toBe(false);
  const fixture = join("e2e", "fixtures", "x.inner.ts");
  const args = "{ pa" + "ge }";
  const healthy = `test("healthy boot passes", async (${args}) => {\n  await page.${go}("/");\n});\n`;
  const broken = `test("renderer crash, unrelated assertion", async (${args}) => {\n  await page.${go}("/");\n});\n`;
  expect(NAVIGATION.test(nonExemptSource(fixture, healthy)), "the healthy scenario is scanned").toBe(true);
  expect(NAVIGATION.test(nonExemptSource(fixture, broken)), "a named failure scenario is exempt").toBe(false);
  expect(NAVIGATION.test(nonExemptSource(join("e2e", "a.spec.ts"), broken)), "a spec is never exempt").toBe(true);
});
